//! Broad phase over the nav meshes of loaded map objects.
//!
//! Tracking a mover's surface ([`NavLocation`](super::NavLocation)) already
//! removes the expensive case: while standing on an object, exactly one
//! object's edges and triangles are examined. What it does not remove is the
//! question "which object is under this point", which every step taken on the
//! terrain asks in order to notice a bridge ramp or a stair tread — and that
//! was a downward probe against *every* loaded nav object, every frame.
//!
//! This is a uniform hash grid over the objects' world XZ footprints. Objects
//! are registered in every cell their padded bounding box overlaps, so a point
//! query needs to look at one cell only.
//!
//! It is deliberately not a per-triangle index: the win is cutting hundreds of
//! objects down to a handful, and beyond that the per-object AABB reject the
//! nav queries already do is enough.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;

use crate::assets::bms::mesh::JMXVBMS;

use super::ObjectNavMesh;

/// World-space side length of a grid cell, a quarter of a region (1920). Small
/// enough that a cell holds few objects, large enough that even a big compound
/// spans only a handful of cells.
const CELL_SIZE: f32 = 480.0;

/// Matches the padding the nav queries apply to object bounding boxes, so an
/// object can never be rejected here that they would have accepted.
const BOUNDS_PAD: f32 = 5.0;

/// Cap on the cells one range query walks per axis. Only reachable when the
/// query box is enormous (a teleport, not a step).
const MAX_QUERY_CELLS: i32 = 64;

/// Spatial index of every loaded [`ObjectNavMesh`] entity, keyed by world XZ
/// cell. Rebuilt by [`rebuild_nav_object_grid`].
#[derive(Resource, Default)]
pub struct NavObjectGrid {
    cells: HashMap<(i32, i32), Vec<Entity>>,
}

impl NavObjectGrid {
    fn key(x: f32, z: f32) -> (i32, i32) {
        (
            (x / CELL_SIZE).floor() as i32,
            (z / CELL_SIZE).floor() as i32,
        )
    }

    /// The objects whose footprint may cover `world_xz`. Objects are registered
    /// in every cell they overlap, so this single-cell lookup misses nothing.
    pub fn candidates(&self, world_xz: Vec2) -> &[Entity] {
        self.cells
            .get(&Self::key(world_xz.x, world_xz.y))
            .map_or(&[], |entities| entities.as_slice())
    }

    /// The objects whose footprint may overlap the XZ box `min..max` — every
    /// cell the box touches, not just the ones its corners land in.
    ///
    /// Movement queries must use this rather than two point lookups at the step
    /// endpoints. A step longer than [`CELL_SIZE`] passes through cells that
    /// contain neither endpoint, and a wall registered only in those cells
    /// would be missed entirely — a mover fast enough (or a frame long enough)
    /// would walk straight through it.
    ///
    /// May yield an entity more than once when it spans several cells; callers
    /// are short-circuiting predicates, so deduplicating would cost more than
    /// it saves.
    pub fn candidates_in_bounds(&self, min: Vec2, max: Vec2) -> impl Iterator<Item = Entity> + '_ {
        let (min_cell, max_cell) = (Self::key(min.x, min.y), Self::key(max.x, max.y));
        // A box this large means a teleport rather than a step; the caller's
        // per-object rejects handle correctness, this only bounds the walk.
        let clamp = |lo: i32, hi: i32| (lo, hi.min(lo.saturating_add(MAX_QUERY_CELLS)));
        let (x0, x1) = clamp(min_cell.0, max_cell.0);
        let (z0, z1) = clamp(min_cell.1, max_cell.1);

        (x0..=x1).flat_map(move |cx| {
            (z0..=z1).flat_map(move |cz| {
                self.cells
                    .get(&(cx, cz))
                    .map_or(&[][..], |entities| entities.as_slice())
                    .iter()
                    .copied()
            })
        })
    }
}

/// Rebuilds [`NavObjectGrid`] when the set of nav objects or their placement
/// changes.
///
/// Runs in `PostUpdate` after transform propagation so the `GlobalTransform`s
/// read here are this frame's. Map objects are static, so in practice this
/// fires when a region streams in or out, or when the world origin rebases —
/// not every frame.
pub fn rebuild_nav_object_grid(
    mut grid: ResMut<NavObjectGrid>,
    objects: Query<(Entity, &ObjectNavMesh, &GlobalTransform)>,
    changed: Query<
        Entity,
        (
            With<ObjectNavMesh>,
            Or<(Added<ObjectNavMesh>, Changed<GlobalTransform>)>,
        ),
    >,
    mut removed: RemovedComponents<ObjectNavMesh>,
    bms_meshes: Res<Assets<JMXVBMS>>,
    mut assets_ready: Local<bool>,
) {
    let dirty = !changed.is_empty() || removed.read().next().is_some() || !*assets_ready;
    if !dirty {
        return;
    }

    // The meshes an object's footprint is derived from load asynchronously, so
    // a rebuild triggered by the spawn alone would size the entry from whatever
    // had arrived by then. Re-run while any handle is still unresolved.
    //
    // Computed after the early-out, not before it: this is a nested walk of every
    // loaded nav object times every `.bms` handle, each an `Assets` hash lookup,
    // and in steady state its result was discarded unread — the system runs in
    // PostUpdate with no run condition, so that was hundreds to thousands of
    // lookups per frame for nothing. The semantics are unchanged because `dirty`
    // already includes `!*assets_ready`: whenever the value could still be false
    // it is recomputed anyway.
    *assets_ready = objects.iter().all(|(_, object_nav, _)| {
        object_nav
            .0
            .iter()
            .all(|handle| bms_meshes.get(handle).is_some())
    });

    grid.cells.clear();
    for (entity, object_nav, transform) in objects.iter() {
        let Some((min, max)) = world_footprint(object_nav, transform, &bms_meshes) else {
            continue;
        };
        let (min_cell, max_cell) = (
            NavObjectGrid::key(min.x, min.y),
            NavObjectGrid::key(max.x, max.y),
        );
        for cx in min_cell.0..=max_cell.0 {
            for cz in min_cell.1..=max_cell.1 {
                grid.cells.entry((cx, cz)).or_default().push(entity);
            }
        }
    }
}

/// World-space XZ bounds of an object's nav-mesh-carrying meshes. `None` while
/// none of them have loaded.
///
/// The instance transform carries an X mirror and a yaw, so the local bounding
/// box corners have to be transformed and re-bounded rather than transformed as
/// a min/max pair.
fn world_footprint(
    object_nav: &ObjectNavMesh,
    transform: &GlobalTransform,
    bms_meshes: &Assets<JMXVBMS>,
) -> Option<(Vec2, Vec2)> {
    let mut min = Vec2::splat(f32::MAX);
    let mut max = Vec2::splat(f32::MIN);
    let mut any = false;

    for handle in &object_nav.0 {
        let Some(bms) = bms_meshes.get(handle) else {
            continue;
        };
        // The nav mesh's own box, not the visual mesh box: a map object's nav
        // geometry reaches past its drawn geometry, and a footprint that missed
        // that would file the object in the wrong grid cells and hide reachable
        // ground from every movement query (see [`BmsNavMesh::bounds`]).
        let Some(nav) = &bms.navmesh else {
            continue;
        };
        any = true;
        let (lo, hi) = nav.bounds;
        for corner in [
            Vec3::new(lo.x, lo.y, lo.z),
            Vec3::new(hi.x, lo.y, lo.z),
            Vec3::new(lo.x, lo.y, hi.z),
            Vec3::new(hi.x, lo.y, hi.z),
            Vec3::new(lo.x, hi.y, lo.z),
            Vec3::new(hi.x, hi.y, lo.z),
            Vec3::new(lo.x, hi.y, hi.z),
            Vec3::new(hi.x, hi.y, hi.z),
        ] {
            let world = transform.transform_point(corner);
            min = min.min(Vec2::new(world.x, world.z));
            max = max.max(Vec2::new(world.x, world.z));
        }
    }

    any.then(|| (min - BOUNDS_PAD, max + BOUNDS_PAD))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::bms::navmesh::{BmsNavMesh, OutlineLookupGrid};
    use crate::assets::bms::vertex::VertexData;

    /// A mesh with a nav section and a 40x40 local footprint.
    fn footprint_bms() -> JMXVBMS {
        JMXVBMS {
            name: "grid test".to_string(),
            vertex_data: VertexData {
                vertices: Vec::new(),
                lightmap_path: None,
                new_vertex_data: None,
            },
            indices: Vec::new(),
            bounding_box: (Vec3::new(-20.0, 0.0, -20.0), Vec3::new(20.0, 0.0, 20.0)),
            navmesh: Some(BmsNavMesh {
                vertices: Vec::new(),
                cells: Vec::new(),
                outline_edges: Vec::new(),
                inline_edges: Vec::new(),
                events: Vec::new(),
                outline_lookup: OutlineLookupGrid {
                    origin: Vec2::ZERO,
                    width: 0,
                    height: 0,
                    cells: Vec::new(),
                },
                // The footprint the grid buckets by now comes from the nav
                // bounds, not the visual box: a 40x40 deck.
                bounds: (Vec3::new(-20.0, 0.0, -20.0), Vec3::new(20.0, 0.0, 20.0)),
            }),
            material: String::new(),
            bone_data: None,
        }
    }

    /// Objects are found in the cell covering them and nowhere else — the whole
    /// point being that a query at one end of the world doesn't pay for objects
    /// at the other.
    #[test]
    fn indexes_objects_by_their_footprint() {
        let mut app = App::new();
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
        ))
        .init_asset::<JMXVBMS>()
        .init_resource::<NavObjectGrid>()
        .add_systems(Update, rebuild_nav_object_grid);

        let handle = app
            .world_mut()
            .resource_mut::<Assets<JMXVBMS>>()
            .add(footprint_bms());

        let near = app
            .world_mut()
            .spawn((
                ObjectNavMesh(vec![handle.clone()]),
                GlobalTransform::from(Transform::from_xyz(100.0, 0.0, 100.0)),
            ))
            .id();
        let far = app
            .world_mut()
            .spawn((
                ObjectNavMesh(vec![handle]),
                GlobalTransform::from(Transform::from_xyz(5000.0, 0.0, 5000.0)),
            ))
            .id();
        app.update();

        let grid = app.world().resource::<NavObjectGrid>();
        assert_eq!(grid.candidates(Vec2::new(100.0, 100.0)), [near]);
        assert_eq!(grid.candidates(Vec2::new(5000.0, 5000.0)), [far]);
        assert!(grid.candidates(Vec2::new(-9000.0, 0.0)).is_empty());
    }

    /// A despawned object must leave the index, or nav queries would keep
    /// resolving entities that no longer exist when a region streams out.
    #[test]
    fn despawned_objects_leave_the_index() {
        let mut app = App::new();
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
        ))
        .init_asset::<JMXVBMS>()
        .init_resource::<NavObjectGrid>()
        .add_systems(Update, rebuild_nav_object_grid);

        let handle = app
            .world_mut()
            .resource_mut::<Assets<JMXVBMS>>()
            .add(footprint_bms());
        let entity = app
            .world_mut()
            .spawn((
                ObjectNavMesh(vec![handle]),
                GlobalTransform::from(Transform::from_xyz(100.0, 0.0, 100.0)),
            ))
            .id();
        app.update();
        assert_eq!(
            app.world()
                .resource::<NavObjectGrid>()
                .candidates(Vec2::new(100.0, 100.0)),
            [entity]
        );

        app.world_mut().despawn(entity);
        app.update();
        assert!(app
            .world()
            .resource::<NavObjectGrid>()
            .candidates(Vec2::new(100.0, 100.0))
            .is_empty());
    }

    /// An object whose footprint straddles a cell boundary must be reachable
    /// from both sides, or movers would fall through it halfway across.
    #[test]
    fn objects_spanning_cells_are_registered_in_each() {
        let mut app = App::new();
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
        ))
        .init_asset::<JMXVBMS>()
        .init_resource::<NavObjectGrid>()
        .add_systems(Update, rebuild_nav_object_grid);

        let handle = app
            .world_mut()
            .resource_mut::<Assets<JMXVBMS>>()
            .add(footprint_bms());
        // Centred on the boundary between cell 0 and cell 1 (CELL_SIZE = 480).
        let entity = app
            .world_mut()
            .spawn((
                ObjectNavMesh(vec![handle]),
                GlobalTransform::from(Transform::from_xyz(CELL_SIZE, 0.0, 0.0)),
            ))
            .id();
        app.update();

        let grid = app.world().resource::<NavObjectGrid>();
        assert_eq!(grid.candidates(Vec2::new(CELL_SIZE - 10.0, 0.0)), [entity]);
        assert_eq!(grid.candidates(Vec2::new(CELL_SIZE + 10.0, 0.0)), [entity]);
    }

    /// A query box spanning many cells must find an object sitting in a cell
    /// that contains neither of its corners.
    ///
    /// This is the tunnelling case: a step long enough to cross a whole cell —
    /// a very high move speed, or a long frame — used to be checked only at its
    /// two endpoints, so a wall in between was never consulted.
    #[test]
    fn range_query_finds_objects_between_the_endpoints() {
        let mut app = App::new();
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
        ))
        .init_asset::<JMXVBMS>()
        .init_resource::<NavObjectGrid>()
        .add_systems(Update, rebuild_nav_object_grid);

        let handle = app
            .world_mut()
            .resource_mut::<Assets<JMXVBMS>>()
            .add(footprint_bms());

        // Sits in cell 2, well inside a span running from cell 0 to cell 4.
        let middle = 2.5 * CELL_SIZE;
        let entity = app
            .world_mut()
            .spawn((
                ObjectNavMesh(vec![handle]),
                GlobalTransform::from(Transform::from_xyz(middle, 0.0, 10.0)),
            ))
            .id();
        app.update();

        let grid = app.world().resource::<NavObjectGrid>();
        let from = Vec2::new(10.0, 10.0);
        let to = Vec2::new(4.5 * CELL_SIZE, 10.0);

        // Neither endpoint's own cell holds it...
        assert!(grid.candidates(from).is_empty());
        assert!(grid.candidates(to).is_empty());
        // ...but the swept range does.
        let found: Vec<_> = grid
            .candidates_in_bounds(from.min(to), from.max(to))
            .collect();
        assert!(
            found.contains(&entity),
            "a long step must still see the object it passes through"
        );
    }
}
