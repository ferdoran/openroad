# MapProject (*.mfo) - JMXVMFO

Which world regions exist: a 65,536-bit bitmask, one bit per region id.

Layout derived from openroad's parser (`client/src/assets/mfo.rs`,
`MFO_HEADER_LEN = 24`) and confirmed against all three corpus files below.
Upstream reference: `SilkroadDoc.wiki/JMXVMFO` —
https://github.com/DummkopfOfHachtenduden/SilkroadDoc/wiki/JMXVMFO

## Layout

| offset | size | type | field | notes |
|---|---|---|---|---|
| 0 | 12 | char[12] | `Signature` | `JMXVMFO 1000` |
| 12 | 2 | i16 LE | `MapWidth` | 256 max — the 8 x-bits of a region id |
| 14 | 2 | i16 LE | `MapHeight` | 128 max — 7 z-bits, the 8th flags a dungeon |
| 16 | 2 | i16 LE | `Short0` | UNKNOWN — 0 in all corpus files |
| 18 | 2 | i16 LE | `Short1` | UNKNOWN — 0 in all corpus files |
| 20 | 2 | i16 LE | `Short2` | UNKNOWN — 0 in all corpus files |
| 22 | 2 | i16 LE | `Short3` | UNKNOWN — 0 in all corpus files |
| 24 | 8192 | u8[8192] | `RegionData` | bit array, 65,536 region flags |

Total 8,216 bytes, EOF-exact. Every integer is little-endian, as everywhere else
in the SRO on-disk formats.

**Bit addressing** is verified from data, not spec — see the next section:
bit index = region id = `(z << 8) | x`, so `byte = id >> 3` and
`mask = 0x80 >> (id & 7)` (MSB-first within the byte).

## Corpus verification (openroad, 2026-08-12)

3 files in the user's PK2s, all `JMXVMFO 1000`, all 8,216 B, EOF-exact 3/3.

- **Bit addressing (verified, not spec):** bit index == region id == `(z << 8) | x`
  → `byte = id >> 3`, `mask = 0x80 >> (id & 7)`, i.e. **MSB-first inside the byte**
  (`go-sro-fileutils/navmesh/map_project_info.go:122`). Proof by refutation:
  MSB order yields 3,300 active regions that are a strict subset of the `.m`
  corpus with **0 exceptions**; LSB order yields 568 active regions with **no
  `.m` file at all**.
- **The upper 4,096 bytes (region ids ≥ 0x8000 — the dungeon-flag half) are zero
  in all three files.** Dungeons are *not* described by MFO; they come from
  `dungeoninfo.txt`. A dungeon-streaming branch must never consult this bitmask.
- `Short0..3` are 0 in all three files (unresolvable from data alone).
- Live file: 3,300 active regions, x ∈ 45..252, z ∈ 57..127. Active ⊆ `.m`
  files: 0 exceptions (1,349 regions have terrain data but are switched **off**).
  Active ⊆ `nv_*.nvm`: 0 exceptions. But **64 active regions have no
  `Media/minimap/{x}x{z}.ddj`** (mostly the 209..232 × 118..127 Forgotten-World
  strip). Via `textzonename.txt`: 2,521 named+active, 334 named-but-inactive
  (including all 24 dungeon ids).
- `Map/mapinfo.mfo` ≡ `Data/navmesh/mapinfo.mfo` (md5 `414b8115…`).
  `Map/_mapinfo.mfo` is a **different revision**, not a subset — 2,346 active,
  enabling 134 regions the live file does not and dropping 1,088.

