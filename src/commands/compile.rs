use std::fs;
use std::io::{self, Write};

use anyhow::{Context, Result};

use crate::commands::{CompileArgs, CompileFormat};

pub fn run(args: &CompileArgs) -> Result<()> {
    let cfg = narm::load_from_path(&args.config)?;
    narm::validate(&cfg)?;

    let radio = args.radio;
    let supported: &[&str] = radio.supported_modes();

    let mut filtered = Vec::with_capacity(cfg.channels.len());
    let mut filter_warnings = Vec::new();
    for ch in cfg.channels {
        let mode_kind = ch.mode.kind();
        if supported.contains(&mode_kind) {
            filtered.push(ch);
        } else {
            filter_warnings.push(format!(
                "skipping {}: mode {} not supported by {}",
                ch.name,
                mode_kind,
                radio.display_name()
            ));
        }
    }

    let report = match args.format {
        CompileFormat::ChirpCsv => narm::chirp::channels_to_csv(&filtered)?,
    };

    for w in &filter_warnings {
        eprintln!("warning: {w}");
    }
    for w in &report.warnings {
        eprintln!("warning: {w}");
    }

    match &args.out {
        Some(path) => fs::write(path, &report.csv)
            .with_context(|| format!("writing output to {}", path.display()))?,
        None => io::stdout()
            .write_all(report.csv.as_bytes())
            .context("writing csv to stdout")?,
    }
    Ok(())
}
