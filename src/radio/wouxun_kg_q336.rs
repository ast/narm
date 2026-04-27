use super::RadioSpec;
use crate::channel::ModeKind;

pub const SPEC: RadioSpec = RadioSpec {
    id: "wouxun-kg-q336",
    display_name: "Wouxun KG-Q336",
    manual_path: "docs/Wouxun-KG-Q336-Bruksanvisning-SE.pdf",
    // 4 m, 2 m, 70 cm — TX/RX. Per docs/Wouxun-KG-Q336.
    tx_bands: &[
        (66_000_000, 88_000_000),
        (136_000_000, 174_000_000),
        (400_000_000, 480_000_000),
    ],
    // RX is wider than TX: adds FM broadcast, AM aviation, and a
    // couple of UHF/microwave receive-only segments.
    rx_bands: &[
        (66_000_000, 88_000_000),   // 4 m TX/RX
        (76_000_000, 108_000_000),  // FM broadcast
        (108_000_000, 136_000_000), // AM aviation
        (136_000_000, 174_000_000), // 2 m + landmobile TX/RX
        (216_000_000, 260_000_000),
        (320_000_000, 400_000_000),
        (400_000_000, 480_000_000), // 70 cm + UHF landmobile TX/RX
        (714_000_000, 999_000_000),
    ],
    supported_modes: &[ModeKind::Fm],
};
