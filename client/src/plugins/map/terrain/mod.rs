use std::collections::HashMap;
use std::fmt::{Formatter, LowerHex, UpperHex};

use bevy::asset::RenderAssetUsages;
use bevy::asset::{Assets, Handle, LoadState};
use bevy::camera::Camera3d;
use bevy::log::warn;
use bevy::math::Vec3;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::Name;
use bevy::prelude::*;

use crate::assets::m::block_mesh::merge_block_meshes;
use crate::assets::m::block_splat_material::{TerrainBlockSplatMaterial, TerrainLightmapFallback};
use crate::assets::m::{TerrainBlock, WaterType, JMXVMAPM};
use crate::assets::mfo::JMXVMFO;
use crate::assets::nvm::JMXVNVM;
use crate::assets::o2::JMXVMAPO2;
use crate::assets::t::JMXVMAPT;
use crate::plugins::cursor::interactions::GameCursorTarget;
use crate::plugins::dev::aabb_lines::DebugAabb;
use crate::plugins::map::assets::MapsAssets;
use crate::plugins::map::water_hq_material::HighQualityWaterMaterial;
use crate::plugins::world_origin::WorldOrigin;
use crate::util::mesh::needs_winding_reversal;
use crate::util::region::RegionIdExt;

pub mod rendering;

/// World-space size of one terrain region/tile (matches the .m/.o2/.nvm grid step).
pub const REGION_SIZE: f32 = 1920.0;
/// How many regions out from the camera stay fully visible (no fog).
pub const VISIBLE_RANGE: i32 = 2;
/// Extra ring of regions, beyond `VISIBLE_RANGE`, over which the fog fades a region
/// out to fully opaque. See `rendering::fog()`.
pub const FOG_RANGE: i32 = 1;
/// Extra rings of regions kept loaded (but already fully hidden by fog) beyond
/// `VISIBLE_RANGE + FOG_RANGE` before actually being despawned — and, read the other
/// way around, how far ahead of the fully-fogged point a region is spawned.
///
/// This needs to be more than one region: fog opacity is a smooth per-pixel distance
/// from the camera's exact position, so it can reach full opacity partway *through*
/// a region rather than exactly on its boundary (e.g. observed to bottom out around
/// the middle of a region rather than at its far edge). If region `n` is where fog
/// hits full opacity, region `n + 1` must already be loaded by then — a single ring
/// of margin only guarantees `n`. On top of that, map objects (props, mountains,
/// buildings) go through their own multi-step async load chain once their region
/// spawns (see `load_terrain_objects_system`/`load_compound_system`/
/// `load_resources_system` in objects.rs), which takes additional time beyond the
/// region's own mesh load. Extra rings buy that pipeline more real time to finish
/// before the fog clears enough to reveal it.
///
/// Lowered from 3 to 1 (the documented minimum above) as a stopgap while the real fix —
/// merging terrain blocks into far fewer draw calls, see `load_terrain_system` — lands.
/// Watch for object/prop pop-in near the fog boundary if this needs to go back up.
pub const UNLOAD_BUFFER: i32 = 1;
/// Total ring width, beyond `VISIBLE_RANGE`, kept loaded before despawning.
pub const UNLOAD_MARGIN: i32 = FOG_RANGE + UNLOAD_BUFFER;

/// Extra slack past the fog end before a region root is hidden outright:
/// covers map-object meshes that overhang their region's 1920x1920 footprint
/// toward the camera (a pixel at fog_end - m is only m/1920 visible through
/// the fog; half a block of margin makes that ~0).
const REGION_HIDE_MARGIN: f32 = 160.0;

/// Region-root visibility for region `(t_x, t_z)` given the camera's
/// SRO-space position: `Hidden` when the nearest point of the region's XZ
/// footprint lies past the distance where `rendering::fog()` is fully opaque
/// (env profiles only ever pull fog closer, never past it — see
/// `envi_fog_range`). Regions stay *loaded* out to the unload boundary so
/// streaming keeps its fog-covered head start, but everything fully behind
/// the fog stops being rendered instead of being drawn and then erased by
/// per-pixel fog. The per-pixel fog distance is 3D >= this XZ distance, so
/// the test is conservative. `fog_active` = the camera actually carries
/// `DistanceFog` (the render-debug panel can remove it) — without fog there
/// is nothing to hide behind.
fn region_visibility(cam_sro: Vec3, t_x: i32, t_z: i32, fog_active: bool) -> Visibility {
    if !fog_active {
        return Visibility::default();
    }
    let hide_dist = (VISIBLE_RANGE + FOG_RANGE) as f32 * REGION_SIZE + REGION_HIDE_MARGIN;
    // SRO-space footprint: region x covers world x in [-(t_x+1), -t_x] * REGION_SIZE
    // (world X is mirrored), region z covers [t_z, t_z+1] * REGION_SIZE.
    let x_max = t_x as f32 * -REGION_SIZE;
    let x_min = x_max - REGION_SIZE;
    let z_min = t_z as f32 * REGION_SIZE;
    let z_max = z_min + REGION_SIZE;
    let dx = cam_sro.x - cam_sro.x.clamp(x_min, x_max);
    let dz = cam_sro.z - cam_sro.z.clamp(z_min, z_max);
    if dx * dx + dz * dz > hide_dist * hide_dist {
        Visibility::Hidden
    } else {
        Visibility::default()
    }
}

/// Fixed block-grid size of a region (see `JMXVMAPM`'s parser: always `for z_block in 0..6
/// { for x_block in 0..6 { ... } }`). Every region merges its whole grid into one mesh and one
/// material: the ground-tile atlas is indexed by tile id globally, so there is no per-group
/// texture budget to blow and no reason to split a region into smaller patches (which is what
/// the old `region_group_size` fallback existed for — see `TerrainTileAtlas`).
const REGION_BLOCKS_PER_SIDE: i32 = 6;

#[derive(Resource)]
pub struct TerrainMesh(pub Handle<Mesh>);

/// Default/high graphics tier — see `water_hq_material.rs` for what it adds over plain
/// transmissive glass. Plain water/ice surfaces otherwise use a flat `StandardMaterial`
/// on the shared `water_patch_mesh()` plane (see `setup_terrain_mesh`).
#[derive(Resource)]
pub struct WaterNormalMaterial(pub Handle<HighQualityWaterMaterial>);
/// Low graphics tier (`graphics.water.quality: low`). Exactly one of this and
/// [`WaterNormalMaterial`] is inserted by `setup_terrain_mesh`, and
/// `load_terrain_system` spawns whichever it finds — so only one water draw
/// path is ever live.
#[derive(Resource)]
pub struct WaterLowMaterial(
    pub Handle<crate::plugins::map::water_material::LowQualityWaterMaterial>,
);
#[derive(Resource)]
pub struct WaterIceMaterial(pub Handle<StandardMaterial>);

/// Any streamed water or ice surface, whichever material tier it carries.
///
/// The three spawn sites below differ only in material type — HQ water, low water and ice
/// — so anything that wants to address "the water" as a category (the `world_counts/water`
/// diagnostic, the `render_water` debug toggle) would otherwise have to name all three and
/// be re-taught every time a tier is added. The marker is the category; the material is an
/// implementation detail of the tier.
#[derive(Component)]
pub struct WaterPlane;

#[derive(Bundle)]
pub struct TerrainBundle {
    terrain: Terrain,
    terrain_load_state: TerrainLoadState,
    map_data: TerrainMeshData,
    lightmap_data: TerrainLightmapData,
    object_data: TerrainObjectData,
    nav_mesh: TerrainNavMeshData,
    transform: Transform,
    visibility: Visibility,
}

#[derive(Component)]
pub struct TerrainMeshData(pub(crate) Handle<JMXVMAPM>);

/// Baked terrain lightmap for this region (JMXVMAPT `.t`, see `assets/t.rs`). Loaded alongside the
/// `.m` heightmap; its decoded DDS texture modulates terrain color in the splat shader. Regions
/// without a `.t` file (or that fail to decode) simply carry no lightmap and render unmodulated.
#[derive(Component)]
pub struct TerrainLightmapData(pub(crate) Handle<JMXVMAPT>);

#[derive(Component)]
pub struct TerrainObjectData(pub(crate) Handle<JMXVMAPO2>);

#[derive(Component)]
pub struct TerrainNavMeshData(pub(crate) Handle<JMXVNVM>);

#[derive(Component, Reflect, Default)]
pub enum TerrainLoadState {
    #[default]
    None,
    /// Ground merge groups built so far. Group builds are budgeted per frame
    /// (see `load_terrain_system`), so a region under construction parks its
    /// progress here between frames. `group_size` is the merge size in blocks
    /// per side — always `REGION_BLOCKS_PER_SIDE` now, kept in the state so the
    /// resume arithmetic stays explicit rather than assuming a constant.
    BuildingMeshes {
        next_group: i32,
        group_size: i32,
    },
    LoadedMeshes,
    Completed,
}

#[derive(Debug, Reflect)]
pub struct TerrainId(pub u16);

impl LowerHex for TerrainId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:x}", self.0)
    }
}
impl UpperHex for TerrainId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:X}", self.0)
    }
}

impl TerrainId {
    pub fn from_x_z(x: u8, z: u8) -> Self {
        let mut region_id = 0_u16;
        region_id += x as u16;
        region_id += (z as u16) << 8;
        TerrainId(region_id)
    }
    pub fn to_x_z(&self) -> (u8, u8) {
        self.0.to_x_z()
    }
}

#[derive(Component, Reflect)]
pub struct Terrain(pub(crate) TerrainId);

impl Terrain {
    pub fn to_x_z(&self) -> (u8, u8) {
        self.0.to_x_z()
    }
}

/// Marks a terrain region as explicitly preloaded (see `preload_terrain_region`) — exempt
/// from the distance-based despawn in `load_terrain_dynamically`, so it stays resident
/// permanently instead of being torn down and rebuilt from scratch as the camera moves
/// away and back. A prototype for keeping a known area (a starting zone, a boss room)
/// always loaded rather than streaming it like the rest of the world.
#[derive(Component)]
pub struct PreloadedTerrain;

/// Eagerly spawns terrain for every valid region in `[min_x..=max_x] x [min_z..=max_z]`
/// and marks it `PreloadedTerrain`. Skips regions the `.mfo` bitmap marks as not present,
/// same check `load_terrain_dynamically` uses.
pub fn preload_terrain_region(
    commands: &mut Commands,
    asset_server: &AssetServer,
    map_info: &JMXVMFO,
    origin: &WorldOrigin,
    min_x: u8,
    max_x: u8,
    min_z: u8,
    max_z: u8,
) {
    for x in min_x..=max_x {
        for z in min_z..=max_z {
            if !map_info.region_data[x as usize + z as usize * map_info.map_width] {
                continue;
            }
            let terrain_id = TerrainId::from_x_z(x, z);
            let hex_id = format!("{:x}", terrain_id);
            let p = origin.to_render(Vec3::new(
                x as f32 * -REGION_SIZE,
                0.0,
                z as f32 * REGION_SIZE,
            ));
            trace!("Preload TerrainId {:?} ({:?})", terrain_id, hex_id);
            commands
                .spawn(TerrainBundle {
                    terrain: Terrain(terrain_id),
                    map_data: TerrainMeshData(asset_server.load(format!("map://{z}/{x}.m"))),
                    lightmap_data: TerrainLightmapData(
                        asset_server.load(format!("map://{z}/{x}.t")),
                    ),
                    object_data: TerrainObjectData(asset_server.load(format!("map://{z}/{x}.o2"))),
                    terrain_load_state: TerrainLoadState::None,
                    nav_mesh: TerrainNavMeshData(
                        asset_server.load(format!("data://navmesh/nv_{hex_id}.nvm")),
                    ),
                    transform: Transform::from_translation(p),
                    visibility: Visibility::default(),
                })
                .insert(Name::from(format!("Region {}/{} (preloaded)", x, z)))
                .insert(PreloadedTerrain);
        }
    }
}

/// Region despawns budgeted per frame. Crossing a region boundary can flag a
/// whole ring/row of regions out-of-range at once, and `despawn()` recursively
/// tears down each one's entire mesh/object/compound subtree — unthrottled,
/// this was half of the ~208ms region-crossing hitch measured live (the
/// other half was the mirrored unbudgeted spawn burst, see
/// `OBJECT_SPAWNS_PER_FRAME` in `objects.rs`), documented in
/// `docs/perf-remote.md`. `out_of_range` is recomputed fresh from the
/// camera's position every time this system runs, so a region that misses
/// its budget this frame is simply re-evaluated (and despawned once budget
/// allows) the next one — no extra state needed.
const REGION_UNLOADS_PER_FRAME: i32 = 2;

pub fn load_terrain_dynamically(
    camera_query: Query<
        (&Transform, &Camera, Has<DistanceFog>),
        (With<Camera>, Changed<Transform>),
    >,
    mut terrain_query: Query<
        (Entity, &Terrain, &mut Visibility, Option<&PreloadedTerrain>),
        With<Terrain>,
    >,
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    map_infos: Res<Assets<JMXVMFO>>,
    maps_assets: Res<MapsAssets>,
    origin: Res<WorldOrigin>,
) {
    let x_range = VISIBLE_RANGE;
    let z_range = VISIBLE_RANGE;
    let x_neg_range = VISIBLE_RANGE;
    let z_neg_range = VISIBLE_RANGE;
    let unload_margin = UNLOAD_MARGIN;

    // Nothing to do
    if camera_query.is_empty() {
        return;
    }

    for (transform, camera, fog_active) in camera_query.iter() {
        if !camera.is_active {
            continue;
        }

        // The camera lives in render space; region ids are derived from SRO
        // space, so the world origin has to be added back first.
        let position = origin.to_sro(transform.translation);
        let (region_x, region_z) = position.to_x_z();

        let mut existing_terrains = HashMap::new();

        let map_info = match map_infos.get(&maps_assets.map_info) {
            Some(map_info) => map_info,
            None => {
                warn!("no map info");
                return;
            }
        };

        if map_info.map_width == 0 || map_info.map_height == 0 {
            warn!("map info has zero size");
            return;
        }

        let min_x = (region_x as i32 - x_neg_range).max(0);
        let max_x = (region_x as i32 + x_range).min(map_info.map_width as i32 - 1);
        let min_z = (region_z as i32 - z_neg_range).max(0);
        let max_z = (region_z as i32 + z_range).min(map_info.map_height as i32 - 1);

        let unload_min_x = min_x - unload_margin;
        let unload_max_x = max_x + unload_margin;
        let unload_min_z = min_z - unload_margin;
        let unload_max_z = max_z + unload_margin;

        // Load chunks out to the same boundary they'd otherwise be despawned at, not just
        // out to the fully-visible (no-fog) range. That gives mesh/texture streaming a
        // head start while the chunk is still hidden behind fog, so by the time the camera
        // gets close enough for the fog to clear, the chunk is already fully loaded instead
        // of popping in.
        let load_min_x = unload_min_x.max(0);
        let load_max_x = unload_max_x.min(map_info.map_width as i32 - 1);
        let load_min_z = unload_min_z.max(0);
        let load_max_z = unload_max_z.min(map_info.map_height as i32 - 1);

        // Regions stay *loaded* out to the unload boundary (streaming head start under
        // fog cover), but only regions not yet fully behind the opaque fog are rendered —
        // see `region_visibility`. Hiding the region root hides its whole subtree
        // (ground groups, map objects, water planes).
        let mut unload_budget = REGION_UNLOADS_PER_FRAME;
        terrain_query
            .iter_mut()
            .for_each(|(entity, terrain, mut visibility, preloaded)| {
                let (t_x, t_z) = terrain.to_x_z();
                let t_x = t_x as i32;
                let t_z = t_z as i32;
                let out_of_range = t_x < unload_min_x
                    || t_x > unload_max_x
                    || t_z < unload_min_z
                    || t_z > unload_max_z;
                if out_of_range && preloaded.is_none() && unload_budget > 0 {
                    unload_budget -= 1;
                    trace!(
                        "Unload TerrainId {:?}",
                        TerrainId::from_x_z(t_x as u8, t_z as u8)
                    );
                    commands.entity(entity).despawn();
                } else {
                    // The preload marker is a *head start*, not a pin: it exists so the
                    // eagerly-spawned starting area survives until the camera actually
                    // reaches it. Retiring it on first arrival is what bounds the resident
                    // set — left on, those regions (up to 7x7 around Jangan) were exempt
                    // from despawn for the whole session, so walking away kept their
                    // meshes, materials, textures and full map-object subtrees resident
                    // and still paying extract/prepare cost every frame.
                    if !out_of_range && preloaded.is_some() {
                        commands.entity(entity).remove::<PreloadedTerrain>();
                    }
                    // set_if_neq: an unconditional write would change-flag all
                    // ~81 region subtrees for visibility re-propagation on
                    // every camera move.
                    visibility.set_if_neq(region_visibility(position, t_x, t_z, fog_active));
                    existing_terrains.insert((t_x, t_z), true);
                }
            });

        for x in load_min_x..=load_max_x {
            for z in load_min_z..=load_max_z {
                let x_u8 = x as u8;
                let z_u8 = z as u8;
                if !existing_terrains.contains_key(&(x_u8 as i32, z_u8 as i32))
                    && map_info.region_data[x as usize + z as usize * map_info.map_width]
                {
                    let p = origin.to_render(Vec3::new(
                        x as f32 * -REGION_SIZE,
                        0.0,
                        z as f32 * REGION_SIZE,
                    ));
                    let terrain_id = TerrainId::from_x_z(x_u8, z_u8);
                    let hex_id = format!("{:x}", terrain_id);
                    trace!("Load TerrainId {:?} ({:?})", terrain_id, hex_id);
                    commands
                        .spawn(TerrainBundle {
                            terrain: Terrain(terrain_id),
                            map_data: TerrainMeshData(
                                asset_server.load(format!("map://{z}/{x}.m")),
                            ),
                            lightmap_data: TerrainLightmapData(
                                asset_server.load(format!("map://{z}/{x}.t")),
                            ),
                            object_data: TerrainObjectData(
                                asset_server.load(format!("map://{z}/{x}.o2")),
                            ),
                            terrain_load_state: TerrainLoadState::None,
                            nav_mesh: TerrainNavMeshData(
                                asset_server.load(format!("data://navmesh/nv_{hex_id}.nvm")),
                            ),
                            transform: Transform::from_translation(p),
                            // outer-ring regions spawn already hidden — they
                            // must not render a stray frame before the next
                            // camera move re-evaluates them
                            visibility: region_visibility(position, x, z, fog_active),
                        })
                        .insert(Name::from(format!("Region {}/{}", x, z)));
                }
            }
        }
    }
}

/// How many merged terrain groups may be built in one frame, across all
/// regions. One build merges the group's block meshes and copies tile pixel
/// data into the splat texture array — several milliseconds of main-thread
/// work. Crossing a region boundary queues several regions × 4 groups at
/// once; building them all in one frame was the terrain-streaming hitch, so
/// the work is spread over frames instead, nearest region first (the far
/// ones sit behind fog while they wait).
const GROUP_BUILDS_PER_FRAME: i32 = 2;

pub fn load_terrain_system(
    mut commands: Commands,
    mut query: Query<(
        Entity,
        &Terrain,
        &Name,
        &Transform,
        &TerrainMeshData,
        &TerrainLightmapData,
        &mut TerrainLoadState,
    )>,
    camera_query: Query<(&Transform, &Camera), (With<Camera3d>, Without<Terrain>)>,
    asset_server: Res<AssetServer>,
    _terrain_mesh: Res<TerrainMesh>,
    map_mesh_assets: Res<Assets<JMXVMAPM>>,
    lightmap_assets: Res<Assets<JMXVMAPT>>,
    lightmap_fallback: Res<TerrainLightmapFallback>,
    mut image_assets: ResMut<Assets<Image>>,
    mut mesh_assets: ResMut<Assets<Mesh>>,
    mut terrain_block_material_assets: ResMut<Assets<TerrainBlockSplatMaterial>>,
    // Exactly one water tier is inserted by `setup_terrain_mesh` (`graphics.water.quality`),
    // so both are optional and the spawn below picks whichever is present.
    water_material: Option<Res<WaterNormalMaterial>>,
    water_low_material: Option<Res<WaterLowMaterial>>,
    ice_material: Res<WaterIceMaterial>,
) {
    // No tile-index gate here any more: ground textures are resolved once, globally, by
    // `build_tile_atlas`, and a region's bind group simply retries until that atlas exists.
    let camera_pos = camera_query
        .iter()
        .find(|(_, camera)| camera.is_active)
        .map(|(transform, _)| transform.translation);

    // Regions with groups left to build, closest to the camera first: when a
    // boundary crossing queues several regions at once, the ground the player
    // can actually see finishes before the fog-hidden ring.
    let mut building = Vec::new();
    for (terrain_entity, _terrain, terrain_name, transform, map_data, lightmap_data, load_state) in
        query.iter_mut()
    {
        if !asset_server
            .get_load_state(&map_data.0)
            .unwrap_or(LoadState::NotLoaded)
            .is_loaded()
        {
            continue;
        }
        // Resolve the region's baked lightmap before building its groups so they pick it up.
        // Wait only while the `.t` is still loading; a missing or failed lightmap resolves to
        // `None` and the group builds unmodulated (white-fallback) rather than blocking forever.
        let lightmap = match asset_server.get_load_state(&lightmap_data.0) {
            Some(LoadState::Loading) => continue,
            _ => lightmap_assets
                .get(&lightmap_data.0)
                .and_then(|lm| lm.lightmap.clone()),
        };
        match load_state.as_ref() {
            TerrainLoadState::None | TerrainLoadState::BuildingMeshes { .. } => {
                let dist_sq = camera_pos
                    .map(|p| p.distance_squared(transform.translation))
                    .unwrap_or(0.0);
                building.push((
                    terrain_entity,
                    terrain_name,
                    map_data,
                    lightmap,
                    load_state,
                    dist_sq,
                ));
            }
            TerrainLoadState::LoadedMeshes => {}
            TerrainLoadState::Completed => {
                if let Ok(mut entity) = commands.get_entity(terrain_entity) {
                    entity.remove::<TerrainMeshData>();
                }
            }
        }
    }
    building.sort_by(|a, b| a.5.total_cmp(&b.5));

    let mut budget = GROUP_BUILDS_PER_FRAME;
    for (terrain_entity, terrain_name, map_data, lightmap, mut load_state, _dist_sq) in building {
        if budget <= 0 {
            break;
        }
        let map_data = map_mesh_assets
            .get(&map_data.0)
            .expect("failed to get map data");
        // Merge size is decided once (on the first frame this region builds) and
        // carried in the load state so `total_groups` is stable across the
        // multi-frame build.
        let (mut next_group, group_size) = match *load_state {
            TerrainLoadState::BuildingMeshes {
                next_group,
                group_size,
            } => (next_group, group_size),
            _ => (0, REGION_BLOCKS_PER_SIDE),
        };
        let groups_per_side = (REGION_BLOCKS_PER_SIDE + group_size - 1) / group_size;
        let total_groups = groups_per_side * groups_per_side;

        while budget > 0 && next_group < total_groups {
            let gx = next_group % groups_per_side;
            let gz = next_group / groups_per_side;
            next_group += 1;

            let origin_x = gx * group_size;
            let origin_z = gz * group_size;
            let mut group_blocks: Vec<(&TerrainBlock, f32, f32)> = Vec::new();
            for dz in 0..group_size {
                for dx in 0..group_size {
                    let bx = origin_x + dx;
                    let bz = origin_z + dz;
                    if bx >= REGION_BLOCKS_PER_SIDE || bz >= REGION_BLOCKS_PER_SIDE {
                        continue;
                    }
                    let block = &map_data.blocks[(bz * REGION_BLOCKS_PER_SIDE + bx) as usize];
                    group_blocks.push((block, (dx * 320) as f32, (dz * 320) as f32));
                }
            }
            if group_blocks.is_empty() {
                continue;
            }
            budget -= 1;

            let group_origin = (origin_x as f32 * -320.0, origin_z as f32 * 320.0);
            let group_transform = Transform {
                translation: Vec3::new(group_origin.0, 0.0, group_origin.1),
                scale: Vec3::new(-1.0, 1.0, 1.0),
                ..default()
            };
            let mesh = merge_block_meshes(
                map_data,
                &group_blocks,
                needs_winding_reversal(&group_transform.to_matrix()),
            );
            let mesh = mesh_assets.add(mesh);
            let material = TerrainBlockSplatMaterial::from(
                &group_blocks,
                lightmap
                    .clone()
                    .unwrap_or_else(|| lightmap_fallback.0.clone()),
                &mut image_assets,
            );
            let material = terrain_block_material_assets.add(material);
            let group_aabb = merged_group_aabb(&group_blocks);

            let Ok(mut entity) = commands.get_entity(terrain_entity) else {
                continue;
            };
            entity.with_children(|terrain_entity: &mut bevy::ecs::hierarchy::ChildSpawnerCommands| {
                terrain_entity.spawn((
                    Mesh3d(mesh),
                    MeshMaterial3d(material),
                    group_transform,
                    Visibility::default(),
                    group_aabb,
                    Name::from(format!("Ground group {}x{} ({})", gx, gz, terrain_name.as_str())),
                )).with_children(|group_entity: &mut bevy::ecs::hierarchy::ChildSpawnerCommands| {
                    for (block, dx, dz) in &group_blocks {
                        group_entity.spawn((
                            Transform::from_xyz(*dx, 0.0, *dz),
                            Visibility::default(),
                            (*block).clone(),
                            block.aabb.clone(),
                            DebugAabb::with_color(bevy::color::palettes::basic::GREEN),
                            GameCursorTarget::default(),
                            Name::from(format!("Ground {}x{} ({})", block.x, block.z, terrain_name.as_str())),
                        )).with_children(|block_entity: &mut bevy::ecs::hierarchy::ChildSpawnerCommands| match &block.water_type {
                            WaterType::None => {}
                            WaterType::Water(_wave_type, height) => {
                                // plane mesh vertices are located around plane center
                                let transform = Transform::from_xyz(320.0, *height, 320.0)
                                    .with_scale(Vec3::new(-1.0, 1.0, -1.0));
                                let name = Name::from(format!("Water {}x{} ({})", block.x, block.z, terrain_name.as_str()));
                                // The two tiers differ only in which material type the plane
                                // carries, so the mesh/transform/name are shared and only the
                                // `MeshMaterial3d` component type changes.
                                match (&water_material, &water_low_material) {
                                    (Some(hq), _) => {
                                        block_entity.spawn((
                                            Mesh3d(_terrain_mesh.0.clone()),
                                            MeshMaterial3d(hq.0.clone()),
                                            transform,
                                            Visibility::default(),
                                            WaterPlane,
                                            name,
                                        ));
                                    }
                                    (None, Some(low)) => {
                                        block_entity.spawn((
                                            Mesh3d(_terrain_mesh.0.clone()),
                                            MeshMaterial3d(low.0.clone()),
                                            transform,
                                            Visibility::default(),
                                            WaterPlane,
                                            name,
                                        ));
                                    }
                                    (None, None) => {}
                                }
                            }
                            WaterType::Ice(height) => {
                                block_entity.spawn((
                                    Mesh3d(_terrain_mesh.0.clone()),
                                    MeshMaterial3d(ice_material.0.clone()),
                                    Transform::from_xyz(320.0, *height, 320.0)
                                        .with_scale(Vec3::new(-1.0, 1.0, -1.0)),
                                    Visibility::default(),
                                    WaterPlane,
                                    Name::from(format!("Ice {}x{} ({})", block.x, block.z, terrain_name.as_str())),
                                ));
                            }
                        });
                    }
                });
            });
        }

        *load_state = if next_group >= total_groups {
            TerrainLoadState::LoadedMeshes
        } else {
            TerrainLoadState::BuildingMeshes {
                next_group,
                group_size,
            }
        };
    }
}

/// Union of a merge group's blocks' AABBs, each offset by its `(dx, dz)` group-local position
/// (see `merge_block_meshes`) — used purely for frustum culling of the merged mesh entity.
fn merged_group_aabb(blocks: &[(&TerrainBlock, f32, f32)]) -> bevy::camera::primitives::Aabb {
    let mut min = Vec3::splat(f32::MAX);
    let mut max = Vec3::splat(f32::MIN);
    for (block, dx, dz) in blocks {
        let offset = Vec3::new(*dx, 0.0, *dz);
        min = min.min(Vec3::from(block.aabb.min()) + offset);
        max = max.max(Vec3::from(block.aabb.max()) + offset);
    }
    bevy::camera::primitives::Aabb::from_min_max(min, max)
}

/// Water and ice animate in the fragment shader, so only the four corners are
/// needed. The 320-unit extent is one terrain block (16 cells of 20 units);
/// preserve its UVs and authored winding under scale=(-1,1,-1) placement.
pub fn water_patch_mesh() -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_POSITION,
        vec![
            [0.0, 0.0, 0.0],
            [320.0, 0.0, 0.0],
            [0.0, 0.0, 320.0],
            [320.0, 0.0, 320.0],
        ],
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 1.0, 0.0]; 4]);
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_UV_0,
        vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]],
    );
    mesh.insert_indices(Indices::U32(vec![2, 0, 3, 3, 0, 1]));
    mesh
}

#[cfg(test)]
mod water_tests {
    use super::*;
    use bevy::mesh::VertexAttributeValues;

    #[test]
    fn water_quad_preserves_extent_uvs_and_authored_winding() {
        let mesh = water_patch_mesh();
        let Some(VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("positions");
        };
        let Some(VertexAttributeValues::Float32x2(uvs)) = mesh.attribute(Mesh::ATTRIBUTE_UV_0)
        else {
            panic!("uvs");
        };
        assert_eq!(positions.len(), 4);
        for (p, uv) in positions.iter().zip(uvs) {
            assert_eq!(*p, [uv[0] * 320.0, 0.0, uv[1] * 320.0]);
        }
        let indices: Vec<_> = mesh.indices().unwrap().iter().collect();
        assert_eq!(indices.len(), 6);
        for tri in indices.chunks_exact(3) {
            let [a, b, c] = [tri[0], tri[1], tri[2]]
                .map(|i| Vec3::from_array(positions[i]) * Vec3::new(-1.0, 1.0, -1.0));
            // Compare against the actual old grid, whose authored winding
            // is opposite the supplied +Y normals. Preserve that convention.
            let reference = &crate::assets::m::block_mesh::BLOCK_INDICES[..3];
            let [ra, rb, rc] = [reference[0], reference[1], reference[2]]
                .map(|i| Vec3::new((i % 17) as f32 * -20.0, 0.0, (i / 17) as f32 * -20.0));
            assert!((b - a).cross(c - a).dot((rb - ra).cross(rc - ra)) > 0.0);
        }
    }
}
