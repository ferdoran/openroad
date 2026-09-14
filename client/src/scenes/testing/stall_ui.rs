//! Idea: an offline preview of the player stall window. Run with
//! `SCENE=ui_testing`. The window's own Update systems already run in
//! `SceneState::UiTesting`, so this only opens it and fills the model with
//! placeholder rows — there is no wire in #779, and the two strings it seeds
//! are the original's own defaults (`UIIT_STT_STALL_DEFAULT_TITLE` /
//! `_OWNERMSG`, with their `[%s]` owner slot filled in) rather than invented
//! copy.

use bevy::prelude::*;

use crate::plugins::hud::stall::model::{StallRow, StallState, StallTradingState};
use crate::scenes::SceneState;

pub struct StallUiPreviewPlugin;

impl Plugin for StallUiPreviewPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(SceneState::UiTesting), open_stall_window);
    }
}

fn open_stall_window(mut state: ResMut<StallState>) {
    state.open = true;
    state.title = "Tobsnti's stall.".to_string();
    state.greeting = "Welcome to Tobsnti's stall.".to_string();
    state.trading = StallTradingState::Open;
    // Three of the ten cells filled, so the preview shows both an occupied and
    // an empty plate in both columns.
    for (index, (name, quantity, price)) in [
        (0usize, ("Steppe Blade", 1u16, 12_500u64)),
        (1, ("HP Potion (S)", 20, 400)),
        (4, ("Devil's Spirit", 3, 98_000)),
    ] {
        state.slots[index] = Some(StallRow {
            name: name.to_string(),
            quantity,
            price,
        });
    }
}
