//! Dungeon-specific navigation data (ADR-0008).
//!
//! Walkability inside a dungeon rides on the existing object-nav machinery:
//! every spawned block root carries the room's `BmsNavMesh` as an
//! [`ObjectNavMesh`](super::ObjectNavMesh), so stepping, wall edges, seam
//! hand-offs between connected blocks and the nearest-Y candidate ranking
//! (stacked floors) all come from the `OnObject` path unchanged. What the
//! DOF adds on top — and what this module models — is:
//!
//! - **collision circles**: `Flag & 2` props are invisible obstacles with a
//!   radius (`sqrt(RadiusSqrt)`), not meshes; the step test consults them
//!   ([`DungeonNavData::step_blocked_by_circle`]).
//! - **the 200³ voxel grid**: position → candidate block indices, used by
//!   the current-block resolver (portal culling / per-block atmosphere) as
//!   the authoritative narrow phase.
//!
//! All queries take *render-space* positions: the data is baked against the
//! world origin at build time, and [`ActiveDungeonNav`] is rebuilt whenever
//! the active dungeon changes (the origin only ever moves on enter/leave).

use std::collections::HashMap;

use bevy::prelude::*;

use crate::assets::dof::format::{DofBlockObject, VoxelId, JMXVDOF, VOXEL_SIZE};
use crate::plugins::dungeon::{mirror_matrix, raw_block_matrix, raw_prop_matrix, ActiveDungeon};
use crate::plugins::dungeon::{DungeonBlock, EnterDungeon};
use crate::plugins::world_origin::WorldOrigin;

/// An invisible collision obstacle (a `Flag & 2` prop), render-space XZ.
#[derive(Debug, Clone, Copy)]
pub struct CollisionCircle {
    pub center: Vec2,
    pub radius: f32,
}

/// The active dungeon's nav-side data, plus the entity ↔ block-index mapping
/// the current-block resolver needs.
#[derive(Resource)]
pub struct ActiveDungeonNav {
    pub region_id: u16,
    pub data: DungeonNavData,
    /// Block root entity → DOF block index.
    pub block_of_entity: HashMap<Entity, usize>,
}

pub struct DungeonNavData {
    /// World origin the render-space data was baked against.
    world_origin: Vec3,
    /// Voxel grid origin in the raw dungeon frame (`CollisionBox0.Min`).
    grid_origin: Vec3,
    grid_dims: (u32, u32, u32),
    voxels: HashMap<VoxelId, Vec<u32>>,
    circles: Vec<CollisionCircle>,
    /// Per-block AABB in the raw dungeon frame. `Block.CollisionBox0` is
    /// stored **block-local** (measured: 0/151 Donwhang boxes sit near their
    /// block's position), so each box is lifted through the block placement
    /// here — every containment/distance consumer must use these, never the
    /// raw boxes.
    world_boxes: Vec<(Vec3, Vec3)>,
}

/// A block-local AABB as a raw-dungeon-frame AABB: transform the 8 corners
/// by the block placement and take the extent (yawed blocks grow, which is
/// correct for a conservative bound).
fn world_box(block: &crate::assets::dof::format::DofBlock) -> (Vec3, Vec3) {
    let matrix = raw_block_matrix(block);
    let bb = &block.collision_box;
    let mut min = Vec3::splat(f32::MAX);
    let mut max = Vec3::splat(f32::MIN);
    for corner in 0..8 {
        let local = Vec3::new(
            if corner & 1 == 0 { bb.min.x } else { bb.max.x },
            if corner & 2 == 0 { bb.min.y } else { bb.max.y },
            if corner & 4 == 0 { bb.min.z } else { bb.max.z },
        );
        let world = matrix.transform_point3(local);
        min = min.min(world);
        max = max.max(world);
    }
    (min, max)
}

impl DungeonNavData {
    pub fn from_dof(dof: &JMXVDOF, origin: &WorldOrigin) -> Self {
        let mut circles = Vec::new();
        for block in &dof.blocks {
            let block_world = mirror_matrix(raw_block_matrix(block));
            for obj in block.objects.iter().filter(|o| o.is_collision_only()) {
                let center_sro = (block_world * raw_prop_matrix(obj)).transform_point3(Vec3::ZERO);
                circles.push(CollisionCircle {
                    center: origin.to_render(center_sro).xz(),
                    radius: circle_radius(obj),
                });
            }
        }
        DungeonNavData {
            world_origin: origin.0,
            grid_origin: dof.collision_box0.min,
            grid_dims: (dof.grid.width, dof.grid.height, dof.grid.length),
            voxels: dof
                .grid
                .voxels
                .iter()
                .map(|voxel| (voxel.id, voxel.block_indices.clone()))
                .collect(),
            circles,
            world_boxes: dof.blocks.iter().map(world_box).collect(),
        }
    }

    /// A block's AABB in the raw dungeon frame.
    pub fn world_box(&self, block_index: usize) -> Option<(Vec3, Vec3)> {
        self.world_boxes.get(block_index).copied()
    }

    /// Candidate block indices for a render-space position, from the voxel
    /// grid. Empty when the position lies outside the grid.
    pub fn voxel_candidates(&self, render_pos: Vec3) -> &[u32] {
        let sro = render_pos + self.world_origin;
        // Back into the raw frame the grid is stored in.
        let raw = Vec3::new(-sro.x, sro.y, sro.z);
        let delta = (raw - self.grid_origin) / VOXEL_SIZE;
        if delta.min_element() < 0.0 {
            return &[];
        }
        let (x, y, z) = (delta.x as u32, delta.y as u32, delta.z as u32);
        let (w, h, l) = self.grid_dims;
        if x >= w || y >= h || z >= l {
            return &[];
        }
        self.voxels
            .get(&VoxelId::pack(x, y, z))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Whether a step (render-space XZ) is stopped by a collision circle.
    /// Outside a circle, entering or crossing it blocks; already inside
    /// (mis-resolve, spawn overlap), only moves that don't go deeper pass —
    /// missing data must never wall a mover in.
    pub fn step_blocked_by_circle(&self, from: Vec2, to: Vec2) -> bool {
        self.circles.iter().any(|circle| {
            let d_from = from.distance(circle.center);
            let d_to = to.distance(circle.center);
            if d_from < circle.radius {
                return d_to < d_from;
            }
            d_to < circle.radius || segment_distance(from, to, circle.center) < circle.radius
        })
    }
}

impl DungeonNavData {
    /// Test-only bare data: just circles, no grid.
    #[cfg(test)]
    pub(crate) fn from_circles(circles: Vec<CollisionCircle>) -> Self {
        DungeonNavData {
            world_origin: Vec3::ZERO,
            grid_origin: Vec3::ZERO,
            grid_dims: (0, 0, 0),
            voxels: HashMap::new(),
            circles,
            world_boxes: Vec::new(),
        }
    }
}

/// Collision radius of a prop: `sqrt(RadiusSqrt)` per the format doc, with
/// non-finite/zero data treated as no obstacle.
fn circle_radius(obj: &DofBlockObject) -> f32 {
    let r = obj.radius_sqrt.max(0.0).sqrt();
    if r.is_finite() {
        r
    } else {
        0.0
    }
}

/// Distance from point `p` to segment `a`→`b`.
fn segment_distance(a: Vec2, b: Vec2, p: Vec2) -> f32 {
    let ab = b - a;
    let len_sq = ab.length_squared();
    if len_sq <= f32::EPSILON {
        return a.distance(p);
    }
    let t = ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0);
    (a + ab * t).distance(p)
}

/// Keep [`ActiveDungeonNav`] in sync with the active dungeon: build it once
/// the DOF is available, drop or rebuild it when the dungeon goes away or
/// changes. Runs in the dungeon plugin's chain after spawning.
pub fn maintain_dungeon_nav(
    active: Option<Res<ActiveDungeon>>,
    nav: Option<Res<ActiveDungeonNav>>,
    dofs: Res<Assets<JMXVDOF>>,
    origin: Res<WorldOrigin>,
    blocks: Query<(Entity, &DungeonBlock)>,
    mut enters: MessageReader<EnterDungeon>,
    mut commands: Commands,
) {
    // A pending enter invalidates the current data even before the new
    // ActiveDungeon exists (the origin is about to move).
    let entering = enters.read().count() > 0;
    match &active {
        None => {
            if nav.is_some() {
                commands.remove_resource::<ActiveDungeonNav>();
            }
        }
        Some(active) => {
            let stale = entering
                || nav
                    .as_ref()
                    .is_some_and(|nav| nav.region_id != active.region_id);
            if stale {
                commands.remove_resource::<ActiveDungeonNav>();
            }
            if (nav.is_none() || stale) && !entering {
                if let Some(dof) = dofs.get(&active.dof) {
                    let block_of_entity = blocks
                        .iter()
                        .map(|(entity, block)| (entity, block.index))
                        .collect();
                    commands.insert_resource(ActiveDungeonNav {
                        region_id: active.region_id,
                        data: DungeonNavData::from_dof(dof, &origin),
                        block_of_entity,
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::assets::dof::format::{DofGrid, DofVoxel};

    fn data(voxels: Vec<DofVoxel>, circles: Vec<CollisionCircle>) -> DungeonNavData {
        DungeonNavData {
            world_origin: Vec3::ZERO,
            grid_origin: Vec3::new(0.0, 0.0, 0.0),
            grid_dims: (4, 2, 4),
            voxels: voxels
                .into_iter()
                .map(|v| (v.id, v.block_indices))
                .collect(),
            circles,
            world_boxes: Vec::new(),
        }
    }

    /// Grid lookups mirror X back into the raw frame and reject positions
    /// outside the grid. Raw x=250 → voxel x=1 lives at render x=-250.
    #[test]
    fn voxel_candidates_lookup() {
        let nav = data(
            vec![
                DofVoxel {
                    id: VoxelId::pack(1, 0, 2),
                    block_indices: vec![3, 7],
                },
                DofVoxel {
                    id: VoxelId::pack(0, 1, 0),
                    block_indices: vec![1],
                },
            ],
            vec![],
        );
        // (raw 250, 50, 450) = voxel (1, 0, 2), reached from render x = -250.
        assert_eq!(
            nav.voxel_candidates(Vec3::new(-250.0, 50.0, 450.0)),
            &[3, 7]
        );
        // Second storey of the first column.
        assert_eq!(nav.voxel_candidates(Vec3::new(-10.0, 250.0, 10.0)), &[1]);
        // Stored-empty voxel and out-of-grid positions yield nothing.
        assert!(nav
            .voxel_candidates(Vec3::new(-10.0, 50.0, 10.0))
            .is_empty());
        assert!(nav.voxel_candidates(Vec3::new(10.0, 50.0, 10.0)).is_empty()); // raw x < 0
        assert!(nav
            .voxel_candidates(Vec3::new(-10.0, 500.0, 10.0))
            .is_empty()); // above the grid
    }

    #[test]
    fn circles_block_entry_and_crossing_but_allow_escape() {
        let nav = data(
            vec![],
            vec![CollisionCircle {
                center: Vec2::new(0.0, 0.0),
                radius: 10.0,
            }],
        );
        // Entering from outside.
        assert!(nav.step_blocked_by_circle(Vec2::new(20.0, 0.0), Vec2::new(5.0, 0.0)));
        // Passing clean through.
        assert!(nav.step_blocked_by_circle(Vec2::new(20.0, 0.0), Vec2::new(-20.0, 0.0)));
        // Skirting past outside the radius.
        assert!(!nav.step_blocked_by_circle(Vec2::new(20.0, 15.0), Vec2::new(-20.0, 15.0)));
        // Inside: moving outward is allowed, deeper is not.
        assert!(!nav.step_blocked_by_circle(Vec2::new(5.0, 0.0), Vec2::new(8.0, 0.0)));
        assert!(nav.step_blocked_by_circle(Vec2::new(5.0, 0.0), Vec2::new(1.0, 0.0)));
    }

    #[test]
    fn from_dof_bakes_circles_in_render_space() {
        use crate::assets::dof::format::{
            DofBlock, DofBlockObject, DofBoundingBox, FogParam, ObjGeneralInfo,
        };
        let obj = DofBlockObject {
            name: String::new(),
            path: String::new(),
            position: Vec3::new(10.0, 0.0, 5.0),
            rotation: Vec3::ZERO,
            scale: Vec3::ONE,
            flag: DofBlockObject::FLAG_COLLISION,
            unk0: 0,
            radius_sqrt: 25.0,
            water_color: None,
        };
        let block = DofBlock {
            path: String::new(),
            name: String::new(),
            unk_uint0: 0,
            position: Vec3::new(100.0, 0.0, 200.0),
            // 90° raw yaw: the local box's X extent maps onto the Z axis.
            yaw: std::f32::consts::FRAC_PI_2,
            is_entrance: 0,
            // Block-LOCAL box (as stored in the data): ±40 x, ±10 z.
            collision_box: DofBoundingBox {
                min: Vec3::new(-40.0, 0.0, -10.0),
                max: Vec3::new(40.0, 100.0, 10.0),
            },
            unk_uint1: 0,
            fog: FogParam {
                color: 0,
                near_plane: 0.0,
                far_plane: 0.0,
                intensity: 0.0,
                height_fog: None,
            },
            unk_byte1: 0,
            unk_byte1_payload: None,
            unk_string: String::new(),
            room_index: 0,
            floor_index: 0,
            connected_block_indices: vec![],
            visible_block_indices: vec![],
            collision_object_count: 1,
            objects: vec![obj],
            lights: vec![],
        };
        let dof = JMXVDOF {
            info: ObjGeneralInfo {
                type_id: -1,
                category: 4,
                name: String::new(),
                unk0: 0,
                unk1: 0,
            },
            region_id: 0x8001,
            collision_box0: DofBoundingBox::default(),
            collision_box1: DofBoundingBox::default(),
            blocks: vec![block],
            links: vec![vec![]],
            grid: DofGrid::default(),
            room_names: vec![],
            floor_names: vec![],
            groups: vec![],
            legacy_block_layout: false,
        };
        // Origin anchored 50 units into mirrored X.
        let origin = WorldOrigin(Vec3::new(-50.0, 0.0, 0.0));
        let nav = DungeonNavData::from_dof(&dof, &origin);
        assert_eq!(nav.circles.len(), 1);
        let circle = nav.circles[0];
        // Prop local (10, 0, 5) under the block's -90° raw rotation lands at
        // raw (95, 0, 210) → sro (-95, 210) → render (-45, 210).
        assert!((circle.center - Vec2::new(-45.0, 210.0)).length() < 1e-4);
        assert!((circle.radius - 5.0).abs() < 1e-6);

        // The block-LOCAL collision box lifts into the raw dungeon frame
        // around the block's position, with the 90° raw yaw (rotation by
        // -yaw) swapping the X/Z extents: ±40 x → ±40 z, ±10 z → ±10 x.
        let (min, max) = nav.world_box(0).unwrap();
        assert!((min - Vec3::new(90.0, 0.0, 160.0)).length() < 1e-3);
        assert!((max - Vec3::new(110.0, 100.0, 240.0)).length() < 1e-3);
        assert!(nav.world_box(1).is_none());
    }
}
