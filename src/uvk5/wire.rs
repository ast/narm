//! On-wire framing, opcode structs, and pure protocol primitives
//! (XOR scrambling, CRC, frame build). No I/O lives here — items in
//! this file are pure functions over byte buffers, unit-testable
//! without a serial port.
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

use zerocopy::byteorder::little_endian::U16;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

use super::error::UvK5Error;

// -------- protocol constants --------

pub(super) const BAUD: u32 = 38400;

/// Cyclic XOR key used by the radio for protocol obfuscation.
/// Same bytes as CHIRP's `xorarr` table (uvk5.py); CHIRP writes them
/// as decimal, we keep hex here for byte-twiddle readability.
const XOR_KEY: [u8; 16] = [
    0x16, 0x6C, 0x14, 0xE6, 0x2E, 0x91, 0x0D, 0x40, 0x21, 0x35, 0xD5, 0x40, 0x13, 0x03, 0xE9, 0x80,
];

pub(super) const HELLO: [u8; 8] = [0x14, 0x05, 0x04, 0x00, 0x6A, 0x39, 0x57, 0x64];

const PROTO_MAGIC: [u8; 4] = [0x6A, 0x39, 0x57, 0x64];

/// 8 KiB of EEPROM, read in 128-byte chunks.
pub const EEPROM_SIZE: usize = 0x2000;

/// CHIRP's `PROG_SIZE` — bytes 0x0000..0x1d00 are channels + settings
/// and safe to overwrite. The remaining 0x300 bytes are factory
/// calibration; narm refuses to write there to avoid bricking the
/// radio's RX/TX alignment.
pub const WRITABLE_SIZE: usize = 0x1d00;

pub(super) const READ_BLOCK: u8 = 0x80;
pub(super) const WRITE_BLOCK: u8 = 0x80;

// -------- opcode/wire structs --------

/// Read-block command, sent to the radio. Wire layout matches CHIRP
/// `_readmem`: `1b 05 08 00 [addr_lo addr_hi] [len] 00 6a 39 57 64`.
#[repr(C)]
#[derive(IntoBytes, Immutable, Debug)]
pub(super) struct ReadCommand {
    op: [u8; 4],
    offset: U16,
    data_len: u8,
    pad: u8,
    magic: [u8; 4],
}

impl ReadCommand {
    const OP: [u8; 4] = [0x1B, 0x05, 0x08, 0x00];

    /// Build a read-block command for the given EEPROM `offset` and
    /// chunk `len`. Pure constructor — pair with `as_bytes()` to get
    /// the wire bytes.
    pub(super) fn new(offset: u16, len: u8) -> Self {
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
    op: [u8; 2],
    payload_len: u8,
    pad1: u8,
    offset: U16,
    data_len: u8,
    one: u8,
    magic: [u8; 4],
}

impl WriteHeader {
    const OP: [u8; 2] = [0x1D, 0x05];

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
/// `OPCODE`) and `offset` (must equal the requested offset); reply
/// may contain additional trailing bytes which we ignore.
#[repr(C)]
#[derive(FromBytes, Immutable, KnownLayout, Debug)]
pub(super) struct WriteReply {
    pub(super) opcode: u8,
    _pad1: [u8; 3],
    pub(super) offset: U16,
    _pad2: [u8; 2],
}

impl WriteReply {
    pub(super) const OPCODE: u8 = 0x1E;
}

/// Outer frame header (4 bytes): `AB CD [body_len] 00`.
#[repr(C)]
#[derive(FromBytes, IntoBytes, Immutable, KnownLayout, Debug)]
pub(super) struct FrameHeader {
    pub(super) magic: [u8; 2],
    pub(super) body_len: u8,
    pub(super) pad: u8,
}

impl FrameHeader {
    pub(super) const MAGIC: [u8; 2] = [0xAB, 0xCD];

    /// Reject anything that doesn't match the expected magic + zero
    /// padding byte. Returns the raw 4 wire bytes inside `BadHeader`
    /// for diagnostics on mismatch.
    pub(super) fn validate(&self) -> Result<(), UvK5Error> {
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
pub(super) struct FrameFooter {
    crc_xor: [u8; 2],
    pub(super) magic: [u8; 2],
}

impl FrameFooter {
    pub(super) const MAGIC: [u8; 2] = [0xDC, 0xBA];

    /// Reject anything that doesn't match the expected trailer magic.
    /// Returns the raw 4 wire bytes inside `BadFooter` for diagnostics
    /// on mismatch.
    pub(super) fn validate(&self) -> Result<(), UvK5Error> {
        if self.magic != Self::MAGIC {
            return Err(UvK5Error::BadFooter(self.as_bytes().try_into().unwrap()));
        }
        Ok(())
    }
}

// -------- pure protocol primitives --------

pub(super) fn xor_inplace(data: &mut [u8]) {
    for (i, b) in data.iter_mut().enumerate() {
        *b ^= XOR_KEY[i % XOR_KEY.len()];
    }
}

/// CRC16/XMODEM: poly 0x1021, init 0x0000, no reflection, no XOR-out.
pub(super) fn crc16_xmodem(data: &[u8]) -> u16 {
    crc::Crc::<u16>::new(&crc::CRC_16_XMODEM).checksum(data)
}

pub(super) fn build_frame(payload: &[u8]) -> Vec<u8> {
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

/// Build the write-block command bytes (pre-framing). Caller
/// guarantees `data.len() <= 0xff - 8`. Returns header + data
/// concatenated as a `Vec<u8>` since `data` is variable-length.
pub(super) fn build_write_payload(offset: u16, data: &[u8]) -> Vec<u8> {
    let header = WriteHeader::new(offset, data.len() as u8);
    let mut out = Vec::with_capacity(12 + data.len());
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(data);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xor(data: &[u8]) -> Vec<u8> {
        let mut out = data.to_vec();
        xor_inplace(&mut out);
        out
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
}
