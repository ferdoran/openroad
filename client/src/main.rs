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

fn main() {
    let working_dir = env::current_dir().unwrap();
    let mut assets_dir = working_dir.clone();
    assets_dir.push("assets");
    println!("assets dir: {}", assets_dir.display());

    // Loaded before the App is built: plugin registration below is decided by
    // config values (`dev_tools`, network), and NetworkPlugin reads the
    // resource during Plugin::build.
    let config = plugins::config::ClientConfig::load();

    // Headless net-check mode: drive the full network roundtrip with no window
    // and dump packets, then exit. Reuses the net stack minus rendering/scenes.
    if env::var("NETCHECK").is_ok() {
        netcheck::run_headless(config);
        return;
    }

    let dev_tools = config.dev_tools;
    let diagnostics = config.diagnostics_enabled();

    let mut app = App::new();
    // must exist before SroAssetStructsPlugin registers BmtLoader (FromWorld)
    let material_defaults = config.graphics.to_material_defaults();
    let present_mode = config.window_settings.present_mode.to_present_mode();
    let desired_maximum_frame_latency = config.window_settings.frame_latency();
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
                        anisotropy_clamp: 4,
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
            plugins::diagnostics::DiagnosticsPlugin,
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
            plugins::animation_culling::AnimationCullingPlugin,
            // animation-keyed SFX from the .bsr mod palette (Sound ModData)
            plugins::animation_sounds::AnimationSoundsPlugin,
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
                // `openroad/diagnostics` dumps the whole DiagnosticsStore
                // (incl. the world_counts/* entity categories) over BRP —
                // brp_extras only exposes FPS/frame-time.
                RemotePlugin::default().with_method_main(
                    "openroad/diagnostics",
                    plugins::diagnostics::brp_all_diagnostics,
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
