# Wouxun KG-Q332 / KG-Q336 codeplug format

Reference for the `.kg` codeplug file produced by Wouxun's CPS,
based on byte-diffing single-field saves and inspecting the
seven CPS UI tabs (the "screenshots from the official
programming software" that prompted this document — kept here
so we don't have to re-paste them every time we extend the
decoder).

The recovered raw image (after `unmojibake`) is **50,000 bytes**.
The same shape will come off the radio over serial in Phase 2,
so the decode logic is shared between the file and live paths.

## Big revision from earlier RE work

Earlier iterations of `src/kgq336/decode.rs` (≤ v0.6) treated
channels as belonging to independent "categories" (Riks, Jakt,
SRBR, PMR, 69M) each with its own data and name base offsets.
The CPS Scan Group tab reveals the true layout: there is one
flat **999-channel array** at offset `0x0140`, with a parallel
12-byte name array at `0x3FBC`. Categories are just user
chosen scan-group ranges over channel numbers (CH-001 .. CH-999).

| Group     | Channel range | First-channel data offset (× 16) | First-channel name offset (× 12) |
|-----------|---------------|----------------------------------|----------------------------------|
| Åkeri     | CH-055..094   | `0x140 + 54·16  = 0x04A0`        | `0x3FBC + 54·12  = 0x4244`       |
| Jakt      | CH-101..107   | `0x140 + 100·16 = 0x0780`        | `0x3FBC + 100·12 = 0x446C`       |
| SRBR 444  | CH-201..208   | `0x140 + 200·16 = 0x0DC0`        | `0x3FBC + 200·12 = 0x491C`       |
| PMR 446   | CH-301..316   | `0x140 + 300·16 = 0x1400`        | `0x3FBC + 300·12 = 0x4DCC`       |
| 69 MHz    | CH-501..518   | `0x140 + 500·16 = 0x2080`        | `0x3FBC + 500·12 = 0x572C`       |

All five category offsets land exactly on the flat-array math —
confirming the layout. The decoder should iterate
`CH-001..CH-999`, decode the 16-byte data slot at
`0x140 + (n-1)*16`, look up the 12-byte name at
`0x3FBC + (n-1)*12`, and emit only `is_plausible()` slots.

## Confirmed codeplug regions (50 000-byte raw image)

| Range             | Size       | Contents                             |
|-------------------|------------|--------------------------------------|
| `0x0000..0x0084`  | 132 B      | **Settings block** — see "Settings block" subsection below |
| `0x0084..0x0098`  | 20 B       | **Startup message** (ASCII, NUL-pad) |
| `0x0098..0x00B0`  | 24 B       | Brand strings + separator            |
| `0x00B0..0x0140`  | 8 × 16 B   | **VFO state** — 8 entries (see below)|
| `0x0140..0x3FB0`  | 999 × 16 B | **Channel data array** (CH-001..999) |
| `0x3FB0..0x3FBC`  | 12 B       | Padding / unused (TBD)               |
| `0x3FBC..0x6EA0`  | 999 × 12 B | **Channel name array** (CH-001..999) |
| `0x6EA0..0x7270`  | ~970 B     | TBD — likely per-channel scan-group / A-B membership flags |
| `0x7270..0x7278`  | 8 B        | Scan Group "All" data + A/B flags (TBD) |
| `0x7278..0x72A0`  | 10 × 4 B   | **Scan Group ranges** (start, end : u16 LE) |
| `0x72A0..0x7318`  | 10 × 12 B  | **Scan Group names** (slots 1..10)   |
| `0x7318..0x73E0`  | ~200 B     | TBD — possibly band edges and scan-group misc |
| `0x73E0..0x7408`  | 20 × 2 B   | **FM broadcast memories** (u16 LE × 100 kHz) |
| `0x7408..0x766C`  | ~600 B     | TBD — possibly default DTMF code table |
| `0x766C..0xC350`  | ~20 KiB    | **Call Settings** + remaining settings (themes, GPS, DTMF). Group 1 name at `0x766C`; rest TBD. |

## Settings block (`0x0000..0x0084`, 132 B)

Mostly 1-byte enums for Configuration / Key Settings tab
fields. Pinned down by single-field byte-diff captures
(`26..52` in the capture set). Offsets are within the block.

| Offset | Width | Field                  | Encoding / notes |
|--------|-------|------------------------|------------------|
| `0x01` | 1     | Battery Save           | bool (0=off, 1=on) |
| `0x03` | 1     | TOT (Time-Out Timer)   | index; baseline `04`, `01`=15 s |
| `0x05` | 1     | VOX                    | 0=off, 1..10 |
| `0x08` | 1     | Beep                   | bool |
| `0x09` | 1     | Scan Mode              | 0=TO (time), 1=CO (carrier) |
| `0x0A` | 1     | Backlight (seconds)    | `05` = 5 s; other values TBD |
| `0x0B` | 1     | Brightness Active      | 1..10 |
| `0x0D` | 1     | Startup Display        | 0=image, 1=batt voltage |
| `0x0E` | 1     | PTT-ID                 | 0=off, 1=BOT (more TBD) |
| `0x10` | 1     | Sidetone               | 0=off, 1=DTST (more TBD) |
| `0x15` | 1     | Auto Lock              | bool |
| `0x16` | 2     | Priority Channel       | u16 LE channel number |
| `0x19` | 1     | RPT Setting            | semantic TBD (baseline `02`) |
| `0x21` | 1     | Theme                  | 0..3 (4 themes) |
| `0x24` | 1     | Time Zone              | index (`0c` baseline) |
| `0x26` | 1     | GPS On                 | bool |
| `0x48` | 6     | Mode Switch password   | ASCII `'0'..'9'` |
| `0x4E` | 6     | Reset password         | ASCII `'0'..'9'` |
| `0x5C` | 2     | VFO A/B Squelch        | 0..9 each |
| `0x64` | 1     | TopKey                 | 0=Alarm, 1=SOS |
| `0x65` | 1     | PF1 short              | semantic TBD |
| `0x67` | 1     | PF2 long               | semantic TBD |
| `0x68` | 1     | PF3 short              | semantic TBD |
| `0x6E` | 6     | ANI code               | 1 digit/byte; `0x0F`/`0xF0` = pad |
| `0x74` | 6     | SCC code               | same encoding as ANI |

Bytes still TBD inside the block: `0x00`, `0x02`, `0x04`,
`0x06..0x07`, `0x0C`, `0x0F`, `0x11..0x14`, `0x18`,
`0x1A..0x20`, `0x22..0x23`, `0x25`, `0x27..0x47`,
`0x54..0x5B`, `0x5E..0x63`, `0x66`, `0x69..0x6D`,
`0x7A..0x83`. These need fresh single-field captures (PTT
long, Kill, Stun, Monitor, Inspector codes, the rest of the
key-config fields, more theme variants, etc.).

## Channel record (16 bytes per slot)

Per-channel fields visible in the CPS **Channel Information**
tab (image 1):
`CH No | RX Freq | TX Freq | RX CTC/DCS | TX CTC/DCS | TX Power
| W/N | Mute Mode | Scramble | Scan Add | Compand | AM | Call
Group | CH-Name`.

Decoded so far:

```text
bytes 0..4   rx_freq        u32 LE × 10 Hz
bytes 4..8   tx_freq        u32 LE × 10 Hz (0 = simplex; absolute
                                            for repeaters)
bytes 8..10  tone_rx_raw    u16 LE — see "Tone slot encoding"
bytes 10..12 tone_tx_raw    u16 LE — see "Tone slot encoding"
byte  12     power_am_scramble u8 — bits 0..1 = power_idx
                                    (0=low, 1=mid, 2=high, 3=ultrahigh);
                                    bits 2..3 = AM mode
                                    (0=OFF, 1=AM Rx, 2=AM Rx&Tx, 3=unused);
                                    bits 4..7 = scramble level
                                    (0=off, 1..8 = group)
byte  13     flags1         u8 — bit 0 = wide bandwidth
                                 bits 1..2 = mute mode
                                 (0=QT, 1=QT+DTMF, 2=QT*DTMF, 3=unused)
                                 bit 3 = compand
                                 bit 5 = scan add
                                 bits 4, 6, 7 unknown
byte  14     call_group     u8 — 1-based index into the Call
                                 Settings table (image 7).
                                 FB-Radio baseline values:
                                 1 (default / 69M / Simplex),
                                 2 (Riks), 3 (Jakt), 4 (SRBR),
                                 5 (PMR).
byte  15     unknown        u8 — 0x00 on every populated FB-Radio
                                 channel; 0x05 on the eight VFO
                                 entries. Possibly a VFO-only
                                 marker — TBD.
```

### Tone slot encoding

Both `tone_rx_raw` and `tone_tx_raw` use the same `u16` layout:

| high nibble | meaning                |
|-------------|------------------------|
| `0x0`       | no tone                |
| `0x4`       | DCS normal polarity    |
| `0x6`       | DCS inverted polarity  |
| `0x8`       | CTCSS                  |

Low 12 bits = value:
- CTCSS: `freq × 10` deci-Hz (88.5 Hz → `0x375`, 100.0 Hz → `0x3E8`).
- DCS: decimal of the octal display code (`023` oct → 19 dec,
  `754` oct → 492 dec).

### Per-channel record — fully decoded

All 14 fields from the CPS Channel Information tab are now
located in the 16-byte record. The remaining unknowns at byte
15 and `flags1` bits 4/6/7 don't appear to drive any user-
visible UI column in this codeplug — they may be reserved or
flagged differently for the live serial protocol.

## VFO state (`0x00B0..0x0140`, 8 × 16 B)

The eight 16-byte entries we previously called "BAND scanner"
match the **VFO Settings** tab (image 3) exactly:

| Slot | Offset | VFO label | Frequency observed |
|------|--------|-----------|--------------------|
| 0    | `0x0B0`| 150M(A)   | 118.10000 MHz      |
| 1    | `0x0C0`| 400M(A)   | 400.02500 MHz      |
| 2    | `0x0D0`| 200M(A)   | 220.02500 MHz      |
| 3    | `0x0E0`| 66M(A)    | 69.18500 MHz       |
| 4    | `0x0F0`| 800M(A)   | 750.02500 MHz      |
| 5    | `0x100`| 300M(A)   | 350.02500 MHz      |
| 6    | `0x110`| 150M(B)   | 156.00000 MHz      |
| 7    | `0x120`| 400M(B)   | 400.02500 MHz      |

Each entry's first 8 bytes are the rx/tx frequency in the same
`u32 LE × 10 Hz` format as channel records. The trailer bytes
`02 01 01 05` are likely VFO-specific flags (TBD).

The CPS VFO tab also shows per-VFO `Step`, `Squelch`, `RX
CTC/DCS`, `TX CTC/DCS`, `TX Power`, `W/N`, `Mute Mode`, `Shift`,
`Scramble`, `Compand`, `Call Group`, `AM`, plus a separate row
table for `Work Mode`, `Selected Channel`, `Step`, `Squelch`,
`Busy Lockout`, `Current Band`. Most of these need to be
located. The 8 × 16 B block can't fit them all — there's
probably more VFO config later in the image.

## FM broadcast memories (`0x73E0..0x7408`, 20 × 2 B)

20 entries, each `u16 LE × 100 kHz` (so `0x02F8` = 760 = 76.0
MHz, the default in image 4). The bytes my v0.6 decoder
spuriously emitted as `Q336_73E0..7400` channels live in this
region — they should be excluded from channel emission once
the flat-array refactor lands.

## Scan groups (`0x7270..0x7318`)

The CPS Scan Group tab (image 6) shows 11 groups
(`All + 1..10`). Storage:

| Range            | Size        | Contents |
|------------------|-------------|----------|
| `0x7270..0x7278` | 8 B         | "All" group + A/B flags (TBD) |
| `0x7278..0x72A0` | 10 × 4 B    | `(start_ch: u16 LE, end_ch: u16 LE)` |
| `0x72A0..0x7318` | 10 × 12 B   | Group names (slot 1..10), ASCII NUL-pad |

Range pairs validated against the FB-Radio baseline:

| Slot | Range     | Name (baseline) |
|------|-----------|-----------------|
| 1    | 501..518  | `69 MHz`        |
| 2    | 55..94    | `Åkeri`         |
| 3    | 101..107  | `Jakt`          |
| 4    | 201..208  | `SRBR 444MHz`   |
| 5    | 301..316  | `PMR 446MHz`    |
| 6..10 | (defaults) | (blank, default name) |

The `All` group's start/end is implicit (all channels) and
probably backed by the 8 bytes at `0x7270..0x7278`. We model
slots 1..10 only; `All` stays TBD until a capture toggles its
A/B flag.

## Call Settings (`0x766C..`)

Slot 0 of the call-group name table is at `0x766C`, 12 bytes
ASCII NUL-padded (baseline `Allanrop`). Slot pitch and the
per-group `Call Code` field are unknown — captures only
covered group 1's name. Image 7 shows 17 visible groups;
need a rename of group 2+ and a code edit to determine
pitch and offsets.

## TBD regions (locations unknown)

- **Configuration Settings** tab (image 2) leftovers: Roger,
  TOT Pre-alert, Language, Voice Guide, PTT-ID Delay, DTMF
  Transmit/Interval time, Ring Time, ALERT, Priority Scan,
  Hold time of repeat, SCAN-DET, SC-QT, Sub-Frequency Mute,
  TIME SET, GPS SEND TYPE, GPS Receive SW. (Most others now
  located in the Settings block.)
- **VFO Settings** extras (image 3): the `Work Mode / Selected
  Channel / Step / Squelch / Busy Lockout / Current Band` row,
  plus per-VFO `Step / Squelch / Mute Mode / Shift / Scramble /
  Compand / Call Group / AM / TX Power / W / N / RX CTC/DCS /
  TX CTC/DCS`.
- **Scan Group** tab (image 6) leftovers: per-group A/B
  flag bits and the `All` group state — storage probably the
  8 B at `0x7270..0x7278` plus per-channel flags somewhere
  in `0x6EA0..0x7270`.
- **Key Settings** tab (image 5) leftovers: PTT long, PF1
  short alternates, Kill, Stun, Monitor, Inspector codes.
  TopKey + PF1 short + PF2 long + PF3 short + ANI + SCC
  already located in the Settings block.
- **Call Settings** tab (image 7): per-group `Call Code` and
  slot pitch beyond group 1's name. Storage TBD.

## Refined implementation plan

### Phase 1 v1.0 — refactor to flat array (in flight)

1. **Drop `CATEGORIES`** in `src/kgq336/decode.rs`. Replace with
   the flat constants:
   ```rust
   const CHANNEL_DATA_BASE: usize = 0x0140;
   const CHANNEL_NAME_BASE: usize = 0x3FBC;
   const CHANNEL_COUNT: usize = 999;
   const CHANNEL_DATA_SIZE: usize = 16;
   const CHANNEL_NAME_SIZE: usize = 12;
   ```
2. Iterate `n in 1..=999`, decode each slot, skip if not plausible.
3. Synthesise the name `CH_NNN` (3-digit zero-padded) when the
   12-byte name slot is blank — drops the per-category prefix
   logic.
4. Move the 8 VFO entries out of the channel emission. Add
   `vfo_state: [Option<VfoEntry>; 8]` to `DecodeReport` (or a
   smaller struct).
5. Add a `fm_broadcast: [Option<u32>; 20]` field decoded from
   `0x73E0..0x7408`. (Each `u16` × 100 kHz → Hz.)
6. Drop the `Q336_<offset>` synthetic-name fallback and the
   `MAX_SHIFT_HZ` heuristic — they were Band-Aids for a wrong
   layout. The flat array's `is_plausible` filter is enough.

### Phase 1 v1.1 — done

All per-channel fields located. Captures 22-25 confirmed:

- **Mute Mode** (3 values) — `flags1` bits 1..2: `0`=QT,
  `1`=QT+DTMF (`0x02` set), `2`=QT*DTMF (`0x04` set).
- **AM Mode** (3 values) — `power_am_scramble` bits 2..3:
  `0`=OFF (FM), `1`=AM Rx (`0x04` set), `2`=AM Rx&Tx
  (`0x08` set).

The decoder emits `Mode::Am` instead of `Mode::Fm` for any
non-zero AM mode (`AM Rx` and `AM Rx&Tx` both); the Rx vs
Rx&Tx distinction is dropped and warned about. Mute Mode is
always warned about when non-default since narm has no
Mute Mode field.

### Phase 1 v2.0 — radio-level settings

- **FM broadcast memories**: done.
- **Startup message**: done.
- **Settings block**: ~70% mapped — see the Settings block
  subsection above. ~10 byte ranges still TBD; need fresh
  single-field captures for PTT long, Kill/Stun/Monitor/
  Inspector, Roger, Voice Guide, etc.
- **Scan groups**: ranges + names done (slots 1..10). The
  `All` group state and per-channel A/B membership flags
  (likely in `0x6EA0..0x7270`) still TBD.
- **Call Settings**: group 1 name done. Slot pitch and the
  per-group `Call Code` field still TBD — need a rename of
  group 2+ and a code edit.

### Phase 1 v3.0 — narm schema extensions (optional)

To round-trip more state into TOML, narm's `Mode::Fm` would
need new fields or sibling structs:

- `scramble: Option<u8>`     — currently warned, not surfaced
- `compand: bool`            — currently warned
- `dcs_polarity: Polarity`   — currently warned
- `mute_mode: MuteMode`      — currently TBD
- `am_mode: bool`            — narm has `Mode::Am` already, so
                                this is a write-side concern
- `call_group: Option<u8>`   — radio-specific; not core narm

Each is an additive change, but they affect the Channel struct
shared with other radios. Probably worth a separate design
pass.

### Phase 2 — live serial protocol (unchanged)

- Capture USB traffic from CPS via VirtualBox + usbmon while
  doing a full read.
- Implement `wire.rs` framing + opcode structs and `io.rs`
  read loop.
- Hook into `narm radio read --port`.

### Phase 3 — encode + write

- `encode_channels(channels: &[Channel]) -> Vec<u8>` produces
  the raw 50 000-byte image.
- `mojibake` already exists for the file path.
- `write_image` for the live-serial path (Phase 2 wire protocol
  reused in reverse).

## CPS UI screenshot summary (from image-cache batch)

So the screenshots don't have to be re-pasted:

1. **Channel Information** — the 14-column channel grid. Row
   highlight on row 55 = `RIKS_TEST` (CH-055), 85.93750 simplex,
   High / Wide / Mute Mode QT / Scan Add ON / Call Group 2.
2. **Configuration Settings** — radio-wide parameters in 3
   tables (function/setting pairs), plus 4 user-editable themes.
3. **VFO Settings** — A/B Work Mode + Selected Channel + Step +
   Squelch + Busy Lockout + Current Band; then an 8-column
   table (six A bands + two B bands) with `Current Freq`,
   `Offset`, `RX/TX CTC/DCS`, `TX Power`, `W/N`, `Mute Mode`,
   `Shift`, `Scramble`, `Compand`, `Call Group`, `AM`.
4. **FM Broadcast Memories** — 20 channels of FM broadcast
   presets, all 76.0 MHz default in this codeplug.
5. **Key Settings** — TopKey / PF1 / PTT / PF2 short+long /
   PF3 short+long; CONTR section with ANI-EDIT, SCC-EDIT,
   Kill, Stun, Monitor, Inspector codes; **Startup Message**
   text field showing `www.fbradio.se`.
6. **Scan Group** — table of 11 scan groups (All + 1..10) with
   A/B radio buttons, start/end channel, group name. VFO Scan
   Mode side panel with A/B range + mode dropdown.
7. **Call Settings** — table of 17 visible call groups (Group #,
   Call Code, Call Name). Group 1 = `70000 / Allanrop`; rest
   default to `123456`.
