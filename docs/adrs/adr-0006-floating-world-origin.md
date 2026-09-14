# ADR 0006: Floating world origin (per-scene coordinate rebasing)

Date: 2026-07-13
Status: Accepted

## Context

SRO world coordinates reach magnitudes of ~3×10⁵ units (Jangan spawn:
`(-323526, -33, 187275)`; the region grid is 256×128 regions of 1920 units).
At that magnitude f32 resolves only 0.016–0.03 units (1 unit = 10 cm). Static
geometry hides this — its transforms are computed once and stay constant — but
skinned characters recompute bone `GlobalTransform`s and joint matrices every
frame from slightly different rotations, so the rounding error re-rolls each
frame and animated characters visibly tremble ("Parkinson" micro-shakes) in
the character selection and world scenes. Bevy renders entirely in f32 (mesh
uniforms, view matrices), so the fix must keep *global* translations small;
there is no camera-relative render path to lean on.

## Decision

Each scene anchors a **floating world origin** `O` — a `WorldOrigin(Vec3)`
resource (`client/src/plugins/world_origin.rs`) — near its action and places
world-anchored entities at `sro_pos - O` ("render space"). The few systems
that map back from render space to SRO regions (terrain streamer, environment
profile lookup, debug region readout) add `O` back. Scene `OnEnter` re-anchors
the origin (intro cinematic: first camera keyframe; char select: `cam_base`;
world: spawn point) and shifts already-loaded terrain roots by `O_old − O_new`
so preloaded regions survive scene switches in place.

Constraints:

- `O` is snapped to the region grid (multiples of 1920, exactly representable
  in f32): `terrain_splat.wgsl` recovers block indices from
  `world_position % span`, which only survives translation by exact multiples.
  `O.y` stays 0.
  *Amendment (2026-08-12, ADR-0008):* the snap invariant only serves the splat
  shader, i.e. it must hold *while overworld terrain is resident*. Dungeon
  interiors (region bit 15) live in their own local frame with no 1920 tiling
  and render no splat terrain, so a dungeon anchor sets `O` unsnapped on the
  dungeon geometry (`set_dungeon_origin`); overworld terrain is hidden or
  despawned while a dungeon is active.
- Flat subtraction at placement sites was chosen over a `WorldRoot(-O)` parent
  entity: a parent would leave moving entities with huge (still-quantized)
  local translations and split the codebase into mixed local/global coordinate
  spaces that nav/player/camera systems would have to navigate silently.
- Dynamic re-basing while roaming is deferred. Within ±2¹⁴ units (~8.5
  regions) of `O` the ULP is ≤ 0.002 units — invisible; the streamer only
  keeps ~±4 regions loaded anyway. A future re-base is just
  `set_world_origin` with a new anchor.
- Networking (join handshake, unimplemented): server positions must be
  converted to SRO space and passed through `WorldOrigin::to_render`, with
  `O` re-anchored on the server-provided spawn.

## Consequences

- Animated characters are steady at any world location; nav, picking and
  water/SSR math run at higher precision for free (they were already
  self-consistent in `GlobalTransform` space).
- Every new world-space placement site must subtract the origin, and every
  new render→SRO lookup must add it back; missing the former places content
  ~3×10⁵ units off (loud), missing the latter derives wrong region ids
  (empty world / wrong environment profile).
- SRO-space constants (spawn points, `.intro`/`.selection` keyframes) stay
  authored in original client coordinates — conversion happens only at the
  boundary.
