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
}
