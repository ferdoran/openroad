# MapEnvironment (*.ifo) - JMXVENVI

Colour grading and post-processing over the course of the day: a set of profiles,
each holding time-keyed colour and scalar curves, plus a tree assigning profiles
to environments.

Layout derived from openroad's parser (`client/src/assets/ifo/environment.rs`).
Upstream reference: `SilkroadDoc.wiki/JMXVENVI` —
https://github.com/DummkopfOfHachtenduden/SilkroadDoc/wiki/JMXVENVI

## Layout

Strings are a `u32` byte length followed by the bytes.

| size | type | field | notes |
|---|---|---|---|
| 12 | char[12] | `Signature` | `JMXVENVI1003` |
| 2 | i16 LE | `ProfileCount` | |
| 4 + n | u32 + bytes | `EnvironmentSetName` | empty in the corpus |

Then `ProfileCount` profiles, then one `Environment` root node.

## Profile

| size | type | field | notes |
|---|---|---|---|
| 2 | u16 LE | `Id` | |
| 4 + n | u32 + bytes | `Name` | |
| 4 + n | u32 + bytes | `DayBGM` | obsolete — superseded by `regioninfo.txt` / `effectenvsnd.txt` |
| 4 + n | u32 + bytes | `NightBGM` | obsolete, as above |

Then sixteen curves, in this order. `Color` is a `ColorGraph`, `Float` a
`FloatGraph` (both below):

| # | kind | field | notes |
|---|---|---|---|
| 0 | Color | `SunColor` | |
| 1 | Color | `SkyTopColor` | |
| 2 | Color | `DiffuseColor` | |
| 3 | Color | `ObjectAmbientColor` | |
| 4 | Color | `Graph4` | UNKNOWN |
| 5 | Color | `TerrainAmbientColor` | |
| 6 | Color | `TerrainShadowColor` | added in 1003 |
| 7 | Float | `FogNearPlane` | |
| 8 | Float | `FogFarPlane` | |
| 9 | Color | `FogColor` | |
| 10 | Float | `Graph10` | UNKNOWN |
| 11 | Float | `Graph11` | UNKNOWN |
| 12 | Float | `Graph12` | UNKNOWN — added in 1001 |
| 13 | Color | `SkyBottomColor` | added in 1002 |
| 14 | Color | `WaterColor` | added in 1002 |
| 15 | Float | `Graph15` | UNKNOWN — added in 1002 |

## ColorGraph

| size | type | field | notes |
|---|---|---|---|
| 4 | i32 LE | `KeyCount` | |
| 16 x count | f32 x 4 | `Keys` | xyz = RGB, w = time of day |

## FloatGraph

| size | type | field | notes |
|---|---|---|---|
| 4 | i32 LE | `KeyCount` | |
| 8 x count | f32 x 2 | `Keys` | x = value, y = time of day |

## Environment (tree node)

The root node closes the file. The client walks it as three nested levels.

| size | type | field | notes |
|---|---|---|---|
| 4 | u32 LE | `ChildCount` | |
| 4 + n | u32 + bytes | `Name` | |
| 2 | u16 LE | `ProfileId` | |
| 2 | u16 LE | `Short0` | equals the node's tree depth in all 78 corpus nodes |
| 4 | i32 LE | `Int0` | UNKNOWN |
| 4 | i32 LE | `Int1` | UNKNOWN |
| — | Environment[] | `Children` | `ChildCount` nested nodes |

## How the client consumes this

Parsed by `client/src/assets/ifo/environment.rs` (`JMXVENVI`), loaded from `map://environment.ifo`
into `MapsAssets.environment_info`, and applied at runtime by `client/src/plugins/environment/`.
Graph time keys span one day cycle (0.0 = midnight, 0.5 = noon); sampling lerps between keys and
wraps across midnight. The active profile follows the camera via the per-block environment id in
the `.m` terrain files (`TerrainBlock::environment_id`), overridable at runtime. The graphs drive
both lighting models — `EnvironmentSettings.enabled` only switches the ambient-brightness scale
(SRO-faithful vs the PBR baseline), not whether the graphs apply.

| Field | Drives |
| --- | --- |
| DiffuseColor | `DirectionalLight` (sun) color, and the fog's directional-light tint |
| SunColor | Sun disc billboard tint (`plugins/environment/celestial.rs`) |
| ObjectAmbientColor | `GlobalAmbientLight` color |
| TerrainAmbientColor | Terrain-specific ambient, applied as a ratio to the object ambient via an `ambient_ratio` uniform in `terrain_splat.wgsl` |
| FogColor | `DistanceFog` color |
| FogNearPlane / FogFarPlane | `DistanceFog` falloff. All FloatGraphs in this format are normalized to [-1, 1]; the fog planes are fractions of a view range (long clear noons, short fogged nights), mapped in `envi_fog_range`, stretched by `EnvironmentSettings::fog_distance_scale` (default 1.5 — the raw mapping reads short vs the original client), with the terrain streaming end as a hard ceiling |
| SkyTopColor / SkyBottomColor | Sky gradient (zenith/horizon) in `SkyGradientMaterial`; the bottom color also feeds the HQ water reflection tint |
| WaterColor | HQ water material base color |
| Graph4 → CloudColor *(inferred)* | Cloud layer tint — white at noon, warm at dawn/dusk, dark navy at night, near-black in dungeon profiles |
| Graph10 / Graph11 → CloudNearAlpha / CloudFarAlpha *(inferred)* | Per-layer cloud opacity: Graph10 (authoring default 0.5) varies with daylight — the cloud1.ddj layer; Graph11 (default 0.9) hovers near 1.0 — the cloud99.ddj layer |
| Graph15 → NightIntensity *(inferred)* | +1 at midnight, -1 at noon in every outdoor profile — star/night-sky factor. Drives the procedural star field in `sky_gradient.wgsl` (the archives ship no star texture) |
| TerrainShadowColor | Unused — would tint terrain in shadow; needs terrain shadow rendering to be meaningful |
| Graph12 | Still unknown: oscillates between -1 and 1 with many keys, no day-cycle correlation (gentle in towns, wild in some wilderness profiles) — possibly wind / cloud-scroll modulation |

Field names marked *(inferred)* have no official/community documentation (SilkroadDoc and JMX-File-Editor both leave them numbered); they were identified empirically from the graph shapes across all 60 profiles of a v1.188 client (see the `environment_graphs.txt` startup dump).
## Corpus verification (openroad, 2026-08-12)

JMXVENVI ships as exactly **one** file — `Map/environment.ifo`, 66,472 B,
`JMXVENVI1003` — and the layout above parses it **EOF-exact** (ends at 0x103a8,
0 trailing bytes). No `.envi` files exist. Note that **JMX-File-Editor has no
JMXVENVI support at all** (0 grep hits), so any citation of it for the unnamed
graphs is unsupported.

- 60 profiles, ids 0..59 all distinct; set name `""`; Day/NightBGM `""` in 60/60
  (the "obsolete" note holds).
- **The [-1,1] / [0,1] normalization claim is now empirical**: every FloatGraph
  value lies inside [−1,1], every ColorGraph channel inside [0,1], and every time
  key inside [0,1], across all 960 graphs.
- Key counts per graph range 2..23 (ObjectAmbientColor is the busiest); modal 5
  for colours, 3 for TerrainShadow/CloudFarAlpha, 6 for FogNearPlane, 9 for the
  inferred NightIntensity. `TerrainShadowColor` spans only 0…0.278 — always a
  dark tint.
- `Environment` tree: **78 nodes, depths {0:1, 1:17, 2:60}**, byte-exact to EOF.
  `Short0` and `Int0` both equal the node depth in all 78 nodes — redundant
  tree-level tags. Every profile id 0..59 is referenced **exactly once** by a
  leaf, with no dangling ids in either direction, so the tree is **editor-only
  grouping**: the runtime path (`.m` block `environment_id` → `profiles[]`)
  loses nothing by ignoring it.
- `Int1` equals depth for 68 nodes but **depth+1 for exactly 10 named leaves**
  (Jangan, Donwhang village, and 8 more) — semantics UNKNOWN.

**Fixed in #292:** the loader read `ProfileId` as an `i32`, swallowing `Short0`
into the high word — since `Short0 == depth`, every depth-2 leaf decoded as
`real_id | 0x20000` (131,072…131,131 instead of 0…59) and every depth-1 node as
`real_id | 0x10000`. It was harmless only because `environment_root` has no
consumers outside the loader (the runtime looks profiles up directly via the
`.m` block's `environment_id`). Now `u16 profile_id` + `u16 short0`, and every
read in the file is explicitly little-endian.

The `Environment` tree itself is **editor-only grouping**: `Short0` and `Int0`
are both just the tree depth, and every profile id 0..59 is referenced exactly
once with no dangling ids in either direction — so ignoring the tree loses
nothing. `Int1` is the one real unknown (depth for 68 nodes, depth+1 for 10
named leaves).