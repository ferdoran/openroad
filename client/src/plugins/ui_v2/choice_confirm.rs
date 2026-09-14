//! `GDR_CHECK_CONFIRM_WND:CIFCheckConfirmWnd` — the original's **generic**
//! two-option confirm (`ginterface.txt:1814`, id 101, `0,0,226,190`, art
//! `interface\messagebox\msgbox2_window_`; body `resinfo/ifcheckconfirmwnd.txt`,
//! 9 blocks). One of the two live registry entries no unit doc had ever
//! mentioned (#579, `docs/re/ui/registry-orphan-widgets.md` §4).
//!
//! Idea: the window is a *question with mutually exclusive answers*, not a
//! specific dialog. Both its labels are `Text=""`, so every prompt is
//! code-supplied per call site — which is why it sits at the top level of the
//! registry rather than inside a feature's tree, and why this module models it
//! as one widget with N options ([`ChoiceConfirmRequest`]) instead of two
//! hard-coded rows. It reuses the shared `msgbox2_window_` shell
//! ([`crate::plugins::hud::modal_dialog`]) exactly as the other dialogs do.
//!
//! **Deviation 1 — radio semantics *and* radio art.** The data paints two
//! `CIFCheckBox` controls with `com_radiobutton_off.ddj` and makes them
//! mutually exclusive under one OK button, i.e. the class says checkbox and
//! the behaviour is a radio group. We take the behaviour as the spec and draw
//! `com_radiobutton_on.ddj` for the selected row (16x16, the same size as the
//! authored `_off`, `frpvp-window.md` §92): a checkbox that behaves like a
//! radio is a defect of the original's authoring, not a look worth cloning.
//!
//! **Deviation 2 — N options.** The file has two rows at `62,66` and `62,96`;
//! rows here are generated on that measured 30 px pitch, so two options are
//! byte-for-byte the authored layout and a third would continue the run. The
//! plate grows with the rows rather than clipping them.
//!
//! **Deviation 3 — prompts are strings, not text keys.** The original's
//! captions are code-supplied at the call site; resolving `textuisystem`
//! is therefore the caller's job (it is the caller that knows which key its
//! question uses), and this widget takes the finished text.

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui::UiTargetCamera;
use bevy::ui_widgets::{Activate, Button};

use crate::assets::FontAssets;
use crate::plugins::hud::modal_dialog::{MODAL_BOTTOM, MODAL_SCRIM, MODAL_SIDE, MODAL_TOP};
use crate::plugins::hud::scale::hud_scale;
use crate::plugins::textdata::ClientUiStrings;

/// Registry rect `0,0,226,190` — the two-option size. Taller requests keep the
/// width and grow by [`ROW_PITCH`] per extra row (deviation 2).
const PLATE: (f32, f32) = (226.0, 190.0);
/// Rows the authored plate holds.
const AUTHORED_ROWS: usize = 2;

const PLATE_ART: &str = "media://interface/messagebox/msgbox2_window_";
const ART: &str = "media://interface/";

/// `GDR_GUILD_POSITION_GRANT_BG` `16,40,194,136` — `com_bg_tile_b.ddj` over
/// the plate interior (the block names are the guild dialog this file was
/// copied from; §4-1 — block-name prefixes are not evidence of a feature).
const BG_RECT: (f32, f32, f32, f32) = (16.0, 40.0, 194.0, 136.0);
/// `_BLACKSQUARE` `23,48,180,75` — the `com_blacksquare_` well.
const BLACK_RECT: (f32, f32, f32, f32) = (23.0, 48.0, 180.0, 75.0);
/// `_TILE_1` `25,50,176,71` — `com_bg_tile_e.ddj` inside that well.
const TILE_RECT: (f32, f32, f32, f32) = (25.0, 50.0, 176.0, 71.0);
/// `GDR_CHECK_CONFIRM_BTN1` `62,66,16,16`; `BTN2` is the same x at y 96.
const TOGGLE_ORIGIN: (f32, f32) = (62.0, 66.0);
const TOGGLE: f32 = 16.0;
/// `GDR_CHECK_CONFIRM_STA1` `88,67,110,15` / `STA2` `88,97,110,15`.
const LABEL_ORIGIN: (f32, f32) = (88.0, 67.0);
const LABEL: (f32, f32) = (110.0, 15.0);
/// `96 - 66` — the authored row pitch, shared by both runs (`97 - 67`).
const ROW_PITCH: f32 = 30.0;
/// `_BTN_OK` `31,150,0,0` / `_BTN_CANCEL` `119,150,0,0`. The rects are
/// degenerate, so the size is the art's: `com_button.ddj` measures **76x24**
/// (`options-screen.md`; #597 corrects our old 76x22 for the same file).
const OK_XY: (f32, f32) = (31.0, 150.0);
const CANCEL_XY: (f32, f32) = (119.0, 150.0);
const BUTTON: (f32, f32) = (76.0, 24.0);

// The one config-flagged HUD scale (#714) — no module declares its own.

/// A question with mutually exclusive answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChoiceConfirmRequest {
    /// Identifies the asking call site in [`ChoiceConfirmed`]; the widget
    /// never interprets it.
    pub tag: &'static str,
    /// The finished prompt text (deviation 3).
    pub prompt: String,
    /// The finished option captions, in row order.
    pub options: Vec<String>,
}

/// The one open question, if any. A resource rather than a message because the
/// dialog is modal: a second question while one is up would have nowhere to go.
#[derive(Resource, Default)]
pub struct ChoiceConfirmState {
    pub request: Option<ChoiceConfirmRequest>,
    /// Row index of the currently ticked option.
    pub selected: usize,
}

impl ChoiceConfirmState {
    /// Raise the dialog with `request`, pre-selecting the first row (the
    /// original's OK is always live, so some row must be the answer).
    pub fn ask(&mut self, request: ChoiceConfirmRequest) {
        self.selected = 0;
        self.request = Some(request);
    }

    /// Close without answering.
    pub fn dismiss(&mut self) {
        self.request = None;
    }
}

/// The answer, once OK is pressed. Cancel emits nothing.
#[derive(Message, Clone, Debug, PartialEq, Eq)]
pub struct ChoiceConfirmed {
    pub tag: &'static str,
    pub option: usize,
}

#[derive(Component)]
pub struct ChoiceConfirmDialog;

/// One option row's toggle; the index is the row.
#[derive(Component)]
pub struct ChoiceConfirmToggle(pub usize);

/// Height of a plate holding `rows` options: the authored 190 plus one pitch
/// per row beyond the authored two (deviation 2).
fn plate_height(rows: usize) -> f32 {
    PLATE.1 + ROW_PITCH * rows.saturating_sub(AUTHORED_ROWS) as f32
}

/// Row `i`'s toggle rect, plate-local.
fn toggle_rect(row: usize) -> (f32, f32, f32, f32) {
    (
        TOGGLE_ORIGIN.0,
        TOGGLE_ORIGIN.1 + ROW_PITCH * row as f32,
        TOGGLE,
        TOGGLE,
    )
}

/// Row `i`'s label rect, plate-local.
fn label_rect(row: usize) -> (f32, f32, f32, f32) {
    (
        LABEL_ORIGIN.0,
        LABEL_ORIGIN.1 + ROW_PITCH * row as f32,
        LABEL.0,
        LABEL.1,
    )
}

/// The footer buttons sit relative to the plate bottom, so extra rows push
/// them down with the plate instead of landing on the last option.
fn footer_rect((x, y): (f32, f32), rows: usize) -> (f32, f32, f32, f32) {
    (x, y + (plate_height(rows) - PLATE.1), BUTTON.0, BUTTON.1)
}

fn plate_node((x, y, w, h): (f32, f32, f32, f32)) -> Node {
    let s = hud_scale();
    Node {
        position_type: PositionType::Absolute,
        left: Val::Px(x * s),
        top: Val::Px(y * s),
        width: Val::Px(w * s),
        height: Val::Px(h * s),
        ..default()
    }
}

fn toggle_art(selected: bool) -> String {
    // deviation 1: the selected row gets the `_on` frame the archive ships
    let frame = if selected { "on" } else { "off" };
    format!("{ART}ifcommon/com_radiobutton_{frame}.ddj")
}

/// Spawn/despawn the dialog to match [`ChoiceConfirmState`].
pub fn sync_choice_confirm(
    state: Res<ChoiceConfirmState>,
    ui_strings: Res<ClientUiStrings>,
    fonts: Res<FontAssets>,
    asset_server: Res<AssetServer>,
    cameras: Query<Entity, With<Camera2d>>,
    open: Query<Entity, With<ChoiceConfirmDialog>>,
    mut commands: Commands,
) {
    if !state.is_changed() {
        return;
    }
    for entity in open.iter() {
        commands.entity(entity).despawn();
    }
    let Some(request) = state.request.as_ref() else {
        return;
    };
    let Some(camera) = cameras.iter().next() else {
        return;
    };

    let s = hud_scale();
    let rows = request.options.len().max(1);
    let (pw, ph) = (PLATE.0, plate_height(rows));
    let text_font = TextFont {
        font: fonts.nine.clone().into(),
        font_size: FontSize::Px(9.0 * s),
        ..default()
    };
    let img = |rect: (f32, f32, f32, f32), path: String| {
        (
            plate_node(rect),
            ImageNode {
                image: asset_server.load(path),
                image_mode: NodeImageMode::Stretch,
                ..default()
            },
            Pickable::IGNORE,
        )
    };

    // Captured out of the button loop below so the root can point Enter at the
    // OK button (`hud::focus::HudDialog`).
    let mut ok_button = None;
    let root = commands
        .spawn((
            ChoiceConfirmDialog,
            Name::from("Choice Confirm"),
            UiTargetCamera(camera),
            GlobalZIndex(90),
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            BackgroundColor(MODAL_SCRIM),
        ))
        .with_children(|scrim| {
            scrim
                .spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        width: Val::Px(pw * s),
                        height: Val::Px(ph * s),
                        margin: UiRect::all(Val::Auto),
                        ..default()
                    },
                    Pickable::IGNORE,
                ))
                .with_children(|plate| {
                    for ((x, y, w, h), piece) in [
                        ((0.0, 0.0, MODAL_SIDE, MODAL_TOP), "left_up"),
                        (
                            (MODAL_SIDE, 0.0, pw - 2.0 * MODAL_SIDE, MODAL_TOP),
                            "mid_up",
                        ),
                        ((pw - MODAL_SIDE, 0.0, MODAL_SIDE, MODAL_TOP), "right_up"),
                        (
                            (0.0, MODAL_TOP, MODAL_SIDE, ph - MODAL_TOP - MODAL_BOTTOM),
                            "left_side",
                        ),
                        (
                            (
                                pw - MODAL_SIDE,
                                MODAL_TOP,
                                MODAL_SIDE,
                                ph - MODAL_TOP - MODAL_BOTTOM,
                            ),
                            "right_side",
                        ),
                        (
                            (0.0, ph - MODAL_BOTTOM, MODAL_SIDE, MODAL_BOTTOM),
                            "left_down",
                        ),
                        (
                            (
                                MODAL_SIDE,
                                ph - MODAL_BOTTOM,
                                pw - 2.0 * MODAL_SIDE,
                                MODAL_BOTTOM,
                            ),
                            "mid_down",
                        ),
                        (
                            (pw - MODAL_SIDE, ph - MODAL_BOTTOM, MODAL_SIDE, MODAL_BOTTOM),
                            "right_down",
                        ),
                    ] {
                        plate.spawn(img((x, y, w, h), format!("{PLATE_ART}{piece}.ddj")));
                    }

                    // the three background layers, grown with the rows
                    let grown = |(x, y, w, h): (f32, f32, f32, f32)| (x, y, w, h + (ph - PLATE.1));
                    plate.spawn(img(
                        grown(BG_RECT),
                        format!("{ART}ifcommon/bg_tile/com_bg_tile_b.ddj"),
                    ));
                    plate.spawn((
                        plate_node(grown(BLACK_RECT)),
                        BackgroundColor(Color::BLACK),
                        Pickable::IGNORE,
                    ));
                    plate.spawn(img(
                        grown(TILE_RECT),
                        format!("{ART}ifcommon/bg_tile/com_bg_tile_e.ddj"),
                    ));

                    // the prompt sits in the well above the first row
                    plate.spawn((
                        Text::new(request.prompt.clone()),
                        text_font.clone(),
                        TextColor(Color::WHITE),
                        TextLayout::justify(Justify::Center),
                        plate_node((
                            TILE_RECT.0,
                            TILE_RECT.1,
                            TILE_RECT.2,
                            TOGGLE_ORIGIN.1 - TILE_RECT.1,
                        )),
                        Pickable::IGNORE,
                    ));

                    for (row, option) in request.options.iter().enumerate() {
                        plate
                            .spawn((
                                ChoiceConfirmToggle(row),
                                Button,
                                Hovered::default(),
                                plate_node(toggle_rect(row)),
                                ImageNode {
                                    image: asset_server.load(toggle_art(row == state.selected)),
                                    image_mode: NodeImageMode::Stretch,
                                    ..default()
                                },
                            ))
                            .observe(on_toggle);
                        plate.spawn((
                            Text::new(option.clone()),
                            text_font.clone(),
                            TextColor(Color::WHITE),
                            plate_node(label_rect(row)),
                            Pickable::IGNORE,
                        ));
                    }

                    for (xy, key, fallback, is_ok) in [
                        (OK_XY, "UIIS_CTL_CONFIRM", "Confirm", true),
                        (CANCEL_XY, "UIIS_CTL_CANCEL", "Cancel", false),
                    ] {
                        let mut button = plate.spawn((
                            Button,
                            Hovered::default(),
                            plate_node(footer_rect(xy, rows)),
                            ImageNode {
                                image: asset_server.load(format!("{ART}ifcommon/com_button.ddj")),
                                image_mode: NodeImageMode::Stretch,
                                ..default()
                            },
                        ));
                        if is_ok {
                            button.observe(on_ok);
                            ok_button = Some(button.id());
                        } else {
                            button.observe(on_cancel);
                        }
                        button.with_children(|button| {
                            button.spawn((
                                Text::new(ui_strings.get_or(key, fallback).to_string()),
                                text_font.clone(),
                                TextColor(Color::WHITE),
                                TextLayout::justify(Justify::Center),
                                Node {
                                    width: Val::Percent(100.0),
                                    align_self: AlignSelf::Center,
                                    ..default()
                                },
                                Pickable::IGNORE,
                            ));
                        });
                    }
                });
        })
        .id();
    if let Some(confirm_button) = ok_button {
        commands
            .entity(root)
            .insert(crate::plugins::hud::focus::HudDialog { confirm_button });
    }
}

/// Ticking a row is the whole interaction: one `T`, so selecting one row
/// deselects every other by construction (deviation 1).
fn on_toggle(
    activate: On<Activate>,
    toggles: Query<&ChoiceConfirmToggle>,
    mut state: ResMut<ChoiceConfirmState>,
) {
    if let Ok(toggle) = toggles.get(activate.entity) {
        state.selected = toggle.0;
    }
}

fn on_ok(
    _: On<Activate>,
    mut state: ResMut<ChoiceConfirmState>,
    mut answered: MessageWriter<ChoiceConfirmed>,
) {
    let Some(request) = state.request.take() else {
        return;
    };
    answered.write(ChoiceConfirmed {
        tag: request.tag,
        option: state.selected.min(request.options.len().saturating_sub(1)),
    });
}

fn on_cancel(_: On<Activate>, mut state: ResMut<ChoiceConfirmState>) {
    state.dismiss();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every rect is the file's, verbatim, for the authored two-row case
    /// (`ifcheckconfirmwnd.txt`, transcribed in
    /// `docs/re/ui/registry-orphan-widgets.md` §4).
    #[test]
    fn the_two_row_layout_is_the_authored_one() {
        assert_eq!(PLATE, (226.0, 190.0));
        assert_eq!(plate_height(AUTHORED_ROWS), PLATE.1);
        assert_eq!(toggle_rect(0), (62.0, 66.0, 16.0, 16.0));
        assert_eq!(toggle_rect(1), (62.0, 96.0, 16.0, 16.0));
        assert_eq!(label_rect(0), (88.0, 67.0, 110.0, 15.0));
        assert_eq!(label_rect(1), (88.0, 97.0, 110.0, 15.0));
        assert_eq!(footer_rect(OK_XY, 2), (31.0, 150.0, 76.0, 24.0));
        assert_eq!(footer_rect(CANCEL_XY, 2), (119.0, 150.0, 76.0, 24.0));
    }

    /// Deviation 2: a third option continues the authored 30px run and grows
    /// the plate by exactly one pitch, so the footer never lands on a row.
    #[test]
    fn extra_options_extend_the_authored_pitch_and_grow_the_plate() {
        assert_eq!(plate_height(3), PLATE.1 + ROW_PITCH);
        assert_eq!(toggle_rect(2).1, toggle_rect(1).1 + ROW_PITCH);
        let (_, footer_y, _, footer_h) = footer_rect(OK_XY, 3);
        let (_, last_row_y, _, last_row_h) = toggle_rect(2);
        assert!(footer_y >= last_row_y + last_row_h, "footer overlaps row 3");
        assert!(footer_y + footer_h <= plate_height(3));
    }

    /// Everything the dialog draws stays inside its plate, at both sizes.
    #[test]
    fn every_rect_fits_its_plate() {
        for rows in [AUTHORED_ROWS, 5] {
            let height = plate_height(rows);
            let mut rects = vec![footer_rect(OK_XY, rows), footer_rect(CANCEL_XY, rows)];
            for row in 0..rows {
                rects.push(toggle_rect(row));
                rects.push(label_rect(row));
            }
            for (x, y, w, h) in rects {
                assert!(x + w <= PLATE.0, "{x}+{w} overflows the plate width");
                assert!(y + h <= height, "{y}+{h} overflows the {rows}-row plate");
            }
        }
    }

    /// Deviation 1: the class is `CIFCheckBox` and the art is
    /// `com_radiobutton_off.ddj`, but the behaviour is a radio group — so the
    /// ticked row draws the `_on` frame and only one row is ever ticked.
    #[test]
    fn selecting_a_row_is_exclusive_and_draws_the_on_frame() {
        let mut state = ChoiceConfirmState::default();
        state.ask(ChoiceConfirmRequest {
            tag: "test",
            prompt: "Which one?".into(),
            options: vec!["A".into(), "B".into()],
        });
        assert_eq!(state.selected, 0);
        state.selected = 1;
        assert!(toggle_art(true).ends_with("com_radiobutton_on.ddj"));
        assert!(toggle_art(false).ends_with("com_radiobutton_off.ddj"));
        // one index means mutual exclusivity is not something the rows can
        // get wrong
        assert_eq!((0..2).filter(|row| *row == state.selected).count(), 1);
    }
}
