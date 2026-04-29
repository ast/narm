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

pub use decode::{
    DecodeReport, PttId, ScanGroup, ScanMode, Settings, Sidetone, StartupDisplay, TopKey, VfoEntry,
    decode_channels,
};
pub use error::KgQ336Error;
pub use file::{mojibake, unmojibake};
