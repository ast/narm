use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Args;

use narm::Radio;
use narm::uvk5;

use crate::commands::Cli;
use crate::commands::format::{Format, resolve};

#[derive(Args, Debug)]
pub struct WriteArgs {
    /// Codeplug to upload. Currently must be a raw EEPROM image
    /// (`-b/--bin`); other formats are decoded later.
    pub file: PathBuf,
}

pub fn run(cli: &Cli, args: &WriteArgs) -> Result<()> {
    let radio = cli
        .radio
        .ok_or_else(|| anyhow::anyhow!("--radio/-R is required"))?;
    let device = cli
        .device
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("-D/--device is required"))?;

    let format = resolve(cli.format_flag(), Some(&args.file))?;
    if format != Format::Bin {
        bail!(
            "narm write currently only accepts -b/--bin images (got `{}`); \
             encode-then-write for other formats is not implemented yet",
            format.as_str()
        );
    }

    if radio != Radio::QuanshengUvK5 {
        bail!(
            "radio write is only implemented for quansheng-uv-k5 (got {})",
            radio.id()
        );
    }

    let image = std::fs::read(&args.file)
        .with_context(|| format!("reading image {}", args.file.display()))?;
    // Accept the same two image sizes as write_eeprom: WRITABLE_SIZE
    // (channels + settings) or full EEPROM_SIZE (calibration tail
    // dropped silently inside write_eeprom).
    if !matches!(image.len(), uvk5::WRITABLE_SIZE | uvk5::EEPROM_SIZE) {
        bail!(
            "image must be {} or {} bytes (got {})",
            uvk5::WRITABLE_SIZE,
            uvk5::EEPROM_SIZE,
            image.len()
        );
    }

    let mut port =
        uvk5::open_port(device).with_context(|| format!("opening serial port {device}"))?;

    eprintln!(
        "writing {} bytes from {} to {} (calibration block 0x1d00..0x2000 preserved)…",
        uvk5::WRITABLE_SIZE,
        args.file.display(),
        device
    );
    let written = uvk5::write_eeprom(&mut *port, &image).context("writing eeprom to radio")?;
    eprintln!("wrote {written} bytes; resetting radio…");
    uvk5::reset_radio(&mut *port).context("sending reset packet")?;
    eprintln!("done. radio will reboot.");
    Ok(())
}
