//! Options -> Camera pane (`OptionsTab::Camera`): the three-way sight radio.
//!
//! Idea: `docs/re/ui/options-camera.md` §6 calls this pane "a pure three-way
//! view-mode radio and nothing else" and files what the modes *do* as UNKNOWN,
//! on the grounds that `ifoption_camera.txt` does not define it. Reading the
//! tree and the string table together corrects both halves, and the correction
//! is why this pane could finally be built:
//!
//! * The tree is **nine** blocks, not three: per mode a `CIFCheckBox` on
//!   `com_radiobutton_off.ddj` (ids 10/11/12 at `25,45` / `25,98` / `25,152`,
//!   16x16), a `CIFStatic` on `interface\option\opt_camera.ddj` (ids 7/8/9 at
//!   `12,29` / `12,83` / `12,137`) and a `CIFPML` caption (ids 13/14/15 at
//!   `57,38` / `57,92` / `57,146`, 282x30). Radio pitch is 53/54px.
//! * The modes **are** defined — in `textuisystem.txt`, by the `_DESC1`/`_DESC2`
//!   lines the pane renders. See [`SightMode`] for the quotes; the short
//!   version is that each mode is named by the orbit axis it removes.
//!
//! Two stated deviations (ADR-0009), both because the vanilla text is the
//! reference and we would rather show more of it than less:
//!
//! 1. The vanilla `CIFPML` caption keys (`UIIT_STT_SIGHT_*_DESC`) are SML
//!    markup that renders to just "Free Camera" / "The Third Person Camera" /
//!    "Quarterview Camera". We have no PML renderer, so each caption is drawn
//!    as the plain mode title (`UIIT_STT_SIGHT_FREE` etc.) with the original's
//!    own two description lines underneath, which is the same copy the original
//!    ships for these three modes and says what the control actually does.
//! 2. The whole row is the click target, not the 16x16 box (WCAG 2.2 AA target
//!    size; `docs/re/ui/options-camera.md` §8.6). No vanilla geometry moves.
//!
//! The radio writes `GameOptions.camera.sight`, which `settings::persistence`
//! already saves on change, and `camera::follow_player_camera` reads every
//! frame — so it survives a restart *and* changes behaviour, which is the
//! acceptance the RE doc states for closing #379.

use bevy::ecs::relationship::RelatedSpawnerCommands;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui_widgets::{Activate, Button};

use crate::plugins::settings::options::{GameOptions, SightMode};
use crate::plugins::textdata::ClientUiStrings;

/// `com_radiobutton_off.ddj` / `_on.ddj`, 16x16 (`ifoption_camera.txt` names
/// only `_off`; the `_on` sibling is how every other checkbox in the options
/// window paints its checked state).
const RADIO_OFF: &str = "media://interface/ifcommon/com_radiobutton_off.ddj";
const RADIO_ON: &str = "media://interface/ifcommon/com_radiobutton_on.ddj";
/// `GDR_OPT_VIDEO_STATIC_BG0..2` all carry this one image
/// (`interface\option\opt_camera.ddj`) — the per-mode illustration strip.
const MODE_ART: &str = "media://interface/option/opt_camera.ddj";

/// `Rect` of the three radios, pane-local (`ifoption_camera.txt` ids 10/11/12).
const RADIO_XY: [(f32, f32); 3] = [(25.0, 45.0), (25.0, 98.0), (25.0, 152.0)];
/// `Rect` of the three `opt_camera.ddj` statics (ids 7/8/9). Vanilla gives them
/// `0,0` extents, i.e. "use the image's own size", so only the origin is here.
const ART_XY: [(f32, f32); 3] = [(12.0, 29.0), (12.0, 83.0), (12.0, 137.0)];
/// `Rect` of the three `CIFPML` captions (ids 13/14/15), 282x30.
const CAPTION_XY: [(f32, f32); 3] = [(57.0, 38.0), (57.0, 92.0), (57.0, 146.0)];
const CAPTION_W: f32 = 282.0;
const RADIO_SIZE: f32 = 16.0;

/// The gold the vanilla captions are written in:
/// `<font color="255,240,217,165">` inside `UIIT_STT_SIGHT_*_DESC` (AARRGGBB).
const TITLE_COLOR: Color = Color::srgb_u8(240, 217, 165);
/// `FontColor="255,255,255,255"` on the `CIFPML` blocks themselves.
const DESC_COLOR: Color = Color::srgb_u8(190, 190, 190);

/// Marks a radio row (and its box) with the mode it selects.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct SightRadio(pub SightMode);

/// The vanilla strings for one mode: the title, then the two lines the original
/// writes underneath it. All six keys exist in the shipped `textuisystem.txt`.
struct ModeStrings {
    title: (&'static str, &'static str),
    desc: [(&'static str, &'static str); 2],
}

/// Ordered top-to-bottom by the radios' `Rect` y (45 / 98 / 152), which is the
/// order the file's blocks are *reversed* from — the tree lists id 15 first.
const MODE_STRINGS: [ModeStrings; 3] = [
    ModeStrings {
        title: ("UIIT_STT_SIGHT_FREE", "Free Movement View"),
        desc: [
            ("UIIT_STT_SIGHT_FREE_DESC1", "Mouse oriented camera control"),
            (
                "UIIT_STT_SIGHT_FREE_DESC2",
                "Operates on multidirectional angle control and mouse movement",
            ),
        ],
    },
    ModeStrings {
        title: ("UIIT_STT_SIGHT_THIRD_PERSON", "Third Person View"),
        desc: [
            // Vanilla's own framing. openroad has no keyboard camera control,
            // so only the second line describes what our build does; the line
            // is kept because it is the original's copy, not ours to rewrite.
            (
                "UIIT_STT_SIGHT_THIRD_PERSON_DESC1",
                "Keyboard oriented camera control",
            ),
            (
                "UIIT_STT_SIGHT_THIRD_PERSON_DESC2",
                "Camera angle is fixed behind the character",
            ),
        ],
    },
    ModeStrings {
        title: ("UIIT_STT_SIGHT_QUATER_VIEW", "Quarter Angle View"),
        desc: [
            (
                "UIIT_STT_SIGHT_QUATER_VIEW_DESC1",
                "The height is fixed to this perspective.",
            ),
            (
                "UIIT_STT_SIGHT_QUATER_VIEW_DESC2",
                "This service is provided for those inconvenienced by 3D motion",
            ),
        ],
    },
];

/// Build the Camera pane into an already-positioned pane node.
pub(crate) fn spawn_camera_pane(
    pane: &mut RelatedSpawnerCommands<ChildOf>,
    asset_server: &AssetServer,
    font: &Handle<Font>,
    ui_strings: &ClientUiStrings,
    options: &GameOptions,
) {
    let off: Handle<Image> = asset_server.load(RADIO_OFF);
    let on: Handle<Image> = asset_server.load(RADIO_ON);
    let art: Handle<Image> = asset_server.load(MODE_ART);

    for (index, mode) in SightMode::ALL.into_iter().enumerate() {
        let strings = &MODE_STRINGS[index];
        let (art_x, art_y) = ART_XY[index];
        let (radio_x, radio_y) = RADIO_XY[index];
        let (caption_x, caption_y) = CAPTION_XY[index];

        // The per-mode illustration. Outside the row so the click target stays
        // a plain rectangle over the radio + caption columns.
        pane.spawn((
            ImageNode {
                image: art.clone(),
                ..default()
            },
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(art_x),
                top: Val::Px(art_y),
                ..default()
            },
            Pickable::IGNORE,
        ));

        // One click target from the radio's left edge to the caption's right,
        // spanning the taller of the two. Vanilla's 16x16 box is under the AA
        // minimum and widening the hit area moves no pixel.
        let row_top = radio_y.min(caption_y);
        let row_bottom = (radio_y + RADIO_SIZE).max(caption_y + CAPTION_LINE_H * 3.0);
        let mut row = pane.spawn((
            SightRadio(mode),
            Button,
            Hovered::default(),
            Pickable::default(),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(radio_x),
                top: Val::Px(row_top),
                width: Val::Px(caption_x + CAPTION_W - radio_x),
                height: Val::Px(row_bottom - row_top),
                ..default()
            },
        ));
        // A radio is idempotent: clicking the selected one re-selects it. No
        // `live` guard like the Setting pane's toggles, because all three
        // modes have a consumer (`camera::sight_axes`) — that was the
        // precondition for building this pane at all.
        row.observe(
            move |_activate: On<Activate>, mut options: ResMut<GameOptions>| {
                options.camera.sight = mode;
            },
        );
        row.with_children(|r| {
            let image = if options.camera.sight == mode {
                on.clone()
            } else {
                off.clone()
            };
            r.spawn((
                SightRadio(mode),
                ImageNode { image, ..default() },
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    top: Val::Px(radio_y - row_top),
                    width: Val::Px(RADIO_SIZE),
                    height: Val::Px(RADIO_SIZE),
                    ..default()
                },
                Pickable::IGNORE,
            ));

            let caption_left = caption_x - radio_x;
            r.spawn((
                Text::new(
                    ui_strings
                        .get_or(strings.title.0, strings.title.1)
                        .to_string(),
                ),
                TextFont {
                    font: font.clone().into(),
                    font_size: FontSize::Px(12.0),
                    ..default()
                },
                TextColor(TITLE_COLOR),
                caption_node(caption_left, caption_y - row_top),
                Pickable::IGNORE,
            ));
            for (line, (key, english)) in strings.desc.iter().enumerate() {
                r.spawn((
                    Text::new(ui_strings.get_or(key, english).to_string()),
                    TextFont {
                        font: font.clone().into(),
                        font_size: FontSize::Px(10.0),
                        ..default()
                    },
                    TextColor(DESC_COLOR),
                    caption_node(
                        caption_left,
                        caption_y - row_top + CAPTION_LINE_H * (line as f32 + 1.0),
                    ),
                    Pickable::IGNORE,
                ));
            }
        });
    }
}

/// Line pitch inside a caption. Vanilla's `CIFPML` is 282x30 for a single
/// markup line; we stack three shorter ones in the same column, which is the
/// deviation the module comment states.
const CAPTION_LINE_H: f32 = 13.0;

fn caption_node(left: f32, top: f32) -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: Val::Px(left),
        top: Val::Px(top),
        width: Val::Px(CAPTION_W),
        ..default()
    }
}

/// Repaint the three boxes from `GameOptions`, so the selection follows a
/// change made anywhere — the radio itself, a loaded `user_settings.yaml`, or a
/// future hotkey (`UIIT_STT_SIGHTCHANGE`, "Change View", exists in the string
/// table but has no binding in this change).
pub(crate) fn refresh_sight_radios(
    options: Res<GameOptions>,
    asset_server: Res<AssetServer>,
    mut boxes: Query<(&SightRadio, &mut ImageNode)>,
) {
    // Mirrors `refresh_game_toggles`: repaint only on a change, so the pane
    // costs nothing per frame while nobody is touching it.
    if !options.is_changed() {
        return;
    }
    for (radio, mut image) in &mut boxes {
        let art = if options.camera.sight == radio.0 {
            RADIO_ON
        } else {
            RADIO_OFF
        };
        image.image = asset_server.load(art);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every rect here is a transcription of `ifoption_camera.txt`, so the
    /// three columns must keep the file's own pitch and alignment. If someone
    /// "tidies" a number, this fails rather than the pane quietly drifting.
    #[test]
    fn the_three_rows_keep_the_trees_rects() {
        assert_eq!(RADIO_XY, [(25.0, 45.0), (25.0, 98.0), (25.0, 152.0)]);
        assert_eq!(ART_XY, [(12.0, 29.0), (12.0, 83.0), (12.0, 137.0)]);
        assert_eq!(CAPTION_XY, [(57.0, 38.0), (57.0, 92.0), (57.0, 146.0)]);

        // The columns share an origin x, and the caption sits a constant 9px
        // below its illustration in all three rows.
        for index in 0..3 {
            assert_eq!(RADIO_XY[index].0, 25.0);
            assert_eq!(ART_XY[index].0, 12.0);
            assert_eq!(CAPTION_XY[index].0, 57.0);
            assert_eq!(CAPTION_XY[index].1 - ART_XY[index].1, 9.0);
        }

        // The rest of the layout is NOT regular, and the irregularity is
        // preserved rather than tidied: the radio sits 16px below its art in
        // the first row and 15px in the other two, and the row pitch is 53
        // then 54. Both are the original's own off-by-ones (`ifoption_camera.txt`
        // ids 7-12); rounding them to one number would be a redesign of a
        // transcription, which is exactly what ADR-0009 asks us not to do
        // silently.
        let radio_over_art: Vec<f32> = (0..3).map(|i| RADIO_XY[i].1 - ART_XY[i].1).collect();
        assert_eq!(radio_over_art, vec![16.0, 15.0, 15.0]);
        assert_eq!(RADIO_XY[1].1 - RADIO_XY[0].1, 53.0);
        assert_eq!(RADIO_XY[2].1 - RADIO_XY[1].1, 54.0);
    }

    /// The pane's three rows must be the three modes, in the tree's top-to-
    /// bottom order — the file lists its blocks bottom-up (id 15 first), which
    /// is exactly the kind of thing a transcription gets backwards.
    #[test]
    fn rows_run_top_to_bottom_in_sight_mode_order() {
        assert_eq!(
            SightMode::ALL,
            [SightMode::Free, SightMode::ThirdPerson, SightMode::Quarter]
        );
        let ys: Vec<f32> = RADIO_XY.iter().map(|(_, y)| *y).collect();
        assert!(ys.windows(2).all(|w| w[0] < w[1]), "rows are out of order");
    }

    /// The whole point of the pane is that each row says what its mode does.
    /// A row with a blank line is a row that ships a control with no
    /// explanation, which is how the original's own copy gets lost.
    #[test]
    fn every_row_carries_the_originals_title_and_both_description_lines() {
        assert_eq!(MODE_STRINGS.len(), SightMode::ALL.len());
        for strings in &MODE_STRINGS {
            assert!(strings.title.0.starts_with("UIIT_STT_SIGHT_"));
            assert!(!strings.title.1.is_empty());
            for (key, english) in &strings.desc {
                assert!(
                    key.ends_with("_DESC1") || key.ends_with("_DESC2"),
                    "{key} is not one of the pane's two description lines"
                );
                assert!(!english.is_empty(), "{key} has no English fallback");
            }
        }
    }
}
