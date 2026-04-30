# Wouxun KG-Q332 / KG-Q336 codeplug reference

Self-contained spec for talking to a KG-Q332 / KG-Q336 over its
programming cable and decoding what comes out — wire protocol,
file format, EEPROM layout, channel record format, the lot.
A future CHIRP driver author or third-party CPS clone should be
able to implement read + write from this file alone.

The Q332 and Q336 are the same radio at the link layer and
codeplug layer; only the band plan and a few labels differ.
This document treats them as one.

## Layouts at a glance

There are **three distinct layouts** for the same data — keep
them straight or nothing else makes sense.

| Layout | Where it lives | Origin |
|---|---|---|
| **Physical** | Radio EEPROM; the bytes you read or write over the serial cable. | The radio's actual storage. Each 1 KiB block is split into 4 × 256-byte slices, stored in *reverse* order. Almost certainly an artifact of however the firmware lays out flash pages. |
| **Logical** | An in-memory image after applying the slice-reversal transform to the physical bytes. | What CHIRP's `kgq10h` driver works with; what every struct definition in this doc is keyed off. **All offsets below are logical** unless explicitly tagged "physical" or ".kg". |
| **`.kg` file** | What the vendor CPS saves to disk via *File → Save*. | Yet another rearrangement, organised by CPS UI category. Scan groups, channel names, and call settings live at completely different offsets from logical. Mostly an interop concern; live reads bypass it entirely. |

Total addressable memory in all three layouts is `0x8000` bytes
(32 KiB). The `.kg` file *also* expands to ≈ 50 000 bytes on
disk because of a UTF-8 wrapper (see "File wrapper").

## Physical → logical conversion

Each 1 KiB physical block (`0x400` bytes) is stored as four
256-byte slices in reverse order:

```text
  Physical 0x0000..0x0100  ↔  Logical 0x0300..0x0400
  Physical 0x0100..0x0200  ↔  Logical 0x0200..0x0300
  Physical 0x0200..0x0300  ↔  Logical 0x0100..0x0200
  Physical 0x0300..0x0400  ↔  Logical 0x0000..0x0100
  (repeats per 1 KiB block, 32 blocks total to 0x8000)
```

Closed-form (CHIRP `convert_address` translates a *logical*
address to the *physical* address you have to send the radio):

```python
def physical(logical: int) -> int:
    q, r = divmod(logical, 0x400)
    slice_idx = r // 0x100
    return (q + 1) * 0x400 - (slice_idx + 1) * 0x100 + (r % 0x100)
```

Inverse is the same function: `physical(physical(x)) == x`.

In Rust the simplest implementation is a buffer-level pass: for
each 1 KiB block, copy slice `i` to slice `3 - i`. That's what
`unscramble_image` does in narm.

## Wire protocol

The radio is reached via a Wouxun K-plug programming cable —
a USB-to-TTL adapter (PL2303 in the wild) terminating in a
2-pin Kenwood plug. From narm's perspective it's just a serial
port (`/dev/ttyUSB0` on Linux).

### Line settings

| Parameter | Value |
|---|---|
| Baud | **115 200** |
| Bits | 8 |
| Parity | none |
| Stop bits | 1 |
| Flow control | none |
| DTR | asserted (high) |
| RTS | deasserted (low) — many K-plug cables wire RTS to a PTT transistor; keeping it low keeps the cable in RX mode |

Note: the older kg935g family runs at 19200; only the Q33x
family is at 115200. The K-plug cable itself doesn't care.

### Frame format

Every frame, both directions, has a 4-byte header followed by an
encrypted payload+checksum blob.

```text
  +------+-----+-----+-----+--------------------------------+
  | 0x7C | cmd | dir | len | enc(payload || cksum)          |
  +------+-----+-----+-----+--------------------------------+
     1B    1B    1B    1B           (len + 1) bytes
```

| Field | Value |
|---|---|
| `0x7C` | start-of-frame, fixed |
| `cmd` | command byte (see table below) |
| `dir` | `0xFF` host → radio, `0x00` radio → host |
| `len` | length of the *plaintext* payload, **excluding** the trailing checksum byte |

Total frame size on the wire = `5 + len` bytes (header 4 +
encrypted payload `len` + encrypted checksum 1).

### Commands

| Code | Name | Direction | Notes |
|---|---|---|---|
| `0x80` | `CMD_ID` | both | Reserved by the kg935g family for an explicit model probe. **Q33x doesn't use it** — CPS does the model probe with `CMD_RD` at address `0x0040` instead. |
| `0x81` | `CMD_END` | host → radio | Session end. Empty payload. The cksum byte is encrypted normally; full wire bytes are the constant `7C 81 FF 00 D7`. |
| `0x82` | `CMD_RD` | both | Read N bytes from a 16-bit physical address. |
| `0x83` | `CMD_WR` | host → radio | Write N bytes to a 16-bit physical address. The radio acks with a `CMD_WR` reply (TBD details). |

### Cipher

A rolling-XOR cipher with a single-byte seed. Same algorithm as
kg935g and kguv9dplus; only the seed byte is per-radio:

| Driver | Seed |
|---|---|
| kg935g | `0x57` |
| kguv9dplus | `0x52` |
| **KG-Q332 / Q336** | **`0x54`** |

Encrypt (host → radio, also for the cksum byte at the tail):

```python
def encrypt(plain, seed=0x54):
    out = bytearray(len(plain))
    out[0] = seed ^ plain[0]
    for i in range(1, len(plain)):
        out[i] = out[i - 1] ^ plain[i]
    return bytes(out)
```

Decrypt is symmetric:

```python
def decrypt(enc, seed=0x54):
    out = bytearray(len(enc))
    out[0] = seed ^ enc[0]
    for i in range(1, len(enc)):
        out[i] = enc[i - 1] ^ enc[i]
    return bytes(out)
```

The cipher applies to the entire `(payload || cksum)` blob as a
single `(len + 1)`-byte stream.

### Checksum

A sum-mod-256 of `cmd || dir || len || plain_payload`, with a
small per-frame *adjustment* picked from a 4-entry table by the
low 2 bits of `payload[0]` (which is always `addr_hi` in Q336
frames):

| `payload[0] & 0x03` | adjustment |
|---|---|
| `0b00` | +3 |
| `0b01` | +1 |
| `0b10` | -1 |
| `0b11` | -3 |

```python
def checksum(cmd, dir_byte, length, plain_payload):
    s = (cmd + dir_byte + length + sum(plain_payload)) & 0xFF
    adj = {0: 3, 1: 1, 2: -1, 3: -3}[(plain_payload[0] if plain_payload else 0) & 0x03]
    return (s + adj) & 0xFF
```

For an empty payload (only `CMD_END` qualifies) the adjustment
input defaults to 0 → +3, which is how `CMD_END`'s constant
trailing `0xD7` falls out.

This adjustment is **specific to the Q33x family**; kg935g uses
plain sum-mod-256, kguv9dplus uses a 4-bit cksum. It's almost
certainly mild anti-tamper — the table is too small to add any
real error-detection value.

### CMD_RD payload (both directions)

Host → radio plaintext (`len = 3`):

```text
  +----------+----------+--------+--------+
  | addr_hi  | addr_lo  | length | cksum  |
  +----------+----------+--------+--------+
```

`length` is `0x40` (64) in every CPS-observed read.

Radio → host plaintext (`len = 0x42`, 66 bytes):

```text
  +----------+----------+----------------+--------+
  | addr_hi  | addr_lo  | data * 0x40    | cksum  |
  +----------+----------+----------------+--------+
```

`addr_hi` and `addr_lo` echo the requested address. The address
is a *physical* address — the radio doesn't know about the
logical layout.

### CMD_WR payload (host → radio)

`len = 0x22` (34), 32 bytes of data:

```text
  +----------+----------+----------------+--------+
  | addr_hi  | addr_lo  | data * 0x20    | cksum  |
  +----------+----------+----------------+--------+
```

Reads are 64-byte blocks; writes are 32-byte blocks.

Empty (unprogrammed) memory contents are `0x00`, **not** `0xFF`
— this surfaces as the all-equal-byte ciphertext signature for
empty pages: rolling-XOR of `[X, 0, 0, 0, …]` produces
`[seed^X, seed^X, seed^X, …]`.

### Handshake

Every session starts with a 3-burst wake-up. Per Mel Terechenok's
KG-Q33x driver (CHIRP issue #10880, `kgq10h ... 2025mar16.py`):

> Wouxun CPS sends the same Read command 3 times to establish
> comms.

Concretely:

1. Open the port (115200 8N1, DTR=1, RTS=0). Sleep ~200 ms to
   let the cable's level shifter settle.
2. Write the canonical model-probe frame **three times
   back-to-back** with no read in between:
   `7C 82 FF 03 54 14 54 53` (= `CMD_RD` of physical address
   `0x0040`, 64 bytes). Subsequent frames use the normal one
   write / one read pattern.
3. Read one reply. Bytes 46..53 of the decrypted payload contain
   the model string (`KG-Q336` or `KG-Q332`); see "Logical
   memory map → OEM info".

The radio doesn't display the green "programming" indicator on
its LCD until the line speed is correct (115200) — wrong baud
gets you silence, not a half-broken handshake.

After the model probe the host can issue `CMD_RD` / `CMD_WR` at
arbitrary physical addresses in `[0x0040, 0x8000)`. Addresses
below `0x0040` aren't accepted. End the session with a single
`CMD_END` (`7C 81 FF 00 D7`) — no reply.

## File wrapper (`.kg`)

The vendor CPS does **not** save its codeplug as a plain binary
file. Every `.kg` is a "text" wrapper:

```text
  "xiepinruanjian\r\n"   ← 14-byte ASCII header (vendor
                            branding; pinyin for "协频软件")
  <body>                  ← UTF-8 of the Latin-1 reading of the
                            raw image — see encoding below
  "\r\n"                  ← 2-byte ASCII footer
```

Each binary byte of the underlying image is encoded:

- `0x00..0x7F` → emitted as-is (one byte).
- `0x80..0xFF` → emitted as the **2-byte UTF-8 sequence for the
  same Unicode codepoint**. So `0x80..0xBF` becomes `c2 XX` and
  `0xC0..0xFF` becomes `c3 XX`.

That's exactly what you get if a program does
`bytes.decode("latin-1").encode("utf-8")` — a common mojibake
mistake. The CPS apparently shoves the image into a string
field on save, and the expansion happens implicitly. Reading
or writing a `.kg` without reversing this encoding will not
work.

The reverse:

```python
HEADER = b"xiepinruanjian\r\n"
FOOTER = b"\r\n"

def unmojibake(file_bytes: bytes) -> bytes:
    assert file_bytes.startswith(HEADER)
    assert file_bytes.endswith(FOOTER)
    body = file_bytes[len(HEADER):-len(FOOTER)]
    return body.decode("utf-8").encode("latin-1")

def mojibake(raw: bytes) -> bytes:
    return HEADER + raw.decode("latin-1").encode("utf-8") + FOOTER
```

In narm: `src/kgq336/file.rs::unmojibake()` and `mojibake()`.

The post-unmojibake size is **50 000 bytes** — bigger than the
radio's 32 KiB EEPROM. The extra ~18 KiB is whatever CPS adds
on top of (or interleaved with) the codeplug data: padding,
CPS-private metadata, expanded tables, etc. Mapping `.kg` ↔
logical isn't a single shift across the whole file; deltas
vary by region in steps of `0x200`, suggesting CPS rearranges
things by category for its UI. This is mostly a separate
reverse-engineering problem and live reads sidestep it.

**The Q336 settings struct is the kgq10h struct minus exactly
three phantom fields**: `wxalert`, `wxalert_type`, and
`batt_ind`. With those removed, the `.kg ↔ logical` shift is a
clean **+`0x0440` across the entire settings region** —
empirically verified with 9 single-byte captures spanning
`.kg 0x01..0x21`.

| Phantom field | kgq10h logical | Why it's not on Q336 |
|---|---|---|
| `wxalert` | `0x0445` | NOAA wx alert — Q336 has no NOAA receiver |
| `wxalert_type` | `0x0446` | NOAA wx alert type — same reason |
| `batt_ind` | `0x0462` | Battery indicator — not a CPS-configurable Q336 setting |

So for the Q336 logical layout, take the kgq10h struct from
`0x0440` and apply this transform:

```
fields up to and including `toalarm`        : same logical offset
fields from `vox` through `smuteset`        : logical -= 2
fields from `ToneScnSave` onwards           : logical -= 3
```

Equivalently: the Q336 has `unk_xp8` (now identified as
**Language**) where kgq10h has `wxalert`, etc. The clean
shift is preserved against `.kg`-space for all 9 captures
because the phantoms aren't in the `.kg` either; CPS knows
the same Q336 field set as the radio firmware.

## Logical memory map

All offsets in this section are **logical** — i.e., into the
post-unscramble image. They come from `_MEM_FORMAT_Q332_oem_read_nolims`
in CHIRP's `kgq10h ... 2025mar16.py`, the only published
authoritative reference. Field types use CHIRP's bitwise syntax
(`u8` = 1 byte, `ul16` = u16 little-endian, etc.).

### Region overview (`0x0000..0x8000`)

| Range | Size | Contents |
|---|---|---|
| `0x000A` | 1 B | `oem_info.locked` |
| `0x0340..0x0398` | ~88 B | OEM info — model strings, firmware version, build date |
| `0x0440..0x0500` | ~192 B | Settings struct (radio-wide config) |
| `0x0540..0x05A0` | 6 × 16 B | VFO A entries (six bands) |
| `0x05A0..0x05E0` | 2 × 16 B | VFO B entries |
| `0x05E0..0x4460` | 1000 × 16 B | Channel data array |
| `0x4460..0x6240` | 1000 × 12 B | Channel name array |
| `0x7340..0x7728` | 1000 B | Channel valid array (1 byte per channel) |
| `0x7740..0x7768` | 10 × 4 B | Scan group ranges |
| `0x7768..0x77E0` | 10 × 12 B | Scan group names |
| `0x77E0..0x77E8` | 8 B | VFO scan range A/B |
| `0x78B0..0x78D8` | 20 × 2 B | FM broadcast presets |
| `0x78E0..0x7B40` | 100 × 6 B | Call IDs |
| `0x7B40..0x7FB0` | 100 × 12 B | Call names |
| (else) | | TBD — reserved, calibration, or unused |

### OEM info

Read-only, factory-set strings. Exact values vary by unit; the
fields below come from a sample with firmware `VA1.27`, build
date `2024-12-3`.

| Logical offset | Field | Type | Sample |
|---|---|---|---|
| `0x000A` | `locked` | `u8` | (lock flag, unverified) |
| `0x0340` | `oem1` | `char[8]` | `"WOUXUN  "` |
| `0x036C` | `name` | `char[8]` | `"KG-Q336 "` |
| `0x0378` | `date` | `char[10]` | `"2024-12-3 "` |
| `0x0392` | `firmware` | `char[6]` | `"VA1.27"` |

The model-probe handshake's first read returns 64 bytes from
physical `0x0040`, which is logical `0x0340`. The `name` field
at logical `0x036C` becomes bytes 46..53 of that decrypted
payload (= `0x036C - 0x0340 + 2` for the leading address echo)
— that's how CHIRP's `_identify` extracts the model.

### Settings struct (`0x0440..0x0500`)

#### Confirmed Q336 positions (`.kg`-derived, ground truth)

These positions come from single-field byte-diff captures
against a baseline `.kg` save. Each row is a CPS field where
we've watched exactly one byte (or run of bytes) change in
response to the corresponding UI toggle.

| `.kg` offset | Field | Encoding |
|---|---|---|
| `0x01` | Battery Save | bool (0=off, 1=on) |
| `0x02` | Roger | 0=OFF, 1=BOT, 2=EOT, 3=BOTH |
| `0x03` | TOT (Time-Out Timer) | u8 index (baseline `0x04` = 60S; full enum TBD) |
| `0x04` | TOT Pre-alert | u8 seconds (`0x00`=OFF, `0x01..0x0A`=1S..10S; direct integer) |
| `0x05` | VOX | u8 level (0=off, 1..10) |
| `0x06` | Language | enum: `0x00`=CHS, `0x01`=EN, `0x02`=TC (resolves kgq10h's `unk_xp8`) |
| `0x07` | Voice Guide | bool (0=off, 1=on); = kgq10h's `voice` |
| `0x08` | Beep | bool |
| `0x09` | Scan Mode | 0=TO (time-op.), 1=CO (carrier-op.) |
| `0x0A` | Backlight (seconds) | u8 (5 in baseline) |
| `0x0B` | Brightness (active) | u8 1..10 |
| `0x0D` | Startup Display | 0=image, 1=batt voltage |
| `0x0E` | PTT-ID | 0=off, 1=BOT (more TBD) |
| `0x10` | Sidetone | 0=off, 1=DTST (more TBD) |
| `0x11` | DTMF Transmit Time | u8, units of 10 ms (`0x08`=80 ms baseline; `0x0A`=100 ms; `0x32`=500 ms) |
| `0x14` | ALERT (tone freq) | enum: `0x00`=1750 Hz, `0x01`=2100 Hz, `0x02`=1000 Hz, `0x03`=1450 Hz (dropdown-order index, not the frequency itself) |
| `0x15` | Auto Lock | bool |
| `0x16..0x18` | Priority Channel | `ul16` channel number |
| `0x19` | RPT Setting | u8 (TBD encoding); = kgq10h's `rpttype` |
| `0x1A` | RPT-SPK | bool (0=off, 1=on); = kgq10h's `rpt_spk` |
| `0x1E` | SCAN-DET | bool (0=off, 1=on); = kgq10h's `scan_det` |
| `0x1F` | Sub-Frequency Mute | enum: `0x00`=OFF, `0x01`=Rx (others Tx/Rx&Tx, dropdown-order); = kgq10h's `smuteset` |
| `0x20` | SC-QT | enum: `0x00`=RX QT/DT-ME (baseline), `0x01`=TX QT/DT-ME (others Rx&Tx Qt/Dt-S, dropdown-order); = kgq10h's `ToneScnSave` |
| `0x21` | Theme | u8 0..3 |
| `0x24` | Time Zone | u8 index |
| `0x26` | GPS On | bool |
| `0x48..0x4E` | Mode Switch password | ASCII digits `'0'..'9'` |
| `0x4E..0x54` | Reset password | same |
| `0x5C..0x5E` | VFO A/B Squelch | `[u8; 2]`, each 0..9 |
| `0x64` | TopKey | 0=Alarm, 1=SOS (more TBD) |
| `0x65` | PF1 short | TBD |
| `0x67` | PF2 long | TBD |
| `0x68` | PF3 short | TBD |
| `0x6E..0x74` | ANI code | 6 digits, `0x0F`-padded |
| `0x74..0x7A` | SCC code | same encoding as ANI |

#### kgq10h-derived field map (logical, **drift-prone for Q336**)

The struct definition below comes from the kgq10h driver,
which targets Q10H first and was extended to Q33x. Several
field positions don't match Q336 reality (see drift table
above). Use this only as a *what fields exist* hint; for
*where they live*, trust the `.kg` table above.

| Logical | Field | Type | Notes |
|---|---|---|---|
| `0x0440` | `channel_menu` | `u8` | |
| `0x0441` | `power_save` | `u8` | bool |
| `0x0442` | `roger_beep` | `u8` | 0=OFF, 1=BOT, 2=EOT, 3=BOTH (CPS Roger dropdown) |
| `0x0443` | `timeout` | `u8` | TOT (Time-Out Timer) index |
| `0x0444` | `toalarm` | `u8` | TOT alarm |
| `0x0445` | `wxalert` | `u8` | NOAA wx alert |
| `0x0446` | `wxalert_type` | `u8` | |
| `0x0447` | `vox` | `u8` | 0=off, 1..10 |
| `0x0448` | `unk_xp8` | `u8` | TBD |
| `0x0449` | `voice` | `u8` | voice prompt |
| `0x044A` | `beep` | `u8` | bool |
| `0x044B` | `scan_rev` | `u8` | scan resume mode |
| `0x044C` | `backlight` | `u8` | seconds |
| `0x044D` | `DspBrtAct` | `u8` | brightness, active |
| `0x044E` | `DspBrtSby` | `u8` | brightness, standby |
| `0x044F` | `ponmsg` | `u8` | startup-display mode |
| `0x0450` | `ptt_id` | `u8` | (driver comment: "0x530"; that's `0x0440 + 0x10 = 0x450` ✓) |
| `0x0451` | `ptt_delay` | `u8` | |
| `0x0452` | `dtmf_st` | `u8` | DTMF sidetone |
| `0x0453` | `dtmf_tx_time` | `u8` | |
| `0x0454` | `dtmf_interval` | `u8` | |
| `0x0455` | `ring_time` | `u8` | |
| `0x0456` | `alert` | `u8` | |
| `0x0457` | `autolock` | `u8` | bool |
| `0x0458` | `pri_ch` | `ul16` | priority channel number |
| `0x045A` | `prich_sw` | `u8` | priority-channel switch |
| `0x045B` | `rpttype` | `u8` | repeater type |
| `0x045C` | `rpt_spk` | `u8` | |
| `0x045D` | `rpt_ptt` | `u8` | |
| `0x045E` | `rpt_tone` | `u8` | |
| `0x045F` | `rpt_hold` | `u8` | |
| `0x0460` | `scan_det` | `u8` | |
| `0x0461` | `smuteset` | `u8` | sub-mute set |
| `0x0462` | `batt_ind` | `u8` | battery indicator |
| `0x0463` | `ToneScnSave` | `u8` | |
| `0x0464` | `theme` | `u8` | 0..3 (4 themes) |
| `0x0465` | `unkx545` | `u8` | TBD |
| `0x0466` | `disp_time` | `u8` | display time on/off |
| `0x0467` | `time_zone` | `u8` | |
| `0x0468` | `GPS_send_freq` | `u8` | |
| `0x0469` | `GPS` | `u8` | bool |
| `0x046A` | `GPS_rcv` | `u8` | |
| `0x046B..0x048B` | `custcol1..4_*` | 16 × `ul16` | RGB565 values for 4 custom themes (text/bg/icon/line each) |
| `0x048B` | `mode_sw_pwd` | `char[6]` | mode-switch password (ASCII digits, `0x0F`-padded) |
| `0x0491` | `reset_pwd` | `char[6]` | reset password |
| `0x0497` | `work_mode_a` | `u8` | |
| `0x0498` | `work_mode_b` | `u8` | |
| `0x0499` | `work_ch_a` | `ul16` | |
| `0x049B` | `work_ch_b` | `ul16` | |
| `0x049D` | `vfostepA` | `u8` | |
| `0x049E` | `vfostepB` | `u8` | |
| `0x049F` | `squelchA` | `u8` | 0..9 |
| `0x04A0` | `squelchB` | `u8` | 0..9 |
| `0x04A1` | `BCL_A` | `u8` | busy-channel lockout A |
| `0x04A2` | `BCL_B` | `u8` | |
| `0x04A3` | `vfobandA` | `u8` | |
| `0x04A4` | `vfobandB` | `u8` | |
| `0x04A7` | `top_short` | `u8` | TopKey short-press code |
| `0x04A8` | `top_long` | `u8` | TopKey long-press |
| `0x04A9` | `ptt1` | `u8` | |
| `0x04AA` | `ptt2` | `u8` | |
| `0x04AB` | `pf1_short` | `u8` | |
| `0x04AC` | `pf1_long` | `u8` | |
| `0x04AD` | `pf2_short` | `u8` | |
| `0x04AE` | `pf2_long` | `u8` | |
| `0x04AF` | `ScnGrpA_Act` | `u8` | active scan group on side A |
| `0x04B0` | `ScnGrpB_Act` | `u8` | |
| `0x04B1` | `vfo_scanmodea` | `u8` | |
| `0x04B2` | `vfo_scanmodeb` | `u8` | |
| `0x04B3` | `ani_id` | `u8[6]` | ANI digits, `0x0F`-padded |
| `0x04B9` | `scc` | `u8[6]` | SCC digits |
| `0x04C1` | `act_area` | `u8` | active area (A/B) |
| `0x04C2` | `tdr` | `u8` | dual receive on/off |
| `0x04C3` | `keylock` | `u8` | |
| `0x04C7` | `stopwatch` | `u8` | |
| `0x04C8` | `x0x04c8` | `u8` | TBD |
| `0x04C9` | `dispstr` | `char[12]` | startup-message text (e.g. `"fbradio.se"`) |
| `0x04DD` | `areamsg` | `char[12]` | area-message text |
| `0x04E9..0x04F4` | `xunk_*`, `xani_*`, … | various | secondary key/PTT codes; mostly TBD |
| `0x04F5` | `main_band` | `u8` | |
| `0x04F6..0x04FB` | `xTDR_single_mode`, `xunk1`, `xunk2`, `cur_call_grp`, `VFO_repeater_a`, `VFO_repeater_b` | `u8` × 6 | |
| `0x04FC` | `sim_rec` | `u8` | simultaneous record |

### VFO A / VFO B records

Six VFO A entries (one per band) at logical `0x0540`, two VFO B
at `0x05A0`. Each is 16 bytes:

```text
ul32  rxfreq            ← × 10 Hz
ul32  offset            ← × 10 Hz, signed (TX = RX + offset)
ul16  rxtone            ← see "Tone slot encoding"
ul16  txtone
u8    scrambler:4       ← bits 4..7
u8    am_mode:2         ← bits 2..3
u8    power:2           ← bits 0..1
u8    ofst_dir:3        ← bits 5..7 (offset direction sign)
u8    unknown:1         ← bit  4
u8    compander:1       ← bit  3
u8    mute_mode:2       ← bits 1..2
u8    iswide:1          ← bit  0
u8    call_group
u8    unknown6
```

VFO A bands (in order at `0x0540`): 150M-A, 400M-A, 200M-A,
66M-A, 800M-A, 300M-A. VFO B at `0x05A0`: 150M-B, 400M-B.

### Channel record (1000 entries at `0x05E0`, 16 B each)

The radio supports 1000 channels (CPS exposes 999; channel 0 is
either reserved or unused). Each record:

```text
ul32  rxfreq            ← × 10 Hz
ul32  txfreq            ← × 10 Hz; 0 = simplex; absolute (not
                          shift) for repeaters
ul16  rxtone            ← see "Tone slot encoding"
ul16  txtone
u8    scrambler:4       ← bits 4..7 (0 = off, 1..8 = group)
u8    am_mode:2         ← bits 2..3 (0=FM, 1=AM Rx, 2=AM Rx+Tx,
                                     3=unused)
u8    power:2           ← bits 0..1 (0=L, 1=M, 2=H, 3=Ultra)
u8    unknown3:1        ← bit  7
u8    send_loc:1        ← bit  6 (GPS location send on this ch)
u8    scan_add:1        ← bit  5
u8    favorite:1        ← bit  4
u8    compander:1       ← bit  3
u8    mute_mode:2       ← bits 1..2 (0=QT, 1=QT+DTMF,
                                     2=QT*DTMF, 3=unused)
u8    iswide:1          ← bit  0 (1 = 25 kHz, 0 = 12.5 kHz)
u8    call_group        ← 1-based index into Call Settings
u8    unknown6          ← TBD (always 0 in observed data)
```

### Channel valid array (`0x7340`, 1000 B)

One byte per channel. Non-zero = valid; zero = empty slot.
CHIRP's struct names this `valid[1000]`. Particular non-zero
encodings (active/transmit/receive-only flags) are TBD.

### Channel names (`0x4460`, 1000 × 12 B)

Parallel array to `memory[1000]`. Each slot is a 12-byte ASCII
string, NUL-padded (or `0xFF`-padded — radios in this family
mix the two). Empty = blank slot.

### Tone slot encoding

Both `rxtone` and `txtone` use the same `u16` little-endian
layout:

| High nibble | Meaning |
|---|---|
| `0x0` | no tone |
| `0x4` | DCS, normal polarity |
| `0x6` | DCS, inverted polarity |
| `0x8` | CTCSS |

Low 12 bits = value:

- CTCSS: `freq × 10` deci-Hz. So `88.5 Hz → 0x375`,
  `100.0 Hz → 0x3E8`.
- DCS: decimal of the octal display code. `023 oct → 19 dec`,
  `754 oct → 492 dec`.

### Scan groups (`0x7740..0x77E0`)

Ten user-definable channel-range groups + a synthetic "All"
group displayed in the CPS UI (probably stored separately or
synthesised at runtime).

| Logical | Field | Type |
|---|---|---|
| `0x7740` | `addrs[10]` | 10 × `{ ul16 scan_st; ul16 scan_end; }` |
| `0x7768` | `names[10]` | 10 × `char[12]` |

Sample (FB-Radio baseline):

| Slot | Range | Name |
|---|---|---|
| 1 | 501..518 | `69 MHz` |
| 2 | 55..94 | `Åkeri` |
| 3 | 101..107 | `Jakt` |
| 4 | 201..208 | `SRBR 444MHz` |
| 5 | 301..316 | `PMR 446MHz` |
| 6..10 | (defaults) | (blank / default) |

### VFO scan ranges (`0x77E0`)

```text
ul16  vfo_scan_start_A
ul16  vfo_scan_end_A
ul16  vfo_scan_start_B
ul16  vfo_scan_end_B
```

### FM broadcast presets (`0x78B0`, 20 × 2 B)

Twenty `ul16` slots, each `freq_kHz / 100` (so
`0x02F8 = 760 = 76.0 MHz`, the factory default).

### Call IDs (`0x78E0`, 100 × 6 B)

Per-group caller ID digits; 6 bytes per slot, digit-per-byte
(`0..9`), with `0x0F` as end-of-list and `0xF0` as filler.
Same encoding as `ani_id` and `scc` in the settings struct.

### Call names (`0x7B40`, 100 × 12 B)

Parallel ASCII names for each call ID. Slot 0 baseline =
`"Allanrop"`. Slot pitch is 12 bytes; each entry is
NUL/`0xFF`-padded.

## Channel-record CPS UI mapping

For reference when matching narm's TOML schema or the CPS UI
columns to fields above:

| CPS column | Field |
|---|---|
| CH No | (implicit — channel index) |
| RX Freq | `rxfreq` |
| TX Freq | `txfreq` (or `rxfreq + offset` for VFO records) |
| RX CTC/DCS | `rxtone` (decoded per tone slot table) |
| TX CTC/DCS | `txtone` |
| TX Power | `power` (0=L, 1=M, 2=H, 3=Ultra) |
| W/N | `iswide` (1 = wide / 25 kHz, 0 = narrow / 12.5 kHz) |
| Mute Mode | `mute_mode` (QT / QT+DTMF / QT*DTMF) |
| Scramble | `scrambler` (0=off, 1..8) |
| Scan Add | `scan_add` |
| Compand | `compander` |
| AM | `am_mode` (0=FM, 1=AM Rx, 2=AM Rx+Tx) |
| Call Group | `call_group` (1-based) |
| CH Name | `names[n]` |

## Empty-memory pattern

Unprogrammed bytes in the radio's EEPROM are `0x00`, not
`0xFF`. This matters for two reasons:

1. The "all bytes equal" signature in encrypted CMD_WR traffic
   is `[seed^X, seed^X, …]` where `X` is the *first* plaintext
   byte (typically the addr_hi being written) — not what you'd
   expect for an `0xFF`-fill.
2. When testing: a fresh blank channel slot reads as 16 bytes
   of `0x00`, not `0xFF`.

## TBD areas

In rough priority order:

- **Channel valid encoding**: `valid[1000]` is non-zero for
  populated channels but the precise meaning (active flag vs
  RX-only vs TX-only vs other) is unverified.
- **Settings struct unknowns**: `unk_xp8`, `unkx545`, `x0x04c8`,
  `xunk1`, `xunk2`, `xTDR_single_mode`, `unknown6` in the VFO
  and channel records. Need single-field captures.
- **Per-channel scan-group / A-B membership flags**: storage
  TBD; not present in the kgq10h struct, but it's a CPS UI
  feature so the bytes are *somewhere*. Likely either bit-
  packed alongside `valid[]` or in an unmapped region.
- **`.kg` ↔ logical transform**: deltas vary by region in
  `0x200`-byte steps, suggesting CPS rearranges by category.
  Not blocking for live reads.
- **CMD_WR reply format**: the radio acks each write but the
  exact reply structure isn't decoded yet — no captures since
  we haven't tested writes.

## Implementation notes

### Reading (live)

1. `open_port(115200, 8N1, DTR=1, RTS=0)`, sleep 200 ms.
2. Build the canonical model probe `7C 82 FF 03 54 14 54 53`,
   write it 3× back to back.
3. Read one frame, verify cksum, extract bytes 2..2+64 = the
   first physical block.
4. Loop addr `= 0x0080, 0x00C0, …, 0x7FC0`: send `CMD_RD`,
   read reply, place data at `image[addr..addr+0x40]`.
5. Send `CMD_END` (`7C 81 FF 00 D7`).
6. Apply slice-reversal to produce the **logical** image —
   that's the canonical form the rest of narm decodes against.

### Writing (live)

Mirror of the read flow:

1. Wake the radio with the same 3× probe.
2. For each 32-byte page in logical order, convert to physical
   address, build the `CMD_WR` frame, send it, read the ack.
3. `CMD_END` to close.

The radio's calibration / reserved regions outside
`[0x0040, 0x8000)` aren't writable through this protocol; CPS
doesn't touch them either.

### File path

`.kg` file → `unmojibake()` → 50 000 raw bytes → … and here
the layout diverges from logical, so existing narm
`decode_channels` (which uses `.kg` offsets) and a hypothetical
`decode_channels_logical` (using the offsets above) need to
stay separate until the `.kg` ↔ logical transform is mapped.

## CHIRP references

- `kg935g.py` — closest predecessor; same framing, cipher
  algorithm, and `_write_record` shape. Different seed (`0x57`)
  and no checksum adjustment.
- `kguv9dplus.py` — sibling family; different start byte
  (`0x7D`), 4-bit cksum, but identical encrypt/decrypt
  primitives.
- `kgq10h ... 2025mar16.py` (attached to issue #10880) — the
  actual Q33x driver. Source of `convert_address`,
  `_checksum2` / `_checksum_adjust`, `_MEM_FORMAT_Q332_oem_read_nolims`.

CHIRP issues to watch upstream: `#10692` (Q10H), `#10880`
(Q332/Q336 channels-only test driver), `#11547` (Q332),
`#11765` (Q336 status).
