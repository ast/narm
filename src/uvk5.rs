//! Quansheng UV-K5 / UV-K5(8) wire protocol — read & write.
//!
//! Derived from CHIRP's `chirp/drivers/uvk5.py` (kk7ds/chirp on
//! GitHub, **GPL-2.0-or-later** per that file's header; the CHIRP
//! project is GPL-3.0 at the top level). The protocol opcodes,
//! magic bytes, XOR key, EEPROM offsets, and channel-record
//! layout all come from there. See `NOTICE.md` at the repo root
//! for the full attribution and licensing implications.
//!
//! Covers handshake, EEPROM block reads (channel decoding into
//! narm [`Channel`]s), block writes back to the radio, and the
//! post-write reset.
//!
//! **Safety**: `write_eeprom` only ever touches the first
//! [`WRITABLE_SIZE`] (`0x1d00`) bytes — bytes `0x1d00..0x2000` hold
//! factory calibration and are deliberately never overwritten.
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

use std::io::{Read, Write};
use std::time::Duration;

use serialport::SerialPort;
use zerocopy::byteorder::little_endian::{U16, U32};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

use crate::channel::{Bandwidth, Channel, Mode, Power};

// -------- on-wire layouts as zerocopy structs --------

/// Read-block command, sent to the radio. Wire layout matches CHIRP
/// `_readmem`: `1b 05 08 00 [addr_lo addr_hi] [len] 00 6a 39 57 64`.
#[repr(C)]
#[derive(IntoBytes, Immutable, Debug)]
struct ReadCommand {
    op: [u8; 4], // 0x1B 0x05 0x08 0x00
    offset: U16,
    data_len: u8,
    pad: u8,        // 0x00
    magic: [u8; 4], // 0x6A 0x39 0x57 0x64
}

const PROTO_MAGIC: [u8; 4] = [0x6A, 0x39, 0x57, 0x64];

impl ReadCommand {
    const OP: [u8; 4] = [0x1B, 0x05, 0x08, 0x00];

    /// Build a read-block command for the given EEPROM `offset` and
    /// chunk `len`. Pure constructor — pair with `as_bytes()` to get
    /// the wire bytes.
    fn new(offset: u16, len: u8) -> Self {
        Self {
            op: Self::OP,
            offset: U16::new(offset),
            data_len: len,
            pad: 0,
            magic: PROTO_MAGIC,
        }
    }
}

/// Write-block command header (12 bytes, followed by raw `data`
/// bytes). Wire layout matches CHIRP `_writemem`:
///   `1d 05 [dlen+8] 00 [addr_lo addr_hi] [dlen] 01 6a 39 57 64
///    [data...]`.
#[repr(C)]
#[derive(IntoBytes, Immutable, Debug)]
struct WriteHeader {
    op: [u8; 2],     // 0x1D 0x05
    payload_len: u8, // dlen + 8
    pad1: u8,        // 0x00
    offset: U16,
    data_len: u8,
    one: u8,        // 0x01
    magic: [u8; 4], // 0x6A 0x39 0x57 0x64
}

impl WriteHeader {
    const OP: [u8; 2] = [0x1D, 0x05];

    /// Build a write-block header for the given EEPROM `offset` and
    /// `data_len`. Caller appends the variable-length data after
    /// `as_bytes()`.
    fn new(offset: u16, data_len: u8) -> Self {
        Self {
            op: Self::OP,
            payload_len: data_len + 8,
            pad1: 0,
            offset: U16::new(offset),
            data_len,
            one: 1,
            magic: PROTO_MAGIC,
        }
    }
}

/// Reply to a write-block command: the radio echoes the request's
/// address back at bytes 4..6. We only care about `opcode` (must be
/// `0x1E`) and `offset` (must equal the requested offset); reply may
/// contain additional trailing bytes which we ignore.
#[repr(C)]
#[derive(FromBytes, Immutable, KnownLayout, Debug)]
struct WriteReply {
    opcode: u8, // expect 0x1E
    _pad1: [u8; 3],
    offset: U16,
    _pad2: [u8; 2],
}

impl WriteReply {
    const OPCODE: u8 = 0x1E;
}

/// Outer frame header (4 bytes): `AB CD [body_len] 00`.
#[repr(C)]
#[derive(FromBytes, IntoBytes, Immutable, KnownLayout, Debug)]
struct FrameHeader {
    magic: [u8; 2],
    body_len: u8,
    pad: u8,
}

impl FrameHeader {
    const MAGIC: [u8; 2] = [0xAB, 0xCD];

    /// Reject anything that doesn't match the expected magic + zero
    /// padding byte. Returns the raw 4 wire bytes inside `BadHeader`
    /// for diagnostics on mismatch.
    fn validate(&self) -> Result<(), UvK5Error> {
        if self.magic != Self::MAGIC || self.pad != 0 {
            return Err(UvK5Error::BadHeader(self.as_bytes().try_into().unwrap()));
        }
        Ok(())
    }
}

/// Outer frame footer (4 bytes): `[crc_xor:2] DC BA`. The CRC is XOR'd
/// with the cyclic key like the body; we don't verify it (radio
/// doesn't either), so it's just bytes here.
#[repr(C)]
#[derive(FromBytes, IntoBytes, Immutable, KnownLayout, Debug)]
struct FrameFooter {
    crc_xor: [u8; 2],
    magic: [u8; 2],
}

impl FrameFooter {
    const MAGIC: [u8; 2] = [0xDC, 0xBA];

    /// Reject anything that doesn't match the expected trailer magic.
    /// Returns the raw 4 wire bytes inside `BadFooter` for diagnostics
    /// on mismatch.
    fn validate(&self) -> Result<(), UvK5Error> {
        if self.magic != Self::MAGIC {
            return Err(UvK5Error::BadFooter(self.as_bytes().try_into().unwrap()));
        }
        Ok(())
    }
}

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

const BAUD: u32 = 38400;
/// Cyclic XOR key used by the radio for protocol obfuscation.
/// Same bytes as CHIRP's `xorarr` table (uvk5.py); CHIRP writes them
/// as decimal, we keep hex here for byte-twiddle readability.
const XOR_KEY: [u8; 16] = [
    0x16, 0x6C, 0x14, 0xE6, 0x2E, 0x91, 0x0D, 0x40, 0x21, 0x35, 0xD5, 0x40, 0x13, 0x03, 0xE9, 0x80,
];

const HELLO: [u8; 8] = [0x14, 0x05, 0x04, 0x00, 0x6A, 0x39, 0x57, 0x64];

/// 8 KiB of EEPROM, read in 128-byte chunks.
pub const EEPROM_SIZE: usize = 0x2000;
/// CHIRP's `PROG_SIZE` — bytes 0x0000..0x1d00 are channels + settings
/// and safe to overwrite. The remaining 0x300 bytes are factory
/// calibration; narm refuses to write there to avoid bricking the
/// radio's RX/TX alignment.
pub const WRITABLE_SIZE: usize = 0x1d00;
const READ_BLOCK: u8 = 0x80;
const WRITE_BLOCK: u8 = 0x80;

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
    #[error("image must be exactly {WRITABLE_SIZE} or {EEPROM_SIZE} bytes (got {got})")]
    BadImageSize { got: usize },
    #[error("write reply at offset 0x{offset:04x}: bad opcode/payload {reply:02x?}")]
    BadWriteReply { offset: u16, reply: Vec<u8> },
    #[error("write reply at offset 0x{expected:04x} echoed wrong addr 0x{got:04x}")]
    BadWriteAddress { expected: u16, got: u16 },
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
    crc::Crc::<u16>::new(&crc::CRC_16_XMODEM).checksum(data)
}

fn build_frame(payload: &[u8]) -> Vec<u8> {
    // XOR the payload + CRC together so each byte is keyed against its
    // cyclic position in the body, then split off the trailing CRC
    // bytes into the footer struct.
    let crc = crc16_xmodem(payload);
    let mut body = Vec::with_capacity(payload.len() + 2);
    body.extend_from_slice(payload);
    body.extend_from_slice(&crc.to_le_bytes());
    xor_inplace(&mut body);
    let crc_xor: [u8; 2] = body[payload.len()..].try_into().unwrap();
    body.truncate(payload.len());

    let header = FrameHeader {
        magic: FrameHeader::MAGIC,
        body_len: payload.len() as u8,
        pad: 0,
    };
    let footer = FrameFooter {
        crc_xor,
        magic: FrameFooter::MAGIC,
    };

    let mut frame = Vec::with_capacity(4 + body.len() + 4);
    frame.extend_from_slice(header.as_bytes());
    frame.extend_from_slice(&body);
    frame.extend_from_slice(footer.as_bytes());
    frame
}

// -------- I/O --------
//
// All protocol functions are generic over the underlying byte
// stream: they take `&mut P where P: Read + Write + ?Sized`. In
// production we pass `&mut *port` from a `Box<dyn SerialPort>`
// (serialport::SerialPort: Read + Write); in tests we pass a
// `MockPort` with split incoming/outgoing buffers.

pub fn open_port(port: &str) -> Result<Box<dyn SerialPort>, UvK5Error> {
    eprintln!("connecting to radio on {port}…");
    serialport::new(port, BAUD)
        .timeout(Duration::from_millis(2000))
        .open()
        .map_err(Into::into)
}

fn read_exact<P: Read + ?Sized>(port: &mut P, n: usize) -> Result<Vec<u8>, UvK5Error> {
    let mut buf = vec![0u8; n];
    port.read_exact(&mut buf)?;
    Ok(buf)
}

fn send<P: Write + ?Sized>(port: &mut P, payload: &[u8]) -> Result<(), UvK5Error> {
    let frame = build_frame(payload);
    port.write_all(&frame)?;
    port.flush()?;
    Ok(())
}

fn recv<P: Read + ?Sized>(port: &mut P) -> Result<Vec<u8>, UvK5Error> {
    let header = FrameHeader::read_from_io(&mut *port)?;
    header.validate()?;
    let mut body = read_exact(port, header.body_len as usize)?;
    let footer = FrameFooter::read_from_io(&mut *port)?;
    footer.validate()?;
    xor_inplace(&mut body);
    Ok(body)
}

/// Send hello packet and return the firmware identity string.
pub fn say_hello<P: Read + Write + ?Sized>(port: &mut P) -> Result<String, UvK5Error> {
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
    eprintln!("radio firmware: {fw}");
    Ok(fw)
}

fn read_mem<P: Read + Write + ?Sized>(
    port: &mut P,
    offset: u16,
    len: u8,
) -> Result<Vec<u8>, UvK5Error> {
    send(port, ReadCommand::new(offset, len).as_bytes())?;
    let reply = recv(port)?;
    // Reply: [0x1B, ..., addr_lo, addr_hi, len, 0x00, data...]
    // CHIRP slices `rep[8:]`.
    if reply.len() < 8 {
        return Err(UvK5Error::ShortEeprom { got: reply.len() });
    }
    Ok(reply[8..].to_vec())
}

/// Build the write-block command bytes (pre-framing). Caller
/// guarantees `data.len() <= 0xff - 8`. Returns header + data
/// concatenated as a `Vec<u8>` since `data` is variable-length.
fn build_write_payload(offset: u16, data: &[u8]) -> Vec<u8> {
    let header = WriteHeader::new(offset, data.len() as u8);
    let mut out = Vec::with_capacity(12 + data.len());
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(data);
    out
}

fn write_mem<P: Read + Write + ?Sized>(
    port: &mut P,
    offset: u16,
    data: &[u8],
) -> Result<(), UvK5Error> {
    let payload = build_write_payload(offset, data);
    send(port, &payload)?;
    let reply = recv(port)?;
    // Reply: WriteReply struct (8 bytes) possibly followed by trailing
    // bytes the radio echoes back. Use `ref_from_prefix` so a longer
    // reply still parses cleanly.
    let parsed = match WriteReply::ref_from_prefix(&reply) {
        Ok((r, _trailer)) if r.opcode == WriteReply::OPCODE => r,
        _ => {
            let head_len = reply.len().min(8);
            return Err(UvK5Error::BadWriteReply {
                offset,
                reply: reply[..head_len].to_vec(),
            });
        }
    };
    let echoed = parsed.offset.get();
    if echoed != offset {
        return Err(UvK5Error::BadWriteAddress {
            expected: offset,
            got: echoed,
        });
    }
    Ok(())
}

/// Send the radio-reset packet (`dd 05 00 00`). The radio reboots so
/// the freshly-uploaded image takes effect immediately.
pub fn reset_radio<P: Write + ?Sized>(port: &mut P) -> Result<(), UvK5Error> {
    send(port, &[0xDD, 0x05, 0x00, 0x00])
}

/// Upload an EEPROM image to the radio. Accepts either a full
/// `EEPROM_SIZE` (0x2000) blob (calibration tail is silently dropped)
/// or a `WRITABLE_SIZE` (0x1d00) channel-and-settings-only image.
/// Always writes exactly the first 0x1d00 bytes; the factory
/// calibration block at `0x1d00..0x2000` is never touched.
pub fn write_eeprom<P: Read + Write + ?Sized>(
    port: &mut P,
    image: &[u8],
) -> Result<usize, UvK5Error> {
    // Two valid input sizes: WRITABLE_SIZE = channels + settings only;
    // EEPROM_SIZE = full dump (calibration tail is silently dropped
    // below).
    if !matches!(image.len(), WRITABLE_SIZE | EEPROM_SIZE) {
        return Err(UvK5Error::BadImageSize { got: image.len() });
    }
    let writable = &image[..WRITABLE_SIZE];
    let _firmware = say_hello(port)?;

    let mut addr: u16 = 0;
    while (addr as usize) < WRITABLE_SIZE {
        let start = addr as usize;
        let end = (start + WRITE_BLOCK as usize).min(WRITABLE_SIZE);
        write_mem(port, addr, &writable[start..end])?;
        addr = addr.saturating_add(WRITE_BLOCK as u16);
    }
    Ok(WRITABLE_SIZE)
}

/// Download the full 8 KiB EEPROM as a single byte vector.
pub fn read_eeprom<P: Read + Write + ?Sized>(port: &mut P) -> Result<Vec<u8>, UvK5Error> {
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
    use std::io::{self, Cursor};

    /// Half-duplex mock port: reads consume from `incoming`; writes
    /// accumulate into `outgoing`. Tests pre-seed `incoming` with the
    /// bytes the radio "would have" sent, run the protocol function,
    /// then assert behaviour on the captured `outgoing` bytes.
    struct MockPort {
        incoming: Cursor<Vec<u8>>,
        outgoing: Vec<u8>,
    }

    impl MockPort {
        fn new(incoming: Vec<u8>) -> Self {
            Self {
                incoming: Cursor::new(incoming),
                outgoing: Vec::new(),
            }
        }
    }

    impl io::Read for MockPort {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.incoming.read(buf)
        }
    }

    impl io::Write for MockPort {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.outgoing.write(buf)
        }
        fn flush(&mut self) -> io::Result<()> {
            self.outgoing.flush()
        }
    }

    /// Build a complete inbound frame the way the radio would send
    /// it, given an un-XOR'd payload.
    fn radio_reply(payload: &[u8]) -> Vec<u8> {
        // Same wire shape as `build_frame` — the radio uses the same
        // framing it expects from us.
        build_frame(payload)
    }

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
        assert_eq!(&frame[..2], &FrameHeader::MAGIC);
        assert_eq!(&frame[2..4], &[payload.len() as u8, 0]);
        assert_eq!(&frame[frame.len() - 2..], &FrameFooter::MAGIC);

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

    #[test]
    fn build_write_payload_matches_chirp_format() {
        // CHIRP `_writemem(serport, data, offset)`:
        //   payload = b"\x1d\x05" + struct.pack("<BBHBB", dlen+8, 0,
        //             offset, dlen, 1) + b"\x6a\x39\x57\x64" + data
        //
        // For data = b"\x00\x01\x02\x03" at offset 0x0080:
        //   1d 05 0c 00  80 00 04 01  6a 39 57 64  00 01 02 03
        let payload = build_write_payload(0x0080, &[0x00, 0x01, 0x02, 0x03]);
        assert_eq!(
            payload,
            [
                0x1D, 0x05, 0x0C, 0x00, 0x80, 0x00, 0x04, 0x01, 0x6A, 0x39, 0x57, 0x64, 0x00, 0x01,
                0x02, 0x03,
            ],
        );
    }

    #[test]
    fn build_write_payload_for_full_block() {
        // 128-byte block at offset 0x1c80 (last writable block before
        // the calibration boundary at 0x1d00).
        let data: Vec<u8> = (0..128u8).collect();
        let payload = build_write_payload(0x1C80, &data);
        // Header + footer-ish framing fields = 12 bytes; data = 128.
        assert_eq!(payload.len(), 12 + 128);
        assert_eq!(&payload[0..2], &[0x1D, 0x05]);
        assert_eq!(payload[2], 128 + 8); // dlen+8
        assert_eq!(payload[3], 0x00);
        assert_eq!(&payload[4..6], &[0x80, 0x1C]); // 0x1c80 LE
        assert_eq!(payload[6], 128); // dlen
        assert_eq!(payload[7], 0x01);
        assert_eq!(&payload[8..12], &[0x6A, 0x39, 0x57, 0x64]);
        assert_eq!(&payload[12..], &data[..]);
    }

    // ===== regression tests captured from the live UV-K5(8) =====
    //
    // Bytes below were read off a real radio with `narm radio read
    // --format raw` and verified against the radio's UI. They lock
    // the wire-format and decoder against future drift.

    #[test]
    fn xor_key_locks_each_byte() {
        // XOR-ing 'A' (0x41) against the 16-byte cyclic key produces
        // a fully-determined sequence; this test fails if any single
        // byte of XOR_KEY changes.
        let scrambled = xor(b"AAAAAAAAAAAAAAAA");
        assert_eq!(
            scrambled,
            vec![
                0x57, 0x2D, 0x55, 0xA7, 0x6F, 0xD0, 0x4C, 0x01, 0x60, 0x74, 0x94, 0x01, 0x52, 0x42,
                0xA8, 0xC1,
            ],
        );
    }

    #[test]
    fn hello_payload_constant() {
        // The hello bytes the radio expects on the wire.
        assert_eq!(HELLO, [0x14, 0x05, 0x04, 0x00, 0x6A, 0x39, 0x57, 0x64]);
    }

    #[test]
    fn read_command_matches_chirp_format() {
        // CHIRP `_readmem`:
        //   payload = b"\x1b\x05\x08\x00" + struct.pack("<HBB", off, len, 0)
        //             + b"\x6a\x39\x57\x64"
        // For offset=0x0080, len=0x80:
        //   1b 05 08 00 80 00 80 00 6a 39 57 64
        assert_eq!(
            ReadCommand::new(0x0080, 0x80).as_bytes(),
            &[
                0x1B, 0x05, 0x08, 0x00, 0x80, 0x00, 0x80, 0x00, 0x6A, 0x39, 0x57, 0x64
            ],
        );
        assert_eq!(
            ReadCommand::new(0x1F80, 0x80).as_bytes(),
            &[
                0x1B, 0x05, 0x08, 0x00, 0x80, 0x1F, 0x80, 0x00, 0x6A, 0x39, 0x57, 0x64
            ],
        );
    }

    #[test]
    fn frame_invariants_for_arbitrary_payload() {
        // For every well-formed payload, the framer must produce:
        //   prefix = AB CD len 00, suffix = DC BA, total = len+8.
        for payload in [
            HELLO.to_vec(),
            ReadCommand::new(0, 0x80).as_bytes().to_vec(),
            build_write_payload(0x0040, &[0xAB; 64]),
        ] {
            let frame = build_frame(&payload);
            assert_eq!(&frame[..2], &FrameHeader::MAGIC);
            assert_eq!(frame[2] as usize, payload.len());
            assert_eq!(frame[3], 0x00);
            assert_eq!(&frame[frame.len() - 2..], &FrameFooter::MAGIC);
            assert_eq!(frame.len(), payload.len() + 8);
        }
    }

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

    // ===== mock-port I/O regression tests =====

    #[test]
    fn send_writes_a_well_formed_frame() {
        let mut port = MockPort::new(Vec::new());
        send(&mut port, &HELLO).unwrap();
        // Frame layout: header (4) + xor(payload+crc) (8+2) + footer (2)
        assert_eq!(port.outgoing.len(), 4 + HELLO.len() + 2 + 2);
        assert_eq!(&port.outgoing[..2], &FrameHeader::MAGIC);
        assert_eq!(port.outgoing[2] as usize, HELLO.len());
        assert_eq!(
            &port.outgoing[port.outgoing.len() - 2..],
            &FrameFooter::MAGIC
        );
    }

    #[test]
    fn recv_returns_un_xored_payload_for_well_formed_frame() {
        let payload = vec![0x18, 0x05, 0x20, 0x00, b'k', b'5', b'_', b'2'];
        let mut port = MockPort::new(radio_reply(&payload));
        let got = recv(&mut port).unwrap();
        assert_eq!(got, payload);
    }

    #[test]
    fn recv_rejects_bad_header_magic() {
        let mut port = MockPort::new(vec![0x00, 0x00, 0x04, 0x00, 0, 0, 0, 0, 0, 0, 0xDC, 0xBA]);
        match recv(&mut port) {
            Err(UvK5Error::BadHeader(h)) => assert_eq!(h, [0x00, 0x00, 0x04, 0x00]),
            other => panic!("expected BadHeader, got {other:?}"),
        }
    }

    #[test]
    fn recv_rejects_bad_footer_magic() {
        // Valid header + body, but footer's last two bytes are wrong.
        let body = vec![0u8; 2];
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&FrameHeader::MAGIC);
        bytes.push(body.len() as u8);
        bytes.push(0);
        bytes.extend_from_slice(&body);
        bytes.extend_from_slice(&[0x00, 0x00, 0xFF, 0xFF]); // wrong footer
        let mut port = MockPort::new(bytes);
        match recv(&mut port) {
            Err(UvK5Error::BadFooter(f)) => assert_eq!(&f[2..], &[0xFF, 0xFF]),
            other => panic!("expected BadFooter, got {other:?}"),
        }
    }

    /// Build a write-reply frame the radio would send for `addr_echo`.
    fn write_reply_frame(opcode: u8, addr_echo: u16) -> Vec<u8> {
        // 8-byte WriteReply payload: opcode | 3 pad | addr_lo addr_hi | 2 pad
        let payload = vec![
            opcode,
            0,
            0,
            0,
            (addr_echo & 0xFF) as u8,
            (addr_echo >> 8) as u8,
            0,
            0,
        ];
        radio_reply(&payload)
    }

    #[test]
    fn write_mem_succeeds_when_radio_echoes_correct_address() {
        let mut port = MockPort::new(write_reply_frame(WriteReply::OPCODE, 0x0080));
        write_mem(&mut port, 0x0080, &[0xAA; 4]).unwrap();
        // Outgoing should be a properly framed write command.
        assert_eq!(&port.outgoing[..2], &FrameHeader::MAGIC);
        assert_eq!(
            &port.outgoing[port.outgoing.len() - 2..],
            &FrameFooter::MAGIC
        );
    }

    #[test]
    fn write_mem_rejects_wrong_opcode() {
        let mut port = MockPort::new(write_reply_frame(0x99, 0x0080));
        match write_mem(&mut port, 0x0080, &[0; 4]) {
            Err(UvK5Error::BadWriteReply { offset, .. }) => assert_eq!(offset, 0x0080),
            other => panic!("expected BadWriteReply, got {other:?}"),
        }
    }

    #[test]
    fn write_mem_rejects_address_mismatch() {
        // Opcode is fine but the radio echoed the wrong address.
        let mut port = MockPort::new(write_reply_frame(WriteReply::OPCODE, 0x0100));
        match write_mem(&mut port, 0x0080, &[0; 4]) {
            Err(UvK5Error::BadWriteAddress { expected, got }) => {
                assert_eq!(expected, 0x0080);
                assert_eq!(got, 0x0100);
            }
            other => panic!("expected BadWriteAddress, got {other:?}"),
        }
    }

    #[test]
    fn say_hello_extracts_firmware_string() {
        // Payload: opcode 0x18 + 3 padding bytes + ASCII firmware tag.
        let mut payload = vec![0x18, 0x05, 0x20, 0x00];
        payload.extend_from_slice(b"k5prog-v0.42\x00\x00");
        let mut port = MockPort::new(radio_reply(&payload));
        let fw = say_hello(&mut port).unwrap();
        assert_eq!(fw, "k5prog-v0.42");
    }
}
