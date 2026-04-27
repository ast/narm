# Frequency lists

Channel data extracted from `docs/rhb-full-v2.2.1.pdf` (Swedish
HF/VHF/UHF radio handbook by Täpp-Anders Sikvall, **SM5UEI**,
2024-07-17), grouped by usage category. The handbook and its
contents are © SM5UEI — see his website at
[https://sikvall.se/](https://sikvall.se/) (source repo:
[github.com/sikvall/rhb](https://github.com/sikvall/rhb)). The SSA
repeater list is at `repeaters_2026_04_27.csv` and gets imported
into SQLite via `narm repeaters import` — those repeaters are not
duplicated here.

All files use the narm channel TOML schema and are loadable by
`narm validate <file-or-dir>`. Every channel has `scan = true` set
since the intent is scanning. `mode = "fm"` is used as a fallback
for channels where the real mode is CW/SSB/AM/DPMR/DSC — flip by
hand if you want the right demod.

## Layout

```
freq_lists/
├── repeaters_2026_04_27.csv      SSA repeater list (CSV)
├── non_amateur/
│   ├── hunting_31mhz.toml        40 ch, "Jakt" hunting band
│   ├── pr_69mhz.toml              8 ch, PR private radio
│   ├── hunting_agri_155mhz.toml   7 ch, hunting/agri/forestry
│   ├── pmr446.toml               16 ch, PMR446 (8 analog + 8 DPMR)
│   └── srbr_25khz.toml            8 ch, SRBR business radio
├── marine/
│   ├── marine_vhf.toml           ~95 ch, VHF marine (ship + shore split)
│   ├── ais.toml                   2 ch, AIS transponder
│   ├── marine_mf.toml            10 ch, MF coast stations (5 ship + 5 shore)
│   ├── marine_distress.toml      12 ch, MF/HF distress + DSC
│   └── marine_hf_ship_to_ship.toml ~38 ch, primary HF S-to-S grid
├── amateur_vhf_uhf/
│   ├── 6m_fm.toml                50 ch, F50–F80 simplex + RF81–RF99 rep
│   ├── 2m_fm.toml                47 ch, V17–V47 + RV48–RV63 (-600 kHz)
│   └── 70cm_fm.toml              82 ch, U272–U320 + RU368–RU400 (-2 MHz)
├── scout/
│   ├── nordic_vhf.toml            3 ch, JOTA Nordic VHF
│   └── jota_hf.toml              18 ch, JOTA HF (CW + SSB)
├── beacons/
│   ├── swedish_vhf_uhf.toml      40 ch, Swedish VHF/UHF beacons (CW)
│   └── ibp_hf.toml                5 ch, IBP global beacon project
└── hf/
    └── pr_27mhz.toml             44 ch, CB / PR 27 MHz (incl. RC)
```

## Conventions

- **Channel names** carry the source list prefix and channel number
  so they're unique across the whole tree (e.g. `2m V17`,
  `PMR446 K1`, `MarVHF 16 Anrop/Nöd`).
- **Repeater outputs** include `shift_hz` (negative) so you can TX
  into the input. Simplex channels omit the field.
- **Marine duplex pairs** are split into two channels (`Ship` /
  `Shore`) for scanning convenience.
- **Mode notes**: where the source mode is non-FM (CW for beacons,
  SSB for marine HF/JOTA HF, GMSK for AIS, etc.), it's recorded as
  `fm` and the file's header comment calls out the real mode.

## What's not extracted

- **Per-district repeater lists** (rhb-full §5.6.2–5.6.9) — already
  in `repeaters_2026_04_27.csv` and the SQLite import; would only
  duplicate.
- **Band plans** (§5.7 VHF/UHF, §8.x HF) — these are frequency
  *ranges* with mode hints, not discrete channels, so they don't fit
  the `[[channels]]` schema.
- **Stockholm Radio coast-station table** (§5.2.3) — a station→
  channel→horizon mapping rather than channel data; the channels
  themselves are already in `marine/marine_vhf.toml`.

## Usage with narm

Validate a single category or all of them:

```sh
narm v docs/freq_lists/non_amateur/        # one category
narm v docs/freq_lists/                    # everything (recursive
                                           # not yet supported — only
                                           # files directly under the
                                           # given dir; one
                                           # subdir per call for now)
```

Today narm's directory loader is non-recursive, so `validate` against
the top-level `freq_lists/` will only see files in that directory.
Run it per subdirectory, or merge into a single dir if you want
everything in one shot.
