use std::path::Path;
use std::str::FromStr;

use anyhow::{Result, bail};

/// Codeplug / config file format.
///
/// Mirrors `dmrconf`'s format flags (`-y/--yaml`, `-c/--csv`,
/// `-b/--bin`, `-m/--manufacturer`) plus narm's canonical
/// `-t/--toml`. Used by `read`, `write`, `encode`, `decode`,
/// and `info` to disambiguate the on-disk shape of a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// narm-canonical TOML.
    Toml,
    /// dmrconf-compatible YAML (interop target — emit not yet
    /// implemented).
    Yaml,
    /// CHIRP-generic CSV (current `compile` output).
    Csv,
    /// Raw radio image (was `radio read --format raw`).
    Bin,
    /// Vendor-native binary (e.g. `.kg` for KG-Q336).
    Manufacturer,
}

impl FromStr for Format {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        Ok(match s {
            "toml" => Format::Toml,
            "yaml" => Format::Yaml,
            "csv" => Format::Csv,
            "bin" => Format::Bin,
            "manufacturer" => Format::Manufacturer,
            other => bail!("unknown format: {other}"),
        })
    }
}

impl Format {
    pub fn as_str(self) -> &'static str {
        match self {
            Format::Toml => "toml",
            Format::Yaml => "yaml",
            Format::Csv => "csv",
            Format::Bin => "bin",
            Format::Manufacturer => "manufacturer",
        }
    }
}

/// Top-level format-flag selection. At most one of `-t/-y/-c/-b/-m`
/// may be set; clap's mutually-exclusive group enforces that.
#[derive(Debug, Clone, Copy, Default)]
pub struct FormatFlag {
    pub toml: bool,
    pub yaml: bool,
    pub csv: bool,
    pub bin: bool,
    pub manufacturer: bool,
}

impl FormatFlag {
    pub fn explicit(self) -> Option<Format> {
        match (self.toml, self.yaml, self.csv, self.bin, self.manufacturer) {
            (true, _, _, _, _) => Some(Format::Toml),
            (_, true, _, _, _) => Some(Format::Yaml),
            (_, _, true, _, _) => Some(Format::Csv),
            (_, _, _, true, _) => Some(Format::Bin),
            (_, _, _, _, true) => Some(Format::Manufacturer),
            _ => None,
        }
    }
}

/// Look up [`Format`] from a path's extension. Returns `None`
/// for unknown / missing extensions.
pub fn from_path(path: &Path) -> Option<Format> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "toml" => Format::Toml,
        "yaml" | "yml" => Format::Yaml,
        "csv" => Format::Csv,
        "bin" => Format::Bin,
        "kg" => Format::Manufacturer,
        _ => return None,
    })
}

/// Resolve format from an explicit `-t/-y/-c/-b/-m` flag, falling
/// back to the file extension if no flag was passed. Returns an
/// error when neither source can determine the format.
pub fn resolve(flag: FormatFlag, path: Option<&Path>) -> Result<Format> {
    if let Some(f) = flag.explicit() {
        return Ok(f);
    }
    if let Some(p) = path {
        if let Some(f) = from_path(p) {
            return Ok(f);
        }
        bail!(
            "cannot infer format from `{}`; pass one of -t/-y/-c/-b/-m",
            p.display()
        );
    }
    bail!("no format specified and no file path to infer from; pass one of -t/-y/-c/-b/-m")
}
