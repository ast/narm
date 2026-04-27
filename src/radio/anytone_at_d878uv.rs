use super::RadioSpec;
use crate::channel::ModeKind;

pub const SPEC: RadioSpec = RadioSpec {
    id: "anytone-at-d878uv",
    display_name: "AnyTone AT-D878UV",
    manual_path: "docs/AnyTone-AT-D878UV-User-Manual.pdf",
    tx_bands: &[(136_000_000, 174_000_000), (400_000_000, 480_000_000)],
    rx_bands: &[(136_000_000, 174_000_000), (400_000_000, 480_000_000)],
    supported_modes: &[ModeKind::Fm, ModeKind::Dmr],
};
