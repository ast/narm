use std::path::PathBuf;

use clap::{ArgGroup, Parser, Subcommand};

use narm::Radio;

pub mod completions;
pub mod decode;
pub mod detect;
pub mod encode;
pub mod encode_db;
pub mod format;
pub mod grid;
pub mod info;
pub mod list_radios;
pub mod read;
pub mod repeaters;
pub mod verify;
pub mod write;
pub mod write_db;

use completions::CompletionsArgs;
use decode::DecodeArgs;
use encode::EncodeArgs;
use grid::GridArgs;
use info::InfoArgs;
use read::ReadArgs;
use repeaters::RepeatersArgs;
use write::WriteArgs;

/// Top-level option group. Mirrors `dmrconf [Options] [COMMAND]
/// [file]` — every flag here is `global = true` so it can be
/// passed before *or* after the subcommand.
#[derive(Parser, Debug)]
#[command(
    name = "narm",
    version,
    about = "Nina Arvid Radio Manager",
    long_about = "Manage channels and setup for several handheld ham radios \
                  from one TOML source of truth."
)]
#[command(group = ArgGroup::new("format").multiple(false))]
pub struct Cli {
    /// Target radio. Required for offline `encode`/`decode`/
    /// `verify`; optional when a radio is connected and `detect`
    /// can identify it.
    #[arg(short = 'R', long, value_enum, global = true)]
    pub radio: Option<Radio>,

    /// Serial port the radio is on (e.g. `/dev/ttyUSB0`, `COM3`).
    #[arg(short = 'D', long, global = true)]
    pub device: Option<String>,

    /// Format: TOML (narm canonical).
    #[arg(short = 't', long, global = true, group = "format")]
    pub toml: bool,
    /// Format: dmrconf-compatible YAML (interop target).
    #[arg(short = 'y', long, global = true, group = "format")]
    pub yaml: bool,
    /// Format: CHIRP-generic CSV.
    #[arg(short = 'c', long, global = true, group = "format")]
    pub csv: bool,
    /// Format: raw radio image.
    #[arg(short = 'b', long, global = true, group = "format")]
    pub bin: bool,
    /// Format: vendor-native binary (e.g. `.kg` for KG-Q336).
    #[arg(short = 'm', long, global = true, group = "format")]
    pub manufacturer: bool,

    /// Output file (`-` for stdout, the default where applicable).
    #[arg(short = 'o', long, global = true)]
    pub out: Option<PathBuf>,

    /// Verbose output.
    #[arg(long, global = true)]
    pub verbose: bool,

    /// Print the list of supported radios and exit.
    #[arg(long)]
    pub list_radios: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

impl Cli {
    /// Bundle the format flags into a single value for the
    /// downstream resolver in [`format::resolve`].
    pub fn format_flag(&self) -> format::FormatFlag {
        format::FormatFlag {
            toml: self.toml,
            yaml: self.yaml,
            csv: self.csv,
            bin: self.bin,
            manufacturer: self.manufacturer,
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Detect the connected radio.
    Detect,
    /// Read a codeplug from the radio into a file.
    Read(ReadArgs),
    /// Write a codeplug file to the radio.
    Write(WriteArgs),
    /// Verify a codeplug file against the connected radio.
    Verify,
    /// Encode a TOML config to YAML / CSV / binary / manufacturer
    /// format (replaces `compile`).
    Encode(EncodeArgs),
    /// Decode a binary codeplug to TOML / YAML / CSV.
    Decode(DecodeArgs),
    /// Print a human-readable summary of a codeplug or config
    /// file.
    Info(InfoArgs),
    /// Write the call-sign database to the radio.
    WriteDb,
    /// Encode the call-sign database to a manufacturer binary.
    EncodeDb,
    /// Convert between Maidenhead grid locator and lat/lng.
    Grid(GridArgs),
    /// Manage and query the SSA repeater database.
    Repeaters(RepeatersArgs),
    /// Generate shell completion scripts.
    Completions(CompletionsArgs),
}
