extern crate core;

use std::env;

use crate::assets::SroAssetStructsPlugin;
use crate::plugins::assets::sro::SroAssetPlugin;
use crate::plugins::config::ConfigPlugin;
use crate::plugins::cursor::CursorPlugin;
use crate::plugins::dev::DevPlugin;
use crate::plugins::dynamic_resource_loader::DynamicResourceLoaderPlugin;
use crate::plugins::textdata::TextdataPlugin;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSamplerDescriptor};
use bevy::picking::mesh_picking::ray_cast::RayCastVisibility;
use bevy::picking::mesh_picking::{MeshPickingPlugin, MeshPickingSettings};
use bevy::prelude::*;
use bevy::remote::RemotePlugin;
use bevy::render::render_asset::RenderAssetBytesPerFrame;
use bevy_brp_extras::BrpExtrasPlugin;
use bevy_egui::{EguiGlobalSettings, EguiPlugin, UiRenderOrder};
use bevy_inspector_egui::quick::StateInspectorPlugin;
use bevy_tweening::TweeningPlugin;

mod assets;
mod commands;
mod net;
mod netcheck;
mod plugins;
mod scenes;
mod util;

/// Per-frame GPU upload budget (textures + meshes), see the
/// `RenderAssetBytesPerFrame` insert in `main`. 16 MiB is a starting value
/// chosen from the cost of the copy, not measured on this client yet: staging
/// 16 MiB is a few ms of memcpy on the render thread, while still moving
/// ~0.5–1 GB/s at 30–60 fps — a whole region's textures within a fraction of a
/// second, well inside the fog-covered head start streaming has. Retune against
/// `frame_time/max_window` while crossing region boundaries.
const UPLOAD_BYTES_PER_FRAME: usize = 16 * 1024 * 1024;

#[derive(Resource)]
pub struct GameSettings {
    pub sro_path: String,
}

#[derive(States, Default, Clone, Eq, PartialEq, Debug, Hash, Reflect)]
pub enum GameState {
    #[default]
    Loading,
    Loaded,
    Game,
}

#[derive(States, Default, Clone, Eq, PartialEq, Debug, Hash)]
enum AppMode {
    PlayMode,
    #[default]
    DebugMode,
}

/// The GPU features Bevy's bindless material slabs need; withheld unless
/// `graphics.bindless_materials` is on (nothing else in the client uses binding arrays).
fn bindless_features() -> bevy::render::settings::WgpuFeatures {
    use bevy::render::settings::WgpuFeatures;
    WgpuFeatures::TEXTURE_BINDING_ARRAY
        | WgpuFeatures::BUFFER_BINDING_ARRAY
        | WgpuFeatures::SAMPLED_TEXTURE_AND_STORAGE_BUFFER_ARRAY_NON_UNIFORM_INDEXING
        | WgpuFeatures::PARTIALLY_BOUND_BINDING_ARRAY
}

/// The render plugin: Bevy's, minus the bindless-material features unless
/// `graphics.bindless_materials` asks for them.
///
/// Measurement switches on top:
/// - `OPENROAD_GPU_BASELINE=1` requests only what a ~2014 desktop GPU guarantees —
///   WebGPU-baseline limits, no optional features except BC texture compression (every
///   D3D10+-class GPU has it, and every DDJ texture is BC) — so features the client would need
///   beyond old hardware show up as validation errors on a current machine.
///   `WGPU_SETTINGS_PRIO=webgpu` alone also drops BC, which no real old PC lacks.
/// - `OPENROAD_GPU_BASELINE=gl33` goes further down, to what wgpu's GL 3.3 backend offers a
///   DX10-class card (GeForce 8-500, Radeon HD 2000-6000): WebGL2-class limits, so no storage
///   buffers, storage textures or compute workgroups, but a realistic 8192 texture size. It is
///   meant to be run on Vulkan or DX12, where it exercises Bevy's uniform-buffer fallbacks and
///   shows every compute path that is not gated as a validation error. The real backend
///   (`--features gles`, `WGPU_BACKEND=gl`) is the confirmation.
/// - `OPENROAD_WGPU_DISABLE` withholds more, a comma-separated list of groups — `bindless`,
///   `indirect` (GPU mesh preprocessing + multi-draw indirect), `mappable`
///   (MAPPABLE_PRIMARY_BUFFERS, which Bevy turns on for integrated GPUs) — or raw wgpu feature
///   names (`TIMESTAMP_QUERY`).
/// - `OPENROAD_GPU_LIMITS=webgpu` constrains the device to WebGPU-default limits, keeping the
///   features (combine with `bindless` off: Bevy enables bindless by feature alone, and the
///   WebGPU limits allow no binding-array elements).
fn render_plugin(
    graphics: &plugins::config::graphics::GraphicsSettings,
    // the probed adapter runs on wgpu's GL backend (`config::gpu_probe`)
    gpu_is_gl: bool,
) -> bevy::render::RenderPlugin {
    use bevy::render::settings::{WgpuFeatures, WgpuSettings, WgpuSettingsPriority};
    if env::var("OPENROAD_GPU_BASELINE").is_ok_and(|v| v == "1") {
        return bevy::render::RenderPlugin {
            render_creation: WgpuSettings {
                priority: WgpuSettingsPriority::WebGPU,
                features: WgpuFeatures::TEXTURE_COMPRESSION_BC,
                ..default()
            }
            .into(),
            ..default()
        };
    }
    if env::var("OPENROAD_GPU_BASELINE").is_ok_and(|v| v == "gl33") {
        let mut limits = bevy::render::settings::WgpuLimits::downlevel_webgl2_defaults();
        // GL 3.3 guarantees 1024, but every DX10-class desktop card offers 8192;
        // WebGL2's 2048 would reject textures those cards render fine.
        limits.max_texture_dimension_1d = 8192;
        limits.max_texture_dimension_2d = 8192;
        return bevy::render::RenderPlugin {
            render_creation: WgpuSettings {
                priority: WgpuSettingsPriority::WebGL2,
                features: WgpuFeatures::TEXTURE_COMPRESSION_BC,
                limits: limits.clone(),
                constrained_limits: Some(limits),
                ..default()
            }
            .into(),
            ..default()
        };
    }

    let mut disabled = if graphics.bindless_materials {
        WgpuFeatures::empty()
    } else {
        bindless_features()
    };
    for name in env::var("OPENROAD_WGPU_DISABLE")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        disabled |= match name {
            "bindless" => bindless_features(),
            "indirect" => {
                WgpuFeatures::MULTI_DRAW_INDIRECT_COUNT | WgpuFeatures::INDIRECT_FIRST_INSTANCE
            }
            "mappable" => WgpuFeatures::MAPPABLE_PRIMARY_BUFFERS,
            raw => WgpuFeatures::from_name(raw).unwrap_or_else(|| {
                eprintln!("OPENROAD_WGPU_DISABLE: unknown feature or group {raw:?}");
                WgpuFeatures::empty()
            }),
        };
    }
    let mut constrained_limits = env::var("OPENROAD_GPU_LIMITS")
        .is_ok_and(|v| v == "webgpu")
        .then(bevy::render::settings::WgpuLimits::default);
    // wgpu's GL backend: hold the device to the floor's limits (ADR 0011),
    // with fewer than the five storage textures Bevy's SSAO needs. On a real
    // GL 3.3 card there are none and Bevy never registers SSAO. A current GL
    // 4.x driver offers them, so Bevy builds SSAO's compute pipeline, whose
    // shader its GLSL translation cannot express (`textureGatherOffset`),
    // and quits. Capping the limit makes both take the same path.
    if gpu_is_gl {
        disabled |= bindless_features();
        let mut limits = constrained_limits.unwrap_or_default();
        limits.max_storage_textures_per_shader_stage =
            limits.max_storage_textures_per_shader_stage.min(4);
        constrained_limits = Some(limits);
    }
    bevy::render::RenderPlugin {
        render_creation: WgpuSettings {
            disabled_features: Some(disabled),
            constrained_limits,
            ..default()
        }
        .into(),
        ..default()
    }
}

fn main() {
    let working_dir = env::current_dir().unwrap();
    let mut assets_dir = working_dir.clone();
    assets_dir.push("assets");
    println!("assets dir: {}", assets_dir.display());

    // Loaded before the App is built: plugin registration below is decided by
    // config values (`dev_tools`, network), and NetworkPlugin reads the
    // resource during Plugin::build.
    let mut config = plugins::config::ClientConfig::load();
    // The options window's saved graphics rows outrank config.yaml (as the
    // original's options file does), and some are read only while the app is
    // built (the sampler, the sheen materials): lay them over it now, not on
    // the first frame.
    if let Some(options) = plugins::settings::persistence::read_user_settings() {
        plugins::options_video::overlay_saved_rows(&options.video.graphic1, &mut config);
    }

    // Headless net-check mode: drive the full network roundtrip with no window
    // and dump packets, then exit. Reuses the net stack minus rendering/scenes.
    if env::var("NETCHECK").is_ok() {
        netcheck::run_headless(config);
        return;
    }

    // `world_debug` is a renderer-floor benchmark, not a normal game scene.
    // Starting it through SceneManagerPlugin still registers the rest of the
    // client and merely leaves most of it idle, which makes it unable to answer
    // how much frame time terrain itself costs.  Give this one scene its own
    // minimal application instead.
    let scene_override = env::var("SCENE").ok();
    let run_terrain_benchmark = scene_override
        .as_deref()
        .is_some_and(|scene| scene.eq_ignore_ascii_case("world_debug"))
        || (scene_override.is_none() && config.scenes.startup.eq_ignore_ascii_case("world_debug"));
    if run_terrain_benchmark {
        scenes::world_debug_scene::run_terrain_benchmark(config, assets_dir);
        return;
    }

    let dev_tools = config.dev_tools;
    let diagnostics = config.diagnostics_enabled();

    let mut app = App::new();
    // read by the spawn path when it builds rim/sheen material variants
    // (`SroMaterialVariants`)
    let material_defaults = config.graphics.to_material_defaults();
    let present_mode = config.window_settings.present_mode.to_present_mode();
    let desired_maximum_frame_latency = config.window_settings.frame_latency();
    let render = render_plugin(
        &config.graphics,
        config.gpu.as_ref().is_some_and(|gpu| gpu.gl),
    );
    // `graphics.anisotropy`: the default image sampler below and the ground
    // tiles' own sampler, both built once when the renderer starts
    let anisotropy = config.graphics.anisotropy.clamp();
    assets::m::block_splat_material::TILE_ANISOTROPY
        .store(anisotropy, std::sync::atomic::Ordering::Relaxed);
    app.insert_resource(config)
        // Read straight out of Media.pk2 before the app ticks: the gateway
        // connect fires on the first frame, so the asset server would deliver
        // these two files too late (#300).
        .insert_resource(plugins::config::division::DivisionInfo::load_or_fallback())
        .insert_resource(material_defaults)
        .add_plugins((
            SroAssetPlugin,
            DefaultPlugins
                .build()
                // Nothing in the client reads gamepads, yet gilrs would poll
                // every one each frame. (`GltfPlugin` is just as unused but
                // cannot go the same way: with the `bevy_gltf` feature on,
                // `PbrPlugin` registers a glTF extension handler into its
                // resource at build and panics without it.)
                .disable::<bevy::gilrs::GilrsPlugin>()
                .set(AssetPlugin {
                    file_path: (String::from(assets_dir.to_str().unwrap())),
                    // Hot-reload is a development affordance, but the `file_watcher`
                    // bevy feature is on in the workspace manifest, so without this
                    // a shipped release also spawns a recursive filesystem watcher
                    // over the whole assets tree and pays its notify traffic for a
                    // directory the user never edits. Tie it to the build profile
                    // instead of the feature.
                    watch_for_changes_override: Some(cfg!(debug_assertions)),
                    ..default()
                })
                .set(render)
                .set(ImagePlugin {
                    default_sampler: ImageSamplerDescriptor {
                        min_filter: ImageFilterMode::Linear,
                        mag_filter: ImageFilterMode::Linear,
                        mipmap_filter: ImageFilterMode::Linear,
                        address_mode_u: ImageAddressMode::Repeat,
                        address_mode_v: ImageAddressMode::Repeat,
                        address_mode_w: ImageAddressMode::Repeat,
                        // needs all filters Linear (they are); mostly pays
                        // off on grazing-angle ground/water once textures
                        // carry mip chains
                        anisotropy_clamp: anisotropy,
                        ..default()
                    },
                })
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        // Both come from `window_settings` and both default to
                        // what this shipped with (Immediate, and wgpu's own
                        // swapchain depth). They live here rather than in the
                        // window plugin's later apply because bevy reads
                        // `desired_maximum_frame_latency` once, when it creates
                        // the surface -- setting it afterwards is silently
                        // ignored. See `config::window::PresentModeConfig`.
                        present_mode,
                        desired_maximum_frame_latency,
                        ..default()
                    }),
                    ..default()
                }),
        ))
        .init_state::<GameState>()
        .register_type::<GameState>()
        .init_state::<AppMode>()
        .add_plugins((
            // could be refactored to use a PluginGroup
            SroAssetStructsPlugin,
            ConfigPlugin,
            plugins::settings::SettingsPlugin,
            EguiPlugin {
                // multipass mode (the default; single-pass is deprecated):
                // hand-written egui windows MUST run in the
                // EguiPrimaryContextPass schedule — built in plain Update
                // they render but never receive input
                ui_render_order: UiRenderOrder::EguiAboveBevyUi,
                bindless_mode_array_size: None,
                ..default()
            },
            TweeningPlugin,
            scenes::SceneManagerPlugin,
            // (DiagnosticsPlugin: registered with the `diagnostics` tier below)
            plugins::net::plugin::NetworkPlugin,
            (
                plugins::system_window::SystemWindowPlugin,
                plugins::options_window::OptionsWindowPlugin,
                plugins::options_input_tab::OptionsInputTabPlugin,
            ),
            plugins::hud::HudPlugin,
            DevPlugin,
            CursorPlugin,
            plugins::nav::decal::NavMeshDecalPlugin,
        ))
        // Keep the custom cursor: stop egui from overwriting the window's CursorIcon
        .insert_resource(EguiGlobalSettings {
            enable_cursor_icon_updates: false,
            ..default()
        })
        .add_plugins((
            // WaterPlugin,
            plugins::input_watchdog::InputWatchdogPlugin,
            // Inert unless SCREENSHOT=<path> is set (#564).
            plugins::screenshot::ScreenshotPlugin,
            plugins::combat::CombatPlugin,
            plugins::skills::SkillsPlugin,
            plugins::gm::GmPlugin,
            plugins::cos::CosPlugin,
            TextdataPlugin,
            DynamicResourceLoaderPlugin,
            plugins::effects::EffectsPlugin,
            // Nested as one element (the tuple is at 14 of Bevy's 15):
            (
                // Stops bone animation of rigs fully behind the fog — map props
                // animate out to the terrain unload boundary otherwise, and
                // Bevy's `animate_targets` has no visibility filter. Was
                // commented out here, which also left the render-debug
                // `play_animations` switch (and with it `make perf attribute`'s
                // animation row) and the `paused_animations` gauge inert.
                plugins::animation_culling::AnimationCullingPlugin,
                // animation-keyed SFX from the .bsr mod palette (Sound ModData);
                // skips rigs the culling above has paused
                plugins::animation_sounds::AnimationSoundsPlugin,
            ),
            // zone BGM from effectenvsnd.txt, played out of Music.pk2 (#771)
            plugins::zone_ambience::ZoneAmbiencePlugin,
            plugins::zone_bgm::ZoneBgmPlugin,
            // per-texel metallic sheen of EnvMap resources (weapons, armor)
            // Nested as one element: Bevy's `Plugins` tuple impl tops out at
            // 15, and these two custom materials belong together anyway.
            (
                bevy::pbr::MaterialPlugin::<assets::bmt::sheen::SroSheenMaterial>::default(),
                // rim doubles as the always-on character rim and the selection
                // highlight (entity_select.rs), so it registers app-wide here
                bevy::pbr::MaterialPlugin::<assets::bmt::rim::SroRimMaterial>::default(),
            ),
            // UV-scrolling TexAni resources (waterfalls, canal water)
            plugins::texani::TexAniPlugin,
        ))
        // Cap the texture/mesh bytes uploaded to the GPU per frame. Without it,
        // everything that finished loading is uploaded in the frame it lands:
        // a region's worth of textures and meshes arriving together showed up
        // as render-thread stalls of up to ~200 ms in `prepare_assets<GpuImage>`
        // and `allocate_and_free_meshes` (trace of 2026-09-17). The limit is
        // soft — one asset larger than the budget still goes through alone —
        // and the rest waits a frame, so assets appear slightly later instead
        // of the frame freezing. See `UPLOAD_BYTES_PER_FRAME`.
        .insert_resource(RenderAssetBytesPerFrame::new(UPLOAD_BYTES_PER_FRAME))
        // `window_settings.fps_limit` / `unfocused_fps_limit`
        .add_plugins(plugins::frame_pacing::FramePacingPlugin)
        // `graphics.gpu_light_clustering`
        .add_plugins(plugins::light_clustering::LightClusteringPlugin)
        // 3d raycast picking for the character selection previews. Strictly
        // opt-in via markers (`Pickable` on meshes, `MeshPickingCamera` on the
        // camera) so the world scene's terrain is never raycast.
        .add_plugins(MeshPickingPlugin)
        .insert_resource(MeshPickingSettings {
            require_markers: true,
            // Any: the entity-click hit proxies (entity_select) are
            // deliberately Hidden — raycastable volumes that never render.
            // Everything is still marker-opted, so this only widens picking
            // to those proxies (and culled-but-marked meshes, harmless).
            ray_cast_visibility: RayCastVisibility::Any,
        });

    // The measurement tier (config.yaml `diagnostics`, implied by `dev_tools`).
    // Cheap enough to leave on while taking a baseline — that is the point of
    // the split; see the field docs on `ClientConfig::diagnostics`.
    if diagnostics {
        // The one line that makes a failed `make perf` self-diagnosing: the tier
        // is restart-only and config-driven, and its absence otherwise shows up
        // only as a connection refused several minutes later, at the far end of
        // a build-and-launch cycle.
        info!(
            "diagnostics tier on: BRP at http://127.0.0.1:{} (override with BRP_EXTRAS_PORT)",
            std::env::var("BRP_EXTRAS_PORT").unwrap_or_else(|_| "15702".into())
        );
        app.add_plugins(
            (
                // The world_counts/* and cache_counts/* gauges, the frame-pacing
                // metrics and the corner performance panel. Registered here,
                // in the measurement tier, rather than for every player; it
                // must precede BrpExtrasPlugin, which only installs its own
                // FrameTimeDiagnosticsPlugin when none is present yet.
                plugins::diagnostics::DiagnosticsPlugin,
                // `openroad/diagnostics` dumps the whole DiagnosticsStore
                // (incl. the world_counts/* entity categories) over BRP —
                // brp_extras only exposes FPS/frame-time.
                RemotePlugin::default()
                    .with_method_main(
                        "openroad/diagnostics",
                        plugins::diagnostics::brp_all_diagnostics,
                    )
                    // Which build this is. Evidence taken from a running game
                    // is unreadable later unless it names its build, and a
                    // tool outside the process can only guess at it.
                    .with_method_main(
                        "openroad/build_info",
                        plugins::dev::build_info::brp_build_info,
                    ),
                BrpExtrasPlugin,
                // Per-pass render timings. Bevy requests every adapter feature
                // (`WgpuSettingsPriority::Functionality`), so on Vulkan and DX12
                // this records real GPU timestamps and pipeline statistics, not
                // just CPU command-encode times; Metal and WebGPU have no
                // timestamp queries and yield `elapsed_cpu` alone.
                //
                // Not under `profile-tracy`: `RenderPlugin` adds this same
                // plugin itself when `tracing-tracy` is on (bevy_render
                // src/lib.rs), because it is the recorder that uploads GPU
                // zones to the Tracy timeline. Adding it twice is a panic on
                // startup, so the tracy build takes bevy's copy -- which is the
                // one wired to Tracy -- and this tier keeps everything else.
                #[cfg(not(feature = "profile-tracy"))]
                bevy::render::diagnostic::RenderDiagnosticsPlugin,
                // Draw items per render phase — the batching/draw-call number the
                // frame-time levers are all defined in terms of.
                plugins::diagnostics::RenderPhaseDiagnosticsPlugin,
                // Where the frame's wall time goes across the two threads that
                // produce it. The per-pass rows above say what the GPU spends;
                // these say whether anything is actually saturated, which is a
                // different question and currently the load-bearing one.
                plugins::diagnostics::PipelineDiagnosticsPlugin,
                // Free counts of what the render world actually holds: mesh slab
                // allocation (meshes sharing a slab are what makes a batch
                // possible, so slab count is a batching signal) and GPU-resident
                // assets, which is where a texture or mesh leak shows up — the
                // `cache_counts/*` rows only see the CPU-side dedup maps.
                bevy::render::diagnostic::MeshAllocatorDiagnosticPlugin,
                bevy::render::diagnostic::RenderAssetDiagnosticPlugin::<
                    bevy::render::mesh::RenderMesh,
                >::new(" meshes"),
                bevy::render::diagnostic::RenderAssetDiagnosticPlugin::<
                    bevy::render::texture::GpuImage,
                >::new(" images"),
            ),
        );
    }

    // Opt-in dev tooling (config.yaml `dev_tools`): the stock world inspector
    // reflects every entity into egui each frame, which costs double-digit
    // FPS with streamed terrain — `plugins::dev::world_inspector` replaces it
    // with a variant that only does that walk when its "Refresh" button is
    // clicked. EguiPlugin itself stays unconditional (the render-debug panels
    // use it).
    if dev_tools {
        app.add_plugins((
            // `.run_if(dev_windows_visible)` like every other inspector in the
            // tree: these two were the only ones the "dev" corner button could
            // not hide, so switching the dev windows off left the world
            // inspector — the most expensive of them — on screen.
            plugins::dev::world_inspector::ManualWorldInspectorPlugin,
            StateInspectorPlugin::<GameState>::default().run_if(plugins::dev::dev_windows_visible),
        ));
    }

    app.run();
}
