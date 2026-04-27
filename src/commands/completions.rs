use std::io;

use anyhow::Result;
use clap::CommandFactory;
use clap_complete::generate;

use crate::commands::{Cli, CompletionsArgs};

pub fn run(args: &CompletionsArgs) -> Result<()> {
    let mut cmd = Cli::command();
    let bin_name = cmd.get_name().to_string();
    generate(args.shell, &mut cmd, bin_name, &mut io::stdout());
    Ok(())
}
