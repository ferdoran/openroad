//! Ground foliage (grass) driven by the tile type in Map.pk2's tile2d.ifo.
//!
//! Two geometry sources feed one scatter pipeline (`graphics.foliage.mode`):
//! *native* — the original 1.188 3D-grass, i.e. each tile's authored
//! `{model,count}` pairs resolved through object.ifo to grass `.bsr` models in
//! the user's own Data.pk2; *pack* — bundled CC0 sprites (assets/foliage/)
//! scattered on Grass/LongGrass-typed tiles, a non-original extra.
//!
//! Per terrain block, all tufts are baked into one merged mesh per material
//! and attached as a child of the existing "Ground BxB" block entity: that
//! entity already carries the parsed `TerrainBlock` (per-vertex heights + tile
//! ids), lives in the exact local frame the ground vertices use (mirrored
//! group transform included), and despawns with its region — so foliage
//! streams, culls and unloads with the terrain for free. See `scatter.rs` for
//! the determinism story and the merge rationale.

use std::collections::HashMap;

use bevy::asset::{AssetPath, Handle, LoadState};
use bevy::camera::visibility::VisibilityRange;
use bevy::ecs::hierarchy::ChildOf;
use bevy::pbr::StandardMaterial;
use bevy::prelude::*;

use crate::assets::bms::mesh::JMXVBMS;
use crate::assets::bsr::resource::SroResource;
use crate::assets::ifo::tile::TileType;
use crate::assets::ifo::IFOAsset;
use crate::assets::m::TerrainBlock;
use crate::assets::t::JMXVMAPT;
use crate::plugins::config::graphics::FoliageMode;
use crate::plugins::config::ClientConfig;
use crate::plugins::map::assets::{MapsAssets, TileAssets};
use crate::plugins::map::terrain::{Terrain, TerrainLightmapData};
use crate::util::mesh::needs_winding_reversal;
use crate::GameState;

pub mod native;
pub mod pack;
pub mod scatter;
pub mod tint;

/// Merged-mesh builds per frame — the same main-thread budgeting pattern as
/// terrain's `GROUP_BUILDS_PER_FRAME` (a fully grassy block stamps thousands
/// of tufts, a multi-ms vertex copy). Builds are nearest-camera-first, so a
/// low budget fills the player's surroundings quickly while distant blocks
/// trickle in under fog cover. Grassless blocks are classified separately and
/// never touch this budget.
const FOLIAGE_BUILDS_PER_FRAME: usize = 2;
/// Fade band width of the optional view-distance cutoff (world units).
const VIEW_DISTANCE_FADE: f32 = 40.0;

/// Stamp-ready geometry of one foliage mesh part (block-local, canonical
/// winding; the merge step compensates for the mirrored terrain transform).
pub struct FoliagePartGeometry {
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    pub material: Handle<StandardMaterial>,
    /// Linear color factor folded into the per-vertex tint. Bevy's vertex
    /// color *replaces* `StandardMaterial::base_color`, so the material's
    /// authored base color moves here and the material itself goes white —
    /// preserving the untinted appearance at blend 0.
    pub color_factor: Vec3,
}

pub struct FoliageModel {
    pub parts: Vec<FoliagePartGeometry>,
}

pub enum NativeModelState {
    Loading(Handle<SroResource>),
    Ready(FoliageModel),
    Failed,
}

/// What a tile id means for foliage, resolved once from tile2d.ifo.
pub struct TileRecipe {
    /// Native `{model,count}` pairs, authored order.
    pub pairs: Vec<(u32, u16)>,
    /// Pack layer scatters on Grass/LongGrass-typed tiles.
    pub pack_eligible: bool,
}

#[derive(Resource, Default)]
pub struct FoliageLibrary {
    pub native: HashMap<u32, NativeModelState>,
    pub pack: Vec<FoliageModel>,
    /// Pack sprites still loading; drained by `pack::extract_pack_models`.
    pub pack_loading: Vec<Handle<Image>>,
    pub tile_recipes: HashMap<u16, TileRecipe>,
    /// Linear-space average color per grass tile id (`tint::resolve_tile_tints`).
    pub tile_tints: HashMap<u16, Vec3>,
    /// All grass tiles have their tint resolved; block builds wait for this
    /// (vertex colors are baked, so building early would freeze untinted grass).
    pub tints_resolved: bool,
    /// Shared double-sided clones of the grass `.bmt` materials (plus their
    /// authored base color, folded into vertex tints), one per labeled
    /// material path (see `native::foliage_material`).
    pub material_cache: HashMap<AssetPath<'static>, (Handle<StandardMaterial>, Vec3)>,
    /// Grass `.bmt` loads still in flight, keyed the same way as
    /// `material_cache`. This map exists only to *own* the handle across
    /// frames: `AssetServer::load` cancels the load when its last strong
    /// handle drops, so a locally-held handle abandoned on the not-yet-resident
    /// path restarts the archive read on every single frame, forever, and the
    /// model may never reach `Ready` at all (#741). Entries move into
    /// `material_cache` once the source material resolves.
    pub material_pending: HashMap<AssetPath<'static>, Handle<StandardMaterial>>,
    pub initialized: bool,
}

impl FoliageLibrary {
    fn any_native_loading(&self) -> bool {
        self.native
            .values()
            .any(|s| matches!(s, NativeModelState::Loading(_)))
    }
}

/// Marker: this block's foliage pass ran (even if it produced nothing).
#[derive(Component)]
pub struct FoliageDone;

/// Classified once by `classify_block_foliage_system`: this block has grass
/// tiles. Carries the pre-resolved model needs so the build system never
/// re-scans the block's 289 vertices while waiting.
#[derive(Component)]
pub struct FoliagePending {
    /// Deduped native model ids referenced by this block's tiles.
    pub needed_models: Vec<u32>,
}

/// Marker on the merged foliage mesh entities (debug toggles, live
/// view-distance updates).
#[derive(Component)]
pub struct FoliageBlock;

pub struct FoliagePlugin;

impl Plugin for FoliagePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FoliageLibrary>()
            .add_systems(
                PreUpdate,
                apply_foliage_settings.run_if(crate::plugins::settings::live::config_changed),
            )
            .add_systems(
                Update,
                (
                    init_foliage_library.run_if(|lib: Res<FoliageLibrary>| !lib.initialized),
                    extract_native_geometry.run_if(|lib: Res<FoliageLibrary>| {
                        lib.initialized && lib.any_native_loading()
                    }),
                    pack::extract_pack_models
                        .run_if(|lib: Res<FoliageLibrary>| !lib.pack_loading.is_empty()),
                    tint::resolve_tile_tints.run_if(|lib: Res<FoliageLibrary>| lib.initialized),
                    classify_block_foliage_system
                        .run_if(|lib: Res<FoliageLibrary>| lib.initialized),
                    build_block_foliage_system.run_if(|lib: Res<FoliageLibrary>| lib.initialized),
                )
                    .chain()
                    .run_if(in_state(GameState::Game)),
            );
    }
}

/// A `graphics.foliage` shape that forces a **rebuild** when it changes.
///
/// Density and mode are baked into the merged meshes at build time, so they
/// cannot be nudged on a live entity — the grass has to be scattered again.
/// `view_distance` is not in here on purpose: it is a `VisibilityRange`
/// component and can be swapped in place, which is the difference between a
/// slider that stutters and one that does not.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct FoliageBuildShape {
    mode: FoliageMode,
    density: f32,
    pack_density: f32,
    cast_shadows: bool,
}

impl FoliageBuildShape {
    fn of(config: &ClientConfig) -> Self {
        let foliage = &config.graphics.foliage;
        Self {
            mode: foliage.mode,
            density: foliage.density,
            pack_density: foliage.pack.density,
            cast_shadows: foliage.cast_shadows,
        }
    }
}

/// Applies a `graphics.foliage` change without a scene reload (#646), on the
/// live-settings mechanism (`plugins::settings::live`).
///
/// Two costs, two paths, which is the whole idea here:
///
/// * **`view_distance`** swaps the `VisibilityRange` on the existing merged
///   meshes. Cheap enough to drive from a slider.
/// * **mode / density / cast_shadows** are baked into those meshes, so they
///   need a rescatter: despawn the merged foliage, drop `FoliageDone` /
///   `FoliagePending` from the terrain blocks so the classify+build pair runs
///   again, and — for a mode change — reset the library, since which native
///   models and pack sprites are loaded is itself a function of the mode.
///
/// The `Local` shape diff matters: `resource_changed::<ClientConfig>` fires for
/// *any* config edit, and rescattering every grass block because a chat colour
/// changed would be a visible hitch for no reason.
pub fn apply_foliage_settings(
    config: Res<ClientConfig>,
    mut last: Local<Option<(FoliageBuildShape, f32)>>,
    mut library: ResMut<FoliageLibrary>,
    mut commands: Commands,
    merged: Query<Entity, With<FoliageBlock>>,
    built: Query<Entity, Or<(With<FoliageDone>, With<FoliagePending>)>>,
) {
    let shape = FoliageBuildShape::of(&config);
    let view_distance = config.graphics.foliage.view_distance;
    let previous = *last;
    *last = Some((shape, view_distance));

    let Some((old_shape, old_view_distance)) = previous else {
        // First observation only seeds the baseline: the normal build path has
        // not run yet, so there is nothing to tear down.
        return;
    };

    if shape != old_shape {
        for entity in merged.iter() {
            commands.entity(entity).despawn();
        }
        for entity in built.iter() {
            commands
                .entity(entity)
                .remove::<FoliageDone>()
                .remove::<FoliagePending>();
        }
        if shape.mode != old_shape.mode {
            // Which models are resident is a function of the mode, so the
            // library is rebuilt rather than patched. Tint values are restored
            // from the persistent loader table; materials load as before.
            *library = FoliageLibrary::default();
        }
        info!(
            "foliage: settings changed ({:?} -> {:?}), rescattering",
            old_shape.mode, shape.mode
        );
        return;
    }

    if view_distance != old_view_distance {
        for entity in merged.iter() {
            if view_distance > 0.0 {
                commands.entity(entity).insert(view_range(view_distance));
            } else {
                commands.entity(entity).remove::<VisibilityRange>();
            }
        }
    }
}

/// One-time setup once the tile index is resident: resolve which tile ids
/// carry foliage and kick off the native grass `.bsr` loads.
fn init_foliage_library(
    mut library: ResMut<FoliageLibrary>,
    config: Res<ClientConfig>,
    tile_assets: Res<TileAssets>,
    maps_assets: Res<MapsAssets>,
    ifo_assets: Res<Assets<IFOAsset>>,
    asset_server: Res<AssetServer>,
) {
    let mode = config.graphics.foliage.mode;
    if mode == FoliageMode::Off {
        library.initialized = true;
        info!("foliage: disabled (graphics.foliage.mode = off)");
        return;
    }
    let Some(tile_index) = ifo_assets
        .get(&tile_assets.tile_index)
        .and_then(|ifo| ifo.tile_info_index.as_ref())
    else {
        return; // retry next frame
    };
    let Some(object_index) = ifo_assets
        .get(&maps_assets.object_index)
        .and_then(|ifo| ifo.object_info_index.as_ref())
    else {
        return;
    };

    let native_active = matches!(mode, FoliageMode::Native | FoliageMode::Both);
    let mut model_ids = Vec::new();
    for (id, info) in &tile_index.tiles {
        let pack_eligible = matches!(
            info.tile_type(),
            Some(TileType::Grass | TileType::LongGrass)
        );
        if info.grass_3d.is_empty() && !pack_eligible {
            continue;
        }
        if native_active {
            model_ids.extend(info.grass_3d.iter().map(|(model, _)| *model));
        }
        library.tile_recipes.insert(
            *id,
            TileRecipe {
                pairs: info.grass_3d.clone(),
                pack_eligible,
            },
        );
    }

    if matches!(mode, FoliageMode::Pack | FoliageMode::Both) {
        pack::start_loading(&mut library, &asset_server);
    }

    model_ids.sort_unstable();
    model_ids.dedup();
    for model_id in model_ids {
        let Some(object) = object_index.0.get(&model_id) else {
            warn!("foliage: tile2d 3D-grass model {model_id} missing from object.ifo");
            continue;
        };
        let handle: Handle<SroResource> =
            asset_server.load(format!("data://{}", object.path.display()));
        library
            .native
            .insert(model_id, NativeModelState::Loading(handle));
    }

    info!(
        "foliage: mode {:?}, {} grass tile ids, {} native models",
        mode,
        library.tile_recipes.len(),
        library.native.len()
    );
    library.initialized = true;
}

/// Copy loaded grass `.bsr` resources into stamp-ready geometry (runs only
/// while any model is still in `Loading`).
fn extract_native_geometry(
    mut library: ResMut<FoliageLibrary>,
    asset_server: Res<AssetServer>,
    resources: Res<Assets<SroResource>>,
    bms_assets: Res<Assets<JMXVBMS>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let FoliageLibrary {
        native,
        material_cache,
        material_pending,
        ..
    } = &mut *library;
    for (model_id, state) in native.iter_mut() {
        let NativeModelState::Loading(handle) = state else {
            continue;
        };
        if matches!(asset_server.load_state(&*handle), LoadState::Failed(_)) {
            warn!("foliage: grass model {model_id} failed to load");
            *state = NativeModelState::Failed;
            continue;
        }
        if !asset_server.is_loaded_with_dependencies(&*handle) {
            continue;
        }
        let Some(resource) = resources.get(&*handle) else {
            continue;
        };
        if let Some(model) = native::extract_model(
            resource,
            &bms_assets,
            &asset_server,
            &mut materials,
            material_cache,
            material_pending,
        ) {
            *state = NativeModelState::Ready(model);
        }
    }
}

/// Scatter + merge foliage for freshly built terrain blocks and attach the
/// merged meshes as block children.
/// One-time per-block classification: scans the block's vertices exactly once.
/// Grassless blocks (the vast majority) are marked done immediately and never
/// touch the build budget (letting them queue behind grassy builds re-scanned
/// them every frame and starved them once the budget was spent); grassy blocks
/// get a `FoliagePending` carrying their deduped model needs.
fn classify_block_foliage_system(
    mut commands: Commands,
    library: Res<FoliageLibrary>,
    config: Res<ClientConfig>,
    blocks: Query<(Entity, &TerrainBlock), (Without<FoliageDone>, Without<FoliagePending>)>,
) {
    for (entity, block) in blocks.iter() {
        let mut needed_models = Vec::new();
        let mut has_grass = false;
        if config.graphics.foliage.mode != FoliageMode::Off {
            for v in &block.vertices {
                if let Some(recipe) = library.tile_recipes.get(&v.texture_id) {
                    has_grass = true;
                    needed_models.extend(recipe.pairs.iter().map(|(model, _)| *model));
                }
            }
        }
        if !has_grass {
            commands.entity(entity).insert(FoliageDone);
            continue;
        }
        needed_models.sort_unstable();
        needed_models.dedup();
        commands
            .entity(entity)
            .insert(FoliagePending { needed_models });
    }
}

#[allow(clippy::too_many_arguments)]
fn build_block_foliage_system(
    mut commands: Commands,
    library: Res<FoliageLibrary>,
    config: Res<ClientConfig>,
    blocks: Query<(
        Entity,
        &TerrainBlock,
        &FoliagePending,
        &ChildOf,
        &GlobalTransform,
    )>,
    camera_query: Query<(&Transform, &Camera), With<Camera3d>>,
    parents: Query<&ChildOf>,
    terrains: Query<(&Terrain, Option<&TerrainLightmapData>)>,
    transforms: Query<&Transform, Without<Camera3d>>,
    lightmap_assets: Res<Assets<JMXVMAPT>>,
    mut meshes: ResMut<Assets<Mesh>>,
    debug_settings: Res<crate::plugins::dev::render_debug::RenderDebugSettings>,
) {
    let foliage = &config.graphics.foliage;
    let native_active = matches!(foliage.mode, FoliageMode::Native | FoliageMode::Both);
    let pack_configured = matches!(foliage.mode, FoliageMode::Pack | FoliageMode::Both);
    // wait for the pack sprites to resolve so early blocks aren't built
    // without the pack layer and then frozen by FoliageDone
    if pack_configured && !library.pack_loading.is_empty() {
        return;
    }
    // same freeze rationale for the baked vertex tints
    if !library.tints_resolved {
        return;
    }
    let pack_active = pack_configured && !library.pack.is_empty();

    // nearest-camera-first, like load_terrain_system: the player's
    // surroundings green up first while distant blocks fill in under the fog
    let camera_pos = camera_query
        .iter()
        .find(|(_, camera)| camera.is_active)
        .map(|(transform, _)| transform.translation);
    let mut ready: Vec<(Entity, f32)> = blocks
        .iter()
        .filter(|(_, _, pending, _, _)| {
            // readiness = a few HashMap lookups on the pre-resolved needs
            !native_active
                || pending.needed_models.iter().all(|model| {
                    !matches!(
                        library.native.get(model),
                        Some(NativeModelState::Loading(_))
                    )
                })
        })
        .map(|(entity, _, _, _, global)| {
            let dist_sq = camera_pos
                .map(|c| c.distance_squared(global.translation()))
                .unwrap_or(0.0);
            (entity, dist_sq)
        })
        .collect();
    ready.sort_by(|a, b| a.1.total_cmp(&b.1));

    for (entity, _) in ready.into_iter().take(FOLIAGE_BUILDS_PER_FRAME) {
        let Ok((_, block, _, child_of, _)) = blocks.get(entity) else {
            continue;
        };
        // ground group (mirrored) -> region root (Terrain)
        let group = child_of.parent();
        let Ok(region_root) = parents.get(group) else {
            continue;
        };
        let Ok((terrain, lightmap_data)) = terrains.get(region_root.parent()) else {
            continue;
        };
        let (rx, rz) = terrain.to_x_z();
        let region = (rz as u16) << 8 | rx as u16;
        // the region's baked per-cell light grid (96x96, one byte per
        // scatter cell) darkens grass to match the baked ground shading
        let tile_light = lightmap_data
            .and_then(|data| lightmap_assets.get(&data.0))
            .map(|t| t.tile_light.as_slice());
        // same winding rule as the merged ground mesh: compensate the
        // mirrored group transform (derived, not hardcoded)
        let reverse_winding = transforms
            .get(group)
            .map(|t| needs_winding_reversal(&t.to_matrix()))
            .unwrap_or(true);

        let tint = scatter::TintContext {
            tile_tints: &library.tile_tints,
            blend_native: foliage.tint.tile_blend_native,
            blend_pack: foliage.tint.tile_blend_pack,
            tile_light,
            light_strength: foliage.tint.baked_light,
        };
        // the debug panel's multiplier/distance ride on top of the config
        // values (seeded from them at startup, see render_debug.rs)
        let density_scale = debug_settings.foliage_density.max(0.0);
        let builds = scatter::build_block_foliage(
            block,
            region,
            &library,
            native_active,
            foliage.density.max(0.0) * density_scale,
            pack_active,
            foliage.pack.density.max(0.0) * density_scale,
            reverse_winding,
            &tint,
        );

        let mut tufts = 0;
        commands.entity(entity).with_children(|block_entity| {
            for build in builds {
                tufts += build.tufts;
                let mut spawned = block_entity.spawn((
                    Mesh3d(meshes.add(build.mesh)),
                    MeshMaterial3d(build.material),
                    Transform::IDENTITY,
                    if debug_settings.render_foliage {
                        Visibility::Inherited
                    } else {
                        Visibility::Hidden
                    },
                    build.aabb,
                    FoliageBlock,
                    Name::from(format!("Foliage {}x{}", block.x, block.z)),
                ));
                // cast_shadows off: each shadow cascade would otherwise
                // re-draw every tuft with alpha-masked fragment work; the
                // marker also makes grass a nameplate non-occluder (see
                // hud/nameplates.rs). Shadow-casting grass is the better
                // look under the scoped default cascades — config decides.
                if !foliage.cast_shadows {
                    spawned.insert(bevy::light::NotShadowCaster);
                }
                if debug_settings.foliage_view_distance > 0.0 {
                    spawned.insert(view_range(debug_settings.foliage_view_distance));
                }
            }
        });
        if tufts > 0 {
            debug!(
                "foliage: region {:04x} block {}x{}: {} tufts",
                region, block.x, block.z, tufts
            );
        }
        commands
            .entity(entity)
            .remove::<FoliagePending>()
            .insert(FoliageDone);
    }
}

/// Distance cutoff of the optional `view_distance` config, with a dithered
/// fade band so grass doesn't pop.
pub fn view_range(view_distance: f32) -> VisibilityRange {
    VisibilityRange {
        start_margin: 0.0..0.0,
        end_margin: (view_distance - VIEW_DISTANCE_FADE).max(0.0)..view_distance,
        use_aabb: true,
    }
}
