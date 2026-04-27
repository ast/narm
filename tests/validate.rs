use std::path::PathBuf;

use narm::{Mode, Power};

fn sample_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/sample.toml")
}

#[test]
fn loads_and_validates_sample() {
    let cfg = narm::load_from_path(&sample_path()).expect("load sample");
    narm::validate(&cfg).expect("validate sample");
    assert_eq!(cfg.channels.len(), 5);

    let gb3we = &cfg.channels[0];
    assert_eq!(gb3we.name, "GB3WE");
    assert_eq!(gb3we.rx_hz, 145_725_000);
    assert_eq!(gb3we.power, Power::Low);
    assert!(matches!(gb3we.mode, Mode::Fm { .. }));

    assert_eq!(cfg.channels[2].mode.kind(), "dmr");
    assert_eq!(cfg.channels[3].mode.kind(), "dstar");
    assert_eq!(cfg.channels[4].mode.kind(), "c4fm");
}
