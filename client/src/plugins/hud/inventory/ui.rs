//! Inventory window layout and refresh.
//!
//! Idea: like the other HUDs, every rect below is hand-transcribed from the
//! vanilla resinfo definitions (`ifinventory.txt` for the left item-grid
//! panel, `ifequipment.txt` for the right equipment panel) and uniformly
//! scaled, all placed inside the shared framed window shell built by
//! `hud::game_window` (mframe chrome, tiled interior, title band with close
//! button, drag-to-move). The item grid reproduces the vanilla `CIFLattice`: the four
//! 36x36 `com_lattice_*` textures are edge variants — `left_up` is the
//! interior tile carrying the separator + junction star on its right/bottom
//! edges, the `right_*`/`*_down` variants terminate the last column/row. The
//! window is spawned once on entering a playable scene (hidden) and toggled
//! via `InventoryState.open`; `refresh_inventory` repaints icons, stack
//! counts, tabs, gold and equipment from the player's `Inventory` component.
//! Slot cells carry hover observers (tooltip) and Press/Release observers
//! implementing the vanilla carry interaction: press an item to pick it up
//! (a ghost icon sticks to the cursor), then either release over the target
//! (classic drag) or click it up and click the target (vanilla click-carry) —
//! both send the same move request, applied only when the server confirms
//! (model.rs). Dropping equipment anywhere on the equipment panel equips it
//! into its type-derived slot, like the original client. Built on raw
//! Press/Release instead of bevy_picking's DragDrop because DragDrop only
//! reaches entities hovered mid-drag and never the press target, which made
//! short drags silently do nothing.

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use bevy::ui::UiTargetCamera;
use bevy::ui_widgets::{Activate, Button};

use packets::agent::character_data::{InventoryItem, ItemTypeData, COS_STATE_SUMMONED};
use packets::agent::prelude::InventoryOperationRequest;
use packets::Packet;

use crate::assets::textdata::itemdata::ItemDataRow;
use crate::assets::FontAssets;
use crate::net::connection::SilkroadConnection;
use crate::plugins::hud::flipbook::Flipbook;
use crate::plugins::hud::game_window;
use crate::plugins::hud::game_window::abs_node;
use crate::plugins::hud::inventory::model::InventoryState;
use crate::plugins::hud::inventory::paperdoll::{PaperDollTarget, PaperDollView, PaperDollYaw};
use crate::plugins::hud::inventory::tooltip;
use crate::plugins::hud::scale::hud_scale;
use crate::plugins::hud::window_positions::PersistedWindow;
use crate::plugins::net::agent::AgentConnection;
use crate::plugins::net::character_info::CharacterInfo;
use crate::plugins::net::inventory::{Inventory, BAG_FIRST_SLOT, SLOTS_PER_PAGE};
use crate::plugins::player::Player;
use crate::plugins::settings::window_positions::WndPosSlot;
use crate::plugins::textdata::{
    ClientCharacterData, ClientItemData, ClientMagicOptions, ClientUiStrings,
};
use crate::plugins::ui_v2::style::ImageButtonStyle;

// --- Layout constants (resinfo window space, scaled by hud_scale()) -----

/// Multiplicative icon tint for bag items the player cannot currently equip
/// (level/sex/armor-mixing) — a desaturated red wash over the icon.
const UNEQUIPPABLE_TINT: Color = Color::srgb(1.0, 0.45, 0.45);

/// Multiplicative icon tint for a COS scroll whose pet is dead.
///
/// ⚠️ **Ours, not vanilla.** Media ships no state-specific icon art for a dead
/// scroll and no RE doc records one, so the blue wash is a choice — reported as
/// matching the original's look, but not measured from it. Independent of the
/// tooltip's `DEAD_PET_COLOR`, which is red: that one is a warning line, this
/// is a state wash over the icon.
const DEAD_PET_TINT: Color = Color::srgb(0.45, 0.6, 0.95);

// --- Animated icon effects --------------------------------------------------
//
// The original animates the *item icon itself* for special items, with a
// sprite sheet per category under `icon/item/etc/`. Each one tiles exactly into
// 32x32 tiles — the icon's own size — with no remainder, which is what
// identifies them as frame sheets rather than plain art:
//
//   icon_edge_rare.ddj      256x128  8x4  = 32 frames
//   icon_edge_nasrun.ddj    512x32   16x1 = 16 frames
//   icon_edge_legendry.ddj  640x64   20x2 = 40 frames
//
// (Measured from the DDS headers. A census of that folder finds nine
// multi-frame 32px sheets in all; the other six are event items.)
//
// This is what the pulsing gold rectangle we used to draw was standing in for.
//
// Two things the archive does NOT give, because no resinfo control references
// these sheets at all — the whole `Style=512` corpus is six controls on four
// files, all vitals gauges. So the *cadence* and the *frame order* are
// code-side in the original and are ours to choose:
//   * frame_ms is a config knob (`hud.icon_effect_frame_ms`), not a constant.
//   * row-major is assumed, matching `hud::cooldown`'s own sheet walk.

const ICON_EFFECT_DIR: &str = "media://icon/item/etc/";

/// Milliseconds per frame when no config says otherwise.
///
/// **Not from the data** — see above; nothing references these sheets, so the
/// original's cadence is unrecorded. 50 ms (20 fps) runs the 32-frame rare
/// sheet in about 1.6 s, which reads as a slow sweep rather than a flicker.
/// `hud.icon_effect_frame_ms` overrides it.
pub const DEFAULT_ICON_EFFECT_FRAME_MS: f32 = 50.0;

/// A sheet's grid, before the configured frame rate is applied.
struct IconEffectSheet {
    file: &'static str,
    frame_count: usize,
    cols: usize,
    rows: usize,
    sheet: (f32, f32),
}

impl IconEffectSheet {
    fn flipbook(&self, frame_ms: f32) -> Flipbook {
        Flipbook {
            frame_count: self.frame_count,
            cols: self.cols,
            rows: self.rows,
            frame_ms,
            sheet: self.sheet,
        }
    }
}

/// Seal-grade items (`_RARE` / `_B_RARE` / `_C_RARE`).
const ICON_EFFECT_RARE: IconEffectSheet = IconEffectSheet {
    file: "icon_edge_rare.ddj",
    frame_count: 32,
    cols: 8,
    rows: 4,
    sheet: (256.0, 128.0),
};
/// Nasrun items — selected by their `MATTR_NASRUN_*` magic options, the only
/// thing in the data that names that family.
const ICON_EFFECT_NASRUN: IconEffectSheet = IconEffectSheet {
    file: "icon_edge_nasrun.ddj",
    frame_count: 16,
    cols: 16,
    rows: 1,
    sheet: (512.0, 32.0),
};

/// Tint for the summoned-COS use of the icon effect.
///
/// ⚠️ **Ours.** The sheet census is exhaustive and holds nothing COS-shaped, so
/// while the *mechanism* here is the original's, marking a summoned scroll with
/// it is an addition (ADR-0009). The state behind it is not invented: it is the
/// wire's own `COS_STATE_SUMMONED`, kept current by 0x3040's `0x40` mask bit.
///
/// Green rather than the dead-pet blue because the two are opposite readings of
/// the same field and must not be confusable at a glance.
const PET_SUMMONED_TINT: Color = Color::srgb(0.36, 0.9, 0.45);

/// Screen anchoring of the whole window (right edge, like the vanilla client).
const WINDOW_RIGHT: f32 = 24.0;
const WINDOW_TOP: f32 = 60.0;
/// Gap between the inventory and equipment panels.
const PANEL_GAP: f32 = 4.0;

// resinfo/ifinventory.txt
/// GDR_INVENTORY_FRAME Rect "0,0,176,308" + the 28px tall downbox below it.
const INV_W: f32 = 176.0;
const INV_FRAME_H: f32 = 308.0;
/// GDR_INVENTORY_STA_MONEY Rect "0,305,176,28" (int_window_downbox.ddj).
const DOWNBOX_RECT: (f32, f32, f32, f32) = (0.0, 305.0, 176.0, 28.0);
const INV_H: f32 = DOWNBOX_RECT.1 + DOWNBOX_RECT.3;
/// GDR_INVENTORY_LATTICE Rect "18,13,144,288" — the 4x8 grid of 36px cells.
const LATTICE_RECT: (f32, f32, f32, f32) = (18.0, 13.0, 144.0, 288.0);
pub const GRID_COLS: u8 = 4;
const GRID_ROWS: u8 = 8;
const CELL: f32 = 36.0;
/// The slot hole inside a lattice tile spans [2, 30) — the tile's right/
/// bottom edges (from x/y 30) carry the separator art. Icons fill the hole.
const ICON_INSET: f32 = 2.0;
const ICON_SIZE: f32 = 28.0;
/// GDR_INVENTORY_BTN_MONEY Rect "12,310,20,20" (com_moneybutton.ddj).
const MONEY_BTN_RECT: (f32, f32, f32, f32) = (12.0, 310.0, 20.0, 20.0);
/// Gold amount: right-aligned ending a little before the vanilla "Gold"
/// label, which sits at GDR_INVENTORY_STA_GOLD Rect "143,314,24,12".
const GOLD_TEXT_RECT: (f32, f32, f32, f32) = (30.0, 313.0, 104.0, 14.0);
const GOLD_LABEL_RECT: (f32, f32, f32, f32) = (143.0, 313.0, 26.0, 14.0);
/// int_window_* frame pieces are 16px (corners/sides).
const INV_FRAME_BORDER: f32 = 16.0;

// resinfo/ifequipment.txt
const EQ_W: f32 = 178.0;
const EQ_H: f32 = 355.0;
/// equip_window_* frame pieces are 12px.
const EQ_FRAME_BORDER: f32 = 12.0;
/// GDR_EQ_NORMALTILE_BG Rect "12,12,154,331" (com_bg_tile_d.ddj, tiled).
const EQ_BG_RECT: (f32, f32, f32, f32) = (12.0, 12.0, 154.0, 331.0);
const EQ_SLOT_SIZE: f32 = 32.0;
/// Paper-doll view: the free area between the two slot columns.
const PAPER_DOLL_RECT: (f32, f32, f32, f32) = (38.0, 16.0, 102.0, 300.0);
/// GDR_EQ_BTN_ROTATE_LEFT / _RESET / _RIGHT rects.
const ROTATE_LEFT_RECT: (f32, f32, f32, f32) = (54.0, 327.0, 28.0, 16.0);
const ROTATE_RESET_RECT: (f32, f32, f32, f32) = (81.0, 327.0, 16.0, 16.0);
const ROTATE_RIGHT_RECT: (f32, f32, f32, f32) = (96.0, 327.0, 28.0, 16.0);
/// GDR_AVATAR_SLOT_01..05 rects (display-only avatar inventory view); the
/// index is the avatar wire slot. resinfo assigns no art (vanilla does it in
/// code), so the placeholder stems are picked from the otherwise-unused
/// equip_slot_* textures: hat/dress/attach/flag/special dress. If an avatar
/// item lands in the wrong hole, fix only this table.
const AVATAR_SLOTS: [(u8, f32, f32, &str); 5] = [
    (0, 5.0, 86.0, "helm"),
    (1, 5.0, 129.0, "cloth"),
    (2, 141.0, 129.0, "pandernt"),
    (3, 141.0, 86.0, "plag"),
    (4, 5.0, 172.0, "specialdress"),
];
/// GDR_EQ_BTN_CHANGE_AVATAR/EQUIP_SLOT: one toggle at Rect "1,305,40,40"
/// whose art depends on the active view.
const AVATAR_BTN_RECT: (f32, f32, f32, f32) = (1.0, 305.0, 40.0, 40.0);

/// Wire equipment slot -> (x, y, empty-slot art stem), from the
/// GDR_EQUIPMENT_SLOT_1xx rects. The resinfo CommandIDs are 0..7, 8..11 and a
/// lone 13; they map to wire slots 0..7 (armor/weapon/shield), 9..12
/// (accessories) and 8 (the job/flag slot at the bottom). If an equipped item
/// ever renders in the wrong hole, fix ONLY this table.
const EQUIP_SLOTS: [(u8, f32, f32, &str); 13] = [
    (0, 5.0, 86.0, "helm"),
    (1, 5.0, 129.0, "mail"),
    (2, 141.0, 86.0, "shoulderguard"),
    (3, 141.0, 129.0, "gauntlet"),
    (4, 5.0, 172.0, "pants"),
    (5, 141.0, 172.0, "boots"),
    (6, 12.0, 12.0, "weapon"),
    (7, 134.0, 12.0, "shield"),
    (8, 141.0, 308.0, "specialdress"),
    (9, 5.0, 215.0, "earring"),
    (10, 141.0, 215.0, "necklace"),
    (11, 5.0, 258.0, "l_ring"),
    (12, 141.0, 258.0, "r_ring"),
];

/// Bag page tabs: com_tab art is 60x24, drawn slightly narrower like vanilla.
const TAB_W: f32 = 52.0;
const TAB_H: f32 = 20.0;
const MAX_TABS: u8 = 4;

/// The window content: the left column (tabs over the inventory panel) next
/// to the equipment panel; both panel tops align at the content origin.
const CONTENT_W: f32 = INV_W + PANEL_GAP + EQ_W;
const CONTENT_H: f32 = EQ_H;

const TAB_ON_DDJ: &str = "media://interface/ifcommon/com_tab_on.ddj";
const TAB_OFF_DDJ: &str = "media://interface/ifcommon/com_tab_off.ddj";
const MONEY_BTN_DDJ: &str = "media://interface/ifcommon/com_moneybutton.ddj";
const DOWNBOX_DDJ: &str = "media://interface/inventory/int_window_downbox.ddj";
const BG_TILE_DDJ: &str = "media://interface/ifcommon/bg_tile/com_bg_tile_d.ddj";
const INV_FRAME_DIR: &str = "media://interface/inventory/int_window_";
const EQ_FRAME_DIR: &str = "media://interface/equipment/equip_window_";
const LATTICE_DIR: &str = "media://interface/ifcommon/lattice_window/com_lattice_";
const EQ_SLOT_DIR: &str = "media://interface/equipment/equip_slot_";
const ROTATE_DIR: &str = "media://interface/equipment/equip_rotate_";

// --- Markers ----------------------------------------------------------------

#[derive(Component)]
pub struct InventoryRoot;

/// One bag page tab ("Page N"); `page` is 0-based.
#[derive(Component)]
pub struct InventoryTabCell {
    pub page: u8,
}

#[derive(Component)]
pub struct InventoryTabLabel {
    pub page: u8,
}

/// One 36px bag grid cell; `index` is 0..31 row-major within the page.
#[derive(Component)]
pub struct InventoryGridCell {
    pub index: u8,
}

/// One equipment slot cell; `slot` is the wire equipment slot (0..12).
#[derive(Component)]
pub struct EquipSlotCell {
    pub slot: u8,
}

/// One display-only avatar slot cell; `slot` indexes the avatar inventory.
#[derive(Component)]
pub struct AvatarSlotCell {
    pub slot: u8,
}

/// The equip-view/avatar-view toggle button on the equipment panel.
#[derive(Component)]
pub struct AvatarToggleButton;

/// The item-icon image inside a slot cell (hidden when the slot is empty).
#[derive(Component)]
pub struct SlotIcon;

/// The animated overlay drawn on top of a slot's icon: the original's
/// `icon_edge_*` frame sheets.
///
/// Hidden for ordinary items. Carries its own [`Flipbook`] because the three
/// sheets have different grids and frame counts, so the animation cannot be a
/// constant of the system that drives it.
///
/// This replaced a static pulsing rectangle behind the icon, which was the
/// stand-in for exactly this — `hud-inventory.md` §6 had already flagged that
/// rectangle as an un-flagged non-original enhancement to remove.
#[derive(Component)]
pub struct SlotIconEffect {
    pub flipbook: Flipbook,
    /// Which sheet is currently loaded into the node's `ImageNode`, or `""`
    /// before any has been.
    ///
    /// **This, not the flipbook, is the reload key.** Keying on the flipbook
    /// meant a slot spawned with the rare grid and then asked to draw the rare
    /// grid compared equal, so the load never ran and the node kept
    /// `Handle::default()` — which Bevy draws as an opaque white 1x1 texture
    /// stretched over the whole icon. Every Seal item rendered as a white
    /// square. The empty string gives "nothing loaded yet" a value the code can
    /// actually see, which is the state the old version could not express.
    pub sheet: &'static str,
}

/// The stack-count label inside a bag cell.
#[derive(Component)]
pub struct SlotCount;

#[derive(Component)]
pub struct GoldText;

/// The icon following the cursor while an item is carried.
#[derive(Component)]
pub struct DragGhost;

/// The original client registers exactly **one** drag ghost for the whole UI:
/// `GDR_SELECTED_ITEM:CIFDraggedStatic`, id 99, `Rect "0,0,32,32"`, no `DDJ`
/// and no subtree (`ginterface.txt:278`) — a registry-level cursor decoration
/// that takes its texture at runtime from whatever is being carried (#579).
/// We had three hand-rolled copies of it (inventory, skill window, underbar),
/// each free to drift in z-order, hit-testing or centering. This is the one
/// builder they now share.
///
/// Deviation: the ghost is spawned at the caller's scaled size rather than the
/// original's fixed 32 px, because our windows are scaled (one shared
/// [`hud_scale`] since #620) and a 32 px ghost would not match the slot
/// it came from. [`DRAG_GHOST_SIZE`] is that unscaled origin.
pub const DRAG_GHOST_SIZE: f32 = 32.0;

/// Top-left of a ghost of `size` centred on `cursor`. Split out because the
/// centring is the one piece of ghost geometry that was copy-pasted three
/// times and is testable without an `App`.
pub fn drag_ghost_top_left(cursor: Vec2, size: f32) -> Vec2 {
    Vec2::new(cursor.x - size / 2.0, cursor.y - size / 2.0)
}

/// Spawn the cursor-following ghost: absolutely positioned, above every HUD
/// window, and never a picking target (it sits under the cursor, so any
/// hit-testing on it would swallow the drop it exists to visualise).
pub fn drag_ghost_bundle(
    name: &'static str,
    icon: Handle<Image>,
    cursor: Vec2,
    size: f32,
    camera: Entity,
) -> impl Bundle {
    let top_left = drag_ghost_top_left(cursor, size);
    (
        DragGhost,
        Name::from(name),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(top_left.x),
            top: Val::Px(top_left.y),
            width: Val::Px(size),
            height: Val::Px(size),
            ..default()
        },
        ImageNode {
            image: icon,
            image_mode: NodeImageMode::Stretch,
            ..default()
        },
        GlobalZIndex(80),
        UiTargetCamera(camera),
        Pickable::IGNORE,
    )
}

/// The equipment panel's background: a panel-wide drop catcher so equipment
/// can be dropped anywhere on the panel and lands in its type-derived slot.
#[derive(Component)]
pub struct EquipPanelCatch;

/// Invisible surface under the whole bag panel: dropping a carried equip-slot
/// item anywhere on it (frame border, gaps around the grid) unequips into the
/// first empty bag slot, like the original client.
#[derive(Component)]
pub struct BagPanelCatch;

// --- Spawning ---------------------------------------------------------------

/// Spawn the (initially hidden) inventory window and its tooltip root.
pub fn spawn_inventory_window(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    fonts: Res<FontAssets>,
    ui_strings: Res<ClientUiStrings>,
    mut images: ResMut<Assets<Image>>,
    cam_query: Query<Entity, With<Camera2d>>,
    mut state: ResMut<InventoryState>,
) {
    let Ok(camera) = cam_query.single() else {
        warn!("inventory: no 2d camera to attach to");
        return;
    };
    // fresh session state (mirrors spawn_mini_info's vitals reset)
    *state = InventoryState::default();

    let paperdoll = images.add(Image::new_target_texture(
        crate::plugins::hud::inventory::paperdoll::RT_W,
        crate::plugins::hud::inventory::paperdoll::RT_H,
        TextureFormat::Bgra8UnormSrgb,
        None,
    ));
    commands.insert_resource(PaperDollTarget(paperdoll.clone()));

    let s = hud_scale();
    let window = game_window::spawn_game_window(
        &mut commands,
        &asset_server,
        &fonts,
        camera,
        ui_strings.get_or("UIIT_STT_INVENTORY", "Inventory"),
        (CONTENT_W, CONTENT_H),
        (WINDOW_RIGHT, WINDOW_TOP),
        s,
    );
    commands
        .entity(window.root)
        // Hovered so storage withdraws can test "is the pointer over the
        // inventory window" for the drop-on-panel path
        .insert((
            InventoryRoot,
            GlobalZIndex(60),
            Hovered::default(),
            // wndpos slot 0: the original persists the whole tabbed MainPopup
            // as one window at one position. We still draw the pages in their
            // own frames, but they now share this slot and are mutually
            // exclusive (`hud::main_popup`), so dragging any page moves "the
            // popup" and the group reopens where the player last put it (#302).
            PersistedWindow(WndPosSlot::MainPopup),
        ))
        .entry::<Node>()
        .and_modify(|mut node| node.display = Display::None);
    commands.entity(window.expect_close_button()).observe(
        |_: On<Activate>, mut state: ResMut<InventoryState>| {
            state.open = false;
        },
    );
    // the left column stacks the page tabs over the inventory panel; the
    // equipment panel's top aligns with the top of the tabs
    commands.entity(window.content).with_children(|content| {
        content
            .spawn((
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::FlexStart,
                    column_gap: Val::Px(PANEL_GAP * s),
                    ..default()
                },
                Pickable::IGNORE,
            ))
            .with_children(|row| {
                row.spawn((
                    Node {
                        flex_direction: FlexDirection::Column,
                        ..default()
                    },
                    Pickable::IGNORE,
                ))
                .with_children(|left| {
                    spawn_tab_strip(left, &asset_server, &fonts);
                    spawn_inventory_panel(left, &asset_server, &fonts);
                });
                spawn_equipment_panel(row, &asset_server, &fonts, paperdoll);
            });
    });

    tooltip::spawn_tooltip(&mut commands, &asset_server, camera);
}

pub fn cleanup_inventory_window(
    roots: Query<
        Entity,
        Or<(
            With<InventoryRoot>,
            With<tooltip::InventoryTooltipRoot>,
            With<DragGhost>,
            With<crate::plugins::hud::inventory::paperdoll::PaperDollClone>,
        )>,
    >,
    mut commands: Commands,
) {
    for entity in roots.iter() {
        commands.entity(entity).despawn();
    }
    commands.remove_resource::<PaperDollTarget>();
}

/// The bag page-tab strip (top-left, above the grid panel); unused tabs are
/// hidden by `refresh_inventory`.
fn spawn_tab_strip(
    content: &mut ChildSpawnerCommands,
    asset_server: &AssetServer,
    fonts: &FontAssets,
) {
    let s = hud_scale();
    content
        .spawn((
            Node {
                height: Val::Px(TAB_H * s),
                padding: UiRect::left(Val::Px(6.0 * s)),
                column_gap: Val::Px(2.0 * s),
                ..default()
            },
            Pickable::IGNORE,
        ))
        .with_children(|tabs| {
            for page in 0..MAX_TABS {
                tabs.spawn((
                    InventoryTabCell { page },
                    Button,
                    Node {
                        width: Val::Px(TAB_W * s),
                        height: Val::Px(TAB_H * s),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        display: if page == 0 {
                            Display::Flex
                        } else {
                            Display::None
                        },
                        ..default()
                    },
                    ImageNode {
                        image: asset_server.load(TAB_ON_DDJ),
                        image_mode: NodeImageMode::Stretch,
                        ..default()
                    },
                ))
                .observe(on_tab_activate)
                .with_children(|tab| {
                    tab.spawn((
                        InventoryTabLabel { page },
                        Text::new(format!("Page {}", page + 1)),
                        TextFont {
                            font: fonts.two.clone().into(),
                            font_size: FontSize::Px(7.0 * s),
                            ..default()
                        },
                        TextColor(Color::WHITE),
                        Pickable::IGNORE,
                    ));
                });
            }
        });
}

/// Left panel: the framed 4x8 item grid with the gold row.
fn spawn_inventory_panel(
    root: &mut ChildSpawnerCommands,
    asset_server: &AssetServer,
    fonts: &FontAssets,
) {
    let s = hud_scale();
    root.spawn((
        Node {
            width: Val::Px(INV_W * s),
            height: Val::Px(INV_H * s),
            ..default()
        },
        Pickable::IGNORE,
    ))
    .with_children(|body| {
        // drop catcher below everything: the frame art is Pickable::IGNORE,
        // so presses on the border/gaps land here (slot cells sit on top and
        // win the hit test)
        body.spawn((BagPanelCatch, abs_node((0.0, 0.0, INV_W, INV_H), s)))
            .observe(on_bag_panel_press)
            .observe(on_bag_panel_release);
        spawn_frame(
            body,
            asset_server,
            INV_FRAME_DIR,
            (0.0, 0.0, INV_W, INV_FRAME_H),
            FrameBorders {
                side: INV_FRAME_BORDER,
                bar: INV_FRAME_BORDER,
            },
            FrameCenter::Color(Color::BLACK),
        );

        // the 4x8 item grid
        body.spawn((
            Node {
                display: Display::Grid,
                grid_template_columns: RepeatedGridTrack::px(GRID_COLS as u16, CELL * s),
                grid_template_rows: RepeatedGridTrack::px(GRID_ROWS as u16, CELL * s),
                ..abs_node(LATTICE_RECT, s)
            },
            Pickable::IGNORE,
        ))
        .with_children(|grid| {
            for index in 0..SLOTS_PER_PAGE {
                let (row, col_i) = (index / GRID_COLS, index % GRID_COLS);
                // the left_up tile carries the inter-cell separator on its
                // right/bottom edges; the right_*/_down variants terminate
                // the lattice at the last column/row
                let quarter = match (row == GRID_ROWS - 1, col_i == GRID_COLS - 1) {
                    (false, false) => "left_up",
                    (false, true) => "right_up",
                    (true, false) => "left_down",
                    (true, true) => "right_down",
                };
                grid.spawn((
                    InventoryGridCell { index },
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
                ))
                .observe(tooltip::on_slot_over)
                .observe(tooltip::on_slot_out)
                .observe(on_slot_press)
                .observe(on_slot_release)
                .with_children(|cell| {
                    // glow behind, icon on top, count and badge above that
                    // icon, then its animated edge on top, then the count
                    spawn_slot_icon(cell, ICON_INSET, ICON_SIZE);
                    spawn_slot_icon_effect(cell, ICON_INSET, ICON_SIZE);
                    spawn_slot_count(cell, fonts, s);
                });
            }
        });

        // gold row art + amount
        body.spawn((
            abs_node(DOWNBOX_RECT, s),
            ImageNode {
                image: asset_server.load(DOWNBOX_DDJ),
                image_mode: NodeImageMode::Stretch,
                ..default()
            },
            Pickable::IGNORE,
        ));
        body.spawn((
            abs_node(MONEY_BTN_RECT, s),
            ImageNode {
                image: asset_server.load(MONEY_BTN_DDJ),
                image_mode: NodeImageMode::Stretch,
                ..default()
            },
            Pickable::IGNORE,
        ));
        let gold_node = abs_node(GOLD_TEXT_RECT, s);
        body.spawn((
            GoldText,
            Text::new("0"),
            TextFont {
                font: fonts.two.clone().into(),
                font_size: FontSize::Px(7.5 * s),
                ..default()
            },
            TextColor(Color::srgb_u8(255, 217, 128)),
            TextLayout::justify(Justify::Right),
            Node {
                justify_content: JustifyContent::FlexEnd,
                ..gold_node
            },
            Pickable::IGNORE,
        ));
        body.spawn((
            Text::new("Gold"),
            TextFont {
                font: fonts.two.clone().into(),
                font_size: FontSize::Px(7.5 * s),
                ..default()
            },
            TextColor(Color::WHITE),
            abs_node(GOLD_LABEL_RECT, s),
            Pickable::IGNORE,
        ));
    });
}

/// The stack-count label of a slot cell, hidden until [`refresh_inventory`]
/// finds something to put in it.
///
/// Shared by the bag grid and the equipment panel because ammunition equips:
/// arrows sit in wire slot 7 carrying a stack of up to 10000, so the equipment
/// holes need the same label the bag has always had.
fn spawn_slot_count(cell: &mut ChildSpawnerCommands, fonts: &FontAssets, s: f32) {
    cell.spawn((
        SlotCount,
        Text::new(""),
        TextFont {
            font: fonts.nine.clone().into(),
            font_size: FontSize::Px(6.0 * s),
            ..default()
        },
        TextColor(Color::WHITE),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(2.0 * s),
            top: Val::Px(1.0 * s),
            ..default()
        },
        Visibility::Hidden,
        Pickable::IGNORE,
    ));
}

/// Right panel: equipment frame, paper-doll view, the 13 slots and the
/// paper-doll rotate buttons.
fn spawn_equipment_panel(
    root: &mut ChildSpawnerCommands,
    asset_server: &AssetServer,
    fonts: &FontAssets,
    paperdoll: Handle<Image>,
) {
    let s = hud_scale();
    root.spawn((
        Node {
            width: Val::Px(EQ_W * s),
            height: Val::Px(EQ_H * s),
            ..default()
        },
        Pickable::IGNORE,
    ))
    .with_children(|panel| {
        spawn_frame(
            panel,
            asset_server,
            EQ_FRAME_DIR,
            (0.0, 0.0, EQ_W, EQ_H),
            FrameBorders {
                side: EQ_FRAME_BORDER,
                bar: EQ_FRAME_BORDER,
            },
            FrameCenter::None,
        );
        // tiled dark background behind everything in the frame; pickable so a
        // carried item dropped anywhere on the panel can auto-equip (the slot
        // cells sit on top and win the hit test)
        panel
            .spawn((
                EquipPanelCatch,
                abs_node(EQ_BG_RECT, s),
                ImageNode {
                    image: asset_server.load(BG_TILE_DDJ),
                    image_mode: NodeImageMode::Tiled {
                        tile_x: true,
                        tile_y: true,
                        stretch_value: 1.0,
                    },
                    ..default()
                },
            ))
            .observe(on_equip_panel_press)
            .observe(on_equip_panel_release);
        // paper-doll render target view
        panel.spawn((
            PaperDollView,
            abs_node(PAPER_DOLL_RECT, s),
            ImageNode {
                image: paperdoll,
                image_mode: NodeImageMode::Stretch,
                ..default()
            },
            Pickable::IGNORE,
        ));

        for (slot, x, y, stem) in EQUIP_SLOTS {
            // The weapon/shield placeholder textures are 56px where the rest
            // are 40px, with the pictogram drawn at the same absolute pixel
            // scale — squeezing both into the 32px cell would render those
            // two smaller. Draw the placeholder as a child scaled by the
            // texture ratio (overhanging the cell) so all pictograms match;
            // the cell itself stays the 32px hit area.
            let art_px = if slot == 6 || slot == 7 { 56.0 } else { 40.0 };
            let art_size = EQ_SLOT_SIZE * art_px / 40.0;
            let art_inset = (EQ_SLOT_SIZE - art_size) / 2.0;
            panel
                .spawn((
                    EquipSlotCell { slot },
                    abs_node((x, y, EQ_SLOT_SIZE, EQ_SLOT_SIZE), s),
                ))
                .observe(tooltip::on_slot_over)
                .observe(tooltip::on_slot_out)
                .observe(on_slot_press)
                .observe(on_slot_release)
                .with_children(|cell| {
                    cell.spawn((
                        abs_node((art_inset, art_inset, art_size, art_size), s),
                        ImageNode {
                            image: asset_server.load(format!("{EQ_SLOT_DIR}{stem}.ddj")),
                            image_mode: NodeImageMode::Stretch,
                            ..default()
                        },
                        Pickable::IGNORE,
                    ));
                    // placeholder art, then the icon, then its animated edge: a
                    // Seal item you are *wearing* is as rare as one in the bag,
                    // and the bag was the only grid that said so.
                    spawn_slot_icon(cell, (EQ_SLOT_SIZE - ICON_SIZE) / 2.0, ICON_SIZE);
                    spawn_slot_icon_effect(cell, (EQ_SLOT_SIZE - ICON_SIZE) / 2.0, ICON_SIZE);
                    // Equipment stacks: ammunition is the one equippable
                    // expendable, and slot 7 holds a quiver of up to 10000.
                    spawn_slot_count(cell, fonts, s);
                });
        }

        // avatar view: 5 display-only slots (hidden until toggled). No count
        // label: avatar items are cosmetics and never stack.
        for (slot, x, y, stem) in AVATAR_SLOTS {
            panel
                .spawn((
                    AvatarSlotCell { slot },
                    abs_node((x, y, EQ_SLOT_SIZE, EQ_SLOT_SIZE), s),
                    ImageNode {
                        image: asset_server.load(format!("{EQ_SLOT_DIR}{stem}.ddj")),
                        image_mode: NodeImageMode::Stretch,
                        ..default()
                    },
                    Visibility::Hidden,
                ))
                .with_children(|cell| {
                    spawn_slot_icon(cell, (EQ_SLOT_SIZE - ICON_SIZE) / 2.0, ICON_SIZE);
                    spawn_slot_icon_effect(cell, (EQ_SLOT_SIZE - ICON_SIZE) / 2.0, ICON_SIZE);
                });
        }

        // equip/avatar view toggle (vanilla shows the *other* view's art)
        let avatar_style = ImageButtonStyle {
            normal: asset_server.load(format!("{EQ_SLOT_DIR}avata_button.ddj")),
            hover: asset_server.load(format!("{EQ_SLOT_DIR}avata_button_focus.ddj")),
            press: asset_server.load(format!("{EQ_SLOT_DIR}avata_button_press.ddj")),
            ..Default::default()
        };
        panel
            .spawn((
                AvatarToggleButton,
                Button,
                Hovered::default(),
                abs_node(AVATAR_BTN_RECT, s),
                ImageNode {
                    image: avatar_style.normal.clone(),
                    image_mode: NodeImageMode::Stretch,
                    ..default()
                },
                avatar_style,
            ))
            .observe(|_: On<Activate>, mut state: ResMut<InventoryState>| {
                state.avatar_view = !state.avatar_view;
            });

        // paper-doll rotation buttons
        for (rect, stem, step) in [
            (ROTATE_LEFT_RECT, "left", -1.0f32),
            (ROTATE_RESET_RECT, "reset", 0.0),
            (ROTATE_RIGHT_RECT, "right", 1.0),
        ] {
            let style = ImageButtonStyle {
                normal: asset_server.load(format!("{ROTATE_DIR}{stem}_button.ddj")),
                hover: asset_server.load(format!("{ROTATE_DIR}{stem}_button_focus.ddj")),
                press: asset_server.load(format!("{ROTATE_DIR}{stem}_button_press.ddj")),
                ..Default::default()
            };
            panel
                .spawn((
                    Button,
                    Hovered::default(),
                    abs_node(rect, s),
                    ImageNode {
                        image: style.normal.clone(),
                        image_mode: NodeImageMode::Stretch,
                        ..default()
                    },
                    style,
                ))
                .observe(move |_: On<Activate>, mut yaw: ResMut<PaperDollYaw>| {
                    if step == 0.0 {
                        yaw.0 = 0.0;
                    } else {
                        yaw.0 += step * PaperDollYaw::STEP;
                    }
                });
        }
    });
}

/// Which animated overlay a slot should draw, if any.
///
/// Ordered by how much the player needs to see it: the Seal grade is the
/// headline fact about an item, so it wins over the Nasrun family and over the
/// summoned marker on the same scroll.
fn icon_effect_for(
    row: Option<&ItemDataRow>,
    item: Option<&InventoryItem>,
    magic_options: &ClientMagicOptions,
    frame_ms: f32,
) -> Option<(Flipbook, &'static str, Color)> {
    let row = row?;
    if row.seal_tier().is_some() {
        return Some((
            ICON_EFFECT_RARE.flipbook(frame_ms),
            ICON_EFFECT_RARE.file,
            Color::WHITE,
        ));
    }
    let nasrun = item.is_some_and(|item| {
        magic_params(&item.data).iter().any(|param| {
            magic_options
                .get(param.kind)
                .is_some_and(|info| info.codename.contains("NASRUN"))
        })
    });
    if nasrun {
        return Some((
            ICON_EFFECT_NASRUN.flipbook(frame_ms),
            ICON_EFFECT_NASRUN.file,
            Color::WHITE,
        ));
    }
    // The summoned-COS marker. Reuses the mechanism and the subtlest sheet;
    // the tint is what makes it read as a state rather than a rarity.
    if item
        .and_then(|item| item.data.cos_state())
        .is_some_and(|state| state == COS_STATE_SUMMONED)
    {
        return Some((
            ICON_EFFECT_NASRUN.flipbook(frame_ms),
            ICON_EFFECT_NASRUN.file,
            PET_SUMMONED_TINT,
        ));
    }
    None
}

/// An item instance's magic options, whichever class carries them.
fn magic_params(data: &ItemTypeData) -> &[packets::agent::character_data::MagicParam] {
    match data {
        ItemTypeData::Equipment(eq) => &eq.mag_params,
        ItemTypeData::Expendable { mag_params, .. } => mag_params,
        _ => &[],
    }
}

/// Everything [`refresh_inventory`] needs to drive the animated icon overlays.
///
/// Bundled because `refresh_inventory` is at Bevy's system-parameter ceiling —
/// the query plus its two lookups pushed it past the limit and the function
/// stopped satisfying `IntoSystem`. Same reason as [`ArmedModes`] below.
#[derive(bevy::ecs::system::SystemParam)]
pub struct IconEffects<'w, 's> {
    slots: Query<
        'w,
        's,
        (
            &'static mut Visibility,
            &'static mut SlotIconEffect,
            &'static mut ImageNode,
        ),
        (
            Without<SlotIcon>,
            Without<SlotCount>,
            Without<InventoryTabCell>,
        ),
    >,
    /// Reads the `MATTR_NASRUN_*` options that select the Nasrun sheet.
    magic_options: Res<'w, ClientMagicOptions>,
    /// Optional: the offline preview scenes build the window without a config.
    config: Option<Res<'w, crate::plugins::config::ClientConfig>>,
}

impl IconEffects<'_, '_> {
    /// The configured frame cadence — see [`DEFAULT_ICON_EFFECT_FRAME_MS`].
    fn frame_ms(&self) -> f32 {
        self.config
            .as_deref()
            .map_or(DEFAULT_ICON_EFFECT_FRAME_MS, |c| c.hud.icon_effect_frame_ms)
    }

    /// Which overlay a slot holding `item` should draw, if any.
    fn select(
        &self,
        row: Option<&ItemDataRow>,
        item: Option<&InventoryItem>,
    ) -> Option<(Flipbook, &'static str, Color)> {
        icon_effect_for(row, item, &self.magic_options, self.frame_ms())
    }

    /// Show or hide one slot's overlay and point it at the right sheet.
    ///
    /// Shared by the three grids so a rare item looks the same whether it is in
    /// the bag, worn, or in the avatar view — the old glow drew only in the
    /// bag, which read as "equipping it made it ordinary".
    fn apply(
        &mut self,
        asset_server: &AssetServer,
        child: Entity,
        effect: Option<(Flipbook, &'static str, Color)>,
    ) {
        let Ok((mut visibility, mut slot_effect, mut image)) = self.slots.get_mut(child) else {
            return;
        };
        match effect {
            Some((flipbook, file, tint)) => {
                // The grid is assigned unconditionally: it is cheap, and making
                // it a precondition of the load is what broke this before.
                slot_effect.flipbook = flipbook;
                if slot_effect.sheet != file {
                    slot_effect.sheet = file;
                    image.image = asset_server.load(format!("{ICON_EFFECT_DIR}{file}"));
                    // Start at frame 0 so a slot that just changed sheets does
                    // not show one frame of the previous grid's tiling.
                    image.rect = Some(flipbook.frame_rect(0));
                }
                if image.color != tint {
                    image.color = tint;
                }
                *visibility = Visibility::Inherited;
            }
            None => *visibility = Visibility::Hidden,
        }
    }
}

/// Spawn the (hidden) animated overlay over a slot's icon.
///
/// Sized and positioned to the icon rather than the cell: the sheets are 32x32
/// tiles authored for the icon box, so stretching one across the whole cell
/// would misalign the art with what it decorates.
fn spawn_slot_icon_effect(cell: &mut ChildSpawnerCommands, inset: f32, size: f32) {
    let s = hud_scale();
    cell.spawn((
        SlotIconEffect {
            flipbook: ICON_EFFECT_RARE.flipbook(DEFAULT_ICON_EFFECT_FRAME_MS),
            // No sheet loaded yet; the first `apply` fills it in.
            sheet: "",
        },
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(inset * s),
            top: Val::Px(inset * s),
            width: Val::Px(size * s),
            height: Val::Px(size * s),
            ..default()
        },
        ImageNode {
            image: Handle::default(),
            image_mode: NodeImageMode::Stretch,
            ..default()
        },
        Visibility::Hidden,
        Pickable::IGNORE,
    ));
}

fn spawn_slot_icon(cell: &mut ChildSpawnerCommands, inset: f32, size: f32) {
    let s = hud_scale();
    cell.spawn((
        SlotIcon,
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(inset * s),
            top: Val::Px(inset * s),
            width: Val::Px(size * s),
            height: Val::Px(size * s),
            ..default()
        },
        ImageNode {
            image: Handle::default(),
            image_mode: NodeImageMode::Stretch,
            ..default()
        },
        Visibility::Hidden,
        Pickable::IGNORE,
    ));
}

enum FrameCenter {
    /// Solid fill for the interior.
    Color(Color),
    /// Open center (the content lays its own background).
    None,
}

/// Corner/edge thicknesses of a 9-slice frame: `side` is the corner width
/// (left/right columns), `bar` the corner height (top/bottom rows).
struct FrameBorders {
    side: f32,
    bar: f32,
}

/// The 9-slice window frame recipe from `system_window.rs`: 8 border pieces
/// in a 3x3 CSS grid (corners fixed, edges stretched), plus the chosen center.
fn spawn_frame(
    parent: &mut ChildSpawnerCommands,
    asset_server: &AssetServer,
    dir: &str,
    rect: (f32, f32, f32, f32),
    borders: FrameBorders,
    center: FrameCenter,
) {
    let s = hud_scale();
    let (side, bar) = (borders.side * s, borders.bar * s);
    let piece = |name: &str| asset_server.load::<Image>(format!("{dir}{name}.ddj"));
    parent
        .spawn((
            Node {
                display: Display::Grid,
                grid_template_columns: vec![
                    RepeatedGridTrack::px(1, side),
                    RepeatedGridTrack::flex(1, 1.0),
                    RepeatedGridTrack::px(1, side),
                ],
                grid_template_rows: vec![
                    RepeatedGridTrack::px(1, bar),
                    RepeatedGridTrack::flex(1, 1.0),
                    RepeatedGridTrack::px(1, bar),
                ],
                ..abs_node(rect, s)
            },
            Pickable::IGNORE,
        ))
        .with_children(|g| {
            // Row-major: TL, T, TR, L, center, R, BL, B, BR.
            slice(g, Some(piece("left_up")));
            slice(g, Some(piece("mid_up")));
            slice(g, Some(piece("right_up")));
            slice(g, Some(piece("left_side")));
            match center {
                FrameCenter::Color(color) => {
                    g.spawn((
                        Node {
                            width: Val::Percent(100.0),
                            height: Val::Percent(100.0),
                            ..default()
                        },
                        BackgroundColor(color),
                        Pickable::IGNORE,
                    ));
                }
                FrameCenter::None => {
                    slice(g, None);
                }
            }
            slice(g, Some(piece("right_side")));
            slice(g, Some(piece("left_down")));
            slice(g, Some(piece("mid_down")));
            slice(g, Some(piece("right_down")));
        });
}

/// One frame grid cell; `None` leaves the cell empty (spacer).
fn slice(grid: &mut ChildSpawnerCommands, image: Option<Handle<Image>>) {
    let mut cell = grid.spawn((
        Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        },
        Pickable::IGNORE,
    ));
    if let Some(image) = image {
        cell.insert(ImageNode {
            image,
            image_mode: NodeImageMode::Stretch,
            ..default()
        });
    }
}

// --- Systems ----------------------------------------------------------------

/// Show/hide the window from `InventoryState.open`; closing also clears the
/// hover/drag state so no tooltip or ghost lingers.
pub fn apply_inventory_visibility(
    mut state: ResMut<InventoryState>,
    mut roots: Query<&mut Node, With<InventoryRoot>>,
    ghosts: Query<Entity, With<DragGhost>>,
    mut commands: Commands,
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
    if !state.open && (state.hovered_slot.is_some() || state.drag.is_some()) {
        state.hovered_slot = None;
        state.drag = None;
        for ghost in ghosts.iter() {
            commands.entity(ghost).despawn();
        }
    }
}

/// Swap the equipment panel between gear and avatar view: toggle the two
/// slot sets' visibility and give the toggle button the other view's art.
#[allow(clippy::type_complexity)]
pub fn apply_avatar_view(
    state: Res<InventoryState>,
    asset_server: Res<AssetServer>,
    mut equip_cells: Query<&mut Visibility, (With<EquipSlotCell>, Without<AvatarSlotCell>)>,
    mut avatar_cells: Query<&mut Visibility, (With<AvatarSlotCell>, Without<EquipSlotCell>)>,
    mut toggle: Query<(&mut ImageButtonStyle, &mut ImageNode), With<AvatarToggleButton>>,
) {
    if !state.is_changed() {
        return;
    }
    let avatar = state.avatar_view;
    for mut visibility in equip_cells.iter_mut() {
        *visibility = if avatar {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
    }
    for mut visibility in avatar_cells.iter_mut() {
        *visibility = if avatar {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
    // vanilla shows the art of the view the button switches TO
    let stem = if avatar {
        "equipment_button"
    } else {
        "avata_button"
    };
    for (mut style, mut image) in toggle.iter_mut() {
        style.normal = asset_server.load(format!("{EQ_SLOT_DIR}{stem}.ddj"));
        style.hover = asset_server.load(format!("{EQ_SLOT_DIR}{stem}_focus.ddj"));
        style.press = asset_server.load(format!("{EQ_SLOT_DIR}{stem}_press.ddj"));
        image.image = style.normal.clone();
    }
}

pub fn inventory_needs_refresh(
    changed: Query<(), (With<Player>, Changed<Inventory>)>,
    state: Res<InventoryState>,
    item_data: Res<ClientItemData>,
    fresh: Query<(), Added<InventoryRoot>>,
) -> bool {
    !changed.is_empty() || state.is_changed() || item_data.is_changed() || !fresh.is_empty()
}

/// Repaint everything data-driven: bag icons + stack counts for the active
/// page, equipment icons, tab visibility/art and the gold amount. Reads the
/// player's [`Inventory`] component; without one (dev sandbox, pre-0x3013)
/// everything renders empty.
/// The number a slot should display, or `None` for an item that does not
/// stack. Shared by the bag grid and the equipment panel — ammunition equips,
/// so "stackable" and "in an equipment slot" are not mutually exclusive.
fn stack_label(data: Option<&ItemTypeData>) -> Option<u32> {
    match data? {
        ItemTypeData::Expendable { stack_count, .. } => Some(*stack_count as u32),
        ItemTypeData::MagicCube { elixir_count } => Some(*elixir_count),
        _ => None,
    }
}

/// Write a slot's count label. A single item shows no number (vanilla only
/// labels real stacks), which is also why an ordinary worn sword stays bare
/// now that equipment cells carry a label at all.
fn apply_slot_count(text: &mut Text, visibility: &mut Visibility, count: Option<u32>) {
    match count {
        Some(count) if count > 1 => {
            let value = count.to_string();
            if text.0 != value {
                text.0 = value;
            }
            *visibility = Visibility::Inherited;
        }
        _ => *visibility = Visibility::Hidden,
    }
}

#[allow(clippy::type_complexity)]
pub fn refresh_inventory(
    mut state: ResMut<InventoryState>,
    inventories: Query<&Inventory, With<Player>>,
    players: Query<&CharacterInfo, With<Player>>,
    item_data: Res<ClientItemData>,
    char_data: Res<ClientCharacterData>,
    asset_server: Res<AssetServer>,
    grid_cells: Query<(&InventoryGridCell, &Children)>,
    equip_cells: Query<(&EquipSlotCell, &Children)>,
    avatar_cells: Query<(&AvatarSlotCell, &Children)>,
    mut icons: Query<
        (&mut ImageNode, &mut Visibility),
        (With<SlotIcon>, Without<InventoryTabCell>),
    >,
    mut effects: IconEffects,
    mut counts: Query<
        (&mut Text, &mut Visibility),
        (
            With<SlotCount>,
            Without<SlotIcon>,
            Without<InventoryTabCell>,
            Without<GoldText>,
        ),
    >,
    mut tabs: Query<(&InventoryTabCell, &mut ImageNode, &mut Node), Without<SlotIcon>>,
    mut tab_labels: Query<(&InventoryTabLabel, &mut TextColor)>,
    mut gold: Query<&mut Text, (With<GoldText>, Without<SlotCount>)>,
) {
    let empty = Inventory::default();
    let inventory = inventories.single().unwrap_or(&empty);

    // Local player's level and sex drive the un-equippable red tint below.
    let (player_level, player_gender) = local_equip_context(&players, &char_data);

    // keep the page valid as the inventory (re)arrives
    let pages = inventory.page_count();
    if state.active_page >= pages {
        state.active_page = pages - 1;
    }
    let page = state.active_page;

    for (cell, children) in grid_cells.iter() {
        let slot = BAG_FIRST_SLOT + page * SLOTS_PER_PAGE + cell.index;
        let item = inventory.get(slot);
        let row = item.and_then(|item| item_data.get(&(item.ref_id as i32)));
        let icon = row.and_then(|row| row.icon_path());
        // Wash the icon when the item cannot be used as-is: red for gear the
        // character cannot equip, blue for a COS scroll whose pet is dead
        // (the server refuses the summon until it is revived). The dead-pet
        // check comes first — it is a property of *this* scroll's pet, where
        // the equip check is about the character.
        let tint = match row {
            _ if item.is_some_and(|item| item.data.cos_is_dead()) => DEAD_PET_TINT,
            Some(row) if !can_equip(row, player_level, player_gender, &item_data, inventory) => {
                UNEQUIPPABLE_TINT
            }
            _ => Color::WHITE,
        };
        // The animated edge, gated on the icon: an overlay with no icon under
        // it is just art floating in an apparently empty slot. The summoned-COS
        // case reads straight off the wire — 0x3013 seeds the state and
        // 0x3040's `0x40` mask bit keeps it current — so it needs no link from
        // the spawned COS entity back to the slot (there is none, and growth
        // re-keys the pet's ref id anyway).
        let effect = icon.is_some().then(|| effects.select(row, item)).flatten();
        let count = stack_label(item.map(|item| &item.data));
        for child in children.iter() {
            if let Ok((mut image, mut visibility)) = icons.get_mut(child) {
                match &icon {
                    Some(path) => {
                        image.image = asset_server.load(path.clone());
                        image.color = tint;
                        *visibility = Visibility::Inherited;
                    }
                    None => *visibility = Visibility::Hidden,
                }
            }
            if let Ok((mut text, mut visibility)) = counts.get_mut(child) {
                apply_slot_count(&mut text, &mut visibility, count);
            }
            effects.apply(&asset_server, child, effect);
        }
    }

    for (cell, children) in equip_cells.iter() {
        let item = inventory.get(cell.slot);
        let row = item.and_then(|item| item_data.get(&(item.ref_id as i32)));
        let icon = row.and_then(|row| row.icon_path());
        let effect = icon.is_some().then(|| effects.select(row, item)).flatten();
        // Equipped ammunition carries a stack (slot 7, up to 10000), so this
        // loop needs the bag's count treatment too — without it an equipped
        // quiver rendered as a bare icon with no quantity at all.
        let count = stack_label(item.map(|item| &item.data));
        for child in children.iter() {
            if let Ok((mut image, mut visibility)) = icons.get_mut(child) {
                match &icon {
                    Some(path) => {
                        image.image = asset_server.load(path.clone());
                        *visibility = Visibility::Inherited;
                    }
                    None => *visibility = Visibility::Hidden,
                }
            }
            if let Ok((mut text, mut visibility)) = counts.get_mut(child) {
                apply_slot_count(&mut text, &mut visibility, count);
            }
            effects.apply(&asset_server, child, effect);
        }
    }

    for (cell, children) in avatar_cells.iter() {
        let item = inventory.get_avatar(cell.slot);
        let row = item.and_then(|item| item_data.get(&(item.ref_id as i32)));
        let icon = row.and_then(|row| row.icon_path());
        let effect = icon.is_some().then(|| effects.select(row, item)).flatten();
        for child in children.iter() {
            if let Ok((mut image, mut visibility)) = icons.get_mut(child) {
                match &icon {
                    Some(path) => {
                        image.image = asset_server.load(path.clone());
                        *visibility = Visibility::Inherited;
                    }
                    None => *visibility = Visibility::Hidden,
                }
            }
            effects.apply(&asset_server, child, effect);
        }
    }

    for (tab, mut image, mut node) in tabs.iter_mut() {
        let display = if tab.page < pages {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != display {
            node.display = display;
        }
        image.image = asset_server.load(if tab.page == page {
            TAB_ON_DDJ
        } else {
            TAB_OFF_DDJ
        });
    }
    for (label, mut color) in tab_labels.iter_mut() {
        let target = if label.page == page {
            Color::WHITE
        } else {
            Color::srgb(0.65, 0.65, 0.65)
        };
        if color.0 != target {
            color.0 = target;
        }
    }

    for mut text in gold.iter_mut() {
        let value = format_thousands(inventory.gold);
        if text.0 != value {
            text.0 = value;
        }
    }
}

/// Advance every visible slot icon effect to the frame its sheet is on.
///
/// Its own per-frame system rather than part of `refresh_inventory`, which is
/// change-gated (`inventory_needs_refresh`) and therefore cannot animate
/// anything — an animation driven from there would freeze the moment the
/// inventory stopped changing, i.e. immediately.
pub fn animate_slot_icon_effects(
    time: Res<Time>,
    mut effects: Query<(&mut ImageNode, &Visibility, &SlotIconEffect)>,
) {
    let now = time.elapsed_secs();
    for (mut image, visibility, effect) in effects.iter_mut() {
        if *visibility == Visibility::Hidden {
            continue;
        }
        let rect = Some(effect.flipbook.rect_at(now));
        if image.rect != rect {
            image.rect = rect;
        }
    }
}

fn on_tab_activate(
    activate: On<Activate>,
    tabs: Query<&InventoryTabCell>,
    mut state: ResMut<InventoryState>,
) {
    if let Ok(tab) = tabs.get(activate.entity) {
        if state.active_page != tab.page {
            state.active_page = tab.page;
        }
    }
}

pub fn format_thousands(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

// --- Item carrying (pick up / drop) -----------------------------------------

/// Wire slot a cell entity stands for (bag cells depend on the active page).
fn wire_slot_of(
    entity: Entity,
    grid: &Query<&InventoryGridCell>,
    equip: &Query<&EquipSlotCell>,
    state: &InventoryState,
) -> Option<u8> {
    if let Ok(cell) = grid.get(entity) {
        return Some(BAG_FIRST_SLOT + state.active_page * SLOTS_PER_PAGE + cell.index);
    }
    equip.get(entity).ok().map(|cell| cell.slot)
}

/// Every equip slot an item is ALLOWED to occupy, derived from its itemdata
/// TypeIDs (verified against a live capture: HA/SA/BA/LA/AA/FA armor pieces
/// occupy wire slots 0/2/1/4/3/5). One slot for everything except a ring,
/// which fits either hand.
///
/// Split from [`equip_target_slot`] because the two questions are different:
/// "where does this go by default" needs the inventory to pick a free hand,
/// "may it go HERE" must not — a ring dragged onto the *occupied* other hand
/// is a legal swap, and judging it by the default target would refuse it.
fn equip_slots(type_ids: (u32, u32, u32, u32)) -> Option<&'static [u8]> {
    let (tid1, tid2, tid3, tid4) = type_ids;
    // Ammunition is the one equippable EXPENDABLE, so it has to be answered
    // before the equipment guard below turns every 3/3/* item away. Arrows
    // (3/3/4/1) and bolts (3/3/4/2) share the secondary hole with the shield,
    // exactly as vanilla does — hence slot 7 and its unchanged "shield" art.
    //
    // Corpus-verified against the user's itemdata: the (3,3,4,_) bucket is
    // exactly eight rows and every one is ammo, so matching on TID3 alone
    // cannot widen this to potions or scrolls.
    if (tid1, tid2, tid3) == (3, 3, 4) {
        return Some(&[7]);
    }
    if tid1 != 3 || tid2 != 1 {
        return None;
    }
    Some(match (tid3, tid4) {
        (1..=3, 1) => &[0],  // head
        (1..=3, 2) => &[2],  // shoulder
        (1..=3, 3) => &[1],  // body
        (1..=3, 4) => &[4],  // legs
        (1..=3, 5) => &[3],  // gauntlets
        (1..=3, 6) => &[5],  // boots
        (4, _) => &[7],      // shield
        (6, _) => &[6],      // weapon
        (5, 1) => &[9],      // earring
        (5, 2) => &[10],     // necklace
        (5, 3) => &[11, 12], // ring — either hand
        (7, _) => &[8],      // job gear
        _ => return None,
    })
}

/// The equip slot an item belongs into by default. Rings prefer the empty hand.
fn equip_target_slot(type_ids: (u32, u32, u32, u32), inventory: &Inventory) -> Option<u8> {
    let slots = equip_slots(type_ids)?;
    slots
        .iter()
        .copied()
        .find(|slot| inventory.get(*slot).is_none())
        .or_else(|| slots.first().copied())
}

/// Whether a click-drop of `source` onto `target` is worth sending at all.
///
/// A bag slot always is. An EQUIPMENT slot only takes an item that belongs
/// there, which is the rule [`equip_panel_drop`] already applies when the
/// carried item lands on the panel background — the per-cell path just never
/// checked, so dropping a potion on the weapon hole put a move on the wire
/// vanilla would never make.
///
/// Judged against [`equip_slots`] rather than the default target, so swapping
/// a ring into the occupied other hand still goes out. Without an inventory to
/// judge from (the dev sandbox) nothing is refused — this is a guard against a
/// nonsense move, not a second authority on what is legal.
fn move_is_worth_sending(
    source: u8,
    target: u8,
    inventory: &Inventory,
    item_data: &ClientItemData,
) -> bool {
    if target >= BAG_FIRST_SLOT {
        return true;
    }
    inventory
        .get(source)
        .and_then(|item| item_data.get(&(item.ref_id as i32)))
        .and_then(|row| row.type_ids())
        .and_then(equip_slots)
        .is_some_and(|slots| slots.contains(&target))
}

/// The armor "family" of a body-armor piece by its TID3: garment (0) vs the
/// protector/armor family (1). Garments cannot be worn alongside
/// protector/armor pieces; protector and armor mix freely. `None` for anything
/// that isn't body armor (weapons, shields, accessories, non-equipment).
fn armor_family(tid3: u32) -> Option<u8> {
    match tid3 {
        1 | 9 => Some(0),           // garment
        2 | 3 | 10 | 11 => Some(1), // protector / armor
        _ => None,
    }
}

/// The local character's `(level, sex)` — the two facts
/// [`equip_criteria`] needs about the *player* rather than the item.
///
/// Shared because it now has two consumers ([`refresh_inventory`]'s slot wash
/// and the tooltip's requirement lines) and they must not disagree: a tooltip
/// that reddened `Required level` for a level the wash considered fine would be
/// two answers to one question, visible at once. Level comes from the 0x3013
/// stats block, sex from the character's own `characterdata` row.
pub(super) fn local_equip_context(
    players: &Query<&CharacterInfo, With<Player>>,
    char_data: &ClientCharacterData,
) -> (u32, Option<&'static str>) {
    players
        .single()
        .ok()
        .and_then(|info| info.stats.as_ref())
        .map(|stats| {
            (
                stats.level as u32,
                char_data
                    .get(&(stats.ref_id as i32))
                    .and_then(|row| row.gender()),
            )
        })
        .unwrap_or((0, None))
}

/// Which of the vanilla equip conditions this character meets for one item.
///
/// Idea: this used to be a bare `bool` ([`can_equip`], still here as the
/// one-question wrapper), which was enough for the slot icon's red wash — a
/// wash has nothing to say about *why*. The tooltip does: it already prints
/// `Sex:` and `Required level` as neutral facts, and the only thing missing to
/// turn them into the reason was carrying the individual verdicts out of here.
/// So the checks are unchanged and in the same order; only the return type
/// grew.
///
/// A non-equipment item meets everything by construction, which keeps the
/// wrapper's "never tinted" behaviour without a special case at each caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct EquipCriteria {
    /// `required_level()` against the character's level.
    pub level_ok: bool,
    /// The item's required sex against the character's. Universal items
    /// (`gender() == None`) always pass.
    pub sex_ok: bool,
    /// Garment ↔ protector/armor mixing against the pieces already worn.
    pub family_ok: bool,
}

impl EquipCriteria {
    /// Everything met — the answer for non-equipment and for a legal item.
    pub(super) const MET: Self = Self {
        level_ok: true,
        sex_ok: true,
        family_ok: true,
    };

    /// Whether the item can be equipped as things stand.
    pub fn all_met(self) -> bool {
        self.level_ok && self.sex_ok && self.family_ok
    }
}

/// Evaluate the vanilla equip conditions for `row`: required level, matching
/// sex (universal items always pass), and the garment↔protector/armor mixing
/// rule against the pieces already worn.
pub(super) fn equip_criteria(
    row: &ItemDataRow,
    player_level: u32,
    player_gender: Option<&str>,
    item_data: &ClientItemData,
    inventory: &Inventory,
) -> EquipCriteria {
    let Some(tids) = row.type_ids() else {
        return EquipCriteria::MET;
    };
    let (tid1, tid2, tid3, _) = tids;
    if (tid1, tid2) != (3, 1) {
        return EquipCriteria::MET; // not equipment
    }
    let level_ok = !row.required_level().is_some_and(|req| player_level < req);
    // Universal items (`gender() == None`) fit either sex.
    let sex_ok = match (row.gender(), player_gender) {
        (Some(item_sex), Some(player_sex)) => item_sex == player_sex,
        _ => true,
    };
    // Garment ↔ protector/armor mixing: every worn armor piece (except the one
    // this item would replace) must share the item's family.
    let mut family_ok = true;
    if let Some(family) = armor_family(tid3) {
        let target = equip_target_slot(tids, inventory);
        for slot in 0..6u8 {
            if Some(slot) == target {
                continue;
            }
            let Some(worn) = inventory.get(slot) else {
                continue;
            };
            let worn_family = item_data
                .get(&(worn.ref_id as i32))
                .and_then(|r| r.type_ids())
                .and_then(|(_, _, t3, _)| armor_family(t3));
            if worn_family.is_some_and(|wf| wf != family) {
                family_ok = false;
                break;
            }
        }
    }
    EquipCriteria {
        level_ok,
        sex_ok,
        family_ok,
    }
}

/// Whether the local player can equip `row` — [`equip_criteria`] reduced to the
/// one question the slot wash asks. Non-equipment items are always
/// "equippable" (never tinted).
fn can_equip(
    row: &ItemDataRow,
    player_level: u32,
    player_gender: Option<&str>,
    item_data: &ClientItemData,
    inventory: &Inventory,
) -> bool {
    equip_criteria(row, player_level, player_gender, item_data, inventory).all_met()
}

/// Pick up the item in `slot`: remember it in `InventoryState.drag` and spawn
/// the cursor-following ghost icon.
fn begin_carry(
    slot: u8,
    cursor: Vec2,
    state: &mut InventoryState,
    inventories: &Query<&Inventory, With<Player>>,
    item_data: &ClientItemData,
    asset_server: &AssetServer,
    cam_query: &Query<Entity, With<Camera2d>>,
    commands: &mut Commands,
) {
    let Some(item) = inventories.single().ok().and_then(|inv| inv.get(slot)) else {
        return;
    };
    let Some(icon) = item_data
        .get(&(item.ref_id as i32))
        .and_then(|row| row.icon_path())
    else {
        return;
    };
    let Ok(camera) = cam_query.single() else {
        return;
    };
    let icon: Handle<Image> = asset_server.load(icon);
    state.drag = Some(slot);

    let size = ICON_SIZE * hud_scale();
    commands.spawn(drag_ghost_bundle(
        "Inventory Drag Ghost",
        icon,
        cursor,
        size,
        camera,
    ));
}

/// Stop carrying: clear the state and despawn the ghost.
///
/// `pub(crate)` because every window that can receive a carried item has to end
/// the carry when it takes one, and while this was private each of them
/// re-inlined the body — storage, the shop, alchemy, the magic-attribute grant,
/// the quickslot bar and the drop confirm all had their own copy. Clearing
/// `drag` without despawning the ghost (or the reverse) leaves the cursor
/// holding an item that is no longer being carried, so the two halves belong
/// together in one place.
pub(crate) fn end_carry(
    state: &mut InventoryState,
    ghosts: &Query<Entity, With<DragGhost>>,
    commands: &mut Commands,
) {
    state.drag = None;
    for ghost in ghosts.iter() {
        commands.entity(ghost).despawn();
    }
}

/// Ask the server to move the carried item. The inventory itself only changes
/// when 0xB034 confirms (model.rs).
fn send_move(
    source: u8,
    target: u8,
    inventories: &Query<&Inventory, With<Player>>,
    conn: &Query<&SilkroadConnection, With<AgentConnection>>,
) {
    let amount = inventories
        .single()
        .ok()
        .and_then(|inv| inv.get(source))
        .map(|item| match &item.data {
            ItemTypeData::Expendable { stack_count, .. } => *stack_count,
            _ => 1,
        })
        .unwrap_or(1);
    let Ok(conn) = conn.single() else {
        warn!("inventory: not sending move, no agent connection");
        return;
    };
    let request = InventoryOperationRequest::Move {
        source,
        target,
        amount,
    };
    debug!("inventory: requesting move {source} -> {target} (x{amount})");
    if let Err(e) = conn.get_sender().send(Packet::from(request).into()) {
        error!("network: failed to send InventoryOperationRequest: {}", e.0);
    }
}

/// The "next click is the argument" modes a slot press has to check before it
/// starts a carry: the store's single-item repair, and item-on-item use.
///
/// Bundled because `on_slot_press` is at Bevy's system-parameter ceiling —
/// five separate resources here pushed it past the limit and the observer
/// stopped satisfying `IntoSystem`.
#[derive(bevy::ecs::system::SystemParam)]
pub struct ArmedModes<'w> {
    repair_mode: ResMut<'w, crate::plugins::hud::store::model::RepairMode>,
    repair_confirm: ResMut<'w, crate::plugins::hud::store::model::RepairConfirm>,
    pending_use: ResMut<'w, super::use_on_item::PendingItemUse>,
    use_confirm: ResMut<'w, super::use_on_item::UseOnItemConfirm>,
    history: ResMut<'w, crate::plugins::hud::chat::model::ChatHistory>,
    /// Needed by the repair estimate, which now skips items carrying
    /// `MATTR_NOT_REPARABLE` — and that classification lives in the magic-option
    /// table, not in itemdata.
    magic_options: Res<'w, crate::plugins::textdata::ClientMagicOptions>,
}

/// What a right-click on a slot resolves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RightClick {
    /// Move the item to this slot — equipping it from the bag, or unequipping
    /// it back into the bag.
    Move(u8),
    /// Use it (0x704C): a potion, a COS summon scroll, …
    Use,
    /// Arm it for a target item: the cursor changes and the next slot click
    /// picks what it is used *on* (pet revival — see `use_on_item`).
    UseOnItem,
    /// Nothing to do (empty slot, or no free bag slot to unequip into).
    Nothing,
}

/// Decide what a right-click on `slot` does, following the original: equipment
/// equips (or unequips), everything else is used.
fn right_click_action(
    slot: u8,
    type_ids: Option<(u32, u32, u32, u32)>,
    inventory: &Inventory,
) -> RightClick {
    if inventory.get(slot).is_none() {
        return RightClick::Nothing;
    }
    // An equipped item goes back to the first free bag slot — the same target
    // `bag_panel_drop` picks when a carried item is dropped on the bag.
    if slot < BAG_FIRST_SLOT {
        return (BAG_FIRST_SLOT..inventory.size())
            .find(|slot| inventory.get(*slot).is_none())
            .map_or(RightClick::Nothing, RightClick::Move);
    }
    // A bag item with an equip slot is equipment.
    if let Some(target) = type_ids.and_then(|ids| equip_target_slot(ids, inventory)) {
        return RightClick::Move(target);
    }
    // The classes that act on a second inventory item arm instead of firing —
    // their 0x704C body needs that item's slot, which a right-click alone
    // cannot supply.
    match type_ids {
        Some((3, 3, 1, 6)) | Some((3, 3, 13, 8 | 11 | 12 | 15 | 16)) => RightClick::UseOnItem,
        _ => RightClick::Use,
    }
}

/// Apply [`right_click_action`], reusing the paths the drag already takes —
/// `send_move` for the 0x7034 move and [`UseItemRequest`] for 0x704C — so
/// nothing new touches the wire. The use half is what summons a COS from its
/// scroll.
#[allow(clippy::too_many_arguments)]
fn right_click_slot(
    slot: u8,
    inventories: &Query<&Inventory, With<Player>>,
    item_data: &ClientItemData,
    conn: &Query<&SilkroadConnection, With<AgentConnection>>,
    item_uses: &mut MessageWriter<crate::plugins::hud::underbar::cast::UseItemRequest>,
    pending: &mut super::use_on_item::PendingItemUse,
) {
    let Ok(inventory) = inventories.single() else {
        return;
    };
    let Some(item) = inventory.get(slot) else {
        return;
    };
    let type_ids = item_data
        .get(&(item.ref_id as i32))
        .and_then(|row| row.type_ids());
    match right_click_action(slot, type_ids, inventory) {
        RightClick::Move(target) => send_move(slot, target, inventories, conn),
        RightClick::Use => {
            item_uses.write(crate::plugins::hud::underbar::cast::UseItemRequest {
                ref_id: item.ref_id,
                slot: Some(slot),
                target_slot: None,
            });
        }
        RightClick::UseOnItem => {
            // Nothing goes out yet — the next slot click supplies the target.
            pending.0 = Some(super::use_on_item::ArmedItem {
                source_slot: slot,
                ref_id: item.ref_id,
            });
        }
        RightClick::Nothing => {
            warn!("inventory: nothing to do for a right-click on slot {slot}");
        }
    }
}

/// Press on a slot cell: pick the item up when idle, drop the carried item
/// here when carrying (the second click of the vanilla click-carry). A press
/// on the source slot itself cancels; a right-click equips, unequips or uses
/// the item under the cursor (see [`right_click_slot`]).
#[allow(clippy::too_many_arguments)]
pub fn on_slot_press(
    mut press: On<Pointer<Press>>,
    grid: Query<&InventoryGridCell>,
    equip: Query<&EquipSlotCell>,
    mut state: ResMut<InventoryState>,
    inventories: Query<&Inventory, With<Player>>,
    item_data: Res<ClientItemData>,
    asset_server: Res<AssetServer>,
    cam_query: Query<Entity, With<Camera2d>>,
    ghosts: Query<Entity, With<DragGhost>>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    store: Res<crate::plugins::hud::store::model::StoreState>,
    names: Res<crate::plugins::textdata::ClientTextNames>,
    mut modes: ArmedModes,
    mut item_uses: MessageWriter<crate::plugins::hud::underbar::cast::UseItemRequest>,
    mut commands: Commands,
) {
    press.propagate(false);
    let Some(slot) = wire_slot_of(press.entity, &grid, &equip, &state) else {
        return;
    };
    if press.event.button != PointerButton::Primary {
        // A right-click while carrying just cancels the carry.
        if state.drag.is_some() {
            end_carry(&mut state, &ghosts, &mut commands);
            return;
        }
        // ...and a right-click while armed cancels the arming, which is the
        // way out of the cursor mode without clicking a target.
        if modes.pending_use.is_armed() {
            modes.pending_use.0 = None;
            return;
        }
        right_click_slot(
            slot,
            &inventories,
            &item_data,
            &conn,
            &mut item_uses,
            &mut modes.pending_use,
        );
        return;
    }
    // Armed item-on-item use (the revival flow): this click picks the target
    // instead of starting a carry. Sits beside the repair arm below because
    // both are the same "next click is the argument" idiom.
    if modes.pending_use.is_armed()
        && super::use_on_item::take_armed_click(
            slot,
            &mut modes.pending_use,
            &mut modes.use_confirm,
            &inventories,
            &item_data,
            &names,
            &mut modes.history,
        )
    {
        return;
    }
    // armed single-repair (the store's "Repair" button): this click opens
    // the repair confirmation for the item instead of starting a carry
    if modes.repair_mode.0 {
        modes.repair_mode.0 = false;
        if store.session.is_none() {
            return;
        }
        use crate::plugins::hud::store::model::{repair_cost_estimate, RepairOp, RepairPrompt};
        let Ok(inventory) = inventories.single() else {
            return;
        };
        let name = inventory
            .get(slot)
            .and_then(|item| item_data.get(&(item.ref_id as i32)))
            .and_then(|row| row.name_key())
            .and_then(|key| names.name(key))
            .unwrap_or("this item");
        modes.repair_confirm.prompt = Some(RepairPrompt {
            op: RepairOp::One(slot),
            message: format!("Repair {name}?"),
            cost: repair_cost_estimate(
                inventory,
                &item_data,
                &modes.magic_options,
                RepairOp::One(slot),
            ),
        });
        return;
    }
    match state.drag {
        None => {
            let cursor = press.pointer_location.position;
            begin_carry(
                slot,
                cursor,
                &mut state,
                &inventories,
                &item_data,
                &asset_server,
                &cam_query,
                &mut commands,
            );
        }
        Some(source) => {
            let allowed = inventories.single().map_or(true, |inv| {
                move_is_worth_sending(source, slot, inv, &item_data)
            });
            if source != slot && allowed {
                send_move(source, slot, &inventories, &conn);
            }
            end_carry(&mut state, &ghosts, &mut commands);
        }
    }
}

/// Release on a slot cell: completes a classic hold-drag when it ends over a
/// different slot. Releasing over the source keeps carrying, which is what
/// turns a plain click into the vanilla click-carry.
pub fn on_slot_release(
    mut release: On<Pointer<Release>>,
    grid: Query<&InventoryGridCell>,
    equip: Query<&EquipSlotCell>,
    mut state: ResMut<InventoryState>,
    inventories: Query<&Inventory, With<Player>>,
    item_data: Res<ClientItemData>,
    ghosts: Query<Entity, With<DragGhost>>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    mut commands: Commands,
) {
    release.propagate(false);
    if release.event.button != PointerButton::Primary {
        return;
    }
    let Some(source) = state.drag else {
        return;
    };
    let Some(target) = wire_slot_of(release.entity, &grid, &equip, &state) else {
        return;
    };
    let allowed = inventories.single().map_or(true, |inv| {
        move_is_worth_sending(source, target, inv, &item_data)
    });
    if target != source && allowed {
        send_move(source, target, &inventories, &conn);
        end_carry(&mut state, &ghosts, &mut commands);
    }
}

/// Press/release on the equipment panel background while carrying: equip the
/// carried item into its type-derived slot, like the original client (no need
/// to hit the exact hole). Reaching the background means no slot cell was hit
/// (their observers stop propagation).
fn equip_panel_drop(
    button: PointerButton,
    state: &mut InventoryState,
    inventories: &Query<&Inventory, With<Player>>,
    item_data: &ClientItemData,
    ghosts: &Query<Entity, With<DragGhost>>,
    conn: &Query<&SilkroadConnection, With<AgentConnection>>,
    commands: &mut Commands,
) {
    let Some(source) = state.drag else {
        return;
    };
    if button != PointerButton::Primary {
        end_carry(state, ghosts, commands);
        return;
    }
    let Ok(inventory) = inventories.single() else {
        return;
    };
    let Some(target) = inventory
        .get(source)
        .and_then(|item| item_data.get(&(item.ref_id as i32)))
        .and_then(|row| row.type_ids())
        .and_then(|tids| equip_target_slot(tids, inventory))
    else {
        // not equipment (or unknown): keep carrying
        return;
    };
    if target != source {
        send_move(source, target, inventories, conn);
    }
    end_carry(state, ghosts, commands);
}

pub fn on_equip_panel_press(
    press: On<Pointer<Press>>,
    mut state: ResMut<InventoryState>,
    inventories: Query<&Inventory, With<Player>>,
    item_data: Res<ClientItemData>,
    ghosts: Query<Entity, With<DragGhost>>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    mut commands: Commands,
) {
    equip_panel_drop(
        press.event.button,
        &mut state,
        &inventories,
        &item_data,
        &ghosts,
        &conn,
        &mut commands,
    );
}

pub fn on_equip_panel_release(
    release: On<Pointer<Release>>,
    mut state: ResMut<InventoryState>,
    inventories: Query<&Inventory, With<Player>>,
    item_data: Res<ClientItemData>,
    ghosts: Query<Entity, With<DragGhost>>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    mut commands: Commands,
) {
    equip_panel_drop(
        release.event.button,
        &mut state,
        &inventories,
        &item_data,
        &ghosts,
        &conn,
        &mut commands,
    );
}

/// Press/release on the bag panel outside any slot cell while carrying an
/// item out of an equipment slot: unequip into the first empty bag slot. The
/// server accepts equip→bag moves only into an empty slot (go-sro
/// `MoveItems`), which is exactly what "first empty" guarantees; the target
/// slot is always the client's choice, echoed back in 0xB034.
fn bag_panel_drop(
    button: PointerButton,
    state: &mut InventoryState,
    inventories: &Query<&Inventory, With<Player>>,
    ghosts: &Query<Entity, With<DragGhost>>,
    conn: &Query<&SilkroadConnection, With<AgentConnection>>,
    commands: &mut Commands,
) {
    let Some(source) = state.drag else {
        return;
    };
    if button != PointerButton::Primary {
        end_carry(state, ghosts, commands);
        return;
    }
    if source >= BAG_FIRST_SLOT {
        // bag-to-bag placement needs an explicit slot: keep carrying
        return;
    }
    let Ok(inventory) = inventories.single() else {
        return;
    };
    let Some(target) =
        (BAG_FIRST_SLOT..inventory.size()).find(|slot| inventory.get(*slot).is_none())
    else {
        warn!("inventory: no empty bag slot to unequip into");
        return;
    };
    send_move(source, target, inventories, conn);
    end_carry(state, ghosts, commands);
}

pub fn on_bag_panel_press(
    press: On<Pointer<Press>>,
    mut state: ResMut<InventoryState>,
    inventories: Query<&Inventory, With<Player>>,
    ghosts: Query<Entity, With<DragGhost>>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    mut commands: Commands,
) {
    bag_panel_drop(
        press.event.button,
        &mut state,
        &inventories,
        &ghosts,
        &conn,
        &mut commands,
    );
}

pub fn on_bag_panel_release(
    release: On<Pointer<Release>>,
    mut state: ResMut<InventoryState>,
    inventories: Query<&Inventory, With<Player>>,
    ghosts: Query<Entity, With<DragGhost>>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    mut commands: Commands,
) {
    bag_panel_drop(
        release.event.button,
        &mut state,
        &inventories,
        &ghosts,
        &conn,
        &mut commands,
    );
}

/// Keep the carried item's ghost icon under the cursor.
pub fn update_drag_ghost(
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut ghosts: Query<&mut Node, With<DragGhost>>,
) {
    // no drag gate: any carry (inventory item OR skill-window icon) owns a
    // ghost, and ghosts only exist while a carry is live
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

#[cfg(test)]
mod test {
    use super::*;
    use crate::plugins::hud::inventory::tooltip::InventoryTooltipRoot;
    use bevy::camera::NormalizedRenderTarget;
    use bevy::picking::backend::HitData;
    use bevy::picking::pointer::{Location, PointerId};
    use bevy::window::WindowRef;

    /// An itemdata row carrying only the columns the equip criteria read:
    /// TID1..4 (9..12), required level (33) and sex (58; 0 = Woman, 1 = Man,
    /// anything else = universal).
    /// Every icon-effect node must start with **no sheet loaded**.
    ///
    /// This is the sentinel the reload keys on. It replaced keying on the
    /// flipbook, which had a silent hole: a node spawned with the rare grid and
    /// then asked to draw the rare grid compared equal, so the image was never
    /// loaded and the node kept `Handle::default()` — an opaque white texture
    /// stretched over the icon. Every Seal item rendered as a white square.
    #[test]
    fn an_icon_effect_starts_with_no_sheet_loaded() {
        let mut app = inventory_app();
        let mut found = 0;
        let mut world = app.world_mut();
        let mut query = world.query::<(&SlotIconEffect, &ImageNode)>();
        for (effect, image) in query.iter(&world) {
            assert_eq!(
                effect.sheet, "",
                "a slot spawned already claiming a loaded sheet would never load one"
            );
            assert_eq!(
                image.image,
                Handle::default(),
                "and its image must be the placeholder until `apply` loads one"
            );
            found += 1;
        }
        assert!(found > 0, "the window spawned no icon-effect nodes at all");
    }

    /// The icon-effect sheets are transcriptions of real art, so pin the
    /// measurements: each sheet must tile **exactly** into 32x32 tiles — the
    /// icon's own size — and its declared frame count must be the grid it
    /// describes. A sheet whose grid does not divide cleanly is a mis-read
    /// header, and the animation would walk off the edge of the art.
    #[test]
    fn the_icon_effect_sheets_tile_exactly_into_icon_sized_frames() {
        for sheet in [&ICON_EFFECT_RARE, &ICON_EFFECT_NASRUN] {
            let fb = sheet.flipbook(DEFAULT_ICON_EFFECT_FRAME_MS);
            assert_eq!(
                fb.tile(),
                (32.0, 32.0),
                "{} does not tile into 32px frames",
                sheet.file
            );
            assert_eq!(
                sheet.frame_count,
                sheet.cols * sheet.rows,
                "{} declares {} frames for a {}x{} grid",
                sheet.file,
                sheet.frame_count,
                sheet.cols,
                sheet.rows
            );
            // and the last frame is inside the sheet
            let last = fb.frame_rect(sheet.frame_count - 1);
            assert!(last.max.x <= sheet.sheet.0 && last.max.y <= sheet.sheet.1);
        }
        // the measurements themselves, from the DDS headers
        assert_eq!(ICON_EFFECT_RARE.sheet, (256.0, 128.0));
        assert_eq!(ICON_EFFECT_RARE.frame_count, 32);
        assert_eq!(ICON_EFFECT_NASRUN.sheet, (512.0, 32.0));
        assert_eq!(ICON_EFFECT_NASRUN.frame_count, 16);
    }

    /// Seal grade is the headline fact about an item, so it wins over the other
    /// two uses of the same overlay. The summoned marker is last because a COS
    /// scroll can be rare *and* out at once, and "this is a Seal item" is the
    /// thing the player is scanning the bag for.
    #[test]
    fn the_seal_grade_outranks_the_other_icon_effects() {
        let magic = ClientMagicOptions::default();
        let rare = criteria_row((3, 2, 1, 1), 0, None);
        // a rare COS scroll whose pet is out picks the rare sheet
        let summoned = InventoryItem {
            slot: 13,
            rent: packets::agent::character_data::RentInfo::default(),
            ref_id: 1,
            data: ItemTypeData::CosPet {
                state: COS_STATE_SUMMONED,
                cos_ref_id: None,
                name: None,
                rent_seconds: None,
                unk: None,
            },
        };
        let mut rare_row = rare.clone();
        rare_row.0[2] = "ITEM_COS_P_WOLF_B_RARE".into();
        let (_, file, tint) = icon_effect_for(Some(&rare_row), Some(&summoned), &magic, 50.0)
            .expect("a rare item must draw an effect");
        assert_eq!(file, ICON_EFFECT_RARE.file);
        assert_eq!(tint, Color::WHITE, "rarity is not tinted");

        // the same scroll when it is NOT rare falls through to the summoned marker
        let plain = criteria_row((3, 2, 1, 1), 0, None);
        let (_, file, tint) = icon_effect_for(Some(&plain), Some(&summoned), &magic, 50.0)
            .expect("a summoned pet must be marked");
        assert_eq!(file, ICON_EFFECT_NASRUN.file);
        assert_eq!(tint, PET_SUMMONED_TINT, "the marker is the tinted one");
    }

    /// An ordinary item draws nothing — the overlay is for the few items that
    /// carry one, and a sheet on every slot would be noise.
    #[test]
    fn an_ordinary_item_draws_no_icon_effect() {
        let magic = ClientMagicOptions::default();
        let plain = criteria_row((3, 1, 6, 2), 0, None);
        assert!(icon_effect_for(Some(&plain), None, &magic, 50.0).is_none());
        assert!(icon_effect_for(None, None, &magic, 50.0).is_none());
    }

    fn criteria_row(
        type_ids: (u32, u32, u32, u32),
        required_level: u32,
        sex: Option<u32>,
    ) -> ItemDataRow {
        let mut fields = vec![String::new(); 60];
        fields[9] = type_ids.0.to_string();
        fields[10] = type_ids.1.to_string();
        fields[11] = type_ids.2.to_string();
        fields[12] = type_ids.3.to_string();
        fields[33] = required_level.to_string();
        fields[58] = sex.unwrap_or(2).to_string();
        ItemDataRow(fields)
    }

    /// Every criterion is reported independently, so the tooltip can redden the
    /// line that actually failed rather than all of them. Before this the gate
    /// was a bare `bool` and the reason was unrecoverable.
    #[test]
    fn each_equip_criterion_is_reported_on_its_own() {
        let item_data = ClientItemData::default();
        let inventory = Inventory::default();
        // a man's level-20 garment, seen by a level-10 woman: two failures
        let row = criteria_row((3, 1, 1, 1), 20, Some(1));
        let c = equip_criteria(&row, 10, Some("Woman"), &item_data, &inventory);
        assert_eq!(
            c,
            EquipCriteria {
                level_ok: false,
                sex_ok: false,
                family_ok: true
            }
        );
        assert!(!c.all_met());

        // the same item at level 20 on a man: everything met
        let c = equip_criteria(&row, 20, Some("Man"), &item_data, &inventory);
        assert_eq!(c, EquipCriteria::MET);
        assert!(c.all_met());

        // a universal item (sex column 2) fits either character
        let universal = criteria_row((3, 1, 6, 2), 0, None);
        assert!(equip_criteria(&universal, 1, Some("Woman"), &item_data, &inventory).sex_ok);
    }

    /// Non-equipment meets everything by construction — that is what keeps the
    /// slot wash off potions and scrolls without a special case per caller.
    #[test]
    fn non_equipment_meets_every_criterion() {
        let item_data = ClientItemData::default();
        let inventory = Inventory::default();
        // an HP potion with a required level the character does not have
        let potion = criteria_row((3, 3, 1, 1), 99, None);
        assert_eq!(
            equip_criteria(&potion, 1, Some("Man"), &item_data, &inventory),
            EquipCriteria::MET
        );
    }

    /// `can_equip` must stay the reduction of the criteria, not a second
    /// implementation of them — the divergence this refactor exists to prevent.
    #[test]
    fn can_equip_is_exactly_all_criteria_met() {
        let item_data = ClientItemData::default();
        let inventory = Inventory::default();
        for (level, sex) in [(1u32, Some("Man")), (99, Some("Woman")), (20, None)] {
            let row = criteria_row((3, 1, 1, 1), 20, Some(1));
            assert_eq!(
                can_equip(&row, level, sex, &item_data, &inventory),
                equip_criteria(&row, level, sex, &item_data, &inventory).all_met()
            );
        }
    }

    /// Boots the real window as `OnEnter(GameWorld)` does, so the test sees the
    /// entities the client actually spawns rather than a hand-built stand-in.
    fn inventory_app() -> App {
        let mut app = App::new();
        // `AssetPlugin` needs the IO task pool, and `App::new()` does not create
        // it — only `DefaultPlugins` does, via `TaskPoolPlugin` ahead of the
        // asset plugin. Without it this fixture panics "The IoTaskPool has not
        // been initialized yet" whenever it is the first test in the process to
        // touch assets, which is a scheduling coin-flip: it passed locally and
        // failed in the gate. Adding the plugin makes the order irrelevant.
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
        ))
        .init_asset::<Image>()
        .init_resource::<InventoryState>()
        .init_resource::<ClientUiStrings>()
        .insert_resource(FontAssets {
            one: Handle::default(),
            two: Handle::default(),
            three: Handle::default(),
            nine: Handle::default(),
        });
        app.world_mut().spawn(Camera2d);
        app.world_mut()
            .run_system_cached(spawn_inventory_window)
            .expect("spawn_inventory_window failed");
        app
    }

    fn hover(app: &mut App, cell: Entity, over: bool) {
        let window = app.world_mut().spawn(Window::default()).id();
        let location = Location {
            target: NormalizedRenderTarget::Window(
                WindowRef::Entity(window).normalize(None).unwrap(),
            ),
            position: Vec2::ZERO,
        };
        let hit = HitData::new(Entity::PLACEHOLDER, 0.0, None, None);
        if over {
            app.world_mut()
                .trigger(Pointer::new(PointerId::Mouse, location, Over { hit }, cell));
        } else {
            app.world_mut()
                .trigger(Pointer::new(PointerId::Mouse, location, Out { hit }, cell));
        }
        app.world_mut().flush();
    }

    fn cells<C: Component>(app: &mut App) -> Vec<Entity> {
        let mut query = app.world_mut().query_filtered::<Entity, With<C>>();
        query.iter(app.world()).collect()
    }

    /// #428: the popup disappeared in live play while every static reading of
    /// [`tooltip::refresh_tooltip`] found the path intact, so the untested gap
    /// was everything *upstream* of `InventoryState.hovered_slot` — the hover
    /// observers being hung on the freshly spawned cells at all, and the
    /// tooltip surface existing to paint into. Pin both against the real
    /// `spawn_inventory_window` output.
    #[test]
    fn spawning_the_window_hangs_the_hover_observers_and_the_tooltip_surface() {
        let mut app = inventory_app();

        // exactly one panel to paint into: `refresh_tooltip` early-returns on
        // `roots.single()`, so zero or two roots is a silent dead tooltip.
        assert_eq!(cells::<InventoryTooltipRoot>(&mut app).len(), 1);

        let bag = cells::<InventoryGridCell>(&mut app);
        assert_eq!(bag.len(), SLOTS_PER_PAGE as usize);
        let equip = cells::<EquipSlotCell>(&mut app);
        assert_eq!(equip.len(), EQUIP_SLOTS.len());

        // a bag cell reports its page-relative slot, and leaving clears it
        let cell = bag
            .iter()
            .copied()
            .find(|e| app.world().get::<InventoryGridCell>(*e).unwrap().index == 3)
            .unwrap();
        hover(&mut app, cell, true);
        assert_eq!(
            app.world().resource::<InventoryState>().hovered_slot,
            Some(BAG_FIRST_SLOT + 3),
            "Pointer<Over> on a bag cell did not reach on_slot_over"
        );
        hover(&mut app, cell, false);
        assert_eq!(app.world().resource::<InventoryState>().hovered_slot, None);

        // an equipment cell reports its absolute slot
        let slot = app.world().get::<EquipSlotCell>(equip[0]).unwrap().slot;
        hover(&mut app, equip[0], true);
        assert_eq!(
            app.world().resource::<InventoryState>().hovered_slot,
            Some(slot),
            "Pointer<Over> on an equipment cell did not reach on_slot_over"
        );
    }

    /// `Pointer<Over>.entity` is the entity the backend actually hit, so a
    /// pickable child (glow, icon, stack count, placeholder art) makes
    /// `on_slot_over`'s `grid.get(entity)` miss and the tooltip never opens.
    /// Every descendant of a slot cell must therefore be `Pickable::IGNORE`.
    #[test]
    fn every_child_of_a_slot_cell_is_unpickable() {
        let mut app = inventory_app();
        let mut roots = cells::<InventoryGridCell>(&mut app);
        roots.extend(cells::<EquipSlotCell>(&mut app));
        assert!(!roots.is_empty());

        let mut stack: Vec<Entity> = roots
            .iter()
            .filter_map(|e| app.world().get::<Children>(*e))
            .flat_map(|c| c.iter())
            .collect();
        assert!(!stack.is_empty(), "slot cells have no children to check");
        while let Some(entity) = stack.pop() {
            let pickable = app.world().get::<Pickable>(entity).copied();
            assert_eq!(
                pickable,
                Some(Pickable::IGNORE),
                "{entity} under a slot cell steals the hover hit"
            );
            if let Some(children) = app.world().get::<Children>(entity) {
                stack.extend(children.iter());
            }
        }
    }

    /// #579: the ghost is centred on the cursor, and all three carry sites
    /// (inventory, skill window, underbar) now get that centring from one
    /// place instead of three copies of the same arithmetic.
    #[test]
    fn drag_ghost_is_centred_on_the_cursor() {
        let cursor = Vec2::new(100.0, 200.0);
        assert_eq!(
            drag_ghost_top_left(cursor, DRAG_GHOST_SIZE),
            Vec2::new(84.0, 184.0)
        );
        // scaled sites stay centred too
        assert_eq!(
            drag_ghost_top_left(cursor, DRAG_GHOST_SIZE * 1.5),
            Vec2::new(76.0, 176.0)
        );
    }

    /// The unscaled origin is the original's own registry entry
    /// (`ginterface.txt:278`, `GDR_SELECTED_ITEM` `Rect "0,0,32,32"`), not a
    /// number someone liked.
    #[test]
    fn drag_ghost_size_matches_the_registry_entry() {
        assert_eq!(DRAG_GHOST_SIZE, 32.0);
    }

    use crate::plugins::net::inventory::Inventory;
    use packets::agent::character_data::{InventoryItem, ItemTypeData, RentInfo};

    fn item(slot: u8, ref_id: u32) -> InventoryItem {
        InventoryItem {
            slot,
            rent: RentInfo::default(),
            ref_id,
            data: ItemTypeData::Unknown,
        }
    }

    fn inventory(items: Vec<InventoryItem>) -> Inventory {
        let mut slots = vec![None; 20];
        for i in items {
            let slot = i.slot as usize;
            slots[slot] = Some(i);
        }
        Inventory {
            slots,
            avatar_slots: vec![],
            gold: 0,
        }
    }

    /// Vanilla right-click semantics: equipment equips, a scroll is used, an
    /// equipped item comes back to the bag. Before this, right-click only
    /// cancelled a carry, which is why a COS scroll did nothing.
    #[test]
    fn right_click_equips_uses_or_unequips() {
        // A sword (3/1/6/2) sitting in a bag slot equips into the weapon slot.
        let inv = inventory(vec![item(BAG_FIRST_SLOT, 100)]);
        assert_eq!(
            right_click_action(BAG_FIRST_SLOT, Some((3, 1, 6, 2)), &inv),
            RightClick::Move(6),
        );

        // A COS summon scroll (3/3/3/2) has no equip slot, so it is used.
        assert_eq!(
            right_click_action(BAG_FIRST_SLOT, Some((3, 3, 3, 2)), &inv),
            RightClick::Use,
        );
        // Ammunition is the expendable that DOES have an equip slot, so it
        // equips rather than being "used" — vanilla right-click behaviour, and
        // the reason this arm has to sit beside the scroll case above.
        assert_eq!(
            right_click_action(BAG_FIRST_SLOT, Some((3, 3, 4, 1)), &inv),
            RightClick::Move(7),
        );
        // So is anything whose itemdata row is missing.
        assert_eq!(
            right_click_action(BAG_FIRST_SLOT, None, &inv),
            RightClick::Use,
        );

        // A revival item ARMS instead of firing — its 0x704C body needs the
        // target item's slot, which a right-click alone cannot supply.
        assert_eq!(
            right_click_action(BAG_FIRST_SLOT, Some((3, 3, 1, 6)), &inv),
            RightClick::UseOnItem,
        );
        assert_eq!(
            right_click_action(BAG_FIRST_SLOT, Some((3, 3, 13, 12)), &inv),
            RightClick::UseOnItem,
        );
        // ...but the COS HP potion does not: that class targets an entity.
        assert_eq!(
            right_click_action(BAG_FIRST_SLOT, Some((3, 3, 1, 9)), &inv),
            RightClick::Use,
        );

        // An equipped item (slot < BAG_FIRST_SLOT) returns to the first free
        // bag slot, whatever its type says.
        let equipped = inventory(vec![item(6, 100), item(BAG_FIRST_SLOT, 101)]);
        assert_eq!(
            right_click_action(6, Some((3, 1, 6, 2)), &equipped),
            RightClick::Move(BAG_FIRST_SLOT + 1),
        );

        // Empty slots do nothing.
        assert_eq!(
            right_click_action(BAG_FIRST_SLOT + 5, Some((3, 1, 6, 2)), &inv),
            RightClick::Nothing,
        );
    }

    /// An equipment cell only accepts what belongs in it, so a stray drop
    /// never reaches the wire — but "belongs" is the item's whole allowed set,
    /// not its default target, or a ring could never be swapped into the
    /// occupied other hand.
    #[test]
    fn only_a_fitting_item_may_be_dropped_on_an_equipment_slot() {
        const SWORD: (u32, u32, u32, u32) = (3, 1, 6, 2);
        const RING: (u32, u32, u32, u32) = (3, 1, 5, 3);
        let data = ClientItemData::default();

        // Bag targets are never second-guessed here.
        let inv = inventory(vec![item(6, 100)]);
        assert!(move_is_worth_sending(6, BAG_FIRST_SLOT, &inv, &data));

        // Without itemdata nothing resolves, so no equip move goes out.
        assert!(!move_is_worth_sending(BAG_FIRST_SLOT, 6, &inv, &data));

        // The slot tables themselves, which is what the guard consults.
        assert_eq!(equip_slots(SWORD), Some(&[6u8][..]));
        assert_eq!(equip_slots(RING), Some(&[11u8, 12][..]));
        // a potion (3/3/...) is not equipment at all
        assert_eq!(equip_slots((3, 3, 3, 2)), None);
        // ...but ammunition is the exception: arrows and bolts are expendables
        // (3/3/4/*) that still wear the secondary hole, sharing it with the
        // shield. The potion assertion above is what pins this to TID3 == 4
        // instead of widening to every expendable.
        assert_eq!(equip_slots((3, 3, 4, 1)), Some(&[7u8][..])); // arrows
        assert_eq!(equip_slots((3, 3, 4, 2)), Some(&[7u8][..])); // bolts

        // Both hands are legal for a ring even when both are worn...
        let worn = inventory(vec![item(11, 200), item(12, 201)]);
        assert!(equip_slots(RING).is_some_and(|s| s.contains(&12)));
        // ...while the default target still falls back to the first hand.
        assert_eq!(equip_target_slot(RING, &worn), Some(11));
        // and prefers a free one when there is one
        let one_free = inventory(vec![item(11, 200)]);
        assert_eq!(equip_target_slot(RING, &one_free), Some(12));
    }

    #[test]
    fn thousands_grouping() {
        assert_eq!(format_thousands(0), "0");
        assert_eq!(format_thousands(999), "999");
        assert_eq!(format_thousands(1000), "1,000");
        assert_eq!(format_thousands(123_456_789), "123,456,789");
    }
}
