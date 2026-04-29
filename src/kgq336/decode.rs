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

/// Settings block — 132 B at offset 0. See [`Settings`] /
/// [`SettingsRaw`] and `docs/kgq336-codeplug.md` for the
/// per-byte map.
const SETTINGS_BASE: usize = 0x0000;
const SETTINGS_SIZE: usize = 0x0084;

/// Scan group ranges — 10 × `(start, end : u16 LE)` at
/// `0x7278`. The 8 B at `0x7270..0x7278` ahead of this likely
/// hold the `All` group plus A/B flags (TBD).
const SCAN_GROUP_RANGE_BASE: usize = 0x7278;
/// Scan group names — 10 × 12 B ASCII NUL-padded at `0x72A0`.
/// Index in storage is `UI group number - 1`.
const SCAN_GROUP_NAME_BASE: usize = 0x72A0;
const SCAN_GROUP_COUNT: usize = 10;
const SCAN_GROUP_NAME_SIZE: usize = 12;

/// Call group 1 name — 12 B ASCII NUL-padded at `0x766C`.
/// Slot pitch unknown; only slot 0 surfaced.
const CALL_GROUP_1_NAME_BASE: usize = 0x766C;
const CALL_GROUP_1_NAME_SIZE: usize = 12;

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
    /// Radio-wide Configuration / Key Settings block. `None`
    /// only if the image is shorter than 0x84 bytes (which
    /// `decode_channels` already rejects via `ShortImage`).
    pub settings: Option<Settings>,
    /// Scan groups 1..10. Always returns 10 entries (blank
    /// `name` and `(0, 0)` range for unconfigured slots) so
    /// the index field tracks the CPS UI row directly.
    pub scan_groups: Vec<ScanGroup>,
    /// Call group 1's name (`0x766C`, 12 B). `None` if blank.
    /// Slot pitch + per-group call code are TBD, so the rest
    /// of the call-settings table isn't decoded.
    pub call_group_1_name: Option<String>,
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

/// CPS Scan Mode for the radio-wide Scan setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanMode {
    /// Time-Operated: dwell on a busy channel for a fixed time.
    TimeOperated,
    /// Carrier-Operated: stay on a busy channel until the
    /// carrier drops.
    CarrierOperated,
    Other(u8),
}

impl ScanMode {
    fn from_raw(b: u8) -> Self {
        match b {
            0 => Self::TimeOperated,
            1 => Self::CarrierOperated,
            n => Self::Other(n),
        }
    }
}

/// CPS PTT-ID setting. Only `Off` and `Bot` are confirmed
/// from captures; `Other(u8)` carries the rest (likely
/// `Eot` / `Both`) until those captures land.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PttId {
    Off,
    Bot,
    Other(u8),
}

impl PttId {
    fn from_raw(b: u8) -> Self {
        match b {
            0 => Self::Off,
            1 => Self::Bot,
            n => Self::Other(n),
        }
    }
}

/// Top-key short-press behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopKey {
    Alarm,
    Sos,
    Other(u8),
}

impl TopKey {
    fn from_raw(b: u8) -> Self {
        match b {
            0 => Self::Alarm,
            1 => Self::Sos,
            n => Self::Other(n),
        }
    }
}

/// Boot screen content selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupDisplay {
    Image,
    BatteryVoltage,
    Other(u8),
}

impl StartupDisplay {
    fn from_raw(b: u8) -> Self {
        match b {
            0 => Self::Image,
            1 => Self::BatteryVoltage,
            n => Self::Other(n),
        }
    }
}

/// Sidetone behaviour. Only `Off` and `Dtst` confirmed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sidetone {
    Off,
    Dtst,
    Other(u8),
}

impl Sidetone {
    fn from_raw(b: u8) -> Self {
        match b {
            0 => Self::Off,
            1 => Self::Dtst,
            n => Self::Other(n),
        }
    }
}

/// Raw 132-byte Settings block at offset `0x0000`. Only the
/// fields with confirmed semantics are named — the rest are
/// `_bNN: [u8; N]` filler so the struct size matches `0x84`
/// exactly. See `docs/kgq336-codeplug.md` for the byte map.
#[repr(C)]
#[derive(FromBytes, Immutable, KnownLayout, Debug, Clone, Copy)]
struct SettingsRaw {
    _b00: u8,
    battery_save: u8,
    _b02: u8,
    tot: u8,
    _b04: u8,
    vox: u8,
    _b06: [u8; 2],
    beep: u8,
    scan_mode: u8,
    backlight: u8,
    brightness_active: u8,
    _b0c: u8,
    startup_display: u8,
    ptt_id: u8,
    _b0f: u8,
    sidetone: u8,
    _b11: [u8; 4],
    auto_lock: u8,
    priority_channel: U16,
    _b18: u8,
    rpt_setting: u8,
    _b1a: [u8; 7],
    theme: u8,
    _b22: [u8; 2],
    time_zone: u8,
    _b25: u8,
    gps_on: u8,
    _b27: [u8; 33],
    mode_switch_password: [u8; 6],
    reset_password: [u8; 6],
    _b54: [u8; 8],
    vfo_squelch: [u8; 2],
    _b5e: [u8; 6],
    top_key: u8,
    pf1_short: u8,
    _b66: u8,
    pf2_long: u8,
    pf3_short: u8,
    _b69: [u8; 5],
    ani_code: [u8; 6],
    scc_code: [u8; 6],
    _b7a: [u8; 10],
}

const _: () = assert!(size_of::<SettingsRaw>() == SETTINGS_SIZE);

/// Decoded radio-wide settings. Binary fields are `bool`;
/// multi-state fields are enums with `Other(u8)` catch-alls
/// so unknown values round-trip without warning noise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub battery_save: bool,
    /// Time-Out Timer index. Encoding TBD — baseline `04`
    /// and `01` = 15 s confirmed; intermediate values not
    /// yet captured.
    pub tot: u8,
    /// VOX level (0=off, 1..10).
    pub vox: u8,
    pub beep: bool,
    pub scan_mode: ScanMode,
    /// Backlight time, seconds. `05` = 5 s confirmed.
    pub backlight_seconds: u8,
    /// Active-screen brightness, 1..10.
    pub brightness_active: u8,
    pub startup_display: StartupDisplay,
    pub ptt_id: PttId,
    pub sidetone: Sidetone,
    pub auto_lock: bool,
    /// Channel number used as the priority-scan channel.
    pub priority_channel: u16,
    /// "RPT Setting" enum index. Semantic TBD.
    pub rpt_setting: u8,
    /// 0..3 (4 themes per CPS image 2).
    pub theme: u8,
    /// Time-zone index (`0c` baseline).
    pub time_zone: u8,
    pub gps_on: bool,
    /// 6 ASCII digits `'0'..'9'`. Baseline = `"000000"`.
    pub mode_switch_password: [u8; 6],
    /// 6 ASCII digits `'0'..'9'`. Baseline = `"000000"`.
    pub reset_password: [u8; 6],
    /// VFO A squelch level, 0..9.
    pub vfo_squelch_a: u8,
    /// VFO B squelch level, 0..9.
    pub vfo_squelch_b: u8,
    pub top_key: TopKey,
    pub pf1_short: u8,
    pub pf2_long: u8,
    pub pf3_short: u8,
    /// 6 bytes; one digit `0..9` per byte, `0x0F` / `0xF0`
    /// = padding/terminator. See [`Settings::ani_code_string`].
    pub ani_code: [u8; 6],
    /// Same encoding as `ani_code`.
    pub scc_code: [u8; 6],
}

impl Settings {
    fn from_raw(r: &SettingsRaw) -> Self {
        Self {
            battery_save: r.battery_save != 0,
            tot: r.tot,
            vox: r.vox,
            beep: r.beep != 0,
            scan_mode: ScanMode::from_raw(r.scan_mode),
            backlight_seconds: r.backlight,
            brightness_active: r.brightness_active,
            startup_display: StartupDisplay::from_raw(r.startup_display),
            ptt_id: PttId::from_raw(r.ptt_id),
            sidetone: Sidetone::from_raw(r.sidetone),
            auto_lock: r.auto_lock != 0,
            priority_channel: r.priority_channel.get(),
            rpt_setting: r.rpt_setting,
            theme: r.theme,
            time_zone: r.time_zone,
            gps_on: r.gps_on != 0,
            mode_switch_password: r.mode_switch_password,
            reset_password: r.reset_password,
            vfo_squelch_a: r.vfo_squelch[0],
            vfo_squelch_b: r.vfo_squelch[1],
            top_key: TopKey::from_raw(r.top_key),
            pf1_short: r.pf1_short,
            pf2_long: r.pf2_long,
            pf3_short: r.pf3_short,
            ani_code: r.ani_code,
            scc_code: r.scc_code,
        }
    }

    /// ANI code as ASCII digits. Stops at the first non-digit
    /// byte (`0x0F`, `0xF0`, etc.), matching the CPS view.
    pub fn ani_code_string(&self) -> String {
        decode_bcd_digits(&self.ani_code)
    }

    /// SCC code as ASCII digits. See [`Self::ani_code_string`].
    pub fn scc_code_string(&self) -> String {
        decode_bcd_digits(&self.scc_code)
    }
}

fn decode_bcd_digits(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take_while(|&&b| b <= 9)
        .map(|&b| char::from(b'0' + b))
        .collect()
}

/// One scan group (CPS UI groups `1..10`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanGroup {
    /// 1-based UI index, matches the row number in image 6.
    pub index: u8,
    /// Group name (NUL/`0xFF`-trimmed). Empty for slots the
    /// user hasn't renamed.
    pub name: String,
    /// First channel in the group's range (1-based). `0` for
    /// unconfigured slots.
    pub start_channel: u16,
    /// Last channel in the group's range (1-based). `0` for
    /// unconfigured slots.
    pub end_channel: u16,
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
    let settings = decode_settings(raw);
    let scan_groups = decode_scan_groups(raw);
    let call_group_1_name = decode_call_group_1_name(raw);

    Ok(DecodeReport {
        channels,
        warnings,
        startup_message,
        vfo_state,
        fm_broadcast,
        settings,
        scan_groups,
        call_group_1_name,
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

fn decode_settings(raw: &[u8]) -> Option<Settings> {
    let end = SETTINGS_BASE + SETTINGS_SIZE;
    if raw.len() < end {
        return None;
    }
    let r = SettingsRaw::ref_from_bytes(&raw[SETTINGS_BASE..end])
        .expect("SETTINGS_SIZE matches SettingsRaw size_of");
    Some(Settings::from_raw(r))
}

fn decode_scan_groups(raw: &[u8]) -> Vec<ScanGroup> {
    let ranges_end = SCAN_GROUP_RANGE_BASE + SCAN_GROUP_COUNT * 4;
    let names_end = SCAN_GROUP_NAME_BASE + SCAN_GROUP_COUNT * SCAN_GROUP_NAME_SIZE;
    if raw.len() < ranges_end || raw.len() < names_end {
        return Vec::new();
    }
    (0..SCAN_GROUP_COUNT)
        .map(|i| {
            let r_off = SCAN_GROUP_RANGE_BASE + i * 4;
            let start = u16::from_le_bytes([raw[r_off], raw[r_off + 1]]);
            let end = u16::from_le_bytes([raw[r_off + 2], raw[r_off + 3]]);
            let n_off = SCAN_GROUP_NAME_BASE + i * SCAN_GROUP_NAME_SIZE;
            let name = read_ascii_slot(&raw[n_off..n_off + SCAN_GROUP_NAME_SIZE]);
            ScanGroup {
                index: (i + 1) as u8,
                name,
                start_channel: start,
                end_channel: end,
            }
        })
        .collect()
}

fn decode_call_group_1_name(raw: &[u8]) -> Option<String> {
    let end = CALL_GROUP_1_NAME_BASE + CALL_GROUP_1_NAME_SIZE;
    if raw.len() < end {
        return None;
    }
    let s = read_ascii_slot(&raw[CALL_GROUP_1_NAME_BASE..end]);
    if s.is_empty() { None } else { Some(s) }
}

/// Read a NUL/`0xFF`-terminated ASCII slot as a trimmed
/// `String`. Shared by scan-group, call-group, and channel
/// name decoding.
fn read_ascii_slot(bytes: &[u8]) -> String {
    let s: String = bytes
        .iter()
        .take_while(|&&b| b != 0 && b != 0xFF)
        .map(|&b| b as char)
        .collect();
    s.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimum image size for the new flat-array decoder:
    /// covers channel data (0x140..0x3FB0), name area
    /// (0x3FBC..0x6EA0), FM broadcast (0x73E0..0x7408), and
    /// the call-group-1 name slot (0x766C..0x7678).
    const MIN_RAW: usize = 0x7700;

    /// Settings block (`0x0000..0x0084`) lifted verbatim from
    /// the FB-Radio baseline (`tmp/KG-Q336_FB_Radio_2024.kg`
    /// after `unmojibake`). Used as the starting point for
    /// single-field-delta tests so each one mirrors the
    /// corresponding capture in `~/Downloads/kg-re/`.
    #[rustfmt::skip]
    const BASELINE_SETTINGS: [u8; 0x84] = [
        0x01, 0x01, 0x00, 0x04, 0x05, 0x00, 0x01, 0x00,
        0x01, 0x00, 0x00, 0x0a, 0x0a, 0x00, 0x00, 0x03,
        0x00, 0x08, 0x08, 0x05, 0x00, 0x00, 0xfc, 0x01,
        0x00, 0x02, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00,
        0x00, 0x03, 0x00, 0x00, 0x0c, 0x00, 0x00, 0x00,
        0x00, 0x00, 0xff, 0xff, 0x00, 0xf8, 0xa0, 0x02,
        0x1f, 0x00, 0xff, 0xff, 0x00, 0xf8, 0x00, 0x00,
        0xa0, 0xfa, 0x00, 0x00, 0x40, 0x05, 0xa0, 0xfa,
        0xff, 0xff, 0x00, 0x00, 0x40, 0x05, 0xff, 0xff,
        0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30,
        0x30, 0x30, 0x30, 0x30, 0x03, 0x03, 0xfc, 0x01,
        0x65, 0x00, 0x05, 0x05, 0x05, 0x05, 0x00, 0x00,
        0x03, 0x00, 0x00, 0x00, 0x00, 0x01, 0x03, 0x01,
        0x04, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00,
        0x01, 0x0f, 0xf0, 0xf0, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00,
        0xf8, 0x02, 0x00, 0x00,
    ];

    /// Build a buffer with the baseline settings block in
    /// place; everything else zeroed. Tests then apply the
    /// single-byte delta from their capture and assert.
    fn buf_with_baseline_settings() -> Vec<u8> {
        let mut buf = vec![0u8; MIN_RAW];
        buf[0..0x84].copy_from_slice(&BASELINE_SETTINGS);
        buf
    }

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

    // ===== Settings-block tests =====
    //
    // Each test starts from `buf_with_baseline_settings()`,
    // overwrites the bytes that the corresponding capture
    // changed (per the diff sweep against `00_baseline.kg`),
    // and asserts on the decoded `Settings` field.

    #[test]
    fn decodes_baseline_settings() {
        let r = decode_channels(&buf_with_baseline_settings()).unwrap();
        let s = r.settings.expect("settings block fits in MIN_RAW");
        assert!(s.battery_save);
        assert!(s.beep);
        assert!(!s.gps_on);
        assert!(!s.auto_lock);
        assert_eq!(s.scan_mode, ScanMode::TimeOperated);
        assert_eq!(s.ptt_id, PttId::Off);
        assert_eq!(s.sidetone, Sidetone::Off);
        assert_eq!(s.startup_display, StartupDisplay::Image);
        assert_eq!(s.top_key, TopKey::Alarm);
        assert_eq!(s.vfo_squelch_a, 5);
        assert_eq!(s.vfo_squelch_b, 5);
        assert_eq!(s.priority_channel, 508);
        assert_eq!(s.theme, 3);
        assert_eq!(s.time_zone, 0x0c);
        assert_eq!(&s.mode_switch_password, b"000000");
        assert_eq!(&s.reset_password, b"000000");
        assert_eq!(s.ani_code_string(), "101");
        // SCC unset in baseline = six zero bytes, which the
        // digit-per-byte encoding renders as "000000". The
        // encoding can't distinguish that from a literal
        // SCC of "000000" — the CPS treats both as default.
        assert_eq!(s.scc_code_string(), "000000");
    }

    #[test]
    fn decodes_battery_save_off() {
        // Cap 26: 0x0001 = 0x01 → 0x00.
        let mut buf = buf_with_baseline_settings();
        buf[0x0001] = 0x00;
        let s = decode_channels(&buf).unwrap().settings.unwrap();
        assert!(!s.battery_save);
    }

    #[test]
    fn decodes_topkey_sos() {
        // Cap 27: 0x0064 = 0x00 → 0x01.
        let mut buf = buf_with_baseline_settings();
        buf[0x0064] = 0x01;
        let s = decode_channels(&buf).unwrap().settings.unwrap();
        assert_eq!(s.top_key, TopKey::Sos);
    }

    #[test]
    fn decodes_vfo_squelch_9() {
        // Cap 30: 0x005C..0x005E = `05 05` → `09 09`.
        let mut buf = buf_with_baseline_settings();
        buf[0x005C] = 0x09;
        buf[0x005D] = 0x09;
        let s = decode_channels(&buf).unwrap().settings.unwrap();
        assert_eq!(s.vfo_squelch_a, 9);
        assert_eq!(s.vfo_squelch_b, 9);
    }

    #[test]
    fn decodes_tot_15s() {
        // Cap 31: 0x0003 = 0x04 → 0x01.
        let mut buf = buf_with_baseline_settings();
        buf[0x0003] = 0x01;
        let s = decode_channels(&buf).unwrap().settings.unwrap();
        assert_eq!(s.tot, 1);
    }

    #[test]
    fn decodes_vox_5() {
        // Cap 32: 0x0005 = 0x00 → 0x05.
        let mut buf = buf_with_baseline_settings();
        buf[0x0005] = 0x05;
        let s = decode_channels(&buf).unwrap().settings.unwrap();
        assert_eq!(s.vox, 5);
    }

    #[test]
    fn decodes_scan_mode_co() {
        // Cap 34: 0x0009 = 0x00 → 0x01.
        let mut buf = buf_with_baseline_settings();
        buf[0x0009] = 0x01;
        let s = decode_channels(&buf).unwrap().settings.unwrap();
        assert_eq!(s.scan_mode, ScanMode::CarrierOperated);
    }

    #[test]
    fn decodes_ptt_id_bot() {
        // Cap 39: 0x000E = 0x00 → 0x01 (plus a theme refresh
        // we don't care about here).
        let mut buf = buf_with_baseline_settings();
        buf[0x000E] = 0x01;
        let s = decode_channels(&buf).unwrap().settings.unwrap();
        assert_eq!(s.ptt_id, PttId::Bot);
    }

    #[test]
    fn decodes_priority_channel_001() {
        // Cap 42: 0x0015..0x0018 = `00 fc 01` → `01 01 00`.
        // priority_channel u16 LE at 0x0016 = 0x0001 = 1.
        let mut buf = buf_with_baseline_settings();
        buf[0x0015] = 0x01;
        buf[0x0016] = 0x01;
        buf[0x0017] = 0x00;
        let s = decode_channels(&buf).unwrap().settings.unwrap();
        assert!(s.auto_lock);
        assert_eq!(s.priority_channel, 1);
    }

    #[test]
    fn decodes_gps_on() {
        // Cap 44: 0x0026 = 0x00 → 0x01.
        let mut buf = buf_with_baseline_settings();
        buf[0x0026] = 0x01;
        let s = decode_channels(&buf).unwrap().settings.unwrap();
        assert!(s.gps_on);
    }

    #[test]
    fn decodes_ani_max_scc_5s() {
        // Cap 50: 0x006E..0x007A all set to ANI=`09*6`,
        // SCC=`05*6`. The string helpers must render both
        // as 6-digit ASCII.
        let mut buf = buf_with_baseline_settings();
        buf[0x006E..0x0074].copy_from_slice(&[9; 6]);
        buf[0x0074..0x007A].copy_from_slice(&[5; 6]);
        let s = decode_channels(&buf).unwrap().settings.unwrap();
        assert_eq!(s.ani_code_string(), "999999");
        assert_eq!(s.scc_code_string(), "555555");
    }

    #[test]
    fn decodes_unknown_enum_value_as_other() {
        // Sanity: a non-0/1 byte for a 2-state enum lands in
        // `Other(_)` rather than panicking or being silently
        // mapped. Useful regression for future captures.
        let mut buf = buf_with_baseline_settings();
        buf[0x000E] = 0x05; // PTT-ID = some unknown variant
        let s = decode_channels(&buf).unwrap().settings.unwrap();
        assert_eq!(s.ptt_id, PttId::Other(5));
    }

    // ===== Scan group tests =====

    /// Place a 12-byte ASCII NUL-padded name at the given
    /// scan-group slot (0-based slot; `slot+1` matches the UI
    /// group number).
    fn place_scan_group_name(buf: &mut [u8], slot: usize, name: &[u8]) {
        let off = SCAN_GROUP_NAME_BASE + slot * SCAN_GROUP_NAME_SIZE;
        buf[off..off + SCAN_GROUP_NAME_SIZE].fill(0);
        buf[off..off + name.len()].copy_from_slice(name);
    }

    /// Place a `(start, end)` pair (u16 LE × 2) at the given
    /// scan-group slot.
    fn place_scan_group_range(buf: &mut [u8], slot: usize, start: u16, end: u16) {
        let off = SCAN_GROUP_RANGE_BASE + slot * 4;
        buf[off..off + 2].copy_from_slice(&start.to_le_bytes());
        buf[off + 2..off + 4].copy_from_slice(&end.to_le_bytes());
    }

    #[test]
    fn scan_groups_always_returns_ten_entries() {
        let r = decode_channels(&vec![0u8; MIN_RAW]).unwrap();
        assert_eq!(r.scan_groups.len(), SCAN_GROUP_COUNT);
        for (i, g) in r.scan_groups.iter().enumerate() {
            assert_eq!(g.index, (i + 1) as u8);
            assert_eq!(g.start_channel, 0);
            assert_eq!(g.end_channel, 0);
            assert!(g.name.is_empty());
        }
    }

    #[test]
    fn decodes_baseline_scan_group_ranges() {
        // The five named ranges in the FB-Radio baseline.
        let mut buf = vec![0u8; MIN_RAW];
        place_scan_group_range(&mut buf, 0, 501, 518);
        place_scan_group_range(&mut buf, 1, 55, 94);
        place_scan_group_range(&mut buf, 2, 101, 107);
        place_scan_group_range(&mut buf, 3, 201, 208);
        place_scan_group_range(&mut buf, 4, 301, 316);
        place_scan_group_name(&mut buf, 0, b"69 MHz");
        place_scan_group_name(&mut buf, 1, b"\xc5keri");
        place_scan_group_name(&mut buf, 2, b"Jakt");
        place_scan_group_name(&mut buf, 3, b"SRBR 444MHz");
        place_scan_group_name(&mut buf, 4, b"PMR 446MHz");
        let r = decode_channels(&buf).unwrap();
        let g: Vec<_> = r
            .scan_groups
            .iter()
            .map(|g| (g.index, g.name.as_str(), g.start_channel, g.end_channel))
            .collect();
        assert_eq!(g[0], (1, "69 MHz", 501, 518));
        assert_eq!(g[1].0, 2);
        // Slot 1 name is "\xc5keri" — Latin-1 Å. The decoder
        // maps each byte to its codepoint, so we get a U+00C5
        // followed by "keri". Don't compare as &str literal —
        // just check the start/end + length.
        assert!(g[1].1.starts_with('\u{00C5}'));
        assert_eq!(g[1].2, 55);
        assert_eq!(g[1].3, 94);
        assert_eq!(g[2], (3, "Jakt", 101, 107));
        assert_eq!(g[3], (4, "SRBR 444MHz", 201, 208));
        assert_eq!(g[4], (5, "PMR 446MHz", 301, 316));
        // Slots 6..10 stay at default (no name, zero range).
        for slot_idx in 5..SCAN_GROUP_COUNT {
            let g = &r.scan_groups[slot_idx];
            assert!(g.name.is_empty());
            assert_eq!(g.start_channel, 0);
            assert_eq!(g.end_channel, 0);
        }
    }

    #[test]
    fn decodes_scangrp1_rename() {
        // Cap 28: 0x72A0..0x72A8 = `36 39 20 4d 48 7a 00 00`
        //   → `54 45 53 54 47 52 50 31` ("TESTGRP1").
        let mut buf = vec![0u8; MIN_RAW];
        place_scan_group_name(&mut buf, 0, b"TESTGRP1");
        let r = decode_channels(&buf).unwrap();
        assert_eq!(r.scan_groups[0].name, "TESTGRP1");
        assert_eq!(r.scan_groups[0].index, 1);
    }

    // ===== Call-group-1 name tests =====

    #[test]
    fn decodes_call_group_1_baseline() {
        // Baseline: 0x766C = "Allanrop\0\0\0\0".
        let mut buf = vec![0u8; MIN_RAW];
        buf[0x766C..0x766C + 8].copy_from_slice(b"Allanrop");
        let r = decode_channels(&buf).unwrap();
        assert_eq!(r.call_group_1_name.as_deref(), Some("Allanrop"));
    }

    #[test]
    fn decodes_callgrp1_rename() {
        // Cap 29: 0x766C..0x7674 = `41 6c 6c 61 6e 72 6f 70`
        //   → `54 45 53 54 43 41 4c 4c` ("TESTCALL").
        let mut buf = vec![0u8; MIN_RAW];
        buf[0x766C..0x766C + 8].copy_from_slice(b"TESTCALL");
        let r = decode_channels(&buf).unwrap();
        assert_eq!(r.call_group_1_name.as_deref(), Some("TESTCALL"));
    }

    #[test]
    fn call_group_1_blank_returns_none() {
        let buf = vec![0u8; MIN_RAW];
        let r = decode_channels(&buf).unwrap();
        assert!(r.call_group_1_name.is_none());
    }
}
