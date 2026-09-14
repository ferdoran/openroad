use bevy::ecs::system::SystemParam;
use bevy::math::Affine3A;
use bevy::prelude::*;

use crate::assets::bms::mesh::JMXVBMS;
use crate::assets::bms::navmesh::BmsNavMesh;
use crate::assets::nvm::JMXVNVM;
use crate::plugins::map::terrain::{TerrainNavMeshData, REGION_SIZE};

pub mod decal;
pub mod dungeon;
pub mod edges;
pub mod index;
pub mod location;

use dungeon::ActiveDungeonNav;
use edges::{classify_crossing, EdgeCross, EdgeProbe};
pub use index::NavObjectGrid;
pub use location::{NavLocation, NavStep};

/// How far along the ray (in world units) a nav mesh hit is searched for.
const MAX_CAST_DISTANCE: f32 = 100_000.0;
/// Step size (world units) for marching the ray across a region's height field.
/// Half the height map's 20-unit sample spacing, so no sample cell is skipped.
const MARCH_STEP: f32 = 10.0;
/// Bisection iterations refining the crossing found by the march (10 halvings of
/// a `MARCH_STEP` bracket → sub-centimeter precision).
const REFINE_ITERATIONS: u32 = 10;
/// How far above the reference height the downward probe for object nav mesh
/// surfaces starts (see [`NavMeshRaycast::object_surface_height`]).
const HEIGHT_PROBE_RANGE: f32 = 500.0;
/// Max world-Y difference between an object edge and the mover for that edge to
/// apply (see [`NavMeshRaycast::object_crossing`]). Keeps a wall on one floor
/// from blocking movement on a different stacked level of the same object
/// (character bodies are ~18 units tall, so 100 clears a storey).
pub(crate) const OBJECT_BLOCK_Y_BAND: f32 = 100.0;
/// How far above the mover's feet a walkable surface may sit and still be
/// stepped onto (see [`pick_step_height`]). Lets a mover climb
/// stairs/step objects (whose next tread is a little above), while a full
/// storey up (an overhead bridge deck, ~[`OBJECT_BLOCK_Y_BAND`]) stays out of
/// reach so the mover keeps to the ground running underneath it.
const MAX_STEP_UP: f32 = 30.0;
/// How close a walkable object surface must be to the mover's feet for the
/// mover to count as *standing on* that object (see
/// [`NavMeshRaycast::standing_on_object`]). Tighter than [`MAX_STEP_UP`]: this
/// answers "what am I on right now", not "what could I step onto".
const STAND_TOLERANCE: f32 = 20.0;

/// The walkable (object-level) nav meshes of a spawned map object. Attached to
/// the resource root entity by `SpawnResource` (commands/mod.rs) whenever any of
/// the object's `.bms` meshes carries a nav mesh section, so bridges, stairs and
/// building floors are cast against in world space via the entity's transform.
#[derive(Component)]
pub struct ObjectNavMesh(pub Vec<Handle<JMXVBMS>>);

/// Result of a ray cast against the nav mesh (terrain height field or a map
/// object's walkable triangle mesh).
#[allow(dead_code)]
pub struct NavMeshHit {
    /// World-space point where the ray meets the nav mesh surface.
    pub point: Vec3,
    /// Distance from the ray origin to `point`.
    pub distance: f32,
    /// World-space normal of the surface at the hit. `Vec3::Y` for terrain;
    /// the triangle plane normal for objects.
    pub normal: Vec3,
    pub surface: NavMeshSurface,
}

#[allow(dead_code)]
pub enum NavMeshSurface {
    Terrain {
        /// The terrain region entity whose height field was hit.
        region: Entity,
        /// Index into that region's `quad_cell_list.items` of the cell containing
        /// the hit, or `None` if the point lies on the height field but outside
        /// every nav cell (i.e. not walkable).
        cell: Option<usize>,
    },
    Object {
        /// The object resource entity (carries [`ObjectNavMesh`]) that was hit.
        object: Entity,
    },
}

impl NavMeshHit {
    /// Whether a character could stand at the hit point: inside a terrain nav
    /// cell, or anywhere on an object's walkable nav mesh.
    pub fn is_walkable(&self) -> bool {
        match &self.surface {
            NavMeshSurface::Terrain { cell, .. } => cell.is_some(),
            NavMeshSurface::Object { .. } => true,
        }
    }
}

/// Casts rays against the nav meshes of all loaded terrain regions.
///
/// Add it as a system param and cast any world-space ray — e.g. the cursor ray
/// that `move_cursor` (cursor/mod.rs) refreshes every frame:
///
/// ```ignore
/// fn my_system(cursor_camera: Query<&GameCursorCamera>, nav: NavMeshRaycast) {
///     let Some(ray) = cursor_camera.single().ok().and_then(|c| c.cursor_ray) else { return };
///     if let Some(hit) = nav.cast(&ray) { /* hit.point */ }
/// }
/// ```
#[derive(SystemParam)]
pub struct NavMeshRaycast<'w, 's> {
    regions: Query<
        'w,
        's,
        (
            Entity,
            &'static TerrainNavMeshData,
            &'static GlobalTransform,
        ),
    >,
    nav_meshes: Res<'w, Assets<JMXVNVM>>,
    objects: Query<'w, 's, (Entity, &'static ObjectNavMesh, &'static GlobalTransform)>,
    bms_meshes: Res<'w, Assets<JMXVBMS>>,
    /// Broad phase over `objects`. Optional so tests and tools work without the
    /// plugin that maintains it; absent means "consider every object".
    object_grid: Option<Res<'w, NavObjectGrid>>,
    /// Dungeon extras (collision circles) while a dungeon interior is active;
    /// block meshes themselves are ordinary [`ObjectNavMesh`] objects.
    dungeon: Option<Res<'w, ActiveDungeonNav>>,
}

impl NavMeshRaycast<'_, '_> {
    /// Nearest intersection of `ray` with the nav mesh — the terrain height
    /// field of any loaded region, or the walkable triangle mesh of any loaded
    /// map object (bridge decks, stairs, ...). Returns `None` on a full miss.
    pub fn cast(&self, ray: &Ray3d) -> Option<NavMeshHit> {
        let mut best: Option<NavMeshHit> = None;

        for (entity, nav_mesh_data, transform) in self.regions.iter() {
            let Some(nav_mesh) = self.nav_meshes.get(&nav_mesh_data.0) else {
                continue;
            };
            let origin = transform.translation();

            let Some((distance, local)) = raycast_height_map(ray, origin, nav_mesh) else {
                continue;
            };
            if best.as_ref().is_some_and(|b| b.distance <= distance) {
                continue;
            }

            // Only open cells count: `NavMeshHit::is_walkable` keys off this,
            // and reporting a solid cell as a hit let click-to-move target the
            // inside of walls. An ice sheet is the exception — walkable whatever
            // the bed cell beneath it does (see `terrain_walkable_at`), so its
            // cell is kept unfiltered to mark the hit standable.
            let open_cells = nav_mesh.quad_cell_list.open_cell_count.max(0) as usize;
            let on_ice = nav_mesh.is_ice_at(local.x, local.y);
            let cell = nav_mesh
                .quad_cell_list
                .cell_at(local)
                .filter(|index| on_ice || *index < open_cells);

            best = Some(NavMeshHit {
                point: ray.origin + *ray.direction * distance,
                distance,
                normal: Vec3::Y,
                surface: NavMeshSurface::Terrain {
                    region: entity,
                    cell,
                },
            });
        }

        for (entity, object_nav, transform) in self.objects.iter() {
            for handle in &object_nav.0 {
                let Some(bms) = self.bms_meshes.get(handle) else {
                    continue;
                };
                let Some(nav) = &bms.navmesh else { continue };

                let Some(hit) = raycast_object_nav_mesh(ray, transform, nav) else {
                    continue;
                };
                if best.as_ref().is_some_and(|b| b.distance <= hit.distance) {
                    continue;
                }
                best = Some(NavMeshHit {
                    surface: NavMeshSurface::Object { object: entity },
                    ..hit
                });
            }
        }

        best
    }

    /// Like [`cast`](Self::cast), but only returns hits a character could
    /// actually stand on (inside a terrain nav cell, or on an object nav mesh).
    pub fn cast_walkable(&self, ray: &Ray3d) -> Option<NavMeshHit> {
        self.cast(ray).filter(NavMeshHit::is_walkable)
    }

    /// Move a mover from `from` towards `to_xz`, resolved against the surface
    /// it is standing on rather than against everything loaded.
    ///
    /// This is the stateful replacement for the
    /// This replaced a stateless `segment_blocked` + `walkable_height` pair
    /// that tested against everything loaded: knowing the mover is on
    /// a bridge deck is what keeps the blocked terrain edges of the water
    /// underneath from stopping it, and what keeps its height on the deck.
    ///
    /// A stale `location` (the object despawned with its region) is silently
    /// re-resolved, so callers can keep handing back whatever they last stored.
    pub fn step(&self, from: Vec3, to_xz: Vec2, location: NavLocation) -> NavStep {
        let location = self.validated(from, location);

        // DOF collision circles (`Flag & 2` props) are invisible obstacles
        // with no mesh — an extra wall test on top of whatever surface the
        // mover is on.
        if let Some(dungeon) = &self.dungeon {
            if dungeon.data.step_blocked_by_circle(from.xz(), to_xz) {
                return NavStep::Blocked;
            }
        }

        match location {
            NavLocation::OnObject { object } => self.step_on_object(object, from, to_xz),
            NavLocation::Terrain => self.step_on_terrain(from, to_xz),
            // Nothing loaded covers the mover: hold position and retry.
            NavLocation::Unresolved => NavStep::Unknown,
        }
    }

    /// Which surface `position` sits on, decided purely geometrically. Used for
    /// spawn, teleport, and recovery from a stale location. Prefers an object
    /// surface within [`STAND_TOLERANCE`] of the position, else terrain.
    pub fn resolve_location(&self, position: Vec3) -> NavLocation {
        self.surface_at(position.xz(), position.y, None)
            .map_or(NavLocation::Unresolved, |(_, location)| location)
    }

    /// Surface height at `xz` on a known surface, with the location corrected
    /// if it went stale. For movers that only need snapping to the ground and
    /// do no blocking tests of their own — server-authoritative remote
    /// entities, decal draping.
    ///
    /// Unlike [`Self::step`] this runs no edge-crossing test, so it never
    /// blocks; it does, however, **re-resolve the surface every call** through
    /// [`Self::surface_at`] — the same helper [`Self::resolve_location`] uses
    /// at spawn. That is what lets a mover walk *onto* an object: stairs, a
    /// bridge deck, the next `.cpd` part of a compound object. Candidacy is
    /// bounded by [`best_object_surface`]'s [`STAND_TOLERANCE`] rule, i.e. an
    /// object only counts when its surface is within a step of the mover's
    /// feet, so a deck overhead can never capture someone walking underneath.
    ///
    /// Before that, terrain movers were locked to the heightfield forever
    /// (`Terrain` had no path back to `OnObject`), which sank remote entities —
    /// mounts, monsters, other players — through every staircase they climbed.
    ///
    /// [`best_object_surface`]: Self::best_object_surface
    pub fn ground(
        &self,
        xz: Vec2,
        reference_y: f32,
        location: NavLocation,
    ) -> Option<(f32, NavLocation)> {
        let probe = Vec3::new(xz.x, reference_y, xz.y);
        let location = self.validated(probe, location);

        match location {
            NavLocation::OnObject { object } => self
                .object_surface_height(object, xz, reference_y)
                .map(|height| (height, location))
                // Walked off the object (or into a gap in its mesh): re-resolve
                // rather than freeze at the old height — the next part of the
                // same staircase, another object, or the terrain below.
                .or_else(|| self.surface_at(xz, reference_y, None)),
            // Terrain movers re-resolve too, so they can board an object.
            NavLocation::Terrain => self.surface_at(xz, reference_y, None),
            NavLocation::Unresolved => None,
        }
    }

    /// The nav objects whose footprint may overlap the step `from_xz`→`to_xz`,
    /// via [`NavObjectGrid`] when it is available and the full set otherwise.
    fn objects_over_segment(
        &self,
        from_xz: Vec2,
        to_xz: Vec2,
    ) -> Box<dyn Iterator<Item = (Entity, &ObjectNavMesh, &GlobalTransform)> + '_> {
        match &self.object_grid {
            Some(grid) => Box::new(
                grid.candidates_in_bounds(from_xz.min(to_xz), from_xz.max(to_xz))
                    .filter_map(|entity| self.objects.get(entity).ok()),
            ),
            None => Box::new(self.objects.iter()),
        }
    }

    /// The nav objects whose footprint may cover `world_xz`, via
    /// [`NavObjectGrid`] when it is available and the full set otherwise.
    ///
    /// Both arms yield the same tuples, so callers are identical; the grid only
    /// changes how many of them there are.
    fn objects_near(
        &self,
        world_xz: Vec2,
    ) -> Box<dyn Iterator<Item = (Entity, &ObjectNavMesh, &GlobalTransform)> + '_> {
        match &self.object_grid {
            Some(grid) => Box::new(
                grid.candidates(world_xz)
                    .iter()
                    .filter_map(|entity| self.objects.get(*entity).ok()),
            ),
            None => Box::new(self.objects.iter()),
        }
    }

    /// Drops a location that no longer refers to a live object and re-resolves
    /// it. `Query::get` failing *is* the invalidation mechanism — regions
    /// despawn their object children on unload, so no separate bookkeeping is
    /// needed to notice.
    fn validated(&self, position: Vec3, location: NavLocation) -> NavLocation {
        let location = match location {
            NavLocation::OnObject { object } if self.objects.get(object).is_err() => {
                NavLocation::Unresolved
            }
            other => other,
        };
        if location.is_resolved() {
            location
        } else {
            self.resolve_location(position)
        }
    }

    /// A step taken while standing on `object`: only that object's own edges
    /// and triangles apply.
    fn step_on_object(&self, object: Entity, from: Vec3, to_xz: Vec2) -> NavStep {
        let Some(crossing) = self.object_crossing(object, from, to_xz) else {
            // The object's meshes aren't loaded (yet). Not a wall.
            return NavStep::Unknown;
        };

        // Other objects' walls still apply — only the *terrain* is ignored while
        // standing on an object, which is what makes bridges work. A building
        // next to the bridge is still a building.
        if self.objects_block(from, to_xz, Some(object)) {
            return NavStep::Blocked;
        }

        match crossing {
            ObjectCrossing::Blocked => NavStep::Blocked,
            ObjectCrossing::Stays => match self.object_surface_height(object, to_xz, from.y) {
                Some(height) => NavStep::Moved {
                    position: Vec3::new(to_xz.x, height, to_xz.y),
                    location: NavLocation::OnObject { object },
                },
                // No triangle under the destination even though no outline edge
                // was crossed — a gap in the object's nav mesh. Re-resolve
                // instead of blocking, or movers strand on imperfect meshes.
                None => self.step_after_transfer(from, to_xz, None),
            },
            ObjectCrossing::Exits { edge } => {
                self.step_after_transfer(from, to_xz, Some((object, edge)))
            }
        }
    }

    /// A step taken on the terrain, which may walk onto an object (a bridge
    /// ramp, the first stair tread).
    ///
    /// Both the terrain's own edges *and* the walls of nearby objects apply.
    /// Skipping the latter is what let movers walk straight through a building
    /// wall: once inside, they were resolved onto the building's nav mesh, and
    /// its outline — correctly a wall from within — blocked every direction out
    /// again. Objects have to keep you out, or their insides become traps.
    fn step_on_terrain(&self, from: Vec3, to_xz: Vec2) -> NavStep {
        if self.terrain_blocked(from.xz(), to_xz) || self.objects_block(from, to_xz, None) {
            return NavStep::Blocked;
        }

        let step = self.step_after_transfer(from, to_xz, None);

        // Blocked edges are supposed to fence off solid ground, but their
        // coverage can't be relied on — a mover that slips past one walks into
        // rock. The cells themselves are the authoritative signal (see
        // `NavCellQuadList`), so a step that would *land on the terrain* has to
        // land in an open one.
        //
        // Only checked when landing on the terrain: stepping onto a building
        // floor or bridge ramp is legitimate even where the ground beneath it
        // is solid, and that case resolves to `OnObject`.
        if let NavStep::Moved {
            location: NavLocation::Terrain,
            ..
        } = step
        {
            if !self.terrain_walkable_at(to_xz) {
                return NavStep::Blocked;
            }
        }

        step
    }

    /// Whether a world XZ position sits in walkable terrain, per the covering
    /// region's open/closed cells. Positions outside every loaded region count
    /// as walkable — missing data must never be a wall.
    fn terrain_walkable_at(&self, world_xz: Vec2) -> bool {
        for (_, nav_mesh_data, transform) in self.regions.iter() {
            let origin = transform.translation();
            let local = to_local(origin, Vec3::new(world_xz.x, 0.0, world_xz.y));
            if !(0.0..=REGION_SIZE).contains(&local.x) || !(0.0..=REGION_SIZE).contains(&local.y) {
                continue;
            }
            return self
                .nav_meshes
                .get(&nav_mesh_data.0)
                .is_none_or(|nav_mesh| {
                    // A frozen plane is walkable regardless of the terrain cell
                    // openness beneath it: you can cross a lake's ice even where its
                    // bed is deep, closed water.
                    nav_mesh.is_ice_at(local.x, local.y)
                        || nav_mesh.quad_cell_list.is_walkable_at(local)
                });
        }
        true
    }

    /// Whether the step crosses a blocked edge of any nearby map object, other
    /// than `standing_on` — that one is the mover's own surface and is handled
    /// by [`object_crossing`](Self::object_crossing), which also has to
    /// recognise the passable outline edges it leaves through.
    fn objects_block(&self, from: Vec3, to_xz: Vec2, standing_on: Option<Entity>) -> bool {
        // The whole swept step, not just its endpoints: a long step (high move
        // speed, or a long frame) crosses cells that contain neither end, and a
        // wall living only in those would be missed.
        let from_xz = from.xz();
        let near = self
            .objects_over_segment(from_xz, to_xz)
            .filter(|(entity, _, _)| standing_on != Some(*entity));

        for (_, object_nav, transform) in near {
            if self.blocked_edge_crossed(object_nav, transform, from, to_xz) {
                return true;
            }
        }
        false
    }

    /// Does the step cross one of this object's *blocked* edges? Object-local
    /// throughout: the instance transform's X mirror would invert side tests
    /// done in world space.
    fn blocked_edge_crossed(
        &self,
        object_nav: &ObjectNavMesh,
        transform: &GlobalTransform,
        from: Vec3,
        to_xz: Vec2,
    ) -> bool {
        let inverse = transform.affine().inverse();
        let a = inverse.transform_point3(from);
        let b = inverse.transform_point3(Vec3::new(to_xz.x, from.y, to_xz.y));
        let (a, b) = (Vec2::new(a.x, a.z), Vec2::new(b.x, b.z));

        for handle in &object_nav.0 {
            let Some(bms) = self.bms_meshes.get(handle) else {
                continue;
            };
            let Some(nav) = &bms.navmesh else { continue };

            let pad = 5.0;
            let (seg_min, seg_max) = (a.min(b), a.max(b));
            if seg_max.x < nav.bounds.0.x - pad
                || seg_min.x > nav.bounds.1.x + pad
                || seg_max.y < nav.bounds.0.z - pad
                || seg_min.y > nav.bounds.1.z + pad
            {
                continue;
            }

            for edge in nav.outline_edges.iter().chain(nav.inline_edges.iter()) {
                if !edge.flag.is_blocked() {
                    continue;
                }
                let v0 = nav.vertices[edge.src_vertex as usize].position;
                let v1 = nav.vertices[edge.dst_vertex as usize].position;

                // Only walls on the mover's own storey apply, so an overhead
                // bridge deck doesn't block the ground running beneath it.
                let edge_y =
                    0.5 * (transform.transform_point(v0).y + transform.transform_point(v1).y);
                if (edge_y - from.y).abs() > OBJECT_BLOCK_Y_BAND {
                    continue;
                }

                let probe = EdgeProbe {
                    src: Vec2::new(v0.x, v0.z),
                    dst: Vec2::new(v1.x, v1.z),
                    flag: &edge.flag,
                };
                if classify_crossing(a, b, &probe) == Some(EdgeCross::Blocked) {
                    return true;
                }
            }
        }
        false
    }

    /// Land the mover at `to_xz`, re-deciding which surface it ends up on.
    /// Used wherever the surface may have changed: leaving an object, entering
    /// one from the terrain, or falling into a hole in an object's mesh.
    ///
    /// `exited` is the object just walked off and the outline edge left
    /// through, when there was one. Where several objects overlap in XZ at a
    /// similar height — the far side of a spiral ramp, the next span of a
    /// multi-part bridge — the one whose own outline meets that edge is the one
    /// actually being stepped onto.
    fn step_after_transfer(
        &self,
        from: Vec3,
        to_xz: Vec2,
        exited: Option<(Entity, (Vec3, Vec3))>,
    ) -> NavStep {
        // Walking *into* an object's walkable area is what puts a mover on it,
        // and it settles the question outright — the terrain does not get to
        // compete. Height proximity alone can't decide this: a bridge deck
        // authored a hair below the ground it meets loses `prefer_height`
        // forever, so the mover walks down the terrain underneath instead of
        // boarding. The crossing already said which surface it is on.
        if let Some((object, height)) = self.object_entered(from, to_xz, exited) {
            return NavStep::Moved {
                position: Vec3::new(to_xz.x, height, to_xz.y),
                location: NavLocation::OnObject { object },
            };
        }

        match self.surface_at(to_xz, from.y, exited) {
            Some((height, location)) => NavStep::Moved {
                position: Vec3::new(to_xz.x, height, to_xz.y),
                location,
            },
            None => NavStep::Unknown,
        }
    }

    /// The surface a mover at `reference_y` would end up on at `world_xz`, as
    /// (height, which surface).
    ///
    /// An object surface wins whenever one resolves at all; the terrain is the
    /// *fallback*, used only when no object cell covers the position.
    ///
    /// Terrain and object used to compete on [`prefer_height`], which takes the
    /// higher surface. That silently made every object authored below the
    /// ground it meets unreachable — including bridge decks sitting a few units
    /// under their own abutment, which is how movers ended up walking down the
    /// terrain underneath a bridge instead of across it. Height is not a
    /// membership signal: the nav data decides membership by 2D cells and edge
    /// adjacency and keeps height in a separate field.
    ///
    /// What bounds this is [`best_object_surface`]'s candidacy rule
    /// ([`STAND_TOLERANCE`]) — an object has to have a surface near the mover's
    /// feet to be considered at all.
    fn surface_at(
        &self,
        world_xz: Vec2,
        reference_y: f32,
        exited: Option<(Entity, (Vec3, Vec3))>,
    ) -> Option<(f32, NavLocation)> {
        if let Some(object) = self.best_object_surface(world_xz, reference_y, exited) {
            if let Some(height) = self.object_surface_height(object, world_xz, reference_y) {
                return Some((height, NavLocation::OnObject { object }));
            }
        }

        self.surface_height(world_xz)
            .map(|height| (height, NavLocation::Terrain))
    }

    /// What a movement segment does to one object's nav mesh: hits a wall,
    /// leaves through an outline edge, or stays inside. `None` when the
    /// object's meshes aren't loaded.
    ///
    /// Everything is computed in object-local space: the instance transform's
    /// X mirror would invert the side tests in world space.
    fn object_crossing(&self, object: Entity, from: Vec3, to_xz: Vec2) -> Option<ObjectCrossing> {
        let (_, object_nav, transform) = self.objects.get(object).ok()?;

        let inverse = transform.affine().inverse();
        let a = inverse.transform_point3(from);
        let b = inverse.transform_point3(Vec3::new(to_xz.x, from.y, to_xz.y));
        let a = Vec2::new(a.x, a.z);
        let b = Vec2::new(b.x, b.z);

        let mut loaded = false;
        let mut exits: Option<(Vec3, Vec3)> = None;

        for handle in &object_nav.0 {
            let Some(bms) = self.bms_meshes.get(handle) else {
                continue;
            };
            let Some(nav) = &bms.navmesh else { continue };
            loaded = true;

            let outline = nav.outline_edges.iter().map(|edge| (edge, true));
            let inline = nav.inline_edges.iter().map(|edge| (edge, false));
            for (edge, is_outline) in outline.chain(inline) {
                let v0 = nav.vertices[edge.src_vertex as usize].position;
                let v1 = nav.vertices[edge.dst_vertex as usize].position;

                // An object's mesh can span several storeys in the same XZ
                // area; only edges on the mover's own level apply.
                let edge_y =
                    0.5 * (transform.transform_point(v0).y + transform.transform_point(v1).y);
                if (edge_y - from.y).abs() > OBJECT_BLOCK_Y_BAND {
                    continue;
                }

                let probe = EdgeProbe {
                    src: Vec2::new(v0.x, v0.z),
                    dst: Vec2::new(v1.x, v1.z),
                    flag: &edge.flag,
                };
                match classify_crossing(a, b, &probe) {
                    Some(EdgeCross::Blocked) => return Some(ObjectCrossing::Blocked),
                    // Crossing the border of the walkable area outwards.
                    Some(EdgeCross::Passable) if is_outline => {
                        // An Underpass outline edge is the fall-off boundary of
                        // an elevated walkable surface — a bridge railing, a
                        // stair side, a wall top, a balcony edge. Its flag means
                        // "passthrough from outside, blocked from inside", and
                        // `object_crossing` only ever runs for the object the
                        // mover is standing *on*: the whole walkable area lies
                        // inside its outline, so crossing one is always a step
                        // off the inside. Block it instead of letting the mover
                        // walk off into the drop underneath.
                        //
                        // Measured across the corpus: all 626 Underpass outline
                        // edges are one-sided boundaries (dst_cell = NO_CELL),
                        // and every carrier is a bridge / stair / wall / balcony
                        // — none is a doorway. Entry is unaffected (the terrain
                        // path blocks only `is_blocked()` edges, preserving
                        // "passthrough from outside"), and unmarked and Global
                        // outline edges still let the mover walk off at the
                        // bridge and stair *ends*, so this cannot trap anyone.
                        if edge.flag.is_underpass() {
                            return Some(ObjectCrossing::Blocked);
                        }
                        // Not gated on the Global flag: an unmarked outline edge
                        // must still let the mover walk off, or stairs become
                        // traps.
                        exits =
                            Some((transform.transform_point(v0), transform.transform_point(v1)));
                    }
                    _ => {}
                }
            }
        }

        if !loaded {
            return None;
        }
        Some(match exits {
            Some(edge) => ObjectCrossing::Exits { edge },
            None => ObjectCrossing::Stays,
        })
    }

    /// The object the step walks *into*, and its surface height at the
    /// destination.
    ///
    /// This is the counterpart to [`object_crossing`](Self::object_crossing),
    /// which only ever looks at the object the mover already stands on. Entry
    /// used to be decided purely by height proximity
    /// ([`best_object_surface`](Self::best_object_surface)), which meant any
    /// deck more than [`STAND_TOLERANCE`] off the mover's feet was invisible —
    /// movers walked under bridges instead of onto them.
    ///
    /// A passable outline crossing alone isn't enough to conclude the mover
    /// went *in*: the same edge is crossed on the way out, and a segment can
    /// clip a corner. Asking whether a walkable triangle covers the
    /// destination settles it, and avoids leaning on `NavEdge::src_cell` /
    /// `dst_cell` side semantics — the same reasoning that made blocking
    /// non-directional (see [`edges`]).
    ///
    /// Ranking matches [`best_object_surface`](Self::best_object_surface): an
    /// object whose outline meets the seam just walked off wins outright,
    /// otherwise the surface nearest the feet does. Height only *ranks* here —
    /// it never rejects a candidate, because the crossing already decided
    /// membership.
    fn object_entered(
        &self,
        from: Vec3,
        to_xz: Vec2,
        exited: Option<(Entity, (Vec3, Vec3))>,
    ) -> Option<(Entity, f32)> {
        // (meets the exited seam, vertical distance from the feet, entity, height)
        let mut best: Option<(bool, f32, Entity, f32)> = None;

        for (entity, object_nav, transform) in self.objects_over_segment(from.xz(), to_xz) {
            // Leaving an object must not immediately re-enter it through the
            // very edge just crossed.
            if exited.is_some_and(|(exited_entity, _)| exited_entity == entity) {
                continue;
            }
            if !self.passable_outline_crossed(object_nav, transform, from, to_xz) {
                continue;
            }
            let Some(height) = self.object_surface_height(entity, to_xz, from.y) else {
                continue;
            };

            let meets_seam = exited
                .is_some_and(|(_, edge)| self.object_has_outline_edge(object_nav, transform, edge));

            // Height ranks candidates but does not gate them. Crossing the
            // outline *is* the membership test — the nav data says so: terrain
            // cells and edges are strictly 2D with height in a separate field,
            // and object membership is edge adjacency (`NavEdge::src_cell` /
            // `dst_cell`, `LinkEdge`), never a distance comparison. Gating on
            // height here is what made a bridge deck authored a few units under
            // its own abutment unenterable.
            //
            // What keeps an overpass out is the storey filter on the outline
            // edge itself (see `passable_outline_crossed`): a deck a full level
            // up is not a door you can walk through in XZ.
            let gap = (height - from.y).abs();
            let better =
                best.is_none_or(
                    |(best_seam, best_gap, _, _)| match (meets_seam, best_seam) {
                        (true, false) => true,
                        (false, true) => false,
                        _ => gap < best_gap,
                    },
                );
            if better {
                best = Some((meets_seam, gap, entity, height));
            }
        }

        best.map(|(_, _, entity, height)| (entity, height))
    }

    /// Does the step cross one of this object's *passable* outline edges — the
    /// border of its walkable area? Object-local throughout, for the same
    /// reason as [`blocked_edge_crossed`](Self::blocked_edge_crossed): the
    /// instance transform's X mirror would invert the side tests in world
    /// space.
    ///
    /// Inline edges are deliberately not scanned: they sit *inside* the
    /// walkable area and say nothing about entering it.
    fn passable_outline_crossed(
        &self,
        object_nav: &ObjectNavMesh,
        transform: &GlobalTransform,
        from: Vec3,
        to_xz: Vec2,
    ) -> bool {
        let inverse = transform.affine().inverse();
        let a = inverse.transform_point3(from);
        let b = inverse.transform_point3(Vec3::new(to_xz.x, from.y, to_xz.y));
        let (a, b) = (Vec2::new(a.x, a.z), Vec2::new(b.x, b.z));

        for handle in &object_nav.0 {
            let Some(bms) = self.bms_meshes.get(handle) else {
                continue;
            };
            let Some(nav) = &bms.navmesh else { continue };

            let pad = 5.0;
            let (seg_min, seg_max) = (a.min(b), a.max(b));
            if seg_max.x < nav.bounds.0.x - pad
                || seg_min.x > nav.bounds.1.x + pad
                || seg_max.y < nav.bounds.0.z - pad
                || seg_min.y > nav.bounds.1.z + pad
            {
                continue;
            }

            for edge in &nav.outline_edges {
                let v0 = nav.vertices[edge.src_vertex as usize].position;
                let v1 = nav.vertices[edge.dst_vertex as usize].position;

                // Only borders on the mover's own storey are entrances; an
                // overhead deck's outline is not a door from the ground.
                let edge_y =
                    0.5 * (transform.transform_point(v0).y + transform.transform_point(v1).y);
                if (edge_y - from.y).abs() > OBJECT_BLOCK_Y_BAND {
                    continue;
                }

                let probe = EdgeProbe {
                    src: Vec2::new(v0.x, v0.z),
                    dst: Vec2::new(v1.x, v1.z),
                    flag: &edge.flag,
                };
                if classify_crossing(a, b, &probe) == Some(EdgeCross::Passable) {
                    return true;
                }
            }
        }
        false
    }

    /// Height of one specific object's walkable surface under `world_xz`.
    fn object_surface_height(
        &self,
        object: Entity,
        world_xz: Vec2,
        reference_y: f32,
    ) -> Option<f32> {
        let (_, object_nav, transform) = self.objects.get(object).ok()?;

        let probe_origin = Vec3::new(world_xz.x, reference_y + HEIGHT_PROBE_RANGE, world_xz.y);
        let inverse = transform.affine().inverse();
        let local_origin = inverse.transform_point3(probe_origin);
        let local_dir = inverse.transform_vector3(Vec3::NEG_Y);

        let mut best: Option<f32> = None;
        for handle in &object_nav.0 {
            let Some(bms) = self.bms_meshes.get(handle) else {
                continue;
            };
            let Some(nav) = &bms.navmesh else { continue };

            let pad = Vec3::splat(5.0);
            if !ray_hits_aabb(
                local_origin,
                local_dir,
                nav.bounds.0 - pad,
                nav.bounds.1 + pad,
            ) {
                continue;
            }

            for cell in &nav.cells {
                let (a, b, c) = nav.cell_triangle(cell);
                let Some(t) = ray_triangle_intersection(local_origin, local_dir, a, b, c) else {
                    continue;
                };
                let height = transform.transform_point(local_origin + local_dir * t).y;
                best = Some(pick_step_height(best, height, reference_y));
            }
        }
        best
    }

    /// Whether a movement segment crosses a blocked *terrain* edge.
    fn terrain_blocked(&self, from_xz: Vec2, to_xz: Vec2) -> bool {
        let seg_min = from_xz.min(to_xz);
        let seg_max = from_xz.max(to_xz);

        for (_, nav_mesh_data, transform) in self.regions.iter() {
            let Some(nav_mesh) = self.nav_meshes.get(&nav_mesh_data.0) else {
                continue;
            };
            let origin = transform.translation();

            if seg_max.x < origin.x - REGION_SIZE
                || seg_min.x > origin.x
                || seg_max.y < origin.z
                || seg_min.y > origin.z + REGION_SIZE
            {
                continue;
            }

            let a = to_local(origin, Vec3::new(from_xz.x, 0.0, from_xz.y));
            let b = to_local(origin, Vec3::new(to_xz.x, 0.0, to_xz.y));
            let local_min = a.min(b);
            let local_max = a.max(b);

            let terrain_edges = nav_mesh
                .global_edge_list
                .0
                .iter()
                .map(|e| (&e.flag, e.line))
                .chain(
                    nav_mesh
                        .internal_edge_list
                        .0
                        .iter()
                        .map(|e| (&e.flag, e.line)),
                );

            for (flag, line) in terrain_edges {
                if !flag.is_blocked() {
                    continue;
                }
                if local_max.x < line.0.x.min(line.1.x)
                    || local_min.x > line.0.x.max(line.1.x)
                    || local_max.y < line.0.y.min(line.1.y)
                    || local_min.y > line.0.y.max(line.1.y)
                {
                    continue;
                }
                let probe = EdgeProbe {
                    src: line.0,
                    dst: line.1,
                    flag,
                };
                if classify_crossing(a, b, &probe) == Some(EdgeCross::Blocked) {
                    return true;
                }
            }
        }
        false
    }

    /// The object nav mesh the mover is currently standing on, if any: the
    /// object whose walkable surface under `world_xz` sits closest to the
    /// mover's feet, within [`STAND_TOLERANCE`].
    ///
    /// This is the (stateless) stand-in for tracking a mover's surface: it
    /// answers "am I on a bridge deck right now" so the terrain underneath can
    /// be ignored. It resolves per call, so it costs a downward probe against
    /// every loaded nav object — acceptable while movement queries are the only
    /// caller, and superseded by a tracked `NavLocation`.
    fn standing_on_object(&self, world_xz: Vec2, reference_y: f32) -> Option<Entity> {
        self.best_object_surface(world_xz, reference_y, None)
    }

    /// The object whose walkable surface under `world_xz` the mover belongs on.
    ///
    /// Candidates are objects with a surface within [`STAND_TOLERANCE`] of
    /// `reference_y`. Ranking: an object whose own outline meets the edge just
    /// walked off wins outright — that seam is a definite hand-off, whereas
    /// height proximity is only a guess — and among equals the surface nearest
    /// the mover's feet wins.
    ///
    /// `exited` carries the object that edge belongs to, which is excluded from
    /// the seam bonus: it trivially matches its own edge.
    fn best_object_surface(
        &self,
        world_xz: Vec2,
        reference_y: f32,
        exited: Option<(Entity, (Vec3, Vec3))>,
    ) -> Option<Entity> {
        let probe_origin = Vec3::new(world_xz.x, reference_y + STAND_TOLERANCE, world_xz.y);
        // (meets the exited seam, vertical distance from the feet, entity)
        let mut best: Option<(bool, f32, Entity)> = None;

        for (entity, object_nav, transform) in self.objects_near(world_xz) {
            let inverse = transform.affine().inverse();
            let local_origin = inverse.transform_point3(probe_origin);
            let local_dir = inverse.transform_vector3(Vec3::NEG_Y);

            let mut gap: Option<f32> = None;
            for handle in &object_nav.0 {
                let Some(bms) = self.bms_meshes.get(handle) else {
                    continue;
                };
                let Some(nav) = &bms.navmesh else { continue };

                let pad = Vec3::splat(5.0);
                if !ray_hits_aabb(
                    local_origin,
                    local_dir,
                    nav.bounds.0 - pad,
                    nav.bounds.1 + pad,
                ) {
                    continue;
                }

                for cell in &nav.cells {
                    let (a, b, c) = nav.cell_triangle(cell);
                    let Some(t) = ray_triangle_intersection(local_origin, local_dir, a, b, c)
                    else {
                        continue;
                    };
                    let surface_y = transform.transform_point(local_origin + local_dir * t).y;
                    let candidate = (surface_y - reference_y).abs();
                    if candidate <= STAND_TOLERANCE && gap.is_none_or(|g| candidate < g) {
                        gap = Some(candidate);
                    }
                }
            }

            let Some(gap) = gap else { continue };

            // Only worth asking once the object is a candidate at all, and
            // never of the object the mover is leaving.
            let meets_seam = exited.is_some_and(|(exited_entity, edge)| {
                exited_entity != entity && self.object_has_outline_edge(object_nav, transform, edge)
            });

            let better =
                best.is_none_or(|(best_seam, best_gap, _)| match (meets_seam, best_seam) {
                    (true, false) => true,
                    (false, true) => false,
                    _ => gap < best_gap,
                });
            if better {
                best = Some((meets_seam, gap, entity));
            }
        }

        best.map(|(_, _, entity)| entity)
    }

    /// Whether any of this object's outline edges coincides with `edge` in
    /// world space — i.e. the two objects meet at that seam.
    fn object_has_outline_edge(
        &self,
        object_nav: &ObjectNavMesh,
        transform: &GlobalTransform,
        edge: (Vec3, Vec3),
    ) -> bool {
        object_nav.0.iter().any(|handle| {
            let Some(bms) = self.bms_meshes.get(handle) else {
                return false;
            };
            let Some(nav) = &bms.navmesh else {
                return false;
            };
            nav.outline_edges.iter().any(|candidate| {
                let v0 = nav.vertices[candidate.src_vertex as usize].position;
                let v1 = nav.vertices[candidate.dst_vertex as usize].position;
                edges_coincide(
                    (transform.transform_point(v0), transform.transform_point(v1)),
                    edge,
                )
            })
        })
    }

    /// Nav mesh surface height (world Y) at a world-space XZ position, from
    /// the loaded region containing it. `None` if no loaded region covers the
    /// position (or its nav mesh asset hasn't finished loading).
    pub fn surface_height(&self, world_xz: Vec2) -> Option<f32> {
        for (_, nav_mesh_data, transform) in self.regions.iter() {
            let origin = transform.translation();
            let local = Vec2::new(origin.x - world_xz.x, world_xz.y - origin.z);
            if !(0.0..=REGION_SIZE).contains(&local.x) || !(0.0..=REGION_SIZE).contains(&local.y) {
                continue;
            }
            return self
                .nav_meshes
                .get(&nav_mesh_data.0)
                // Not the raw terrain height: over a frozen lake the surface is
                // the ice sheet, so a mover stands on it rather than on the bed
                // underneath (see `JMXVNVM::walkable_height_at`).
                .map(|nav_mesh| origin.y + nav_mesh.walkable_height_at(local.x, local.y));
        }
        None
    }
}

/// What a movement segment does to the object nav mesh it is tested against.
enum ObjectCrossing {
    /// Hit a wall on this object.
    Blocked,
    /// Left the object's walkable area through an outline edge, carrying that
    /// edge's world-space endpoints so the transfer can prefer an object
    /// meeting the mover at the same seam (see [`edges_coincide`]).
    Exits { edge: (Vec3, Vec3) },
    /// Stayed inside.
    Stays,
}

/// How close two outline-edge endpoints must be, in world units, to count as
/// the same seam. Parts of one compound object are authored to meet exactly,
/// so this only has to absorb the f32 error of two transform round trips.
const SEAM_EPSILON: f32 = 0.5;

/// Whether two world-space outline edges describe the same seam, in either
/// orientation.
///
/// Two objects meeting here is exactly the relation the `.nvm` `LinkEdge` table
/// encodes (a Global edge of one instance linked to a Global edge of another),
/// recovered from geometry instead — which also covers the object↔terrain case
/// `LinkEdge` has no entry for.
fn edges_coincide(a: (Vec3, Vec3), b: (Vec3, Vec3)) -> bool {
    let same = |p: Vec3, q: Vec3| p.distance_squared(q) <= SEAM_EPSILON * SEAM_EPSILON;
    (same(a.0, b.0) && same(a.1, b.1)) || (same(a.0, b.1) && same(a.1, b.0))
}

/// Is `candidate` a better surface to stand on than `current`, for a mover
/// whose feet are at `reference_y`?
///
/// Prefers the *highest* surface still within [`MAX_STEP_UP`] of the feet: that
/// climbs stairs and step objects even where the terrain keeps running
/// underneath them (whose height stays near the feet and would otherwise always
/// win), while a full storey up — an overhead bridge deck — stays out of reach
/// so the mover keeps to the ground below it. With nothing in stepping range
/// (the mover is below everything) the nearest surface wins.
fn prefer_height(current: f32, candidate: f32, reference_y: f32) -> bool {
    let reachable = candidate - reference_y <= MAX_STEP_UP;
    let current_reachable = current - reference_y <= MAX_STEP_UP;
    match (reachable, current_reachable) {
        (true, true) => candidate > current,
        (true, false) => true,
        (false, true) => false,
        (false, false) => (candidate - reference_y).abs() < (current - reference_y).abs(),
    }
}

/// [`prefer_height`] folded over an running best.
fn pick_step_height(current: Option<f32>, candidate: f32, reference_y: f32) -> f32 {
    match current {
        Some(current) if !prefer_height(current, candidate, reference_y) => current,
        _ => candidate,
    }
}

/// World position → region-local nav mesh coordinates. Regions are spawned at
/// `x * -REGION_SIZE` with X-mirrored meshes (see terrain/mod.rs), so world X is
/// negated relative to the region origin; both axes land in `[0, REGION_SIZE]`.
fn to_local(origin: Vec3, world: Vec3) -> Vec2 {
    Vec2::new(origin.x - world.x, world.z - origin.z)
}

/// Nearest crossing of `ray` with one region's *walkable* surface, as (distance
/// along ray, region-local hit position). Marches the ray through the region's
/// XZ footprint and bisects the first above→below transition.
///
/// The surface is `walkable_height_at`, not the raw terrain: over a frozen lake
/// the cursor must land on the ice sheet, not on the bed underneath it — a click
/// there was resolving to the terrain point the ray reached *through* the ice,
/// so the player walked to the wrong XZ.
fn raycast_height_map(ray: &Ray3d, origin: Vec3, nav_mesh: &JMXVNVM) -> Option<(f32, Vec2)> {
    let (t_enter, t_exit) = region_footprint_range(ray, origin)?;

    let above = |t: f32| {
        let p = ray.origin + *ray.direction * t;
        let local = to_local(origin, p);
        p.y > nav_mesh.walkable_height_at(local.x, local.y)
    };

    // A hit is a transition from above the surface to below it; a ray starting
    // below the terrain (underground) can't hit its top side.
    if !above(t_enter) {
        return None;
    }

    let mut t_above = t_enter;
    let mut t = t_enter;
    loop {
        t = (t + MARCH_STEP).min(t_exit);
        if above(t) {
            if t >= t_exit {
                return None;
            }
            t_above = t;
            continue;
        }

        // Crossed the surface between t_above and t: bisect.
        let mut lo = t_above;
        let mut hi = t;
        for _ in 0..REFINE_ITERATIONS {
            let mid = (lo + hi) / 2.0;
            if above(mid) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let t_hit = (lo + hi) / 2.0;
        let local = to_local(origin, ray.origin + *ray.direction * t_hit);
        return Some((t_hit, local));
    }
}

/// Nearest intersection of `ray` with one object instance's walkable nav mesh
/// triangles, in world space. The returned `surface` holds a placeholder
/// entity — the caller fills in the real one.
fn raycast_object_nav_mesh(
    ray: &Ray3d,
    transform: &GlobalTransform,
    nav: &BmsNavMesh,
) -> Option<NavMeshHit> {
    let inverse: Affine3A = transform.affine().inverse();
    let local_origin = inverse.transform_point3(ray.origin);
    let local_dir = inverse.transform_vector3(*ray.direction);

    // Cheap reject against the nav mesh's own bounding box (object-local
    // space), padded only for f32 slop. Not the visual mesh box
    // (`bms.bounding_box`): the nav mesh routinely reaches past the drawn
    // geometry, so culling against the visual box dropped reachable ground.
    let pad = Vec3::splat(5.0);
    if !ray_hits_aabb(
        local_origin,
        local_dir,
        nav.bounds.0 - pad,
        nav.bounds.1 + pad,
    ) {
        return None;
    }

    let mut best: Option<(f32, Vec3)> = None; // (local t, unnormalized local normal)
    for cell in &nav.cells {
        let (a, b, c) = nav.cell_triangle(cell);
        let Some(t) = ray_triangle_intersection(local_origin, local_dir, a, b, c) else {
            continue;
        };
        if best.map_or(true, |(best_t, _)| t < best_t) {
            best = Some((t, (b - a).cross(c - a)));
        }
    }
    let (t, local_normal) = best?;

    let point = transform.transform_point(local_origin + local_dir * t);
    let distance = point.distance(ray.origin);
    if distance > MAX_CAST_DISTANCE {
        return None;
    }
    // The instance transforms are yaw + translation + an X mirror, so plain
    // vector transform is fine for normals — but the mirror flips winding, so
    // force the walkable surface to face up.
    let mut normal = transform
        .affine()
        .transform_vector3(local_normal)
        .normalize_or_zero();
    if normal.y < 0.0 {
        normal = -normal;
    }

    Some(NavMeshHit {
        point,
        distance,
        normal,
        surface: NavMeshSurface::Object {
            object: Entity::PLACEHOLDER,
        },
    })
}

/// Slab test: does the ray (origin, dir) pass through the AABB, at t >= 0?
fn ray_hits_aabb(origin: Vec3, dir: Vec3, min: Vec3, max: Vec3) -> bool {
    let mut t_enter = 0.0_f32;
    let mut t_exit = f32::INFINITY;
    for axis in 0..3 {
        if dir[axis].abs() < 1e-8 {
            if origin[axis] < min[axis] || origin[axis] > max[axis] {
                return false;
            }
            continue;
        }
        let t0 = (min[axis] - origin[axis]) / dir[axis];
        let t1 = (max[axis] - origin[axis]) / dir[axis];
        let (t0, t1) = if t0 <= t1 { (t0, t1) } else { (t1, t0) };
        t_enter = t_enter.max(t0);
        t_exit = t_exit.min(t1);
        if t_enter > t_exit {
            return false;
        }
    }
    true
}

/// Möller–Trumbore without backface culling (instance transforms mirror X,
/// which flips triangle winding). Returns the distance along `dir`.
fn ray_triangle_intersection(origin: Vec3, dir: Vec3, v0: Vec3, v1: Vec3, v2: Vec3) -> Option<f32> {
    let e0 = v1 - v0;
    let e1 = v2 - v0;
    let h = dir.cross(e1);
    let det = e0.dot(h);
    if det.abs() < 1e-8 {
        return None;
    }
    let inv_det = 1.0 / det;
    let s = origin - v0;
    let u = inv_det * s.dot(h);
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e0);
    let v = inv_det * dir.dot(q);
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = inv_det * e1.dot(q);
    (t > 1e-4).then_some(t)
}

/// Range of distances along `ray` that lie over the region's XZ footprint
/// (world X in `[origin.x - REGION_SIZE, origin.x]`, world Z in
/// `[origin.z, origin.z + REGION_SIZE]`), clipped to `[0, MAX_CAST_DISTANCE]`.
fn region_footprint_range(ray: &Ray3d, origin: Vec3) -> Option<(f32, f32)> {
    let mut t_enter = 0.0_f32;
    let mut t_exit = MAX_CAST_DISTANCE;

    let slabs = [
        (
            ray.origin.x,
            ray.direction.x,
            origin.x - REGION_SIZE,
            origin.x,
        ),
        (
            ray.origin.z,
            ray.direction.z,
            origin.z,
            origin.z + REGION_SIZE,
        ),
    ];
    for (start, dir, min, max) in slabs {
        if dir.abs() < 1e-8 {
            if start < min || start > max {
                return None;
            }
            continue;
        }
        let (t0, t1) = ((min - start) / dir, (max - start) / dir);
        let (t0, t1) = if t0 <= t1 { (t0, t1) } else { (t1, t0) };
        t_enter = t_enter.max(t0);
        t_exit = t_exit.min(t1);
        if t_enter > t_exit {
            return None;
        }
    }

    Some((t_enter, t_exit))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::bms::navmesh::{NavCell, NavEdge, NavVertex, OutlineLookupGrid};
    use crate::assets::bms::vertex::VertexData;
    use crate::assets::nvm::nav_mesh_edge_flag::NavMeshEdgeFlag;

    /// `NavEdge::src_cell`/`dst_cell` sentinel for "no cell on this side".
    const NO_CELL: u16 = 0xFFFF;
    const BLOCKED: u8 = 3;
    const PASSABLE: u8 = 0;
    /// `Underpass` (bit 4): the fall-off boundary of an elevated walkable
    /// surface — a bridge railing, a stair side. Blocked from the inside only.
    const UNDERPASS: u8 = 16;

    /// A flat 40x40 deck at object-local y = 0, as two triangles, ringed by four
    /// outline edges. `blocked_side` marks the +x border as a wall so both the
    /// "walk off" and "walk into a wall" cases can be driven from one mesh.
    ///
    /// Vertex layout (local XZ), triangles (0,1,2) and (0,2,3):
    /// ```text
    ///   3 ---- 2      -z
    ///   |    / |      ^
    ///   |  /   |      |
    ///   0 ---- 1      +---> +x
    /// ```
    fn deck_navmesh(blocked_side: bool) -> BmsNavMesh {
        let corner = |x: f32, z: f32| NavVertex {
            position: Vec3::new(x, 0.0, z),
            bisector_index: 0,
        };
        let edge = |src: u16, dst: u16, flag: u8| NavEdge {
            src_vertex: src,
            dst_vertex: dst,
            // Outline edges have walkable area on one side only.
            src_cell: 0,
            dst_cell: NO_CELL,
            flag: NavMeshEdgeFlag(flag),
            event_zone: None,
        };

        BmsNavMesh {
            vertices: vec![
                corner(-20.0, -20.0),
                corner(20.0, -20.0),
                corner(20.0, 20.0),
                corner(-20.0, 20.0),
            ],
            cells: vec![
                NavCell {
                    vertices: [0, 1, 2],
                    flag: 0,
                    event_zone: None,
                },
                NavCell {
                    vertices: [0, 2, 3],
                    flag: 0,
                    event_zone: None,
                },
            ],
            outline_edges: vec![
                // +x border: the wall under test.
                edge(1, 2, if blocked_side { BLOCKED } else { PASSABLE }),
                // The other three borders are plain walk-off boundaries.
                edge(2, 3, PASSABLE),
                edge(3, 0, PASSABLE),
                edge(0, 1, PASSABLE),
            ],
            inline_edges: vec![NavEdge {
                src_vertex: 0,
                dst_vertex: 2,
                src_cell: 0,
                dst_cell: 1,
                flag: NavMeshEdgeFlag(PASSABLE),
                event_zone: None,
            }],
            events: Vec::new(),
            outline_lookup: OutlineLookupGrid {
                origin: Vec2::ZERO,
                width: 0,
                height: 0,
                cells: Vec::new(),
            },
            // Matches the corner vertices above.
            bounds: (Vec3::new(-20.0, 0.0, -20.0), Vec3::new(20.0, 0.0, 20.0)),
        }
    }

    fn deck_bms(blocked_side: bool) -> JMXVBMS {
        JMXVBMS {
            name: "test deck".to_string(),
            vertex_data: VertexData {
                vertices: Vec::new(),
                lightmap_path: None,
                new_vertex_data: None,
            },
            indices: Vec::new(),
            bounding_box: (Vec3::new(-20.0, 0.0, -20.0), Vec3::new(20.0, 0.0, 20.0)),
            navmesh: Some(deck_navmesh(blocked_side)),
            material: String::new(),
            bone_data: None,
        }
    }

    /// App with the asset collections `NavMeshRaycast` needs, plus one deck
    /// object placed with the real map-object instance transform: an X mirror
    /// (`scale.x = -1`) combined with a yaw, which is what makes world-space
    /// side tests unsafe (see `nav::edges`).
    ///
    /// The deck sits at world y = `deck_y`; no terrain region is spawned, so
    /// `surface_height` finds nothing and the terrain path yields `Unknown` —
    /// which is exactly what the bridge test needs to distinguish.
    fn app_with_deck(deck_y: f32, yaw: f32, blocked_side: bool) -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
        ))
        .init_asset::<JMXVNVM>()
        .init_asset::<JMXVBMS>();

        let handle = app
            .world_mut()
            .resource_mut::<Assets<JMXVBMS>>()
            .add(deck_bms(blocked_side));

        let transform = Transform::from_matrix(Mat4::from_scale_rotation_translation(
            Vec3::new(-1.0, 1.0, 1.0),
            Quat::from_rotation_y(yaw),
            Vec3::new(0.0, deck_y, 0.0),
        ));
        let object = app
            .world_mut()
            .spawn((
                ObjectNavMesh(vec![handle]),
                GlobalTransform::from(transform),
            ))
            .id();

        (app, object)
    }

    /// Runs `step` through a one-shot system so the `SystemParam` is built the
    /// same way it is in the real schedule.
    fn run_step(app: &mut App, from: Vec3, to_xz: Vec2, location: NavLocation) -> NavStep {
        let mut system =
            IntoSystem::into_system(move |nav: NavMeshRaycast| nav.step(from, to_xz, location));
        system.initialize(app.world_mut());
        system
            .run((), app.world_mut())
            .expect("nav step system ran")
    }

    fn run_resolve(app: &mut App, position: Vec3) -> NavLocation {
        let mut system =
            IntoSystem::into_system(move |nav: NavMeshRaycast| nav.resolve_location(position));
        system.initialize(app.world_mut());
        system
            .run((), app.world_mut())
            .expect("nav resolve system ran")
    }

    fn run_ground(
        app: &mut App,
        xz: Vec2,
        reference_y: f32,
        location: NavLocation,
    ) -> Option<(f32, NavLocation)> {
        let mut system = IntoSystem::into_system(move |nav: NavMeshRaycast| {
            nav.ground(xz, reference_y, location)
        });
        system.initialize(app.world_mut());
        system
            .run((), app.world_mut())
            .expect("nav ground system ran")
    }

    /// The stairs regression: a mover tracked as `Terrain` that walks under an
    /// object surface within a step of its feet must BOARD it. `ground()` used
    /// to be a one-way ratchet (terrain → object was impossible), so every
    /// remote entity — mounts included — sank through staircases.
    #[test]
    fn ground_boards_an_object_surface_within_a_step() {
        // Deck at y = 10, mover walking on "terrain" at y = 0: 10 units up is
        // within STAND_TOLERANCE (20), i.e. a step up onto a stair tread.
        let (mut app, object) = app_with_deck(10.0, 0.0, false);

        let grounded = run_ground(&mut app, Vec2::new(0.0, 0.0), 0.0, NavLocation::Terrain);

        assert_eq!(
            grounded,
            Some((10.0, NavLocation::OnObject { object })),
            "a terrain mover must board a surface within a step of its feet"
        );
    }

    /// The bound on that: a deck far overhead (a bridge you are walking under)
    /// must NOT capture the mover. With no terrain region spawned there is no
    /// surface left to report, which is exactly how we tell "did not board"
    /// apart from "boarded".
    #[test]
    fn ground_does_not_board_a_deck_overhead() {
        let (mut app, _object) = app_with_deck(100.0, 0.0, false);

        let grounded = run_ground(&mut app, Vec2::new(0.0, 0.0), 0.0, NavLocation::Terrain);

        assert_eq!(
            grounded, None,
            "a deck {} units up is out of STAND_TOLERANCE and must be ignored",
            100
        );
    }

    /// Walking off a deck with nothing under it reports no surface rather than
    /// freezing at the old height — the `or_else` re-resolution path.
    #[test]
    fn ground_leaves_an_object_when_its_mesh_ends() {
        let (mut app, object) = app_with_deck(100.0, 0.0, false);

        // Still on the deck (the mesh spans ±20 in local XZ).
        assert_eq!(
            run_ground(
                &mut app,
                Vec2::new(0.0, 10.0),
                100.0,
                NavLocation::OnObject { object }
            ),
            Some((100.0, NavLocation::OnObject { object })),
        );
        // Past its edge: no terrain in this world, so nothing is found.
        assert_eq!(
            run_ground(
                &mut app,
                Vec2::new(0.0, 400.0),
                100.0,
                NavLocation::OnObject { object }
            ),
            None,
        );
    }

    /// The bridge regression: a mover on a deck keeps walking along it and stays
    /// `OnObject`. No terrain exists in this world at all, so if the step were
    /// resolved against the terrain it could only come back `Unknown`.
    #[test]
    fn walks_along_a_deck_without_consulting_terrain() {
        let (mut app, object) = app_with_deck(100.0, 0.0, false);

        let from = Vec3::new(0.0, 100.0, 0.0);
        let step = run_step(
            &mut app,
            from,
            Vec2::new(0.0, 10.0),
            NavLocation::OnObject { object },
        );

        match step {
            NavStep::Moved { position, location } => {
                assert_eq!(location, NavLocation::OnObject { object });
                assert!(
                    (position.y - 100.0).abs() < 1e-3,
                    "expected to stay on the deck at y=100, got {}",
                    position.y
                );
            }
            other => panic!("expected to stay on the deck, got {other:?}"),
        }
    }

    /// A blocked outline edge stops the step instead of transferring — and the
    /// identical walk over the same border with the flag cleared does not, so
    /// the assertion is about the flag rather than about the geometry.
    #[test]
    fn blocked_outline_edge_stops_the_mover() {
        // Local +x is world -x under the mirror, so walk towards world -x.
        let from = Vec3::new(0.0, 100.0, 0.0);
        let to = Vec2::new(-30.0, 0.0);

        let (mut app, object) = app_with_deck(100.0, 0.0, true);
        assert_eq!(
            run_step(&mut app, from, to, NavLocation::OnObject { object }),
            NavStep::Blocked
        );

        let (mut app, object) = app_with_deck(100.0, 0.0, false);
        assert_ne!(
            run_step(&mut app, from, to, NavLocation::OnObject { object }),
            NavStep::Blocked,
            "an unblocked border must let the mover walk off"
        );
    }

    /// An `Underpass` outline edge is a railing: standing on the deck, the
    /// mover cannot walk off over it, even though it carries no block bits.
    ///
    /// This is the bridge report — a mover on `ruin_takla_tembrig_01` walked
    /// straight off the side railings onto the terrain hundreds of units below.
    /// Measured, the railing edges are all `Underpass`; the fix blocks a
    /// crossing of one from the surface the mover stands on. A plain passable
    /// border on the same deck must still let the mover walk off, so the fix
    /// can't be "block every outline edge".
    #[test]
    fn underpass_outline_edge_holds_the_mover_on_the_deck() {
        // Local +x is world -x under the mirror, so walk towards world -x.
        let from = Vec3::new(0.0, 100.0, 0.0);
        let to = Vec2::new(-30.0, 0.0);

        // Mark the +x border (outline edge 0) Underpass; the other three borders
        // stay plain passable walk-off edges.
        let mut bms = deck_bms(false);
        bms.navmesh.as_mut().unwrap().outline_edges[0].flag = NavMeshEdgeFlag(UNDERPASS);

        let mut app = App::new();
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
        ))
        .init_asset::<JMXVNVM>()
        .init_asset::<JMXVBMS>();
        let handle = app.world_mut().resource_mut::<Assets<JMXVBMS>>().add(bms);
        let transform = Transform::from_matrix(Mat4::from_scale_rotation_translation(
            Vec3::new(-1.0, 1.0, 1.0),
            Quat::IDENTITY,
            Vec3::new(0.0, 100.0, 0.0),
        ));
        let object = app
            .world_mut()
            .spawn((
                ObjectNavMesh(vec![handle]),
                GlobalTransform::from(transform),
            ))
            .id();

        assert_eq!(
            run_step(&mut app, from, to, NavLocation::OnObject { object }),
            NavStep::Blocked,
            "crossing an Underpass railing from the deck must be blocked",
        );

        // Walking towards local -x (world +x) crosses a plain passable border,
        // which must still let the mover leave the deck.
        assert_ne!(
            run_step(
                &mut app,
                from,
                Vec2::new(30.0, 0.0),
                NavLocation::OnObject { object }
            ),
            NavStep::Blocked,
            "a plain passable border on the same deck must still let the mover off",
        );
    }

    /// Walking off a passable outline edge leaves the object. With no terrain
    /// under it there is nothing to land on, so the step reports `Unknown` —
    /// crucially not `Blocked`, which would trap the mover.
    #[test]
    fn walking_off_a_passable_outline_edge_is_not_blocked() {
        let (mut app, object) = app_with_deck(100.0, 0.0, false);

        let step = run_step(
            &mut app,
            Vec3::new(0.0, 100.0, 0.0),
            Vec2::new(0.0, 40.0),
            NavLocation::OnObject { object },
        );
        assert_eq!(step, NavStep::Unknown);
    }

    /// A location naming a despawned object must recover, not panic: regions
    /// despawn their object children when they stream out.
    #[test]
    fn stale_object_location_recovers() {
        let (mut app, object) = app_with_deck(100.0, 0.0, false);
        app.world_mut().despawn(object);

        let step = run_step(
            &mut app,
            Vec3::new(0.0, 100.0, 0.0),
            Vec2::new(0.0, 10.0),
            NavLocation::OnObject { object },
        );
        assert_eq!(step, NavStep::Unknown);
    }

    /// Resolution is geometric: standing at deck height finds the deck, and
    /// standing far above it finds nothing (there is no terrain here).
    #[test]
    fn resolve_location_finds_the_deck_by_height() {
        let (mut app, object) = app_with_deck(100.0, 0.0, false);

        assert_eq!(
            run_resolve(&mut app, Vec3::new(0.0, 100.0, 0.0)),
            NavLocation::OnObject { object }
        );
        assert_eq!(
            run_resolve(&mut app, Vec3::new(0.0, 500.0, 0.0)),
            NavLocation::Unresolved
        );
    }

    /// Spawn a second deck into an existing test world at `deck_y`.
    fn add_deck(app: &mut App, deck_y: f32) -> Entity {
        let handle = app
            .world_mut()
            .resource_mut::<Assets<JMXVBMS>>()
            .add(deck_bms(false));
        app.world_mut()
            .spawn((
                ObjectNavMesh(vec![handle]),
                GlobalTransform::from(Transform::from_xyz(0.0, deck_y, 0.0)),
            ))
            .id()
    }

    /// The dungeon stacked-floor rule: two overlapping decks 200 units apart
    /// (jinsi-style storeys, each its own DOF block / nav object) — a mover
    /// resolves to the floor nearest its own height, in both directions.
    /// This is the mechanism that keeps a character on the upper storey from
    /// snapping down through it (and vice versa).
    #[test]
    fn stacked_decks_resolve_to_the_nearest_floor() {
        let (mut app, lower) = app_with_deck(0.0, 0.0, false);
        let upper = add_deck(&mut app, 200.0);

        assert_eq!(
            run_resolve(&mut app, Vec3::new(0.0, 5.0, 0.0)),
            NavLocation::OnObject { object: lower }
        );
        assert_eq!(
            run_resolve(&mut app, Vec3::new(0.0, 195.0, 0.0)),
            NavLocation::OnObject { object: upper }
        );
        // Between floors, out of STAND_TOLERANCE of both: no surface claims it.
        assert_eq!(
            run_resolve(&mut app, Vec3::new(0.0, 100.0, 0.0)),
            NavLocation::Unresolved
        );
    }

    /// DOF collision circles (`Flag & 2` props) veto a step on any surface:
    /// walking into or through one blocks, skirting past it doesn't.
    #[test]
    fn dungeon_collision_circles_block_steps() {
        use dungeon::{ActiveDungeonNav, CollisionCircle, DungeonNavData};

        let (mut app, object) = app_with_deck(0.0, 0.0, false);
        app.insert_resource(ActiveDungeonNav {
            region_id: 0x8001,
            data: DungeonNavData::from_circles(vec![CollisionCircle {
                center: Vec2::new(0.0, 8.0),
                radius: 3.0,
            }]),
            block_of_entity: Default::default(),
        });

        let from = Vec3::new(0.0, 0.0, 0.0);
        let location = NavLocation::OnObject { object };
        // Straight into the circle: blocked before the mesh is even asked.
        assert_eq!(
            run_step(&mut app, from, Vec2::new(0.0, 12.0), location),
            NavStep::Blocked
        );
        // Skirting past outside the radius still walks.
        assert!(matches!(
            run_step(&mut app, from, Vec2::new(8.0, 12.0), location),
            NavStep::Moved { .. }
        ));
    }

    /// The same walk under a yawed, X-mirrored instance transform. The mirror
    /// inverts every cross product, so a side test done in world space would
    /// silently flip: this pins that the object path stays in object-local
    /// space.
    #[test]
    fn mirrored_and_yawed_instance_behaves_the_same() {
        let yaw = std::f32::consts::FRAC_PI_4;
        let (mut app, object) = app_with_deck(100.0, yaw, true);

        // Walk towards the deck centre from just inside a passable border: must
        // stay on the deck regardless of the instance's orientation.
        let step = run_step(
            &mut app,
            Vec3::new(0.0, 100.0, 0.0),
            Vec2::new(1.0, 1.0),
            NavLocation::OnObject { object },
        );
        match step {
            NavStep::Moved { location, .. } => {
                assert_eq!(location, NavLocation::OnObject { object })
            }
            other => panic!("expected to stay on the deck, got {other:?}"),
        }
    }

    /// A region whose terrain is flat at `height`, spawned at the world origin.
    /// Its footprint is world x in [-1920, 0], z in [0, 1920] (see `to_local`).
    fn spawn_flat_terrain(app: &mut App, height: f32) -> Entity {
        use crate::assets::nvm::height_map::HeightMap;
        use crate::assets::nvm::nav_cell_quad::{NavCellQuad, NavCellQuadList, ObjectIndices};

        let nav = JMXVNVM {
            object_list: Default::default(),
            // Cells tile the region without overlapping, stored open-first:
            // two open strips either side of a solid one over local x
            // 1000..1200 — a wall footprint with no blocked edge fencing it.
            quad_cell_list: NavCellQuadList {
                items: vec![
                    NavCellQuad {
                        rectangle: Rect {
                            min: Vec2::ZERO,
                            max: Vec2::new(1000.0, REGION_SIZE),
                        },
                        object_indices: ObjectIndices(Vec::new()),
                    },
                    NavCellQuad {
                        rectangle: Rect {
                            min: Vec2::new(1200.0, 0.0),
                            max: Vec2::splat(REGION_SIZE),
                        },
                        object_indices: ObjectIndices(Vec::new()),
                    },
                    NavCellQuad {
                        rectangle: Rect {
                            min: Vec2::new(1000.0, 0.0),
                            max: Vec2::new(1200.0, REGION_SIZE),
                        },
                        object_indices: ObjectIndices(Vec::new()),
                    },
                ],
                open_cell_count: 2,
            },
            global_edge_list: Default::default(),
            internal_edge_list: Default::default(),
            tile_map: Default::default(),
            height_map: HeightMap::flat(height),
            plane_type_map: Default::default(),
            plane_height_map: Default::default(),
        };
        let handle = app.world_mut().resource_mut::<Assets<JMXVNVM>>().add(nav);
        app.world_mut()
            .spawn((TerrainNavMeshData(handle), GlobalTransform::IDENTITY))
            .id()
    }

    /// Solid ground stops a mover even with no blocked edge fencing it off.
    ///
    /// Blocked edges are supposed to bound solid regions, but their coverage
    /// can't be relied on — this is how movers walked into walls whose edges
    /// didn't catch them. The cells are the authoritative signal: every tile
    /// the data flags blocked belongs to a cell past `open_cell_count`.
    #[test]
    fn a_closed_terrain_cell_stops_the_mover() {
        let mut app = App::new();
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
        ))
        .init_asset::<JMXVNVM>()
        .init_asset::<JMXVBMS>();
        spawn_flat_terrain(&mut app, 0.0);

        // The solid cell spans local x 1000..1200; local x is measured back
        // from the region origin (world x = -local x here), so it covers world
        // x -1200..-1000.
        let from = Vec3::new(-980.0, 0.0, 500.0);
        assert_eq!(
            run_step(
                &mut app,
                from,
                Vec2::new(-1020.0, 500.0),
                NavLocation::Terrain
            ),
            NavStep::Blocked,
            "stepping into a closed cell must be refused"
        );

        // Stepping the other way, staying in the open cell, is fine.
        assert!(matches!(
            run_step(
                &mut app,
                from,
                Vec2::new(-940.0, 500.0),
                NavLocation::Terrain
            ),
            NavStep::Moved {
                location: NavLocation::Terrain,
                ..
            }
        ));
    }

    /// An object surface below the terrain captures the mover: the object wins
    /// whenever one resolves, and the terrain is only the fallback.
    ///
    /// This test asserted the opposite until the height competition was
    /// removed. `prefer_height` took the *higher* surface, which meant any
    /// object authored below the ground it meets was unreachable — bridge decks
    /// sitting a few units under their own abutment among them, which is the
    /// bug that forced the change.
    ///
    /// The cost is real and deliberate: a tree or building whose nav mesh sits
    /// under walkable ground now drags a mover down onto it. The only thing
    /// bounding that is [`STAND_TOLERANCE`] — the object's surface has to be
    /// within 20 units of the feet to be a candidate at all. If buried meshes
    /// turn out to capture movers in practice, the fix is a better membership
    /// signal (cell adjacency), not a height comparison: height cannot tell a
    /// sunken bridge deck from a buried tree.
    #[test]
    fn an_object_below_the_terrain_captures_the_mover() {
        let mut app = App::new();
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
        ))
        .init_asset::<JMXVNVM>()
        .init_asset::<JMXVBMS>();

        spawn_flat_terrain(&mut app, 100.0);

        // A deck 15 units below the ground, inside STAND_TOLERANCE (20).
        let handle = app
            .world_mut()
            .resource_mut::<Assets<JMXVBMS>>()
            .add(deck_bms(false));
        let object = app
            .world_mut()
            .spawn((
                ObjectNavMesh(vec![handle]),
                GlobalTransform::from(Transform::from_matrix(
                    Mat4::from_scale_rotation_translation(
                        Vec3::new(-1.0, 1.0, 1.0),
                        Quat::IDENTITY,
                        Vec3::new(-100.0, 85.0, 100.0),
                    ),
                )),
            ))
            .id();

        let from = Vec3::new(-100.0, 100.0, 100.0);
        match run_step(
            &mut app,
            from,
            Vec2::new(-105.0, 100.0),
            NavLocation::Terrain,
        ) {
            NavStep::Moved { position, location } => {
                assert_eq!(location, NavLocation::OnObject { object });
                assert!(
                    (position.y - 85.0).abs() < 1e-3,
                    "expected to stand on the deck at y=85, got {}",
                    position.y
                );
            }
            other => panic!("expected to land on the deck, got {other:?}"),
        }
    }

    /// The converse: a deck *above* the terrain but within stepping range still
    /// captures the mover, so ramps and stair treads keep working.
    #[test]
    fn a_steppable_object_above_the_terrain_still_captures_the_mover() {
        let mut app = App::new();
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
        ))
        .init_asset::<JMXVNVM>()
        .init_asset::<JMXVBMS>();

        spawn_flat_terrain(&mut app, 100.0);

        let handle = app
            .world_mut()
            .resource_mut::<Assets<JMXVBMS>>()
            .add(deck_bms(false));
        let object = app
            .world_mut()
            .spawn((
                ObjectNavMesh(vec![handle]),
                GlobalTransform::from(Transform::from_matrix(
                    Mat4::from_scale_rotation_translation(
                        Vec3::new(-1.0, 1.0, 1.0),
                        Quat::IDENTITY,
                        Vec3::new(-100.0, 110.0, 100.0),
                    ),
                )),
            ))
            .id();

        let from = Vec3::new(-100.0, 100.0, 100.0);
        match run_step(
            &mut app,
            from,
            Vec2::new(-105.0, 100.0),
            NavLocation::Terrain,
        ) {
            NavStep::Moved { position, location } => {
                assert_eq!(location, NavLocation::OnObject { object });
                assert!((position.y - 110.0).abs() < 1e-3, "got {}", position.y);
            }
            other => panic!("expected to step up onto the deck, got {other:?}"),
        }
    }

    /// A building's wall must stop a mover walking on the terrain outside it.
    ///
    /// This is the trap that made "walk into a building and get stuck": the
    /// terrain path tested only terrain edges, so movers passed straight
    /// through the wall, were then resolved onto the building's own nav mesh,
    /// and found that same outline blocking every direction back out.
    #[test]
    fn an_object_wall_blocks_a_mover_on_the_terrain() {
        let mut app = App::new();
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
        ))
        .init_asset::<JMXVNVM>()
        .init_asset::<JMXVBMS>();

        // Flat, fully walkable terrain — nothing here blocks on its own.
        spawn_flat_terrain(&mut app, 100.0);

        // A floor at ground level whose local +x border is a wall. Under the
        // mirror that border lands at world x = -120, with the floor spanning
        // world x in [-120, -80].
        let handle = app
            .world_mut()
            .resource_mut::<Assets<JMXVBMS>>()
            .add(deck_bms(true));
        app.world_mut().spawn((
            ObjectNavMesh(vec![handle]),
            GlobalTransform::from(Transform::from_matrix(
                Mat4::from_scale_rotation_translation(
                    Vec3::new(-1.0, 1.0, 1.0),
                    Quat::IDENTITY,
                    Vec3::new(-100.0, 100.0, 100.0),
                ),
            )),
        ));

        // Walk from outside (x = -130) towards the wall at x = -120.
        let from = Vec3::new(-130.0, 100.0, 100.0);
        assert_eq!(
            run_step(
                &mut app,
                from,
                Vec2::new(-110.0, 100.0),
                NavLocation::Terrain
            ),
            NavStep::Blocked,
            "the building wall must keep the mover out"
        );

        // ...and walking away from it is still fine.
        assert!(matches!(
            run_step(
                &mut app,
                from,
                Vec2::new(-140.0, 100.0),
                NavLocation::Terrain
            ),
            NavStep::Moved { .. }
        ));
    }

    /// Two decks meet at a seam, and a third overlaps the same XZ area slightly
    /// closer to the mover's feet. Height proximity alone would hand the mover
    /// to the decoy; the coincident outline edge is what identifies the real
    /// continuation. This is the relation `.nvm` LinkEdge encodes, recovered
    /// geometrically.
    #[test]
    fn transfer_prefers_the_object_meeting_the_exited_seam() {
        let mut app = App::new();
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
        ))
        .init_asset::<JMXVNVM>()
        .init_asset::<JMXVBMS>();

        let handle = app
            .world_mut()
            .resource_mut::<Assets<JMXVBMS>>()
            .add(deck_bms(false));

        // Mirror only, no yaw: a local point (lx, _, lz) lands at world
        // (tx - lx, ty, tz + lz), so a deck at tx spans world x in
        // [tx - 20, tx + 20].
        let place = |x: f32, z: f32, y: f32| {
            GlobalTransform::from(Transform::from_matrix(
                Mat4::from_scale_rotation_translation(
                    Vec3::new(-1.0, 1.0, 1.0),
                    Quat::IDENTITY,
                    Vec3::new(x, y, z),
                ),
            ))
        };

        // The mover walks at y = 100.4, a little above the decks it is on.
        let feet_y = 100.4;

        // Deck A: the mover starts here, spanning world x in [-20, 20].
        let a = app
            .world_mut()
            .spawn((ObjectNavMesh(vec![handle.clone()]), place(0.0, 0.0, 100.0)))
            .id();
        // Deck B: the real continuation, meeting A exactly at world x = -20.
        // Its surface is 0.4 below the feet.
        let b = app
            .world_mut()
            .spawn((
                ObjectNavMesh(vec![handle.clone()]),
                place(-40.0, 0.0, 100.0),
            ))
            .id();
        // Decoy: covers the same destination and sits exactly at the feet, so
        // it wins on height proximity outright — but it is shifted in z, so its
        // outline does not meet A's. Only the seam test can tell them apart.
        let decoy = app
            .world_mut()
            .spawn((ObjectNavMesh(vec![handle]), place(-40.0, 15.0, feet_y)))
            .id();

        // Walk from deck A across the shared border towards world -x.
        let step = run_step(
            &mut app,
            Vec3::new(0.0, feet_y, 0.0),
            Vec2::new(-25.0, 0.0),
            NavLocation::OnObject { object: a },
        );

        match step {
            NavStep::Moved { location, .. } => {
                assert_eq!(
                    location,
                    NavLocation::OnObject { object: b },
                    "expected the seam-matching deck, not the nearer decoy {decoy:?}"
                );
            }
            other => panic!("expected a transfer onto the next deck, got {other:?}"),
        }
    }

    /// Terrain plus one deck placed with the real mirror transform, spanning
    /// world x in [-120, -80] at `deck_y`. Movers approach from x = -130, so a
    /// step to x = -110 crosses the deck's outline at x = -120.
    fn app_with_terrain_and_deck(terrain_y: f32, deck_y: f32) -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
        ))
        .init_asset::<JMXVNVM>()
        .init_asset::<JMXVBMS>();
        spawn_flat_terrain(&mut app, terrain_y);

        let handle = app
            .world_mut()
            .resource_mut::<Assets<JMXVBMS>>()
            .add(deck_bms(false));
        let object = app
            .world_mut()
            .spawn((
                ObjectNavMesh(vec![handle]),
                GlobalTransform::from(Transform::from_matrix(
                    Mat4::from_scale_rotation_translation(
                        Vec3::new(-1.0, 1.0, 1.0),
                        Quat::IDENTITY,
                        Vec3::new(-100.0, deck_y, 100.0),
                    ),
                )),
            ))
            .id();

        (app, object)
    }

    /// Walking across a deck's outline boards it even when the deck sits
    /// further above the feet than [`STAND_TOLERANCE`].
    ///
    /// The bridge report: entry used to be decided only by height proximity,
    /// so a deck 25 units up was never even a candidate and the mover kept
    /// walking on the terrain underneath.
    #[test]
    fn crossing_an_outline_boards_a_deck_beyond_stand_tolerance() {
        let (mut app, object) = app_with_terrain_and_deck(100.0, 125.0);

        match run_step(
            &mut app,
            Vec3::new(-130.0, 100.0, 100.0),
            Vec2::new(-110.0, 100.0),
            NavLocation::Terrain,
        ) {
            NavStep::Moved { position, location } => {
                assert_eq!(location, NavLocation::OnObject { object });
                assert!((position.y - 125.0).abs() < 1e-3, "got {}", position.y);
            }
            other => panic!("expected to board the deck, got {other:?}"),
        }
    }

    /// A deck authored just *below* the ground it meets still wins once the
    /// mover crosses its outline. `prefer_height` would hand this to the
    /// terrain — it prefers the higher surface — and the mover would ride the
    /// terrain down instead of following the deck.
    #[test]
    fn crossing_an_outline_beats_terrain_sitting_slightly_higher() {
        let (mut app, object) = app_with_terrain_and_deck(100.0, 99.5);

        match run_step(
            &mut app,
            Vec3::new(-130.0, 100.0, 100.0),
            Vec2::new(-110.0, 100.0),
            NavLocation::Terrain,
        ) {
            NavStep::Moved { position, location } => {
                assert_eq!(location, NavLocation::OnObject { object });
                assert!((position.y - 99.5).abs() < 1e-3, "got {}", position.y);
            }
            other => panic!("expected to board the deck, got {other:?}"),
        }
    }

    /// A deck sunk well below the terrain it meets is still boarded when the
    /// mover crosses its outline.
    ///
    /// `prefer_height` takes the *higher* surface, so the terrain used to win
    /// this outright no matter how the mover got there — a bridge whose deck is
    /// authored a few units under its own abutment could never be stepped onto,
    /// and the mover walked down the terrain under it instead. Crossing the
    /// outline is the membership test; height only reads off the surface.
    #[test]
    fn crossing_an_outline_boards_a_deck_sunk_below_the_terrain() {
        // 50 below the ground: past both STAND_TOLERANCE and MAX_STEP_UP, so
        // neither the old candidacy gate nor the old step-up gate would let the
        // mover on. Still within OBJECT_BLOCK_Y_BAND, so the outline is a door.
        let (mut app, object) = app_with_terrain_and_deck(100.0, 50.0);

        match run_step(
            &mut app,
            Vec3::new(-130.0, 100.0, 100.0),
            Vec2::new(-110.0, 100.0),
            NavLocation::Terrain,
        ) {
            NavStep::Moved { position, location } => {
                assert_eq!(location, NavLocation::OnObject { object });
                assert!((position.y - 50.0).abs() < 1e-3, "got {}", position.y);
            }
            other => panic!("expected to board the sunken deck, got {other:?}"),
        }
    }

    /// Passing under an overhead deck must not capture the mover: its outline
    /// is crossed in XZ, but it is a storey up, not a door.
    ///
    /// This is the guard that replaced the `MAX_STEP_UP` gate on the entry
    /// path — the storey filter on the outline edge, not a height comparison.
    #[test]
    fn crossing_under_an_overhead_deck_stays_on_the_terrain() {
        let (mut app, _) = app_with_terrain_and_deck(100.0, 250.0);

        match run_step(
            &mut app,
            Vec3::new(-130.0, 100.0, 100.0),
            Vec2::new(-110.0, 100.0),
            NavLocation::Terrain,
        ) {
            NavStep::Moved { position, location } => {
                assert_eq!(location, NavLocation::Terrain);
                assert!((position.y - 100.0).abs() < 1e-3, "got {}", position.y);
            }
            other => panic!("expected to keep walking underneath, got {other:?}"),
        }
    }

    /// Walking off a deck lands on the terrain and stays there. The outline
    /// edge crossed on the way out is the same one crossed on the way in, so
    /// without excluding the object being left the mover would re-board it
    /// every step and never get off.
    #[test]
    fn leaving_a_deck_does_not_immediately_re_enter_it() {
        let (mut app, object) = app_with_terrain_and_deck(100.0, 100.0);

        // Start on the deck at world x = -110 and walk out past its x = -120
        // border onto the ground.
        match run_step(
            &mut app,
            Vec3::new(-110.0, 100.0, 100.0),
            Vec2::new(-130.0, 100.0),
            NavLocation::OnObject { object },
        ) {
            NavStep::Moved { location, .. } => {
                assert_eq!(location, NavLocation::Terrain);
            }
            other => panic!("expected to step off onto the terrain, got {other:?}"),
        }
    }

    /// A deck whose walkable nav mesh reaches well past the visual mesh box is
    /// still found by every nav query.
    ///
    /// `JMXVBMS::bounding_box` is the *visual* mesh's AABB, read from the file
    /// header; a map object's nav mesh routinely extends past its drawn
    /// geometry (a bridge deck reaches beyond its planks). The queries used to
    /// cull nav triangles against the visual box, so a mover standing over nav
    /// ground that lay outside it was rejected before a single triangle was
    /// tested — `resolve_location` returned `Terrain` on a bridge, and the
    /// mover stood at terrain height inside the deck. Culling against
    /// [`BmsNavMesh::bounds`] fixes it. Observed on `cj_bridgetree`, whose nav
    /// mesh is ~345 units wide in X where its visual box is ~30.
    #[test]
    fn nav_mesh_reaching_past_the_visual_box_is_still_found() {
        let mut app = App::new();
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
        ))
        .init_asset::<JMXVNVM>()
        .init_asset::<JMXVBMS>();

        // The deck's nav mesh spans local XZ [-20, 20], but the visual box is a
        // 2x2 sliver at the origin — far smaller than the walkable area, as on
        // the real bridge. The old cull would reject everything beyond it.
        let mut bms = deck_bms(false);
        bms.bounding_box = (Vec3::new(-1.0, 0.0, -1.0), Vec3::new(1.0, 0.0, 1.0));
        let handle = app.world_mut().resource_mut::<Assets<JMXVBMS>>().add(bms);
        let object = app
            .world_mut()
            .spawn((
                ObjectNavMesh(vec![handle]),
                GlobalTransform::from(Transform::from_xyz(0.0, 100.0, 0.0)),
            ))
            .id();

        // Stand at local (15, 15): inside the nav mesh, well outside the visual
        // box. This is the point the old cull dropped.
        let here = Vec3::new(15.0, 100.0, 15.0);
        assert_eq!(
            run_resolve(&mut app, here),
            NavLocation::OnObject { object },
            "a position over the deck's nav mesh but outside its visual box \
             must resolve onto the deck, not fall through to the terrain",
        );
    }
}
