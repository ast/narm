use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;

use narm::Radio;

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
#[clap(rename_all = "kebab-case")]
pub enum CompileFormat {
    /// Generic CHIRP CSV — importable by CHIRP for any supported radio.
    ChirpCsv,
}

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
    #[command(visible_alias = "v")]
    Validate(ValidateArgs),
    /// Compile a config to a target radio's format.
    #[command(visible_alias = "c")]
    Compile(CompileArgs),
    /// List supported radio targets.
    #[command(visible_alias = "lr")]
    ListRadios,
    /// Convert between Maidenhead grid locator and lat/lng.
    #[command(visible_alias = "g")]
    Grid(GridArgs),
    /// Manage and query the SSA repeater database.
    #[command(visible_alias = "rep")]
    Repeaters(RepeatersArgs),
    /// Generate shell completion scripts.
    #[command(visible_alias = "comp")]
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
    /// Output format. Defaults to chirp-csv (the universal CHIRP
    /// generic-CSV interchange).
    #[arg(long, value_enum, default_value_t = CompileFormat::ChirpCsv)]
    pub format: CompileFormat,
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
    #[command(visible_alias = "i")]
    Import(ImportRepeatersArgs),
    /// List repeaters within a radius of a location.
    #[command(visible_alias = "n")]
    Near(NearArgs),
    /// Full-text search over call, city, district, network (FTS5).
    #[command(visible_alias = "s")]
    Search(SearchArgs),
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
    /// Filter by band (e.g. 2, 70, 23). Comma-separated and/or
    /// repeated: --band 2,70 or --band 2 --band 70.
    #[arg(long, value_delimiter = ',')]
    pub band: Vec<String>,
    /// Filter by mode (case-insensitive: fm, dmr, c4fm, dstar).
    /// Comma-separated and/or repeated.
    #[arg(long, value_delimiter = ',')]
    pub mode: Vec<String>,
    /// Maximum number of results (default: no limit).
    #[arg(long)]
    pub limit: Option<usize>,
    /// Emit tab-separated output instead of an aligned table.
    #[arg(long)]
    pub tsv: bool,
}

#[derive(Args, Debug)]
pub struct SearchArgs {
    /// Free-text query (terms ANDed together, FTS5 metacharacters like
    /// `-`, `:`, `*` are treated literally). Pass --raw to use FTS5
    /// syntax directly (e.g. `call:SK6*`, `A AND B`, `-noise`).
    pub query: String,
    /// Filter by band (comma-separated and/or repeated).
    #[arg(long, value_delimiter = ',')]
    pub band: Vec<String>,
    /// Filter by mode (comma-separated and/or repeated, case-insensitive).
    #[arg(long, value_delimiter = ',')]
    pub mode: Vec<String>,
    /// Maximum number of results (default: no limit).
    #[arg(long)]
    pub limit: Option<usize>,
    /// Emit tab-separated output instead of an aligned table.
    #[arg(long)]
    pub tsv: bool,
    /// Pass the query verbatim to FTS5 (no escaping).
    #[arg(long)]
    pub raw: bool,
}
