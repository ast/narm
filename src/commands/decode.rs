use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Args;

use narm::Radio;
use narm::channel::Config;
use narm::{kgq336, uvk5};

use crate::commands::Cli;
use crate::commands::format::{Format, resolve};

#[derive(Args, Debug)]
pub struct DecodeArgs {
    /// Target radio.
    #[arg(short = 'R', long, value_enum)]
    pub radio: Radio,

    /// Binary codeplug file (`.bin` raw image or `.kg` vendor file).
    pub file: PathBuf,
}

pub fn run(cli: &Cli, args: &DecodeArgs) -> Result<()> {
    let in_format = resolve(cli.format_flag(), Some(&args.file))?;
    let out_format = match cli.out.as_deref() {
        Some(p) => resolve(cli.format_flag(), Some(p)).unwrap_or(Format::Toml),
        None => Format::Toml,
    };

    if !matches!(in_format, Format::Bin | Format::Manufacturer) {
        bail!(
            "decode input must be -b/--bin or -m/--manufacturer (got `{}`)",
            in_format.as_str()
        );
    }
    if out_format != Format::Toml {
        bail!(
            "decode currently only emits TOML; `{}` output is not implemented",
            out_format.as_str()
        );
    }

    let bytes =
        std::fs::read(&args.file).with_context(|| format!("reading {}", args.file.display()))?;

    // Format flag (or path extension) decides how to interpret
    // the bytes — no content sniffing.
    let image = match (args.radio, in_format) {
        (Radio::WouxunKgQ336, Format::Bin) => {
            if bytes.len() != kgq336::PHYSICAL_LEN {
                bail!(
                    "expected {}-byte physical EEPROM dump for {} -b (got {})",
                    kgq336::PHYSICAL_LEN,
                    args.radio.id(),
                    bytes.len()
                );
            }
            let logical = kgq336::unscramble(&bytes);
            kgq336::logical_to_kg_shape(&logical)
        }
        (Radio::WouxunKgQ336, Format::Manufacturer) => {
            kgq336::unmojibake(&bytes).context("de-mojibaking .kg file")?
        }
        (Radio::QuanshengUvK5, Format::Bin) => bytes,
        (_, Format::Toml | Format::Yaml | Format::Csv) => bail!(
            "decode input must be a binary format (-b or -m), got `{}`",
            in_format.as_str()
        ),
        (other, _) => bail!("decoding is not implemented for {} yet", other.id()),
    };

    let toml_bytes = decode_to_toml(args.radio, &image)?;
    write_output(cli.out.as_deref(), &toml_bytes)
}

fn write_output(out: Option<&std::path::Path>, bytes: &[u8]) -> Result<()> {
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

fn decode_to_toml(radio: Radio, image: &[u8]) -> Result<Vec<u8>> {
    let (channels, warnings) = match radio {
        Radio::QuanshengUvK5 => {
            let r = uvk5::decode_channels(image).context("decoding channels")?;
            (r.channels, r.warnings)
        }
        Radio::WouxunKgQ336 => {
            let r = kgq336::decode_channels(image).context("decoding channels")?;
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
