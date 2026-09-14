use bevy::image::TRANSPARENT_IMAGE_HANDLE;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui_widgets::Activate;

use packets::gateway::Shard;

use crate::assets::FontAssets;
use crate::plugins::net::gateway::shard_list::ShardList;
use crate::plugins::settings::options::GameOptions;
use crate::plugins::ui_v2::style::{ButtonSound, ImageButtonStyle, TargetColor};
use crate::plugins::ui_v2::widgets::{image_button, label};

use super::assets::IntroV2Assets;
use super::login_form::{main_button_style, ShardNameText};
use super::IntroV2State;

/// Root marker of the server selection screen.
#[derive(Component, Default, Clone)]
pub struct ServerSelectRoot;

/// Marker on the window image node; the dynamic shard rows are spawned as
/// its children.
#[derive(Component, Default, Clone)]
pub struct ServerWindow;

/// One row per shard, carrying the shard id.
#[derive(Component, Default, Clone)]
pub struct ShardRow(pub u16);

/// The currently highlighted row (not yet committed).
#[derive(Component, Default, Clone)]
pub struct SelectedRow;

#[derive(Component, Default, Clone)]
pub struct SelectButton;

#[derive(Component, Default, Clone)]
pub struct CancelButton;

/// The shard id committed via the Select button; read by the Connect flow.
#[derive(Resource, Default)]
pub struct SelectedShardV2(pub Option<u16>);

fn slider_style(
    normal: &Handle<Image>,
    hover: &Handle<Image>,
    press: &Handle<Image>,
) -> ImageButtonStyle {
    ImageButtonStyle {
        normal: normal.clone(),
        hover: hover.clone(),
        press: press.clone(),
        ..Default::default()
    }
}

pub fn server_window(assets: &IntroV2Assets, fonts: &FontAssets) -> impl Scene {
    let window = assets.server_list_window.clone();
    let font = fonts.nine.clone();
    let button_sound = assets.sound_button_sound_a.clone();
    let close_sound = assets.sound_window_close.clone();
    let cancel_close_sound = close_sound.clone();
    let up = slider_style(
        &assets.server_list_slider_button_up,
        &assets.server_list_slider_button_up_focus,
        &assets.server_list_slider_button_up_press,
    );
    let mov = slider_style(
        &assets.server_list_slider_button_mov,
        &assets.server_list_slider_button_mov_focus,
        &assets.server_list_slider_button_mov_press,
    );
    let down = slider_style(
        &assets.server_list_slider_button_down,
        &assets.server_list_slider_button_down_focus,
        &assets.server_list_slider_button_down_press,
    );

    bsn! {
        ServerSelectRoot
        Name("Server Selection V2")
        Node {
            position_type: PositionType::Absolute,
            justify_content: JustifyContent::Center,
            flex_direction: FlexDirection::Column,
            width: percent(100),
            height: percent(100),
        }
        Visibility::Hidden
        Children [
            (
                ServerWindow
                ImageNode { image: {window}, color: Color::NONE, image_mode: NodeImageMode::Stretch }
                Node {
                    flex_direction: FlexDirection::Column,
                    align_self: AlignSelf::Center,
                    justify_content: JustifyContent::FlexStart,
                    width: px(240),
                    height: px(300),
                    overflow: Overflow::clip(),
                }
                Children [
                    (
                        image_button(up, 20.0, 20.0)
                        ImageNode { color: Color::NONE }
                        Node { position_type: PositionType::Absolute, top: px(25), right: px(6), align_self: AlignSelf::FlexEnd }
                    ),
                    (
                        image_button(mov, 20.0, 20.0)
                        ImageNode { color: Color::NONE }
                        Node { position_type: PositionType::Absolute, top: px(45), right: px(6), align_self: AlignSelf::FlexEnd }
                    ),
                    (
                        image_button(down, 20.0, 20.0)
                        ImageNode { color: Color::NONE }
                        Node { position_type: PositionType::Absolute, top: px(275), right: px(6), bottom: px(6), align_self: AlignSelf::FlexEnd }
                    ),
                ]
            ),
            // Select / Cancel button row
            (
                Node {
                    width: px(288),
                    height: px(41),
                    align_self: AlignSelf::Center,
                    justify_content: JustifyContent::SpaceEvenly,
                    top: px(10),
                }
                Children [
                    (
                        image_button(main_button_style(assets), 91.0, 41.0)
                        SelectButton
                        ImageNode { color: Color::NONE }
                        ButtonSound({button_sound.clone()})
                        Children [ (label("Select", font.clone(), 16.0)) ]
                        on(move |_activate: On<Activate>,
                            selected_row: Query<&ShardRow, With<SelectedRow>>,
                            mut selected_shard: ResMut<SelectedShardV2>,
                            mut next_state: ResMut<NextState<IntroV2State>>,
                            options: Res<GameOptions>,
                            mut commands: Commands| {
                            let Ok(row) = selected_row.single() else {
                                return;
                            };
                            if let Some(playback) = options.audio.fx_playback() {
                                commands.spawn((
                                    AudioPlayer::new(close_sound.clone()),
                                    playback,
                                ));
                            }
                            selected_shard.0 = Some(row.0);
                            next_state.set(IntroV2State::LoginForm);
                        })
                    ),
                    (
                        image_button(main_button_style(assets), 91.0, 41.0)
                        CancelButton
                        ImageNode { color: Color::NONE }
                        ButtonSound({button_sound})
                        Children [ (label("Cancel", font, 16.0)) ]
                        on(move |_activate: On<Activate>,
                            mut next_state: ResMut<NextState<IntroV2State>>,
                            options: Res<GameOptions>,
                            mut commands: Commands| {
                            if let Some(playback) = options.audio.fx_playback() {
                                commands.spawn((
                                    AudioPlayer::new(cancel_close_sound.clone()),
                                    playback,
                                ));
                            }
                            next_state.set(IntroV2State::LoginForm);
                        })
                    ),
                ]
            ),
        ]
    }
}

fn shard_row(shard: &Shard, _assets: &IntroV2Assets, fonts: &FontAssets) -> impl Scene {
    let shard_id = shard.id;
    let name = shard.name.clone();
    let font = fonts.nine.clone();
    let status_font = fonts.nine.clone();

    let (status_color, status_text) = if !shard.is_operating {
        (bevy::color::palettes::css::GRAY, "Maintenance")
    } else {
        let quota = shard.online_count as f32 / shard.capacity as f32;
        if quota <= 0.33 {
            (bevy::color::palettes::css::AQUAMARINE, "Easy")
        } else if quota <= 0.67 {
            (bevy::color::palettes::css::YELLOW, "Easy")
        } else {
            (bevy::color::palettes::css::RED, "Crowded")
        }
    };
    let status_text = status_text.to_string();

    bsn! {
        bevy::ui_widgets::Button
        Hovered
        ShardRow({shard_id})
        ImageNode { image: {TRANSPARENT_IMAGE_HANDLE}, color: Color::NONE, image_mode: NodeImageMode::Stretch }
        Node {
            width: px(204),
            height: px(20),
            margin: {UiRect { top: Val::Px(5.0), left: Val::Px(5.0), ..default() }},
            top: px(25),
            padding: {UiRect::horizontal(Val::Px(5.0))},
            justify_content: JustifyContent::SpaceBetween,
            flex_direction: FlexDirection::Row,
            position_type: PositionType::Relative,
        }
        BackgroundColor(Color::NONE)
        Children [
            (
                label(name.as_str(), font, 12.0)
                Node { justify_content: JustifyContent::FlexStart, align_self: AlignSelf::Center, max_width: px(120) }
            ),
            (
                label(status_text.as_str(), status_font, 12.0)
                TargetColor({status_color})
                Node { justify_content: JustifyContent::FlexEnd, align_self: AlignSelf::Center, max_width: px(60) }
            ),
        ]
        on(move |activate: On<Activate>,
            previously_selected: Query<Entity, With<SelectedRow>>,
            mut commands: Commands| {
            for entity in previously_selected.iter() {
                commands.entity(entity).remove::<SelectedRow>();
            }
            commands.entity(activate.entity).insert(SelectedRow);
        })
    }
}

/// Run condition of [`update_shard_rows`]: the rows must (re)build when the
/// shard list changes — or when the server window itself has just spawned.
/// The gateway can deliver the list during scene loading, before the chrome
/// exists; with a plain `resource_exists_and_changed` gate that change tick
/// is consumed against a missing window and the server list stays empty
/// (live-observed 2026-08-11: response 0.1s after connect, still in Splash).
pub fn shard_rows_need_refresh(
    shard_list: Option<Res<ShardList>>,
    new_window: Query<(), Added<ServerWindow>>,
) -> bool {
    match shard_list {
        Some(list) => list.is_changed() || !new_window.is_empty(),
        None => false,
    }
}

/// (Re-)spawns one row per shard whenever [`shard_rows_need_refresh`] fires.
pub fn update_shard_rows(
    shard_list: Res<ShardList>,
    window_query: Query<Entity, With<ServerWindow>>,
    existing_rows: Query<Entity, With<ShardRow>>,
    assets: Res<IntroV2Assets>,
    fonts: Res<FontAssets>,
    mut commands: Commands,
) {
    let Ok(window) = window_query.single() else {
        return;
    };

    for row in existing_rows.iter() {
        commands.entity(row).despawn();
    }

    let rows: Vec<_> = shard_list
        .0
        .shards
        .iter()
        .map(|shard| shard_row(shard, &assets, &fonts))
        .collect();
    commands
        .entity(window)
        .queue_spawn_related_scenes::<Children>(rows);
}

/// Swaps row art for hover/selection, mirroring the old
/// `on_server_list_item_selected` visuals.
pub fn update_shard_row_visuals(
    mut rows: Query<
        (
            &Hovered,
            Has<SelectedRow>,
            &mut ImageNode,
            &mut BackgroundColor,
        ),
        With<ShardRow>,
    >,
    assets: Res<IntroV2Assets>,
) {
    for (hovered, selected, mut image, mut bg_color) in rows.iter_mut() {
        if selected {
            let target = &assets.server_list_item_select;
            if image.image != *target {
                image.image = target.clone();
            }
            bg_color.0 = Color::NONE;
        } else if hovered.get() {
            let target = &assets.server_list_item_hover;
            if image.image != *target {
                image.image = target.clone();
            }
            bg_color.0 = Color::NONE;
        } else {
            if image.image != TRANSPARENT_IMAGE_HANDLE {
                image.image = TRANSPARENT_IMAGE_HANDLE;
            }
            bg_color.0 = Color::NONE;
        }
    }
}

/// Shows the committed shard's name in the login form.
pub fn update_shard_name_text(
    selected_shard: Res<SelectedShardV2>,
    shard_list: Res<ShardList>,
    mut query: Query<&mut Text, With<ShardNameText>>,
) {
    let Ok(mut text) = query.single_mut() else {
        return;
    };

    let Some(selected) = selected_shard.0 else {
        return;
    };

    let Some(shard) = shard_list
        .0
        .shards
        .iter()
        .find(|shard| shard.id == selected)
    else {
        return;
    };

    if text.0 != shard.name {
        text.0 = shard.name.clone();
    }
}
