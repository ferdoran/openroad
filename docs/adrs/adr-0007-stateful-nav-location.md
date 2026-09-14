# ADR 0007: Stateful nav location (terrain ↔ object nav mesh transfer)

Date: 2026-07-20
Status: Accepted

## Context

SRO's walkable world is two overlapping surfaces:

- the per-region terrain height field and quad cells of a `.nvm`
  (`data://navmesh/nv_<region>.nvm`), and
- triangle nav meshes carried inside map objects' `.bms` files — bridge decks,
  stairs, building floors — instanced into the world by the object placement in
  each region's `.o2`.

Nothing in the data links the two. `.nvm` `LinkEdge` records join a Global edge
of one *object instance* to a Global edge of another within the same region;
there is no record anywhere that joins an object to the terrain. So the client
must decide, at load time or at runtime, when an actor moves from one surface to
the other.

Both nav meshes were already parsed, and object nav meshes were already cast
against. What was missing was any notion of *which surface an actor is on*.
Movement resolved each frame purely geometrically: it tested the movement
segment against every loaded region's blocked terrain edges *and* every nearby
object's edges, then chose a height by heuristics (`MAX_STEP_UP`,
`OBJECT_BLOCK_Y_BAND`, `HEIGHT_PROBE_RANGE`).

That is why bridges did not work. The terrain running under a bridge is water or
cliff, whose edges are blocked; those edges stopped a mover standing on the deck
above them. The height pick had the mirror problem — the "highest reachable
surface" rule could snap a mover on a deck back down to the ground.

## Decision

Track the surface. A `NavLocation` component (`plugins/nav/location.rs`) on every
mover is one of `Unresolved`, `Terrain`, or `OnObject { object }`, and a single
`NavMeshRaycast::step()` resolves a movement step against that surface alone:

- **`OnObject`** — the mover's own object decides where it may go and how high
  it stands; the **terrain is not consulted at all**, which is the whole point.
  Crossing a passable outline edge leaves the object and transfers.
- **`Terrain`** — terrain edges apply, and crossing into an object's walkable
  area transfers onto it.

Only the *terrain* is skipped while on an object. **Other objects' walls always
apply, on both paths.** Skipping them on the terrain path was a real regression:
movers walked straight through building walls, were then resolved onto the
building's own nav mesh, and found that outline — correctly a wall from within —
blocking every direction back out. An object that does not keep you out becomes
a trap once you are inside it.

**Entering an object is decided by its outline, not by height.** A step that
crosses a passable outline edge and lands on a walkable triangle beyond it has
gone *in*, and that settles the surface outright — the terrain does not get to
compete. Deciding entry by height proximity alone was wrong twice over: a deck
further from the mover's feet than `STAND_TOLERANCE` was never a candidate, and
a deck authored a hair *below* the ground it meets lost the height comparison
forever. Both produced the same symptom — the mover walks down the terrain under
a bridge instead of boarding it.

**Height never decides which surface a mover is on.** It ranks candidates and
reads off the standing height; it does not gate, and terrain and object do not
compete. Wherever an object surface resolves — via a crossing, or via
`STAND_TOLERANCE` candidacy when there is no crossing to read — the object wins,
and the terrain is the fallback for "nothing else covers this position".

This follows the data: terrain cells and edges are strictly 2D
(`NavRect`/`NavLine` of `Vec2`) with height in a separate field, and object
membership is edge adjacency (`NavEdge::src_cell`/`dst_cell`, `LinkEdge`).
Nowhere does the format express "close enough in Y to count" — that was our
invention, and it made every object authored below the ground it meets
unreachable. `prefer_height` takes the higher surface, so a bridge deck a few
units under its own abutment lost to the terrain, the mover stayed on the
terrain, and the ground then sloped away underneath the bridge.

The cost is accepted knowingly: a tree or building whose nav mesh sits under
walkable ground now captures a mover passing over it, bounded only by
`STAND_TOLERANCE`. Height cannot separate that case from a sunken bridge deck —
both are "an object surface just below my feet". If buried meshes prove to be a
problem in practice, the answer is a better membership signal (cell adjacency,
the relation `LinkEdge` encodes), not a restored height comparison.

What keeps an overpass out is the storey filter on the outline edge
(`OBJECT_BLOCK_Y_BAND`), not a height comparison: a deck a full level up is not
a door you can walk through in XZ. That filter is a *disambiguator* for stacked
geometry, which is the one job height legitimately has here — telling two floors
apart when there is no history to say which one you are on.

**Transfers are resolved geometrically at runtime, not precomputed from
`LinkEdge`.** `LinkEdge` cannot answer the object↔terrain case, which is the one
that actually broke; and mapping an `.nvm` object-list index to a spawned entity
is unsound here, because objects are spawned from `.o2`, a compound object
produces one nav-carrying entity per `.cpd` part, and all of it resolves
asynchronously over several frames. Instead, on leaving an object the outline
edge just crossed is carried along, and a candidate whose own outline coincides
with it (`SEAM_EPSILON`) wins over one that is merely at a similar height. That
coincidence *is* the relation `LinkEdge` encodes, recovered from geometry — and
it covers object↔terrain too.

**Invalidation is the entity's liveness.** A `Query::get` miss on the stored
entity — region streamed out, object despawned — drops the location to
`Unresolved`, which re-resolves geometrically on the next query. There is no
separate bookkeeping, no component hooks, and no coupling to region unloading.
Teleports set `Unresolved` explicitly.

**Missing data is never a wall.** A step that cannot be resolved because assets
or regions have not streamed in returns `NavStep::Unknown`; the caller holds
position and retries. Only a genuinely blocked edge returns `NavStep::Blocked`.
Conflating the two would wall movers in at streaming seams.

`Player` and `RemoteEntity` both `#[require(NavLocation)]`, so a spawn site
cannot forget it — the movement systems query it mutably and a mover without it
would silently stop.

Remote entities get `ground()`, not `step()`: they are server-authoritative, and
a client-side wall test would freeze or desync them when client and server
disagree. Only their height is corrected.

**Amendment (2026-08-14): `ground()` re-resolves its surface.** As first written
it only *corrected* a known surface, which made `Terrain` a terminal state — a
remote entity could fall off an object but never climb onto one, so every mount,
monster and remote player sank through staircases and bridge ramps. It now runs
the same `surface_at` resolution the spawn path uses, in both arms: terrain
movers can board an object, and an entity leaving one object can land on the
next (a staircase is often several nav objects, one per `.cpd` part). The
`STAND_TOLERANCE` candidacy rule bounds this to step-up distance, so a deck
overhead still cannot capture someone walking underneath. This does **not**
reintroduce client-side blocking — the decision above stands; only the surface
follows the geometry now.

## Alternatives considered

**Precompute a link graph from `.nvm` `object_list` + `LinkEdge` +
`NavCellQuad.object_indices`.** Faithful to the original client, and the reason
those fields exist. Rejected because it answers the wrong question (object↔object
only), and because the index→entity mapping is one-to-many and frame-order
dependent in this codebase. The fields remain parsed and unused; if a case is
ever found where geometry picks wrong, `LinkEdge` can be layered on as a
disambiguator rather than the primary mechanism.

**Track a cell index alongside the object**, walking cell→cell through inline
edges as the original client does. This would make queries O(1) and is the
"correct" long-term shape. Deferred: the cell index would need re-validating
against a possibly-unloaded asset every frame, and it must recover from
teleports and server position syncs. The entity handle already gives the
narrowing that matters. Worth revisiting if per-object scans ever show up in a
profile.

## Consequences

- Movement call sites use one `step()` (or `ground()`) instead of a
  `segment_blocked` + `walkable_height` pair; both of those are gone.
- Anything new that moves an actor must thread `NavLocation` through, and
  anything that moves an actor *discontinuously* must set `Unresolved`.
- The heuristic constants remain, but their role shrank to intra-surface
  questions: `MAX_STEP_UP` for stair treads within one object,
  `OBJECT_BLOCK_Y_BAND` for storeys of one object, `STAND_TOLERANCE` for
  "am I on this".
- General blocking uses the plain, non-directional `is_blocked()` (the two
  block bits). An earlier revision also honoured the flag byte's *directional*
  bits, which measuring showed to be a no-op by construction (every blocked
  terrain edge borders void, so the mover is always on the `src` side).
- `Underpass` outline edges *are* honoured, as the fall-off boundary of an
  elevated walkable surface — a bridge railing, a stair side, a wall top. This
  reverses an earlier decision that called `Underpass` non-blocking. Re-measuring
  the corpus settled it: all 626 `Underpass` outline edges are one-sided
  boundaries and every carrier is a bridge/stair/wall/balcony, none a doorway
  (see `docs/formats/nvm-jmxvnvm.md`). The flag is directional —
  "passthrough from outside, blocked from inside" — and is applied only in
  `object_crossing`, which runs for the object the mover stands *on*, where every
  outline crossing is outward; entry (the terrain path) tests only `is_blocked()`
  and so is never affected. The old "trapped indoors" regression came from
  blocking `Underpass` both ways (killing the outside→in passthrough), bundled
  with the directional-bit change — not from these edges being doorways. Lesson
  unchanged: measure a flag's distribution *and its carriers' geometry* before
  fixing its semantics; the first measurement mistook the railings for doors.
- "Which object is under this point" is served by `NavObjectGrid`
  (`plugins/nav/index.rs`), a world-XZ hash grid rebuilt when the object set or
  its placement changes.
- Ice planes are modelled: over a frozen cell (`plane_type_map` = `Ice` or
  `WaterIce`) the walkable surface is the ice sheet (`plane_height_map`), raised
  above the lake bed, and the cell is walkable regardless of the terrain cell
  openness beneath it — you can cross a frozen lake even over deep, closed
  water. Without this a mover walked the lake bed *under* the rendered ice (the
  Karakoram bug). See `JMXVNVM::walkable_height_at` / `is_ice_at` and
  `docs/formats/nvm-jmxvnvm.md`.
- Plain `Water` is still unmodelled: it is not lifted to a surface, so falling
  off a bridge over open water still lands on the terrain height field rather
  than a water plane.
