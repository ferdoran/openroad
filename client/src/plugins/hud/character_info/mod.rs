pub mod model;
pub mod ui;

use bevy::prelude::*;

/// Self-registration for the character-info window (#558). The HUD registry holds one line per
/// window, so two windows landing in the same lap no longer collide on it.
pub struct CharacterInfoPlugin;

impl Plugin for CharacterInfoPlugin {
    fn build(&self, app: &mut App) {
        use crate::scenes::SceneState;

        app.init_resource::<model::CharacterInfoState>()
            .init_resource::<model::PlayerStats>()
            .add_systems(
                OnEnter(SceneState::GameWorld),
                ui::spawn_character_info_window,
            )
            .add_systems(OnExit(SceneState::GameWorld), ui::cleanup_character_info)
            .add_systems(
                Update,
                (
                    model::toggle_character_info
                        .run_if(not(crate::plugins::settings::keymap::text_field_focused)),
                    model::on_stats_update,
                    model::on_points_update,
                    model::on_experience_stat_points,
                    model::seed_stat_points,
                    model::on_stat_spend_ack,
                    ui::apply_character_info_visibility,
                    ui::refresh_character_info.run_if(ui::character_info_needs_refresh),
                )
                    .run_if(
                        in_state(SceneState::GameWorld).or_else(in_state(SceneState::UiTesting)),
                    ),
            );
    }
}
