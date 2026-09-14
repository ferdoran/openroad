//! Auto-potion window (`GDR_AUTO_POTION`) layout + refresh — **read-only**.
//!
//! Idea: every rect below is transcribed from the vanilla
//! `resinfo/ifautopotion.txt` (17 controls in three sections, window space
//! 394x520 per `ginterface.txt:1500`) and from `resinfo/ifautopotionslot.txt`
//! (the 13-control `CIFAutoPotionSlot` template, instantiated twice — HP and
//! MP). Window-space rects are rebased into the shared `hud::game_window`
//! shell's content space by [`content_rect`], which subtracts the shell's *own*
//! `FRAME_VIS_SIDE + CHROME_PAD` / `CONTENT_TOP` constants — never a value
//! re-derived from the interior tile's inset (#310). Every call site keeps the
//! vanilla numbers so each constant is eyeball-checkable against the data file.
//! The slot template's rects are slot-local, so each slot is spawned as a
//! container at its own `347x73` box with its children carrying the template's
//! own coordinates verbatim.
//!
//! The panel only ever *displays* server state: the C→S "set auto potion"
//! opcode is UNKNOWN (no doc, zero outbound samples), so OK is spawned
//! permanently disabled, the sliders render a thumb at the current percentage
//! with no drag observers, and the combo boxes render as empty boxes — what they
//! enumerate is UNKNOWN too, and this repo has no combo widget to fill them
//! with. Cancel (and the shell's X) closes the window.

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui::InteractionDisabled;
use bevy::ui_widgets::{Activate, Button};

use crate::assets::FontAssets;
use crate::plugins::hud::autopotion::model::{
    AutoPotionRow, AutoPotionSettings, AutoPotionState, MAX_PERCENT,
};
use crate::plugins::hud::game_window::{self, abs_node};
use crate::plugins::hud::scale::hud_scale;
use crate::plugins::textdata::ClientUiStrings;
use crate::plugins::ui_v2::style::ImageButtonStyle;

// --- Shell geometry ---------------------------------------------------------

/// `ginterface.txt:1500` — `GDR_AUTO_POTION` (id 135) is `Rect="0,0,394,520"`.
const OUTER_W: f32 = 394.0;
const OUTER_H: f32 = 520.0;

/// The shared shell's content origin, `(12, 36)`.
const ORIGIN_X: f32 = game_window::FRAME_VIS_SIDE + game_window::CHROME_PAD;
const ORIGIN_Y: f32 = game_window::CONTENT_TOP;

/// Content box that makes the shell wrap exactly the vanilla 394x520 outer rect
/// (asserted by `outer_window_matches_the_vanilla_window_rect`).
const CONTENT_W: f32 = OUTER_W - 2.0 * ORIGIN_X;
const CONTENT_H: f32 = OUTER_H - ORIGIN_Y - game_window::CHROME_PAD - game_window::FRAME_VIS_BOTTOM;

/// Right/top anchor of the (draggable) window on spawn. Ours: vanilla's
/// `wndpos.dat` slot 8 sample is a left/top pair at 800x600
/// (`docs/formats/wndpos.md:45`), which this right/top-anchored, 1.5-scaled
/// shell cannot reuse, and wndpos persistence is not wired up.
const WINDOW_RIGHT: f32 = 700.0;
const WINDOW_TOP: f32 = 40.0;

/// Window-space `(x, y, w, h)` from the resinfo file, rebased into the shell's
/// content space.
const fn content_rect(rect: (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
    (rect.0 - ORIGIN_X, rect.1 - ORIGIN_Y, rect.2, rect.3)
}

// --- Art extents (vanilla leaves these `Rect="…,0,0"` and takes them from the
// texture; all read off the DDJ files' DDS headers) --------------------------

/// `interface/ifcommon/com_button.ddj`.
const BUTTON_W: f32 = 76.0;
const BUTTON_H: f32 = 24.0;
/// `interface/ifcommon/com_checkbutton_{off,on}.ddj`.
const CHECKBOX: f32 = 16.0;
/// `interface/pet/pt_figure.ddj`, the numeric value's inset box.
const DATA_BOX_W: f32 = 36.0;
const DATA_BOX_H: f32 = 20.0;
/// `interface/ifcommon/com_scroll_button.ddj`, the slider thumb.
const THUMB: f32 = 16.0;

// --- Section `Create` (ifautopotion.txt), drawn back to front = ascending ID -

/// `GDR_AUTO_POTION_FRAME_1:CIFFrame` id 1, `13,42,368,465`.
const FRAME_1_RECT: (f32, f32, f32, f32) = content_rect((13.0, 42.0, 368.0, 465.0));
/// `GDR_AUTO_POTION_BG_1:CIFNormalTile` id 2, `29,58,336,433`.
const BG_RECT: (f32, f32, f32, f32) = content_rect((29.0, 58.0, 336.0, 433.0));
/// `GDR_AUTO_POTION_FRAME_2:CIFFrame` id 5, `24,55,346,126`.
const FRAME_2_RECT: (f32, f32, f32, f32) = content_rect((24.0, 55.0, 346.0, 126.0));
/// `GDR_AUTO_POTION_DESC_PML:CIFPML` id 7, `43,76,309,83`.
const DESC_RECT: (f32, f32, f32, f32) = content_rect((43.0, 76.0, 309.0, 83.0));
/// `GDR_AUTO_POTION_SLOT_HP:CIFAutoPotionSlot` id 10, `23,197,347,73`, ADDID 100.
const SLOT_HP_RECT: (f32, f32, f32, f32) = content_rect((23.0, 197.0, 347.0, 73.0));
/// `GDR_AUTO_POTION_SLOT_MP:CIFAutoPotionSlot` id 11, `23,283,347,73`, ADDID 200.
const SLOT_MP_RECT: (f32, f32, f32, f32) = content_rect((23.0, 283.0, 347.0, 73.0));
/// `GDR_AUTO_POTION_OK_BTN:CIFButton` id 30, `117,462,0,0`.
const OK_RECT: (f32, f32, f32, f32) = content_rect((117.0, 462.0, BUTTON_W, BUTTON_H));
/// `GDR_AUTO_POTION_CANCEL_BTN:CIFButton` id 31, `205,462,0,0`.
const CANCEL_RECT: (f32, f32, f32, f32) = content_rect((205.0, 462.0, BUTTON_W, BUTTON_H));

/// The `ADDID` values on the two `CIFAutoPotionSlot` controls (HP 100, MP 200)
/// — a grammar key that appears nowhere else in the whole resinfo corpus. Kept
/// as the slot's opaque id offset and deliberately **not** reinterpreted as an
/// item or potion-category id; its semantics are UNKNOWN.
const SLOT_HP_ADDID: u16 = 100;
const SLOT_MP_ADDID: u16 = 200;

// --- Section `AbnormalSlot` -------------------------------------------------

/// id 15, `30,377,0,0`.
const ABNORMAL_CHECK_RECT: (f32, f32, f32, f32) = content_rect((30.0, 377.0, CHECKBOX, CHECKBOX));
/// id 16, `55,379,61,12`.
const ABNORMAL_NAME_RECT: (f32, f32, f32, f32) = content_rect((55.0, 379.0, 61.0, 12.0));
/// id 17, `136,379,45,12`.
const ABNORMAL_BELT_LABEL_RECT: (f32, f32, f32, f32) = content_rect((136.0, 379.0, 45.0, 12.0));
/// id 18, `188,375,57,20`.
const ABNORMAL_BELT_COMBO_RECT: (f32, f32, f32, f32) = content_rect((188.0, 375.0, 57.0, 20.0));
/// id 19, `255,379,45,12`.
const ABNORMAL_QUICK_LABEL_RECT: (f32, f32, f32, f32) = content_rect((255.0, 379.0, 45.0, 12.0));
/// id 20, `306,375,57,20`.
const ABNORMAL_QUICK_COMBO_RECT: (f32, f32, f32, f32) = content_rect((306.0, 375.0, 57.0, 20.0));

// --- Section `PotionDelaySlot` ----------------------------------------------

/// id 25, `30,416,0,0`.
const DELAY_CHECK_RECT: (f32, f32, f32, f32) = content_rect((30.0, 416.0, CHECKBOX, CHECKBOX));
/// id 26, `55,417,105,12`.
const DELAY_NAME_RECT: (f32, f32, f32, f32) = content_rect((55.0, 417.0, 105.0, 12.0));
/// id 27, `306,413,58,25` — `CIFVerticalSpinCtrl`, the only occurrence of that
/// class in the corpus.
const DELAY_SPIN_RECT: (f32, f32, f32, f32) = content_rect((306.0, 413.0, 58.0, 25.0));

// --- The slot template (ifautopotionslot.txt), slot-local -------------------

/// id 1, `7,8,0,0`.
const L_CHECK: (f32, f32, f32, f32) = (7.0, 8.0, CHECKBOX, CHECKBOX);
/// id 2, `32,10,19,12` — empty `Text`, runtime-filled with the row's name.
const L_NAME: (f32, f32, f32, f32) = (32.0, 10.0, 19.0, 12.0);
/// id 5, `54,8,0,0` — `interface\pet\pt_figure.ddj`.
const L_DATA_BOX: (f32, f32, f32, f32) = (54.0, 8.0, DATA_BOX_W, DATA_BOX_H);
/// id 6, `59,11,25,12` — the numeric value, inside the box above.
const L_DATA: (f32, f32, f32, f32) = (59.0, 11.0, 25.0, 12.0);
/// id 7, `91,10,14,12` — a 14px one-glyph slot butted against the box's right
/// edge (54 + 36 = 90): the `%` sign.
const L_UNIT: (f32, f32, f32, f32) = (91.0, 10.0, 14.0, 12.0);
/// id 10, `113,10,45,12`.
const L_BELT_LABEL: (f32, f32, f32, f32) = (113.0, 10.0, 45.0, 12.0);
/// id 11, `165,6,57,20`.
const L_BELT_COMBO: (f32, f32, f32, f32) = (165.0, 6.0, 57.0, 20.0);
/// id 12, `232,10,45,12`.
const L_QUICK_LABEL: (f32, f32, f32, f32) = (232.0, 10.0, 45.0, 12.0);
/// id 13, `282,6,57,20`.
const L_QUICK_COMBO: (f32, f32, f32, f32) = (282.0, 6.0, 57.0, 20.0);
/// id 15, `6,32,334,21` — `CIFSliderCtrl` on `interface\recovery\re_selectbar.ddj`.
const L_SLIDER: (f32, f32, f32, f32) = (6.0, 32.0, 334.0, 21.0);
/// ids 17/18/19, `10,55,26,12` / `164,56,26,12` / `317,55,26,12` — all with an
/// empty `Text` in the data (runtime-filled), so the three label *strings* are
/// ours; the range they mark is grounded (see [`MAX_PERCENT`]).
const L_MIN: (f32, f32, f32, f32) = (10.0, 55.0, 26.0, 12.0);
const L_CENTER: (f32, f32, f32, f32) = (164.0, 56.0, 26.0, 12.0);
const L_MAX: (f32, f32, f32, f32) = (317.0, 55.0, 26.0, 12.0);

// --- Slider anatomy ---------------------------------------------------------

/// `re_selectbar.ddj` is a 336x24 canvas whose opaque track occupies x 1..333
/// and y 0..20 — i.e. the `334x21` control rect *is* the art's extent inside a
/// padded canvas. So the track is drawn at its native size at the rect's
/// origin: stretching the canvas into 334x21 would squash the track by 12.5%
/// vertically.
const SLIDER_ART_W: f32 = 336.0;
const SLIDER_ART_H: f32 = 24.0;
/// The groove inside that art, measured off the decoded texture: black rails at
/// x 20 and x 313 bound an interior of x 21..312 with tick marks at
/// 77/122/167/212/257 — 167 being the midpoint the CENTER label sits under.
const GROOVE_X0: f32 = 20.0;
const GROOVE_X1: f32 = 313.0;
/// Thumb offset inside the slider box, from the `CIFSliderCtrl` prototype:
/// `ifsliderctrl.txt:6` `GDR_SLIDER_CTRL_BTN_THUMB:CIFButton` `Rect="24,4,0,0"`
/// on `com_scroll_button.ddj`. Only the `y` is layout — the prototype's `x` is
/// editor scratch, since the runtime derives it from the value (which is what
/// [`thumb_left`] does). The prototype's prev/next arrow buttons
/// (`com_{left,right}_bigarrow.ddj`) are omitted: they are 24x24 and cannot fit
/// this 21px-tall control, so their placement here is UNKNOWN.
const THUMB_Y: f32 = 4.0;

// --- Art paths --------------------------------------------------------------

const FRAME_1_DIR: &str = "media://interface/inventory/int_window_";
const FRAME_2_DIR: &str = "media://interface/frame/frameg_wnd_";
const BG_TILE_DDJ: &str = "media://interface/ifcommon/bg_tile/com_bg_tile_b.ddj";
const CHECK_OFF_DDJ: &str = "media://interface/ifcommon/com_checkbutton_off.ddj";
const CHECK_ON_DDJ: &str = "media://interface/ifcommon/com_checkbutton_on.ddj";
const DATA_BOX_DDJ: &str = "media://interface/pet/pt_figure.ddj";
const SLIDER_DDJ: &str = "media://interface/recovery/re_selectbar.ddj";
const THUMB_DDJ: &str = "media://interface/ifcommon/com_scroll_button.ddj";
const BUTTON_DDJ: &str = "media://interface/ifcommon/com_button.ddj";
const BUTTON_FOCUS_DDJ: &str = "media://interface/ifcommon/com_button_focus.ddj";
const BUTTON_PRESS_DDJ: &str = "media://interface/ifcommon/com_button_press.ddj";
const BUTTON_DISABLE_DDJ: &str = "media://interface/ifcommon/com_button_disable.ddj";

// --- Colours (resinfo `COLOR` is A,R,G,B) -----------------------------------

/// `FontColor="255,239,218,164"` on the Belt / Quick-slot statics
/// (`ifautopotion.txt:188,226`, `ifautopotionslot.txt:106,144`).
const LABEL_COLOR: Color = Color::srgb_u8(239, 218, 164);
/// `FontColor="255,255,255,255"` on the row-name, value and description statics.
const TEXT_COLOR: Color = Color::WHITE;
/// `FontColor="255,255,247,202"` on the OK / Cancel buttons
/// (`ifautopotion.txt:11,30`).
const BUTTON_TEXT_COLOR: Color = Color::srgb_u8(255, 247, 202);
/// `CIFComboBox` and `CIFVerticalSpinCtrl` carry an **empty** `DDJ` and have no
/// prototype file anywhere in the resinfo corpus, so their art is code-side in
/// the original and UNKNOWN to us. They are drawn as the house inset the
/// store/storage modals already use (`storage/ui.rs:837-841`) so the boxes are
/// visible without inventing an ornament.
const INSET_BG: Color = Color::srgb(0.09, 0.08, 0.06);
const INSET_BORDER: Color = Color::srgb(0.55, 0.45, 0.25);

// --- Markers ----------------------------------------------------------------

#[derive(Component)]
pub struct ApWindowRoot;

/// A row's numeric readout (`GDR_AUTOPOTION_SLOT_DATA`, or the delay spin).
#[derive(Component)]
pub struct ApValue(AutoPotionRow);

/// A row's on/off checkbox.
#[derive(Component)]
pub struct ApCheck(AutoPotionRow);

/// A slider thumb, positioned from its row's percentage.
#[derive(Component)]
pub struct ApThumb(AutoPotionRow);

/// The slot's opaque `ADDID` (see [`SLOT_HP_ADDID`]).
#[derive(Component)]
struct ApSlotAddId(#[allow(dead_code)] u16);

// --- Spawning ---------------------------------------------------------------

/// Spawn the (initially hidden) auto-potion window.
pub fn spawn_autopotion_window(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    fonts: Res<FontAssets>,
    ui_strings: Res<ClientUiStrings>,
    cam_query: Query<Entity, With<Camera2d>>,
) {
    let Ok(camera) = cam_query.single() else {
        warn!("autopotion: no 2d camera to attach to");
        return;
    };
    let s = hud_scale();
    let title = ui_strings
        .get_or("UIIT_PAG_MACROPOTION_TITLE", "Auto potion")
        .to_string();

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
        .insert((ApWindowRoot, GlobalZIndex(59)))
        .entry::<Node>()
        .and_modify(|mut node| node.display = Display::None);
    commands
        .entity(window.expect_close_button())
        .observe(on_close);

    commands.entity(window.content).with_children(|content| {
        let ui = |key: &str, fallback: &str| ui_strings.get_or(key, fallback).to_string();

        // --- backdrop: int_window_ ring, bg tiling, then the description's
        // own frameg_wnd_ ring (ascending control id = back to front)
        spawn_frame(
            content,
            &asset_server,
            FRAME_1_DIR,
            FRAME_1_RECT,
            16.0,
            16.0,
            s,
        );
        content.spawn((
            abs_node(BG_RECT, s),
            ImageNode {
                image: asset_server.load(BG_TILE_DDJ),
                image_mode: NodeImageMode::Tiled {
                    tile_x: true,
                    tile_y: true,
                    stretch_value: s,
                },
                ..default()
            },
            Pickable::IGNORE,
        ));
        spawn_frame(
            content,
            &asset_server,
            FRAME_2_DIR,
            FRAME_2_RECT,
            24.0,
            16.0,
            s,
        );
        content.spawn((
            Text::new(strip_pml(&ui(
                "UIIT_STT_MACROPOTION_CONTENTS",
                "Set the % of the HP/MP gage to the preferred value, then auto potion recovery \
                 will be in use. To not use auto recovery for a certain category, remove the \
                 check from the check box.",
            ))),
            text_font(&fonts, 7.5, s),
            TextColor(TEXT_COLOR),
            abs_node(DESC_RECT, s),
            Pickable::IGNORE,
        ));

        // --- the two CIFAutoPotionSlot instantiations
        for (row, rect, addid, name_key, name_fallback) in [
            (
                AutoPotionRow::Hp,
                SLOT_HP_RECT,
                SLOT_HP_ADDID,
                "PARAM_HP",
                "HP",
            ),
            (
                AutoPotionRow::Mp,
                SLOT_MP_RECT,
                SLOT_MP_ADDID,
                "PARAM_MP",
                "MP",
            ),
        ] {
            spawn_slot(
                content,
                &asset_server,
                &fonts,
                row,
                rect,
                addid,
                &ui(name_key, name_fallback),
                &ui("UIIT_STT_MACROPOTION_BELT", "Belt"),
                &ui("UIIT_STT_MACROPOTION_QUICKSLOT", "Quick slot"),
                s,
            );
        }

        // --- Section AbnormalSlot: a checkbox, a name and the two source
        // pickers. No threshold control exists in the data for this row, so
        // `auto_universal` only drives the checkbox.
        content.spawn(check_box(
            &asset_server,
            AutoPotionRow::Universal,
            ABNORMAL_CHECK_RECT,
            s,
        ));
        content.spawn(static_text(
            &fonts,
            ABNORMAL_NAME_RECT,
            ui("UIIT_STT_MACROPOTION_ABNORMAL", "Abnormal status"),
            TEXT_COLOR,
            Justify::Left,
            s,
        ));
        content.spawn(static_text(
            &fonts,
            ABNORMAL_BELT_LABEL_RECT,
            ui("UIIT_STT_MACROPOTION_BELT", "Belt"),
            LABEL_COLOR,
            Justify::Right,
            s,
        ));
        content.spawn(inset_box(ABNORMAL_BELT_COMBO_RECT, s));
        content.spawn(static_text(
            &fonts,
            ABNORMAL_QUICK_LABEL_RECT,
            ui("UIIT_STT_MACROPOTION_QUICKSLOT", "Quick slot"),
            LABEL_COLOR,
            Justify::Right,
            s,
        ));
        content.spawn(inset_box(ABNORMAL_QUICK_COMBO_RECT, s));

        // --- Section PotionDelaySlot: the spin control shows the raw number
        // (the row has no unit static, so the delay's unit is UNKNOWN).
        content.spawn(check_box(
            &asset_server,
            AutoPotionRow::Delay,
            DELAY_CHECK_RECT,
            s,
        ));
        content.spawn(static_text(
            &fonts,
            DELAY_NAME_RECT,
            ui("UIIT_STT_MACROPOTION_DELAY", "Potion use delay"),
            TEXT_COLOR,
            Justify::Left,
            s,
        ));
        content
            .spawn(inset_box(DELAY_SPIN_RECT, s))
            .with_children(|spin| {
                spin.spawn((
                    ApValue(AutoPotionRow::Delay),
                    Text::new("0"),
                    text_font(&fonts, 8.0, s),
                    TextColor(TEXT_COLOR),
                    TextLayout::justify(Justify::Center),
                    abs_node((0.0, 6.0, DELAY_SPIN_RECT.2, 12.0), s),
                    Pickable::IGNORE,
                ));
            });

        // --- OK is permanently disabled: the C→S set opcode is UNKNOWN, so
        // there is nothing an accepted edit could be sent to. Same disabled-art
        // pattern as the character window's + buttons: `update_button_visuals`
        // repaints the ImageNode from the style, so all three handles must be
        // the _disable art, and `InteractionDisabled` blocks `Activate`.
        let disabled = asset_server.load::<Image>(BUTTON_DISABLE_DDJ);
        let ok = spawn_text_button(
            content,
            &fonts,
            OK_RECT,
            ui("UIIT_CTL_CONFIRM", "Confirm"),
            ImageButtonStyle {
                normal: disabled.clone(),
                hover: disabled.clone(),
                press: disabled,
                ..Default::default()
            },
            s,
        );
        content.commands().entity(ok).insert(InteractionDisabled);
        let cancel = spawn_text_button(
            content,
            &fonts,
            CANCEL_RECT,
            ui("UIIT_CTL_CANCEL", "Cancel"),
            ImageButtonStyle {
                normal: asset_server.load(BUTTON_DDJ),
                hover: asset_server.load(BUTTON_FOCUS_DDJ),
                press: asset_server.load(BUTTON_PRESS_DDJ),
                ..Default::default()
            },
            s,
        );
        content.commands().entity(cancel).observe(on_close);
    });
}

/// One `CIFAutoPotionSlot` instance: a container at the slot's own `347x73` box
/// whose children carry `ifautopotionslot.txt`'s slot-local rects verbatim.
#[allow(clippy::too_many_arguments)]
fn spawn_slot(
    content: &mut ChildSpawnerCommands,
    asset_server: &AssetServer,
    fonts: &FontAssets,
    row: AutoPotionRow,
    rect: (f32, f32, f32, f32),
    addid: u16,
    name: &str,
    belt: &str,
    quickslot: &str,
    s: f32,
) {
    content
        .spawn((
            Name::from(format!("AutoPotion Slot {addid}")),
            ApSlotAddId(addid),
            abs_node(rect, s),
            Pickable::IGNORE,
        ))
        .with_children(|slot| {
            slot.spawn(check_box(asset_server, row, L_CHECK, s));
            slot.spawn(static_text(
                fonts,
                L_NAME,
                name.to_string(),
                TEXT_COLOR,
                Justify::Left,
                s,
            ));
            slot.spawn((
                abs_node(L_DATA_BOX, s),
                ImageNode {
                    image: asset_server.load(DATA_BOX_DDJ),
                    image_mode: NodeImageMode::Stretch,
                    ..default()
                },
                Pickable::IGNORE,
            ));
            slot.spawn((
                ApValue(row),
                Text::new("0"),
                text_font(fonts, 8.0, s),
                TextColor(TEXT_COLOR),
                TextLayout::justify(Justify::Right),
                abs_node(L_DATA, s),
                Pickable::IGNORE,
            ));
            slot.spawn(static_text(
                fonts,
                L_UNIT,
                "%".to_string(),
                TEXT_COLOR,
                Justify::Left,
                s,
            ));
            slot.spawn(static_text(
                fonts,
                L_BELT_LABEL,
                belt.to_string(),
                LABEL_COLOR,
                Justify::Right,
                s,
            ));
            slot.spawn(inset_box(L_BELT_COMBO, s));
            slot.spawn(static_text(
                fonts,
                L_QUICK_LABEL,
                quickslot.to_string(),
                LABEL_COLOR,
                Justify::Right,
                s,
            ));
            slot.spawn(inset_box(L_QUICK_COMBO, s));

            // the slider: track art at its native extent, plus a static thumb
            // (no drag observers — the panel is read-only)
            slot.spawn((
                abs_node((L_SLIDER.0, L_SLIDER.1, SLIDER_ART_W, SLIDER_ART_H), s),
                ImageNode {
                    image: asset_server.load(SLIDER_DDJ),
                    image_mode: NodeImageMode::Stretch,
                    ..default()
                },
                Pickable::IGNORE,
            ));
            slot.spawn((
                ApThumb(row),
                abs_node((thumb_left(0), L_SLIDER.1 + THUMB_Y, THUMB, THUMB), s),
                ImageNode {
                    image: asset_server.load(THUMB_DDJ),
                    image_mode: NodeImageMode::Stretch,
                    ..default()
                },
                Pickable::IGNORE,
            ));
            // The three range marks. Their `Text` is empty in the data
            // (runtime-filled), so these strings are OURS — the 0..=100 range
            // they mark is what [`MAX_PERCENT`] grounds. `HAlign=0` on all
            // three, hence left.
            for (local, text) in [(L_MIN, "0"), (L_CENTER, "50"), (L_MAX, "100")] {
                slot.spawn(static_text(
                    fonts,
                    local,
                    text.to_string(),
                    LABEL_COLOR,
                    Justify::Left,
                    s,
                ));
            }
        });
}

pub fn cleanup_autopotion_window(
    mut commands: Commands,
    windows: Query<Entity, With<ApWindowRoot>>,
) {
    for entity in windows.iter() {
        commands.entity(entity).despawn();
    }
}

// --- Spawn helpers ----------------------------------------------------------

fn text_font(fonts: &FontAssets, size: f32, s: f32) -> TextFont {
    TextFont {
        font: fonts.two.clone().into(),
        font_size: FontSize::Px(size * s),
        ..default()
    }
}

/// A `CIFStatic` label at a rect.
fn static_text(
    fonts: &FontAssets,
    rect: (f32, f32, f32, f32),
    text: String,
    color: Color,
    justify: Justify,
    s: f32,
) -> impl Bundle {
    (
        Text::new(text),
        text_font(fonts, 8.0, s),
        TextColor(color),
        TextLayout::justify(justify),
        abs_node(rect, s),
        Pickable::IGNORE,
    )
}

/// A row's `CIFCheckBox`. Not a `Button`: it reflects server state and toggling
/// it would need the unknown write path.
fn check_box(
    asset_server: &AssetServer,
    row: AutoPotionRow,
    rect: (f32, f32, f32, f32),
    s: f32,
) -> impl Bundle {
    (
        ApCheck(row),
        abs_node(rect, s),
        ImageNode {
            image: asset_server.load(CHECK_OFF_DDJ),
            image_mode: NodeImageMode::Stretch,
            ..default()
        },
        Pickable::IGNORE,
    )
}

/// The stand-in box for a `CIFComboBox` / `CIFVerticalSpinCtrl` (see
/// [`INSET_BG`]).
fn inset_box(rect: (f32, f32, f32, f32), s: f32) -> impl Bundle {
    (
        abs_node(rect, s),
        BackgroundColor(INSET_BG),
        Outline {
            width: Val::Px(1.0),
            color: INSET_BORDER,
            ..default()
        },
        Pickable::IGNORE,
    )
}

/// A `com_button.ddj` push button with a centred caption.
fn spawn_text_button(
    parent: &mut ChildSpawnerCommands,
    fonts: &FontAssets,
    rect: (f32, f32, f32, f32),
    label: String,
    style: ImageButtonStyle,
    s: f32,
) -> Entity {
    parent
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
        .with_children(|button| {
            button.spawn((
                Text::new(label),
                text_font(fonts, 8.5, s),
                TextColor(BUTTON_TEXT_COLOR),
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
}

/// Minimal local copy of the private 9-slice recipe in `inventory::ui`: eight
/// `*_wnd_` border pieces in a 3x3 CSS grid (corners fixed to the art's extent,
/// edges stretched) around an open centre. Local because that helper hardcodes
/// the inventory's own scale and promoting it would mean restructuring a shared
/// window for no gain here.
#[allow(clippy::too_many_arguments)]
fn spawn_frame(
    parent: &mut ChildSpawnerCommands,
    asset_server: &AssetServer,
    dir: &str,
    rect: (f32, f32, f32, f32),
    corner_w: f32,
    corner_h: f32,
    s: f32,
) {
    let (side, bar) = (corner_w * s, corner_h * s);
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
            // Row-major: TL, T, TR, L, (open centre), R, BL, B, BR.
            for name in [
                Some("left_up"),
                Some("mid_up"),
                Some("right_up"),
                Some("left_side"),
                None,
                Some("right_side"),
                Some("left_down"),
                Some("mid_down"),
                Some("right_down"),
            ] {
                let mut cell = g.spawn((
                    Node {
                        width: Val::Percent(100.0),
                        height: Val::Percent(100.0),
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
                if let Some(name) = name {
                    cell.insert(ImageNode {
                        image: piece(name),
                        image_mode: NodeImageMode::Stretch,
                        ..default()
                    });
                }
            }
        });
}

/// `UIIT_STT_MACROPOTION_CONTENTS` is authored for the original's `CIFPML`
/// rich-text control, so it carries markup a plain `Text` node would render
/// literally. Minimal local copy of `intro_v2::captcha`'s private `plain_text`
/// (keep the authored breaks, drop every other tag) rather than making that
/// scene's helper public.
fn strip_pml(pml: &str) -> String {
    let mut out = String::with_capacity(pml.len());
    let mut rest = pml;
    while let Some(open) = rest.find('<') {
        let Some(len) = rest[open..].find('>') else {
            break;
        };
        out.push_str(&rest[..open]);
        if rest[open + 1..open + len]
            .trim_end_matches('/')
            .eq_ignore_ascii_case("br")
        {
            out.push('\n');
        }
        rest = &rest[open + len + 1..];
    }
    out.push_str(rest);
    out
}

// --- Behaviour --------------------------------------------------------------

/// Cancel and the shell's X share one handler: with no write path there is
/// nothing OK could have committed, so there is nothing for Cancel to revert.
fn on_close(_: On<Activate>, mut state: ResMut<AutoPotionState>) {
    state.open = false;
}

/// The thumb's left edge inside the slot, for a 0..=100 percentage: the groove
/// spans `GROOVE_X0..GROOVE_X1` inside the track art and the thumb travels that
/// span minus its own width.
fn thumb_left(percent: u8) -> f32 {
    let travel = GROOVE_X1 - GROOVE_X0 - THUMB;
    L_SLIDER.0 + GROOVE_X0 + travel * (percent.min(MAX_PERCENT) as f32 / MAX_PERCENT as f32)
}

/// Reflect `AutoPotionState.open` in the window's display.
pub fn apply_autopotion_visibility(
    state: Res<AutoPotionState>,
    mut roots: Query<&mut Node, With<ApWindowRoot>>,
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

/// Run condition: the settings changed, or the window was respawned.
pub fn autopotion_needs_refresh(
    settings: Res<AutoPotionSettings>,
    fresh: Query<(), Added<ApWindowRoot>>,
) -> bool {
    settings.is_changed() || !fresh.is_empty()
}

/// Repaint the numbers, the checkbox art and the slider thumbs from the
/// server's settings.
pub fn refresh_autopotion(
    settings: Res<AutoPotionSettings>,
    asset_server: Res<AssetServer>,
    mut values: Query<(&ApValue, &mut Text)>,
    mut checks: Query<(&ApCheck, &mut ImageNode)>,
    mut thumbs: Query<(&ApThumb, &mut Node)>,
) {
    for (value, mut text) in values.iter_mut() {
        let new = settings.value(value.0).to_string();
        if text.0 != new {
            text.0 = new;
        }
    }
    for (check, mut image) in checks.iter_mut() {
        let art = if settings.row_enabled(check.0) {
            CHECK_ON_DDJ
        } else {
            CHECK_OFF_DDJ
        };
        let handle = asset_server.load(art);
        if image.image != handle {
            image.image = handle;
        }
    }
    for (thumb, mut node) in thumbs.iter_mut() {
        node.left = Val::Px(thumb_left(settings.value(thumb.0)) * hud_scale());
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// `ginterface.txt:1500` — `GDR_AUTO_POTION` is `0,0,394,520`, so the shared
    /// shell must wrap our content box in exactly that.
    #[test]
    fn outer_window_matches_the_vanilla_window_rect() {
        assert_eq!(ORIGIN_X, 12.0);
        assert_eq!(ORIGIN_Y, 36.0);
        assert_eq!((CONTENT_W, CONTENT_H), (370.0, 468.0));
        assert_eq!(
            game_window::outer_size((CONTENT_W, CONTENT_H)),
            (OUTER_W, OUTER_H)
        );
    }

    /// Every interior rect equals its vanilla `ifautopotion.txt` rect minus the
    /// shell's content origin, computed from `game_window`'s own constants so a
    /// change there cannot silently desync this window (#310).
    #[test]
    fn interior_rects_sit_on_the_shells_content_origin() {
        // (vanilla x, vanilla y, ours) — control ids 1, 2, 5, 7, 10, 11, 30, 31
        // then the AbnormalSlot ids 15..20 and the PotionDelaySlot ids 25..27.
        let cases = [
            (13.0, 42.0, FRAME_1_RECT),
            (29.0, 58.0, BG_RECT),
            (24.0, 55.0, FRAME_2_RECT),
            (43.0, 76.0, DESC_RECT),
            (23.0, 197.0, SLOT_HP_RECT),
            (23.0, 283.0, SLOT_MP_RECT),
            (117.0, 462.0, OK_RECT),
            (205.0, 462.0, CANCEL_RECT),
            (30.0, 377.0, ABNORMAL_CHECK_RECT),
            (55.0, 379.0, ABNORMAL_NAME_RECT),
            (136.0, 379.0, ABNORMAL_BELT_LABEL_RECT),
            (188.0, 375.0, ABNORMAL_BELT_COMBO_RECT),
            (255.0, 379.0, ABNORMAL_QUICK_LABEL_RECT),
            (306.0, 375.0, ABNORMAL_QUICK_COMBO_RECT),
            (30.0, 416.0, DELAY_CHECK_RECT),
            (55.0, 417.0, DELAY_NAME_RECT),
            (306.0, 413.0, DELAY_SPIN_RECT),
        ];
        for (vanilla_x, vanilla_y, ours) in cases {
            assert_eq!(ours.0, vanilla_x - ORIGIN_X, "x of vanilla {vanilla_x}");
            assert_eq!(ours.1, vanilla_y - ORIGIN_Y, "y of vanilla {vanilla_y}");
        }
        // Nothing overflows the content box — except the outer `int_window_`
        // ring, which vanilla runs 3 units past it into the shell's bottom band
        // (`13,42,368,465` ends at window y 507 against the content's 504).
        // Deliberate, and the same kind of overhang the character window's
        // bottom ornament has over its own frame.
        assert_eq!(FRAME_1_RECT.1 + FRAME_1_RECT.3 - CONTENT_H, 3.0);
        assert!(FRAME_1_RECT.0 + FRAME_1_RECT.2 <= CONTENT_W);
        for rect in [
            BG_RECT,
            FRAME_2_RECT,
            DESC_RECT,
            SLOT_MP_RECT,
            CANCEL_RECT,
            ABNORMAL_QUICK_COMBO_RECT,
            DELAY_SPIN_RECT,
        ] {
            assert!(
                rect.0 + rect.2 <= CONTENT_W,
                "{rect:?} wider than the content"
            );
            assert!(
                rect.1 + rect.3 <= CONTENT_H,
                "{rect:?} taller than the content"
            );
        }
    }

    /// `ifautopotionslot.txt`'s rects are slot-local, and the window nests each
    /// slot's children under a container at the slot's own box — so a child's
    /// window-space position is `slot origin + local`. Pinned for both
    /// instantiations (HP `23,197`, MP `23,283`).
    #[test]
    fn slot_template_lands_at_both_instantiations() {
        // (slot content rect, vanilla slot origin)
        for (slot, origin) in [(SLOT_HP_RECT, (23.0, 197.0)), (SLOT_MP_RECT, (23.0, 283.0))] {
            assert_eq!((slot.0 + ORIGIN_X, slot.1 + ORIGIN_Y), origin);
            // the numeric value: local 59,11 -> window 82,208 / 82,294
            assert_eq!(slot.0 + L_DATA.0 + ORIGIN_X, origin.0 + 59.0);
            assert_eq!(slot.1 + L_DATA.1 + ORIGIN_Y, origin.1 + 11.0);
            // the slider: local 6,32 -> window 29,229 / 29,315
            assert_eq!(slot.0 + L_SLIDER.0 + ORIGIN_X, origin.0 + 6.0);
            assert_eq!(slot.1 + L_SLIDER.1 + ORIGIN_Y, origin.1 + 32.0);
            // the MAX label: local 317,55 -> window 340,252 / 340,338
            assert_eq!(slot.0 + L_MAX.0 + ORIGIN_X, origin.0 + 317.0);
            assert_eq!(slot.1 + L_MAX.1 + ORIGIN_Y, origin.1 + 55.0);
            // every child stays inside the slot's own 347x73 box
            for local in [
                L_CHECK,
                L_NAME,
                L_DATA_BOX,
                L_DATA,
                L_UNIT,
                L_BELT_LABEL,
                L_BELT_COMBO,
                L_QUICK_LABEL,
                L_QUICK_COMBO,
                L_SLIDER,
                L_MIN,
                L_CENTER,
                L_MAX,
            ] {
                assert!(local.0 + local.2 <= slot.2, "{local:?} overflows the slot");
                assert!(local.1 + local.3 <= slot.3, "{local:?} overflows the slot");
            }
        }
    }

    /// The thumb travels the groove measured off `re_selectbar.ddj` (x 20..313
    /// inside the art) minus its own 16px, so 0% parks it on the left rail and
    /// 100% ends flush with the right one. Out-of-range values are clamped, as
    /// `AutoPotionSettings` already clamps them.
    #[test]
    fn thumb_spans_the_measured_groove() {
        assert_eq!(thumb_left(0), L_SLIDER.0 + GROOVE_X0);
        assert_eq!(thumb_left(MAX_PERCENT) + THUMB, L_SLIDER.0 + GROOVE_X1);
        assert_eq!(thumb_left(200), thumb_left(MAX_PERCENT));
        let mid = thumb_left(50);
        assert!(mid > thumb_left(0) && mid < thumb_left(MAX_PERCENT));
        // the whole slider (art + thumb) stays inside the slot's 347 width
        assert!(L_SLIDER.0 + SLIDER_ART_W <= SLOT_HP_RECT.2);
    }

    /// The `CIFPML` description carries markup; only its authored breaks survive.
    #[test]
    fn pml_markup_is_stripped_to_plain_text() {
        assert_eq!(
            strip_pml("<sml2><font color=\"1,2,3,4\">Head</font><br>Body</sml2>"),
            "Head\nBody"
        );
    }
}
