pub mod mode_modal;
pub mod model;
pub mod ui;

use bevy::prelude::*;

/// Self-registration for the party roster window. The HUD registry holds one
/// line per window, so this file owns every system the party page needs.
pub struct PartyWindowPlugin;

impl Plugin for PartyWindowPlugin {
    fn build(&self, app: &mut App) {
        use crate::scenes::SceneState;

        // The mode modal is registered from here rather than from the HUD
        // registry because it is the second half of ONE flow: the roster page's
        // "Set" button is the only thing that opens it, and it writes the
        // `PartyCreateSetup` the page's "Invite" then sends. Splitting them
        // across two registry lines would make that ordering implicit.
        app.add_plugins(mode_modal::PartyModeModalPlugin)
            .init_resource::<model::PartyWindowState>()
            .init_resource::<ui::PartyRowViews>()
            // Shared with the target menu and the quick board; `init_resource`
            // is idempotent, so each surface registers it rather than one
            // depending on another's registration order.
            .init_resource::<crate::plugins::hud::context_menu::ContextMenuOwner>()
            .add_message::<ui::PartyWindowRequest>()
            .add_systems(OnEnter(SceneState::GameWorld), ui::spawn_party_window)
            .add_systems(OnExit(SceneState::GameWorld), ui::cleanup_party_window)
            .add_systems(
                Update,
                (
                    model::toggle_party
                        .run_if(not(crate::plugins::settings::keymap::text_field_focused)),
                    model::send_party_invite_request,
                    ui::apply_party_visibility,
                    ui::pick_party_row_menu,
                    // Chained on purpose: the painters read what the compute
                    // pass writes, and an unordered tuple would paint last
                    // frame's roster on the frame a member joined.
                    (
                        ui::compute_party_views,
                        ui::refresh_party_text,
                        ui::refresh_party_visuals,
                    )
                        .chain()
                        .run_if(ui::party_needs_refresh),
                )
                    .run_if(
                        in_state(SceneState::GameWorld).or_else(in_state(SceneState::UiTesting)),
                    ),
            );
    }
}
