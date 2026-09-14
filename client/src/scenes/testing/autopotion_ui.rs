//! Idea: an offline preview of the auto-potion window with mock non-zero
//! settings. Both in-tree 0x3013 captures carry all-zero auto-potion fields, so
//! a live session shows an empty panel — this scene is the only way to see the
//! sliders, the `%` readouts and the ticked checkboxes. Run with
//! `SCENE=ui_testing`. The window's own Update systems already run in
//! `SceneState::UiTesting`, so this only seeds `AutoPotionSettings`, spawns the
//! window and opens it.

use bevy::prelude::*;

use crate::plugins::hud::autopotion::model::{AutoPotionSettings, AutoPotionState};
use crate::plugins::hud::autopotion::ui::spawn_autopotion_window;
use crate::scenes::SceneState;

pub struct AutoPotionUiPreviewPlugin;

impl Plugin for AutoPotionUiPreviewPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            OnEnter(SceneState::UiTesting),
            (seed_mock_settings, spawn_autopotion_window).chain(),
        );
    }
}

/// Four distinct rows: HP high, MP lower, abnormal-status on, and a non-zero
/// delay — so every readout, both slider thumbs and all four checkboxes show a
/// different state. Ours; the wire values a real server sends are whatever the
/// player last saved.
fn seed_mock_settings(
    mut settings: ResMut<AutoPotionSettings>,
    mut state: ResMut<AutoPotionState>,
) {
    *settings = AutoPotionSettings {
        hp_percent: 70,
        mp_percent: 35,
        universal_percent: 50,
        delay: 3,
    };
    state.open = true;
}
