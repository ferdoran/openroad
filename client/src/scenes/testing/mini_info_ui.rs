//! Idea: an offline preview of the in-game player mini-info HUD with mock
//! vitals, so the pixel layout, bar fills and the portrait render-to-texture
//! can be iterated without logging into a live server. Run with
//! `SCENE=ui_testing`; combine with `OPENROAD_SCREENSHOT` for headless
//! capture. The HUD's own Update systems (refresh, portrait rig/tagging) are
//! already gated to also run in `SceneState::UiTesting`, so this only spawns
//! the panel, a character to portray, and the fake numbers.

use bevy::prelude::*;

use crate::plugins::hud::player_mini_info::{spawn_mini_info, PlayerVitals};
use crate::plugins::player::{spawn_player_character, PlayerConfig};
use crate::scenes::SceneState;

pub struct MiniInfoUiPreviewPlugin;

impl Plugin for MiniInfoUiPreviewPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            OnEnter(SceneState::UiTesting),
            (spawn_mini_info, spawn_mock_data).chain(),
        );
    }
}

fn spawn_mock_data(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    config: Res<PlayerConfig>,
    mut vitals: ResMut<PlayerVitals>,
) {
    *vitals = PlayerVitals {
        name: "Testchar".to_string(),
        level: 42,
        hp: 3450,
        mp: 2210,
        max_hp: Some(5230),
        max_mp: Some(4180),
        berserk_pips: 3,
        hwan_level: 0,
    };
    // a real character for the portrait camera to shoot
    spawn_player_character(&mut commands, &asset_server, &config, Transform::IDENTITY);
}
