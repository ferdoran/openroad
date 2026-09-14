//! NPC store window layout + buy/sell interaction.
//!
//! Idea: transcribed from `resinfo/ifstore.txt` + ginterface's `GDR_STORE`
//! (a 254x370 mframe window): a 4-tab strip, a 6x5 com_lattice goods grid
//! (32px slots on a 36px pitch, exactly the inventory's lattice anatomy), a
//! page spinner and a detail board that shows the hovered good's name and
//! price (vanilla shows details on hover rather than a floating tooltip).
//! The window is rebuilt from `StoreState` on every session change (open,
//! close, tab, page; a dragged position survives the rebuild) — inside the
//! shared `game_window` chrome. The spinner pages through (tab group,
//! 30-slot chunk) entries — e.g. the armor shop's male/female tab groups; no
//! tab in this media overflows a chunk. Buying is the vanilla carry:
//! pressing a good sticks its ghost to the cursor, dropping it on the open
//! inventory's grid opens the quantity modal (scrim + panel with the item
//! icon and a digits-only amount input), and OK sends the 0x7034 op-8.
//! Dragging a bag item onto the store window opens the same modal in sell
//! mode (op-9), pre-filled with the slot's full stack count. Application of
//! both is server-confirmed only (model.rs).
//!
//! The repurchase ("buy back") strip below the detail board is live in this
//! media (`RESTORE_SOLDITEM_INSHOP` is defined) and is drawn empty on purpose:
//! its frame, label and five slot rects are authored data, but no known packet
//! delivers the sold-item list.

use bevy::input_focus::{FocusCause, InputFocus};
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::text::{EditableText, EditableTextFilter, TextCursorStyle, TextEdit};
use bevy::ui::UiTargetCamera;
use bevy::ui_widgets::{Activate, Button};

use packets::agent::character_data::ItemTypeData;
use packets::agent::prelude::InventoryOperationRequest;
use packets::Packet;

use crate::assets::textdata::shops::ShopCurrency;
use crate::assets::FontAssets;
use crate::net::connection::SilkroadConnection;
use crate::plugins::hud::game_window::{self, abs_node};
use crate::plugins::hud::inventory::model::InventoryState;
use crate::plugins::hud::scale::hud_scale;
use crate::plugins::hud::store::model::{PendingStoreOp, StoreHoveredGood, StoreOp, StoreState};
use crate::plugins::hud::window_positions::PersistedWindow;
use crate::plugins::net::agent::AgentConnection;
use crate::plugins::net::inventory::Inventory;
use crate::plugins::player::Player;
use crate::plugins::settings::window_positions::WndPosSlot;
use crate::plugins::textdata::{ClientItemData, ClientItemIndex, ClientTextNames, ClientUiStrings};
use crate::plugins::ui_v2::style::ImageButtonStyle;

/// The live `GDR_STORE` block: `ginterface.txt:590`, id 15,
/// `CIFStoreForPackage`, `Rect="0,0,254,370"`. Four `GDR_STORE` blocks exist
/// (370 / 317 / 360 / 317); this one is gated on `RENEWAL_SHOP_SYSTEM` **and**
/// `RESTORE_SOLDITEM_INSHOP`, and the user's `config/define.txt` defines both
/// (lines 6 and 8), so 370 is the height and the repurchase strip is live.
const VANILLA_OUTER: (f32, f32) = (254.0, 370.0);

/// Content-space origin, taken from the shell's own exports rather than
/// re-derived. A hand-derived `(12, 26)` used to live here and put every
/// element 10px low while making `CONTENT_H` 16px too tall; three windows
/// independently produced that same wrong `26`, so the rule is to import the
/// pair, never to subtract a fresh number (#314).
const ORIGIN_X: f32 = game_window::FRAME_VIS_SIDE + game_window::CHROME_PAD;
const ORIGIN_Y: f32 = game_window::CONTENT_TOP;

/// An `ifstore.txt` window-space rect in content space. Keeping the vanilla
/// numbers at the call sites means every constant below is checkable against
/// the data file by eye.
const fn content_rect(rect: (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
    (rect.0 - ORIGIN_X, rect.1 - ORIGIN_Y, rect.2, rect.3)
}

// The inverse of `game_window::outer_size`; the test below pins the two
// together against `VANILLA_OUTER`.
const CONTENT_W: f32 = VANILLA_OUTER.0 - 2.0 * ORIGIN_X;
const CONTENT_H: f32 = VANILLA_OUTER.1
    - game_window::CONTENT_TOP
    - game_window::CHROME_PAD
    - game_window::FRAME_VIS_BOTTOM;
const WINDOW_RIGHT: f32 = 300.0;
const WINDOW_TOP: f32 = 60.0;

const GRID_COLS: usize = 6;
const GRID_ROWS: usize = 5;
pub const SLOTS_PER_PAGE: usize = GRID_COLS * GRID_ROWS;
const CELL: f32 = 36.0;
const ICON_INSET: f32 = 2.0;
const ICON_SIZE: f32 = 32.0;
/// `GDR_STORE_LAT` (`ifstore.txt:1479`).
const LATTICE_RECT: (f32, f32, f32, f32) = content_rect((21.0, 71.0, 216.0, 180.0));
// The tab strip is exe-built (no resinfo row). We draw it with the shared
// `com_short_tab_on/off.ddj` (56x24); the store's own `interface/store/str_*_
// tab_*` art measures the same 56x24, so the size is right either way — 4 tabs
// on a 56 pitch, bottom-flush with the frame top.
const TAB_Y: f32 = 10.0;
const TAB_W: f32 = 56.0;
const TAB_H: f32 = 24.0;
/// `GDR_STORE_SPIN_PAGE` (`:1424`): ◄ 16x16, an 18x12 current-page label,
/// ► 16x16 (ifspincontrol.txt anatomy).
const SPIN_RECT: (f32, f32, f32, f32) = content_rect((102.0, 254.0, 50.0, 16.0));
/// `GDR_STORE_DETAIL_BGTILE` (`:1540`).
const DETAIL_RECT: (f32, f32, f32, f32) = content_rect((40.0, 279.0, 174.0, 54.0));
/// `GDR_STORE_BTN_REPAIR` (`:1462`) / `_REPAIRALL` (`:1443`).
const REPAIR_RECT: (f32, f32, f32, f32) = content_rect((86.0, 332.0, 76.0, 24.0));
const REPAIR_ALL_RECT: (f32, f32, f32, f32) = content_rect((166.0, 332.0, 76.0, 24.0));

// The repurchase ("buy back") strip, live because `RESTORE_SOLDITEM_INSHOP` is
// defined. Which packet fills these slots is UNKNOWN — there is no builder, no
// parser and no dump sample — so the strip is built empty: the frame and label
// are data, an item in a slot would be an invention.
/// `GDR_STORE_STRETCH_REDEEM` (`:1403`), `com_redeem_window.ddj`; the art's own
/// 188x40 matches the rect exactly.
const BUYBACK_REDEEM_RECT: (f32, f32, f32, f32) = content_rect((57.0, 285.0, 188.0, 40.0));
/// `GDR_STORE_CHARGY_STATIC` (`:1279`), the strip's label. Its content-space x
/// is -5: vanilla x 7 sits inside the frame's own visible margin
/// (`FRAME_VIS_SIDE` 8), left of where the content container starts.
const BUYBACK_LABEL_RECT: (f32, f32, f32, f32) = content_rect((7.0, 300.0, 43.0, 16.0));
/// `GDR_STORE_ICON_SLOT_01..05` (`:1384`, `:1363`, `:1342`, `:1321`, `:1300`):
/// 32x32 slots at x 61/96/131/166/201, y 289. The pitch is **35**, not the main
/// grid's 36 — at 36 the fifth slot would overrun the 188px redeem art.
const BUYBACK_SLOT_Y: f32 = content_rect((0.0, 289.0, 0.0, 0.0)).1;
const BUYBACK_SLOT_XS: [f32; 5] = [
    content_rect((61.0, 0.0, 0.0, 0.0)).0,
    content_rect((96.0, 0.0, 0.0, 0.0)).0,
    content_rect((131.0, 0.0, 0.0, 0.0)).0,
    content_rect((166.0, 0.0, 0.0, 0.0)).0,
    content_rect((201.0, 0.0, 0.0, 0.0)).0,
];
const BUYBACK_SLOT_SIZE: f32 = 32.0;

const LATTICE_DIR: &str = "media://interface/ifcommon/lattice_window/com_lattice_";
const REDEEM_WINDOW_DDJ: &str = "media://interface/ifcommon/com_redeem_window.ddj";
const TAB_ON_DDJ: &str = "media://interface/ifcommon/com_short_tab_on.ddj";
const TAB_OFF_DDJ: &str = "media://interface/ifcommon/com_short_tab_off.ddj";
// _b like the tab/grid backdrop — the resinfo-suggested _a reads too dark
// next to it (playtest feedback)
const DETAIL_TILE: &str = "media://interface/ifcommon/bg_tile/com_bg_tile_b.ddj";

const PRICE_COLOR: Color = Color::srgb_u8(255, 217, 83);
const NAME_COLOR: Color = Color::srgb_u8(255, 226, 123);

#[derive(Component)]
pub struct StoreWindowRoot;

/// Deferred despawn marker, same trick as the NPC dialog.
#[derive(Component)]
pub struct StoreClosing;

#[derive(Component)]
pub struct StoreTabCell {
    tab: usize,
}

#[derive(Component)]
pub struct StoreSlotCell {
    /// Index within the visible page grid.
    cell: usize,
}

#[derive(Component)]
pub struct StoreDetailText;

/// The shop's "Gold: N" readout — refreshed every frame from the live
/// inventory gold (a sale bumps gold via 0x304E, which does not touch
/// `StoreState`, so the window's own rebuild would otherwise never fire).
#[derive(Component)]
pub struct StoreGoldText;

#[derive(Component)]
enum PageButton {
    Prev,
    Next,
}

/// One of the five repurchase slots (`GDR_STORE_ICON_SLOT_01..05`), indexed
/// left to right. Reserves the fill site: no packet is known to deliver the
/// sold-item list, so these stay empty.
#[derive(Component)]
#[allow(dead_code)]
struct BuybackSlot(usize);

/// The blacksmith repair buttons (GDR_STORE_BTN_REPAIR / _REPAIRALL).
#[derive(Component, Clone, Copy)]
pub enum RepairButton {
    /// Arms single-repair mode: the next inventory-slot click repairs it.
    One,
    /// Repairs every damaged equipment item at once.
    All,
}

/// The quantity/confirm modal (rebuilds only when the prompt changes — the
/// amount lives in the [`ModalAmount`] resource + the text input, so typing
/// never fights the rebuild).
#[derive(Resource, Default)]
pub struct QuantityModal {
    pub prompt: Option<QuantityPrompt>,
}

/// The currently entered amount, parsed+clamped from the modal's text input
/// every frame (empty input counts as 1).
#[derive(Resource)]
pub struct ModalAmount(pub u16);

impl Default for ModalAmount {
    fn default() -> Self {
        Self(1)
    }
}

#[derive(Clone, Debug)]
pub struct QuantityPrompt {
    pub sell: bool,
    pub ref_id: i32,
    pub name: String,
    pub unit_price: u64,
    /// What `unit_price` is denominated in. Selling back to an NPC is always
    /// gold (itemdata col 31); buying follows the good's price policy.
    pub currency: ShopCurrency,
    pub max: u16,
    /// Buy: the store tab/slot; sell: the inventory slot in `slot`.
    pub tab: u8,
    pub slot: u8,
    pub npc_id: u32,
    pub opt_level: u8,
}

/// The active buy carry (a good picked up from the grid) and its ghost icon.
#[derive(Resource, Default)]
pub struct StoreCarry(pub Option<StoreCarryData>);

pub struct StoreCarryData {
    pub prompt: QuantityPrompt,
    pub ghost: Entity,
    /// True until the pickup press's own just_pressed frame has passed, so
    /// the click-carry logic doesn't instantly cancel itself.
    pub just_picked: bool,
}

/// The buy-carry's cursor-following icon.
#[derive(Component)]
pub struct StoreGhost;

#[derive(Component)]
pub struct ModalRoot;

/// The digits-only amount input inside the modal.
#[derive(Component)]
pub struct ModalAmountInput;

/// The "xN   M Gold" line, repainted as the amount changes.
#[derive(Component)]
pub struct ModalTotalText;

#[derive(Component)]
enum ModalButton {
    Minus,
    Plus,
    Ok,
    Cancel,
}

/// The spinner's page list: for each tab group, one entry per 30-slot chunk
/// (chunk count = max over the group's tabs). Flipping pages walks groups AND
/// chunks with one control, matching vanilla (e.g. armor shop page 1 = male
/// tabs, page 2 = female tabs).
///
/// No tab in the v1.188 media actually overflows a page — the slot histogram
/// over all 1,166 `refshopgoods` rows is min 0, max 29 — so the chunking
/// always yields exactly one chunk here. It is kept because the wire format
/// allows higher slots, and it degenerates correctly.
pub fn page_entries(layout: &crate::assets::textdata::shops::ShopLayout) -> Vec<(usize, usize)> {
    let mut entries = Vec::new();
    for (group_index, group) in layout.pages.iter().enumerate() {
        let max_slot = group
            .tabs
            .iter()
            .flat_map(|tab| tab.goods.iter().map(|good| good.slot as usize))
            .max()
            .unwrap_or(0);
        for chunk in 0..=(max_slot / SLOTS_PER_PAGE) {
            entries.push((group_index, chunk));
        }
    }
    entries
}

/// The (tab group, slot chunk) the session's spinner page points at.
fn current_entry(session: &crate::plugins::hud::store::model::StoreSession) -> (usize, usize) {
    page_entries(&session.layout)
        .get(session.page)
        .copied()
        .unwrap_or((0, 0))
}

/// The good shown in a grid cell on the current page/tab, if any.
fn good_at<'a>(
    session: &'a crate::plugins::hud::store::model::StoreSession,
    cell: usize,
) -> Option<&'a crate::assets::textdata::shops::ShopGood> {
    let (group, chunk) = current_entry(session);
    let tab = session.layout.pages.get(group)?.tabs.get(session.tab)?;
    let wanted = (chunk * SLOTS_PER_PAGE + cell) as u8;
    tab.goods.iter().find(|good| good.slot == wanted)
}

/// An amount with its currency — or bare, while the currency flag is still
/// UNKNOWN, since printing the wrong unit is worse than printing none.
fn priced(amount: u64, currency: ShopCurrency) -> String {
    match currency.label() {
        Some(unit) => format!("{amount} {unit}"),
        None => amount.to_string(),
    }
}

/// Tab name keys resolve in either string table depending on the shop: the
/// NPC stores' `SN_TAB_*` keys live in textuisystem (NOT textdataname — the
/// SN_ prefix lies), the mall tabs use `UIIT_*` keys.
fn tab_label(name_key: &str, names: &ClientTextNames, ui_strings: &ClientUiStrings) -> String {
    names
        .name(name_key)
        .or_else(|| ui_strings.get(name_key))
        .unwrap_or(name_key)
        .to_string()
}

/// Rebuild the store window whenever the session changes.
#[allow(clippy::too_many_arguments)]
pub fn sync_store_window(
    state: Res<StoreState>,
    existing: Query<(Entity, &Node), With<StoreWindowRoot>>,
    item_data: Res<ClientItemData>,
    item_index: Res<ClientItemIndex>,
    magic_options: Res<crate::plugins::textdata::ClientMagicOptions>,
    names: Res<ClientTextNames>,
    ui_strings: Res<ClientUiStrings>,
    inventories: Query<&Inventory, With<Player>>,
    fonts: Res<FontAssets>,
    asset_server: Res<AssetServer>,
    cam_query: Query<Entity, With<Camera2d>>,
    mut commands: Commands,
) {
    if !state.is_changed() {
        return;
    }
    // tab/page switches rebuild the window — keep a dragged position
    let mut anchor = (WINDOW_RIGHT, WINDOW_TOP);
    for (entity, node) in existing.iter() {
        if let (Val::Px(right), Val::Px(top)) = (node.right, node.top) {
            anchor = (right, top);
        }
        commands.entity(entity).insert(StoreClosing);
    }
    let Some(session) = &state.session else {
        return;
    };
    let Ok(camera) = cam_query.single() else {
        warn!("store: no 2d camera to attach to");
        return;
    };
    let s = hud_scale();

    let window = game_window::spawn_game_window(
        &mut commands,
        &asset_server,
        &fonts,
        camera,
        &session.title,
        (CONTENT_W, CONTENT_H),
        anchor,
        s,
    );
    commands
        .entity(window.root)
        // Hovered so sell-drops can test "is the pointer over the store"
        .insert((
            StoreWindowRoot,
            GlobalZIndex(58),
            Hovered::default(),
            PersistedWindow(WndPosSlot::Store),
        ));
    commands
        .entity(window.expect_close_button())
        .observe(on_close_button);

    let text_font = |size: f32| TextFont {
        font: fonts.two.clone().into(),
        font_size: FontSize::Px(size * s),
        ..default()
    };
    let gold = inventories.single().map(|inv| inv.gold).unwrap_or(0);

    commands.entity(window.content).with_children(|content| {
        // brighter backdrop across the WHOLE content area (first child, so
        // everything draws above) — vanilla tiles the full pane, not just
        // the tab strip + grid: the spinner row, detail board surround and
        // margins sit on it too (playtest screenshot 2026-08-07)
        content.spawn((
            abs_node((0.0, 0.0, CONTENT_W, CONTENT_H), s),
            ImageNode {
                image: asset_server.load("media://interface/ifcommon/bg_tile/com_bg_tile_b.ddj"),
                image_mode: NodeImageMode::Tiled {
                    tile_x: true,
                    tile_y: true,
                    stretch_value: s,
                },
                ..default()
            },
            Pickable::IGNORE,
        ));

        // tab strip
        let (group, _) = current_entry(session);
        let group_tabs = session
            .layout
            .pages
            .get(group)
            .map(|page| page.tabs.as_slice())
            .unwrap_or(&[]);
        for (tab_index, tab) in group_tabs.iter().take(4).enumerate() {
            let on = tab_index == session.tab;
            content
                .spawn((
                    StoreTabCell { tab: tab_index },
                    Button,
                    Hovered::default(),
                    abs_node(
                        // pitch = art width, no gap: the tab art carries its
                        // own bevel
                        (9.0 + tab_index as f32 * TAB_W, TAB_Y, TAB_W, TAB_H),
                        s,
                    ),
                    ImageNode {
                        image: asset_server.load(if on { TAB_ON_DDJ } else { TAB_OFF_DDJ }),
                        image_mode: NodeImageMode::Stretch,
                        ..default()
                    },
                ))
                .observe(on_tab_press)
                .with_children(|cell| {
                    cell.spawn((
                        Text::new(tab_label(&tab.name_key, &names, &ui_strings)),
                        text_font(7.5),
                        TextColor(if on {
                            Color::srgb(0.95, 0.9, 0.75)
                        } else {
                            Color::srgb(0.6, 0.6, 0.6)
                        }),
                        TextLayout::justify(Justify::Center),
                        Node {
                            position_type: PositionType::Absolute,
                            top: Val::Px(5.0 * s),
                            width: Val::Percent(100.0),
                            ..default()
                        },
                        Pickable::IGNORE,
                    ));
                });
        }

        // goods grid (com_lattice anatomy, same as the inventory bag)
        content
            .spawn((
                Node {
                    display: Display::Grid,
                    grid_template_columns: RepeatedGridTrack::px(GRID_COLS as u16, CELL * s),
                    grid_template_rows: RepeatedGridTrack::px(GRID_ROWS as u16, CELL * s),
                    ..abs_node(LATTICE_RECT, s)
                },
                Pickable::IGNORE,
            ))
            .with_children(|grid| {
                for cell in 0..SLOTS_PER_PAGE {
                    let (row, col) = (cell / GRID_COLS, cell % GRID_COLS);
                    let quarter = match (row == GRID_ROWS - 1, col == GRID_COLS - 1) {
                        (false, false) => "left_up",
                        (false, true) => "right_up",
                        (true, false) => "left_down",
                        (true, true) => "right_down",
                    };
                    let good = good_at(session, cell);
                    let mut cell_cmd = grid.spawn((
                        StoreSlotCell { cell },
                        Hovered::default(),
                        Node {
                            width: Val::Px(CELL * s),
                            height: Val::Px(CELL * s),
                            ..default()
                        },
                        ImageNode {
                            image: asset_server.load(format!("{LATTICE_DIR}{quarter}.ddj")),
                            image_mode: NodeImageMode::Stretch,
                            ..default()
                        },
                    ));
                    cell_cmd.observe(on_good_press);
                    if let Some(good) = good {
                        let icon = item_index
                            .id(&good.item_codename)
                            .and_then(|id| item_data.get(&id))
                            .and_then(|row| row.icon_path());
                        if let Some(icon) = icon {
                            cell_cmd.with_children(|slot| {
                                slot.spawn((
                                    Node {
                                        position_type: PositionType::Absolute,
                                        left: Val::Px(ICON_INSET * s),
                                        top: Val::Px(ICON_INSET * s),
                                        width: Val::Px(ICON_SIZE * s),
                                        height: Val::Px(ICON_SIZE * s),
                                        ..default()
                                    },
                                    ImageNode {
                                        image: asset_server.load(icon),
                                        image_mode: NodeImageMode::Stretch,
                                        ..default()
                                    },
                                    Pickable::IGNORE,
                                ));
                                if good.opt_level > 0 {
                                    slot.spawn((
                                        Text::new(format!("+{}", good.opt_level)),
                                        text_font(7.0),
                                        TextColor(PRICE_COLOR),
                                        Node {
                                            position_type: PositionType::Absolute,
                                            right: Val::Px(2.0 * s),
                                            top: Val::Px(1.0 * s),
                                            ..default()
                                        },
                                        Pickable::IGNORE,
                                    ));
                                }
                            });
                        }
                    }
                }
            });

        // page spinner — vanilla CIFSpinButtonCtrl anatomy: ◄ 16x16, the
        // CURRENT page number only (18x12, centered), ► 16x16, with the
        // com_*_arrow normal/_focus/_press art triples
        let arrow_style = |name: &str| ImageButtonStyle {
            normal: asset_server.load(format!("media://interface/ifcommon/com_{name}_arrow.ddj")),
            hover: asset_server.load(format!(
                "media://interface/ifcommon/com_{name}_arrow_focus.ddj"
            )),
            press: asset_server.load(format!(
                "media://interface/ifcommon/com_{name}_arrow_press.ddj"
            )),
            ..Default::default()
        };
        for (button, name, x) in [
            (PageButton::Prev, "left", SPIN_RECT.0),
            (PageButton::Next, "right", SPIN_RECT.0 + 34.0),
        ] {
            let style = arrow_style(name);
            content
                .spawn((
                    button,
                    Button,
                    Hovered::default(),
                    abs_node((x, SPIN_RECT.1, 16.0, 16.0), s),
                    ImageNode {
                        image: style.normal.clone(),
                        image_mode: NodeImageMode::Stretch,
                        ..default()
                    },
                    style,
                ))
                .observe(on_page_press);
        }
        content.spawn((
            Text::new(format!("{}", session.page + 1)),
            text_font(8.0),
            TextColor(Color::srgb(0.85, 0.85, 0.85)),
            TextLayout::justify(Justify::Center),
            abs_node((SPIN_RECT.0 + 16.0, SPIN_RECT.1 + 2.0, 18.0, 12.0), s),
            Pickable::IGNORE,
        ));

        // detail board: hovered good name + price, and the player's gold
        content.spawn((
            abs_node(DETAIL_RECT, s),
            ImageNode {
                image: asset_server.load(DETAIL_TILE),
                image_mode: NodeImageMode::Tiled {
                    tile_x: true,
                    tile_y: true,
                    stretch_value: s,
                },
                ..default()
            },
            Pickable::IGNORE,
        ));
        content.spawn((
            StoreDetailText,
            Text::new(""),
            text_font(7.5),
            TextColor(NAME_COLOR),
            TextLayout::justify(Justify::Center),
            abs_node(
                (
                    DETAIL_RECT.0 + 4.0,
                    DETAIL_RECT.1 + 6.0,
                    DETAIL_RECT.2 - 8.0,
                    26.0,
                ),
                s,
            ),
            Pickable::IGNORE,
        ));
        content.spawn((
            StoreGoldText,
            Text::new(format!("Gold: {gold}")),
            text_font(7.5),
            TextColor(PRICE_COLOR),
            TextLayout::justify(Justify::Center),
            abs_node(
                (
                    DETAIL_RECT.0 + 4.0,
                    DETAIL_RECT.1 + 36.0,
                    DETAIL_RECT.2 - 8.0,
                    14.0,
                ),
                s,
            ),
            Pickable::IGNORE,
        ));

        // repair buttons — only for shops that actually sell equipment
        // (blacksmith/armorer; potion and stable shops have none in vanilla)
        let sells_equipment = session
            .layout
            .pages
            .iter()
            .flat_map(|page| &page.tabs)
            .flat_map(|tab| &tab.goods)
            .any(|good| {
                item_index
                    .id(&good.item_codename)
                    .and_then(|id| item_data.get(&id))
                    .is_some_and(|row| row.is_equipment())
            });
        if sells_equipment {
            // enabled state is a snapshot at open/rebuild — a stale enable
            // is harmless (the server arbitrates)
            let nothing_damaged = inventories
                .single()
                .map(|inv| inv.damaged_slots(&item_data, &magic_options).is_empty())
                .unwrap_or(true);
            let repair_style = ImageButtonStyle {
                normal: asset_server.load("media://interface/ifcommon/com_button.ddj"),
                hover: asset_server.load("media://interface/ifcommon/com_button_focus.ddj"),
                press: asset_server.load("media://interface/ifcommon/com_button_press.ddj"),
                ..Default::default()
            };
            for (button, rect, key, fallback) in [
                (RepairButton::One, REPAIR_RECT, "UIIT_CTL_REPAIR", "Repair"),
                (
                    RepairButton::All,
                    REPAIR_ALL_RECT,
                    "UIIT_CTL_REPAIR_ALL",
                    "Repair all",
                ),
            ] {
                let mut cmd = content.spawn((
                    button,
                    Button,
                    Hovered::default(),
                    abs_node(rect, s),
                    ImageNode {
                        image: repair_style.normal.clone(),
                        image_mode: NodeImageMode::Stretch,
                        ..default()
                    },
                    repair_style.clone(),
                ));
                cmd.observe(on_repair_button);
                if nothing_damaged {
                    cmd.insert(bevy::ui::InteractionDisabled);
                }
                let label_color = if nothing_damaged {
                    Color::srgb(0.5, 0.5, 0.5)
                } else {
                    Color::srgb_u8(254, 251, 216)
                };
                cmd.with_children(|b| {
                    b.spawn((
                        Text::new(ui_strings.get_or(key, fallback).to_string()),
                        text_font(8.0),
                        TextColor(label_color),
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

        // The repurchase ("buy back") strip: the redeem frame, its label and
        // five reserved slots. Deliberately empty — which packet delivers the
        // sold-item list is UNKNOWN (no builder, no parser, no dump sample), so
        // the frame and label are data and an item in a slot would be an
        // invention. `BuybackSlot` reserves the fill site for when it lands.
        content.spawn((
            abs_node(BUYBACK_REDEEM_RECT, s),
            ImageNode {
                image: asset_server.load(REDEEM_WINDOW_DDJ),
                image_mode: NodeImageMode::Stretch,
                ..default()
            },
            Pickable::IGNORE,
        ));
        content.spawn((
            Text::new(
                ui_strings
                    .get_or("UIIT_STT_RE_BUY_OBJECT", "Buy back")
                    .to_string(),
            ),
            text_font(7.5),
            // GDR_STORE_CHARGY_STATIC FontColor="255,255,255,255" (ARGB).
            TextColor(Color::srgb_u8(255, 255, 255)),
            abs_node(BUYBACK_LABEL_RECT, s),
            Pickable::IGNORE,
        ));
        for (index, x) in BUYBACK_SLOT_XS.iter().enumerate() {
            content.spawn((
                BuybackSlot(index),
                abs_node(
                    (*x, BUYBACK_SLOT_Y, BUYBACK_SLOT_SIZE, BUYBACK_SLOT_SIZE),
                    s,
                ),
                Pickable::IGNORE,
            ));
        }
    });
}

/// "Repair" arms the single-repair mode (next inventory-slot click brings up
/// the confirm dialog for that item); "Repair all" opens the vanilla
/// confirmation msgbox directly — repairing costs gold, nothing is sent
/// until Confirm.
fn on_repair_button(
    activate: On<Activate>,
    buttons: Query<&RepairButton>,
    state: Res<crate::plugins::hud::store::model::StoreState>,
    inventories: Query<&Inventory, With<Player>>,
    item_data: Res<ClientItemData>,
    magic_options: Res<crate::plugins::textdata::ClientMagicOptions>,
    ui_strings: Res<ClientUiStrings>,
    mut repair_mode: ResMut<crate::plugins::hud::store::model::RepairMode>,
    mut confirm: ResMut<crate::plugins::hud::store::model::RepairConfirm>,
) {
    use crate::plugins::hud::store::model::{repair_cost_estimate, RepairOp, RepairPrompt};
    let Ok(button) = buttons.get(activate.entity) else {
        return;
    };
    if state.session.is_none() {
        return;
    }
    match button {
        RepairButton::One => {
            repair_mode.0 = !repair_mode.0;
            info!(
                "store: single-repair mode {}",
                if repair_mode.0 {
                    "armed — click the inventory item to repair"
                } else {
                    "disarmed"
                }
            );
        }
        RepairButton::All => {
            let cost = inventories
                .single()
                .map(|inv| repair_cost_estimate(inv, &item_data, &magic_options, RepairOp::All))
                .unwrap_or(0);
            confirm.prompt = Some(RepairPrompt {
                op: RepairOp::All,
                message: ui_strings.get_plain_or(
                    "UIIT_MSG_MSGBOX_REPAIR_ITEM",
                    "Repair all equipment in the inventory and weaponry slots.",
                ),
                cost,
            });
        }
    }
}

/// Marker + buttons of the repair confirmation msgbox.
#[derive(Component)]
pub struct RepairConfirmRoot;

#[derive(Component, Clone, Copy)]
enum RepairConfirmButton {
    Ok,
    Cancel,
}

/// Rebuild the repair confirmation msgbox on prompt changes — the quantity
/// modal's scrim + panel chrome, with the vanilla message and the prorated
/// cost estimate (server authoritative).
pub fn sync_repair_confirm(
    confirm: Res<crate::plugins::hud::store::model::RepairConfirm>,
    existing: Query<Entity, With<RepairConfirmRoot>>,
    ui_strings: Res<ClientUiStrings>,
    fonts: Res<FontAssets>,
    asset_server: Res<AssetServer>,
    cam_query: Query<Entity, With<Camera2d>>,
    mut commands: Commands,
) {
    if !confirm.is_changed() {
        return;
    }
    for entity in existing.iter() {
        commands.entity(entity).insert(StoreClosing);
    }
    let Some(prompt) = &confirm.prompt else {
        return;
    };
    let Ok(camera) = cam_query.single() else {
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
        ..Default::default()
    };
    let cost_line = format!(
        "{}: {} Gold",
        ui_strings.get_or("PARAM_REPAIRING_CHARGES", "Repairing cost"),
        prompt.cost
    );
    // Captured out of the button loop so the root can point Enter at Confirm
    // (`hud::focus::HudDialog`).
    let mut confirm_button = None;
    let root = commands
        .spawn((
            RepairConfirmRoot,
            Name::from("Repair Confirm"),
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.6)),
            GlobalZIndex(66),
            UiTargetCamera(camera),
        ))
        .with_children(|scrim| {
            scrim
                .spawn((
                    Node {
                        width: Val::Px(240.0 * s),
                        height: Val::Px(110.0 * s),
                        ..default()
                    },
                    BackgroundColor(Color::srgb(0.09, 0.08, 0.06)),
                    Outline {
                        width: Val::Px(1.0),
                        color: Color::srgb(0.55, 0.45, 0.25),
                        ..default()
                    },
                ))
                .with_children(|panel| {
                    panel.spawn((
                        Text::new(prompt.message.clone()),
                        text_font(8.0),
                        TextColor(NAME_COLOR),
                        TextLayout::justify(Justify::Center),
                        abs_node((10.0, 10.0, 220.0, 34.0), s),
                        Pickable::IGNORE,
                    ));
                    panel.spawn((
                        Text::new(cost_line),
                        text_font(8.0),
                        TextColor(PRICE_COLOR),
                        TextLayout::justify(Justify::Center),
                        abs_node((10.0, 48.0, 220.0, 14.0), s),
                        Pickable::IGNORE,
                    ));
                    for (button, key, fallback, x) in [
                        (RepairConfirmButton::Ok, "UIIT_CTL_CONFIRM", "Confirm", 35.0),
                        (
                            RepairConfirmButton::Cancel,
                            "UIIT_CTL_CANCEL",
                            "Cancel",
                            125.0,
                        ),
                    ] {
                        let is_ok = matches!(button, RepairConfirmButton::Ok);
                        let mut spawned = panel.spawn((
                            button,
                            Button,
                            Hovered::default(),
                            abs_node((x, 66.0, 80.0, 24.0), s),
                            ImageNode {
                                image: button_style.normal.clone(),
                                image_mode: NodeImageMode::Stretch,
                                ..default()
                            },
                            button_style.clone(),
                        ));
                        spawned.observe(on_repair_confirm_button);
                        if is_ok {
                            confirm_button = Some(spawned.id());
                        }
                        spawned.with_children(|b| {
                            b.spawn((
                                Text::new(ui_strings.get_or(key, fallback).to_string()),
                                text_font(8.0),
                                TextColor(Color::WHITE),
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
        })
        .id();
    if let Some(confirm_button) = confirm_button {
        commands
            .entity(root)
            .insert(crate::plugins::hud::focus::HudDialog { confirm_button });
    }
}

/// Confirm sends the EXPERIMENTAL 0x703E request; Cancel just closes.
fn on_repair_confirm_button(
    activate: On<Activate>,
    buttons: Query<&RepairConfirmButton>,
    state: Res<crate::plugins::hud::store::model::StoreState>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    mut confirm: ResMut<crate::plugins::hud::store::model::RepairConfirm>,
    mut pending: ResMut<crate::plugins::hud::store::model::PendingRepair>,
) {
    use crate::plugins::hud::store::model::RepairOp;
    let Ok(button) = buttons.get(activate.entity) else {
        return;
    };
    let Some(prompt) = confirm.prompt.take() else {
        return;
    };
    if matches!(button, RepairConfirmButton::Cancel) {
        return;
    }
    let Some(session) = &state.session else {
        return;
    };
    let request = match prompt.op {
        RepairOp::One(slot) => packets::agent::inventory::ItemRepairRequest::One {
            slot,
            npc_unique_id: session.npc_id,
        },
        RepairOp::All => packets::agent::inventory::ItemRepairRequest::All {
            npc_unique_id: session.npc_id,
        },
    };
    let Ok(conn) = conn.single() else {
        warn!("store: no agent connection, dropping repair request");
        return;
    };
    pending.0 = Some(prompt.op);
    info!("store: sending {:?} (experimental 0x703E v2)", request);
    if let Err(e) = conn.get_sender().send(Packet::from(request).into()) {
        error!("network: failed to send ItemRepairRequest: {}", e.0);
    }
}

/// The shop's X ends the whole conversation: the store session lives inside
/// the talk session, so closing it sends the 0x704B close and takes the NPC
/// dialog down with it (rather than falling back to the dialog, which would
/// leave the client showing a conversation the server has ended).
fn on_close_button(
    _: On<Activate>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    mut state: ResMut<StoreState>,
    mut dialog: ResMut<crate::plugins::hud::npc_dialog::model::NpcDialogState>,
) {
    if let Some(session) = state.session.take() {
        crate::plugins::hud::npc_dialog::model::send_close_to(&conn, session.npc_id);
    }
    *dialog = crate::plugins::hud::npc_dialog::model::NpcDialogState::Closed;
}

fn on_tab_press(activate: On<Activate>, tabs: Query<&StoreTabCell>, mut state: ResMut<StoreState>) {
    let Ok(cell) = tabs.get(activate.entity) else {
        return;
    };
    if let Some(session) = &mut state.session {
        if session.tab != cell.tab {
            session.tab = cell.tab;
        }
    }
}

fn on_page_press(
    activate: On<Activate>,
    buttons: Query<&PageButton>,
    mut state: ResMut<StoreState>,
) {
    let Ok(button) = buttons.get(activate.entity) else {
        return;
    };
    let Some(session) = &mut state.session else {
        return;
    };
    let entries = page_entries(&session.layout);
    let old_group = current_entry(session).0;
    match button {
        PageButton::Prev if session.page > 0 => session.page -= 1,
        PageButton::Next if session.page + 1 < entries.len() => session.page += 1,
        _ => return,
    }
    // switching to another tab group resets the tab selection
    if entries.get(session.page).map(|entry| entry.0) != Some(old_group) {
        session.tab = 0;
    }
}

/// Show the hovered good in the detail board.
pub fn update_store_detail(
    state: Res<StoreState>,
    cells: Query<(&StoreSlotCell, &Hovered)>,
    item_data: Res<ClientItemData>,
    item_index: Res<ClientItemIndex>,
    names: Res<ClientTextNames>,
    mut detail: Query<&mut Text, With<StoreDetailText>>,
) {
    let Some(session) = &state.session else {
        return;
    };
    let hovered = cells
        .iter()
        .find(|(_, hovered)| hovered.get())
        .and_then(|(cell, _)| good_at(session, cell.cell));
    let text = hovered
        .map(|good| {
            let name = item_index
                .id(&good.item_codename)
                .and_then(|id| item_data.get(&id))
                .and_then(|row| row.name_key())
                .and_then(|key| names.name(key))
                .unwrap_or(&good.item_codename);
            format!("{name}\n{}", priced(good.price, good.currency))
        })
        .unwrap_or_default();
    for mut node in detail.iter_mut() {
        if node.0 != text {
            text.clone_into(&mut node.0);
        }
    }
}

/// Publish the hovered good's itemdata ref for the shared item tooltip
/// (`inventory::tooltip`), the way the storage window publishes its hovered
/// item — polled from the cells' `Hovered` components, since the goods grid
/// carries no Over/Out observers.
///
/// The original shows the help bubble over these slots too: the store's 30
/// goods cells are `CIFSlotWithHelpForPackage` (`docs/re/ui/hud-store-window.md`
/// §4), and the help bubble IS the item tooltip
/// (`docs/re/ui/help-tooltip-widget.md`). The detail board stays — vanilla
/// draws both.
///
/// Suppressed mid-carry, matching the inventory and storage tooltips.
pub fn track_store_hover(
    state: Res<StoreState>,
    cells: Query<(&StoreSlotCell, &Hovered)>,
    carry: Res<StoreCarry>,
    inv_state: Res<InventoryState>,
    item_index: Res<ClientItemIndex>,
    mut hovered: ResMut<StoreHoveredGood>,
) {
    let carrying = carry.0.is_some() || inv_state.drag.is_some();
    let ref_id = state
        .session
        .as_ref()
        .filter(|_| !carrying)
        .and_then(|session| {
            cells
                .iter()
                .find(|(_, hovered)| hovered.get())
                .and_then(|(cell, _)| good_at(session, cell.cell))
        })
        .and_then(|good| item_index.id(&good.item_codename))
        .map(|id| id as u32);
    if hovered.0 != ref_id {
        hovered.0 = ref_id;
    }
}

/// Keep the shop's "Gold: N" readout current. A sale raises gold via 0x304E,
/// which does not change `StoreState`, so `sync_store_window`'s change-gated
/// rebuild never fires — without this the displayed gold only updates on a
/// tab/page switch. Separate system (not folded into `update_store_detail`) to
/// avoid two `&mut Text` queries conflicting in one system.
pub fn refresh_store_gold(
    inventories: Query<&Inventory, With<Player>>,
    mut gold_q: Query<&mut Text, With<StoreGoldText>>,
) {
    let Ok(inv) = inventories.single() else {
        return;
    };
    let text = format!("Gold: {}", inv.gold);
    for mut node in gold_q.iter_mut() {
        if node.0 != text {
            text.clone_into(&mut node.0);
        }
    }
}

/// Press on a good: start the vanilla buy carry — a ghost icon sticks to the
/// cursor; dropping it on the (open) inventory grid opens the quantity
/// modal, releasing anywhere else cancels.
fn on_good_press(
    press: On<Pointer<Press>>,
    cells: Query<&StoreSlotCell>,
    state: Res<StoreState>,
    item_data: Res<ClientItemData>,
    item_index: Res<ClientItemIndex>,
    names: Res<ClientTextNames>,
    cam_query: Query<Entity, With<Camera2d>>,
    mut carry: ResMut<StoreCarry>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
) {
    if press.event.button != PointerButton::Primary {
        return;
    }
    if carry.0.is_some() {
        return;
    }
    let Ok(cell) = cells.get(press.entity) else {
        return;
    };
    let Some(session) = &state.session else {
        return;
    };
    let Some(good) = good_at(session, cell.cell) else {
        return;
    };
    let Some(ref_id) = item_index.id(&good.item_codename) else {
        return;
    };
    let row = item_data.get(&ref_id);
    let name = row
        .and_then(|row| row.name_key())
        .and_then(|key| names.name(key))
        .unwrap_or(&good.item_codename)
        .to_string();
    let max = row
        .and_then(|row| row.max_stack())
        .unwrap_or(1)
        .clamp(1, u16::MAX as u32) as u16;
    let (group, _) = current_entry(session);
    let prompt = QuantityPrompt {
        sell: false,
        ref_id,
        name,
        unit_price: good.price,
        currency: good.currency,
        max,
        // the wire tab index counts across the store's flattened groups
        tab: session.layout.wire_tab_index(group, session.tab),
        slot: good.slot,
        npc_id: session.npc_id,
        opt_level: good.opt_level,
    };

    let mut ghost = commands.spawn((
        StoreGhost,
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(-1000.0),
            top: Val::Px(-1000.0),
            width: Val::Px(ICON_SIZE * hud_scale()),
            height: Val::Px(ICON_SIZE * hud_scale()),
            ..default()
        },
        GlobalZIndex(80),
        Pickable::IGNORE,
    ));
    if let Some(icon) = row.and_then(|row| row.icon_path()) {
        ghost.insert(ImageNode {
            image: asset_server.load(icon),
            image_mode: NodeImageMode::Stretch,
            ..default()
        });
    }
    if let Ok(camera) = cam_query.single() {
        ghost.insert(UiTargetCamera(camera));
    }
    carry.0 = Some(StoreCarryData {
        prompt,
        ghost: ghost.id(),
        just_picked: true,
    });
}

/// Cursor-follow for the buy-carry ghost (same as the inventory's).
pub fn update_store_ghost(
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut ghosts: Query<&mut Node, With<StoreGhost>>,
) {
    if ghosts.is_empty() {
        return;
    }
    let Some(cursor) = windows.single().ok().and_then(|w| w.cursor_position()) else {
        return;
    };
    let size = ICON_SIZE * hud_scale();
    for mut node in ghosts.iter_mut() {
        node.left = Val::Px(cursor.x - size / 2.0);
        node.top = Val::Px(cursor.y - size / 2.0);
    }
}

/// Finish the buy carry with the inventory's dual semantics: a hold-drag
/// released over the open inventory's grid drops (quantity modal), releasing
/// while still over the store keeps carrying (vanilla click-carry), and a
/// later click either drops (over the grid) or cancels.
pub fn finish_store_carry(
    buttons: Res<ButtonInput<MouseButton>>,
    inv_state: Res<InventoryState>,
    store_cells: Query<&Hovered, With<StoreSlotCell>>,
    mut carry: ResMut<StoreCarry>,
    mut modal: ResMut<QuantityModal>,
    mut commands: Commands,
) {
    if carry.0.is_none() {
        return;
    }
    let over_inventory = inv_state.hovered_slot.is_some();
    let over_store_cell = store_cells.iter().any(|hovered| hovered.get());
    let mut finish = |carry: &mut StoreCarry, drop: bool, modal: &mut QuantityModal| {
        let Some(data) = carry.0.take() else {
            return;
        };
        commands.entity(data.ghost).despawn();
        if drop {
            modal.prompt = Some(data.prompt);
        }
    };

    if buttons.just_pressed(MouseButton::Left) {
        let data = carry.0.as_mut().expect("checked above");
        if data.just_picked {
            // the pickup press itself
            data.just_picked = false;
        } else {
            finish(&mut carry, over_inventory, &mut modal);
            return;
        }
    }
    if buttons.just_released(MouseButton::Left) {
        if over_inventory {
            finish(&mut carry, true, &mut modal);
        } else if !over_store_cell {
            // released off both the store cells and the bag: cancel; over a
            // store cell the carry survives as a click-carry
            finish(&mut carry, false, &mut modal);
        }
    }
}

/// Dropping a carried bag item onto the store window (hold-drag release OR
/// the click-carry's second click) opens the sell quantity modal. The
/// inventory's own carry state (`InventoryState.drag`) and ghost are taken
/// over here — no move request is sent for this drop.
#[allow(clippy::too_many_arguments)]
pub fn sell_drop_on_store(
    buttons: Res<ButtonInput<MouseButton>>,
    store: Res<StoreState>,
    store_roots: Query<&Hovered, With<StoreWindowRoot>>,
    inventories: Query<&Inventory, With<Player>>,
    item_data: Res<ClientItemData>,
    names: Res<ClientTextNames>,
    ghosts: Query<Entity, With<crate::plugins::hud::inventory::ui::DragGhost>>,
    mut inv_state: ResMut<InventoryState>,
    mut modal: ResMut<QuantityModal>,
    mut commands: Commands,
) {
    if !buttons.just_released(MouseButton::Left) && !buttons.just_pressed(MouseButton::Left) {
        return;
    }
    let Some(session) = &store.session else {
        return;
    };
    let Some(slot) = inv_state.drag else {
        return;
    };
    if !store_roots.iter().any(|hovered| hovered.get()) {
        return;
    }
    inv_state.drag = None;
    for ghost in ghosts.iter() {
        commands.entity(ghost).despawn();
    }
    let Some(item) = inventories
        .single()
        .ok()
        .and_then(|inv| inv.get(slot).cloned())
    else {
        return;
    };
    let row = item_data.get(&(item.ref_id as i32));
    let name = row
        .and_then(|row| row.name_key())
        .and_then(|key| names.name(key))
        .unwrap_or("?")
        .to_string();
    let max = match &item.data {
        ItemTypeData::Expendable { stack_count, .. } => (*stack_count).max(1),
        _ => 1,
    };
    // per-unit NPC sell value: itemdata col 31 (col 17 CanSell gates it) —
    // unsellable items (quest/mall) don't open the modal at all
    let Some(unit_price) = row.and_then(|row| row.sell_price()) else {
        info!("store: {name} cannot be sold to NPCs");
        return;
    };
    modal.prompt = Some(QuantityPrompt {
        sell: true,
        ref_id: item.ref_id as i32,
        name,
        unit_price,
        currency: ShopCurrency::Gold,
        max,
        tab: 0,
        slot,
        npc_id: session.npc_id,
        opt_level: 0,
    });
}

/// Rebuild the quantity modal on prompt/count changes.
#[allow(clippy::too_many_arguments)]
pub fn sync_quantity_modal(
    modal: Res<QuantityModal>,
    existing: Query<Entity, With<ModalRoot>>,
    ui_strings: Res<ClientUiStrings>,
    fonts: Res<FontAssets>,
    item_data: Res<ClientItemData>,
    asset_server: Res<AssetServer>,
    cam_query: Query<Entity, With<Camera2d>>,
    mut focus: ResMut<InputFocus>,
    mut amount: ResMut<ModalAmount>,
    mut commands: Commands,
) {
    if !modal.is_changed() {
        return;
    }
    for entity in existing.iter() {
        commands.entity(entity).insert(StoreClosing);
    }
    let Some(prompt) = &modal.prompt else {
        if !existing.is_empty() {
            focus.clear();
        }
        return;
    };
    let Ok(camera) = cam_query.single() else {
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
        ..Default::default()
    };
    let title = if prompt.sell {
        format!("Sell {}", prompt.name)
    } else {
        format!("Buy {}", prompt.name)
    };
    // selling defaults to the whole stack (vanilla feel); buying to 1
    let initial = if prompt.sell { prompt.max.max(1) } else { 1 };
    amount.0 = initial;
    let icon = item_data
        .get(&prompt.ref_id)
        .and_then(|row| row.icon_path());
    let price_line = format!(
        "x{initial}   {}",
        priced(prompt.unit_price * initial as u64, prompt.currency)
    );

    let mut input_entity = None;
    // Captured out of the button loop so the root can point Enter at Confirm
    // (`hud::focus::HudDialog`). The amount field takes focus below, and its
    // `EditableText` disallows newlines, so Enter there submits the modal
    // rather than being swallowed by the input.
    let mut confirm_button = None;
    let root = commands
        .spawn((
            ModalRoot,
            Name::from("Store Quantity Modal"),
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            // scrim deliberately swallows clicks
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.6)),
            GlobalZIndex(65),
            UiTargetCamera(camera),
        ))
        .with_children(|scrim| {
            scrim
                .spawn((
                    Node {
                        width: Val::Px(220.0 * s),
                        height: Val::Px(110.0 * s),
                        ..default()
                    },
                    BackgroundColor(Color::srgb(0.09, 0.08, 0.06)),
                    Outline {
                        width: Val::Px(1.0),
                        color: Color::srgb(0.55, 0.45, 0.25),
                        ..default()
                    },
                ))
                .with_children(|panel| {
                    panel.spawn((
                        Text::new(title),
                        text_font(8.5),
                        TextColor(NAME_COLOR),
                        TextLayout::justify(Justify::Center),
                        abs_node((0.0, 10.0, 220.0, 14.0), s),
                        Pickable::IGNORE,
                    ));
                    // the traded item's icon, left of the amount row
                    if let Some(icon) = &icon {
                        panel.spawn((
                            abs_node((14.0, 28.0, 24.0, 24.0), s),
                            ImageNode {
                                image: asset_server.load(icon.clone()),
                                image_mode: NodeImageMode::Stretch,
                                ..default()
                            },
                            Pickable::IGNORE,
                        ));
                    }
                    // - [amount input] +
                    for (button, label, x) in [
                        (ModalButton::Minus, "-", 50.0),
                        (ModalButton::Plus, "+", 150.0),
                    ] {
                        panel
                            .spawn((
                                button,
                                Button,
                                Hovered::default(),
                                Text::new(label),
                                text_font(11.0),
                                TextColor(Color::srgb(0.9, 0.9, 0.9)),
                                abs_node((x, 32.0, 20.0, 18.0), s),
                            ))
                            .observe(on_modal_button);
                    }
                    // digits-only text input, clamped to the max stack by
                    // `sync_modal_amount` (empty = 1); opens pre-filled with
                    // the initial amount (a sell defaults to the full stack)
                    let mut input_box = abs_node((76.0, 31.0, 68.0, 18.0), s);
                    input_box.padding = UiRect::top(Val::Px(2.0 * s));
                    let mut editable = EditableText {
                        visible_lines: Some(1.0),
                        allow_newlines: false,
                        max_characters: Some(4),
                        ..default()
                    };
                    if initial > 1 {
                        editable.queue_edit(TextEdit::Insert(initial.to_string().into()));
                    }
                    input_entity = Some(
                        panel
                            .spawn((
                                ModalAmountInput,
                                editable,
                                EditableTextFilter::new(|c: char| c.is_ascii_digit()),
                                input_box,
                                text_font(9.0),
                                TextColor(Color::WHITE),
                                TextLayout::justify(Justify::Center),
                                TextCursorStyle {
                                    color: Color::WHITE,
                                    ..default()
                                },
                                BackgroundColor(Color::srgb(0.16, 0.14, 0.1)),
                            ))
                            .id(),
                    );
                    panel.spawn((
                        ModalTotalText,
                        Text::new(price_line),
                        text_font(8.5),
                        TextColor(PRICE_COLOR),
                        TextLayout::justify(Justify::Center),
                        abs_node((30.0, 52.0, 160.0, 12.0), s),
                        Pickable::IGNORE,
                    ));
                    for (button, key, fallback, x) in [
                        (ModalButton::Ok, "UIIT_CTL_CONFIRM", "Confirm", 25.0),
                        (ModalButton::Cancel, "UIIT_CTL_CANCEL", "Cancel", 115.0),
                    ] {
                        let is_ok = matches!(button, ModalButton::Ok);
                        let mut spawned = panel.spawn((
                            button,
                            Button,
                            Hovered::default(),
                            abs_node((x, 66.0, 80.0, 24.0), s),
                            ImageNode {
                                image: button_style.normal.clone(),
                                image_mode: NodeImageMode::Stretch,
                                ..default()
                            },
                            button_style.clone(),
                        ));
                        spawned.observe(on_modal_button);
                        if is_ok {
                            confirm_button = Some(spawned.id());
                        }
                        spawned.with_children(|b| {
                            b.spawn((
                                Text::new(ui_strings.get_or(key, fallback).to_string()),
                                text_font(8.0),
                                TextColor(Color::WHITE),
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
        })
        .id();
    if let Some(confirm_button) = confirm_button {
        commands
            .entity(root)
            .insert(crate::plugins::hud::focus::HudDialog { confirm_button });
    }
    // type-ready immediately
    if let Some(input) = input_entity {
        focus.set(input, FocusCause::Navigated);
    }
}

fn on_modal_button(
    activate: On<Activate>,
    buttons: Query<&ModalButton>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    amount: Res<ModalAmount>,
    mut inputs: Query<&mut EditableText, With<ModalAmountInput>>,
    mut modal: ResMut<QuantityModal>,
    mut pending: ResMut<PendingStoreOp>,
) {
    let Ok(button) = buttons.get(activate.entity) else {
        return;
    };
    let Some(prompt) = modal.prompt.clone() else {
        return;
    };
    // +/- steer the text input; `sync_modal_amount` parses it back.
    // SelectAll + Insert (not `clear()` + Insert): `clear()` empties the buffer
    // but leaves parley's selection at its old byte index, and an Insert queued
    // in the same frame then panics on the stale index inside apply_text_edits.
    let mut set_amount = |value: u16| {
        let Ok(mut editable) = inputs.single_mut() else {
            return;
        };
        editable.queue_edit(TextEdit::SelectAll);
        editable.queue_edit(TextEdit::Insert(value.to_string().into()));
    };
    match button {
        ModalButton::Minus => {
            set_amount(amount.0.saturating_sub(1).max(1));
        }
        ModalButton::Plus => {
            set_amount((amount.0 + 1).min(prompt.max));
        }
        ModalButton::Cancel => {
            modal.prompt = None;
        }
        ModalButton::Ok => {
            let quantity = amount.0;
            let request = if prompt.sell {
                pending.0 = Some(StoreOp::Sell {
                    slot: prompt.slot,
                    quantity,
                });
                InventoryOperationRequest::Sell {
                    slot: prompt.slot,
                    quantity,
                    npc_unique_id: prompt.npc_id,
                }
            } else {
                pending.0 = Some(StoreOp::Buy {
                    ref_id: prompt.ref_id,
                    opt_level: prompt.opt_level,
                    quantity,
                });
                InventoryOperationRequest::Buy {
                    tab: prompt.tab,
                    slot: prompt.slot,
                    quantity,
                    npc_unique_id: prompt.npc_id,
                }
            };
            modal.prompt = None;
            let Ok(conn) = conn.single() else {
                warn!("store: no agent connection, dropping store request");
                return;
            };
            info!("store: sending {:?}", request);
            if let Err(e) = conn.get_sender().send(Packet::from(request).into()) {
                error!("network: failed to send store request: {}", e.0);
            }
        }
    }
}

/// Parse + clamp the typed amount into [`ModalAmount`] and repaint the total
/// line (an empty input counts as 1; values clamp to the max stack).
pub fn sync_modal_amount(
    modal: Res<QuantityModal>,
    inputs: Query<&EditableText, With<ModalAmountInput>>,
    mut amount: ResMut<ModalAmount>,
    mut totals: Query<&mut Text, With<ModalTotalText>>,
) {
    let Some(prompt) = &modal.prompt else {
        return;
    };
    let Ok(editable) = inputs.single() else {
        return;
    };
    let parsed = editable
        .value()
        .to_string()
        .trim()
        .parse::<u16>()
        .unwrap_or(1)
        .clamp(1, prompt.max.max(1));
    if amount.0 != parsed {
        amount.0 = parsed;
    }
    let line = format!(
        "x{parsed}   {}",
        priced(prompt.unit_price * parsed as u64, prompt.currency)
    );
    for mut text in totals.iter_mut() {
        if text.0 != line {
            line.clone_into(&mut text.0);
        }
    }
}

/// The quantity modal and buy-carry are bound to the store session (parity
/// with the storage window's `clear_modal_with_storage`): when the session
/// ends with them open (walk-away, deselect, another NPC's dialog), the
/// prompt reset lets `sync_quantity_modal` despawn the click-swallowing
/// scrim and clear the input focus, and the carry ghost is despawned.
pub fn clear_modal_with_store(
    state: Res<StoreState>,
    mut modal: ResMut<QuantityModal>,
    mut carry: ResMut<StoreCarry>,
    mut commands: Commands,
) {
    if state.session.is_none() {
        if modal.prompt.is_some() {
            modal.prompt = None;
        }
        if let Some(data) = carry.0.take() {
            commands.entity(data.ghost).despawn();
        }
    }
}

/// PostUpdate: despawn store windows/modals marked [`StoreClosing`].
pub fn despawn_closing_store(closing: Query<Entity, With<StoreClosing>>, mut commands: Commands) {
    for entity in closing.iter() {
        commands.entity(entity).despawn();
    }
}

/// OnExit cleanup.
pub fn cleanup_store(
    mut commands: Commands,
    windows: Query<Entity, Or<(With<StoreWindowRoot>, With<ModalRoot>, With<StoreGhost>)>>,
    mut state: ResMut<StoreState>,
    mut modal: ResMut<QuantityModal>,
    mut carry: ResMut<StoreCarry>,
) {
    for entity in windows.iter() {
        commands.entity(entity).despawn();
    }
    state.session = None;
    modal.prompt = None;
    carry.0 = None;
}

#[cfg(test)]
mod test {
    use super::*;

    /// Four `GDR_STORE` blocks exist at 370 / 317 / 360 / 317; the 370 one is
    /// gated on `RENEWAL_SHOP_SYSTEM` + `RESTORE_SOLDITEM_INSHOP`, both defined
    /// in the shipped `config/define.txt`. Getting the branch wrong is a 53px
    /// error, so pin the height the shell actually produces.
    #[test]
    fn outer_window_matches_the_live_registry_branch() {
        assert_eq!(
            game_window::outer_size((CONTENT_W, CONTENT_H)),
            VANILLA_OUTER
        );
    }

    /// Every interior rect must be its `ifstore.txt` rect minus the shell's
    /// content origin — computed from `game_window`'s own exports rather than
    /// restated, so a change over there fails here instead of silently
    /// desyncing the window (#314).
    #[test]
    fn interior_rects_sit_on_the_shells_content_origin() {
        // (vanilla x, vanilla y, ours) — GDR_STORE_LAT :1479,
        // _SPIN_PAGE :1424, _DETAIL_BGTILE :1540, _BTN_REPAIR :1462,
        // _BTN_REPAIRALL :1443, _STRETCH_REDEEM :1403, _CHARGY_STATIC :1279.
        let cases = [
            (21.0, 71.0, LATTICE_RECT),
            (102.0, 254.0, SPIN_RECT),
            (40.0, 279.0, DETAIL_RECT),
            (86.0, 332.0, REPAIR_RECT),
            (166.0, 332.0, REPAIR_ALL_RECT),
            (57.0, 285.0, BUYBACK_REDEEM_RECT),
            (7.0, 300.0, BUYBACK_LABEL_RECT),
        ];
        for (vanilla_x, vanilla_y, ours) in cases {
            assert_eq!(
                ours.0,
                vanilla_x - (game_window::FRAME_VIS_SIDE + game_window::CHROME_PAD),
                "x of vanilla {vanilla_x}"
            );
            assert_eq!(
                ours.1,
                vanilla_y - game_window::CONTENT_TOP,
                "y of vanilla {vanilla_y}"
            );
        }
    }

    /// The repurchase slots run on a 35px pitch, not the main grid's 36. At 36
    /// the fifth slot's right edge would land past the 188px width of
    /// `com_redeem_window.ddj`, i.e. art overrunning its own frame.
    #[test]
    fn buyback_slots_use_a_35px_pitch_inside_the_redeem_frame() {
        for pair in BUYBACK_SLOT_XS.windows(2) {
            assert_eq!(pair[1] - pair[0], 35.0);
        }
        let first = BUYBACK_SLOT_XS[0];
        let last_right = BUYBACK_SLOT_XS[4] + BUYBACK_SLOT_SIZE;
        assert!(
            first >= BUYBACK_REDEEM_RECT.0,
            "first slot inside the frame"
        );
        assert!(
            last_right <= BUYBACK_REDEEM_RECT.0 + BUYBACK_REDEEM_RECT.2,
            "fifth slot inside the frame"
        );
        // GDR_STORE_ICON_SLOT_* are all y 289.
        assert_eq!(BUYBACK_SLOT_Y, 289.0 - game_window::CONTENT_TOP);
    }
}
