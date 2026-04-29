//! Serial I/O over the UV-K5 cable: open a port, send/receive
//! framed packets, and the high-level read-EEPROM / write-EEPROM /
//! reset workflows.
//!
//! All protocol functions are generic over the underlying byte
//! stream: they take `&mut P where P: Read + Write + ?Sized`. In
//! production we pass `&mut *port` from a `Box<dyn SerialPort>`
//! (serialport::SerialPort: Read + Write); in tests we pass a
//! `MockPort` with split incoming/outgoing buffers.

use std::io::{Read, Write};
use std::time::Duration;

use serialport::SerialPort;
use zerocopy::{FromBytes, IntoBytes};

use super::error::UvK5Error;
use super::wire::{
    BAUD, EEPROM_SIZE, FrameFooter, FrameHeader, HELLO, READ_BLOCK, ReadCommand, WRITABLE_SIZE,
    WRITE_BLOCK, WriteReply, build_frame, build_write_payload, xor_inplace,
};

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
fn say_hello<P: Read + Write + ?Sized>(port: &mut P) -> Result<String, UvK5Error> {
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
