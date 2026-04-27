use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
pub struct Config {
    #[serde(default)]
    pub channels: Vec<Channel>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Channel {
    pub name: String,
    pub rx_hz: u64,
    #[serde(default)]
    pub shift_hz: i64,
    #[serde(default)]
    pub power: Power,
    #[serde(default)]
    pub scan: bool,
    #[serde(flatten)]
    pub mode: Mode,
    #[serde(skip)]
    pub source: Option<PathBuf>,
}

#[derive(Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Power {
    Low,
    #[default]
    Mid,
    High,
}

#[derive(Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Bandwidth {
    Narrow,
    #[default]
    Wide,
}

#[derive(Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Admit {
    #[default]
    Always,
    ChannelFree,
    ColorCodeFree,
}

#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum C4fmRate {
    Dn,
    Vw,
    Voice,
    Data,
}

#[derive(Deserialize, Debug, Clone)]
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
    pub fn kind(&self) -> &'static str {
        match self {
            Mode::Fm { .. } => "fm",
            Mode::Dmr { .. } => "dmr",
            Mode::Dstar { .. } => "dstar",
            Mode::C4fm { .. } => "c4fm",
            Mode::P25 { .. } => "p25",
            Mode::M17 { .. } => "m17",
        }
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
    #[error(
        "channel {name}: rx_hz {hz} outside any supported ham band{}",
        format_loc(source_path)
    )]
    OutOfBand {
        name: String,
        hz: u64,
        source_path: Option<PathBuf>,
    },
}

fn format_dup_locs(first: &Option<PathBuf>, second: &Option<PathBuf>) -> String {
    match (first, second) {
        (Some(a), Some(b)) => format!(": in {} and {}", a.display(), b.display()),
        _ => String::new(),
    }
}

fn format_loc(source: &Option<PathBuf>) -> String {
    match source {
        Some(p) => format!(" (in {})", p.display()),
        None => String::new(),
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
        if !in_supported_band(ch.rx_hz) {
            return Err(NarmError::OutOfBand {
                name: ch.name.clone(),
                hz: ch.rx_hz,
                source_path: ch.source.clone(),
            });
        }
    }
    Ok(())
}

fn in_supported_band(hz: u64) -> bool {
    // 2 m: 144–148 MHz, 70 cm: 420–450 MHz. Extend as more radios land.
    (144_000_000..=148_000_000).contains(&hz) || (420_000_000..=450_000_000).contains(&hz)
}
