//! EEPROM-image decoding: parses the on-radio channel records into
//! narm [`Channel`] values. Pure and testable — no I/O.
//!
//! Channel records (16 bytes each) live at EEPROM `0x0000`; channel
//! names (16 bytes ASCII, NUL/0xFF padded) live at `0xf50`.

use zerocopy::byteorder::little_endian::U32;
use zerocopy::{FromBytes, Immutable, KnownLayout};

use crate::channel::{Bandwidth, Channel, Mode, Power};

use super::error::UvK5Error;
use super::wire::EEPROM_SIZE;

// -------- channel-area constants --------

/// Channel record block — 200 channels × 16 bytes at 0x0000.
const CHANNEL_COUNT: usize = 200;
const CHANNEL_SIZE: usize = 16;
const CHANNEL_NAME_BASE: usize = 0xF50;
const CHANNEL_NAME_SIZE: usize = 16;

/// 50-entry CTCSS table — same indexing as CHIRP's `chirp_common.TONES`.
const CTCSS_TONES: [f32; 50] = [
    67.0, 69.3, 71.9, 74.4, 77.0, 79.7, 82.5, 85.4, 88.5, 91.5, 94.8, 97.4, 100.0, 103.5, 107.2,
    110.9, 114.8, 118.8, 123.0, 127.3, 131.8, 136.5, 141.3, 146.2, 151.4, 156.7, 159.8, 162.2,
    165.5, 167.9, 171.3, 173.8, 177.3, 179.9, 183.5, 186.2, 189.9, 192.8, 196.6, 199.5, 203.5,
    206.5, 210.7, 218.1, 225.7, 229.1, 233.6, 241.8, 250.3, 254.1,
];

/// 104-entry DCS code table — `chirp_common.DTCS_CODES`.
const DTCS_CODES: [u16; 104] = [
    23, 25, 26, 31, 32, 36, 43, 47, 51, 53, 54, 65, 71, 72, 73, 74, 114, 115, 116, 122, 125, 131,
    132, 134, 143, 145, 152, 155, 156, 162, 165, 172, 174, 205, 212, 223, 225, 226, 243, 244, 245,
    246, 251, 252, 255, 261, 263, 265, 266, 271, 274, 306, 311, 315, 325, 331, 332, 343, 346, 351,
    356, 364, 365, 371, 411, 412, 413, 423, 431, 432, 445, 446, 452, 454, 455, 462, 464, 465, 466,
    503, 506, 516, 523, 526, 532, 546, 565, 606, 612, 624, 627, 631, 632, 654, 662, 664, 703, 712,
    723, 731, 732, 734, 743, 754,
];

// -------- on-wire channel layout --------

/// On-wire layout of a single channel record (16 bytes at EEPROM
/// offset `0x0000 + slot * 16`). Mirrors CHIRP's `MEM_FORMAT` struct
/// in `uvk5.py`; field order and sizes are load-bearing — do not
/// reorder. Multi-byte fields are little-endian.
///
/// Bit-packed bytes (`codeflags`, `flags1`, `flags2`, `dtmf_flags`)
/// stay as `u8` here; the decoder unpacks the individual bits below.
#[repr(C)]
#[derive(FromBytes, Immutable, KnownLayout, Debug, Clone, Copy)]
struct ChannelRecord {
    /// RX frequency in 10 Hz units.
    freq: U32,
    /// TX offset in 10 Hz units (sign comes from `flags1.shift`).
    offset: U32,
    rxcode: u8,
    txcode: u8,
    /// `tx_codeflag:4 | rx_codeflag:4` — 0=none, 1=CTCSS, 2=DCS,
    /// 3=DCS-reversed.
    codeflags: u8,
    /// `?:3 | enable_am:1 | ?:1 | is_in_scanlist:1 | shift:2`.
    flags1: u8,
    /// `?:3 | bclo:1 | txpower:2 | bandwidth:1 | freq_reverse:1`.
    flags2: u8,
    dtmf_flags: u8,
    step: u8,
    scrambler: u8,
}

impl ChannelRecord {
    fn is_empty(&self) -> bool {
        let f = self.freq.get();
        f == 0 || f == 0xFFFF_FFFF
    }

    fn rx_hz(&self) -> u64 {
        self.freq.get() as u64 * 10
    }

    fn offset_hz(&self) -> u64 {
        self.offset.get() as u64 * 10
    }

    /// Signed TX shift derived from the `shift` bits + offset.
    /// `0b01` → +, `0b10` → −, anything else → 0.
    fn shift_hz(&self) -> i64 {
        let off = self.offset_hz() as i64;
        match self.flags1 & 0b11 {
            0b01 => off,
            0b10 => -off,
            _ => 0,
        }
    }

    fn enable_am(&self) -> bool {
        (self.flags1 >> 4) & 1 != 0
    }

    fn bandwidth(&self) -> Bandwidth {
        if (self.flags2 >> 1) & 1 == 0 {
            Bandwidth::Wide
        } else {
            Bandwidth::Narrow
        }
    }

    fn power(&self) -> Power {
        match (self.flags2 >> 2) & 0b11 {
            0b10 => Power::High,
            0b01 => Power::Mid,
            _ => Power::Low,
        }
    }

    fn tx_codeflag(&self) -> u8 {
        (self.codeflags >> 4) & 0x0F
    }

    fn rx_codeflag(&self) -> u8 {
        self.codeflags & 0x0F
    }
}

// -------- public decode API --------

/// Decode the 200 user-channel slots out of a complete EEPROM dump.
/// Empty slots (freq 0 or 0xFFFF_FFFF) are skipped. AM channels are
/// emitted as [`Mode::Am`]. Warnings vec is reserved for future
/// per-channel skips; currently always empty.
pub struct DecodeReport {
    pub channels: Vec<Channel>,
    pub warnings: Vec<String>,
}

pub fn decode_channels(eeprom: &[u8]) -> Result<DecodeReport, UvK5Error> {
    if eeprom.len() < EEPROM_SIZE {
        return Err(UvK5Error::ShortEeprom { got: eeprom.len() });
    }
    let mut channels = Vec::new();
    let warnings = Vec::new();

    for i in 0..CHANNEL_COUNT {
        let off = i * CHANNEL_SIZE;
        let rec = ChannelRecord::ref_from_bytes(&eeprom[off..off + CHANNEL_SIZE])
            .expect("CHANNEL_SIZE matches ChannelRecord size_of");
        if rec.is_empty() {
            continue;
        }

        let (tone_tx_hz, tone_rx_hz, dcs_code) =
            decode_tones(rec.tx_codeflag(), rec.txcode, rec.rx_codeflag(), rec.rxcode);
        let bandwidth = rec.bandwidth();
        let mode = if rec.enable_am() {
            Mode::Am {
                bandwidth,
                tone_tx_hz,
                tone_rx_hz,
                dcs_code,
            }
        } else {
            Mode::Fm {
                bandwidth,
                tone_tx_hz,
                tone_rx_hz,
                dcs_code,
                call_group: None,
            }
        };

        let name = read_channel_name(eeprom, i);
        channels.push(Channel {
            name: if name.is_empty() {
                format!("CH{:03}", i + 1)
            } else {
                name
            },
            rx_hz: rec.rx_hz(),
            shift_hz: rec.shift_hz(),
            power: rec.power(),
            scan: true,
            mode,
            source: None,
        });
    }
    Ok(DecodeReport { channels, warnings })
}

fn read_channel_name(eeprom: &[u8], idx: usize) -> String {
    let off = CHANNEL_NAME_BASE + idx * CHANNEL_NAME_SIZE;
    let raw = &eeprom[off..off + CHANNEL_NAME_SIZE];
    raw.iter()
        .take_while(|&&b| b != 0 && b != 0xFF)
        .map(|&b| b as char)
        .collect::<String>()
        .trim()
        .to_string()
}

fn decode_tones(
    tx_flag: u8,
    txcode: u8,
    rx_flag: u8,
    rxcode: u8,
) -> (Option<f32>, Option<f32>, Option<u16>) {
    // CHIRP TMODES: 0=None, 1=CTCSS, 2=DCS, 3=DCS-inverted (treated as DCS).
    // For DCS we surface the rx_dcs since narm only tracks one dcs_code per
    // channel; if tx and rx differ that's a known v1 lossiness.
    let mut tone_tx = None;
    let mut tone_rx = None;
    let mut dcs = None;
    match tx_flag {
        1 => tone_tx = CTCSS_TONES.get(txcode as usize).copied(),
        2 | 3 => dcs = DTCS_CODES.get(txcode as usize).copied(),
        _ => {}
    }
    match rx_flag {
        1 => tone_rx = CTCSS_TONES.get(rxcode as usize).copied(),
        2 | 3 if dcs.is_none() => dcs = DTCS_CODES.get(rxcode as usize).copied(),
        _ => {}
    }
    (tone_tx, tone_rx, dcs)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a fake 8 KiB EEPROM with a single populated channel at slot 0
    /// (2 m repeater output 145.700 MHz, -600 kHz shift, CTCSS 88.5 Hz
    /// TX+RX, low power, narrow bandwidth). Slot 1 is empty (freq=0).
    fn fake_eeprom() -> Vec<u8> {
        let mut e = vec![0u8; EEPROM_SIZE];
        // Channel 0 at 0x0000.
        let freq_10hz: u32 = 14_570_000; // 145.7 MHz
        let offset_10hz: u32 = 60_000; // 600 kHz
        e[0..4].copy_from_slice(&freq_10hz.to_le_bytes());
        e[4..8].copy_from_slice(&offset_10hz.to_le_bytes());
        // 88.5 Hz is CTCSS index 8 (CTCSS_TONES[8] == 88.5).
        e[8] = 8; // rxcode
        e[9] = 8; // txcode
        e[10] = (1u8 << 4) | 1; // tx_flag=1, rx_flag=1
        e[11] = 0b10; // shift = minus
        // power = Low (txpower bits 0b00) | bandwidth = Narrow (bit 1 set)
        e[12] = 0b0000_0010;
        // Channel 0 name "GB3WE" at 0xf50.
        let name = b"GB3WE";
        e[CHANNEL_NAME_BASE..CHANNEL_NAME_BASE + name.len()].copy_from_slice(name);
        e
    }

    #[test]
    fn decode_one_channel_from_fake_eeprom() {
        let e = fake_eeprom();
        let report = decode_channels(&e).unwrap();
        assert_eq!(report.channels.len(), 1);
        let ch = &report.channels[0];
        assert_eq!(ch.name, "GB3WE");
        assert_eq!(ch.rx_hz, 145_700_000);
        assert_eq!(ch.shift_hz, -600_000);
        assert_eq!(ch.power, Power::Low);
        match &ch.mode {
            Mode::Fm {
                bandwidth,
                tone_tx_hz,
                tone_rx_hz,
                dcs_code,
                ..
            } => {
                assert_eq!(*bandwidth, Bandwidth::Narrow);
                assert_eq!(*tone_tx_hz, Some(88.5));
                assert_eq!(*tone_rx_hz, Some(88.5));
                assert!(dcs_code.is_none());
            }
            other => panic!("expected FM mode, got {other:?}"),
        }
    }

    #[test]
    fn empty_channel_slots_are_skipped() {
        let e = vec![0u8; EEPROM_SIZE];
        let report = decode_channels(&e).unwrap();
        assert!(report.channels.is_empty());
    }

    #[test]
    fn am_channel_decoded_as_am_mode() {
        // 121.250 MHz aviation, AM mode, no tones.
        let mut e = vec![0u8; EEPROM_SIZE];
        let freq_10hz: u32 = 12_125_000;
        e[0..4].copy_from_slice(&freq_10hz.to_le_bytes());
        e[11] = 1 << 4; // enable_am bit
        let report = decode_channels(&e).unwrap();
        assert_eq!(report.channels.len(), 1);
        assert!(report.warnings.is_empty());
        let ch = &report.channels[0];
        assert_eq!(ch.rx_hz, 121_250_000);
        assert!(matches!(
            ch.mode,
            Mode::Am {
                bandwidth: Bandwidth::Wide,
                tone_tx_hz: None,
                tone_rx_hz: None,
                dcs_code: None,
            }
        ));
    }

    #[test]
    fn am_channel_preserves_bandwidth_and_tones() {
        // Aviation channel with narrow AM (rare but representable)
        // and a TX CTCSS — verify both fields make it through.
        let mut e = vec![0u8; EEPROM_SIZE];
        let freq_10hz: u32 = 12_125_000;
        e[0..4].copy_from_slice(&freq_10hz.to_le_bytes());
        e[9] = 8; // CTCSS index 8 = 88.5 Hz
        e[10] = 1 << 4; // tx_flag = CTCSS
        e[11] = 1 << 4; // enable_am
        e[12] = 0b0000_0010; // bandwidth bit set = narrow
        let r = decode_channels(&e).unwrap();
        match &r.channels[0].mode {
            Mode::Am {
                bandwidth,
                tone_tx_hz,
                ..
            } => {
                assert_eq!(*bandwidth, Bandwidth::Narrow);
                assert_eq!(*tone_tx_hz, Some(88.5));
            }
            other => panic!("expected AM, got {other:?}"),
        }
    }

    // ===== regression tests captured from the live UV-K5(8) =====
    //
    // Bytes below were read off a real radio with `narm radio read
    // --format raw` and verified against the radio's UI. They lock
    // the wire-format and decoder against future drift.

    /// Captured from a real UV-K5(8): channel slot 0 = SK6RFQ on 2 m,
    /// 145.650 MHz, -600 kHz shift, CTCSS 114.8 Hz on TX only, high
    /// power, wide bandwidth.
    const REAL_SLOT_0_BYTES: [u8; 16] = [
        0x88, 0x3E, 0xDE, 0x00, 0x60, 0xEA, 0x00, 0x00, 0x00, 0x10, 0x10, 0x02, 0x08, 0x00, 0x04,
        0x00,
    ];
    const REAL_SLOT_0_NAME: [u8; 16] = [
        0x53, 0x4B, 0x36, 0x52, 0x46, 0x51, 0x20, 0x20, 0x20, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00,
    ];

    /// Slot 1 = SK6RFQ on 70 cm, 434.650 MHz, -2 MHz shift, CTCSS
    /// 114.8 Hz TX only, high power, wide bandwidth.
    const REAL_SLOT_1_BYTES: [u8; 16] = [
        0x28, 0x39, 0x97, 0x02, 0x40, 0x0D, 0x03, 0x00, 0x00, 0x10, 0x10, 0x02, 0x08, 0x00, 0x04,
        0x00,
    ];

    /// Slot 2 = PMR446 K1, 446.00625 MHz, no shift, no tone, low
    /// power, wide bandwidth.
    const REAL_SLOT_2_BYTES: [u8; 16] = [
        0x31, 0x8D, 0xA8, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04,
        0x00,
    ];
    const REAL_SLOT_2_NAME: [u8; 16] = [
        0x4B, 0x31, 0x20, 0x20, 0x20, 0x20, 0x20, 0x20, 0x20, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00,
    ];

    fn fake_eeprom_with(slot0: &[u8], slot1: Option<&[u8]>, name0: &[u8]) -> Vec<u8> {
        let mut e = vec![0u8; EEPROM_SIZE];
        e[0..16].copy_from_slice(slot0);
        if let Some(s1) = slot1 {
            e[16..32].copy_from_slice(s1);
        }
        e[CHANNEL_NAME_BASE..CHANNEL_NAME_BASE + 16].copy_from_slice(name0);
        e
    }

    #[test]
    fn decodes_real_uvk5_slot_0_sk6rfq_2m() {
        let e = fake_eeprom_with(&REAL_SLOT_0_BYTES, None, &REAL_SLOT_0_NAME);
        let r = decode_channels(&e).unwrap();
        assert_eq!(r.channels.len(), 1);
        let ch = &r.channels[0];
        assert_eq!(ch.name, "SK6RFQ");
        assert_eq!(ch.rx_hz, 145_650_000);
        assert_eq!(ch.shift_hz, -600_000);
        assert_eq!(ch.power, Power::High);
        match &ch.mode {
            Mode::Fm {
                bandwidth,
                tone_tx_hz,
                tone_rx_hz,
                dcs_code,
                ..
            } => {
                assert_eq!(*bandwidth, Bandwidth::Wide);
                assert_eq!(*tone_tx_hz, Some(114.8));
                assert_eq!(*tone_rx_hz, None);
                assert!(dcs_code.is_none());
            }
            other => panic!("expected FM, got {other:?}"),
        }
    }

    #[test]
    fn decodes_real_uvk5_slot_1_sk6rfq_70cm() {
        let mut e = vec![0u8; EEPROM_SIZE];
        e[16..32].copy_from_slice(&REAL_SLOT_1_BYTES);
        // No name set → slot uses synthetic CH002.
        let r = decode_channels(&e).unwrap();
        assert_eq!(r.channels.len(), 1);
        let ch = &r.channels[0];
        assert_eq!(ch.name, "CH002");
        assert_eq!(ch.rx_hz, 434_650_000);
        assert_eq!(ch.shift_hz, -2_000_000);
        assert_eq!(ch.power, Power::High);
    }

    #[test]
    fn decodes_real_uvk5_slot_2_pmr446_k1() {
        let mut e = vec![0u8; EEPROM_SIZE];
        e[32..48].copy_from_slice(&REAL_SLOT_2_BYTES);
        e[CHANNEL_NAME_BASE + 32..CHANNEL_NAME_BASE + 48].copy_from_slice(&REAL_SLOT_2_NAME);
        let r = decode_channels(&e).unwrap();
        assert_eq!(r.channels.len(), 1);
        let ch = &r.channels[0];
        assert_eq!(ch.name, "K1");
        assert_eq!(ch.rx_hz, 446_006_250);
        assert_eq!(ch.shift_hz, 0);
        assert_eq!(ch.power, Power::Low);
        match &ch.mode {
            Mode::Fm {
                bandwidth,
                tone_tx_hz,
                tone_rx_hz,
                dcs_code,
                ..
            } => {
                assert_eq!(*bandwidth, Bandwidth::Wide);
                assert!(tone_tx_hz.is_none());
                assert!(tone_rx_hz.is_none());
                assert!(dcs_code.is_none());
            }
            _ => panic!("expected FM"),
        }
    }

    fn one_channel_with_flags(flags1: u8, flags2: u8, offset_10hz: u32) -> Channel {
        let mut e = vec![0u8; EEPROM_SIZE];
        let freq_10hz: u32 = 14_565_000; // 145.65 MHz
        e[0..4].copy_from_slice(&freq_10hz.to_le_bytes());
        e[4..8].copy_from_slice(&offset_10hz.to_le_bytes());
        e[11] = flags1;
        e[12] = flags2;
        decode_channels(&e)
            .unwrap()
            .channels
            .into_iter()
            .next()
            .unwrap()
    }

    #[test]
    fn shift_direction_matrix() {
        // shift bits live in the low 2 of flags1: 00=none, 01=plus, 10=minus.
        let off_10hz: u32 = 60_000; // 600 kHz
        assert_eq!(one_channel_with_flags(0b00, 0, off_10hz).shift_hz, 0);
        assert_eq!(one_channel_with_flags(0b01, 0, off_10hz).shift_hz, 600_000);
        assert_eq!(one_channel_with_flags(0b10, 0, off_10hz).shift_hz, -600_000);
    }

    #[test]
    fn power_levels_matrix() {
        // txpower is bits 2..3 of flags2 (0b00=Low, 0b01=Mid, 0b10=High).
        assert_eq!(one_channel_with_flags(0, 0b0000_0000, 0).power, Power::Low);
        assert_eq!(one_channel_with_flags(0, 0b0000_0100, 0).power, Power::Mid);
        assert_eq!(one_channel_with_flags(0, 0b0000_1000, 0).power, Power::High);
    }

    #[test]
    fn bandwidth_wide_and_narrow() {
        // bandwidth is bit 1 of flags2 (0=wide, 1=narrow).
        let wide = one_channel_with_flags(0, 0b0000_0000, 0);
        let narrow = one_channel_with_flags(0, 0b0000_0010, 0);
        assert!(matches!(
            wide.mode,
            Mode::Fm {
                bandwidth: Bandwidth::Wide,
                ..
            }
        ));
        assert!(matches!(
            narrow.mode,
            Mode::Fm {
                bandwidth: Bandwidth::Narrow,
                ..
            }
        ));
    }

    fn decode_with_codes(tx_flag: u8, txcode: u8, rx_flag: u8, rxcode: u8) -> Channel {
        let mut e = vec![0u8; EEPROM_SIZE];
        let freq_10hz: u32 = 14_565_000;
        e[0..4].copy_from_slice(&freq_10hz.to_le_bytes());
        e[8] = rxcode;
        e[9] = txcode;
        e[10] = (tx_flag << 4) | (rx_flag & 0x0F);
        decode_channels(&e)
            .unwrap()
            .channels
            .into_iter()
            .next()
            .unwrap()
    }

    #[test]
    fn ctcss_index_boundaries() {
        // Index 0 = 67.0 Hz, index 49 = 254.1 Hz, index 50 = out of range → no tone.
        let lo = decode_with_codes(1, 0, 0, 0);
        let hi = decode_with_codes(1, 49, 0, 0);
        let oob = decode_with_codes(1, 50, 0, 0);
        if let Mode::Fm { tone_tx_hz, .. } = lo.mode {
            assert_eq!(tone_tx_hz, Some(67.0));
        } else {
            panic!()
        }
        if let Mode::Fm { tone_tx_hz, .. } = hi.mode {
            assert_eq!(tone_tx_hz, Some(254.1));
        } else {
            panic!()
        }
        if let Mode::Fm { tone_tx_hz, .. } = oob.mode {
            assert!(
                tone_tx_hz.is_none(),
                "out-of-range CTCSS index returns None"
            );
        } else {
            panic!()
        }
    }

    #[test]
    fn dcs_index_boundaries() {
        // Index 0 = 23, index 103 = 754, index 104 = out of range → None.
        let lo = decode_with_codes(2, 0, 0, 0);
        let hi = decode_with_codes(2, 103, 0, 0);
        let oob = decode_with_codes(2, 104, 0, 0);
        if let Mode::Fm { dcs_code, .. } = lo.mode {
            assert_eq!(dcs_code, Some(23));
        } else {
            panic!()
        }
        if let Mode::Fm { dcs_code, .. } = hi.mode {
            assert_eq!(dcs_code, Some(754));
        } else {
            panic!()
        }
        if let Mode::Fm { dcs_code, .. } = oob.mode {
            assert!(dcs_code.is_none());
        } else {
            panic!()
        }
    }

    #[test]
    fn dcs_flag_3_reversed_polarity_treated_as_dcs() {
        // Flag value 3 (DCS-reversed) shares the DCS table; we surface it
        // as the same DCS code as flag 2 since narm has no polarity field.
        let ch = decode_with_codes(3, 5, 0, 0);
        if let Mode::Fm { dcs_code, .. } = ch.mode {
            assert_eq!(dcs_code, Some(36)); // DTCS_CODES[5] == 36
        } else {
            panic!()
        }
    }

    #[test]
    fn tx_only_ctcss_doesnt_set_rx_tone() {
        let ch = decode_with_codes(1, 8, 0, 0); // tx CTCSS index 8 = 88.5
        if let Mode::Fm {
            tone_tx_hz,
            tone_rx_hz,
            ..
        } = ch.mode
        {
            assert_eq!(tone_tx_hz, Some(88.5));
            assert!(tone_rx_hz.is_none());
        }
    }

    #[test]
    fn rx_only_ctcss_doesnt_set_tx_tone() {
        let ch = decode_with_codes(0, 0, 1, 8);
        if let Mode::Fm {
            tone_tx_hz,
            tone_rx_hz,
            ..
        } = ch.mode
        {
            assert!(tone_tx_hz.is_none());
            assert_eq!(tone_rx_hz, Some(88.5));
        }
    }

    #[test]
    fn tx_and_rx_different_ctcss_both_recorded() {
        let ch = decode_with_codes(1, 0, 1, 49); // tx 67.0, rx 254.1
        if let Mode::Fm {
            tone_tx_hz,
            tone_rx_hz,
            ..
        } = ch.mode
        {
            assert_eq!(tone_tx_hz, Some(67.0));
            assert_eq!(tone_rx_hz, Some(254.1));
        }
    }

    fn decode_with_name(name_bytes: &[u8]) -> Channel {
        let mut e = vec![0u8; EEPROM_SIZE];
        let freq_10hz: u32 = 14_565_000;
        e[0..4].copy_from_slice(&freq_10hz.to_le_bytes());
        e[CHANNEL_NAME_BASE..CHANNEL_NAME_BASE + name_bytes.len()].copy_from_slice(name_bytes);
        decode_channels(&e)
            .unwrap()
            .channels
            .into_iter()
            .next()
            .unwrap()
    }

    #[test]
    fn channel_name_nul_terminated() {
        let ch = decode_with_name(b"GB3WE\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00");
        assert_eq!(ch.name, "GB3WE");
    }

    #[test]
    fn channel_name_ff_terminated() {
        let ch = decode_with_name(b"GB3WE\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff");
        assert_eq!(ch.name, "GB3WE");
    }

    #[test]
    fn channel_name_full_16_chars_no_terminator() {
        // 16 printable chars exactly — no NUL/FF in the slot.
        let ch = decode_with_name(b"ABCDEFGHIJKLMNOP");
        assert_eq!(ch.name, "ABCDEFGHIJKLMNOP");
    }

    #[test]
    fn channel_name_trailing_spaces_trimmed() {
        // CHIRP/UV-K5 pads short names with spaces.
        let ch = decode_with_name(b"K1      \x00\x00\x00\x00\x00\x00\x00\x00");
        assert_eq!(ch.name, "K1");
    }

    #[test]
    fn channel_name_blank_falls_back_to_synthetic() {
        let ch =
            decode_with_name(b"\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00");
        assert_eq!(ch.name, "CH001");
    }

    #[test]
    fn dcs_code_decoded() {
        let mut e = vec![0u8; EEPROM_SIZE];
        let freq_10hz: u32 = 14_570_000;
        e[0..4].copy_from_slice(&freq_10hz.to_le_bytes());
        // DTCS_CODES[5] == 36.
        e[8] = 5;
        e[9] = 5;
        e[10] = (2u8 << 4) | 2; // tx and rx both DCS (flag=2)
        let report = decode_channels(&e).unwrap();
        assert_eq!(report.channels.len(), 1);
        match &report.channels[0].mode {
            Mode::Fm {
                tone_tx_hz,
                tone_rx_hz,
                dcs_code,
                ..
            } => {
                assert!(tone_tx_hz.is_none());
                assert!(tone_rx_hz.is_none());
                assert_eq!(*dcs_code, Some(36));
            }
            _ => panic!("expected FM mode"),
        }
    }
}
