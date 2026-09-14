use bevy::audio::Volume;
use bevy::input_focus::tab_navigation::TabNavigationPlugin;
use bevy::log::warn_once;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::text::EditableText;
use bevy::ui::{InteractionDisabled, Pressed};

use crate::assets::textdata::effectsound::SoundAddress;
use crate::assets::FontAssets;
use crate::plugins::settings::options::GameOptions;
use crate::plugins::textdata::{ClientEffectSounds, ClientUiStrings};
use style::{ButtonSound, ImageButtonStyle, PasswordEcho};

pub mod choice_confirm;
pub mod style;
pub mod widgets;

/// Widget library built on bevy 0.19's headless widgets (`bevy_ui_widgets`),
/// styled with the game's own image assets. Used by the intro v2 scene.
pub struct UiV2Plugin;

impl Plugin for UiV2Plugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(TabNavigationPlugin)
            .init_resource::<choice_confirm::ChoiceConfirmState>()
            .add_message::<choice_confirm::ChoiceConfirmed>()
            .add_systems(
                Update,
                (
                    update_image_button_visuals,
                    play_button_press_sound,
                    update_password_echo,
                    // The dialog needs the loaded font/string tables, and
                    // `UiV2Plugin` is scene-agnostic, so this system exists from
                    // the very first frame — before the loading screen has put
                    // `FontAssets`/`ClientUiStrings` in the world. Without this
                    // gate, parameter validation fails on frame 1 and takes the
                    // whole app down in every scene (§4b class of defect).
                    choice_confirm::sync_choice_confirm.run_if(
                        resource_exists::<FontAssets>.and_then(resource_exists::<ClientUiStrings>),
                    ),
                ),
            );
    }
}

fn update_image_button_visuals(
    mut query: Query<(
        &Hovered,
        Has<Pressed>,
        Has<InteractionDisabled>,
        &ImageButtonStyle,
        &mut ImageNode,
    )>,
) {
    for (hovered, pressed, disabled, style, mut image) in query.iter_mut() {
        let target = match (disabled, pressed, hovered.get()) {
            (true, _, _) => disabled_art(style),
            (_, true, _) => &style.press,
            (_, _, true) => &style.hover,
            _ => &style.normal,
        };
        if image.image != *target {
            image.image = target.clone();
        }
    }
}

/// The `disable` slot, or `normal` when the call site has no `_disable` art.
/// The fallback is legitimate (not every button in the archive ships a
/// disabled frame), but it is reported once so an unset slot on art that
/// *does* have one cannot hide as "looks normal".
fn disabled_art(style: &ImageButtonStyle) -> &Handle<Image> {
    if style.disable == Handle::default() {
        warn_once!(
            "ui_v2: disabled button has no `disable` art, falling back to `normal` ({:?})",
            style.normal.path()
        );
        &style.normal
    } else {
        &style.disable
    }
}

/// Plays the click through the `effectsound.txt` registry (#773): the row
/// `UI / SND_BUTTON_CLICK` names both the `.wav` and the volume the data wants
/// it at (80), which no call site could have invented. The `ButtonSound`
/// component stays as the fallback for the frames before the table is loaded —
/// `UiV2Plugin` is scene-agnostic and runs from frame 1, so the registry
/// resource is read as `Option<Res<_>>` (same reason as the run condition on
/// `sync_choice_confirm` above).
fn play_button_press_sound(
    query: Query<&ButtonSound, Added<Pressed>>,
    options: Res<GameOptions>,
    effect_sounds: Option<Res<ClientEffectSounds>>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
) {
    let Some(playback) = options.audio.fx_playback() else {
        return;
    };
    let row = effect_sounds
        .as_ref()
        .and_then(|s| s.first(&SoundAddress::new("UI", "SND_BUTTON_CLICK")));
    for sound in query.iter() {
        let (source, playback) = match row {
            // the row's volume scales the FX channel, it does not replace it
            Some(row) => (
                asset_server.load(row.asset_path()),
                PlaybackSettings {
                    volume: Volume::Linear(playback.volume.to_linear() * row.gain()),
                    ..playback
                },
            ),
            None => (sound.0.clone(), playback),
        };
        commands.spawn((AudioPlayer::new(source), playback));
    }
}

/// Mirrors the sibling password `EditableText` as asterisks (see
/// [`PasswordEcho`]).
fn update_password_echo(
    mut echoes: Query<(&ChildOf, &mut Text), With<PasswordEcho>>,
    children_query: Query<&Children>,
    sources: Query<&EditableText>,
) {
    for (child_of, mut text) in echoes.iter_mut() {
        let Ok(siblings) = children_query.get(child_of.parent()) else {
            continue;
        };
        let Some(source) = siblings.iter().find_map(|e| sources.get(e).ok()) else {
            continue;
        };

        let masked = "*".repeat(source.value().to_string().chars().count());
        if text.0 != masked {
            text.0 = masked;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drive the real system in an `App`: the disabled arm used to resolve to
    /// `normal`, which is exactly the defect (#640) — a button that cannot be
    /// pressed looked like one that can.
    fn app_with_button(disable: Option<&str>) -> (App, Entity, Handle<Image>, Handle<Image>) {
        let mut app = App::new();
        // TaskPoolPlugin BEFORE AssetPlugin: `asset_server.load()` touches the
        // IoTaskPool, so the order decides green/red, not the test itself.
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
        ))
        // `AssetPlugin` installs the asset *server*, not any asset *type*:
        // `Image` is registered by `ImagePlugin`, which drags in the whole
        // render stack. So register just the one type this test loads, or
        // `assets.load::<Image>` panics "asset type has not been initialized".
        .init_asset::<Image>()
        .add_systems(Update, update_image_button_visuals);
        let assets = app.world().resource::<AssetServer>().clone();
        let normal: Handle<Image> = assets.load("normal.png");
        let disable_handle: Handle<Image> = match disable {
            Some(path) => assets.load(path.to_string()),
            None => Handle::default(),
        };
        let style = ImageButtonStyle {
            normal: normal.clone(),
            hover: assets.load("hover.png"),
            press: assets.load("press.png"),
            disable: disable_handle.clone(),
        };
        let entity = app
            .world_mut()
            .spawn((
                Hovered::default(),
                style,
                ImageNode {
                    image: normal.clone(),
                    ..Default::default()
                },
                InteractionDisabled,
            ))
            .id();
        (app, entity, normal, disable_handle)
    }

    #[test]
    fn a_disabled_button_shows_the_disable_art() {
        let (mut app, entity, _normal, disable) = app_with_button(Some("disable.png"));
        app.update();
        let image = app.world().entity(entity).get::<ImageNode>().unwrap();
        assert_eq!(image.image, disable);
    }

    #[test]
    fn a_disabled_button_without_disable_art_falls_back_to_normal() {
        let (mut app, entity, normal, _) = app_with_button(None);
        app.update();
        let image = app.world().entity(entity).get::<ImageNode>().unwrap();
        assert_eq!(image.image, normal);
    }
}
