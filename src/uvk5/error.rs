//! Error type shared across the UV-K5 protocol stack.

use super::wire::{EEPROM_SIZE, WRITABLE_SIZE};

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
