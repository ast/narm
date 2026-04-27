use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use clap_complete::Shell;

use narm::Radio;

pub mod compile;
pub mod completions;
pub mod grid;
pub mod list_radios;
pub mod repeaters;
pub mod validate;

#[derive(Parser, Debug)]
#[command(name = "narm", version, about = "Nina Arvid Radio Manager")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Parse and validate a channel config file.
    Validate(ValidateArgs),
    /// Compile a config to a target radio's format.
    Compile(CompileArgs),
    /// List supported radio targets.
    ListRadios,
    /// Convert between Maidenhead grid locator and lat/lng.
    Grid(GridArgs),
    /// Manage and query the SSA repeater database.
    Repeaters(RepeatersArgs),
    /// Generate shell completion scripts.
    Completions(CompletionsArgs),
}

#[derive(Args, Debug)]
pub struct ValidateArgs {
    /// Path to a TOML config file or a directory of `*.toml` files.
    pub config: PathBuf,
}

#[derive(Args, Debug)]
pub struct CompileArgs {
    /// Path to a TOML config file or a directory of `*.toml` files.
    pub config: PathBuf,
    /// Target radio.
    #[arg(long, value_enum)]
    pub radio: Radio,
    /// Output file (defaults to stdout).
    #[arg(long, short)]
    pub out: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct CompletionsArgs {
    /// Shell to generate completions for.
    pub shell: Shell,
}

#[derive(Args, Debug)]
pub struct GridArgs {
    /// Either a Maidenhead locator (e.g. JO67AT) — one arg —
    /// or a "lat lng" pair (e.g. 57.8125 12.0417) — two args.
    #[arg(num_args = 1..=2, value_name = "LOCATOR | LAT LNG")]
    pub input: Vec<String>,
}

#[derive(Args, Debug)]
pub struct RepeatersArgs {
    /// SQLite database path. Defaults to $XDG_DATA_HOME/narm/repeaters.db.
    #[arg(long, env = "NARM_DB", global = true)]
    pub db: Option<PathBuf>,
    #[command(subcommand)]
    pub command: RepeatersCommand,
}

#[derive(Subcommand, Debug)]
pub enum RepeatersCommand {
    /// Import a SSA repeater CSV (https://www.ssa.se/vushf/repeatrar-fyrar/).
    Import(ImportRepeatersArgs),
    /// List repeaters within a radius of a location.
    Near(NearArgs),
}

#[derive(Args, Debug)]
pub struct ImportRepeatersArgs {
    /// Path to the SSA repeaters CSV.
    pub csv: PathBuf,
}

#[derive(Args, Debug)]
pub struct NearArgs {
    /// Maidenhead locator (one arg) or "lat lng" coords (two args).
    #[arg(num_args = 1..=2, value_name = "LOCATOR | LAT LNG")]
    pub location: Vec<String>,
    /// Search radius in kilometres.
    #[arg(long, default_value_t = 50.0)]
    pub radius: f64,
    /// Filter by band column (e.g. 2, 70, 23).
    #[arg(long)]
    pub band: Option<String>,
    /// Filter by mode column (case-insensitive: fm, dmr, c4fm, dstar).
    #[arg(long)]
    pub mode: Option<String>,
    /// Maximum number of results (default: no limit).
    #[arg(long)]
    pub limit: Option<usize>,
    /// Emit tab-separated output instead of an aligned table.
    #[arg(long)]
    pub tsv: bool,
}
