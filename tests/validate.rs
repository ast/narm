use std::fs;
use std::path::PathBuf;

use narm::{Mode, ModeKind, NarmError, Power};

fn sample_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("samples/sample.toml")
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
    assert_eq!(gb3we.source.as_deref(), Some(sample_path().as_path()));

    assert_eq!(cfg.channels[2].mode.kind(), ModeKind::Dmr);
    assert_eq!(cfg.channels[3].mode.kind(), ModeKind::Dstar);
    assert_eq!(cfg.channels[4].mode.kind(), ModeKind::C4fm);
}

#[test]
fn loads_directory_in_lex_order() {
    let dir = tempfile::tempdir().expect("tempdir");
    let a = dir.path().join("10-first.toml");
    let b = dir.path().join("20-second.toml");
    let hidden = dir.path().join(".hidden.toml");
    let other = dir.path().join("readme.txt");

    fs::write(
        &a,
        r#"
[[channels]]
name = "A1"
rx_hz = 145_500_000
mode = "fm"

[[channels]]
name = "A2"
rx_hz = 145_525_000
mode = "fm"
"#,
    )
    .unwrap();
    fs::write(
        &b,
        r#"
[[channels]]
name = "B1"
rx_hz = 433_500_000
mode = "fm"
"#,
    )
    .unwrap();
    fs::write(&hidden, "[[channels]]\nname=\"H\"\nrx_hz=0\nmode=\"fm\"\n").unwrap();
    fs::write(&other, "not toml").unwrap();

    let cfg = narm::load_from_path(dir.path()).expect("load dir");
    narm::validate(&cfg).expect("validate merged");

    let names: Vec<&str> = cfg.channels.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["A1", "A2", "B1"], "lex order across files");

    assert_eq!(cfg.channels[0].source.as_deref(), Some(a.as_path()));
    assert_eq!(cfg.channels[1].source.as_deref(), Some(a.as_path()));
    assert_eq!(cfg.channels[2].source.as_deref(), Some(b.as_path()));
}

#[test]
fn cross_file_duplicate_reports_both_paths() {
    let dir = tempfile::tempdir().expect("tempdir");
    let a = dir.path().join("a.toml");
    let b = dir.path().join("b.toml");
    let body = "[[channels]]\nname = \"DUP\"\nrx_hz = 145_500_000\nmode = \"fm\"\n";
    fs::write(&a, body).unwrap();
    fs::write(&b, body).unwrap();

    let cfg = narm::load_from_path(dir.path()).expect("load dir");
    let err = narm::validate(&cfg).expect_err("must detect duplicate");
    match err {
        NarmError::DuplicateName {
            name,
            first,
            second,
        } => {
            assert_eq!(name, "DUP");
            assert_eq!(first.as_deref(), Some(a.as_path()));
            assert_eq!(second.as_deref(), Some(b.as_path()));
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}
