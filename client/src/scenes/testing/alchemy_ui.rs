//! Idea: an offline preview of the alchemy box's Attribute Grant page. Run
//! with `SCENE=ui_testing`. The window's own Update systems already run in
//! `SceneState::UiTesting`, so this only opens the box — `sync_alchemy_window`
//! builds it on the next frame. The slots start empty; the preview scene's
//! mock inventory (`inventory_ui`) is what a drag into them comes from, which
//! is the interaction under test.

use bevy::prelude::*;

use crate::plugins::hud::alchemy::grant::GrantState;
use crate::plugins::hud::alchemy::model::AlchemyState;
use crate::scenes::SceneState;

pub struct AlchemyUiPreviewPlugin;

impl Plugin for AlchemyUiPreviewPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            OnEnter(SceneState::UiTesting),
            (open_alchemy_box, open_grant_dialog),
        );
    }
}

fn open_alchemy_box(mut state: ResMut<AlchemyState>) {
    state.open = true;
}

/// The avatar magic-option grant dialog (#660) previews the same way. Its
/// **in-game** entry point is deliberately not invented here: the original's
/// trigger for `GDR_GRANT_MAGIC_ATTRIBUTE` is not in the resinfo data, and
/// `docs/re/ui/alchemy-window.md` §9 leaves it open.
fn open_grant_dialog(mut state: ResMut<GrantState>) {
    state.open = true;
}
