use super::RadioSpec;
use crate::channel::ModeKind;

pub const SPEC: RadioSpec = RadioSpec {
    id: "tyt-md-380",
    display_name: "TYT MD-380",
    manual_path: "docs/TYT-MD-380-Owners-Manual.pdf",
    // The MD-380 ships as either a VHF (136–174) or a UHF (400–480)
    // single-band radio. We list both ranges here since narm can't
    // know which variant the user owns; the user filters their
    // channel set accordingly.
    tx_bands: &[(136_000_000, 174_000_000), (400_000_000, 480_000_000)],
    rx_bands: &[(136_000_000, 174_000_000), (400_000_000, 480_000_000)],
    supported_modes: &[ModeKind::Fm, ModeKind::Dmr],
};
