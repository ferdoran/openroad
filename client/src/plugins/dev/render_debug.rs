use bevy::camera::{Hdr, RenderTarget};
use bevy::ecs::resource::IsResource;
use bevy::pbr::wireframe::{Wireframe, WireframeColor};
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use bevy::render::batching::NoAutomaticBatching;
use bevy::render::render_resource::Face;
use bevy_inspector_egui::prelude::ReflectInspectorOptions;
use bevy_inspector_egui::quick::ResourceInspectorPlugin;
use bevy_inspector_egui::InspectorOptions;

use crate::assets::m::block_splat_material::TerrainBlockSplatMaterial;
use crate::assets::o2::MapObject;
use crate::plugins::config::ClientConfig;
use crate::plugins::effects::material::SroEffectMaterial;
use crate::plugins::effects::{
    EffectAdditiveIntensity, EffectInstance, EffectLdrAdditive, EffectPlaybackSpeed,
    EffectsEnabled, LeafEmitPolicy,
};
use crate::plugins::map::terrain::rendering;
use bevy::pbr::MeshMaterial3d;

/// The effect-runtime knobs `on_settings_changed` drives, bundled to stay
/// under the system-parameter arity limit.
#[derive(bevy::ecs::system::SystemParam)]
struct EffectDebugParams<'w> {
    enabled: ResMut<'w, EffectsEnabled>,
    leaf: ResMut<'w, LeafEmitPolicy>,
    additive: ResMut<'w, EffectAdditiveIntensity>,
    ldr: ResMut<'w, EffectLdrAdditive>,
    speed: ResMut<'w, EffectPlaybackSpeed>,
    materials: ResMut<'w, Assets<SroEffectMaterial>>,
}

/// The [`RenderDebugSettings`] resource itself, its config seeding, and the
/// systems that apply it — **always on**, deliberately.
///
/// This used to live entirely behind `dev_tools`, which crashed the client in
/// the shipped default configuration: `RenderDebugSettings` is not only the
/// debug panel's state any more, it is the render feature-toggle resource that
/// three *shipping* systems read as a plain `Res<_>` —
/// `animation_culling::cull_distant_animations`, `map::objects::cull_fogged_objects`
/// and the foliage block builder. With `dev_tools: false` the resource did not
/// exist and system-parameter validation panicked on the first frame.
///
/// The seeding and apply systems are part of this, not of the inspector, for a
/// second reason: `RenderDebugSettings::default()` is *not* the configured
/// state. `enable_shadows` defaults to `false` and `foliage_view_distance` to
/// `0.0` (which means "never cull"), so registering the resource without
/// `seed_terrain_settings_from_config` would silently disable shadows and
/// silently disable the foliage cull — trading a loud crash for two quiet
/// regressions, one of them a performance one.
///
/// Only [`RenderControlsInspectorPlugin`] is genuinely dev-only.
pub(crate) struct RenderControlsPlugin;

impl Plugin for RenderControlsPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<RenderDebugSettings>()
            .init_resource::<RenderDebugSettings>()
            .add_systems(Startup, seed_terrain_settings_from_config)
            .add_systems(
                Update,
                (
                    on_settings_changed,
                    on_foliage_settings_changed,
                    on_water_settings_changed,
                )
                    .run_if(resource_changed::<RenderDebugSettings>),
            );
    }
}

/// The egui panel that edits [`RenderDebugSettings`] — the only dev-gated half.
pub(crate) struct RenderControlsInspectorPlugin;

impl Plugin for RenderControlsInspectorPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(
            ResourceInspectorPlugin::<RenderDebugSettings>::default()
                .run_if(super::dev_windows_visible),
        );
    }
}

/// Copies the config-driven graphics values into the debug panel's fields,
/// so the panel starts on the configured state instead of silently
/// reverting it on the first unrelated settings change (the bloom two-tier
/// pattern: config = master, this panel = the live toggle on top).
fn seed_terrain_settings_from_config(
    config: Res<crate::plugins::config::ClientConfig>,
    mut settings: ResMut<RenderDebugSettings>,
) {
    use crate::plugins::config::graphics::TerrainLightingConfig;
    settings.terrain_lighting_mode = match config.graphics.terrain.lighting {
        TerrainLightingConfig::Dynamic => 0,
        TerrainLightingConfig::FlatBaked => 1,
        TerrainLightingConfig::Baked => 2,
    };
    settings.terrain_lightmap_flip_v = config.graphics.terrain.lightmap_flip_v;
    settings.enable_shadows = config.graphics.shadows.enabled;
    settings.foliage_view_distance = config.graphics.foliage.view_distance;
    settings.render_scale = config.graphics.render_scale.factor();
}

#[derive(Resource, Reflect, InspectorOptions)]
#[reflect(Resource, InspectorOptions)]
pub struct RenderDebugSettings {
    pub render_terrain: bool,
    pub render_objects: bool,
    /// Water and ice surfaces of both `graphics.water.quality` tiers — the
    /// `make perf attribute` A/B switch for the water shader's per-pixel cost.
    pub render_water: bool,
    /// Ground foliage (grass) master toggle — the `make perf` A/B switch.
    /// Only meaningful when `graphics.foliage.mode` isn't `off`.
    pub render_foliage: bool,
    /// Density multiplier on top of `graphics.foliage.density`. Applies to
    /// blocks built after the change — walk to fresh terrain for a clean A/B.
    #[inspector(min = 0.0, max = 2.0)]
    pub foliage_density: f32,
    /// Foliage draw distance in world units, applied live (0 = unlimited /
    /// max fidelity). Seeded from `graphics.foliage.view_distance`.
    #[inspector(min = 0.0, max = 2000.0)]
    pub foliage_view_distance: f32,
    /// Particle/visual effects: hides all effect wrappers and pauses the
    /// whole effect runtime (zero CPU cost) — for A/B-ing its FPS impact.
    pub render_effects: bool,
    /// Leaf self-emission for ALL effects (default on — exe-faithful:
    /// StaticEmit always emits; off = leaf emitters degrade to single
    /// plates replaying their envelope once per loop). Applies to effects
    /// spawned after the toggle — walk to fresh terrain or reload the
    /// scene for a clean A/B.
    pub leaf_emit_global: bool,
    /// Density multiplier on authored leaf particle counts when
    /// `leaf_emit_global` is on (calibration sweep: 0.25 / 0.5 / 1.0).
    #[inspector(min = 0.0, max = 1.0)]
    pub leaf_emit_density: f32,
    /// Brightness multiplier for ADDITIVE effects (dst blend ONE) — the
    /// LDR-vs-HDR calibration dial: the original saturated an 8-bit
    /// backbuffer where our HDR + bloom keeps accumulating, so dense
    /// additive stacks (Seal auras, glows) can read hotter than authored.
    /// Applies live to all effect materials. 1.0 = authored data.
    #[inspector(min = 0.0, max = 2.0)]
    pub effect_additive_intensity: f32,
    /// Global effect playback-rate multiplier (< 1.0 slows all effects:
    /// ages, emission pacing, program scheduling, velocities, trails).
    /// Calibration dial against the original client; 1.0 = the exe-derived
    /// 20 fps effect timebase as-is. Applies live to running effects.
    #[inspector(min = 0.05, max = 2.0)]
    pub effect_time_scale: f32,
    /// LDR-additive emulation on additive (dst ONE) effect materials:
    /// saturating blend + 8-bit output quantization, reproducing the
    /// original's LDR backbuffer where stacked plates clamp at white
    /// (purple stays purple, no bloom blowout) and sub-1/255 wisps vanish
    /// (smoke tails end where the original's did). Off = unbounded HDR
    /// accumulation (the previous behavior), for A/B.
    pub effect_ldr_additive: bool,
    /// Terrain nav mesh debug lines + cursor hit marker (hotkey: T).
    pub render_navmesh: bool,
    /// Object (`.bms`) nav mesh debug lines: walkable triangles and
    /// flag-coloured outline/inline edges (hotkey: Y). Separate from
    /// `render_navmesh` because object meshes are dense enough to bury the
    /// terrain lines when both are on.
    pub render_object_navmesh: bool,
    /// Skeletal animation master switch: off freezes every animated rig
    /// (map props, NPCs, the player) at its current pose by stashing the
    /// roots' graph handles — the same zero-cost gate the distance culling
    /// uses (see `animation_culling`). Playback resumes where it froze.
    pub play_animations: bool,
    /// Terrain lighting model A/B (gap #6 in
    /// `docs/rendering-mobile-shader-comparison.md`): 0 = dynamic PBR over
    /// the baked lightmap (current), 1 = flat baked with the time-of-day
    /// ambient tint, 2 = fully baked albedo × lightmap (mobile port /
    /// original ground — no day/night response). Rides the shared
    /// `TerrainRenderParams` buffer, so cycling it never re-prepares a
    /// material (see the bind-group-leak note below). Seeded from
    /// `graphics.terrain.lighting`.
    #[inspector(min = 0, max = 2)]
    pub terrain_lighting_mode: u32,
    /// Mirror the baked terrain lightmap's V axis — the pending `.t`
    /// row-order calibration (`docs/formats/mapt-jmxvmapt.md`): flip if
    /// baked shadows sit on the wrong side of trees/buildings along Z.
    pub terrain_lightmap_flip_v: bool,
    /// Ground-tile repeat factors per splat-scale code (0/8/16/24/32 —
    /// docs/formats/mapm-jmxvmapm.md; the Splat teleport spots visit the
    /// 24/32 patches). Live through the shared params buffer. Playtest
    /// verdict 2026-08-10: the field does NOT drive tiling — a constant
    /// 0.25 for every code matches vanilla (checked on city pavement for
    /// code 16 and at the census spots for 24/32). Kept as sliders for
    /// future re-calibration; 0 is clamped to 0.01 at write (the shader
    /// divides by the factor).
    #[inspector(min = 0.0, max = 8.0)]
    pub terrain_splat_factor_0: f32,
    #[inspector(min = 0.0, max = 8.0)]
    pub terrain_splat_factor_8: f32,
    #[inspector(min = 0.0, max = 8.0)]
    pub terrain_splat_factor_16: f32,
    #[inspector(min = 0.0, max = 8.0)]
    pub terrain_splat_factor_24: f32,
    #[inspector(min = 0.0, max = 8.0)]
    pub terrain_splat_factor_32: f32,
    pub terrain_wireframe: bool,
    pub object_wireframe: bool,
    /// Cascaded sun shadows, live toggle. Seeded from
    /// `graphics.shadows.enabled` (the master switch, which also sets the
    /// cascade count/distance at sun spawn); flip here for the perf A/B.
    /// Orthogonal to the vanilla/PBR switch (hotkey N), which decides who
    /// *casts* into the maps (`environment::apply_vanilla_shadow_casters`),
    /// not whether the maps render.
    pub enable_shadows: bool,
    pub enable_fog: bool,
    /// Bloom post-process on the main-view cameras. Only has an effect when
    /// `graphics.bloom.enabled` is on in `config.yaml`: that config flag is the
    /// master switch, because it also decides whether the cameras are spawned
    /// HDR at all. Turning this off removes both `Bloom` and the `Hdr` it
    /// requires, so the off state matches pre-bloom rendering exactly.
    pub enable_bloom: bool,
    pub backface_culling: bool,
    /// Fraction of the window the 3D view renders at (`graphics.render_scale`).
    /// Seeded from config; live here so a resolution ladder is five `make perf
    /// set` calls in one session rather than five relaunches. See
    /// `config::graphics::RenderScale` for why the HUD is unaffected.
    pub render_scale: f32,
    pub terrain_wireframe_color: Color,
    pub object_wireframe_color: Color,
    pub automatic_batching: bool,
}

impl Default for RenderDebugSettings {
    fn default() -> Self {
        Self {
            render_terrain: true,
            render_objects: true,
            render_water: true,
            render_foliage: true,
            foliage_density: 1.0,
            foliage_view_distance: 0.0,
            render_effects: true,
            leaf_emit_global: true,
            leaf_emit_density: 1.0,
            effect_additive_intensity: 1.0,
            effect_time_scale: 1.0,
            effect_ldr_additive: true,
            render_navmesh: false,
            render_object_navmesh: false,
            play_animations: true,
            terrain_lighting_mode: 0,
            terrain_lightmap_flip_v: false,
            terrain_splat_factor_0: 0.25,
            terrain_splat_factor_8: 0.25,
            terrain_splat_factor_16: 0.25,
            terrain_splat_factor_24: 0.25,
            terrain_splat_factor_32: 0.25,
            terrain_wireframe: false,
            object_wireframe: false,
            // Seeded from `graphics.shadows.enabled` at startup (see
            // seed_terrain_settings_from_config); the historic perf concern —
            // 4 cascades over a 100k-unit range re-rendering everything —
            // is addressed by the config's scoped cascades (2 × ~120 units).
            enable_shadows: false,
            enable_fog: true,
            enable_bloom: true,
            backface_culling: true,
            render_scale: 1.0,
            terrain_wireframe_color: Color::WHITE,
            object_wireframe_color: Color::from(bevy::color::palettes::basic::YELLOW),
            automatic_batching: true,
        }
    }
}

/// Foliage lives in its own settings-changed system: `on_settings_changed`
/// is at bevy's 16-system-param limit, and the foliage toggles touch nothing
/// it touches (the `FoliageBlock` marker makes the visibility query disjoint).
fn on_foliage_settings_changed(
    settings: Res<RenderDebugSettings>,
    mut commands: Commands,
    mut foliage_query: Query<
        (Entity, &mut Visibility),
        With<crate::plugins::map::foliage::FoliageBlock>,
    >,
) {
    for (foliage_entity, mut visibility) in foliage_query.iter_mut() {
        // Inherited (not Visible) on re-enable — same reasoning as the
        // terrain loop in `on_settings_changed`.
        *visibility = if settings.render_foliage {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if settings.foliage_view_distance > 0.0 {
            commands
                .entity(foliage_entity)
                .insert(crate::plugins::map::foliage::view_range(
                    settings.foliage_view_distance,
                ));
        } else {
            commands
                .entity(foliage_entity)
                .remove::<bevy::camera::visibility::VisibilityRange>();
        }
    }
}

/// Water/ice visibility, split out for the same reason as the foliage
/// toggle above: `on_settings_changed` is at bevy's 16-system-param limit.
/// Keyed on the `WaterPlane` marker rather than a material type so the toggle
/// covers both `graphics.water.quality` tiers and ice — it was declared but
/// never wired while it needed a per-tier query.
fn on_water_settings_changed(
    settings: Res<RenderDebugSettings>,
    mut water_query: Query<&mut Visibility, With<crate::plugins::map::terrain::WaterPlane>>,
) {
    for mut visibility in water_query.iter_mut() {
        // Inherited (not Visible) on re-enable — same reasoning as the
        // terrain loop in `on_settings_changed`: water planes are children of
        // terrain blocks whose region root does the fog-distance hiding.
        let wanted = if settings.render_water {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        visibility.set_if_neq(wanted);
    }
}

/// Last-applied values for `on_settings_changed`'s "only write when the flag
/// actually moved" guards — one struct because the system is at the
/// 16-system-param limit and can't afford a `Local` per flag.
#[derive(Default)]
struct AppliedToggles {
    backface_culling: Option<bool>,
    shadows: Option<bool>,
}

fn on_settings_changed(
    settings: Res<RenderDebugSettings>,
    mut commands: Commands,
    mut terrain_query: Query<
        (Entity, &mut Visibility),
        (
            With<MeshMaterial3d<TerrainBlockSplatMaterial>>,
            Without<MapObject>,
        ),
    >,
    mut object_query: Query<
        (&Children, &mut Visibility, Option<&NoAutomaticBatching>),
        With<MapObject>,
    >,
    mut effect_query: Query<
        &mut Visibility,
        (
            With<EffectInstance>,
            Without<MapObject>,
            Without<MeshMaterial3d<TerrainBlockSplatMaterial>>,
        ),
    >,
    mut effect_params: EffectDebugParams,
    // Only the Sun follows the shadow toggle. The portrait/paper-doll headlights
    // carry Bevy's default CascadeShadowConfig (4 cascades) vs the Sun's
    // mode-dependent config (`map::sun_cascade_config`); if they also became
    // shadow casters, `check_dir_light_mesh_visibility` would index its shared
    // per-thread cascade queue out of bounds (issue #207).
    mut directional_light: Query<&mut DirectionalLight, With<crate::plugins::environment::Sun>>,
    all_entities: Query<Entity, Without<IsResource>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut terrain_materials: ResMut<Assets<TerrainBlockSplatMaterial>>,
    camera_query: Query<Entity, With<Camera>>,
    main_cameras: Query<(Entity, &RenderTarget), With<Camera3d>>,
    ui_camera: Query<Entity, With<Camera2d>>,
    config: Res<ClientConfig>,
    // One Local for every "only write when the flag actually moved" guard:
    // the system is at the 16-param limit (see on_foliage_settings_changed).
    mut applied: Local<AppliedToggles>,
    mut terrain_params: Option<ResMut<crate::assets::m::block_splat_material::TerrainRenderParams>>,
) {
    // Terrain lighting mode + lightmap V-flip: written through the shared
    // TerrainRenderParams storage buffer, so cycling them never marks a
    // terrain material Modified (see the bind-group-leak note below).
    // set_if_neq keeps unrelated settings changes from rewriting the buffer.
    if let Some(terrain_params) = terrain_params.as_mut() {
        use crate::assets::m::block_splat_material::{TerrainLightingMode, TerrainRenderParams};
        // lightmap_enabled + shadow_strength are owned by
        // environment::apply_render_mode (the vanilla/PBR hotkey-N switch),
        // not this panel — carry the current values through the rebuild.
        let lightmap_enabled = terrain_params.lightmap_enabled;
        let shadow_strength = terrain_params.shadow_strength;
        terrain_params.set_if_neq(TerrainRenderParams {
            lightmap_flip_v: settings.terrain_lightmap_flip_v,
            lightmap_enabled,
            shadow_strength,
            lighting_mode: match settings.terrain_lighting_mode {
                0 => TerrainLightingMode::Dynamic,
                1 => TerrainLightingMode::FlatBaked,
                _ => TerrainLightingMode::Baked,
            },
            splat_factors: [
                settings.terrain_splat_factor_0,
                settings.terrain_splat_factor_8,
                settings.terrain_splat_factor_16,
                settings.terrain_splat_factor_24,
                settings.terrain_splat_factor_32,
            ],
        });
    }
    for camera_entity in camera_query.iter() {
        if settings.enable_fog {
            commands
                .entity(camera_entity)
                .insert(rendering::fog(&config.graphics.fog));
        } else {
            commands.entity(camera_entity).remove::<DistanceFog>();
        }
    }

    if config.graphics.bloom.enabled {
        let bloom_on = settings.enable_bloom;
        // Only the cameras that draw to the window: the HUD portrait and
        // paper-doll cameras render into their own Rgba8 images and must stay
        // LDR (see `plugins::camera::attach_bloom`).
        for (camera_entity, target) in main_cameras.iter() {
            if !matches!(target, RenderTarget::Window(_)) {
                continue;
            }
            if bloom_on {
                commands
                    .entity(camera_entity)
                    .insert(config.graphics.bloom.to_bloom());
            } else {
                // `Bloom` is `#[require(Hdr)]` and required components are not
                // removed with the component that pulled them in, so `Hdr` has
                // to go explicitly — otherwise "bloom off" still renders to a
                // float target and doesn't match the pre-bloom look.
                commands.entity(camera_entity).remove::<(Bloom, Hdr)>();
            }
        }
        // The 2d UI camera shares the window's main texture, so its format has
        // to track the 3d cameras' in lockstep or the 3d view disappears.
        for camera_entity in ui_camera.iter() {
            if bloom_on {
                commands.entity(camera_entity).insert(Hdr);
            } else {
                commands.entity(camera_entity).remove::<Hdr>();
            }
        }
    }
    // Only touch the materials when the culling flag actually moved: this system
    // fires on ANY settings change, and dirtying every material re-prepares all
    // their bind groups — for the terrain splat materials each re-prepare also
    // permanently leaks the previous bind group (bevy_pbr 0.19's
    // CreateBindGroupDirectly path never frees it, see TerrainAmbientRatio).
    if applied.backface_culling != Some(settings.backface_culling) {
        applied.backface_culling = Some(settings.backface_culling);
        // The skybox no longer needs excluding here: it has its own material type
        // (`SkyGradientMaterial`) whose cull mode is fixed in its pipeline specialization.
        for (_id, material) in materials.iter_mut() {
            material.cull_mode = if settings.backface_culling {
                Some(Face::Back)
            } else {
                None
            };
        }
        for (_, material) in terrain_materials.iter_mut() {
            material.backface_culling = settings.backface_culling;
        }
    }
    for (terrain_entity, mut visibility) in terrain_query.iter_mut() {
        // Inherited (not Visible) on re-enable: Visible on a child overrides
        // a Hidden ancestor, which would defeat the fog-distance hiding of
        // whole region roots (see terrain::region_visibility). Inherited is
        // also these entities' spawn state.
        *visibility = if settings.render_terrain {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        commands.entity(terrain_entity).insert(WireframeColor {
            color: settings.terrain_wireframe_color,
        });
        match settings.terrain_wireframe {
            true => commands.entity(terrain_entity).insert(Wireframe),
            false => commands.entity(terrain_entity).remove::<Wireframe>(),
        };
    }
    for (children, mut visibility, _batching) in object_query.iter_mut() {
        // Inherited, not Visible — same reason as the terrain loop above.
        *visibility = if settings.render_objects {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        for child in children.iter() {
            commands.entity(child).insert(WireframeColor {
                color: settings.object_wireframe_color,
            });
            match settings.object_wireframe {
                true => commands.entity(child).insert(Wireframe),
                false => commands.entity(child).remove::<Wireframe>(),
            };
        }
    }
    // Guarded like backface_culling: skip the redundant light write (and its
    // change-detection dirtying) when an unrelated panel field moved.
    if applied.shadows != Some(settings.enable_shadows) {
        applied.shadows = Some(settings.enable_shadows);
        for mut directional_light in directional_light.iter_mut() {
            directional_light.shadow_maps_enabled = settings.enable_shadows;
        }
    }
    // Effects: pause the runtime and hide the (frozen) wrappers. Inherited
    // (not Visible) on re-enable so wrappers keep following their anchor's
    // visibility (hidden map objects, future night_only gating).
    effect_params.enabled.0 = settings.render_effects;
    effect_params.leaf.global = settings.leaf_emit_global;
    effect_params.leaf.density = settings.leaf_emit_density;
    effect_params.speed.0 = settings.effect_time_scale.max(0.0);
    if effect_params.ldr.0 != settings.effect_ldr_additive {
        effect_params.ldr.0 = settings.effect_ldr_additive;
        // Flipping the flag re-specializes each material's pipeline (it is
        // part of the material key) and re-syncs the shader mode uniform.
        for (_, material) in effect_params.materials.iter_mut() {
            material.set_ldr_additive(settings.effect_ldr_additive);
        }
    }
    if (effect_params.additive.0 - settings.effect_additive_intensity).abs() > f32::EPSILON {
        effect_params.additive.0 = settings.effect_additive_intensity;
        // Re-derive every additive effect material's color multiplier from
        // its authored base; only touched on an actual slider change (a
        // material write re-prepares its bind group).
        let intensity = effect_params.additive.0;
        for (_, material) in effect_params.materials.iter_mut() {
            if material.dst_blend == 2 {
                material.params.x = material.base_color_scale * intensity;
            }
        }
    }
    for mut visibility in effect_query.iter_mut() {
        *visibility = if settings.render_effects {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
    for entity in all_entities.iter() {
        if settings.automatic_batching {
            commands.entity(entity).remove::<NoAutomaticBatching>();
        } else {
            commands.entity(entity).insert(NoAutomaticBatching);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::config::ClientConfig;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::prelude::World;

    /// The shipped default is `dev_tools: false`, and it has to *run*.
    ///
    /// `RenderDebugSettings` is read as a plain `Res<_>` by three shipping
    /// systems — `animation_culling::cull_distant_animations`,
    /// `map::objects::cull_fogged_objects` and the foliage block builder — so
    /// while it lived behind the `dev_tools` gate, system-parameter validation
    /// panicked on the first frame for anyone who used `config.example.yaml`
    /// unedited. The neighbouring config test asserts the *value* defaults to
    /// false; this asserts the app is actually viable in that state.
    #[test]
    fn settings_resource_exists_without_dev_tools() {
        let mut app = App::new();
        app.add_plugins(RenderControlsPlugin);
        assert!(
            app.world().get_resource::<RenderDebugSettings>().is_some(),
            "RenderDebugSettings must be registered independently of `dev_tools`"
        );
    }

    /// Registering the resource is only half the fix: its `Default` is not the
    /// configured state, so dropping the seeding would silently disable
    /// shadows (`enable_shadows` defaults to false) and silently disable the
    /// foliage cull (`foliage_view_distance` defaults to 0.0, which means "no
    /// cull"). That would trade the crash above for two quiet regressions, one
    /// of them a performance one — hence this guards the seeding, not just the
    /// registration.
    #[test]
    fn config_seeding_overrides_the_struct_defaults() {
        let example = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../config.example.yaml")
            .with_extension("")
            .to_str()
            .expect("the example path is utf-8")
            .to_string();
        let config = ClientConfig::from_file(&example).expect("config.example.yaml loads");
        let expected_shadows = config.graphics.shadows.enabled;
        let expected_view_distance = config.graphics.foliage.view_distance;

        // Precondition: the defaults really do differ from the shipped config,
        // or this test would pass without the seeding running at all.
        let defaults = RenderDebugSettings::default();
        assert_ne!(defaults.enable_shadows, expected_shadows);
        assert_ne!(defaults.foliage_view_distance, expected_view_distance);

        let mut world = World::new();
        world.insert_resource(config);
        world.init_resource::<RenderDebugSettings>();
        world
            .run_system_once(seed_terrain_settings_from_config)
            .expect("seeding runs");

        let settings = world.resource::<RenderDebugSettings>();
        assert_eq!(settings.enable_shadows, expected_shadows);
        assert_eq!(settings.foliage_view_distance, expected_view_distance);
    }
}
