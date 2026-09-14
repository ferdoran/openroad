//! The S-key skill window: mastery boards with learnable skill cells,
//! SP accounting, drag-to-underbar, and (right-click) withdrawal.

pub mod model;
pub mod ui;
pub mod withdrawal;

use bevy::prelude::*;

/// Self-registration for the skill window (#558). The HUD registry holds one line per
/// window, so two windows landing in the same lap no longer collide on it.
pub struct SkillWindowPlugin;

impl Plugin for SkillWindowPlugin {
    fn build(&self, app: &mut App) {
        use crate::scenes::SceneState;

        app.init_resource::<model::SkillWindowState>()
            .init_resource::<withdrawal::SkillWithdrawalState>()
            .init_resource::<model::SkillDrag>()
            .init_resource::<model::SkillTreeRace>()
            .add_systems(OnEnter(SceneState::GameWorld), ui::spawn_skill_window)
            .add_systems(OnExit(SceneState::GameWorld), ui::cleanup_skill_window)
            // the skill window also lives in the offline Skills test scene
            // (the scene spawns the underbar/target window itself)
            .add_systems(OnEnter(SceneState::Skills), ui::spawn_skill_window)
            .add_systems(OnExit(SceneState::Skills), ui::cleanup_skill_window)
            .add_systems(
                Update,
                (
                    model::toggle_skill_window
                        .run_if(not(crate::plugins::settings::keymap::text_field_focused)),
                    model::sync_quickslots_with_book,
                    ui::apply_skill_window_visibility,
                    ui::apply_practice_prompt,
                    ui::practice_prompt_keys,
                    ui::cancel_skill_drag,
                    ui::update_skill_tooltip,
                    ui::update_skill_scroll_thumb,
                    ui::refresh_skill_window.run_if(model::skill_window_needs_refresh),
                    withdrawal::apply_skill_withdrawal,
                    withdrawal::close_withdrawal_with_skill_window,
                )
                    .run_if(super::hud_scenes),
            );
    }
}
