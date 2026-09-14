# AI_NAVIGATION (*.dat) - AINavData

Precomputed pathfinding tables for dungeon regions: per block, a cell-to-cell
edge lookup, plus a block-to-block subgoal lookup.

Layout derived from openroad's parser (`client/src/assets/ainav.rs`) and
corpus-verified below. Upstream reference: `SilkroadDoc.wiki/AINavData` —
https://github.com/DummkopfOfHachtenduden/SilkroadDoc/wiki/AINavData

## Header

| size | type | field | notes |
|---|---|---|---|
| 1 | u8 | `Version` | expected 1 |
| 4 | u32 LE | `SimpleDungeonDataOffset` | absolute offset of the second section |

## Navigation data

| size | type | field |
|---|---|---|
| 2 | u16 LE | `RegionID` |
| 4 | u32 LE | `BlockCount` |

Then `BlockCount` blocks:

| size | type | field | notes |
|---|---|---|---|
| 4 | u32 LE | `Index` | |
| 4 | u32 LE | `CellCount` | cells in this block |
| 4 | u32 LE | `EdgeCount` | edges in this block |

**Cell lookup table** — `CellCount x CellCount` entries, `goal` outer, `start`
inner. Given a start and goal cell it yields the edge to traverse:

| size | type | field | notes |
|---|---|---|---|
| 2 | i16 LE | `RefEdgeIndex0` | edge to use going start → goal |
| 2 | i16 LE | `RefEdgeIndex1` | edge to use going goal → start |

**Links** — connections into neighbouring blocks:

| size | type | field | notes |
|---|---|---|---|
| 4 | u32 LE | `LinkCount` | |
| 4 | u32 LE | `ID` | same value as `CellID` |
| 2 | u16 LE | `CellID` | |
| 2 | u16 LE | `LinkedObjID` | the other block |
| 2 | u16 LE | `LinkedObjRefEdgeIndex` | a global edge of that block |

**Block lookup table** — `BlockCount x BlockCount` entries, `goal` outer, `start`
inner, giving the cell to use as a subgoal:

| size | type | field | notes |
|---|---|---|---|
| 2 | i16 LE | `RefCellID` | the diagonal (`start == goal`) is uninitialised memory — those paths are invalid anyway, so the value must not be trusted |

| size | type | field | notes |
|---|---|---|---|
| 4 | u32 LE | `Unknown0` | expected 0 |

## SimpleDungeonData

At `SimpleDungeonDataOffset`:

| size | type | field | notes |
|---|---|---|---|
| 2 | u16 LE | `RegionID` | repeated from the first section |
| 4 | u32 LE | `BlockCount` | |

Then per block:

| size | type | field |
|---|---|---|
| 4 | u32 LE | `EdgeCount` |
| 12 x count | f32 x 3 | `EdgeCenter[]` |

| size | type | field | notes |
|---|---|---|---|
| 4 | u32 LE | `Unknown1` | expected 0 |

## Caveat: simplified meshes

Some blocks' tables are computed against a **simplified, retriangulated** version
of the navmesh rather than the original, so cell indices need not line up with the
original mesh's. The simplification algorithm is not documented and openroad does
not reproduce it.

## Corpus verification (openroad, 2026-08-12)

All **27** `Data/navmesh/AINavData_*.dat` (3.5 KB – 21.4 MB) parse under the
layout above with **zero slack — EOF-exact on every file**. `version` `{1: 27}`,
`int0` and `int1` both `{0: 27}`, and `simpleDungeonDataOffset` equals the
computed end of the RefDungeon section in 27/27 (the two sections are
contiguous). Region ids are **32768–32794 (0x8000–0x801A)**, contiguous and all
dungeon-flagged. Block counts 1–151; cells 28–17,426; edges 28–20,868; links
0–1,128, with **16 of 27 files carrying no links at all**.

Re-measured independently on 2026-08-15 with a second, from-scratch parser over
the same `Data.pk2`: every statement above reproduces exactly — 27/27 EOF-exact,
`version`/`int0`/`int1`, contiguous sections, region ids, the 3,800 links and the
17,426-cell maximum — **except the zero-link count, which is 16, not 17**. The
eleven files that do carry links carry 24, 48, 56, 58, 224, 264, 300, 300, 300,
1,098 and 1,128 of them, summing to the 3,800 the invariants below are counted
over, so the total was right and only the prose count was off by one.

Invariants proven here that this doc did not state:

- `blockCount == blockCount2` (RefDungeon vs SimpleDungeonData) in 27/27.
- Per block, **`SimpleDungeonData.EdgeCount == RefBlock.EdgeCount` in 835/835
  blocks** ⇒ SimpleDungeonData is the index-aligned 3D centroid of *every*
  RefBlock edge, not an independent edge set.
- **`link.ID == link.CellID` in 3,800/3,800 links** — the u32 `ID` is fully
  redundant with the u16 `CellID`; and `link.LinkedObjID < 0x8000` in all of them.
- The link record is exactly **10 bytes** (u32 + 3×u16), confirmed by
  EOF-exactness across all 3,800 links.

**Case trap:** two files ship as uppercase `.DAT` (`AINavData_32768.DAT`,
`AINavData_32794.DAT`) — an extension-matched loader must be case-insensitive.

## Parser + second corpus (openroad, 2026-08-12)

Parser implemented as `client/src/assets/ainav.rs` (`parse` is EOF-exact by
construction — trailing bytes are an error; loader registered for `dat`/`DAT`
because Bevy extension matching is case-sensitive). Runtime consumers: none by
design — dungeon navigation runs off `.dof` geometry + per-block `BmsNavMesh`,
with AINavData reserved as a later routing optimisation.

Validated by `tools/src/bin/dungeon_scan` against the project's own
`assets/Data.pk2` (a different corpus from the 27-file one above): **33
`navmesh/ainavdata_*.dat` files, 33/33 parse EOF-exact**, sections contiguous
33/33, `SimpleDungeonData.EdgeCount == RefBlock.EdgeCount` in **963/963**
blocks, and every embedded region id matches its filename number.
