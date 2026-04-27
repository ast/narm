use super::RadioSpec;
use crate::channel::ModeKind;

pub const SPEC: RadioSpec = RadioSpec {
    id: "yaesu-ft-50r",
    display_name: "Yaesu FT-50R",
    manual_path: "docs/YAESU--FT-50-User-Manual.pdf",
    // US version. Other regional variants have different TX edges
    // ("Frequency ranges and repeater shift vary according to
    // transceiver version" — manual p. 10). The wide RX coverage
    // is the same across versions modulo cellular blocking.
    tx_bands: &[(144_000_000, 148_000_000), (430_000_000, 450_000_000)],
    // 800 MHz cellular is blocked in US versions; that gap is small
    // enough that we model the rest as continuous.
    rx_bands: &[
        (76_000_000, 200_000_000),
        (300_000_000, 400_000_000),
        (400_000_000, 540_000_000),
        (590_000_000, 999_000_000),
    ],
    supported_modes: &[ModeKind::Fm],
};
