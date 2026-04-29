use clap::{Parser, Subcommand};

pub mod compile;
pub mod completions;
pub mod grid;
pub mod kgq336;
pub mod list_radios;
pub mod radio;
pub mod repeaters;
pub mod validate;

use compile::CompileArgs;
use completions::CompletionsArgs;
use grid::GridArgs;
use kgq336::Kgq336Args;
use radio::RadioArgs;
use repeaters::RepeatersArgs;
use validate::ValidateArgs;

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
    /// Talk to a connected radio over serial (read EEPROM, …).
    #[command(visible_alias = "rad")]
    Radio(RadioArgs),
    /// Inspect / debug Wouxun KG-Q332 / KG-Q336 codeplugs.
    #[command(visible_alias = "kg")]
    Kgq336(Kgq336Args),
    /// Generate shell completion scripts.
    #[command(visible_alias = "comp")]
    Completions(CompletionsArgs),
}
