use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Args;

use narm::Radio;

use crate::commands::Cli;
use crate::commands::format::{Format, resolve};

#[derive(Args, Debug)]
pub struct EncodeArgs {
    /// Target radio.
    #[arg(short = 'R', long, value_enum)]
    pub radio: Radio,

    /// Path to a TOML config file or a directory of `*.toml` files.
    pub config: PathBuf,
}

pub fn run(cli: &Cli, args: &EncodeArgs) -> Result<()> {
    let format = resolve(cli.format_flag(), cli.out.as_deref())?;
    if format != Format::Csv {
        bail!(
            "encode currently only emits CSV (`-c`); `{}` is not yet implemented",
            format.as_str()
        );
    }

    let cfg = narm::load_from_path(&args.config)?;
    narm::validate(&cfg)?;

    let spec = args.radio.spec();

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

    let report = narm::chirp::channels_to_csv(&filtered)?;

    for w in filter_warnings.iter().chain(report.warnings.iter()) {
        eprintln!("warning: {w}");
    }

    match cli.out.as_deref() {
        Some(path) if path.as_os_str() != "-" => std::fs::write(path, &report.csv)
            .with_context(|| format!("writing output to {}", path.display()))?,
        _ => {
            use std::io::Write;
            std::io::stdout()
                .write_all(report.csv.as_bytes())
                .context("writing csv to stdout")?
        }
    }
    Ok(())
}
