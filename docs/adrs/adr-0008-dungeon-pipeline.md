# ADR 0008 — Dungeon pipeline: DOF-driven rendering + navigation, dungeon-local coordinates

Date: 2026-08-12
Status: Accepted

## Context

Dungeons (issue #100, epic EP-18) use a completely different map structure
than the overworld: one `JMXVDOF` file per dungeon — a list of placed room
"blocks" plus a 200×200×200-unit voxel index — instead of the 1920-unit
region tiling (`.m`/`.o2`/`.nvm`). A dungeon region id sets bit 15
(`0x8000 | dungeoninfo-id`); its low byte is the `dungeoninfo.txt` table
index, not a sector coordinate, and interiors live in a local frame reaching
±20k units. Formats are corpus-verified in `docs/formats/dof-jmxvdof.md` and
`docs/formats/ainavdata.md`.

## Decision

- **The client renders and navigates a dungeon entirely from the `.dof`**,
  matching the original client. Per-block room resources (`Block.Path`
  `.bsr`) go through the existing `bsr → bms/bmt/ddj` chain; the corpus
  probe (`tools/src/bin/dungeon_scan`) proves all 410 unique block resources
  carry a `BmsNavMesh`, so dungeon walkability reuses the already-parsed
  per-block nav meshes. **AINavData (`ainavdata_*.dat`) is loader-only**
  (`client/src/assets/ainav.rs`): it is the server/mob-AI all-pairs routing
  table, not required for client movement, and its cell indices may be
  computed on simplified meshes that don't match the `.bms` cells. It stays
  available for a later autopath/AI layer.
- **Runtime ownership**: a dedicated `client/src/plugins/dungeon/` plugin
  (spawn, portal culling via `VisibleBlockIndices`, per-block fog/lights,
  offline gates), gated on an `ActiveDungeon` resource and scene-agnostic.
  The overworld streamer (`plugins/map/`) never runs for dungeon-flagged
  regions — previously that was only an accident of the `.mfo` bitmap's
  upper half being zero.
- **Navigation shape**: dungeon walkability rides the existing object-nav
  path — block roots carry the room's `BmsNavMesh` as ordinary
  `ObjectNavMesh` objects, so stepping, wall edges, seam hand-offs between
  connected blocks and nearest-Y candidate ranking (stacked floors) come
  from the `OnObject` machinery unchanged, and `NavLocation` needs no new
  variant (entity liveness stays the validity check). What the DOF adds is
  modeled in `plugins/nav/dungeon.rs` and dispatched *inside*
  `NavMeshRaycast`: collision circles (`Flag & 2` props) veto steps, and
  the 200³ voxel grid narrows the current-block resolve. The three call
  sites (`nav/decal.rs` click path, player movement, remote ground snaps)
  stay untouched and `decal.rs` remains the single click entry. The corpus
  probe shows 34/929 connected pairs share *no* coincident outline edge, so
  the geometric link is the common case, not an invariant — hand-off fails
  soft.
- **One coordinate convention**: dungeon-local SRO space mirrors X exactly
  like the overworld (`sro = (-x, y, z)`), with **no region tiling**.
  `server_position_to_sro` branches on the region flag; blocks place at
  `mirror ∘ Translate(Position) ∘ RotY(-yaw)`.
- **World origin**: `set_dungeon_origin` anchors `O` unsnapped on the
  dungeon frame (ADR-0006 amendment) — the multiple-of-1920 invariant only
  serves the terrain splat shader, which never draws inside a dungeon.
- **Wire format**: absolute `SpawnPosition` blocks never change shape;
  movement *destination* triples switch u16 → i32 keyed by the entity's
  **current** position region (xBot/go-sro rule), implemented in
  `EntityMovement::read_with_region` and `Reader::skip_movement(region)`.
  The `dest_region > 0` presence gate is kept (the third-party sources conflict
  over whether the read is unconditional; ours stays gated until a capture settles it).

## Out of scope / follow-ups

- Networked dungeon entry end-to-end (0x705A gate use → 0x34B5 replay
  against a live server) — decode paths are dungeon-correct, but the live
  capture (CAPTURE_LIST Group L) and the `GameWorld` gate-use flow are
  follow-up. Offline gates in the `World` scene cover the teleporter AC.
- ~~Dungeon minimap floor tiles + floor readout (#72, `minimap_d`)~~ —
  implemented in the 2026-08-12 playtest round (see Consequences).
- `render_to_server_position` still derives overworld ids only; the
  dungeon-local inverse ships with the movement-request wiring.
- AINavData-based long-range autopath (optional EP-18.5).

## Consequences

- Dungeon fidelity (geometry, portal culling, per-block atmosphere,
  stacked-floor navigation) is implementable and testable fully offline —
  interiors are pure PK2 data.
- Every dungeon-facing consumer must check `region.is_dungeon()` before
  applying sector math; `RegionIdExt::to_x_z` now masks Z to 7 bits so the
  dungeon flag can never silently fold into a sector coordinate again.
- Per-block point lights and fog use documented approximations
  (D3D attenuation → Bevy `PointLight` range) until an exe pass pins the
  exact model; values stay PK2-sourced.
- *(2026-08-12 playtest round 3)* `Block.CollisionBox0` turned out to be
  **block-local** (0/151 Donwhang boxes near their block position) — round-2
  culling/resolve compared it against dungeon-frame positions, which both
  neutered the fog culling (FPS unchanged) and hid rooms the player walked
  into. All box consumers now go through the world-lifted AABBs in
  `DungeonNavData::world_box`. A periodic `dungeon perf` log line (FPS,
  shown blocks, lit lights, simulating effects, fog reach) keeps future
  playtests quantitative.
- *(2026-08-12 playtest round 3, measured)* The Donwhang 15 FPS was the
  **effect runtime**: disabling `render_effects` took it to 120. The
  effect-simulation culler paused only beyond the overworld fully-fogged
  distance (5,760 units) — the entire ~4,000-unit cave with its
  four-digit torch-flame population stayed inside it. Inside a dungeon the
  pause radius now follows the current block's fog far plane and effects in
  portal-hidden blocks pause outright (`cull_effect_simulation`).
- *(2026-08-12 playtest round)* Dense open dungeons (Donwhang cave: 151
  blocks in a 20×4×18 voxel grid with near-total visibility sets) defeat
  portal culling alone; draw load is bounded by **fog-distance block
  culling** (blocks beyond the current fog far plane hide — the fog would
  have erased them anyway) plus a **nearest-N point-light budget**
  (`graphics.dungeon.max_lights`, default 16, calibration knob). Arrivals
  snap to the nearest walkable floor via a widening downward probe
  (authored arrival Ys are unreliable and nav resolution has a 20-unit
  stand tolerance). The dungeon minimap/world map (absorbing #72) render
  from the `#section Dungeonmap` table + `minimap_d` tiles, with the
  `<group>` directory discovered from the archive layout at runtime.
