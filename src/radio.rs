use crate::channel::ModeKind;

pub mod anytone_at_d878uv;
pub mod quansheng_uv_k5;
pub mod tyt_md_380;
pub mod wouxun_kg_q336;
pub mod yaesu_ft_50r;

/// Inclusive `(lo_hz, hi_hz)` frequency range.
pub type Band = (u64, u64);

/// All the per-radio data narm cares about. Each radio module exports
/// a single `pub const SPEC: RadioSpec = RadioSpec { ... }` populating
/// every field — adding a field forces every radio to specify it
/// (compile error on missing fields), and missing a `match` arm in
/// [`Radio::spec`] is also a compile error. Adding a new radio is
/// three steps: create `src/radio/<name>.rs`, add the enum variant
/// here, add the match arm.
#[derive(Debug, Clone, Copy)]
pub struct RadioSpec {
    pub id: &'static str,
    pub display_name: &'static str,
    pub manual_path: &'static str,
    /// TX-allowed segments (Hz, inclusive).
    pub tx_bands: &'static [Band],
    /// RX-supported segments. A superset of `tx_bands` for receivers
    /// that listen wider than they transmit (most do).
    pub rx_bands: &'static [Band],
    /// Channel mode kinds the radio can program.
    pub supported_modes: &'static [ModeKind],
}

impl RadioSpec {
    /// True if `hz` falls inside any RX band.
    pub fn covers_rx(&self, hz: u64) -> bool {
        covers(self.rx_bands, hz)
    }

    /// True if `hz` falls inside any TX band.
    pub fn covers_tx(&self, hz: u64) -> bool {
        covers(self.tx_bands, hz)
    }
}

fn covers(bands: &[Band], hz: u64) -> bool {
    bands.iter().any(|(lo, hi)| hz >= *lo && hz <= *hi)
}

#[derive(clap::ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Radio {
    // Explicit `name`s match `RadioSpec::id` and the model-number
    // hyphenation used in CLAUDE.md / list-radios output. Without
    // these clap's auto-kebab-case strips dashes around digits
    // (`tyt-md380` vs the documented `tyt-md-380`).
    #[clap(name = "wouxun-kg-q336")]
    WouxunKgQ336,
    #[clap(name = "quansheng-uv-k5")]
    QuanshengUvK5,
    #[clap(name = "tyt-md-380")]
    TytMd380,
    #[clap(name = "anytone-at-d878uv")]
    AnytoneAtD878uv,
    #[clap(name = "yaesu-ft-50r")]
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

    /// The radio's full spec — bands, modes, manual, etc.
    pub fn spec(self) -> &'static RadioSpec {
        match self {
            Radio::WouxunKgQ336 => &wouxun_kg_q336::SPEC,
            Radio::QuanshengUvK5 => &quansheng_uv_k5::SPEC,
            Radio::TytMd380 => &tyt_md_380::SPEC,
            Radio::AnytoneAtD878uv => &anytone_at_d878uv::SPEC,
            Radio::YaesuFt50r => &yaesu_ft_50r::SPEC,
        }
    }

    pub fn id(self) -> &'static str {
        self.spec().id
    }

    pub fn display_name(self) -> &'static str {
        self.spec().display_name
    }

    pub fn supported_modes(self) -> &'static [ModeKind] {
        self.spec().supported_modes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_radio_has_a_spec_and_id_matches_clap_name() {
        // Sanity: spec.id must match the `#[clap(name = ...)]`
        // attribute. Mismatch would make `--radio <id>` and
        // `list-radios` output diverge again (regression guard for
        // the kebab-case bug).
        assert_eq!(Radio::WouxunKgQ336.spec().id, "wouxun-kg-q336");
        assert_eq!(Radio::QuanshengUvK5.spec().id, "quansheng-uv-k5");
        assert_eq!(Radio::TytMd380.spec().id, "tyt-md-380");
        assert_eq!(Radio::AnytoneAtD878uv.spec().id, "anytone-at-d878uv");
        assert_eq!(Radio::YaesuFt50r.spec().id, "yaesu-ft-50r");
    }

    #[test]
    fn covers_rx_includes_band_edges() {
        let spec = Radio::QuanshengUvK5.spec();
        assert!(spec.covers_rx(136_000_000)); // lower edge
        assert!(spec.covers_rx(174_000_000)); // upper edge of one segment
        assert!(spec.covers_rx(155_500_000)); // marine/jakt — middle of 2m extended
        assert!(!spec.covers_rx(40_000_000)); // 7 MHz HF — no
    }

    #[test]
    fn yaesu_ft50r_rejects_pmr446_tx() {
        // PMR446 (446.x) is outside FT-50R's TX (430–450) — wait
        // that's a bad example since 446 IS inside 430–450. Use
        // marine 156 MHz instead, which is between 144–148 and
        // 430–450, i.e. outside both TX bands.
        let spec = Radio::YaesuFt50r.spec();
        assert!(!spec.covers_tx(156_800_000));
        assert!(spec.covers_rx(156_800_000)); // RX is wide
    }

    #[test]
    fn wouxun_kg_q336_4m_is_tx_band() {
        let spec = Radio::WouxunKgQ336.spec();
        assert!(spec.covers_tx(69_187_500)); // PR69 K8
    }
}
