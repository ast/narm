use anyhow::Result;
use clap::Parser;

mod commands;

use commands::{Cli, Cmd};

fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let cli = Cli::parse();
    match &cli.cmd {
        Cmd::Validate(args) => commands::validate::run(args),
        Cmd::Compile(args) => commands::compile::run(args),
        Cmd::ListRadios => commands::list_radios::run(),
        Cmd::Completions(args) => commands::completions::run(args),
    }
}
