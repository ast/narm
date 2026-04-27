use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Config {
    #[serde(default)]
    pub channels: Vec<Channel>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Channel {
    pub name: String,
    pub rx_hz: u64,
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub shift_hz: i64,
    #[serde(default, skip_serializing_if = "is_default_power")]
    pub power: Power,
    #[serde(default, skip_serializing_if = "is_false")]
    pub scan: bool,
    #[serde(flatten)]
    pub mode: Mode,
    #[serde(skip)]
    pub source: Option<PathBuf>,
}

fn is_zero_i64(v: &i64) -> bool {
    *v == 0
}
fn is_default_power(p: &Power) -> bool {
    *p == Power::default()
}
fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Deserialize, Serialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Power {
    Low,
    #[default]
    Mid,
    High,
}

#[derive(Deserialize, Serialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Bandwidth {
    Narrow,
    #[default]
    Wide,
}

#[derive(Deserialize, Serialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Admit {
    #[default]
    Always,
    ChannelFree,
    ColorCodeFree,
}

#[derive(Deserialize, Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum C4fmRate {
    Dn,
    Vw,
    Voice,
    Data,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum Mode {
    Fm {
        #[serde(default)]
        bandwidth: Bandwidth,
        #[serde(default)]
        tone_tx_hz: Option<f32>,
        #[serde(default)]
        tone_rx_hz: Option<f32>,
        #[serde(default)]
        dcs_code: Option<u16>,
    },
    /// Amplitude modulation. Wide AM is broadcast / aviation; narrow
    /// AM is rarer. Tone fields exist for symmetry with `Fm` but most
    /// AM use cases (aviation 108–137 MHz) don't use CTCSS/DCS.
    Am {
        #[serde(default)]
        bandwidth: Bandwidth,
        #[serde(default)]
        tone_tx_hz: Option<f32>,
        #[serde(default)]
        tone_rx_hz: Option<f32>,
        #[serde(default)]
        dcs_code: Option<u16>,
    },
    Dmr {
        color_code: u8,
        slot: u8,
        talkgroup: u32,
        #[serde(default)]
        admit: Admit,
    },
    Dstar {
        urcall: String,
        rpt1: String,
        rpt2: String,
    },
    C4fm {
        dg_id_tx: u8,
        dg_id_rx: u8,
        data_rate: C4fmRate,
    },
    P25 {
        nac: u16,
        talkgroup: u32,
    },
    M17 {
        destination: String,
        can: u8,
    },
}

impl Mode {
    pub fn kind(&self) -> ModeKind {
        match self {
            Mode::Fm { .. } => ModeKind::Fm,
            Mode::Am { .. } => ModeKind::Am,
            Mode::Dmr { .. } => ModeKind::Dmr,
            Mode::Dstar { .. } => ModeKind::Dstar,
            Mode::C4fm { .. } => ModeKind::C4fm,
            Mode::P25 { .. } => ModeKind::P25,
            Mode::M17 { .. } => ModeKind::M17,
        }
    }
}

/// Discriminant of [`Mode`] without the per-mode payload. Used by
/// [`crate::radio::RadioSpec::supported_modes`] and the compile
/// filter. Display gives the kebab-case wire form (`"fm"`, `"am"`,
/// `"dmr"`, `"dstar"`, `"c4fm"`, `"p25"`, `"m17"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModeKind {
    Fm,
    Am,
    Dmr,
    Dstar,
    C4fm,
    P25,
    M17,
}

impl ModeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ModeKind::Fm => "fm",
            ModeKind::Am => "am",
            ModeKind::Dmr => "dmr",
            ModeKind::Dstar => "dstar",
            ModeKind::C4fm => "c4fm",
            ModeKind::P25 => "p25",
            ModeKind::M17 => "m17",
        }
    }
}

impl std::fmt::Display for ModeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(thiserror::Error, Debug)]
pub enum NarmError {
    #[error("failed to read {path}: {source}", path = path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse TOML in {path}: {source}", path = path.display())]
    Toml {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("duplicate channel name {name:?}{}", format_dup_locs(first, second))]
    DuplicateName {
        name: String,
        first: Option<PathBuf>,
        second: Option<PathBuf>,
    },
}

fn format_dup_locs(first: &Option<PathBuf>, second: &Option<PathBuf>) -> String {
    match (first, second) {
        (Some(a), Some(b)) => format!(": in {} and {}", a.display(), b.display()),
        _ => String::new(),
    }
}

pub fn load_from_path(path: &Path) -> Result<Config, NarmError> {
    let metadata = std::fs::metadata(path).map_err(|source| NarmError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.is_dir() {
        load_dir(path)
    } else {
        load_file(path)
    }
}

fn load_file(path: &Path) -> Result<Config, NarmError> {
    let text = std::fs::read_to_string(path).map_err(|source| NarmError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut cfg: Config = toml::from_str(&text).map_err(|source| NarmError::Toml {
        path: path.to_path_buf(),
        source,
    })?;
    for ch in &mut cfg.channels {
        ch.source = Some(path.to_path_buf());
    }
    Ok(cfg)
}

fn load_dir(dir: &Path) -> Result<Config, NarmError> {
    let entries = std::fs::read_dir(dir).map_err(|source| NarmError::Io {
        path: dir.to_path_buf(),
        source,
    })?;

    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            let stem_visible = p
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| !n.starts_with('.'));
            let is_toml = p
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("toml"));
            stem_visible && is_toml && p.is_file()
        })
        .collect();
    paths.sort();

    let mut merged = Config {
        channels: Vec::new(),
    };
    for p in paths {
        let cfg = load_file(&p)?;
        merged.channels.extend(cfg.channels);
    }
    Ok(merged)
}

/// Structural validation: duplicate channel-name detection. Per-radio
/// frequency / mode coverage is checked at compile time against the
/// target radio's [`crate::radio::RadioSpec`], not here — a config
/// that's invalid for one radio may be perfectly valid for another.
pub fn validate(cfg: &Config) -> Result<(), NarmError> {
    let mut seen: HashMap<&str, &Option<PathBuf>> = HashMap::new();
    for ch in &cfg.channels {
        if let Some(first_src) = seen.get(ch.name.as_str()) {
            return Err(NarmError::DuplicateName {
                name: ch.name.clone(),
                first: (*first_src).clone(),
                second: ch.source.clone(),
            });
        }
        seen.insert(&ch.name, &ch.source);
    }
    Ok(())
}
