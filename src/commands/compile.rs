use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args, ValueEnum};

use narm::Radio;

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
#[clap(rename_all = "kebab-case")]
pub enum CompileFormat {
    /// Generic CHIRP CSV — importable by CHIRP for any supported radio.
    ChirpCsv,
}

#[derive(Args, Debug)]
pub struct CompileArgs {
    /// Path to a TOML config file or a directory of `*.toml` files.
    pub config: PathBuf,
    /// Target radio.
    #[arg(long, value_enum)]
    pub radio: Radio,
    /// Output format. Defaults to chirp-csv (the universal CHIRP
    /// generic-CSV interchange).
    #[arg(long, value_enum, default_value_t = CompileFormat::ChirpCsv)]
    pub format: CompileFormat,
    /// Output file (defaults to stdout).
    #[arg(long, short)]
    pub out: Option<PathBuf>,
}

pub fn run(args: &CompileArgs) -> Result<()> {
    let cfg = narm::load_from_path(&args.config)?;
    narm::validate(&cfg)?;

    let radio = args.radio;
    let spec = radio.spec();

    let mut filtered = Vec::with_capacity(cfg.channels.len());
    let mut filter_warnings = Vec::new();
    for ch in cfg.channels {
        let mode_kind = ch.mode.kind();
        if !spec.supported_modes.contains(&mode_kind) {
            filter_warnings.push(format!(
                "skipping {}: mode {} not supported by {}",
                ch.name, mode_kind, spec.display_name
            ));
            continue;
        }
        if !spec.covers_rx(ch.rx_hz) {
            filter_warnings.push(format!(
                "skipping {}: rx {:.4} MHz outside {}'s coverage",
                ch.name,
                ch.rx_hz as f64 / 1_000_000.0,
                spec.display_name
            ));
            continue;
        }
        filtered.push(ch);
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
