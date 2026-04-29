//! `.kg` file format used by the Wouxun KG-Q332/Q336 CPS.
//!
//! The CPS stores its codeplug as a "text" file:
//!
//! ```text
//!   "xiepinruanjian\r\n"  ← 14-byte ASCII header
//!   <body>                 ← UTF-8-encoded Latin-1 of the raw image
//!   "\r\n"                 ← 2-byte ASCII footer
//! ```
//!
//! Each binary byte of the radio's memory is encoded:
//!
//! - `0x00..0x7F` → emitted as-is (one byte).
//! - `0x80..0xFF` → emitted as the 2-byte UTF-8 sequence for
//!   the same codepoint (i.e. `c2 XX` for `0x80..0xBF`,
//!   `c3 XX` for `0xC0..0xFF`).
//!
//! That's exactly what you get if a program does
//! `bytes.decode("latin-1").encode("utf-8")` — a common
//! mistake we exploit here.
//!
//! [`unmojibake`] reverses this transformation, returning the
//! recovered raw image which the wire-protocol path will
//! deliver verbatim in Phase 2.

use super::error::KgQ336Error;

/// ASCII header at the start of every `.kg` file. Pinyin for
/// "协频软件" — the CPS vendor's branding.
const HEADER: &[u8] = b"xiepinruanjian\r\n";

/// ASCII footer at the end of every `.kg` file.
const FOOTER: &[u8] = b"\r\n";

/// Decode a `.kg` file into the underlying raw image bytes.
///
/// Strips the header/footer and undoes the
/// UTF-8-of-Latin-1 mojibake encoding the CPS uses for the
/// body.
pub fn unmojibake(file_bytes: &[u8]) -> Result<Vec<u8>, KgQ336Error> {
    let body = file_bytes
        .strip_prefix(HEADER)
        .ok_or(KgQ336Error::MissingHeader)?;
    let body = body
        .strip_suffix(FOOTER)
        .ok_or(KgQ336Error::MissingFooter)?;

    let header_len = HEADER.len();
    let mut out = Vec::with_capacity(body.len());
    let mut i = 0;
    while i < body.len() {
        let b = body[i];
        match b {
            0x00..=0x7F => {
                out.push(b);
                i += 1;
            }
            0xC2 | 0xC3 => {
                let Some(&b1) = body.get(i + 1) else {
                    return Err(KgQ336Error::BadMojibake {
                        offset: header_len + i,
                        byte: b,
                    });
                };
                if !(0x80..=0xBF).contains(&b1) {
                    return Err(KgQ336Error::BadMojibake {
                        offset: header_len + i + 1,
                        byte: b1,
                    });
                }
                // Standard 2-byte UTF-8 decode.
                let cp = ((b as u16 & 0x1F) << 6) | (b1 as u16 & 0x3F);
                out.push(cp as u8);
                i += 2;
            }
            _ => {
                return Err(KgQ336Error::BadMojibake {
                    offset: header_len + i,
                    byte: b,
                });
            }
        }
    }
    Ok(out)
}

/// Re-encode a raw image as a `.kg` file. Inverse of
/// [`unmojibake`]. Useful for round-trip tests and (later)
/// writing modified codeplugs.
pub fn mojibake(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER.len() + raw.len() + FOOTER.len());
    out.extend_from_slice(HEADER);
    for &b in raw {
        if b < 0x80 {
            out.push(b);
        } else {
            // 2-byte UTF-8 encoding of codepoint `b`.
            out.push(0xC0 | (b >> 6));
            out.push(0x80 | (b & 0x3F));
        }
    }
    out.extend_from_slice(FOOTER);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_empty_body() {
        let kg = mojibake(&[]);
        assert_eq!(kg, b"xiepinruanjian\r\n\r\n");
        assert_eq!(unmojibake(&kg).unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn round_trip_pure_ascii() {
        let raw = b"hello world".to_vec();
        let kg = mojibake(&raw);
        assert_eq!(unmojibake(&kg).unwrap(), raw);
    }

    #[test]
    fn round_trip_all_byte_values() {
        let raw: Vec<u8> = (0..=255u8).collect();
        let kg = mojibake(&raw);
        assert_eq!(unmojibake(&kg).unwrap(), raw);
    }

    #[test]
    fn boundary_byte_values_decode_correctly() {
        // For each interesting boundary byte, hand-encode a
        // tiny .kg file and confirm we recover the original.
        for &raw_byte in &[0x00u8, 0x7F, 0x80, 0xA0, 0xBF, 0xC0, 0xFC, 0xFF] {
            let kg = mojibake(&[raw_byte]);
            let got = unmojibake(&kg).unwrap();
            assert_eq!(got, vec![raw_byte], "byte 0x{:02x}", raw_byte);
        }
    }

    #[test]
    fn handcrafted_mojibake_pairs_decode_to_expected_bytes() {
        // Verified against the actual sample file:
        //   c2 80 → 0x80
        //   c2 a0 → 0xA0
        //   c2 bf → 0xBF
        //   c3 80 → 0xC0
        //   c3 bc → 0xFC
        //   c3 bf → 0xFF
        let mut kg = Vec::from(b"xiepinruanjian\r\n" as &[u8]);
        kg.extend_from_slice(&[
            0xC2, 0x80, 0xC2, 0xA0, 0xC2, 0xBF, 0xC3, 0x80, 0xC3, 0xBC, 0xC3, 0xBF,
        ]);
        kg.extend_from_slice(b"\r\n");
        let raw = unmojibake(&kg).unwrap();
        assert_eq!(raw, [0x80, 0xA0, 0xBF, 0xC0, 0xFC, 0xFF]);
    }

    #[test]
    fn missing_header_rejected() {
        let kg = b"not the right header\r\nbody\r\n";
        assert!(matches!(unmojibake(kg), Err(KgQ336Error::MissingHeader)));
    }

    #[test]
    fn missing_footer_rejected() {
        let mut kg = Vec::from(HEADER);
        kg.extend_from_slice(b"body without footer");
        assert!(matches!(unmojibake(&kg), Err(KgQ336Error::MissingFooter)));
    }

    #[test]
    fn truncated_two_byte_sequence_rejected() {
        // Header + lone 0xC2 + footer → BadMojibake for the
        // missing continuation byte.
        let mut kg = Vec::from(HEADER);
        kg.push(0xC2);
        kg.extend_from_slice(FOOTER);
        // The 0xC2 is at body offset 0; absolute offset = 14.
        // Footer parsing strips before we walk, so body has 0xC2
        // alone and we look for index 1 which is missing.
        match unmojibake(&kg) {
            Err(KgQ336Error::BadMojibake { offset, byte }) => {
                assert_eq!(byte, 0xC2);
                assert_eq!(offset, HEADER.len()); // 14
            }
            other => panic!("expected BadMojibake, got {other:?}"),
        }
    }

    #[test]
    fn invalid_lead_byte_rejected() {
        // 0x80 is not a valid first byte of a UTF-8 sequence
        // and shouldn't appear in the body of a .kg.
        let mut kg = Vec::from(HEADER);
        kg.push(0x80);
        kg.extend_from_slice(FOOTER);
        match unmojibake(&kg) {
            Err(KgQ336Error::BadMojibake { offset, byte }) => {
                assert_eq!(byte, 0x80);
                assert_eq!(offset, HEADER.len());
            }
            other => panic!("expected BadMojibake, got {other:?}"),
        }
    }

    #[test]
    fn invalid_continuation_byte_rejected() {
        // 0xC2 followed by a non-continuation byte (e.g. 0x20).
        let mut kg = Vec::from(HEADER);
        kg.extend_from_slice(&[0xC2, 0x20]);
        kg.extend_from_slice(FOOTER);
        match unmojibake(&kg) {
            Err(KgQ336Error::BadMojibake { offset, byte }) => {
                assert_eq!(byte, 0x20);
                assert_eq!(offset, HEADER.len() + 1);
            }
            other => panic!("expected BadMojibake, got {other:?}"),
        }
    }
}
