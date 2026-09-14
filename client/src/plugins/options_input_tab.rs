//! The options window's **Key Map** tab: rebind the `OptionSet.csv` shortcuts.
//!
//! Idea: the row list is generated from [`KEY_ACTIONS`] — the 32 shortcut actions
//! `OptionSet.csv` names — so the tab's contents are grounded in the user's own
//! data file rather than a hand-written menu. Clicking a row's key button arms a
//! capture; the next key press is translated to a Win32 VK code and stored in
//! [`GameOptions`], which the settings plugin persists on change. A binding that
//! collides with another action is shown in red on both rows rather than being
//! rejected — two actions on one key is a state the option stream can hold, so the
//! UI surfaces it instead of silently repairing it.
//!
//! The pane also carries the vanilla **mouse two-state radio** (`SROptionSet`
//! id 3101, `isMouseShortcutSwapped`): wheel-changes-view vs wheel-uses-shortcut,
//! on the authored rects of `ifoption_input.txt`. Before #605 that id was parsed,
//! stored and persisted with **no** control and, in `zoom_orbit`, no visible
//! effect — `camera.rs::mouse_camera_roles` is its reader, and this is its writer.
//!
//! Labels: the CSV `Name` column is an identifier (`KeyInventory`), not a caption,
//! and the original's textuisystem keys for these rows are not established, so the
//! identifier is shown split into words. That is deliberately a placeholder for a
//! grounded string, not an invented translation — see the note on `humanize`.

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui_widgets::{Activate, Button};

use crate::plugins::config::input::MouseScheme;
use crate::plugins::config::ClientConfig;
use crate::plugins::settings::keymap::KEY_ACTIONS;
use crate::plugins::settings::options::GameOptions;
use crate::plugins::textdata::ClientUiStrings;
use crate::plugins::ui_v2::style::ImageButtonStyle;

/// The pane's own rects, pane-local, from `resinfo/ifoption_input.txt:6,25,44`
/// (`docs/re/ui/options-controls.md` §2): the mouse frame, its `UIIT_STT_MOUSE`
/// title, the two-state radio box, and the key list below the section header.
const MOUSE_FRAME: (f32, f32, f32, f32) = (14.0, 13.0, 337.0, 80.0);
const MOUSE_TITLE: (f32, f32) = (14.0, 20.0);
const MOUSE_RADIO: (f32, f32, f32, f32) = (29.0, 43.0, 326.0, 39.0);
const KEY_LIST: (f32, f32, f32, f32) = (14.0, 133.0, 336.0, 161.0);

const RADIO_ON: &str = "media://interface/ifcommon/com_radiobutton_on.ddj";
const RADIO_OFF: &str = "media://interface/ifcommon/com_radiobutton_off.ddj";
/// `com_radiobutton_off.ddj` is 16x16 (`options-controls.md` §5).
const RADIO_SIZE: f32 = 16.0;
/// Gap between the box and its caption. **Ours**: the classic control carries
/// `Text=""` and no per-state geometry at all, so only the 39px box as a whole
/// is sourced; splitting it into two 19.5px rows is the obvious two-state read
/// of a two-state radio, and the 4th-gen tree's own pair (`78,160` / `78,183`,
/// 16x16) corroborates the shape.
const RADIO_LABEL_GAP: f32 = 6.0;

const ROW_H: f32 = 18.0;
const LABEL_W: f32 = 200.0;
const KEY_BTN_W: f32 = 96.0;
const FONT: &str = crate::assets::BUNDLED_FALLBACK_FACE;
const FONT_SIZE: f32 = 11.0;
/// Same button art the window shell uses for its own rows.
const COM_BUTTON_DDJ: &str = "media://interface/ifcommon/com_button.ddj";

/// Row text; red while the row's key collides with another action.
const ROW_COLOR: Color = Color::WHITE;
const CONFLICT_COLOR: Color = Color::srgb(0.93, 0.31, 0.31);
/// Gold while the row is waiting for a key press, matching the active-tab gold.
const CAPTURING_COLOR: Color = Color::srgb(1.0, 0.816, 0.318);
/// openroad-only: a control whose value nothing acts on in the current scheme is
/// dimmed rather than silently inert (the same convention as `options_game.rs`).
const INERT_COLOR: Color = Color::srgb_u8(128, 128, 128);

/// Which action, if any, is currently waiting for a key press.
#[derive(Resource, Default, Debug, PartialEq, Eq)]
pub struct KeyCapture(pub Option<u16>);

/// The key-button caption of one action row.
#[derive(Component)]
struct KeyBindingLabel(u16);

pub struct OptionsInputTabPlugin;

impl Plugin for OptionsInputTabPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<KeyCapture>().add_systems(
            Update,
            (
                capture_pressed_key,
                refresh_binding_labels,
                // The radio repaint needs the two app-level resources the pane
                // itself needs (the config it reports and the art it loads); the
                // module's own App tests build a bare `App` with neither, and a
                // missing `Res` is a run-time panic, not a compile error.
                refresh_mouse_radio.run_if(
                    resource_exists::<ClientConfig>.and_then(resource_exists::<AssetServer>),
                ),
            ),
        );
    }
}

/// One position of the mouse two-state radio: the value of id 3101 it selects.
///
/// Vanilla's pair is *which device changes the view* — `textuisystem` 917
/// `UIIT_STT_USE_WHEEL_TO_CHANGE_SIGHT` vs 918 `UIIT_STT_USE_WHEEL_TO_USE_SKILL`
/// (`docs/re/ui/options-controls.md` §2). `camera.rs::mouse_camera_roles` is the
/// reader, so this control is a behaviour and not another dead wire (#605).
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub struct MouseSwapRadio {
    /// The `mouse_shortcut_swapped` (id 3101) value this position writes.
    swapped: bool,
}

/// The caption next to a radio box, so the inert colour can follow the scheme.
#[derive(Component, Clone, Copy)]
struct MouseSwapLabel;

/// The two positions, in the 4th-gen tree's own order (entry 140 = "use skill"
/// above entry 141 = "change sight").
const MOUSE_ROWS: [(bool, &str, &str); 2] = [
    (
        true,
        "UIIT_STT_USE_WHEEL_TO_USE_SKILL",
        "Use the wheel for shortcuts",
    ),
    (
        false,
        "UIIT_STT_USE_WHEEL_TO_CHANGE_SIGHT",
        "Use the wheel to change the view",
    ),
];

/// Fills the Key Map pane. Called by the options window while it spawns the pane
/// so the tab body lives here rather than in the shell.
pub fn build_input_pane(
    pane: &mut ChildSpawnerCommands,
    assets: &AssetServer,
    ui_strings: &ClientUiStrings,
    options: &GameOptions,
) {
    let font = assets.load::<Font>(FONT);
    let button_style = ImageButtonStyle {
        normal: assets.load(COM_BUTTON_DDJ),
        hover: assets.load(COM_BUTTON_DDJ),
        press: assets.load(COM_BUTTON_DDJ),
        ..Default::default()
    };

    spawn_mouse_radio(pane, assets, &font, ui_strings, options);

    // The key list now sits on its authored rect (`ifoption_input.txt`, the
    // `CIFScrollManager` at `14,133,336,161`) instead of filling the pane —
    // it has to, or it would run under the mouse frame above it.
    pane.spawn((
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(KEY_LIST.0),
            top: Val::Px(KEY_LIST.1),
            width: Val::Px(KEY_LIST.2),
            height: Val::Px(KEY_LIST.3),
            flex_direction: FlexDirection::Column,
            overflow: Overflow::scroll_y(),
            ..default()
        },
        Pickable::IGNORE,
    ))
    .with_children(|list| {
        for act in KEY_ACTIONS {
            list.spawn((
                Node {
                    width: Val::Percent(100.0),
                    height: Val::Px(ROW_H),
                    align_items: AlignItems::Center,
                    ..default()
                },
                Pickable::IGNORE,
            ))
            .with_children(|row| {
                row.spawn((
                    Node {
                        width: Val::Px(LABEL_W),
                        ..default()
                    },
                    Pickable::IGNORE,
                ))
                .with_children(|c| {
                    c.spawn((
                        Text::new(humanize(act.name)),
                        TextFont {
                            font: font.clone().into(),
                            font_size: FontSize::Px(FONT_SIZE),
                            ..default()
                        },
                        TextColor(ROW_COLOR),
                        Pickable::IGNORE,
                    ));
                });

                row.spawn((
                    Button,
                    Hovered::default(),
                    Node {
                        width: Val::Px(KEY_BTN_W),
                        height: Val::Px(ROW_H - 2.0),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        ..default()
                    },
                    ImageNode::new(button_style.normal.clone()),
                    button_style.clone(),
                ))
                .observe(
                    move |_activate: On<Activate>, mut capture: ResMut<KeyCapture>| {
                        // Clicking the armed row disarms it, so a mis-click is
                        // undoable without binding something.
                        capture.0 = if capture.0 == Some(act.id) {
                            None
                        } else {
                            Some(act.id)
                        };
                    },
                )
                .with_children(|c| {
                    c.spawn((
                        Text::new(String::new()),
                        TextFont {
                            font: font.clone().into(),
                            font_size: FontSize::Px(FONT_SIZE),
                            ..default()
                        },
                        TextColor(ROW_COLOR),
                        KeyBindingLabel(act.id),
                        Pickable::IGNORE,
                    ));
                });
            });
        }
    });
}

/// The mouse frame and its two-state radio, above the key list.
///
/// The frame is drawn as a hairline box rather than the vanilla `opt_inner_box_*`
/// tileset: that tileset is not transcribed anywhere in this tree yet, and an
/// untextured border is honest about it (openroad convention, see
/// `options_game.rs`'s dimmed rows).
fn spawn_mouse_radio(
    pane: &mut ChildSpawnerCommands,
    assets: &AssetServer,
    font: &Handle<Font>,
    ui_strings: &ClientUiStrings,
    options: &GameOptions,
) {
    pane.spawn((
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(MOUSE_FRAME.0),
            top: Val::Px(MOUSE_FRAME.1),
            width: Val::Px(MOUSE_FRAME.2),
            height: Val::Px(MOUSE_FRAME.3),
            border: UiRect::all(Val::Px(1.0)),
            ..default()
        },
        BorderColor::all(Color::srgba(1.0, 1.0, 1.0, 0.15)),
        Pickable::IGNORE,
    ));

    pane.spawn((
        Text::new(ui_strings.get_or("UIIT_STT_MOUSE", "Mouse").to_string()),
        TextFont {
            font: font.clone().into(),
            font_size: FontSize::Px(FONT_SIZE),
            ..default()
        },
        TextColor(ROW_COLOR),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(MOUSE_TITLE.0),
            top: Val::Px(MOUSE_TITLE.1),
            ..default()
        },
        Pickable::IGNORE,
    ));

    let row_h = MOUSE_RADIO.3 / MOUSE_ROWS.len() as f32;
    for (index, (swapped, key, english)) in MOUSE_ROWS.iter().enumerate() {
        let selected = *swapped == options.keymap.mouse_shortcut_swapped;
        let art: Handle<Image> = assets.load(if selected { RADIO_ON } else { RADIO_OFF });
        // The whole row is the hit target, not the 16x16 box (same reason as
        // `options_game.rs`: the vanilla box is under the WCAG 2.2 AA minimum
        // and widening only the click area changes no vanilla geometry).
        pane.spawn((
            MouseSwapRadio { swapped: *swapped },
            Button,
            Hovered::default(),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(MOUSE_RADIO.0),
                top: Val::Px(MOUSE_RADIO.1 + index as f32 * row_h),
                width: Val::Px(MOUSE_RADIO.2),
                height: Val::Px(row_h),
                align_items: AlignItems::Center,
                ..default()
            },
            Pickable::default(),
        ))
        .observe(on_mouse_radio_activate)
        .with_children(|row| {
            row.spawn((
                MouseSwapRadio { swapped: *swapped },
                ImageNode {
                    image: art,
                    ..default()
                },
                Node {
                    width: Val::Px(RADIO_SIZE),
                    height: Val::Px(RADIO_SIZE),
                    ..default()
                },
                Pickable::IGNORE,
            ));
            row.spawn((
                MouseSwapLabel,
                Text::new(ui_strings.get_or(key, english).to_string()),
                TextFont {
                    font: font.clone().into(),
                    font_size: FontSize::Px(FONT_SIZE),
                    ..default()
                },
                TextColor(ROW_COLOR),
                Node {
                    margin: UiRect::left(Val::Px(RADIO_LABEL_GAP)),
                    ..default()
                },
                Pickable::IGNORE,
            ));
        });
    }
}

/// Select this position: write id 3101. Persistence picks the change up on its
/// own (`settings::persistence::save_on_change`), and `camera.rs` reads it on
/// the next frame — that read is the whole point of #605.
fn on_mouse_radio_activate(
    activate: On<Activate>,
    radios: Query<&MouseSwapRadio>,
    mut options: ResMut<GameOptions>,
) {
    let Ok(radio) = radios.get(activate.entity) else {
        return;
    };
    if options.keymap.mouse_shortcut_swapped != radio.swapped {
        options.keymap.mouse_shortcut_swapped = radio.swapped;
    }
}

/// Repaint the pair, and dim it while `input.mouse_scheme` is openroad's
/// non-original `zoom_orbit` — in that scheme both devices drive the camera and
/// id 3101 changes nothing, so the control says so instead of lying (ADR 0009:
/// the deviation is named, not hidden).
fn refresh_mouse_radio(
    options: Res<GameOptions>,
    config: Res<ClientConfig>,
    assets: Res<AssetServer>,
    mut boxes: Query<(&MouseSwapRadio, &mut ImageNode)>,
    mut labels: Query<&mut TextColor, With<MouseSwapLabel>>,
) {
    if !options.is_changed() && !config.is_changed() {
        return;
    }
    let vanilla = matches!(config.input.mouse_scheme, MouseScheme::Vanilla);
    for (radio, mut image) in &mut boxes {
        let art = if radio.swapped == options.keymap.mouse_shortcut_swapped {
            RADIO_ON
        } else {
            RADIO_OFF
        };
        image.image = assets.load(art);
        image.color = if vanilla { Color::WHITE } else { INERT_COLOR };
    }
    let color = if vanilla { ROW_COLOR } else { INERT_COLOR };
    for mut text_color in &mut labels {
        text_color.0 = color;
    }
}

/// While a row is armed, the next press becomes its binding.
///
/// Escape cancels instead of binding — it is the window's own close key and is not
/// a `KeyMap` action, so it must stay unbindable. A key with no VK code is refused
/// (the capture stays armed) rather than stored as a value we could not read back.
fn capture_pressed_key(
    keys: Res<ButtonInput<KeyCode>>,
    mut capture: ResMut<KeyCapture>,
    mut options: ResMut<GameOptions>,
) {
    let Some(id) = capture.0 else {
        return;
    };
    let Some(&key) = keys.get_just_pressed().next() else {
        return;
    };
    if key == KeyCode::Escape {
        capture.0 = None;
        return;
    }
    if options.bind_key(id, key) {
        capture.0 = None;
    }
}

/// Repaints every row's key caption and conflict colour.
///
/// Runs on change only. `GameOptions` is the single source of truth here, so a
/// binding written by any other path shows up without extra plumbing.
fn refresh_binding_labels(
    options: Res<GameOptions>,
    capture: Res<KeyCapture>,
    mut labels: Query<(&KeyBindingLabel, &mut Text, &mut TextColor)>,
) {
    if !options.is_changed() && !capture.is_changed() {
        return;
    }
    for (label, mut text, mut color) in labels.iter_mut() {
        let id = label.0;
        let armed = capture.0 == Some(id);
        let bound = options.key_for(id);

        text.0 = if armed {
            "...".to_string()
        } else {
            bound.map(key_caption).unwrap_or_else(|| "-".to_string())
        };
        color.0 = if armed {
            CAPTURING_COLOR
        } else if !options.key_conflicts(id).is_empty() {
            CONFLICT_COLOR
        } else {
            ROW_COLOR
        };
    }
}

/// Clears every stored binding, so all rows fall back to their defaults.
/// Wired to the options window's `Default` button for this tab.
pub fn reset_all_bindings(options: &mut GameOptions) {
    for act in KEY_ACTIONS {
        options.reset_key(act.id);
    }
}

/// A short caption for a key. `KeyCode`'s `Debug` is already close to what the
/// vanilla tab shows (`KeyI` -> `I`, `F5` -> `F5`), so it is trimmed rather than
/// given a 90-entry caption table that would duplicate the VK table.
fn key_caption(key: KeyCode) -> String {
    let raw = format!("{key:?}");
    raw.strip_prefix("Key")
        .or_else(|| raw.strip_prefix("Digit"))
        .unwrap_or(&raw)
        .to_string()
}

/// `KeyInventory` -> `Key Inventory`.
///
/// A stopgap: the CSV column is an identifier, and the original's caption strings
/// for these rows are not established. It stays mechanical on purpose — the moment
/// the textuisystem keys are known, this is the one place to replace, and nothing
/// here pretends to be a translated caption.
fn humanize(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (i, ch) in name.char_indices() {
        if i > 0 && ch.is_uppercase() {
            out.push(' ');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::camera::{mouse_camera_roles, MouseCameraRoles};
    use crate::plugins::settings::keymap::{action, keycode_to_vk};

    /// The pair must cover both values of id 3101 exactly once: a radio that
    /// can only write one of them is the dead wire this ticket is about.
    #[test]
    fn the_mouse_radio_covers_both_states_of_id_3101() {
        let mut states: Vec<bool> = MOUSE_ROWS.iter().map(|(swapped, ..)| *swapped).collect();
        states.sort_unstable();
        assert_eq!(states, vec![false, true]);
        // and both positions are labelled by the vanilla strings, not by ours
        assert_eq!(MOUSE_ROWS[0].1, "UIIT_STT_USE_WHEEL_TO_USE_SKILL");
        assert_eq!(MOUSE_ROWS[1].1, "UIIT_STT_USE_WHEEL_TO_CHANGE_SIGHT");
    }

    /// Rects are transcribed, not designed (`ifoption_input.txt:6,25,44`), and
    /// the two rows must tile the authored 39px box exactly.
    #[test]
    fn the_mouse_rects_are_the_authored_ones() {
        assert_eq!(MOUSE_FRAME, (14.0, 13.0, 337.0, 80.0));
        assert_eq!(MOUSE_TITLE, (14.0, 20.0));
        assert_eq!(MOUSE_RADIO, (29.0, 43.0, 326.0, 39.0));
        assert_eq!(KEY_LIST, (14.0, 133.0, 336.0, 161.0));
        let row_h = MOUSE_RADIO.3 / MOUSE_ROWS.len() as f32;
        assert_eq!(row_h * MOUSE_ROWS.len() as f32, MOUSE_RADIO.3);
        // the list starts below the frame it now shares the pane with
        assert!(KEY_LIST.1 >= MOUSE_FRAME.1 + MOUSE_FRAME.3);
    }

    /// The point of the control: each position selects a *different* camera
    /// behaviour, so flipping it is observable (acceptance 2 of #605).
    #[test]
    fn each_radio_position_selects_a_different_camera_role() {
        let roles: Vec<MouseCameraRoles> = MOUSE_ROWS
            .iter()
            .map(|(swapped, ..)| mouse_camera_roles(MouseScheme::Vanilla, *swapped))
            .collect();
        assert_ne!(roles[0], roles[1]);
        // exactly one device changes the view in either position
        for role in roles {
            assert_ne!(role.wheel_changes_view, role.right_button_changes_view);
        }
    }

    /// Clicking a position writes id 3101 through the same field the option
    /// stream parses and persists, so the value round-trips (acceptance 3).
    #[test]
    fn selecting_a_position_writes_the_persisted_field() {
        let mut options = GameOptions::default();
        for (swapped, ..) in MOUSE_ROWS {
            options.keymap.mouse_shortcut_swapped = swapped;
            assert_eq!(options.keymap.mouse_shortcut_swapped, swapped);
        }
    }

    #[test]
    fn key_captions_drop_the_keycode_prefix() {
        assert_eq!(key_caption(KeyCode::KeyI), "I");
        assert_eq!(key_caption(KeyCode::Digit4), "4");
        assert_eq!(key_caption(KeyCode::F5), "F5");
        assert_eq!(key_caption(KeyCode::Space), "Space");
    }

    #[test]
    fn identifiers_are_split_into_words() {
        assert_eq!(humanize("KeyInventory"), "Key Inventory");
        assert_eq!(humanize("KeyCOSRide"), "Key C O S Ride");
    }

    /// Every action the tab lists must be resolvable back through the keymap
    /// table, or a row would render with no way to bind it.
    #[test]
    fn every_listed_row_maps_to_a_known_action() {
        for act in KEY_ACTIONS {
            assert!(action(act.id).is_some());
        }
    }

    /// Defaults must be storable, otherwise a rebind to a default key would be
    /// refused by `bind_key`.
    #[test]
    fn every_default_key_has_a_vk_code() {
        for act in KEY_ACTIONS {
            if let Some(key) = act.default_key {
                assert!(keycode_to_vk(key).is_some(), "{} has no VK", act.name);
            }
        }
    }

    #[test]
    fn resetting_all_bindings_restores_every_default() {
        let mut options = GameOptions::default();
        assert!(options.bind_key(3002, KeyCode::F9));
        assert!(options.bind_key(3003, KeyCode::F10));

        reset_all_bindings(&mut options);

        assert!(options.keymap.bindings.is_empty());
        assert_eq!(options.key_for(3002), Some(KeyCode::KeyI));
        assert_eq!(options.key_for(3003), Some(KeyCode::KeyS));
    }

    /// The capture systems must be able to actually run in a schedule — a
    /// conflicting query would only panic at run time, which no plain unit test
    /// would reach (runbook §4b).
    #[test]
    fn the_tab_systems_run_in_a_schedule_without_query_conflicts() {
        let mut app = App::new();
        app.init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<GameOptions>()
            .add_plugins(OptionsInputTabPlugin);

        app.update();
        app.update();

        assert_eq!(*app.world().resource::<KeyCapture>(), KeyCapture(None));
    }

    /// Arming a row and pressing a key binds it; Escape cancels instead.
    #[test]
    fn an_armed_row_binds_the_next_press_and_escape_cancels() {
        let mut app = App::new();
        app.init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<GameOptions>()
            .add_plugins(OptionsInputTabPlugin);

        app.world_mut().resource_mut::<KeyCapture>().0 = Some(3002);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::F7);
        app.update();

        assert_eq!(app.world().resource::<KeyCapture>().0, None);
        assert_eq!(
            app.world().resource::<GameOptions>().key_for(3002),
            Some(KeyCode::F7)
        );

        // Escape must not become a binding — it closes the window.
        app.world_mut().resource_mut::<KeyCapture>().0 = Some(3003);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .clear();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Escape);
        app.update();

        assert_eq!(app.world().resource::<KeyCapture>().0, None);
        assert_eq!(
            app.world().resource::<GameOptions>().key_for(3003),
            Some(KeyCode::KeyS)
        );
    }
}
