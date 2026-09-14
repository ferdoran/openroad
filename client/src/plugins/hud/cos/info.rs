//! COS info page — `resinfo/ifcosinfo.txt`, the first of the shell's three
//! pages (`GDR_COS_INFO:CIFCOSInfo`, id 121).
//!
//! Idea: the shell (`ui.rs`) already hosts three empty page containers; this
//! fills the first one from the descriptor rather than from a design. Every
//! rect, art path and string key below is transcribed from the user's own
//! `ifcosinfo.txt` (35 blocks, all of them here), and each constant names the
//! `GDR_*` block it came from so the transcription can be re-checked against
//! the data. Rects are in the page's own space — the page container is already
//! placed at vanilla's `12,66,331,314`, so nothing here is rebased again.
//!
//! The file carries one preprocessor branch, on `GDR_COS_INFO_NONE`:
//! `UI_UPDATE_2009_FIRST` is defined (`Media/config/define.txt:17`), so the
//! live rect is `68,191,194,11` with `HAlign=1`, not the `#else` arm's
//! `94,191,194,11`.
//!
//! **What is filled and what is deliberately blank.** Vanilla ships the eight
//! stat statics and the two time statics with `Text=STRING,""` — they are
//! server-filled. Name, **level, EXP and HGP** all come off `0x30C8`'s growth
//! block (see [`packets::agent::pet::CosGrowth`], whose three fields were `[U]`
//! by name until the 2026-08-19 Grey Wolf capture); HP is the entity's own
//! vitals over characterdata `MaxHP`.
//!
//! The six combat stats and the rent timer stay empty, and the reason is now
//! settled rather than assumed: **no COS packet carries them.** `0x30C8`'s body
//! is fully accounted for field by field by two independent sources, `0x30C9`'s
//! seven arms are all decoded, and `0x30CA` is a `u8` mask plus up to two state
//! bytes (`docs/re/net/inbound/pet-cos.md`). characterdata cannot stand in
//! either: `_RefObjChar` has PD/MD/PAR/MAR/ER/BR/HR/CHR but **no attack
//! columns at all**, so four of the six cells would be base defense values and
//! the other two would have nothing behind them. `GDR_COS_INFO_NONE`
//! (`UIIT_MSG_COS_NOT_ABILITY`) is shown instead whenever no COS is summoned,
//! which is the block's own purpose.
//!
//! Art sizes for the four `w=h=0` controls are measured from the user's PK2
//! (a `w=h=0` rect means "size to the art"): `pt_messagebox` 200x24,
//! `pt_time` 280x28, `pt_stat_window` 284x24, `com_mall_button` 76x24.

use bevy::prelude::*;

use crate::assets::FontAssets;
use crate::plugins::cos::ActiveCosList;
use crate::plugins::hud::cos::state::{Cos, CosState};
use crate::plugins::hud::game_window::abs_node;
use crate::plugins::net::entities::{EntityVitals, NetworkEntities};
use crate::plugins::textdata::{ClientCharacterData, ClientLevelData, ClientUiStrings};

// --- Colours, straight off each block's FontColor ----------------------------

/// `FontColor=COLOR,"255,240,217,168"` — the eight stat captions.
const CAPTION_GOLD: Color = Color::srgb_u8(240, 217, 168);
/// `FontColor=COLOR,"255,255,255,255"` — every value static and the gauge
/// captions.
const VALUE_WHITE: Color = Color::WHITE;
/// `FontColor=COLOR,"255,153,153,153"` — `GDR_COS_INFO_NONE` only.
const NONE_GREY: Color = Color::srgb_u8(153, 153, 153);

const ART: &str = "media://interface/";

// --- Rects (page space), one per GDR_* block ---------------------------------

/// `GDR_COS_GFRAME:CIFFrame` id 10 — the `frameg_wnd_` ring around the page.
const GFRAME_RECT: (f32, f32, f32, f32) = (8.0, 9.0, 315.0, 297.0);
/// The `frameg_wnd_` pieces, measured from the user's PK2: corners 24x16,
/// `mid_*` 128x16, `*_side` 24x52 — so the ring is 24px at the sides and 16px
/// top and bottom.
const GFRAME_SIDE: f32 = 24.0;
const GFRAME_TOP: f32 = 16.0;
/// `GDR_COS_BG_01:CIFNormalTile` id 5 — `com_bg_tile_b` fill inside it.
const BG_RECT: (f32, f32, f32, f32) = (32.0, 25.0, 267.0, 265.0);
/// `GDR_COS_INFO_BLACKBOX_NAME:CIFStatic` id 25, `pt_messagebox.ddj` (200x24).
const NAME_BOX_RECT: (f32, f32, f32, f32) = (25.0, 41.0, 200.0, 24.0);
/// `GDR_COS_INFO_NAME:CIFStatic` id 27, `HAlign=1` (centred).
const NAME_RECT: (f32, f32, f32, f32) = (45.0, 47.0, 164.0, 11.0);
/// `GDR_COS_INFO_NAME_BTN:CIFButton` id 26, `com_mall_button.ddj` (76x24).
const NAME_BTN_RECT: (f32, f32, f32, f32) = (232.0, 41.0, 76.0, 24.0);
/// `GDR_COS_INFO_TIME_WND:CIFStatic` id 50, `pt_time.ddj` (280x28).
const TIME_BOX_RECT: (f32, f32, f32, f32) = (26.0, 80.0, 280.0, 28.0);
/// `GDR_COS_INFO_TIME_STA` id 51 (`HAlign=2`, right) and `_TIME` id 52.
const TIME_CAPTION_RECT: (f32, f32, f32, f32) = (29.0, 88.0, 63.0, 11.0);
const TIME_VALUE_RECT: (f32, f32, f32, f32) = (106.0, 88.0, 188.0, 11.0);

/// The three gauge rows. Each row is a `pt_stat_window.ddj` plate (284x24, the
/// `w=h=0` `_STAT_*_WND` blocks), a right-aligned caption, a 228x12 gauge and a
/// centred percentage static. Vanilla's y values differ per row by design —
/// they are transcribed, not derived from a pitch.
struct GaugeRow {
    /// `GDR_COS_INFO_STAT_*_WND` id 30/31/32.
    plate: (f32, f32, f32, f32),
    /// `GDR_COS_INFO_*_STA` id 40/42/44, `HAlign=2`.
    caption: (f32, f32, f32, f32),
    /// `GDR_COS_INFO_*_GAUGE:CIFGauge` id 35/36/37.
    gauge: (f32, f32, f32, f32),
    /// `GDR_COS_INFO_*_PERCENT` id 41/43/45, `HAlign=1`.
    percent: (f32, f32, f32, f32),
    /// The gauge's own `DDJ=` art.
    art: &'static str,
    /// The caption's `Text=` key, with an English fallback.
    key: (&'static str, &'static str),
}

const HP_ROW: GaugeRow = GaugeRow {
    plate: (24.0, 83.0, 284.0, 24.0),
    caption: (36.0, 91.0, 28.0, 11.0),
    gauge: (75.0, 89.0, 228.0, 12.0),
    percent: (82.0, 90.0, 214.0, 11.0),
    art: "pet/pt_hp.ddj",
    key: ("PARAM_HP", "HP"),
};
const HGP_ROW: GaugeRow = GaugeRow {
    plate: (24.0, 113.0, 284.0, 24.0),
    caption: (36.0, 121.0, 28.0, 11.0),
    gauge: (75.0, 119.0, 228.0, 12.0),
    percent: (82.0, 121.0, 214.0, 11.0),
    art: "pet/pt_hgp.ddj",
    key: ("UIIT_STT_COSNEWUI_BASICINFO_HGP", "HGP"),
};
const EXP_ROW: GaugeRow = GaugeRow {
    plate: (24.0, 142.0, 284.0, 24.0),
    caption: (36.0, 149.0, 28.0, 11.0),
    gauge: (75.0, 148.0, 228.0, 12.0),
    percent: (82.0, 149.0, 214.0, 11.0),
    art: "pet/pt_exp.ddj",
    key: ("UIO_CHARINFO_STT_EXP", "EXP"),
};

/// `GDR_COS_INFO_NONE:CIFStatic` id 100 — the "this COS has no such ability"
/// line, live arm (`UI_UPDATE_2009_FIRST`).
const NONE_RECT: (f32, f32, f32, f32) = (68.0, 191.0, 194.0, 11.0);

/// The 4x2 stat grid: caption (`*_STA`) then value (`*_DATA`), in the file's
/// own order. Columns are at x=36/108 and x=174/246, rows at y=198/218/238/258.
struct StatCell {
    caption: (f32, f32, f32, f32),
    value: (f32, f32, f32, f32),
    key: (&'static str, &'static str),
    field: CosInfoField,
}

const STAT_CELLS: [StatCell; 8] = [
    // ids 60/65
    StatCell {
        caption: (36.0, 198.0, 59.0, 11.0),
        value: (108.0, 198.0, 59.0, 11.0),
        key: ("UIIT_CTL_COSNEWUI_PETINFO_LEVEL", "Level"),
        field: CosInfoField::Level,
    },
    // ids 61/66
    StatCell {
        caption: (36.0, 218.0, 59.0, 11.0),
        value: (108.0, 218.0, 59.0, 11.0),
        key: ("UIIT_STT_HIT_RATIO", "Hit ratio"),
        field: CosInfoField::HitRatio,
    },
    // ids 62/67
    StatCell {
        caption: (36.0, 238.0, 59.0, 11.0),
        value: (108.0, 238.0, 59.0, 11.0),
        key: ("UIIT_STT_PHYSICAL_ATTACK", "Phy. attack"),
        field: CosInfoField::PhysicalAttack,
    },
    // ids 63/68
    StatCell {
        caption: (36.0, 258.0, 59.0, 11.0),
        value: (108.0, 258.0, 59.0, 11.0),
        key: ("UIIT_STT_PHYSICAL_DEFENCE", "Phy. defense"),
        field: CosInfoField::PhysicalDefense,
    },
    // ids 71/76
    StatCell {
        caption: (174.0, 218.0, 59.0, 11.0),
        value: (246.0, 218.0, 59.0, 11.0),
        key: ("UIIT_STT_PARRY_RATIO", "Parry ratio"),
        field: CosInfoField::ParryRatio,
    },
    // ids 72/77
    StatCell {
        caption: (174.0, 238.0, 59.0, 11.0),
        value: (246.0, 238.0, 59.0, 11.0),
        key: ("UIIT_STT_MAGICAL_ATTACK", "Mag. attack"),
        field: CosInfoField::MagicalAttack,
    },
    // ids 73/78
    StatCell {
        caption: (174.0, 258.0, 59.0, 11.0),
        value: (246.0, 258.0, 59.0, 11.0),
        key: ("UIIT_STT_MAGICAL_DEFENCE", "Mag. defense"),
        field: CosInfoField::MagicalDefense,
    },
    // the file has no caption at (174,198): vanilla leaves that cell empty, so
    // the grid is 7 stats in a 4x2 frame. Kept as an explicit hole rather than
    // being closed up, because closing it would move six transcribed rects.
    StatCell {
        caption: (174.0, 198.0, 0.0, 0.0),
        value: (246.0, 198.0, 0.0, 0.0),
        key: ("", ""),
        field: CosInfoField::Unused,
    },
];

// --- Markers ----------------------------------------------------------------

/// A value static the state can write into.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CosInfoField {
    /// `GDR_COS_INFO_NAME` — the pet's own name (`0x30C8` body).
    Name,
    /// `GDR_COS_INFO_LEVEL_DATA` — the pet's own level from `0x30C8`'s growth
    /// block, falling back to characterdata `Lvl` for the kinds that carry no
    /// block. It used to read characterdata unconditionally, which is the
    /// *growth stage's* base level: the two agree on the ladder rows (a wolf at
    /// `COS_P_WOLF_005` is level 5 in both), but only the wire is right while a
    /// stage swap is in flight, and only the wire exists at all before one.
    Level,
    /// `GDR_COS_INFO_HGP_PERCENT` — `0x30C8`'s growth block, then `0x30C9`
    /// arm 4. Per-10,000, see [`super::state::HGP_FULL`].
    HgpPercent,
    /// `GDR_COS_INFO_HP_PERCENT` — the entity's own vitals over the
    /// characterdata `MaxHP` of the resolved (growth-stage) ref id.
    HpPercent,
    /// `GDR_COS_INFO_EXP_PERCENT` — `Cos::exp` over the leveldata column-1
    /// threshold at the pet's level. **`[V]`.**
    ///
    /// It shipped `[S]` first, reasoned from the binary: the original's pet
    /// level-up loop reads `FUN_00937f20(pet_level)`, which
    /// `docs/re/net/inbound/misc-debug.md:618` identifies as the `CRefLevel`
    /// (= `leveldata.txt`) row accessor, and round 2
    /// (`docs/re/round2/gamedata.md` G6a) reconstructs that record as
    /// `vtable | int Level | __int64 Exp | int×7` — putting the `__int64 Exp`
    /// at `+0x8` on **column 1**, the same curve a character levels on.
    ///
    /// The playtest then settled it. `packet_dump/0x30c9.log`'s 2026-08-19T22:25
    /// session replays exactly: seven exp deltas from the capture's own seed
    /// reproduce all three arm-7 model swaps (stages 3, 4, 5), and both rival
    /// readings die on the first kill — leveldata column 5 lands on stage 8, and
    /// resetting exp to 0 per level lands on stage 2. See
    /// [`super::state::apply_pet_exp_gain`], where that replay is a test.
    ExpPercent,
    /// Blank, and settled: **no COS packet carries the combat stats.** See the
    /// module header — 0x30C8's body is accounted for field by field, 0x30C9's
    /// seven arms are all decoded, 0x30CA is a state mask, and characterdata
    /// has no attack columns to substitute with.
    HitRatio,
    PhysicalAttack,
    PhysicalDefense,
    ParryRatio,
    MagicalAttack,
    MagicalDefense,
    /// The empty cell in the 4x2 grid — never spawned.
    Unused,
}

/// The `GDR_COS_INFO_NONE` line, shown while nothing is summoned.
#[derive(Component)]
pub struct CosInfoNoPet;

/// The HGP gauge's fill node (width = HGP fraction).
#[derive(Component)]
pub struct CosHgpFill;

/// The HP gauge's fill node.
#[derive(Component)]
pub struct CosHpFill;

/// The EXP gauge's fill node.
#[derive(Component)]
pub struct CosExpFill;

// --- Building ---------------------------------------------------------------

/// Fill the `CosPage::Info` container. Called from `ui::spawn_cos_window` with
/// the page entity's child builder, so the page keeps owning its children.
pub fn build_info_page(
    page: &mut ChildSpawnerCommands,
    asset_server: &AssetServer,
    fonts: &FontAssets,
    ui_strings: &ClientUiStrings,
    s: f32,
) {
    let text_font = TextFont {
        font: fonts.two.clone().into(),
        font_size: FontSize::Px(8.0 * s),
        ..default()
    };
    let img = |rect: (f32, f32, f32, f32), path: String, mode: NodeImageMode| {
        (
            abs_node(rect, s),
            ImageNode {
                image: asset_server.load(path),
                image_mode: mode,
                ..default()
            },
            Pickable::IGNORE,
        )
    };
    let label = |rect, text: String, color: Color, justify: Justify| {
        (
            Text::new(text),
            text_font.clone(),
            TextColor(color),
            TextLayout::justify(justify),
            abs_node(rect, s),
            Pickable::IGNORE,
        )
    };
    let ui = |key: &str, fallback: &str| ui_strings.get_or(key, fallback).to_string();

    // GDR_COS_BG_01 first, then the frame ring over it
    page.spawn(img(
        BG_RECT,
        format!("{ART}ifcommon/bg_tile/com_bg_tile_b.ddj"),
        NodeImageMode::Tiled {
            tile_x: true,
            tile_y: true,
            stretch_value: s,
        },
    ));
    // GDR_COS_GFRAME's DDJ is the `frameg_wnd_` PREFIX, i.e. a 9-slice: the
    // eight pieces measure 24x16 (corners), 128x16 (mid) and 24x52 (sides) in
    // the user's PK2, so the ring's border is 24px wide and 16px tall.
    let (fx, fy, fw, fh) = GFRAME_RECT;
    for ((x, y, w, h), piece) in [
        ((0.0, 0.0, GFRAME_SIDE, GFRAME_TOP), "left_up"),
        (
            (GFRAME_SIDE, 0.0, fw - 2.0 * GFRAME_SIDE, GFRAME_TOP),
            "mid_up",
        ),
        ((fw - GFRAME_SIDE, 0.0, GFRAME_SIDE, GFRAME_TOP), "right_up"),
        (
            (0.0, GFRAME_TOP, GFRAME_SIDE, fh - 2.0 * GFRAME_TOP),
            "left_side",
        ),
        (
            (
                fw - GFRAME_SIDE,
                GFRAME_TOP,
                GFRAME_SIDE,
                fh - 2.0 * GFRAME_TOP,
            ),
            "right_side",
        ),
        ((0.0, fh - GFRAME_TOP, GFRAME_SIDE, GFRAME_TOP), "left_down"),
        (
            (
                GFRAME_SIDE,
                fh - GFRAME_TOP,
                fw - 2.0 * GFRAME_SIDE,
                GFRAME_TOP,
            ),
            "mid_down",
        ),
        (
            (fw - GFRAME_SIDE, fh - GFRAME_TOP, GFRAME_SIDE, GFRAME_TOP),
            "right_down",
        ),
    ] {
        page.spawn(img(
            (fx + x, fy + y, w, h),
            format!("{ART}frame/frameg_wnd_{piece}.ddj"),
            NodeImageMode::Stretch,
        ));
    }

    // name box + centred name + the "give it a name" button
    page.spawn(img(
        NAME_BOX_RECT,
        format!("{ART}pet/pt_messagebox.ddj"),
        NodeImageMode::Stretch,
    ));
    page.spawn((
        CosInfoField::Name,
        label(NAME_RECT, String::new(), VALUE_WHITE, Justify::Center),
    ));
    page.spawn(img(
        NAME_BTN_RECT,
        format!("{ART}ifcommon/com_mall_button.ddj"),
        NodeImageMode::Stretch,
    ));
    page.spawn(label(
        NAME_BTN_RECT,
        ui("UIIT_STT_COSNEWUI_BASICINFO_MAKENAME", "Name"),
        Color::srgb_u8(255, 245, 218),
        Justify::Center,
    ));

    // rent timer plate: caption right-aligned, value centred, both empty
    page.spawn(img(
        TIME_BOX_RECT,
        format!("{ART}pet/pt_time.ddj"),
        NodeImageMode::Stretch,
    ));
    page.spawn(label(
        TIME_CAPTION_RECT,
        ui("UIIT_STT_COSNEWUI_RENTTIME", "Rent time"),
        VALUE_WHITE,
        Justify::Right,
    ));
    page.spawn(label(
        TIME_VALUE_RECT,
        String::new(),
        VALUE_WHITE,
        Justify::Center,
    ));

    // the three gauge rows
    for row in [HP_ROW, HGP_ROW, EXP_ROW] {
        page.spawn(img(
            row.plate,
            format!("{ART}pet/pt_stat_window.ddj"),
            NodeImageMode::Stretch,
        ));
        page.spawn(label(
            row.caption,
            ui(row.key.0, row.key.1),
            VALUE_WHITE,
            Justify::Right,
        ));
        // CIFGauge Style=0 draws its art 1:1 and crops along X, so the fill is
        // a clipping wrapper over a full-width image, never a stretched one.
        let mut gauge_node = abs_node(row.gauge, s);
        gauge_node.overflow = Overflow::clip();
        let is_hgp = row.art == HGP_ROW.art;
        page.spawn((gauge_node, Pickable::IGNORE))
            .with_children(|wrap| {
                let fill = Node {
                    position_type: PositionType::Absolute,
                    width: Val::Percent(0.0),
                    height: Val::Percent(100.0),
                    overflow: Overflow::clip(),
                    ..default()
                };
                let mut fill = wrap.spawn((fill, Pickable::IGNORE));
                if is_hgp {
                    fill.insert(CosHgpFill);
                } else if row.art == HP_ROW.art {
                    fill.insert(CosHpFill);
                } else {
                    fill.insert(CosExpFill);
                }
                fill.with_children(|fill| {
                    fill.spawn(img(
                        (0.0, 0.0, row.gauge.2, row.gauge.3),
                        format!("{ART}{}", row.art),
                        NodeImageMode::Stretch,
                    ));
                });
            });
        let mut percent = page.spawn(label(
            row.percent,
            String::new(),
            VALUE_WHITE,
            Justify::Center,
        ));
        if is_hgp {
            percent.insert(CosInfoField::HgpPercent);
        } else if row.art == HP_ROW.art {
            percent.insert(CosInfoField::HpPercent);
        } else {
            percent.insert(CosInfoField::ExpPercent);
        }
    }

    // the 4x2 stat grid (7 stats, one empty cell)
    for cell in STAT_CELLS {
        if cell.field == CosInfoField::Unused {
            continue;
        }
        page.spawn(label(
            cell.caption,
            ui(cell.key.0, cell.key.1),
            CAPTION_GOLD,
            Justify::Left,
        ));
        page.spawn((
            cell.field,
            label(cell.value, String::new(), VALUE_WHITE, Justify::Left),
        ));
    }

    // GDR_COS_INFO_NONE, centred (HAlign=1), shown while nothing is summoned
    page.spawn((
        CosInfoNoPet,
        label(
            NONE_RECT,
            ui("UIIT_MSG_COS_NOT_ABILITY", "This COS has no such ability."),
            NONE_GREY,
            Justify::Center,
        ),
    ));
}

// --- Repaint ----------------------------------------------------------------

/// Mirror the summoned pet onto the page's statics and gauges.
///
/// HP is not gated on `CosState` changing: it follows the entity's own vitals,
/// which move with no COS packet in sight.
#[allow(clippy::type_complexity)]
pub fn apply_cos_info(
    state: Res<CosState>,
    char_data: Res<ClientCharacterData>,
    level_data: Res<ClientLevelData>,
    ui_strings: Res<ClientUiStrings>,
    list: Res<ActiveCosList>,
    index: Res<NetworkEntities>,
    vitals: Query<&EntityVitals>,
    mut values: Query<(&CosInfoField, &mut Text)>,
    mut none_line: Query<
        &mut Node,
        (
            With<CosInfoNoPet>,
            Without<CosHgpFill>,
            Without<CosHpFill>,
            Without<CosExpFill>,
        ),
    >,
    mut hgp_fill: Query<
        &mut Node,
        (
            With<CosHgpFill>,
            Without<CosInfoNoPet>,
            Without<CosHpFill>,
            Without<CosExpFill>,
        ),
    >,
    mut hp_fill: Query<
        &mut Node,
        (
            With<CosHpFill>,
            Without<CosInfoNoPet>,
            Without<CosHgpFill>,
            Without<CosExpFill>,
        ),
    >,
    mut exp_fill: Query<
        &mut Node,
        (
            With<CosExpFill>,
            Without<CosInfoNoPet>,
            Without<CosHgpFill>,
            Without<CosHpFill>,
        ),
    >,
) {
    let cos = state.active_pet();
    // Live HP over the growth stage's own characterdata maximum, with the
    // summon-time seed as the fallback while the entity has not spawned.
    let hp_fraction = cos
        .map(|pet| {
            index
                .get(pet.unique_id)
                .and_then(|entity| vitals.get(entity).ok())
                .map(EntityVitals::fill)
                .or_else(|| {
                    list.get(pet.unique_id)
                        .map(|status| status.hp as f32 / status.hp_max.max(1) as f32)
                })
                .unwrap_or(1.0)
                .clamp(0.0, 1.0)
        })
        .unwrap_or(0.0);
    for mut node in hp_fill.iter_mut() {
        let width = Val::Percent(hp_fraction * 100.0);
        if node.width != width {
            node.width = width;
        }
    }

    if !state.is_changed() {
        return;
    }
    let level = cos.and_then(|cos| pet_level(cos, &char_data));
    let exp_fraction = cos.and_then(|cos| exp_fraction(cos, level, &level_data));
    for (field, mut text) in values.iter_mut() {
        let value = match (field, cos) {
            (_, None) => String::new(),
            // Vanilla's own placeholder, not a blank: an unnamed pet reads
            // "No name" here, the same string the original defaults this plate
            // to (`crate::plugins::cos::spawn::COS_NO_NAME_KEY`).
            (CosInfoField::Name, Some(cos)) => {
                crate::plugins::cos::spawn::pet_display_name(cos.name(), &ui_strings)
            }
            (CosInfoField::Level, Some(_)) => {
                level.map(|level| level.to_string()).unwrap_or_default()
            }
            (CosInfoField::HgpPercent, Some(cos)) => cos
                .hgp_fraction()
                .map(|f| format!("{:.0}%", f * 100.0))
                .unwrap_or_default(),
            (CosInfoField::HpPercent, Some(_)) => format!("{:.0}%", hp_fraction * 100.0),
            (CosInfoField::ExpPercent, Some(_)) => exp_fraction
                .map(|f| format!("{:.0}%", f * 100.0))
                .unwrap_or_default(),
            // the six combat stats: on no packet, see `CosInfoField::HitRatio`
            _ => String::new(),
        };
        if text.0 != value {
            text.0 = value;
        }
    }
    for mut node in none_line.iter_mut() {
        node.display = if cos.is_some() {
            Display::None
        } else {
            Display::Flex
        };
    }
    for mut node in hgp_fill.iter_mut() {
        let fraction = cos.and_then(|cos| cos.hgp_fraction()).unwrap_or(0.0);
        node.width = Val::Percent(fraction * 100.0);
    }
    for mut node in exp_fill.iter_mut() {
        node.width = Val::Percent(exp_fraction.unwrap_or(0.0) * 100.0);
    }
}

/// The pet's level: the wire's own value, or the growth stage's characterdata
/// `Lvl` for the kinds that carry no growth block.
fn pet_level(cos: &Cos, char_data: &ClientCharacterData) -> Option<u32> {
    cos.level.map(u32::from).or_else(|| {
        char_data
            .get(&(cos.ref_obj_id as i32))
            .and_then(|row| row.level())
    })
}

/// Progress through the current level, `0.0..=1.0`.
///
/// `Cos::exp` is already a *within-level* offset — the arm-3 loop
/// ([`super::state::apply_pet_exp_gain`]) subtracts each threshold as it is
/// crossed, so this is a plain division rather than a walk down the curve. The
/// divisor's provenance is on [`CosInfoField::ExpPercent`].
///
/// **Blank past the end of the table**, rather than a bar stuck full: a pet over
/// the last leveldata row has no threshold to be a fraction of. The clamp still
/// earns its place — the loop stops there too, so the surplus keeps piling up.
fn exp_fraction(cos: &Cos, level: Option<u32>, level_data: &ClientLevelData) -> Option<f32> {
    let level = u8::try_from(level?).ok()?;
    let required = level_data.max_exp(level)?;
    if required == 0 {
        return None;
    }
    Some((cos.exp as f64 / required as f64).clamp(0.0, 1.0) as f32)
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::assets::textdata::leveldata::LevelData;
    use packets::agent::pet::{CosBody, CosGrowth, CosKind};

    fn wolf(exp: u64, level: Option<u8>, hgp: Option<u16>) -> Cos {
        Cos {
            unique_id: 0x0002_6cd1,
            ref_obj_id: 6106,
            kind: CosKind::GrowthPet,
            body: CosBody {
                hp: 360,
                unk_b: 0,
                growth: level.map(|level| CosGrowth {
                    exp,
                    level,
                    hgp: hgp.unwrap_or(0),
                }),
                unk_f: Some(0),
                name: Some(String::new()),
                inventory_size: 0,
                items: Vec::new(),
                unk_g: None,
                unk_h: None,
            },
            hgp,
            exp,
            level,
        }
    }

    /// leveldata column 1 for the first few levels, from the user's own
    /// `Media.pk2` table (row "1 118" = 118 exp to go 1 -> 2).
    fn leveldata() -> ClientLevelData {
        let mut data = LevelData::default();
        for (level, exp) in [(1u8, 118u64), (2, 320), (3, 1058)] {
            data.exp.insert(level, exp);
        }
        ClientLevelData::from_data(data)
    }

    /// The captured Grey Wolf, end to end: 77 exp at level 1 against the 118
    /// the client's own leveldata asks for. All three numbers come off one
    /// packet, which is the whole point of the change.
    #[test]
    fn the_captured_wolf_reads_level_one_at_sixty_five_percent() {
        let cos = wolf(77, Some(1), Some(9_932));
        let level = pet_level(&cos, &ClientCharacterData::default());
        assert_eq!(level, Some(1));

        let fraction = exp_fraction(&cos, level, &leveldata()).expect("level 1 has a threshold");
        assert_eq!(format!("{:.0}%", fraction * 100.0), "65%");
        assert_eq!(
            format!("{:.0}%", cos.hgp_fraction().unwrap() * 100.0),
            "99%"
        );
    }

    /// The arm-3 loop keeps `exp` under the level's threshold, so a fraction
    /// over 1.0 should be unreachable — but the loop stops at the end of the
    /// table and on a server whose rates outrun the curve, so the gauge draws
    /// full rather than overdrawing its own track.
    #[test]
    fn an_offset_over_the_threshold_pins_the_gauge_at_full() {
        let cos = wolf(1_180, Some(1), None);
        assert_eq!(exp_fraction(&cos, Some(1), &leveldata()), Some(1.0));
    }

    /// Past the end of the table there is no threshold to be a fraction of, so
    /// the gauge goes blank rather than sitting full forever.
    #[test]
    fn a_level_off_the_end_of_the_table_leaves_the_gauge_blank() {
        let cos = wolf(500, Some(140), None);
        assert_eq!(exp_fraction(&cos, Some(140), &leveldata()), None);
    }

    /// The kinds with no growth block fall back to the growth stage's own
    /// characterdata level, which is what every kind used to do.
    #[test]
    fn a_kind_without_a_growth_block_falls_back_to_characterdata() {
        let mut cos = wolf(0, None, None);
        cos.kind = CosKind::GrabPet;
        // an empty table -> no row, so no level rather than a made-up one
        assert_eq!(pet_level(&cos, &ClientCharacterData::default()), None);
        assert_eq!(exp_fraction(&cos, None, &leveldata()), None);
    }

    /// Every rect this page draws is inside vanilla's page control
    /// (`ifcos.txt`, `12,66,331,314` — the page's own space is 331x314).
    #[test]
    fn every_transcribed_rect_fits_the_page() {
        let page = (331.0, 314.0);
        let mut rects = vec![
            GFRAME_RECT,
            BG_RECT,
            NAME_BOX_RECT,
            NAME_RECT,
            NAME_BTN_RECT,
            TIME_BOX_RECT,
            TIME_CAPTION_RECT,
            TIME_VALUE_RECT,
            NONE_RECT,
        ];
        for row in [HP_ROW, HGP_ROW, EXP_ROW] {
            rects.extend([row.plate, row.caption, row.gauge, row.percent]);
        }
        for cell in STAT_CELLS {
            rects.extend([cell.caption, cell.value]);
        }
        for (x, y, w, h) in rects {
            assert!(x + w <= page.0, "{x}+{w} overflows the page width");
            assert!(y + h <= page.1, "{y}+{h} overflows the page height");
        }
    }

    /// The gauge rects are byte-identical to their art (228x12), which is what
    /// makes `CIFGauge Style=0`'s 1:1-and-crop reading applicable here.
    #[test]
    fn gauges_are_the_size_of_their_art() {
        for row in [HP_ROW, HGP_ROW, EXP_ROW] {
            assert_eq!((row.gauge.2, row.gauge.3), (228.0, 12.0), "{}", row.art);
        }
    }

    /// The stat grid is two columns of captions/values at vanilla's own x
    /// positions, four rows 20px apart — asserted so a later edit cannot
    /// silently re-space it.
    #[test]
    fn the_stat_grid_keeps_the_authored_columns_and_pitch() {
        let ys: Vec<f32> = STAT_CELLS
            .iter()
            .filter(|c| c.caption.0 == 36.0)
            .map(|c| c.caption.1)
            .collect();
        assert_eq!(ys, vec![198.0, 218.0, 238.0, 258.0]);
        for cell in STAT_CELLS {
            assert!(
                (cell.caption.0, cell.value.0) == (36.0, 108.0)
                    || (cell.caption.0, cell.value.0) == (174.0, 246.0),
                "{:?} is not on an authored column",
                cell.field
            );
        }
    }

    /// Vanilla authors seven stats in a 4x2 frame; the eighth cell carries no
    /// block at all, so it must stay unspawned rather than being filled.
    #[test]
    fn the_grid_has_one_authored_hole() {
        let unused = STAT_CELLS
            .iter()
            .filter(|c| c.field == CosInfoField::Unused)
            .count();
        assert_eq!(unused, 1);
        assert_eq!(STAT_CELLS.len() - unused, 7);
    }
}
