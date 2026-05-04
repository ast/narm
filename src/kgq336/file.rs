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

/// Length of the post-unmojibake `.kg` image. Matches `.kg`
/// files the CPS produces, and is the canonical shape that
/// [`super::decode_channels`] and [`mojibake`] operate on.
pub const KG_SHAPE_LEN: usize = 50_000;

/// Length of a raw radio EEPROM dump (32 KiB) — the "physical"
/// layout you get from `narm read -b` over USB serial.
pub const PHYSICAL_LEN: usize = 0x8000;

/// Normalise any KG-Q336 codeplug input to the canonical
/// 50 KiB `.kg`-shape buffer that [`super::decode_channels`]
/// and [`mojibake`] operate on.
///
/// Auto-detects the input shape:
///
/// - `.kg` text file (`xiepinruanjian\r\n` header) →
///   [`unmojibake`].
/// - 32 KiB physical EEPROM dump → [`unscramble`] +
///   [`logical_to_kg_shape`].
/// - 50 KiB image → returned as-is (already in `.kg` shape).
///
/// Anything else is rejected with [`KgQ336Error::UnknownShape`].
pub fn to_kg_shape(bytes: Vec<u8>) -> Result<Vec<u8>, KgQ336Error> {
    if bytes.starts_with(HEADER) {
        return unmojibake(&bytes);
    }
    match bytes.len() {
        PHYSICAL_LEN => {
            let logical = unscramble(&bytes);
            Ok(logical_to_kg_shape(&logical))
        }
        KG_SHAPE_LEN => Ok(bytes),
        other => Err(KgQ336Error::UnknownShape { got: other }),
    }
}

/// Convert the radio's physical EEPROM layout (32 KiB,
/// what `narm radio read --format raw` produces) into the
/// "logical" layout that CHIRP's `kgq10h` driver works with.
///
/// Each 1 KiB physical block is stored as 4 × 256-byte slices
/// in *reverse* order; this function reverses that (swap
/// slice 0 ↔ slice 3, 1 ↔ 2 within each block). The output is
/// the same length as the input.
///
/// See `docs/kgq336-codeplug.md` → "Physical → logical
/// conversion".
pub fn unscramble(physical: &[u8]) -> Vec<u8> {
    const BLOCK: usize = 0x400; // 1 KiB
    const SLICE: usize = 0x100; // 256 B
    let mut logical = vec![0u8; physical.len()];
    let mut block_start = 0;
    while block_start + BLOCK <= physical.len() {
        for slice_idx in 0..4 {
            let phys_off = block_start + slice_idx * SLICE;
            let log_off = block_start + (3 - slice_idx) * SLICE;
            logical[log_off..log_off + SLICE]
                .copy_from_slice(&physical[phys_off..phys_off + SLICE]);
        }
        block_start += BLOCK;
    }
    // Trailing partial block (shouldn't happen for a proper 32
    // KiB image, but guard against weird sizes).
    if block_start < physical.len() {
        logical[block_start..].copy_from_slice(&physical[block_start..]);
    }
    logical
}

/// Build a synthesized `.kg`-shape buffer (50 000 bytes) from
/// a logical-layout image (post-[`unscramble`]) by copying the
/// known regions to their `.kg` offsets. Regions we haven't
/// mapped yet are left zeroed — the decoder will render those
/// as blank/default in inspect output.
///
/// This lets us reuse `decode_channels` (which is keyed off
/// `.kg` offsets) for live-read `.bin` files without duplicating
/// the whole decoder. As more `.kg ↔ logical` shifts are
/// confirmed, extend the `REGIONS` table below.
pub fn logical_to_kg_shape(logical: &[u8]) -> Vec<u8> {
    /// Length of the post-unmojibake `.kg` image. Matches
    /// `.kg` files the CPS produces.
    const KG_SIZE: usize = 50_000;

    // (kg_dst, logical_src, length) — confirmed shifts only.
    // See docs/kgq336-codeplug.md → "Confirmed `.kg` ↔ logical
    // region shifts" for how each was verified.
    const REGIONS: &[(usize, usize, usize)] = &[
        (0x0000, 0x0440, 0x0084),   // Settings struct (132 B)
        (0x0084, 0x04C4, 0x0014),   // Startup message (20 B; shift +0x0440 like settings)
        (0x0140, 0x05E0, 999 * 16), // Channel data array (999 × 16)
        (0x3FBC, 0x4460, 999 * 12), // Channel name array (999 × 12)
        (0x6E91, 0x7340, 999),      // Channel valid array
        (0x7278, 0x7740, 10 * 4),   // Scan group ranges (10 × 4)
        (0x72A0, 0x7768, 10 * 12),  // Scan group names (10 × 12)
        (0x73E0, 0x78B0, 20 * 2),   // FM broadcast presets (20 × u16)
        (0x766C, 0x7B4C, 12),       // Call group 1 name (slot 1, "Allanrop")
    ];

    let mut kg = vec![0u8; KG_SIZE];
    for &(dst, src, len) in REGIONS {
        if src + len <= logical.len() && dst + len <= kg.len() {
            kg[dst..dst + len].copy_from_slice(&logical[src..src + len]);
        }
    }
    kg
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
    fn to_kg_shape_passes_through_50k() {
        let img = vec![0xAB; KG_SHAPE_LEN];
        let out = to_kg_shape(img.clone()).unwrap();
        assert_eq!(out, img);
    }

    #[test]
    fn to_kg_shape_unmojibakes_kg_text() {
        let raw: Vec<u8> = (0..200u8).collect();
        let kg = mojibake(&raw);
        let out = to_kg_shape(kg).unwrap();
        assert_eq!(out, raw);
    }

    #[test]
    fn to_kg_shape_reshapes_32k_physical() {
        let physical = vec![0u8; PHYSICAL_LEN];
        let out = to_kg_shape(physical).unwrap();
        assert_eq!(out.len(), KG_SHAPE_LEN);
    }

    #[test]
    fn to_kg_shape_rejects_unknown_size() {
        let bytes = vec![0u8; 1234];
        assert!(matches!(
            to_kg_shape(bytes),
            Err(KgQ336Error::UnknownShape { got: 1234 })
        ));
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
