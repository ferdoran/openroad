# Nav Mesh (*.nvm) - JMXVNVM

The per-region navigation mesh: placed collision objects, walkable cell quads,
the edges between them, and the tile/height/plane maps.

Layout derived from openroad's parser (`client/src/assets/nvm/`, one module per
section — `map_object.rs`, `nav_cell_quad.rs`, `nav_edge_global.rs`,
`nav_edge_internal.rs`, `tile_map.rs`, `height_map.rs`, `plane_type_map.rs`,
`plane_height_map.rs`). Those modules are the authoritative field-by-field
reference; the tables here give the section structure. Upstream reference:
`SilkroadDoc.wiki/JMXVNVM` —
https://github.com/DummkopfOfHachtenduden/SilkroadDoc/wiki/JMXVNVM

Sections appear in the order below, each prefixed by its own count.

## Signature

| size | type | field | notes |
|---|---|---|---|
| 12 | char[12] | `Signature` | `JMXVNVM 1000` |

## ObjectList

A `u16` count, then that many placed object instances. Each carries the object
id, its position, a collision/flag word, its yaw, a per-region unique id and the
region id it belongs to — the same instance shape `.o2` uses, so one object
spanning several regions appears in each.

## RTNavMeshCellQuad

A `u32` count, then that many axis-aligned quads. Each has its min/max corner in
region-local space and the index of the object it belongs to. **Cell openness is
the authoritative walkability signal** — see the section further down.

## RTNavMeshEdgeGlobal

Edges on the region boundary, which hand pathing over to the neighbouring
region: a count, then per edge its two endpoints, a flag word
(`nav_mesh_edge_flag.rs`), the cells on each side and the neighbouring region.

## RTNavMeshEdgeInternal

Edges wholly inside the region: the same shape without the neighbour-region
field.

## TileMap

The per-tile terrain classification grid for the region.

## HeightMap

The per-vertex terrain height grid.

## PlaneMap

Two parallel grids: `PlaneType` (`plane_type.rs` — including `Water`, `Ice` and
`WaterIce`) and `PlaneHeight` (`plane_height_map.rs`), the height of that plane
where one exists.

### How the client uses it

Only the **ice** sheet is walkable, and it is the surface an actor stands *on*.
`JMXVNVM::walkable_height_at` returns, over an `Ice`/`WaterIce` cell,
`max(terrain, planeHeight)` — the ice plane where the lake bed lies below it,
the terrain where land pokes up through the ice (a shore). Ice cells are also
walkable regardless of the terrain cell openness underneath (`is_ice_at`), so a
frozen lake can be crossed even where its bed is deep, closed water. Plain
`Water` is **not** lifted — it is not a standable surface, so a mover over open
water still resolves to the terrain (lake bed) height.

Measured example — the Karakoram region (`nv_5c81`): all 36 cells are `Ice`/
`WaterIce` at a uniform `planeHeight` of 800, over a lake bed ranging 516–1173.
Where the bed is below 800 the actor should stand on the ice at 800; before this
was honoured, it walked the bed *underneath* the rendered ice.
## Cell openness is the authoritative walkability signal

`RTNavMeshCellQuad` stores cells **open-first**: the first `openCellCount`
entries are walkable and everything after them is solid. Measured over v1.188
(regions 167x97, 167x98, 166x97), cross-checking every tile's `CellID` against
its own blocked flag:

| | blocked tiles | free tiles |
|---|---|---|
| in open cells | **0** | 8946 / 8998 / 8990 |
| in closed cells | **270 / 218 / 226** | **0** |

Perfect agreement in all three regions, so `cellIndex >= openCellCount` is
exactly "solid ground".

This matters because the blocked edge lists are *not* a sufficient substitute.
They bound solid regions, but their coverage cannot be relied on to fence every
approach — a mover that slips past one walks into rock, which is how this client
let players walk into the Jangan west wall and get stuck inside it. Movement
must test the cell a step lands in, not only the edges it crosses.

## Terrain and object blocking hand over at gates, and can leave a gap

Region 167x97's blocked-tile map shows Jangan's west wall as a clean north-south
line at tile x 8-14 that simply **stops at tile z=83**. The gate's object hull
(`cj_w_stair.bms`) picks up around tile z 88-90. Between them, tiles z 84-87
(~100 units of visible masonry) are blocked by neither system, and a mover walks
straight through.

The hull's long wall face also ends at region-local x = 304.5, so a mover at
x = 307 slips past its east end without crossing anything.

Whether that gap is authored or whether the original client draws blocking from
somewhere this client does not read is **unresolved**. Worth checking before
assuming a movement bug — the objects there are flagged `IsStruct`, which per the
ObjectList docs "requires an objectstring.ifo entry with additional info", and
that file is not parsed by this client at all.

## Object collision hulls cover less than you'd expect

Blocking for a map object comes from the nav mesh sections of its `.bms` meshes.
`.bsr` also declares a collision mesh explicitly (`header.CollisionOffset`), and
this client ignores that field — harmlessly, as it turns out: across the 1164
`res/bldg/**` resources, the declared collision mesh is *also* one of the render
meshes in all 1007 cases that have one, and 999 of those carry a nav section, so
it is already loaded and attached.

What the collision hull does **not** do is cover the whole object. Jangan's west
gate (`res/bldg/china/jangan_enter/cj_w.bsr`, 14 meshes of which exactly one has
a nav section) resolves to `prim/mesh/bldg/china/jangan_enter/cj_w_stair.bms`.
That mesh spans 327 x 4376 units — the entire wall run, longer than a region —
but its *ground-level* blocked outline exists only in a ~650-unit band around the
gate building itself. Some 2000 units of wall either side carry no blocked edge
near the floor at all.

So a long wall is blocked by the **terrain** nav mesh along its run, and by the
**object** hull only where a gate or stair interrupts it. Movement has to consult
both, and a location where neither covers the wall is a data seam rather than a
client bug — worth confirming with the `KeyU` nav snapshot
(`plugins/dev/navmesh_lines.rs`) before hunting for one.

## EdgeFlag

Shared by the terrain edges here and the outline/inline edges of an object's
`.bms` nav mesh (`client/src/assets/nvm/nav_mesh_edge_flag.rs`).

```
None         = 0
BlockDst2Src = 1
BlockSrc2Dst = 2
Blocked      = 3    // BlockDst2Src | BlockSrc2Dst
Internal     = 4
Global       = 8
Underpass    = 16   // actor passthrough from outside, blocked from inside
Entrance     = 32   // dungeon (obsolete?)
Bit6         = 64
Siege        = 128  // fortress war: attack passthrough
```

Note bit 4 is **Underpass**, not "Bridge" — it was misnamed in this client until
2026-07-20.

### The block bits are directional, but acting on that is a mistake

The two block bits name a direction against the edge's own `src`→`dst` sense.
Measured over the whole v1.188 corpus, honouring that direction gains nothing
and is actively dangerous:

| | count |
|---|---|
| terrain internal edges, `BlockSrc2Dst` only | 1,586,920 |
| terrain internal edges, `BlockDst2Src` only | 0 |
| terrain internal edges, blocked both ways | 0 |
| ...of the blocked ones, with `AssocCell[1] == -1` | **all 1,586,920** |
| terrain global edges, blocked at all | 0 |
| object outline edges, one-way | 0 (all 71,395 blocked ones block both ways) |
| object inline edges, one-way | 26 of ~209,000 |

A blocked terrain edge separates a walkable cell (`AssocCell[0]`) from **void**
(`AssocCell[1] == -1`). There is no far side an actor could stand on, so the
mover is always on the `src` side and the directional answer is by construction
identical to the non-directional one. Unblocked edges are the ones with both
cells valid.

So the directional reading is a no-op when correct — and when the polarity is
inverted it turns every wall in the world into a one-way membrane you can enter
but not leave. This client therefore blocks on the plain
`Blocked = BlockDst2Src | BlockSrc2Dst` test.

### Global (bit 3) is a dungeon mechanism, not the terrain hand-off

Measured over every `.bms` in Data.pk2 (16,853 meshes, 2,103 carrying a nav
section), the `Global` bit is **rare and almost entirely underground**:

| | count |
|---|---|
| nav meshes with at least one global edge | **390 of 2,103** |
| global *outline* edges (flags 8, 24, 136) | 1,614 of 75,847 |
| global *inline* edges | **0** |

Outline flag byte distribution: `3` (blocked) 66,119 · `131` (Siege+blocked)
5,276 · `0` 2,164 · `8` (global, passable) 1,602 · `16` (underpass) 566 · `128`
54 · `144` 52 · `24` 8 · `136` 4 · `32` 2. Inline is almost entirely `4`
(Internal) at 206,043, and carries no global bit at all — `Global` is an
outline-only concept.

The meshes that have them are overwhelmingly `prim/mesh/dun/**` — the Jinsi and
Donhwang cave floors, the Flame passages, the Wreck bridges — plus a handful of
guild fortress parts. **Ordinary outdoor bridges have none**: `oas_hot_bridge`,
`cj2_brg_floor`, `oas_hot_brg_m01`, `w_cd_brid02`,
`euro_esteuro_w01_bridge01` all measure 0. The exceptions are indoor-ish
structures like `cj_pal_south_brid03_floor02` (2) and `cj_jin_brg_02` (4).

So `Global` cannot be the signal that hands a mover between an object and the
terrain: the objects that most need that hand-off do not carry it. It reads as
a dungeon/room-linkage mechanism, which matches `LinkEdge` joining one object's
global edge to *another object's* — object↔object, never object↔terrain.

That leaves `NavCellQuad`'s object index list (`objIndex[]`, parsed into
`ObjectIndices` and currently unused) as the only thing in the format that
relates a terrain cell to the object standing on it.

### Underpass (bit 4) is the fall-off boundary of an elevated surface

**Corrected 2026-07-21.** An earlier revision of this note called `Underpass` a
non-blocking flag and said honouring it trapped players inside buildings — that
was a misdiagnosis. Re-measuring the corpus tells a consistent story:

- All 626 `Underpass` outline edges (flag bytes `16` 566, `24` 8, `144` 52)
  carry **no block bits**, and every one is a **one-sided boundary**
  (`dst_cell = NO_CELL`) — the edge of the walkable area, never a two-sided
  interior edge.
- They are rare — **38 of 2,103** nav meshes — and the carriers are all one
  kind of thing: **bridges** (`ruin_takla_tembrig_01` 114 edges,
  `rock_mt_bridge_01` 99, `w_earthgst_brid_floor` 51, the Tarim/Oasis/Dunhuang
  spans), **stair sides** (`cj_w_stair`, `cj_s_stair`, `cj_e_stair`), and
  **wall tops / balconies** (`euro_const_mili_wall`,
  `asiaminor_theater02_brokenwall`, the guild-building floors). **None is a
  doorway.**

So the wiki's "passthrough from outside, blocked from inside" is literally
right: the edge is the railing of an elevated walkable surface. Standing on the
surface, you are *inside* its outline (the whole walkable area is), and the edge
stops you stepping off into the drop; approaching from *outside* (below/beside)
you may still pass onto it.

The client honours this **directionally, by construction**: an Underpass outline
crossing is blocked only in `object_crossing` (`plugins/nav/mod.rs`), which runs
solely for the object the mover is standing on — where every outline crossing is
outward, i.e. "from inside". The terrain/entry path (`blocked_edge_crossed`)
tests only `is_blocked()` edges and so never blocks entry, preserving
"passthrough from outside". Unmarked (`0`) and `Global` (`8`) outline edges
still let a mover walk off at the bridge and stair *ends*.

The old "trapped inside" regression came from blocking `Underpass` in *both*
directions (which also kills the outside→in passthrough), bundled with acting on
the directional block bits — not from these edges being doorways. Blocking only
the inside→out crossing is safe: measured, there is no case where a mover needs
to cross an Underpass outline edge from the cell side.
## Corpus verification (openroad, 2026-08-11)

Byte-parsed all **6229** `nv_XXXX.nvm` in the user's Data.pk2 (`navmesh/` —
note: Data.pk2, not Map.pk2): signature `JMXVNVM 1000` in 6229/6229; the
documented layout consumes **6128/6229 files to exact EOF** (zero trailing
bytes), which leaves no slack for hidden fields anywhere before the fixed
96×96 / 97×97 / 6×6 tails. `openCellCount ≤ totalCellCount` holds in all
files; `MapObject.RegionID == filename region` for 43,421/53,860 objects
(rest are cross-region spill-ins, matching the ownership semantics above).

**Legacy variant (101 files, region ids 0x11a5–0x1fae, old/unused areas):**
4-byte tile records (sampled dwords all zero); 60 of those files additionally
lack the plane maps (exact 180-byte deficit). Height maps are sane in both
forms. Not described by any reference source.

**Loader seam (resolved):** `client/src/assets/nvm/map_object.rs` used to read
the two `IsBig`/`IsStruct` bool bytes as one LE i16 with nibble masks — both
masks landed on the low byte, so all 1,328 IsBig flags were lost, `is_struct`
received IsBig, and the IsStruct byte was never read at all. It now reads two
`u8` bools, matching this section (and the `.o`/`.o2` parsers, which always
did).

The legacy variant carries the **same** `JMXVNVM 1000` signature, so only its
short trailer distinguishes it. The loader now rejects a trailer below
`96*96*8 + 97*97*4 + 36 + 144` bytes with an error instead of running the
height map off the end of the buffer and aborting the client. That path is not
reachable in normal play — none of the 101 legacy region ids is active in
`mapinfo.mfo`, none has a `Map/{z}/{x}.m`, and both terrain-streaming call
sites gate on the mfo bitmask — so it is a robustness guard, not a live fix.

