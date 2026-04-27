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
