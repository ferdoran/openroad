//! Vanilla-style chat window: tabbed viewer over a shared message ring
//! buffer, chat networking (0x7025/0xB025/0x3026/0x302D) and the
//! Enter-toggled input. Split by concern: `model` (data), `net` (packet
//! consumers), `ui` (window/scrolling), `input` (Enter toggle, parsing,
//! sending). System wiring lives in the parent `HudPlugin`.

pub mod input;
pub mod model;
pub mod net;
pub mod ui;

use bevy::prelude::*;

/// Self-registration for the chat window (#558). The HUD registry holds one line per
/// window, so two windows landing in the same lap no longer collide on it.
pub struct ChatPlugin;

impl Plugin for ChatPlugin {
    fn build(&self, app: &mut App) {
        use crate::scenes::SceneState;

        app.init_resource::<crate::plugins::config::chat::ChatColors>()
            .add_systems(
                PreUpdate,
                apply_chat_colors.run_if(crate::plugins::settings::live::config_changed),
            )
            .init_resource::<model::ChatHistory>()
            .init_resource::<model::ChatState>()
            .add_systems(OnEnter(SceneState::GameWorld), ui::spawn_chat_window)
            .add_systems(OnExit(SceneState::GameWorld), ui::cleanup_chat_window)
            .add_systems(
                Update,
                (
                    net::on_chat_update,
                    net::on_chat_response,
                    net::on_chat_restriction,
                    net::on_notice_update,
                    // after the Esc-menu toggle so Esc-while-typing only
                    // closes the chat input (the toggle checks `input_open`)
                    input::handle_chat_enter
                        .after(crate::plugins::system_window::toggle_system_window),
                    input::refresh_whisper_panel,
                    ui::update_chat_tab_visuals,
                    ui::update_chat_mode_popup,
                    ui::update_chat_penalty,
                    ui::apply_chat_window_mode,
                    ui::fade_chat_window,
                    (
                        ui::refresh_chat_list.run_if(ui::chat_needs_refresh),
                        ui::apply_chat_scroll,
                        ui::update_chat_scroll_thumb,
                        ui::update_chat_scroll_arrows,
                    )
                        .chain(),
                )
                    .run_if(
                        in_state(SceneState::GameWorld).or_else(in_state(SceneState::UiTesting)),
                    ),
            );
    }
}

/// Re-resolves the per-channel chat palette from `config.yaml`.
///
/// The apply-system half of the live-settings mechanism
/// ([`crate::plugins::settings::live`]): `resource_changed` is also true on the
/// frame `ClientConfig` is inserted, so this both seeds the palette at boot and
/// follows a later edit — where the previous `Plugin::build` derivation could
/// only ever do the first.
pub fn apply_chat_colors(
    config: Res<crate::plugins::config::ClientConfig>,
    mut colors: ResMut<crate::plugins::config::chat::ChatColors>,
) {
    *colors = config.chat.colors.resolved();
}
