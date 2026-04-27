use super::RadioSpec;
use crate::channel::ModeKind;

pub const SPEC: RadioSpec = RadioSpec {
    id: "quansheng-uv-k5",
    display_name: "Quansheng UV-K5",
    manual_path: "docs/Quansheng-UV-K5-User-Manual.pdf",
    // "Normal" (non-FCC, non-CE) firmware: VHF 136–174, UHF 400–470.
    // The narrower FCC (144–148 + 420–450) and CE (144–146 + 430–440)
    // versions are subsets — the wider Normal range is what most users
    // running aftermarket firmware (k5prog, etc.) end up with.
    tx_bands: &[(136_000_000, 174_000_000), (400_000_000, 470_000_000)],
    // RX covers far more — broadcast FM, AM aviation, plus a "wideband
    // RX" segment up to 600 MHz.
    rx_bands: &[
        (50_000_000, 76_000_000),
        (76_000_000, 108_000_000),  // WFM broadcast
        (108_000_000, 135_997_500), // AM aviation
        (136_000_000, 173_997_500),
        (174_000_000, 349_997_500),
        (350_000_000, 399_997_500),
        (400_000_000, 469_997_500),
        (470_000_000, 599_997_500),
    ],
    supported_modes: &[ModeKind::Fm],
};
