use crate::assets::m::block_splat_material::TerrainBlockSplatMaterial;
use crate::assets::m::TerrainBlock;
use crate::assets::o2::MapObject;
use crate::commands::{Bone, MeshGroup, SpawnedFromResource};
use crate::plugins::animation_culling::PausedAnimationGraph;
use crate::plugins::camera::CameraLayers;
use crate::plugins::effects::spawn::{EffectMaterials, EffectMeshes};
use crate::plugins::effects::{
    EffectInstance, EffectNode, EffectSimPaused, EmittedBy, PooledParticle,
};
use crate::plugins::map::foliage::FoliageBlock;
use crate::plugins::map::objects::{
    CompoundPart, LoadingCompound, LoadingResources, SpawnedMapObjects, SroBindPoses, SroMeshes,
};
use crate::plugins::map::terrain::{TerrainLoadState, WaterPlane};
use crate::GameState;
use bevy::camera::visibility::RenderLayers;
use bevy::diagnostic::{
    Diagnostic, DiagnosticPath, Diagnostics, DiagnosticsStore, EntityCountDiagnosticsPlugin,
    FrameTimeDiagnosticsPlugin, RegisterDiagnostic,
};
use bevy::ecs::entity::Entities;
use bevy::mesh::Mesh3d;
use bevy::pbr::MeshMaterial3d;
use bevy::prelude::*;
use bevy::remote::BrpResult;
use bevy::render::render_phase::{
    BinnedPhaseItem, SortedPhaseItem, ViewBinnedRenderPhases, ViewSortedRenderPhases,
};
use bevy::render::{Render, RenderApp, RenderSystems};
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

// A unit struct to help identify the FPS UI component, since there may be many Text components
#[derive(Component)]
struct FpsText;

/// The multi-line per-category entity table below the FPS row.
#[derive(Component)]
struct StatsText;

// One counter per suspect entity category, registered as real diagnostics so
// both the in-game panel and BRP clients can attribute entity-count jumps
// without an inspector. Marker-only `With<C>` queries are archetype-level, so
// counting is O(archetypes), effectively free — the counters stay always-on.
pub const TERRAIN_BLOCK_COUNT: DiagnosticPath =
    DiagnosticPath::const_new("world_counts/terrain_blocks");
pub const TERRAIN_TILE_COUNT: DiagnosticPath =
    DiagnosticPath::const_new("world_counts/terrain_tiles");
pub const MAP_OBJECT_COUNT: DiagnosticPath = DiagnosticPath::const_new("world_counts/map_objects");
pub const COMPOUND_PART_COUNT: DiagnosticPath =
    DiagnosticPath::const_new("world_counts/compound_parts");
pub const RESOURCE_ROOT_COUNT: DiagnosticPath =
    DiagnosticPath::const_new("world_counts/resource_roots");
pub const MESH_GROUP_COUNT: DiagnosticPath = DiagnosticPath::const_new("world_counts/mesh_groups");
pub const MESH_PART_COUNT: DiagnosticPath = DiagnosticPath::const_new("world_counts/mesh_parts");
pub const FOLIAGE_BLOCK_COUNT: DiagnosticPath =
    DiagnosticPath::const_new("world_counts/foliage_blocks");
pub const WATER_COUNT: DiagnosticPath = DiagnosticPath::const_new("world_counts/water");
pub const EFFECT_INSTANCE_COUNT: DiagnosticPath =
    DiagnosticPath::const_new("world_counts/effect_instances");
pub const EFFECT_NODE_COUNT: DiagnosticPath =
    DiagnosticPath::const_new("world_counts/effect_nodes");
pub const PARTICLE_COUNT: DiagnosticPath = DiagnosticPath::const_new("world_counts/particles");
pub const POOLED_PARTICLE_COUNT: DiagnosticPath =
    DiagnosticPath::const_new("world_counts/pooled_particles");
pub const BONE_COUNT: DiagnosticPath = DiagnosticPath::const_new("world_counts/bones");
pub const UI_NODE_COUNT: DiagnosticPath = DiagnosticPath::const_new("world_counts/ui_nodes");
pub const OTHER_COUNT: DiagnosticPath = DiagnosticPath::const_new("world_counts/other");

// Load/gating gauges, BRP-only (not in `PANEL_ROWS` and not subtracted in
// `other_count_system` — they overlap the categories above): in-flight object
// loads, the terrain build queue, and how many animations/effect subtrees the
// distance gates currently hold paused.
pub const LOADING_COMPOUND_COUNT: DiagnosticPath =
    DiagnosticPath::const_new("world_counts/loading_compounds");
pub const LOADING_RESOURCES_COUNT: DiagnosticPath =
    DiagnosticPath::const_new("world_counts/loading_resources");
pub const PAUSED_ANIMATION_COUNT: DiagnosticPath =
    DiagnosticPath::const_new("world_counts/paused_animations");
pub const PAUSED_EFFECT_COUNT: DiagnosticPath =
    DiagnosticPath::const_new("world_counts/paused_effects");
pub const TERRAIN_BUILDING_COUNT: DiagnosticPath =
    DiagnosticPath::const_new("world_counts/terrain_building");

// Sizes of the dedup/registry HashMaps (mesh, bind-pose, material and
// map-object caches). Growth that never plateaus while revisiting the same
// area = a cache leak; sampled at 4 Hz, that's all the resolution needed.
pub const SRO_MESH_CACHE: DiagnosticPath = DiagnosticPath::const_new("cache_counts/sro_meshes");
pub const SRO_BIND_POSE_CACHE: DiagnosticPath =
    DiagnosticPath::const_new("cache_counts/sro_bind_poses");
pub const SPAWNED_MAP_OBJECT_CACHE: DiagnosticPath =
    DiagnosticPath::const_new("cache_counts/spawned_map_objects");
pub const EFFECT_MESH_CACHE: DiagnosticPath =
    DiagnosticPath::const_new("cache_counts/effect_meshes");
pub const EFFECT_MATERIAL_CACHE: DiagnosticPath =
    DiagnosticPath::const_new("cache_counts/effect_materials");

pub struct DiagnosticsPlugin;

impl Plugin for DiagnosticsPlugin {
    fn build(&self, app: &mut App) {
        // The FPS overlay must own its diagnostic source: previously the
        // frame-time diagnostic was only present as a side effect of
        // BrpExtrasPlugin (which defensively installs it), so gating BRP
        // behind `dev_tools` silently froze the FPS counter.
        app.add_plugins(FrameTimeDiagnosticsPlugin::default())
            .add_plugins(EntityCountDiagnosticsPlugin::default())
            // process/mem_usage (GB) + cpu_usage in the BRP dump: macOS
            // sandboxing blocks ps/vmmap from outside, so leak hunts need the
            // game to report its own footprint.
            .add_plugins(bevy::diagnostic::SystemInformationDiagnosticsPlugin)
            .register_diagnostic(Diagnostic::new(MESH_PART_COUNT).with_smoothing_factor(0.0))
            .register_diagnostic(Diagnostic::new(OTHER_COUNT).with_smoothing_factor(0.0))
            .register_diagnostic(Diagnostic::new(TERRAIN_BUILDING_COUNT).with_smoothing_factor(0.0))
            .register_diagnostic(Diagnostic::new(SRO_MESH_CACHE).with_smoothing_factor(0.0))
            .register_diagnostic(Diagnostic::new(SRO_BIND_POSE_CACHE).with_smoothing_factor(0.0))
            .register_diagnostic(
                Diagnostic::new(SPAWNED_MAP_OBJECT_CACHE).with_smoothing_factor(0.0),
            )
            .register_diagnostic(Diagnostic::new(EFFECT_MESH_CACHE).with_smoothing_factor(0.0))
            .register_diagnostic(Diagnostic::new(EFFECT_MATERIAL_CACHE).with_smoothing_factor(0.0))
            .add_systems(OnEnter(GameState::Loading), setup)
            .add_systems(
                Update,
                (
                    // Rewriting a bevy_ui Text relayouts it, so the overlay
                    // refreshes at 4 Hz (plenty for reading numbers) instead
                    // of building ~17 format! strings + relayouting per frame.
                    (fps_update_system, stats_text_update_system).run_if(
                        bevy::time::common_conditions::on_timer(std::time::Duration::from_millis(
                            250,
                        )),
                    ),
                    // Cache sizes only move while regions stream in; 4 Hz
                    // keeps six resource borrows off the per-frame schedule.
                    cache_count_system.run_if(bevy::time::common_conditions::on_timer(
                        std::time::Duration::from_millis(250),
                    )),
                    mesh_part_count_system,
                    other_count_system,
                    terrain_building_count_system,
                ),
            );

        // Terrain blocks each carry a unique splat material (a distinct draw
        // call/bind group), so their count is a direct proxy for terrain draw
        // cost. Water counts every streamed water/ice surface via `WaterPlane`
        // rather than one tier's material type — `graphics.water.quality: low`
        // swaps the material, and keying on the HQ type made the row read 0
        // (and miscounted the planes as `mesh parts`) in that tier.
        track::<MeshMaterial3d<TerrainBlockSplatMaterial>>(app, TERRAIN_BLOCK_COUNT);
        track::<TerrainBlock>(app, TERRAIN_TILE_COUNT);
        track::<MapObject>(app, MAP_OBJECT_COUNT);
        track::<CompoundPart>(app, COMPOUND_PART_COUNT);
        track::<SpawnedFromResource>(app, RESOURCE_ROOT_COUNT);
        track::<MeshGroup>(app, MESH_GROUP_COUNT);
        track::<WaterPlane>(app, WATER_COUNT);
        track::<FoliageBlock>(app, FOLIAGE_BLOCK_COUNT);
        track::<EffectInstance>(app, EFFECT_INSTANCE_COUNT);
        track::<EffectNode>(app, EFFECT_NODE_COUNT);
        track::<EmittedBy>(app, PARTICLE_COUNT);
        track::<PooledParticle>(app, POOLED_PARTICLE_COUNT);
        track::<Bone>(app, BONE_COUNT);
        track::<Node>(app, UI_NODE_COUNT);
        track::<LoadingCompound>(app, LOADING_COMPOUND_COUNT);
        track::<LoadingResources>(app, LOADING_RESOURCES_COUNT);
        track::<PausedAnimationGraph>(app, PAUSED_ANIMATION_COUNT);
        track::<EffectSimPaused>(app, PAUSED_EFFECT_COUNT);
    }
}

/// Closure-factory counter system (same pattern as Bevy's `in_state`): one
/// generic system per tracked marker component instead of N hand-written ones.
fn count_marker<C: Component>(path: DiagnosticPath) -> impl FnMut(Diagnostics, Query<(), With<C>>) {
    move |mut diagnostics, query| {
        diagnostics.add_measurement(&path, || query.iter().len() as f64);
    }
}

fn track<C: Component>(app: &mut App, path: DiagnosticPath) {
    app.register_diagnostic(Diagnostic::new(path.clone()).with_smoothing_factor(0.0))
        .add_systems(Update, count_marker::<C>(path));
}

/// Meshes not already attributed to terrain, water, or effects: object and
/// character mesh parts (the children `spawn_mesh_groups` stamps out — the
/// bulk of a loaded region's entities, several per `MapObject`).
type MeshPartFilter = (
    With<Mesh3d>,
    Without<MeshMaterial3d<TerrainBlockSplatMaterial>>,
    Without<WaterPlane>,
    Without<EffectNode>,
    Without<FoliageBlock>,
);

fn mesh_part_count_system(mut diagnostics: Diagnostics, parts: Query<(), MeshPartFilter>) {
    diagnostics.add_measurement(&MESH_PART_COUNT, || parts.iter().len() as f64);
}

// Accounting rule for the remainder: subtract only *disjoint* categories from
// the total. `EmittedBy` particles are themselves `EffectNode` trees and
// `EffectInstance` wrappers parent `EffectNode` children, so effects are
// subtracted once via `Or<(EffectNode, EffectInstance)>`; the `Without<>`
// guards keep entities that fall into two categories from being subtracted
// twice. What remains in "other": region roots, loading intermediates,
// scene-spawned resource anchors, UI text spans, cameras/lights, and
// engine-internal entities (systems, observers).
fn other_count_system(
    mut diagnostics: Diagnostics,
    entities: &Entities,
    terrain: Query<(), With<MeshMaterial3d<TerrainBlockSplatMaterial>>>,
    tiles: Query<(), With<TerrainBlock>>,
    objects: Query<(), With<MapObject>>,
    compound_parts: Query<(), With<CompoundPart>>,
    resource_roots: Query<(), With<SpawnedFromResource>>,
    mesh_groups: Query<(), With<MeshGroup>>,
    mesh_parts: Query<(), MeshPartFilter>,
    foliage: Query<(), With<FoliageBlock>>,
    water: Query<(), With<WaterPlane>>,
    effects: Query<(), Or<(With<EffectNode>, With<EffectInstance>)>>,
    bone: Query<(), With<Bone>>,
    ui_nodes: Query<(), With<Node>>,
) {
    diagnostics.add_measurement(&OTHER_COUNT, || {
        let categorized = terrain.iter().len()
            + tiles.iter().len()
            + objects.iter().len()
            + compound_parts.iter().len()
            + resource_roots.iter().len()
            + mesh_groups.iter().len()
            + mesh_parts.iter().len()
            + foliage.iter().len()
            + water.iter().len()
            + effects.iter().len()
            + bone.iter().len()
            + ui_nodes.iter().len();
        // Same total source as EntityCountDiagnosticsPlugin, so the panel rows
        // sum to its `entities` row.
        entities.count_spawned() as f64 - categorized as f64
    });
}

/// Depth of the terrain build queue: regions parked mid-build by the
/// per-frame merge-group budget (`GROUP_BUILDS_PER_FRAME`). A value-match on
/// the enum, so it can't reuse the marker-only `track::<C>` helper.
fn terrain_building_count_system(mut diagnostics: Diagnostics, states: Query<&TerrainLoadState>) {
    diagnostics.add_measurement(&TERRAIN_BUILDING_COUNT, || {
        states
            .iter()
            .filter(|s| matches!(s, TerrainLoadState::BuildingMeshes { .. }))
            .count() as f64
    });
}

/// The caches are `Option<Res<..>>` because test scenes without `MapPlugin` /
/// `EffectsPlugin` lack them — their diagnostics simply stay empty there.
fn cache_count_system(
    mut diagnostics: Diagnostics,
    sro_meshes: Option<Res<SroMeshes>>,
    sro_bind_poses: Option<Res<SroBindPoses>>,
    spawned_map_objects: Option<Res<SpawnedMapObjects>>,
    effect_meshes: Option<Res<EffectMeshes>>,
    effect_materials: Option<Res<EffectMaterials>>,
) {
    if let Some(cache) = sro_meshes {
        diagnostics.add_measurement(&SRO_MESH_CACHE, || cache.0.len() as f64);
    }
    if let Some(cache) = sro_bind_poses {
        diagnostics.add_measurement(&SRO_BIND_POSE_CACHE, || cache.0.len() as f64);
    }
    if let Some(cache) = spawned_map_objects {
        diagnostics.add_measurement(&SPAWNED_MAP_OBJECT_CACHE, || cache.0.len() as f64);
    }
    if let Some(cache) = effect_meshes {
        diagnostics.add_measurement(&EFFECT_MESH_CACHE, || cache.0.len() as f64);
    }
    if let Some(cache) = effect_materials {
        diagnostics.add_measurement(&EFFECT_MATERIAL_CACHE, || cache.0.len() as f64);
    }
}

fn setup(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands
        .spawn((
            RenderLayers::layer(CameraLayers::Debug.into()),
            Node {
                position_type: PositionType::Absolute,
                right: Val::Px(12.0),
                bottom: Val::Px(12.0),
                padding: UiRect::axes(Val::Px(10.0), Val::Px(6.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(4.0),
                ..default()
            },
            BackgroundColor(Color::srgba(0.02, 0.025, 0.03, 0.72)),
            // Debug overlay: pin it to the back of the UI stack so it never
            // z-fights with (or covers) the HUD. Render layers don't order UI
            // within a camera pass — GlobalZIndex does — and without one this
            // panel ties at 0 with the underbar buttons, flipping in front of
            // them depending on spawn order.
            GlobalZIndex(i32::MIN),
            Name::from("Performance Panel"),
        ))
        .with_children(|children| {
            children
                .spawn((
                    RenderLayers::layer(CameraLayers::Debug.into()),
                    Text::new("FPS: "),
                    TextFont {
                        font: asset_server
                            .load::<Font>(crate::assets::BUNDLED_FALLBACK_FACE)
                            .into(),
                        font_size: FontSize::Px(18.0),
                        ..default()
                    },
                    TextColor(Color::WHITE),
                    Name::from("FPS Label"),
                ))
                .with_children(|fps_text| {
                    fps_text.spawn((
                        TextSpan::default(),
                        TextFont {
                            font: asset_server
                                .load::<Font>("fonts/FiraMono-Medium.ttf")
                                .into(),
                            font_size: FontSize::Px(18.0),
                            ..default()
                        },
                        TextColor(bevy::color::palettes::css::GOLD.into()),
                        FpsText,
                    ));
                });
            children.spawn((
                RenderLayers::layer(CameraLayers::Debug.into()),
                Text::new(""),
                TextFont {
                    font: asset_server
                        .load::<Font>("fonts/FiraMono-Medium.ttf")
                        .into(),
                    font_size: FontSize::Px(13.0),
                    ..default()
                },
                TextColor(bevy::color::palettes::css::GOLD.into()),
                StatsText,
                Name::from("Entity Stats"),
            ));
        });
}

fn fps_update_system(
    diagnostics: Res<DiagnosticsStore>,
    mut query: Query<&mut TextSpan, With<FpsText>>,
) {
    for mut span in &mut query {
        if let Some(fps) = diagnostics.get(&FrameTimeDiagnosticsPlugin::FPS) {
            if let Some(value) = fps.smoothed() {
                let formatted = format!("{value:.2}");
                if span.0 != formatted {
                    span.0 = formatted;
                }
            }
        }
    }
}

/// Label/diagnostic pairs rendered by `stats_text_update_system`, top to
/// bottom. Adding a category = one `track::<Marker>` call + one row here.
static PANEL_ROWS: [(&str, DiagnosticPath); 17] = [
    ("entities", EntityCountDiagnosticsPlugin::ENTITY_COUNT),
    ("terrain blocks", TERRAIN_BLOCK_COUNT),
    ("terrain tiles", TERRAIN_TILE_COUNT),
    ("map objects", MAP_OBJECT_COUNT),
    ("compound parts", COMPOUND_PART_COUNT),
    ("resource roots", RESOURCE_ROOT_COUNT),
    ("mesh groups", MESH_GROUP_COUNT),
    ("mesh parts", MESH_PART_COUNT),
    ("foliage", FOLIAGE_BLOCK_COUNT),
    ("water", WATER_COUNT),
    ("effect roots", EFFECT_INSTANCE_COUNT),
    ("effect nodes", EFFECT_NODE_COUNT),
    ("  particles", PARTICLE_COUNT),
    ("  pooled", POOLED_PARTICLE_COUNT),
    ("bones", BONE_COUNT),
    ("ui nodes", UI_NODE_COUNT),
    ("other", OTHER_COUNT),
];

/// Draw calls per render phase, from [`RenderPhaseDiagnosticsPlugin`]. Present
/// only when the `diagnostics` config tier is on; rows are skipped otherwise.
///
/// This is the batching readout: `opaque` and `prepass` are the object/terrain
/// draw count, `transparent` is the effects-and-blended-water count that never
/// batches, and `shadow` is what each cascade re-draws.
static PHASE_ROWS: [(&str, &str); 5] = [
    ("draws opaque", "opaque_3d"),
    ("draws mask", "alpha_mask_3d"),
    ("draws prepass", "opaque_prepass"),
    ("draws transp", "transparent_3d"),
    ("draws shadow", "shadow"),
];

/// How many render-pass timing rows the panel will print before giving up. The
/// pass list is walked dynamically (see `render_pass_rows`) rather than hard-
/// coded, so it has to be bounded by something; the panel is a corner overlay,
/// not a report.
const MAX_RENDER_PASS_ROWS: usize = 12;

/// The render-pass timings to show, most expensive first.
///
/// Walked out of the store rather than listed, for two reasons. The passes that
/// exist depend on configuration (shadows, bloom, the transmissive copy, the
/// prepass) and on Bevy's internal graph, so a hard-coded list quietly goes
/// stale — the previous one named three passes and missed the shadow pass
/// entirely. And each pass reports up to two spans: `elapsed_gpu` is only
/// recorded where wgpu supports timestamp queries (Vulkan and DX12 — Bevy
/// requests every adapter feature via `WgpuSettingsPriority::Functionality`, so
/// no opt-in is needed), while Metal and WebGPU yield `elapsed_cpu`, the
/// command-encode time, alone. Preferring GPU per pass means the panel shows
/// real GPU cost where the platform has it without a `cfg`.
fn render_pass_rows(diagnostics: &DiagnosticsStore) -> Vec<(String, f64, bool)> {
    let mut by_pass: std::collections::HashMap<String, (Option<f64>, Option<f64>)> =
        std::collections::HashMap::new();
    for diagnostic in diagnostics.iter() {
        let path = diagnostic.path().as_str();
        let Some(rest) = path.strip_prefix("render/") else {
            continue;
        };
        let (pass, metric) = match rest.rsplit_once('/') {
            Some(split) => split,
            None => continue,
        };
        let Some(value) = diagnostic.smoothed() else {
            continue;
        };
        let entry = by_pass.entry(pass.to_string()).or_default();
        match metric {
            "elapsed_gpu" => entry.0 = Some(value),
            "elapsed_cpu" => entry.1 = Some(value),
            _ => {}
        }
    }
    let mut rows: Vec<(String, f64, bool)> = by_pass
        .into_iter()
        .filter_map(|(pass, (gpu, cpu))| match (gpu, cpu) {
            (Some(v), _) => Some((pass, v, true)),
            (None, Some(v)) => Some((pass, v, false)),
            (None, None) => None,
        })
        .collect();
    // Cost order, not name order: the point of the block is which pass to attack.
    rows.sort_by(|a, b| b.1.total_cmp(&a.1));
    rows.truncate(MAX_RENDER_PASS_ROWS);
    rows
}

/// Trims Bevy's render-graph node names to fit the panel's 16-character label
/// column: `main_opaque_pass_3d` -> `opaque`, `main_transparent_pass_3d` ->
/// `transparent`. Names that do not match the pattern are left alone.
fn short_pass_name(pass: &str) -> &str {
    let pass = pass.strip_prefix("main_").unwrap_or(pass);
    let pass = pass.strip_suffix("_pass_3d").unwrap_or(pass);
    pass.strip_suffix("_3d").unwrap_or(pass)
}

fn stats_text_update_system(
    diagnostics: Res<DiagnosticsStore>,
    mut query: Query<&mut Text, With<StatsText>>,
) {
    let mut lines = String::new();
    for (label, path) in PANEL_ROWS.iter() {
        let value = diagnostics.get(path).and_then(|d| d.value());
        match value {
            Some(v) => lines.push_str(&format!("{label:<16}{v:>7.0}\n")),
            None => lines.push_str(&format!("{label:<16}{:>7}\n", "-")),
        }
    }
    for (label, phase) in PHASE_ROWS.iter() {
        let path = phase_path(phase, "draws");
        if let Some(v) = diagnostics.get(&path).and_then(|d| d.value()) {
            lines.push_str(&format!("{label:<16}{v:>7.0}\n"));
        }
    }
    // Suffix marks the source: "gpu" is a real timestamp query, "cpu" is the
    // command-encode time on a platform without them.
    for (pass, value, is_gpu) in render_pass_rows(&diagnostics) {
        let unit = if is_gpu { "gpu" } else { "cpu" };
        let label = format!("{} {unit}", short_pass_name(&pass));
        lines.push_str(&format!("{label:<16}{value:>7.2}\n"));
    }
    // Net entity drift over the diagnostic's history window (~120 frames).
    // Steady-state churn (spawn N / despawn N per second) is invisible here —
    // it shows up as a large-but-stable `particles` row instead. A persistent
    // non-zero drift while standing still means a leak.
    if let Some(diag) = diagnostics.get(&EntityCountDiagnosticsPlugin::ENTITY_COUNT) {
        if let (Some(first), Some(last), Some(duration)) =
            (diag.values().next(), diag.values().last(), diag.duration())
        {
            let secs = duration.as_secs_f64();
            if secs > f64::EPSILON {
                let rate = (last - first) / secs;
                lines.push_str(&format!("{:<16}{rate:>+7.0}/s", "Δ entities"));
            }
        }
    }
    for mut text in &mut query {
        if text.0 != lines {
            text.0 = lines.clone();
        }
    }
}

/// BRP method `openroad/diagnostics` (registered in `main.rs` when
/// `dev_tools` is on): dumps every diagnostic in the store as JSON, since
/// `brp_extras/get_diagnostics` only exposes FPS/frame-time and `bevy/query`
/// cannot filter on the non-Reflect marker components counted above.
pub fn brp_all_diagnostics(
    In(_params): In<Option<serde_json::Value>>,
    diagnostics: Res<DiagnosticsStore>,
) -> BrpResult {
    let mut map = serde_json::Map::new();
    for diagnostic in diagnostics.iter() {
        let Some(value) = diagnostic.value() else {
            continue;
        };
        map.insert(
            diagnostic.path().as_str().to_string(),
            // `smoothed` is what the on-screen FPS overlay shows, so remote
            // and in-game numbers stay directly comparable.
            serde_json::json!({
                "value": value,
                "avg": diagnostic.average(),
                "smoothed": diagnostic.smoothed(),
            }),
        );
    }
    Ok(serde_json::Value::Object(map))
}

// ---------------------------------------------------------------------------
// Draw items per render phase
// ---------------------------------------------------------------------------

/// Per-phase draw-call counts — the number every batching lever is defined in
/// terms of.
///
/// The idea: frame-time work in `bevy_render` scales with *draw items*, not with
/// entities. `prepare_material_bind_groups`, the instance-buffer writers and the
/// passes themselves all walk the binned and sorted phases, so "collapse the
/// terrain materials", "restore effect batching" and "merge object mesh parts"
/// are all statements about these counts. Nothing in the tree reported them,
/// which is why those levers were arguments rather than measurements.
///
/// Mechanics: the counters live behind an `Arc` inserted into *both* worlds —
/// the pattern `RenderDiagnosticsPlugin` uses for its own mutex — because the
/// phases are only final late in the `Render` schedule, well past the point
/// where `Extract` is available. They cannot be read from `ExtractSchedule`
/// instead: `extract_core_3d_camera_phases` and its siblings call
/// `prepare_for_new_frame` there, which clears the sorted phases.
///
/// Counts are summed over every view, so shadow cascades and the offscreen
/// portrait/paper-doll rigs are included — the GPU pays for all of them.
///
/// One number is deliberately absent: the instance count inside a multidraw
/// batch set. `RenderMultidrawableBatchSet`'s bins are private in bevy 0.19, so
/// reporting "instances" would mean reporting a figure that silently excludes
/// whatever the multidraw path swallowed. Draw calls are the actionable number
/// and are fully reachable.
pub struct RenderPhaseDiagnosticsPlugin;

impl Plugin for RenderPhaseDiagnosticsPlugin {
    fn build(&self, app: &mut App) {
        use bevy::core_pipeline::core_3d::{AlphaMask3d, Opaque3d, Transparent3d};
        use bevy::core_pipeline::prepass::{AlphaMask3dPrepass, Opaque3dPrepass};
        use bevy::pbr::{Shadow, Transmissive3d};

        binned_phase::<Opaque3d>(app, "opaque_3d");
        binned_phase::<AlphaMask3d>(app, "alpha_mask_3d");
        binned_phase::<Opaque3dPrepass>(app, "opaque_prepass");
        binned_phase::<AlphaMask3dPrepass>(app, "alpha_mask_prepass");
        binned_phase::<Shadow>(app, "shadow");
        sorted_phase::<Transmissive3d>(app, "transmissive_3d");
        sorted_phase::<Transparent3d>(app, "transparent_3d");
    }
}

/// Where each frame's wall time actually goes, split across the two threads
/// that produce it.
///
/// Idea: a 2026-09-05 Tracy capture found that **nothing in the frame was
/// saturated** — the main thread waited 12.53 ms, the render thread waited
/// 10.80 ms inside the swapchain acquire, and the GPU was busy only ~13.9 ms of
/// a 21.85 ms frame. That is the signature of a serialised pipeline rather than
/// a busy one, and it is worth more than any per-pass number, because it says
/// the ceiling is the *shape* of the frame and not the work in it.
///
/// Finding it cost a 26 MB Tracy capture, a CSV export and a bespoke script.
/// These four rows put the same picture on every `make perf` line, so a change
/// can be judged against it without re-deriving it each time:
///
/// - `acquire_ms`   — inside `prepare_windows`, i.e. waiting for a swapchain image
/// - `render_cpu_ms`— the rest of the `Render` schedule
/// - `main_ms`      — the main thread's own work
/// - `extract_wait_ms` — the gap between one `Main` and the next: extract, plus
///   the main thread waiting for the render thread to release the world
///
/// `main_ms + extract_wait_ms` is the frame; so is `acquire_ms + render_cpu_ms`
/// plus whatever the render thread waits for outside its schedule. Reading them
/// together is what distinguishes "this thread is busy" from "this thread is
/// blocked on the other one".
///
/// Mechanics are the [`RenderPhaseDiagnosticsPlugin`] pattern: an `Arc` of
/// atomics in both worlds, because the render thread cannot touch the main
/// world's `Diagnostics`.
pub struct PipelineDiagnosticsPlugin;

#[derive(Default)]
struct PipelineTimings {
    acquire_ns: AtomicU64,
    render_cpu_ns: AtomicU64,
    main_ns: AtomicU64,
    extract_wait_ns: AtomicU64,
}

/// The shared atomics, under one resource identity per world.
#[derive(Resource)]
struct PipelineMeasurements(Arc<PipelineTimings>);

/// Render-thread marks. Two, because the acquire is bracketed *inside* the
/// schedule that is itself being timed.
#[derive(Resource)]
struct RenderMarks {
    schedule_start: Instant,
    acquire_start: Instant,
}

/// Main-thread marks. `main_end` is what makes the inter-frame gap measurable
/// at all: the wait we care about happens between two `Main` runs, where no
/// system of ours is scheduled.
#[derive(Resource)]
struct MainMarks {
    main_start: Instant,
    main_end: Option<Instant>,
}

fn pipeline_path(metric: &str) -> DiagnosticPath {
    DiagnosticPath::from_components(["render", "pipeline", metric])
}

const PIPELINE_METRICS: [&str; 4] = ["acquire_ms", "render_cpu_ms", "main_ms", "extract_wait_ms"];

impl Plugin for PipelineDiagnosticsPlugin {
    fn build(&self, app: &mut App) {
        let timings = Arc::new(PipelineTimings::default());
        for metric in PIPELINE_METRICS {
            app.register_diagnostic(
                Diagnostic::new(pipeline_path(metric)).with_smoothing_factor(0.0),
            );
        }
        app.insert_resource(PipelineMeasurements(timings.clone()))
            .insert_resource(MainMarks {
                main_start: Instant::now(),
                main_end: None,
            })
            // `First` and `Last` bracket the main thread's own work, and the
            // gap between them across frames is the extract-plus-wait. Both
            // have to be ordinary systems in those schedules -- there is no
            // hook around the pipelined-rendering handover itself.
            .add_systems(First, mark_main_start)
            .add_systems(Last, mark_main_end)
            .add_systems(PreUpdate, publish_pipeline_timings);

        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .insert_resource(PipelineMeasurements(timings))
            .insert_resource(RenderMarks {
                schedule_start: Instant::now(),
                acquire_start: Instant::now(),
            })
            .add_systems(
                Render,
                (
                    mark_render_start.before(RenderSystems::ExtractCommands),
                    // Ordered against the system, not its set: `PrepareViews`
                    // holds other systems too, and bracketing the set would
                    // charge their cost to the acquire.
                    mark_acquire_start.before(bevy::render::view::prepare_windows),
                    mark_acquire_end.after(bevy::render::view::prepare_windows),
                    mark_render_end.after(RenderSystems::Render),
                ),
            );
    }
}

fn mark_main_start(mut marks: ResMut<MainMarks>, m: Res<PipelineMeasurements>) {
    let now = Instant::now();
    if let Some(end) = marks.main_end {
        m.0.extract_wait_ns
            .store(now.duration_since(end).as_nanos() as u64, Ordering::Relaxed);
    }
    marks.main_start = now;
}

fn mark_main_end(mut marks: ResMut<MainMarks>, m: Res<PipelineMeasurements>) {
    let now = Instant::now();
    m.0.main_ns.store(
        now.duration_since(marks.main_start).as_nanos() as u64,
        Ordering::Relaxed,
    );
    marks.main_end = Some(now);
}

fn mark_render_start(mut marks: ResMut<RenderMarks>) {
    marks.schedule_start = Instant::now();
}

fn mark_acquire_start(mut marks: ResMut<RenderMarks>) {
    marks.acquire_start = Instant::now();
}

fn mark_acquire_end(marks: Res<RenderMarks>, m: Res<PipelineMeasurements>) {
    // A near-zero sample is a real reading, not a dropped one: `prepare_windows`
    // returns immediately when the previous frame was never presented and the
    // swapchain texture is still held. Filtering those out would flatter the
    // average and hide exactly the frames where the pipeline did *not* stall.
    m.0.acquire_ns.store(
        marks.acquire_start.elapsed().as_nanos() as u64,
        Ordering::Relaxed,
    );
}

fn mark_render_end(marks: Res<RenderMarks>, m: Res<PipelineMeasurements>) {
    let total = marks.schedule_start.elapsed().as_nanos() as u64;
    let acquire = m.0.acquire_ns.load(Ordering::Relaxed);
    m.0.render_cpu_ns
        .store(total.saturating_sub(acquire), Ordering::Relaxed);
}

fn publish_pipeline_timings(mut diagnostics: Diagnostics, m: Res<PipelineMeasurements>) {
    let ms = |ns: u64| ns as f64 / 1e6;
    let t = &m.0;
    diagnostics.add_measurement(&pipeline_path("acquire_ms"), || {
        ms(t.acquire_ns.load(Ordering::Relaxed))
    });
    diagnostics.add_measurement(&pipeline_path("render_cpu_ms"), || {
        ms(t.render_cpu_ns.load(Ordering::Relaxed))
    });
    diagnostics.add_measurement(&pipeline_path("main_ms"), || {
        ms(t.main_ns.load(Ordering::Relaxed))
    });
    diagnostics.add_measurement(&pipeline_path("extract_wait_ms"), || {
        ms(t.extract_wait_ns.load(Ordering::Relaxed))
    });
}

#[derive(Default)]
struct PhaseCounters {
    /// Multi-draw-indirect batch sets: one GPU draw command each.
    batch_sets: AtomicUsize,
    /// Batchable-but-not-multidrawable bins: one draw each.
    bins: AtomicUsize,
    /// Entities that could not be batched at all: one draw each.
    unbatchable: AtomicUsize,
    /// The three above plus non-mesh items — the phase's draw-call count.
    draws: AtomicUsize,
}

/// The same `Arc` under two resource identities, one per world.
#[derive(Resource)]
struct PhaseMeasurements<P> {
    counters: Arc<PhaseCounters>,
    _phantom: PhantomData<fn() -> P>,
}

impl<P> PhaseMeasurements<P> {
    fn new(counters: Arc<PhaseCounters>) -> Self {
        Self {
            counters,
            _phantom: PhantomData,
        }
    }
}

fn phase_path(phase: &str, metric: &str) -> DiagnosticPath {
    DiagnosticPath::from_components(["render_phase", phase, metric])
}

/// Registers the diagnostics and the main-world reader, and hands back the
/// shared counters for the caller's render-world writer. `binned` decides
/// whether the three batching breakdown rows exist at all: a sorted phase has
/// no bins, so publishing them as a constant 0 would be a lie.
fn register_phase<P: Send + Sync + 'static>(
    app: &mut App,
    phase: &'static str,
    binned: bool,
) -> Arc<PhaseCounters> {
    let counters = Arc::new(PhaseCounters::default());
    let metrics: &[&str] = if binned {
        &["batch_sets", "bins", "unbatchable", "draws"]
    } else {
        &["draws"]
    };
    for metric in metrics {
        app.register_diagnostic(
            Diagnostic::new(phase_path(phase, metric)).with_smoothing_factor(0.0),
        );
    }
    app.insert_resource(PhaseMeasurements::<P>::new(counters.clone()))
        .add_systems(
            PreUpdate,
            move |mut diagnostics: Diagnostics, m: Res<PhaseMeasurements<P>>| {
                let c = &m.counters;
                if binned {
                    diagnostics.add_measurement(&phase_path(phase, "batch_sets"), || {
                        c.batch_sets.load(Ordering::Relaxed) as f64
                    });
                    diagnostics.add_measurement(&phase_path(phase, "bins"), || {
                        c.bins.load(Ordering::Relaxed) as f64
                    });
                    diagnostics.add_measurement(&phase_path(phase, "unbatchable"), || {
                        c.unbatchable.load(Ordering::Relaxed) as f64
                    });
                }
                diagnostics.add_measurement(&phase_path(phase, "draws"), || {
                    c.draws.load(Ordering::Relaxed) as f64
                });
            },
        );
    counters
}

fn binned_phase<P: BinnedPhaseItem>(app: &mut App, phase: &'static str) {
    let counters = register_phase::<P>(app, phase, true);
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render_app.insert_resource(PhaseMeasurements::<P>::new(counters));
    // After batching has built the batch sets, before the passes execute: the
    // counts are then exactly what the GPU is about to be handed.
    render_app.add_systems(
        Render,
        (|m: Res<PhaseMeasurements<P>>, phases: Res<ViewBinnedRenderPhases<P>>| {
            let (mut batch_sets, mut bins, mut unbatchable, mut non_mesh) = (0, 0, 0, 0);
            for phase in phases.values() {
                batch_sets += phase.multidrawable_meshes.len();
                bins += phase.batchable_meshes.len();
                unbatchable += phase
                    .unbatchable_meshes
                    .values()
                    .map(|b| b.entities.len())
                    .sum::<usize>();
                non_mesh += phase
                    .non_mesh_items
                    .values()
                    .map(|b| b.entities.len())
                    .sum::<usize>();
            }
            let c = &m.counters;
            c.batch_sets.store(batch_sets, Ordering::Relaxed);
            c.bins.store(bins, Ordering::Relaxed);
            c.unbatchable.store(unbatchable, Ordering::Relaxed);
            c.draws.store(
                batch_sets + bins + unbatchable + non_mesh,
                Ordering::Relaxed,
            );
        })
        .after(RenderSystems::PrepareResourcesBatchPhases)
        .before(RenderSystems::Render),
    );
}

/// Sorted phases (transparent, transmissive) are back-to-front lists with no
/// binning, so only `draws` is meaningful — every item is its own draw. That is
/// exactly why blended materials are expensive, and why this row is the one to
/// watch when A/B-ing `graphics.water.quality`: the low tier trades the
/// transmissive pass's per-frame texture copy for one sorted draw per plane.
fn sorted_phase<P: SortedPhaseItem>(app: &mut App, phase: &'static str) {
    let counters = register_phase::<P>(app, phase, false);
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render_app.insert_resource(PhaseMeasurements::<P>::new(counters));
    render_app.add_systems(
        Render,
        (|m: Res<PhaseMeasurements<P>>, phases: Res<ViewSortedRenderPhases<P>>| {
            let draws: usize = phases.values().map(|phase| phase.items.len()).sum();
            m.counters.draws.store(draws, Ordering::Relaxed);
        })
        .after(RenderSystems::PrepareResourcesBatchPhases)
        .before(RenderSystems::Render),
    );
}
