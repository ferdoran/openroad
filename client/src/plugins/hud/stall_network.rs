//! Stall network board — the cross-server stall search
//! (`ginterface.txt:1587` `GDR_STALL_NETWORK:CIFStallNetwork` id 150,
//! `Rect="0,0,545,666"`, trees `ifstallnetwork.txt` + `ifstallnetworkslot.txt`).
//! Doc: `docs/re/ui/hud-stall-network.md`.
//!
//! Idea: the whole board is authored geometry, so this module is a transcription
//! rather than a design. Every rect below is the original's, written in **window
//! coordinates** exactly as the tree spells them, and turned into a node by
//! [`local`], which subtracts the `int_window_` origin `14,44` — our
//! `game_window` shell supplies the `mframe_wnd_` frame and the `int_window_`
//! board, so the board *is* our content area. Keeping the authored numbers
//! literal is what lets the tests below re-check the doc's two structural
//! findings against this file: the column header chain closes on **opaque** art
//! widths (`29+50+68+146+50+50+125 == 518`, the result bed's right edge), and
//! the 15 row origins are a hand-nudged list (pitch 23 with a four-row run of
//! 22), not a computed pitch.
//!
//! Rows are paged, not scrolled: the tree has no `CIFVerticalScroll`, it has one
//! `CIFPageManager` (`80,582,385,26`), and both generations independently land on
//! 15 rows per page. Sorting is client-side over the page set and the ITEM column
//! is deliberately not sortable — it is a `CIFStatic` (`:814`) where the other
//! five are `CIFButton`s.
//!
//! Deviations, all stated:
//!
//! * **Combo option lists are empty.** The three `CIFComboBox`es carry `DDJ=""`
//!   and `Text=""`; their contents are code-side (doc §9-U2/U4) and no
//!   `WNETWORK` key names a *degree* at all. We draw the authored closed-state
//!   field and leave the lists unpopulated rather than invent a category tree.
//! * **The page strip is text, not art.** `CIFPageManager` has no art in the data
//!   (`DDJ=""`, doc §9-U3), so the page numbers are drawn as plain labels on the
//!   authored `80,582,385,26` band instead of guessed chrome.
//! * **`_BG_02` (`78,495,120,16`) is not drawn.** It is the doc's §9-U1 anomaly:
//!   a `com_bg_tile_c` patch floating in the middle of the result list, with no
//!   control on it. Drawing it would paint a stray band across rows 12/13.
//! * **No wire.** Search and buy opcodes are UNKNOWN (doc §9-U6), so the board
//!   renders [`StallNetworkState::rows`] and the buttons only set state; nothing
//!   here fabricates a request.

use bevy::prelude::*;
use bevy::ui_widgets::{Activate, Button};

use crate::assets::FontAssets;
use crate::plugins::hud::game_window::{abs_node, spawn_game_window};
use crate::plugins::hud::scale::hud_scale;
use crate::plugins::textdata::ClientUiStrings;
use crate::scenes::SceneState;

/// `GDR_STALLNETWORK_FRAME` `14,44,518,608` — the `int_window_` board is our
/// content area, and every authored rect is expressed relative to its origin.
const BOARD_ORIGIN: (f32, f32) = (14.0, 44.0);
const CONTENT: (f32, f32) = (518.0, 608.0);

// --- §Create, authored window coordinates -----------------------------------
const BG_TILE_01: (f32, f32, f32, f32) = (46.0, 88.0, 455.0, 78.0);
const BG_TILE_02: (f32, f32, f32, f32) = (30.0, 153.0, 486.0, 60.0);
const BG_TILE_03: (f32, f32, f32, f32) = (30.0, 572.0, 486.0, 64.0);
const BLACKSQUARE_01: (f32, f32, f32, f32) = (26.0, 206.0, 494.0, 370.0);
const BLACKSQUARE_02: (f32, f32, f32, f32) = (76.0, 616.0, 124.0, 20.0);
/// `_BG_01` — the result-list bed; the pitch law is applied to *this* height
/// (364), not to the blacksquare shell's 370.
const RESULT_BED: (f32, f32, f32, f32) = (28.0, 208.0, 490.0, 364.0);
const INNER_BOX: (f32, f32, f32, f32) = (26.0, 60.0, 495.0, 93.0);
const SEARCH_RESULT_STA: (f32, f32, f32, f32) = (29.0, 189.0, 111.0, 11.0);
const HAVEGOLD_STA: (f32, f32, f32, f32) = (23.0, 623.0, 45.0, 11.0);
/// `GDR_STALLNET_GOLD_BG` and `GDR_STALLNET_HAVEGOLD` share this rect byte for
/// byte — the tile is the bed, the static is the value drawn on it.
const GOLD_FIELD: (f32, f32, f32, f32) = (78.0, 618.0, 120.0, 16.0);
/// `com_button.ddj` is 76x24; both buttons author `w=h=0` and take the art size.
const BUY_BTN: (f32, f32, f32, f32) = (428.0, 609.0, 76.0, 24.0);
const SEARCH_BTN: (f32, f32, f32, f32) = (428.0, 160.0, 76.0, 24.0);
const PAGE_MGR: (f32, f32, f32, f32) = (80.0, 582.0, 385.0, 26.0);

// --- §StallSearch — the three combos and their labels ------------------------
const FRAME_TITLE: (f32, f32, f32, f32) = (208.0, 66.0, 120.0, 12.0);
const COMBOS: [(f32, f32, f32, f32); 3] = [
    (79.0, 106.0, 121.0, 20.0),
    (259.0, 106.0, 109.0, 20.0),
    (419.0, 106.0, 90.0, 20.0),
];
const COMBO_LABELS: [(f32, f32, f32, f32); 3] = [
    (31.0, 112.0, 43.0, 11.0),
    (208.0, 112.0, 43.0, 11.0),
    (369.0, 112.0, 43.0, 11.0),
];
const COMBO_LABEL_KEYS: [(&str, &str); 3] = [
    ("UIIT_STT_WARENETWORK_SCAN_BIG", "Class"),
    ("UIIT_STT_WARENETWORK_SCAN_MIDDLE", "Type"),
    ("UIIT_STT_WARENETWORK_SCAN_SMALL", "Degree"),
];
/// The only non-white font colour among this file's statics.
const COMBO_LABEL_COLOR: Color = Color::srgb(239.0 / 255.0, 218.0 / 255.0, 164.0 / 255.0);
/// `FontColor=255,255,247,202` on both buttons.
const BUTTON_CAPTION_COLOR: Color = Color::srgb(255.0 / 255.0, 247.0 / 255.0, 202.0 / 255.0);

// --- §StallSearchResult — header row and rows --------------------------------
/// All six header controls sit at y = 209 and are art-sized. The widths are the
/// arts' **opaque** widths (52/68/148/52/52/128 on disk), which is what makes
/// the abutment chain close on the bed's right edge.
const HEADER_Y: f32 = 209.0;
const HEADER_H: f32 = 24.0;
/// Opaque height of three of the four distinct header arts (`gil_subj_button11`
/// alone is opaque over all 24 rows). This is the height the header row abuts
/// row 1 on; the drawn rect stays 24 so no authored pixel is cropped.
const HEADER_OPAQUE_H: f32 = 22.0;
const HEADERS: [(f32, f32, &str, &str, &str); 6] = [
    (
        29.0,
        50.0,
        "guild/gil_subj_button04.ddj",
        "UIIT_STT_WARENETWORK_RESULT_NUMBER",
        "Number",
    ),
    (
        79.0,
        68.0,
        "guild/gil_subj_button05.ddj",
        "UIIT_STT_WARENETWORK_RESULT_ITEM",
        "Item",
    ),
    (
        147.0,
        146.0,
        "guild/gil_subj_button11.ddj",
        "UIIT_STT_WARENETWORK_RESULT_NAME",
        "Name",
    ),
    (
        293.0,
        50.0,
        "guild/gil_subj_button04.ddj",
        "UIIT_STT_WARENETWORK_RESULT_FIGURE",
        "Quantity",
    ),
    (
        343.0,
        50.0,
        "guild/gil_subj_button04.ddj",
        "UIIT_STT_WARENETWORK_RESULT_LEVEL",
        "Level",
    ),
    (
        393.0,
        125.0,
        "guild/gil_subj_button16.ddj",
        "UIIT_STT_WARENETWORK_RESULT_PRICE",
        "Price",
    ),
];

/// The 15 authored row origins, literally (`ifstallnetwork.txt:453-719`). The
/// pitch is 23 with a hand-nudged run of 22 at rows 9-12; do not recompute it.
const ROW_Y: [f32; 15] = [
    231.0, 254.0, 277.0, 300.0, 323.0, 346.0, 369.0, 392.0, 414.0, 436.0, 458.0, 480.0, 503.0,
    526.0, 549.0,
];
const ROW_X: f32 = 29.0;
const ROW_W: f32 = 488.0;
const ROW_H: f32 = 24.0;
pub const ROWS_PER_PAGE: usize = ROW_Y.len();

/// Row fields, slot-local (`ifstallnetworkslot.txt`), in column order.
const FIELD_NUM: (f32, f32, f32, f32) = (6.0, 7.0, 38.0, 16.0);
const FIELD_ICON: (f32, f32, f32, f32) = (76.0, 4.0, 16.0, 16.0);
const FIELD_NAME: (f32, f32, f32, f32) = (125.0, 7.0, 132.0, 16.0);
const FIELD_EA: (f32, f32, f32, f32) = (270.0, 7.0, 39.0, 16.0);
const FIELD_LEV: (f32, f32, f32, f32) = (319.0, 7.0, 39.0, 16.0);
/// The only right-aligned field — a u64 gold value.
const FIELD_PRICE: (f32, f32, f32, f32) = (368.0, 7.0, 111.0, 16.0);

const ART: &str = "media://interface/";
const BG_TILE_B: &str = "media://interface/ifcommon/bg_tile/com_bg_tile_b.ddj";
const BG_TILE_C: &str = "media://interface/ifcommon/bg_tile/com_bg_tile_c.ddj";
const BG_TILE_E: &str = "media://interface/ifcommon/bg_tile/com_bg_tile_e.ddj";
const BLACKSQUARE: &str = "media://interface/ifcommon/com_blacksquare_left_up.ddj";
const BAR01: &str = "media://interface/ifcommon/com_bar01_left.ddj";
const BUTTON: &str = "media://interface/ifcommon/com_button.ddj";
const INNER_BOX_ART: &str = "media://interface/option/opt_inner_box_mid_up.ddj";

const LABEL_FONT: f32 = 9.0;

/// A sortable column. `Item` is absent on purpose: its header is a `CIFStatic`,
/// so the original ships **five** sort keys, not six.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum StallSortKey {
    #[default]
    Number,
    Name,
    Quantity,
    Level,
    Price,
}

impl StallSortKey {
    /// The sort key of header column `index`, or `None` for the ITEM column.
    pub fn from_column(index: usize) -> Option<Self> {
        match index {
            0 => Some(Self::Number),
            1 => None,
            2 => Some(Self::Name),
            3 => Some(Self::Quantity),
            4 => Some(Self::Level),
            5 => Some(Self::Price),
            _ => None,
        }
    }
}

/// One search result. Everything here is server state (doc §9-U6); the client
/// has no way to produce a row on its own.
#[derive(Clone, Debug, Default)]
pub struct StallNetworkRow {
    /// The result's ordinal as the server listed it — the NUM column.
    pub number: u32,
    pub name: String,
    /// Media-relative icon path, drawn at the slot's authored 16x16.
    pub icon: String,
    pub quantity: u32,
    pub level: u8,
    pub price: u64,
}

#[derive(Resource, Default)]
pub struct StallNetworkState {
    pub open: bool,
    /// Results as received; the board sorts and pages a view of this.
    pub rows: Vec<StallNetworkRow>,
    pub sort: StallSortKey,
    pub descending: bool,
    /// Zero-based page index.
    pub page: usize,
    /// The player's gold, drawn in the `GDR_STALLNET_HAVEGOLD` field.
    pub gold: u64,
    /// The row picked for purchase, as an index into the *sorted* order.
    pub selected: Option<usize>,
}

impl StallNetworkState {
    /// Result indices in sorted order — the board's view of [`Self::rows`].
    pub fn sorted_indices(&self) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.rows.len()).collect();
        order.sort_by(|&a, &b| {
            let (a, b) = (&self.rows[a], &self.rows[b]);
            let ord = match self.sort {
                StallSortKey::Number => a.number.cmp(&b.number),
                StallSortKey::Name => a.name.cmp(&b.name),
                StallSortKey::Quantity => a.quantity.cmp(&b.quantity),
                StallSortKey::Level => a.level.cmp(&b.level),
                StallSortKey::Price => a.price.cmp(&b.price),
            };
            if self.descending {
                ord.reverse()
            } else {
                ord
            }
        });
        order
    }

    /// Number of pages, at least one so the strip never disappears.
    pub fn page_count(&self) -> usize {
        self.rows.len().div_ceil(ROWS_PER_PAGE).max(1)
    }

    /// The indices drawn on the current page, in slot order.
    pub fn page_indices(&self) -> Vec<usize> {
        let page = self.page.min(self.page_count() - 1);
        self.sorted_indices()
            .into_iter()
            .skip(page * ROWS_PER_PAGE)
            .take(ROWS_PER_PAGE)
            .collect()
    }

    /// Click on header column `index`: pick the key, or flip the direction when
    /// it is already the active one. The ITEM column is inert.
    pub fn sort_by_column(&mut self, index: usize) {
        let Some(key) = StallSortKey::from_column(index) else {
            return;
        };
        if self.sort == key {
            self.descending = !self.descending;
        } else {
            self.sort = key;
            self.descending = false;
        }
        self.page = 0;
        self.selected = None;
    }
}

#[derive(Component)]
pub struct StallNetworkRoot;

/// The rebuilt part of the board (rows, page strip, gold).
#[derive(Component)]
pub struct StallNetworkResults;

#[derive(Component, Clone, Copy)]
pub struct StallHeaderColumn(pub usize);

#[derive(Component, Clone, Copy)]
pub struct StallResultRow(pub usize);

#[derive(Component, Clone, Copy)]
pub struct StallPageButton(pub usize);

/// An authored window rect as a board-local node.
fn local(rect: (f32, f32, f32, f32), s: f32) -> Node {
    abs_node(
        (
            rect.0 - BOARD_ORIGIN.0,
            rect.1 - BOARD_ORIGIN.1,
            rect.2,
            rect.3,
        ),
        s,
    )
}

fn stretch(asset_server: &AssetServer, path: &str) -> ImageNode {
    ImageNode {
        image: asset_server.load(path.to_string()),
        image_mode: NodeImageMode::Stretch,
        ..default()
    }
}

fn tile(asset_server: &AssetServer, path: &str, s: f32) -> ImageNode {
    ImageNode {
        image: asset_server.load(path.to_string()),
        image_mode: NodeImageMode::Tiled {
            tile_x: true,
            tile_y: true,
            stretch_value: s,
        },
        ..default()
    }
}

fn label(
    content: String,
    rect: (f32, f32, f32, f32),
    justify: JustifyContent,
    color: Color,
    font: Handle<Font>,
    s: f32,
) -> impl Bundle {
    let mut node = local(rect, s);
    node.justify_content = justify;
    node.align_items = AlignItems::Center;
    (
        node,
        Pickable::IGNORE,
        children![(
            Text::new(content),
            TextFont {
                font: font.into(),
                font_size: FontSize::Px(LABEL_FONT * s),
                ..default()
            },
            TextColor(color),
        )],
    )
}

/// The board's static chrome — everything that does not depend on the results.
pub fn spawn_stall_network(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    fonts: Res<FontAssets>,
    ui_strings: Res<ClientUiStrings>,
    cameras: Query<Entity, With<Camera2d>>,
    mut state: ResMut<StallNetworkState>,
) {
    let Ok(camera) = cameras.single() else {
        warn!("stall network: no 2d camera to attach to");
        return;
    };
    *state = StallNetworkState::default();
    let s = hud_scale();

    let window = spawn_game_window(
        &mut commands,
        &asset_server,
        &fonts,
        camera,
        ui_strings.get_or("UIIT_STT_WARENETWORK_TITLE", "Stall network"),
        CONTENT,
        // The 666-tall window clears `GDR_UNDERBAR` (`112,684,800,52`) only if
        // it is top-anchored at y <= 18; it cannot be vertically centred.
        (60.0, 18.0),
        s,
    );
    commands
        .entity(window.root)
        .insert((StallNetworkRoot, GlobalZIndex(20)));
    commands.entity(window.expect_close_button()).observe(
        |_: On<Activate>, mut state: ResMut<StallNetworkState>| {
            state.open = false;
        },
    );

    commands
        .entity(window.content)
        .despawn_related::<Children>()
        .with_children(|board| {
            // --- criteria box -------------------------------------------------
            board.spawn((
                local(INNER_BOX, s),
                stretch(&asset_server, INNER_BOX_ART),
                Pickable::IGNORE,
            ));
            board.spawn(label(
                ui_strings
                    .get_or("UIIT_STT_WARENETWORK_SCAN_WAREHOUSE", "Stall search")
                    .to_string(),
                FRAME_TITLE,
                JustifyContent::Center,
                Color::WHITE,
                fonts.nine.clone(),
                s,
            ));
            for band in [BG_TILE_01, BG_TILE_02, BG_TILE_03] {
                board.spawn((
                    local(band, s),
                    tile(&asset_server, BG_TILE_B, s),
                    Pickable::IGNORE,
                ));
            }
            for (index, combo) in COMBOS.iter().enumerate() {
                board.spawn(label(
                    ui_strings
                        .get_or(COMBO_LABEL_KEYS[index].0, COMBO_LABEL_KEYS[index].1)
                        .to_string(),
                    COMBO_LABELS[index],
                    JustifyContent::FlexEnd,
                    COMBO_LABEL_COLOR,
                    fonts.nine.clone(),
                    s,
                ));
                // Closed state only: the list chrome is engine-side art we do
                // not have, and the option lists are code-side data we have not
                // read (doc §9-U2/U4).
                board.spawn((
                    local(*combo, s),
                    tile(&asset_server, BG_TILE_E, s),
                    Pickable::IGNORE,
                ));
            }
            spawn_button(
                board,
                &asset_server,
                &fonts,
                SEARCH_BTN,
                ui_strings.get_or("UIIT_STT_WARENETWORK_SCAN", "Search"),
                s,
            );
            board.spawn(label(
                ui_strings
                    .get_or("UIIT_STT_WARENETWORK_SCAN_RESULT", "Search result")
                    .to_string(),
                SEARCH_RESULT_STA,
                JustifyContent::FlexStart,
                Color::WHITE,
                fonts.nine.clone(),
                s,
            ));

            // --- result list shell --------------------------------------------
            board.spawn((
                local(BLACKSQUARE_01, s),
                stretch(&asset_server, BLACKSQUARE),
                Pickable::IGNORE,
            ));
            board.spawn((
                local(RESULT_BED, s),
                tile(&asset_server, BG_TILE_C, s),
                Pickable::IGNORE,
            ));
            for (index, (x, w, art, key, fallback)) in HEADERS.iter().enumerate() {
                board
                    .spawn((
                        StallHeaderColumn(index),
                        Button,
                        local((*x, HEADER_Y, *w, HEADER_H), s),
                        stretch(&asset_server, &format!("{ART}{art}")),
                    ))
                    .observe(
                        |activate: On<Activate>,
                         columns: Query<&StallHeaderColumn>,
                         mut state: ResMut<StallNetworkState>| {
                            if let Ok(column) = columns.get(activate.entity) {
                                state.sort_by_column(column.0);
                            }
                        },
                    );
                board.spawn(label(
                    ui_strings.get_or(key, fallback).to_string(),
                    (*x, HEADER_Y, *w, HEADER_H),
                    JustifyContent::Center,
                    Color::WHITE,
                    fonts.nine.clone(),
                    s,
                ));
            }

            // --- footer -------------------------------------------------------
            board.spawn((
                local(BLACKSQUARE_02, s),
                stretch(&asset_server, BLACKSQUARE),
                Pickable::IGNORE,
            ));
            board.spawn((
                local(GOLD_FIELD, s),
                tile(&asset_server, BG_TILE_E, s),
                Pickable::IGNORE,
            ));
            board.spawn(label(
                ui_strings
                    .get_or("UIIT_STT_WARENETWORK_GOLD", "Amount")
                    .to_string(),
                HAVEGOLD_STA,
                JustifyContent::FlexEnd,
                Color::WHITE,
                fonts.nine.clone(),
                s,
            ));
            spawn_button(
                board,
                &asset_server,
                &fonts,
                BUY_BTN,
                ui_strings.get_or("UIIT_STT_WARENETWORK_BUY", "Purchase"),
                s,
            );

            // The results, the page strip and the gold value are rebuilt from
            // state; they hang off their own node so a rebuild never touches the
            // chrome above.
            board.spawn((
                StallNetworkResults,
                Node {
                    position_type: PositionType::Absolute,
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
                Pickable::IGNORE,
            ));
        });
}

fn spawn_button(
    board: &mut ChildSpawnerCommands,
    asset_server: &AssetServer,
    fonts: &FontAssets,
    rect: (f32, f32, f32, f32),
    caption: &str,
    s: f32,
) {
    board.spawn((
        Button,
        local(rect, s),
        stretch(asset_server, BUTTON),
        children![(
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            Pickable::IGNORE,
            children![(
                Text::new(caption.to_string()),
                TextFont {
                    font: fonts.nine.clone().into(),
                    font_size: FontSize::Px(LABEL_FONT * s),
                    ..default()
                },
                TextColor(BUTTON_CAPTION_COLOR),
            )],
        )],
    ));
}

/// Rebuild the 15 slots, the page strip and the gold readout from state.
pub fn refresh_stall_network(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    fonts: Res<FontAssets>,
    state: Res<StallNetworkState>,
    results: Query<Entity, With<StallNetworkResults>>,
) {
    if !state.is_changed() {
        return;
    }
    let s = hud_scale();
    let page = state.page_indices();
    for parent in results.iter() {
        commands.entity(parent).despawn_related::<Children>();
        commands.entity(parent).with_children(|list| {
            for (slot, index) in page.iter().enumerate() {
                let row = &state.rows[*index];
                let y = ROW_Y[slot];
                list.spawn((
                    StallResultRow(*index),
                    Button,
                    local((ROW_X, y, ROW_W, ROW_H), s),
                    stretch(&asset_server, BAR01),
                ))
                .observe(
                    |activate: On<Activate>,
                     rows: Query<&StallResultRow>,
                     mut state: ResMut<StallNetworkState>| {
                        if let Ok(row) = rows.get(activate.entity) {
                            state.selected = Some(row.0);
                        }
                    },
                );
                let field =
                    |rect: (f32, f32, f32, f32)| (ROW_X + rect.0, y + rect.1, rect.2, rect.3);
                list.spawn(label(
                    row.number.to_string(),
                    field(FIELD_NUM),
                    JustifyContent::Center,
                    Color::WHITE,
                    fonts.nine.clone(),
                    s,
                ));
                if !row.icon.is_empty() {
                    list.spawn((
                        local(field(FIELD_ICON), s),
                        stretch(&asset_server, &format!("media://{}", row.icon)),
                        Pickable::IGNORE,
                    ));
                }
                list.spawn(label(
                    row.name.clone(),
                    field(FIELD_NAME),
                    JustifyContent::Center,
                    Color::WHITE,
                    fonts.nine.clone(),
                    s,
                ));
                list.spawn(label(
                    row.quantity.to_string(),
                    field(FIELD_EA),
                    JustifyContent::Center,
                    Color::WHITE,
                    fonts.nine.clone(),
                    s,
                ));
                list.spawn(label(
                    row.level.to_string(),
                    field(FIELD_LEV),
                    JustifyContent::Center,
                    Color::WHITE,
                    fonts.nine.clone(),
                    s,
                ));
                list.spawn(label(
                    row.price.to_string(),
                    field(FIELD_PRICE),
                    JustifyContent::FlexEnd,
                    Color::WHITE,
                    fonts.nine.clone(),
                    s,
                ));
            }

            // page strip — text on the authored band (see the module note)
            let pages = state.page_count();
            let step = PAGE_MGR.2 / pages.max(1) as f32;
            for page_index in 0..pages {
                let rect = (
                    PAGE_MGR.0 + page_index as f32 * step,
                    PAGE_MGR.1,
                    step,
                    PAGE_MGR.3,
                );
                list.spawn((StallPageButton(page_index), Button, local(rect, s)))
                    .observe(
                        |activate: On<Activate>,
                         buttons: Query<&StallPageButton>,
                         mut state: ResMut<StallNetworkState>| {
                            if let Ok(button) = buttons.get(activate.entity) {
                                state.page = button.0;
                                state.selected = None;
                            }
                        },
                    );
                list.spawn(label(
                    (page_index + 1).to_string(),
                    rect,
                    JustifyContent::Center,
                    if page_index == state.page {
                        BUTTON_CAPTION_COLOR
                    } else {
                        Color::WHITE
                    },
                    fonts.nine.clone(),
                    s,
                ));
            }

            list.spawn(label(
                state.gold.to_string(),
                GOLD_FIELD,
                JustifyContent::FlexEnd,
                Color::WHITE,
                fonts.nine.clone(),
                s,
            ));
        });
    }
}

pub fn apply_stall_network_visibility(
    state: Res<StallNetworkState>,
    mut roots: Query<&mut Node, With<StallNetworkRoot>>,
) {
    if !state.is_changed() {
        return;
    }
    let display = if state.open {
        Display::Flex
    } else {
        Display::None
    };
    for mut node in roots.iter_mut() {
        if node.display != display {
            node.display = display;
        }
    }
}

pub fn cleanup_stall_network(
    mut commands: Commands,
    roots: Query<Entity, With<StallNetworkRoot>>,
    mut state: ResMut<StallNetworkState>,
) {
    for root in roots.iter() {
        commands.entity(root).despawn();
    }
    *state = StallNetworkState::default();
}

pub struct StallNetworkPlugin;

impl Plugin for StallNetworkPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<StallNetworkState>()
            .add_systems(OnEnter(SceneState::GameWorld), spawn_stall_network)
            .add_systems(OnExit(SceneState::GameWorld), cleanup_stall_network)
            .add_systems(
                Update,
                (apply_stall_network_visibility, refresh_stall_network)
                    .chain()
                    .run_if(in_state(SceneState::GameWorld)),
            );
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn row(number: u32, name: &str, quantity: u32, level: u8, price: u64) -> StallNetworkRow {
        StallNetworkRow {
            number,
            name: name.to_string(),
            icon: String::new(),
            quantity,
            level,
            price,
        }
    }

    /// The column headers abut exactly, and only on the arts' **opaque** widths:
    /// `29+50+68+146+50+50+125 == 518 == 28+490`, the result bed's right edge.
    /// With the file widths (52/68/148/52/52/128) the chain breaks at the first
    /// seam.
    #[test]
    fn header_chain_closes_on_the_result_bed() {
        let mut x = HEADERS[0].0;
        assert_eq!(x, 29.0);
        for (start, width, ..) in HEADERS.iter() {
            assert_eq!(*start, x, "header columns must abut");
            x += width;
        }
        assert_eq!(x, 518.0);
        assert_eq!(RESULT_BED.0 + RESULT_BED.2, 518.0);
        // 1 px left inset of the header row inside the bed
        assert_eq!(HEADERS[0].0 - RESULT_BED.0, 1.0);
    }

    /// The row origins are the authored list, pitch 23 with a hand-nudged run of
    /// 22 at rows 9-12 — a computed pitch would not reproduce them.
    #[test]
    fn row_origins_are_the_authored_list() {
        let deltas: Vec<f32> = ROW_Y.windows(2).map(|pair| pair[1] - pair[0]).collect();
        assert_eq!(
            deltas,
            vec![
                23.0, 23.0, 23.0, 23.0, 23.0, 23.0, 23.0, 22.0, 22.0, 22.0, 22.0, 23.0, 23.0, 23.0
            ]
        );
        assert_eq!(*ROW_Y.last().unwrap(), 549.0);
        // the pitch law over the *bed* (364), not the blacksquare shell (370)
        assert_eq!(
            ((RESULT_BED.3 - 1.0) / 23.0).floor() as usize,
            ROWS_PER_PAGE
        );
        // The header row abuts row 1 on the arts' *opaque* height, not their
        // file height: 209 + 22 == 231. Three of the four distinct header arts
        // are opaque 0..21 of 24 (only `gil_subj_button11` fills all 24), which
        // is also why the drawn rect keeps the full 24 — cropping it would move
        // authored art. Vertically the chain closes the same way it does
        // horizontally.
        assert_eq!(HEADER_Y + HEADER_OPAQUE_H, ROW_Y[0]);
        assert!(ROW_Y[0] > RESULT_BED.1);
        // The band is 231..573 against a bed that ends at 572: the last row
        // overhangs by exactly 1 px, which is the same hand-authoring off-by-one
        // the doc records for this tree (§3.5). Flush within 1 px is the honest
        // claim; strictly inside would be false.
        assert_eq!(ROW_Y[14] + ROW_H, RESULT_BED.1 + RESULT_BED.3 + 1.0);
        // a 16th row would miss the bed by a whole row, which is the property
        // this test cares about
        assert!(ROW_Y[14] + 2.0 * ROW_H > RESULT_BED.1 + RESULT_BED.3);
    }

    /// Every row field lands inside its header column (6 of 6), with the icon
    /// column at the slot's authored 16x16.
    #[test]
    fn row_fields_land_in_their_columns() {
        let fields = [
            FIELD_NUM,
            FIELD_ICON,
            FIELD_NAME,
            FIELD_EA,
            FIELD_LEV,
            FIELD_PRICE,
        ];
        for (field, (x, w, ..)) in fields.iter().zip(HEADERS.iter()) {
            let left = ROW_X + field.0;
            assert!(left >= *x, "{field:?} starts left of its column");
            assert!(left + field.2 <= x + w, "{field:?} overruns its column");
        }
        assert_eq!((FIELD_ICON.2, FIELD_ICON.3), (16.0, 16.0));
    }

    /// The `opt_inner_box_` insets are proven by its art: left side 20 wide,
    /// mid_up 28 tall, so the criteria box's interior is exactly `_BG_TILE_01`.
    #[test]
    fn inner_box_interior_is_the_criteria_tile() {
        assert_eq!(INNER_BOX.0 + 20.0, BG_TILE_01.0);
        assert_eq!(INNER_BOX.2 - 40.0, BG_TILE_01.2);
        assert_eq!(INNER_BOX.1 + 28.0, BG_TILE_01.1);
    }

    /// The ITEM column is a `CIFStatic`, so five of the six headers sort.
    #[test]
    fn item_column_does_not_sort() {
        assert_eq!(StallSortKey::from_column(1), None);
        let keys: Vec<_> = (0..6).filter_map(StallSortKey::from_column).collect();
        assert_eq!(keys.len(), 5);
    }

    #[test]
    fn sorting_picks_a_key_then_flips_direction() {
        let mut state = StallNetworkState {
            rows: vec![
                row(1, "Blade", 3, 20, 900),
                row(2, "Axe", 1, 40, 100),
                row(3, "Bow", 2, 30, 500),
            ],
            ..default()
        };
        // default: the server's own ordering
        assert_eq!(state.sorted_indices(), vec![0, 1, 2]);
        state.sort_by_column(5); // PRICE
        assert_eq!(state.sorted_indices(), vec![1, 2, 0]);
        state.sort_by_column(5); // same column flips
        assert!(state.descending);
        assert_eq!(state.sorted_indices(), vec![0, 2, 1]);
        state.sort_by_column(4); // LEVEL, direction resets
        assert!(!state.descending);
        assert_eq!(state.sorted_indices(), vec![0, 2, 1]);
        // the inert ITEM column changes nothing
        let before = state.sort;
        state.sort_by_column(1);
        assert_eq!(state.sort, before);
    }

    /// Paging, not scrolling: 15 rows per page and the strip never vanishes.
    #[test]
    fn pages_hold_fifteen_rows() {
        let mut state = StallNetworkState::default();
        assert_eq!(state.page_count(), 1);
        assert!(state.page_indices().is_empty());
        state.rows = (0..16)
            .map(|index| row(index as u32, "Item", 1, 1, index as u64))
            .collect();
        assert_eq!(state.page_count(), 2);
        assert_eq!(state.page_indices().len(), ROWS_PER_PAGE);
        state.page = 1;
        assert_eq!(state.page_indices(), vec![15]);
        // a page index past the end clamps instead of drawing nothing
        state.page = 9;
        assert_eq!(state.page_indices(), vec![15]);
    }
}
