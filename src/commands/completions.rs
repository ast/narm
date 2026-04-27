use std::io;

use anyhow::Result;
use clap::{Args, CommandFactory};
use clap_complete::{Shell, generate};

use crate::commands::Cli;

#[derive(Args, Debug)]
pub struct CompletionsArgs {
    /// Shell to generate completions for.
    pub shell: Shell,
}

pub fn run(args: &CompletionsArgs) -> Result<()> {
    let mut cmd = Cli::command();
    let bin_name = cmd.get_name().to_string();
    generate(args.shell, &mut cmd, bin_name, &mut io::stdout());
    Ok(())
}
