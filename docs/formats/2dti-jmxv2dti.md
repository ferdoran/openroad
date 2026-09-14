# Map2DTileIndex (tile2d.ifo) - JMXV2DTI

The terrain tile catalogue: for each tile id, its material type, texture file and
authored 3D-grass placements.

**Text, not binary** — read line by line. Layout derived from openroad's parser
(`client/src/assets/ifo/tile.rs`). Upstream reference:
`SilkroadDoc.wiki/JMXV2DTI` —
https://github.com/DummkopfOfHachtenduden/SilkroadDoc/wiki/JMXV2DTI

## Structure

| line | content |
|---|---|
| 1 | header |
| 2 | tile count |
| 3+ | one tile entry per line |

## Tile entry columns

| # | column | format | notes |
|---|---|---|---|
| 0 | `TileIndex` | zero-padded decimal | the id `.m` vertices reference |
| 1 | `TileType` | `0x%08X` | material class, see below |
| 2 | `TileCategory` | quoted string | |
| 3 | `TileFile` | quoted string | the `.ddj` texture |
| 4 | `3D-Grass` | `{model,count}` pairs | zero or more, space-separated |

**3D-Grass** pairs are `{model, count}`: the model is an index into
`object.ifo`, the count is how many are scattered randomly across the tile. A
tile may carry several pairs.

Note the original v1.188 client **parses these pairs but never renders them**
(RE-verified). openroad's `graphics.foliage.mode: native` is therefore an
enhancement built on authored-but-unused data, and `off` is the faithful setting.

## TileType

| value | name |
|---|---|
| 0 | Dirt |
| 1 | Sand |
| 2 | Ashfield |
| 3 | Stone |
| 4 | Metal |
| 5 | Wood |
| 6 | Mud |
| 7 | Water |
| 8 | DeepWater |
| 9 | Snow |
| 10 | Grass |
| 11 | LongGrass |
| 12 | Forest |
| 13 | Cloud |

## Corpus re-verification (openroad, 2026-08-12)

Re-probed against this build's `Map/tile2d.ifo` (which is byte-identical to
`Data/navmesh/tile2d.ifo`, md5 `774de5e5…`, 30,797 B, LF-only, pure ASCII):

- Signature `JMXV2DTI1001`; **count-exact — declared 603 == 603 records**, ids
  contiguous 0..602, regex-exact on all rows, no residual tail.
- **The corpus note above is stale** (719 entries / 36 grass tiles / max 5 pairs /
  tile 00689 `masin_grass_tile.ddj`). This build has **603 entries, 34 grass
  tiles, 3 multi-pair rows, max 4 pairs, max id 602 — tile 00689 does not
  exist.** The older numbers came from a different PK2 build.
- `type` histogram: Dirt 414 · Stone 66 · Grass 62 · Sand 22 · Snow 22 · Mud 6 ·
  Water 6 · LongGrass 3 · Forest 2 (types 2, 4, 5, 8, 13 unused).
- `category` (column 3, currently parsed-but-unused) is a **20-value free-text
  region tag** — `WC` 74, `East Eurpoe` 68 (typo present in the data), `OAKK` 63,
  `Arabia` 57, `Asia minor` 46, … Undocumented until now.
- `Map/tile3d.ifo` is a 1-line file containing `0` — an empty table with no
  signature.

**Adjacent loader bug worth recording here** (same `.ifo` reader):
`client/src/assets/ifo/mod.rs:73-76` reads lines via `BufReader::lines.flatten`,
which silently discards `Err` (invalid-UTF-8) lines. `Map/object.ifo` contains
**4 CP949 lines** (ids 00090 `박스.bsr`, 01535, 01536, 01537), so **4 of 2,767
objects are dropped without a warning**, and grass pairs referencing them log
"missing from object.ifo". `tile2d.ifo` itself is pure ASCII, so it is
unaffected.