use anyhow::Result;
use clap::Parser;

mod commands;

use commands::{Cli, Command};

fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let cli = Cli::parse();

    if cli.list_radios {
        return commands::list_radios::run();
    }

    let Some(command) = &cli.command else {
        // No subcommand and no --list-radios: behave like
        // `narm --help`.
        use clap::CommandFactory;
        let mut cmd = Cli::command();
        cmd.print_help()?;
        println!();
        return Ok(());
    };

    match command {
        Command::Detect => commands::detect::run(&cli),
        Command::Read(args) => commands::read::run(&cli, args),
        Command::Write(args) => commands::write::run(&cli, args),
        Command::Verify => commands::verify::run(&cli),
        Command::Encode(args) => commands::encode::run(&cli, args),
        Command::Decode(args) => commands::decode::run(&cli, args),
        Command::Info(args) => commands::info::run(&cli, args),
        Command::WriteDb => commands::write_db::run(&cli),
        Command::EncodeDb => commands::encode_db::run(&cli),
        Command::Grid(args) => commands::grid::run(args),
        Command::Repeaters(args) => commands::repeaters::run(args),
        Command::Completions(args) => commands::completions::run(args),
    }
}
