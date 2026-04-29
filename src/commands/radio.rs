use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand, ValueEnum};
use clap_stdin::FileOrStdout;

use narm::Radio;
use narm::channel::Config;
use narm::{kgq336, uvk5};

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
#[group(required = true, multiple = false)]
pub struct ReadSource {
    /// Serial port the radio is on (e.g. /dev/ttyUSB0, COM3).
    /// Mutually exclusive with `--from-file`.
    #[arg(long)]
    pub port: Option<String>,
    /// Decode a saved codeplug file instead of reading from
    /// the radio. For `wouxun-kg-q336` this is a `.kg` file
    /// from the vendor CPS.
    #[arg(long)]
    pub from_file: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct ReadArgs {
    #[command(flatten)]
    pub source: ReadSource,
    /// Target radio. `quansheng-uv-k5` supports both serial
    /// and file reads; `wouxun-kg-q336` currently supports
    /// only `--from-file`.
    #[arg(long, value_enum)]
    pub radio: Radio,
    /// Output format. `toml` decodes channels; `raw` writes the
    /// unmodified EEPROM/codeplug bytes.
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
    let image = match (
        args.source.port.as_deref(),
        args.source.from_file.as_deref(),
    ) {
        (Some(port), None) => read_from_port(args.radio, port)?,
        (None, Some(path)) => read_from_file(args.radio, path)?,
        // Clap's `required = true, multiple = false` group keeps
        // these two unreachable, but spell them out for clarity.
        (None, None) => bail!("either --port or --from-file is required"),
        (Some(_), Some(_)) => bail!("--port and --from-file are mutually exclusive"),
    };

    let bytes: Vec<u8> = match args.format {
        ReadFormat::Raw => image,
        ReadFormat::Toml => decode_to_toml(args.radio, &image)?,
    };

    args.out
        .into_writer()
        .context("opening output")?
        .write_all(&bytes)
        .context("writing output")?;
    Ok(())
}

fn read_from_port(radio: Radio, port: &str) -> Result<Vec<u8>> {
    if radio != Radio::QuanshengUvK5 {
        bail!(
            "live serial read is only implemented for quansheng-uv-k5 (got {})",
            radio.id()
        );
    }
    let mut p = uvk5::open_port(port).with_context(|| format!("opening serial port {port}"))?;
    let eeprom = uvk5::read_eeprom(&mut *p).context("reading eeprom from radio")?;
    eprintln!("read {} bytes from EEPROM", eeprom.len());
    Ok(eeprom)
}

fn read_from_file(radio: Radio, path: &std::path::Path) -> Result<Vec<u8>> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    match radio {
        Radio::WouxunKgQ336 => {
            let raw = kgq336::unmojibake(&bytes).context("de-mojibaking .kg file")?;
            eprintln!("recovered {} bytes from {}", raw.len(), path.display());
            Ok(raw)
        }
        Radio::QuanshengUvK5 => {
            // For UV-K5, the file is a raw EEPROM dump (what
            // `--format raw` produces).
            eprintln!("loaded {} bytes from {}", bytes.len(), path.display());
            Ok(bytes)
        }
        other => bail!(
            "--from-file decoding is not implemented for {} yet",
            other.id()
        ),
    }
}

fn decode_to_toml(radio: Radio, image: &[u8]) -> Result<Vec<u8>> {
    let (channels, warnings) = match radio {
        Radio::QuanshengUvK5 => {
            let r = uvk5::decode_channels(image).context("decoding channels")?;
            (r.channels, r.warnings)
        }
        Radio::WouxunKgQ336 => {
            let r = kgq336::decode_channels(image).context("decoding channels")?;
            if let Some(msg) = &r.startup_message {
                eprintln!("startup message: {msg}");
            }
            if !r.vfo_state.is_empty() {
                eprintln!(
                    "VFO state: {} entries; first = {} Hz",
                    r.vfo_state.len(),
                    r.vfo_state[0].rx_hz
                );
            }
            if !r.fm_broadcast.is_empty() {
                let unique: std::collections::BTreeSet<u64> =
                    r.fm_broadcast.iter().copied().collect();
                eprintln!(
                    "FM broadcast presets: {} entries, {} unique frequencies",
                    r.fm_broadcast.len(),
                    unique.len()
                );
            }
            (r.channels, r.warnings)
        }
        other => bail!("channel decoding is not implemented for {} yet", other.id()),
    };
    for w in &warnings {
        eprintln!("warning: {w}");
    }
    eprintln!("decoded {} channels", channels.len());
    let cfg = Config { channels };
    Ok(toml::to_string(&cfg)
        .context("serialising channels to TOML")?
        .into_bytes())
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
