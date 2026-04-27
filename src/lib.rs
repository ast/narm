use std::collections::HashSet;
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

#[derive(clap::ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
#[clap(rename_all = "kebab-case")]
pub enum Radio {
    WouxunKgQ336,
    QuanshengUvK5,
    TytMd380,
    AnytoneAtD878uv,
    YaesuFt50r,
}

impl Radio {
    pub const ALL: [Radio; 5] = [
        Radio::WouxunKgQ336,
        Radio::QuanshengUvK5,
        Radio::TytMd380,
        Radio::AnytoneAtD878uv,
        Radio::YaesuFt50r,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Radio::WouxunKgQ336 => "wouxun-kg-q336",
            Radio::QuanshengUvK5 => "quansheng-uv-k5",
            Radio::TytMd380 => "tyt-md-380",
            Radio::AnytoneAtD878uv => "anytone-at-d878uv",
            Radio::YaesuFt50r => "yaesu-ft-50r",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Radio::WouxunKgQ336 => "Wouxun KG-Q336",
            Radio::QuanshengUvK5 => "Quansheng UV-K5",
            Radio::TytMd380 => "TYT MD-380",
            Radio::AnytoneAtD878uv => "AnyTone AT-D878UV",
            Radio::YaesuFt50r => "Yaesu FT-50R",
        }
    }

    pub fn supported_modes(self) -> &'static [&'static str] {
        match self {
            Radio::WouxunKgQ336 => &["fm"],
            Radio::QuanshengUvK5 => &["fm"],
            Radio::TytMd380 => &["fm", "dmr"],
            Radio::AnytoneAtD878uv => &["fm", "dmr"],
            Radio::YaesuFt50r => &["fm"],
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum NarmError {
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse TOML: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("duplicate channel name: {0}")]
    DuplicateName(String),
    #[error("channel {name}: rx_hz {hz} outside any supported ham band")]
    OutOfBand { name: String, hz: u64 },
}

pub fn load_from_path(path: &Path) -> Result<Config, NarmError> {
    let text = std::fs::read_to_string(path).map_err(|source| NarmError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let cfg: Config = toml::from_str(&text)?;
    Ok(cfg)
}

pub fn validate(cfg: &Config) -> Result<(), NarmError> {
    let mut seen: HashSet<&str> = HashSet::new();
    for ch in &cfg.channels {
        if !seen.insert(ch.name.as_str()) {
            return Err(NarmError::DuplicateName(ch.name.clone()));
        }
        if !in_supported_band(ch.rx_hz) {
            return Err(NarmError::OutOfBand {
                name: ch.name.clone(),
                hz: ch.rx_hz,
            });
        }
    }
    Ok(())
}

fn in_supported_band(hz: u64) -> bool {
    // 2 m: 144–148 MHz, 70 cm: 420–450 MHz. Extend as more radios land.
    (144_000_000..=148_000_000).contains(&hz) || (420_000_000..=450_000_000).contains(&hz)
}
