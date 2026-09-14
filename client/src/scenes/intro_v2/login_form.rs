use bevy::app::AppExit;
use bevy::input_focus::tab_navigation::TabGroup;
use bevy::prelude::*;
use bevy::ui_widgets::Activate;

use crate::assets::FontAssets;
use crate::plugins::settings::options::GameOptions;
use crate::plugins::ui_v2::style::{ButtonSound, ImageButtonStyle};
use crate::plugins::ui_v2::widgets::{image_button, label, password_input, text_input};

use super::assets::IntroV2Assets;
use super::IntroV2State;

/// Root marker of the login form screen.
#[derive(Component, Default, Clone)]
pub struct LoginFormRoot;

/// Marker on the username `EditableText`.
#[derive(Component, Default, Clone)]
pub struct IdInput;

/// Marker on the password `EditableText`.
#[derive(Component, Default, Clone)]
pub struct PwInput;

/// Marker on the text showing the currently selected shard's name.
#[derive(Component, Default, Clone)]
pub struct ShardNameText;

#[derive(Component, Default, Clone)]
pub struct ConnectButton;

#[derive(Component, Default, Clone)]
pub struct ExitButton;

#[derive(Component, Default, Clone)]
pub struct ServerListButton;

/// Style of the intro's main buttons (Connect, Start, ...). These are the two
/// buttons that actually go disabled today — `net.rs` disables Connect while a
/// login request is in flight, `character_select.rs` disables Start while the
/// join is pending — so this is the call site that has to carry `disable`.
pub fn main_button_style(assets: &IntroV2Assets) -> ImageButtonStyle {
    ImageButtonStyle {
        normal: assets.button.clone(),
        hover: assets.button_focus.clone(),
        press: assets.button_press.clone(),
        disable: assets.button_disable.clone(),
    }
}

fn list_button_style(assets: &IntroV2Assets) -> ImageButtonStyle {
    ImageButtonStyle {
        normal: assets.list_button.clone(),
        hover: assets.list_button_focus.clone(),
        press: assets.list_button_press.clone(),
        ..Default::default()
    }
}

pub fn login_form(assets: &IntroV2Assets, fonts: &FontAssets) -> impl Scene {
    let logo = assets.logo.clone();
    let window = assets.login_window.clone();
    let font = fonts.nine.clone();
    let button_sound = assets.sound_button_sound_a.clone();
    let window_open_sound = assets.sound_window_open.clone();

    bsn! {
        LoginFormRoot
        Name("Login Form V2")
        Node {
            justify_content: JustifyContent::Center,
            flex_direction: FlexDirection::Column,
            width: percent(100),
            height: percent(100),
            position_type: PositionType::Absolute,
        }
        Visibility::Hidden
        TabGroup::new(0)
        Children [
            // Small logo above the window
            (
                ImageNode { image: {logo}, color: Color::NONE }
                Node { align_self: AlignSelf::Center }
                Pickable::IGNORE
            ),
            // The login window with labels, inputs and the server-list button
            (
                ImageNode { image: {window}, color: Color::NONE, image_mode: NodeImageMode::Stretch }
                Node {
                    width: px(288),
                    height: px(140),
                    flex_direction: FlexDirection::Column,
                    align_self: AlignSelf::Center,
                    justify_content: JustifyContent::FlexStart,
                }
                Children [
                    (
                        label("ID", font.clone(), 16.0)
                        Node { position_type: PositionType::Absolute, left: px(36), top: px(35), width: px(59), height: px(15), align_self: AlignSelf::FlexStart }
                    ),
                    (
                        label("PW", font.clone(), 16.0)
                        Node { position_type: PositionType::Absolute, left: px(36), top: px(62), width: px(59), height: px(15), align_self: AlignSelf::FlexStart }
                    ),
                    (
                        label("Server", font.clone(), 16.0)
                        Node { position_type: PositionType::Absolute, left: px(36), top: px(89), width: px(59), height: px(15), align_self: AlignSelf::FlexStart }
                    ),
                    (
                        text_input(font.clone(), 0)
                        IdInput
                        Node { position_type: PositionType::Absolute, left: px(110), top: px(35), width: px(135), height: px(20) }
                    ),
                    (
                        password_input::<PwInput>(font.clone(), 1)
                        Node { position_type: PositionType::Absolute, left: px(110), top: px(62), width: px(135), height: px(20) }
                    ),
                    (
                        Node {
                            position_type: PositionType::Absolute,
                            left: px(110),
                            top: px(88),
                            width: px(86),
                            height: px(20),
                            justify_content: JustifyContent::Center,
                        }
                        Children [
                            (label("", font.clone(), 16.0) ShardNameText),
                        ]
                    ),
                    (
                        image_button(list_button_style(assets), 48.0, 24.0)
                        ServerListButton
                        ImageNode { color: Color::NONE }
                        Node { position_type: PositionType::Absolute, left: px(203), top: px(86), align_self: AlignSelf::FlexStart }
                        ButtonSound({button_sound.clone()})
                        on(move |_activate: On<Activate>,
                            mut next_state: ResMut<NextState<IntroV2State>>,
                            options: Res<GameOptions>,
                            mut commands: Commands| {
                            if let Some(playback) = options.audio.fx_playback() {
                                commands.spawn((
                                    AudioPlayer::new(window_open_sound.clone()),
                                    playback,
                                ));
                            }
                            next_state.set(IntroV2State::ServerSelection);
                        })
                    ),
                ]
            ),
            // Connect / Exit button row
            (
                Node {
                    width: px(288),
                    height: px(41),
                    align_self: AlignSelf::Center,
                    justify_content: JustifyContent::SpaceEvenly,
                    margin: {UiRect::top(Val::Percent(1.0))},
                }
                Children [
                    (
                        image_button(main_button_style(assets), 91.0, 41.0)
                        ConnectButton
                        ImageNode { color: Color::NONE }
                        ButtonSound({button_sound.clone()})
                        Children [ (label("Connect", font.clone(), 16.0)) ]
                        on(super::net::on_connect_activate)
                    ),
                    (
                        image_button(main_button_style(assets), 91.0, 41.0)
                        ExitButton
                        ImageNode { color: Color::NONE }
                        ButtonSound({button_sound})
                        Children [ (label("Exit", font, 16.0)) ]
                        on(|_activate: On<Activate>, mut exit: MessageWriter<AppExit>| {
                            exit.write(AppExit::Success);
                        })
                    ),
                ]
            ),
        ]
    }
}
