//! The talisman collection book (`res_ui/nifcollectionwnd.2dt`, `CNIFCollectionWnd`
//! id 210) — a 4th-gen-only window whose entire content ships in textdata.
//!
//! Idea: unlike every other window in this lane the book needs almost nothing
//! from the server. `collectionbook_theme.txt` (4 themes) and
//! `collectionbook_item.txt` (32 talismans) carry the whole book, and
//! `4 x 8 = 32` re-adds exactly — which settles the layout question: the
//! window's 4x2 grid of 88x128 slots is **one theme's complete set**, not a
//! scrolled view. So the book is useful read-only today; only the register
//! round-trip (which slots the player owns) needs the wire, and that is UNKNOWN
//! (`docs/re/ui/hud-collection-book.md` §U1) — nothing here paints an ownership
//! state, because inventing one would be inventing data.
//!
//! Geometry is the descriptor's, re-read from the user's `nifcollectionwnd.2dt`
//! (67 records) and expressed **board-local**: the authored window nests an
//! `mframe_wnd_` shell around an `int_window_` board at `255,179,663,344`, and
//! every rect below is `authored - (255, 179)`. Our own framed window supplies
//! the shell, so the nesting is not reproduced — the board *is* our content
//! area. That is the only structural deviation.
//!
//! Two more, both stated:
//!
//! * **Uniform 23px theme-row pitch.** The authored rows step `22, 23, 23, 23,
//!   23, 23`; the 22 is hand-authoring noise, and a uniform pitch is what makes
//!   the list a loop over N themes instead of seven hardcoded rows. The
//!   descriptor authors **seven** rows for **four** themes, so three were always
//!   spare — we draw one row per theme.
//! * **The 13-frame `com_collection_effect.ddj` sheet is not drawn.** Only slot
//!   1 binds it in the data (the other seven statics carry `Background=""`), and
//!   its authored U (`0.0769`) is a rounded `1/13`, so sampling the stored
//!   constant bleeds 0.04px into the next frame. Sampling `frame/13` is the fix
//!   when the animation is built; until then no effect is better than a wrong
//!   one.

use bevy::prelude::*;
use bevy::ui_widgets::{Activate, Button};

use crate::assets::textdata::collectionbook::CollectionTheme;
use crate::assets::FontAssets;
use crate::plugins::hud::game_window::{abs_node, spawn_game_window};
use crate::plugins::hud::scale::hud_scale;
use crate::plugins::textdata::{ClientCollectionBook, ClientTextNames, ClientUiStrings};

/// The `int_window_` board (`255,179,663,344`) is our content area; every rect
/// below is board-local.
const CONTENT_W: f32 = 663.0;
const CONTENT_H: f32 = 344.0;

/// Theme pane: `com_bg_tile_e` fill, its `com_bar02_left` header, and the rows.
const THEME_FILL: (f32, f32, f32, f32) = (15.0, 20.0, 256.0, 181.0);
const THEME_HEADER: (f32, f32, f32, f32) = (14.0, 19.0, 245.0, 24.0);
const THEME_ROW_X: f32 = 14.0;
const THEME_ROW_Y: f32 = 41.0;
const THEME_ROW_W: f32 = 244.0;
const THEME_ROW_H: f32 = 24.0;
/// Authored `22, 23, 23, 23, 23, 23` — uniform here (see the module note).
const THEME_ROW_PITCH: f32 = 23.0;
/// Label inset inside a row (`274,y+5,226,16` authored).
const THEME_LABEL_INSET: (f32, f32, f32, f32) = (5.0, 5.0, 226.0, 16.0);

/// Item pane: fill, header, and the 4x2 grid of 88x128 slots.
const ITEM_FILL: (f32, f32, f32, f32) = (283.0, 19.0, 364.0, 276.0);
const ITEM_HEADER: (f32, f32, f32, f32) = (281.0, 17.0, 352.0, 24.0);
const SLOT_X0: f32 = 280.0;
const SLOT_Y0: f32 = 39.0;
const SLOT_W: f32 = 88.0;
const SLOT_H: f32 = 128.0;
/// Authored slot origins step 88 across and 129 down (`218` → `347`).
const SLOT_PITCH_Y: f32 = 129.0;
const SLOT_COLS: usize = 4;
/// `com_collection_deco_window.ddj`, inset 3px inside the slot (83x123).
const DECO_INSET: f32 = 3.0;
const DECO_W: f32 = 83.0;
const DECO_H: f32 = 123.0;
/// Icons are the 32x32 item icons, centred in the slot.
const ICON: f32 = 32.0;
/// `com_lattice_outline_` is a SIX-piece family — no `mid_up`/`mid_down` exist,
/// so the ring is corners plus sides only. Pieces are 4x4.
const OUTLINE_PIECE: f32 = 4.0;

/// Story pane at the bottom of the theme column.
const STORY_FILL: (f32, f32, f32, f32) = (15.0, 213.0, 256.0, 81.0);
const STORY_TEXT: (f32, f32, f32, f32) = (18.0, 216.0, 250.0, 75.0);

const BG_TILE_E: &str = "media://interface/ifcommon/bg_tile/com_bg_tile_e.ddj";
const BAR01: &str = "media://interface/ifcommon/com_bar01_left.ddj";
const BAR02: &str = "media://interface/ifcommon/com_bar02_left.ddj";
const DECO: &str = "media://interface/ifcommon/com_collection_deco_window.ddj";
const OUTLINE_DIR: &str = "media://interface/ifcommon/lattice_window/com_lattice_outline_";

const HEADER_FONT: f32 = 9.0;
const ROW_FONT: f32 = 9.0;
const STORY_FONT: f32 = 8.0;

#[derive(Resource, Default)]
pub struct CollectionWindowState {
    pub open: bool,
    /// Index into the theme table.
    pub theme: usize,
    /// Slot whose story is shown, if any.
    pub slot: Option<usize>,
}

#[derive(Component)]
pub struct CollectionWindowRoot;

/// The rebuilt part of the window (everything but the shell).
#[derive(Component)]
pub struct CollectionBoard;

#[derive(Component, Clone, Copy)]
pub struct ThemeRow(pub usize);

#[derive(Component, Clone, Copy)]
pub struct TalismanSlot(pub usize);

pub fn spawn_collection_window(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    fonts: Res<FontAssets>,
    ui_strings: Res<ClientUiStrings>,
    cameras: Query<Entity, With<Camera2d>>,
    mut state: ResMut<CollectionWindowState>,
) {
    let Ok(camera) = cameras.single() else {
        warn!("collection book: no 2d camera to attach to");
        return;
    };
    *state = CollectionWindowState::default();

    let window = spawn_game_window(
        &mut commands,
        &asset_server,
        &fonts,
        camera,
        ui_strings.get_or("UIIT_PAG_COLLECTION_WINDOW", "Collection book"),
        (CONTENT_W, CONTENT_H),
        (120.0, 120.0),
        hud_scale(),
    );
    commands
        .entity(window.root)
        .insert((CollectionWindowRoot, GlobalZIndex(20)));
    commands.entity(window.expect_close_button()).observe(
        |_: On<Activate>, mut state: ResMut<CollectionWindowState>| {
            state.open = false;
        },
    );
    commands
        .entity(window.content)
        .insert(CollectionBoard)
        .despawn_related::<Children>();
}

/// Rebuild the board whenever the selection changes (and once on load, when the
/// tables arrive).
pub fn refresh_collection_window(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    fonts: Res<FontAssets>,
    ui_strings: Res<ClientUiStrings>,
    names: Res<ClientTextNames>,
    book: Res<ClientCollectionBook>,
    state: Res<CollectionWindowState>,
    boards: Query<Entity, With<CollectionBoard>>,
) {
    if !state.is_changed() && !book.is_changed() {
        return;
    }
    let Some(table) = book.table() else { return };
    let s = hud_scale();
    let text = |content: String, size: f32, rect: (f32, f32, f32, f32), font: Handle<Font>| {
        (
            Text::new(content),
            TextFont {
                font: font.into(),
                font_size: FontSize::Px(size * s),
                ..default()
            },
            TextColor(Color::WHITE),
            abs_node(rect, s),
            Pickable::IGNORE,
        )
    };

    for board in boards.iter() {
        commands.entity(board).despawn_related::<Children>();
        commands.entity(board).with_children(|page| {
            // --- theme column -------------------------------------------------
            page.spawn((
                abs_node(THEME_FILL, s),
                tile(&asset_server, BG_TILE_E, s),
                Pickable::IGNORE,
            ));
            page.spawn((
                abs_node(THEME_HEADER, s),
                stretch(&asset_server, BAR02),
                Pickable::IGNORE,
            ));
            page.spawn(text(
                ui_strings
                    .get_or("UIIT_STT_COLLECTION_WINDOW_THEME", "Theme")
                    .to_string(),
                HEADER_FONT,
                (
                    THEME_HEADER.0 + THEME_LABEL_INSET.0,
                    THEME_HEADER.1 + THEME_LABEL_INSET.1,
                    THEME_LABEL_INSET.2,
                    THEME_LABEL_INSET.3,
                ),
                fonts.nine.clone(),
            ));

            for (index, theme) in table.themes.iter().enumerate() {
                let y = THEME_ROW_Y + index as f32 * THEME_ROW_PITCH;
                page.spawn((
                    ThemeRow(index),
                    Button,
                    abs_node((THEME_ROW_X, y, THEME_ROW_W, THEME_ROW_H), s),
                    stretch(&asset_server, BAR01),
                ))
                .observe(
                    |activate: On<Activate>,
                     rows: Query<&ThemeRow>,
                     mut state: ResMut<CollectionWindowState>| {
                        if let Ok(row) = rows.get(activate.entity) {
                            state.theme = row.0;
                            state.slot = None;
                        }
                    },
                );
                page.spawn(text(
                    theme_label(&names, theme),
                    ROW_FONT,
                    (
                        THEME_ROW_X + THEME_LABEL_INSET.0,
                        y + THEME_LABEL_INSET.1,
                        THEME_LABEL_INSET.2,
                        THEME_LABEL_INSET.3,
                    ),
                    fonts.nine.clone(),
                ));
            }

            // --- story pane ---------------------------------------------------
            page.spawn((
                abs_node(STORY_FILL, s),
                tile(&asset_server, BG_TILE_E, s),
                Pickable::IGNORE,
            ));
            let theme = table.themes.get(state.theme);
            let story = theme
                .map(|theme| {
                    let slots = table.theme_items(theme);
                    match state
                        .slot
                        .and_then(|slot| slots.get(slot).copied().flatten())
                    {
                        // a picked talisman shows its story
                        Some(item) => names.name(&item.story_key).unwrap_or_default().to_string(),
                        // otherwise the theme's own description
                        None => names.name(&theme.desc_key).unwrap_or_default().to_string(),
                    }
                })
                .unwrap_or_default();
            page.spawn(text(story, STORY_FONT, STORY_TEXT, fonts.nine.clone()));

            // --- item grid ----------------------------------------------------
            page.spawn((
                abs_node(ITEM_FILL, s),
                tile(&asset_server, BG_TILE_E, s),
                Pickable::IGNORE,
            ));
            page.spawn((
                abs_node(ITEM_HEADER, s),
                stretch(&asset_server, BAR02),
                Pickable::IGNORE,
            ));
            page.spawn(text(
                ui_strings
                    .get_or(
                        "UIIT_STT_COLLECTION_WINDOW_COLLECTION_CONTENT",
                        "Collection",
                    )
                    .to_string(),
                HEADER_FONT,
                (
                    ITEM_HEADER.0 + THEME_LABEL_INSET.0,
                    ITEM_HEADER.1 + THEME_LABEL_INSET.1,
                    THEME_LABEL_INSET.2,
                    THEME_LABEL_INSET.3,
                ),
                fonts.nine.clone(),
            ));

            let Some(theme) = theme else { return };
            for (index, item) in table.theme_items(theme).into_iter().enumerate() {
                let rect = slot_rect(index);
                page.spawn((TalismanSlot(index), Button, abs_node(rect, s)))
                    .observe(
                        |activate: On<Activate>,
                         slots: Query<&TalismanSlot>,
                         mut state: ResMut<CollectionWindowState>| {
                            if let Ok(slot) = slots.get(activate.entity) {
                                state.slot = Some(slot.0);
                            }
                        },
                    );
                // the six-piece outline ring (no mid pieces exist in this family)
                let (x, y, w, h) = rect;
                let c = OUTLINE_PIECE;
                for (name, piece) in [
                    ("left_up", (x, y, c, c)),
                    ("right_up", (x + w - c, y, c, c)),
                    ("left_down", (x, y + h - c, c, c)),
                    ("right_down", (x + w - c, y + h - c, c, c)),
                    ("left_side", (x, y + c, c, h - 2.0 * c)),
                    ("right_side", (x + w - c, y + c, c, h - 2.0 * c)),
                ] {
                    page.spawn((
                        abs_node(piece, s),
                        stretch(&asset_server, &format!("{OUTLINE_DIR}{name}.ddj")),
                        Pickable::IGNORE,
                    ));
                }
                page.spawn((
                    abs_node((x + DECO_INSET, y + DECO_INSET, DECO_W, DECO_H), s),
                    stretch(&asset_server, DECO),
                    Pickable::IGNORE,
                ));
                if let Some(item) = item {
                    page.spawn((
                        abs_node((x + (w - ICON) / 2.0, y + (h - ICON) / 2.0, ICON, ICON), s),
                        stretch(&asset_server, &format!("media://{}", item.icon)),
                        Pickable::IGNORE,
                    ));
                }
            }
        });
    }
}

/// Slot `index` of a theme's grid, board-local.
fn slot_rect(index: usize) -> (f32, f32, f32, f32) {
    let (col, row) = (index % SLOT_COLS, index / SLOT_COLS);
    (
        SLOT_X0 + col as f32 * SLOT_W,
        SLOT_Y0 + row as f32 * SLOT_PITCH_Y,
        SLOT_W,
        SLOT_H,
    )
}

/// A theme's display name: the `SN_*` key against `textdata_object.txt`, with
/// the authored code as the visible fallback (never an invented string).
fn theme_label(names: &ClientTextNames, theme: &CollectionTheme) -> String {
    names
        .name(&theme.name_key)
        .unwrap_or(&theme.code)
        .to_string()
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

pub fn apply_collection_window_visibility(
    state: Res<CollectionWindowState>,
    mut roots: Query<&mut Node, With<CollectionWindowRoot>>,
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

pub fn cleanup_collection_window(
    mut commands: Commands,
    roots: Query<Entity, With<CollectionWindowRoot>>,
    mut state: ResMut<CollectionWindowState>,
) {
    for root in roots.iter() {
        commands.entity(root).despawn();
    }
    *state = CollectionWindowState::default();
}

#[cfg(test)]
mod test {
    use super::*;

    /// The 4x2 grid, board-local, against the authored slot origins
    /// (`535/623/711/799` x, `218`/`347` y, minus the board origin `255,179`).
    #[test]
    fn slot_rects_match_the_authored_grid() {
        assert_eq!(slot_rect(0), (280.0, 39.0, 88.0, 128.0)); // 535,218
        assert_eq!(slot_rect(3), (544.0, 39.0, 88.0, 128.0)); // 799,218
        assert_eq!(slot_rect(4), (280.0, 168.0, 88.0, 128.0)); // 535,347
        assert_eq!(slot_rect(7), (544.0, 168.0, 88.0, 128.0)); // 799,347
                                                               // the whole grid stays inside the board
        for index in 0..8 {
            let (x, y, w, h) = slot_rect(index);
            assert!(x + w <= CONTENT_W && y + h <= CONTENT_H);
        }
    }

    /// A theme is a full page: 4 themes x 8 items = the 32 authored rows, which
    /// is exactly the 4x2 grid twice over — so the grid is one theme's set and
    /// never needs to scroll.
    #[test]
    fn one_theme_fills_the_grid_exactly() {
        assert_eq!(SLOT_COLS * 2, 8);
        // The pane holds the two authored rows and no more. It holds them to
        // within 1 px, not strictly: the second row's authored bottom
        // (`347 + 128 = 475`) sits one pixel below the item pane's authored
        // bottom (`198 + 276 = 474`). That is the same hand-authoring
        // off-by-one `docs/re/ui/hud-collection-book.md` reports elsewhere in
        // this descriptor (the frame client area is 1 px short of entry `[1]`,
        // §3.3; consecutive theme bars overlap by 1 px, §3.4), so the honest
        // claim is "flush within the authored 1 px", not "strictly inside".
        let (_, y, _, h) = slot_rect(4);
        assert!(y + h <= ITEM_FILL.1 + ITEM_FILL.3 + 1.0);
        // and a third row would miss the pane by a whole slot, which is the
        // property this test actually cares about
        assert!(SLOT_Y0 + 2.0 * SLOT_PITCH_Y + SLOT_H > ITEM_FILL.1 + ITEM_FILL.3);
        assert!(slot_rect(0).1 >= ITEM_FILL.1);
    }

    /// The theme list is a loop over N themes at a uniform pitch; the authored
    /// seven rows are spare capacity for four themes.
    #[test]
    fn theme_rows_step_by_a_uniform_pitch() {
        assert_eq!(THEME_ROW_PITCH, 23.0);
        // four themes fit above the story pane
        let last = THEME_ROW_Y + 3.0 * THEME_ROW_PITCH + THEME_ROW_H;
        assert!(last <= STORY_FILL.1);
        // and so would the authored seven
        assert!(THEME_ROW_Y + 6.0 * THEME_ROW_PITCH + THEME_ROW_H <= STORY_FILL.1 + STORY_FILL.3);
    }
}

/// Self-registration for the collection book (#558). The HUD registry holds one
/// line per window, so two windows landing in the same lap no longer collide on
/// it — this carries the wiring that used to live in `hud/mod.rs`.
pub struct CollectionPlugin;

impl Plugin for CollectionPlugin {
    fn build(&self, app: &mut App) {
        use crate::scenes::SceneState;

        app.init_resource::<CollectionWindowState>()
            .add_systems(OnEnter(SceneState::GameWorld), spawn_collection_window)
            .add_systems(OnExit(SceneState::GameWorld), cleanup_collection_window)
            // built on enter, shown on demand; the content is rebuilt when the
            // selected theme changes.
            .add_systems(
                Update,
                (
                    apply_collection_window_visibility,
                    refresh_collection_window,
                )
                    .chain()
                    .run_if(in_state(SceneState::GameWorld)),
            );
    }
}
