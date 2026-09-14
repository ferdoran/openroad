use std::f32::consts::PI;
use std::time::Duration;

use bevy::image::ImageLoaderSettings;
use bevy::light::{CascadeShadowConfig, CascadeShadowConfigBuilder, DirectionalLightShadowMap};
use bevy::prelude::*;
use bevy::time::common_conditions::on_timer;
use bevy_asset_loader::prelude::{ConfigureLoadingState, LoadingStateAppExt, LoadingStateConfig};

use crate::assets::m::block_splat_material::{
    TerrainAmbientRatioPlugin, TerrainBlockSplatMaterial,
};
use crate::plugins::map::assets::{MapsAssets, TileAssets};
use crate::plugins::map::objects::*;
use crate::plugins::map::terrain::{
    load_terrain_dynamically, load_terrain_system, water_patch_mesh, Terrain, TerrainLoadState,
    TerrainMesh, WaterIceMaterial, WaterLowMaterial, WaterNormalMaterial,
};
use crate::plugins::map::water_hq_material::{
    HighQualityWaterMaterial, WaterExtension, WaterHqSettings,
};
use crate::scenes::SceneState;
use crate::GameState;

pub mod assets;
mod events;
pub mod foliage;
pub mod objects;
pub mod terrain;
pub mod water_hq_material;
pub mod water_material;

pub struct MapPlugin;
impl Plugin for MapPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Terrain>()
            .register_type::<TerrainLoadState>()
            .init_resource::<SroMeshes>()
            .init_resource::<SroBindPoses>()
            .init_resource::<SpawnedMapObjects>()
            .init_resource::<objects::UnknownObjectIds>()
            .configure_loading_state(
                LoadingStateConfig::new(SceneState::Loading)
                    .load_collection::<TileAssets>()
                    .load_collection::<MapsAssets>(),
            );

        app.add_plugins(MaterialPlugin::<TerrainBlockSplatMaterial>::default())
            .add_plugins(TerrainAmbientRatioPlugin)
            .add_plugins(MaterialPlugin::<HighQualityWaterMaterial>::default())
            .add_plugins(MaterialPlugin::<water_material::LowQualityWaterMaterial>::default())
            .add_plugins(crate::plugins::skybox::SkyboxPlugin)
            .add_plugins(crate::plugins::environment::EnvironmentPlugin)
            .add_plugins(foliage::FoliagePlugin)
            .add_systems(
                OnExit(GameState::Loading),
                (setup_terrain_mesh, setup_lighting),
            )
            // The per-system run conditions skip the load-state polling
            // entirely once streaming settles: `TerrainLoadState` is removed
            // when a region completes and `LoadingCompound`/`LoadingResources`
            // are removed once their assets spawned, so in steady state all
            // conditions are archetypally empty and only the (camera-gated)
            // dynamic loader still runs.
            .add_systems(
                Update,
                (
                    load_terrain_system.run_if(any_with_component::<TerrainLoadState>),
                    load_terrain_dynamically,
                    // Must precede the object pass: it re-arms `TerrainLoadState`
                    // on completed regions when a neighbour unloads, and the pass
                    // is what consumes that (#571).
                    rearm_object_passes_on_region_unload,
                    load_terrain_objects_system.run_if(any_with_component::<TerrainLoadState>),
                    load_compound_system.run_if(any_with_component::<LoadingCompound>),
                    load_resources_system.run_if(any_with_component::<LoadingResources>),
                )
                    .chain()
                    .run_if(in_state(GameState::Game))
                    // Dungeon interiors are not part of the region grid: the
                    // overworld streamer stands down entirely while one is
                    // active (explicit guard — previously only the mfo
                    // bitmap's empty upper half kept dungeon ids out).
                    .run_if(not(resource_exists::<
                        crate::plugins::dungeon::ActiveDungeon,
                    >)),
            )
            .add_systems(
                Update,
                generate_water_normal_mips.run_if(resource_exists::<WaterNormalMapImage>),
            )
            .add_systems(
                Update,
                cull_fogged_objects.run_if(in_state(GameState::Game)),
            )
            // not gated on GameState::Game so it also sweeps after leaving it
            .add_systems(
                Update,
                prune_spawn_caches.run_if(on_timer(Duration::from_secs(10))),
            );
    }
}

pub fn setup_terrain_mesh(
    mut commands: Commands,
    mut mesh_assets: ResMut<Assets<Mesh>>,
    mut ice_material_assets: ResMut<Assets<StandardMaterial>>,
    mut hq_water_material_assets: ResMut<Assets<HighQualityWaterMaterial>>,
    mut low_water_material_assets: ResMut<Assets<water_material::LowQualityWaterMaterial>>,
    config: Res<crate::plugins::config::ClientConfig>,
    asset_server: Res<AssetServer>,
) {
    use crate::plugins::config::graphics::WaterQuality;
    let water_quality = config.graphics.water.quality;
    let mesh = water_patch_mesh();
    let mesh_handle = mesh_assets.add(mesh);
    commands.insert_resource(TerrainMesh(mesh_handle));

    let water_tex: Handle<Image> = asset_server.load("map://water/water101.ddj");
    let ice_tex: Handle<Image> = asset_server.load("map://water/water201.ddj");
    // Only the selected tier's material is created and inserted; the terrain streamer
    // branches on which of the two resources exists, so exactly one water draw path is
    // ever live and the unused tier costs nothing.
    if water_quality == WaterQuality::Low {
        let water_mat = water_material::LowQualityWaterMaterial {
            base: StandardMaterial {
                base_color: Color::WHITE.with_alpha(0.75),
                base_color_texture: Some(water_tex),
                // Blend, not the HQ tier's Opaque: with no `specular_transmission` there is
                // no transmissive branch to fall through to, so plain alpha blending is what
                // keeps the lakebed visible — and it avoids the Transmissive phase's
                // per-frame full-resolution copy of the main texture entirely.
                alpha_mode: AlphaMode::Blend,
                perceptual_roughness: 0.2,
                metallic: 0.0,
                reflectance: 0.02,
                ..default()
            },
            extension: crate::plugins::map::water_material::WaterLowExtension {
                settings: crate::plugins::map::water_material::WaterLowSettings::default(),
            },
        };
        // Ice drops transmission for the same reason: a single transmissive mesh anywhere in
        // view is enough to make the renderer take the transmission-texture copy, so leaving
        // ice transmissive here would give back most of what the low tier just saved.
        let ice_mat = StandardMaterial {
            base_color: Color::WHITE.with_alpha(0.8),
            base_color_texture: Some(ice_tex),
            alpha_mode: AlphaMode::Blend,
            perceptual_roughness: 0.1,
            metallic: 1.0,
            reflectance: 0.1,
            ..default()
        };
        commands.insert_resource(WaterLowMaterial(low_water_material_assets.add(water_mat)));
        commands.insert_resource(WaterIceMaterial(ice_material_assets.add(ice_mat)));
        return;
    }

    // Procedurally generated (not SRO data — see `tools/src/bin/gen_water_normal`); must load
    // non-sRGB like any normal map, or its packed xyz would get gamma-corrected as color.
    let normal_map: Handle<Image> = asset_server
        .load_builder()
        .with_settings::<ImageLoaderSettings>(|settings| {
            settings.is_srgb = false;
        })
        .load("textures/water_normal.png");
    // PNGs load without mips; `generate_water_normal_mips` builds the chain
    // once the image is in, so the huge tiling water surface stops sampling
    // full-res at distance (shimmer + texture-cache thrash).
    commands.insert_resource(WaterNormalMapImage(normal_map.clone()));

    // Default/high graphics tier — see `water_hq_material.rs` for what the extension adds.
    let water_mat = HighQualityWaterMaterial {
        base: StandardMaterial {
            base_color: Color::WHITE,
            base_color_texture: Some(water_tex),
            // Deliberately not AlphaMode::Blend: Bevy's material queueing (bevy_pbr
            // material.rs) checks alpha_mode before `reads_view_transmission_texture`, and
            // Blend wins outright, routing the mesh into the plain alpha-blended Transparent
            // phase instead of the Transmissive phase specular_transmission depends on — the
            // water would still request the transmission texture but never get drawn in the
            // pass that provides it, making it end up effectively invisible. Opaque (the
            // default) is what actually falls through to the transmissive branch.
            specular_transmission: 0.9,
            diffuse_transmission: 0.1,
            ior: 1.33,
            thickness: 1.0,
            perceptual_roughness: 0.02,
            metallic: 0.0,
            reflectance: 0.02,
            ..default()
        },
        extension: WaterExtension {
            settings: WaterHqSettings::default(),
            normal_map,
        },
    };

    let ice_mat = StandardMaterial {
        base_color: Color::WHITE,
        base_color_texture: Some(ice_tex),
        specular_transmission: 0.9,
        diffuse_transmission: 0.1,
        ior: 1.31,
        thickness: 2.0,
        perceptual_roughness: 0.02,
        metallic: 1.0,
        reflectance: 0.1,
        ..default()
    };

    commands.insert_resource(WaterNormalMaterial(hq_water_material_assets.add(water_mat)));
    commands.insert_resource(WaterIceMaterial(ice_material_assets.add(ice_mat)));
}

/// One-shot marker: the water normal map still needs its mip chain built
/// (removed once done — see `generate_water_normal_mips`).
#[derive(Resource)]
pub struct WaterNormalMapImage(Handle<Image>);

/// Rewrites the (mipless) water normal PNG with a renormalizing mip chain
/// once it finishes loading, then removes the marker resource. Changing
/// `mip_level_count` alters the texture descriptor, so the GPU texture is
/// recreated with the full chain and the water material re-prepares
/// automatically.
pub fn generate_water_normal_mips(
    handle: Res<WaterNormalMapImage>,
    mut images: ResMut<Assets<Image>>,
    mut commands: Commands,
) {
    // Immutable get first: the asset must only be change-flagged when the
    // chain is actually written.
    let Some(image) = images.get(&handle.0) else {
        return;
    };
    if image.texture_descriptor.format != bevy::render::render_resource::TextureFormat::Rgba8Unorm {
        warn!(
            "water normal map is {:?}, expected Rgba8Unorm; skipping mip generation",
            image.texture_descriptor.format
        );
        commands.remove_resource::<WaterNormalMapImage>();
        return;
    }
    if image.texture_descriptor.mip_level_count == 1 {
        let size = image.texture_descriptor.size;
        let Some(mip0) = image.data.clone() else {
            warn!("water normal map has no CPU-side data; cannot build mips");
            commands.remove_resource::<WaterNormalMapImage>();
            return;
        };
        let (levels, data) =
            crate::util::mips::rgba8_normal_mip_chain(size.width, size.height, &mip0);
        let Some(mut image) = images.get_mut(&handle.0) else {
            return;
        };
        image.texture_descriptor.mip_level_count = levels;
        image.data = Some(data);
    }
    commands.remove_resource::<WaterNormalMapImage>();
}

/// The Sun's cascade config per render mode. Vanilla: only the player casts
/// (`environment::apply_vanilla_shadow_casters`), so the maps only need to
/// cover the camera→ground range (follow camera is 40..400 units out, plus
/// pitch) — tight bounds keep the texels small and the character shadow
/// crisp. PBR: everything casts, range comes from config; note huge distances
/// spread the map thin (see the `ShadowSettings::distance` doc). Applied at
/// Sun spawn here and re-inserted on mode flips by
/// `environment::apply_render_mode` (a runtime `CascadeShadowConfig` insert
/// takes effect the next frame).
///
/// The cascade COUNT is deliberately identical in both modes: growing
/// `bounds.len()` on a live light panics in bevy_light's
/// `check_dir_light_mesh_visibility` (the issue-#207 mechanism — stale
/// per-thread queues sized for the old count are indexed unconditionally, so
/// 2→3 cascades = index out of bounds). Only the depth range differs per mode.
pub fn sun_cascade_config(
    vanilla: bool,
    shadows: &crate::plugins::config::graphics::ShadowSettings,
) -> CascadeShadowConfig {
    if vanilla {
        vanilla_cascade_config(shadows, VANILLA_SHADOW_BASE_DISTANCE)
    } else {
        CascadeShadowConfigBuilder {
            num_cascades: shadows.cascades.clamp(1, 4),
            maximum_distance: shadows.distance.max(1.0),
            ..default()
        }
        .build()
    }
}

/// Baseline vanilla cascade range: covers the follow camera's full zoom
/// (40..400 units) plus pitch with room to spare.
pub const VANILLA_SHADOW_BASE_DISTANCE: f32 = 800.0;

/// The vanilla cascade config at a given range. Cascades are scoped to the
/// VIEW frustum, so a free/debug camera watching the player from afar needs a
/// longer range than the follow camera —
/// `environment::scale_vanilla_cascades` stretches `maximum_distance` to the
/// player's view depth each frame and calls this. Cascade count comes from
/// config in both modes (see `sun_cascade_config` on why it must not change).
pub fn vanilla_cascade_config(
    shadows: &crate::plugins::config::graphics::ShadowSettings,
    maximum_distance: f32,
) -> CascadeShadowConfig {
    CascadeShadowConfigBuilder {
        num_cascades: shadows.cascades.clamp(1, 4),
        first_cascade_far_bound: 150.0,
        maximum_distance: maximum_distance.max(VANILLA_SHADOW_BASE_DISTANCE),
        ..default()
    }
    .build()
}

pub fn setup_lighting(
    mut commands: Commands,
    config: Res<crate::plugins::config::ClientConfig>,
    env: Res<crate::plugins::environment::EnvironmentSettings>,
) {
    commands.insert_resource(GlobalAmbientLight {
        color: Color::WHITE,
        brightness: 100.0,
        ..default()
    });

    // Clear to the fog color rather than black: it is normally hidden behind the opaque
    // skybox, but shows through the brief prepass-vs-main-pass gap of a freshly streamed
    // object, so a fog-colored clear makes distant pop-in blend into the fog. The
    // environment system keeps this in sync with time-of-day (see `apply_environment`).
    commands.insert_resource(ClearColor(terrain::rendering::FOG_COLOR.into()));

    // Shadows come from config (`graphics.shadows`), scoped down: few, short
    // cascades — the baked terrain lightmap carries distant shading, so the
    // dynamic maps only need to cover the player's surroundings. The old
    // 100k-unit default re-rendered all loaded terrain/objects per frame
    // (the perf note on `RenderDebugSettings::enable_shadows`, which
    // remains the live toggle on top of this).
    let shadows = &config.graphics.shadows;
    commands.insert_resource(DirectionalLightShadowMap {
        size: (shadows.map_size.max(512).next_power_of_two()) as usize,
    });
    commands.spawn((
        DirectionalLight {
            // The maps stay on in both render modes; the vanilla/PBR switch
            // (`EnvironmentSettings.enabled`, hotkey N) instead decides who
            // casts into them (player-only vs everything — see
            // `environment::apply_vanilla_shadow_casters`) and how the
            // cascades are scoped (`sun_cascade_config`).
            shadow_maps_enabled: shadows.enabled,
            illuminance: light_consts::lux::AMBIENT_DAYLIGHT,
            ..default()
        },
        sun_cascade_config(env.enabled, shadows),
        Transform::from_rotation(Quat::from_rotation_x(-PI / 4.)),
        crate::plugins::environment::Sun,
        Name::from("Sun"),
    ));
}
