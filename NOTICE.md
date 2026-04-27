# NOTICE — third-party material in narm

## CHIRP — UV-K5 protocol

The Quansheng UV-K5 / UV-K5(8) support in narm
(`src/uvk5.rs` and the related subcommand handlers
in `src/commands/radio.rs`) is **derived** from CHIRP:

- Project: <https://github.com/kk7ds/chirp>
- Top-level licence: GPL-3.0
- Specific source consulted: `chirp/drivers/uvk5.py`
- That file's per-file header: **GPL-2.0-or-later**
  (`(c) 2023 Jacek Lipkowski SQ5BPF`, based on
  `template.py (c) 2012 Dan Smith`)

What was taken from CHIRP and re-implemented in Rust:

- The 8-byte hello packet (`14 05 04 00 6a 39 57 64`).
- The frame format (`AB CD len 00 [xor(payload+crc16)]
  DC BA`) and the inbound footer carrying an unchecked
  XOR'd CRC.
- The 16-byte cyclic XOR obfuscation key.
- Read-block opcode `1B 05 …`, write-block opcode
  `1D 05 …`, and the address-echo response check on
  writes.
- The reset packet (`DD 05 00 00`).
- The 16-byte channel-record layout at EEPROM `0x0000`
  (200 records) and the 16-byte name slot at `0xF50`,
  including the bit-packed `flags1` / `flags2` /
  `codeflags` semantics.
- The `WRITABLE_SIZE = 0x1d00` boundary that protects
  the factory calibration block at `0x1d00..0x2000`.
- The CTCSS and DCS code tables (which CHIRP itself
  inherits from `chirp_common`; these are also
  industry-standard tables, not novel to CHIRP).

The Rust implementation is freshly written — no
mechanical translation of Python — but the design is
unambiguously informed by the CHIRP driver, so the
ethical and legal posture for this module is to
treat it as a GPL-derived work.

## Sources of frequency data

- `docs/freq_lists/` channel TOMLs are extracted from
  *Radiohandbok HF/VHF/UHF* by Täpp-Anders Sikvall
  (SM5UEI), 2024-07-17. Source repo:
  <https://github.com/sikvall/rhb>; site:
  <https://sikvall.se/>.
- `docs/freq_lists/repeaters_2026_04_27.csv` is the
  Swedish Sändareamatörer (SSA) repeater list:
  <https://www.ssa.se/vushf/repeatrar-fyrar/>.

## Per-radio reference manuals

The PDFs under `docs/` (Wouxun KG-Q336, Quansheng
UV-K5, TYT MD-380, AnyTone AT-D878UV, Yaesu FT-50R)
are vendor or community-published user manuals
included for reference only; they are © their
respective copyright holders and are not redistributed
under any narm licence.

## License decision pending

narm itself does not yet have a `LICENSE` file or a
`license =` field in `Cargo.toml`. Before publishing
or sharing the binary the maintainer should pick one;
the obvious choices given the CHIRP-derived UV-K5
module are:

- **GPL-3.0**: matches CHIRP's top-level licence;
  the cleanest path. Pretty much eliminates the
  derivative-work question.
- **GPL-2.0-or-later**: matches `uvk5.py`'s per-file
  header; later upgradeable to GPL-3.0.
- **Permissive (MIT/Apache-2.0)** for the non-UV-K5
  parts, with `src/uvk5.rs` and `src/commands/
  radio.rs` segregated under GPL: technically possible
  but creates a dual-licence headache; not recommended
  unless you genuinely need permissive distribution
  for the channel-config / repeater-DB parts.

This file is a record of the derivation, not a
licence grant in either direction.
