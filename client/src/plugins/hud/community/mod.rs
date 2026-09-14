pub mod guild;
pub mod letter;
pub mod letter_sub;
pub mod model;
pub mod ui;

use bevy::prelude::*;

/// Self-registration for the community window (#558). The HUD registry holds one line per
/// window, so two windows landing in the same lap no longer collide on it.
pub struct CommunityPlugin;

impl Plugin for CommunityPlugin {
    fn build(&self, app: &mut App) {
        use crate::scenes::SceneState;

        app.init_resource::<model::CommunityState>()
            .init_resource::<guild::GuildNoticeOpen>()
            .add_systems(OnEnter(SceneState::GameWorld), ui::spawn_community_window)
            .add_systems(OnExit(SceneState::GameWorld), ui::cleanup_community_window)
            .add_systems(
                Update,
                letter_sub::apply_letter_sub_window.run_if(
                    in_state(SceneState::GameWorld).or_else(in_state(SceneState::UiTesting)),
                ),
            )
            .add_systems(
                Update,
                (
                    guild::update_guild_info,
                    guild::update_guild_roster,
                    guild::update_guild_notice,
                )
                    .run_if(
                        in_state(SceneState::GameWorld).or_else(in_state(SceneState::UiTesting)),
                    ),
            )
            .add_systems(
                Update,
                ui::apply_community_visibility.run_if(
                    in_state(SceneState::GameWorld).or_else(in_state(SceneState::UiTesting)),
                ),
            );
    }
}
