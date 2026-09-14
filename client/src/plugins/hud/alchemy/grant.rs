//! The avatar magic-option grant dialog — `GDR_GRANT_MAGIC_ATTRIBUTE`, ID 156.
//!
//! Idea: `ginterface.txt:1904-1927` declares this window `Rect="0,0,272,395"`
//! behind `#ifdef APPLY_AVATAR_SYSTEM`, and that symbol *is* defined
//! (`config/define.txt:10`), so it is live in the user's own data. Its body is
//! `resinfo/ifgrantmagicattributewnd.txt`: an item slot over a description
//! block, then a scrollable option list whose rows are the 215x24 prototype of
//! `resinfo/ifgrantmagicattributeslot.txt` — instantiated here, never
//! re-measured off the parent rect.
//!
//! **The rects are rebased, and the arithmetic is the source.** The window's
//! `DDJ` is `interface\messagebox\msgbox2_window_`, the shared modal plate,
//! whose art insets are 16 at the sides, 40 at the top and 16 at the bottom
//! (`hud/modal_dialog.rs`). The tree closes on those insets from the other
//! side: `_MAIN_BG1` is `16,39,240,340`, and `16 + 240 == 256 == 272 - 16`
//! while `39 + 340 == 379 == 395 - 16`. So the plate's interior is
//! `240 x 339`, and every authored rect below is translated by `(-16, -40)`
//! into that interior. The window's own `Text`
//! (`UIIT_STT_ALCHEMYBOX_ENCHANT_MAGIC_PARAM`) rides the plate's top strip,
//! i.e. it is a caption band — which is why this is hosted on the shared
//! `game_window` chrome the alchemy box uses, exactly as
//! `docs/re/ui/alchemy-window.md` §8-3 prescribes, rather than re-drawing the
//! plate. `_MAIN_BG1` and the button row each overhang the interior by one
//! unit (39 vs 40, 356+24 vs 355); both are kept verbatim rather than nudged.
//!
//! Nothing is sent to the server. `docs/re/systems/alchemy.md`'s opcode map is
//! `[S]`-inferred, not captured, so Confirm is drawn in the disabled state for
//! the same reason the alchemy box draws its Fuse button that way
//! (`alchemy/ui.rs:16-19`). The OLD-vs-NEW shell question (§9) is untouched:
//! this dialog is its own `ginterface` entry and does not depend on it.

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui_widgets::Activate;

use crate::assets::FontAssets;
use crate::plugins::hud::game_window::{self, abs_node};
use crate::plugins::hud::inventory::model::InventoryState;
use crate::plugins::hud::inventory::ui::DragGhost;
use crate::plugins::hud::modal_dialog::{MODAL_BOTTOM, MODAL_SIDE, MODAL_TOP};
use crate::plugins::hud::scale::hud_scale;
use crate::plugins::net::inventory::Inventory;
use crate::plugins::player::Player;
use crate::plugins::textdata::{ClientItemData, ClientTextNames, ClientUiStrings};

// --- Layout constants (resinfo/ifgrantmagicattributewnd.txt, window units) --

/// `ginterface.txt:1904-1927` `Rect="0,0,272,395"`.
const WINDOW_SIZE: (f32, f32) = (272.0, 395.0);
/// The plate interior the authored rects live in: `272 - 2*16` by
/// `395 - 40 - 16` (`hud/modal_dialog.rs` insets, confirmed by `_MAIN_BG1`).
const CONTENT_W: f32 = WINDOW_SIZE.0 - 2.0 * MODAL_SIDE;
const CONTENT_H: f32 = WINDOW_SIZE.1 - MODAL_TOP - MODAL_BOTTOM;

/// Translate an authored window-space rect into the plate interior.
const fn rebase(rect: (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
    (rect.0 - MODAL_SIDE, rect.1 - MODAL_TOP, rect.2, rect.3)
}

/// `GDR_GRANT_MAGIC_ATTRIBUTE_MAIN_BG1:CIFNormalTile` `16,39,240,340`.
const MAIN_BG_RECT: (f32, f32, f32, f32) = rebase((16.0, 39.0, 240.0, 340.0));
/// `_BLACKSQUARE_1` `18,44,236,148` over `_BG2` `22,48,228,139` — the item box.
const ITEM_PLATE_RECT: (f32, f32, f32, f32) = rebase((18.0, 44.0, 236.0, 148.0));
const ITEM_BG_RECT: (f32, f32, f32, f32) = rebase((22.0, 48.0, 228.0, 139.0));
/// `_BLACKSQUARE_2` `18,202,236,143` over `_BG3` `22,206,228,135` — the list.
const LIST_PLATE_RECT: (f32, f32, f32, f32) = rebase((18.0, 202.0, 236.0, 143.0));
const LIST_BG_RECT: (f32, f32, f32, f32) = rebase((22.0, 206.0, 228.0, 135.0));
const BG_TILE_B: &str = "media://interface/ifcommon/bg_tile/com_bg_tile_b.ddj";
const BG_TILE_E: &str = "media://interface/ifcommon/bg_tile/com_bg_tile_e.ddj";
const BLACKSQUARE_PIECE: f32 = 4.0;
const BLACKSQUARE_DIR: &str = "media://interface/ifcommon/com_blacksquare_";
/// `_ITEM_WND:CIFStatic` `30,56,0,0` — `Rect` w,h are `0,0`, so the size is
/// the art's: `msgbox_itemwindow.ddj` is 48x48 (DDS header).
const ITEM_WND_RECT: (f32, f32, f32, f32) = rebase((30.0, 56.0, 48.0, 48.0));
const ITEM_WND_DDJ: &str = "media://interface/messagebox/msgbox_itemwindow.ddj";
/// `_ITEM_SLOT:CIFSlotWithHelp` `37,63,32,32`.
const ITEM_SLOT_RECT: (f32, f32, f32, f32) = rebase((37.0, 63.0, 32.0, 32.0));
/// `_ITEM_NAME:CIFStatic` `90,73,153,15`.
const ITEM_NAME_RECT: (f32, f32, f32, f32) = rebase((90.0, 73.0, 153.0, 15.0));
/// `_DESC:CIFPML` `29,114,213,45`, `Text="UIIT_STT_AVATAR_MAGICOPTION_GUIDE"`.
const DESC_RECT: (f32, f32, f32, f32) = rebase((29.0, 114.0, 213.0, 45.0));
/// `_LIST_OPTION_WND:CIFBarWnd` `20,204,215,24`, `Text="UIIT_CTL_OPTION"`,
/// art `com_bar02_` (left/right caps are 12x24, mid is 24x24 — DDS headers).
const LIST_HEADER_RECT: (f32, f32, f32, f32) = rebase((20.0, 204.0, 215.0, 24.0));
const BAR02_DIR: &str = "media://interface/ifcommon/com_bar02_";
const BAR02_CAP: f32 = 12.0;
/// `_LIST_DUMY:CIFStatic` `235,204,0,0`, art `gil_shape.ddj` (16x24).
const LIST_CAP_RECT: (f32, f32, f32, f32) = rebase((235.0, 204.0, 16.0, 24.0));
const LIST_CAP_DDJ: &str = "media://interface/guild/gil_shape.ddj";
/// `_SLOT_BAR_01..05:CIFSlot`, `20,{226,249,272,295,318},215,24` — five rows
/// on a 23-unit pitch. The **size** is the row prototype's, not this rect's:
/// `ifgrantmagicattributeslot.txt`'s single control is `0,0,215,24`.
const ROW_PROTOTYPE: (f32, f32) = (215.0, 24.0);
const ROW_X: f32 = 20.0;
const ROW_YS: [f32; 5] = [226.0, 249.0, 272.0, 295.0, 318.0];
const BAR01_DIR: &str = "media://interface/ifcommon/com_bar01_";
const BAR01_CAP: f32 = 4.0;
/// `_LIST_SCROLLMGR:CIFScrollManager` `20,226,231,119` — the scroll viewport
/// the five rows live in. [U] how many rows the manager can page through; the
/// data declares five instantiated rows and nothing about the total.
const LIST_VIEWPORT_RECT: (f32, f32, f32, f32) = rebase((20.0, 226.0, 231.0, 119.0));
/// `_CONFIRM_BTN` `55,356,0,0` and `_CANCEL_BTN` `143,356,0,0` — `Rect` w,h
/// are `0,0`, so the size is `com_button.ddj`'s 76x24 (DDS header, as #597).
const BUTTON_Y: f32 = 356.0;
const BUTTON_SIZE: (f32, f32) = (76.0, 24.0);
const CONFIRM_X: f32 = 55.0;
const CANCEL_X: f32 = 143.0;
const BUTTON_DDJ: &str = "media://interface/ifcommon/com_button.ddj";

/// Spawn anchor (right/top, physical px). **Ours**: `wndpos.dat` does not
/// persist this window, so vanilla's own default position is not in the data.
const WINDOW_RIGHT: f32 = 420.0;
const WINDOW_TOP: f32 = 90.0;

const LABEL_COLOR: Color = Color::srgb(0.92, 0.92, 0.92);
/// Confirm is inert (see the module doc), so its label is dimmed the way the
/// alchemy box dims its disabled Fuse button.
const DISABLED_TEXT_COLOR: Color = Color::srgb_u8(140, 135, 120);

// --- State ------------------------------------------------------------------

/// Open/closed state of the grant dialog plus what its one slot references.
///
/// The slot holds an **inventory wire slot**, not an item — the same model
/// `alchemy/model.rs` uses, and for the same reason: the vanilla dialog does
/// not move the item, the server does when it acks.
#[derive(Resource, Default)]
pub struct GrantState {
    pub open: bool,
    item: Option<u8>,
    selected: Option<usize>,
}

impl GrantState {
    /// The inventory wire slot the item box references.
    pub fn item(&self) -> Option<u8> {
        self.item
    }

    /// Put an inventory item in the box (there is exactly one slot).
    pub fn place(&mut self, inventory_slot: u8) {
        self.item = Some(inventory_slot);
    }

    /// Empty the box, returning what was in it.
    pub fn take(&mut self) -> Option<u8> {
        self.item.take()
    }

    /// The highlighted option row, if any.
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// Highlight one of the five instantiated rows; anything past them is
    /// ignored rather than panicking (the UI feeds this from cells).
    pub fn select(&mut self, row: usize) {
        if row < ROW_YS.len() {
            self.selected = Some(row);
        }
    }

    /// Closing releases the reference and the selection.
    pub fn clear(&mut self) {
        self.item = None;
        self.selected = None;
    }
}

// --- Markers ----------------------------------------------------------------

#[derive(Component)]
pub struct GrantWindowRoot;

/// Despawn marker, mirroring the alchemy box's two-phase close.
#[derive(Component)]
pub struct GrantClosing;

/// The single `CIFSlotWithHelp` item cell.
#[derive(Component)]
pub struct GrantItemCell;

/// One of the five instantiated option rows.
#[derive(Component)]
pub struct GrantOptionRow(pub usize);

// --- Spawning ---------------------------------------------------------------

/// Rebuild the dialog whenever [`GrantState`] changes.
pub fn sync_grant_window(
    state: Res<GrantState>,
    existing: Query<(Entity, &Node), With<GrantWindowRoot>>,
    inventories: Query<&Inventory, With<Player>>,
    item_data: Res<ClientItemData>,
    names: Res<ClientTextNames>,
    ui_strings: Res<ClientUiStrings>,
    fonts: Res<FontAssets>,
    asset_server: Res<AssetServer>,
    cam_query: Query<Entity, With<Camera2d>>,
    mut commands: Commands,
) {
    if !state.is_changed() {
        return;
    }
    let mut anchor = (WINDOW_RIGHT, WINDOW_TOP);
    for (entity, node) in existing.iter() {
        if let (Val::Px(right), Val::Px(top)) = (node.right, node.top) {
            anchor = (right, top);
        }
        commands.entity(entity).insert(GrantClosing);
    }
    if !state.open {
        return;
    }
    let Ok(camera) = cam_query.single() else {
        warn!("grant dialog: no 2d camera to attach to");
        return;
    };
    let s = hud_scale();
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

    let window = game_window::spawn_game_window(
        &mut commands,
        &asset_server,
        &fonts,
        camera,
        ui_strings.get_or(
            "UIIT_STT_ALCHEMYBOX_ENCHANT_MAGIC_PARAM",
            "Magic Option Grant",
        ),
        (CONTENT_W, CONTENT_H),
        anchor,
        s,
    );
    commands.entity(window.root).insert((
        GrantWindowRoot,
        GlobalZIndex(58),
        // See `AlchemyWindowRoot`: this window hovers its cells, so the
        // root needs `Hovered` for the drop detector to see the window at
        // all.
        Hovered::default(),
    ));
    commands
        .entity(window.expect_close_button())
        .observe(on_close_button);

    let inventory = inventories.single().ok();
    let item_icon = state
        .item
        .and_then(|wire| inventory?.get(wire))
        .and_then(|item| {
            item_data
                .get(&(item.ref_id as i32))
                .and_then(|row| row.icon_path())
        });
    let item_name = state
        .item
        .and_then(|wire| inventory?.get(wire))
        .and_then(|item| item_data.get(&(item.ref_id as i32)))
        .and_then(|row| row.name_key())
        .and_then(|key| names.name(key))
        .unwrap_or_default()
        .to_string();

    commands.entity(window.content).with_children(|content| {
        content.spawn(tile(MAIN_BG_RECT, BG_TILE_B));
        for (plate, bg) in [
            (ITEM_PLATE_RECT, ITEM_BG_RECT),
            (LIST_PLATE_RECT, LIST_BG_RECT),
        ] {
            content.spawn(tile(bg, BG_TILE_E));
            for ((x, y, w, h), piece) in blacksquare(plate) {
                content.spawn(image((x, y, w, h), format!("{BLACKSQUARE_DIR}{piece}.ddj")));
            }
        }

        // the item box: the 48x48 art, the 32x32 slot inside it, the name
        content.spawn(image(ITEM_WND_RECT, ITEM_WND_DDJ.to_string()));
        let mut cell = content.spawn((
            GrantItemCell,
            Hovered::default(),
            abs_node(ITEM_SLOT_RECT, s),
            Pickable::default(),
        ));
        cell.observe(on_item_slot_press);
        if let Some(icon) = item_icon {
            cell.with_children(|slot| {
                slot.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        width: Val::Percent(100.0),
                        height: Val::Percent(100.0),
                        ..default()
                    },
                    ImageNode {
                        image: asset_server.load(icon),
                        image_mode: NodeImageMode::Stretch,
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
            });
        }
        content.spawn((
            Text::new(item_name),
            text_font(8.0),
            TextColor(LABEL_COLOR),
            TextLayout::justify(Justify::Left),
            abs_node(ITEM_NAME_RECT, s),
            Pickable::IGNORE,
        ));

        // the CIFPML guide text
        content.spawn((
            Text::new(ui_strings.get_plain_or(
                "UIIT_STT_AVATAR_MAGICOPTION_GUIDE",
                "Select an avatar item and the magic option to grant it.",
            )),
            text_font(8.0),
            TextColor(LABEL_COLOR),
            TextLayout::justify(Justify::Left),
            abs_node(DESC_RECT, s),
            Pickable::IGNORE,
        ));

        // the list header bar + its end cap
        for (rect, piece) in bar(LIST_HEADER_RECT, BAR02_CAP) {
            content.spawn(image(rect, format!("{BAR02_DIR}{piece}.ddj")));
        }
        content.spawn((
            Text::new(ui_strings.get_or("UIIT_CTL_OPTION", "Option").to_string()),
            text_font(8.0),
            TextColor(LABEL_COLOR),
            TextLayout::justify(Justify::Center),
            abs_node(LIST_HEADER_RECT, s),
            Pickable::IGNORE,
        ));
        content.spawn(image(LIST_CAP_RECT, LIST_CAP_DDJ.to_string()));

        // The five instantiated rows, at the prototype's own 215x24, inside
        // the `_LIST_SCROLLMGR` viewport so they are clipped by the rect the
        // data gives the scroll manager rather than by the window.
        let mut viewport = abs_node(LIST_VIEWPORT_RECT, s);
        viewport.overflow = Overflow::clip();
        content
            .spawn((viewport, Pickable::IGNORE))
            .with_children(|list| {
                for (row, y) in ROW_YS.iter().enumerate() {
                    // viewport-local: the manager's origin is the first row's
                    // (`20,226` for both), so row 0 sits at (0,0).
                    let rect = (0.0, *y - ROW_YS[0], ROW_PROTOTYPE.0, ROW_PROTOTYPE.1);
                    for (piece_rect, piece) in bar(rect, BAR01_CAP) {
                        list.spawn(image(piece_rect, format!("{BAR01_DIR}{piece}.ddj")));
                    }
                    list.spawn((
                        GrantOptionRow(row),
                        Hovered::default(),
                        abs_node(rect, s),
                        Pickable::default(),
                    ))
                    .observe(on_option_row_press);
                    if state.selected == Some(row) {
                        list.spawn((
                            abs_node(rect, s),
                            BackgroundColor(Color::srgba(1.0, 0.9, 0.5, 0.18)),
                            Pickable::IGNORE,
                        ));
                    }
                }
            });

        // Confirm (inert) + Cancel
        for (x, key, fallback, enabled) in [
            (CONFIRM_X, "UIIS_CTL_CONFIRM", "Confirm", false),
            (CANCEL_X, "UIIS_CTL_CANCEL", "Cancel", true),
        ] {
            let rect = rebase((x, BUTTON_Y, BUTTON_SIZE.0, BUTTON_SIZE.1));
            let mut button = content.spawn(image(rect, BUTTON_DDJ.to_string()));
            if enabled {
                button
                    .insert((Hovered::default(), Pickable::default()))
                    .observe(on_cancel_button);
            }
            button.with_children(|b| {
                b.spawn((
                    Text::new(ui_strings.get_or(key, fallback).to_string()),
                    text_font(8.0),
                    TextColor(if enabled {
                        LABEL_COLOR
                    } else {
                        DISABLED_TEXT_COLOR
                    }),
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

        // [U] The option list itself is server-fed and no capture exists, so
        // the rows are empty and there is no scrollbar art invented for the
        // manager: `ifgrantmagicattributewnd.txt` declares none.
    });
}

/// Despawn the windows marked closing, one frame after they were marked.
pub fn despawn_closing_grant(closing: Query<Entity, With<GrantClosing>>, mut commands: Commands) {
    for entity in closing.iter() {
        commands.entity(entity).despawn();
    }
}

pub fn cleanup_grant(windows: Query<Entity, With<GrantWindowRoot>>, mut commands: Commands) {
    for entity in windows.iter() {
        commands.entity(entity).despawn();
    }
}

/// Drop an inventory drag onto the item box — the alchemy box's own idiom.
pub fn place_drop_on_grant(
    buttons: Res<ButtonInput<MouseButton>>,
    cells: Query<&Hovered, With<GrantItemCell>>,
    ghosts: Query<Entity, With<DragGhost>>,
    mut inv_state: ResMut<InventoryState>,
    mut state: ResMut<GrantState>,
    mut commands: Commands,
) {
    if !buttons.just_released(MouseButton::Left) && !buttons.just_pressed(MouseButton::Left) {
        return;
    }
    let Some(source) = inv_state.drag else {
        return;
    };
    if !cells.iter().any(|hovered| hovered.get()) {
        return;
    }
    inv_state.drag = None;
    for ghost in ghosts.iter() {
        commands.entity(ghost).despawn();
    }
    state.place(source);
}

// --- Behavior ---------------------------------------------------------------

fn on_close_button(_: On<Activate>, mut state: ResMut<GrantState>) {
    state.open = false;
    state.clear();
}

fn on_cancel_button(_: On<Pointer<Press>>, mut state: ResMut<GrantState>) {
    state.open = false;
    state.clear();
}

/// Clicking the filled slot returns the item — the reference is released, no
/// inventory move happens (the server owns that).
fn on_item_slot_press(_: On<Pointer<Press>>, mut state: ResMut<GrantState>) {
    state.take();
}

fn on_option_row_press(
    press: On<Pointer<Press>>,
    rows: Query<&GrantOptionRow>,
    mut state: ResMut<GrantState>,
) {
    if let Ok(row) = rows.get(press.entity) {
        state.select(row.0);
    }
}

/// The 3 pieces of a `com_bar0*_` bar over a rect: two caps and the middle.
fn bar(rect: (f32, f32, f32, f32), cap: f32) -> [((f32, f32, f32, f32), &'static str); 3] {
    let (x, y, w, h) = rect;
    [
        ((x, y, cap, h), "left"),
        ((x + cap, y, w - 2.0 * cap, h), "mid"),
        ((x + w - cap, y, cap, h), "right"),
    ]
}

/// The 6 `com_blacksquare_` trim pieces around a plate.
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

#[cfg(test)]
mod test {
    use super::*;

    /// The plate interior is the art's insets, and the tree closes on them
    /// from the other side (`_MAIN_BG1` `16,39,240,340`).
    #[test]
    fn the_interior_is_what_the_authored_background_implies() {
        assert_eq!((CONTENT_W, CONTENT_H), (240.0, 339.0));
        assert_eq!(16.0 + 240.0, WINDOW_SIZE.0 - MODAL_SIDE);
        assert_eq!(39.0 + 340.0, WINDOW_SIZE.1 - MODAL_BOTTOM);
    }

    /// Rebasing is a pure translation by the plate's own insets — undoing it
    /// must give the authored rect back.
    #[test]
    fn rebasing_is_the_plate_inset_and_nothing_else() {
        for rect in [
            (16.0, 39.0, 240.0, 340.0),
            (20.0, 204.0, 215.0, 24.0),
            (55.0, 356.0, 76.0, 24.0),
        ] {
            let (x, y, w, h) = rebase(rect);
            assert_eq!((x + MODAL_SIDE, y + MODAL_TOP, w, h), rect);
        }
    }

    /// The five rows are the 215x24 prototype of
    /// `ifgrantmagicattributeslot.txt` on the 23-unit pitch the wnd declares —
    /// not a height divided out of the viewport.
    #[test]
    fn rows_are_the_prototype_on_the_authored_pitch() {
        assert_eq!(ROW_PROTOTYPE, (215.0, 24.0));
        assert_eq!(ROW_YS.len(), 5);
        for pair in ROW_YS.windows(2) {
            assert_eq!(pair[1] - pair[0], 23.0);
        }
        // the pitch is tighter than the row, i.e. rows overlap by one unit —
        // vanilla's own numbers, kept rather than rounded to 24.
        assert!(ROW_YS[1] - ROW_YS[0] < ROW_PROTOTYPE.1);
    }

    /// Every authored rect stays inside the 272x395 window.
    #[test]
    fn every_rect_fits_the_authored_window() {
        let mut boxes = vec![
            (16.0, 39.0, 240.0, 340.0),
            (18.0, 44.0, 236.0, 148.0),
            (22.0, 48.0, 228.0, 139.0),
            (18.0, 202.0, 236.0, 143.0),
            (22.0, 206.0, 228.0, 135.0),
            (30.0, 56.0, 48.0, 48.0),
            (37.0, 63.0, 32.0, 32.0),
            (90.0, 73.0, 153.0, 15.0),
            (29.0, 114.0, 213.0, 45.0),
            (20.0, 204.0, 215.0, 24.0),
            (20.0, 226.0, 231.0, 119.0),
        ];
        for x in [CONFIRM_X, CANCEL_X] {
            boxes.push((x, BUTTON_Y, BUTTON_SIZE.0, BUTTON_SIZE.1));
        }
        for y in ROW_YS {
            boxes.push((ROW_X, y, ROW_PROTOTYPE.0, ROW_PROTOTYPE.1));
        }
        for (x, y, w, h) in boxes {
            assert!(
                x + w <= WINDOW_SIZE.0,
                "rect {x},{y},{w},{h} overflows width"
            );
            assert!(
                y + h <= WINDOW_SIZE.1,
                "rect {x},{y},{w},{h} overflows height"
            );
        }
    }

    /// The slot is a *reference* to an inventory slot, released on close —
    /// the alchemy model's contract, and nothing is ever sent.
    #[test]
    fn the_item_slot_holds_an_inventory_reference() {
        let mut state = GrantState::default();
        assert_eq!(state.item(), None);
        state.place(13);
        assert_eq!(state.item(), Some(13));
        state.select(2);
        assert_eq!(state.selected(), Some(2));
        state.select(99);
        assert_eq!(state.selected(), Some(2), "a row past the five is ignored");
        assert_eq!(state.take(), Some(13));
        state.place(7);
        state.clear();
        assert_eq!(state.item(), None);
        assert_eq!(state.selected(), None);
    }
}
