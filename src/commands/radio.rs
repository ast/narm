use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand, ValueEnum};
use clap_stdin::FileOrStdout;

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
    /// Upload a raw EEPROM image to the radio (channels + settings
    /// region only — the factory calibration block at 0x1d00..0x2000
    /// is never overwritten).
    #[command(visible_alias = "w")]
    Write(WriteArgs),
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
    /// Output file, or `-` for stdout (default).
    #[arg(long, short, default_value = "-")]
    pub out: FileOrStdout,
}

#[derive(Args, Debug)]
pub struct WriteArgs {
    /// Serial port the radio is on (e.g. /dev/ttyUSB0, COM3).
    #[arg(long)]
    pub port: String,
    /// Target radio. Only `quansheng-uv-k5` is supported in this
    /// release.
    #[arg(long, value_enum)]
    pub radio: Radio,
    /// Raw EEPROM image to upload. Must be either `WRITABLE_SIZE`
    /// (0x1d00 = 7424) bytes or full `EEPROM_SIZE` (0x2000 = 8192)
    /// bytes; in the latter case the calibration tail is silently
    /// dropped.
    #[arg(long)]
    pub from: PathBuf,
}

pub fn run(args: RadioArgs) -> Result<()> {
    match args.command {
        RadioCommand::Read(a) => run_read(a),
        RadioCommand::Write(a) => run_write(a),
    }
}

fn run_read(args: ReadArgs) -> Result<()> {
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

    args.out
        .into_writer()
        .context("opening output")?
        .write_all(&bytes)
        .context("writing output")?;
    Ok(())
}

fn run_write(args: WriteArgs) -> Result<()> {
    if args.radio != Radio::QuanshengUvK5 {
        bail!(
            "radio write is only implemented for quansheng-uv-k5 (got {})",
            args.radio.id()
        );
    }

    let image = std::fs::read(&args.from)
        .with_context(|| format!("reading image {}", args.from.display()))?;
    if image.len() != uvk5::WRITABLE_SIZE && image.len() != uvk5::EEPROM_SIZE {
        bail!(
            "image must be {} or {} bytes (got {})",
            uvk5::WRITABLE_SIZE,
            uvk5::EEPROM_SIZE,
            image.len()
        );
    }

    let mut port = uvk5::open_port(&args.port)
        .with_context(|| format!("opening serial port {}", args.port))?;

    eprintln!(
        "writing {} bytes from {} to {} (calibration block 0x1d00..0x2000 preserved)…",
        uvk5::WRITABLE_SIZE,
        args.from.display(),
        args.port
    );
    let written = uvk5::write_eeprom(&mut *port, &image).context("writing eeprom to radio")?;
    eprintln!("wrote {written} bytes; resetting radio…");
    uvk5::reset_radio(&mut *port).context("sending reset packet")?;
    eprintln!("done. radio will reboot.");
    Ok(())
}
