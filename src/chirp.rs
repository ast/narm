//! CHIRP-compatible CSV export.
//!
//! Produces the column layout that CHIRP's "File → Import" / generic CSV
//! driver accepts. Covers analog FM (with CTCSS / DCS) and D-STAR (DV).
//! DMR / C4FM / P25 / M17 channels are not representable and are skipped
//! with a warning — those targets need the manufacturer's own CPS.
//!
//! Reference: <https://chirpmyradio.com/projects/chirp/wiki/CSV_Generic>

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

const HEADER: &[&str] = &[
    "Location",
    "Name",
    "Frequency",
    "Duplex",
    "Offset",
    "Tone",
    "rToneFreq",
    "cToneFreq",
    "DtcsCode",
    "DtcsPolarity",
    "RxDtcsCode",
    "CrossMode",
    "Mode",
    "TStep",
    "Skip",
    "Power",
    "Comment",
    "URCALL",
    "RPT1CALL",
    "RPT2CALL",
    "DVCODE",
];

pub fn channels_to_csv(channels: &[Channel]) -> Result<ConvertReport, ChirpError> {
    let mut wtr = csv::WriterBuilder::new()
        .has_headers(false)
        .from_writer(Vec::new());
    wtr.write_record(HEADER)?;

    let mut warnings = Vec::new();
    let mut location: u32 = 1;
    for ch in channels {
        match channel_to_row(ch, location) {
            Ok(row) => {
                wtr.write_record(&row)?;
                if ch.name.chars().count() > NAME_MAX {
                    warnings.push(format!("{}: name truncated to {NAME_MAX} chars", ch.name));
                }
                location += 1;
            }
            Err(reason) => {
                warnings.push(format!("skipping {}: {reason}", ch.name));
            }
        }
    }

    let csv_bytes = wtr.into_inner().expect("csv writer into_inner");
    let csv = String::from_utf8(csv_bytes)?;
    Ok(ConvertReport { csv, warnings })
}

fn channel_to_row(ch: &Channel, location: u32) -> Result<Vec<String>, &'static str> {
    let freq_mhz = ch.rx_hz as f64 / 1_000_000.0;
    let (duplex, offset_mhz) = duplex_offset(ch.shift_hz);
    let power = power_str(ch.power);
    let skip = if ch.scan { "" } else { "S" };
    let name = truncate(&ch.name, NAME_MAX);

    match &ch.mode {
        Mode::Fm {
            bandwidth,
            tone_tx_hz,
            tone_rx_hz,
            dcs_code,
        } => {
            let mode = match bandwidth {
                Bandwidth::Wide => "FM",
                Bandwidth::Narrow => "NFM",
            };
            let tone = ToneCells::for_fm(*tone_tx_hz, *tone_rx_hz, *dcs_code);
            Ok(vec![
                location.to_string(),
                name,
                format!("{freq_mhz:.6}"),
                duplex.to_string(),
                format!("{offset_mhz:.6}"),
                tone.tone.to_string(),
                tone.rtone,
                tone.ctone,
                tone.dtcs,
                tone.polarity.to_string(),
                String::new(),
                String::new(),
                mode.to_string(),
                "12.50".to_string(),
                skip.to_string(),
                power.to_string(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
            ])
        }
        Mode::Dstar { urcall, rpt1, rpt2 } => Ok(vec![
            location.to_string(),
            name,
            format!("{freq_mhz:.6}"),
            duplex.to_string(),
            format!("{offset_mhz:.6}"),
            String::new(),
            "88.5".to_string(),
            "88.5".to_string(),
            "023".to_string(),
            "NN".to_string(),
            String::new(),
            String::new(),
            "DV".to_string(),
            "12.50".to_string(),
            skip.to_string(),
            power.to_string(),
            String::new(),
            urcall.clone(),
            rpt1.clone(),
            rpt2.clone(),
            String::new(),
        ]),
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
        assert_eq!(r[0].len(), HEADER.len());
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
