//! CHIRP-compatible CSV export.
//!
//! Produces the column layout that CHIRP's "File → Import" / generic CSV
//! driver accepts. Covers analog FM (with CTCSS / DCS) and D-STAR (DV).
//! DMR / C4FM / P25 / M17 channels are not representable and are skipped
//! with a warning — those targets need the manufacturer's own CPS.
//!
//! Reference: <https://chirpmyradio.com/projects/chirp/wiki/CSV_Generic>

use serde::Serialize;

use crate::channel::{Bandwidth, Channel, Mode, Power};

#[derive(thiserror::Error, Debug)]
pub enum ChirpError {
    #[error("CSV write failed: {0}")]
    Csv(#[from] csv::Error),
    #[error("UTF-8 conversion failed: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
}

/// CHIRP CSV plus any per-channel warnings raised during conversion.
#[derive(Debug, Clone, Default)]
pub struct ConvertReport {
    pub csv: String,
    pub warnings: Vec<String>,
}

/// Maximum channel-name length the generic CHIRP driver accepts.
/// Per-radio drivers sometimes truncate further; that's their problem.
const NAME_MAX: usize = 16;

/// One row of CHIRP's generic-CSV format. Field order here is the column
/// order in the emitted CSV; the `serde(rename)` attributes match
/// CHIRP's exact header spelling (irregular casing on `rToneFreq`,
/// `cToneFreq`, `URCALL`, `RPT1CALL`, `RPT2CALL`, `DVCODE`).
///
/// All-empty defaults reflect the values CHIRP fills in when a channel
/// doesn't use that feature; preserved verbatim to keep CSV output
/// byte-stable across this refactor.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
struct ChirpRow {
    location: u32,
    name: String,
    frequency: String,
    duplex: String,
    offset: String,
    tone: String,
    #[serde(rename = "rToneFreq")]
    r_tone_freq: String,
    #[serde(rename = "cToneFreq")]
    c_tone_freq: String,
    dtcs_code: String,
    dtcs_polarity: String,
    rx_dtcs_code: String,
    cross_mode: String,
    mode: String,
    #[serde(rename = "TStep")]
    t_step: String,
    skip: String,
    power: String,
    comment: String,
    #[serde(rename = "URCALL")]
    urcall: String,
    #[serde(rename = "RPT1CALL")]
    rpt1call: String,
    #[serde(rename = "RPT2CALL")]
    rpt2call: String,
    #[serde(rename = "DVCODE")]
    dvcode: String,
}

impl ChirpRow {
    /// Common defaults for every row (CHIRP fills these even when a
    /// channel ignores the feature). Mode-specific arms override.
    fn base(ch: &Channel, location: u32) -> Self {
        let (duplex, offset_mhz) = duplex_offset(ch.shift_hz);
        ChirpRow {
            location,
            name: truncate(&ch.name, NAME_MAX),
            frequency: format!("{:.6}", ch.rx_hz as f64 / 1_000_000.0),
            duplex: duplex.to_string(),
            offset: format!("{offset_mhz:.6}"),
            tone: String::new(),
            r_tone_freq: "88.5".to_string(),
            c_tone_freq: "88.5".to_string(),
            dtcs_code: "023".to_string(),
            dtcs_polarity: "NN".to_string(),
            rx_dtcs_code: String::new(),
            cross_mode: String::new(),
            mode: String::new(),
            t_step: "12.50".to_string(),
            skip: (if ch.scan { "" } else { "S" }).to_string(),
            power: power_str(ch.power).to_string(),
            comment: String::new(),
            urcall: String::new(),
            rpt1call: String::new(),
            rpt2call: String::new(),
            dvcode: String::new(),
        }
    }
}

pub fn channels_to_csv(channels: &[Channel]) -> Result<ConvertReport, ChirpError> {
    // `has_headers(true)` makes csv::Writer emit header names from the
    // first serialised struct's field renames. To keep header-on-empty
    // behaviour, we always serialise — see below.
    let mut wtr = csv::Writer::from_writer(Vec::new());

    let mut warnings = Vec::new();
    let mut location: u32 = 1;
    let mut wrote_any = false;
    for ch in channels {
        match channel_to_row(ch, location) {
            Ok(row) => {
                wtr.serialize(&row)?;
                if ch.name.chars().count() > NAME_MAX {
                    warnings.push(format!("{}: name truncated to {NAME_MAX} chars", ch.name));
                }
                location += 1;
                wrote_any = true;
            }
            Err(reason) => {
                warnings.push(format!("skipping {}: {reason}", ch.name));
            }
        }
    }

    // csv::Writer with has_headers(true) only writes the header on the
    // first serialise call. If every channel was skipped (or `channels`
    // was empty), force a header-only output by serialising a default
    // row, then dropping its bytes.
    if !wrote_any {
        wtr.serialize(ChirpRow::base(
            &Channel {
                name: String::new(),
                rx_hz: 0,
                shift_hz: 0,
                power: Power::Low,
                scan: true,
                mode: Mode::Fm {
                    bandwidth: Bandwidth::Wide,
                    tone_tx_hz: None,
                    tone_rx_hz: None,
                    dcs_code: None,
                },
                source: None,
            },
            0,
        ))?;
        let bytes = wtr.into_inner().expect("csv writer into_inner");
        // Keep only the header line.
        let header_only = match bytes.iter().position(|&b| b == b'\n') {
            Some(i) => bytes[..=i].to_vec(),
            None => bytes,
        };
        let csv = String::from_utf8(header_only)?;
        return Ok(ConvertReport { csv, warnings });
    }

    let csv_bytes = wtr.into_inner().expect("csv writer into_inner");
    let csv = String::from_utf8(csv_bytes)?;
    Ok(ConvertReport { csv, warnings })
}

fn channel_to_row(ch: &Channel, location: u32) -> Result<ChirpRow, &'static str> {
    let mut row = ChirpRow::base(ch, location);
    match &ch.mode {
        Mode::Fm {
            bandwidth,
            tone_tx_hz,
            tone_rx_hz,
            dcs_code,
        } => {
            row.mode = match bandwidth {
                Bandwidth::Wide => "FM",
                Bandwidth::Narrow => "NFM",
            }
            .to_string();
            let tone = ToneCells::for_fm(*tone_tx_hz, *tone_rx_hz, *dcs_code);
            row.tone = tone.tone.to_string();
            row.r_tone_freq = tone.rtone;
            row.c_tone_freq = tone.ctone;
            row.dtcs_code = tone.dtcs;
            row.dtcs_polarity = tone.polarity.to_string();
            Ok(row)
        }
        Mode::Dstar { urcall, rpt1, rpt2 } => {
            row.mode = "DV".to_string();
            row.urcall = urcall.clone();
            row.rpt1call = rpt1.clone();
            row.rpt2call = rpt2.clone();
            Ok(row)
        }
        Mode::Dmr { .. } => Err("DMR not supported by CHIRP CSV"),
        Mode::C4fm { .. } => Err("C4FM not supported by CHIRP CSV"),
        Mode::P25 { .. } => Err("P25 not supported by CHIRP CSV"),
        Mode::M17 { .. } => Err("M17 not supported by CHIRP CSV"),
    }
}

struct ToneCells {
    tone: &'static str,
    rtone: String,
    ctone: String,
    dtcs: String,
    polarity: &'static str,
}

impl ToneCells {
    fn empty() -> Self {
        ToneCells {
            tone: "",
            rtone: "88.5".into(),
            ctone: "88.5".into(),
            dtcs: "023".into(),
            polarity: "NN",
        }
    }

    fn for_fm(tx: Option<f32>, rx: Option<f32>, dcs: Option<u16>) -> Self {
        if let Some(d) = dcs {
            return ToneCells {
                tone: "DTCS",
                rtone: "88.5".into(),
                ctone: "88.5".into(),
                dtcs: format!("{d:03}"),
                polarity: "NN",
            };
        }
        match (tx, rx) {
            (None, None) => Self::empty(),
            (Some(t), None) => ToneCells {
                tone: "Tone",
                rtone: format!("{t:.1}"),
                ..Self::empty()
            },
            (Some(tx), Some(rx)) if (tx - rx).abs() < 0.05 => ToneCells {
                tone: "TSQL",
                rtone: format!("{tx:.1}"),
                ctone: format!("{tx:.1}"),
                ..Self::empty()
            },
            (Some(tx), Some(rx)) => ToneCells {
                tone: "Cross",
                rtone: format!("{tx:.1}"),
                ctone: format!("{rx:.1}"),
                ..Self::empty()
            },
            (None, Some(rx)) => ToneCells {
                tone: "Cross",
                ctone: format!("{rx:.1}"),
                ..Self::empty()
            },
        }
    }
}

fn duplex_offset(shift_hz: i64) -> (&'static str, f64) {
    match shift_hz.cmp(&0) {
        std::cmp::Ordering::Equal => ("", 0.0),
        std::cmp::Ordering::Less => ("-", (-shift_hz) as f64 / 1_000_000.0),
        std::cmp::Ordering::Greater => ("+", shift_hz as f64 / 1_000_000.0),
    }
}

fn power_str(p: Power) -> &'static str {
    match p {
        Power::Low => "1.0W",
        Power::Mid => "2.5W",
        Power::High => "5.0W",
    }
}

fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::{Admit, Bandwidth, C4fmRate, Channel, Mode, Power};

    fn fm(name: &str, rx_hz: u64) -> Channel {
        Channel {
            name: name.into(),
            rx_hz,
            shift_hz: 0,
            power: Power::Low,
            scan: true,
            mode: Mode::Fm {
                bandwidth: Bandwidth::Wide,
                tone_tx_hz: None,
                tone_rx_hz: None,
                dcs_code: None,
            },
            source: None,
        }
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
    fn empty_input_yields_header_only() {
        let report = channels_to_csv(&[]).unwrap();
        assert!(report.warnings.is_empty());
        let r = rows(&report.csv);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0][0], "Location");
        // 21 columns in CHIRP's generic CSV format.
        assert_eq!(r[0].len(), 21);
    }

    #[test]
    fn single_simplex_fm() {
        let report = channels_to_csv(&[fm("Simplex 2m", 145_500_000)]).unwrap();
        let r = rows(&report.csv);
        assert_eq!(r.len(), 2);
        let row = &r[1];
        assert_eq!(row[0], "1");
        assert_eq!(row[1], "Simplex 2m");
        assert_eq!(row[2], "145.500000");
        assert_eq!(row[3], ""); // simplex
        assert_eq!(row[4], "0.000000");
        assert_eq!(row[5], ""); // no tone
        assert_eq!(row[12], "FM");
        assert_eq!(row[14], ""); // scan = true → no skip
        assert_eq!(row[15], "1.0W");
    }

    #[test]
    fn two_meter_repeater_minus_600k() {
        let mut ch = fm("GB3WE", 145_725_000);
        ch.shift_hz = -600_000;
        let report = channels_to_csv(&[ch]).unwrap();
        let row = &rows(&report.csv)[1];
        assert_eq!(row[3], "-");
        assert_eq!(row[4], "0.600000");
    }

    #[test]
    fn ctcss_tx_only_yields_tone() {
        let mut ch = fm("Tone TX", 145_500_000);
        ch.mode = Mode::Fm {
            bandwidth: Bandwidth::Wide,
            tone_tx_hz: Some(88.5),
            tone_rx_hz: None,
            dcs_code: None,
        };
        let row = &rows(&channels_to_csv(&[ch]).unwrap().csv)[1];
        assert_eq!(row[5], "Tone");
        assert_eq!(row[6], "88.5");
        assert_eq!(row[7], "88.5"); // cToneFreq default
    }

    #[test]
    fn ctcss_tx_and_rx_same_yields_tsql() {
        let mut ch = fm("TSQL", 145_500_000);
        ch.mode = Mode::Fm {
            bandwidth: Bandwidth::Wide,
            tone_tx_hz: Some(123.0),
            tone_rx_hz: Some(123.0),
            dcs_code: None,
        };
        let row = &rows(&channels_to_csv(&[ch]).unwrap().csv)[1];
        assert_eq!(row[5], "TSQL");
        assert_eq!(row[6], "123.0");
        assert_eq!(row[7], "123.0");
    }

    #[test]
    fn dcs_code_yields_dtcs_mode() {
        let mut ch = fm("DTCS", 145_500_000);
        ch.mode = Mode::Fm {
            bandwidth: Bandwidth::Wide,
            tone_tx_hz: None,
            tone_rx_hz: None,
            dcs_code: Some(74),
        };
        let row = &rows(&channels_to_csv(&[ch]).unwrap().csv)[1];
        assert_eq!(row[5], "DTCS");
        assert_eq!(row[8], "074");
        assert_eq!(row[9], "NN");
    }

    #[test]
    fn narrow_bandwidth_yields_nfm() {
        let mut ch = fm("Narrow", 446_006_250);
        ch.mode = Mode::Fm {
            bandwidth: Bandwidth::Narrow,
            tone_tx_hz: None,
            tone_rx_hz: None,
            dcs_code: None,
        };
        let row = &rows(&channels_to_csv(&[ch]).unwrap().csv)[1];
        assert_eq!(row[12], "NFM");
    }

    #[test]
    fn dstar_emits_dv_mode_and_calls() {
        let ch = Channel {
            name: "GB7DC".into(),
            rx_hz: 439_575_000,
            shift_hz: -9_000_000,
            power: Power::Mid,
            scan: true,
            mode: Mode::Dstar {
                urcall: "CQCQCQ".into(),
                rpt1: "GB7DC  B".into(),
                rpt2: "GB7DC  G".into(),
            },
            source: None,
        };
        let row = &rows(&channels_to_csv(&[ch]).unwrap().csv)[1];
        assert_eq!(row[12], "DV");
        assert_eq!(row[17], "CQCQCQ");
        assert_eq!(row[18], "GB7DC  B");
        assert_eq!(row[19], "GB7DC  G");
    }

    #[test]
    fn dmr_channel_skipped_with_warning() {
        let ch = Channel {
            name: "DMR-SW".into(),
            rx_hz: 439_412_500,
            shift_hz: -9_400_000,
            power: Power::Mid,
            scan: true,
            mode: Mode::Dmr {
                color_code: 1,
                slot: 2,
                talkgroup: 23_505,
                admit: Admit::ColorCodeFree,
            },
            source: None,
        };
        let report = channels_to_csv(&[ch]).unwrap();
        assert_eq!(rows(&report.csv).len(), 1, "header only");
        assert_eq!(report.warnings.len(), 1);
        assert!(report.warnings[0].contains("DMR-SW"));
        assert!(report.warnings[0].contains("DMR"));
    }

    #[test]
    fn unsupported_modes_all_skip_with_warning() {
        let chans = vec![
            Channel {
                name: "C4".into(),
                rx_hz: 145_375_000,
                shift_hz: 0,
                power: Power::Mid,
                scan: true,
                mode: Mode::C4fm {
                    dg_id_tx: 0,
                    dg_id_rx: 0,
                    data_rate: C4fmRate::Dn,
                },
                source: None,
            },
            Channel {
                name: "P".into(),
                rx_hz: 145_500_000,
                shift_hz: 0,
                power: Power::Mid,
                scan: true,
                mode: Mode::P25 {
                    nac: 0x293,
                    talkgroup: 1,
                },
                source: None,
            },
            Channel {
                name: "M".into(),
                rx_hz: 433_475_000,
                shift_hz: 0,
                power: Power::Mid,
                scan: true,
                mode: Mode::M17 {
                    destination: "ALL".into(),
                    can: 0,
                },
                source: None,
            },
        ];
        let report = channels_to_csv(&chans).unwrap();
        assert_eq!(rows(&report.csv).len(), 1);
        assert_eq!(report.warnings.len(), 3);
    }

    #[test]
    fn channel_name_truncated_with_warning() {
        let ch = fm("ThisNameIsWayTooLongForCHIRP", 145_500_000);
        let report = channels_to_csv(&[ch]).unwrap();
        let row = &rows(&report.csv)[1];
        assert_eq!(row[1].chars().count(), NAME_MAX);
        assert_eq!(row[1], "ThisNameIsWayToo");
        assert_eq!(report.warnings.len(), 1);
        assert!(report.warnings[0].contains("truncated"));
    }

    #[test]
    fn scan_false_emits_skip_s() {
        let mut ch = fm("NoScan", 145_500_000);
        ch.scan = false;
        let row = &rows(&channels_to_csv(&[ch]).unwrap().csv)[1];
        assert_eq!(row[14], "S");
    }

    #[test]
    fn power_levels_map_to_watts() {
        for (p, expected) in [
            (Power::Low, "1.0W"),
            (Power::Mid, "2.5W"),
            (Power::High, "5.0W"),
        ] {
            let mut ch = fm("X", 145_500_000);
            ch.power = p;
            let row = &rows(&channels_to_csv(&[ch]).unwrap().csv)[1];
            assert_eq!(row[15], expected);
        }
    }
}
