//! Auto-potion configuration window (`GDR_AUTO_POTION`), read-only.
//!
//! See `model.rs` for the wire state it renders and `ui.rs` for the layout.

pub mod model;
pub mod ui;

use bevy::prelude::*;

/// Self-registration for the auto-potion window (#558). The HUD registry holds one line per
/// window, so two windows landing in the same lap no longer collide on it.
pub struct AutoPotionPlugin;

impl Plugin for AutoPotionPlugin {
    fn build(&self, app: &mut App) {
        use crate::scenes::SceneState;

        app.init_resource::<model::AutoPotionState>()
            .init_resource::<model::AutoPotionSettings>()
            .add_systems(OnEnter(SceneState::GameWorld), ui::spawn_autopotion_window)
            .add_systems(OnExit(SceneState::GameWorld), ui::cleanup_autopotion_window)
            // UiTesting so the offline preview scene exercises the same
            // seeding/refresh path with mock settings (both in-tree 0x3013
            // captures are all zeros).
            .add_systems(
                Update,
                (
                    model::seed_autopotion_from_character_info,
                    model::toggle_autopotion_window
                        .run_if(not(crate::plugins::settings::keymap::text_field_focused)),
                    ui::apply_autopotion_visibility,
                    ui::refresh_autopotion.run_if(ui::autopotion_needs_refresh),
                )
                    .run_if(
                        in_state(SceneState::GameWorld).or_else(in_state(SceneState::UiTesting)),
                    ),
            );
    }
}
