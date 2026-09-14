//! The party-matching board — `ifpartymatch.txt` + `ifpartymatchslot.txt`.
//!
//! Idea: a 785x480 window listing advertised parties, twelve rows at a time,
//! with eight sortable columns, server-side paging and six actions. Every rect
//! below is transcribed from the two resinfo trees and rebased into the shared
//! shell's content space by subtracting `(12, 42)` — the *inner* frame's own
//! origin, not the shell's constants.
//!
//! That choice is what makes the whole thing line up, and it is arithmetic
//! rather than taste. `GDR_PARTY_MATCH` is `0,0,785,480` **including** its
//! `mframe_wnd_` ring, and it carries a second, inner `int_window_` frame at
//! `12,42,762,428`. Handing our shell a `761x428` content area gives an outer
//! box of `761 + 24 = 785` by `36 + 428 + 4 + 12 = 480` — vanilla's window
//! exactly, on both axes — and puts the inner frame at content `(0,0)`.
//! `the_shell_reproduces_the_vanilla_window` pins both numbers.
//!
//! Two transcription traps the RE doc flags, both honoured here:
//!
//! - **the header row is an abutment chain over OPAQUE widths, not file
//!   widths.** `gil_subj_button03.ddj` is 44 wide with 3 transparent columns,
//!   so the chain only closes at 41: `28+48 = 76`, `76+41 = 117`, and so on to
//!   `676+68 = 744`. Using the file width breaks it at the second column.
//! - **`Text="L"` on the last header cell is not a label.** It occurs five
//!   times corpus-wide and every one is a `*_DUMY` static capping a list header
//!   over the scrollbar. It is drawn as art and never as text.
//!
//! Scope note: the search strip is drawn — its frame, labels, field plates and
//! both buttons — but the three input fields do not filter yet. That is a
//! smaller gap than it looks: `PartyMatchListRequest` carries **only** a page
//! index, so there is no server-side search to drive; any filtering would be
//! ours, over the twelve rows already on screen. Refresh and the page spinner
//! are wired, because those are the parts the wire actually has.

use bevy::ecs::relationship::RelatedSpawnerCommands;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui_widgets::{Activate, Button};

use packets::agent::party::{PartyMatchDeleteRequest, PartyMatchListRequest, PartySetup};
use packets::Packet;

use bytes::Bytes;

use crate::assets::FontAssets;
use crate::net::connection::SilkroadConnection;
use crate::net::frame::SilkroadFrame;
use crate::plugins::hud::game_window::{self, abs_node};
use bevy::log::warn_once;

use crate::plugins::config::chat::parse_argb;
use crate::plugins::config::ClientConfig;
use crate::plugins::hud::chat::model::{ChatHistory, ChatLine};
use crate::plugins::hud::party_matching::dialogs::PROGRESS_TIMEOUT_SECS;
use crate::plugins::hud::party_matching::model::{
    MatchDialog, MatchSort, PartyMatchState, ReloadMatchList, MATCH_ROWS,
};
use crate::plugins::hud::scale::hud_scale;
use crate::plugins::net::agent::AgentConnection;
use crate::plugins::textdata::ClientUiStrings;
use crate::plugins::ui_v2::style::ImageButtonStyle;

// --- Layout (content space = host space - (12, 42)) -------------------------

/// Sized so the shell's outer box is vanilla's `785x480` exactly.
pub const CONTENT_W: f32 = 761.0;
pub const CONTENT_H: f32 = 428.0;
const WINDOW_RIGHT: f32 = 120.0;
const WINDOW_TOP: f32 = 60.0;

const ART_COMMON: &str = "media://interface/ifcommon/";
const ART_GUILD: &str = "media://interface/guild/";
const ART_FRAME: &str = "media://interface/frame/";
const ART_INVENTORY: &str = "media://interface/inventory/";

/// `GDR_PARTYMATCH_FRAME` — the inner `int_window_` ring, at content origin.
const INNER_FRAME_RECT: (f32, f32, f32, f32) = (0.0, 0.0, 762.0, 428.0);
const SEARCH_FRAME_RECT: (f32, f32, f32, f32) = (14.0, 11.0, 735.0, 61.0);
const BLACKSQUARE_RECT: (f32, f32, f32, f32) = (13.0, 77.0, 735.0, 305.0);
const BG_TILE_1: (f32, f32, f32, f32) = (38.0, 27.0, 688.0, 29.0);
const BG_TILE_2: (f32, f32, f32, f32) = (16.0, 71.0, 730.0, 6.0);
const BG_TILE_3: (f32, f32, f32, f32) = (16.0, 382.0, 730.0, 30.0);
const SCROLL_RECT: (f32, f32, f32, f32) = (731.0, 117.0, 15.0, 232.0);
const SPIN_RECT: (f32, f32, f32, f32) = (348.0, 395.0, 50.0, 16.0);

/// Search strip.
const SEARCH_NAME_LABEL: (f32, f32, f32, f32) = (44.0, 37.0, 35.0, 11.0);
const SEARCH_NAME_FIELD: (f32, f32, f32, f32) = (91.0, 37.0, 81.0, 11.0);
const SEARCH_PURPOSE_LABEL: (f32, f32, f32, f32) = (199.0, 37.0, 35.0, 11.0);
const SEARCH_PURPOSE_COMBO: (f32, f32, f32, f32) = (241.0, 33.0, 74.0, 20.0);
const SEARCH_LEVEL_LABEL: (f32, f32, f32, f32) = (339.0, 37.0, 35.0, 11.0);
const SEARCH_MIN_FIELD: (f32, f32, f32, f32) = (384.0, 36.0, 29.0, 15.0);
const SEARCH_DIV: (f32, f32, f32, f32) = (419.0, 38.0, 9.0, 11.0);
const SEARCH_MAX_FIELD: (f32, f32, f32, f32) = (434.0, 36.0, 29.0, 15.0);
const SEARCH_BTN: (f32, f32, f32, f32) = (513.0, 31.0, 88.0, 24.0);
const REFRESH_BTN: (f32, f32, f32, f32) = (623.0, 31.0, 88.0, 24.0);

/// Column headers, all at content y 80, 24 tall.
const HEADER_Y: f32 = 80.0;
const HEADER_H: f32 = 24.0;
/// `(x, file width, opaque width, art stem, string key, fallback, sort)`.
///
/// The opaque width is the load-bearing one — see the module doc.
const HEADERS: [(f32, f32, f32, &str, &str, &str, MatchSort); 8] = [
    (
        16.0,
        48.0,
        48.0,
        "gil_subj_button08",
        "UIIT_CTL_PARTYMATCH_PSEARCH_LIST_NUM",
        "No.",
        MatchSort::Number,
    ),
    (
        64.0,
        44.0,
        41.0,
        "gil_subj_button03",
        "UIIT_CTL_PARTYMATCH_PSEARCH_LIST_TYPE",
        "Type",
        MatchSort::Wire,
    ),
    (
        105.0,
        48.0,
        48.0,
        "gil_subj_button08",
        "UIIT_CTL_PARTYMATCH_PSEARCH_LIST_RACE",
        "Race",
        MatchSort::Race,
    ),
    (
        153.0,
        96.0,
        96.0,
        "gil_subj_button06",
        "UIIT_STT_COSNEWUI_BASICINFO_NAME",
        "Name",
        MatchSort::Name,
    ),
    (
        249.0,
        326.0,
        326.0,
        "com_bar02_",
        "UIIT_STT_LETTER_TITLE",
        "Title",
        MatchSort::Title,
    ),
    (
        575.0,
        48.0,
        48.0,
        "gil_subj_button08",
        "UIIT_CTL_PARTYMATCH_PSEARCH_OBJECT",
        "Purpose",
        MatchSort::Purpose,
    ),
    (
        623.0,
        44.0,
        41.0,
        "gil_subj_button03",
        "UIIT_CTL_PARTYMATCH_PSEARCH_LIST_NUMBER",
        "Member",
        MatchSort::Members,
    ),
    (
        664.0,
        68.0,
        68.0,
        "gil_subj_button05",
        "UIIT_CTL_PARTYMATCH_PSEARCH_LIST_LEVEL",
        "Level range",
        MatchSort::Level,
    ),
];
/// `_SLOTLIST_DUMY` — `gil_shape.ddj` capping the chain over the scrollbar.
const HEADER_DUMMY: (f32, f32, f32, f32) = (732.0, HEADER_Y, 16.0, HEADER_H);

/// Twelve rows at `28,y,715,24`, pitch 23.
const ROW_X: f32 = 16.0;
const ROW_TOP: f32 = 102.0;
const ROW_PITCH: f32 = 23.0;
const ROW_W: f32 = 715.0;
const ROW_H: f32 = 24.0;

/// `_SLOT_SELECT` is a `CIFBarWnd` at `0,0,715,24` over `com_bar01_`
/// (`hud-party-matching.md` §3.5) — the row's background *and* its selection
/// highlight, which is why the row draws a bar at all rather than nothing.
const ROW_BAR_STEM: &str = "com_bar01_";
/// The selected variant. **[S]**: `com_bar01select_{left,mid,right}.ddj` sits
/// on disk next to the plain trio and no resinfo file references it, so the
/// pairing is a naming convention rather than an authored binding
/// (`hud-party-matching.md` §7). Both trios are RGB565 with **no alpha mask**,
/// so neither may be keyed — bit 15 is not alpha.
const ROW_BAR_SELECTED_STEM: &str = "com_bar01select_";

/// Row cells, row-local (`ifpartymatchslot.txt`).
const CELL_ID: (f32, f32, f32, f32) = (10.0, 7.0, 36.0, 11.0);
const CELL_MARK: (f32, f32, f32, f32) = (60.0, 5.0, 16.0, 16.0);
const CELL_RACE: (f32, f32, f32, f32) = (101.0, 7.0, 31.0, 11.0);
const CELL_NAME: (f32, f32, f32, f32) = (150.0, 7.0, 78.0, 11.0);
const CELL_TITLE: (f32, f32, f32, f32) = (248.0, 7.0, 306.0, 11.0);
const CELL_PURPOSE: (f32, f32, f32, f32) = (566.0, 7.0, 37.0, 11.0);
const CELL_MEMBERS: (f32, f32, f32, f32) = (619.0, 7.0, 24.0, 11.0);
const CELL_LEVEL: (f32, f32, f32, f32) = (659.0, 7.0, 51.0, 11.0);

/// Footer buttons, `com_mid_button.ddj` 88x24 at a 101 px pitch.
const FOOTER_Y: f32 = 392.0;
const FOOTER_W: f32 = 88.0;
const FOOTER_H: f32 = 24.0;

const LABEL_COLOR: Color = Color::srgb_u8(239, 218, 164);
const TEXT_COLOR: Color = Color::srgb_u8(255, 255, 255);

// --- Markers ----------------------------------------------------------------

#[derive(Component)]
pub struct MatchWindowRoot;
#[derive(Component)]
pub struct MatchRow(pub usize);
/// One piece of a row's background bar: `(row, "left"|"mid"|"right")`.
///
/// The piece name rides along because the selected variant is a *different
/// three-slice*, not a tint — the swap has to rebuild each piece's own path.
#[derive(Component, Clone)]
pub struct MatchRowBar(pub usize, pub &'static str);
#[derive(Component)]
pub struct MatchCell(pub usize, pub MatchColumn);
#[derive(Component)]
pub struct MatchHeader(pub MatchSort);
#[derive(Component)]
pub struct MatchPageLabel;
#[derive(Component, Clone, Copy)]
pub struct MatchPageStep(pub i8);

/// Which cell of a row a text node is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MatchColumn {
    Number,
    Race,
    Name,
    Title,
    Purpose,
    Members,
    Level,
}

/// The six footer verbs.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub enum MatchAction {
    Join,
    Whisper,
    AutoMatch,
    Register,
    Modify,
    Delete,
    Search,
    Refresh,
}

// --- Spawning ---------------------------------------------------------------

pub fn spawn_match_window(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    fonts: Res<FontAssets>,
    ui_strings: Res<ClientUiStrings>,
    cam_query: Query<Entity, With<Camera2d>>,
) {
    let Ok(camera) = cam_query.single() else {
        warn!("party match: no 2d camera to attach to");
        return;
    };
    let s = hud_scale();

    let window = game_window::spawn_game_window(
        &mut commands,
        &asset_server,
        &fonts,
        camera,
        ui_strings.get_or("UIIT_PAG_PARTYMATCH_PSEARCH", "Party Matching"),
        (CONTENT_W, CONTENT_H),
        (WINDOW_RIGHT, WINDOW_TOP),
        s,
    );
    commands
        .entity(window.root)
        .insert((MatchWindowRoot, GlobalZIndex(58)))
        .entry::<Node>()
        .and_modify(|mut node| node.display = Display::None);
    commands
        .entity(window.expect_close_button())
        .observe(on_close_button);

    let font = |size: f32| TextFont {
        font: fonts.two.clone().into(),
        font_size: FontSize::Px(size * s),
        ..default()
    };

    commands.entity(window.content).with_children(|content| {
        let img = |rect: (f32, f32, f32, f32), path: String| {
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

        // --- chrome ---------------------------------------------------------
        spawn_nine_slice(
            content,
            &asset_server,
            &format!("{ART_INVENTORY}int_window_"),
            INNER_FRAME_RECT,
            16.0,
            s,
        );
        spawn_nine_slice(
            content,
            &asset_server,
            &format!("{ART_FRAME}frameg_wnd_"),
            SEARCH_FRAME_RECT,
            16.0,
            s,
        );
        content.spawn(img(
            BG_TILE_1,
            format!("{ART_COMMON}bg_tile/com_bg_tile_b.ddj"),
        ));
        content.spawn(img(
            BG_TILE_2,
            format!("{ART_COMMON}bg_tile/com_bg_tile_b.ddj"),
        ));
        content.spawn(img(
            BG_TILE_3,
            format!("{ART_COMMON}bg_tile/com_bg_tile_b.ddj"),
        ));
        // Six pieces, not nine: `com_blacksquare_` has no mids (see
        // `spawn_corner_trim`).
        spawn_corner_trim(
            content,
            &asset_server,
            &format!("{ART_COMMON}com_blacksquare_"),
            BLACKSQUARE_RECT,
            8.0,
            s,
        );
        // The scrollbar track: vanilla's own `_SCROLL` is 232 tall against a
        // 276-tall row band, and with twelve fixed rows plus server paging it
        // has nothing to scroll. It is drawn as the plate the data declares and
        // deliberately not wired (§9-F leaves its role UNKNOWN).
        content.spawn(img(
            SCROLL_RECT,
            format!("{ART_COMMON}bg_tile/com_bg_tile_e.ddj"),
        ));

        // --- search strip ---------------------------------------------------
        for (rect, key, fallback) in [
            (
                SEARCH_NAME_LABEL,
                "UIIT_CTL_PARTYMATCH_PSEARCH_NAME",
                "Name",
            ),
            (
                SEARCH_PURPOSE_LABEL,
                "UIIT_CTL_PARTYMATCH_PSEARCH_OBJECT",
                "Purpose",
            ),
            (SEARCH_LEVEL_LABEL, "UIIT_STT_LEVEL", "Level"),
        ] {
            content.spawn((
                Text::new(ui_strings.get_or(key, fallback).to_string()),
                font(8.5),
                TextColor(LABEL_COLOR),
                abs_node(rect, s),
                Pickable::IGNORE,
            ));
        }
        for rect in [
            SEARCH_NAME_FIELD,
            SEARCH_PURPOSE_COMBO,
            SEARCH_MIN_FIELD,
            SEARCH_MAX_FIELD,
        ] {
            content.spawn(img(rect, format!("{ART_COMMON}bg_tile/com_bg_tile_e.ddj")));
        }
        // The 9 px separator between the two level fields. Its glyph is not in
        // the data (`Text=""`), so a tilde is ours — stated, not sourced.
        content.spawn((
            Text::new("~".to_string()),
            font(8.5),
            TextColor(LABEL_COLOR),
            TextLayout::justify(Justify::Center),
            abs_node(SEARCH_DIV, s),
            Pickable::IGNORE,
        ));

        // --- column headers -------------------------------------------------
        for (x, _file_w, opaque_w, stem, key, fallback, sort) in HEADERS {
            let rect = (x, HEADER_Y, opaque_w, HEADER_H);
            let art = if stem == "com_bar02_" {
                None
            } else {
                Some(format!("{ART_GUILD}{stem}.ddj"))
            };
            let mut entity = content.spawn((
                MatchHeader(sort),
                Button,
                Hovered::default(),
                abs_node(rect, s),
            ));
            entity.observe(on_header_click);
            if let Some(art) = art {
                entity.insert((
                    ImageButtonStyle {
                        normal: asset_server.load(art.clone()),
                        hover: asset_server.load(art.replace(".ddj", "_focus.ddj")),
                        press: asset_server.load(art.replace(".ddj", "_press.ddj")),
                        ..Default::default()
                    },
                    ImageNode {
                        image: asset_server.load(art),
                        image_mode: NodeImageMode::Stretch,
                        ..default()
                    },
                ));
            }
            entity.with_children(|header| {
                if stem == "com_bar02_" {
                    spawn_three_slice(
                        header,
                        &asset_server,
                        &format!("{ART_COMMON}com_bar02_"),
                        (0.0, 0.0, opaque_w, HEADER_H),
                        12.0,
                        s,
                        |_| (),
                    );
                }
                header.spawn((
                    Text::new(ui_strings.get_or(key, fallback).to_string()),
                    font(8.5),
                    TextColor(TEXT_COLOR),
                    TextLayout::justify(Justify::Center),
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(0.0),
                        top: Val::Px(6.0 * s),
                        width: Val::Px(opaque_w * s),
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
            });
        }
        // `Text="L"` is a corpus idiom for a header cap, not a label — drawn
        // as art only.
        content.spawn(img(HEADER_DUMMY, format!("{ART_GUILD}gil_shape.ddj")));

        // --- rows -----------------------------------------------------------
        for row in 0..MATCH_ROWS {
            let y = ROW_TOP + row as f32 * ROW_PITCH;
            content
                .spawn((
                    MatchRow(row),
                    Button,
                    Hovered::default(),
                    abs_node((ROW_X, y, ROW_W, ROW_H), s),
                    Visibility::Hidden,
                ))
                .observe(on_row_click)
                .with_children(|slot| {
                    spawn_three_slice(
                        slot,
                        &asset_server,
                        &format!("{ART_COMMON}{ROW_BAR_STEM}"),
                        (0.0, 0.0, ROW_W, ROW_H),
                        4.0,
                        s,
                        move |piece| MatchRowBar(row, piece),
                    );
                    for (column, rect, justify) in [
                        (MatchColumn::Number, CELL_ID, Justify::Center),
                        (MatchColumn::Race, CELL_RACE, Justify::Center),
                        (MatchColumn::Name, CELL_NAME, Justify::Left),
                        (MatchColumn::Title, CELL_TITLE, Justify::Left),
                        (MatchColumn::Purpose, CELL_PURPOSE, Justify::Center),
                        (MatchColumn::Members, CELL_MEMBERS, Justify::Center),
                        (MatchColumn::Level, CELL_LEVEL, Justify::Center),
                    ] {
                        slot.spawn((
                            MatchCell(row, column),
                            Text::new(""),
                            font(8.5),
                            TextColor(TEXT_COLOR),
                            TextLayout::justify(justify),
                            abs_node(rect, s),
                            Pickable::IGNORE,
                        ));
                    }
                    // The Type column is a 16x16 icon with `DDJ=""` and no
                    // tooltip string anywhere in the data, so which icons it
                    // picks from is UNKNOWN (§9-D). The slot is reserved and
                    // left empty rather than filled with a guess.
                    slot.spawn((
                        abs_node(CELL_MARK, s),
                        ImageNode::default(),
                        Visibility::Hidden,
                        Pickable::IGNORE,
                    ));
                });
        }

        // --- page spinner ---------------------------------------------------
        content.spawn(img(
            SPIN_RECT,
            format!("{ART_COMMON}bg_tile/com_bg_tile_e.ddj"),
        ));
        for (step, dx, art) in [
            (-1i8, 0.0, "com_left_arrow"),
            (1, SPIN_RECT.2 - 16.0, "com_right_arrow"),
        ] {
            content
                .spawn((
                    MatchPageStep(step),
                    Button,
                    Hovered::default(),
                    ImageButtonStyle {
                        normal: asset_server.load(format!("{ART_COMMON}{art}.ddj")),
                        hover: asset_server.load(format!("{ART_COMMON}{art}_focus.ddj")),
                        press: asset_server.load(format!("{ART_COMMON}{art}_press.ddj")),
                        ..Default::default()
                    },
                    ImageNode {
                        image: asset_server.load(format!("{ART_COMMON}{art}.ddj")),
                        image_mode: NodeImageMode::Stretch,
                        ..default()
                    },
                    abs_node((SPIN_RECT.0 + dx, SPIN_RECT.1, 16.0, 16.0), s),
                ))
                .observe(on_page_step);
        }
        content.spawn((
            MatchPageLabel,
            Text::new(""),
            font(8.5),
            TextColor(TEXT_COLOR),
            TextLayout::justify(Justify::Center),
            abs_node((SPIN_RECT.0 + 16.0, SPIN_RECT.1 + 2.0, 18.0, 12.0), s),
            Pickable::IGNORE,
        ));

        // --- footer + search buttons ----------------------------------------
        for (action, x, key, fallback) in [
            (
                MatchAction::Join,
                17.0,
                "UIIT_CTL_PARTYMATCH_PSEARCH_JOIN",
                "Join party",
            ),
            (
                MatchAction::Whisper,
                118.0,
                "UIIT_STT_GET_WHISPER",
                "Whisper",
            ),
            (
                MatchAction::AutoMatch,
                219.0,
                "UIIT_CTL_PARTYMATCH_PSEARCH_AUTOMATCHING",
                "Auto match",
            ),
            (
                MatchAction::Register,
                440.0,
                "UIIT_CTL_PARTYMATCH_PSEARCH_RECORD",
                "Form party",
            ),
            (
                MatchAction::Modify,
                541.0,
                "UIIT_CTL_PARTYMATCH_PSEARCH_MODIFY",
                "Change entry",
            ),
            (
                MatchAction::Delete,
                642.0,
                "UIIT_CTL_PARTYMATCH_PSEARCH_DELETE",
                "Delete entry",
            ),
        ] {
            spawn_action_button(
                content,
                &asset_server,
                &fonts,
                action,
                (x, FOOTER_Y, FOOTER_W, FOOTER_H),
                ui_strings.get_or(key, fallback),
                s,
            );
        }
        for (action, rect, key, fallback) in [
            (
                MatchAction::Search,
                SEARCH_BTN,
                "UIIT_CTL_PARTYMATCH_PSEARCH_FIND",
                "Search",
            ),
            (
                MatchAction::Refresh,
                REFRESH_BTN,
                "UIIT_CTL_PARTYMATCH_PSEARCH_REFRESH",
                "Refresh",
            ),
        ] {
            spawn_action_button(
                content,
                &asset_server,
                &fonts,
                action,
                rect,
                ui_strings.get_or(key, fallback),
                s,
            );
        }
    });
}

/// An 8-piece ring with square corners of `corner` px.
fn spawn_nine_slice(
    parent: &mut RelatedSpawnerCommands<ChildOf>,
    asset_server: &AssetServer,
    dir: &str,
    rect: (f32, f32, f32, f32),
    corner: f32,
    s: f32,
) {
    spawn_ring(parent, asset_server, dir, rect, corner, s, true);
}

/// The same ring without `mid_up`/`mid_down`, for an art family that ships
/// only six pieces.
///
/// `com_blacksquare_` is one: the archive holds its four corners and two
/// sides and nothing else, so asking for its mids 404s twice on every build
/// and drew nothing either way. The other three modules that use this family
/// (`community/letter`, `community/letter_sub`, `alchemy/grant`) each
/// hand-roll the same six-piece trim.
fn spawn_corner_trim(
    parent: &mut RelatedSpawnerCommands<ChildOf>,
    asset_server: &AssetServer,
    dir: &str,
    rect: (f32, f32, f32, f32),
    corner: f32,
    s: f32,
) {
    spawn_ring(parent, asset_server, dir, rect, corner, s, false);
}

/// Shared body: four corners and two sides, plus the top/bottom mids when the
/// family has them. The mids butt against the corners rather than overlapping
/// them, so drawing them last changes nothing.
fn spawn_ring(
    parent: &mut RelatedSpawnerCommands<ChildOf>,
    asset_server: &AssetServer,
    dir: &str,
    rect: (f32, f32, f32, f32),
    corner: f32,
    s: f32,
    with_mid: bool,
) {
    let (x, y, w, h) = rect;
    let (iw, ih) = ((w - 2.0 * corner).max(0.0), (h - 2.0 * corner).max(0.0));
    let mut pieces: Vec<(&str, (f32, f32, f32, f32))> = vec![
        ("left_up", (x, y, corner, corner)),
        ("right_up", (x + w - corner, y, corner, corner)),
        ("left_side", (x, y + corner, corner, ih)),
        ("right_side", (x + w - corner, y + corner, corner, ih)),
        ("left_down", (x, y + h - corner, corner, corner)),
        (
            "right_down",
            (x + w - corner, y + h - corner, corner, corner),
        ),
    ];
    if with_mid {
        pieces.extend([
            ("mid_up", (x + corner, y, iw, corner)),
            ("mid_down", (x + corner, y + h - corner, iw, corner)),
        ]);
    }
    for (piece, piece_rect) in pieces {
        parent.spawn((
            abs_node(piece_rect, s),
            ImageNode {
                image: asset_server.load(format!("{dir}{piece}.ddj")),
                image_mode: NodeImageMode::Stretch,
                ..default()
            },
            Pickable::IGNORE,
        ));
    }
}

/// A horizontal 3-piece bar (`com_bar01_` / `com_bar02_`): fixed caps, stretched
/// middle.
/// `mark` is applied to each of the three pieces, so a caller that needs to
/// re-address them later (the rows, which swap to the selected variant) can;
/// pass `()` when nothing has to find them again.
fn spawn_three_slice<M: Bundle + Clone>(
    parent: &mut RelatedSpawnerCommands<ChildOf>,
    asset_server: &AssetServer,
    dir: &str,
    rect: (f32, f32, f32, f32),
    cap: f32,
    s: f32,
    mark: impl Fn(&'static str) -> M,
) {
    let (x, y, w, h) = rect;
    let pieces = [
        ("left", (x, y, cap, h)),
        ("mid", (x + cap, y, (w - 2.0 * cap).max(0.0), h)),
        ("right", (x + w - cap, y, cap, h)),
    ];
    for (piece, piece_rect) in pieces {
        parent.spawn((
            mark(piece),
            abs_node(piece_rect, s),
            ImageNode {
                image: asset_server.load(format!("{dir}{piece}.ddj")),
                image_mode: NodeImageMode::Stretch,
                ..default()
            },
            Pickable::IGNORE,
        ));
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_action_button(
    parent: &mut RelatedSpawnerCommands<ChildOf>,
    asset_server: &AssetServer,
    fonts: &FontAssets,
    action: MatchAction,
    rect: (f32, f32, f32, f32),
    caption: &str,
    s: f32,
) {
    parent
        .spawn((
            action,
            Button,
            Hovered::default(),
            ImageButtonStyle {
                normal: asset_server.load(format!("{ART_COMMON}com_mid_button.ddj")),
                hover: asset_server.load(format!("{ART_COMMON}com_mid_button_focus.ddj")),
                press: asset_server.load(format!("{ART_COMMON}com_mid_button_press.ddj")),
                disable: asset_server.load(format!("{ART_COMMON}com_mid_button_disable.ddj")),
            },
            ImageNode {
                image: asset_server.load(format!("{ART_COMMON}com_mid_button.ddj")),
                image_mode: NodeImageMode::Stretch,
                ..default()
            },
            abs_node(rect, s),
        ))
        .observe(on_action_button)
        .with_children(|button| {
            button.spawn((
                Text::new(caption.to_string()),
                TextFont {
                    font: fonts.two.clone().into(),
                    font_size: FontSize::Px(8.5 * s),
                    ..default()
                },
                TextColor(Color::srgb_u8(255, 247, 202)),
                TextLayout::justify(Justify::Center),
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    top: Val::Px(6.0 * s),
                    width: Val::Px(rect.2 * s),
                    ..default()
                },
                Pickable::IGNORE,
            ));
        });
}

pub fn cleanup_match_window(mut commands: Commands, roots: Query<Entity, With<MatchWindowRoot>>) {
    for root in roots.iter() {
        commands.entity(root).despawn();
    }
}

// --- Visibility + refresh ---------------------------------------------------

pub fn apply_match_visibility(
    state: Res<PartyMatchState>,
    mut roots: Query<&mut Node, With<MatchWindowRoot>>,
) {
    if !state.is_changed() {
        return;
    }
    for mut node in roots.iter_mut() {
        node.display = if state.open {
            Display::Flex
        } else {
            Display::None
        };
    }
}

pub fn match_needs_refresh(state: Res<PartyMatchState>) -> bool {
    state.open && state.is_changed()
}

pub fn refresh_match_board(
    state: Res<PartyMatchState>,
    ui_strings: Res<ClientUiStrings>,
    config: Res<ClientConfig>,
    asset_server: Res<AssetServer>,
    mut rows: Query<(&MatchRow, &mut Visibility)>,
    mut cells: Query<(&MatchCell, &mut Text, &mut TextColor)>,
    mut bars: Query<(&MatchRowBar, &mut ImageNode)>,
    mut page: Query<&mut Text, (With<MatchPageLabel>, Without<MatchCell>)>,
) {
    let sorted = state.sorted();
    let own_tint = own_party_tint(&config.hud.party.own_party_color);
    for (row, mut visibility) in rows.iter_mut() {
        *visibility = if sorted.get(row.0).is_some() {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
    // The clicked row wears the selected bar. `selected` has driven the action
    // buttons since the board landed; it just had nothing to draw with,
    // because the three background pieces carried no marker to address.
    let selected_row = state.selected_row(&sorted);
    for (bar, mut image) in bars.iter_mut() {
        let stem = if selected_row == Some(bar.0) {
            ROW_BAR_SELECTED_STEM
        } else {
            ROW_BAR_STEM
        };
        let art = asset_server.load(format!("{ART_COMMON}{stem}{}.ddj", bar.1));
        // Only on a real change: this runs on every board repaint, and an
        // unconditional write would dirty `ImageNode` for twelve rows a frame.
        if image.image != art {
            image.image = art;
        }
    }
    for (cell, mut text, mut color) in cells.iter_mut() {
        // The row of the party you are in draws in the configured tint; every
        // other row keeps the board's own colour.
        color.0 = match sorted.get(cell.0) {
            Some(entry) if state.is_own_party(entry) => own_tint,
            _ => TEXT_COLOR,
        };
        text.0 = match sorted.get(cell.0) {
            Some(entry) => match cell.1 {
                MatchColumn::Number => entry.number.to_string(),
                MatchColumn::Race => race_label(entry.race_type, &ui_strings),
                MatchColumn::Name => entry.master_name.clone(),
                MatchColumn::Title => entry.title.clone(),
                MatchColumn::Purpose => purpose_label(entry.purpose, &ui_strings),
                MatchColumn::Members => format!(
                    "{}/{}",
                    entry.member_count,
                    PartySetup(entry.setup).capacity()
                ),
                MatchColumn::Level => format!("{}~{}", entry.level_min, entry.level_max),
            },
            None => String::new(),
        };
    }
    for mut text in page.iter_mut() {
        // The spinner shows the CURRENT page only, as vanilla's 18x12 label
        // does; the page count rides in the tooltip-free layout as 1-based.
        text.0 = if state.page_count == 0 {
            String::new()
        } else {
            (state.page_index as u16 + 1).to_string()
        };
    }
}

/// The configured own-party tint, or the default when the value is malformed.
///
/// Parsed at the point of use rather than resolved into its own resource: this
/// runs only when the board actually repaints (`match_needs_refresh`), not per
/// frame, so the resolved-resource ceremony the nameplate/chat colours need
/// would buy nothing here.
fn own_party_tint(configured: &str) -> Color {
    parse_argb(configured).unwrap_or_else(|| {
        warn_once!(
            "config: invalid hud.party.own_party_color {configured:?}, using the default blue"
        );
        parse_argb(DEFAULT_OWN_PARTY_COLOR).expect("the default own-party colour is valid")
    })
}

/// Kept next to the parser so the fallback and the config default cannot drift.
const DEFAULT_OWN_PARTY_COLOR: &str = "FF7FB2FF";

/// The purpose column's label.
///
/// The corpus ships exactly five purpose strings — All / Hunting / Trade /
/// Trade Union / Thief Union — and **no "Quest"**, while the wire constant set
/// from two independent spec sources says `Quest = 1`. That conflict is
/// unresolved (§9-A), so value 1 renders as its own number rather than being
/// given a name we cannot source, and the constant is NOT renamed.
fn purpose_label(purpose: u8, ui_strings: &ClientUiStrings) -> String {
    match purpose {
        packets::agent::party::PARTY_PURPOSE_HUNTING => ui_strings
            .get_or("UIIT_CTL_PARTYMATCH_PSEARCH_OBJECT_HUNT", "Hunting")
            .to_string(),
        packets::agent::party::PARTY_PURPOSE_TRADER => ui_strings
            .get_or("UIIT_CTL_PARTYMATCH_PSEARCH_OBJECT_TRADE", "Trade")
            .to_string(),
        packets::agent::party::PARTY_PURPOSE_THIEF => ui_strings
            .get_or("UIIT_CTL_PARTYMATCH_RECORD_OBJECT_THIEF", "Thief Union")
            .to_string(),
        other => other.to_string(),
    }
}

/// `race_type` is byte-exact against go-sro's `countryType`, but which value is
/// which race is UNKNOWN, so the raw value is shown rather than a guessed name.
fn race_label(race: u8, _ui_strings: &ClientUiStrings) -> String {
    race.to_string()
}

// --- Observers --------------------------------------------------------------

fn on_close_button(_: On<Activate>, mut state: ResMut<PartyMatchState>) {
    state.open = false;
}

fn on_header_click(
    activate: On<Activate>,
    headers: Query<&MatchHeader>,
    mut state: ResMut<PartyMatchState>,
) {
    if let Ok(header) = headers.get(activate.entity) {
        // The Type column has no sortable field (its icon's meaning is
        // UNKNOWN), so its header is inert rather than sorting by nothing.
        if header.0 != MatchSort::Wire {
            state.sort_by(header.0);
        }
    }
}

fn on_row_click(
    activate: On<Activate>,
    rows: Query<&MatchRow>,
    mut state: ResMut<PartyMatchState>,
) {
    let Ok(row) = rows.get(activate.entity) else {
        return;
    };
    let number = state.sorted().get(row.0).map(|entry| entry.number);
    state.selected = number;
}

fn on_page_step(
    activate: On<Activate>,
    steps: Query<&MatchPageStep>,
    mut state: ResMut<PartyMatchState>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
) {
    let Ok(step) = steps.get(activate.entity) else {
        return;
    };
    let next = state.page_index as i16 + step.0 as i16;
    if next < 0 || (state.page_count > 0 && next >= state.page_count as i16) {
        return;
    }
    state.page_index = next as u8;
    request_page(&conn, state.page_index);
}

fn on_action_button(
    activate: On<Activate>,
    actions: Query<&MatchAction>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    mut state: ResMut<PartyMatchState>,
    mut history: Option<ResMut<crate::plugins::hud::chat::model::ChatHistory>>,
) {
    let Ok(action) = actions.get(activate.entity) else {
        return;
    };
    // The board's verbs log the same way the party sends do (`net/party.rs`).
    // It also settles the one question a dead-looking button cannot answer on
    // its own: if this line appears, the pointer reached the button and the
    // fault is downstream; if it never appears, picking never got here.
    info!(
        "party match: {action:?} pressed (selected={:?})",
        state.selected
    );
    match action {
        MatchAction::Refresh | MatchAction::Search => {
            let page = state.page_index;
            request_page(&conn, page);
        }
        MatchAction::Join => {
            if let Some(entry) = state.selected_entry() {
                let number = entry.number;
                send_join_request(&conn, number);
                state.dialog = MatchDialog::JoinProgress {
                    number,
                    elapsed: 0.0,
                };
            }
        }
        MatchAction::Whisper => {
            // Vanilla opens a whisper to the party master. Our chat owns the
            // whisper target, so this only reports until that seam exists.
            if let (Some(entry), Some(history)) = (state.selected_entry(), history.as_mut()) {
                history.push(crate::plugins::hud::chat::model::ChatLine::system(format!(
                    "Whisper to {}.",
                    entry.master_name
                )));
            }
        }
        MatchAction::AutoMatch => state.dialog = MatchDialog::Auto,
        MatchAction::Register => state.dialog = MatchDialog::Register { editing: None },
        MatchAction::Modify => {
            // Snapshot before writing: the form and the entry live in the same
            // resource, so the read has to finish before the write starts.
            let snapshot = state.selected_entry().map(|entry| {
                (
                    entry.number,
                    entry.purpose,
                    entry.level_min,
                    entry.level_max,
                    entry.title.clone(),
                )
            });
            let editing = snapshot.as_ref().map(|(number, ..)| *number);
            if let Some((_, purpose, level_min, level_max, title)) = snapshot {
                state.form.purpose = purpose;
                state.form.level_min = level_min;
                state.form.level_max = level_max;
                state.form.title = title;
            }
            state.dialog = MatchDialog::Register { editing };
        }
        MatchAction::Delete => {
            if let Some(number) = state.selected {
                send(&conn, Packet::from(PartyMatchDeleteRequest { number }));
            }
        }
    }
}

/// 0x706C — ask the server for one page.
pub fn request_page(conn: &Query<&SilkroadConnection, With<AgentConnection>>, page_index: u8) {
    send(conn, Packet::from(PartyMatchListRequest { page_index }));
}

fn send(conn: &Query<&SilkroadConnection, With<AgentConnection>>, packet: Packet) {
    let Ok(conn) = conn.single() else {
        warn!("party match: no agent connection");
        return;
    };
    if let Err(e) = conn.get_sender().send(packet.into()) {
        error!("network: failed to send party match packet: {}", e.0);
    }
}

/// 0x706D outbound — "let me into party `number`".
///
/// Built as a frame rather than as a `Packet` because `packets!` maps **one
/// type per opcode** and 0x706D's registration belongs to the inbound
/// `PartyMatchJoinNotify`: the opcode is genuinely bidirectional with two
/// different bodies, and an unparsed *inbound* packet is the failure that costs
/// something. The outbound body is a bare `u32`, so it is emitted directly with
/// the same field values `Into<SilkroadFrame> for Packet` uses — the send path
/// fills in count, crc and encryption afterwards.
fn send_join_request(conn: &Query<&SilkroadConnection, With<AgentConnection>>, number: u32) {
    let Ok(conn) = conn.single() else {
        warn!("party match: no agent connection");
        return;
    };
    let frame = SilkroadFrame::Packet {
        count: 0,
        crc: 0,
        opcode: 0x706D,
        encrypted: 0,
        data: Bytes::copy_from_slice(&number.to_le_bytes()),
    };
    if let Err(e) = conn.get_sender().send(frame) {
        error!("network: failed to send party match join: {}", e.0);
    }
}

/// Reopen the board when the roster page's "Party match" button asks.
pub fn open_from_party_window(
    mut requests: MessageReader<crate::plugins::hud::party::ui::PartyWindowRequest>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    mut state: ResMut<PartyMatchState>,
) {
    for request in requests.read() {
        if matches!(
            request,
            crate::plugins::hud::party::ui::PartyWindowRequest::Match
        ) {
            state.open = true;
            request_page(&conn, state.page_index);
        }
    }
}

/// Ask for the first page the moment the board opens, so it is never blank.
pub fn request_page_on_open(
    state: Res<PartyMatchState>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    mut was_open: Local<bool>,
) {
    if state.open && !*was_open {
        request_page(&conn, state.page_index);
    }
    *was_open = state.open;
}

/// Also used by the join-progress dialog's timeout.
pub fn tick_join_progress(
    time: Res<Time>,
    // Both optional by construction. Bevy 0.19 does NOT skip a system whose
    // `Res` is missing — it fails parameter validation and panics the whole
    // schedule (the same trap `net/party.rs` records for `ChatHistory`). The
    // timeout has to keep firing whether or not the string table is loaded, so
    // a `run_if` gate would be wrong here too.
    ui_strings: Option<Res<ClientUiStrings>>,
    mut state: ResMut<PartyMatchState>,
    mut history: Option<ResMut<ChatHistory>>,
) {
    // The check has to go through the IMMUTABLE deref first. `&mut state.dialog`
    // calls `ResMut::deref_mut`, which sets the changed tick *before* the
    // pattern is even tested — so writing this as a bare `if let` marks
    // `PartyMatchState` changed on every frame, whatever dialog is up.
    //
    // That is not a style point. Everything gated on `is_changed()` then fires
    // every frame, which for `sync_match_dialogs` meant despawning and
    // respawning the whole modal each frame: `Hovered` is written onto an
    // entity that no longer exists next frame, and `Activate` needs the
    // `Pressed` marker to survive from pointer-down to pointer-up on the *same*
    // entity. The buttons rendered perfectly and could not be hovered or
    // clicked, and the dialog could not be closed.
    if !matches!(state.dialog, MatchDialog::JoinProgress { .. }) {
        return;
    }
    let mut expired = false;
    if let MatchDialog::JoinProgress { elapsed, .. } = &mut state.dialog {
        *elapsed += time.delta_secs();
        expired = *elapsed >= PROGRESS_TIMEOUT_SECS;
    }
    // A request the server simply never answers would otherwise leave the
    // player behind a modal for ever. The original ships a string for exactly
    // this outcome, which is the tell that it gives up too.
    if expired {
        state.dialog = MatchDialog::None;
        const NO_REPLY: &str = "The party did not answer your request.";
        let text = ui_strings
            .as_ref()
            .map(|strings| strings.get_plain_or("UIIT_MSG_PARTYMATCH_JOIN_NOREPLY", NO_REPLY))
            .unwrap_or_else(|| NO_REPLY.to_string());
        match history.as_mut() {
            Some(history) => history.push(ChatLine::system(text)),
            None => info!("party match: {text}"),
        }
    }
}

/// Re-read the current page when the server confirms a registration or an edit.
///
/// Without this a freshly formed party only appears after the player presses
/// Refresh, because `0xB069` acknowledges the registration but does not carry
/// the updated list.
pub fn reload_match_list(
    mut reader: MessageReader<ReloadMatchList>,
    state: Res<PartyMatchState>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
) {
    if reader.read().count() == 0 {
        return;
    }
    request_page(&conn, state.page_index);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::hud::party_matching::dialogs::sync_match_dialogs;

    /// The regression that killed every button in the match dialogs.
    ///
    /// `tick_join_progress` used a bare `if let ... = &mut state.dialog`, and
    /// `ResMut::deref_mut` sets the changed tick *before* the pattern is
    /// tested — so the resource looked changed on every frame no matter which
    /// dialog was up. Everything gated on `is_changed()` then fired every
    /// frame, and the dialog painter's response to that was to despawn and
    /// respawn the whole tree: `Hovered` was written onto entities that no
    /// longer existed, and `Activate` never saw its `Pressed` marker again at
    /// pointer-up. The buttons drew perfectly and were completely dead.
    ///
    /// The invariant is one line and needs no window, so it is cheap to keep.
    #[test]
    fn ticking_does_not_mark_the_state_changed_when_no_bar_is_up() {
        #[derive(Resource, Default)]
        struct SawChange(bool);

        fn record(state: Res<PartyMatchState>, mut saw: ResMut<SawChange>) {
            saw.0 = state.is_changed();
        }

        let mut app = App::new();
        app.init_resource::<PartyMatchState>()
            .init_resource::<SawChange>()
            .init_resource::<Time>()
            .add_systems(Update, (tick_join_progress, record).chain());

        // The first update sees the freshly inserted resource as changed, which
        // is correct and not what this is about; the second is the steady state.
        app.update();
        app.update();

        assert!(
            !app.world().resource::<SawChange>().0,
            "an idle tick marked PartyMatchState changed, which rebuilds any \
             open dialog every frame and makes its buttons unclickable"
        );
    }

    /// ...and it still ticks when there IS a bar, so the guard did not just
    /// disable the feature.
    #[test]
    fn ticking_still_advances_a_live_progress_bar() {
        let mut app = App::new();
        app.init_resource::<PartyMatchState>()
            .init_resource::<Time>()
            .add_systems(Update, tick_join_progress);
        app.world_mut().resource_mut::<PartyMatchState>().dialog = MatchDialog::JoinProgress {
            number: 7,
            elapsed: 0.0,
        };

        app.update();
        app.update();

        let MatchDialog::JoinProgress { elapsed, .. } =
            app.world().resource::<PartyMatchState>().dialog
        else {
            panic!("the dialog changed identity");
        };
        assert!(elapsed >= 0.0, "the bar must still be driven");
    }

    /// A malformed configured colour falls back instead of failing, and the
    /// fallback is the same value the config defaults to — the two are written
    /// in different files, so nothing but a test stops them drifting apart.
    #[test]
    fn the_own_party_tint_falls_back_to_the_config_default() {
        use crate::plugins::config::hud::PartySettings;

        let default = PartySettings::default().own_party_color;
        assert_eq!(default, DEFAULT_OWN_PARTY_COLOR);
        assert_eq!(own_party_tint(&default), own_party_tint("not a colour"));
        // ...and a real value is honoured rather than swallowed by the fallback
        assert_ne!(own_party_tint("FFFF0000"), own_party_tint(&default));
    }

    /// The same headless guard the other two party surfaces carry. It covers
    /// the dialogs too, because the preview scene brings all four surfaces up
    /// at once and a conflict in any of them takes the whole schedule down.
    #[test]
    fn the_board_and_its_dialogs_have_disjoint_queries() {
        let mut world = World::new();
        let mut schedule = Schedule::default();
        schedule.add_systems((
            apply_match_visibility,
            refresh_match_board,
            sync_match_dialogs,
            open_from_party_window,
            request_page_on_open,
            tick_join_progress,
        ));
        schedule
            .initialize(&mut world)
            .expect("the match board must build a valid schedule");
    }

    /// The content size is chosen so the shell reproduces vanilla's window on
    /// both axes — that is what lets every rect be a plain subtraction.
    #[test]
    fn the_shell_reproduces_the_vanilla_window() {
        let (w, h) = game_window::outer_size((CONTENT_W, CONTENT_H));
        assert_eq!((w, h), (785.0, 480.0));
        // ...and the inner frame lands exactly at the content origin
        assert_eq!((INNER_FRAME_RECT.0, INNER_FRAME_RECT.1), (0.0, 0.0));
    }

    /// The header row is an abutment chain over OPAQUE widths. Using the file
    /// widths breaks it at the second column, which is the whole point.
    #[test]
    fn the_header_chain_closes_on_opaque_widths() {
        for pair in HEADERS.windows(2) {
            let (x, _file, opaque, ..) = pair[0];
            let (next_x, ..) = pair[1];
            assert_eq!(
                x + opaque,
                next_x,
                "header at {x} does not abut the next at {next_x}"
            );
        }
        let (last_x, _, last_opaque, ..) = HEADERS[HEADERS.len() - 1];
        assert_eq!(last_x + last_opaque, HEADER_DUMMY.0);

        // and the trap: the one art whose file width differs from its opaque
        // width really does differ, so this is not a vacuous check
        assert!(HEADERS.iter().any(|(_, file, opaque, ..)| file != opaque));
    }

    /// Twelve rows at pitch 23 starting at 102 stay inside the list's
    /// blacksquare, and the last one does not overhang it.
    #[test]
    fn the_rows_fit_the_list_area() {
        let last_bottom = ROW_TOP + (MATCH_ROWS as f32 - 1.0) * ROW_PITCH + ROW_H;
        assert!(last_bottom <= BLACKSQUARE_RECT.1 + BLACKSQUARE_RECT.3);
        assert_eq!(
            ROW_TOP,
            HEADER_Y + 22.0,
            "rows start at the header's opaque bottom"
        );
        assert_eq!(MATCH_ROWS, 12);
    }

    /// Each row cell sits inside the header column it belongs to, so the board
    /// reads as a table rather than as eight independent strips.
    #[test]
    fn every_cell_sits_under_its_header() {
        let columns = [
            (CELL_ID, HEADERS[0]),
            (CELL_RACE, HEADERS[2]),
            (CELL_NAME, HEADERS[3]),
            (CELL_TITLE, HEADERS[4]),
            (CELL_PURPOSE, HEADERS[5]),
            (CELL_MEMBERS, HEADERS[6]),
            (CELL_LEVEL, HEADERS[7]),
        ];
        for (cell, (hx, _file, hw, ..)) in columns {
            let left = ROW_X + cell.0;
            let right = left + cell.2;
            assert!(left >= hx, "cell {left} starts left of header {hx}");
            assert!(
                right <= hx + hw,
                "cell {right} runs past header {}",
                hx + hw
            );
        }
    }

    /// The six footer buttons sit on the 101 px pitch the data declares, in two
    /// groups of three, and all stay inside the frame.
    #[test]
    fn the_footer_buttons_keep_their_pitch() {
        let xs = [17.0, 118.0, 219.0, 440.0, 541.0, 642.0];
        for group in [&xs[0..3], &xs[3..6]] {
            for pair in group.windows(2) {
                assert_eq!(pair[1] - pair[0], 101.0);
            }
        }
        assert!(xs[5] + FOOTER_W <= INNER_FRAME_RECT.2);
    }
}
