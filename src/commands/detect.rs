use anyhow::{Result, bail};

use crate::commands::Cli;

pub fn run(_cli: &Cli) -> Result<()> {
    bail!(
        "detect: not implemented yet — narm doesn't probe USB / serial \
         identity yet; pass -R/--radio explicitly for now"
    )
}
