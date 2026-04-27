# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when
working with code in this repository.

## Project

`narm` (Nina Arvid Radio Manager) is a CLI that manages channels and
setup for several handheld ham radios from one TOML source of truth.
The central config is "compiled" to each radio's native format,
filtering out unsupported channels and modes per target.

Supported radio targets:

- Wouxun KG-Q336
- Quansheng UV-K5
- TYT MD-380
- AnyTone AT-D878UV
- Yaesu FT-50R

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
