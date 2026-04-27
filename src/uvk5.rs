//! Quansheng UV-K5 / UV-K5(8) wire protocol — read-side.
//!
//! Reverse-engineered from CHIRP's `chirp/drivers/uvk5.py`
//! (kk7ds/chirp on GitHub, MIT licence). This module implements the
//! download path only — handshake, EEPROM block reads, and channel
//! decoding into narm [`Channel`]s. No write/upload.
//!
//! Wire framing on the cable (38400 8N1):
//!
//! ```text
//!   send: [0xAB 0xCD] [len:u8] [0x00]  XOR(payload + CRC16-XMODEM)  [0xDC 0xBA]
//!   recv: [0xAB 0xCD] [len:u8] [0x00]  XOR(payload)  [XOR(CRC16):2]  [0xDC 0xBA]
//! ```
//!
//! CRC is sent on both sides but the radio does not verify the
//! request's CRC and CHIRP does not verify the reply's, so we follow
//! suit and ignore the inbound CRC.
//!
//! Channel records (16 bytes each) live at EEPROM `0x0000`; channel
//! names (16 bytes ASCII, NUL/0xFF padded) live at `0xf50`.

use std::time::Duration;

use serialport::SerialPort;

use crate::channel::{Bandwidth, Channel, Mode, Power};

const BAUD: u32 = 38400;
const HEADER_MAGIC: [u8; 2] = [0xAB, 0xCD];
const FOOTER_MAGIC: [u8; 2] = [0xDC, 0xBA];

/// Cyclic XOR key used by the radio for protocol obfuscation.
/// Verbatim from CHIRP's `xorarr` table (uvk5.py).
const XOR_KEY: [u8; 16] = [
    22, 108, 20, 230, 46, 145, 13, 64, 33, 53, 213, 64, 19, 3, 233, 128,
];

const HELLO: [u8; 8] = [0x14, 0x05, 0x04, 0x00, 0x6A, 0x39, 0x57, 0x64];

/// 8 KiB of EEPROM, read in 128-byte chunks.
pub const EEPROM_SIZE: usize = 0x2000;
const READ_BLOCK: u8 = 0x80;

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

#[derive(thiserror::Error, Debug)]
pub enum UvK5Error {
    #[error("serial I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("serial port: {0}")]
    Serial(#[from] serialport::Error),
    #[error("bad reply header: {0:02x?}")]
    BadHeader([u8; 4]),
    #[error("bad reply footer: {0:02x?}")]
    BadFooter([u8; 4]),
    #[error("radio did not respond to hello")]
    NoHelloReply,
    #[error("eeprom too short: got {got} bytes, expected {EEPROM_SIZE}")]
    ShortEeprom { got: usize },
}

// -------- pure protocol primitives (no I/O) --------

pub fn xor_inplace(data: &mut [u8]) {
    for (i, b) in data.iter_mut().enumerate() {
        *b ^= XOR_KEY[i % XOR_KEY.len()];
    }
}

pub fn xor(data: &[u8]) -> Vec<u8> {
    let mut out = data.to_vec();
    xor_inplace(&mut out);
    out
}

/// CRC16/XMODEM: poly 0x1021, init 0x0000, no reflection, no XOR-out.
pub fn crc16_xmodem(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

fn build_frame(payload: &[u8]) -> Vec<u8> {
    let crc = crc16_xmodem(payload);
    let mut body = payload.to_vec();
    body.push((crc & 0xFF) as u8);
    body.push((crc >> 8) as u8);
    xor_inplace(&mut body);

    let mut frame = Vec::with_capacity(4 + body.len() + 2);
    frame.extend_from_slice(&HEADER_MAGIC);
    frame.push(payload.len() as u8);
    frame.push(0);
    frame.extend_from_slice(&body);
    frame.extend_from_slice(&FOOTER_MAGIC);
    frame
}

// -------- I/O --------

pub fn open_port(port: &str) -> Result<Box<dyn SerialPort>, UvK5Error> {
    serialport::new(port, BAUD)
        .timeout(Duration::from_millis(2000))
        .open()
        .map_err(Into::into)
}

fn read_exact(port: &mut dyn SerialPort, n: usize) -> Result<Vec<u8>, UvK5Error> {
    let mut buf = vec![0u8; n];
    port.read_exact(&mut buf)?;
    Ok(buf)
}

fn send(port: &mut dyn SerialPort, payload: &[u8]) -> Result<(), UvK5Error> {
    let frame = build_frame(payload);
    port.write_all(&frame)?;
    port.flush()?;
    Ok(())
}

fn recv(port: &mut dyn SerialPort) -> Result<Vec<u8>, UvK5Error> {
    let header = read_exact(port, 4)?;
    if header[0] != HEADER_MAGIC[0] || header[1] != HEADER_MAGIC[1] || header[3] != 0 {
        return Err(UvK5Error::BadHeader([
            header[0], header[1], header[2], header[3],
        ]));
    }
    let body_len = header[2] as usize;
    let mut body = read_exact(port, body_len)?;
    let footer = read_exact(port, 4)?;
    if footer[2] != FOOTER_MAGIC[0] || footer[3] != FOOTER_MAGIC[1] {
        return Err(UvK5Error::BadFooter([
            footer[0], footer[1], footer[2], footer[3],
        ]));
    }
    xor_inplace(&mut body);
    Ok(body)
}

/// Send hello packet and return the firmware identity string.
pub fn say_hello(port: &mut dyn SerialPort) -> Result<String, UvK5Error> {
    send(port, &HELLO)?;
    let reply = recv(port)?;
    if reply.len() < 5 {
        return Err(UvK5Error::NoHelloReply);
    }
    // Firmware ASCII starts at byte 4, terminated by anything outside
    // printable range (NUL or 0xFF in practice).
    let fw: String = reply[4..]
        .iter()
        .take_while(|&&b| (0x20..=0x7E).contains(&b))
        .map(|&b| b as char)
        .collect();
    Ok(fw)
}

fn read_mem(port: &mut dyn SerialPort, offset: u16, len: u8) -> Result<Vec<u8>, UvK5Error> {
    let payload = [
        0x1B,
        0x05,
        0x08,
        0x00,
        (offset & 0xFF) as u8,
        (offset >> 8) as u8,
        len,
        0x00,
        0x6A,
        0x39,
        0x57,
        0x64,
    ];
    send(port, &payload)?;
    let reply = recv(port)?;
    // Reply: [0x1B, ..., addr_lo, addr_hi, len, 0x00, data...]
    // CHIRP slices `rep[8:]`.
    if reply.len() < 8 {
        return Err(UvK5Error::ShortEeprom { got: reply.len() });
    }
    Ok(reply[8..].to_vec())
}

/// Download the full 8 KiB EEPROM as a single byte vector.
pub fn read_eeprom(port: &mut dyn SerialPort) -> Result<Vec<u8>, UvK5Error> {
    let _firmware = say_hello(port)?;
    let mut eeprom = Vec::with_capacity(EEPROM_SIZE);
    let mut addr: u16 = 0;
    while (addr as usize) < EEPROM_SIZE {
        let chunk = read_mem(port, addr, READ_BLOCK)?;
        eeprom.extend_from_slice(&chunk);
        addr = addr.saturating_add(READ_BLOCK as u16);
    }
    if eeprom.len() < EEPROM_SIZE {
        return Err(UvK5Error::ShortEeprom { got: eeprom.len() });
    }
    eeprom.truncate(EEPROM_SIZE);
    Ok(eeprom)
}

// -------- channel decode (pure, testable) --------

/// Decode the 200 user-channel slots out of a complete EEPROM dump.
/// Empty slots (freq 0 or 0xFFFF_FFFF) are skipped. Channels with AM
/// modulation are skipped with a warning since narm has no AM mode
/// today; warnings are returned alongside the channel list.
pub struct DecodeReport {
    pub channels: Vec<Channel>,
    pub warnings: Vec<String>,
}

pub fn decode_channels(eeprom: &[u8]) -> Result<DecodeReport, UvK5Error> {
    if eeprom.len() < EEPROM_SIZE {
        return Err(UvK5Error::ShortEeprom { got: eeprom.len() });
    }
    let mut channels = Vec::new();
    let mut warnings = Vec::new();

    for i in 0..CHANNEL_COUNT {
        let off = i * CHANNEL_SIZE;
        let rec = &eeprom[off..off + CHANNEL_SIZE];
        let freq_raw = u32::from_le_bytes([rec[0], rec[1], rec[2], rec[3]]);
        if freq_raw == 0 || freq_raw == 0xFFFF_FFFF {
            continue;
        }
        let offset_raw = u32::from_le_bytes([rec[4], rec[5], rec[6], rec[7]]);

        // Frequency stored in 10 Hz units.
        let rx_hz = (freq_raw as u64) * 10;
        let offset_hz = (offset_raw as u64) * 10;

        let rxcode = rec[8];
        let txcode = rec[9];
        let codeflags = rec[10];
        let rx_codeflag = codeflags & 0x0F;
        let tx_codeflag = (codeflags >> 4) & 0x0F;

        let flags1 = rec[11];
        let shift = flags1 & 0b11;
        let enable_am = (flags1 >> 4) & 1 != 0;

        let flags2 = rec[12];
        let bandwidth_bit = (flags2 >> 1) & 1;
        let txpower_bits = (flags2 >> 2) & 0b11;

        // Skip AM channels — narm has no AM mode (yet).
        let name = read_channel_name(eeprom, i);
        let display = if name.is_empty() {
            format!("CH{:03}", i + 1)
        } else {
            name.clone()
        };
        if enable_am {
            warnings.push(format!(
                "skipping {display}: AM modulation not yet supported"
            ));
            continue;
        }

        let bandwidth = if bandwidth_bit == 0 {
            Bandwidth::Wide
        } else {
            Bandwidth::Narrow
        };

        let power = match txpower_bits {
            0b10 => Power::High,
            0b01 => Power::Mid,
            _ => Power::Low,
        };

        let shift_hz: i64 = match shift {
            0b01 => offset_hz as i64,    // +
            0b10 => -(offset_hz as i64), // -
            _ => 0,                      // none
        };

        let (tone_tx_hz, tone_rx_hz, dcs_code) =
            decode_tones(tx_codeflag, txcode, rx_codeflag, rxcode);

        let mode = Mode::Fm {
            bandwidth,
            tone_tx_hz,
            tone_rx_hz,
            dcs_code,
        };

        channels.push(Channel {
            name: if name.is_empty() {
                format!("CH{:03}", i + 1)
            } else {
                name
            },
            rx_hz,
            shift_hz,
            power,
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

    #[test]
    fn xor_round_trip() {
        let data = b"hello world".to_vec();
        let scrambled = xor(&data);
        assert_ne!(scrambled, data);
        let unscrambled = xor(&scrambled);
        assert_eq!(unscrambled, data);
    }

    #[test]
    fn crc16_xmodem_known_vectors() {
        // Standard XMODEM test vectors.
        assert_eq!(crc16_xmodem(b""), 0x0000);
        assert_eq!(crc16_xmodem(b"123456789"), 0x31C3);
        assert_eq!(crc16_xmodem(b"A"), 0x58E5);
    }

    #[test]
    fn build_frame_round_trips() {
        let payload = HELLO.to_vec();
        let frame = build_frame(&payload);
        // header (4) + xor'd(payload+crc) (8+2) + footer (2) = 16
        assert_eq!(frame.len(), 4 + payload.len() + 2 + 2);
        assert_eq!(&frame[..2], &HEADER_MAGIC);
        assert_eq!(&frame[2..4], &[payload.len() as u8, 0]);
        assert_eq!(&frame[frame.len() - 2..], &FOOTER_MAGIC);

        // The xor'd body, when un-xor'd, must reproduce payload + CRC.
        let mut body = frame[4..frame.len() - 2].to_vec();
        xor_inplace(&mut body);
        assert_eq!(&body[..payload.len()], &payload[..]);
        let crc = crc16_xmodem(&payload);
        assert_eq!(body[body.len() - 2], (crc & 0xFF) as u8);
        assert_eq!(body[body.len() - 1], (crc >> 8) as u8);
    }

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
    fn am_channel_skipped_with_warning() {
        let mut e = vec![0u8; EEPROM_SIZE];
        let freq_10hz: u32 = 12_125_000;
        e[0..4].copy_from_slice(&freq_10hz.to_le_bytes());
        e[11] = 1 << 4; // enable_am bit
        let report = decode_channels(&e).unwrap();
        assert!(report.channels.is_empty());
        assert_eq!(report.warnings.len(), 1);
        assert!(report.warnings[0].contains("AM"));
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
