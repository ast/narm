use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand, ValueEnum};

use narm::Radio;
use narm::channel::Config;
use narm::uvk5;

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
#[clap(rename_all = "kebab-case")]
pub enum ReadFormat {
    /// Decode channel records into narm's TOML schema.
    Toml,
    /// Write the unmodified EEPROM bytes (calibration, settings,
    /// channel-attributes block, etc.) — suitable for full backup.
    Raw,
}

#[derive(Args, Debug)]
pub struct RadioArgs {
    #[command(subcommand)]
    pub command: RadioCommand,
}

#[derive(Subcommand, Debug)]
pub enum RadioCommand {
    /// Read channel data from a connected radio over serial.
    #[command(visible_alias = "r")]
    Read(ReadArgs),
}

#[derive(Args, Debug)]
pub struct ReadArgs {
    /// Serial port the radio is on (e.g. /dev/ttyUSB0, COM3).
    #[arg(long)]
    pub port: String,
    /// Target radio. Only `quansheng-uv-k5` is supported in this
    /// release; other radios will reject with an error.
    #[arg(long, value_enum)]
    pub radio: Radio,
    /// Output format. `toml` decodes channels; `raw` writes the
    /// unmodified 8 KiB EEPROM image (full backup).
    #[arg(long, value_enum, default_value_t = ReadFormat::Toml)]
    pub format: ReadFormat,
    /// Output file (defaults to stdout).
    #[arg(long, short)]
    pub out: Option<PathBuf>,
}

pub fn run(args: &RadioArgs) -> Result<()> {
    match &args.command {
        RadioCommand::Read(a) => run_read(a),
    }
}

fn run_read(args: &ReadArgs) -> Result<()> {
    if args.radio != Radio::QuanshengUvK5 {
        bail!(
            "radio read is only implemented for quansheng-uv-k5 (got {})",
            args.radio.id()
        );
    }

    let mut port = uvk5::open_port(&args.port)
        .with_context(|| format!("opening serial port {}", args.port))?;

    eprintln!("connecting to radio on {}…", args.port);
    let eeprom = uvk5::read_eeprom(&mut *port).context("reading eeprom from radio")?;
    eprintln!("read {} bytes from EEPROM", eeprom.len());

    let bytes: Vec<u8> = match args.format {
        ReadFormat::Raw => eeprom,
        ReadFormat::Toml => {
            let report = uvk5::decode_channels(&eeprom).context("decoding channels")?;
            for w in &report.warnings {
                eprintln!("warning: {w}");
            }
            eprintln!("decoded {} channels", report.channels.len());
            let cfg = Config {
                channels: report.channels,
            };
            toml::to_string(&cfg)
                .context("serialising channels to TOML")?
                .into_bytes()
        }
    };

    match &args.out {
        Some(path) => std::fs::write(path, &bytes)
            .with_context(|| format!("writing output to {}", path.display()))?,
        None => io::stdout()
            .write_all(&bytes)
            .context("writing output to stdout")?,
    }
    Ok(())
}
