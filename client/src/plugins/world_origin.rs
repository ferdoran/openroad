//! Floating world origin: SRO world coordinates reach magnitudes of ~3*10^5
//! units, where f32 only resolves 0.016-0.03 units. Skinned bone matrices are
//! recomputed every frame from slightly different rotations, so at those
//! magnitudes the rounding error re-rolls each frame and animated characters
//! visibly tremble. The fix: every scene picks an origin `O` near its action
//! (snapped to the region grid) and places world-anchored entities at
//! `sro_pos - O`, keeping render-space coordinates small. The few systems that
//! map back from render space to SRO regions add `O` again.
//!
//! Invariants / notes:
//! - `O` MUST be a multiple of `REGION_SIZE` (1920 = 2^7 * 15, exactly
//!   representable in f32) *while overworld terrain is resident*:
//!   `terrain_splat.wgsl` recovers block indices from `world_position % span`
//!   (span divides 1920), which only survives translation by exact multiples
//!   of the span. Dungeon interiors render no splat terrain, so a dungeon
//!   anchor ([`set_dungeon_origin`]) is exempt and anchors unsnapped on the
//!   dungeon's local frame (ADR-0006 amendment).
//! - `O.y` stays 0; heights are small enough for f32.
//! - Precision budget: within ~2^14 units (~8.5 regions) of `O` the f32 ULP is
//!   <= 0.002 units (0.2mm) - invisible. Dynamic re-basing while roaming is
//!   deferred; a re-base is "set a new snapped origin", which
//!   [`set_world_origin`] already supports by shifting loaded terrain.
//! - Networking (once the join handshake lands): convert the packet's
//!   (region, local offset) to an SRO-space Vec3, pass it through
//!   [`WorldOrigin::to_render`], and set `O = snap_to_region_grid(spawn)`
//!   instead of a hardcoded anchor.

use bevy::ecs::query::QueryFilter;
use bevy::prelude::{Query, ResMut, Resource, Transform, Vec3};

use crate::plugins::map::terrain::REGION_SIZE;

/// The SRO-space position rendered at the render-space origin. World-anchored
/// spawns subtract it ([`Self::to_render`]); render->SRO lookups (terrain
/// streaming, environment profiles) add it back ([`Self::to_sro`]).
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq)]
pub struct WorldOrigin(pub Vec3);

impl WorldOrigin {
    pub fn to_render(&self, sro: Vec3) -> Vec3 {
        sro - self.0
    }

    pub fn to_sro(&self, render: Vec3) -> Vec3 {
        render + self.0
    }
}

/// Snap an SRO-space position down to the region grid (x/z multiples of
/// [`REGION_SIZE`], y = 0). `floor` keeps the snap consistent on the negative
/// world-X side (SRO's mirrored X convention: region x -> world -x * 1920).
pub fn snap_to_region_grid(sro: Vec3) -> Vec3 {
    Vec3::new(
        (sro.x / REGION_SIZE).floor() * REGION_SIZE,
        0.0,
        (sro.z / REGION_SIZE).floor() * REGION_SIZE,
    )
}

/// Point the world origin at (the region grid snap of) `sro_anchor` and shift
/// any already-loaded terrain regions so they keep their world placement.
/// `terrain` must be a `With<Terrain>` query (extra `Without<...>` filters for
/// disjointness are fine): terrain roots carry their whole region subtree (map
/// objects, water, compounds) and `PreloadedTerrain` regions survive scene
/// switches, so shifting them is mandatory - despawned-and-restreamed regions
/// pick the new origin up on spawn.
pub fn set_world_origin<F: QueryFilter>(
    sro_anchor: Vec3,
    origin: &mut ResMut<WorldOrigin>,
    terrain: &mut Query<&mut Transform, F>,
) {
    apply_origin(snap_to_region_grid(sro_anchor), origin, terrain);
}

/// Point the world origin at an *unsnapped* dungeon anchor. Dungeon interiors
/// live in their own local frame (no 1920 tiling and no terrain splat shader,
/// which is the only consumer of the multiple-of-1920 invariant), so the
/// origin anchors directly on the dungeon geometry — typically the mirrored
/// `CollisionBox0.Min` or an arrival point — with `y` forced to 0 like the
/// grid snap does. Any still-loaded overworld terrain is shifted along, same
/// as a grid re-anchor (it is hidden/despawned while a dungeon is active).
pub fn set_dungeon_origin<F: QueryFilter>(
    sro_anchor: Vec3,
    origin: &mut ResMut<WorldOrigin>,
    terrain: &mut Query<&mut Transform, F>,
) {
    apply_origin(sro_anchor.with_y(0.0), origin, terrain);
}

fn apply_origin<F: QueryFilter>(
    new_origin: Vec3,
    origin: &mut ResMut<WorldOrigin>,
    terrain: &mut Query<&mut Transform, F>,
) {
    let shift = origin.0 - new_origin;
    if shift == Vec3::ZERO {
        return;
    }
    for mut transform in terrain.iter_mut() {
        transform.translation += shift;
    }
    origin.0 = new_origin;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snap_handles_negative_world_x() {
        // Jangan spawn; world x is negative (mirrored region x).
        let jangan = Vec3::new(-323526.03, -32.608875, 187275.28);
        assert_eq!(
            snap_to_region_grid(jangan),
            Vec3::new(-324480.0, 0.0, 186240.0)
        );
    }

    #[test]
    fn snap_is_exact_on_grid_points() {
        let on_grid = Vec3::new(-155520.0, 0.0, 201600.0); // constantinople cam_base
        assert_eq!(snap_to_region_grid(on_grid), on_grid);
    }

    #[test]
    fn render_round_trip() {
        let origin = WorldOrigin(snap_to_region_grid(Vec3::new(-323526.03, 0.0, 187275.28)));
        let sro = Vec3::new(-323526.03, -32.608875, 187275.28);
        let render = origin.to_render(sro);
        // small render-space coordinates and exact round-trip
        assert!(render.length() < 2.0 * REGION_SIZE);
        assert_eq!(origin.to_sro(render), sro);
    }
}
