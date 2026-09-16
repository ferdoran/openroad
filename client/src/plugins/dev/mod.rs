use bevy::app::App;
use bevy::pbr::wireframe::{WireframeConfig, WireframePlugin};
use bevy::prelude::*;
use bevy::ui::UiTargetCamera;
// use bevy_prototype_debug_lines::DebugLinesPlugin;
use crate::plugins::config::ClientConfig;
use crate::AppMode;

use crate::plugins::dev::aabb_lines::draw_debug_lines_for_aabb;
use crate::plugins::dev::fps_graph::FpsGraphPlugin;
use crate::plugins::dev::glass_ball::GlassballPlugin;
use crate::plugins::dev::lighting::LightingPlugin;
use crate::plugins::dev::navmesh_lines::{
    draw_debug_lines_for_nav_mesh, draw_nav_cursor_hit, draw_nav_location,
    draw_object_global_edges, draw_object_nav_meshes, dump_nav_snapshot, log_nav_diagnostics,
    warn_when_inside_solid_ground,
};
use crate::plugins::dev::player_config::PlayerConfigPlugin;
use crate::plugins::dev::render_debug::{RenderControlsInspectorPlugin, RenderControlsPlugin};
use crate::plugins::dev::teleport::TeleportPlugin;

pub mod aabb_lines;
pub mod glass_ball;
pub mod lighting;
pub mod navmesh_lines;
pub mod world_inspector;
// pub: `map::objects::cull_fogged_objects` reads `RenderDebugSettings` to
// stand down while the panel's render_objects toggle owns wrapper visibility
mod auto_screenshot;
mod cos_spawner;
mod fps_graph;
mod gm_commands;
mod player_config;
pub mod render_debug;
mod teleport;

#[derive(Resource)]
pub struct DevConfig {
    show_aabb: bool,
}

impl Default for DevConfig {
    fn default() -> Self {
        DevConfig { show_aabb: false }
    }
}

/// Global visibility of the egui dev windows (inspectors, lighting, player
/// config, particle pickers), flipped by the "dev" corner button. Every egui
/// window system is gated on [`dev_windows_visible`].
#[derive(Resource)]
pub struct DevWindowsVisible(pub bool);

impl Default for DevWindowsVisible {
    fn default() -> Self {
        DevWindowsVisible(true)
    }
}

/// Run condition for every egui dev-window system.
pub fn dev_windows_visible(visible: Res<DevWindowsVisible>) -> bool {
    visible.0
}

/// The letters dev tooling claims while it is enabled, and the vanilla
/// shortcut each one would steal (`docs/re/ui/scene-game-hud-composition.md`
/// §6 — the original binds these HUD toggles):
///
/// | key | dev use | vanilla shortcut |
/// |---|---|---|
/// | `Q` | wireframe toggle | Quest |
/// | `E` | AABB lines / light adjust | Party Match |
/// | `T` | light adjust | Auto Potion |
/// | `Y` | *(free today)* | Alchemy |
/// | `U` | nav snapshot / light adjust | Community |
/// | `I` | light adjust | Inventory (bound already) |
///
/// The `I` row is the reason this is not theoretical: the inventory toggle
/// ships today and `dev/lighting.rs` reads the same key in `Update`.
pub const DEV_HOTKEY_COLLISIONS: [KeyCode; 5] = [
    KeyCode::KeyQ,
    KeyCode::KeyE,
    KeyCode::KeyT,
    KeyCode::KeyU,
    KeyCode::KeyI,
];

/// Whether the dev tooling may be registered at all — the key-driven systems
/// **and** the egui windows and the corner button that toggles them.
///
/// It is opt-in via `config.yaml`'s `dev_tools` (the same switch the egui
/// inspectors use, `environment/mod.rs:550-563`), because the hotkeys read
/// bare letters in `Update` with no gate: before this, a plain play session
/// had `Q`/`E`/`T`/`U`/`I` silently claimed by wireframe, AABB lines, light
/// tweaks and the nav snapshot. Gating on `AppMode::DebugMode` would not
/// help — `AppMode` **defaults to `DebugMode`** (`main.rs:45-49`), so every
/// session starts in it.
///
/// The window half used to be ungated, which was the same bug one layer up:
/// `DevWindowsVisible` defaults to `true`, so the GM-command, COS-spawner,
/// teleport, player-config and render-debug panels — and the "dev" button
/// itself — rendered over a normal play session even with `dev_tools: false`.
fn dev_tools_enabled(config: Option<&ClientConfig>) -> bool {
    config.is_some_and(|config| config.dev_tools)
}

pub struct DevPlugin;

impl Plugin for DevPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DevConfig>()
            .init_resource::<DevWindowsVisible>()
            // NOT in the gated block below: `RenderDebugSettings` is read as a
            // plain `Res<_>` by three shipping culling systems, so gating it on
            // `dev_tools` panicked the client in the shipped default config.
            // See `RenderControlsPlugin` for the full rationale; only the
            // inspector half is dev-only.
            .add_plugins(RenderControlsPlugin)
            // .add_plugins(DebugLinesPlugin::with_depth_test(true))
            //
            // `WireframePlugin` and the auto-screenshot harness carry no UI and
            // no hotkey of their own — the `Q` toggle that drives the wireframe
            // is in the gated block below — so they stay unconditional.
            .add_plugins((
                WireframePlugin::default(),
                auto_screenshot::AutoScreenshotPlugin,
            ))
            // Always on: the mode switch (Tab — the way *into* debug mode),
            // which claims no letter a vanilla HUD toggle wants. The dev-window
            // button moved into the gated block: it is chrome for tooling that
            // no longer exists when `dev_tools` is off.
            .add_systems(Update, switch_mode);

        // Everything key-driven or egui-driven: opt-in, so a play session keeps
        // its letters and its screen (see `dev_tools_enabled` and
        // DEV_HOTKEY_COLLISIONS).
        if dev_tools_enabled(app.world().get_resource::<ClientConfig>()) {
            app.add_plugins((
                RenderControlsInspectorPlugin,
                PlayerConfigPlugin,
                TeleportPlugin,
                cos_spawner::CosSpawnerPlugin,
                gm_commands::GmCommandsPlugin,
                FpsGraphPlugin,
            ));
            app.add_systems(Update, (spawn_dev_windows_button, on_dev_windows_button));
            // Both of these are bare-letter hotkey plugins: lighting adjusts
            // on E/T/U/I/P, the glass ball spawns on its own key.
            app.add_plugins((LightingPlugin, GlassballPlugin));
            app.add_systems(
                Update,
                (
                    toggle_wireframe,
                    draw_debug_lines_for_aabb,
                    draw_debug_lines_for_nav_mesh,
                    draw_object_nav_meshes,
                    draw_object_global_edges,
                    draw_nav_location,
                    log_nav_diagnostics,
                    warn_when_inside_solid_ground,
                    dump_nav_snapshot,
                    draw_nav_cursor_hit,
                ),
            );
        }
    }
}

/// The "dev" corner button toggling [`DevWindowsVisible`].
#[derive(Component)]
struct DevWindowsButton;
#[derive(Component)]
struct DevWindowsButtonLabel;

/// Spawn the toggle button once the persistent 2d UI camera exists (it is
/// created in `OnEnter(GameState::Loading)`, so polling instead of a startup
/// hook).
fn spawn_dev_windows_button(
    mut commands: Commands,
    existing: Query<(), With<DevWindowsButton>>,
    cam_query: Query<Entity, With<Camera2d>>,
) {
    if !existing.is_empty() {
        return;
    }
    let Some(camera) = cam_query.iter().next() else {
        return;
    };
    commands
        .spawn((
            DevWindowsButton,
            Name::from("Dev Windows Toggle"),
            Interaction::default(),
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(8.0),
                right: Val::Px(8.0),
                width: Val::Px(36.0),
                height: Val::Px(22.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                border_radius: BorderRadius::all(Val::Px(4.0)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
            // above the HUD (50), below the loading overlay (200)
            GlobalZIndex(150),
            UiTargetCamera(camera),
        ))
        .with_children(|parent| {
            parent.spawn((
                DevWindowsButtonLabel,
                Text::new("dev"),
                TextFont {
                    font_size: FontSize::Px(12.0),
                    ..default()
                },
                TextColor(Color::WHITE),
                Pickable::IGNORE,
            ));
        });
}

/// Flip the visibility on click; dim the label while the windows are hidden.
fn on_dev_windows_button(
    interactions: Query<&Interaction, (Changed<Interaction>, With<DevWindowsButton>)>,
    mut visible: ResMut<DevWindowsVisible>,
    mut labels: Query<&mut TextColor, With<DevWindowsButtonLabel>>,
) {
    for interaction in interactions.iter() {
        if *interaction == Interaction::Pressed {
            visible.0 = !visible.0;
            for mut color in labels.iter_mut() {
                color.0 = if visible.0 {
                    Color::WHITE
                } else {
                    Color::srgb(0.5, 0.5, 0.5)
                };
            }
        }
    }
}

pub(crate) fn toggle_wireframe(
    keys: Res<ButtonInput<KeyCode>>,
    mut wireframe_config: ResMut<WireframeConfig>,
) {
    if keys.just_released(KeyCode::KeyQ) {
        wireframe_config.global = !wireframe_config.global;
        info!("Toggled Wireframe: {}", wireframe_config.global);
    }
}

fn switch_mode(
    app_mode: Res<State<AppMode>>,
    mut next_app_mode: ResMut<NextState<AppMode>>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    if keys.just_pressed(KeyCode::Tab) {
        if *app_mode.get() == AppMode::PlayMode {
            next_app_mode.set(AppMode::DebugMode)
        } else {
            next_app_mode.set(AppMode::PlayMode)
        };
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// The key-driven dev systems must be opt-in: they read bare letters in
    /// `Update`, and five of them are vanilla HUD shortcuts
    /// (`docs/re/ui/scene-game-hud-composition.md` §6). Gating on
    /// `AppMode::DebugMode` would be a no-op — `AppMode` defaults to
    /// `DebugMode` — so the gate is `config.yaml`'s `dev_tools`. The egui
    /// windows and the "dev" button ride the same gate: `DevWindowsVisible`
    /// defaults to `true`, so an ungated window shows in a play session.
    #[test]
    fn dev_tools_are_off_without_the_dev_tools_config() {
        assert!(!dev_tools_enabled(None));
    }

    /// The collision list must keep naming the keys the HUD wants back, so a
    /// future dev binding on one of them is a conscious choice.
    #[test]
    fn the_collision_list_names_the_vanilla_shortcut_letters() {
        for key in [
            KeyCode::KeyQ,
            KeyCode::KeyE,
            KeyCode::KeyT,
            KeyCode::KeyU,
            KeyCode::KeyI,
        ] {
            assert!(DEV_HOTKEY_COLLISIONS.contains(&key), "{key:?}");
        }
        // Tab (the mode switch) stays ungated and must not be in the list
        assert!(!DEV_HOTKEY_COLLISIONS.contains(&KeyCode::Tab));
    }
}
