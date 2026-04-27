use anyhow::Result;
use clap::Parser;

mod commands;

use commands::{Cli, Command};

fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let cli = Cli::parse();
    match &cli.command {
        Command::Validate(args) => commands::validate::run(args),
        Command::Compile(args) => commands::compile::run(args),
        Command::ListRadios => commands::list_radios::run(),
        Command::Grid(args) => commands::grid::run(args),
        Command::Repeaters(args) => commands::repeaters::run(args),
        Command::Radio(args) => commands::radio::run(args),
        Command::Completions(args) => commands::completions::run(args),
    }
}
