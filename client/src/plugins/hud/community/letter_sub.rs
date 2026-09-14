//! The mail read + write sub-windows — `ifletterread.txt` / `ifletterwrite.txt`.
//!
//! Idea: `ifletter.txt`'s `LetterRead` and `LetterWrite` sections declare
//! nothing but a **host**: `GDR_LETTER_READ:CIFLetterRead` (id 55) and
//! `GDR_LETTER_WRITE:CIFLetterWrite` (id 51), both `Rect="0,0,439,271"` with
//! `DDJ="interface\messagebox\msgbox2_window_"`. That DDJ is the shared modal
//! plate ([`crate::plugins::hud::modal_dialog`]), so these are dialogs over
//! the community shell rather than pages inside it — and their contents live
//! in their own trees, `resinfo/ifletterread.txt` (236 ln) and
//! `ifletterwrite.txt` (217 ln), whose rects are plate-local and are
//! transcribed verbatim below.
//!
//! Both trees are the same skeleton: a one-line header field and a body box,
//! each a `com_bg_tile_*` fill behind a `com_blacksquare_` plate, plus three
//! (read) / two (write) `com_button.ddj` buttons on one 88-unit pitch at
//! y=234. Read is SENDER + CONTENTS + Reply/Delete/Close; write is RECEIVER +
//! CONTENTS + Send/Cancel.
//!
//! Three absences are load-bearing (`docs/re/ui/mail-letter-window.md` §3, §9)
//! and are **deliberately not filled in**:
//!
//! * **No subject and no attachment slot** exist in either tree — v1.188 mail
//!   is sender + body. Nothing here adds either.
//! * **Read/unread state is UNKNOWN**: no control for it exists anywhere in
//!   the mail trees, so no badge, bold row or colour swap is invented.
//! * **Send is inert.** `0x7309 MailSendRequest` is in the `packets!` macro,
//!   but `packets/src/agent/mail.rs` states it is spec-derived and *not*
//!   capture-verified, and its response `0xB309` is unwired pending
//!   `packet_dump/0xb309.log`. Firing an unverified request at a real server
//!   with no response path is worse than a button that does nothing, so Send
//!   is drawn and does nothing until that dump exists. Delete is inert for the
//!   same reason. Reply, Cancel and Close are pure UI and do work.

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::text::{EditableText, TextCursorStyle};
use bevy::ui::UiTargetCamera;
use bevy::ui_widgets::{Activate, Button};

use crate::assets::FontAssets;
use crate::plugins::hud::community::model::{CommunityState, LetterSubWindow};
use crate::plugins::hud::game_window::abs_node;
use crate::plugins::hud::modal_dialog::{MODAL_BOTTOM, MODAL_SCRIM, MODAL_SIDE, MODAL_TOP};
use crate::plugins::hud::scale::hud_scale;
use crate::plugins::textdata::ClientUiStrings;
use crate::plugins::ui_v2::style::ImageButtonStyle;

// --- Layout constants (resinfo/ifletterread.txt + ifletterwrite.txt) --------

/// Host rect of both sub-windows (`ifletter.txt` `GDR_LETTER_READ` id 55 and
/// `GDR_LETTER_WRITE` id 51, both `Rect="0,0,439,271"`).
const PLATE_SIZE: (f32, f32) = (439.0, 271.0);
const PLATE_DIR: &str = "media://interface/messagebox/msgbox2_window_";

/// `GDR_LETTER_*_BG_1:CIFNormalTile` — the header field's tiled backing.
const HEADER_BG_RECT: (f32, f32, f32, f32) = (16.0, 40.0, 407.0, 59.0);
/// `GDR_LETTER_*_BG_2:CIFNormalTile` — the button strip's tiled backing.
const BUTTONS_BG_RECT: (f32, f32, f32, f32) = (16.0, 224.0, 407.0, 31.0);
/// `GDR_LETTER_*_BG_3` and `_BG_4:CIFNormalTile` — the two inner fields.
const NAME_BG_RECT: (f32, f32, f32, f32) = (98.0, 58.0, 247.0, 17.0);
const BODY_BG_RECT: (f32, f32, f32, f32) = (19.0, 103.0, 402.0, 117.0);
const BG_TILE_B: &str = "media://interface/ifcommon/bg_tile/com_bg_tile_b.ddj";
const BG_TILE_E: &str = "media://interface/ifcommon/bg_tile/com_bg_tile_e.ddj";
/// `GDR_LETTER_*_BLACKSQUARE_1` and `_2:CIFStretchWnd` — the fields' plates.
const NAME_PLATE_RECT: (f32, f32, f32, f32) = (94.0, 54.0, 255.0, 25.0);
const BODY_PLATE_RECT: (f32, f32, f32, f32) = (15.0, 99.0, 410.0, 125.0);
const BLACKSQUARE_PIECE: f32 = 4.0;
const BLACKSQUARE_DIR: &str = "media://interface/ifcommon/com_blacksquare_";
/// `GDR_LETTER_READ_SENDER_STA` / `GDR_LETTER_WRITE_RECEIVER_STA:CIFStatic`,
/// the field caption — the same rect in both trees.
const CAPTION_RECT: (f32, f32, f32, f32) = (31.0, 60.0, 59.0, 15.0);
/// `GDR_LETTER_READ_SENDER:CIFStatic` (`110,60,170,15`) and
/// `GDR_LETTER_WRITE_RECEIVER:CIFEdit` (`110,56,170,20`) — the write side is
/// an edit box, which is why its rect is the taller of the two.
const SENDER_RECT: (f32, f32, f32, f32) = (110.0, 60.0, 170.0, 15.0);
const RECEIVER_RECT: (f32, f32, f32, f32) = (110.0, 56.0, 170.0, 20.0);
/// `GDR_LETTER_*_CONTENTS:CIFEdit`, byte-identical in both trees.
const CONTENTS_RECT: (f32, f32, f32, f32) = (16.0, 100.0, 408.0, 123.0);
/// Both trees put their buttons on one row at y=234 with an 88-unit pitch:
/// read `94/182/270`, write `140/228`. The blocks carry `Rect` w,h = `0,0`,
/// so the size is the art's: `com_button.ddj` is 76x24 (DDS header — the same
/// measurement #597 makes).
const BUTTON_Y: f32 = 234.0;
const BUTTON_SIZE: (f32, f32) = (76.0, 24.0);
const READ_BUTTON_XS: [f32; 3] = [94.0, 182.0, 270.0];
const WRITE_BUTTON_XS: [f32; 2] = [140.0, 228.0];

const LABEL_COLOR: Color = Color::srgb(0.92, 0.92, 0.92);
const FIELD_FONT: f32 = 8.0;

// --- Markers ----------------------------------------------------------------

/// One of the buttons of either sub-window, named after its `GDR_*` block.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub enum LetterSubButton {
    /// `GDR_LETTER_READ_REPLY_BTN` — opens the write window. Pure UI.
    Reply,
    /// `GDR_LETTER_READ_DELETE_BTN` — inert: it needs the unverified 0x7309
    /// mail family (module doc).
    Delete,
    /// `GDR_LETTER_READ_CLOSE_BTN`.
    Close,
    /// `GDR_LETTER_WRITE_SEND_BTN` — inert, same reason as [`Self::Delete`].
    Send,
    /// `GDR_LETTER_WRITE_CANCEL_BTN`.
    Cancel,
}

/// Root of whichever sub-window is currently up.
#[derive(Component)]
pub struct LetterSubRoot;

/// The write window's addressee field (`GDR_LETTER_WRITE_RECEIVER`), so the
/// wire slice that eventually sends has something to read.
#[derive(Component)]
pub struct LetterReceiverInput;

/// The write window's body field (`GDR_LETTER_WRITE_CONTENTS`).
#[derive(Component)]
pub struct LetterContentsInput;

// --- Spawning ---------------------------------------------------------------

/// Spawn / despawn the sub-window when [`CommunityState::letter_sub`] changes.
/// One reactive system rather than two hidden trees: the two windows share a
/// rect and are mutually exclusive in the data, so only one can ever be up.
pub fn apply_letter_sub_window(
    state: Res<CommunityState>,
    existing: Query<Entity, With<LetterSubRoot>>,
    asset_server: Res<AssetServer>,
    fonts: Res<FontAssets>,
    ui_strings: Res<ClientUiStrings>,
    cam_query: Query<Entity, With<Camera2d>>,
    mut commands: Commands,
) {
    if !state.is_changed() {
        return;
    }
    for entity in existing.iter() {
        commands.entity(entity).despawn();
    }
    let window = match state.letter_sub {
        LetterSubWindow::None => return,
        window => window,
    };
    // The sub-windows belong to the shell: closing the community window has to
    // take them with it rather than leave a modal floating over the game.
    if !state.open {
        return;
    }
    let Ok(camera) = cam_query.single() else {
        warn!("letter sub-window: no 2d camera to attach to");
        return;
    };
    let s = hud_scale();
    let text_font = |size: f32| TextFont {
        font: fonts.two.clone().into(),
        font_size: FontSize::Px(size * s),
        ..default()
    };
    let button_style = ImageButtonStyle {
        normal: asset_server.load("media://interface/ifcommon/com_button.ddj"),
        hover: asset_server.load("media://interface/ifcommon/com_button_focus.ddj"),
        press: asset_server.load("media://interface/ifcommon/com_button_press.ddj"),
        // No `disable` art: neither sub-window disables a button today (Send
        // and Delete are inert but drawn normally, per #654's acceptance 4).
        // The shared updater's `normal` fallback (#640) covers it.
        ..default()
    };
    let image = |rect: (f32, f32, f32, f32), path: String| {
        (
            abs_node(rect, s),
            ImageNode {
                image: asset_server.load(path),
                image_mode: NodeImageMode::Stretch,
                ..default()
            },
            Pickable::IGNORE,
        )
    };
    let tile = |rect: (f32, f32, f32, f32), path: &'static str| {
        (
            abs_node(rect, s),
            ImageNode {
                image: asset_server.load(path),
                image_mode: NodeImageMode::Tiled {
                    tile_x: true,
                    tile_y: true,
                    stretch_value: s,
                },
                ..default()
            },
            Pickable::IGNORE,
        )
    };

    let (caption_key, caption_fallback) = match window {
        LetterSubWindow::Write => ("UIIT_STT_LETTER_ADDRESEE", "To"),
        _ => ("UIIT_STT_LETTER_SENDER", "Sender"),
    };

    commands
        .spawn((
            LetterSubRoot,
            Name::from("Letter Sub Window"),
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(MODAL_SCRIM),
            // Above the community shell's own GlobalZIndex(55).
            GlobalZIndex(60),
            UiTargetCamera(camera),
        ))
        .with_children(|scrim| {
            scrim
                .spawn((
                    Node {
                        width: Val::Px(PLATE_SIZE.0 * s),
                        height: Val::Px(PLATE_SIZE.1 * s),
                        ..default()
                    },
                    Pickable::default(),
                ))
                .with_children(|plate| {
                    // the msgbox2_window_ ring: 16px sides, a 40px top strip
                    // and a 16px bottom (modal_dialog's measured insets)
                    for ((x, y, w, h), piece) in plate_ring(PLATE_SIZE.0, PLATE_SIZE.1) {
                        plate.spawn(image((x, y, w, h), format!("{PLATE_DIR}{piece}.ddj")));
                    }
                    for (rect, art) in [
                        (HEADER_BG_RECT, BG_TILE_B),
                        (BUTTONS_BG_RECT, BG_TILE_B),
                        (NAME_BG_RECT, BG_TILE_E),
                        (BODY_BG_RECT, BG_TILE_E),
                    ] {
                        plate.spawn(tile(rect, art));
                    }
                    for rect in [NAME_PLATE_RECT, BODY_PLATE_RECT] {
                        for ((x, y, w, h), piece) in blacksquare(rect) {
                            plate.spawn(image(
                                (x, y, w, h),
                                format!("{BLACKSQUARE_DIR}{piece}.ddj"),
                            ));
                        }
                    }

                    // the field caption ("Sender" / "To")
                    plate.spawn((
                        Text::new(ui_strings.get_or(caption_key, caption_fallback).to_string()),
                        text_font(FIELD_FONT),
                        TextColor(LABEL_COLOR),
                        TextLayout::justify(Justify::Left),
                        abs_node(CAPTION_RECT, s),
                        Pickable::IGNORE,
                    ));

                    match window {
                        LetterSubWindow::Write => {
                            spawn_input(
                                plate,
                                LetterReceiverInput,
                                RECEIVER_RECT,
                                s,
                                text_font(FIELD_FONT),
                                true,
                            );
                            spawn_input(
                                plate,
                                LetterContentsInput,
                                CONTENTS_RECT,
                                s,
                                text_font(FIELD_FONT),
                                false,
                            );
                        }
                        _ => {
                            // Read is a *view*: the sender and body are
                            // statics in the tree (CIFStatic / a read-only
                            // CIFEdit) and stay empty until a mail list
                            // exists on the wire.
                            plate.spawn((
                                Text::new(""),
                                text_font(FIELD_FONT),
                                TextColor(LABEL_COLOR),
                                TextLayout::justify(Justify::Left),
                                abs_node(SENDER_RECT, s),
                                Pickable::IGNORE,
                            ));
                            plate.spawn((
                                Text::new(""),
                                text_font(FIELD_FONT),
                                TextColor(LABEL_COLOR),
                                TextLayout::justify(Justify::Left),
                                abs_node(CONTENTS_RECT, s),
                                Pickable::IGNORE,
                            ));
                        }
                    }

                    for (button, x, key, fallback) in buttons(window) {
                        plate
                            .spawn((
                                button,
                                Button,
                                Hovered::default(),
                                abs_node((x, BUTTON_Y, BUTTON_SIZE.0, BUTTON_SIZE.1), s),
                                ImageNode {
                                    image: button_style.normal.clone(),
                                    image_mode: NodeImageMode::Stretch,
                                    ..default()
                                },
                                button_style.clone(),
                            ))
                            .observe(on_letter_sub_button)
                            .with_children(|b| {
                                b.spawn((
                                    Text::new(ui_strings.get_or(key, fallback).to_string()),
                                    text_font(FIELD_FONT),
                                    TextColor(LABEL_COLOR),
                                    TextLayout::justify(Justify::Center),
                                    Node {
                                        position_type: PositionType::Absolute,
                                        top: Val::Px(6.0 * s),
                                        width: Val::Percent(100.0),
                                        ..default()
                                    },
                                    Pickable::IGNORE,
                                ));
                            });
                    }
                });
        });
}

/// One editable field of the write window.
fn spawn_input(
    plate: &mut ChildSpawnerCommands,
    marker: impl Component,
    rect: (f32, f32, f32, f32),
    s: f32,
    font: TextFont,
    single_line: bool,
) {
    let mut node = abs_node(rect, s);
    node.padding = UiRect::all(Val::Px(2.0 * s));
    plate.spawn((
        marker,
        EditableText {
            visible_lines: if single_line { Some(1.0) } else { None },
            allow_newlines: !single_line,
            ..default()
        },
        node,
        font,
        TextColor(Color::WHITE),
        TextLayout::justify(Justify::Left),
        TextCursorStyle {
            color: Color::WHITE,
            ..default()
        },
    ));
}

/// The buttons a sub-window declares, left to right, with their `Text` keys.
fn buttons(window: LetterSubWindow) -> Vec<(LetterSubButton, f32, &'static str, &'static str)> {
    match window {
        LetterSubWindow::Write => vec![
            (
                LetterSubButton::Send,
                WRITE_BUTTON_XS[0],
                "UIIT_CTL_LETTER_SEND",
                "Send",
            ),
            (
                LetterSubButton::Cancel,
                WRITE_BUTTON_XS[1],
                "UIIS_CTL_CANCEL",
                "Cancel",
            ),
        ],
        _ => vec![
            (
                LetterSubButton::Reply,
                READ_BUTTON_XS[0],
                "UIIT_CTL_LETTER_REPLYLETTER",
                "Reply",
            ),
            (
                LetterSubButton::Delete,
                READ_BUTTON_XS[1],
                "UIIT_CTL_LETTER_DELETE",
                "Delete",
            ),
            (
                LetterSubButton::Close,
                READ_BUTTON_XS[2],
                "UIIT_CTL_LETTER_WINDOWSCLOSE",
                "Close",
            ),
        ],
    }
}

/// The 8 `msgbox2_window_` pieces around a `w x h` plate. The insets are the
/// art's own extents, stated once in [`crate::plugins::hud::modal_dialog`].
fn plate_ring(w: f32, h: f32) -> [((f32, f32, f32, f32), &'static str); 8] {
    let side = MODAL_SIDE;
    let top = MODAL_TOP;
    let bottom = MODAL_BOTTOM;
    [
        ((0.0, 0.0, side, top), "left_up"),
        ((side, 0.0, w - 2.0 * side, top), "mid_up"),
        ((w - side, 0.0, side, top), "right_up"),
        ((0.0, top, side, h - top - bottom), "left_side"),
        ((w - side, top, side, h - top - bottom), "right_side"),
        ((0.0, h - bottom, side, bottom), "left_down"),
        ((side, h - bottom, w - 2.0 * side, bottom), "mid_down"),
        ((w - side, h - bottom, side, bottom), "right_down"),
    ]
}

/// The 6 `com_blacksquare_` trim pieces around a field plate.
fn blacksquare(rect: (f32, f32, f32, f32)) -> [((f32, f32, f32, f32), &'static str); 6] {
    let (x, y, w, h) = rect;
    let p = BLACKSQUARE_PIECE;
    [
        ((x, y, p, p), "left_up"),
        ((x + w - p, y, p, p), "right_up"),
        ((x, y + p, p, h - 2.0 * p), "left_side"),
        ((x + w - p, y + p, p, h - 2.0 * p), "right_side"),
        ((x, y + h - p, p, p), "left_down"),
        ((x + w - p, y + h - p, p, p), "right_down"),
    ]
}

// --- Behavior ---------------------------------------------------------------

/// Reply, Cancel and Close move UI state; Send and Delete do nothing on
/// purpose — see the module doc for why a half-wired Send is worse than an
/// inert one.
fn on_letter_sub_button(
    activate: On<Activate>,
    buttons: Query<&LetterSubButton>,
    mut state: ResMut<CommunityState>,
) {
    let Ok(button) = buttons.get(activate.entity) else {
        return;
    };
    match button {
        LetterSubButton::Reply => state.letter_sub = LetterSubWindow::Write,
        LetterSubButton::Close | LetterSubButton::Cancel => {
            state.letter_sub = LetterSubWindow::None
        }
        LetterSubButton::Send | LetterSubButton::Delete => {
            info!("mail {button:?} is inert: 0xB309 has no packet_dump sample yet");
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// Every transcribed rect stays inside the `0,0,439,271` host both trees
    /// declare — a rect past it would mean a bad transcription.
    #[test]
    fn every_rect_fits_the_authored_host() {
        let mut boxes = vec![
            HEADER_BG_RECT,
            BUTTONS_BG_RECT,
            NAME_BG_RECT,
            BODY_BG_RECT,
            NAME_PLATE_RECT,
            BODY_PLATE_RECT,
            CAPTION_RECT,
            SENDER_RECT,
            RECEIVER_RECT,
            CONTENTS_RECT,
        ];
        for x in READ_BUTTON_XS.iter().chain(WRITE_BUTTON_XS.iter()) {
            boxes.push((*x, BUTTON_Y, BUTTON_SIZE.0, BUTTON_SIZE.1));
        }
        for (x, y, w, h) in boxes {
            assert!(
                x + w <= PLATE_SIZE.0,
                "rect {x},{y},{w},{h} overflows width"
            );
            assert!(
                y + h <= PLATE_SIZE.1,
                "rect {x},{y},{w},{h} overflows height"
            );
        }
    }

    /// Both trees put their buttons on one row with the same 88-unit pitch.
    #[test]
    fn both_button_rows_keep_the_vanilla_pitch() {
        for pair in READ_BUTTON_XS.windows(2) {
            assert_eq!(pair[1] - pair[0], 88.0);
        }
        assert_eq!(WRITE_BUTTON_XS[1] - WRITE_BUTTON_XS[0], 88.0);
        assert_eq!(BUTTON_SIZE, (76.0, 24.0), "com_button.ddj is 76x24");
    }

    /// The whole point of the ticket: neither sub-window grows a subject field
    /// or an attachment slot, and neither tree has one to grow it from.
    #[test]
    fn neither_sub_window_invents_a_subject_or_attachment() {
        for window in [LetterSubWindow::Read, LetterSubWindow::Write] {
            for (_, _, key, fallback) in buttons(window) {
                for word in ["ITEM", "GOLD", "TITLE", "SUBJECT"] {
                    assert!(!key.contains(word), "{key} looks like {word} control");
                }
                assert!(!fallback.is_empty());
            }
        }
    }

    /// Send and Delete are the two controls that would need the unverified
    /// 0x7309 family, so they are declared and must stay inert.
    #[test]
    fn send_and_delete_exist_but_carry_no_wire() {
        let write: Vec<LetterSubButton> = buttons(LetterSubWindow::Write)
            .into_iter()
            .map(|(b, ..)| b)
            .collect();
        assert_eq!(
            write,
            vec![LetterSubButton::Send, LetterSubButton::Cancel],
            "ifletterwrite.txt declares exactly Send + Cancel"
        );
        let read: Vec<LetterSubButton> = buttons(LetterSubWindow::Read)
            .into_iter()
            .map(|(b, ..)| b)
            .collect();
        assert_eq!(
            read,
            vec![
                LetterSubButton::Reply,
                LetterSubButton::Delete,
                LetterSubButton::Close
            ],
            "ifletterread.txt declares exactly Reply + Delete + Close"
        );
    }

    /// The plate ring covers the whole `439x271` host with no gap and no
    /// overlap, at the insets the art measures.
    #[test]
    fn the_plate_ring_covers_the_host_edges() {
        let ring = plate_ring(PLATE_SIZE.0, PLATE_SIZE.1);
        let widths: f32 = ring[..3].iter().map(|((_, _, w, _), _)| w).sum();
        assert_eq!(widths, PLATE_SIZE.0);
        let (_, _, _, top_h) = ring[0].0;
        let (_, side_y, _, side_h) = ring[3].0;
        let (_, down_y, _, down_h) = ring[5].0;
        assert_eq!(top_h + side_h + down_h, PLATE_SIZE.1);
        assert_eq!(side_y, MODAL_TOP);
        assert_eq!(down_y + down_h, PLATE_SIZE.1);
    }
}
