// Idea: drive the world's lighting from SRO's environment.ifo (JMXVENVI, already loaded
// into `MapsAssets.environment_info`) instead of the hardcoded values in `setup_lighting`/
// `rendering::fog()`/`skybox.rs`. Each environment profile is a set of day-cycle graphs
// (0.0..1.0 = midnight..midnight); every frame the active profile is sampled at the current
// `TimeOfDay` and written into Bevy's standard lighting inputs — `GlobalAmbientLight`,
// the `Sun` directional light, `DistanceFog`, the skybox material, and the HQ water
// material. Which profile is active follows the camera: every terrain block header in the
// `.m` files names a profile id (see `TerrainBlock::environment_id`), recorded per region
// while its `JMXVMAPM` asset is still resident. Profile switches (region borders, manual
// override) are smoothed by lerping the *sampled output* toward its target instead of
// tracking from/to profiles. SRO dims the world via these colors, so mostly colors are
// written here — the sun's illuminance scalar stays owned by `dev/lighting.rs`, and
// `DistanceFog` presence (insert/remove) stays owned by `dev/render_debug.rs`. The graphs
// drive both lighting models: `EnvironmentSettings.enabled` (hotkey N) switches the full
// vanilla/PBR render mode — the ambient-brightness model (SRO-faithful
// `ambient_brightness` vs the PBR baseline of `setup_lighting`), the baked terrain
// lightmap (`apply_render_mode`), and who casts into the shadow maps (vanilla = only the
// player, like the original; PBR = everything — `apply_vanilla_shadow_casters`) — while
// the graph colors/fog/sky apply identically in both modes. The hardcoded defaults
// survive only as the pre-load fallback (no envi asset / no active profile yet).

use std::collections::HashMap;
use std::f32::consts::{PI, TAU};

use bevy::prelude::*;
use bevy_inspector_egui::prelude::ReflectInspectorOptions;
use bevy_inspector_egui::quick::ResourceInspectorPlugin;
use bevy_inspector_egui::InspectorOptions;

use crate::assets::ifo::environment::{ColorGraph, EnvironmentProfile, FloatGraph, JMXVENVI};
use crate::assets::ifo::IFOAsset;
use crate::assets::m::block_splat_material::TerrainAmbientRatio;
use crate::assets::m::JMXVMAPM;
use crate::plugins::config::ClientConfig;
use crate::plugins::environment::celestial::{CelestialMaterials, CelestialPlugin};
use crate::plugins::map::assets::MapsAssets;
use crate::plugins::map::terrain::{
    Terrain, TerrainMeshData, WaterLowMaterial, WaterNormalMaterial, FOG_RANGE, REGION_SIZE,
    VISIBLE_RANGE,
};
use crate::plugins::map::water_hq_material::HighQualityWaterMaterial;
use crate::plugins::map::water_material::LowQualityWaterMaterial;
use crate::plugins::skybox::{
    CloudMaterial, CloudMaterials, SkyGradientMaterial, SkyboxMaterial, SKY_COLOR_HEX,
};
use crate::plugins::world_origin::WorldOrigin;
use crate::scenes::in_playable_world;
use crate::GameState;

pub mod celestial;
pub mod reflections;

/// Marks the world's single directional light (spawned in `map::setup_lighting`).
#[derive(Component)]
pub struct Sun;

#[derive(Resource, Reflect, InspectorOptions)]
#[reflect(Resource, InspectorOptions)]
pub struct TimeOfDay {
    /// Day-cycle progress, 0.0 = midnight, 0.5 = noon; wraps at 1.0.
    #[inspector(min = 0.0, max = 1.0)]
    pub t: f32,
    /// Real-time seconds for one full in-game day.
    pub day_length_secs: f32,
    /// Freeze `t` (hotkey P); K/L scrub backward/forward while held.
    pub paused: bool,
}

impl Default for TimeOfDay {
    fn default() -> Self {
        Self {
            t: 0.5,
            day_length_secs: 900.0,
            paused: false,
        }
    }
}

#[derive(Resource, Reflect, InspectorOptions)]
#[reflect(Resource, InspectorOptions)]
pub struct EnvironmentSettings {
    /// Vanilla/PBR render-mode switch (hotkey N): on = SRO-faithful — the
    /// `ambient_brightness` model, the baked terrain lightmap, and only the
    /// player casting a dynamic shadow (like the original client); off = PBR —
    /// the ambient baseline from `setup_lighting`, lightmap off, everything
    /// casting into the config-scoped cascaded shadows (`apply_render_mode` +
    /// `apply_vanilla_shadow_casters`). The environment graphs (colors, fog,
    /// sky, sun animation) apply in both modes.
    pub enabled: bool,
    /// -1 = follow the camera's region; otherwise forces this profile id.
    pub profile_override: i32,
    /// Rotate the sun's pitch with the time of day. While on, this overwrites the
    /// dev U/I light-rotation hotkeys every frame.
    pub animate_sun_direction: bool,
    /// Ambient illuminance while the system is on. SRO's model is `diffuse * NdotL +
    /// ambient` with *comparable* color magnitudes, so the ambient scale must be within
    /// an order of magnitude of the sun's 10k lux — surfaces the sun never hits (its
    /// azimuth stays locked to the Z axis while it arcs, so east/west-facing walls) are
    /// lit by ambient alone and go black at the old default of 100.
    pub ambient_brightness: f32,
    /// Use the profile's fog planes — normalized [-1, 1] fractions of the view range,
    /// see `envi_fog_range` — instead of the streaming-derived defaults. Gives SRO's
    /// long clear noons and short, heavily fogged nights.
    pub use_envi_fog_distances: bool,
    /// Multiplier on the ENVI-mapped fog distances (the raw mapping reads short: noon
    /// median ~3530 of the 5760 ceiling across the 1.188 profiles). The far plane stays
    /// clamped to the terrain streaming end so regions still despawn fully fogged.
    #[inspector(min = 0.5, max = 3.0)]
    pub fog_distance_scale: f32,
    /// Time constant for smoothing profile switches and scrubbing.
    pub transition_seconds: f32,
    /// Ambient illuminance while the system is off (PBR mode) — the baseline
    /// where the sun's N·L dominates and ambient only fills. Live like
    /// `ambient_brightness`; note the sky-reflection `EnvironmentMapLight`
    /// (intensity 1200, `reflections.rs`) adds image-based ambient on top
    /// that this knob does not control, and at noon the 10k-lux sun
    /// dominates sunlit surfaces in both modes — judge the knob on
    /// sun-averted faces.
    #[inspector(min = 0.0, max = 20_000.0)]
    pub pbr_ambient_brightness: f32,
    /// Extra darkening of the ground under the player's shadow in vanilla mode
    /// (0 = off, 1 = black). The cascaded maps alone read too faint there: SRO's
    /// ambient is comparable to the sun, and a shadow only removes the sun's
    /// direct term — this multiplies the *lit* result down instead, standing in
    /// for the original client's dark projected player shadow. PBR mode ignores
    /// it (physically-based shadowing only).
    #[inspector(min = 0.0, max = 1.0)]
    pub vanilla_shadow_strength: f32,
}

impl Default for EnvironmentSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            profile_override: -1,
            animate_sun_direction: true,
            ambient_brightness: 3_000.0,
            use_envi_fog_distances: true,
            fog_distance_scale: 1.5,
            transition_seconds: 1.0,
            pbr_ambient_brightness: 100.0,
            vanilla_shadow_strength: 0.4,
        }
    }
}

/// Per-region environment profile ids, one per terrain block (6x6, index `bz * 6 + bx`),
/// keyed by region (x, z). Recorded once per region while its `JMXVMAPM` is resident;
/// entries are tiny and never evicted.
#[derive(Resource, Default)]
struct RegionEnvironments(HashMap<(u8, u8), [u16; 36]>);

/// Profile id under the active camera (or the manual override). Keeps its last value
/// while the camera is over a not-yet-recorded region, so streaming never causes flicker.
#[derive(Resource, Default)]
struct ActiveEnvironment(Option<u16>);

/// Last applied sample; the smoothing state for `apply_environment`.
#[derive(Resource, Default)]
struct EnvSmoothing(Option<EnvSample>);

/// Everything sampled from a profile for one frame, in Bevy-ready form.
#[derive(Clone, Copy)]
struct EnvSample {
    sun_color: Vec3,
    /// Tint for the sun disc billboard (ENVI SunColor; distinct from the directional term).
    sun_disc_color: Vec3,
    ambient_color: Vec3,
    /// SRO's terrain-specific ambient; applied as a ratio to `ambient_color` in the
    /// terrain splat shader.
    terrain_ambient_color: Vec3,
    fog_color: Vec3,
    fog_near: f32,
    fog_far: f32,
    sky_color: Vec3,
    sky_bottom_color: Vec3,
    water_color: Vec3,
    cloud_color: Vec3,
    cloud_near_alpha: f32,
    cloud_far_alpha: f32,
    /// 0 at noon, 1 at midnight (mapped from the [-1, 1] NightIntensity graph); drives
    /// the procedural star field in the sky gradient.
    night_intensity: f32,
}

/// Material-driving colors of the last applied sample, quantized to 1/256 per channel
/// (one 8-bit step — below anything visible). Each `get_mut` rebuilds that material's
/// whole bind group (the HQ water is a full `ExtendedMaterial` re-prepare), so writes
/// only happen when a quantized input actually moves — a few Hz over the 15-minute day
/// cycle instead of every frame. Covers the sky/sun/cloud/water materials; the terrain
/// ambient goes through [`TerrainAmbientRatio`] (a shared GPU buffer) and never touches
/// the splat material assets at all.
#[derive(Resource, Default)]
struct AppliedEnvColors(Option<QuantizedEnvColors>);

#[derive(Clone, Copy, PartialEq)]
struct QuantizedEnvColors {
    /// sky gradient: top, bottom, fog
    sky: [IVec3; 3],
    night_intensity: i32,
    sun_disc: IVec3,
    cloud: IVec3,
    cloud_alphas: [i32; 2],
    water: IVec3,
    /// feeds the water's sky reflection tint (and the sky gradient)
    sky_bottom: IVec3,
}

impl QuantizedEnvColors {
    fn from_sample(sample: &EnvSample) -> Self {
        let q = |v: Vec3| (v * 256.0).round().as_ivec3();
        let qf = |v: f32| (v * 256.0).round() as i32;
        Self {
            sky: [
                q(sample.sky_color),
                q(sample.sky_bottom_color),
                q(sample.fog_color),
            ],
            night_intensity: qf(sample.night_intensity),
            sun_disc: q(sample.sun_disc_color),
            cloud: q(sample.cloud_color),
            cloud_alphas: [qf(sample.cloud_near_alpha), qf(sample.cloud_far_alpha)],
            water: q(sample.water_color),
            sky_bottom: q(sample.sky_bottom_color),
        }
    }
}

/// All material handles/assets the environment writes to, bundled to stay under Bevy's
/// system-parameter arity limit. The handle resources are `Option` because they are
/// inserted at different lifecycle points than this plugin's systems start running.
#[derive(bevy::ecs::system::SystemParam)]
struct EnvMaterials<'w> {
    skybox: Option<Res<'w, SkyboxMaterial>>,
    sky: ResMut<'w, Assets<SkyGradientMaterial>>,
    celestial: Option<Res<'w, CelestialMaterials>>,
    clouds: Option<Res<'w, CloudMaterials>>,
    cloud_assets: ResMut<'w, Assets<CloudMaterial>>,
    standard: ResMut<'w, Assets<StandardMaterial>>,
    water_handle: Option<Res<'w, WaterNormalMaterial>>,
    water: ResMut<'w, Assets<HighQualityWaterMaterial>>,
    // Exactly one water tier is ever inserted (see `WaterLowMaterial`), so one of these
    // two handle resources is always absent — but the day/night water tint has to reach
    // whichever one is live, or the low tier stays its default colour at midnight.
    water_low_handle: Option<Res<'w, WaterLowMaterial>>,
    water_low: ResMut<'w, Assets<LowQualityWaterMaterial>>,
}

/// Streaming-derived fog distances, identical to `rendering::fog()` — regions must be
/// fully fogged before they despawn, so these also act as the ceiling for ENVI values.
fn default_fog_range() -> (f32, f32) {
    (
        VISIBLE_RANGE as f32 * REGION_SIZE,
        (VISIBLE_RANGE + FOG_RANGE) as f32 * REGION_SIZE,
    )
}

fn default_sky_color() -> Vec3 {
    let sky = Srgba::hex(SKY_COLOR_HEX).unwrap();
    Vec3::new(sky.red, sky.green, sky.blue)
}

impl EnvSample {
    /// Missing/empty graphs fall back to the hardcoded defaults so a sparse profile
    /// degrades to today's look instead of black.
    fn from_profile(profile: &EnvironmentProfile, t: f32) -> Self {
        let (fog_near, fog_far) = default_fog_range();
        let ambient_color = profile.object_ambient_color.sample(t).unwrap_or(Vec3::ONE);
        let sky_color = profile
            .sky_top_color
            .sample(t)
            .unwrap_or_else(default_sky_color);
        Self {
            sun_color: profile.diffuse_color.sample(t).unwrap_or(Vec3::ONE),
            sun_disc_color: profile.sun_color.sample(t).unwrap_or(Vec3::ONE),
            ambient_color,
            // Falls back to the object ambient => ratio 1 => no terrain-specific shift.
            terrain_ambient_color: profile
                .terrain_ambient_color
                .sample(t)
                .unwrap_or(ambient_color),
            fog_color: profile
                .fog_color
                .sample(t)
                .unwrap_or(Vec3::new(0.1, 0.2, 0.4)),
            fog_near: profile.fog_near_plane.sample(t).unwrap_or(fog_near),
            fog_far: profile.fog_far_plane.sample(t).unwrap_or(fog_far),
            sky_color,
            // Falls back to the top color => flat sky, exactly the pre-gradient look.
            sky_bottom_color: profile.sky_bottom_color.sample(t).unwrap_or(sky_color),
            water_color: profile.water_color.sample(t).unwrap_or(Vec3::ONE),
            // Cloud tint falls back to the diffuse color (the pre-identification
            // behavior); the alphas to their authoring defaults from the data.
            cloud_color: profile
                .cloud_color
                .sample(t)
                .unwrap_or_else(|| profile.diffuse_color.sample(t).unwrap_or(Vec3::ONE)),
            cloud_near_alpha: profile
                .cloud_near_alpha
                .sample(t)
                .unwrap_or(0.5)
                .clamp(0.0, 1.0),
            cloud_far_alpha: profile
                .cloud_far_alpha
                .sample(t)
                .unwrap_or(0.9)
                .clamp(0.0, 1.0),
            // Empty graph => no stars (dungeon-style profiles have no sky to show them).
            night_intensity: profile
                .night_intensity
                .sample(t)
                .map_or(0.0, |v| ((v + 1.0) / 2.0).clamp(0.0, 1.0)),
        }
    }

    fn lerp(&self, target: &EnvSample, s: f32) -> Self {
        Self {
            sun_color: self.sun_color.lerp(target.sun_color, s),
            sun_disc_color: self.sun_disc_color.lerp(target.sun_disc_color, s),
            ambient_color: self.ambient_color.lerp(target.ambient_color, s),
            terrain_ambient_color: self
                .terrain_ambient_color
                .lerp(target.terrain_ambient_color, s),
            fog_color: self.fog_color.lerp(target.fog_color, s),
            fog_near: self.fog_near + (target.fog_near - self.fog_near) * s,
            fog_far: self.fog_far + (target.fog_far - self.fog_far) * s,
            sky_color: self.sky_color.lerp(target.sky_color, s),
            sky_bottom_color: self.sky_bottom_color.lerp(target.sky_bottom_color, s),
            water_color: self.water_color.lerp(target.water_color, s),
            cloud_color: self.cloud_color.lerp(target.cloud_color, s),
            cloud_near_alpha: self.cloud_near_alpha
                + (target.cloud_near_alpha - self.cloud_near_alpha) * s,
            cloud_far_alpha: self.cloud_far_alpha
                + (target.cloud_far_alpha - self.cloud_far_alpha) * s,
            night_intensity: self.night_intensity
                + (target.night_intensity - self.night_intensity) * s,
        }
    }
}

/// Plain-text dump of every graph's raw keys for every profile, `value@time` per key.
/// Written once at startup next to the binary — the source material for identifying the
/// unnamed graphs (no public documentation names them; even JMX-File-Editor calls them
/// Curve4/10/11/12/15).
fn dump_environment_graphs(envi: &JMXVENVI) -> String {
    use std::fmt::Write;

    fn colors(out: &mut String, name: &str, graph: &ColorGraph) {
        let _ = write!(out, "  {name}:");
        for key in &graph.keys {
            let _ = write!(
                out,
                " ({:.3},{:.3},{:.3})@{:.3}",
                key.x, key.y, key.z, key.w
            );
        }
        let _ = writeln!(out);
    }
    fn floats(out: &mut String, name: &str, graph: &FloatGraph) {
        let _ = write!(out, "  {name}:");
        for key in &graph.keys {
            let _ = write!(out, " {:.4}@{:.3}", key.x, key.y);
        }
        let _ = writeln!(out);
    }

    let mut out = String::new();
    let _ = writeln!(
        out,
        "# JMXVENVI set '{}', {} profiles — graph keys as value@time (time 0=midnight, 0.5=noon)",
        envi.environment_set_name,
        envi.profiles.len()
    );
    for profile in &envi.profiles {
        let _ = writeln!(
            out,
            "\nprofile {} '{}' (bgm day='{}' night='{}')",
            profile.id, profile.name, profile.string0, profile.string1
        );
        colors(&mut out, "sun_color", &profile.sun_color);
        colors(&mut out, "sky_top_color", &profile.sky_top_color);
        colors(&mut out, "diffuse_color", &profile.diffuse_color);
        colors(
            &mut out,
            "object_ambient_color",
            &profile.object_ambient_color,
        );
        colors(&mut out, "cloud_color(graph4)", &profile.cloud_color);
        colors(
            &mut out,
            "terrain_ambient_color",
            &profile.terrain_ambient_color,
        );
        colors(
            &mut out,
            "terrain_shadow_color",
            &profile.terrain_shadow_color,
        );
        floats(&mut out, "fog_near_plane", &profile.fog_near_plane);
        floats(&mut out, "fog_far_plane", &profile.fog_far_plane);
        colors(&mut out, "fog_color", &profile.fog_color);
        floats(
            &mut out,
            "cloud_near_alpha(graph10)",
            &profile.cloud_near_alpha,
        );
        floats(
            &mut out,
            "cloud_far_alpha(graph11)",
            &profile.cloud_far_alpha,
        );
        floats(&mut out, "graph12", &profile.graph12);
        colors(&mut out, "sky_bottom_color", &profile.sky_bottom_color);
        colors(&mut out, "water_color", &profile.water_color);
        floats(
            &mut out,
            "night_intensity(graph15)",
            &profile.night_intensity,
        );
    }
    out
}

/// Sampled colors are in sRGB space (see the note in `apply_environment`); the sky
/// gradient uniform wants linear RGB.
fn srgb_to_linear_vec4(v: Vec3) -> Vec4 {
    let c = Color::srgb(v.x, v.y, v.z).to_linear();
    Vec4::new(c.red, c.green, c.blue, 1.0)
}

/// Maps the profile's normalized fog planes onto world distances. The graph dump showed
/// both planes are [-1, 1] fractions of a view range (the original client's baseline is
/// its view-distance setting): they push outward toward noon and pull in at night —
/// SRO's short, heavily fogged nights. The far plane spans [MIN_FOG_FAR, streaming end]
/// scaled by `EnvironmentSettings::fog_distance_scale` (the streaming end stays a hard
/// ceiling so regions still despawn fully fogged, and the floor keeps a -1 night value
/// playable instead of fog-at-the-camera); the near plane spans [0, 90% of far].
fn envi_fog_range(near: f32, far: f32, streaming_end: f32, scale: f32) -> (f32, f32) {
    const MIN_FOG_FAR: f32 = 800.0;
    let unit = |v: f32| (v.clamp(-1.0, 1.0) + 1.0) / 2.0;
    let end =
        ((MIN_FOG_FAR + (streaming_end - MIN_FOG_FAR) * unit(far)) * scale).min(streaming_end);
    let start = end * 0.9 * unit(near);
    (start, end)
}

/// Component-wise terrain/object ambient ratio, computed in linear space (the shader
/// multiplies it into the linear `lights.ambient_color`). Kept sane when the object
/// ambient is near black (night): both terms vanish there, so the ratio's exact value
/// stops mattering — clamp instead of letting it blow up.
fn terrain_ambient_ratio(sample: &EnvSample) -> Vec3 {
    let lin = |v: Vec3| {
        let c = Color::srgb(v.x, v.y, v.z).to_linear();
        Vec3::new(c.red, c.green, c.blue)
    };
    let terrain = lin(sample.terrain_ambient_color);
    let object = lin(sample.ambient_color);
    let ratio = |t: f32, o: f32| {
        if o > 1e-4 {
            (t / o).clamp(0.0, 4.0)
        } else {
            1.0
        }
    };
    Vec3::new(
        ratio(terrain.x, object.x),
        ratio(terrain.y, object.y),
        ratio(terrain.z, object.z),
    )
}

/// Region (x, z) and block index (0..36) under a world-space position. World X is mirrored
/// relative to the SRO grid (regions sit at `x * -REGION_SIZE`, blocks/vertices are laid
/// out with negated X too — see `preload_terrain_region` and `load_terrain_system`'s group
/// transform), so X is negated before snapping to the 6x6 grid of 320-unit blocks. The
/// index layout matches the `.m` parser's push order (`bz * 6 + bx`).
fn active_block(position: Vec3) -> ((u8, u8), usize) {
    const BLOCK_SIZE: f32 = REGION_SIZE / 6.0;
    let u = -position.x;
    let v = position.z;
    let region_x = (u / REGION_SIZE).floor().clamp(0.0, 255.0);
    let region_z = (v / REGION_SIZE).floor().clamp(0.0, 255.0);
    let bx = ((u - region_x * REGION_SIZE) / BLOCK_SIZE)
        .floor()
        .clamp(0.0, 5.0) as usize;
    let bz = ((v - region_z * REGION_SIZE) / BLOCK_SIZE)
        .floor()
        .clamp(0.0, 5.0) as usize;
    ((region_x as u8, region_z as u8), bz * 6 + bx)
}

pub struct EnvironmentPlugin;

impl Plugin for EnvironmentPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<TimeOfDay>()
            .register_type::<EnvironmentSettings>()
            .init_resource::<TimeOfDay>()
            .init_resource::<EnvironmentSettings>()
            .init_resource::<RegionEnvironments>()
            .init_resource::<ActiveEnvironment>()
            .init_resource::<EnvSmoothing>()
            .init_resource::<AppliedEnvColors>()
            .add_plugins(CelestialPlugin)
            .add_systems(
                Update,
                (
                    advance_time_of_day,
                    record_region_env_ids,
                    resolve_active_environment,
                    apply_environment,
                )
                    .chain()
                    .run_if(in_state(GameState::Game))
                    // Inside a dungeon the per-block DOF fog/ambient replaces
                    // the ENVI region profiles (ADR-0008).
                    .run_if(not(resource_exists::<
                        crate::plugins::dungeon::ActiveDungeon,
                    >)),
            )
            // Condition order matters: `run_if` chains combine lazily left to
            // right, so `resource_changed`'s tick is only consumed while in
            // Game — a toggle can't be swallowed outside it. Deliberately not
            // dungeon-gated (unlike the chain above): the vanilla/PBR switch
            // keeps working inside dungeons.
            .add_systems(
                Update,
                apply_render_mode
                    .run_if(in_state(GameState::Game))
                    .run_if(resource_changed::<EnvironmentSettings>),
            )
            // Every frame (cheap: one distance compare at steady state) — the
            // vanilla cascade range follows the player's view depth so free
            // cameras keep the player shadow (see the system doc).
            .add_systems(
                Update,
                scale_vanilla_cascades
                    .after(apply_render_mode)
                    .run_if(in_state(GameState::Game)),
            )
            // PostUpdate, every frame: fresh spawns must be caught after
            // Update's spawn commands applied (see the system doc).
            .add_systems(
                PostUpdate,
                apply_vanilla_shadow_casters.run_if(in_state(GameState::Game)),
            );

        // Dev-tools gate (config.yaml `dev_tools`, inserted in main() before
        // any Plugin::build): egui inspector windows cost FPS every frame, and
        // the hotkeys below claim bare letters a play session wants back.
        if app
            .world()
            .get_resource::<ClientConfig>()
            .is_some_and(|config| config.dev_tools)
        {
            app.add_plugins((
                ResourceInspectorPlugin::<TimeOfDay>::default()
                    .run_if(crate::plugins::dev::dev_windows_visible),
                ResourceInspectorPlugin::<EnvironmentSettings>::default()
                    .run_if(crate::plugins::dev::dev_windows_visible),
            ));
            // Debug env controls (N toggle, Shift+M pause, K/L scrub). Two
            // gates, both needed: `dev_tools` because these are bare letters in
            // a shipping build — `L` is the bound vanilla `KeyAcademy` action
            // (`settings/keymap.rs`), and `N` silently flips the whole
            // vanilla/PBR render mode — and in-world because typing in the
            // login / character-select forms must not reach them either.
            app.add_systems(
                Update,
                environment_hotkeys
                    .run_if(in_state(GameState::Game))
                    .run_if(in_playable_world),
            );
        }
    }
}

fn advance_time_of_day(time: Res<Time>, mut tod: ResMut<TimeOfDay>) {
    if tod.paused {
        return;
    }
    let day_length = tod.day_length_secs.max(1.0);
    tod.t = (tod.t + time.delta_secs() / day_length).rem_euclid(1.0);
}

fn environment_hotkeys(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut settings: ResMut<EnvironmentSettings>,
    mut tod: ResMut<TimeOfDay>,
) {
    if keys.just_pressed(KeyCode::KeyN) {
        settings.enabled = !settings.enabled;
    }
    // Shift+M (bare M opens the world map; P steps the env-reflection
    // intensity in dev/lighting.rs)
    if keys.just_pressed(KeyCode::KeyM)
        && (keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight))
    {
        tod.paused = !tod.paused;
    }
    // Scrub at a tenth of a day per second while held.
    let scrub = 0.1 * time.delta_secs();
    if keys.pressed(KeyCode::KeyK) {
        tod.t = (tod.t - scrub).rem_euclid(1.0);
    } else if keys.pressed(KeyCode::KeyL) {
        tod.t = (tod.t + scrub).rem_euclid(1.0);
    }
}

/// Wires the vanilla/PBR switch (`EnvironmentSettings.enabled`, hotkey N) to the baked
/// terrain lightmap, the player-shadow boost, and the Sun's cascade scoping: vanilla =
/// lightmap multiplied into the ground albedo + `vanilla_shadow_strength` darkening
/// under the player's shadow + tight player-scoped cascades (crisp character shadow);
/// PBR = lightmap off, boost off, config-scoped cascades (everything casts and the
/// cascaded shadows shade the ground physically). Deliberately separate from
/// `apply_environment`, which early-returns until environment.ifo resolves and is
/// dungeon-gated. The lightmap rides the shared `TerrainRenderParams` buffer, never the
/// splat material assets (bind-group leak, see block_splat_material.rs). Who *casts*
/// into the shadow maps is the mode's other half — see `apply_vanilla_shadow_casters`.
/// Note: shadows only land on terrain in `lighting_mode: dynamic` — the other modes
/// bypass `apply_pbr_lighting`.
fn apply_render_mode(
    settings: Res<EnvironmentSettings>,
    config: Res<ClientConfig>,
    terrain_params: Option<ResMut<crate::assets::m::block_splat_material::TerrainRenderParams>>,
    sun_query: Query<Entity, With<Sun>>,
    mut commands: Commands,
    mut last_enabled: Local<Option<bool>>,
) {
    if let Some(mut params) = terrain_params {
        let shadow_strength = if settings.enabled {
            settings.vanilla_shadow_strength
        } else {
            0.0
        };
        // Guarded write: don't dirty the extract/write_terrain_params chain when
        // another EnvironmentSettings field changed.
        if params.lightmap_enabled != settings.enabled || params.shadow_strength != shadow_strength
        {
            params.lightmap_enabled = settings.enabled;
            params.shadow_strength = shadow_strength;
        }
    }
    // Cascade re-scope + console confirmation only on an actual mode flip
    // (this system fires on ANY EnvironmentSettings change, e.g. slider drags).
    // Only the PBR config is inserted here — the vanilla range is owned by
    // `scale_vanilla_cascades`, which runs right after this system and writes
    // the component directly; a deferred insert here would overwrite its
    // same-frame write at the end of the flip frame.
    if *last_enabled != Some(settings.enabled) {
        *last_enabled = Some(settings.enabled);
        if !settings.enabled {
            for sun in &sun_query {
                commands
                    .entity(sun)
                    .insert(crate::plugins::map::sun_cascade_config(
                        false,
                        &config.graphics.shadows,
                    ));
            }
        }
        if settings.enabled {
            info!(
                "render mode: vanilla (lightmap on, player-only shadow casters, \
                 shadow boost {}, ambient {})",
                settings.vanilla_shadow_strength, settings.ambient_brightness
            );
        } else {
            info!(
                "render mode: PBR (lightmap off, all shadow casters, ambient {})",
                settings.pbr_ambient_brightness
            );
        }
    }
}

/// Stretches the vanilla cascade range to keep the player's shadow alive under free
/// cameras. Bevy scopes cascades to the VIEW frustum, so "player-scoped" bounds only
/// work while the camera follows the player — a debug/fly camera watching from afar
/// leaves the player beyond the last cascade and the shadow fades out (world-scene
/// playtest). Every frame in vanilla mode, take the player's view depth from the
/// farthest active camera and rebuild the Sun's `CascadeShadowConfig` to reach just
/// past it — tight (crisp texels) when the camera is close, extended when it is not.
/// The range is quantized to coarse steps so bounds don't jitter per frame, and the
/// cascade count never changes (see `map::sun_cascade_config` — growing it panics in
/// bevy_light). PBR mode is untouched: `apply_render_mode` re-inserts the config-scoped
/// cascades on the mode flip.
fn scale_vanilla_cascades(
    settings: Res<EnvironmentSettings>,
    config: Res<ClientConfig>,
    players: Query<&GlobalTransform, With<crate::plugins::player::Player>>,
    cameras: Query<(&GlobalTransform, &Camera, &bevy::camera::RenderTarget), With<Camera3d>>,
    mut suns: Query<&mut bevy::light::CascadeShadowConfig, With<Sun>>,
    mut applied_distance: Local<f32>,
) {
    if !settings.enabled {
        // Forget the applied range so re-entering vanilla mode re-applies it
        // (apply_render_mode resets the Sun to the baseline on the flip).
        *applied_distance = 0.0;
        return;
    }
    let Ok(player) = players.single() else {
        return;
    };
    let mut view_depth: f32 = 0.0;
    for (camera_transform, camera, target) in &cameras {
        // Only the camera the user actually views through: the offscreen RTT
        // rigs (portrait, paper doll) are active Camera3ds too and must not
        // stretch the Sun's cascades; `switch_camera` keeps the window
        // cameras (player/fly) mutually exclusive.
        if !camera.is_active || !matches!(target, bevy::camera::RenderTarget::Window(_)) {
            continue;
        }
        let to_player = player.translation() - camera_transform.translation();
        view_depth = view_depth.max(to_player.dot(*camera_transform.forward()));
    }
    // Margin keeps the ground around the player covered; 200-unit steps keep the
    // rebuilt bounds stable across frames (bevy rebuilds the actual cascades from
    // this config every frame regardless).
    let max_distance = ((view_depth * 1.2 + 200.0) / 200.0).ceil() * 200.0;
    let max_distance =
        max_distance.clamp(crate::plugins::map::VANILLA_SHADOW_BASE_DISTANCE, 20_000.0);
    if *applied_distance == max_distance {
        return;
    }
    *applied_distance = max_distance;
    debug!("vanilla shadow range -> {max_distance} (player view depth {view_depth:.0})");
    for mut cascade_config in &mut suns {
        *cascade_config =
            crate::plugins::map::vanilla_cascade_config(&config.graphics.shadows, max_distance);
    }
}

/// Meshes whose shadow casting the vanilla mode suppressed. Distinct from meshes that
/// are `NotShadowCaster` for reasons of their own (skybox, effects, item drops, foliage
/// config…), so PBR mode restores exactly what vanilla removed and nothing else.
#[derive(Component)]
struct VanillaShadowSuppressed;

/// The vanilla/PBR switch's shadow half: shadow maps stay enabled in both modes
/// (`graphics.shadows.enabled` is the master gate), the modes differ in who casts.
/// Vanilla = only the local player's meshes — the original client draws just a player
/// shadow, with the baked terrain lightmap carrying every static shadow; PBR = every
/// mesh casts. Runs every frame (PostUpdate, after the spawn commands of Update have
/// applied, so streamed-in meshes never cast for a frame): in vanilla the un-suppressed
/// query is only the player's meshes plus fresh spawns, in PBR the suppressed query is
/// empty after the first sweep — both are cheap at steady state.
fn apply_vanilla_shadow_casters(
    settings: Res<EnvironmentSettings>,
    unsuppressed: Query<
        Entity,
        (
            With<bevy::mesh::Mesh3d>,
            Without<bevy::light::NotShadowCaster>,
        ),
    >,
    suppressed: Query<Entity, With<VanillaShadowSuppressed>>,
    parents: Query<&ChildOf>,
    players: Query<(), With<crate::plugins::player::Player>>,
    mut commands: Commands,
) {
    if settings.enabled {
        for entity in &unsuppressed {
            let is_player_mesh = std::iter::once(entity)
                .chain(parents.iter_ancestors(entity))
                .any(|ancestor| players.contains(ancestor));
            if !is_player_mesh {
                commands
                    .entity(entity)
                    .insert((bevy::light::NotShadowCaster, VanillaShadowSuppressed));
            }
        }
    } else {
        for entity in &suppressed {
            commands
                .entity(entity)
                .remove::<(bevy::light::NotShadowCaster, VanillaShadowSuppressed)>();
        }
    }
}

/// Copies each streamed-in region's per-block profile ids out of its `JMXVMAPM` asset.
/// Must run every frame: the asset (and `TerrainMeshData`) only stays resident until the
/// region reaches `TerrainLoadState::Completed`, but it is guaranteed present for at least
/// the frames in which `load_terrain_system` first processes the region.
fn record_region_env_ids(
    mut regions: ResMut<RegionEnvironments>,
    terrain_query: Query<(&Terrain, &TerrainMeshData)>,
    map_assets: Res<Assets<JMXVMAPM>>,
) {
    for (terrain, mesh_data) in &terrain_query {
        let key = terrain.to_x_z();
        if regions.0.contains_key(&key) {
            continue;
        }
        let Some(map) = map_assets.get(&mesh_data.0) else {
            continue;
        };
        let mut ids = [0u16; 36];
        for (id, block) in ids.iter_mut().zip(&map.blocks) {
            *id = block.environment_id;
        }
        regions.0.insert(key, ids);
    }
}

fn resolve_active_environment(
    settings: Res<EnvironmentSettings>,
    regions: Res<RegionEnvironments>,
    camera_query: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    mut active: ResMut<ActiveEnvironment>,
    origin: Res<WorldOrigin>,
) {
    let resolved = if settings.profile_override >= 0 {
        Some(settings.profile_override as u16)
    } else {
        camera_query
            .iter()
            .find(|(camera, _)| camera.is_active)
            .and_then(|(_, transform)| {
                // region/block lookup is SRO-space; the camera is render-space
                let (region, block) = active_block(origin.to_sro(transform.translation()));
                regions.0.get(&region).map(|ids| ids[block])
            })
    };
    // Keep the last profile while the camera is over an unrecorded region.
    if let Some(id) = resolved {
        if active.0 != Some(id) {
            active.0 = Some(id);
        }
    }
}

/// While the sun animates with the time of day its elevation never dips below this —
/// a below-horizon directional light looks broken; the profile's near-black night
/// colors do the actual dimming. There is no upper cap: the light follows the drawn
/// sun disc (`celestial::sun_direction`) through the zenith at noon, so the azimuth
/// flip from +Z (morning) to -Z (afternoon) is continuous.
const MIN_SUN_ELEVATION: f32 = 10.0 * PI / 180.0;

#[allow(clippy::too_many_arguments)]
fn apply_environment(
    time: Res<Time>,
    tod: Res<TimeOfDay>,
    settings: Res<EnvironmentSettings>,
    config: Res<crate::plugins::config::ClientConfig>,
    active: Res<ActiveEnvironment>,
    maps_assets: Res<MapsAssets>,
    ifo_assets: Res<Assets<IFOAsset>>,
    mut smoothing: ResMut<EnvSmoothing>,
    mut ambient: ResMut<GlobalAmbientLight>,
    mut sun_query: Query<(&mut DirectionalLight, &mut Transform), With<Sun>>,
    mut fog_query: Query<&mut DistanceFog>,
    mut clear_color: ResMut<ClearColor>,
    mut materials: EnvMaterials,
    mut terrain_ratio: ResMut<TerrainAmbientRatio>,
    mut applied_colors: ResMut<AppliedEnvColors>,
    mut dumped_profiles: Local<bool>,
) {
    let Some(envi) = ifo_assets
        .get(&maps_assets.environment_info)
        .and_then(|ifo| ifo.environment.as_ref())
    else {
        return;
    };

    if !*dumped_profiles {
        *dumped_profiles = true;
        info!(
            "environment.ifo: set '{}', {} profiles",
            envi.environment_set_name,
            envi.profiles.len()
        );
        // Full key data of every graph, for identifying the still-unknown ones
        // (graph4/10/11/12/15, fog plane scale) by their shapes over the day cycle.
        // const GRAPH_DUMP_PATH: &str = "environment_graphs.txt";
        // match std::fs::write(GRAPH_DUMP_PATH, dump_environment_graphs(envi)) {
        //     Ok(()) => info!("full environment graph dump written to {GRAPH_DUMP_PATH}"),
        //     Err(error) => warn!("failed to write {GRAPH_DUMP_PATH}: {error}"),
        // }
        for profile in &envi.profiles {
            debug!(
                "  profile {:>3} '{}': diffuse@noon={:?} fog@noon={:?} fog_near@noon={:?} fog_far@noon={:?}",
                profile.id,
                profile.name,
                profile.diffuse_color.sample(0.5),
                profile.fog_color.sample(0.5),
                profile.fog_near_plane.sample(0.5),
                profile.fog_far_plane.sample(0.5),
            );
        }
    }

    let Some(profile) = active
        .0
        .and_then(|id| envi.profiles.iter().find(|profile| profile.id == id))
    else {
        return;
    };

    let target = EnvSample::from_profile(profile, tod.t);
    let sample = match &smoothing.0 {
        Some(current) => {
            let tau = settings.transition_seconds.max(0.001);
            current.lerp(&target, 1.0 - (-time.delta_secs() / tau).exp())
        }
        None => target,
    };
    smoothing.0 = Some(sample);

    // SRO's color values predate any linear-workflow pipeline, so they are treated as
    // sRGB — the same space the hardcoded defaults were authored in.
    let srgb = |v: Vec3| Color::srgb(v.x, v.y, v.z);

    ambient.color = srgb(sample.ambient_color);
    ambient.brightness = if settings.enabled {
        settings.ambient_brightness
    } else {
        settings.pbr_ambient_brightness
    };

    for (mut light, mut transform) in &mut sun_query {
        light.color = srgb(sample.sun_color);
        if settings.animate_sun_direction {
            // The light's direction shares the sun disc's source of truth
            // (`celestial::sun_direction`: theta 0.25 = +Z horizon, 0.5 = zenith,
            // 0.75 = -Z horizon) — a fixed-azimuth light with a sweeping disc lit
            // the whole afternoon world from the side opposite the visible sun,
            // with every shadow pointing at it. Elevation is clamped to the
            // horizon minimum only; `rotation_x(-e)` lights from +Z (morning),
            // `rotation_x(e - PI)` from -Z (afternoon), and both meet at
            // `rotation_x(-PI/2)` at the zenith, so the noon flip is continuous.
            let theta = (tod.t - 0.25) * TAU;
            let elevation = f32::asin(theta.sin().clamp(MIN_SUN_ELEVATION.sin(), 1.0));
            let angle = if theta.cos() >= 0.0 {
                -elevation
            } else {
                elevation - PI
            };
            *transform = Transform::from_rotation(Quat::from_rotation_x(angle));
        }
    }

    let (default_start, default_end) = default_fog_range();
    let (fog_start, fog_end) = if settings.use_envi_fog_distances {
        envi_fog_range(
            sample.fog_near,
            sample.fog_far,
            default_end,
            settings.fog_distance_scale.max(0.0),
        )
    } else {
        (default_start, default_end)
    };
    // Alpha is the in-scattering strength, not an opacity: Bevy scales the
    // whole sun-halo term by it. It stays a config knob defaulting to 0 because
    // the sun's color reaches the shader premultiplied by its 10k-lux
    // illuminance and modulated by the shadow map, which made the halo read as
    // gloss patches on distant fogged geometry rather than as scattered light
    // (see `FogGraphicsSettings`).
    let scattering = config.graphics.fog.sun_scattering;
    for mut fog in &mut fog_query {
        fog.color = srgb(sample.fog_color);
        fog.directional_light_color = Color::srgba(
            sample.sun_color.x,
            sample.sun_color.y,
            sample.sun_color.z,
            scattering,
        );
        fog.directional_light_exponent = config.graphics.fog.sun_scattering_exponent;
        fog.falloff = FogFalloff::Linear {
            start: fog_start,
            end: fog_end,
        };
    }
    // Paint the framebuffer clear with the current fog color. It is normally hidden by the
    // opaque skybox, but shows through in exactly one place: the 1-2 frame gap where a freshly
    // streamed distant object has already written the depth prepass yet its main-pass pipeline
    // is still specializing, so the skybox depth-fails against its silhouette. Filling that gap
    // with the fog color makes such pop-in blend into the fog instead of flashing black.
    clear_color.0 = srgb(sample.fog_color);

    // Each `get_mut` below rebuilds that material's bind group, so the writes are
    // gated on their quantized inputs actually moving (see [`AppliedEnvColors`]).
    let colors = QuantizedEnvColors::from_sample(&sample);
    let prev = applied_colors.0;
    applied_colors.0 = Some(colors);

    if prev.is_none_or(|p| p.sky != colors.sky || p.night_intensity != colors.night_intensity) {
        if let Some(handle) = materials.skybox.as_ref().map(|s| s.0.clone()) {
            if let Some(mut material) = materials.sky.get_mut(&handle) {
                material.gradient.top_color = srgb_to_linear_vec4(sample.sky_color);
                material.gradient.bottom_color = srgb_to_linear_vec4(sample.sky_bottom_color);
                material.gradient.fog_color = srgb_to_linear_vec4(sample.fog_color);
                material.gradient.params.x = sample.night_intensity;
            }
        }
    }

    if prev.is_none_or(|p| p.sun_disc != colors.sun_disc) {
        if let Some(handle) = materials.celestial.as_ref().map(|c| c.sun.clone()) {
            if let Some(mut material) = materials.standard.get_mut(&handle) {
                material.base_color = Color::LinearRgba(
                    srgb(sample.sun_disc_color).to_linear() * celestial::SUN_TINT_BOOST,
                );
            }
        }
    }

    // Clouds are unlit, so tint and opacity come straight from the profile's cloud
    // graphs: white/dense at noon, warm at dusk, dark and thinner at night.
    if prev.is_none_or(|p| p.cloud != colors.cloud || p.cloud_alphas != colors.cloud_alphas) {
        let cloud_handles = materials.clouds.as_ref().map(|c| {
            [
                (c.near.clone(), sample.cloud_near_alpha),
                (c.far.clone(), sample.cloud_far_alpha),
            ]
        });
        if let Some(handles) = cloud_handles {
            for (handle, alpha) in handles {
                if let Some(mut material) = materials.cloud_assets.get_mut(&handle) {
                    let mut tint = srgb_to_linear_vec4(sample.cloud_color);
                    tint.w = alpha;
                    material.settings.tint = tint;
                }
            }
        }
    }

    if prev.is_none_or(|p| p.water != colors.water || p.sky_bottom != colors.sky_bottom) {
        if let Some(handle) = materials.water_handle.as_ref().map(|w| w.0.clone()) {
            let material = materials.water.get_mut(&handle);
            if let Some(mut material) = material {
                material.base.base_color = srgb(sample.water_color);
                // Keep the faked water reflection in sync with the sky, preserving only the
                // configured blend strength (alpha). Grazing-angle reflections see the low
                // sky, so the horizon (bottom) color is the right one.
                let tint_strength = material.extension.settings.sky_tint.w;
                material.extension.settings.sky_tint =
                    sample.sky_bottom_color.extend(tint_strength);
            }
        }
        // The low tier has no faked sky reflection to keep in sync — that raymarch is
        // exactly what it drops — so only the base colour follows the day cycle. Its alpha
        // is load-bearing, unlike the HQ tier's: low water is `AlphaMode::Blend`, so the
        // configured translucency has to survive the tint or the lakebed disappears.
        if let Some(handle) = materials.water_low_handle.as_ref().map(|w| w.0.clone()) {
            if let Some(mut material) = materials.water_low.get_mut(&handle) {
                let alpha = material.base.base_color.alpha();
                material.base.base_color = srgb(sample.water_color).with_alpha(alpha);
            }
        }
    }

    // Terrain ambient ratio: written into the shared GPU buffer every splat material
    // binds (see `TerrainAmbientRatio`), NOT into the material assets — a Modified
    // splat material permanently leaks its previous bind group in bevy_pbr 0.19's
    // CreateBindGroupDirectly path, and new streamed-in materials pick the shared
    // buffer up automatically. Change-gated only so the render side skips the
    // (16-byte) upload on identical frames; no quantizing needed anymore.
    let ratio = terrain_ambient_ratio(&sample);
    let ratio_array = [ratio.x, ratio.y, ratio.z, 1.0];
    if terrain_ratio.0 != ratio_array {
        terrain_ratio.0 = ratio_array;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envi_fog_range_maps_normalized_planes() {
        let end = 5760.0;
        // Full daylight (+1/+1): fog band pushed to the streaming limits.
        let (start, far) = envi_fog_range(1.0, 1.0, end, 1.0);
        assert_eq!(far, end);
        assert!((start - end * 0.9).abs() < 1e-3);
        // Deep night (-1/-1): short but playable view distance, fog from the camera.
        let (start, far) = envi_fog_range(-1.0, -1.0, end, 1.0);
        assert_eq!(far, 800.0);
        assert_eq!(start, 0.0);
        // Far plane never exceeds the streaming ceiling, even for out-of-range data.
        let (_, far) = envi_fog_range(0.0, 2.0, end, 1.0);
        assert!(far <= end);
    }

    #[test]
    fn envi_fog_range_scale_stretches_but_respects_ceiling() {
        let end = 5760.0;
        // Night floor scales with the multiplier.
        let (_, far) = envi_fog_range(-1.0, -1.0, end, 1.5);
        assert_eq!(far, 1200.0);
        // Mid-range value stretches linearly (unit 0.5 => 3280 * 1.5).
        let (_, far) = envi_fog_range(-1.0, 0.0, end, 1.5);
        assert!((far - 4920.0).abs() < 1e-3);
        // Full daylight clamps to the streaming ceiling; near stays below far.
        let (start, far) = envi_fog_range(1.0, 1.0, end, 1.5);
        assert_eq!(far, end);
        assert!(start <= far * 0.9 + 1e-3);
    }

    #[test]
    fn active_block_at_jangan_spawn() {
        // Jangan spawn (see `SpawnPoints::jangan`): region 168/97, block 3x3.
        let position = Vec3::new(-323526.03, -32.6, 187275.28);
        let (region, block) = active_block(position);
        assert_eq!(region, (168, 97));
        assert_eq!(block, 3 * 6 + 3);
    }

    #[test]
    fn active_block_region_origin_is_block_zero() {
        let position = Vec3::new(-168.0 * REGION_SIZE, 0.0, 97.0 * REGION_SIZE);
        let (region, block) = active_block(position);
        assert_eq!(region, (168, 97));
        assert_eq!(block, 0);
    }

    #[test]
    fn active_block_last_block_of_region() {
        // Just inside the far corner of region 10/20.
        let position = Vec3::new(-(11.0 * REGION_SIZE - 1.0), 0.0, 21.0 * REGION_SIZE - 1.0);
        let (region, block) = active_block(position);
        assert_eq!(region, (10, 20));
        assert_eq!(block, 35);
    }
}
