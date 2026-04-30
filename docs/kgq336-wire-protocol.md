# KG-Q336 wire protocol

This is the live serial protocol the Wouxun CPS speaks to a
KG-Q336 over its programming cable. Sister doc to
`docs/kgq336-codeplug.md` (which covers the in-memory layout
of the `.kg` codeplug image).

The Q336 belongs to the kg935g / kguv9dplus family of Wouxun
serial protocols — same framing, same rolling-XOR cipher,
with two Q336-specific tweaks (seed byte + checksum xor).

## Cable & link layer

- **Cable**: USB-to-2-pin Kenwood "K1" plug, typically a PL2303
  or CH340 USB-serial adapter. Same physical interface as
  Baofeng/Wouxun handhelds.
- **UART**: 8N1, **115200 baud**, no flow control. (kg935g and
  kguv9dplus run at 19200; the Q332/Q336 family bumps it to
  115200 — confirmed against CHIRP's `KGQ332GRadio` class in
  `kgq10h ... 2025mar16.py`, attached to issue #10880.)
- On Linux, the kernel `pl2303` driver auto-binds to the cable.
  When passing through to a VM (VirtualBox), the host driver
  must release the device — usbmon captures still see the
  bus traffic regardless.

## Frame format

Every frame, both directions, has a 4-byte header followed by
an encrypted payload+checksum blob.

```
+------+-----+-----+-----+------------------------------+
| 0x7C | cmd | dir | len | enc(payload || cksum)        |
+------+-----+-----+-----+------------------------------+
   1B    1B    1B    1B          (len + 1) bytes
```

| Field | Value |
|---|---|
| `0x7C` | start-of-frame, fixed |
| `cmd` | command byte — see below |
| `dir` | `0xFF` host → radio, `0x00` radio → host |
| `len` | length of payload **excluding** the trailing cksum byte |

Total frame size on the wire = `5 + len` bytes
(header 4 + payload `len` + cksum 1).

### Commands

| Code | Name | Direction | Notes |
|---|---|---|---|
| `0x80` | `CMD_ID` | both | Not observed in Q336 captures — CPS uses `CMD_RD` to fetch model info instead |
| `0x81` | `CMD_END` | host → radio | Session-end. Sent **unencrypted** as the literal bytes `7C 81 FF 00 D7` |
| `0x82` | `CMD_RD` | both | Read `len` bytes from address. Payload structure below. |
| `0x83` | `CMD_WR` | host → radio | Write block. Payload structure below. |

### Cipher

Rolling-XOR with a one-byte seed. Same algorithm as
kg935g/kguv9dplus, only the seed differs:

| Driver | Seed |
|---|---|
| kg935g | `0x57` |
| kguv9dplus | `0x52` |
| **KG-Q336** | **`0x54`** |

Encrypt (host → radio, also for the cksum byte):

```python
def encrypt(plain, seed=0x54):
    out = bytearray(len(plain))
    out[0] = seed ^ plain[0]
    for i in range(1, len(plain)):
        out[i] = out[i-1] ^ plain[i]
    return bytes(out)
```

Decrypt (radio → host):

```python
def decrypt(enc, seed=0x54):
    out = bytearray(len(enc))
    out[0] = seed ^ enc[0]
    for i in range(1, len(enc)):
        out[i] = enc[i-1] ^ enc[i]
    return bytes(out)
```

Verified by hand on the first model-probe reply:

```
enc:   54 14 43 0c 59 01 54 1a 1a 1a 1a 1a 1a 1a 4b 1e ...
plain: 00 40 57 4f 55 58 55 4e 00 00 00 00 00 00 51 55 ...
       └─addr─┘  W  O  U  X  U  N                  Q  U
```

The `0x00 0x40` prefix is the address echo (CPS read addr
`0x0040` first); bytes 2..7 spell `WOUXUN` as the model probe
response.

### Checksum

Sum-mod-256 of `cmd || dir || len || plain_payload`, plus a
per-frame **adjustment** chosen by the low 2 bits of
`plain_payload[0]` (i.e. the address-high byte that every Q336
payload starts with):

| `payload[0] & 0x03` | adjustment |
|---|---|
| `0b00` | +3 |
| `0b01` | +1 |
| `0b10` | -1 |
| `0b11` | -3 |

```python
def cksum(cmd, dir_byte, length, plain_payload):
    s = (cmd + dir_byte + length + sum(plain_payload)) & 0xFF
    adj = {0: 3, 1: 1, 2: -1, 3: -3}[plain_payload[0] & 0x03]
    return (s + adj) & 0xFF
```

For an empty payload (`CMD_END` only), `payload[0]` is taken as
0, giving adjustment +3 — that's why the canonical end frame
ends in `0xD7` (`SEED ^ ((0x81 + 0xFF + 0x00 + 0x00) + 3 & 0xFF) = 0x54 ^ 0x83 = 0xD7`).

This formula is from CHIRP's `_checksum2` / `_checksum_adjust`
in `kgq10h ... 2025mar16.py` (issue #10880).

The cksum byte is appended to the plaintext payload **before**
encryption, so the encrypted blob covers `payload || cksum`.

## CMD_RD payload (both directions)

Host → radio, plaintext payload (3 bytes + 1 cksum):

```
+----------+----------+--------+--------+
| addr_hi  | addr_lo  | length | cksum  |
+----------+----------+--------+--------+
```

`length` is always `0x40` (64 bytes) in observed captures.

Radio → host, plaintext payload (`len` = `0x42`, 66 bytes
including cksum):

```
+----------+----------+----------------+--------+
| addr_hi  | addr_lo  | data * 0x40    | cksum  |
+----------+----------+----------------+--------+
```

The `addr_hi/addr_lo` bytes echo the requested address.

CPS does **not** read sequentially — it fetches selected
regions only. Observed read order (first 6 reads):

1. `0x0040` — model-info probe (`WOUXUN`, then `KG-Q336…`)
2. `0x0080`
3. `0x00C0`
4. `0x0700` ← jump
5. `0x0740`
6. `0x0780`

Empty regions (calibration, unused channels, etc.) are
skipped, which is why `02_read_full_a` only contains 500
read commands instead of the ~782 a contiguous full read of
the 50 KB image would require.

## CMD_WR payload (host → radio)

Plaintext payload (`len` = `0x22`, 34 bytes + 1 cksum):

```
+----------+----------+----------------+--------+
| addr_hi  | addr_lo  | data * 0x20    | cksum  |
+----------+----------+----------------+--------+
```

So writes are **32-byte** blocks plus a 2-byte address
prefix. (Reads are 64-byte blocks.)

Empty / unprogrammed memory is `0x00`, **not** `0xFF` —
this surfaces as an "all enc bytes equal" pattern in the
ciphertext, because rolling-XOR of `[X, 0, 0, …]` produces
`[seed^X, seed^X, seed^X, …]`. 111 of 988 write blocks in
`03_write_noop` exhibit this signature, all with their first
byte being the high nibble of the destination address.

CMD_END terminates the write session: `7C 81 FF 00 D7`,
unencrypted.

## Session shape

Observed in the `02_read_full_a` and `03_write_noop` captures:

| Phase | Role |
|---|---|
| 0–3 | CPS reads four blocks at `0x0040..0x00FF` for the model handshake (returns `WOUXUN` + `KG-Q336` text) |
| 4–N | Bulk read or write of selected regions |
| last | `CMD_END` from host |

Both sessions share the first 4 OUT commands verbatim — i.e.
the model-info probe is identical regardless of whether the
session will go on to read or write.

## USB capture notes

- usbmon emits S+C URB pairs for every transfer. Bulk OUT
  data appears in the S URB, IN data in the C URB. With the
  default tshark filter you'll see each frame **twice**;
  dedupe by content before analysis.
- USB-FS bulk transfers are capped at 64 bytes per packet,
  so each 70-byte IN frame (`4 header + 66 payload`) splits
  into two URBs. Stitch URBs that don't start with `0x7C`
  back onto the previous frame.
- The PL2303 control transfers (frames 1–~640 in our
  captures) are CDC-ACM line setup (baud, DTR/RTS) — no
  protocol data lives there.

## Reference drivers in CHIRP

The Q336 isn't merged in CHIRP yet, but two related drivers
implement the same family:

- `chirp/drivers/kg935g.py` — closest reference. Same framing
  and `_write_record` / `_read_record` shape. Cipher seed
  `0x57`; Q336 changes it to `0x54`. Checksum is the same
  `(cmd + 0xFF + len + sum(payload)) & 0xFF` formula but
  Q336 XORs the result with `0x03`.
- `chirp/drivers/kguv9dplus.py` — different start byte
  (`0x7D`), 4-bit checksum, but the encrypt/decrypt
  primitives are otherwise identical. Useful for a second
  reading of the cipher.

CHIRP issues to watch for upstream Q336 work:
`#10692` (Q10H), `#10880` (Q332/Q336 channels test),
`#11547` (Q332), `#11765` (Q336 status). Per Mel
Terechenok, the Q332 family is identical at the link layer
to the Q10H/Q10G — only the codeplug memory map differs.

## Captures on disk

In `~/Downloads/kg-re/wire/`:

| File | What it contains |
|---|---|
| `02_read_full_a.pcapng` | Full read of an FB-Radio codeplug |
| `03_write_noop.pcapng` | Read + write-back-unchanged of same |
| `*.out.tsv` / `*.in.tsv` | tshark-extracted bulk URBs (deduped on demand) |
| `analysis/decrypt.py` | End-to-end Python decoder (cipher + frame parsing) |
| `analysis/02_read_full_a.decoded.bin` | Reconstructed image after applying decrypt to all 500 reads |
