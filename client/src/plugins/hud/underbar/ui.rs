//! Underbar layout, interactions and refresh.
//!
//! Idea: like the other HUDs, every rect below is hand-transcribed from the
//! vanilla resinfo definitions (`ifunderbar.txt`, placed by ginterface.txt's
//! `GDR_UNDERBAR` — an 800x52 bar, bottom-centered) and uniformly scaled. The
//! bar art (`ub_window_01.ddj`) bakes in the slot sockets and gauge grooves;
//! slot cells are transparent hit areas over the sockets with an icon child,
//! the EXP/SP gauges are the clip-wrapper + percentage-fill recipe from the
//! player mini-info. Spawned programmatically (not bsn) because the slot
//! cells need Press/Release observers for the inventory drag interplay, like
//! the inventory grid. Quickslot content and progression numbers live in
//! `model.rs`; slot activation goes through `cast.rs`.

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui::UiTargetCamera;
use bevy::ui_widgets::{Activate, Button};

use crate::assets::FontAssets;
use crate::plugins::config::ClientConfig;
use crate::plugins::cursor::interactions::entity_select::SelectedEntity;
use crate::plugins::hud::chat::model::ChatState;
use crate::plugins::hud::community::model::CommunityState;
use crate::plugins::hud::game_window::abs_node;
use crate::plugins::hud::gauge::{gauge_art_node, gauge_crop_node, gauge_fill_width};
use crate::plugins::hud::inventory::model::InventoryState;
use crate::plugins::hud::inventory::ui::{drag_ghost_bundle, DragGhost, DRAG_GHOST_SIZE};
use crate::plugins::hud::scale::hud_scale;
use crate::plugins::hud::skill_window::model::SkillDrag;
use crate::plugins::hud::underbar::model::{
    PlayerProgress, QuickSlots, SlotAction, PAGES, SKILL_EXP_PER_SP, SLOTS_PER_PAGE,
};
use crate::plugins::hud::underbar::{cast, menu_popup};
use crate::plugins::net::inventory::{Inventory, BAG_FIRST_SLOT};
use crate::plugins::player::Player;
use crate::plugins::skills::cast::CastRequest;
use crate::plugins::system_window::{spawn_system_window, SystemWindow};
use crate::plugins::textdata::{ClientItemData, ClientLevelData, ClientSkillData};
use crate::plugins::ui_v2::style::ImageButtonStyle;

// --- Layout constants (resinfo/ifunderbar.txt, window space) ----------------

/// ginterface.txt GDR_UNDERBAR Rect "112,684,800,52": an 800x52 bar,
/// horizontally centered, flush with the bottom screen edge.
pub const BAR_W: f32 = 800.0;
pub const BAR_H: f32 = 52.0;
const BAR_BG: &str = "media://interface/underbar/ub_window_01.ddj";
const UB_DIR: &str = "media://interface/underbar/";

// GDR_DECORATE_1/2 — flourishes outside the bar proper.
const DECO_LEFT_RECT: (f32, f32, f32, f32) = (-64.0, 0.0, 64.0, 52.0);
const DECO_RIGHT_RECT: (f32, f32, f32, f32) = (800.0, 0.0, 64.0, 52.0);
// GDR_DECORATE_3 — the backdrop of the raised menu-button cluster.
const MENU_BACKDROP_RECT: (f32, f32, f32, f32) = (677.0, -40.0, 112.0, 92.0);

// GDR_GAUGE_SP (fill art ub_sp_bar.ddj, 144x8) and GDR_STATIC_SP
// (FontColor 255,220,86, HAlign right).
const SP_BAR_RECT: (f32, f32, f32, f32) = (17.0, 11.0, 144.0, 8.0);
const SP_TEXT_RECT: (f32, f32, f32, f32) = (167.0, 9.0, 48.0, 12.0);
// GDR_STATIC_EXP (FontColor 241,255,220, HAlign center) — the percentage
// text overlay only.
const EXP_TEXT_RECT: (f32, f32, f32, f32) = (18.0, 31.0, 198.0, 12.0);
// The EXP gauge itself has no resinfo control — vanilla draws it in code as
// 10 segments, one 20x20 tile per socket: ub_exp_bar (earned, green),
// ub_exp_bar_50 (half-earned, orange); empty sockets stay bare (their look
// is baked into the bar art — the gray ub_exp_bar_0 tile is NOT drawn).
// Socket geometry measured from ub_window_01.ddj: interiors x 16..216 at a
// 20px pitch, groove rows y 25..44 (each tile's right edge column is
// transparent, letting the baked divider show through).
const EXP_SEGMENTS: u8 = 10;
const EXP_SEG_X: f32 = 16.0;
const EXP_SEG_Y: f32 = 25.0;
const EXP_SEG_SIZE: f32 = 20.0;

// GDR_TMPQS_0 — the special slot (badge "M", semantics unknown).
const SPECIAL_SLOT_RECT: (f32, f32, f32, f32) = (238.0, 11.0, 32.0, 32.0);
// GDR_TMPQS_1..10 — the visible page slots (pages 2-4 repeat these rects).
const SLOT_XS: [f32; 10] = [
    289.0, 325.0, 361.0, 397.0, 433.0, 469.0, 505.0, 541.0, 577.0, 613.0,
];
const SLOT_Y: f32 = 11.0;
const SLOT_SIZE: f32 = 32.0;
/// Icon hole inside a slot socket.
const ICON_INSET: f32 = 2.0;
const ICON_SIZE: f32 = 28.0;
// GDR_QS_NUMBER_1..9,0,M — the key badges at the slots' bottom-right.
const BADGE_XS: [f32; 10] = [
    311.0, 347.0, 383.0, 418.0, 455.0, 490.0, 527.0, 563.0, 599.0, 635.0,
];
const BADGE_M_X: f32 = 259.0;
const BADGE_Y: f32 = 32.0;
const BADGE_W: f32 = 8.0;
const BADGE_H: f32 = 12.0;
// GDR_QS_INIDCTION — the arrow ring around the armed slot (slot rect - 4px).
const ARROW_SIZE: f32 = 40.0;
const ARROW_INSET: f32 = 4.0;
const ARROW_Y: f32 = 8.0;

// GDR_BTN_QUICKSLOTUP/-DOWN and GDR_STATIC_QUICKSLOT (the page digit).
const UP_ARROW_RECT: (f32, f32, f32, f32) = (652.0, 9.0, 20.0, 12.0);
const DOWN_ARROW_RECT: (f32, f32, f32, f32) = (652.0, 33.0, 20.0, 12.0);
const PAGE_DIGIT_RECT: (f32, f32, f32, f32) = (653.0, 22.0, 17.0, 11.0);

// The raised button cluster (negative y = above the bar) + the mall button.
const MENU_BTN_RECT: (f32, f32, f32, f32) = (688.0, -17.0, 60.0, 56.0);
const COMMUNITY_BTN_RECT: (f32, f32, f32, f32) = (738.0, -34.0, 24.0, 24.0);
const OPTION_BTN_RECT: (f32, f32, f32, f32) = (757.0, -15.0, 24.0, 24.0);
const MALL_BTN_RECT: (f32, f32, f32, f32) = (749.0, 12.0, 36.0, 36.0);

const EXP_TEXT_COLOR: Color = Color::srgb_u8(241, 255, 220);
const SP_TEXT_COLOR: Color = Color::srgb_u8(255, 220, 86);
const TEXT_FONT_SIZE: f32 = 9.0;

// --- Markers ----------------------------------------------------------------

#[derive(Component)]
pub struct UnderbarRoot;
/// The 800x52 bar node itself. Bar-local absolute children hang off this, so
/// the menu popup (which is placed at a negative y, above the bar) needs it.
#[derive(Component)]
pub struct UnderbarBody;
#[derive(Component)]
pub struct UbExpText;
/// One EXP gauge segment with its index (0-9).
#[derive(Component)]
pub struct UbExpSegment(pub u8);
#[derive(Component)]
pub struct UbSpText;
#[derive(Component)]
pub struct UbSpFill;
/// A visible page slot cell with its column index (0-9).
#[derive(Component)]
pub struct UbSlotCell(pub u8);
/// The special "M" slot cell.
#[derive(Component)]
pub struct UbSpecialSlotCell;
/// The icon image inside a slot cell (hidden when the slot is empty).
#[derive(Component)]
pub struct UbSlotIcon;
#[derive(Component)]
pub struct UbArrowIndicator;
#[derive(Component)]
pub struct UbPageDigit;
#[derive(Component)]
pub struct UbPageUpButton;
#[derive(Component)]
pub struct UbPageDownButton;

// --- Spawn / cleanup --------------------------------------------------------

pub fn spawn_underbar(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    fonts: Res<FontAssets>,
    cam_query: Query<Entity, With<Camera2d>>,
    mut quickslots: ResMut<QuickSlots>,
    mut progress: ResMut<PlayerProgress>,
) {
    let Ok(camera) = cam_query.single() else {
        warn!("underbar: no 2d camera to attach to");
        return;
    };
    // fresh session state (mirrors spawn_mini_info's vitals reset)
    *quickslots = QuickSlots::default();
    *progress = PlayerProgress::default();

    let s = hud_scale();

    // full-width bottom anchor centering the fixed-size bar
    commands
        .spawn((
            UnderbarRoot,
            // Hovered so the inventory's drop detector can tell "released on
            // the quickslot bar" from "released on nothing". The bar takes a
            // carried item through per-slot observers, which say nothing about
            // the strip between them.
            Hovered::default(),
            Name::from("Underbar"),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                bottom: Val::Px(0.0),
                width: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                ..default()
            },
            // above the world, below the loading overlay (200)
            GlobalZIndex(50),
            Pickable::IGNORE,
            UiTargetCamera(camera),
        ))
        .with_children(|wrap| {
            wrap.spawn((
                UnderbarBody,
                Node {
                    width: Val::Px(BAR_W * s),
                    height: Val::Px(BAR_H * s),
                    ..default()
                },
                ImageNode {
                    image: asset_server.load(BAR_BG),
                    image_mode: NodeImageMode::Stretch,
                    ..default()
                },
                Pickable::IGNORE,
            ))
            .with_children(|bar| spawn_bar_content(bar, &asset_server, &fonts));
        });
}

pub fn cleanup_underbar(mut commands: Commands, roots: Query<Entity, With<UnderbarRoot>>) {
    for entity in roots.iter() {
        commands.entity(entity).despawn();
    }
}

fn spawn_bar_content(
    bar: &mut ChildSpawnerCommands,
    asset_server: &AssetServer,
    fonts: &FontAssets,
) {
    let s = hud_scale();

    // decorations first so everything else draws on top
    for (rect, stem) in [
        (DECO_LEFT_RECT, "ub_deco_left"),
        (DECO_RIGHT_RECT, "ub_deco_right"),
        (MENU_BACKDROP_RECT, "ub_window_02"),
    ] {
        bar.spawn((
            abs_node(rect, s),
            ImageNode {
                image: asset_server.load(format!("{UB_DIR}{stem}.ddj")),
                image_mode: NodeImageMode::Stretch,
                ..default()
            },
            Pickable::IGNORE,
        ));
    }

    // SP gauge: the shared three-node crop recipe (`hud::gauge`) — track,
    // crop (the fill), art pinned at its native 144x8 so `ub_sp_bar`'s border
    // columns 0-1 / 142-143 stay put instead of travelling inward.
    let (_, _, sp_w, sp_h) = SP_BAR_RECT;
    bar.spawn((clip_node(SP_BAR_RECT, s), Pickable::IGNORE))
        .with_children(|clip| {
            clip.spawn((
                UbSpFill,
                gauge_crop_node(gauge_fill_width(1.0, sp_w * s), sp_h * s),
                Pickable::IGNORE,
            ))
            .with_children(|crop| {
                crop.spawn((
                    gauge_art_node(sp_w * s, sp_h * s),
                    ImageNode {
                        image: asset_server.load(format!("{UB_DIR}ub_sp_bar.ddj")),
                        image_mode: NodeImageMode::Stretch,
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
            });
        });

    // EXP gauge: one tile node per socket, art picked by refresh_underbar
    for segment in 0..EXP_SEGMENTS {
        bar.spawn((
            UbExpSegment(segment),
            abs_node(
                (
                    EXP_SEG_X + segment as f32 * EXP_SEG_SIZE,
                    EXP_SEG_Y,
                    EXP_SEG_SIZE,
                    EXP_SEG_SIZE,
                ),
                s,
            ),
            ImageNode {
                image: asset_server.load(format!("{UB_DIR}ub_exp_bar.ddj")),
                image_mode: NodeImageMode::Stretch,
                ..default()
            },
            Visibility::Hidden,
            Pickable::IGNORE,
        ));
    }

    // gauge text overlays
    spawn_text(
        bar,
        UbSpText,
        SP_TEXT_RECT,
        SP_TEXT_COLOR,
        Justify::Right,
        fonts,
    );
    spawn_text(
        bar,
        UbExpText,
        EXP_TEXT_RECT,
        EXP_TEXT_COLOR,
        Justify::Center,
        fonts,
    );

    // the special "M" slot + the 10 page slots, each a transparent hit area
    // over the socket baked into the bar art (Hovered feeds the skill
    // tooltip, see `update_skill_tooltip`)
    bar.spawn((
        UbSpecialSlotCell,
        Hovered::default(),
        abs_node(SPECIAL_SLOT_RECT, s),
    ))
    .observe(on_slot_press)
    .observe(on_slot_release)
    .with_children(|cell| spawn_slot_icon(cell, fonts, asset_server));
    for (column, x) in SLOT_XS.iter().enumerate() {
        bar.spawn((
            UbSlotCell(column as u8),
            Hovered::default(),
            abs_node((*x, SLOT_Y, SLOT_SIZE, SLOT_SIZE), s),
        ))
        .observe(on_slot_press)
        .observe(on_slot_click)
        .observe(on_slot_drag_start)
        .observe(on_slot_release)
        .with_children(|cell| spawn_slot_icon(cell, fonts, asset_server));
    }

    // key badges over the slots' bottom-right corners
    let badge = |stem: &str| format!("{UB_DIR}ub_number_{stem}.ddj");
    bar.spawn((
        abs_node((BADGE_M_X, BADGE_Y, BADGE_W, BADGE_H), s),
        ImageNode {
            image: asset_server.load(badge("m")),
            image_mode: NodeImageMode::Stretch,
            ..default()
        },
        Pickable::IGNORE,
    ));
    for (column, x) in BADGE_XS.iter().enumerate() {
        // key 1..9,0 for columns 0..9
        let stem = ((column + 1) % 10).to_string();
        bar.spawn((
            abs_node((*x, BADGE_Y, BADGE_W, BADGE_H), s),
            ImageNode {
                image: asset_server.load(badge(&stem)),
                image_mode: NodeImageMode::Stretch,
                ..default()
            },
            Pickable::IGNORE,
        ));
    }

    // the armed-slot arrow ring, positioned by refresh_underbar
    bar.spawn((
        UbArrowIndicator,
        abs_node(
            (SLOT_XS[0] - ARROW_INSET, ARROW_Y, ARROW_SIZE, ARROW_SIZE),
            s,
        ),
        ImageNode {
            image: asset_server.load(format!("{UB_DIR}ub_slot_arrow.ddj")),
            image_mode: NodeImageMode::Stretch,
            ..default()
        },
        Visibility::Hidden,
        Pickable::IGNORE,
    ));

    // page up/down arrows + the page digit between them
    spawn_image_button(bar, asset_server, UP_ARROW_RECT, "ub_up_arrow")
        .insert(UbPageUpButton)
        .observe(|_: On<Activate>, mut qs: ResMut<QuickSlots>| {
            if qs.page + 1 < PAGES {
                qs.page += 1;
            }
        });
    spawn_image_button(bar, asset_server, DOWN_ARROW_RECT, "ub_down_arrow")
        .insert(UbPageDownButton)
        .observe(|_: On<Activate>, mut qs: ResMut<QuickSlots>| {
            if qs.page > 0 {
                qs.page -= 1;
            }
        });
    spawn_text(
        bar,
        UbPageDigit,
        PAGE_DIGIT_RECT,
        Color::WHITE,
        Justify::Center,
        fonts,
    );

    // the raised button cluster + the item mall button. Only Option is wired
    // (toggles the Esc/system window); the rest log until their windows exist.
    spawn_image_button(bar, asset_server, MENU_BTN_RECT, "ub_menu_button").observe(on_menu_button);
    spawn_image_button(bar, asset_server, COMMUNITY_BTN_RECT, "ub_community_button").observe(
        |_: On<Activate>, mut community: ResMut<CommunityState>| {
            community.open = !community.open;
        },
    );
    spawn_image_button(bar, asset_server, OPTION_BTN_RECT, "ub_option_button")
        .observe(on_option_button);
    spawn_image_button(bar, asset_server, MALL_BTN_RECT, "ub_mall_button").observe(
        |_: On<Activate>| {
            debug!("underbar: item mall not implemented yet");
        },
    );
}

fn clip_node(rect: (f32, f32, f32, f32), s: f32) -> Node {
    Node {
        overflow: Overflow::clip(),
        ..abs_node(rect, s)
    }
}

fn spawn_slot_icon(
    cell: &mut ChildSpawnerCommands,
    fonts: &FontAssets,
    asset_server: &AssetServer,
) {
    let s = hud_scale();
    cell.spawn((
        UbSlotIcon,
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(ICON_INSET * s),
            top: Val::Px(ICON_INSET * s),
            width: Val::Px(ICON_SIZE * s),
            height: Val::Px(ICON_SIZE * s),
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
    // cooldown countdown, centered over the icon (see hud/cooldown.rs)
    cell.spawn((
        crate::plugins::hud::cooldown::CooldownOverlay,
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(0.0),
            top: Val::Px(0.0),
            width: Val::Px(SLOT_SIZE * s),
            height: Val::Px(SLOT_SIZE * s),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        // the original's clock sweep; the frame is picked per-frame in
        // hud/cooldown.rs
        crate::plugins::hud::cooldown::wedge_image(&asset_server),
        Visibility::Hidden,
        Pickable::IGNORE,
    ))
    .with_children(|overlay| {
        overlay.spawn((
            crate::plugins::hud::cooldown::CooldownText,
            Text::new(""),
            TextFont {
                font: fonts.nine.clone().into(),
                font_size: FontSize::Px(TEXT_FONT_SIZE * hud_scale()),
                ..default()
            },
            TextColor(Color::WHITE),
            Pickable::IGNORE,
        ));
    });
}

fn spawn_text<'a>(
    bar: &'a mut ChildSpawnerCommands,
    marker: impl Component,
    rect: (f32, f32, f32, f32),
    color: Color,
    justify: Justify,
    fonts: &FontAssets,
) -> EntityCommands<'a> {
    let s = hud_scale();
    let justify_content = match justify {
        Justify::Right => JustifyContent::FlexEnd,
        Justify::Center => JustifyContent::Center,
        _ => JustifyContent::FlexStart,
    };
    bar.spawn((
        marker,
        Text::new(""),
        TextFont {
            font: fonts.nine.clone().into(),
            font_size: FontSize::Px(TEXT_FONT_SIZE * hud_scale()),
            ..default()
        },
        TextColor(color),
        TextLayout::justify(justify),
        Node {
            justify_content,
            ..abs_node(rect, s)
        },
        Pickable::IGNORE,
    ))
}

/// A game-styled button in the inventory's programmatic idiom: `Button` +
/// `ImageButtonStyle` (art swap by ui_v2's `update_image_button_visuals`).
fn spawn_image_button<'a>(
    bar: &'a mut ChildSpawnerCommands,
    asset_server: &AssetServer,
    rect: (f32, f32, f32, f32),
    stem: &str,
) -> EntityCommands<'a> {
    let style = ImageButtonStyle {
        normal: asset_server.load(format!("{UB_DIR}{stem}.ddj")),
        hover: asset_server.load(format!("{UB_DIR}{stem}_focus.ddj")),
        press: asset_server.load(format!("{UB_DIR}{stem}_press.ddj")),
        ..Default::default()
    };
    bar.spawn((
        Button,
        Hovered::default(),
        abs_node(rect, hud_scale()),
        ImageNode {
            image: style.normal.clone(),
            image_mode: NodeImageMode::Stretch,
            ..default()
        },
        style,
    ))
}

// --- Interactions -----------------------------------------------------------

/// Press on a slot cell: drop-assign a carried inventory item, activate the
/// already-armed slot, or arm it (moving the arrow ring).
#[allow(clippy::too_many_arguments)]
pub fn on_slot_press(
    mut press: On<Pointer<Press>>,
    cells: Query<&UbSlotCell>,
    special: Query<(), With<UbSpecialSlotCell>>,
    mut quickslots: ResMut<QuickSlots>,
    mut inv_state: ResMut<InventoryState>,
    mut skill_drag: ResMut<SkillDrag>,
    inventories: Query<&Inventory, With<Player>>,
    ghosts: Query<Entity, With<DragGhost>>,
    selected: Res<SelectedEntity>,
    time: Res<Time>,
    mut casts: MessageWriter<CastRequest>,
    mut item_uses: MessageWriter<cast::UseItemRequest>,
    mut commands: Commands,
) {
    press.propagate(false);
    if press.event.button != PointerButton::Primary {
        return;
    }
    if inv_state.drag.is_some() || skill_drag.skill.is_some() {
        // the second click of the inventory's click-carry lands here
        drop_assign(
            press.entity,
            &cells,
            &special,
            &mut quickslots,
            &mut inv_state,
            &mut skill_drag,
            &inventories,
            &ghosts,
            &mut commands,
        );
        return;
    }
    if let Ok(cell) = cells.get(press.entity) {
        let index = quickslots.page * SLOTS_PER_PAGE + cell.0;
        let action = quickslots.visible(cell.0);
        if quickslots.armed == Some(index) {
            // fire on the Click (release-in-place) so a press-and-drag can
            // lift the slot content instead of casting
            quickslots.pending_activate = Some(index);
        } else if action.is_some() {
            quickslots.arm(index, time.elapsed_secs_f64());
            quickslots.pending_activate = None;
        }
    } else if special.contains(press.entity) {
        if let Some(action) = quickslots.special {
            cast::activate_slot(action, &selected, &mut casts, &mut item_uses);
        }
    }
}

/// Click (press + release in place) on a slot pressed while armed: cast.
pub fn on_slot_click(
    mut click: On<Pointer<Click>>,
    cells: Query<&UbSlotCell>,
    mut quickslots: ResMut<QuickSlots>,
    selected: Res<SelectedEntity>,
    mut casts: MessageWriter<CastRequest>,
    mut item_uses: MessageWriter<cast::UseItemRequest>,
) {
    click.propagate(false);
    if click.event.button != PointerButton::Primary {
        return;
    }
    let Ok(cell) = cells.get(click.entity) else {
        return;
    };
    let index = quickslots.page * SLOTS_PER_PAGE + cell.0;
    if quickslots.pending_activate.take() != Some(index) {
        return;
    }
    if let Some(action) = quickslots.visible(cell.0) {
        cast::activate_slot(action, &selected, &mut casts, &mut item_uses);
    }
}

/// Dragging a skill off its slot lifts it into the shared carry: drop it on
/// another slot to move it, release it anywhere else to discard it
/// (vanilla drag-out removal).
#[allow(clippy::too_many_arguments)]
pub fn on_slot_drag_start(
    mut drag_start: On<Pointer<DragStart>>,
    cells: Query<&UbSlotCell>,
    mut quickslots: ResMut<QuickSlots>,
    mut skill_drag: ResMut<SkillDrag>,
    skill_data: Res<ClientSkillData>,
    ghosts: Query<Entity, With<DragGhost>>,
    cam_query: Query<Entity, With<Camera2d>>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
) {
    drag_start.propagate(false);
    if drag_start.event.button != PointerButton::Primary || skill_drag.skill.is_some() {
        return;
    }
    let Ok(cell) = cells.get(drag_start.entity) else {
        return;
    };
    let index = (quickslots.page * SLOTS_PER_PAGE + cell.0) as usize;
    let Some(SlotAction::Skill { ref_id }) = quickslots.slots[index] else {
        return;
    };
    let Ok(camera) = cam_query.single() else {
        return;
    };
    quickslots.slots[index] = None;
    quickslots.pending_activate = None;
    skill_drag.skill = Some(ref_id);
    skill_drag.from_underbar = true;
    for ghost in ghosts.iter() {
        commands.entity(ghost).despawn();
    }
    let icon = skill_data
        .get(&(ref_id as i32))
        .and_then(|row| row.icon_path())
        .map(|path| asset_server.load(path))
        .unwrap_or_default();
    let size = DRAG_GHOST_SIZE * hud_scale();
    let cursor = drag_start.pointer_location.position;
    commands.spawn(drag_ghost_bundle(
        "Skill Drag Ghost",
        icon,
        cursor,
        size,
        camera,
    ));
}

/// Release on a slot cell: completes a classic hold-drag from the inventory
/// or the skill window.
#[allow(clippy::too_many_arguments)]
pub fn on_slot_release(
    mut release: On<Pointer<Release>>,
    cells: Query<&UbSlotCell>,
    special: Query<(), With<UbSpecialSlotCell>>,
    mut quickslots: ResMut<QuickSlots>,
    mut inv_state: ResMut<InventoryState>,
    mut skill_drag: ResMut<SkillDrag>,
    inventories: Query<&Inventory, With<Player>>,
    ghosts: Query<Entity, With<DragGhost>>,
    mut commands: Commands,
) {
    release.propagate(false);
    if release.event.button != PointerButton::Primary
        || (inv_state.drag.is_none() && skill_drag.skill.is_none())
    {
        return;
    }
    drop_assign(
        release.entity,
        &cells,
        &special,
        &mut quickslots,
        &mut inv_state,
        &mut skill_drag,
        &inventories,
        &ghosts,
        &mut commands,
    );
}

/// Assign the carried item/skill to the slot cell under the pointer and end
/// the carry locally — deliberately WITHOUT a server move request, the item
/// stays in the bag (quickslot assignments are client-side references).
#[allow(clippy::too_many_arguments)]
fn drop_assign(
    target: Entity,
    cells: &Query<&UbSlotCell>,
    special: &Query<(), With<UbSpecialSlotCell>>,
    quickslots: &mut QuickSlots,
    inv_state: &mut InventoryState,
    skill_drag: &mut SkillDrag,
    inventories: &Query<&Inventory, With<Player>>,
    ghosts: &Query<Entity, With<DragGhost>>,
    commands: &mut Commands,
) {
    // a carried skill icon (from the skill window or lifted off another
    // slot) wins over an item carry
    if let Some(skill_id) = skill_drag.skill.take() {
        skill_drag.from_underbar = false;
        let action = Some(SlotAction::Skill { ref_id: skill_id });
        if let Ok(cell) = cells.get(target) {
            let index = quickslots.page * SLOTS_PER_PAGE + cell.0;
            quickslots.slots[index as usize] = action;
            debug!("underbar: assigned skill {skill_id} to quickslot {index}");
        } else if special.contains(target) {
            quickslots.special = action;
            debug!("underbar: assigned skill {skill_id} to the special slot");
        }
        for ghost in ghosts.iter() {
            commands.entity(ghost).despawn();
        }
        return;
    }
    let Some(source) = inv_state.drag else {
        return;
    };
    // equipment sources keep carrying (only bag items are assignable)
    if source < BAG_FIRST_SLOT {
        return;
    }
    if let Some(item) = inventories.single().ok().and_then(|inv| inv.get(source)) {
        let action = Some(SlotAction::Item {
            ref_id: item.ref_id,
        });
        if let Ok(cell) = cells.get(target) {
            let index = quickslots.page * SLOTS_PER_PAGE + cell.0;
            quickslots.slots[index as usize] = action;
            debug!(
                "underbar: assigned item {} to quickslot {}",
                item.ref_id, index
            );
        } else if special.contains(target) {
            quickslots.special = action;
            debug!(
                "underbar: assigned item {} to the special slot",
                item.ref_id
            );
        }
    }
    inv_state.drag = None;
    for ghost in ghosts.iter() {
        commands.entity(ghost).despawn();
    }
}

/// Number keys 1-0 arm + activate the matching column of the visible page.
pub fn handle_slot_keys(
    keys: Res<ButtonInput<KeyCode>>,
    chat: Res<ChatState>,
    store_modal: Res<crate::plugins::hud::store::ui::QuantityModal>,
    mut quickslots: ResMut<QuickSlots>,
    selected: Res<SelectedEntity>,
    time: Res<Time>,
    mut casts: MessageWriter<CastRequest>,
    mut item_uses: MessageWriter<cast::UseItemRequest>,
) {
    // typing digits into the store's amount modal must not fire quickslots
    if chat.input_open || store_modal.prompt.is_some() {
        return;
    }
    const DIGITS: [KeyCode; 10] = [
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
        KeyCode::Digit6,
        KeyCode::Digit7,
        KeyCode::Digit8,
        KeyCode::Digit9,
        KeyCode::Digit0,
    ];
    for (column, key) in DIGITS.iter().enumerate() {
        if !keys.just_pressed(*key) {
            continue;
        }
        let index = quickslots.page * SLOTS_PER_PAGE + column as u8;
        let action = quickslots.visible(column as u8);
        // Re-arm even when this slot is already the armed one: the stamp is
        // what keeps the ring lit, so holding down a rotation must refresh it
        // rather than let it fade under the player's own keypresses.
        if action.is_some() {
            quickslots.arm(index, time.elapsed_secs_f64());
        }
        if let Some(action) = action {
            cast::activate_slot(action, &selected, &mut casts, &mut item_uses);
        }
    }
}

/// Expire the arm indicator: hold it lit, fade it out, then disarm.
///
/// Idea: arming used to be permanent, so the last slot the player touched kept
/// its ring for the rest of the session. This ages it out instead.
///
/// It cannot live in [`refresh_underbar`], which only runs when
/// [`underbar_needs_refresh`] sees a changed resource — a fade is exactly the
/// case where nothing changed. So this runs every frame like
/// `cooldown::update_cooldown_overlays` does over the same cells, and touches
/// [`QuickSlots`] only on the single frame it disarms, which is what wakes
/// `refresh_underbar` up to hide the ring.
///
/// The disarm is deliberate rather than cosmetic-only: `armed` is also what a
/// second mouse press casts from, so a ring that faded while the slot stayed
/// armed would leave an invisible control that still fires.
pub fn fade_armed_slot(
    time: Res<Time>,
    config: Res<ClientConfig>,
    mut quickslots: ResMut<QuickSlots>,
    mut arrows: Query<&mut ImageNode, With<UbArrowIndicator>>,
) {
    let hold = config.hud.quickslot_arm_hold_seconds;
    let fade = config.hud.quickslot_arm_fade_seconds;
    // A negative hold would expire the ring before it was ever drawn; treat it
    // as the "never expires" escape hatch rather than a broken bar.
    if hold < 0.0 || fade < 0.0 {
        return;
    }
    let Some(progress) = quickslots.arm_fade(time.elapsed_secs_f64(), hold, fade) else {
        return;
    };
    // `arm_fade` reads through `Deref`, so the resource is only marked changed
    // by the `disarm()` above — one frame, not every frame the ring is up.
    // Writing it each frame would re-run the whole underbar refresh.
    if progress >= 1.0 {
        quickslots.disarm();
        return;
    }
    for mut image in arrows.iter_mut() {
        let alpha = 1.0 - progress;
        if image.color.alpha() != alpha {
            image.color.set_alpha(alpha);
        }
    }
}

/// The Option button toggles the Esc/system window (same behavior as Esc
/// without a target selected).
/// The MENU button toggles the 4th-gen menu popup (`menu_popup.rs`), which
/// replaces the "not implemented yet" stub this button used to carry.
fn on_menu_button(
    _: On<Activate>,
    bars: Query<Entity, With<UnderbarBody>>,
    popups: Query<Entity, With<menu_popup::MenuPopupRoot>>,
    asset_server: Res<AssetServer>,
    fonts: Res<FontAssets>,
    ui_strings: Res<crate::plugins::textdata::ClientUiStrings>,
    mut commands: Commands,
) {
    let Ok(bar) = bars.single() else {
        warn!("underbar: no bar body for the menu popup");
        return;
    };
    menu_popup::toggle_menu_popup(
        bar,
        popups.iter().next(),
        &asset_server,
        &fonts,
        &ui_strings,
        hud_scale(),
        &mut commands,
    );
}

fn on_option_button(
    _: On<Activate>,
    window: Query<Entity, With<SystemWindow>>,
    cameras: Query<Entity, With<Camera2d>>,
    asset_server: Res<AssetServer>,
    ui_strings: Res<crate::plugins::textdata::ClientUiStrings>,
    mut commands: Commands,
) {
    if let Ok(open) = window.single() {
        commands.entity(open).despawn();
    } else if let Some(camera) = cameras.iter().next() {
        spawn_system_window(&mut commands, &asset_server, &ui_strings, camera);
    } else {
        warn!("underbar: no 2d camera for the system window");
    }
}

// --- Refresh ----------------------------------------------------------------

/// Run condition: state or a consulted table changed, or the bar was just
/// (re)spawned.
pub fn underbar_needs_refresh(
    quickslots: Res<QuickSlots>,
    progress: Res<PlayerProgress>,
    skill_data: Res<ClientSkillData>,
    item_data: Res<ClientItemData>,
    fresh: Query<(), Added<UnderbarRoot>>,
) -> bool {
    quickslots.is_changed()
        || progress.is_changed()
        || skill_data.is_changed()
        || item_data.is_changed()
        || !fresh.is_empty()
}

/// Push [`QuickSlots`] + [`PlayerProgress`] into the bar: slot icons, the
/// arrow ring, page digit and arrow art, EXP/SP texts and fills.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn refresh_underbar(
    quickslots: Res<QuickSlots>,
    progress: Res<PlayerProgress>,
    skill_data: Res<ClientSkillData>,
    item_data: Res<ClientItemData>,
    level_data: Res<ClientLevelData>,
    asset_server: Res<AssetServer>,
    cells: Query<(&UbSlotCell, &Children)>,
    special_cells: Query<&Children, With<UbSpecialSlotCell>>,
    (exp_text_q, sp_text_q, page_digit_q, sp_fill_q, up_button_q, down_button_q): (
        Query<Entity, With<UbExpText>>,
        Query<Entity, With<UbSpText>>,
        Query<Entity, With<UbPageDigit>>,
        Query<Entity, With<UbSpFill>>,
        Query<Entity, With<UbPageUpButton>>,
        Query<Entity, With<UbPageDownButton>>,
    ),
    mut arrows: Query<(&mut Node, &mut Visibility, &mut ImageNode), With<UbArrowIndicator>>,
    mut icons: Query<
        (&mut ImageNode, &mut Visibility),
        (With<UbSlotIcon>, Without<UbArrowIndicator>),
    >,
    mut segments: Query<
        (&UbExpSegment, &mut ImageNode, &mut Visibility),
        (Without<UbSlotIcon>, Without<UbArrowIndicator>),
    >,
    mut texts: Query<&mut Text>,
    mut nodes: Query<&mut Node, Without<UbArrowIndicator>>,
    mut buttons: Query<
        (&mut ImageButtonStyle, &mut ImageNode),
        (
            Without<UbSlotIcon>,
            Without<UbArrowIndicator>,
            Without<UbExpSegment>,
        ),
    >,
) {
    let icon_of = |action: &SlotAction| match action {
        SlotAction::Skill { ref_id } => skill_data
            .get(&(*ref_id as i32))
            .and_then(|row| row.icon_path()),
        SlotAction::Item { ref_id } => item_data
            .get(&(*ref_id as i32))
            .and_then(|row| row.icon_path()),
    };
    let mut paint = |children: &Children, action: Option<SlotAction>| {
        let icon = action.as_ref().and_then(icon_of);
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
        }
    };
    for (cell, children) in cells.iter() {
        paint(children, quickslots.visible(cell.0));
    }
    for children in special_cells.iter() {
        paint(children, quickslots.special);
    }

    // arrow ring: visible only when the armed slot is on the visible page
    let armed_column = quickslots
        .armed
        .filter(|index| index / SLOTS_PER_PAGE == quickslots.page)
        .map(|index| index % SLOTS_PER_PAGE);
    for (mut node, mut visibility, mut image) in arrows.iter_mut() {
        match armed_column {
            Some(column) => {
                node.left = Val::Px((SLOT_XS[column as usize] - ARROW_INSET) * hud_scale());
                *visibility = Visibility::Inherited;
                // Re-show at full opacity: `fade_armed_slot` leaves the alpha
                // wherever the last fade ended, and this runs on every arming.
                if image.color.alpha() != 1.0 {
                    image.color.set_alpha(1.0);
                }
            }
            None => *visibility = Visibility::Hidden,
        }
    }

    let mut set_text = |entity: Entity, value: String| {
        if let Ok(mut text) = texts.get_mut(entity) {
            if text.0 != value {
                text.0 = value;
            }
        }
    };
    for entity in page_digit_q.iter() {
        set_text(entity, (quickslots.page + 1).to_string());
    }

    // EXP: percentage of the current level's requirement (leveldata is
    // byte-identical to the server's Media.pk2, so this matches the server).
    let (exp_label, exp_fill) = match level_data.max_exp(progress.level) {
        Some(max) if max > 0 => {
            let fraction = (progress.exp_offset as f64 / max as f64).clamp(0.0, 1.0);
            (format!("{:.2}%", fraction * 100.0), fraction as f32)
        }
        _ => (String::from("-"), 0.0),
    };
    for entity in exp_text_q.iter() {
        set_text(entity, exp_label.clone());
    }
    // per-segment art: earned tiles are green (`ub_exp_bar`), empty sockets are
    // hidden (the bare socket is baked into the bar art), quantized like
    // vanilla; the text carries the precise value. A fully-earned socket uses
    // the green tile and a 50-99% one the orange `_50` tile — that half-earned
    // art ships on disk and this used to light the full green tile from 0.5,
    // so a half-earned segment read as complete (#281).
    for (segment, mut image, mut visibility) in segments.iter_mut() {
        let earned = exp_fill * EXP_SEGMENTS as f32 - segment.0 as f32;
        let stem = if earned >= 1.0 {
            Some("ub_exp_bar")
        } else if earned >= 0.5 {
            Some("ub_exp_bar_50")
        } else {
            None
        };
        match stem {
            Some(stem) => {
                image.image = asset_server.load(format!("{UB_DIR}{stem}.ddj"));
                *visibility = Visibility::Inherited;
            }
            None => *visibility = Visibility::Hidden,
        }
    }
    // SP: point count + progress toward the next point (skill_exp arrives as
    // the within-point offset; 400 skill exp = 1 SP, dump-verified)
    let sp_fill = (progress.skill_exp % SKILL_EXP_PER_SP) as f32 / SKILL_EXP_PER_SP as f32;
    for entity in sp_text_q.iter() {
        set_text(entity, progress.skill_points.to_string());
    }

    let mut set_fill = |entity: Entity, fill: f32| {
        if let Ok(mut node) = nodes.get_mut(entity) {
            // The crop node, never the art node (#630).
            node.width = gauge_fill_width(fill, SP_BAR_RECT.2 * hud_scale());
        }
    };
    for entity in sp_fill_q.iter() {
        set_fill(entity, sp_fill);
    }

    // page arrows: disabled art at the bounds
    let mut set_arrow_art = |entity: Entity, stem: &str, disabled: bool| {
        let Ok((mut style, mut image)) = buttons.get_mut(entity) else {
            return;
        };
        let art = |suffix: &str| asset_server.load(format!("{UB_DIR}{stem}{suffix}.ddj"));
        if disabled {
            style.normal = art("_disable");
            style.hover = art("_disable");
            style.press = art("_disable");
        } else {
            style.normal = art("");
            style.hover = art("_focus");
            style.press = art("_press");
        }
        image.image = style.normal.clone();
    };
    for entity in up_button_q.iter() {
        set_arrow_art(entity, "ub_up_arrow", quickslots.page + 1 >= PAGES);
    }
    for entity in down_button_q.iter() {
        set_arrow_art(entity, "ub_down_arrow", quickslots.page == 0);
    }
}

#[cfg(test)]
mod tests {
    /// Which EXP tile a socket shows, extracted so the three-way choice is
    /// testable without a running app.
    fn tile_for(earned: f32) -> Option<&'static str> {
        if earned >= 1.0 {
            Some("ub_exp_bar")
        } else if earned >= 0.5 {
            Some("ub_exp_bar_50")
        } else {
            None
        }
    }

    /// A 50-99% socket used to light the full green tile, so a half-earned
    /// segment read as complete. The orange `_50` art ships on disk and the
    /// module comment already named it (#281).
    #[test]
    fn a_half_earned_socket_uses_the_50_tile() {
        assert_eq!(tile_for(0.5), Some("ub_exp_bar_50"));
        assert_eq!(tile_for(0.99), Some("ub_exp_bar_50"));
        assert_eq!(tile_for(1.0), Some("ub_exp_bar"), "full stays green");
        assert_eq!(tile_for(0.49), None, "under half stays bare");
        assert_eq!(tile_for(0.0), None);
    }

    /// Sockets past the fill are bare, not gray: `ub_exp_bar_0` is deliberately
    /// never drawn — the empty look is baked into the bar art.
    #[test]
    fn unearned_sockets_draw_nothing() {
        for earned in [-2.0f32, -1.0, -0.1] {
            assert_eq!(tile_for(earned), None);
        }
    }

    /// `ifunderbar.txt` — the base bar's slot geometry, which the
    /// three-mechanism confusion around the extended bar (#358) must not
    /// disturb: the off-grid "M" slot `GDR_TMPQS_0` (ID 19) at
    /// `238,11,32,32`, then `TMPQS_1..40` on exactly 10 rects at x 289..613,
    /// pitch 36, y 11, 32x32 — four IDs deep per position, which is where
    /// `PAGES = 4` comes from.
    #[test]
    fn base_quickslot_rects_match_ifunderbar() {
        use super::*;
        assert_eq!(SPECIAL_SLOT_RECT, (238.0, 11.0, 32.0, 32.0));
        assert_eq!(
            SLOT_XS.len(),
            crate::plugins::hud::underbar::model::SLOTS_PER_PAGE as usize
        );
        assert_eq!((SLOT_XS[0], SLOT_XS[9]), (289.0, 613.0));
        for pair in SLOT_XS.windows(2) {
            assert_eq!(pair[1] - pair[0], 36.0);
        }
        assert_eq!((SLOT_Y, SLOT_SIZE), (11.0, 32.0));
        // the M slot is off the banked grid, not one of its positions
        assert!(!SLOT_XS.contains(&SPECIAL_SLOT_RECT.0));
        assert_eq!(
            SLOT_XS.len() * crate::plugins::hud::underbar::model::PAGES as usize,
            40
        );
    }
}
