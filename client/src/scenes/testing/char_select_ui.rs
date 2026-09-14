//! Idea: an offline preview of the character-selection selection UI (info
//! box and Start/Delete/Cancel row) with dummy lobby data, so the pixel
//! layout can be iterated without logging into a live server. Run with
//! `SCENE=ui_testing`; combine with `OPENROAD_SCREENSHOT` for headless
//! capture.

use bevy::prelude::*;

use packets::agent::prelude::LobbyCharacter;

use crate::assets::FontAssets;
use crate::plugins::textdata::{ClientLevelData, ClientUiStrings};
use crate::scenes::intro_v2::assets::IntroV2Assets;
use crate::scenes::intro_v2::{character_select, chrome, fade};
use crate::scenes::SceneState;

pub struct CharSelectUiPreviewPlugin;

impl Plugin for CharSelectUiPreviewPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(SceneState::UiTesting), spawn_preview);
    }
}

fn dummy_character() -> LobbyCharacter {
    LobbyCharacter {
        ref_obj_id: 1907,
        name: "Testcharacter".to_string(),
        scale: 34,
        level: 42,
        exp_offset: 123_456,
        str: 87,
        int: 65,
        stat_points: 12,
        hp: 5230,
        mp: 4180,
        is_deleting: false,
        deletion_time: None,
        guild_member_class: 0,
        is_guild_rename_required: false,
        current_guild_name: None,
        academy_member_class: 0,
        char_items: vec![],
        avatar_items: vec![],
    }
}

fn spawn_preview(
    assets: Res<IntroV2Assets>,
    fonts: Res<FontAssets>,
    level_data: Res<ClientLevelData>,
    ui_strings: Res<ClientUiStrings>,
    cam_query: Query<Entity, With<Camera2d>>,
    mut commands: Commands,
) {
    let Some(camera) = cam_query.iter().next() else {
        warn!("no 2d camera found");
        return;
    };

    // the intro chrome is included so occlusion/z-order problems between it
    // and the selection UI reproduce here too
    commands
        .spawn_scene(fade::fade_screen())
        .insert(UiTargetCamera(camera));
    commands
        .spawn_scene(chrome::header(&assets))
        .insert(UiTargetCamera(camera));
    commands
        .spawn_scene(chrome::footer(&assets))
        .insert(UiTargetCamera(camera));
    commands
        .spawn_scene(chrome::info_text())
        .insert(UiTargetCamera(camera));
    // `PREVIEW_DELETING=1` previews the deletion-pending variant, where the
    // original swaps Start/Delete for Restore + Cancel (#202). That state is
    // otherwise only reachable by actually deleting a character against a live
    // server, so the row would never get eyeballed offline.
    let deleting = std::env::var("PREVIEW_DELETING").is_ok();
    let mut character = dummy_character();
    character.is_deleting = deleting;

    commands
        .spawn_scene(character_select::info_box(
            &character,
            &level_data,
            &assets,
            &fonts,
        ))
        .insert(UiTargetCamera(camera));
    if deleting {
        commands
            .spawn_scene(character_select::deleting_control_buttons(
                &assets,
                &fonts,
                &ui_strings,
            ))
            .insert(UiTargetCamera(camera));
    } else {
        commands
            .spawn_scene(character_select::selected_control_buttons(&assets, &fonts))
            .insert(UiTargetCamera(camera));
    }
}
