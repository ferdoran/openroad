use crate::plugins::cursor::interactions::GameCursorTarget;
use crate::plugins::player::Player;
use bevy::camera::primitives::Aabb;
use bevy::prelude::*;

pub fn check_player_cursor_intersection(
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    mut player_query: Query<
        (&Player, &Aabb, &GameCursorTarget),
        (With<Player>, With<GameCursorTarget>),
    >,
) {
    // ensure mouse click is done with combination of shift key
    if !keys.just_released(KeyCode::ShiftLeft) || !buttons.just_released(MouseButton::Left) {
        return;
    }

    if let Ok((_player, _aabb, _cursor_target)) = player_query.single_mut() {
        println!("Selected player!");
    }
}
