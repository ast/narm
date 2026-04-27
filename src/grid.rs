//! Maidenhead grid locator <-> latitude/longitude.
//!
//! Pair index N (0-based) covers, on each axis, a step of:
//!   N=0 (field):       lng 20°,  lat 10°    chars A–R
//!   N=1 (square):      lng  2°,  lat  1°    chars 0–9
//!   N=2 (subsquare):   lng  5',  lat  2.5'  chars a–x
//!   N=3 (extended):    lng 30",  lat 15"    chars 0–9
//!   N=4 (extended):    lng 1.25",lat 0.625" chars a–x
//! Standard precisions are 4-, 6-, and 8-character locators.

use std::sync::LazyLock;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LatLng {
    pub lat: f64,
    pub lng: f64,
}

#[derive(thiserror::Error, Debug)]
pub enum GridError {
    #[error("invalid Maidenhead locator {0:?}: must be even length 2..=10")]
    InvalidLength(String),
    #[error("invalid Maidenhead locator {0:?}: bad character at position {1}")]
    InvalidChar(String, usize),
    #[error("coordinates out of range: lat={lat} (need −90..=90), lng={lng} (need −180..=180)")]
    OutOfRange { lat: f64, lng: f64 },
    #[error("encode length must be even and in 2..=10, got {0}")]
    BadEncodeLength(usize),
}

/// Width of one cell in (lng_deg, lat_deg) at pair index N.
static PAIR_STEP: LazyLock<[(f64, f64); 5]> = LazyLock::new(|| {
    [
        (20.0, 10.0),                    // field, A–R
        (2.0, 1.0),                      // square, 0–9
        (5.0 / 60.0, 2.5 / 60.0),        // subsquare, a–x
        (30.0 / 3600.0, 15.0 / 3600.0),  // extended sub, 0–9
        (1.25 / 3600.0, 0.625 / 3600.0), // extended sub, a–x
    ]
});

/// Decode a Maidenhead locator to the centre of its encoded cell.
pub fn decode(loc: &str) -> Result<LatLng, GridError> {
    let bytes = loc.as_bytes();
    let n = bytes.len();
    if !(2..=10).contains(&n) || !n.is_multiple_of(2) {
        return Err(GridError::InvalidLength(loc.to_string()));
    }

    let mut lng = -180.0_f64;
    let mut lat = -90.0_f64;
    for pair in 0..n / 2 {
        let (lng_step, lat_step) = PAIR_STEP[pair];
        let (lng_idx, lat_idx) = pair_to_indices(loc, pair, bytes[pair * 2], bytes[pair * 2 + 1])?;
        lng += lng_idx as f64 * lng_step;
        lat += lat_idx as f64 * lat_step;
    }
    // Move from lower-left corner to centre of the smallest cell decoded.
    let (last_lng_step, last_lat_step) = PAIR_STEP[n / 2 - 1];
    lng += last_lng_step / 2.0;
    lat += last_lat_step / 2.0;

    Ok(LatLng { lat, lng })
}

fn pair_to_indices(loc: &str, pair_idx: usize, a: u8, b: u8) -> Result<(u32, u32), GridError> {
    let pos = pair_idx * 2;
    match pair_idx {
        // Letter pairs (case-insensitive); a is lng, b is lat.
        // Field uses A–R (max 18); subsquare/extended-letter use a–x (max 24).
        0 | 2 | 4 => {
            let max = if pair_idx == 0 { 18 } else { 24 };
            let lng = letter_idx(a, max).ok_or_else(|| GridError::InvalidChar(loc.into(), pos))?;
            let lat =
                letter_idx(b, max).ok_or_else(|| GridError::InvalidChar(loc.into(), pos + 1))?;
            Ok((lng, lat))
        }
        // Digit pairs 0–9.
        1 | 3 => {
            let lng = digit_idx(a).ok_or_else(|| GridError::InvalidChar(loc.into(), pos))?;
            let lat = digit_idx(b).ok_or_else(|| GridError::InvalidChar(loc.into(), pos + 1))?;
            Ok((lng, lat))
        }
        _ => unreachable!("pair index already bounded by length check"),
    }
}

fn letter_idx(c: u8, max: u32) -> Option<u32> {
    let upper = c.to_ascii_uppercase();
    if !upper.is_ascii_alphabetic() {
        return None;
    }
    let idx = (upper - b'A') as u32;
    if idx < max { Some(idx) } else { None }
}

fn digit_idx(c: u8) -> Option<u32> {
    if c.is_ascii_digit() {
        Some((c - b'0') as u32)
    } else {
        None
    }
}

/// Encode coordinates as a Maidenhead locator of the requested length.
pub fn encode(coords: LatLng, length: usize) -> Result<String, GridError> {
    if !(2..=10).contains(&length) || !length.is_multiple_of(2) {
        return Err(GridError::BadEncodeLength(length));
    }
    if !(-90.0..=90.0).contains(&coords.lat) || !(-180.0..=180.0).contains(&coords.lng) {
        return Err(GridError::OutOfRange {
            lat: coords.lat,
            lng: coords.lng,
        });
    }

    let mut out = String::with_capacity(length);
    let mut lng_rem = coords.lng + 180.0;
    let mut lat_rem = coords.lat + 90.0;

    for pair in 0..length / 2 {
        let (lng_step, lat_step) = PAIR_STEP[pair];
        let lng_idx = (lng_rem / lng_step).floor() as u32;
        let lat_idx = (lat_rem / lat_step).floor() as u32;
        lng_rem -= lng_idx as f64 * lng_step;
        lat_rem -= lat_idx as f64 * lat_step;

        match pair {
            0 => {
                out.push(((b'A' + lng_idx.min(17) as u8) as char).to_ascii_uppercase());
                out.push(((b'A' + lat_idx.min(17) as u8) as char).to_ascii_uppercase());
            }
            1 | 3 => {
                out.push((b'0' + lng_idx.min(9) as u8) as char);
                out.push((b'0' + lat_idx.min(9) as u8) as char);
            }
            2 | 4 => {
                // Convention varies; ham logs and the bundled CSV use uppercase
                // subsquare letters, so emit uppercase to match.
                out.push((b'A' + lng_idx.min(23) as u8) as char);
                out.push((b'A' + lat_idx.min(23) as u8) as char);
            }
            _ => unreachable!(),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64, tol: f64) {
        assert!((a - b).abs() < tol, "{a} !≈ {b} (tol {tol})");
    }

    #[test]
    fn decode_jo67at_matches_csv_centre() {
        // SA6AR/R in repeaters.csv: lat=57.8125, lng=12.0417, locator=JO67AT.
        let p = decode("JO67AT").unwrap();
        approx(p.lat, 57.8125, 1e-4);
        approx(p.lng, 12.0417, 1e-4);
    }

    #[test]
    fn decode_kp04ls_matches_csv_centre() {
        // SK2AU/R Skellefteå: lat=64.774, lng=20.95, locator=KP04LS.
        let p = decode("KP04LS").unwrap();
        approx(p.lat, 64.7708, 1e-3);
        approx(p.lng, 20.9583, 1e-3);
    }

    #[test]
    fn decode_is_case_insensitive() {
        let upper = decode("JO67AT").unwrap();
        let lower = decode("jo67at").unwrap();
        approx(upper.lat, lower.lat, 1e-9);
        approx(upper.lng, lower.lng, 1e-9);
    }

    #[test]
    fn encode_jo67at_roundtrip() {
        let loc = encode(
            LatLng {
                lat: 57.8125,
                lng: 12.0417,
            },
            6,
        )
        .unwrap();
        assert_eq!(loc, "JO67AT");
    }

    #[test]
    fn encode_4char() {
        // The 4-char square JO67 covers lng 12..14, lat 57..58.
        let loc = encode(
            LatLng {
                lat: 57.5,
                lng: 13.0,
            },
            4,
        )
        .unwrap();
        assert_eq!(loc, "JO67");
    }

    #[test]
    fn round_trip_random_points() {
        for &(lat, lng) in &[(0.0, 0.0), (-45.5, 100.25), (89.0, -179.0), (-89.0, 179.0)] {
            let loc = encode(LatLng { lat, lng }, 8).unwrap();
            let back = decode(&loc).unwrap();
            // 8-char cell: lng 30", lat 15"; decode returns the centre, so
            // round-trip error is bounded by half-a-cell.
            approx(back.lat, lat, 15.0 / 3600.0 / 2.0 + 1e-9);
            approx(back.lng, lng, 30.0 / 3600.0 / 2.0 + 1e-9);
        }
    }

    #[test]
    fn rejects_bad_length() {
        assert!(matches!(decode("J"), Err(GridError::InvalidLength(_))));
        assert!(matches!(decode("JO6"), Err(GridError::InvalidLength(_))));
        assert!(matches!(
            decode("JO67ATXY1"),
            Err(GridError::InvalidLength(_))
        ));
    }

    #[test]
    fn rejects_bad_chars() {
        assert!(matches!(
            decode("ZZ00aa"),
            Err(GridError::InvalidChar(_, 0))
        ));
        assert!(matches!(decode("JOAA"), Err(GridError::InvalidChar(_, 2))));
    }

    #[test]
    fn rejects_out_of_range_coords() {
        assert!(matches!(
            encode(
                LatLng {
                    lat: 95.0,
                    lng: 0.0
                },
                6
            ),
            Err(GridError::OutOfRange { .. })
        ));
        assert!(matches!(
            encode(
                LatLng {
                    lat: 0.0,
                    lng: 200.0
                },
                6
            ),
            Err(GridError::OutOfRange { .. })
        ));
    }

    #[test]
    fn rejects_bad_encode_length() {
        assert!(matches!(
            encode(LatLng { lat: 0.0, lng: 0.0 }, 5),
            Err(GridError::BadEncodeLength(5))
        ));
        assert!(matches!(
            encode(LatLng { lat: 0.0, lng: 0.0 }, 12),
            Err(GridError::BadEncodeLength(12))
        ));
    }
}
