# Frame codec + massive-packet (0x600D) reassembly

Every byte the client exchanges with the gateway/agent servers is wrapped in a
`SilkroadFrame`. This note documents the on-wire framing (the 6-byte header, the
`0x8000` encryption mask, blowfish padding), the inbound accumulator that drains
frames across TCP reads, and how `0x600D` massive packets are reassembled into a
single logical packet. The per-opcode payload layouts live with their types in `packets/src/**`; the
umbrella opcode↔type wiring lives in the
`packets! { ... }` macro (`packets/src/lib.rs`) and is mirrored in
[`protocol/opcodes.md`](protocol/opcodes.md).

Source of truth:

- `client/src/net/frame.rs` — `SilkroadFrame`, `parse`/`serialize`/`wire_len`.
- `client/src/plugins/net/plugin.rs` — `receive_packets`, `next_packet`,
  `FrameStep` (the accumulator + massive reassembly).
- `client/src/net/connection.rs` — `SilkroadConnection.read_buf` (the
  accumulator field).

Handshake/security internals (DH key exchange, blowfish setup, CRC/sequence
seeding) are only pointed at here — their full treatment is EP-13.2 territory.
All multi-byte integers are little-endian.

---

## 1. Frame header

The 2-byte size prefix is **always plaintext**. For an unencrypted frame
everything after it is plaintext too; for an encrypted frame the region after
the prefix is a single blowfish ciphertext block chain that must be decrypted
before the opcode/count/crc/body can be read.

Post-decrypt (or plaintext) layout:

| offset | size | field | notes |
|---|---|---|---|
| 0x00 | 2 | `size` | u16. Bit `0x8000` = encrypted flag. Low 15 bits (`0x7FFF`) = `content_len` = the plaintext **body** length (excludes these 6 header bytes and any padding). |
| 0x02 | 2 | `opcode` | u16. `0x600D` selects the massive-packet path (§4). |
| 0x04 | 1 | `count` | sequence byte, derived per frame from the handshake sequence seed (`client/src/net/sequence.rs`). |
| 0x05 | 1 | `crc` | frame CRC over the header+body (`client/src/net/crc.rs`). |
| 0x06 | `content_len` | `data` | payload. For `0x600D` the first byte is the massive flag (§4). |

`parse` reads `size` at `[0..2]`, then works relative to `data[2..]`: `opcode`
at `[2..4]`, `count` at `[4]`, `crc` at `[5]`, body from `[6..]`.

> The smallest legal frame is 6 bytes (header only, `content_len == 0`). The
> accumulator treats a `wire_len` below 6 as a desynchronised stream (§3).

---

## 2. Size field & encryption mask

```
size = content_len (0..=0x7FFF)  |  (encrypted ? 0x8000 : 0)
```

- **`content_len` = body length only.** `serialize` writes `size = data.len()`
  (the body), not the header-inclusive length. `parse` reconstructs the region
  after the prefix as `content_len + 4` (opcode 2 + count 1 + crc 1).
- **Bit `0x8000` = encrypted.** When set, the `content_len + 4` region is
  blowfish-encrypted and padded (§3). When clear, that region is plaintext.

Handshake and keep-alive frames the client sends are unencrypted
(`encrypted = 0`); the module-identification frame (`0x2001`) and normal
post-handshake traffic are encrypted once security is `Established`.

### The bit is recorded in `packet_dump/` (#459)

Since **2026-08-14** an inbound dump line is `<RFC3339-ms UTC> <hex payload> <E|P>`:
`E` = the frame carried the `0x8000` bit, `P` = it did not. `next_packet` reads
it off the always-plaintext size prefix before `parse` decrypts the buffer in
place (`SilkroadFrame::wire_is_encrypted`), because the parsed `MassiveHeader`
does not keep it; for a massive packet the recorded bit is the header frame's.

Without it a length read out of an old log is ambiguous — an encrypted body is
block-padded (§3), so 12 of the 58 captured opcodes (`0x3016 0x3027 0x303d
0x304d 0x3054 0x30d0 0x30d2 0x30d7 0x3153 0x34be 0xb023 0xb0bd`) could not be
decided from length alone.

**Format break, backward-compatible in the way that matters:** the flag is a
suffix, so `cut -d' ' -f2 f.log | xxd -r -p` still yields the payload. Lines
written before that date have **no third column**; their encryption state is
UNKNOWN and must not be assumed `P`. `c2s/` lines are always `P` because sent
payloads are captured *before* serialization — the column describes the bytes
in the log, not what went on the wire.

---

## 3. Encrypted frames, blowfish padding, and `wire_len`

Blowfish operates on 8-byte blocks, so an encrypted frame pads the
`content_len + 4` region up to the next multiple of 8. `wire_len` computes the
full on-wire length without decrypting:

```
encrypted:   wire_len = 2 + ((content_len + 4 + 7) & !7)      // pad up to 8
unencrypted: wire_len = 2 + (content_len + 4)                 // 6 + content_len
```

Worked padding (encrypted region = `content_len + 4`, padded to a multiple of 8):

| content_len | region (`+4`) | padded | pad bytes | wire_len |
|---|---|---|---|---|
| 3 | 7 | 8 | 1 | 10 |
| 4 | 8 | 8 | 0 | 10 |
| 12 | 16 | 16 | 0 | 18 |
| 13 | 17 | 24 | 7 | 26 |

The padding bytes sit after the body inside the ciphertext. The original trims
them — it reads exactly `size` body bytes.

> **`parse` now trims them (#130/#449, fixed).** Both the `Packet` arm and the
> massive arm used to slice `&data_without_size[4..]`, i.e. `total_size - 4`
> bytes, with `content_len` used only to compute `total_size` and never bounding
> the body. An encrypted frame therefore dispatched `content_len + pad` bytes —
> 1-7 bytes of blowfish padding leaked into the body handed to
> `Packet::deserialize` **and to `packet_dump`** for 7 of every 8 frames (only
> `content_len ≡ 4 (mod 8)` was clean), and for a **massive** sub-frame each
> chunk spliced its own padding into the *middle* of the reassembled payload.
> `parse` now binds the body once as `data_without_size[4..4 + content_len]`
> (`Incomplete` if the buffer is short), matching the original's "read exactly
> `size` body bytes".
>
> **Any pre-fix "N trailing unknown bytes" claim about an *encrypted* opcode is
> void** — up to 7 of those bytes were blowfish padding this codec invented, not
> wire data. Re-verified against `packet_dump/` at the fix: `0x2113` dumps 1028
> bytes for a 1026-byte body and `0xA102` 28 for a 23-byte body (`01 45 00 00 00
> | 0e 00 "198.51.100.200" | 0c 3e`, then five padding zeros). Frames whose
> dumped length is **not** `≡ 4 (mod 8)` (e.g. `0xA101` 42, `0xA106` 6) were
> never padded and their dumps stand byte-exact.

> **`parse` returns a decrypt-relative size, not the wire length.** For a
> decrypted frame `parse` reports `total_size - 4` (and `total_size` for a
> plaintext one) — an artefact of decrypting in place. The receive loop must
> **not** use that return value to advance the buffer; it uses `wire_len`
> instead, which is why `next_packet` discards the size `parse` returns
> (`Ok((_, frame))`). `wire_len` returns `None` when fewer than 2 bytes are
> buffered (the size prefix has not fully arrived yet).

`SilkroadFrame` decrypts **in place**: `parse` takes `&mut [u8]`, so the caller
passes exactly `&mut buf[..wire_len]` and copies the body out afterwards.

---

## 4. Frame kinds

`parse` yields one of three `SilkroadFrame` variants:

| variant | when | carries |
|---|---|---|
| `Packet` | opcode ≠ `0x600D` | `opcode`, `count`, `crc`, `encrypted`, `data` (body) |
| `MassiveHeader` | opcode = `0x600D`, body flag = 1 | `count`, `crc`, inner `opcode`, `amount`, trailing `data` |
| `MassivePayload` | opcode = `0x600D`, body flag = 0 | `count`, `crc`, `inner` (body chunk) |

A body flag other than 0/1 on a `0x600D` frame is a parse error
(`InvalidMassiveHeaderFlag`).

### 4.1 `0x600D` massive packets

A logical packet too large (or batched) for one frame is split across a header
frame (flag 1) followed by `amount` payload frames (flag 0). Each is a normal
`0x600D` `Packet` on the wire; the first **body** byte is the massive flag.

**Header frame body** (opcode `0x600D`, flag = 1):

| offset (in body) | size | field | notes |
|---|---|---|---|
| 0x00 | 1 | `flag` | `= 1` |
| 0x01 | 2 | `amount` | number of payload frames that follow |
| 0x03 | 2 | inner `opcode` | the real opcode of the reassembled packet |
| 0x05 | .. | trailing | present in the struct as `data`; **not** used for reassembly |

**Payload frame body** (opcode `0x600D`, flag = 0):

| offset (in body) | size | field | notes |
|---|---|---|---|
| 0x00 | 1 | `flag` | `= 0` |
| 0x01 | .. | `inner` | one chunk of the logical body |

**Reassembly** (`next_packet`, `MassiveHeader` arm): on a header, read forward
over the next `amount` frames, require each to parse as a `MassivePayload`, and
concatenate their `inner` bytes in order into the logical body. The result is
dispatched as `opcode = <inner opcode>`, `data = <concatenated inner>`,
`consumed = <bytes of header + all payloads>`. The header's own trailing `data`
is intentionally ignored — the body lives entirely in the payload frames.

**Incomplete / corrupt massive handling:**

- If the header parses but not all `amount` payloads have arrived yet,
  `next_packet` returns `Incomplete`; the **header stays buffered** and the whole
  group is retried on the next read.
- If a following frame is missing its length prefix or its bytes haven't all
  arrived, that too is `Incomplete`.
- If a following frame parses as anything other than a `MassivePayload`, the
  group is `Corrupt`.
- A `0x600D` frame whose body is too short to hold the flag byte (or, for a
  header, the `amount`+opcode dwords) is a `TruncatedMassiveFrame` parse error →
  `Corrupt` [V] `frame.rs:181-206`. **Before #130 those three shapes panicked**
  inside `bytes` (`Buf::get_*` aborts on underflow) rather than erroring, so a
  desynchronised stream that happened to read as `0x600D` with a short body took
  the whole process down instead of resynchronising — `frame_len >= 6`
  [V] `plugin.rs:147-149` is the only structural pre-filter and all three shapes
  clear it. Regression: `truncated_massive_frames_are_corrupt_not_a_panic`.

**Divergence from the original's reassembly (`drift`, undocumented until #130).**
The original is a **stateful counter** — a per-connection `m_massive_count` that
decrements as chunks arrive, so unrelated frames may
be interleaved between chunks. Ours is a **contiguity-requiring look-ahead**
[V] `plugin.rs:174-195` holding no state on the connection (`connection.rs:19-34`
has only `read_buf`). Consequences, all verified by reading the code:

| case | ours | original |
|---|---|---|
| normal frame interleaved between chunks | `Corrupt` → whole `read_buf` dropped | tolerated |
| second header mid-sequence | `Corrupt` | tolerated |
| two massive groups interleaved | unsupported | tolerated |
| stray payload, no header | `Skip`, silently discarded, never dumped [V] `plugin.rs:202-206` | counter guards it |
| more chunks than `amount` | extras `Skip`ped, packet dispatched **truncated**, no warning | counter guards it |
| `amount == 0` | dispatches an **empty** packet under the inner opcode [V] `plugin.rs:177,196-200` | — |

Whether real servers ever interleave is **UNKNOWN-3** — it decides whether this
is cosmetic or a live desync. Also note the retry is not free: on `Incomplete`
the header and every already-seen payload are re-parsed from scratch next tick
[V] `plugin.rs:154,177-195`, which is O(amount²) and, on an encrypted stream,
**re-decrypts already-decrypted bytes in place** [V] `frame.rs:168-171` — the
group then stops parsing as `0x600D` and is dispatched as garbage. Gated on the
same UNKNOWN-2 as above. With `amount: u16` and no inbound size cap, `read_buf`
can also be driven to ~2 GB before a group completes.

---

## 5. The inbound accumulator (post-#145)

`SilkroadConnection.read_buf: Vec<u8>` (initial capacity `4096 * 4`) is a
persistent byte accumulator. `receive_packets` (PreUpdate) reads up to 4096
bytes into a stack `scratch`, appends them to `read_buf`, then drains complete
frames off the front:

```
loop {
    match next_packet(&mut read_buf[consumed..], &security) {
        Ready { opcode, data, consumed: n } => { dump; dispatch; consumed += n }
        Skip  { consumed: n }               => { consumed += n }
        Incomplete                          => break            // keep the tail
        Corrupt                             => { drop rest; break }
    }
}
read_buf.drain(..consumed);   // the unparsed tail survives to the next read
```

`FrameStep` (the outcome of one drain step):

| variant | meaning | loop action |
|---|---|---|
| `Ready { opcode, data, consumed }` | a complete `Packet`, or a reassembled massive packet | dump + `Packet::deserialize` + dispatch; advance |
| `Skip { consumed }` | a frame that carries nothing to dispatch (a stray `MassivePayload`, or a future frame kind) | advance past it |
| `Incomplete` | not enough bytes buffered for the next frame | stop; keep the tail for the next read |
| `Corrupt` | impossible length prefix or a frame failed to parse | drop all remaining buffered bytes and stop |

`next_packet` steps, in order:

1. `wire_len(buf)` is `None` (fewer than 2 bytes) → `Incomplete`.
2. `wire_len < 6` (shorter than a header) → `Corrupt`.
3. `wire_len > buf.len()` (partial tail) → `Incomplete`.
4. `parse` errors → `Corrupt`.
5. `Packet` → `Ready`; `MassiveHeader` → reassemble (§4.1); stray
   `MassivePayload`/other → `Skip`.

### Why the old fixed 4 KB buffer dropped split frames

Before #145, `read_buf` was a fixed `[u8; 4096]` array read into directly, and
the drain loop parsed only within the current read: when a frame's tail had not
arrived (`offset + frame_len > read_bytes`) the loop just `break`ed and the
partial head was **discarded**. The next read overwrote the array from offset 0,
so the stream resumed mid-frame — corrupting the split frame *and every frame
after it* in that burst. This bites because the server packs the post-join burst
(character-data begin/body/end, celestial position, …) back-to-back, and a
single 4 KB read routinely ends mid-frame. The accumulator fixes this by keeping
the unparsed tail in `read_buf` across reads (verified by the
`frame_split_across_reads_is_not_dropped`,
`split_between_frames_keeps_the_second`, and
`massive_packet_reassembled_across_reads` tests in `plugin.rs`).

---

## 6. Handshake & security (pointers only)

Framing above assumes security is already set up. The handshake runs
synchronously in `SilkroadConnection::connect` before the socket goes
non-blocking (`client/src/net/connection.rs`), exchanging unencrypted `0x5000`
frames and finishing with `0x9000`:

| file | responsibility |
|---|---|
| `client/src/net/handshake.rs` | `0x5000` setup → challenge → `0x9000` finalize; Diffie-Hellman (`g_pow_x_mod_p`), challenge derivation, final blowfish key |
| `client/src/net/security.rs` | `SilkroadSecurityState` (`None`→`Initialized`→`Established`) + the `SilkroadSecurityData` context (blowfish, sequence, CRC, DH values) |
| `client/src/net/blowfish.rs` | the blowfish cipher used for frame en/decryption |
| `client/src/net/crc.rs` | per-frame CRC (byte at header offset 0x05), seeded from the handshake `crc_seed` |
| `client/src/net/sequence.rs` | per-frame `count` byte (header offset 0x04), seeded from the handshake `sequence_seed` |

The encoding options advertised in the `0x5000` setup packet
(`encryption` / `edc` / `key_exchange` / `key_challenge`) are decoded by
`SilkroadEncodingOptions` (`client/src/net/codec.rs`). After the client sends
`0x9000`, it sends the module-identification frame (`0x2001`, encrypted) and
expects the server's `0x2001` reply. `parse` only decrypts when the state is
`Established`, so all pre-handshake frames take the plaintext path.

---

## 7. Illustrative frames

Constructed (synthetic) frames — **not** live captures; no real capture bytes
are reproduced here. They mirror the byte fixtures in the `plugin.rs` tests.

**Unencrypted `Packet`** — opcode `0x3013`, 3-byte body `01 02 03`:

```
03 00  13 30  00  00  01 02 03
└─size └─op   cnt crc └─body
size = 0x0003 (content_len 3, not encrypted) → wire_len = 6 + 3 = 9
```

**Massive group** — logical opcode `0x34A5`, body `01 02 03 04 05` in 2 payloads:

```
header : 05 00  0D 60  00 00  01  02 00  A5 34
                └600D         f=1 amt=2  op=0x34A5
payload1: 04 00  0D 60  00 00  00  01 02 03
                └600D         f=0 └chunk
payload2: 03 00  0D 60  00 00  00  04 05
                └600D         f=0 └chunk

reassembled → opcode 0x34A5, body 01 02 03 04 05
```

---
