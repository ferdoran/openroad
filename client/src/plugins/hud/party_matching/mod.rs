pub mod dialogs;
pub mod model;
pub mod ui;

use bevy::prelude::*;

/// Self-registration for the party-matching board and its four dialogs.
///
/// The dialogs are registered from here rather than as their own registry line
/// for the same reason the mode modal is: they are not independent windows.
/// Three of the four are opened by a board button and the fourth is opened by a
/// packet the board's own state machine owns, and all four write back into
/// `PartyMatchState`.
pub struct PartyMatchingPlugin;

impl Plugin for PartyMatchingPlugin {
    fn build(&self, app: &mut App) {
        use crate::scenes::SceneState;

        app.init_resource::<model::PartyMatchState>()
            .add_message::<model::ReloadMatchList>()
            .add_systems(OnEnter(SceneState::GameWorld), ui::spawn_match_window)
            .add_systems(OnExit(SceneState::GameWorld), ui::cleanup_match_window)
            .add_systems(
                Update,
                (
                    // Inbound first, so a page that arrived this frame is the
                    // one the board paints.
                    (
                        model::on_match_list,
                        model::on_join_request,
                        model::on_join_ack,
                        model::on_form_acks,
                        model::on_delete_ack,
                        // After the roster has folded 0x3065, so the wait
                        // dialog closes on the push that actually resolved it.
                        model::close_join_progress_on_party,
                        model::sync_own_party_master,
                    ),
                    (
                        model::toggle_party_match
                            .run_if(not(crate::plugins::settings::keymap::text_field_focused)),
                        ui::open_from_party_window,
                        ui::request_page_on_open,
                        ui::tick_join_progress,
                        ui::reload_match_list,
                    ),
                    (
                        ui::apply_match_visibility,
                        ui::refresh_match_board.run_if(ui::match_needs_refresh),
                        dialogs::sync_match_dialogs,
                        // After the builder, so a bar spawned this frame is
                        // painted this frame rather than one frame late.
                        dialogs::refresh_join_progress,
                    )
                        .chain(),
                )
                    .chain()
                    .run_if(
                        in_state(SceneState::GameWorld).or_else(in_state(SceneState::UiTesting)),
                    ),
            );
    }
}
