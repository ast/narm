//! Channel decoding for the KG-Q332/Q336.
//!
//! See `docs/kgq336-codeplug.md` in the repo root for the full
//! codeplug reference (CPS UI mappings, region map, byte
//! layouts, refined plan).
//!
//! ## Layout (recovered 50 000-byte image)
//!
//! - **Startup message** at `0x0084..0x0098` (20 bytes ASCII).
//! - **VFO state** at `0x00B0..0x0140` (8 × 16 bytes — A/B
//!   VFO across six bands, see [`VfoEntry`]).
//! - **Channel data array** at `0x0140..0x3FB0` (999 × 16 B,
//!   indexed by 1-based channel number).
//! - **Channel name array** at `0x3FBC..0x6EA0` (999 × 12 B,
//!   parallel to the data array).
//! - **FM broadcast memories** at `0x73E0..0x7408` (20 × `u16`
//!   LE × 100 kHz).
//!
//! Earlier versions of this module modelled channels as
//! independent "categories" (Riks, Jakt, SRBR, PMR, 69M) each
//! with its own data and name base offsets. The CPS Scan Group
//! tab revealed that's wrong: there is one flat 999-channel
//! array, and the "categories" are just user-defined channel-
//! number ranges.
//!
//! ## On-disk channel record (16 bytes)
//!
//! ```text
//!   bytes 0..4   rx_freq:        u32 LE × 10 Hz
//!   bytes 4..8   tx_freq:        u32 LE × 10 Hz (0 → simplex;
//!                                                 absolute for repeaters
//!                                                 — shift sign comes from tx-rx)
//!   bytes 8..10  tone_rx_raw:    u16 LE — see "Tone slot encoding"
//!   bytes 10..12 tone_tx_raw:    u16 LE — same encoding as RX
//!   byte 12      power_am_scramble: u8 — bits 0..1 = power
//!                                         (0=low, 1=mid, 2=high, 3=ultrahigh);
//!                                         bits 2..3 = AM mode
//!                                         (0=OFF, 1=AM Rx, 2=AM Rx&Tx, 3=unused);
//!                                         bits 4..7 = scramble level
//!                                         (0=off, 1..8 = group)
//!   byte 13      flags1:         u8    — bit 0 = wide bandwidth,
//!                                         bits 1..2 = mute mode
//!                                         (0=QT, 1=QT+DTMF, 2=QT*DTMF, 3=unused),
//!                                         bit 3 = compand,
//!                                         bit 5 = scan add;
//!                                         bits 4/6/7 unknown
//!   byte 14      call_group:     u8    — 1-based index into the
//!                                         Call Settings table
//!                                         (DTMF / 5-tone targets)
//!   byte 15      ???:            u8    — unknown (almost always 0
//!                                         on real channels; 0x05
//!                                         on the eight VFO entries.
//!                                         Likely packs Mute Mode +
//!                                         AM — TBD)
//! ```
//!
//! ## Tone slot encoding
//!
//! Both `tone_rx_raw` and `tone_tx_raw` use the same 16-bit
//! layout:
//!
//! | high nibble | meaning              |
//! |-------------|----------------------|
//! | `0x0`       | no tone              |
//! | `0x4`       | DCS normal polarity  |
//! | `0x6`       | DCS inverted polarity|
//! | `0x8`       | CTCSS                |
//!
//! Low 12 bits = value:
//! - CTCSS: `freq × 10` deci-Hz (88.5 Hz → `0x375`, 100.0 Hz → `0x3E8`).
//! - DCS: decimal of the octal display code (`023` oct → 19 dec,
//!   `754` oct → 492 dec).
//!
//! narm's [`Mode::Fm`] has one `dcs_code` slot, so if both TX
//! and RX carry DCS we surface the TX value (matches the UV-K5
//! decoder's policy). DCS polarity is currently lost in decode
//! — a `dcs_polarity` field on [`Mode::Fm`] would round-trip it.

use zerocopy::byteorder::little_endian::{U16, U32};
use zerocopy::{FromBytes, Immutable, KnownLayout};

use crate::channel::{Bandwidth, Channel, Mode, Power};

use super::error::KgQ336Error;

#[repr(C)]
#[derive(FromBytes, Immutable, KnownLayout, Debug, Clone, Copy)]
struct ChannelRecord {
    rx_freq: U32,
    tx_freq: U32,
    /// RX tone slot. Same layout as `tone_tx_raw`.
    tone_rx_raw: U16,
    /// TX tone slot. See module docstring for the encoding.
    tone_tx_raw: U16,
    /// Packed: bits 0..1 = power index (0=low, 1=mid, 2=high,
    /// 3=ultrahigh); bits 2..3 = AM mode (0=OFF, 1=AM Rx,
    /// 2=AM Rx&Tx, 3=unused); bits 4..7 = scramble level
    /// (0=off, 1..8 = scramble group).
    power_am_scramble: u8,
    /// Bit 0 = wide bandwidth (clear = narrow).
    /// Bits 1..2 = mute mode (0=QT, 1=QT+DTMF, 2=QT*DTMF;
    /// 3 unused).
    /// Bit 3 = compand (1 = on).
    /// Bit 5 = scan add (1 = in scan list).
    /// Bits 4, 6, 7: unknown.
    flags1: u8,
    /// Call Group — 1-based index into the radio's Call
    /// Settings table (DTMF / 5-tone targets, see image 7 in
    /// `docs/kgq336-codeplug.md`). Confirmed from the Channel
    /// Information tab: CH-001 has Call Group 1, SRBR Kan 01
    /// has 4, PMR Kan 01 has 5.
    call_group: u8,
    /// Unknown — `0x05` for the eight VFO-state entries at
    /// 0x00B0..0x0140, `0x00` for normal channels. Not used
    /// by [`decode_channels`] (VFO entries get decoded
    /// separately).
    _byte15: u8,
}

const RECORD_SIZE: usize = 16;
const NAME_SIZE: usize = 12;

/// Channel data + name arrays — flat, parallel, 1-based
/// channel numbering. CH-N's data lives at
/// `CHANNEL_DATA_BASE + (N-1) * RECORD_SIZE` and its name at
/// `CHANNEL_NAME_BASE + (N-1) * NAME_SIZE`.
const CHANNEL_DATA_BASE: usize = 0x0140;
const CHANNEL_NAME_BASE: usize = 0x3FBC;
const CHANNEL_COUNT: usize = 999;

/// VFO state — 8 × 16 B at `0x00B0`. Slots 0..5 are A-VFO
/// across six bands; slots 6..7 are B-VFO across two bands.
/// See [`VfoEntry`].
const VFO_BASE: usize = 0x00B0;
const VFO_COUNT: usize = 8;

/// FM broadcast presets — 20 × `u16` LE at `0x73E0`. Each
/// value is the frequency in 100 kHz units (so 760 = 76.0 MHz).
const FM_BROADCAST_BASE: usize = 0x73E0;
const FM_BROADCAST_COUNT: usize = 20;
/// FM-broadcast frequency unit: 100 kHz.
const FM_BROADCAST_UNIT_HZ: u64 = 100_000;

const MIN_PLAUSIBLE_HZ: u64 = 1_000_000;
const MAX_PLAUSIBLE_HZ: u64 = 1_000_000_000;

/// Mask isolating the high nibble of a tone slot — the mode
/// selector. `0x8` = CTCSS, `0x4` = DCS-N, `0x6` = DCS-I,
/// `0x0` = no tone (covered by the catch-all match arm).
const TONE_MODE_MASK: u16 = 0xF000;
const TONE_MODE_DCS_NORMAL: u16 = 0x4000;
const TONE_MODE_DCS_INVERTED: u16 = 0x6000;
const TONE_MODE_CTCSS: u16 = 0x8000;
/// Mask isolating the 12-bit value (CTCSS dHz or DCS decimal).
const TONE_VALUE_MASK: u16 = 0x0FFF;
/// Plausible CTCSS-tone range (`Hz`). The EIA-RS-220 standard
/// runs 67.0–254.1; anything outside means we're decoding
/// random bytes from a non-channel region.
const CTCSS_MIN_HZ: f32 = 60.0;
const CTCSS_MAX_HZ: f32 = 260.0;

/// Bit 0 of `flags1`: wide bandwidth (clear = narrow).
const FLAGS1_WIDE_BIT: u8 = 0x01;
/// Bit 3 of `flags1`: compand (1 = on).
const FLAGS1_COMPAND_BIT: u8 = 0x08;
/// Bit 5 of `flags1`: scan-add (clear = excluded from scan).
const FLAGS1_SCAN_BIT: u8 = 0x20;

/// Editable startup / boot screen string. 20 bytes ASCII at
/// offset 0x84, NUL-padded.
const STARTUP_MESSAGE_OFFSET: usize = 0x0084;
const STARTUP_MESSAGE_LEN: usize = 20;

impl ChannelRecord {
    fn rx_hz(&self) -> u64 {
        self.rx_freq.get() as u64 * 10
    }
    fn tx_hz(&self) -> u64 {
        self.tx_freq.get() as u64 * 10
    }

    fn is_plausible(&self) -> bool {
        let rx = self.rx_hz();
        if !(MIN_PLAUSIBLE_HZ..=MAX_PLAUSIBLE_HZ).contains(&rx) {
            return false;
        }
        let tx = self.tx_hz();
        // tx == 0 is the radio's "simplex" sentinel — accept it.
        // Otherwise tx must also land in a plausible band.
        // Repeater shift can be very large for split bands
        // (cross-band repeat etc.), so we no longer cap |tx-rx|;
        // walking the flat 999-slot array means we don't pick up
        // random bytes outside the channel region.
        tx == 0 || (MIN_PLAUSIBLE_HZ..=MAX_PLAUSIBLE_HZ).contains(&tx)
    }

    /// Signed shift (`tx - rx`); 0 for simplex (`tx_freq == 0`).
    fn shift_hz(&self) -> i64 {
        if self.tx_freq.get() == 0 {
            0
        } else {
            self.tx_hz() as i64 - self.rx_hz() as i64
        }
    }

    fn tone_tx(&self) -> ToneSlot {
        ToneSlot::decode(self.tone_tx_raw.get())
    }

    fn tone_rx(&self) -> ToneSlot {
        ToneSlot::decode(self.tone_rx_raw.get())
    }

    fn bandwidth(&self) -> Bandwidth {
        if self.flags1 & FLAGS1_WIDE_BIT != 0 {
            Bandwidth::Wide
        } else {
            Bandwidth::Narrow
        }
    }

    fn scan_add(&self) -> bool {
        self.flags1 & FLAGS1_SCAN_BIT != 0
    }

    fn compand(&self) -> bool {
        self.flags1 & FLAGS1_COMPAND_BIT != 0
    }

    /// Call-group index, 1-based. `1` is the default in most
    /// FB-Radio channels; SRBR uses 4, PMR uses 5.
    fn call_group(&self) -> u8 {
        self.call_group
    }
}

/// One tone slot (TX or RX), decoded from its raw `u16`.
#[derive(Debug, Clone, Copy, PartialEq)]
enum ToneSlot {
    None,
    Ctcss(f32),
    /// DCS code as the octal-digits-read-as-decimal value the
    /// CPS / users see (e.g. 23 for octal `023`, 754 for
    /// octal `754`). Polarity tracked alongside.
    Dcs {
        code: u16,
        inverted: bool,
    },
}

impl ToneSlot {
    fn decode(raw: u16) -> Self {
        let value = raw & TONE_VALUE_MASK;
        match raw & TONE_MODE_MASK {
            TONE_MODE_CTCSS => {
                let hz = value as f32 / 10.0;
                if (CTCSS_MIN_HZ..=CTCSS_MAX_HZ).contains(&hz) {
                    Self::Ctcss(hz)
                } else {
                    // Out-of-range = random bytes with the
                    // CTCSS flag bit set; not a real tone.
                    Self::None
                }
            }
            TONE_MODE_DCS_NORMAL => Self::Dcs {
                code: decimal_to_octal_display(value),
                inverted: false,
            },
            TONE_MODE_DCS_INVERTED => Self::Dcs {
                code: decimal_to_octal_display(value),
                inverted: true,
            },
            _ => Self::None,
        }
    }
}

/// Convert a stored decimal value back to its octal display
/// representation. The radio stores DCS `023` (octal) as
/// decimal `19`; the user-facing code is `23` — the octal
/// digits read as a decimal number, the same convention the
/// UV-K5 module uses for [`crate::channel::Mode::Fm::dcs_code`].
fn decimal_to_octal_display(stored: u16) -> u16 {
    let mut s = stored;
    let mut digits = [0u16; 6];
    let mut n = 0;
    while s > 0 {
        digits[n] = s % 8;
        s /= 8;
        n += 1;
    }
    let mut out = 0u16;
    for i in (0..n).rev() {
        out = out * 10 + digits[i];
    }
    out
}

impl ChannelRecord {
    fn power(&self) -> Power {
        // Power index lives in bits 0..1 only (4 values).
        // Bits 2..3 are the AM-mode field; bits 4..7 are
        // scramble. Bits beyond 0..1 must be masked out or
        // we'd map "AM Rx" / "AM Rx&Tx" to bogus power levels.
        // The radio has 4 power levels but narm's enum has 3,
        // so ultrahigh (3) maps to High (lossy).
        match self.power_am_scramble & 0b0000_0011 {
            0 => Power::Low,
            1 => Power::Mid,
            _ => Power::High,
        }
    }

    /// Scramble level 0..8. 0 = off, 1..8 = scramble group.
    /// Currently decoded but not surfaced in the [`Channel`]
    /// output — narm's `Mode::Fm` has no scramble field.
    fn scramble_level(&self) -> u8 {
        (self.power_am_scramble >> 4) & 0x0F
    }

    /// AM mode: `0` = OFF (FM), `1` = AM Rx, `2` = AM Rx&Tx,
    /// `3` = unused / reserved.
    fn am_mode(&self) -> u8 {
        (self.power_am_scramble >> 2) & 0b0000_0011
    }

    /// Mute Mode: `0` = QT (default), `1` = QT+DTMF,
    /// `2` = QT*DTMF, `3` = unused / reserved.
    fn mute_mode(&self) -> u8 {
        (self.flags1 >> 1) & 0b0000_0011
    }
}

pub struct DecodeReport {
    pub channels: Vec<Channel>,
    pub warnings: Vec<String>,
    /// Radio-level boot/startup message at offset 0x84 (20
    /// bytes ASCII NUL-padded). `None` means the slot is
    /// blank; otherwise the trimmed string. Surfaced in the
    /// report but not in the per-channel TOML.
    pub startup_message: Option<String>,
    /// VFO state — 8 entries (A-VFO across 6 bands + B-VFO
    /// across 2 bands). Order matches the CPS VFO Settings
    /// table (image 3 in `docs/kgq336-codeplug.md`).
    pub vfo_state: Vec<VfoEntry>,
    /// FM broadcast preset frequencies in Hz, 20 slots. The
    /// CPS default is 76.0 MHz for every slot.
    pub fm_broadcast: Vec<u64>,
}

/// One VFO state entry. Reuses the channel record's `rx_freq`
/// + `tx_freq` layout but most of the per-channel flag fields
///   either don't apply to VFOs or haven't been RE'd yet.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VfoEntry {
    pub rx_hz: u64,
    /// 0 for simplex / receive-only VFOs.
    pub tx_hz: u64,
}

pub fn decode_channels(raw: &[u8]) -> Result<DecodeReport, KgQ336Error> {
    let min = CHANNEL_DATA_BASE + CHANNEL_COUNT * RECORD_SIZE;
    if raw.len() < min {
        return Err(KgQ336Error::ShortImage {
            got: raw.len(),
            min,
        });
    }

    let mut channels = Vec::new();
    let mut warnings = Vec::new();

    // Walk the flat 1..=999 channel array. Each slot is 16
    // bytes of data plus a parallel 12-byte name slot. Empty
    // slots fail `is_plausible` and are skipped.
    for ch_no in 1..=CHANNEL_COUNT {
        let data_off = CHANNEL_DATA_BASE + (ch_no - 1) * RECORD_SIZE;
        let rec = ChannelRecord::ref_from_bytes(&raw[data_off..data_off + RECORD_SIZE])
            .expect("RECORD_SIZE matches ChannelRecord size_of");
        if !rec.is_plausible() {
            continue;
        }
        let name = decode_channel_name(raw, ch_no);
        note_unrepresented_fields(rec, &name, &mut warnings);
        channels.push(record_to_channel(rec, name));
    }

    let startup_message = decode_startup_message(raw);
    let vfo_state = decode_vfo_state(raw);
    let fm_broadcast = decode_fm_broadcast(raw);

    Ok(DecodeReport {
        channels,
        warnings,
        startup_message,
        vfo_state,
        fm_broadcast,
    })
}

/// Push a warning for any field that's set on the record but
/// dropped in the [`Channel`] output. Currently:
/// - non-zero scramble level (narm has no scramble field)
/// - compand on (narm has no compand field)
/// - DCS inverted polarity (narm has no polarity field)
/// - non-default call group (narm has no call-group field)
fn note_unrepresented_fields(rec: &ChannelRecord, name: &str, warnings: &mut Vec<String>) {
    let lvl = rec.scramble_level();
    if lvl != 0 {
        warnings.push(format!(
            "channel '{name}' has scramble level {lvl} (not represented in TOML)"
        ));
    }
    if rec.compand() {
        warnings.push(format!(
            "channel '{name}' has compand on (not represented in TOML)"
        ));
    }
    if matches!(rec.tone_tx(), ToneSlot::Dcs { inverted: true, .. })
        || matches!(rec.tone_rx(), ToneSlot::Dcs { inverted: true, .. })
    {
        warnings.push(format!(
            "channel '{name}' uses DCS inverted polarity (polarity dropped in TOML)"
        ));
    }
    let cg = rec.call_group();
    if cg != 1 {
        warnings.push(format!(
            "channel '{name}' uses Call Group {cg} (not represented in TOML)"
        ));
    }
    match rec.mute_mode() {
        0 => {} // QT — default
        1 => warnings.push(format!(
            "channel '{name}' uses Mute Mode QT+DTMF (not represented in TOML)"
        )),
        2 => warnings.push(format!(
            "channel '{name}' uses Mute Mode QT*DTMF (not represented in TOML)"
        )),
        n => warnings.push(format!(
            "channel '{name}' has unknown Mute Mode bits 0b{n:02b}"
        )),
    }
    // Mode::Am is emitted for both AM Rx (1) and AM Rx&Tx
    // (2). The Rx&Tx variant is the radio's intent to also
    // transmit AM; warn about that since narm's Mode::Am
    // doesn't distinguish.
    if rec.am_mode() == 2 {
        warnings.push(format!(
            "channel '{name}' uses AM Rx&Tx; emitted as Mode::Am (TX-side AM dropped)"
        ));
    }
    if rec.am_mode() == 3 {
        warnings.push(format!("channel '{name}' has unknown AM-mode bits 0b11"));
    }
}

fn decode_startup_message(raw: &[u8]) -> Option<String> {
    let end = STARTUP_MESSAGE_OFFSET + STARTUP_MESSAGE_LEN;
    if raw.len() < end {
        return None;
    }
    let s: String = raw[STARTUP_MESSAGE_OFFSET..end]
        .iter()
        .take_while(|&&b| b != 0 && b != 0xFF)
        .map(|&b| b as char)
        .collect();
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn record_to_channel(rec: &ChannelRecord, name: String) -> Channel {
    let tone_tx = rec.tone_tx();
    let tone_rx = rec.tone_rx();
    let tone_tx_hz = match tone_tx {
        ToneSlot::Ctcss(hz) => Some(hz),
        _ => None,
    };
    let tone_rx_hz = match tone_rx {
        ToneSlot::Ctcss(hz) => Some(hz),
        _ => None,
    };
    // narm has a single dcs_code field, so prefer TX over RX
    // (matching the UV-K5 decoder's policy). Polarity is
    // currently dropped — narm's Mode::Fm has no polarity slot.
    let dcs_code = match (tone_tx, tone_rx) {
        (ToneSlot::Dcs { code, .. }, _) => Some(code),
        (_, ToneSlot::Dcs { code, .. }) => Some(code),
        _ => None,
    };
    let bandwidth = rec.bandwidth();
    // AM mode 0 = FM (default); 1 = AM Rx; 2 = AM Rx&Tx.
    // narm's Mode::Am has the same fields as Mode::Fm; the
    // "Rx-only vs Rx&Tx" distinction is dropped here and
    // warned about in `note_unrepresented_fields`.
    let mode = if rec.am_mode() == 0 {
        Mode::Fm {
            bandwidth,
            tone_tx_hz,
            tone_rx_hz,
            dcs_code,
        }
    } else {
        Mode::Am {
            bandwidth,
            tone_tx_hz,
            tone_rx_hz,
            dcs_code,
        }
    };
    Channel {
        name,
        rx_hz: rec.rx_hz(),
        shift_hz: rec.shift_hz(),
        power: rec.power(),
        scan: rec.scan_add(),
        mode,
        source: None,
    }
}

/// Look up CH-`ch_no`'s 12-byte ASCII name slot. NUL- and
/// 0xFF-terminated; trailing spaces trimmed. If the slot is
/// blank, synthesise `CH_NNN` (3-digit zero-padded) — matches
/// the radio's UI numbering.
fn decode_channel_name(raw: &[u8], ch_no: usize) -> String {
    let off = CHANNEL_NAME_BASE + (ch_no - 1) * NAME_SIZE;
    if off + NAME_SIZE <= raw.len() {
        let bytes = &raw[off..off + NAME_SIZE];
        let s: String = bytes
            .iter()
            .take_while(|&&b| b != 0 && b != 0xFF)
            .map(|&b| b as char)
            .collect();
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    format!("CH_{:03}", ch_no)
}

fn decode_vfo_state(raw: &[u8]) -> Vec<VfoEntry> {
    let mut out = Vec::with_capacity(VFO_COUNT);
    for slot in 0..VFO_COUNT {
        let off = VFO_BASE + slot * RECORD_SIZE;
        if off + RECORD_SIZE > raw.len() {
            break;
        }
        let rec = ChannelRecord::ref_from_bytes(&raw[off..off + RECORD_SIZE])
            .expect("RECORD_SIZE matches ChannelRecord size_of");
        out.push(VfoEntry {
            rx_hz: rec.rx_hz(),
            tx_hz: rec.tx_hz(),
        });
    }
    out
}

fn decode_fm_broadcast(raw: &[u8]) -> Vec<u64> {
    let end = FM_BROADCAST_BASE + FM_BROADCAST_COUNT * 2;
    if raw.len() < end {
        return Vec::new();
    }
    raw[FM_BROADCAST_BASE..end]
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]) as u64 * FM_BROADCAST_UNIT_HZ)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimum image size for the new flat-array decoder:
    /// covers channel data (0x140..0x3FB0), name area
    /// (0x3FBC..0x6EA0), and FM broadcast (0x73E0..0x7408).
    const MIN_RAW: usize = 0x7500;

    #[test]
    fn rejects_image_shorter_than_channel_block() {
        // The decoder requires at least the full 999-channel
        // data block (0x140 + 999*16 = 0x3FB0 bytes).
        let r = decode_channels(&[0u8; 8]);
        assert!(matches!(r, Err(KgQ336Error::ShortImage { got: 8, .. })));
    }

    #[test]
    fn empty_image_yields_no_channels() {
        let r = decode_channels(&vec![0u8; MIN_RAW]).unwrap();
        assert!(r.channels.is_empty());
        assert!(r.warnings.is_empty());
    }

    /// PMR Kan 01 channel record + name slot, captured from
    /// the FB-Radio baseline (`KG-Q336_FB_Radio_2024.kg`).
    fn place_pmr1(buf: &mut [u8], rec: &[u8; 16], name: &[u8]) {
        buf[0x1400..0x1410].copy_from_slice(rec);
        buf[0x4DCC..0x4DCC + name.len()].copy_from_slice(name);
    }

    #[test]
    fn decodes_pmr1_baseline() {
        let mut buf = vec![0u8; MIN_RAW];
        place_pmr1(
            &mut buf,
            &[
                0x31, 0x8D, 0xA8, 0x02, 0x31, 0x8D, 0xA8, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x20,
                0x05, 0x00,
            ],
            b"PMR Kan 01",
        );
        let r = decode_channels(&buf).unwrap();
        let pmr = r.channels.iter().find(|c| c.name == "PMR Kan 01").unwrap();
        assert_eq!(pmr.rx_hz, 446_006_250);
        assert_eq!(pmr.shift_hz, 0);
        assert_eq!(pmr.power, Power::Low);
        match pmr.mode {
            Mode::Fm {
                bandwidth,
                tone_tx_hz,
                tone_rx_hz,
                dcs_code,
            } => {
                assert_eq!(bandwidth, Bandwidth::Narrow);
                assert!(tone_tx_hz.is_none());
                assert!(tone_rx_hz.is_none());
                assert!(dcs_code.is_none());
            }
            _ => panic!("expected FM"),
        }
    }

    #[test]
    fn decodes_pmr1_power_ultrahigh_lossy_to_high() {
        // Captured from `02_pmr1_pwr_ultrahigh.kg`: byte 12 = 03.
        let mut buf = vec![0u8; MIN_RAW];
        place_pmr1(
            &mut buf,
            &[
                0x31, 0x8D, 0xA8, 0x02, 0x31, 0x8D, 0xA8, 0x02, 0x00, 0x00, 0x00, 0x00, 0x03, 0x20,
                0x05, 0x00,
            ],
            b"TESTNAME.AAA",
        );
        let r = decode_channels(&buf).unwrap();
        let pmr = r
            .channels
            .iter()
            .find(|c| c.name == "TESTNAME.AAA")
            .unwrap();
        // Ultrahigh (raw 3) maps to High because narm's Power
        // enum only has 3 levels.
        assert_eq!(pmr.power, Power::High);
    }

    #[test]
    fn decodes_pmr1_bandwidth_wide_scan_on() {
        // Captured from the corrected `04_pmr1_bw_wide.kg`:
        // byte 13 = 0x21 (bit 0 = wide, bit 5 = scan add).
        let mut buf = vec![0u8; MIN_RAW];
        place_pmr1(
            &mut buf,
            &[
                0x31, 0x8D, 0xA8, 0x02, 0x31, 0x8D, 0xA8, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x21,
                0x05, 0x00,
            ],
            b"TESTNAME.AAA",
        );
        let r = decode_channels(&buf).unwrap();
        let pmr = r
            .channels
            .iter()
            .find(|c| c.name == "TESTNAME.AAA")
            .unwrap();
        assert!(pmr.scan, "scan_add bit 5 set → scan = true");
        match pmr.mode {
            Mode::Fm { bandwidth, .. } => assert_eq!(bandwidth, Bandwidth::Wide),
            _ => panic!("expected FM"),
        }
    }

    #[test]
    fn decodes_pmr1_scan_off() {
        // Captured from `05_pmr1_scan_off.kg`: byte 13 = 0x01.
        // Note: bit 0 is set (wide) too, because the CPS
        // session carried bandwidth state forward from when
        // file 04 was saved. Filenames mark the *delta* per
        // step; the file is the *cumulative* state.
        let mut buf = vec![0u8; MIN_RAW];
        place_pmr1(
            &mut buf,
            &[
                0x31, 0x8D, 0xA8, 0x02, 0x31, 0x8D, 0xA8, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
                0x05, 0x00,
            ],
            b"TESTNAME.AAA",
        );
        let r = decode_channels(&buf).unwrap();
        let pmr = r
            .channels
            .iter()
            .find(|c| c.name == "TESTNAME.AAA")
            .unwrap();
        assert!(!pmr.scan, "scan_add bit 5 clear → scan = false");
    }

    #[test]
    fn baseline_pmr_has_scan_on() {
        // Sanity check: PMR Kan 01 in the FB-Radio baseline
        // (byte 13 = 0x20) decodes with scan = true.
        let mut buf = vec![0u8; MIN_RAW];
        place_pmr1(
            &mut buf,
            &[
                0x31, 0x8D, 0xA8, 0x02, 0x31, 0x8D, 0xA8, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x20,
                0x05, 0x00,
            ],
            b"PMR Kan 01",
        );
        let r = decode_channels(&buf).unwrap();
        let pmr = r.channels.iter().find(|c| c.name == "PMR Kan 01").unwrap();
        assert!(pmr.scan);
    }

    #[test]
    fn decodes_pmr1_ctcss_tx_88_5() {
        // Captured from `06_pmr1_ctcss_tx_885.kg`: bytes
        // 10..12 = 0x75 0x83 → tone_tx_raw = 0x8375. Strip
        // the 0x8000 enable bit → 0x0375 = 885 dHz = 88.5 Hz.
        // Byte 13 also went 0x20 → 0x00 (factory flag cleared,
        // bandwidth still narrow).
        let mut buf = vec![0u8; MIN_RAW];
        place_pmr1(
            &mut buf,
            &[
                0x31, 0x8D, 0xA8, 0x02, 0x31, 0x8D, 0xA8, 0x02, 0x00, 0x00, 0x75, 0x83, 0x00, 0x00,
                0x05, 0x00,
            ],
            b"TESTNAME.AAA",
        );
        let r = decode_channels(&buf).unwrap();
        let pmr = &r
            .channels
            .iter()
            .find(|c| c.name == "TESTNAME.AAA")
            .unwrap();
        match pmr.mode {
            Mode::Fm {
                tone_tx_hz,
                bandwidth,
                ..
            } => {
                assert_eq!(tone_tx_hz, Some(88.5));
                // Narrow stays narrow even with the factory
                // flag cleared.
                assert_eq!(bandwidth, Bandwidth::Narrow);
            }
            _ => panic!("expected FM"),
        }
    }

    #[test]
    fn ctcss_tx_disabled_when_enable_bit_clear() {
        // Even if the 15-bit value would decode to a valid
        // tone, no 0x8000 bit means "no tone".
        let mut buf = vec![0u8; MIN_RAW];
        place_pmr1(
            &mut buf,
            &[
                0x31, 0x8D, 0xA8, 0x02, 0x31, 0x8D, 0xA8, 0x02, 0x00, 0x00, 0x75, 0x03, 0x00, 0x20,
                0x05, 0x00,
            ],
            b"PMR Kan 01",
        );
        let r = decode_channels(&buf).unwrap();
        let pmr = r.channels.iter().find(|c| c.name == "PMR Kan 01").unwrap();
        match pmr.mode {
            Mode::Fm { tone_tx_hz, .. } => assert!(tone_tx_hz.is_none()),
            _ => panic!(),
        }
    }

    #[test]
    fn ctcss_tx_out_of_range_treated_as_no_tone() {
        // Random bytes from a non-channel region happen to
        // have the 0x8000 enable bit set but a value outside
        // the EIA CTCSS range. Decoder must not surface them
        // as a real tone.
        let mut buf = vec![0u8; MIN_RAW];
        place_pmr1(
            &mut buf,
            &[
                0x31, 0x8D, 0xA8, 0x02, 0x31, 0x8D, 0xA8, 0x02, 0x00, 0x00, 0xF8, 0xDD, 0x00, 0x20,
                0x05, 0x00, // tone_tx_raw = 0xDDF8 → 0x5DF8 = 24056 dHz = 2405.6 Hz
            ],
            b"PMR Kan 01",
        );
        let r = decode_channels(&buf).unwrap();
        let pmr = r.channels.iter().find(|c| c.name == "PMR Kan 01").unwrap();
        match pmr.mode {
            Mode::Fm { tone_tx_hz, .. } => assert!(tone_tx_hz.is_none()),
            _ => panic!(),
        }
    }

    #[test]
    fn empty_name_slot_synthesises_ch_number() {
        // CH-302's data is populated, but its name slot is all
        // zeros — fall back to the synthetic "CH_302".
        // CH-302 data lives at 0x140 + 301*16 = 0x1410; its
        // name slot at 0x3FBC + 301*12 = 0x4DD8 stays zeroed.
        let mut buf = vec![0u8; MIN_RAW];
        buf[0x1410..0x1420].copy_from_slice(&[
            0x31, 0x8D, 0xA8, 0x02, 0x31, 0x8D, 0xA8, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x20,
            0x05, 0x00,
        ]);
        let r = decode_channels(&buf).unwrap();
        assert!(
            r.channels.iter().any(|c| c.name == "CH_302"),
            "expected CH_302, got {:?}",
            r.channels.iter().map(|c| &c.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn decodes_pmr1_ctcss_tx_100() {
        // From `07_pmr1_ctcss_tx_100.kg`: bytes 10..12 =
        // 0xE8 0x83 → tone_tx_raw = 0x83E8 → 1000 dHz = 100.0 Hz.
        // Confirms CTCSS encoding is linear value × 10.
        let mut buf = vec![0u8; MIN_RAW];
        place_pmr1(
            &mut buf,
            &[
                0x31, 0x8D, 0xA8, 0x02, 0x31, 0x8D, 0xA8, 0x02, 0x00, 0x00, 0xE8, 0x83, 0x00, 0x21,
                0x05, 0x00,
            ],
            b"TESTNAME.AAA",
        );
        let r = decode_channels(&buf).unwrap();
        let pmr = r
            .channels
            .iter()
            .find(|c| c.name == "TESTNAME.AAA")
            .unwrap();
        match pmr.mode {
            Mode::Fm { tone_tx_hz, .. } => assert_eq!(tone_tx_hz, Some(100.0)),
            _ => panic!(),
        }
    }

    #[test]
    fn decodes_pmr1_ctcss_rx_88_5() {
        // From `08_pmr1_ctcss_rx_885.kg`: bytes 8..10 =
        // 0x75 0x83 → tone_rx_raw = 0x8375 → 88.5 Hz on RX.
        // TX stays clear. Confirms RX-tone byte location.
        let mut buf = vec![0u8; MIN_RAW];
        place_pmr1(
            &mut buf,
            &[
                0x31, 0x8D, 0xA8, 0x02, 0x31, 0x8D, 0xA8, 0x02, 0x75, 0x83, 0x00, 0x00, 0x00, 0x21,
                0x05, 0x00,
            ],
            b"TESTNAME.AAA",
        );
        let r = decode_channels(&buf).unwrap();
        let pmr = r
            .channels
            .iter()
            .find(|c| c.name == "TESTNAME.AAA")
            .unwrap();
        match pmr.mode {
            Mode::Fm {
                tone_tx_hz,
                tone_rx_hz,
                ..
            } => {
                assert!(tone_tx_hz.is_none());
                assert_eq!(tone_rx_hz, Some(88.5));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn decodes_pmr1_dcs_tx_023n() {
        // From `09_pmr1_dcs_tx_023n.kg`: bytes 10..12 = 0x13 0x40
        //   → tone_tx_raw = 0x4013 (mode 0x4 = DCS-N, value 0x13 = 19).
        // 19 in octal display = "023" → narm dcs_code 23.
        let mut buf = vec![0u8; MIN_RAW];
        place_pmr1(
            &mut buf,
            &[
                0x31, 0x8D, 0xA8, 0x02, 0x31, 0x8D, 0xA8, 0x02, 0x00, 0x00, 0x13, 0x40, 0x00, 0x21,
                0x05, 0x00,
            ],
            b"TESTNAME.AAA",
        );
        let r = decode_channels(&buf).unwrap();
        let pmr = r
            .channels
            .iter()
            .find(|c| c.name == "TESTNAME.AAA")
            .unwrap();
        match pmr.mode {
            Mode::Fm {
                dcs_code,
                tone_tx_hz,
                tone_rx_hz,
                ..
            } => {
                assert_eq!(dcs_code, Some(23));
                assert!(tone_tx_hz.is_none());
                assert!(tone_rx_hz.is_none());
            }
            _ => panic!(),
        }
    }

    #[test]
    fn decodes_pmr1_dcs_tx_754n() {
        // From `10_pmr1_dcs_tx_754n.kg`: bytes 10..12 = 0xEC 0x41
        //   → tone_tx_raw = 0x41EC (mode 0x4 = DCS-N, value 0x1EC = 492).
        // 492 in octal = "754" → dcs_code 754.
        let mut buf = vec![0u8; MIN_RAW];
        place_pmr1(
            &mut buf,
            &[
                0x31, 0x8D, 0xA8, 0x02, 0x31, 0x8D, 0xA8, 0x02, 0x00, 0x00, 0xEC, 0x41, 0x00, 0x21,
                0x05, 0x00,
            ],
            b"TESTNAME.AAA",
        );
        let r = decode_channels(&buf).unwrap();
        let pmr = r
            .channels
            .iter()
            .find(|c| c.name == "TESTNAME.AAA")
            .unwrap();
        match pmr.mode {
            Mode::Fm { dcs_code, .. } => assert_eq!(dcs_code, Some(754)),
            _ => panic!(),
        }
    }

    #[test]
    fn decodes_pmr1_dcs_tx_023i_polarity_currently_dropped() {
        // From `11_pmr1_dcs_tx_023i.kg`: bytes 10..12 = 0x13 0x60
        //   → tone_tx_raw = 0x6013 (mode 0x6 = DCS-I, value 19).
        // We surface dcs_code = 23 just like 023N — narm's
        // Mode::Fm has no polarity slot, so the inverted flag
        // is currently lost. (See module docstring.)
        let mut buf = vec![0u8; MIN_RAW];
        place_pmr1(
            &mut buf,
            &[
                0x31, 0x8D, 0xA8, 0x02, 0x31, 0x8D, 0xA8, 0x02, 0x00, 0x00, 0x13, 0x60, 0x00, 0x21,
                0x05, 0x00,
            ],
            b"TESTNAME.AAA",
        );
        let r = decode_channels(&buf).unwrap();
        let pmr = r
            .channels
            .iter()
            .find(|c| c.name == "TESTNAME.AAA")
            .unwrap();
        match pmr.mode {
            Mode::Fm { dcs_code, .. } => assert_eq!(dcs_code, Some(23)),
            _ => panic!(),
        }
    }

    #[test]
    fn repeater_shift_minus_600khz() {
        // From `12_2m_minus.kg`: RX 145.650, TX 145.050 → shift -600 kHz.
        let mut buf = vec![0u8; MIN_RAW];
        place_pmr1(
            &mut buf,
            &[
                0x88, 0x3E, 0xDE, 0x00, // rx 145.650
                0x28, 0x54, 0xDD, 0x00, // tx 145.050
                0x00, 0x00, 0x00, 0x00, 0x00, 0x21, 0x05, 0x00,
            ],
            b"TESTNAME.AAA",
        );
        let r = decode_channels(&buf).unwrap();
        let ch = r
            .channels
            .iter()
            .find(|c| c.name == "TESTNAME.AAA")
            .unwrap();
        assert_eq!(ch.rx_hz, 145_650_000);
        assert_eq!(ch.shift_hz, -600_000);
    }

    #[test]
    fn repeater_shift_plus_600khz() {
        // From `13_2m_plus.kg`: RX 145.650, TX 146.250 → shift +600 kHz.
        let mut buf = vec![0u8; MIN_RAW];
        place_pmr1(
            &mut buf,
            &[
                0x88, 0x3E, 0xDE, 0x00, // rx 145.650
                0xE8, 0x28, 0xDF, 0x00, // tx 146.250
                0x00, 0x00, 0x00, 0x00, 0x00, 0x21, 0x05, 0x00,
            ],
            b"TESTNAME.AAA",
        );
        let r = decode_channels(&buf).unwrap();
        let ch = r
            .channels
            .iter()
            .find(|c| c.name == "TESTNAME.AAA")
            .unwrap();
        assert_eq!(ch.rx_hz, 145_650_000);
        assert_eq!(ch.shift_hz, 600_000);
    }

    #[test]
    fn decodes_pmr1_power_middle() {
        // From `14_pmr1_pwr_mid.kg`: byte 12 low nibble = 0x1
        // (the file also has freq carried over from 13, but
        // we only assert power here).
        let mut buf = vec![0u8; MIN_RAW];
        place_pmr1(
            &mut buf,
            &[
                0x88, 0x3E, 0xDE, 0x00, 0xE8, 0x28, 0xDF, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x21,
                0x05, 0x00,
            ],
            b"TESTNAME.AAA",
        );
        let r = decode_channels(&buf).unwrap();
        let pmr = r
            .channels
            .iter()
            .find(|c| c.name == "TESTNAME.AAA")
            .unwrap();
        assert_eq!(pmr.power, Power::Mid);
    }

    #[test]
    fn decodes_pmr1_power_high_normal() {
        // From `15_pmr1_pwr_high.kg`: byte 12 low nibble = 0x2.
        let mut buf = vec![0u8; MIN_RAW];
        place_pmr1(
            &mut buf,
            &[
                0x88, 0x3E, 0xDE, 0x00, 0xE8, 0x28, 0xDF, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x21,
                0x05, 0x00,
            ],
            b"TESTNAME.AAA",
        );
        let r = decode_channels(&buf).unwrap();
        let pmr = r
            .channels
            .iter()
            .find(|c| c.name == "TESTNAME.AAA")
            .unwrap();
        assert_eq!(pmr.power, Power::High);
    }

    #[test]
    fn power_low_nibble_is_independent_of_scramble_high_nibble() {
        // Captured from `16_pmr1_scramble_1.kg`: byte 12 = 0x10
        // (scramble level 1 in high nibble, power = low in
        // low nibble). Power decode must mask off the high
        // nibble or it'd come back as `High`.
        let rec = ChannelRecord {
            rx_freq: U32::new(14_565_000),
            tx_freq: U32::new(14_625_000),
            tone_rx_raw: U16::new(0),
            tone_tx_raw: U16::new(0),
            power_am_scramble: 0x10,
            flags1: 0x21,
            call_group: 0x05,
            _byte15: 0x00,
        };
        assert_eq!(rec.power(), Power::Low);
        assert_eq!(rec.scramble_level(), 1);
    }

    #[test]
    fn scramble_level_8_extracted_from_high_nibble() {
        // From `16b_pmr1_scramble_8.kg`: byte 12 = 0x80.
        let rec = ChannelRecord {
            rx_freq: U32::new(14_565_000),
            tx_freq: U32::new(14_625_000),
            tone_rx_raw: U16::new(0),
            tone_tx_raw: U16::new(0),
            power_am_scramble: 0x80,
            flags1: 0x21,
            call_group: 0x05,
            _byte15: 0x00,
        };
        assert_eq!(rec.scramble_level(), 8);
        assert_eq!(rec.power(), Power::Low);
    }

    #[test]
    fn decodes_pmr1_compand_on() {
        // From `17_pmr1_compand_on.kg`: byte 13 = 0x29.
        // Diff vs the 145.65/146.25 baseline (which was 0x21):
        // bit 3 (0x08) set → compand on.
        let mut buf = vec![0u8; MIN_RAW];
        place_pmr1(
            &mut buf,
            &[
                0x88, 0x3E, 0xDE, 0x00, 0xE8, 0x28, 0xDF, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x29,
                0x05, 0x00,
            ],
            b"TESTNAME.AAA",
        );
        let r = decode_channels(&buf).unwrap();
        assert!(
            r.warnings
                .iter()
                .any(|w| w.contains("TESTNAME.AAA") && w.contains("compand")),
            "expected compand warning, got {:?}",
            r.warnings
        );
    }

    #[test]
    fn startup_message_decoded_from_offset_0x84() {
        // From `18_owner.kg`: 20 bytes at 0x84 changed from
        // "www.fbradio.se\0\0\0\0\0\0" to "abcd1234567890abcd01".
        let mut buf = vec![0u8; MIN_RAW];
        buf[0x0084..0x0098].copy_from_slice(b"abcd1234567890abcd01");
        let r = decode_channels(&buf).unwrap();
        assert_eq!(r.startup_message.as_deref(), Some("abcd1234567890abcd01"));
    }

    #[test]
    fn startup_message_handles_nul_padded_short_string() {
        let mut buf = vec![0u8; MIN_RAW];
        buf[0x0084..0x008e].copy_from_slice(b"hello\0\0\0\0\0");
        let r = decode_channels(&buf).unwrap();
        assert_eq!(r.startup_message.as_deref(), Some("hello"));
    }

    #[test]
    fn startup_message_blank_returns_none() {
        let buf = vec![0u8; MIN_RAW];
        let r = decode_channels(&buf).unwrap();
        assert_eq!(r.startup_message, None);
    }

    #[test]
    fn decodes_riks1_at_sparse_slot_0() {
        // Riks category data + name slot 0. Captured from
        // the FB-Radio baseline: 85.9375 MHz simplex.
        let mut buf = vec![0u8; MIN_RAW];
        buf[0x04A0..0x04B0].copy_from_slice(&[
            0x56, 0x21, 0x83, 0x00, 0x56, 0x21, 0x83, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x21,
            0x02, 0x00,
        ]);
        buf[0x4244..0x4250].copy_from_slice(b"Riks 1\x00\x00\x00\x00\x00\x00");
        let r = decode_channels(&buf).unwrap();
        let ch = r.channels.iter().find(|c| c.name == "Riks 1").unwrap();
        assert_eq!(ch.rx_hz, 85_937_500);
    }

    #[test]
    fn decodes_riks2_at_sparse_slot_35() {
        // Riks 2 is at slot 35: data at 0x04A0+35*16=0x06D0,
        // name at 0x4244+35*12=0x43E8.
        let mut buf = vec![0u8; MIN_RAW];
        buf[0x06D0..0x06E0].copy_from_slice(&[
            0x1A, 0x2B, 0x83, 0x00, 0x1A, 0x2B, 0x83, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x21,
            0x02, 0x00,
        ]);
        buf[0x43E8..0x43F4].copy_from_slice(b"Riks 2\x00\x00\x00\x00\x00\x00");
        let r = decode_channels(&buf).unwrap();
        let ch = r.channels.iter().find(|c| c.name == "Riks 2").unwrap();
        assert_eq!(ch.rx_hz, 85_962_500);
    }

    #[test]
    fn riks_rename_writes_to_offset_0x4244() {
        // From `20_riks1_name.kg`: bytes 0x4244..0x424f changed
        // from "Riks 1\0\0\0\0\0\0" to "RIKS_TEST\0\0\0".
        let mut buf = vec![0u8; MIN_RAW];
        // Populate Riks 1 data so the name lookup actually
        // runs (otherwise is_plausible rejects the slot).
        buf[0x04A0..0x04B0].copy_from_slice(&[
            0x56, 0x21, 0x83, 0x00, 0x56, 0x21, 0x83, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x21,
            0x02, 0x00,
        ]);
        buf[0x4244..0x4250].copy_from_slice(b"RIKS_TEST\x00\x00\x00");
        let r = decode_channels(&buf).unwrap();
        assert!(r.channels.iter().any(|c| c.name == "RIKS_TEST"));
    }

    #[test]
    fn decodes_pmr1_dcs_rx_023n() {
        // From `21_pmr1_dcs_rx_023n.kg`: bytes 8..10 = 0x13 0x40
        //   → tone_rx_raw = 0x4013 (DCS-N, value 19 = 023 oct).
        // Confirms RX uses the same tone encoding as TX.
        let mut buf = vec![0u8; MIN_RAW];
        place_pmr1(
            &mut buf,
            &[
                0x31, 0x8D, 0xA8, 0x02, 0x31, 0x8D, 0xA8, 0x02, 0x13, 0x40, 0x00, 0x00, 0x00, 0x20,
                0x05, 0x00,
            ],
            b"PMR Kan 01",
        );
        let r = decode_channels(&buf).unwrap();
        let pmr = r.channels.iter().find(|c| c.name == "PMR Kan 01").unwrap();
        match pmr.mode {
            Mode::Fm {
                dcs_code,
                tone_tx_hz,
                tone_rx_hz,
                ..
            } => {
                assert_eq!(dcs_code, Some(23));
                assert!(tone_tx_hz.is_none());
                assert!(tone_rx_hz.is_none());
            }
            _ => panic!(),
        }
    }

    #[test]
    fn decodes_pmr1_mute_mode_qt_plus_dtmf() {
        // From `22_pmr1_mute_qt_plus_dtmf.kg`: byte 13 went
        // 0x20 → 0x22. The +0x02 is bit 1 = mute_mode bit 0.
        let mut buf = vec![0u8; MIN_RAW];
        place_pmr1(
            &mut buf,
            &[
                0x31, 0x8D, 0xA8, 0x02, 0x31, 0x8D, 0xA8, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x22,
                0x05, 0x00,
            ],
            b"PMR Kan 01",
        );
        let r = decode_channels(&buf).unwrap();
        assert!(
            r.warnings
                .iter()
                .any(|w| w.contains("PMR Kan 01") && w.contains("QT+DTMF")),
            "expected QT+DTMF warning, got {:?}",
            r.warnings
        );
    }

    #[test]
    fn decodes_pmr1_mute_mode_qt_star_dtmf() {
        // From `23_pmr1_mute_qt_star_dtmf.kg`: byte 13 = 0x24
        // (bit 2 = mute_mode bit 1).
        let mut buf = vec![0u8; MIN_RAW];
        place_pmr1(
            &mut buf,
            &[
                0x31, 0x8D, 0xA8, 0x02, 0x31, 0x8D, 0xA8, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x24,
                0x05, 0x00,
            ],
            b"PMR Kan 01",
        );
        let r = decode_channels(&buf).unwrap();
        assert!(
            r.warnings
                .iter()
                .any(|w| w.contains("PMR Kan 01") && w.contains("QT*DTMF")),
            "expected QT*DTMF warning, got {:?}",
            r.warnings
        );
    }

    #[test]
    fn decodes_pmr1_am_rx_emits_am_mode() {
        // From `24_pmr1_am_rx.kg`: byte 12 = 0x04 (bit 2
        // = am_mode bit 0). AM Rx → Mode::Am, no Rx&Tx
        // warning.
        let mut buf = vec![0u8; MIN_RAW];
        place_pmr1(
            &mut buf,
            &[
                0x31, 0x8D, 0xA8, 0x02, 0x31, 0x8D, 0xA8, 0x02, 0x00, 0x00, 0x00, 0x00, 0x04, 0x20,
                0x05, 0x00,
            ],
            b"PMR Kan 01",
        );
        let r = decode_channels(&buf).unwrap();
        let pmr = r.channels.iter().find(|c| c.name == "PMR Kan 01").unwrap();
        assert!(matches!(pmr.mode, Mode::Am { .. }));
        assert!(
            !r.warnings.iter().any(|w| w.contains("Rx&Tx")),
            "AM Rx (mode 1) should not warn about Rx&Tx"
        );
    }

    #[test]
    fn decodes_pmr1_am_rx_tx_emits_am_with_warning() {
        // From `25_pmr1_am_rx_tx.kg`: byte 12 = 0x08 (bit 3
        // = am_mode bit 1). AM Rx&Tx → Mode::Am + warning
        // about TX-side AM being dropped.
        let mut buf = vec![0u8; MIN_RAW];
        place_pmr1(
            &mut buf,
            &[
                0x31, 0x8D, 0xA8, 0x02, 0x31, 0x8D, 0xA8, 0x02, 0x00, 0x00, 0x00, 0x00, 0x08, 0x20,
                0x05, 0x00,
            ],
            b"PMR Kan 01",
        );
        let r = decode_channels(&buf).unwrap();
        let pmr = r.channels.iter().find(|c| c.name == "PMR Kan 01").unwrap();
        assert!(matches!(pmr.mode, Mode::Am { .. }));
        assert!(
            r.warnings
                .iter()
                .any(|w| w.contains("PMR Kan 01") && w.contains("AM Rx&Tx")),
            "expected Rx&Tx warning, got {:?}",
            r.warnings
        );
    }

    #[test]
    fn power_does_not_collide_with_am_bits() {
        // Regression: power_idx must mask only bits 0..1.
        // If we accidentally masked the low nibble, AM Rx
        // (bit 2 = 0x04) would parse as power=4 → mapped
        // to High; with the correct mask power must stay Low.
        let mut buf = vec![0u8; MIN_RAW];
        place_pmr1(
            &mut buf,
            &[
                0x31, 0x8D, 0xA8, 0x02, 0x31, 0x8D, 0xA8, 0x02, 0x00, 0x00, 0x00, 0x00, 0x04, 0x20,
                0x05, 0x00,
            ],
            b"PMR Kan 01",
        );
        let r = decode_channels(&buf).unwrap();
        let pmr = r.channels.iter().find(|c| c.name == "PMR Kan 01").unwrap();
        assert_eq!(pmr.power, Power::Low);
    }

    #[test]
    fn srbr_kan_01_decodes_with_call_group_4() {
        // CH-201 = SRBR Kan 01 in the FB-Radio baseline.
        // Captured trailer bytes show byte 14 = 0x04 → Call
        // Group 4, matching the CPS Channel Information tab
        // (image 9 in docs/kgq336-codeplug.md).
        let mut buf = vec![0u8; MIN_RAW];
        // CH-201 data at 0x140 + 200*16 = 0x0DC0.
        buf[0x0DC0..0x0DD0].copy_from_slice(&[
            0xE0, 0x67, 0xA6, 0x02, 0xE0, 0x67, 0xA6, 0x02, 0x00, 0x00, 0x00, 0x00, 0x02, 0x21,
            0x04, 0x00,
        ]);
        // CH-201 name at 0x3FBC + 200*12 = 0x491C.
        buf[0x491C..0x4928].copy_from_slice(b"SRBR  Kan 01");
        let r = decode_channels(&buf).unwrap();
        let ch = r
            .channels
            .iter()
            .find(|c| c.name == "SRBR  Kan 01")
            .unwrap();
        assert_eq!(ch.rx_hz, 444_600_000);
        // Call Group 4 ≠ 1 → emit a warning.
        assert!(
            r.warnings
                .iter()
                .any(|w| w.contains("SRBR  Kan 01") && w.contains("Call Group 4")),
            "expected Call Group 4 warning, got {:?}",
            r.warnings
        );
    }

    #[test]
    fn call_group_1_does_not_warn() {
        // CH-501 = 69M Kanal 01 with Call Group 1 (default) →
        // no warning emitted.
        let mut buf = vec![0u8; MIN_RAW];
        buf[0x2080..0x2090].copy_from_slice(&[
            0x02, 0x4E, 0x69, 0x00, 0x02, 0x4E, 0x69, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x21,
            0x01, 0x00,
        ]);
        buf[0x572C..0x5738].copy_from_slice(b"69M Kanal 01");
        let r = decode_channels(&buf).unwrap();
        assert!(r.channels.iter().any(|c| c.name == "69M Kanal 01"));
        assert!(
            !r.warnings.iter().any(|w| w.contains("Call Group")),
            "expected no Call Group warning for default group 1, got {:?}",
            r.warnings
        );
    }

    #[test]
    fn decimal_to_octal_display_known_values() {
        // Sanity checks for the helper.
        assert_eq!(decimal_to_octal_display(0), 0);
        assert_eq!(decimal_to_octal_display(19), 23); // 023 oct
        assert_eq!(decimal_to_octal_display(492), 754); // 754 oct
        assert_eq!(decimal_to_octal_display(8), 10); // 010 oct
    }

    #[test]
    fn arbitrary_channel_index_decodes_with_synthetic_name() {
        // 0x3000 = data for CH = (0x3000 - 0x140) / 16 + 1 = 749.
        // No name written → fall back to "CH_749".
        let mut buf = vec![0u8; MIN_RAW];
        buf[0x3000..0x3010].copy_from_slice(&[
            0x31, 0x8D, 0xA8, 0x02, 0x31, 0x8D, 0xA8, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ]);
        let r = decode_channels(&buf).unwrap();
        let ch = r.channels.iter().find(|c| c.name == "CH_749").unwrap();
        assert_eq!(ch.rx_hz, 446_006_250);
    }

    #[test]
    fn vfo_state_decoded_from_0x00b0() {
        // The 8 VFO entries occupy 0x00B0..0x0140 in the same
        // [rx_freq u32 LE × 10 Hz] format as channel records.
        let mut buf = vec![0u8; MIN_RAW];
        // VFO slot 0 = 118.10 MHz simplex (real bytes from
        // FB-Radio baseline).
        buf[0x00B0..0x00C0].copy_from_slice(&[
            0xD0, 0x34, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x01,
            0x01, 0x05,
        ]);
        let r = decode_channels(&buf).unwrap();
        assert_eq!(r.vfo_state.len(), 8);
        assert_eq!(r.vfo_state[0].rx_hz, 118_100_000);
        assert_eq!(r.vfo_state[0].tx_hz, 0);
        // VFO entries are NOT emitted as channels (they live
        // outside the flat 999-slot channel array).
        assert!(r.channels.is_empty());
    }

    #[test]
    fn fm_broadcast_decoded_from_0x73e0() {
        // 20 × u16 LE × 100 kHz starting at 0x73E0.
        let mut buf = vec![0u8; MIN_RAW];
        // Slot 0 = 76.0 MHz (760 = 0x02F8) → matches the CPS
        // default in the FB-Radio baseline.
        buf[0x73E0..0x73E2].copy_from_slice(&760u16.to_le_bytes());
        // Slot 5 = 100.5 MHz (1005 = 0x03ED).
        buf[0x73E0 + 10..0x73E0 + 12].copy_from_slice(&1005u16.to_le_bytes());
        let r = decode_channels(&buf).unwrap();
        assert_eq!(r.fm_broadcast.len(), 20);
        assert_eq!(r.fm_broadcast[0], 76_000_000);
        assert_eq!(r.fm_broadcast[5], 100_500_000);
        // FM broadcast presets are NOT emitted as channels.
        assert!(r.channels.is_empty());
    }
}
