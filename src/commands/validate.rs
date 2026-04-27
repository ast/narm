use std::path::PathBuf;

use anyhow::Result;
use clap::Args;

#[derive(Args, Debug)]
pub struct ValidateArgs {
    /// Path to a TOML config file or a directory of `*.toml` files.
    pub config: PathBuf,
}

pub fn run(args: &ValidateArgs) -> Result<()> {
    let cfg = narm::load_from_path(&args.config)?;
    narm::validate(&cfg)?;
    println!("OK: {} channels", cfg.channels.len());
    Ok(())
}
