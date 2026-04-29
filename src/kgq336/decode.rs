//! Channel decoding for the KG-Q332/Q336.
//!
//! Reverse-engineered from `.kg` files saved by the vendor's
//! CPS via byte-diffing single-field changes. v0.3 decodes:
//! frequency (rx + tx), channel name, power, bandwidth,
//! CTCSS TX, and scan-add flag. Still TBD: CTCSS RX, DCS,
//! repeater-shift sign — gated on tier-2 sample files.
//!
//! ## On-disk record (16 bytes)
//!
//! ```text
//!   bytes 0..4   rx_freq:    u32 LE × 10 Hz
//!   bytes 4..8   tx_freq:    u32 LE × 10 Hz  (0 → simplex)
//!   bytes 8..10  tone_other: u16 LE          (CTCSS RX or DCS — TBD)
//!   bytes 10..12 tone_tx:    u16 LE          (low 15 bits = freq×10
//!                                             dHz, bit 15 = enable)
//!   byte 12      power:      u8              (0=low, 1=mid, 2=high, 3=ultrahigh)
//!   byte 13      flags1:     u8              (bit 0 = wide, bit 5 = factory-preset)
//!   byte 14      category:   u8              (1=simplex/69M, 2=86MHz,
//!                                             3=SRBR-VHF, 4=SRBR-UHF, 5=PMR)
//!   byte 15      flags2:     u8              (5 for band scanner, 0 otherwise)
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
    /// CTCSS RX or DCS — layout unknown until tier-2 saves
    /// land. All-zero in the tier-1 samples we have.
    _tone_other: U16,
    /// CTCSS TX: low 15 bits = tone × 10 (deci-Hz), bit 15 =
    /// enable. `0` means no tone.
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

/// CTCSS-TX enable flag in the high bit of `tone_tx_raw`.
const TONE_ENABLE_BIT: u16 = 0x8000;
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

    fn ctcss_tx_hz(&self) -> Option<f32> {
        let v = self.tone_tx_raw.get();
        if v & TONE_ENABLE_BIT == 0 {
            return None;
        }
        let dhz = (v & !TONE_ENABLE_BIT) as f32;
        let hz = dhz / 10.0;
        if (CTCSS_MIN_HZ..=CTCSS_MAX_HZ).contains(&hz) {
            Some(hz)
        } else {
            // Out-of-range — we're probably looking at random
            // bytes that happened to have the 0x8000 bit set.
            None
        }
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
    Channel {
        name,
        rx_hz: rec.rx_hz(),
        shift_hz: rec.shift_hz(),
        power: rec.power(),
        scan: rec.scan_add(),
        mode: Mode::Fm {
            bandwidth: rec.bandwidth(),
            tone_tx_hz: rec.ctcss_tx_hz(),
            tone_rx_hz: None, // bytes 8..10 layout TBD (tier 2)
            dcs_code: None,
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
