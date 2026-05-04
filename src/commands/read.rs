use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Args;

use narm::Radio;
use narm::channel::Config;
use narm::{kgq336, uvk5};

use crate::commands::Cli;
use crate::commands::format::{self, Format};

#[derive(Args, Debug)]
pub struct ReadArgs {
    /// Decode a saved codeplug file instead of reading from the
    /// radio. For `wouxun-kg-q336` this is a `.kg` file from the
    /// vendor CPS, or a `.bin` raw EEPROM dump (32 KiB) saved by
    /// `narm read -D <port> -b`. Mutually exclusive with `-D`.
    #[arg(long)]
    pub from_file: Option<PathBuf>,
}

pub fn run(cli: &Cli, args: &ReadArgs) -> Result<()> {
    let radio = cli
        .radio
        .ok_or_else(|| anyhow::anyhow!("--radio/-R is required"))?;

    match (cli.device.as_deref(), args.from_file.as_deref()) {
        (Some(port), None) => run_live(cli, radio, port),
        (None, Some(path)) => run_from_file(cli, radio, path),
        (None, None) => bail!("either -D/--device or --from-file is required"),
        (Some(_), Some(_)) => bail!("-D/--device and --from-file are mutually exclusive"),
    }
}

/// Live read over serial.
///
/// The format flag describes the *output* format here (matching
/// `dmrconf read`): the file is what comes out, the radio is the
/// input. Default = TOML.
fn run_live(cli: &Cli, radio: Radio, port: &str) -> Result<()> {
    let out_fmt = cli
        .format_flag()
        .explicit()
        .or_else(|| cli.out.as_deref().and_then(format::from_path))
        .unwrap_or(Format::Toml);

    let raw = read_from_port(radio, port)?;
    let bytes = encode_live_output(radio, raw, out_fmt)?;
    write_output(cli.out.as_deref(), &bytes)
}

/// Offline `--from-file` decoding.
///
/// The format flag describes the *input* format here (the file is
/// the input). Output is TOML by default, or inferred from `-o`'s
/// extension.
fn run_from_file(cli: &Cli, radio: Radio, path: &Path) -> Result<()> {
    let in_fmt = format::resolve(cli.format_flag(), Some(path))?;
    let out_fmt = cli
        .out
        .as_deref()
        .and_then(format::from_path)
        .unwrap_or(Format::Toml);

    // Same-format → byte-for-byte copy. Avoids needing inverse
    // transforms (e.g. kg-shape → physical) we don't have.
    if in_fmt == out_fmt {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        return write_output(cli.out.as_deref(), &bytes);
    }

    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    eprintln!(
        "loaded {} bytes ({}) from {}",
        bytes.len(),
        in_fmt.as_str(),
        path.display()
    );

    let canonical = decode_input(radio, in_fmt, bytes)?;
    let out_bytes = encode_canonical(radio, &canonical, out_fmt)?;
    write_output(cli.out.as_deref(), &out_bytes)
}

fn write_output(out: Option<&Path>, bytes: &[u8]) -> Result<()> {
    match out {
        None => std::io::stdout()
            .write_all(bytes)
            .context("writing to stdout"),
        Some(p) if p.as_os_str() == "-" => std::io::stdout()
            .write_all(bytes)
            .context("writing to stdout"),
        Some(p) => std::fs::write(p, bytes).with_context(|| format!("writing {}", p.display())),
    }
}

fn read_from_port(radio: Radio, port: &str) -> Result<Vec<u8>> {
    match radio {
        Radio::QuanshengUvK5 => {
            let mut p =
                uvk5::open_port(port).with_context(|| format!("opening serial port {port}"))?;
            let eeprom = uvk5::read_eeprom(&mut *p).context("reading eeprom from radio")?;
            eprintln!("read {} bytes from EEPROM", eeprom.len());
            Ok(eeprom)
        }
        Radio::WouxunKgQ336 => {
            let mut p =
                kgq336::open_port(port).with_context(|| format!("opening serial port {port}"))?;
            let image = kgq336::read_codeplug(&mut *p).context("reading codeplug from radio")?;
            eprintln!("read {} bytes of radio memory", image.len());
            Ok(image)
        }
        other => bail!("live serial read is not implemented for {} yet", other.id()),
    }
}

/// Decode a file input (`-b` or `-m`) into the radio's canonical
/// in-memory shape: `.kg`-shape (50 KiB) for KG-Q336, raw EEPROM
/// for UV-K5.
fn decode_input(radio: Radio, fmt: Format, bytes: Vec<u8>) -> Result<Vec<u8>> {
    match (radio, fmt) {
        (Radio::WouxunKgQ336, Format::Bin) => {
            if bytes.len() != kgq336::PHYSICAL_LEN {
                bail!(
                    "expected {}-byte physical EEPROM dump for {} -b (got {})",
                    kgq336::PHYSICAL_LEN,
                    radio.id(),
                    bytes.len()
                );
            }
            let logical = kgq336::unscramble(&bytes);
            Ok(kgq336::logical_to_kg_shape(&logical))
        }
        (Radio::WouxunKgQ336, Format::Manufacturer) => {
            kgq336::unmojibake(&bytes).context("de-mojibaking .kg file")
        }
        (Radio::QuanshengUvK5, Format::Bin) => Ok(bytes),
        (_, Format::Toml | Format::Yaml | Format::Csv) => bail!(
            "input format `{}` is not a binary codeplug; pass -b or -m",
            fmt.as_str()
        ),
        (other, _) => bail!(
            "input format `{}` is not implemented for {} yet",
            fmt.as_str(),
            other.id()
        ),
    }
}

/// Encode the canonical in-memory shape (`.kg`-shape for Q336,
/// raw EEPROM for UV-K5) to the requested output format.
///
/// Note: there's no kg-shape → physical inverse for Q336, so a
/// `-m → -b` conversion bails. (Same-format copy is handled by
/// the caller before reaching here.)
fn encode_canonical(radio: Radio, canonical: &[u8], fmt: Format) -> Result<Vec<u8>> {
    match (radio, fmt) {
        (Radio::WouxunKgQ336, Format::Bin) => bail!(
            "kg-shape → 32 KiB physical EEPROM conversion is not implemented; \
             only same-format copy supports -b output for {}",
            radio.id()
        ),
        (Radio::WouxunKgQ336, Format::Manufacturer) => Ok(kgq336::mojibake(canonical)),
        (_, Format::Toml) => decode_to_toml(radio, canonical),
        (_, Format::Bin) => Ok(canonical.to_vec()),
        (_, Format::Yaml | Format::Csv) => {
            bail!("output format `{}` is not implemented yet", fmt.as_str())
        }
        (other, _) => bail!(
            "output format `{}` not implemented for {} yet",
            fmt.as_str(),
            other.id()
        ),
    }
}

/// Live-read encoding: the radio returns its native binary shape
/// (32 KiB physical for Q336, raw EEPROM for UV-K5). For `-b` we
/// emit those bytes verbatim; for everything else we transform.
fn encode_live_output(radio: Radio, raw: Vec<u8>, fmt: Format) -> Result<Vec<u8>> {
    match (radio, fmt) {
        (_, Format::Bin) => Ok(raw),
        (Radio::WouxunKgQ336, Format::Manufacturer) => {
            let logical = kgq336::unscramble(&raw);
            let kg = kgq336::logical_to_kg_shape(&logical);
            Ok(kgq336::mojibake(&kg))
        }
        (Radio::WouxunKgQ336, Format::Toml) => {
            let logical = kgq336::unscramble(&raw);
            let kg = kgq336::logical_to_kg_shape(&logical);
            decode_to_toml(radio, &kg)
        }
        (Radio::QuanshengUvK5, Format::Toml) => decode_to_toml(radio, &raw),
        (Radio::QuanshengUvK5, Format::Manufacturer) => {
            bail!("UV-K5 has no manufacturer file format")
        }
        (_, Format::Yaml | Format::Csv) => {
            bail!("output format `{}` is not implemented yet", fmt.as_str())
        }
        (other, _) => bail!(
            "output format `{}` not implemented for {} yet",
            fmt.as_str(),
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
