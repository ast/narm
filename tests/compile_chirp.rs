use std::path::PathBuf;

use narm::chirp;

fn sample_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("samples/sample.toml")
}

fn rows(csv: &str) -> Vec<Vec<String>> {
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(false)
        .from_reader(csv.as_bytes());
    rdr.records()
        .map(|r| r.unwrap().iter().map(|s| s.to_string()).collect())
        .collect()
}

#[test]
fn compiles_full_sample_for_chirp_csv() {
    let cfg = narm::load_from_path(&sample_path()).expect("load sample");
    let report = chirp::channels_to_csv(&cfg.channels).expect("convert");

    let r = rows(&report.csv);
    // Header + 3 supported (FM, FM, D-STAR); the DMR + C4FM rows should
    // be skipped with warnings. sample.toml has 5 channels: 2 FM, 1 DMR,
    // 1 D-STAR, 1 C4FM.
    assert_eq!(r.len(), 1 + 3, "header + 3 supported rows");
    assert_eq!(report.warnings.len(), 2, "DMR + C4FM warnings");

    // GB3WE: FM 145.725 MHz, -600 kHz, TSQL 88.5 Hz, low power.
    let gb3we = r
        .iter()
        .find(|row| row[1] == "GB3WE")
        .expect("GB3WE row present");
    assert_eq!(gb3we[2], "145.725000");
    assert_eq!(gb3we[3], "-");
    assert_eq!(gb3we[4], "0.600000");
    assert_eq!(gb3we[5], "TSQL");
    assert_eq!(gb3we[6], "88.5");
    assert_eq!(gb3we[7], "88.5");
    assert_eq!(gb3we[12], "FM");
    assert_eq!(gb3we[15], "1.0W");
    assert_eq!(gb3we[14], ""); // scan = true

    // GB7DC: D-STAR.
    let gb7dc = r
        .iter()
        .find(|row| row[1] == "GB7DC")
        .expect("GB7DC row present");
    assert_eq!(gb7dc[12], "DV");
    assert_eq!(gb7dc[17], "CQCQCQ");
    assert_eq!(gb7dc[18], "GB7DC  B");

    // Warnings mention the skipped channels.
    assert!(report.warnings.iter().any(|w| w.contains("DMR-SW")));
    assert!(report.warnings.iter().any(|w| w.contains("Wires-X")));
}
