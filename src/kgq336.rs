//! Wouxun KG-Q332 / KG-Q336 codeplug — file decode (Phase 1)
//! and live serial protocol (Phase 2, not yet implemented).
//!
//! No CHIRP driver exists for these radios at the time of
//! writing, so the layout is being reverse-engineered from
//! `.kg` files saved by the vendor's CPS plus (eventually)
//! USB-serial captures of the live protocol.
//!
//! Submodules:
//!
//! - [`error`] — the [`KgQ336Error`] enum.
//! - [`file`] — [`unmojibake`]/[`mojibake`]: convert between
//!   the CPS's `.kg` text wrapper and the underlying raw
//!   image bytes.
//! - [`decode`] — channel decoding (Phase 1, in progress).
//!
//! See `/home/albin/.claude/plans/is-there-not-a-glowing-token.md`
//! for the full implementation plan.

mod decode;
mod error;
mod file;
pub mod inspect;
mod io;
mod wire;

pub use decode::{
    Alert, DecodeReport, Language, PttId, Roger, ScQt, ScanGroup, ScanMode, Settings, Sidetone,
    StartupDisplay, SubFreqMute, TopKey, VfoBand, VfoEntry, VfoStep, WorkMode, decode_channels,
};
pub use error::KgQ336Error;
pub use file::{
    KG_SHAPE_LEN, PHYSICAL_LEN, logical_to_kg_shape, mojibake, to_kg_shape, unmojibake, unscramble,
};
pub use io::{open_port, read_codeplug};
pub use wire::{
    CMD_END, CMD_RD, CMD_WR, DIR_IN, DIR_OUT, END_FRAME, InFrame, READ_BLOCK, SOF, WRITE_BLOCK,
    build_read_cmd, build_write_cmd, checksum, decrypt_inplace, encrypt_inplace, parse_in_frame,
    split_read_reply,
};
