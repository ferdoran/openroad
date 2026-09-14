pub mod enchant;
pub mod grant;
pub mod model;
pub mod ui;

use bevy::prelude::*;

/// Self-registration for the alchemy box (#558). The HUD registry holds one line per
/// window, so two windows landing in the same lap no longer collide on it.
pub struct AlchemyPlugin;

impl Plugin for AlchemyPlugin {
    fn build(&self, app: &mut App) {
        use crate::scenes::SceneState;

        app.add_plugins(enchant::EnchantPlugin)
            .init_resource::<model::AlchemyState>()
            .init_resource::<grant::GrantState>()
            .add_systems(
                OnExit(SceneState::GameWorld),
                (ui::cleanup_alchemy, grant::cleanup_grant),
            )
            .add_systems(
                PostUpdate,
                (ui::despawn_closing_alchemy, grant::despawn_closing_grant),
            )
            .add_systems(
                Update,
                (
                    model::toggle_alchemy_window
                        .run_if(not(crate::plugins::settings::keymap::text_field_focused)),
                    ui::sync_alchemy_window,
                    ui::place_drop_on_alchemy,
                    grant::sync_grant_window,
                    grant::place_drop_on_grant,
                )
                    .run_if(super::hud_scenes),
            );
    }
}
