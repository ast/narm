use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};

use narm::Radio;
use narm::channel::Config;
use narm::uvk5;

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

    let report = uvk5::decode_channels(&eeprom).context("decoding channels")?;
    for w in &report.warnings {
        eprintln!("warning: {w}");
    }
    eprintln!("decoded {} channels", report.channels.len());

    let cfg = Config {
        channels: report.channels,
    };
    let toml = toml::to_string(&cfg).context("serialising channels to TOML")?;

    match &args.out {
        Some(path) => std::fs::write(path, &toml)
            .with_context(|| format!("writing TOML to {}", path.display()))?,
        None => io::stdout()
            .write_all(toml.as_bytes())
            .context("writing TOML to stdout")?,
    }
    Ok(())
}
