use std::collections::HashMap;

use bevy::camera::Camera3d;
use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::prelude::*;

use crate::assets::bms::mesh::JMXVBMS;
use crate::assets::bsk::JMXVBSK;
use crate::assets::bsr::resource::SroResource;
use crate::assets::cpd::JMXVCPD;
use crate::assets::ifo::object::{ObjectInfo, ObjectInfoIndex};
use crate::assets::ifo::IFOAsset;
use crate::assets::o2::{MapObject, JMXVMAPO2};
use crate::plugins::cursor::interactions::GameCursorTarget;
use crate::plugins::dev::render_debug::RenderDebugSettings;
use crate::plugins::dynamic_resource_loader::{MirroredResource, UnloadedResource};
use crate::plugins::map::assets::MapsAssets;
use crate::plugins::map::terrain::{
    Terrain, TerrainLoadState, TerrainObjectData, FOG_RANGE, REGION_SIZE, VISIBLE_RANGE,
};
use crate::util::mesh::needs_winding_reversal;
use crate::util::region::RegionIdExt;

/// Extra slack past the fog end before an object wrapper is hidden: the
/// wrapper's translation is the object's *anchor*, and its meshes can extend
/// from there toward the camera (long wall segments, large buildings). An
/// anchor at fog_end + m whose geometry reaches m units closer is exactly at
/// the fully-fogged distance — invisible either way.
const OBJECT_HIDE_MARGIN: f32 = 480.0;

/// Hides map-object wrappers whose anchor sits fully behind the opaque fog.
/// The region-root hiding (`terrain::region_visibility`) only covers regions
/// *entirely* past the fog end — objects in boundary regions, and objects
/// parented to a nearer region than the one they geographically occupy
/// (`load_terrain_objects_system` spawns region-edge objects under whichever
/// region's `.o2` listed them first), still rendered from deep inside the
/// fog. Stands down while the render-debug `render_objects` toggle is off —
/// that path owns wrapper visibility then — and, like the region hiding,
/// while the camera carries no `DistanceFog` to hide behind.
pub fn cull_fogged_objects(
    settings: Res<RenderDebugSettings>,
    cameras: Query<(&Transform, &Camera, Has<DistanceFog>), With<Camera3d>>,
    mut objects: Query<(&GlobalTransform, &mut Visibility), With<MapObject>>,
) {
    if !settings.render_objects {
        return;
    }
    let Some((camera_pos, fog_active)) = cameras
        .iter()
        .find(|(_, camera, _)| camera.is_active)
        .map(|(transform, _, fog)| (transform.translation, fog))
    else {
        return;
    };
    let hide_dist = (VISIBLE_RANGE + FOG_RANGE) as f32 * REGION_SIZE + OBJECT_HIDE_MARGIN;
    let hide_dist_sq = hide_dist * hide_dist;

    for (global, mut visibility) in &mut objects {
        let desired =
            if fog_active && global.translation().distance_squared(camera_pos) > hide_dist_sq {
                Visibility::Hidden
            } else {
                Visibility::Inherited
            };
        // set_if_neq: an unconditional write would re-propagate visibility
        // through every object's resource subtree each frame.
        visibility.set_if_neq(desired);
    }
}

/// Caches converted prop meshes by source `.bms` handle, so repeated instances of the
/// same model (a common tree, rock, etc. reused across many regions) share one GPU mesh
/// instead of each spawn rebuilding and registering its own copy — shared handles are
/// also what lets Bevy batch/instance the repeated draws. Keyed also on the two
/// spawn-time parameters that change the produced mesh bytes: the winding direction
/// and whether joint attributes were built in (`JMXVBMS::to_mesh(reverse_winding,
/// with_skinning)`). The skinning flag must be part of the key: the same skinned
/// `.bms` is spawned both skinned (worn armor) and unskinned (ground drop), and a
/// shared mesh would give one of the two a pipeline/bind-group mismatch (wgpu
/// validation error, see `to_mesh`).
///
/// Entries are weak `AssetId`s, not `Handle`s: the spawned entities are the
/// only strong owners, so a mesh (and its source `.bms`) frees once the last
/// region using it unloads instead of staying pinned for the whole session.
/// Lookups resolve via `Assets::get_strong_handle` and rebuild on a dead id;
/// `prune_spawn_caches` sweeps dead entries so the map itself stays bounded.
#[derive(Resource, Default)]
pub struct SroMeshes(pub HashMap<(AssetId<JMXVBMS>, bool, bool), AssetId<Mesh>>);

/// Caches skinned-mesh inverse bind poses by (source `.bms` handle, skeleton handle) —
/// this pair fully determines the bind pose matrices, so repeated spawns of the same
/// model+skeleton combination (e.g. many instances of one monster/NPC type) share one
/// asset instead of each spawn registering its own identical copy. Only populated for
/// skinned meshes; non-skinned props don't need bind poses at all (see
/// `SpawnResource::prepare_mesh_groups`). The skeleton is usually the resource's own,
/// but for attached clothes it is the skeleton of the character wearing them.
/// Weak `AssetId` entries, same lifetime scheme as [`SroMeshes`].
#[derive(Resource, Default)]
pub struct SroBindPoses(
    pub HashMap<(AssetId<JMXVBMS>, AssetId<JMXVBSK>), AssetId<SkinnedMeshInverseBindposes>>,
);

/// Wrapper entity of every spawned map object, keyed by (region id << 16 | uid).
/// Region-edge objects are listed in *several* regions' `.o2` files, and regions
/// complete on different frames (mesh builds are budgeted per frame), so the
/// dedup must persist across frames — a per-frame map spawned an object once per
/// listing region, each duplicate dragging a whole compound/resource subtree
/// (and double draws + z-fighting on the overlap). Entries self-heal instead of
/// needing despawn bookkeeping: a key whose entity no longer exists (region
/// unloaded, scene torn down) is treated as absent — `Entities::contains`
/// matches by generation and counts same-frame reservations as alive.
#[derive(Resource, Default)]
pub struct SpawnedMapObjects(pub HashMap<u32, Entity>);

/// `ObjID`s seen in a `.o2` that have no `object.ifo` row, so the warning is
/// logged once per id rather than once per placement.
///
/// This is a real gap in the shipped data, not a parse error: this build's
/// `Map/object.ifo` declares 2767 rows (`ObjID` 0..2766) while `.o2` placements
/// reference ids up to **3253** — 357 distinct ids across **42,746 placements
/// (18.5% of 231,261)** in **520 of 4,506** files. Measured with an independent
/// reader over the user's `Map/`; see #429.
#[derive(Resource, Default)]
pub struct UnknownObjectIds(pub std::collections::HashSet<u32>);

/// Periodic sweep of the spawn caches: they store weak ids (see
/// [`SroMeshes`]) so region unloads free the assets, but the map entries
/// themselves — and [`SpawnedMapObjects`]' dead `Entity` ids — would still
/// grow forever without it. Timer-driven rather than on-miss because a
/// player always moving into *new* areas never revisits a key.
pub fn prune_spawn_caches(
    meshes: Res<Assets<Mesh>>,
    inverse_bindposes: Res<Assets<SkinnedMeshInverseBindposes>>,
    entities: &bevy::ecs::entity::Entities,
    mut mesh_cache: ResMut<SroMeshes>,
    mut bind_pose_cache: ResMut<SroBindPoses>,
    mut spawned: ResMut<SpawnedMapObjects>,
) {
    mesh_cache.0.retain(|_, id| meshes.contains(*id));
    bind_pose_cache
        .0
        .retain(|_, id| inverse_bindposes.contains(*id));
    spawned.0.retain(|_, e| entities.contains(*e));
}

pub fn load_compound_system(
    mut commands: Commands,
    query: Query<(Entity, &LoadingCompound)>,
    compound_assets: Res<Assets<JMXVCPD>>,
    asset_server: Res<AssetServer>,
) {
    query.iter().for_each(|(entity, loading_compound)| {
        if asset_server.load_state(&loading_compound.0).is_loaded() {
            let compound = compound_assets.get(&loading_compound.0).expect("i failed");
            let resource_handles: Vec<Handle<SroResource>> = compound
                .resources
                .iter()
                .map(|res| asset_server.load(format!("data://{}", res.display())))
                .collect();
            if let Ok(mut entity) = commands.get_entity(entity) {
                entity
                    .remove::<LoadingCompound>()
                    .insert(LoadingResources(resource_handles));
            }
        }
    });
}

pub fn load_resources_system(
    mut commands: Commands,
    query: Query<(Entity, &LoadingResources, Has<MirroredResource>)>,
    asset_server: Res<AssetServer>,
) {
    query
        .iter()
        .for_each(|(entity, loading_resources, mirrored)| {
            if loading_resources
                .0
                .iter()
                .all(|res| asset_server.load_state(res.id()).is_loaded())
            {
                // A compound (.cpd) references several resources (walls, roof, ...),
                // each authored in the shared compound space. Fan each one out onto
                // its own child entity so `spawn_resources_when_loaded` spawns every
                // part; inserting `UnloadedResource` repeatedly on this single entity
                // would overwrite all but the last, dropping most of the building.
                let handles = loading_resources.0.clone();
                commands
                    .entity(entity)
                    .remove::<LoadingResources>()
                    .with_children(|parent| {
                        for res in handles {
                            let mut part = parent.spawn((
                                Transform::default(),
                                Visibility::default(),
                                UnloadedResource(res),
                                CompoundPart,
                            ));
                            // Propagate the mirror flag so each part's meshes get
                            // winding-corrected when spawned.
                            if mirrored {
                                part.insert(MirroredResource);
                            }
                        }
                    });
            }
        });
}

#[allow(dead_code)]
pub struct EnMat {
    pub entity: Entity,
    pub mat: Mat4,
}

#[derive(Component, Clone)]
pub struct LoadingCompound(Handle<JMXVCPD>);

#[derive(Component, Clone)]
pub struct LoadingResources(pub Vec<Handle<SroResource>>);

/// Anchor entity of one compound (.cpd) part, parenting that part's spawned
/// resource. Pure marker so entity-count diagnostics can attribute these
/// (their `UnloadedResource` is removed once the resource spawns).
#[derive(Component)]
pub struct CompoundPart;

#[derive(Component, Clone)]
#[allow(dead_code)]
pub struct LoadingResource(pub Handle<SroResource>);

#[derive(Component, Clone)]
#[allow(dead_code)]
pub struct LoadingSkeleton(Handle<JMXVBSK>);

#[derive(Component, Default, Clone, Reflect)]
#[reflect(Component)]
pub struct Bones {
    pub bones: Vec<String>,
}

#[derive(Component, Clone)]
#[allow(dead_code)]
pub struct CompletedSkinningData(SkinnedMesh);

#[derive(Component, Clone)]
#[allow(dead_code)]
pub struct CompletedMesh(Handle<Mesh>);

#[derive(Component)]
#[allow(dead_code)]
pub struct CompletedSkeleton;

/// The `object.ifo` row for a placed `ObjID`, or `None` if the table has no
/// such row.
///
/// `object.ifo` does not cover every placed id: this build's table declares 2767
/// rows (`ObjID` 0..2766) while `.o2` placements reference ids up to **3253** —
/// 357 distinct ids across **42,746 placements (18.5 % of 231,261)** in **520 of
/// 4,506** files, measured with an independent reader over the user's `Map/`
/// (#429). Panicking on the lookup therefore takes the whole client down the
/// moment one of those regions streams in; skipping the placement costs one prop
/// and reports it **once per id** rather than once per placement.
fn object_details<'a>(
    object_info: &'a ObjectInfoIndex,
    id: u32,
    unknown: &mut UnknownObjectIds,
) -> Option<&'a ObjectInfo> {
    match object_info.0.get(&id) {
        Some(details) => Some(details),
        None => {
            if unknown.0.insert(id) {
                warn!("map: ObjID {id} has no object.ifo row; skipping its placements");
            }
            None
        }
    }
}

/// Object spawns budgeted per frame, across every region processed this
/// call. A freshly-loaded region (`TerrainLoadState::LoadedMeshes`) can list
/// hundreds of objects across its blocks/LOD groups, and spawning them all
/// in one frame — on top of the mirrored despawn burst on the opposite side
/// of the same crossing — was the ~208ms region-crossing hitch measured live
/// and documented in `docs/perf-remote.md`. The mesh-build stage already
/// solved this exact problem (`GROUP_BUILDS_PER_FRAME`, `terrain/mod.rs`);
/// this is the same idiom for object spawning. Deferred objects cost nothing
/// extra to retry: the cross-frame `SpawnedMapObjects` dedup below already
/// makes re-walking a partially-spawned region safe.
const OBJECT_SPAWNS_PER_FRAME: i32 = 64;

pub fn load_terrain_objects_system(
    mut commands: Commands,
    mut query: Query<(Entity, &Terrain, &TerrainObjectData, &mut TerrainLoadState)>,
    map_object_assets: Res<Assets<JMXVMAPO2>>,
    asset_server: Res<AssetServer>,
    object_info_assets: Res<Assets<IFOAsset>>,
    maps_assets: Res<MapsAssets>,
    mut spawned: ResMut<SpawnedMapObjects>,
    mut unknown_objects: ResMut<UnknownObjectIds>,
    entities: &bevy::ecs::entity::Entities,
) {
    let object_info = object_info_assets
        .get(&maps_assets.object_index)
        .expect("i failed");
    let Some(object_info) = &object_info.object_info_index else {
        return;
    };
    let mut budget = OBJECT_SPAWNS_PER_FRAME;
    query
        .iter_mut()
        .for_each(|(terrain_entity, terrain, object_data, mut load_state)| {
            if asset_server.load_state(&object_data.0).is_loaded() {
                let object_data = map_object_assets.get(&object_data.0).expect("i failed");

                match load_state.as_ref() {
                    TerrainLoadState::LoadedMeshes => {
                        // Only true once every object in this region has been
                        // spawned (or was already); stays `LoadedMeshes` (retried
                        // next frame) instead of advancing to `Completed` below
                        // if the budget ran out partway through.
                        let mut all_spawned = true;
                        object_data
                            .blocks
                            .iter()
                            .flat_map(|block| block.lod_groups.iter())
                            .for_each(|lod| {
                                commands.entity(terrain_entity).with_children(|parent| {
                                    lod.objects.iter().for_each(|object| {
                                        let obj_key: u32 =
                                            ((object.region_id as u32) << 16) | object.uid as u32;
                                        // Cross-frame dedup via the persistent registry;
                                        // dead entries (unloaded regions) read as absent.
                                        let already_spawned = spawned
                                            .0
                                            .get(&obj_key)
                                            .is_some_and(|&e| entities.contains(e));
                                        if already_spawned {
                                            return;
                                        }
                                        if budget <= 0 {
                                            all_spawned = false;
                                            return;
                                        }
                                        budget -= 1;
                                        {
                                            let (x, z) = object.region_id.to_x_z();
                                            let (cx, cz) = terrain.to_x_z();
                                            let dx = x as i32 - cx as i32;
                                            let dz = z as i32 - cz as i32;

                                            let relative_pos = Vec3::new(
                                                dx as f32 * 1920.0,
                                                0.0,
                                                dz as f32 * 1920.0,
                                            );
                                            let mut translation = object.position + relative_pos;
                                            translation.x *= -1.0;
                                            let trs_matrix = Mat4::from_scale_rotation_translation(
                                                Vec3::new(-1.0, 1.0, 1.0),
                                                Quat::from_rotation_y(object.yaw),
                                                translation,
                                            );
                                            let transform = Transform::from_matrix(trs_matrix);
                                            // `object.ifo` does not cover every
                                            // placed `ObjID`: this build's table
                                            // ends at 2766, while `.o2` records
                                            // reference ids up to 3253 — 357
                                            // distinct ids over 42,746 placements
                                            // (18.5%) in 520 of 4,506 files. An
                                            // `expect` here takes the whole client
                                            // down as soon as one of those regions
                                            // streams in; skipping the placement
                                            // loses one prop and says so once.
                                            let Some(object_details) = object_details(
                                                object_info,
                                                object.id,
                                                &mut unknown_objects,
                                            ) else {
                                                return;
                                            };
                                            let path =
                                                format!("data://{}", object_details.path.display());

                                            // Map objects are placed with a mirroring transform
                                            // (scale.x = -1). Derive whether their meshes need
                                            // reversed winding from the transform determinant,
                                            // via the single shared rule (see `needs_winding_reversal`).
                                            let mut object_entity =
                                                parent.spawn((transform, Visibility::default()));
                                            spawned.0.insert(obj_key, object_entity.id());
                                            if needs_winding_reversal(&trs_matrix) {
                                                object_entity.insert(MirroredResource);
                                            }
                                            if object_details.is_compound() {
                                                object_entity
                                                    .insert(Name::from(format!(
                                                        "Compound - {}|{} {}",
                                                        object.id,
                                                        object.uid,
                                                        &object_details.path.display()
                                                    )))
                                                    .insert(object.clone())
                                                    .insert(GameCursorTarget::default());
                                                let compound_handle: Handle<JMXVCPD> =
                                                    asset_server.load(path);
                                                object_entity
                                                    .insert(LoadingCompound(compound_handle));
                                            } else {
                                                let resource_handle: Handle<SroResource> =
                                                    asset_server.load(path);
                                                object_entity
                                                    .insert(Name::from(format!(
                                                        "Resource - {}",
                                                        object_details.path.display()
                                                    )))
                                                    .insert(object.clone())
                                                    .insert(GameCursorTarget::default())
                                                    .insert(LoadingResources(vec![
                                                        resource_handle,
                                                    ]));
                                                // parent.add_command(SpawnResource(resource_handle, transform, None));
                                            }
                                        }
                                    });
                                });
                            });
                        if all_spawned {
                            *load_state = TerrainLoadState::Completed;
                        }
                    }
                    TerrainLoadState::None | TerrainLoadState::BuildingMeshes { .. } => {}
                    TerrainLoadState::Completed => {
                        // Disarm the per-frame polling (the run condition is on
                        // `TerrainLoadState`, see `map/mod.rs`) but *keep*
                        // `TerrainObjectData`, so this region can be re-armed and
                        // re-run its object pass later — see
                        // `rearm_object_passes_on_region_unload` for why it has to.
                        if let Ok(mut entity) = commands.get_entity(terrain_entity) {
                            entity.remove::<TerrainLoadState>();
                        }
                    }
                };
            }
        });
}

/// Re-arms every completed region's object pass after any region unloads.
///
/// Idea: a map object is spawned as a child of whichever region's `.o2` listed
/// it *first*, but 10,820 of the 74,060 distinct objects in the shipped corpus
/// (14.6%) are listed by more than one region — one by 64 of them. Fences and
/// walls running along a region border are exactly this class. So when region A
/// unloads, Bevy despawns its whole subtree, including objects that
/// geographically belong to still-loaded region B; B has already `Completed`
/// and, before this system, could never spawn them again. The prop stayed
/// missing until B itself unloaded and reloaded (#571).
///
/// The obvious alternative — parent each object to its own `region_id`'s
/// terrain entity — was rejected on the data: 45 of those 74,060 objects are
/// listed *only* by a foreign region, 43 of which resolve to a real
/// `object.ifo` model. Strict ownership would silently delete them, and they
/// are not filler: `alex_paros` (the Pharos of Alexandria), `alex_harbor`,
/// `alex_obelisk` and two of Baghdad's city gates are in that set. Re-arming
/// keeps the listing-region spawn (nothing is lost) and repairs the lifetime
/// instead.
///
/// Re-arming is deliberately unconditional across loaded regions rather than
/// restricted to the unloaded region's neighbours: an object may be listed by a
/// region 8 sectors away (the 64-region case above), so adjacency is not a safe
/// filter. It costs nothing in steady state — regions unload only on a boundary
/// crossing, and the re-armed pass is one dedup lookup per listed object before
/// `Completed` disarms it again. `TerrainObjectData` survives `Completed`
/// precisely so this needs no asset reload.
pub fn rearm_object_passes_on_region_unload(
    mut commands: Commands,
    mut unloaded_regions: RemovedComponents<Terrain>,
    completed: Query<
        Entity,
        (
            With<Terrain>,
            With<TerrainObjectData>,
            Without<TerrainLoadState>,
        ),
    >,
) {
    if unloaded_regions.is_empty() {
        return;
    }
    unloaded_regions.clear();
    for terrain_entity in &completed {
        if let Ok(mut entity) = commands.get_entity(terrain_entity) {
            entity.insert(TerrainLoadState::LoadedMeshes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::ifo::object::ObjectInfo;
    use crate::plugins::map::terrain::TerrainId;
    use std::path::PathBuf;

    fn index(ids: &[u32]) -> ObjectInfoIndex {
        ObjectInfoIndex(
            ids.iter()
                .map(|&id| {
                    (
                        id,
                        ObjectInfo {
                            id,
                            flag: 0,
                            path: PathBuf::from("res/bldg/china/whatever.bsr"),
                        },
                    )
                })
                .collect(),
        )
    }

    /// A placed `ObjID` with no `object.ifo` row must skip the placement, not
    /// take the client down. The gap is real and large in the shipped data:
    /// `Map/object.ifo` declares 2767 rows (ids 0..2766) while `.o2` records
    /// reference ids up to 3253 — 357 distinct ids over 42,746 placements
    /// (18.5%) in 520 of 4,506 files (#429). `2767` below is exactly the first
    /// id past the table.
    #[test]
    fn an_objid_without_an_ifo_row_is_skipped_not_fatal() {
        let index = index(&[47, 2766]);
        let mut unknown = UnknownObjectIds::default();

        assert!(object_details(&index, 2766, &mut unknown).is_some());
        assert!(object_details(&index, 2767, &mut unknown).is_none());
        assert!(object_details(&index, 3253, &mut unknown).is_none());
        assert!(unknown.0.contains(&2767) && unknown.0.contains(&3253));
    }

    /// A region that has `Completed` keeps `TerrainObjectData` and is re-armed
    /// when any *other* region unloads — the lifetime half of #571. Without
    /// this, an object listed by both A and B but parented under A is gone for
    /// good the moment A unloads, even though B is still on screen.
    #[test]
    fn an_unloaded_region_re_arms_the_surviving_regions() {
        let mut app = App::new();
        app.add_systems(Update, rearm_object_passes_on_region_unload);

        // Two "completed" regions: object data kept, load state disarmed.
        let a = app
            .world_mut()
            .spawn((
                Terrain(TerrainId::from_x_z(1, 1)),
                TerrainObjectData(Handle::default()),
            ))
            .id();
        let b = app
            .world_mut()
            .spawn((
                Terrain(TerrainId::from_x_z(2, 1)),
                TerrainObjectData(Handle::default()),
            ))
            .id();

        // Nothing unloaded yet: no region may be re-armed, or every completed
        // region would re-run its whole object pass every single frame.
        app.update();
        assert!(app.world().get::<TerrainLoadState>(b).is_none());

        app.world_mut().entity_mut(a).despawn();
        app.update();

        assert!(
            matches!(
                app.world().get::<TerrainLoadState>(b),
                Some(TerrainLoadState::LoadedMeshes)
            ),
            "surviving region must re-run its object pass so it can re-adopt \
             objects that died with the unloaded region"
        );
    }

    /// The re-arm needs the region's `.o2` handle, so a region that no longer
    /// carries `TerrainObjectData` must be left alone rather than re-armed into
    /// a pass it cannot run (`load_terrain_objects_system` queries it).
    #[test]
    fn a_region_without_object_data_is_not_re_armed() {
        let mut app = App::new();
        app.add_systems(Update, rearm_object_passes_on_region_unload);

        let a = app
            .world_mut()
            .spawn((
                Terrain(TerrainId::from_x_z(1, 1)),
                TerrainObjectData(Handle::default()),
            ))
            .id();
        let bare = app
            .world_mut()
            .spawn(Terrain(TerrainId::from_x_z(3, 1)))
            .id();

        app.world_mut().entity_mut(a).despawn();
        app.update();

        assert!(app.world().get::<TerrainLoadState>(bare).is_none());
    }

    /// `SpawnedMapObjects` is keyed globally and self-heals by entity liveness,
    /// so the re-armed pass above actually re-spawns: the registry entry of an
    /// object that died with its region must read as absent, not as "already
    /// spawned". This is the other half of the fix — re-arming a region whose
    /// dedup still claimed the dead object would change nothing.
    #[test]
    fn a_dead_registry_entry_reads_as_not_spawned() {
        let mut world = World::new();
        let object = world.spawn_empty().id();
        let key: u32 = ((0x595c_u32) << 16) | 12290;
        let mut spawned = SpawnedMapObjects::default();
        spawned.0.insert(key, object);

        assert!(world.entities().contains(object));
        world.entity_mut(object).despawn();
        assert!(
            !world.entities().contains(spawned.0[&key]),
            "an object despawned with its region must not keep the key claimed"
        );
    }

    /// The warning is per id, not per placement: those 357 ids cover 42,746
    /// placements, and one line each is diagnosable where 42,746 is noise.
    #[test]
    fn unknown_objids_are_reported_once_each() {
        let index = index(&[1]);
        let mut unknown = UnknownObjectIds::default();

        // first sighting records it, every repeat is silent
        assert!(unknown.0.insert(9999));
        assert!(object_details(&index, 9999, &mut unknown).is_none());
        assert_eq!(unknown.0.len(), 1);
        for _ in 0..10 {
            assert!(object_details(&index, 9999, &mut unknown).is_none());
        }
        assert_eq!(unknown.0.len(), 1);
    }
}
