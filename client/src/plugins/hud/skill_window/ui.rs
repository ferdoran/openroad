//! Skill window layout, interactions and refresh.
//!
//! Idea: like the other HUDs the layout is hand-transcribed from the vanilla
//! resinfo definitions (`ifskill.txt` 364×333 main window, `ifskillboard.txt`
//! per-mastery board, `ifskill_slot.txt` per-skill cell,
//! `ifskillpracticebox.txt` for the level-up confirmation) with the shared
//! `game_window` chrome standing in for the vanilla equip_window frame.
//! Structure mirrors vanilla: two tab rows — the race's top-level trees
//! (CH Weapon/Force, EU Physical/Magical/Assist; mastery table col 6, on
//! the generic `com_tab_on/off` art) over per-mastery tabs
//! (`skl_mastery_tab_on/off` art) — then one mastery board:
//! the `skl_mastery_subject` header band and the mastery's skill *branches*,
//! one `skl_mastery_bar` row each (branch icon socket + 9 skill sockets,
//! art-verified 328 = 4+32+4+9×32). A branch is the prerequisite chain of
//! skill groups extending to the right (SMASH_A → SMASH_B → ...). Learned
//! cells carry a level number and a `skl_button_add` level-up button
//! (`skl_level_max` once the ladder tops out); the closed-book art hides a
//! skill only while its mastery level is unmet — other unmet conditions
//! (prereq skill level, SP) keep the icon but drop the button, and the
//! tooltip lists every requirement (unmet ones in red). Level-ups go
//! through the practice box (confirm/cancel + SP cost)
//! instead of applying immediately. Left-press on a learned skill starts a
//! vanilla click-carry toward the underbar (shared `DragGhost` + the
//! `SkillDrag` resource, cancelled by Escape/right-click or a click that
//! misses every slot); right-press is inert — withdrawal waits for the
//! removal-box UI.

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui::{ComputedNode, Overflow, ScrollPosition, UiTargetCamera};
use bevy::ui_widgets::{Activate, Button};

use crate::assets::textdata::skilldata::SkillData;
use crate::assets::FontAssets;
use crate::plugins::hud::game_window::spawn_game_window;
use crate::plugins::hud::inventory::ui::{drag_ghost_bundle, DragGhost};
use crate::plugins::hud::magic_state_board::BuffIcon;
use crate::plugins::hud::scale::hud_scale;
use crate::plugins::hud::skill_window::model::{
    mastery_group_abbrev, mastery_name, PracticeAction, SkillDrag, SkillTreeRace, SkillWindowState,
};
use crate::plugins::hud::skill_window::withdrawal::SkillWithdrawalState;
use crate::plugins::hud::underbar::model::{PlayerProgress, QuickSlots, SlotAction};
use crate::plugins::hud::underbar::ui::{UbSlotCell, UbSpecialSlotCell};
use crate::plugins::skills::book::{self, LearnBlock, LearnReq, SkillBook, SkillGroupIndex};
use crate::plugins::skills::status::{ActiveBuffs, Frozen, Stunned};
use crate::plugins::textdata::{
    ClientLevelData, ClientMasteryData, ClientSkillData, ClientSkillGroups, ClientTextNames,
    ClientUiStrings,
};
use crate::plugins::ui_v2::style::ImageButtonStyle;

// --- Layout constants (resinfo/ifskill*.txt + art extents, window units) ----

/// Page extent: `GDR_SKILL_FRAME:CIFFrame` is `0,0,364,333` (`ifskill.txt`),
/// the same 364x333 as the MainPopup page `GDR_SKILL` id 73 occupies
/// (`ifmainpopup.txt:84`). The vanilla page carries no title bar of its own —
/// MainPopup hosting is #302 (#307 turned out to be the shared-chrome issue
/// and is closed) — so page coordinates are our content
/// coordinates and the rects below are used unshifted.
const CONTENT_W: f32 = 364.0;
const CONTENT_H: f32 = 333.0;
/// Top tree tabs use the generic (icon-free) com_tab art, 60×24, with a
/// centered text label.
const TOP_TAB_W: f32 = 60.0;
const TOP_TAB_H: f32 = 24.0;
/// Per-mastery tabs use the plain (icon-free) skl_mastery_tab art, 68×28 —
/// vanilla renders the labels as text on it.
const TAB_W: f32 = 68.0;
const TAB_H: f32 = 28.0;
const MASTERY_TABS_Y: f32 = 24.0;
/// The int_window_ 9-piece board frame (16px opaque pieces, ~4px outer
/// ridge) around the mastery board, vanilla's GDR_SKILL_BOARD frame
/// (ifskill.txt: int_window_ at 6,29,351,270) — the sub-mastery tabs sit
/// on its top edge, the header/rows/scrollbar live inside it.
const BOARD_X: f32 = 6.0;
const BOARD_W: f32 = 351.0;
const BOARD_FRAME_Y: f32 = 29.0;
const BOARD_FRAME_H: f32 = 270.0;
const BOARD_FRAME_PIECE: f32 = 16.0;
/// Mastery header band (skl_mastery_subject art is 352×44), tucked flush
/// under the sub-mastery tabs (which end at MASTERY_TABS_Y + TAB_H = 52).
const HEADER_Y: f32 = 52.0;
const HEADER_W: f32 = 352.0;
const HEADER_H: f32 = 44.0;
/// The "Lv N" mastery-level box inside the header (vanilla
/// GDR_SKILLBOARD_MASTERYLEV rect, text centered both ways).
const LEVEL_BOX: (f32, f32, f32, f32) = (291.0, 12.0, 46.0, 24.0);
/// Its localized "Lv" caption, a separate static in the data
/// (`GDR_SKILLBOARD_STATIC_LV`, `UIIT_STT_LEVEL_LV`).
const LV_LABEL_BOX: (f32, f32, f32, f32) = (267.0, 12.0, 40.0, 24.0);
/// The scrollable branch-row area, inset like vanilla's scroll region
/// (ifskillboard.txt GDR_SKILLBOARD_BOARD: 4,41,342,225, board-local — rows
/// and scrollbar packed flush). The y is ours, pushed down by the two tab
/// rows the vanilla data does not place anywhere ([U]); the height is
/// clamped to the board frame's inner bottom edge (29 + 270 - 16).
const ROWS_X: f32 = 4.0;
const ROWS_Y: f32 = 96.0;
const ROWS_H: f32 = BOARD_FRAME_Y + BOARD_FRAME_H - BOARD_FRAME_PIECE - ROWS_Y;
/// One branch row: skl_mastery_bar art, 328×60 — art-measured: the branch
/// socket frame at (2,2), then 8 skill socket frames on a 36px pitch from
/// x 35; the 32px icons sit inset in the frames (x 38+36k, y 2), which
/// keeps the frames' separators visible as small gaps between icons.
/// The button strip runs under the sockets.
const ROW_W: f32 = 328.0;
const ROW_H: f32 = 60.0;
const ROW_X: f32 = 0.0;
const ROW_GAP: f32 = 0.0;
const ROW_SOCKETS: usize = 8;
const SOCKET_Y: f32 = 2.0;
const SOCKET0_X: f32 = 38.0;
const SOCKET_PITCH: f32 = 36.0;
/// `ifskill_slot.txt` id2 `0,32,32,20` — the add button sits flush under the
/// 32px icon.
const BUTTON_Y: f32 = 32.0;
/// The skl_wnd_box SP bar under the board (`GDR_SKILL_BOTTOM_BOX` at y 299,
/// art-sized 364×36 — the art overhangs the 333 page by 2px, as in vanilla).
const BOTTOM_Y: f32 = 299.0;
const BOTTOM_H: f32 = 36.0;
/// The bottom bar's four labels sit at y 311 on the page, i.e. 12 inside the
/// box, and carry their own FontColor in the data: `255,255,217,83` for the
/// skill-point pair, `255,151,224,255` for the mastery pair.
const SP_LABEL_Y: f32 = 12.0;
/// Their x positions, `ifskill.txt` MainSkillWnd ids 21-24.
const BOTTOM_LABEL_X: [f32; 4] = [14.0, 86.0, 183.0, 293.0];
const SP_COLOR: Color = Color::srgb_u8(255, 217, 83);
const MASTERY_COLOR: Color = Color::srgb_u8(151, 224, 255);
const ICON_SIZE: f32 = 32.0;
/// The 20x20 branch glyph inside its socket frame (ifskill_group.txt).
const BRANCH_ICON_SIZE: f32 = 20.0;
/// The rows scrollbar gutter at the window's right edge (16px art pieces).
const SCROLL_W: f32 = 16.0;
const SCROLL_ARROW_STEP: f32 = 62.0;

const ART: &str = "media://interface/skill/";

// --- Markers ----------------------------------------------------------------

#[derive(Component)]
pub struct SkillWindowRoot;

/// The tab + board area; children rebuilt on refresh.
#[derive(Component)]
pub struct SkillBoardNode;

/// The scrollable branch-row list of the selected mastery.
#[derive(Component)]
pub struct SkillRowsNode;

/// SP number in the bottom box.
#[derive(Component)]
pub struct SkillSpText;

/// Total-mastery-level number in the bottom box.
#[derive(Component)]
pub struct SkillMasteryTotalText;

/// The floating hover tooltip (title + description).
#[derive(Component)]
pub struct SkillTooltipRoot;

#[derive(Component)]
pub struct SkillTooltipTitle;

#[derive(Component)]
pub struct SkillTooltipDesc;

#[derive(Component)]
pub struct SkillTooltipCost;

/// Container for the per-requirement lines (children rebuilt on change).
#[derive(Component)]
pub struct SkillTooltipReqs;

/// A branch's socket glyph (hover shows the series' own tooltip).
#[derive(Component)]
pub struct BranchBadge {
    pub title: String,
    pub description: String,
}

/// The rows scrollbar thumb.
#[derive(Component)]
pub struct SkillScrollThumb;

/// The rows scrollbar track.
#[derive(Component)]
pub struct SkillScrollTrack;

/// The practice (level-up confirmation) box root.
#[derive(Component)]
pub struct PracticeBoxRoot;

/// A top-level tree tab.
#[derive(Component)]
struct TopTab(u8);

/// A per-mastery tab.
#[derive(Component)]
struct MasteryTab(u32);

/// A skill cell's icon (per group). Carries everything the hover line and
/// the press handler need.
#[derive(Component)]
pub struct SkillCell {
    pub group_id: i32,
    /// Currently learned skilldata id (what a drag carries), if learned.
    pub learned_id: Option<i32>,
    pub display_name: String,
}

/// A cell's skl_button_add level-up button (opens the practice box).
#[derive(Component)]
pub struct SkillPlusButton {
    pub group_id: i32,
}

/// A mastery header's skl_mastery_levelup button (opens the practice box).
#[derive(Component)]
pub struct MasteryPlusButton {
    pub mastery: u32,
}

// --- Spawn / cleanup --------------------------------------------------------

pub fn spawn_skill_window(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    fonts: Res<FontAssets>,
    ui_strings: Res<ClientUiStrings>,
    cam_query: Query<Entity, With<Camera2d>>,
    mut state: ResMut<SkillWindowState>,
) {
    let Ok(camera) = cam_query.single() else {
        warn!("skill window: no 2d camera to attach to");
        return;
    };
    *state = SkillWindowState::default();
    let s = hud_scale();

    let window = spawn_game_window(
        &mut commands,
        &asset_server,
        &fonts,
        camera,
        ui_strings.get_or("UIIT_STT_SKILL", "Skills"),
        (CONTENT_W, CONTENT_H),
        (460.0, 60.0),
        s,
    );
    commands
        .entity(window.root)
        // Hovered so the camera-zoom system can yield the wheel to the
        // window's scroll areas
        .insert((
            SkillWindowRoot,
            GlobalZIndex(20),
            Hovered::default(),
            // Shares the MainPopup wndpos slot with the other pages of the
            // original's single frame — see `hud::main_popup`.
            crate::plugins::hud::window_positions::PersistedWindow(
                crate::plugins::settings::window_positions::WndPosSlot::MainPopup,
            ),
        ));
    commands.entity(window.expect_close_button()).observe(
        |_: On<Activate>, mut state: ResMut<SkillWindowState>| {
            state.open = false;
            state.prompt = None;
        },
    );

    let text_font = |size: f32| TextFont {
        font: fonts.two.clone().into(),
        font_size: FontSize::Px(size * s),
        ..default()
    };

    // floating tooltip (title + description), shown while hovering a skill
    // cell or a branch badge
    commands
        .spawn((
            SkillTooltipRoot,
            Name::from("Skill Tooltip"),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Px(240.0 * s),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(3.0 * s),
                padding: UiRect::all(Val::Px(6.0 * s)),
                display: Display::None,
                ..default()
            },
            BackgroundColor(Color::srgba(0.02, 0.02, 0.05, 0.92)),
            GlobalZIndex(90),
            UiTargetCamera(camera),
            Pickable::IGNORE,
        ))
        .with_children(|tip| {
            tip.spawn((
                SkillTooltipTitle,
                Text::new(""),
                text_font(9.0),
                TextColor(Color::srgb(1.0, 0.95, 0.75)),
                Pickable::IGNORE,
            ));
            tip.spawn((
                SkillTooltipDesc,
                Text::new(""),
                text_font(8.0),
                TextColor(Color::srgb(0.85, 0.85, 0.8)),
                Pickable::IGNORE,
            ));
            tip.spawn((
                SkillTooltipReqs,
                Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(2.0 * s),
                    ..default()
                },
                Pickable::IGNORE,
            ));
            tip.spawn((
                SkillTooltipCost,
                Text::new(""),
                text_font(8.0),
                TextColor(Color::srgb(1.0, 0.85, 0.33)),
                Pickable::IGNORE,
            ));
        });

    commands.entity(window.content).with_children(|content| {
        // tabs + mastery board, rebuilt by the refresh
        content.spawn((
            SkillBoardNode,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Px(CONTENT_W * s),
                height: Val::Px(BOTTOM_Y * s),
                ..default()
            },
        ));

        // bottom SP box (skl_wnd_box art with the SP / total mastery texts)
        content
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    top: Val::Px(BOTTOM_Y * s),
                    width: Val::Px(CONTENT_W * s),
                    height: Val::Px(BOTTOM_H * s),
                    ..default()
                },
                ImageNode {
                    image: asset_server.load(format!("{ART}skl_wnd_box.ddj")),
                    image_mode: NodeImageMode::Stretch,
                    ..default()
                },
                Pickable::IGNORE,
            ))
            .with_children(|bottom| {
                let label = |x: f32, y: f32, color: Color, value: &str| {
                    (
                        Text::new(value.to_string()),
                        text_font(8.0),
                        TextColor(color),
                        Node {
                            position_type: PositionType::Absolute,
                            left: Val::Px(x * s),
                            top: Val::Px(y * s),
                            ..default()
                        },
                        Pickable::IGNORE,
                    )
                };
                // ifskill.txt / MainSkillWnd ids 21-24: the label/number pairs
                // at x 14/86/183/293, y 311 (box-local 12), with the data's own
                // FontColor per pair.
                let y = SP_LABEL_Y;
                bottom.spawn(label(
                    BOTTOM_LABEL_X[0],
                    y,
                    SP_COLOR,
                    ui_strings.get_or("UIIT_STT_SKILLPOINT", "Skill point"),
                ));
                bottom.spawn((label(BOTTOM_LABEL_X[1], y, SP_COLOR, "0"), SkillSpText));
                bottom.spawn(label(
                    BOTTOM_LABEL_X[2],
                    y,
                    MASTERY_COLOR,
                    ui_strings.get_or("PARAM_MASTERY_LEVEL_TOTAL", "Mastery level total"),
                ));
                bottom.spawn((
                    label(BOTTOM_LABEL_X[3], y, MASTERY_COLOR, "0"),
                    SkillMasteryTotalText,
                ));
            });
    });
}

pub fn cleanup_skill_window(
    roots: Query<
        Entity,
        Or<(
            With<SkillWindowRoot>,
            With<PracticeBoxRoot>,
            With<SkillTooltipRoot>,
        )>,
    >,
    mut drag: ResMut<SkillDrag>,
    mut commands: Commands,
) {
    drag.skill = None;
    drag.from_underbar = false;
    for entity in roots.iter() {
        commands.entity(entity).despawn();
    }
}

/// Show/hide with the S-key state (same recipe as the inventory).
pub fn apply_skill_window_visibility(
    state: Res<SkillWindowState>,
    mut roots: Query<&mut Node, With<SkillWindowRoot>>,
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

// --- Branch derivation ------------------------------------------------------

/// The vanilla board's ordering key for a group: the required mastery level
/// of its level-1 rung (skilldata col 36). Tiers within a series and the
/// series rows themselves climb by this, matching the top-to-bottom board
/// (smash unlocks at mLv 5, chain at 7, shield at 10, …). Col 53 turned out
/// to be Consume_MP, not an order key; the native grid lives in cols 57-60
/// (see `SkillDataRow::ui_grid`).
fn group_ui_order(index: &SkillGroupIndex, data: &SkillData, group: i32) -> u32 {
    index
        .ladders
        .get(&group)
        .and_then(|ladder| ladder.first())
        .and_then(|id| data.get(id))
        .and_then(|row| row.mastery_req())
        .map(|(_, level)| level)
        .unwrap_or(u32::MAX)
}

/// The series (branch) a skill group belongs to. The two races organize
/// their boards at different granularities:
/// - **CH** by weapon *concept* (`SKILL_CH_SWORD_SMASH_A/_B/_C` → `sword_smash`):
///   drop only the trailing single-letter grade, so the graded tiers of one
///   skill share a row.
/// - **EU** by class *element/weapon* (`SKILL_EU_WIZARD_EARTHA_POINT_A`,
///   `..._AREA_A` → `wizard_eartha`): drop the per-spell concept token too, so
///   every Earth spell (Ground Charge, Earth Shock, …) shares one branch.
fn series_key(basic_group: &str) -> String {
    if let Some(body) = basic_group.strip_prefix("SKILL_EU_") {
        let body = body.to_lowercase();
        let mut parts = body.splitn(3, '_');
        if let (Some(class), Some(element)) = (parts.next(), parts.next()) {
            return format!("{class}_{element}");
        }
        return body;
    }
    let body = basic_group
        .strip_prefix("SKILL_CH_")
        .unwrap_or(basic_group)
        .to_lowercase();
    match body.rsplit_once('_') {
        Some((head, tail)) if tail.len() == 1 && tail.chars().all(|c| c.is_ascii_alphabetic()) => {
            head.to_string()
        }
        _ => body,
    }
}

/// The mastery's skill branches: one row per skill *series* (all graded tiers
/// of a skill), ordered by skillgroup.txt's Row column (the vanilla top-to-
/// bottom board order), matched via the branch icon concept; series with no
/// skillgroup match fall back to their first-unlock mastery level and sort
/// after the matched ones. Skills within a row keep mastery-level order.
fn branch_rows(
    index: &SkillGroupIndex,
    groups_data: &ClientSkillGroups,
    data: &SkillData,
    mastery: u32,
) -> Vec<Vec<i32>> {
    let Some(groups) = index.by_mastery.get(&mastery) else {
        return Vec::new();
    };
    // the raw basic_group of a group's root, for skillgroup matching
    let root_basic = |group: i32| -> Option<String> {
        let root_id = index.ladders.get(&group)?.first()?;
        data.get(root_id)?.basic_group().map(str::to_string)
    };
    let series_of = |group: i32| root_basic(group).map(|bg| series_key(&bg));
    let mut series: std::collections::HashMap<String, Vec<i32>> = Default::default();
    let mut order: Vec<String> = Vec::new();
    for &group in groups {
        let Some(key) = series_of(group) else {
            continue;
        };
        let entry = series.entry(key.clone()).or_insert_with(|| {
            order.push(key);
            Vec::new()
        });
        entry.push(group);
    }
    // Sort key per series: its skillgroup Row when matched, else after all
    // rows (u32::MAX) sub-ordered by mastery level so unmatched stay stable.
    let sort_key = |row: &[i32]| -> (u32, u32) {
        let level = row
            .first()
            .map(|&group| group_ui_order(index, data, group))
            .unwrap_or(u32::MAX);
        let branch_row = row
            .first()
            .and_then(|&group| root_basic(group))
            .and_then(|bg| groups_data.row_for(mastery, &bg))
            .unwrap_or(u32::MAX);
        (branch_row, level)
    };
    let mut rows: Vec<Vec<i32>> = order
        .into_iter()
        .map(|key| {
            let mut row = series.remove(&key).unwrap_or_default();
            row.sort_by_key(|&group| group_ui_order(index, data, group));
            row
        })
        .collect();
    rows.sort_by_key(|row| sort_key(row));
    // the bar art has 8 sockets; longer series continue on follow-up rows
    rows.into_iter()
        .flat_map(|row| {
            row.chunks(ROW_SOCKETS)
                .map(<[i32]>::to_vec)
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The native-grid board (skilldata cols 57-60): one bar per authored
/// UI_SkillColumn lane, each group socketed at its UI_SkillRow slot with the
/// gaps preserved (`None` sockets) — the exact vanilla placement. Tab/page
/// (cols 57/58) address the mastery's board inside the whole window and are
/// constant per mastery, so only column/row place cells here. Returns `None`
/// when no group of the mastery carries a grid (the caller falls back to the
/// derived layout); grid-less or slot-colliding groups append as plain rows
/// below the grid.
fn native_grid_rows(
    index: &SkillGroupIndex,
    data: &SkillData,
    mastery: u32,
) -> Option<Vec<Vec<Option<i32>>>> {
    let groups = index.by_mastery.get(&mastery)?;
    // a group's grid slot: the first rung that carries one (roots usually do)
    let grid_of = |group: i32| -> Option<(u8, u8, u8, u8)> {
        index
            .ladders
            .get(&group)?
            .iter()
            .find_map(|id| data.get(id).and_then(|row| row.ui_grid()))
    };
    let mut lanes: std::collections::BTreeMap<u8, Vec<Option<i32>>> = Default::default();
    let mut leftovers: Vec<i32> = Vec::new();
    for &group in groups {
        let slot = grid_of(group).map(|(_, _, lane, slot)| (lane, slot as usize));
        match slot {
            Some((lane, slot)) if slot < ROW_SOCKETS => {
                let lane = lanes.entry(lane).or_default();
                if lane.len() <= slot {
                    lane.resize(slot + 1, None);
                }
                if lane[slot].is_none() {
                    lane[slot] = Some(group);
                } else {
                    leftovers.push(group);
                }
            }
            _ => leftovers.push(group),
        }
    }
    if lanes.is_empty() {
        return None;
    }
    let mut rows: Vec<Vec<Option<i32>>> = lanes.into_values().collect();
    rows.extend(
        leftovers
            .chunks(ROW_SOCKETS)
            .map(|chunk| chunk.iter().copied().map(Some).collect()),
    );
    Some(rows)
}

/// The branch (skill-series) icons of `icon/SkillGroup/` — corpus-derived
/// list of the `pack_*` stems that exist in Media.pk2 (each also has a
/// `_focus` variant, unused here).
const GROUP_ICON_PACKS: &[&str] = &[
    "china/pack_bow_area",
    "china/pack_bow_call",
    "china/pack_bow_chain",
    "china/pack_bow_critical",
    "china/pack_bow_divide_a",
    "china/pack_bow_flow",
    "china/pack_bow_normal",
    "china/pack_bow_passive",
    "china/pack_bow_pierce",
    "china/pack_bow_power",
    "china/pack_bow_sky",
    "china/pack_cold_bingbyeok",
    "china/pack_cold_bingpan",
    "china/pack_cold_ganggi",
    "china/pack_cold_gigongjang",
    "china/pack_cold_gigongsul",
    "china/pack_cold_gigongta",
    "china/pack_cold_passive",
    "china/pack_cold_subsul",
    "china/pack_fire_balhwa",
    "china/pack_fire_ganggi",
    "china/pack_fire_gigongsul",
    "china/pack_fire_gigongta",
    "china/pack_fire_gongup",
    "china/pack_fire_hwabyeok",
    "china/pack_fire_passive",
    "china/pack_fire_shield",
    "china/pack_lightning_bobeop",
    "china/pack_lightning_chundung",
    "china/pack_lightning_gigongta",
    "china/pack_lightning_gwantong",
    "china/pack_lightning_gyeonggong",
    "china/pack_lightning_jipjung",
    "china/pack_lightning_passive",
    "china/pack_lightning_storm",
    "china/pack_spear_chain",
    "china/pack_spear_counter",
    "china/pack_spear_divide_a",
    "china/pack_spear_frontarea",
    "china/pack_spear_passive",
    "china/pack_spear_pierce",
    "china/pack_spear_roundarea",
    "china/pack_spear_shoot",
    "china/pack_spear_spin",
    "china/pack_spear_stun",
    "china/pack_sword_chain",
    "china/pack_sword_divide_a",
    "china/pack_sword_downattack",
    "china/pack_sword_geomgi",
    "china/pack_sword_knockdown",
    "china/pack_sword_passive",
    "china/pack_sword_shield",
    "china/pack_sword_shieldpd",
    "china/pack_sword_smash",
    "china/pack_sword_special",
    "china/pack_water_allcure",
    "china/pack_water_bless",
    "china/pack_water_cancel",
    "china/pack_water_cancel_b",
    "china/pack_water_cure",
    "china/pack_water_harmony",
    "china/pack_water_heal",
    "china/pack_water_passive",
    "china/pack_water_rebirth_group",
    "china/pack_water_recovery_quick",
    "china/pack_water_resurrection",
    "china/pack_water_selfheal",
    "europe/pack_attack",
    "europe/pack_battle",
    "europe/pack_battle_b",
    "europe/pack_beautiful",
    "europe/pack_bless",
    "europe/pack_bless_b",
    "europe/pack_blood",
    "europe/pack_cold",
    "europe/pack_cold_b",
    "europe/pack_common",
    "europe/pack_common_b",
    "europe/pack_cross",
    "europe/pack_cross_expert",
    "europe/pack_crossbow",
    "europe/pack_crossbow_b",
    "europe/pack_cruel",
    "europe/pack_dagger",
    "europe/pack_dagger_b",
    "europe/pack_dance",
    "europe/pack_dark",
    "europe/pack_divine",
    "europe/pack_dream",
    "europe/pack_dual",
    "europe/pack_dual_b",
    "europe/pack_dual_lord",
    "europe/pack_earth",
    "europe/pack_earth_b",
    "europe/pack_energe",
    "europe/pack_fire",
    "europe/pack_fire_b",
    "europe/pack_force",
    "europe/pack_frenzy",
    "europe/pack_frenzy_b",
    "europe/pack_glory",
    "europe/pack_guard",
    "europe/pack_holy",
    "europe/pack_holy_b",
    "europe/pack_light",
    "europe/pack_light_b",
    "europe/pack_mask",
    "europe/pack_melody",
    "europe/pack_melody_b",
    "europe/pack_mental",
    "europe/pack_mental_b",
    "europe/pack_mind",
    "europe/pack_music",
    "europe/pack_natural",
    "europe/pack_onehand",
    "europe/pack_onehand_b",
    "europe/pack_onehand_lord",
    "europe/pack_over",
    "europe/pack_poison",
    "europe/pack_poison_b",
    "europe/pack_raze",
    "europe/pack_raze_b",
    "europe/pack_recover",
    "europe/pack_resuscitation",
    "europe/pack_saint",
    "europe/pack_silent",
    "europe/pack_silent_b",
    "europe/pack_soul",
    "europe/pack_soul_b",
    "europe/pack_sound",
    "europe/pack_sound_b",
    "europe/pack_stealth",
    "europe/pack_stealth_expert",
    "europe/pack_steela",
    "europe/pack_steela_b",
    "europe/pack_steelp",
    "europe/pack_twohand",
    "europe/pack_twohand_b",
    "europe/pack_twohand_lord",
];

/// The branch socket icon for a branch root: skillgroup.txt's authoritative
/// icon (col 6) when the branch matched, else a best-effort derivation from
/// the `basic_group` codename against the known `pack_*` glyphs, else the root
/// skill's own icon (the caller dims that last fallback).
/// Codename derivation: CH `SKILL_CH_SWORD_SMASH_A` → `china/pack_sword_smash`
/// (grade dropped, or kept when only the graded file exists, e.g.
/// `pack_bow_divide_a`); EU `SKILL_EU_WARRIOR_DUALA_CROSS_A` → `europe/pack_dual`
/// (series token with its active/passive suffix stripped).
fn branch_icon(
    skillgroup_icon: Option<String>,
    basic_group: &str,
    root_icon: &str,
) -> (String, bool) {
    if let Some(icon) = skillgroup_icon {
        return (icon, true);
    }
    let mut candidates: Vec<String> = Vec::new();
    if let Some(body) = basic_group.strip_prefix("SKILL_CH_") {
        let body = body.to_lowercase();
        candidates.push(format!("china/pack_{body}"));
        if let Some((stem, _grade)) = body.rsplit_once('_') {
            candidates.push(format!("china/pack_{stem}"));
        }
    } else if let Some(body) = basic_group.strip_prefix("SKILL_EU_") {
        let mut parts = body.splitn(3, '_');
        let _class = parts.next();
        if let Some(series) = parts.next() {
            let series = series.to_lowercase();
            let trimmed = series.trim_end_matches(['a', 'p']);
            candidates.push(format!("europe/pack_{trimmed}"));
            candidates.push(format!("europe/pack_{series}"));
        }
    }
    for candidate in candidates {
        if GROUP_ICON_PACKS.contains(&candidate.as_str()) {
            return (format!("media://icon/skillgroup/{candidate}.ddj"), true);
        }
    }
    (root_icon.to_string(), false)
}

/// A branch's own tooltip (title + description). CH masteries have per-series
/// strings (`UIIT_STT_MASTERY_GROUP_<abbr>_<n>`, one string with the title on
/// the first line); EU has none, so fall back to the branch root skill's name
/// and `TT_DESC`.
fn branch_tooltip(
    root_row: &crate::assets::textdata::skilldata::SkillDataRow,
    mastery_id: u32,
    branch_index: usize,
    names: &ClientTextNames,
    ui_strings: &ClientUiStrings,
) -> (String, String) {
    if let Some(abbr) = mastery_group_abbrev(mastery_id) {
        let key = format!("UIIT_STT_MASTERY_GROUP_{abbr}_{branch_index}");
        if let Some(text) = ui_strings.get(&key) {
            let text = text.replace("\\n", "\n");
            let (title, desc) = text.split_once('\n').unwrap_or((text.as_str(), ""));
            return (title.trim().to_string(), desc.trim().to_string());
        }
    }
    let title = root_row
        .name_key()
        .and_then(|key| names.name(key))
        .unwrap_or(root_row.code_name())
        .to_string();
    let description = root_row
        .tooltip_key()
        .and_then(|key| names.name(key))
        .unwrap_or("")
        .to_string();
    (title, description)
}

// --- Refresh ----------------------------------------------------------------

/// Rebuild the tab rows + the selected mastery board, repaint SP numbers.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]
/// The window's two header text queries, bundled to keep
/// [`refresh_skill_window`] under Bevy's 16-system-param limit.
#[derive(bevy::ecs::system::SystemParam)]
pub struct SkillHeaderTexts<'w, 's> {
    sp: Query<'w, 's, &'static mut Text, (With<SkillSpText>, Without<SkillMasteryTotalText>)>,
    total: Query<'w, 's, &'static mut Text, (With<SkillMasteryTotalText>, Without<SkillSpText>)>,
}

pub fn refresh_skill_window(
    boards: Query<Entity, With<SkillBoardNode>>,
    mut state: ResMut<SkillWindowState>,
    race: Res<SkillTreeRace>,
    masteries: Res<ClientMasteryData>,
    skill_groups: Res<ClientSkillGroups>,
    skill_data: Res<ClientSkillData>,
    (names, ui_strings): (Res<ClientTextNames>, Res<ClientUiStrings>),
    index: Res<SkillGroupIndex>,
    book: Res<SkillBook>,
    progress: Res<PlayerProgress>,
    fonts: Res<FontAssets>,
    asset_server: Res<AssetServer>,
    config: Res<crate::plugins::config::ClientConfig>,
    rows_scroll: Query<&ScrollPosition, With<SkillRowsNode>>,
    mut header_texts: SkillHeaderTexts,
    mut commands: Commands,
) {
    let Ok(board) = boards.single() else {
        return;
    };
    for mut text in header_texts.sp.iter_mut() {
        text.0 = progress.skill_points.to_string();
    }
    let total: u32 = book
        .masteries
        .iter()
        .filter(|(id, _)| race.owns_mastery(**id))
        .map(|(_, level)| level)
        .sum();
    for mut text in header_texts.total.iter_mut() {
        text.0 = total.to_string();
    }

    // remember the live scroll so a learn/level-up rebuild keeps the view in
    // place (tab/mastery presses zero it in their handlers)
    if let Ok(scroll) = rows_scroll.single() {
        state.bypass_change_detection().scroll_y = scroll.0.y;
    }
    commands.entity(board).despawn_related::<Children>();
    let Some(data) = skill_data.data() else {
        return;
    };
    if !index.is_built() {
        return;
    }
    let s = hud_scale();
    let race = *race;

    // the race's masteries under the selected top tab (mastery col 6)
    let top_tabs = race.top_tabs();
    let top_tab = (state.top_tab as usize).min(top_tabs.len() - 1) as u8;
    let mut tab_masteries: Vec<_> = masteries
        .iter()
        .filter(|m| race.owns_mastery(m.id) && m.tab == top_tab)
        .collect();
    tab_masteries.sort_by_key(|m| m.id);
    let selected_mastery = state
        .mastery
        .filter(|id| tab_masteries.iter().any(|m| m.id == *id))
        .or_else(|| tab_masteries.first().map(|m| m.id));
    // normalize without re-triggering change detection every frame
    let state = state.bypass_change_detection();
    state.top_tab = top_tab;
    state.mastery = selected_mastery;

    let text_font = |size: f32| TextFont {
        font: fonts.two.clone().into(),
        font_size: FontSize::Px(size * s),
        ..default()
    };

    commands.entity(board).with_children(|board| {
        // top tree tabs use the generic com_tab art, mastery tabs the plain
        // skl_mastery_tab art, both with a centered text label
        let top_tab_art = |on: bool| {
            let variant = if on { "on" } else { "off" };
            format!("media://interface/ifcommon/com_tab_{variant}.ddj")
        };
        let tab_art = |on: bool| {
            let variant = if on { "on" } else { "off" };
            format!("{ART}skl_mastery_tab_{variant}.ddj")
        };
        // (left, width, top) place the label clear of any baked-in icon
        let tab_label =
            |tab: &mut ChildSpawnerCommands, label: &str, on: bool, region: (f32, f32, f32)| {
                let (x, w, y) = region;
                tab.spawn((
                    Text::new(label.to_string()),
                    text_font(8.0),
                    TextColor(if on {
                        Color::srgb(1.0, 0.95, 0.75)
                    } else {
                        Color::srgb(0.7, 0.7, 0.65)
                    }),
                    TextLayout::justify(Justify::Center),
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(x * s),
                        top: Val::Px(y * s),
                        width: Val::Px(w * s),
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
            };

        // the board frame ring, spawned first so the sub-mastery tabs and
        // board content draw over it. Mids/sides overlap their neighbors
        // 1 unit against fractional-position AA seams (see game_window.rs).
        let fp = BOARD_FRAME_PIECE;
        let fw = BOARD_W;
        let fh = BOARD_FRAME_H;
        let frame_pieces = [
            ((0.0, 0.0, fp, fp), "left_up"),
            ((fp - 1.0, 0.0, fw - 2.0 * fp + 2.0, fp), "mid_up"),
            ((fw - fp, 0.0, fp, fp), "right_up"),
            ((0.0, fp - 1.0, fp, fh - 2.0 * fp + 2.0), "left_side"),
            ((fw - fp, fp - 1.0, fp, fh - 2.0 * fp + 2.0), "right_side"),
            ((0.0, fh - fp, fp, fp), "left_down"),
            ((fp - 1.0, fh - fp, fw - 2.0 * fp + 2.0, fp), "mid_down"),
            ((fw - fp, fh - fp, fp, fp), "right_down"),
        ];
        for ((x, y, w, h), piece) in frame_pieces {
            board.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px((BOARD_X + x) * s),
                    top: Val::Px((BOARD_FRAME_Y + y) * s),
                    width: Val::Px(w * s),
                    height: Val::Px(h * s),
                    ..default()
                },
                ImageNode {
                    image: asset_server.load(format!(
                        "media://interface/inventory/int_window_{piece}.ddj"
                    )),
                    image_mode: NodeImageMode::Stretch,
                    ..default()
                },
                Pickable::IGNORE,
            ));
        }

        // top-level tree tabs
        for (idx, label) in top_tabs.iter().enumerate() {
            let on = idx as u8 == top_tab;
            board
                .spawn((
                    TopTab(idx as u8),
                    Hovered::default(),
                    // above the branch rows: scrolled-out cells are clipped
                    // visually but still hit-test, so tabs must win picking
                    ZIndex(2),
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px((4.0 + idx as f32 * TOP_TAB_W) * s),
                        top: Val::Px(0.0),
                        width: Val::Px(TOP_TAB_W * s),
                        height: Val::Px(TOP_TAB_H * s),
                        ..default()
                    },
                    ImageNode {
                        image: asset_server.load(top_tab_art(on)),
                        image_mode: NodeImageMode::Stretch,
                        ..default()
                    },
                ))
                .observe(on_top_tab_press)
                .with_children(|tab| tab_label(tab, label, on, (0.0, TOP_TAB_W, 7.0)));
        }

        // per-mastery tabs
        for (idx, mastery) in tab_masteries.iter().enumerate() {
            let on = Some(mastery.id) == selected_mastery;
            board
                .spawn((
                    MasteryTab(mastery.id),
                    Hovered::default(),
                    ZIndex(2),
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px((4.0 + idx as f32 * (TAB_W + 2.0)) * s),
                        top: Val::Px(MASTERY_TABS_Y * s),
                        width: Val::Px(TAB_W * s),
                        height: Val::Px(TAB_H * s),
                        ..default()
                    },
                    ImageNode {
                        image: asset_server.load(tab_art(on)),
                        image_mode: NodeImageMode::Stretch,
                        ..default()
                    },
                ))
                .observe(on_mastery_tab_press)
                .with_children(|tab| {
                    tab_label(
                        tab,
                        mastery_name(mastery.id, &masteries, &ui_strings),
                        on,
                        (0.0, TAB_W, 9.0),
                    )
                });
        }

        let Some(mastery_id) = selected_mastery else {
            return;
        };
        let mastery = tab_masteries.iter().find(|m| m.id == mastery_id).copied();
        let level = book.mastery_level(mastery_id);

        // mastery header band (raised above the rows for picking, like the
        // tabs)
        board
            .spawn((
                ZIndex(1),
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(BOARD_X * s),
                    top: Val::Px(HEADER_Y * s),
                    width: Val::Px(HEADER_W * s),
                    height: Val::Px(HEADER_H * s),
                    ..default()
                },
                ImageNode {
                    image: asset_server.load(format!("{ART}skl_mastery_subject.ddj")),
                    image_mode: NodeImageMode::Stretch,
                    ..default()
                },
            ))
            .with_children(|header| {
                if let Some(icon) = mastery.and_then(|m| m.icon.clone()) {
                    header.spawn((
                        Node {
                            position_type: PositionType::Absolute,
                            left: Val::Px(7.0 * s),
                            top: Val::Px(7.0 * s),
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
                }
                header.spawn((
                    Text::new(format!(
                        "{} Mastery",
                        mastery_name(mastery_id, &masteries, &ui_strings)
                    )),
                    text_font(9.0),
                    TextColor(Color::srgb(0.92, 0.88, 0.72)),
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(48.0 * s),
                        top: Val::Px(15.0 * s),
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
                // mastery level-up button (offline rule: capped by character
                // level; the practice box confirms)
                let can_raise = level < progress.level as u32;
                let style = ImageButtonStyle {
                    normal: asset_server.load(format!("{ART}skl_mastery_levelup.ddj")),
                    hover: asset_server.load(format!("{ART}skl_mastery_levelup_focus.ddj")),
                    press: asset_server.load(format!("{ART}skl_mastery_levelup_press.ddj")),
                    ..Default::default()
                };
                header
                    .spawn((
                        MasteryPlusButton {
                            mastery: mastery_id,
                        },
                        Button,
                        Hovered::default(),
                        Node {
                            position_type: PositionType::Absolute,
                            left: Val::Px(219.0 * s),
                            top: Val::Px(12.0 * s),
                            width: Val::Px(64.0 * s),
                            height: Val::Px(20.0 * s),
                            ..default()
                        },
                        ImageNode {
                            image: style.normal.clone(),
                            image_mode: NodeImageMode::Stretch,
                            color: if can_raise {
                                Color::WHITE
                            } else {
                                Color::srgb(0.45, 0.45, 0.45)
                            },
                            ..default()
                        },
                        style,
                    ))
                    .observe(on_mastery_plus);
                // the "Lv" caption is its own localized static in vanilla
                // (GDR_SKILLBOARD_STATIC_LV, UIIT_STT_LEVEL_LV) — only the
                // number lives in the level box next to it.
                let level_cell =
                    |parent: &mut ChildSpawnerCommands, x: f32, w: f32, value: String| {
                        parent
                            .spawn((
                                Node {
                                    position_type: PositionType::Absolute,
                                    left: Val::Px(x * s),
                                    top: Val::Px(LEVEL_BOX.1 * s),
                                    width: Val::Px(w * s),
                                    height: Val::Px(LEVEL_BOX.3 * s),
                                    justify_content: JustifyContent::Center,
                                    align_items: AlignItems::Center,
                                    ..default()
                                },
                                Pickable::IGNORE,
                            ))
                            .with_children(|cell| {
                                cell.spawn((
                                    Text::new(value),
                                    text_font(9.0),
                                    TextColor(Color::srgb(0.92, 0.88, 0.72)),
                                    Pickable::IGNORE,
                                ));
                            });
                    };
                level_cell(
                    header,
                    LV_LABEL_BOX.0,
                    LV_LABEL_BOX.2,
                    ui_strings.get_or("UIIT_STT_LEVEL_LV", "Lv").to_string(),
                );
                level_cell(header, LEVEL_BOX.0, LEVEL_BOX.2, level.to_string());
            });

        // scrollable branch rows (the scrollbar takes the right gutter)
        board
            .spawn((
                SkillRowsNode,
                Hovered::default(),
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px((BOARD_X + ROWS_X) * s),
                    top: Val::Px(ROWS_Y * s),
                    width: Val::Px(ROW_W * s),
                    height: Val::Px(ROWS_H * s),
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(ROW_GAP * s),
                    overflow: Overflow::scroll_y(),
                    ..default()
                },
                ScrollPosition(Vec2::new(0.0, state.scroll_y)),
            ))
            .observe(
                |mut scroll: On<Pointer<Scroll>>,
                 mut lists: Query<(&mut ScrollPosition, &ComputedNode), With<SkillRowsNode>>| {
                    let Ok((mut pos, computed)) = lists.single_mut() else {
                        return;
                    };
                    let max = ((computed.content_size().y - computed.size().y)
                        * computed.inverse_scale_factor())
                    .max(0.0);
                    pos.y = (pos.y - scroll.event.y * 30.0).clamp(0.0, max);
                    scroll.propagate(false);
                },
            )
            .with_children(|rows| {
                // native skilldata grid (cols 57-60) when configured and
                // authored; the derived skillgroup.txt layout otherwise
                let board_rows: Vec<Vec<Option<i32>>> = config
                    .hud
                    .skill_window_native_grid
                    .then(|| native_grid_rows(&index, data, mastery_id))
                    .flatten()
                    .unwrap_or_else(|| {
                        branch_rows(&index, &skill_groups, data, mastery_id)
                            .into_iter()
                            .map(|row| row.into_iter().map(Some).collect())
                            .collect()
                    });
                for (branch_index, row) in board_rows.iter().enumerate() {
                    spawn_branch_row(
                        rows,
                        row,
                        branch_index,
                        mastery_id,
                        data,
                        &skill_groups,
                        &names,
                        &ui_strings,
                        &index,
                        &book,
                        &progress,
                        &asset_server,
                        &text_font,
                    );
                }
            });

        // the rows scrollbar: up/down arrows + com_scroll track and thumb
        // (thumb position mirrors the wheel scroll; arrows step it)
        let scroll_by = |delta: f32| {
            move |_: On<Activate>,
                  mut lists: Query<(&mut ScrollPosition, &ComputedNode), With<SkillRowsNode>>| {
                let Ok((mut pos, computed)) = lists.single_mut() else {
                    return;
                };
                let max = ((computed.content_size().y - computed.size().y)
                    * computed.inverse_scale_factor())
                .max(0.0);
                pos.y = (pos.y + delta).clamp(0.0, max);
            }
        };
        board
            .spawn((
                ZIndex(1),
                Node {
                    position_type: PositionType::Absolute,
                    // packed flush against the rows, like vanilla's scroll
                    // region — no bare strip between rows and gutter
                    left: Val::Px((BOARD_X + ROWS_X + ROW_W) * s),
                    top: Val::Px(ROWS_Y * s),
                    width: Val::Px(SCROLL_W * s),
                    height: Val::Px(ROWS_H * s),
                    flex_direction: FlexDirection::Column,
                    ..default()
                },
            ))
            .with_children(|gutter| {
                let arrow = |gutter: &mut ChildSpawnerCommands, stem: &str| -> Entity {
                    let style = ImageButtonStyle {
                        normal: asset_server
                            .load(format!("media://interface/chattingwnd/{stem}.ddj")),
                        hover: asset_server
                            .load(format!("media://interface/chattingwnd/{stem}_focus.ddj")),
                        press: asset_server
                            .load(format!("media://interface/chattingwnd/{stem}_press.ddj")),
                        ..Default::default()
                    };
                    gutter
                        .spawn((
                            Button,
                            Hovered::default(),
                            Node {
                                width: Val::Px(SCROLL_W * s),
                                height: Val::Px(SCROLL_W * s),
                                flex_shrink: 0.0,
                                ..default()
                            },
                            ImageNode {
                                image: style.normal.clone(),
                                image_mode: NodeImageMode::Stretch,
                                ..default()
                            },
                            style,
                        ))
                        .id()
                };
                let up = arrow(gutter, "chat_arrow_up");
                gutter
                    .spawn((
                        SkillScrollTrack,
                        Node {
                            width: Val::Px(SCROLL_W * s),
                            flex_grow: 1.0,
                            ..default()
                        },
                        ImageNode {
                            image: asset_server
                                .load("media://interface/ifcommon/com_scroll_bar.ddj"),
                            image_mode: NodeImageMode::Stretch,
                            ..default()
                        },
                    ))
                    .with_children(|track| {
                        track.spawn((
                            SkillScrollThumb,
                            Node {
                                position_type: PositionType::Absolute,
                                left: Val::Px(0.0),
                                top: Val::Px(0.0),
                                width: Val::Px(SCROLL_W * s),
                                height: Val::Px(SCROLL_W * s),
                                ..default()
                            },
                            ImageNode {
                                image: asset_server
                                    .load("media://interface/ifcommon/com_scroll_button.ddj"),
                                image_mode: NodeImageMode::Stretch,
                                ..default()
                            },
                            Pickable::IGNORE,
                        ));
                    });
                let down = arrow(gutter, "chat_arrow_down");
                gutter
                    .commands()
                    .entity(up)
                    .observe(scroll_by(-SCROLL_ARROW_STEP));
                gutter
                    .commands()
                    .entity(down)
                    .observe(scroll_by(SCROLL_ARROW_STEP));
            });
    });
}

/// Mirror the rows list's scroll fraction onto the scrollbar thumb.
pub fn update_skill_scroll_thumb(
    lists: Query<&ComputedNode, With<SkillRowsNode>>,
    tracks: Query<&ComputedNode, With<SkillScrollTrack>>,
    mut thumbs: Query<&mut Node, With<SkillScrollThumb>>,
) {
    let (Ok(list), Ok(track), Ok(mut thumb)) =
        (lists.single(), tracks.single(), thumbs.single_mut())
    else {
        return;
    };
    let max_scroll = (list.content_size().y - list.size().y).max(0.0);
    let fraction = if max_scroll > 0.0 {
        (list.scroll_position.y / max_scroll).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let travel =
        ((track.size().y - SCROLL_W * hud_scale()) * track.inverse_scale_factor()).max(0.0);
    thumb.top = Val::Px(travel * fraction);
}

/// One skl_mastery_bar row: the branch's root skill icon in the left socket,
/// then each group of the chain in the 8 skill sockets.
#[allow(clippy::too_many_arguments)]
fn spawn_branch_row(
    rows: &mut ChildSpawnerCommands,
    row: &[Option<i32>],
    branch_index: usize,
    mastery_id: u32,
    data: &SkillData,
    skill_groups: &ClientSkillGroups,
    names: &ClientTextNames,
    ui_strings: &ClientUiStrings,
    index: &SkillGroupIndex,
    book: &SkillBook,
    progress: &PlayerProgress,
    asset_server: &AssetServer,
    text_font: &dyn Fn(f32) -> TextFont,
) {
    let s = hud_scale();
    rows.spawn((
        Node {
            width: Val::Px(ROW_W * s),
            height: Val::Px(ROW_H * s),
            flex_shrink: 0.0,
            ..default()
        },
        Pickable::IGNORE,
    ))
    .with_children(|row_node| {
        row_node
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(ROW_X * s),
                    top: Val::Px(0.0),
                    width: Val::Px(ROW_W * s),
                    height: Val::Px(ROW_H * s),
                    ..default()
                },
                ImageNode {
                    image: asset_server.load(format!("{ART}skl_mastery_bar.ddj")),
                    image_mode: NodeImageMode::Stretch,
                    ..default()
                },
                Pickable::IGNORE,
            ))
            .with_children(|bar| {
                // branch socket: the dedicated SkillGroup pack glyph when
                // the archive has one, else the root skill's icon dimmed
                // (first FILLED socket — native-grid lanes can lead with gaps)
                let root_row = row
                    .iter()
                    .flatten()
                    .next()
                    .and_then(|group| index.ladders.get(group))
                    .and_then(|ladder| ladder.first())
                    .and_then(|id| data.get(id));
                if let Some((root_row, root_icon)) =
                    root_row.and_then(|r| r.icon_path().map(|icon| (r, icon)))
                {
                    let basic_group = root_row.basic_group().unwrap_or("");
                    let (icon, dedicated) = branch_icon(
                        skill_groups.icon_for(mastery_id, basic_group),
                        basic_group,
                        &root_icon,
                    );
                    let (title, description) =
                        branch_tooltip(root_row, mastery_id, branch_index, names, ui_strings);
                    // socket frame (skl_mastery_nothing) with the 20x20
                    // branch glyph centered on it (GDR_SKILLGROUP_ICON size),
                    // aligned on the bar art's baked branch-socket frame
                    bar.spawn((
                        BranchBadge { title, description },
                        Hovered::default(),
                        Node {
                            position_type: PositionType::Absolute,
                            left: Val::Px(2.0 * s),
                            top: Val::Px(SOCKET_Y * s),
                            width: Val::Px(ICON_SIZE * s),
                            height: Val::Px(ICON_SIZE * s),
                            ..default()
                        },
                        ImageNode {
                            image: asset_server.load(format!("{ART}skl_mastery_nothing.ddj")),
                            image_mode: NodeImageMode::Stretch,
                            ..default()
                        },
                    ))
                    .with_children(|socket| {
                        socket.spawn((
                            Node {
                                position_type: PositionType::Absolute,
                                left: Val::Px(6.0 * s),
                                top: Val::Px(6.0 * s),
                                width: Val::Px(BRANCH_ICON_SIZE * s),
                                height: Val::Px(BRANCH_ICON_SIZE * s),
                                ..default()
                            },
                            ImageNode {
                                image: asset_server.load(icon),
                                image_mode: NodeImageMode::Stretch,
                                color: if dedicated {
                                    Color::WHITE
                                } else {
                                    Color::srgb(0.55, 0.5, 0.35)
                                },
                                ..default()
                            },
                            Pickable::IGNORE,
                        ));
                    });
                }
                // filled sockets get cells, gaps and the tail the embossed
                // empty-slot filler
                for socket in 0..ROW_SOCKETS {
                    match row.get(socket).copied().flatten() {
                        Some(group_id) => {
                            spawn_skill_cell(
                                bar,
                                socket,
                                group_id,
                                data,
                                names,
                                index,
                                book,
                                progress,
                                asset_server,
                                text_font,
                            );
                        }
                        None => {
                            bar.spawn((
                                Node {
                                    position_type: PositionType::Absolute,
                                    left: Val::Px((SOCKET0_X + socket as f32 * SOCKET_PITCH) * s),
                                    top: Val::Px(SOCKET_Y * s),
                                    width: Val::Px(ICON_SIZE * s),
                                    height: Val::Px(ICON_SIZE * s),
                                    ..default()
                                },
                                ImageNode {
                                    image: asset_server
                                        .load(format!("{ART}skl_mastery_nothing.ddj")),
                                    image_mode: NodeImageMode::Stretch,
                                    ..default()
                                },
                                Pickable::IGNORE,
                            ));
                        }
                    }
                }
            });
    });
}

#[allow(clippy::too_many_arguments)]
fn spawn_skill_cell(
    bar: &mut ChildSpawnerCommands,
    socket: usize,
    group_id: i32,
    data: &SkillData,
    names: &ClientTextNames,
    index: &SkillGroupIndex,
    book: &SkillBook,
    progress: &PlayerProgress,
    asset_server: &AssetServer,
    text_font: &dyn Fn(f32) -> TextFont,
) -> Option<()> {
    let s = hud_scale();
    let x = SOCKET0_X + socket as f32 * SOCKET_PITCH;
    let ladder = index.ladders.get(&group_id)?;
    let learned = book.learned.get(&group_id);
    let shown_id = learned.map(|l| l.skill_id).or(ladder.first().copied())?;
    let row = data.get(&shown_id)?;
    let display_name = row
        .name_key()
        .and_then(|key| names.name(key))
        .unwrap_or(row.code_name())
        .to_string();
    let level = learned.map(|l| l.level).unwrap_or(0);
    let next = index
        .next_level_id(book, group_id)
        .and_then(|id| data.get(&id));
    let block = next
        .as_ref()
        .map(|next| book::can_learn(next, book, progress.skill_points));
    // only an unmet mastery level hides the icon behind the closed-book art
    let locked = matches!(block, Some(Err(LearnBlock::MasteryTooLow { .. }))) && level == 0;
    // any unmet condition (mastery, prereq skill, SP) hides the + button;
    // the icon itself stays visible once the mastery level is met
    let gated = matches!(block, Some(Err(_)));

    // vanilla: requirement-locked skills hide behind the closed-book art
    let (icon_image, icon_color) = if locked {
        (
            Some(asset_server.load(format!("{ART}skl_mastery_disable.ddj"))),
            Color::WHITE,
        )
    } else {
        (
            row.icon_path().map(|path| asset_server.load(path)),
            if level > 0 {
                Color::WHITE
            } else {
                Color::srgb(0.5, 0.5, 0.5)
            },
        )
    };
    bar.spawn((
        SkillCell {
            group_id,
            learned_id: learned.map(|l| l.skill_id),
            display_name,
        },
        Hovered::default(),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(x * s),
            top: Val::Px(SOCKET_Y * s),
            width: Val::Px(ICON_SIZE * s),
            height: Val::Px(ICON_SIZE * s),
            ..default()
        },
        ImageNode {
            image: icon_image.unwrap_or_default(),
            image_mode: NodeImageMode::Stretch,
            color: icon_color,
            ..default()
        },
    ))
    .observe(on_skill_cell_press)
    .with_children(|icon| {
        if level > 0 {
            icon.spawn((
                Text::new(level.to_string()),
                text_font(7.0),
                TextColor(Color::srgb(1.0, 0.9, 0.3)),
                Node {
                    position_type: PositionType::Absolute,
                    right: Val::Px(2.0 * s),
                    bottom: Val::Px(1.0 * s),
                    ..default()
                },
                Pickable::IGNORE,
            ));
            // cooldown countdown, centered over the icon (hud/cooldown.rs)
            icon.spawn((
                crate::plugins::hud::cooldown::CooldownOverlay,
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    top: Val::Px(0.0),
                    width: Val::Px(ICON_SIZE * s),
                    height: Val::Px(ICON_SIZE * s),
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    ..default()
                },
                // the original's clock sweep; the frame is picked per-frame in
                // hud/cooldown.rs
                crate::plugins::hud::cooldown::wedge_image(asset_server),
                Visibility::Hidden,
                Pickable::IGNORE,
            ))
            .with_children(|overlay| {
                overlay.spawn((
                    crate::plugins::hud::cooldown::CooldownText,
                    Text::new(""),
                    text_font(9.0),
                    TextColor(Color::WHITE),
                    Pickable::IGNORE,
                ));
            });
        }
    });

    // the strip under the socket: + when the next rung is learnable right
    // now, MAX when topped out, nothing while any requirement is unmet
    if next.is_some() && !gated {
        let style = ImageButtonStyle {
            normal: asset_server.load(format!("{ART}skl_button_add.ddj")),
            hover: asset_server.load(format!("{ART}skl_button_add_focus.ddj")),
            press: asset_server.load(format!("{ART}skl_button_add_press.ddj")),
            ..Default::default()
        };
        bar.spawn((
            SkillPlusButton { group_id },
            Button,
            Hovered::default(),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(x * s),
                top: Val::Px(BUTTON_Y * s),
                width: Val::Px(ICON_SIZE * s),
                height: Val::Px(20.0 * s),
                ..default()
            },
            ImageNode {
                image: style.normal.clone(),
                image_mode: NodeImageMode::Stretch,
                ..default()
            },
            style,
        ))
        .observe(on_skill_plus);
    } else if next.is_none() && level > 0 {
        bar.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(x * s),
                top: Val::Px(BUTTON_Y * s),
                width: Val::Px(ICON_SIZE * s),
                height: Val::Px(20.0 * s),
                ..default()
            },
            ImageNode {
                image: asset_server.load(format!("{ART}skl_level_max.ddj")),
                image_mode: NodeImageMode::Stretch,
                ..default()
            },
            Pickable::IGNORE,
        ));
    }
    Some(())
}

// --- Interactions -----------------------------------------------------------

fn on_top_tab_press(
    mut press: On<Pointer<Press>>,
    tabs: Query<&TopTab>,
    mut state: ResMut<SkillWindowState>,
) {
    press.propagate(false);
    let Ok(tab) = tabs.get(press.entity) else {
        return;
    };
    if state.top_tab != tab.0 {
        state.top_tab = tab.0;
        state.mastery = None;
        state.scroll_y = 0.0;
    }
}

fn on_mastery_tab_press(
    mut press: On<Pointer<Press>>,
    tabs: Query<&MasteryTab>,
    mut state: ResMut<SkillWindowState>,
) {
    press.propagate(false);
    let Ok(tab) = tabs.get(press.entity) else {
        return;
    };
    if state.mastery != Some(tab.0) {
        state.mastery = Some(tab.0);
        state.scroll_y = 0.0;
    }
}

/// The + under a cell: open the practice box for the ladder's next rung.
fn on_skill_plus(
    activate: On<Activate>,
    buttons: Query<&SkillPlusButton>,
    index: Res<SkillGroupIndex>,
    book: Res<SkillBook>,
    mut state: ResMut<SkillWindowState>,
) {
    let Ok(button) = buttons.get(activate.entity) else {
        return;
    };
    let Some(skill_id) = index.next_level_id(&book, button.group_id) else {
        return;
    };
    state.prompt = Some(PracticeAction::LearnSkill {
        group_id: button.group_id,
        skill_id,
    });
}

/// The mastery header button: open the practice box for the mastery raise.
fn on_mastery_plus(
    activate: On<Activate>,
    buttons: Query<&MasteryPlusButton>,
    book: Res<SkillBook>,
    progress: Res<PlayerProgress>,
    mut state: ResMut<SkillWindowState>,
) {
    let Ok(button) = buttons.get(activate.entity) else {
        return;
    };
    if book.mastery_level(button.mastery) >= progress.level as u32 {
        info!(
            "skills: mastery {} capped by character level",
            button.mastery
        );
        return;
    }
    state.prompt = Some(PracticeAction::RaiseMastery {
        mastery: button.mastery,
    });
}

/// Press on a skill icon: left starts the click-carry toward the underbar
/// (learned skills only); right opens the Cyclical Growth removal box on that
/// ladder (#317).
#[allow(clippy::too_many_arguments)]
pub fn on_skill_cell_press(
    mut press: On<Pointer<Press>>,
    cells: Query<&SkillCell>,
    skill_data: Res<ClientSkillData>,
    book: Res<SkillBook>,
    mut withdrawal: ResMut<SkillWithdrawalState>,
    mut drag: ResMut<SkillDrag>,
    ghosts: Query<Entity, With<DragGhost>>,
    asset_server: Res<AssetServer>,
    cam_query: Query<Entity, With<Camera2d>>,
    mut commands: Commands,
) {
    press.propagate(false);
    let Ok(cell) = cells.get(press.entity) else {
        return;
    };
    match press.event.button {
        PointerButton::Secondary => {
            // Right-press still never levels a skill down by itself — it
            // raises the removal box, which is where the decision is taken
            // and confirmed. An unlearned ladder has nothing to withdraw.
            let learned = book.learned_level(cell.group_id);
            if learned > 0 {
                withdrawal.open(cell.group_id, learned);
            }
        }
        PointerButton::Primary => {
            let Some(learned_id) = cell.learned_id else {
                return;
            };
            let Some(row) = skill_data.get(&learned_id) else {
                return;
            };
            // passives can't go on the bar or be cast
            if !row.is_castable() {
                info!("skills: {} is passive", row.code_name());
                return;
            }
            let Some(icon) = row.icon_path() else {
                return;
            };
            let Ok(camera) = cam_query.single() else {
                return;
            };
            // picking up another skill replaces a lingering carry ghost
            for ghost in ghosts.iter() {
                commands.entity(ghost).despawn();
            }
            drag.skill = Some(learned_id as u32);
            drag.from_underbar = false;
            let size = ICON_SIZE * hud_scale();
            let cursor = press.pointer_location.position;
            commands.spawn(drag_ghost_bundle(
                "Skill Drag Ghost",
                asset_server.load(icon),
                cursor,
                size,
                camera,
            ));
        }
        _ => {}
    }
}

/// End a skill carry on Escape, right-click, or a left press that missed
/// every slot (dropping on terrain / other UI discards, like vanilla) — a
/// click-carry otherwise survives until an underbar slot consumes it.
pub fn cancel_skill_drag(
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut drag: ResMut<SkillDrag>,
    ghosts: Query<Entity, With<DragGhost>>,
    mut pending_discard: Local<bool>,
    mut commands: Commands,
) {
    if drag.skill.is_none() {
        *pending_discard = false;
        return;
    }
    // a carry lifted off an underbar slot is a hold-drag: releasing it
    // outside a slot discards it (the slot observers consumed the drop in
    // the frame the release happened, so one frame of grace suffices).
    // A left press with the carry still live means it missed every slot —
    // the slot/cell observers run in PreUpdate and would have consumed or
    // rewritten the drag by now (`is_changed` also skips the frame the
    // carry itself started).
    let discard = *pending_discard
        || mouse.just_pressed(MouseButton::Right)
        || keys.just_pressed(KeyCode::Escape)
        || (mouse.just_pressed(MouseButton::Left) && !drag.is_changed());
    if discard {
        drag.skill = None;
        drag.from_underbar = false;
        *pending_discard = false;
        for ghost in ghosts.iter() {
            commands.entity(ghost).despawn();
        }
        return;
    }
    if drag.from_underbar && mouse.just_released(MouseButton::Left) {
        *pending_discard = true;
    }
}

/// Show the floating tooltip for the hovered skill cell or branch badge:
/// title (+ level), the skilldata `TT_DESC` description, and the next
/// level's SP cost.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
/// Display name of a prerequisite skill group (its first rung's name),
/// falling back to the codename, then the raw group id.
fn prereq_skill_name(
    group_id: i32,
    index: &SkillGroupIndex,
    skill_data: &ClientSkillData,
    names: &ClientTextNames,
) -> String {
    index
        .ladders
        .get(&group_id)
        .and_then(|ladder| ladder.first())
        .and_then(|id| skill_data.get(id))
        .map(|row| {
            row.name_key()
                .and_then(|key| names.name(key))
                .unwrap_or(row.code_name())
                .to_string()
        })
        .unwrap_or_else(|| format!("skill {group_id}"))
}

/// The Consume_* cost lines (cols 52/53) vanilla lists ahead of the param
/// stats; empty for free skills.
fn consume_lines(row: &crate::assets::textdata::skilldata::SkillDataRow) -> Vec<(String, bool)> {
    let mut lines = Vec::new();
    if row.mp_cost() > 0 {
        lines.push((format!("MP consumed: {}", row.mp_cost()), true));
    }
    if row.hp_cost() > 0 {
        lines.push((format!("HP consumed: {}", row.hp_cost()), true));
    }
    lines
}

pub fn update_skill_tooltip(
    mut commands: Commands,
    cells: Query<(&SkillCell, &Hovered)>,
    badges: Query<(&BranchBadge, &Hovered)>,
    skill_data: Res<ClientSkillData>,
    names: Res<ClientTextNames>,
    index: Res<SkillGroupIndex>,
    book: Res<SkillBook>,
    progress: Res<PlayerProgress>,
    fonts: Res<FontAssets>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut roots: Query<(&mut Node, &ComputedNode), With<SkillTooltipRoot>>,
    mut title: Query<&mut Text, (With<SkillTooltipTitle>, Without<SkillTooltipDesc>)>,
    mut desc: Query<
        &mut Text,
        (
            With<SkillTooltipDesc>,
            Without<SkillTooltipTitle>,
            Without<SkillTooltipCost>,
        ),
    >,
    mut cost: Query<
        (&mut Text, &mut TextColor),
        (
            With<SkillTooltipCost>,
            Without<SkillTooltipTitle>,
            Without<SkillTooltipDesc>,
        ),
    >,
    (reqs, ub_cells, ub_special, quickslots, buff_icons, player_status, masteries, ui_strings): (
        Query<Entity, With<SkillTooltipReqs>>,
        Query<(&UbSlotCell, &Hovered)>,
        Query<&Hovered, With<UbSpecialSlotCell>>,
        Res<QuickSlots>,
        Query<(&BuffIcon, &Hovered)>,
        Query<
            (Option<&ActiveBuffs>, Option<&Stunned>, Option<&Frozen>),
            With<crate::plugins::player::Player>,
        >,
        Res<ClientMasteryData>,
        Res<ClientUiStrings>,
    ),
    mut req_cache: Local<Vec<(String, bool)>>,
) {
    let Ok((mut root, computed)) = roots.single_mut() else {
        return;
    };

    // a hovered branch badge shows the series' own title + description; a
    // hovered skill cell adds the next rung's requirements (unmet in red)
    // and its SP cost (red when the wallet is short)
    let (title_line, description, req_lines, cost_line, cost_affordable) =
        if let Some((cell, _)) = cells.iter().find(|(_, hovered)| hovered.get()) {
            let group_id = cell.group_id;
            let level = book.learned_level(group_id);
            let shown = book
                .learned
                .get(&group_id)
                .map(|l| l.skill_id)
                .or_else(|| {
                    index
                        .ladders
                        .get(&group_id)
                        .and_then(|ladder| ladder.first().copied())
                })
                .and_then(|id| skill_data.get(&id));
            // unlearned ladders show the study text (col 65, what learning
            // buys) like vanilla's learn tooltip; learned ones the TT_DESC
            let description = shown
                .and_then(|row| {
                    let study = (level == 0)
                        .then(|| row.study_key().and_then(|key| names.name(key)))
                        .flatten();
                    study.or_else(|| row.tooltip_key().and_then(|key| names.name(key)))
                })
                .unwrap_or("")
                .to_string();
            let passive = shown.is_some_and(|row| !row.is_castable());
            let kind = if passive { " (passive)" } else { "" };
            let next = index
                .next_level_id(&book, group_id)
                .and_then(|id| skill_data.get(&id));
            // the shown rung's MP/HP cost then its stat lines (damage /
            // crit / duration …) lead the panel, the next rung's
            // requirements follow
            let mut req_lines: Vec<(String, bool)> = shown.map(consume_lines).unwrap_or_default();
            req_lines.extend(
                shown
                    .map(|row| row.stat_lines())
                    .unwrap_or_default()
                    .into_iter()
                    .map(|line| (line, true)),
            );
            req_lines.extend(next.into_iter().flat_map(|next| {
                book::learn_requirements(next, &book)
                    .into_iter()
                    .map(|(req, met)| {
                        let line = match req {
                            LearnReq::Mastery { mastery, required } => {
                                format!(
                                    "Requires {} Lv. {required}",
                                    mastery_name(mastery, &masteries, &ui_strings)
                                )
                            }
                            LearnReq::Skill { group_id, level } => format!(
                                "Requires {} Lv. {level}",
                                prereq_skill_name(group_id, &index, &skill_data, &names)
                            ),
                        };
                        (line, met)
                    })
                    .collect::<Vec<_>>()
            }));
            let (cost, affordable) = match next.map(|row| row.sp_cost()) {
                Some(sp) => (format!("Next level: {sp} SP"), progress.skill_points >= sp),
                None => ("Maximum level".to_string(), true),
            };
            (
                format!("{}  Lv {level}{kind}", cell.display_name),
                description,
                req_lines,
                cost,
                affordable,
            )
        } else if let Some((badge, _)) = badges.iter().find(|(_, hovered)| hovered.get()) {
            (
                badge.title.clone(),
                badge.description.clone(),
                Vec::new(),
                String::new(),
                true,
            )
        } else if let Some(SlotAction::Skill { ref_id }) = ub_cells
            .iter()
            .find(|(_, hovered)| hovered.get())
            .and_then(|(cell, _)| quickslots.visible(cell.0))
            .or_else(|| {
                ub_special
                    .iter()
                    .find(|hovered| hovered.get())
                    .and_then(|_| quickslots.special)
            })
        {
            // a hovered underbar skill slot: name + level + description +
            // stat lines (items carry no tooltip data yet)
            let Some(shown) = skill_data.get(&(ref_id as i32)) else {
                if root.display != Display::None {
                    root.display = Display::None;
                }
                req_cache.clear();
                return;
            };
            let display_name = shown
                .name_key()
                .and_then(|key| names.name(key))
                .unwrap_or(shown.code_name())
                .to_string();
            let level = book.learned_level(shown.group_id());
            let description = shown
                .tooltip_key()
                .and_then(|key| names.name(key))
                .unwrap_or("")
                .to_string();
            let mut stats: Vec<(String, bool)> = consume_lines(shown);
            stats.extend(shown.stat_lines().into_iter().map(|line| (line, true)));
            (
                format!("{display_name}  Lv {level}"),
                description,
                stats,
                String::new(),
                true,
            )
        } else if let Some(icon) = buff_icons
            .iter()
            .find(|(_, hovered)| hovered.get())
            .map(|(icon, _)| icon)
        {
            // a hovered magic-state-board icon: buff name + remaining time
            // (whole seconds, so the panel diff rebuilds once per second) +
            // the buff's stat lines; debuffs show their state name
            let (title, remaining, row) = match &icon.0 {
                crate::plugins::hud::magic_state_board::GaugeSource::Buff(codename) => {
                    let remaining = player_status
                        .single()
                        .ok()
                        .and_then(|(buffs, _, _)| {
                            buffs?.0.iter().find(|buff| &buff.codename == codename)
                        })
                        .map(|buff| buff.timer.remaining_secs())
                        .unwrap_or(0.0);
                    let row = skill_data.data().and_then(|data| {
                        data.values()
                            .find(|r| r.basic_group() == Some(codename.as_str()))
                    });
                    let title = row
                        .map(|r| {
                            r.name_key()
                                .and_then(|key| names.name(key))
                                .unwrap_or(r.code_name())
                                .to_string()
                        })
                        .unwrap_or_else(|| codename.clone());
                    (title, remaining, row)
                }
                crate::plugins::hud::magic_state_board::GaugeSource::Stunned => (
                    "Stunned".to_string(),
                    player_status
                        .single()
                        .ok()
                        .and_then(|(_, stunned, _)| stunned)
                        .map(|s| s.0.remaining_secs())
                        .unwrap_or(0.0),
                    None,
                ),
                crate::plugins::hud::magic_state_board::GaugeSource::Frozen => (
                    "Frozen".to_string(),
                    player_status
                        .single()
                        .ok()
                        .and_then(|(_, _, frozen)| frozen)
                        .map(|f| f.0.remaining_secs())
                        .unwrap_or(0.0),
                    None,
                ),
                // A server-reported ailment: the wire names the state but
                // carries no duration, so there is no remaining time to show.
                crate::plugins::hud::magic_state_board::GaugeSource::Ailment(ailment) => {
                    (format!("{ailment:?}"), f32::NAN, None)
                }
            };
            // NaN marks "no duration on the wire" — omit the line rather than
            // print a made-up 0s.
            let mut lines = Vec::new();
            if !remaining.is_nan() {
                lines.push((format!("Remaining: {}s", remaining.ceil() as u32), true));
            }
            lines.extend(
                row.map(|r| r.stat_lines())
                    .unwrap_or_default()
                    .into_iter()
                    .map(|line| (line, true)),
            );
            let description = row
                .and_then(|r| r.tooltip_key())
                .and_then(|key| names.name(key))
                .unwrap_or("")
                .to_string();
            (title, description, lines, String::new(), true)
        } else {
            if root.display != Display::None {
                root.display = Display::None;
            }
            // force a repaint of the requirement lines on the next hover
            req_cache.clear();
            return;
        };

    if let Ok(mut text) = title.single_mut() {
        if text.0 != title_line {
            text.0 = title_line;
        }
    }
    if let Ok(mut text) = desc.single_mut() {
        if text.0 != description {
            text.0 = description;
        }
    }
    if let Ok((mut text, mut color)) = cost.single_mut() {
        if text.0 != cost_line {
            text.0 = cost_line;
        }
        let wanted = if cost_affordable {
            Color::srgb(1.0, 0.85, 0.33)
        } else {
            Color::srgb(1.0, 0.35, 0.3)
        };
        if color.0 != wanted {
            color.0 = wanted;
        }
    }
    // rebuild the requirement lines only when they change (this system runs
    // every frame while hovering); met/unmet recomputes from the live book,
    // so learning a prereq mid-hover flips the line color next frame
    if *req_cache != req_lines {
        req_cache.clone_from(&req_lines);
        if let Ok(reqs_entity) = reqs.single() {
            commands
                .entity(reqs_entity)
                .despawn_related::<Children>()
                .with_children(|panel| {
                    for (line, met) in req_lines {
                        panel.spawn((
                            Text::new(line),
                            TextFont {
                                font: fonts.two.clone().into(),
                                font_size: FontSize::Px(8.0 * hud_scale()),
                                ..default()
                            },
                            TextColor(if met {
                                Color::WHITE
                            } else {
                                Color::srgb(1.0, 0.35, 0.3)
                            }),
                            Pickable::IGNORE,
                        ));
                    }
                });
        }
    }
    // Follow the cursor, offset so the pointer never covers the text;
    // clamped like the inventory tooltip: x pinned to the screen, y flips
    // ABOVE the cursor when the panel would run off the bottom (underbar
    // hovers). The computed size is one frame stale on the first hover
    // (Display::None layout) — accepted, same as inventory.
    if let Some((cursor, window)) = windows
        .single()
        .ok()
        .and_then(|w| w.cursor_position().map(|c| (c, w)))
    {
        let size = computed.size() * computed.inverse_scale_factor();
        const MARGIN: f32 = 4.0;
        let x = (cursor.x + 18.0).min((window.width() - size.x - MARGIN).max(0.0));
        let y = if cursor.y + 12.0 + size.y > window.height() {
            (cursor.y - size.y - 12.0).max(0.0)
        } else {
            cursor.y + 12.0
        };
        root.left = Val::Px(x);
        root.top = Val::Px(y);
    }
    if root.display != Display::Flex {
        root.display = Display::Flex;
    }
}

// --- Practice box (level-up confirmation) -----------------------------------

// Idea: the practice box's every rect is authored in `ifskillpracticebox.txt`
// in the *window's* own coordinates, while our shell hands the caller a
// content node that starts inside the frame. So the rects are kept here
// verbatim as the data has them and converted once, by `pb()`, instead of
// being pre-subtracted by hand — a transcription error then shows up against
// the file rather than hiding in a magic number.
//
// The conversion origin is measured, not fitted: `msgbox2_window_`'s corner
// art is 16 px wide and its top strip 40 px tall (DDS headers of
// `msgbox2_window_left_up.ddj` = 16x40, `_left_down.ddj` = 16x16), and the
// box's two background tiles begin exactly there — `GDR_SKLPB_BGTILE`
// `16,40,288,57`. The width closes independently: 16 + 288 = 304 = 320 - 16
// against `GDR_PRACTICEPOPUP`'s `Rect="100,100,320,310"` (`ginterface.txt:221`).
//
// Still drifting after this, deliberately: the frame *family*. Vanilla draws
// `interface\messagebox\msgbox2_window_`, a modal dialog shell with no title
// and no close button; we still draw the titled `mframe_wnd_` window. Swapping
// it means re-cutting `spawn_game_window`'s 8-piece ring for a second family
// (16/40/16 instead of 40/68/48) and dropping the title band — shared chrome
// used by five other windows, and #317 lists the exact 9-piece slicing of
// `msgbox2_window_` as an open UNKNOWN. Out of scope here; the interior is
// what the data itemizes and what the player reads.

/// Interior origin of the vanilla box in its own window coordinates.
const PB_ORIGIN: (f32, f32) = (16.0, 40.0);
/// Interior extent: the two `com_bg_tile_b` strips span x 16..304 and
/// y 40..318 (`GDR_SKLPB_BGTILE` `16,40,288,57` + `GDR_SKLPB_BGTILE2`
/// `16,217,288,101`).
const PB_CONTENT: (f32, f32) = (288.0, 278.0);

/// `ifskillpracticebox.txt` rects, exactly as authored (window-relative).
const PB_SLOT: (f32, f32, f32, f32) = (22.0, 48.0, 32.0, 32.0);
const PB_MNDECO: (f32, f32, f32, f32) = (54.0, 48.0, 212.0, 36.0);
const PB_SKILLNAME: (f32, f32, f32, f32) = (76.0, 52.0, 168.0, 20.0);
const PB_MASTERYNAME: (f32, f32, f32, f32) = (59.0, 60.0, 190.0, 12.0);
const PB_LEVEL_LV: (f32, f32, f32, f32) = (247.0, 52.0, 19.0, 20.0);
const PB_LEVEL_NUM: (f32, f32, f32, f32) = (269.0, 52.0, 32.0, 20.0);
const PB_SQUARE: (f32, f32, f32, f32) = (16.0, 97.0, 288.0, 120.0);
const PB_DESCRIPTION: (f32, f32, f32, f32) = (30.0, 106.0, 267.0, 105.0);
const PB_NEEDSP: (f32, f32, f32, f32) = (43.0, 255.0, 119.0, 20.0);
const PB_NEEDSP_AMOUNT: (f32, f32, f32, f32) = (170.0, 254.0, 89.0, 24.0);
/// `GDR_SKLPB_BLACKBOX` is authored `165,254,0,0` — a zero rect means "draw
/// the art at its native size", and `msgbox_blackbox.ddj`'s DDS header reads
/// 100x24. That brackets the amount static (170..259) exactly, which is the
/// cross-check that the zero-rect reading is right.
const PB_BLACKBOX: (f32, f32, f32, f32) = (165.0, 254.0, 100.0, 24.0);
const PB_BTN_CONFIRM: (f32, f32, f32, f32) = (81.0, 288.0, 76.0, 24.0);
const PB_BTN_CANCEL: (f32, f32, f32, f32) = (162.0, 288.0, 76.0, 24.0);

/// Window-relative resinfo rect -> content-relative, for the content node
/// `spawn_game_window` hands back.
const fn pb(r: (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
    (r.0 - PB_ORIGIN.0, r.1 - PB_ORIGIN.1, r.2, r.3)
}

/// Spawn/despawn the practice box to match `state.prompt`
/// (transcribed from ifskillpracticebox.txt: icon slot + name + level row,
/// description square, "Need SP" row, com_button Confirm/Cancel).
#[allow(clippy::too_many_arguments)]
pub fn apply_practice_prompt(
    state: Res<SkillWindowState>,
    existing: Query<Entity, With<PracticeBoxRoot>>,
    skill_data: Res<ClientSkillData>,
    names: Res<ClientTextNames>,
    masteries: Res<ClientMasteryData>,
    ui_strings: Res<ClientUiStrings>,
    book: Res<SkillBook>,
    level_data: Res<ClientLevelData>,
    progress: Res<PlayerProgress>,
    fonts: Res<FontAssets>,
    asset_server: Res<AssetServer>,
    cam_query: Query<Entity, With<Camera2d>>,
    mut commands: Commands,
) {
    if !state.is_changed() {
        return;
    }
    for entity in existing.iter() {
        commands.entity(entity).despawn();
    }
    let Some(action) = state.prompt else {
        return;
    };
    let Ok(camera) = cam_query.single() else {
        return;
    };
    let s = hud_scale();

    // what the box shows, per action
    let (icon, name, level_value, cost) = match action {
        PracticeAction::LearnSkill { skill_id, .. } => {
            let Some(row) = skill_data.get(&skill_id) else {
                return;
            };
            let name = row
                .name_key()
                .and_then(|key| names.name(key))
                .unwrap_or(row.code_name())
                .to_string();
            (
                row.icon_path(),
                name,
                row.basic_level().to_string(),
                row.sp_cost(),
            )
        }
        PracticeAction::RaiseMastery { mastery } => {
            let target = book.mastery_level(mastery) + 1;
            (
                None,
                format!("{} Mastery", mastery_name(mastery, &masteries, &ui_strings)),
                target.to_string(),
                mastery_raise_cost(&level_data, target),
            )
        }
    };

    let window = spawn_game_window(
        &mut commands,
        &asset_server,
        &fonts,
        camera,
        "Practice",
        PB_CONTENT,
        (700.0, 240.0),
        s,
    );
    commands
        .entity(window.root)
        .insert((PracticeBoxRoot, GlobalZIndex(40)));
    commands.entity(window.expect_close_button()).observe(
        |_: On<Activate>, mut state: ResMut<SkillWindowState>| {
            state.prompt = None;
        },
    );

    let text_font = |size: f32| TextFont {
        font: fonts.two.clone().into(),
        font_size: FontSize::Px(size * s),
        ..default()
    };
    commands.entity(window.content).with_children(|content| {
        // icon + name + level row, on GDR_SKLPB_SLOT
        let is_skill = icon.is_some();
        if let Some(icon) = icon {
            content.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(pb(PB_SLOT).0 * s),
                    top: Val::Px(pb(PB_SLOT).1 * s),
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
        }
        // A mastery raise gets vanilla's name-box decoration behind its
        // caption (`GDR_SKLPB_MNDECO`, `npc_mastery_namebox.ddj`, whose DDS
        // header 212x36 matches the authored rect exactly); a skill learn has
        // no deco and sits on the wider `GDR_SKLPB_SKILLNAME` line.
        let name_rect = if is_skill {
            pb(PB_SKILLNAME)
        } else {
            content.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(pb(PB_MNDECO).0 * s),
                    top: Val::Px(pb(PB_MNDECO).1 * s),
                    width: Val::Px(pb(PB_MNDECO).2 * s),
                    height: Val::Px(pb(PB_MNDECO).3 * s),
                    ..default()
                },
                ImageNode {
                    image: asset_server.load("media://interface/npc/npc_mastery_namebox.ddj"),
                    image_mode: NodeImageMode::Stretch,
                    ..default()
                },
                Pickable::IGNORE,
            ));
            pb(PB_MASTERYNAME)
        };
        content.spawn((
            Text::new(name.clone()),
            text_font(9.0),
            TextColor(Color::WHITE),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(name_rect.0 * s),
                top: Val::Px(name_rect.1 * s),
                ..default()
            },
            Pickable::IGNORE,
        ));
        // vanilla splits the level row: a localized "Lv" static
        // (`GDR_SKLPB_SKILLLEV_LV`, `UIIT_STT_LEVEL_LV`) and the number on its
        // own rect (`GDR_SKLPB_SKILLLEV`) — not one baked English string.
        let level_label = |rect: (f32, f32, f32, f32), text: String| {
            (
                Text::new(text),
                text_font(9.0),
                TextColor(Color::srgb(0.94, 0.85, 0.64)),
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(rect.0 * s),
                    top: Val::Px(rect.1 * s),
                    ..default()
                },
                Pickable::IGNORE,
            )
        };
        content.spawn(level_label(
            pb(PB_LEVEL_LV),
            ui_strings.get_or("UIIT_STT_LEVEL_LV", "Lv").to_string(),
        ));
        content.spawn(level_label(pb(PB_LEVEL_NUM), level_value));
        // description square (com_square_ stand-in: dark fill)
        content
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(pb(PB_SQUARE).0 * s),
                    top: Val::Px(pb(PB_SQUARE).1 * s),
                    width: Val::Px(pb(PB_SQUARE).2 * s),
                    height: Val::Px(pb(PB_SQUARE).3 * s),
                    padding: UiRect {
                        left: Val::Px((PB_DESCRIPTION.0 - PB_SQUARE.0) * s),
                        top: Val::Px((PB_DESCRIPTION.1 - PB_SQUARE.1) * s),
                        ..default()
                    },
                    ..default()
                },
                BackgroundColor(Color::srgba(0.04, 0.04, 0.05, 0.9)),
                Pickable::IGNORE,
            ))
            .with_children(|desc| {
                desc.spawn((
                    Text::new(format!("Practice {name} to the next stage?")),
                    text_font(8.5),
                    TextColor(Color::srgb(0.88, 0.86, 0.78)),
                    Pickable::IGNORE,
                ));
            });
        // Need SP row: label + the amount on its own dark box (vanilla's
        // msgbox_blackbox), red when the wallet can't cover it
        let affordable = progress.skill_points >= cost;
        content.spawn((
            Text::new("Need SP"),
            text_font(8.5),
            TextColor(Color::srgb(1.0, 0.85, 0.33)),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(pb(PB_NEEDSP).0 * s),
                top: Val::Px(pb(PB_NEEDSP).1 * s),
                ..default()
            },
            Pickable::IGNORE,
        ));
        // vanilla's own dark plate behind the amount, at last wired
        // (`GDR_SKLPB_BLACKBOX`) instead of approximated with a fill colour
        content.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(pb(PB_BLACKBOX).0 * s),
                top: Val::Px(pb(PB_BLACKBOX).1 * s),
                width: Val::Px(pb(PB_BLACKBOX).2 * s),
                height: Val::Px(pb(PB_BLACKBOX).3 * s),
                ..default()
            },
            ImageNode {
                image: asset_server.load("media://interface/messagebox/msgbox_blackbox.ddj"),
                image_mode: NodeImageMode::Stretch,
                ..default()
            },
            Pickable::IGNORE,
        ));
        content
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(pb(PB_NEEDSP_AMOUNT).0 * s),
                    top: Val::Px(pb(PB_NEEDSP_AMOUNT).1 * s),
                    width: Val::Px(pb(PB_NEEDSP_AMOUNT).2 * s),
                    height: Val::Px(pb(PB_NEEDSP_AMOUNT).3 * s),
                    justify_content: JustifyContent::FlexEnd,
                    padding: UiRect::right(Val::Px(6.0 * s)),
                    ..default()
                },
                Pickable::IGNORE,
            ))
            .with_children(|amount_box| {
                amount_box.spawn((
                    Text::new(cost.to_string()),
                    text_font(8.5),
                    TextColor(if affordable {
                        Color::srgb(1.0, 0.85, 0.33)
                    } else {
                        Color::srgb(1.0, 0.35, 0.3)
                    }),
                    Node {
                        top: Val::Px(3.0 * s),
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
            });
        // Confirm / Cancel (`GDR_SKLPB_BTN_PRACTICE` / `_BTN_CANCEL`)
        let button = |content: &mut ChildSpawnerCommands, x: f32, label: &str| -> Entity {
            let style = ImageButtonStyle {
                normal: asset_server.load("media://interface/ifcommon/com_button.ddj"),
                hover: asset_server.load("media://interface/ifcommon/com_button_focus.ddj"),
                press: asset_server.load("media://interface/ifcommon/com_button_press.ddj"),
                ..Default::default()
            };
            content
                .spawn((
                    Button,
                    Hovered::default(),
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(x * s),
                        top: Val::Px(pb(PB_BTN_CONFIRM).1 * s),
                        width: Val::Px(PB_BTN_CONFIRM.2 * s),
                        height: Val::Px(PB_BTN_CONFIRM.3 * s),
                        ..default()
                    },
                    ImageNode {
                        image: style.normal.clone(),
                        image_mode: NodeImageMode::Stretch,
                        ..default()
                    },
                    style,
                ))
                .with_children(|b| {
                    b.spawn((
                        Text::new(label.to_string()),
                        text_font(8.5),
                        TextColor(Color::srgb(1.0, 0.96, 0.85)),
                        TextLayout::justify(Justify::Center),
                        Node {
                            position_type: PositionType::Absolute,
                            top: Val::Px(6.0 * s),
                            width: Val::Percent(100.0),
                            ..default()
                        },
                        Pickable::IGNORE,
                    ));
                })
                .id()
        };
        let confirm = button(content, pb(PB_BTN_CONFIRM).0, "Confirm");
        let cancel = button(content, pb(PB_BTN_CANCEL).0, "Cancel");
        content
            .commands()
            .entity(confirm)
            .observe(on_practice_confirm);
        content.commands().entity(cancel).observe(
            |_: On<Activate>, mut state: ResMut<SkillWindowState>| {
                state.prompt = None;
            },
        );
    });
}

/// SP cost of raising a mastery to `target` (leveldata col 2; 1 SP when the
/// table is missing the level, matching the low-level rows).
fn mastery_raise_cost(level_data: &ClientLevelData, target: u32) -> u32 {
    level_data
        .mastery_sp_cost(target.min(u8::MAX as u32) as u8)
        .unwrap_or(1)
}

/// Apply the pending practice: learn the rung / raise the mastery (SP
/// checked and deducted here for masteries; `learn_next` does it for
/// skills). Shared by the Confirm button and the Enter key. Online (an
/// agent connection and no [`LocalCombat`] sim) the request goes to the
/// server instead — vSRO opcodes 0x70A1/0x70A2; the 0xB0A1/0xB0A2 ack then
/// updates the book (`book::apply_learn_responses`), which repaints the
/// window and upgrades quickslots.
fn confirm_practice(
    state: &mut SkillWindowState,
    skill_data: &ClientSkillData,
    index: &SkillGroupIndex,
    level_data: &ClientLevelData,
    book: &mut SkillBook,
    progress: &mut PlayerProgress,
    online: Option<&crate::net::connection::SilkroadConnection>,
) {
    let Some(action) = state.prompt.take() else {
        return;
    };
    match action {
        PracticeAction::LearnSkill {
            group_id, skill_id, ..
        } => {
            if let Some(conn) = online {
                info!("skills: learn skill {skill_id} sent (experimental 0x70A1)");
                let packet = packets::agent::ingame::SkillLearnRequest {
                    ref_skill_id: skill_id as u32,
                };
                if let Err(e) = conn.get_sender().send(packets::Packet::from(packet).into()) {
                    bevy::log::error!("network: failed to send SkillLearnRequest: {}", e.0);
                }
                return;
            }
            let Some(data) = skill_data.data() else {
                return;
            };
            match book::learn_next(book, index, data, progress, group_id) {
                Some(id) => info!("skills: learned skill {id} (group {group_id})"),
                None => info!("skills: cannot level group {group_id} (requirements/SP)"),
            }
        }
        PracticeAction::RaiseMastery { mastery } => {
            if let Some(conn) = online {
                info!("skills: raise mastery {mastery} sent (experimental 0x70A2)");
                let packet = packets::agent::ingame::MasteryLearnRequest {
                    mastery_id: mastery,
                    amount: 1,
                };
                if let Err(e) = conn.get_sender().send(packets::Packet::from(packet).into()) {
                    bevy::log::error!("network: failed to send MasteryLearnRequest: {}", e.0);
                }
                return;
            }
            let level = book.mastery_level(mastery);
            if level >= progress.level as u32 {
                return;
            }
            let cost = mastery_raise_cost(level_data, level + 1);
            if progress.skill_points < cost {
                info!("skills: not enough SP for the mastery raise ({cost})");
                return;
            }
            progress.skill_points -= cost;
            book.masteries.insert(mastery, level + 1);
            info!(
                "skills: mastery {mastery} raised to {} (-{cost} SP)",
                level + 1
            );
        }
    }
}

/// The agent connection when the offline sim is absent — the practice box
/// then talks to the server instead of mutating the book.
fn online_connection<'a>(
    local: &Option<Res<crate::plugins::skills::LocalCombat>>,
    conn: &'a Query<
        &crate::net::connection::SilkroadConnection,
        With<crate::plugins::net::agent::AgentConnection>,
    >,
) -> Option<&'a crate::net::connection::SilkroadConnection> {
    if local.is_some() {
        return None;
    }
    conn.single().ok()
}

/// Confirm the pending practice from its button.
#[allow(clippy::too_many_arguments)]
fn on_practice_confirm(
    _: On<Activate>,
    mut state: ResMut<SkillWindowState>,
    skill_data: Res<ClientSkillData>,
    index: Res<SkillGroupIndex>,
    level_data: Res<ClientLevelData>,
    mut book: ResMut<SkillBook>,
    mut progress: ResMut<PlayerProgress>,
    local: Option<Res<crate::plugins::skills::LocalCombat>>,
    conn: Query<
        &crate::net::connection::SilkroadConnection,
        With<crate::plugins::net::agent::AgentConnection>,
    >,
) {
    confirm_practice(
        &mut state,
        &skill_data,
        &index,
        &level_data,
        &mut book,
        &mut progress,
        online_connection(&local, &conn),
    );
}

/// Keyboard path for the practice box: Enter confirms, Escape cancels.
#[allow(clippy::too_many_arguments)]
pub fn practice_prompt_keys(
    keys: Res<ButtonInput<KeyCode>>,
    mut state: ResMut<SkillWindowState>,
    skill_data: Res<ClientSkillData>,
    index: Res<SkillGroupIndex>,
    level_data: Res<ClientLevelData>,
    mut book: ResMut<SkillBook>,
    mut progress: ResMut<PlayerProgress>,
    local: Option<Res<crate::plugins::skills::LocalCombat>>,
    conn: Query<
        &crate::net::connection::SilkroadConnection,
        With<crate::plugins::net::agent::AgentConnection>,
    >,
) {
    if state.prompt.is_none() {
        return;
    }
    if keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter) {
        confirm_practice(
            &mut state,
            &skill_data,
            &index,
            &level_data,
            &mut book,
            &mut progress,
            online_connection(&local, &conn),
        );
    } else if keys.just_pressed(KeyCode::Escape) {
        state.prompt = None;
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// `ifskill.txt` — `GDR_SKILL_FRAME:CIFFrame` is `0,0,364,333`, the same
    /// extent as the MainPopup page `GDR_SKILL` id 73 (`ifmainpopup.txt:84`).
    /// The page carries no chrome of its own, so page coordinates *are* our
    /// content coordinates (MainPopup hosting is #302; #307 is closed).
    #[test]
    fn skill_page_extent_matches_gdr_skill_frame() {
        assert_eq!((CONTENT_W, CONTENT_H), (364.0, 333.0));
    }

    /// `ifskill.txt` — `GDR_SKILL_BOARD` (`int_window_`) is `6,29,351,270` and
    /// `GDR_SKILL_BOTTOM_BOX` sits at y 299 with the art-sized 364x36
    /// `skl_wnd_box`, i.e. flush under the board.
    #[test]
    fn board_frame_and_bottom_box_match_ifskill_rects() {
        assert_eq!(
            (BOARD_X, BOARD_FRAME_Y, BOARD_W, BOARD_FRAME_H),
            (6.0, 29.0, 351.0, 270.0)
        );
        assert_eq!(BOTTOM_Y, BOARD_FRAME_Y + BOARD_FRAME_H);
        assert_eq!((BOTTOM_Y, BOTTOM_H), (299.0, 36.0));
        // the bar's four labels: ids 21-24 at y 311, i.e. 12 inside the box
        assert_eq!(BOTTOM_LABEL_X, [14.0, 86.0, 183.0, 293.0]);
        assert_eq!(BOARD_FRAME_Y + BOARD_FRAME_H + SP_LABEL_Y, 311.0);
    }

    /// `ifskillboard.txt` — the "Lv" caption (`GDR_SKILLBOARD_STATIC_LV`,
    /// id 15) is its own localized static at `267,12,40,24`; only the number
    /// lives in `GDR_SKILLBOARD_MASTERYLEV` (id 13) at `291,12,46,24`.
    #[test]
    fn header_level_boxes_match_ifskillboard_rects() {
        assert_eq!(LV_LABEL_BOX, (267.0, 12.0, 40.0, 24.0));
        assert_eq!(LEVEL_BOX, (291.0, 12.0, 46.0, 24.0));
        // the caption box ends where the number box begins, no overlap
        assert!(LV_LABEL_BOX.0 + LV_LABEL_BOX.2 <= LEVEL_BOX.0 + LEVEL_BOX.2);
    }

    /// The scroll region and its gutter are board-local (`ifskillboard.txt`
    /// `GDR_SKILLBOARD_BOARD` `4,41,342,225`), so they must stay inside the
    /// board frame's 16px pieces in both axes. The row y is ours — the two tab
    /// rows above it are not placed by any resinfo file ([U]).
    #[test]
    fn rows_and_gutter_stay_inside_the_board_frame() {
        let inner_right = BOARD_X + BOARD_W - BOARD_FRAME_PIECE;
        let inner_bottom = BOARD_FRAME_Y + BOARD_FRAME_H - BOARD_FRAME_PIECE;
        assert!(BOARD_X + ROWS_X >= BOARD_X);
        assert!(BOARD_X + ROWS_X + ROW_W + SCROLL_W <= inner_right + BOARD_FRAME_PIECE);
        assert_eq!(ROWS_Y + ROWS_H, inner_bottom);
        assert!(HEADER_Y + HEADER_H <= ROWS_Y);
    }

    /// `ifskill_slot.txt` id2 — `GDR_STMS_BTN_LEVELUP` is `0,32,32,20`
    /// (`skl_button_add`, art 32x20), flush under the 32px icon, and the whole
    /// cell fits the 60px `skl_mastery_bar` row.
    #[test]
    fn row_add_button_sits_flush_under_the_icon() {
        assert_eq!(BUTTON_Y, ICON_SIZE);
        assert_eq!(BUTTON_Y, 32.0);
        assert!(BUTTON_Y + 20.0 <= ROW_H);
    }

    /// The practice box is laid out from `ifskillpracticebox.txt` in the
    /// window coordinates that file authors, so the one thing that can
    /// silently rot is the conversion into our content node. Pin the origin
    /// against the two independent things that fix it: `msgbox2_window_`'s
    /// art extents (corner 16 wide, top strip 40 tall) and the background
    /// tiles that start exactly there — and pin the width against
    /// `GDR_PRACTICEPOPUP`'s `100,100,320,310` (`ginterface.txt:221`), which
    /// it must close as 16 + 288 + 16 = 320.
    #[test]
    fn the_practice_box_interior_is_the_vanilla_one() {
        assert_eq!(PB_ORIGIN, (16.0, 40.0), "msgbox2_window_ corner/top art");
        assert_eq!(PB_CONTENT.0, 288.0, "GDR_SKLPB_BGTILE width");
        assert_eq!(
            PB_ORIGIN.0 + PB_CONTENT.0 + PB_ORIGIN.0,
            320.0,
            "must close against GDR_PRACTICEPOPUP's 320-wide rect"
        );
        // y 40..318: BGTILE starts at 40, BGTILE2 (16,217,288,101) ends at 318.
        assert_eq!(PB_CONTENT.1, 318.0 - PB_ORIGIN.1);
        // and the interior must actually contain the lowest element, the
        // buttons at y 288..312 — the old 250 tall box cut them off.
        assert!(PB_BTN_CONFIRM.1 + PB_BTN_CONFIRM.3 - PB_ORIGIN.1 <= PB_CONTENT.1);
    }

    /// Every rect is transcribed once and converted once; this is the
    /// by-name guard that the transcription still matches the file. Values
    /// are quoted from `ifskillpracticebox.txt` in the comment beside each.
    #[test]
    fn the_practice_box_rects_match_ifskillpracticebox() {
        assert_eq!(PB_SLOT, (22.0, 48.0, 32.0, 32.0)); // GDR_SKLPB_SLOT
        assert_eq!(PB_SKILLNAME, (76.0, 52.0, 168.0, 20.0)); // _SKILLNAME
        assert_eq!(PB_MASTERYNAME, (59.0, 60.0, 190.0, 12.0)); // _MASTERYNAME
        assert_eq!(PB_LEVEL_LV, (247.0, 52.0, 19.0, 20.0)); // _SKILLLEV_LV
        assert_eq!(PB_LEVEL_NUM, (269.0, 52.0, 32.0, 20.0)); // _SKILLLEV
        assert_eq!(PB_SQUARE, (16.0, 97.0, 288.0, 120.0)); // _SQUARE
        assert_eq!(PB_DESCRIPTION, (30.0, 106.0, 267.0, 105.0)); // _DESCRIPTION
        assert_eq!(PB_NEEDSP, (43.0, 255.0, 119.0, 20.0)); // _NEEDSP
        assert_eq!(PB_NEEDSP_AMOUNT, (170.0, 254.0, 89.0, 24.0)); // _NEEDSP_AMOUNT
        assert_eq!(PB_BTN_CONFIRM, (81.0, 288.0, 76.0, 24.0)); // _BTN_PRACTICE
        assert_eq!(PB_BTN_CANCEL, (162.0, 288.0, 76.0, 24.0)); // _BTN_CANCEL
                                                               // The two art-sized statics, cross-checked against their DDS headers:
                                                               // npc_mastery_namebox.ddj is 212x36 and matches its authored rect;
                                                               // msgbox_blackbox.ddj is 100x24 for the zero-rect `165,254,0,0`, and
                                                               // must bracket the amount static it sits behind.
        assert_eq!(PB_MNDECO, (54.0, 48.0, 212.0, 36.0));
        assert_eq!((PB_BLACKBOX.2, PB_BLACKBOX.3), (100.0, 24.0));
        assert!(PB_BLACKBOX.0 <= PB_NEEDSP_AMOUNT.0);
        assert!(PB_BLACKBOX.0 + PB_BLACKBOX.2 >= PB_NEEDSP_AMOUNT.0 + PB_NEEDSP_AMOUNT.2);
    }

    /// `pb()` is the only place a window-relative rect becomes a
    /// content-relative one; a sign slip here would move the whole box.
    #[test]
    fn pb_shifts_by_the_interior_origin_and_keeps_the_extent() {
        assert_eq!(pb(PB_SLOT), (6.0, 8.0, 32.0, 32.0));
        assert_eq!(pb(PB_BTN_CANCEL), (146.0, 248.0, 76.0, 24.0));
    }
}
