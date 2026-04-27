# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when
working with code in this repository.

## Project

`narm` (Nina Arvid Radio Manager) is a CLI that manages channels and
setup for several handheld ham radios from one TOML source of truth.
The central config is "compiled" to each radio's native format,
filtering out unsupported channels and modes per target.

See **## Radios** below for the supported targets and their
per-unit capabilities (TX bands, modes, power, tones,
codeplug limits) — this is what `compile` filters against.

## Crates

`clap` (derive), `clap_complete`, `serde` (derive), `toml`,
`thiserror`, `anyhow`, `dotenvy`.

## Project layout

- Subcommand-based CLI.
- `commands` module with each subcommand in its own file.
- Do **not** use the `mod.rs` pattern — use `commands.rs` plus a
  `commands/` directory.
- `lib.rs` holds everything that is not a command, so integration
  tests can exercise core functionality.

## Style

- Rust: `cargo fmt` and `cargo clippy` (treat warnings as errors).
- Text files: wrap around 70 characters.

## Radios

Tables extracted from the manuals in `docs/`. Use `unknown`
for any row the manual does not document — do not guess.
Same row order across radios so they diff cleanly. DMR-only
rows are appended below the common rows for the two DMR
radios.

To search across the PDFs in `docs/`, use `rga`
(ripgrep-all) — plain `rg` does not look inside PDF content.
Example: `rga -i "color code" docs/`.

### Wouxun KG-Q336

| Capability | Value |
|---|---|
| Manual | `docs/Wouxun-KG-Q336-Bruksanvisning-SE.pdf` (Swedish, 4-page leaflet) |
| TX bands (MHz) | 66–88, 136–174, 400–480 |
| RX bands (MHz) | 76–108 (FM bcast), 66–88, 108–136 (AM), 136–174, 216–260, 320–400, 400–480, 714–999 |
| Modes | FM (TX/RX); AM (RX only); FM broadcast (RX only) |
| Power levels | 1 / 2 / 5 W |
| Channel bandwidth | unknown |
| CTCSS | TX + RX (e.g. 67.0, 71.9, 77.0, 82.5 Hz documented) |
| DCS | unknown |
| Channel memory | 999 |
| Notes | dual receive (Area A/B); cross-band repeat (must be VHF↔UHF); IP55; USB-C; no digital modes |

### Quansheng UV-K5

| Capability | Value |
|---|---|
| Manual | `docs/Quansheng-UV-K5-User-Manual.pdf` (specs p. 17–18) |
| TX bands (MHz) | Normal: 136–174 + 400–470; FCC: 144–148 + 420–450; CE: 144–146 + 430–440 |
| RX bands (MHz) | 50–76, 76–108 (WFM bcast), 108–136 (AM aviation), 136–174, 174–350, 350–400, 400–470, 470–600 |
| Modes | FM (TX/RX, narrow/wide); AM (RX only); WFM (RX only) |
| Power levels | H / M / L (≤5 W) |
| Channel bandwidth | 12.5 kHz (narrow) / 25 kHz (wide) |
| CTCSS | TX + RX (50 codes) |
| DCS | TX + RX (208 codes incl. reverse) |
| Channel memory | 200 (+ 20 FM bcast presets, + 10 NOAA) |
| Notes | dual-watch; cross-band intercom; aviation band RX; NOAA wx; FM broadcast RX; voice scrambler (10 group); spectrum/frequency meter; emergency alarm; no digital voice modes |

### TYT MD-380

| Capability | Value |
|---|---|
| Manual | `docs/TYT-MD-380-Owners-Manual.pdf` (specs p. 79–81 of printed numbering) |
| TX bands (MHz) | single-band per variant: VHF 136–174 **or** UHF 400–480 |
| RX bands (MHz) | same as TX (single-band) |
| Modes | FM analog (TX/RX, 11K0F3E); DMR digital (TX/RX, 4FSK 7K60FXD/FXE) |
| Power levels | 1 W (Low) / 5 W (High) |
| Channel bandwidth | 12.5 kHz (12.5/25 kHz selectable on analog) |
| CTCSS | TX + RX (analog channels only; non-standard codes supported) |
| DCS | TX + RX (analog channels only) |
| Channel memory | 1000 |
| Notes | VOX; emergency call/alarm; SMS; basic+DTMF encryption; talkaround; "Repeater Slot" and "Color Code" only valid on digital channels |
| DMR tier | I/II (ETSI TS 102 361-1/-2/-3) |
| DMR slots | 1, 2 (per channel) |
| Color codes | 0–15 (DMR standard; range not enumerated in user manual) |
| Max contacts | unknown (not in user manual; defined in CPS) |
| Max RX groups | unknown (see CPS) |
| Max zones | unknown (see CPS) |
| Channels per zone | unknown (see CPS) |

### AnyTone AT-D878UV

| Capability | Value |
|---|---|
| Manual | `docs/AnyTone-AT-D878UV-User-Manual.pdf` (specs p. 44) |
| TX bands (MHz) | 136–174 (V) + 400–480 (U) |
| RX bands (MHz) | same as TX |
| Modes | Analog FM (TX/RX); DMR digital (TX/RX); A+D / D+A mixed RX with single-mode TX |
| Power levels | VHF: 7 / 5 / 2.5 / 1 W; UHF: 6 / 5 / 2.5 / 1 W |
| Channel bandwidth | analog: 25 kHz wide / 12.5 kHz narrow; digital: 12.5 kHz only |
| CTCSS | TX + RX (51 codes, 62.5–254.1 Hz) |
| DCS | TX + RX (1024 codes, 000N–7771) |
| Channel memory | 4000 |
| Notes | GPS + APRS (analog & digital, 8 report channels); SMS (M-SMS / H-SMS); 10–500 h voice recording; 32 digital encryption groups; roaming; multiple DMR IDs; man-down alarm; tuning steps 2.5/5/6.25/10/12.5/20/25/30/50 kHz |
| DMR tier | I/II |
| DMR slots | 1, 2 (double-slot supported in simplex) |
| Color codes | 0–15 (DMR standard) |
| Max contacts | unknown (not in user manual; defined in CPS) |
| Max RX groups | unknown (see CPS) |
| Max zones | 250 |
| Channels per zone | 160 (analog and/or digital) |

### Yaesu FT-50R

| Capability | Value |
|---|---|
| Manual | `docs/YAESU--FT-50-User-Manual.pdf` (specs p. 10) |
| TX bands (MHz) | 144–148, 430–450 (US version; "frequency ranges and repeater shift vary according to transceiver version") |
| RX bands (MHz) | 76–200, 300–400, 400–540, 590–999 (cellular blocked on 800 MHz) |
| Modes | FM (TX/RX, F2/F3); FM broadcast 76–108 MHz (RX only) |
| Power levels | 5.0 / 2.8 / 1 / 0.1 W |
| Channel bandwidth | tuning steps 5 / 10 / 12.5 / 15 / 20 / 25 / 50 kHz; max deviation ±5 kHz (no explicit narrow/wide setting) |
| CTCSS | TX (39 tones, encoder); RX requires optional FTT-12 keypad |
| DCS | TX + RX (104 codes) |
| Channel memory | 100 freely tunable + dedicated HOME per band |
| Notes | dual VFO; dual-watch; ARTS (Auto Range Transpond); DTMF paging; 4-char memory names; no digital voice modes |

## Channel schema

Channels are TOML tables tagged by `mode`. Mode-specific fields live
on the same table as the common ones (`#[serde(flatten)]` on the
Rust side):

```toml
[[channels]]
name = "GB3WE"
rx_hz = 145_725_000
shift_hz = -600_000
power = "low"
scan = true
mode = "fm"
bandwidth = "wide"
tone_tx_hz = 88.5
tone_rx_hz = 88.5

[[channels]]
name = "Simplex 2m"
rx_hz = 145_500_000
mode = "fm"
bandwidth = "wide"

[[channels]]
name = "DMR-SW"
rx_hz = 439_412_500
shift_hz = -9_400_000
mode = "dmr"
color_code = 1
slot = 2
talkgroup = 23_505
admit = "color_code_free"

[[channels]]
name = "GB7DC"
rx_hz = 439_575_000
shift_hz = -9_000_000
mode = "dstar"
urcall = "CQCQCQ"
rpt1 = "GB7DC  B"
rpt2 = "GB7DC  G"

[[channels]]
name = "Wires-X"
rx_hz = 145_375_000
mode = "c4fm"
dg_id_tx = 0
dg_id_rx = 0
data_rate = "dn"
```

## Type sketch

```rust
#[derive(Deserialize)]
struct Channel {
    name: String,
    rx_hz: u64,
    #[serde(default)] shift_hz: i64,
    #[serde(default)] power: Power,
    #[serde(default)] scan: bool,
    #[serde(flatten)]  mode: Mode,
}

#[derive(Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
enum Mode {
    Fm {
        #[serde(default)] bandwidth: Bandwidth,
        #[serde(default)] tone_tx_hz: Option<f32>,
        #[serde(default)] tone_rx_hz: Option<f32>,
        #[serde(default)] dcs_code:   Option<u16>,
    },
    Dmr   { color_code: u8, slot: u8, talkgroup: u32,
            #[serde(default)] admit: Admit },
    Dstar { urcall: String, rpt1: String, rpt2: String },
    C4fm  { dg_id_tx: u8, dg_id_rx: u8, data_rate: C4fmRate },
    P25   { nac: u16, talkgroup: u32 },
    M17   { destination: String, can: u8 },
}
```
