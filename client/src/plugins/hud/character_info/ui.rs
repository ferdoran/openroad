//! Character info window (C) layout + refresh.
//!
//! Idea: every rect below is hand-transcribed from the vanilla
//! `resinfo/ifplayerinfo.txt` (window space 364x356) and remapped into the
//! shared `hud::game_window` shell's content space by subtracting that shell's
//! content origin — `(FRAME_VIS_SIDE + CHROME_PAD, CONTENT_TOP)` = `(12, 36)`.
//! Subtract the shell's own constants, never a value re-derived from vanilla's
//! interior tile inset: `GDR_PI_BG_TILE_D` starts at y=26, and taking 26 as the
//! origin put every element 10 units too low and made the outer window 382 tall
//! against vanilla's 356 (#310). `interior_rects_*` below pins this.
//! Vanilla's chr_window_left/right_* edge ornaments are skipped (they are
//! sframe decorations that would fight the mframe ring); the horizontal
//! chr_window_mid_* dividers and the bottom ornament are kept. The window is
//! spawned once (hidden) on entering a playable scene and toggled via
//! `CharacterInfoState.open`; `refresh_character_info` repaints the level,
//! exp, stat-point, STR/INT, HP/MP gauge and combat-stat texts from
//! `PlayerStats`/`PlayerVitals`/`PlayerProgress`. The + buttons send the
//! EXPERIMENTAL 0x7050/0x7051 stat-spend requests (model.rs applies acks).

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui::InteractionDisabled;
use bevy::ui_widgets::{Activate, Button};

use packets::agent::prelude::{IncreaseIntRequest, IncreaseStrRequest};
use packets::Packet;

use crate::assets::FontAssets;
use crate::net::connection::SilkroadConnection;
use crate::plugins::hud::character_info::model::{balance_text, CharacterInfoState, PlayerStats};
use crate::plugins::hud::game_window::{self, abs_node};
use crate::plugins::hud::player_mini_info::PlayerVitals;
use crate::plugins::hud::scale::hud_scale;
use crate::plugins::hud::underbar::model::PlayerProgress;
use crate::plugins::hud::window_positions::PersistedWindow;
use crate::plugins::net::agent::AgentConnection;
use crate::plugins::net::character_info::CharacterInfo;
use crate::plugins::player::Player;
use crate::plugins::settings::window_positions::WndPosSlot;
use crate::plugins::textdata::{ClientLevelData, ClientUiStrings};
use crate::plugins::ui_v2::style::ImageButtonStyle;

// --- Layout constants (content space = resinfo window space - (12, 36)) -----

const CONTENT_W: f32 = 340.0;
/// Sized so the shell's outer box equals vanilla's tab rect (364x356):
/// `CONTENT_TOP 36 + 304 + CHROME_PAD 4 + FRAME_VIS_BOTTOM 12`. The bottom
/// ornament reaches content y 320 and so overhangs this box into the frame's
/// bottom band — deliberate, and what vanilla does over its own sframe edge.
const CONTENT_H: f32 = 304.0;
/// Right/top anchor of the (draggable) window on spawn.
const WINDOW_RIGHT: f32 = 620.0;
const WINDOW_TOP: f32 = 60.0;

const ART: &str = "media://interface/character/";

// exp strip (vanilla window y 26..63)
const CUR_EXP_LABEL_RECT: (f32, f32, f32, f32) = (20.0, 5.0, 65.0, 12.0);
const CUR_EXP_VALUE_RECT: (f32, f32, f32, f32) = (84.0, 5.0, 72.0, 12.0);
const NEXT_EXP_LABEL_RECT: (f32, f32, f32, f32) = (185.0, 5.0, 65.0, 12.0);
const NEXT_EXP_VALUE_RECT: (f32, f32, f32, f32) = (248.0, 5.0, 72.0, 12.0);

// horizontal dividers + bottom ornament (chr_window_mid_*)
const DIVIDER_TOP_RECT: (f32, f32, f32, f32) = (4.0, 27.0, 332.0, 12.0);
const DIVIDER_MID_RECT: (f32, f32, f32, f32) = (4.0, 212.0, 332.0, 12.0);
const BOTTOM_DECO_RECT: (f32, f32, f32, f32) = (4.0, 284.0, 332.0, 36.0);

// stat point / honor row
const STAT_LABEL_RECT: (f32, f32, f32, f32) = (5.0, 49.0, 94.0, 12.0);
const STAT_VALUE_RECT: (f32, f32, f32, f32) = (77.0, 49.0, 28.0, 12.0);
const HONOR_LABEL_RECT: (f32, f32, f32, f32) = (195.0, 49.0, 94.0, 12.0);
const HONOR_VALUE_RECT: (f32, f32, f32, f32) = (265.0, 49.0, 28.0, 12.0);

// STR + HP row (chr_stat_window board, 332x24)
const HP_BOARD_RECT: (f32, f32, f32, f32) = (3.0, 71.0, 332.0, 24.0);
const STR_LABEL_RECT: (f32, f32, f32, f32) = (5.0, 77.0, 46.0, 12.0);
const ADD_STR_RECT: (f32, f32, f32, f32) = (50.0, 73.0, 20.0, 20.0);
const STR_VALUE_RECT: (f32, f32, f32, f32) = (72.0, 78.0, 30.0, 12.0);
const HP_LABEL_RECT: (f32, f32, f32, f32) = (121.0, 79.0, 16.0, 11.0);
const HP_GAUGE_RECT: (f32, f32, f32, f32) = (141.0, 77.0, 188.0, 12.0);
const HP_VALUE_RECT: (f32, f32, f32, f32) = (141.0, 78.0, 188.0, 11.0);

// INT + MP row
const MP_BOARD_RECT: (f32, f32, f32, f32) = (3.0, 100.0, 332.0, 24.0);
const INT_LABEL_RECT: (f32, f32, f32, f32) = (5.0, 106.0, 46.0, 12.0);
const ADD_INT_RECT: (f32, f32, f32, f32) = (50.0, 102.0, 20.0, 20.0);
const INT_VALUE_RECT: (f32, f32, f32, f32) = (72.0, 107.0, 30.0, 12.0);
const MP_LABEL_RECT: (f32, f32, f32, f32) = (121.0, 108.0, 16.0, 11.0);
const MP_GAUGE_RECT: (f32, f32, f32, f32) = (141.0, 106.0, 188.0, 12.0);
const MP_VALUE_RECT: (f32, f32, f32, f32) = (141.0, 108.0, 188.0, 11.0);

// combat stat grid: left column (physical + hit), right column (magical +
// parry); label x then value-box x per column, four rows each
const GRID_LEFT_LABEL_X: f32 = 2.0;
const GRID_LEFT_VALUE_X: f32 = 63.0;
const GRID_RIGHT_LABEL_X: f32 = 190.0;
const GRID_RIGHT_VALUE_X: f32 = 251.0;
const GRID_ROW_YS: [f32; 4] = [135.0, 155.0, 176.0, 197.0];
const GRID_LABEL_W: f32 = 58.0;
const GRID_VALUE_W: f32 = 98.0;
const GRID_ROW_H: f32 = 15.0;

// job section rows (merchant / hunter / thief, top to bottom). Vanilla puts the
// labels at y 266/284/302 (`GDR_PI_TEXT_MERCHANT` :357, `_HUNTER` :224,
// `_THIEF` :91); the `_GRADE` slots the level number reuses (:338/:205/:72) and
// the icons at y-2 (:433/:300/:167) agree, so three controls confirm each row.
const JOB_ROW_YS: [f32; 3] = [230.0, 248.0, 266.0];

// The two brighter com_bg_tile_b panels behind the stats and job sections
// (`GDR_PI_BG_TILE_B` 16,75,332,173 :1233, `GDR_PI_BG_TILE_B2` 16,260,332,60
// :1214). Named rather than inline so the rebase test below can see them — as
// inline literals they were missed by #310's shift and kept the old origin.
const STATS_BG_RECT: (f32, f32, f32, f32) = (4.0, 39.0, 332.0, 173.0);
const JOB_BG_RECT: (f32, f32, f32, f32) = (4.0, 224.0, 332.0, 60.0);
const JOB_ICON_X: f32 = 5.0;
const JOB_LABEL_X: f32 = 25.0;
const JOB_LABEL_W: f32 = 57.0;
const JOB_LEVEL_X: f32 = 84.0;
const JOB_LEVEL_W: f32 = 32.0;
const JOB_BOARD_X: f32 = 217.0;

const LABEL_COLOR: Color = Color::srgb_u8(239, 218, 164);
const GOLD_COLOR: Color = Color::srgb_u8(255, 217, 83);
const CUR_EXP_COLOR: Color = Color::srgb_u8(178, 235, 96);
const NEXT_EXP_COLOR: Color = Color::srgb_u8(165, 183, 140);
const VALUE_COLOR: Color = Color::srgb(0.95, 0.95, 0.95);
const TITLE_GOLD: Color = Color::srgb_u8(255, 226, 123);

// --- Markers ----------------------------------------------------------------

#[derive(Component)]
pub struct CiWindowRoot;

/// The "Level N" text on the title band.
#[derive(Component)]
pub struct CiLevelText;

/// Which stat a value text displays (one refresh loop repaints them all).
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub enum CiValue {
    CurExp,
    NextExp,
    StatPoints,
    Honor,
    Str,
    Int,
    Hp,
    Mp,
    PhyAtk,
    PhyDef,
    PhyBal,
    Hit,
    MagAtk,
    MagDef,
    MagBal,
    Parry,
    /// Job level rows: 0 = merchant/trader, 1 = hunter, 2 = thief (matching
    /// the vSRO `JobInfo.job_type` 1/2/3 minus one).
    JobLevel(u8),
}

#[derive(Component)]
pub struct CiHpFill;
#[derive(Component)]
pub struct CiMpFill;

/// The + stat buttons (hidden while the wallet is empty).
#[derive(Component)]
pub struct CiAddStr;
#[derive(Component)]
pub struct CiAddInt;

// --- Spawning ---------------------------------------------------------------

/// Spawn the (initially hidden) character info window.
pub fn spawn_character_info_window(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    fonts: Res<FontAssets>,
    ui_strings: Res<ClientUiStrings>,
    vitals: Res<PlayerVitals>,
    cam_query: Query<Entity, With<Camera2d>>,
) {
    let Ok(camera) = cam_query.single() else {
        warn!("character info: no 2d camera to attach to");
        return;
    };
    let s = hud_scale();
    let title = if vitals.name.is_empty() {
        ui_strings
            .get_or("UIIT_STT_CHARACTER", "Character")
            .to_string()
    } else {
        vitals.name.clone()
    };

    let window = game_window::spawn_game_window(
        &mut commands,
        &asset_server,
        &fonts,
        camera,
        &title,
        (CONTENT_W, CONTENT_H),
        (WINDOW_RIGHT, WINDOW_TOP),
        s,
    );
    commands
        .entity(window.root)
        .insert((
            CiWindowRoot,
            GlobalZIndex(55),
            // Shares the MainPopup wndpos slot with the other pages of the
            // original's single frame — see `hud::main_popup`.
            PersistedWindow(WndPosSlot::MainPopup),
        ))
        .entry::<Node>()
        .and_modify(|mut node| node.display = Display::None);
    commands
        .entity(window.expect_close_button())
        .observe(on_close_button);

    let text_font = |size: f32| TextFont {
        font: fonts.two.clone().into(),
        font_size: FontSize::Px(size * s),
        ..default()
    };

    // level readout, right-aligned on the title band (vanilla GDR_PI_TEXT_LEVEL)
    let (outer_w, _) = game_window::outer_size((CONTENT_W, CONTENT_H));
    commands.entity(window.root).with_children(|root| {
        root.spawn((
            CiLevelText,
            Text::new(""),
            text_font(8.5),
            TextColor(TITLE_GOLD),
            TextLayout::justify(Justify::Right),
            abs_node((9.0, 8.0, outer_w - 36.0, 12.0), s),
            Pickable::IGNORE,
        ));
    });

    let plus_style = || ImageButtonStyle {
        normal: asset_server.load("media://interface/ifcommon/com_plus_button.ddj"),
        hover: asset_server.load("media://interface/ifcommon/com_plus_button_focus.ddj"),
        press: asset_server.load("media://interface/ifcommon/com_plus_button_press.ddj"),
        ..Default::default()
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
        let label = |rect, text: String, color: Color, justify: Justify| {
            (
                Text::new(text),
                text_font(8.0),
                TextColor(color),
                TextLayout::justify(justify),
                abs_node(rect, s),
                Pickable::IGNORE,
            )
        };
        let value = |rect, field: CiValue, color: Color, justify: Justify| {
            (
                field,
                Text::new("-"),
                text_font(8.0),
                TextColor(color),
                TextLayout::justify(justify),
                abs_node(rect, s),
                Pickable::IGNORE,
            )
        };
        let ui = |key: &str, fallback: &str| ui_strings.get_or(key, fallback).to_string();

        // --- section backgrounds (spawned first, so everything draws above):
        // vanilla tiles the stats and job areas with the brighter
        // com_bg_tile_b over the chrome's dark _d tiling
        for rect in [STATS_BG_RECT, JOB_BG_RECT] {
            content.spawn((
                abs_node(rect, s),
                ImageNode {
                    image: asset_server
                        .load("media://interface/ifcommon/bg_tile/com_bg_tile_b.ddj"),
                    image_mode: NodeImageMode::Tiled {
                        tile_x: true,
                        tile_y: true,
                        stretch_value: s,
                    },
                    ..default()
                },
                Pickable::IGNORE,
            ));
        }

        // --- exp strip -------------------------------------------------------
        content.spawn(label(
            CUR_EXP_LABEL_RECT,
            ui("UIIT_STT_CURRENT_EXP", "Current Exp."),
            CUR_EXP_COLOR,
            Justify::Left,
        ));
        content.spawn(value(
            CUR_EXP_VALUE_RECT,
            CiValue::CurExp,
            CUR_EXP_COLOR,
            Justify::Center,
        ));
        content.spawn(label(
            NEXT_EXP_LABEL_RECT,
            ui("UIIT_STT_NEXT_EXP", "Next Exp."),
            NEXT_EXP_COLOR,
            Justify::Left,
        ));
        content.spawn(value(
            NEXT_EXP_VALUE_RECT,
            CiValue::NextExp,
            NEXT_EXP_COLOR,
            Justify::Center,
        ));

        // --- dividers + bottom ornament -------------------------------------
        for (rect, piece) in [
            (DIVIDER_TOP_RECT, "chr_window_mid_mid01"),
            (DIVIDER_MID_RECT, "chr_window_mid_mid02"),
            (BOTTOM_DECO_RECT, "chr_window_mid_down"),
        ] {
            content.spawn(img(rect, format!("{ART}{piece}.ddj")));
        }

        // --- stat point / honor row ------------------------------------------
        content.spawn(label(
            STAT_LABEL_RECT,
            ui("UIIT_STT_CURRENT_STAT_POINT", "Stat Point"),
            GOLD_COLOR,
            Justify::Left,
        ));
        content.spawn(value(
            STAT_VALUE_RECT,
            CiValue::StatPoints,
            GOLD_COLOR,
            Justify::Right,
        ));
        content.spawn(label(
            HONOR_LABEL_RECT,
            ui("UIIT_STT_TC_HONOR_POINT", "Honor Point"),
            GOLD_COLOR,
            Justify::Left,
        ));
        content.spawn(value(
            HONOR_VALUE_RECT,
            CiValue::Honor,
            GOLD_COLOR,
            Justify::Right,
        ));

        // --- STR + HP row ----------------------------------------------------
        content.spawn(img(HP_BOARD_RECT, format!("{ART}chr_stat_window.ddj")));
        content.spawn(label(
            STR_LABEL_RECT,
            ui("PARAM_STR", "Str"),
            LABEL_COLOR,
            Justify::Center,
        ));
        content.spawn(value(
            STR_VALUE_RECT,
            CiValue::Str,
            VALUE_COLOR,
            Justify::Center,
        ));
        content.spawn(label(
            HP_LABEL_RECT,
            ui("PARAM_HP", "HP"),
            VALUE_COLOR,
            Justify::Left,
        ));
        // gauge: clipping wrapper, fill becomes a percentage width
        let mut hp_gauge_node = abs_node(HP_GAUGE_RECT, s);
        hp_gauge_node.overflow = Overflow::clip();
        content
            .spawn((hp_gauge_node, Pickable::IGNORE))
            .with_children(|wrap| {
                wrap.spawn((
                    CiHpFill,
                    Node {
                        position_type: PositionType::Absolute,
                        width: Val::Percent(100.0),
                        height: Val::Percent(100.0),
                        overflow: Overflow::clip(),
                        ..default()
                    },
                    Pickable::IGNORE,
                ))
                .with_children(|fill| {
                    fill.spawn((
                        abs_node((0.0, 0.0, HP_GAUGE_RECT.2, HP_GAUGE_RECT.3), s),
                        ImageNode {
                            image: asset_server.load(format!("{ART}chr_hp.ddj")),
                            image_mode: NodeImageMode::Stretch,
                            ..default()
                        },
                        Pickable::IGNORE,
                    ));
                });
            });
        content.spawn(value(
            HP_VALUE_RECT,
            CiValue::Hp,
            VALUE_COLOR,
            Justify::Center,
        ));
        content
            .spawn((
                CiAddStr,
                Button,
                Hovered::default(),
                abs_node(ADD_STR_RECT, s),
                ImageNode {
                    image: asset_server.load("media://interface/ifcommon/com_plus_button.ddj"),
                    image_mode: NodeImageMode::Stretch,
                    ..default()
                },
                plus_style(),
            ))
            .observe(on_add_str);

        // --- INT + MP row ----------------------------------------------------
        content.spawn(img(MP_BOARD_RECT, format!("{ART}chr_stat_window.ddj")));
        content.spawn(label(
            INT_LABEL_RECT,
            ui("PARAM_INT", "Int"),
            LABEL_COLOR,
            Justify::Center,
        ));
        content.spawn(value(
            INT_VALUE_RECT,
            CiValue::Int,
            VALUE_COLOR,
            Justify::Center,
        ));
        content.spawn(label(
            MP_LABEL_RECT,
            ui("PARAM_MP", "MP"),
            VALUE_COLOR,
            Justify::Left,
        ));
        let mut mp_gauge_node = abs_node(MP_GAUGE_RECT, s);
        mp_gauge_node.overflow = Overflow::clip();
        content
            .spawn((mp_gauge_node, Pickable::IGNORE))
            .with_children(|wrap| {
                wrap.spawn((
                    CiMpFill,
                    Node {
                        position_type: PositionType::Absolute,
                        width: Val::Percent(100.0),
                        height: Val::Percent(100.0),
                        overflow: Overflow::clip(),
                        ..default()
                    },
                    Pickable::IGNORE,
                ))
                .with_children(|fill| {
                    fill.spawn((
                        abs_node((0.0, 0.0, MP_GAUGE_RECT.2, MP_GAUGE_RECT.3), s),
                        ImageNode {
                            image: asset_server.load(format!("{ART}chr_mp.ddj")),
                            image_mode: NodeImageMode::Stretch,
                            ..default()
                        },
                        Pickable::IGNORE,
                    ));
                });
            });
        content.spawn(value(
            MP_VALUE_RECT,
            CiValue::Mp,
            VALUE_COLOR,
            Justify::Center,
        ));
        content
            .spawn((
                CiAddInt,
                Button,
                Hovered::default(),
                abs_node(ADD_INT_RECT, s),
                ImageNode {
                    image: asset_server.load("media://interface/ifcommon/com_plus_button.ddj"),
                    image_mode: NodeImageMode::Stretch,
                    ..default()
                },
                plus_style(),
            ))
            .observe(on_add_int);

        // --- combat stat grid ------------------------------------------------
        let grid: [(&str, &str, CiValue, f32, f32, f32); 8] = [
            (
                "UIIT_STT_PHYSICAL_ATTACK",
                "Phy. atk",
                CiValue::PhyAtk,
                GRID_LEFT_LABEL_X,
                GRID_LEFT_VALUE_X,
                GRID_ROW_YS[0],
            ),
            (
                "UIIT_STT_PHYSICAL_DEFENCE",
                "Phy. def.",
                CiValue::PhyDef,
                GRID_LEFT_LABEL_X,
                GRID_LEFT_VALUE_X,
                GRID_ROW_YS[1],
            ),
            (
                "UIIT_STT_PHYSICAL_BALANCE",
                "Phy. balance",
                CiValue::PhyBal,
                GRID_LEFT_LABEL_X,
                GRID_LEFT_VALUE_X,
                GRID_ROW_YS[2],
            ),
            (
                "UIIT_STT_HIT_RATIO",
                "Hit rate",
                CiValue::Hit,
                GRID_LEFT_LABEL_X,
                GRID_LEFT_VALUE_X,
                GRID_ROW_YS[3],
            ),
            (
                "UIIT_STT_MAGICAL_ATTACK",
                "Mag. atk",
                CiValue::MagAtk,
                GRID_RIGHT_LABEL_X,
                GRID_RIGHT_VALUE_X,
                GRID_ROW_YS[0],
            ),
            (
                "UIIT_STT_MAGICAL_DEFENCE",
                "Mag. def.",
                CiValue::MagDef,
                GRID_RIGHT_LABEL_X,
                GRID_RIGHT_VALUE_X,
                GRID_ROW_YS[1],
            ),
            (
                "UIIT_STT_MAGICAL_BALANCE",
                "Mag. balance",
                CiValue::MagBal,
                GRID_RIGHT_LABEL_X,
                GRID_RIGHT_VALUE_X,
                GRID_ROW_YS[2],
            ),
            (
                "UIIT_STT_PARRY_RATIO",
                "Parry ratio",
                CiValue::Parry,
                GRID_RIGHT_LABEL_X,
                GRID_RIGHT_VALUE_X,
                GRID_ROW_YS[3],
            ),
        ];
        for (key, fallback, field, label_x, value_x, y) in grid {
            content.spawn(label(
                (label_x, y, GRID_LABEL_W, GRID_ROW_H),
                ui(key, fallback),
                LABEL_COLOR,
                Justify::Left,
            ));
            content.spawn(value(
                (value_x, y, GRID_VALUE_W, GRID_ROW_H),
                field,
                VALUE_COLOR,
                Justify::Center,
            ));
        }

        // --- job section (merchant / hunter / thief levels) ------------------
        let jobs: [(&str, &str, &str); 3] = [
            ("UIIT_STT_MERCHANT_LEVEL", "Trader Lv.", "com_job_merchant"),
            ("UIIT_STT_HUNTER_LEVEL", "Hunter Lv.", "com_job_hunter"),
            ("UIIT_STT_THIEF_LEVEL", "Thief Lv.", "com_job_thief"),
        ];
        for (row, (key, fallback, icon)) in jobs.into_iter().enumerate() {
            let y = JOB_ROW_YS[row];
            content.spawn(img(
                (JOB_ICON_X, y - 2.0, 16.0, 16.0),
                format!("media://interface/ifcommon/{icon}.ddj"),
            ));
            content.spawn(label(
                (JOB_LABEL_X, y, JOB_LABEL_W, 14.0),
                ui(key, fallback),
                VALUE_COLOR,
                Justify::Left,
            ));
            content.spawn(value(
                (JOB_LEVEL_X, y, JOB_LEVEL_W, 14.0),
                CiValue::JobLevel(row as u8),
                VALUE_COLOR,
                Justify::Right,
            ));
            content.spawn(img(
                (JOB_BOARD_X, y - 2.0, 125.0, 12.0),
                format!("{ART}chr_stat_window02.ddj"),
            ));
        }
    });
}

pub fn cleanup_character_info(mut commands: Commands, windows: Query<Entity, With<CiWindowRoot>>) {
    for entity in windows.iter() {
        commands.entity(entity).despawn();
    }
}

// --- Behavior ---------------------------------------------------------------

fn on_close_button(_: On<Activate>, mut state: ResMut<CharacterInfoState>) {
    state.open = false;
}

fn send_stat_request(conn: &Query<&SilkroadConnection, With<AgentConnection>>, packet: Packet) {
    let Ok(conn) = conn.single() else {
        warn!("character info: not sending stat request, no agent connection");
        return;
    };
    if let Err(e) = conn.get_sender().send(packet.into()) {
        error!("network: failed to send stat request: {}", e.0);
    }
}

fn on_add_str(
    _: On<Activate>,
    stats: Res<PlayerStats>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
) {
    if stats.stat_points == 0 {
        return;
    }
    debug!("character info: requesting STR increase (0x7050)");
    send_stat_request(&conn, Packet::from(IncreaseStrRequest));
}

fn on_add_int(
    _: On<Activate>,
    stats: Res<PlayerStats>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
) {
    if stats.stat_points == 0 {
        return;
    }
    debug!("character info: requesting INT increase (0x7051)");
    send_stat_request(&conn, Packet::from(IncreaseIntRequest));
}

/// Reflect `CharacterInfoState.open` in the window's display.
pub fn apply_character_info_visibility(
    state: Res<CharacterInfoState>,
    mut roots: Query<&mut Node, With<CiWindowRoot>>,
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

/// Run condition: any displayed source changed, or the window was respawned.
pub fn character_info_needs_refresh(
    stats: Res<PlayerStats>,
    vitals: Res<PlayerVitals>,
    progress: Res<PlayerProgress>,
    fresh: Query<(), Added<CiWindowRoot>>,
) -> bool {
    stats.is_changed() || vitals.is_changed() || progress.is_changed() || !fresh.is_empty()
}

/// Repaint every value text, the gauges and the + button enabled state.
#[allow(clippy::too_many_arguments)]
pub fn refresh_character_info(
    stats: Res<PlayerStats>,
    vitals: Res<PlayerVitals>,
    progress: Res<PlayerProgress>,
    level_data: Res<ClientLevelData>,
    asset_server: Res<AssetServer>,
    player: Query<&CharacterInfo, With<Player>>,
    mut values: Query<(&CiValue, &mut Text)>,
    mut level_text: Query<&mut Text, (With<CiLevelText>, Without<CiValue>)>,
    mut fills: Query<(&mut Node, Option<&CiHpFill>), Or<(With<CiHpFill>, With<CiMpFill>)>>,
    mut buttons: Query<
        (Entity, &mut ImageButtonStyle, &mut ImageNode),
        Or<(With<CiAddStr>, With<CiAddInt>)>,
    >,
    mut commands: Commands,
) {
    let sheet = stats.sheet.as_ref();
    let job = player.single().ok().and_then(|info| info.job.as_ref());

    // Compared before writing, like the value fields below already are: a
    // `Text` write re-measures the glyph run and marks the UI tree dirty, so an
    // unconditional one keeps bevy_ui's Taffy layout re-running every frame for
    // a level that changes a few times an hour.
    for mut text in level_text.iter_mut() {
        let new = format!("Level {}", vitals.level);
        if text.0 != new {
            text.0 = new;
        }
    }

    for (field, mut text) in values.iter_mut() {
        let new = match field {
            CiValue::CurExp => progress.exp_offset.to_string(),
            CiValue::NextExp => level_data
                .max_exp(progress.level)
                .map(|exp| exp.to_string())
                .unwrap_or_else(|| "-".into()),
            CiValue::StatPoints => stats.stat_points.to_string(),
            // honor points are not decoded from any packet yet
            CiValue::Honor => "0".into(),
            CiValue::Str => sheet.map_or("-".into(), |s| s.strength.to_string()),
            CiValue::Int => sheet.map_or("-".into(), |s| s.intelligence.to_string()),
            CiValue::Hp => format!("{} / {}", vitals.hp, vitals.max_hp.unwrap_or(vitals.hp)),
            CiValue::Mp => format!("{} / {}", vitals.mp, vitals.max_mp.unwrap_or(vitals.mp)),
            CiValue::PhyAtk => sheet.map_or("-".into(), |s| {
                format!("{} ~ {}", s.phys_attack_min, s.phys_attack_max)
            }),
            CiValue::MagAtk => sheet.map_or("-".into(), |s| {
                format!("{} ~ {}", s.mag_attack_min, s.mag_attack_max)
            }),
            CiValue::PhyDef => sheet.map_or("-".into(), |s| s.phys_defense.to_string()),
            CiValue::MagDef => sheet.map_or("-".into(), |s| s.mag_defense.to_string()),
            // Balance is NOT on the wire (all 492 captured 0x303D bodies are
            // byte-exact 36 bytes), so it is derived: each side measures how
            // close that stat is to `MaxStat = 28 + 4·level`. See
            // `model::balance_percent` for the sourcing and for why the 120%
            // cap stays unapplied.
            CiValue::PhyBal => sheet.map_or("-".into(), |s| {
                balance_text(vitals.level as u32, s.strength)
            }),
            CiValue::MagBal => sheet.map_or("-".into(), |s| {
                balance_text(vitals.level as u32, s.intelligence)
            }),
            CiValue::Hit => sheet.map_or("-".into(), |s| s.hit_rate.to_string()),
            CiValue::Parry => sheet.map_or("-".into(), |s| s.parry_rate.to_string()),
            CiValue::JobLevel(row) => job
                .filter(|j| j.job_type == row + 1)
                .map(|j| j.job_level.to_string())
                .unwrap_or_else(|| "-".into()),
        };
        if text.0 != new {
            text.0 = new;
        }
    }

    let hp_max = vitals.max_hp.unwrap_or(vitals.hp).max(1);
    let mp_max = vitals.max_mp.unwrap_or(vitals.mp).max(1);
    for (mut node, is_hp) in fills.iter_mut() {
        let fill = if is_hp.is_some() {
            vitals.hp as f32 / hp_max as f32
        } else {
            vitals.mp as f32 / mp_max as f32
        };
        // Guarded for the same reason: a `Node` write dirties the layout, and
        // the bar only moves when vitals do.
        let width = Val::Percent(fill.clamp(0.0, 1.0) * 100.0);
        if node.width != width {
            node.width = width;
        }
    }

    // + buttons: grey out (vanilla-style) instead of hiding. The button
    // visuals system repaints the ImageNode from the style every frame, so
    // the style's own handles must switch to the _disable art (same pattern
    // as the underbar page arrows); InteractionDisabled blocks Activate.
    let enabled = stats.stat_points > 0;
    let stem = if enabled {
        "media://interface/ifcommon/com_plus_button"
    } else {
        "media://interface/ifcommon/com_plus_button_disable"
    };
    for (entity, mut style, mut image) in buttons.iter_mut() {
        if enabled {
            style.normal = asset_server.load(format!("{stem}.ddj"));
            style.hover = asset_server.load("media://interface/ifcommon/com_plus_button_focus.ddj");
            style.press = asset_server.load("media://interface/ifcommon/com_plus_button_press.ddj");
            commands.entity(entity).remove::<InteractionDisabled>();
        } else {
            let disable = asset_server.load::<Image>(format!("{stem}.ddj"));
            style.normal = disable.clone();
            style.hover = disable.clone();
            style.press = disable;
            commands.entity(entity).insert(InteractionDisabled);
        }
        image.image = style.normal.clone();
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// The shared shell's content origin — what the layout constants above are
    /// rebased on. Derived from `game_window`'s own exports so a change there
    /// cannot silently desync this window again (#310).
    const ORIGIN_X: f32 = game_window::FRAME_VIS_SIDE + game_window::CHROME_PAD;
    const ORIGIN_Y: f32 = game_window::CONTENT_TOP;

    /// Vanilla rects are tab-local: `GDR_PI_SUBFRAME` spans `0,0,364,356`
    /// (`ifplayerinfo.txt:1385`), the same extent as `GDR_PLAYERINFO`'s own
    /// `13,38,364,356` (`ifmainpopup.txt:55`), so a control's `y` is measured
    /// from the window's top edge and our content space starts `ORIGIN_Y` in.
    #[test]
    fn interior_rects_sit_on_the_shells_content_origin() {
        // (vanilla x, vanilla y, ours x, ours y) from ifplayerinfo.txt:
        // GDR_PI_TEXT_CURXP :1119, GDR_PI_TEXT_STAT :1043,
        // GDR_PI_STAT_WND_HP :1157, GDR_PI_TEXT_STRENGTH :927,
        // GDR_PI_TEXT_PHYATT :737, GDR_PI_DECO_MID_DOWN :1251,
        // GDR_PI_BG_TILE_B :1233, GDR_PI_BG_TILE_B2 :1214,
        // GDR_PI_TEXT_MERCHANT :357, GDR_PI_TEXT_THIEF :91.
        let cases = [
            (32.0, 41.0, CUR_EXP_LABEL_RECT.0, CUR_EXP_LABEL_RECT.1),
            (17.0, 85.0, STAT_LABEL_RECT.0, STAT_LABEL_RECT.1),
            (15.0, 107.0, HP_BOARD_RECT.0, HP_BOARD_RECT.1),
            (17.0, 113.0, STR_LABEL_RECT.0, STR_LABEL_RECT.1),
            (14.0, 171.0, GRID_LEFT_LABEL_X, GRID_ROW_YS[0]),
            (16.0, 320.0, BOTTOM_DECO_RECT.0, BOTTOM_DECO_RECT.1),
            // The section backdrops — inline literals until they were missed by
            // the #310 shift, so they are pinned here now.
            (16.0, 75.0, STATS_BG_RECT.0, STATS_BG_RECT.1),
            (16.0, 260.0, JOB_BG_RECT.0, JOB_BG_RECT.1),
            (37.0, 266.0, JOB_LABEL_X, JOB_ROW_YS[0]),
            (37.0, 302.0, JOB_LABEL_X, JOB_ROW_YS[2]),
        ];
        for (vanilla_x, vanilla_y, our_x, our_y) in cases {
            assert_eq!(our_x, vanilla_x - ORIGIN_X, "x of vanilla {vanilla_x}");
            assert_eq!(our_y, vanilla_y - ORIGIN_Y, "y of vanilla {vanilla_y}");
        }
    }

    /// `ifmainpopup.txt:55` — `GDR_PLAYERINFO` (ID 75) is `13,38,364,356`, so
    /// the shell must wrap our content in exactly 364x356.
    #[test]
    fn outer_window_matches_the_vanilla_tab_rect() {
        assert_eq!(
            game_window::outer_size((CONTENT_W, CONTENT_H)),
            (364.0, 356.0)
        );
    }
}
