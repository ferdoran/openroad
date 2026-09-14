# Dungeon (*.dof) - JMXVDOF

A dungeon: a list of blocks (rooms/corridors) with their placed objects and
lights, a voxel lookup grid, inter-block links, room/floor labels and groups.

Layout derived from openroad's parser (`client/src/assets/dof/format.rs`), which
round-trips the whole corpus — see the verification sections below. That file is
the authoritative field-by-field reference; the tables here give the section
structure. Upstream reference: `SilkroadDoc.wiki/JMXVDOF` —
https://github.com/DummkopfOfHachtenduden/SilkroadDoc/wiki/JMXVDOF

## Header

Every offset is **absolute** from the start of the file.

| size | type | field | notes |
|---|---|---|---|
| 12 | char[12] | `Signature` | `JMXVDOF 0101` |
| 4 | u32 LE | `BlockOffset` | |
| 4 | u32 LE | `LinkOffset` | |
| 4 | u32 LE | `GridOffset` | |
| 4 | u32 LE | `GroupOffset` | |
| 4 | u32 LE | `LabelOffset` | |
| 4 | u32 LE | `Offset5` | 0 in every corpus file |
| 4 | u32 LE | `Offset6` | 0 in every corpus file |
| 4 | u32 LE | `BoundingBoxOffset` | |

## Info

Follows the header directly. Strings are a `u32` byte length plus CP949 bytes.

| size | type | field | notes |
|---|---|---|---|
| 2 | i16 LE | `Type` | the low half of the old `u32 Type` |
| 2 | i16 LE | `Category` | the high half |
| 4 + n | u32 + bytes | `Name` | |
| 4 | u32 LE | `Unknown0` | UNKNOWN |
| 4 | u32 LE | `Unknown1` | UNKNOWN |
| 2 | u16 LE | `RegionID` | |

As in `.cpd`, the single `u32 Type` documented upstream is really an `i16` pair.

## BoundingBox

At `BoundingBoxOffset`:

| size | type | field | notes |
|---|---|---|---|
| 24 | f32 x 6 | `CollisionBox0` | min/max |
| 24 | f32 x 6 | `CollisionBox1` | unused by the dungeon navmesh |

## BlockList

At `BlockOffset`: a `u32` count, then that many blocks. Each block carries

- its resource `Path` and `Name`, `Position` and `Yaw`
- `IsEntrance`
- a `CollisionBox` (as above)
- a **fog parameter set** — colour (RGBA8888), near/far plane, intensity, and an
  optional four-float height-fog tail
- `RoomIndex` and `FloorIndex`, indices into the label tables
- `ConnectedBlockIndices` and `VisibleBlockIndices`, each a counted `u32` list
- a counted list of **objects**: name, path, position, rotation, scale, a flag
  word (`2` = collision, `4` = water), a squared radius, and — when the water
  flag is set — a water colour
- a counted list of **lights**: name, position, diffuse/ambient/specular colours
  and a three-float attenuation

Two block layouts exist in the corpus; the parser detects the legacy one
(`legacy_block_layout`) rather than assuming a version.

## 3D Block Lookup Grid

At `GridOffset`: grid `Width`, `Height` and `Length`, then a voxel per cell.
Each voxel has an id and a counted list of the block indices it contains. Voxels
are **200 units** on a side (`VOXEL_SIZE`).

## Links

At `LinkOffset`: per block, a counted list of `u32` block indices.

## Labels

At `LabelOffset`: the room-name table and the floor-name table, which
`RoomIndex` and `FloorIndex` index into.

## Groups

At `GroupOffset`: a counted list of groups, each a name, a flag word and a
counted list of member block indices.

## Round-trip + corpus verification (openroad, 2026-08-11)

Layout above cross-checked against the JMX-File-Editor round-trip serializer
(checkout f8bbd96, `JMX File Editor/Silkroad/Data/JMXVDOF/*.cs`) and byte-parsed
over all 34 `.dof` files in the user's Data.pk2 (`Data/dungeon/**`) — 33/34 parse
exactly with every header-offset checkpoint hit. Corrections to the wiki text
above (all [V]):

- **Floor-label length is `u32`**, not `u16` — both room and floor names go through
  the same u32-length string reader (`Silkroad/IO/BSReader.cs:43-53`); u32 parses
  all 28 label-bearing corpus files exactly to GroupOffset. The `2 u16` line in
  the Labels section is a wiki doc error.
- **BlockObject string order is Name, then Path** (as written above) — corpus:
  first string dotless, second ends `.bsr` in 28/28 object-bearing files.
  JMX-File-Editor's `BlockObject.cs` property *names* are swapped (naming bug,
  round-trip-safe).
- `objInfo.Type` is really **i16 Type + i16 Category**
  (`Silkroad/Data/Common/ObjectGeneralInfo.cs:18-19`); corpus constant
  `(-1, 4, "Noname", -1, -1)` in 34/34.
- Conditional triggers: serializer checks `HasHeightFog == 1` and `unkByte1 == 2`
  exactly; corpus only ever shows {0,1} and {0,2} respectively.
- Strings are **CP949**-encoded (u32 length + bytes).
- `Offset5`/`Offset6` = 0 in 34/34 · `linkCount == dunBlockCnt` in 34/34 ·
  `block.unkUInt0` = 0 in all 941 blocks · `obj.Flag` ∈ {0, 2, 4} only, never
  combined · every block has ≥1 light (median 7, max 117) · `LabelOffset == 0`
  in 6 unwired files · RegionID = 0x8001–0x801A for live dungeons, 0 for
  unwired files, with id collisions (e.g. prison = flame = 0x8012) — the
  `dungeoninfo.txt` path table, not the id, is authoritative.
- **Legacy variant under the same `0101` signature**:
  `Data/dungeon/wchina/dunhwang_cv1.dof` (RegionID 0, unreferenced) stores a
  **single** flag byte per block instead of `hasHeightFog` + `unkByte1`
  (value ∈ {0, 2}; ==2 → the same Vec3×2+u32 payload) and then parses to EOF
  exactly. Blocks-end == GridOffset works as a variant-detection oracle.

## Second corpus + parser implementation (openroad, 2026-08-12)

Parser implemented (`client/src/assets/dof/format.rs`, seek-by-offset with the
blocks-end-equals-next-offset oracle for the legacy variant) and validated by
the committed probe `tools/src/bin/dungeon_scan` against the project's own
`assets/Data.pk2` — a *different* corpus from the 2026-08-11 one: **33 files,
33/33 parse exactly, all modern layout** (no `dunhwang_cv1.dof` here; this set
ships `Dunhwang_Cv_Clone.dof` instead), `dungeon/dungeoninfo.txt` has **32
enabled rows** (vs 24), plain ASCII. New findings this corpus adds:

- **`RoomIndex` is signed: `-1` (0xFFFFFFFF) = "no room label" sentinel** —
  14 blocks across `Dunhwang_Cv.dof` / `Dunhwang_Cv_Clone.dof` / `gngwc.dof` /
  `gm_event.dof`. Every other room/floor/connected/visible/voxel index is in
  range across all 941 blocks.
- **Blocks can carry 0 lights** (min 0, median 7, max 117) — the earlier
  corpus's "every block has ≥1 light" does not generalize.
- **`field_18` (`block.unkUInt1`) is a packed 0xAARRGGBB color with alpha
  always 0xFF** (e.g. `0xFF1E1E1E`, `0xFF203B1D`, `0xFF476FA5` — dark
  ambient-ish tones, 12 distinct values dominate 941 blocks). Hypothesis [S]:
  the block's ambient light color (D3D per-block ambient). Semantics still
  need an exe read to promote to [V].
- `block.IsEntrance` = 1 on 10 blocks (marked obsolete in the wiki but set in
  data) · `obj.Flag` `{0: 9600, 2: 59, 4: 53}` · `unkByte1` `{0: 816, 2: 125}`
  · height fog on 99/941 blocks · `objInfo` constant `(-1, 4, "Noname", -1,
  -1)` in 33/33 — all consistent with the first corpus.
- **Region-id header collisions confirmed again**, now as direct
  dungeoninfo↔DOF mismatches: `gngwc` (info 0x8008, header 0x8001),
  `gm_event` (0x8017 vs 0x8006), `prison` (0x8018 vs 0x8012),
  `secret_tomb_Top` (0x801B vs 0x801C). The `dungeoninfo.txt` id is
  authoritative; the header id is untrustworthy.
- **Nav-reuse assumption verified on this corpus: all 410 unique `Block.Path`
  `.bsr` resources carry a `BmsNavMesh`** (0 without, 0 unreadable).
- **Transition-edge probe**: 895/929 connected block pairs share a
  world-space-coincident outline edge (5-unit tolerance) after placing
  outlines at `Translate(Position)·RotY(-Yaw)`. The 34 pairs without one
  cluster in `demon_tower_ice.dof`, `hide.dof` and friends — a dungeon nav
  runtime must treat the geometric link as the common case, not an invariant.
- **`Block.CollisionBox0` is BLOCK-LOCAL, not dungeon-frame [V]**
  (2026-08-12, playtest round 3): across all 151 Donwhang blocks, 0 boxes
  sit near their block's position — they span ±50..±250 around the block's
  own origin (e.g. a block at position (620, 0, −240) stores box
  (−48..48, −148..140)). Every containment/distance consumer must lift the
  box through the block placement `Translate(Position)·RotY(−Yaw)` first
  (`plugins/nav/dungeon.rs::world_box`). The dungeon-level
  `CollisionBox0`/voxel-grid origin *is* dungeon-frame — only the per-block
  boxes are local.
- **Stacked-floor probe** (world-lifted box centers): 6,515 voxels hold ≥2
  candidate blocks; min pairwise center-Y separation buckets:
  `<1: 536 · 1-30: 1617 · 30-100: 1128 · 100-300: 2128 · ≥300: 1106` —
  same-floor overlaps remain common, so nearest-Y resolution must rank
  *surface* heights, not boxes, and tolerate same-height candidates.