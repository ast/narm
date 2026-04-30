//! Serial I/O over the KG-Q336 cable: open the PL2303 port, send
//! framed packets, and the high-level read-codeplug workflow.
//!
//! All protocol functions are generic over the underlying byte
//! stream (`Read + Write + ?Sized`), mirroring `uvk5::io` — in
//! production we pass `&mut *port` from a `Box<dyn SerialPort>`,
//! and in tests we pass a `MockPort` that splits incoming/outgoing
//! buffers.
//!
//! ## Address space note
//!
//! The radio's wire-level memory layout is **not** the same as the
//! `.kg` file's layout. CPS reads the radio in non-contiguous
//! chunks (model probe at `0x0040`, then jumps to `0x0700`+) and
//! transforms the bytes when it writes the `.kg` file. We don't
//! yet know that transform, so live reads here return the raw
//! linear radio image as bytes; converting to the `.kg` schema
//! that [`super::decode::decode_channels`] expects is a separate
//! step we haven't implemented.

use std::io::{Read, Write};
use std::thread::sleep;
use std::time::Duration;

use serialport::{ClearBuffer, SerialPort};

use super::error::KgQ336Error;
use super::wire::{
    BAUD, CMD_RD, END_FRAME, READ_BLOCK, SOF, build_read_cmd, parse_in_frame, split_read_reply,
};

/// Linear address window the live reader walks. Covers all known
/// codeplug regions plus a safety margin. CPS itself only reads
/// ~32 KB across non-contiguous addresses; we do contiguous reads
/// since the radio happily returns zero pages for any address in
/// this range.
const READ_END: u16 = 0x8000;

/// Serial-port read timeout. Generous enough to cover the radio's
/// occasional 100ms-ish stalls between blocks.
const TIMEOUT_MS: u64 = 2000;

/// Number of times the very first CMD_RD is blasted to wake the
/// radio. Per Mel Terechenok's KG-Q10H/Q33x driver (CHIRP issue
/// #10880, attached `kgq10h ... 2025mar16.py` line 5538):
/// "Wouxun CPS sends the same Read command 3 times to establish
/// comms" — only the third copy actually elicits a reply.
const WAKEUP_BLASTS: usize = 3;
const WAKEUP_DELAY_MS: u64 = 200;

/// Open the PL2303 cable and put the line into the state CPS uses:
/// 19200 8N1, DTR asserted, RTS deasserted, input buffer flushed.
///
/// Captured CPS sets DTR=1, RTS=0 via SET_CONTROL_LINE_STATE
/// just before the first CMD_RD. Many K-plug cables wire RTS to a
/// PTT transistor, so RTS must stay low to keep the cable in RX
/// mode (otherwise the radio's audio path is muted by an active
/// PTT).
pub fn open_port(port: &str) -> Result<Box<dyn SerialPort>, KgQ336Error> {
    eprintln!("connecting to KG-Q336 on {port}…");
    let mut p = serialport::new(port, BAUD)
        .timeout(Duration::from_millis(TIMEOUT_MS))
        .open()?;
    p.write_data_terminal_ready(true)?;
    p.write_request_to_send(false)?;
    // Clear any stale bytes (e.g. partial frame from a previous
    // session) so the first recv_frame doesn't trip on garbage.
    let _ = p.clear(ClearBuffer::All);
    // Let the radio's UART settle after the line-state change
    // before we start banging frames at it.
    sleep(Duration::from_millis(WAKEUP_DELAY_MS));
    Ok(p)
}

fn read_exact<P: Read + ?Sized>(port: &mut P, n: usize) -> Result<Vec<u8>, KgQ336Error> {
    let mut buf = vec![0u8; n];
    port.read_exact(&mut buf)?;
    Ok(buf)
}

/// Read one full inbound frame from the wire. Reads the 4-byte
/// header to learn the length, then reads `len + 1` more bytes
/// (encrypted payload + cksum), then hands the whole thing to
/// [`parse_in_frame`] for decryption + checksum verification.
fn recv_frame<P: Read + ?Sized>(port: &mut P) -> Result<super::wire::InFrame, KgQ336Error> {
    let header = read_exact(port, 4)?;
    if header[0] != SOF {
        return Err(KgQ336Error::BadSof { got: header[0] });
    }
    let len = header[3];
    let body = read_exact(port, len as usize + 1)?;
    let mut buf = header;
    buf.extend_from_slice(&body);
    let (frame, _) = parse_in_frame(&buf)?;
    Ok(frame)
}

/// Send a CMD_RD for `addr`/`length` and return the data bytes
/// from the radio's reply (with the address echo verified).
fn read_block<P: Read + Write + ?Sized>(
    port: &mut P,
    addr: u16,
    length: u8,
) -> Result<Vec<u8>, KgQ336Error> {
    let cmd = build_read_cmd(addr, length);
    port.write_all(&cmd)?;
    port.flush()?;

    let frame = recv_frame(port)?;
    if frame.cmd != CMD_RD {
        return Err(KgQ336Error::BadReplyCmd {
            want: CMD_RD,
            got: frame.cmd,
        });
    }
    let (echoed, data) = split_read_reply(&frame.payload)?;
    if echoed != addr {
        return Err(KgQ336Error::BadReadAddress {
            expected: addr,
            got: echoed,
        });
    }
    Ok(data.to_vec())
}

/// Send the unencrypted CMD_END frame to terminate the session.
/// The radio does not reply; this just closes the conversation
/// cleanly so the CPS-LED state on the radio resets.
fn send_end<P: Write + ?Sized>(port: &mut P) -> Result<(), KgQ336Error> {
    port.write_all(&END_FRAME)?;
    port.flush()?;
    Ok(())
}

/// Send the first CMD_RD `WAKEUP_BLASTS` times back-to-back with
/// no read in between, then consume a single reply. This is the
/// handshake the radio expects — it ignores the first 1–2 copies
/// while its UART firmware spins up.
fn wake_up_read<P: Read + Write + ?Sized>(
    port: &mut P,
    addr: u16,
    length: u8,
) -> Result<Vec<u8>, KgQ336Error> {
    let cmd = build_read_cmd(addr, length);
    for _ in 0..WAKEUP_BLASTS {
        port.write_all(&cmd)?;
    }
    port.flush()?;
    sleep(Duration::from_millis(WAKEUP_DELAY_MS));

    let frame = recv_frame(port)?;
    if frame.cmd != CMD_RD {
        return Err(KgQ336Error::BadReplyCmd {
            want: CMD_RD,
            got: frame.cmd,
        });
    }
    let (echoed, data) = split_read_reply(&frame.payload)?;
    if echoed != addr {
        return Err(KgQ336Error::BadReadAddress {
            expected: addr,
            got: echoed,
        });
    }
    Ok(data.to_vec())
}

/// First address the radio responds to. CPS's model probe also
/// starts here; addresses below `0x0040` may not be accepted.
const READ_START: u16 = 0x0040;

/// Read the radio's full linear memory (`READ_START..READ_END`)
/// over the serial link, in `READ_BLOCK`-sized chunks, and return
/// a `READ_END`-byte buffer with the radio data placed at its
/// wire-level address (bytes 0..READ_START left zeroed since the
/// radio doesn't expose those).
///
/// The returned image is the radio's **wire-level** memory, not
/// the `.kg` file layout — see the module docstring.
pub fn read_codeplug<P: Read + Write + ?Sized>(port: &mut P) -> Result<Vec<u8>, KgQ336Error> {
    let mut image = vec![0u8; READ_END as usize];

    // Wake-up: 3× the canonical model-probe CMD_RD at 0x0040, then
    // read one reply. Matches CPS's identification dance.
    let probe = wake_up_read(port, READ_START, READ_BLOCK)?;
    let start = READ_START as usize;
    image[start..start + probe.len()].copy_from_slice(&probe);

    let mut addr: u16 = READ_START + READ_BLOCK as u16;
    while addr < READ_END {
        let chunk = read_block(port, addr, READ_BLOCK)?;
        let off = addr as usize;
        image[off..off + chunk.len()].copy_from_slice(&chunk);
        addr = addr.saturating_add(READ_BLOCK as u16);
    }
    send_end(port)?;
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kgq336::wire::{DIR_IN, build_write_cmd, checksum, encrypt_inplace};
    use std::io::{self, Cursor};

    /// Half-duplex mock port: reads consume from `incoming`; writes
    /// accumulate into `outgoing`. Tests pre-seed `incoming` with
    /// the bytes the radio "would have" sent, run the protocol
    /// function, then assert behaviour on the captured `outgoing`
    /// bytes.
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

    /// Build a CMD_RD reply frame the radio would send for `addr`
    /// + `data`, ready to drop into a `MockPort::incoming`.
    fn read_reply_frame(addr: u16, data: &[u8]) -> Vec<u8> {
        // IN payload = [addr_hi, addr_lo, data*N]
        let mut payload = vec![(addr >> 8) as u8, addr as u8];
        payload.extend_from_slice(data);
        let len = payload.len() as u8;
        let cs = checksum(CMD_RD, DIR_IN, len, &payload);
        let mut blob = payload;
        blob.push(cs);
        encrypt_inplace(&mut blob);

        let mut frame = vec![SOF, CMD_RD, DIR_IN, len];
        frame.extend_from_slice(&blob);
        frame
    }

    #[test]
    fn read_block_round_trips_a_known_reply() {
        // Radio memory at 0x0040 is "WOUXUN..." in our captures.
        let payload: Vec<u8> = b"WOUXUN".iter().chain([0u8; 58].iter()).copied().collect();
        let mut port = MockPort::new(read_reply_frame(0x0040, &payload));
        let data = read_block(&mut port, 0x0040, 0x40).unwrap();
        assert_eq!(data.len(), 64);
        assert_eq!(&data[..6], b"WOUXUN");
        // Outgoing should be the well-known wire bytes for cmd[0]
        // of the read capture: 7c 82 ff 03 54 14 54 53.
        assert_eq!(
            port.outgoing,
            vec![0x7C, 0x82, 0xFF, 0x03, 0x54, 0x14, 0x54, 0x53]
        );
    }

    #[test]
    fn read_block_rejects_address_mismatch() {
        let payload = vec![0u8; 64];
        let mut port = MockPort::new(read_reply_frame(0x0080, &payload));
        match read_block(&mut port, 0x0040, 0x40) {
            Err(KgQ336Error::BadReadAddress { expected, got }) => {
                assert_eq!(expected, 0x0040);
                assert_eq!(got, 0x0080);
            }
            other => panic!("expected BadReadAddress, got {other:?}"),
        }
    }

    #[test]
    fn read_block_rejects_unexpected_cmd() {
        // Radio sends a CMD_WR-shaped reply instead of CMD_RD.
        let payload = vec![0x00, 0x40];
        let len = payload.len() as u8;
        let cs = checksum(0x83, DIR_IN, len, &payload);
        let mut blob = payload;
        blob.push(cs);
        encrypt_inplace(&mut blob);
        let mut frame = vec![SOF, 0x83, DIR_IN, len];
        frame.extend_from_slice(&blob);

        let mut port = MockPort::new(frame);
        match read_block(&mut port, 0x0040, 0x40) {
            Err(KgQ336Error::BadReplyCmd { want, got }) => {
                assert_eq!(want, CMD_RD);
                assert_eq!(got, 0x83);
            }
            other => panic!("expected BadReplyCmd, got {other:?}"),
        }
    }

    #[test]
    fn read_codeplug_walks_the_full_address_window() {
        // For each `READ_BLOCK`-sized step from READ_START..READ_END,
        // queue a reply whose data is the addr_hi byte repeated 64
        // times — gives us a deterministic image to verify.
        let mut incoming = Vec::new();
        let mut addr: u16 = READ_START;
        while addr < READ_END {
            let data = vec![(addr >> 8) as u8; READ_BLOCK as usize];
            incoming.extend_from_slice(&read_reply_frame(addr, &data));
            addr = addr.saturating_add(READ_BLOCK as u16);
        }
        let mut port = MockPort::new(incoming);
        let image = read_codeplug(&mut port).unwrap();
        assert_eq!(image.len(), READ_END as usize);
        // Bytes below READ_START are zero-filled (never read).
        assert!(image[..READ_START as usize].iter().all(|&b| b == 0));
        // 0x0040 block was filled with 0x00 (addr_hi), 0x0400 with
        // 0x04, last block (0x7FC0) with 0x7F.
        assert_eq!(image[0x0040], 0x00);
        assert_eq!(image[0x0400], 0x04);
        assert_eq!(image[0x7FC0], 0x7F);

        // After the last read, we should have sent CMD_END (the
        // 5-byte termination frame).
        assert_eq!(&port.outgoing[port.outgoing.len() - 5..], &END_FRAME);

        // Wake-up: the first WAKEUP_BLASTS×8 bytes must be exactly
        // three back-to-back copies of the canonical 0x0040 probe
        // (`7c 82 ff 03 54 14 54 53`).
        let probe = build_read_cmd(READ_START, READ_BLOCK);
        let mut expected = Vec::new();
        for _ in 0..WAKEUP_BLASTS {
            expected.extend_from_slice(&probe);
        }
        assert_eq!(&port.outgoing[..expected.len()], &expected[..]);

        // No CMD_WR-shaped frame should appear (smoke check that
        // read path doesn't accidentally call write).
        let stray_wr = build_write_cmd(0x0000, &[0; 32]);
        assert!(
            port.outgoing.windows(stray_wr.len()).all(|w| w != stray_wr),
            "read_codeplug must not emit a CMD_WR frame"
        );
    }
}
