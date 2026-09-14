//! The mail (letter) list page — page 14 of the community shell.
//!
//! Idea: transcribed from the user's own `resinfo/ifletter.txt`, whose
//! `Create` + `LetterList` + `CommandButton` sections *are* the list page
//! (its other three sections — `LetterWrite`, `LetterRead`, `MultiLetter` —
//! are hosts for separate sub-windows and are not built here). All rects are
//! page-local, i.e. relative to the `13,61,451,320` page rect the six
//! community pages share.
//!
//! Two absences are load-bearing and deliberate
//! (`docs/re/ui/mail-letter-window.md` §3): v1.188 mail has **no attachment
//! slot and no gold field**, and **no subject/title field**, in either UI
//! generation. Mail is sender + body. Nothing here adds either.
//!
//! Where a block's `Rect` carries `w,h = 0,0` the original takes the extent
//! from the art, so the sizes below are the DDS extents of the named `.ddj`
//! (`com_mid_button` 88x24, `gil_subj_button10` 156x24, `gil_shape` 16x24),
//! not chosen numbers. The rows themselves are runtime-populated through
//! `CIFScrollManager` ([U] how many fit) and the send/read/delete commands
//! stay presentational until the `0x7309`/`0xB309` wire exists
//! (`docs/net-mail-consignment-0x7309.md`), so they are drawn as art + label
//! rather than as buttons that would do nothing.

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui_widgets::{Activate, Button};

use crate::assets::FontAssets;
use crate::plugins::hud::community::model::{CommunityState, LetterSubWindow};
use crate::plugins::hud::game_window::abs_node;
use crate::plugins::textdata::ClientUiStrings;

// --- Layout constants (resinfo/ifletter.txt, page-local units) --------------

/// `GDR_LETTER_FRAME:CIFFrame` (`frameg01_wnd_`, 16px pieces).
const FRAME_RECT: (f32, f32, f32, f32) = (6.0, 6.0, 440.0, 308.0);
const FRAME_PIECE: f32 = 16.0;
const FRAME_DIR: &str = "media://interface/frame/frameg01_wnd_";
/// `GDR_LETTER_BLACKSQUARE:CIFStretchWnd` — the list area's black plate; its
/// `com_blacksquare_` art is six 4px trim pieces around a flat black interior.
const LIST_RECT: (f32, f32, f32, f32) = (14.0, 18.0, 333.0, 282.0);
const BLACKSQUARE_PIECE: f32 = 4.0;
const BLACKSQUARE_DIR: &str = "media://interface/ifcommon/com_blacksquare_";
/// `GDR_LETTER_BG_01:CIFNormalTile` — the command column's tiled background.
const COMMAND_BG_RECT: (f32, f32, f32, f32) = (328.0, 22.0, 102.0, 276.0);
const BG_TILE_DDJ: &str = "media://interface/ifcommon/bg_tile/com_bg_tile_b.ddj";
/// `GDR_LETTER_NUMBER_STA` (label) and `GDR_LETTER_NUMBER` (count).
const COUNT_LABEL_RECT: (f32, f32, f32, f32) = (353.0, 27.0, 33.0, 15.0);
const COUNT_VALUE_RECT: (f32, f32, f32, f32) = (380.0, 27.0, 40.0, 15.0);
/// `GDR_LETTER_LIST_SORT_*_BTN` — column headers over the list.
const SORT_BTN_SIZE: (f32, f32) = (156.0, 24.0);
const SORT_SENDER_POS: (f32, f32) = (17.0, 21.0);
const SORT_TIME_POS: (f32, f32) = (173.0, 21.0);
const SORT_BTN_DDJ: &str = "media://interface/guild/gil_subj_button10.ddj";
/// `GDR_LETTER_LIST_BTN_SHAPE_END:CIFStatic` — the header strip's end cap.
const SHAPE_END_POS: (f32, f32) = (329.0, 21.0);
const SHAPE_END_SIZE: (f32, f32) = (16.0, 24.0);
const SHAPE_END_DDJ: &str = "media://interface/guild/gil_shape.ddj";
/// `GDR_LETTER_COMMAND_BUTTON_1..5`, top to bottom, one 29-unit pitch apart.
const COMMAND_X: f32 = 352.0;
const COMMAND_YS: [f32; 5] = [55.0, 84.0, 113.0, 142.0, 171.0];
const COMMAND_BTN_SIZE: (f32, f32) = (88.0, 24.0);
const COMMAND_BTN_DDJ: &str = "media://interface/ifcommon/com_mid_button.ddj";
/// The five commands' `Text` keys, in button order 1..5.
const COMMAND_KEYS: [(&str, &str); 5] = [
    ("UIIT_CTL_LETTER_SEND", "Send"),
    ("UIIT_STT_LETTER_READ", "Read"),
    ("UIIT_CTL_LETTER_DELETE", "Delete"),
    ("UIIT_CTL_LETTERERR_GUILD", "Guild"),
    ("UIIT_CTL_LETTERERR_UNION", "Union"),
];

const LABEL_COLOR: Color = Color::srgb(0.92, 0.92, 0.92);

/// The mail count readout, refreshed once a mail list exists.
#[derive(Component)]
pub struct LetterCountText;

/// A list-page command that opens one of the mail sub-windows.
#[derive(Component)]
pub struct LetterListCommand(pub LetterSubWindow);

/// Open the sub-window the pressed command hosts. No wire is touched: both
/// windows are hosted by `ifletter.txt` itself.
fn on_letter_command(
    activate: On<Activate>,
    commands: Query<&LetterListCommand>,
    mut state: ResMut<CommunityState>,
) {
    let Ok(command) = commands.get(activate.entity) else {
        return;
    };
    state.letter_sub = command.0;
}

/// Fill the Letter page container with the list page.
pub fn spawn_letter_page(
    page: &mut ChildSpawnerCommands,
    asset_server: &AssetServer,
    fonts: &FontAssets,
    ui_strings: &ClientUiStrings,
    s: f32,
) {
    let text_font = |size: f32| TextFont {
        font: fonts.two.clone().into(),
        font_size: FontSize::Px(size * s),
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

    // GDR_LETTER_FRAME: the frameg01_wnd_ ring around the whole page
    let (fx, fy, fw, fh) = FRAME_RECT;
    for ((x, y, w, h), piece) in ring(fw, fh, FRAME_PIECE) {
        page.spawn(image(
            (fx + x, fy + y, w, h),
            format!("{FRAME_DIR}{piece}.ddj"),
        ));
    }

    // GDR_LETTER_BG_01: tiled background of the command column
    page.spawn((
        abs_node(COMMAND_BG_RECT, s),
        ImageNode {
            image: asset_server.load(BG_TILE_DDJ),
            image_mode: NodeImageMode::Tiled {
                tile_x: true,
                tile_y: true,
                stretch_value: s,
            },
            ..default()
        },
        Pickable::IGNORE,
    ));

    // GDR_LETTER_BLACKSQUARE: the list plate (flat black + its 4px trim)
    page.spawn((
        abs_node(LIST_RECT, s),
        BackgroundColor(Color::BLACK),
        Pickable::IGNORE,
    ));
    let (lx, ly, lw, lh) = LIST_RECT;
    for ((x, y, w, h), piece) in [
        ((0.0, 0.0, BLACKSQUARE_PIECE, BLACKSQUARE_PIECE), "left_up"),
        (
            (
                lw - BLACKSQUARE_PIECE,
                0.0,
                BLACKSQUARE_PIECE,
                BLACKSQUARE_PIECE,
            ),
            "right_up",
        ),
        (
            (
                0.0,
                BLACKSQUARE_PIECE,
                BLACKSQUARE_PIECE,
                lh - 2.0 * BLACKSQUARE_PIECE,
            ),
            "left_side",
        ),
        (
            (
                lw - BLACKSQUARE_PIECE,
                BLACKSQUARE_PIECE,
                BLACKSQUARE_PIECE,
                lh - 2.0 * BLACKSQUARE_PIECE,
            ),
            "right_side",
        ),
        (
            (
                0.0,
                lh - BLACKSQUARE_PIECE,
                BLACKSQUARE_PIECE,
                BLACKSQUARE_PIECE,
            ),
            "left_down",
        ),
        (
            (
                lw - BLACKSQUARE_PIECE,
                lh - BLACKSQUARE_PIECE,
                BLACKSQUARE_PIECE,
                BLACKSQUARE_PIECE,
            ),
            "right_down",
        ),
    ] {
        page.spawn(image(
            (lx + x, ly + y, w, h),
            format!("{BLACKSQUARE_DIR}{piece}.ddj"),
        ));
    }

    // column headers (sender / time) + the strip's end cap
    for (pos, (key, fallback)) in [
        (SORT_SENDER_POS, ("UIIT_STT_LETTER_SENDER", "Sender")),
        (SORT_TIME_POS, ("UIIT_STT_LETTER_ADDTIME", "Time")),
    ] {
        let rect = (pos.0, pos.1, SORT_BTN_SIZE.0, SORT_BTN_SIZE.1);
        page.spawn(image(rect, SORT_BTN_DDJ.to_string()))
            .with_children(|header| {
                header.spawn((
                    Text::new(ui_strings.get_or(key, fallback).to_string()),
                    text_font(7.5),
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
    page.spawn(image(
        (
            SHAPE_END_POS.0,
            SHAPE_END_POS.1,
            SHAPE_END_SIZE.0,
            SHAPE_END_SIZE.1,
        ),
        SHAPE_END_DDJ.to_string(),
    ));

    // mail count: "Letter" label + the number
    page.spawn((
        Text::new(ui_strings.get_or("UIIT_STT_LETTER", "Letter").to_string()),
        text_font(7.5),
        TextColor(LABEL_COLOR),
        TextLayout::justify(Justify::Left),
        abs_node(COUNT_LABEL_RECT, s),
        Pickable::IGNORE,
    ));
    page.spawn((
        LetterCountText,
        Text::new("0"),
        text_font(7.5),
        TextColor(LABEL_COLOR),
        TextLayout::justify(Justify::Left),
        abs_node(COUNT_VALUE_RECT, s),
        Pickable::IGNORE,
    ));

    // The five command plates. Send and Read *open* the sub-windows of
    // `ifletterwrite.txt` / `ifletterread.txt` — that is pure UI state and
    // needs no wire. The other three stay presentational: Delete, Guild and
    // Union all need the 0x7309 family, which is spec-derived and has no
    // `packet_dump/` sample (`community/letter_sub.rs` module doc).
    for (index, (y, (key, fallback))) in COMMAND_YS.iter().zip(COMMAND_KEYS).enumerate() {
        let opens = match index {
            0 => Some(LetterSubWindow::Write),
            1 => Some(LetterSubWindow::Read),
            _ => None,
        };
        let mut plate = page.spawn(image(
            (COMMAND_X, *y, COMMAND_BTN_SIZE.0, COMMAND_BTN_SIZE.1),
            COMMAND_BTN_DDJ.to_string(),
        ));
        if let Some(window) = opens {
            plate
                .insert((
                    LetterListCommand(window),
                    Button,
                    Hovered::default(),
                    Pickable::default(),
                ))
                .observe(on_letter_command);
        }
        plate.with_children(|button| {
            button.spawn((
                Text::new(ui_strings.get_or(key, fallback).to_string()),
                text_font(7.5),
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
}

/// The 8 pieces of a `*_wnd_` frame ring over a `w x h` box, at `p` px pieces.
fn ring(w: f32, h: f32, p: f32) -> [((f32, f32, f32, f32), &'static str); 8] {
    [
        ((0.0, 0.0, p, p), "left_up"),
        ((p - 1.0, 0.0, w - 2.0 * p + 2.0, p), "mid_up"),
        ((w - p, 0.0, p, p), "right_up"),
        ((0.0, p - 1.0, p, h - 2.0 * p + 2.0), "left_side"),
        ((w - p, p - 1.0, p, h - 2.0 * p + 2.0), "right_side"),
        ((0.0, h - p, p, p), "left_down"),
        ((p - 1.0, h - p, w - 2.0 * p + 2.0, p), "mid_down"),
        ((w - p, h - p, p, p), "right_down"),
    ]
}

#[cfg(test)]
mod test {
    use super::*;

    /// Every page-local rect must stay inside the shared community page rect
    /// (`ifcommunity.txt` `13,61,451,320`) — the list page is drawn *into* that
    /// page, so a rect past it would mean a bad transcription.
    #[test]
    fn page_content_fits_the_shared_page_rect() {
        let (pw, ph) = (451.0, 320.0);
        let boxes = [
            FRAME_RECT,
            LIST_RECT,
            COMMAND_BG_RECT,
            COUNT_LABEL_RECT,
            COUNT_VALUE_RECT,
            (
                COMMAND_X,
                COMMAND_YS[4],
                COMMAND_BTN_SIZE.0,
                COMMAND_BTN_SIZE.1,
            ),
        ];
        for (x, y, w, h) in boxes {
            assert!(x + w <= pw, "rect {x},{y},{w},{h} overflows width");
            assert!(y + h <= ph, "rect {x},{y},{w},{h} overflows height");
        }
    }

    /// The five commands are exactly what `ifletter.txt` declares — no
    /// attachment or subject control exists in the tree, and none is added.
    #[test]
    fn mail_has_no_attachment_or_subject_control() {
        assert_eq!(COMMAND_KEYS.len(), 5);
        for (key, _) in COMMAND_KEYS {
            assert!(
                !key.contains("ITEM") && !key.contains("GOLD") && !key.contains("TITLE"),
                "{key} looks like an attachment/subject control"
            );
        }
    }

    /// `GDR_LETTER_COMMAND_BUTTON_1..5` sit on a 29-unit pitch from y=55.
    #[test]
    fn command_buttons_keep_the_vanilla_pitch() {
        assert_eq!(COMMAND_YS, [55.0, 84.0, 113.0, 142.0, 171.0]);
        for pair in COMMAND_YS.windows(2) {
            assert_eq!(pair[1] - pair[0], 29.0);
        }
    }
}
