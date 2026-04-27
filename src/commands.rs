use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use clap_complete::Shell;

use narm::Radio;

pub mod compile;
pub mod completions;
pub mod list_radios;
pub mod validate;

#[derive(Parser, Debug)]
#[command(name = "narm", version, about = "Nina Arvid Radio Manager")]
pub struct Cli {
    #[command(subcommand)]
    pub cmd: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Parse and validate a channel config file.
    Validate(ValidateArgs),
    /// Compile a config to a target radio's format.
    Compile(CompileArgs),
    /// List supported radio targets.
    ListRadios,
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
