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
//! narm [`crate::channel::Channel`]s), block writes back to the
//! radio, and the post-write reset.
//!
//! **Safety**: [`write_eeprom`] only ever touches the first
//! [`WRITABLE_SIZE`] (`0x1d00`) bytes — bytes `0x1d00..0x2000` hold
//! factory calibration and are deliberately never overwritten.
//!
//! Submodules:
//!
//! - [`error`] — the [`UvK5Error`] enum.
//! - [`wire`] — pure protocol primitives (XOR, CRC, framing,
//!   opcode structs).
//! - [`decode`] — parses an EEPROM dump into [`crate::channel::Channel`]s.
//! - [`io`] — serial port operations and the high-level
//!   [`read_eeprom`] / [`write_eeprom`] / [`reset_radio`] flows.

mod decode;
mod error;
mod io;
mod wire;

pub use decode::{DecodeReport, decode_channels};
pub use error::UvK5Error;
pub use io::{open_port, read_eeprom, reset_radio, write_eeprom};
pub use wire::{EEPROM_SIZE, WRITABLE_SIZE};
