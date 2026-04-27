use anyhow::{Result, bail};

use crate::commands::CompileArgs;

pub fn run(args: &CompileArgs) -> Result<()> {
    let cfg = narm::load_from_path(&args.config)?;
    narm::validate(&cfg)?;
    bail!(
        "compile: not yet implemented for {} ({} channels loaded)",
        args.radio.display_name(),
        cfg.channels.len()
    );
}
