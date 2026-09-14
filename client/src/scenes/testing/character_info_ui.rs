//! Idea: an offline preview of the character info window (C) with a mock
//! stat sheet — a mid-level character with spendable stat points so the +
//! buttons show, believable attack/defense ranges and partial HP/MP fills.
//! Run with `SCENE=ui_testing`. The window's own Update systems already run
//! in `SceneState::UiTesting`, so this only spawns the window, seeds the
//! mock `PlayerStats`/`PlayerVitals`/`PlayerProgress` resources and opens it.

use bevy::prelude::*;

use packets::agent::prelude::CharacterStatsUpdate;

use crate::plugins::hud::character_info::model::{CharacterInfoState, PlayerStats};
use crate::plugins::hud::character_info::ui::spawn_character_info_window;
use crate::plugins::hud::player_mini_info::PlayerVitals;
use crate::plugins::hud::underbar::model::PlayerProgress;
use crate::scenes::SceneState;

pub struct CharacterInfoUiPreviewPlugin;

impl Plugin for CharacterInfoUiPreviewPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            OnEnter(SceneState::UiTesting),
            (seed_mock_stats, spawn_character_info_window).chain(),
        );
    }
}

fn seed_mock_stats(
    mut stats: ResMut<PlayerStats>,
    mut vitals: ResMut<PlayerVitals>,
    mut progress: ResMut<PlayerProgress>,
    mut state: ResMut<CharacterInfoState>,
) {
    stats.sheet = Some(CharacterStatsUpdate {
        phys_attack_min: 312,
        phys_attack_max: 388,
        mag_attack_min: 501,
        mag_attack_max: 622,
        phys_defense: 148,
        mag_defense: 201,
        hit_rate: 96,
        parry_rate: 87,
        max_hp: 2210,
        max_mp: 2890,
        strength: 41,
        intelligence: 58,
    });
    stats.stat_points = 9;

    vitals.name = "Testcharacter".into();
    vitals.level = 32;
    vitals.hp = 1656;
    vitals.mp = 2890;
    vitals.max_hp = Some(2210);
    vitals.max_mp = Some(2890);

    progress.level = 32;
    progress.exp_offset = 1_284_550;

    state.open = true;
}
