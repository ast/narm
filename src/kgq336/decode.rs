//! Channel decoding for the KG-Q332/Q336.
//!
//! Reverse-engineered from `.kg` files saved by the vendor's
//! CPS via byte-diffing single-field changes. v0.4 decodes:
//! frequency (rx + tx, absolute split for repeaters), channel
//! name, power, bandwidth, CTCSS TX, CTCSS RX, DCS (normal +
//! inverted polarity), and the scan-add flag.
//!
//! ## Tone-field encoding (bytes 8..10 = RX, 10..12 = TX)
//!
//! Both the RX and TX tone slots share the same 16-bit layout:
//!
//! | high nibble | meaning              |
//! |-------------|----------------------|
//! | `0x0`       | no tone              |
//! | `0x4`       | DCS normal polarity  |
//! | `0x6`       | DCS inverted polarity|
//! | `0x8`       | CTCSS                |
//!
//! Low 12 bits = value:
//! - CTCSS: `freq × 10` deci-Hz (so 88.5 Hz → `0x375`, 100.0 Hz → `0x3E8`).
//! - DCS: decimal of the octal display code (so `023` oct → 19 dec, `754` oct → 492 dec).
//!
//! narm's [`Mode::Fm`] only has one `dcs_code` slot, so if both
//! TX and RX carry DCS we surface the TX value (matching the
//! UV-K5 decoder's policy). DCS polarity is currently lost in
//! the decode — a `dcs_polarity` field would need to be added
//! to [`Mode::Fm`].
//!
//! ## On-disk record (16 bytes)
//!
//! ```text
//!   bytes 0..4   rx_freq:     u32 LE × 10 Hz
//!   bytes 4..8   tx_freq:     u32 LE × 10 Hz  (0 → simplex; absolute
//!                                              for repeaters — sign of
//!                                              shift comes from tx-rx)
//!   bytes 8..10  tone_rx_raw: u16 LE          (see tone-field encoding)
//!   bytes 10..12 tone_tx_raw: u16 LE          (same encoding as RX)
//!   byte 12      power:       u8              (0=low, 1=mid, 2=high, 3=ultrahigh)
//!   byte 13      flags1:      u8              (bit 0 = wide bandwidth,
//!                                              bit 5 = scan add)
//!   byte 14      category:    u8              (1=simplex/69M, 2=86MHz,
//!                                              3=SRBR-VHF, 4=SRBR-UHF, 5=PMR)
//!   byte 15      flags2:      u8              (5 for band scanner, 0 otherwise)
//! ```
//!
//! ## Layout
//!
//! Channels are grouped by category. Each category has a
//! contiguous 16-byte data block plus a contiguous 12-byte
//! name block at a different offset. Categories observed in
//! the FB-Radio sample:
//!
//! | name  | data offset | name offset | count |
//! |-------|-------------|-------------|-------|
//! | scanner | 0x00b0    | (no names)  | 8     |
//! | simplex | 0x0140    | (no names)  | 1     |
//! | Jakt    | 0x0780    | 0x446c      | 7     |
//! | SRBR    | 0x0dc0    | 0x491c      | 8     |
//! | PMR     | 0x1400    | 0x4dcc      | 16    |
//! | 69M     | 0x2080    | 0x572c      | 18    |
//!
//! Categories without a name block synthesise names from the
//! prefix + slot index.

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
    /// 0=low, 1=mid, 2=high, 3=ultrahigh. The vendor's CPS
    /// exposes 4 levels; narm's [`Power`] only has 3, so
    /// ultrahigh maps to `High` (lossy — see `decode_power`).
    power_idx: u8,
    /// Bit 0 = wide bandwidth (clear = narrow).
    /// Bit 5 = "scan add" — channel is in the scan list
    /// (the CPS toggle between Scramble and Compand).
    flags1: u8,
    /// Category / zone index (PMR=5, 69M=1, SRBR-UHF=4, …).
    /// We don't surface this directly; it's a hint for the
    /// per-category layout we already encode in [`CATEGORIES`].
    _category: u8,
    /// 5 for band-scanner entries (0x00b0–0x0120), 0 elsewhere.
    /// Treated as an opaque flag for now.
    _flags2: u8,
}

const RECORD_SIZE: usize = 16;
const NAME_SIZE: usize = 12;

const MIN_PLAUSIBLE_HZ: u64 = 1_000_000;
const MAX_PLAUSIBLE_HZ: u64 = 1_000_000_000;
const MAX_SHIFT_HZ: i64 = 50_000_000;

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
/// Bit 5 of `flags1`: scan-add (clear = excluded from scan).
const FLAGS1_SCAN_BIT: u8 = 0x20;

/// Per-category layout: a contiguous block of channel records
/// plus an optional contiguous block of 12-byte ASCII names.
struct Category {
    /// Prefix used to synthesise names when `name_offset` is
    /// `None`, or when a slot's name area is blank. e.g.
    /// "PMR" → `PMR_03` for slot index 2.
    prefix: &'static str,
    data_offset: usize,
    name_offset: Option<usize>,
    count: usize,
}

const CATEGORIES: &[Category] = &[
    Category {
        prefix: "BAND",
        data_offset: 0x00B0,
        name_offset: None,
        count: 8,
    },
    Category {
        prefix: "SIMPLEX",
        data_offset: 0x0140,
        name_offset: None,
        count: 1,
    },
    Category {
        prefix: "Jakt",
        data_offset: 0x0780,
        name_offset: Some(0x446C),
        count: 7,
    },
    Category {
        prefix: "SRBR",
        data_offset: 0x0DC0,
        name_offset: Some(0x491C),
        count: 8,
    },
    Category {
        prefix: "PMR",
        data_offset: 0x1400,
        name_offset: Some(0x4DCC),
        count: 16,
    },
    Category {
        prefix: "69M",
        data_offset: 0x2080,
        name_offset: Some(0x572C),
        count: 18,
    },
];

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
        if tx == 0 {
            return true;
        }
        if !(MIN_PLAUSIBLE_HZ..=MAX_PLAUSIBLE_HZ).contains(&tx) {
            return false;
        }
        (tx as i64 - rx as i64).abs() <= MAX_SHIFT_HZ
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
        // The radio has 4 power levels but narm's enum has 3,
        // so 2 (high) and 3 (ultrahigh) both map to High. If
        // we ever care about ultrahigh, we'd add a 4th
        // variant or a wattage field.
        match self.power_idx {
            0 => Power::Low,
            1 => Power::Mid,
            _ => Power::High,
        }
    }
}

pub struct DecodeReport {
    pub channels: Vec<Channel>,
    pub warnings: Vec<String>,
}

pub fn decode_channels(raw: &[u8]) -> Result<DecodeReport, KgQ336Error> {
    if raw.len() < RECORD_SIZE {
        return Err(KgQ336Error::ShortImage {
            got: raw.len(),
            min: RECORD_SIZE,
        });
    }

    let mut channels = Vec::new();
    let mut warnings = Vec::new();
    // Track which 16-byte slots we've already emitted from a
    // categorised block, so the fallback walk below doesn't
    // double-count.
    let mut covered = Vec::<bool>::new();
    covered.resize(raw.len() / RECORD_SIZE + 1, false);

    for cat in CATEGORIES {
        for slot in 0..cat.count {
            let off = cat.data_offset + slot * RECORD_SIZE;
            if off + RECORD_SIZE > raw.len() {
                break;
            }
            let rec = ChannelRecord::ref_from_bytes(&raw[off..off + RECORD_SIZE])
                .expect("RECORD_SIZE matches ChannelRecord size_of");
            covered[off / RECORD_SIZE] = true;
            if !rec.is_plausible() {
                continue;
            }
            let name = decode_name(raw, cat, slot);
            channels.push(record_to_channel(rec, name));
        }
    }

    // Fallback: walk every remaining 16-byte slot for any
    // plausible channels we missed. These get synthetic
    // `Q336_<offset_hex>` names since they aren't in a known
    // category. If any show up in real codeplugs, they're
    // worth investigating to extend [`CATEGORIES`].
    let mut off = 0;
    while off + RECORD_SIZE <= raw.len() {
        if !covered[off / RECORD_SIZE] {
            let rec = ChannelRecord::ref_from_bytes(&raw[off..off + RECORD_SIZE])
                .expect("RECORD_SIZE matches ChannelRecord size_of");
            if rec.is_plausible() {
                let name = format!("Q336_{:04x}", off);
                warnings.push(format!(
                    "uncategorised channel at 0x{:04x} — extend CATEGORIES if recurring",
                    off
                ));
                channels.push(record_to_channel(rec, name));
            }
        }
        off += RECORD_SIZE;
    }

    Ok(DecodeReport { channels, warnings })
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
    Channel {
        name,
        rx_hz: rec.rx_hz(),
        shift_hz: rec.shift_hz(),
        power: rec.power(),
        scan: rec.scan_add(),
        mode: Mode::Fm {
            bandwidth: rec.bandwidth(),
            tone_tx_hz,
            tone_rx_hz,
            dcs_code,
        },
        source: None,
    }
}

/// Read a 12-byte ASCII name slot. NUL- and 0xFF-terminated;
/// trailing spaces trimmed. If the slot is empty (or the
/// category has no name block), synthesise from the
/// category prefix + 1-based slot index.
fn decode_name(raw: &[u8], cat: &Category, slot: usize) -> String {
    if let Some(base) = cat.name_offset {
        let off = base + slot * NAME_SIZE;
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
    }
    format!("{}_{:02}", cat.prefix, slot + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN_RAW: usize = 0x6000;

    #[test]
    fn rejects_image_shorter_than_one_record() {
        let r = decode_channels(&[0u8; 8]);
        assert!(matches!(
            r,
            Err(KgQ336Error::ShortImage { got: 8, min: 16 })
        ));
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
    fn empty_name_slot_synthesises_from_prefix() {
        // PMR slot 1's data is populated, but its name is all
        // zeros — fall back to the synthetic "PMR_02".
        let mut buf = vec![0u8; MIN_RAW];
        // Slot index 1 (i.e. PMR Kan 02), data at 0x1410.
        buf[0x1410..0x1420].copy_from_slice(&[
            0x31, 0x8D, 0xA8, 0x02, 0x31, 0x8D, 0xA8, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x20,
            0x05, 0x00,
        ]);
        // No name written → name slot at 0x4DD8 stays zeroed.
        let r = decode_channels(&buf).unwrap();
        assert!(r.channels.iter().any(|c| c.name == "PMR_02"));
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
    fn decimal_to_octal_display_known_values() {
        // Sanity checks for the helper.
        assert_eq!(decimal_to_octal_display(0), 0);
        assert_eq!(decimal_to_octal_display(19), 23); // 023 oct
        assert_eq!(decimal_to_octal_display(492), 754); // 754 oct
        assert_eq!(decimal_to_octal_display(8), 10); // 010 oct
    }

    #[test]
    fn uncategorised_channel_emits_warning() {
        // A channel at 0x3000 (outside any known category).
        let mut buf = vec![0u8; MIN_RAW];
        buf[0x3000..0x3010].copy_from_slice(&[
            0x31, 0x8D, 0xA8, 0x02, 0x31, 0x8D, 0xA8, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ]);
        let r = decode_channels(&buf).unwrap();
        let ch = r.channels.iter().find(|c| c.name == "Q336_3000").unwrap();
        assert_eq!(ch.rx_hz, 446_006_250);
        assert_eq!(r.warnings.len(), 1);
        assert!(r.warnings[0].contains("0x3000"));
    }
}
