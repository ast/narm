use anyhow::Result;

use crate::commands::ValidateArgs;

pub fn run(args: &ValidateArgs) -> Result<()> {
    let cfg = narm::load_from_path(&args.config)?;
    narm::validate(&cfg)?;
    println!("OK: {} channels", cfg.channels.len());
    Ok(())
}
