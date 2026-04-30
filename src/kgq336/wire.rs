//! On-wire framing, opcode constants, and pure protocol primitives
//! (rolling-XOR cipher, checksum, frame build/parse) for the
//! Wouxun KG-Q336 serial link.
//!
//! No I/O lives here — items in this file are pure functions over
//! byte buffers, unit-testable without a serial port.
//!
//! See `docs/kgq336-codeplug.md` for the full protocol
//! reference (the wire-protocol section). In short: each frame is
//!
//! ```text
//!   [0x7C] [cmd] [dir] [len]  enc(payload || cksum)
//! ```
//!
//! with a rolling-XOR cipher (seed `0x54`) over `payload || cksum`
//! together. `len` covers the payload only; the cksum is the +1
//! trailing byte. The Q336 belongs to the kg935g / kguv9dplus
//! family; only the cipher seed (`0x54` vs 0x57 / 0x52) and a
//! `^ 0x03` on the checksum differ from kg935g.

use super::error::KgQ336Error;

// -------- protocol constants --------

/// UART baud rate. Per CHIRP's `KGQ332GRadio` driver (covers
/// both Q332 and Q336), the cable runs at 115200, **not** 19200
/// like the older kg935g family. Our narm-side serialport must
/// match — the Windows CPS green "programming" indicator only
/// lights up if the line speed is correct.
pub(super) const BAUD: u32 = 115_200;

/// Start-of-frame marker for every Q336 packet (both directions).
pub const SOF: u8 = 0x7C;

/// Direction byte: host → radio.
pub const DIR_OUT: u8 = 0xFF;
/// Direction byte: radio → host.
pub const DIR_IN: u8 = 0x00;

/// Session end. Sent by the host after the last read/write command.
/// The payload is empty so the encrypted blob is just the (one-byte)
/// encrypted cksum — wire bytes are `7C 81 FF 00 D7` verbatim
/// (`0xD7 = SEED ^ ((CMD_END + DIR_OUT) & 0xFF ^ 0x03)`).
pub const CMD_END: u8 = 0x81;

/// Read N bytes from a 16-bit address.
pub const CMD_RD: u8 = 0x82;

/// Write N bytes to a 16-bit address.
pub const CMD_WR: u8 = 0x83;

/// Rolling-XOR seed. kg935g uses 0x57, kguv9dplus uses 0x52.
const CIPHER_SEED: u8 = 0x54;

// Q336 adds a per-frame checksum *adjustment* on top of the
// kg935g sum-mod-256 formula. The adjustment is +3/+1/-1/-3
// chosen by the low 2 bits of the first plaintext payload byte
// (addr_hi). See `_checksum2` / `_checksum_adjust` in CHIRP's
// `kgq10h ... 2025mar16.py` (issue #10880).

/// Read block size: each `CMD_RD` request fetches this many data
/// bytes. CPS uses 0x40 in every observed capture.
pub const READ_BLOCK: u8 = 0x40;

/// Write block size: each `CMD_WR` request carries this many data
/// bytes. CPS uses 0x20 in every observed capture.
pub const WRITE_BLOCK: u8 = 0x20;

/// The fixed CMD_END frame the host sends to terminate a session.
/// Header is plaintext; the trailing `0xD7` is the encrypted cksum
/// (single-byte rolling-XOR of `((CMD_END + DIR_OUT) & 0xFF) ^ 0x03
/// = 0x83`, encrypted as `SEED ^ 0x83 = 0xD7`).
pub const END_FRAME: [u8; 5] = [SOF, CMD_END, DIR_OUT, 0x00, 0xD7];

// -------- pure cipher primitives --------

/// Encrypt `buf` in place with the rolling-XOR cipher.
///
/// `enc[0] = SEED ^ plain[0]`, `enc[i] = enc[i-1] ^ plain[i]`.
/// Symmetric counterpart: [`decrypt_inplace`].
pub fn encrypt_inplace(buf: &mut [u8]) {
    let mut prev = CIPHER_SEED;
    for b in buf.iter_mut() {
        *b ^= prev;
        prev = *b;
    }
}

/// Decrypt `buf` in place — inverse of [`encrypt_inplace`].
///
/// `plain[0] = SEED ^ enc[0]`, `plain[i] = enc[i-1] ^ enc[i]`.
pub fn decrypt_inplace(buf: &mut [u8]) {
    let mut prev = CIPHER_SEED;
    for b in buf.iter_mut() {
        let cur = *b;
        *b = prev ^ cur;
        prev = cur;
    }
}

/// Compute the Q336 frame checksum: a sum-mod-256 of `cmd || dir
/// || len || payload` with a small per-frame adjustment chosen
/// by the low 2 bits of `payload[0]` (addr_hi):
///
/// | `addr_hi & 0x03` | adjustment |
/// |---|---|
/// | `0b00` | +3 |
/// | `0b01` | +1 |
/// | `0b10` | -1 |
/// | `0b11` | -3 |
///
/// `payload` must be non-empty (every Q336 frame begins with at
/// least the address-high byte). For the rare empty-payload case
/// like `CMD_END`, `payload[0]` defaults to 0, giving +3.
pub fn checksum(cmd: u8, dir: u8, len: u8, payload: &[u8]) -> u8 {
    let mut sum: u8 = cmd.wrapping_add(dir).wrapping_add(len);
    for &b in payload {
        sum = sum.wrapping_add(b);
    }
    let first = payload.first().copied().unwrap_or(0);
    let adj: i8 = match first & 0x03 {
        0b00 => 3,
        0b01 => 1,
        0b10 => -1,
        _ => -3, // 0b11
    };
    sum.wrapping_add(adj as u8)
}

// -------- frame builders --------

/// Build a complete CMD_RD frame (header + encrypted payload+cksum)
/// ready to write to the serial port.
///
/// Plaintext payload is `[addr_hi, addr_lo, length]`; the cksum is
/// computed over `cmd || dir || len || payload` and appended before
/// encryption.
pub fn build_read_cmd(addr: u16, length: u8) -> Vec<u8> {
    let payload = [(addr >> 8) as u8, addr as u8, length];
    build_out_frame(CMD_RD, &payload)
}

/// Build a complete CMD_WR frame ready to write to the serial
/// port. `data.len()` must fit a `u8` (`<= 253` so `len = 2 +
/// data.len()` doesn't overflow). For the canonical 32-byte CPS
/// page, pass [`WRITE_BLOCK`] worth of bytes.
pub fn build_write_cmd(addr: u16, data: &[u8]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(2 + data.len());
    payload.push((addr >> 8) as u8);
    payload.push(addr as u8);
    payload.extend_from_slice(data);
    build_out_frame(CMD_WR, &payload)
}

fn build_out_frame(cmd: u8, payload: &[u8]) -> Vec<u8> {
    let len = payload.len() as u8;
    let cs = checksum(cmd, DIR_OUT, len, payload);
    let mut frame = Vec::with_capacity(4 + payload.len() + 1);
    frame.extend_from_slice(&[SOF, cmd, DIR_OUT, len]);
    let body_start = frame.len();
    frame.extend_from_slice(payload);
    frame.push(cs);
    encrypt_inplace(&mut frame[body_start..]);
    frame
}

// -------- frame parser --------

/// A successfully parsed inbound frame, with the encrypted blob
/// already turned into plaintext and the checksum verified.
#[derive(Debug, Clone)]
pub struct InFrame {
    pub cmd: u8,
    pub payload: Vec<u8>,
}

/// Parse one full inbound frame from `bytes` and return the
/// decrypted payload (cksum verified). Returns the number of
/// bytes consumed alongside the frame, so callers can chain
/// multiple frames out of a single buffer.
///
/// The `bytes` slice must contain at least
/// `4 + len + 1` bytes (header + encrypted payload + cksum). If
/// fewer are present, returns `KgQ336Error::ShortFrame`.
pub fn parse_in_frame(bytes: &[u8]) -> Result<(InFrame, usize), KgQ336Error> {
    if bytes.len() < 4 {
        return Err(KgQ336Error::ShortFrame {
            got: bytes.len(),
            want: 4,
        });
    }
    if bytes[0] != SOF {
        return Err(KgQ336Error::BadSof { got: bytes[0] });
    }
    let cmd = bytes[1];
    let dir = bytes[2];
    if dir != DIR_IN {
        return Err(KgQ336Error::BadDirection { got: dir });
    }
    let len = bytes[3];
    let want = 4 + len as usize + 1;
    if bytes.len() < want {
        return Err(KgQ336Error::ShortFrame {
            got: bytes.len(),
            want,
        });
    }

    // Decrypt the (len+1)-byte blob: payload || cksum.
    let mut blob = bytes[4..want].to_vec();
    decrypt_inplace(&mut blob);
    let cs_actual = blob.pop().expect("blob has cksum byte");
    let cs_expected = checksum(cmd, dir, len, &blob);
    if cs_actual != cs_expected {
        return Err(KgQ336Error::BadChecksum {
            cmd,
            expected: cs_expected,
            got: cs_actual,
        });
    }
    Ok((InFrame { cmd, payload: blob }, want))
}

/// Decompose a `CMD_RD` reply payload into `(addr, data)`.
///
/// IN reply payload layout: `[addr_hi, addr_lo, data*N]`.
pub fn split_read_reply(payload: &[u8]) -> Result<(u16, &[u8]), KgQ336Error> {
    if payload.len() < 2 {
        return Err(KgQ336Error::ShortFrame {
            got: payload.len(),
            want: 2,
        });
    }
    let addr = ((payload[0] as u16) << 8) | (payload[1] as u16);
    Ok((addr, &payload[2..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------- cipher --------

    #[test]
    fn encrypt_decrypt_round_trip() {
        let plain: Vec<u8> = (0..=200u8).collect();
        let mut buf = plain.clone();
        encrypt_inplace(&mut buf);
        assert_ne!(buf, plain);
        decrypt_inplace(&mut buf);
        assert_eq!(buf, plain);
    }

    #[test]
    fn decrypt_first_in_reply_yields_wouxun() {
        // First IN reply from the 02_read_full_a capture (after
        // the `7c 82 00 42` header). Decryption must place the
        // ASCII model string `WOUXUN` at plaintext bytes 2..8.
        let enc: [u8; 8] = [0x54, 0x14, 0x43, 0x0c, 0x59, 0x01, 0x54, 0x1a];
        let mut buf = enc.to_vec();
        decrypt_inplace(&mut buf);
        assert_eq!(&buf[..2], &[0x00, 0x40]); // addr echo
        assert_eq!(&buf[2..8], b"WOUXUN");
    }

    #[test]
    fn encrypt_first_read_command_matches_capture() {
        // From 02_read_full_a, OUT cmd[0]:
        //   wire bytes (after header): 54 14 54 53
        //   plain payload + cksum:     00 40 40 07
        // i.e. read 0x40 bytes from addr 0x0040, cksum 0x07.
        let mut buf = vec![0x00, 0x40, 0x40, 0x07];
        encrypt_inplace(&mut buf);
        assert_eq!(buf, vec![0x54, 0x14, 0x54, 0x53]);
    }

    // -------- checksum --------

    #[test]
    fn checksum_matches_observed_read_commands() {
        // OUT cmd[0]: payload = [0x00, 0x40, 0x40], cksum = 0x07
        assert_eq!(checksum(CMD_RD, DIR_OUT, 3, &[0x00, 0x40, 0x40]), 0x07);
        // OUT cmd[1]: payload = [0x00, 0x80, 0x40], cksum = 0x47
        assert_eq!(checksum(CMD_RD, DIR_OUT, 3, &[0x00, 0x80, 0x40]), 0x47);
        // OUT cmd[3]: payload = [0x07, 0x00, 0x40], cksum = 0xC8
        assert_eq!(checksum(CMD_RD, DIR_OUT, 3, &[0x07, 0x00, 0x40]), 0xC8);
    }

    #[test]
    fn checksum_matches_observed_write_anchor() {
        // 03_write_noop, idx=10 anchor: enc payload 34×0x51 + cksum
        // 0xfb. Decrypted plain payload = [0x05, 0x00*33], cksum
        // 0xAA. The cksum field on the wire equals
        //   ((0x83 + 0xFF + 0x22 + 0x05) & 0xFF) ^ 0x03 = 0xAA.
        let mut payload = vec![0x05];
        payload.extend(std::iter::repeat_n(0x00, 33));
        assert_eq!(checksum(CMD_WR, DIR_OUT, 0x22, &payload), 0xAA);
    }

    // -------- frame build / parse --------

    #[test]
    fn build_read_cmd_matches_first_capture() {
        // Read 0x40 bytes from 0x0040 → header 7c 82 ff 03 + enc
        // (00 40 40 07) → 54 14 54 53.
        let frame = build_read_cmd(0x0040, 0x40);
        assert_eq!(frame, vec![0x7C, 0x82, 0xFF, 0x03, 0x54, 0x14, 0x54, 0x53]);
    }

    #[test]
    fn build_read_cmd_addr_high_byte_increments() {
        // Read 0x40 bytes from 0x0700 (cmd[3] in the read capture)
        // — header + enc(07 00 40 c8) = 53 53 13 db.
        let frame = build_read_cmd(0x0700, 0x40);
        assert_eq!(frame, vec![0x7C, 0x82, 0xFF, 0x03, 0x53, 0x53, 0x13, 0xDB]);
    }

    #[test]
    fn build_write_cmd_anchor_block() {
        // Construct the all-zero 32-byte write block at addr_hi
        // 0x05, addr_lo 0x00. Plaintext payload = [0x05, 0x00, data
        // *32]; with data all-zero the encrypted payload collapses
        // to 34×0x51 by rolling XOR (because plain[0] = 0x05 and
        // every other byte is 0, so enc stays at SEED^0x05 = 0x51).
        let frame = build_write_cmd(0x0500, &[0x00; 32]);
        // Header
        assert_eq!(&frame[..4], &[0x7C, 0x83, 0xFF, 0x22]);
        // 34 bytes of 0x51, then cksum 0xfb
        assert_eq!(&frame[4..38], &[0x51; 34]);
        assert_eq!(frame[38], 0xFB);
    }

    #[test]
    fn parse_in_frame_handles_first_read_reply() {
        // Wire bytes for the first IN reply, captured live. Header
        // is 7c 82 00 42 followed by the encrypted (66+1)-byte
        // payload. Parser must:
        //   - return cmd=0x82,
        //   - decrypt to addr 0x0040 + "WOUXUN" + …,
        //   - verify the trailing cksum.
        // We synthesize the wire bytes here from a known plaintext
        // and the trusted encrypt_inplace() (cipher already
        // round-trip tested above), so this is really checking the
        // header walk + cksum verification path, not the cipher.
        let mut plain = vec![0u8; 67];
        plain[0] = 0x00;
        plain[1] = 0x40;
        plain[2..8].copy_from_slice(b"WOUXUN");
        let cs = checksum(CMD_RD, DIR_IN, 0x42, &plain[..66]);
        plain[66] = cs;
        let mut blob = plain.clone();
        encrypt_inplace(&mut blob);
        let mut wire = vec![0x7C, 0x82, 0x00, 0x42];
        wire.extend_from_slice(&blob);

        let (frame, n) = parse_in_frame(&wire).unwrap();
        assert_eq!(n, wire.len());
        assert_eq!(frame.cmd, CMD_RD);
        assert_eq!(&frame.payload[..2], &[0x00, 0x40]);
        assert_eq!(&frame.payload[2..8], b"WOUXUN");
    }

    #[test]
    fn parse_in_frame_rejects_short_input() {
        match parse_in_frame(&[0x7C, 0x82]) {
            Err(KgQ336Error::ShortFrame { got: 2, want: 4 }) => (),
            other => panic!("expected ShortFrame, got {other:?}"),
        }
    }

    #[test]
    fn parse_in_frame_rejects_bad_sof() {
        match parse_in_frame(&[0x00, 0x82, 0x00, 0x00, 0x00]) {
            Err(KgQ336Error::BadSof { got: 0x00 }) => (),
            other => panic!("expected BadSof, got {other:?}"),
        }
    }

    #[test]
    fn parse_in_frame_rejects_outgoing_direction() {
        // dir = 0xFF means host→radio; we only parse radio→host
        // here.
        match parse_in_frame(&[0x7C, 0x82, 0xFF, 0x00, 0x00]) {
            Err(KgQ336Error::BadDirection { got: 0xFF }) => (),
            other => panic!("expected BadDirection, got {other:?}"),
        }
    }

    #[test]
    fn parse_in_frame_rejects_bad_checksum() {
        // Build a valid frame, then flip one bit in the cksum.
        let mut plain = vec![0u8; 5];
        plain[0] = 0x00;
        plain[1] = 0x40;
        plain[4] = checksum(CMD_RD, DIR_IN, 0x04, &plain[..4]);
        let mut blob = plain.clone();
        encrypt_inplace(&mut blob);
        // Corrupt the last enc byte (which decrypts to cksum).
        let last = blob.len() - 1;
        blob[last] ^= 0x01;

        let mut wire = vec![0x7C, 0x82, 0x00, 0x04];
        wire.extend_from_slice(&blob);

        match parse_in_frame(&wire) {
            Err(KgQ336Error::BadChecksum { .. }) => (),
            other => panic!("expected BadChecksum, got {other:?}"),
        }
    }

    #[test]
    fn end_frame_constants_self_consistent() {
        // CMD_END has an empty payload, so the encrypted blob is
        // just the (one-byte) cksum: enc = SEED ^ cksum.
        let cs_raw = checksum(CMD_END, DIR_OUT, 0, &[]);
        assert_eq!(cs_raw, 0x83);
        let cs_enc = CIPHER_SEED ^ cs_raw;
        assert_eq!(cs_enc, 0xD7);
        assert_eq!(END_FRAME, [SOF, CMD_END, DIR_OUT, 0x00, cs_enc]);
        // The same wire bytes also fall out of build_out_frame for a
        // zero-byte payload — sanity check that path agrees.
        assert_eq!(build_out_frame(CMD_END, &[]), END_FRAME.to_vec());
    }

    #[test]
    fn split_read_reply_extracts_addr_and_data() {
        let payload: Vec<u8> = (0..66u8).collect();
        let (addr, data) = split_read_reply(&payload).unwrap();
        assert_eq!(addr, 0x0001);
        assert_eq!(data.len(), 64);
        assert_eq!(data[0], 2);
        assert_eq!(data[63], 65);
    }
}
