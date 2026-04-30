//! Error type shared across the KG-Q336 file/decode/protocol stack.

#[derive(thiserror::Error, Debug)]
pub enum KgQ336Error {
    #[error("missing 'xiepinruanjian\\r\\n' header")]
    MissingHeader,
    #[error("missing trailing CRLF")]
    MissingFooter,
    #[error("bad mojibake byte 0x{byte:02x} at offset 0x{offset:x}")]
    BadMojibake { offset: usize, byte: u8 },
    #[error("image too short: {got} bytes, expected at least {min}")]
    ShortImage { got: usize, min: usize },
    #[error("serial I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("serial port: {0}")]
    Serial(#[from] serialport::Error),
    #[error("frame too short: got {got}, want {want}")]
    ShortFrame { got: usize, want: usize },
    #[error("bad start-of-frame byte: 0x{got:02x} (expected 0x7C)")]
    BadSof { got: u8 },
    #[error("bad direction byte in inbound frame: 0x{got:02x} (expected 0x00)")]
    BadDirection { got: u8 },
    #[error("bad checksum in cmd 0x{cmd:02x}: expected 0x{expected:02x}, got 0x{got:02x}")]
    BadChecksum { cmd: u8, expected: u8, got: u8 },
    #[error("read reply at addr 0x{expected:04x} echoed wrong addr 0x{got:04x}")]
    BadReadAddress { expected: u16, got: u16 },
    #[error("read reply for cmd 0x{got:02x} (expected 0x{want:02x})")]
    BadReplyCmd { want: u8, got: u8 },
}
