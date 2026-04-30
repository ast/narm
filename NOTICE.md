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

## CHIRP — Wouxun KG-Q332 / KG-Q336 protocol

The Wouxun KG-Q332 / KG-Q336 support in narm
(`src/kgq336/wire.rs`, `src/kgq336/io.rs`, parts of
`src/kgq336/decode.rs`, and the related subcommand
handlers in `src/commands/radio.rs` and
`src/commands/kgq336.rs`) is **derived** from CHIRP:

- Project: <https://github.com/kk7ds/chirp>
- Top-level licence: GPL-3.0
- Sources consulted (in order of relevance):
  - `chirp/drivers/kg935g.py` — closest published
    predecessor; same framing family. Per-file header:
    GPL-3.0 (Krystian Struzik / CHIRP contributors).
  - `chirp/drivers/kguv9dplus.py` — sibling family;
    cross-referenced for the rolling-XOR cipher
    primitive shape. Per-file header: GPL-3.0.
  - `kgq10h ... 2025mar16.py` by Mel Terechenok,
    attached to CHIRP issue
    <https://chirpmyradio.com/issues/10880>
    ("Wouxun KG-Q336/332" channels-only test driver).
    This is the only published reference for the Q33x
    family. The patch is posted under CHIRP's GPL.
- Issue tracker threads followed:
  - #10692 (KG-Q10H, the precursor radio)
  - #10880 (Q332 / Q336 channels-only test driver)
  - #11547 (Q332 status), #11765 (Q336 status)

What was taken from CHIRP and re-implemented in Rust:

- **Frame format** (`7C [cmd] [dir] [len] enc(payload ||
  cksum)`), the four command bytes (`CMD_ID = 0x80`,
  `CMD_END = 0x81`, `CMD_RD = 0x82`, `CMD_WR = 0x83`),
  and the direction byte convention
  (`0xFF` host→radio, `0x00` radio→host).
- **Rolling-XOR cipher** (`enc[0] = seed ^ plain[0]`,
  `enc[i] = enc[i-1] ^ plain[i]`) with the Q33x-specific
  seed byte `0x54`. The algorithm is shared with kg935g
  (seed `0x57`) and kguv9dplus (seed `0x52`).
- **Checksum** — sum-mod-256 of `cmd || dir || len ||
  payload` (the kg935g formula) with the Q33x-specific
  per-frame +3/+1/-1/-3 adjustment keyed by
  `payload[0] & 0x03` (kgq10h's `_checksum2` /
  `_checksum_adjust`).
- **Handshake** — the 3-burst wake-up: send the canonical
  `CMD_RD` of physical address `0x0040` three times
  back-to-back, then read one reply. Per Mel Terechenok's
  `_identify`: *"Wouxun CPS sends the same Read command 3
  times to establish comms."*
- **Address transform** — `convert_address`, undoing the
  radio's physical 4×256-byte slice-reversal within each
  1 KiB block to produce a flat logical image.
- **Line settings** — 115200 8N1, DTR=1, RTS=0, max
  addressable `0x8000`, no read of physical addresses
  below `0x0040`.
- **Logical memory map** — the `_MEM_FORMAT_Q332_oem_read_nolims`
  struct (OEM info offsets, settings struct field order,
  channel-data / channel-name array bases, scan-group
  layout, FM presets, call IDs / call names). narm's Rust
  translation drops three phantom fields the Q336 firmware
  doesn't actually have (`wxalert`, `wxalert_type`,
  `batt_ind` — confirmed by byte-diff captures against the
  vendor CPS); these were inherited in kgq10h from the
  earlier KG-Q10H struct.
- The CMD_END constant `7C 81 FF 00 D7`.

Independent reverse-engineering done locally
(*not* derived from CHIRP):

- The `.kg` file wrapper format and its mojibake
  encoding (`xiepinruanjian\r\n` header + UTF-8-of-Latin-1
  body) — CHIRP doesn't read CPS files, so this isn't in
  any driver. Implemented as `unmojibake` / `mojibake`
  in `src/kgq336/file.rs`.
- The `.kg` ↔ logical mapping for the settings region
  (clean +`0x0440` shift, with the three phantom fields
  identified). The full `.kg` ↔ logical transform for
  other regions is still TBD.
- Per-field empirical encodings for the Q336 settings
  struct (Roger / Language / ALERT / Sub-Frequency Mute /
  SC-QT / VOX / DTMF transmit time / TOT pre-alert /
  Voice Guide / RPT-SPK / SCAN-DET, etc.), derived from
  single-field byte-diff captures against vendor CPS
  saves. Recorded in `docs/kgq336-codeplug.md`.

The Rust implementation is freshly written — no
mechanical translation of Python — but the design is
unambiguously informed by the CHIRP drivers, so the
ethical and legal posture for this module is to treat
it as a GPL-derived work.

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

## Licence

narm is distributed under the **GNU General Public License
version 3** — see the `LICENSE` file at the repo root for the
full text. This matches CHIRP's top-level licence.
