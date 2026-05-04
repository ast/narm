use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Args;

use narm::kgq336;

use crate::commands::Cli;

#[derive(Args, Debug)]
pub struct InfoArgs {
    /// Path to a config file (`.toml`) or codeplug file
    /// (`.kg`, raw `.bin`).
    pub file: PathBuf,
}

pub fn run(_cli: &Cli, args: &InfoArgs) -> Result<()> {
    let bytes =
        std::fs::read(&args.file).with_context(|| format!("reading {}", args.file.display()))?;

    // Auto-detect by magic / size first, then by extension. We
    // can't trust the extension on everything (a `.bin` could be
    // any radio's image), so the strong signals come first.
    let label = if bytes.starts_with(b"xiepinruanjian\r\n") {
        "kg-file"
    } else if bytes.len() == kgq336::PHYSICAL_LEN {
        "raw-radio-dump"
    } else if bytes.len() == kgq336::KG_SHAPE_LEN {
        "kg-shape"
    } else {
        // Fall back to extension-based dispatch for text files.
        return match args
            .file
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_ascii_lowercase())
            .as_deref()
        {
            Some("toml") => print_toml(&args.file),
            _ => bail!(
                "unrecognised file: not a `.kg` (`xiepinruanjian` header), \
                 not a {}-byte raw KG-Q336 image, not a {}-byte `.kg`-shape \
                 image, and not `.toml` ({} bytes)",
                kgq336::PHYSICAL_LEN,
                kgq336::KG_SHAPE_LEN,
                bytes.len()
            ),
        };
    };
    let raw = kgq336::to_kg_shape(bytes).context("normalising KG-Q336 image to .kg shape")?;
    kgq336::inspect::print_report(&args.file, label, &raw)
}

fn print_toml(path: &std::path::Path) -> Result<()> {
    let cfg = narm::load_from_path(path)?;
    narm::validate(&cfg)?;
    println!("file:     {}", path.display());
    println!("format:   toml");
    println!("channels: {}", cfg.channels.len());
    Ok(())
}
