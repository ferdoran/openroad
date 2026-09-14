//! Chat viewer window (bottom-left): tab bar, filtered message list with a
//! working left-edge scrollbar, and the always-visible input row (Enter only
//! toggles its *focus*). The whole window can fade out after idle time
//! ([`fade_chat_window`]) behind the `chat.idle_fade` flag — not original
//! behaviour, so it is off by default.
//!
//! Idea: the layout is hand-transcribed from the vanilla
//! `Media.pk2/resinfo/ifchatviewer.txt` like the mini-info and minimap panels,
//! uniformly scaled. Every `#ifdef` in that file is active — `Media/config/
//! define.txt:17` defines `UI_UPDATE_2009_FIRST` — so the **2009** branch is
//! the live one wherever the file offers both. The window is a flex column
//! (header / body / bottom row) so the zoom (small↔large) and hide (collapse)
//! modes only have to change the body node's height/display — everything
//! inside the body anchors to its top *and* bottom edges. The message list is
//! a real `Overflow::scroll_y` node: bevy's layout clamps `ScrollPosition`
//! and exposes content/viewport sizes on `ComputedNode`, which drives the
//! scrollbar thumb geometry and the stick-to-bottom behavior. List rows are
//! rebuilt from the [`ChatHistory`] ring whenever it or the active tab
//! changes (capped at [`MAX_RENDERED_LINES`] rows).

use bevy::input_focus::InputFocus;
use bevy::picking::events::{Drag, DragStart, Pointer, Scroll};
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::text::EditableText;
use bevy::ui::{InteractionDisabled, Overflow, ScrollPosition, UiTargetCamera};
use bevy::ui_widgets::{Activate, Button};

use crate::assets::FontAssets;
use crate::plugins::config::chat::ChatColors;
use crate::plugins::config::ClientConfig;
use crate::plugins::hud::player_mini_info::PlayerVitals;
use crate::plugins::textdata::ClientUiStrings;
use crate::plugins::ui_v2::style::ImageButtonStyle;
use crate::plugins::ui_v2::widgets::{image_button, label, text_input};

use super::model::{
    format_restriction, ChatHistory, ChatState, ChatTab, CANT_CHATTING_FALLBACK, CANT_CHATTING_KEY,
    MAX_RENDERED_LINES, WHISPER_FROM_FALLBACK, WHISPER_FROM_KEY, WHISPER_TO_FALLBACK,
    WHISPER_TO_KEY,
};
use crate::plugins::hud::scale::hud_scale;

// --- Layout constants (resinfo/ifchatviewer.txt, window space) --------------

/// Left margin, and bottom clearance for the skill/exp bar under the chat.
/// `ginterface.txt:760` `GDR_CHAT_BOARD` is `0,546,399,398`, so the left
/// margin is 0. The bottom clearance is spec-derived, not read from data: the
/// underbar's top edge sits at 684 in the 768-tall design space, i.e. 84 px of
/// clearance (the board's own y=546 puts its bottom at 944, off-canvas —
/// see docs/re/ui/hud-chat.md §9-U1).
const WINDOW_MARGIN: (f32, f32) = (0.0, 84.0); // left, bottom (unscaled px)
const WINDOW_W: f32 = 399.0;
/// Tab-bar strip height; the frame bg starts right below it (resinfo y=21).
const HEADER_H: f32 = 21.0;
const BOTTOM_H: f32 = 20.0;
const BOTTOM_GAP: f32 = 1.0;

/// Message-list heights for the two zoom modes. Large is the resinfo value
/// (window 398 tall).
const LIST_H_LARGE: f32 = 343.0;
/// UNKNOWN (docs/re/ui/hud-chat.md §9-U5): no PK2 file carries the collapsed
/// height. 83 is the best data-supported inference — 138 total window height
/// between the board top (546) and the underbar top (684), minus the same
/// 55 px of chrome the large mode uses — not a read value.
const LIST_H_SMALL: f32 = 83.0;

// Body-local rects (body top = window y 21). The bg is chat_window.ddj cut in
// three horizontal slices; mid/list/gutter anchor top+bottom so only the body
// height changes between zoom modes.
const BG_X: f32 = 18.0;
const BG_W: f32 = 381.0;
const BG_CAP_H: f32 = 4.0;
const LIST_X: f32 = 27.0;
const LIST_W: f32 = 365.0;
const LIST_TOP: f32 = 7.0; // window y 28
const LIST_BOTTOM: f32 = 6.0;
/// The scrollbar spans the body's full height: arrow buttons flush with the
/// window's upper/lower borders, the thumb running between them over the bare
/// chat backdrop. A `CIFVerticalScroll` is only ever three pieces —
/// `com_scroll_button` thumb + `chat_arrow_up`/`_down`, all square 16x16 drawn
/// 1:1 (`resinfo/ifverticalscroll.txt`) — with **no track background**.
///
/// UNKNOWN (docs/re/ui/hud-chat.md §6-5): `GDR_CHAT_VSCROLL` is `0,36,16,308`
/// (ifchatviewer.txt:34), i.e. a fixed 308-px extent in the large mode. We
/// span the body instead so the bar still works when the window is collapsed;
/// how the original re-lays the bar across zoom modes is not in the data.
const GUTTER_W: f32 = 16.0;

// Header-local rects.
const WHISPER_BTN: (f32, f32, f32, f32) = (15.0, 0.0, 16.0, 20.0);
const HIDE_BTN: (f32, f32, f32, f32) = (30.0, 0.0, 16.0, 20.0);
/// Tab strip: five fixed cells, not a stretch-to-the-right-edge flex row.
/// There is no tab control in the resinfo (the strip is code-drawn in the
/// original too), but its geometry is pinned by the five lamp rects, which are
/// data: `50,6` `101,6` `152,6` `203,6` `254,6`, each `4,8`
/// (ifchatviewer.txt:131/:112/:93/:74/:54). The lamp x-run has pitch **51** =
/// the cell pitch, and each lamp sits [`LAMP_OFFSET`] into its own cell, which
/// puts the strip origin at 50-5 = **45** and its right edge at 45+5*51 = 300
/// — well short of the 399-wide window.
const TAB_X0: f32 = 45.0;
const TAB_W: f32 = 51.0;
const TAB_H: f32 = 20.0;
/// chat_tab.ddj is a 52x20 canvas; the rightmost column and bottom 2 rows are
/// fully transparent, so the visible tab is 51x18 — exactly [`TAB_W`] wide.
const TAB_ART: (f32, f32) = (51.0, 18.0);
/// Lamp position inside its cell: the resinfo lamp x-run minus [`TAB_X0`]
/// (50-45) and the shared lamp y (6). Drawn at the art's full 4x8 canvas size
/// straight from the rects above — the previous percent-of-cell sizing only
/// looked right against the old, too-wide cell.
const LAMP_OFFSET: (f32, f32) = (5.0, 6.0);
const LAMP_SIZE: (f32, f32) = (4.0, 8.0);

// Bottom-row-local rects.
const ZOOM_BTN: (f32, f32, f32, f32) = (0.0, 0.0, 16.0, 20.0);
/// `GDR_CHAT_INPUTBOX` (ifchatviewer.txt:354). The file ships both branches;
/// `UI_UPDATE_2009_FIRST` is defined (`Media/config/define.txt:17`), so the
/// live rect is the 2009 one at :364 — `38,378,360,20`, not the `#else`
/// `18,378,381,20` at :366. The 20 px it gives up at x=18..38 is exactly
/// `GDR_CHAT_MODE_BTN` (:334, `18,378,20,20`).
const INPUT_ROW: (f32, f32, f32, f32) = (38.0, 0.0, 360.0, 20.0);

/// `GDR_CHAT_MODE_BTN` (ifchatviewer.txt:333-342, inside the same
/// `UI_UPDATE_2009_FIRST` branch as the input rect) — a 20x20 `CIFButton` at
/// window `18,378`, i.e. exactly the 20 px the 2009 input row gives up.
/// Bottom-row local, so y is 378-378 = 0.
/// Its art is `chat_order_button.ddj` (20x20, with the `_focus`/`_press`
/// frames every chat button ships).
const MODE_BTN: (f32, f32, f32, f32) = (18.0, 0.0, 20.0, 20.0);
/// The dropdown body (`Section = CreateChatMode`, ifchatviewer.txt:435-532):
/// `GDR_CHAT_MODE_BG_UP` `0,0,136,4` + two 136x20 mid statics +
/// `GDR_CHAT_MODE_BG_DOWN` `0,24,136,4`, all three the same `chat_window.ddj`
/// slices the window body uses, and a `0,6,136,20` row box. So: 136 wide,
/// 4 px caps, one 20 px row per entry.
const MODE_W: f32 = 136.0;
const MODE_ROW_H: f32 = 20.0;
const MODE_CAP_H: f32 = 4.0;
/// Gap between the dropdown's lower cap and the button that opens it.
/// **openroad choice**: the section's rects are body-local and the data does
/// not say where the body is anchored (§9) — we hang it directly above its
/// button, which is where a dropdown that opens upward has to go in a window
/// sitting on the screen bottom.
const MODE_POPUP_GAP: f32 = 2.0;

/// `GDR_CHAT_STA_PENALTY` (ifchatviewer.txt) — the chat-restriction readout,
/// a 160x20 static at window `404,380`, i.e. **outside** the 399-wide window,
/// level with the input row (`GDR_CHAT_INPUTBOX` y 378). Stored bottom-row
/// local, so x is unchanged and y is 380-378 = 2.
const PENALTY_RECT: (f32, f32, f32, f32) = (404.0, 2.0, 160.0, 20.0);
/// Its resinfo `FontColor` RGB (the same ARGB layout as the tab lamps).
const PENALTY_COLOR: Color = Color::srgb_u8(160, 247, 153);

/// Whisper-partner panel, floating above the tab bar
/// (`GDR_WHISPERLIST`, ifchatviewer.txt:15 — `16,-152,141,153`).
const WHISPER_PANEL: (f32, f32, f32, f32) = (16.0, -152.0, 141.0, 153.0);

// window_all.ddj atlas crops (1024x512, values from
// tmp_textures/window_all_regions/manifest.txt).
const WINDOW_ATLAS: &str = "media://interface/ifcommon/window_all.ddj";
const INPUT_BG_CROP: (f32, f32, f32, f32) = (18.0, 15.0, 399.0, 35.0);
const WHISPER_PANEL_CROP: (f32, f32, f32, f32) = (741.0, 166.0, 882.0, 319.0);

/// chat_window.ddj is 384x12: three 4px slices (up/mid/down), used 381 wide.
const CHAT_BG: &str = "media://interface/chattingwnd/chat_window.ddj";
const CHAT_BG_SLICE_W: f32 = 381.0;
const CHAT_BG_SLICE_H: f32 = 4.0;

const TAB_BG: &str = "media://interface/chattingwnd/chat_tab.ddj";

const MESSAGE_FONT_SIZE: f32 = 12.0;
const TAB_FONT_SIZE: f32 = 11.0;

/// Logical-px scroll steps: mouse wheel notch and scrollbar arrow click.
const WHEEL_STEP: f32 = 36.0;
const ARROW_STEP: f32 = 54.0;

/// Idle fade: the whole window fades out after this long without activity and
/// reappears on any chat activity (new line, Enter, clicks). Opt-in via
/// `chat.idle_fade`; the original has no idle fade at all.
const FADE_AFTER_SECS: f32 = 10.0;
const FADE_DURATION_SECS: f32 = 1.5;

const INACTIVE_TINT: Color = Color::srgb(0.55, 0.55, 0.55);
const INACTIVE_LABEL: Color = Color::srgb_u8(150, 150, 150);

// --- Markers ----------------------------------------------------------------

#[derive(Component, Default, Clone)]
pub struct ChatRoot;
/// The middle section (frame bg + list + scrollbar); hidden when collapsed,
/// resized by the zoom mode.
#[derive(Component, Default, Clone)]
pub struct ChatBody;
#[derive(Component, Default, Clone)]
pub struct ChatBottomRow;
/// The input background + editable text container (visible while typing).
#[derive(Component, Default, Clone)]
pub struct ChatInputRow;
/// Marker on the `EditableText` entity itself.
#[derive(Component, Default, Clone)]
pub struct ChatInputBox;
/// The scrollable message list node.
#[derive(Component, Default, Clone)]
pub struct ChatMessageList;
/// Region between the scrollbar arrows the thumb moves in.
#[derive(Component, Default, Clone)]
pub struct ChatScrollTrack;
#[derive(Component, Default, Clone)]
pub struct ChatScrollThumb;
/// A scrollbar arrow button; `up` distinguishes direction (for the
/// at-top/at-bottom disable logic).
#[derive(Component, Default, Clone)]
pub struct ChatScrollArrow {
    pub up: bool,
}
/// Scroll position (logical px) captured when a thumb drag started.
#[derive(Component)]
pub struct ChatThumbDrag(f32);
/// A clickable tab-bar cell (also on its lamp/label children for tinting).
#[derive(Component, Default, Clone)]
pub struct ChatTabCell {
    pub tab: ChatTab,
}
#[derive(Component, Default, Clone)]
pub struct ChatTabLamp {
    pub tab: ChatTab,
}
#[derive(Component, Default, Clone)]
pub struct ChatTabLabel {
    pub tab: ChatTab,
}
#[derive(Component, Default, Clone)]
pub struct ChatWhisperPanel;
/// Container the whisper-partner rows are rebuilt into.
#[derive(Component, Default, Clone)]
pub struct ChatWhisperEntries;
/// `GDR_CHAT_STA_PENALTY`: the chat-restriction readout, shown only while the
/// server's 0x302D restriction is still running.
#[derive(Component, Default, Clone)]
pub struct ChatPenaltyLabel;
/// `GDR_CHAT_MODE_BTN`: opens/closes the chat-mode dropdown.
#[derive(Component, Default, Clone)]
pub struct ChatModeButton;
/// The dropdown body (`Section = CreateChatMode`), hidden unless open.
#[derive(Component, Default, Clone)]
pub struct ChatModePopup;
/// One selectable entry of the dropdown.
#[derive(Component, Default, Clone)]
pub struct ChatModeRow {
    pub tab: ChatTab,
}
/// The label inside a [`ChatModeRow`] (tinted like the tab labels).
#[derive(Component, Default, Clone)]
pub struct ChatModeRowLabel {
    pub tab: ChatTab,
}

/// A message line in the list; its text survives the idle fade (vanilla shows
/// bare chat text when the window art has faded out).
#[derive(Component, Default, Clone)]
pub struct ChatMessageRow;

// --- Spawn / cleanup --------------------------------------------------------

pub fn spawn_chat_window(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    fonts: Res<FontAssets>,
    ui_strings: Res<ClientUiStrings>,
    mut state: ResMut<ChatState>,
    mut history: ResMut<ChatHistory>,
    cam_query: Query<Entity, With<Camera2d>>,
) {
    let Some(camera) = cam_query.iter().next() else {
        warn!("no 2d camera found for the chat window");
        return;
    };

    // fresh session state
    *state = ChatState::default();
    history.clear();

    commands
        .spawn_scene(chat_window(&asset_server, &fonts, &ui_strings))
        .insert(UiTargetCamera(camera));
}

pub fn cleanup_chat_window(mut commands: Commands, roots: Query<Entity, With<ChatRoot>>) {
    for entity in roots.iter() {
        commands.entity(entity).despawn();
    }
}

fn chat_window(
    asset_server: &AssetServer,
    fonts: &FontAssets,
    ui_strings: &ClientUiStrings,
) -> impl Scene {
    let s = hud_scale();
    // `UIIT_CTL_WHISPER_LIST` (textuisystem L470) is "Whisper list", not
    // "Whisper". The five tab captions stay hardcoded on purpose: which key
    // run labels them is [U] (hud-chat.md §9-U2 — the plain
    // `UIIT_CTL_CHAT_*` run or the sigil `UIIT_CTL_CHATMENU_*` run, which are
    // provably the dropdown's entries), so picking one here would be a guess.
    let whisper_title = ui_strings
        .get_or("UIIT_CTL_WHISPER_LIST", "Whisper list")
        .to_string();

    let atlas: Handle<Image> = asset_server.load(WINDOW_ATLAS);
    let atlas_input = atlas.clone();
    let atlas_whisper = atlas;
    let bg_up: Handle<Image> = asset_server.load(CHAT_BG);
    let bg_mid = bg_up.clone();
    let bg_down = bg_up.clone();
    // the dropdown body is the same three chat_window.ddj slices, at 136 wide
    let mode_bg_up = bg_up.clone();
    let mode_bg_mid = bg_up.clone();
    let mode_bg_down = bg_up.clone();
    let thumb_style = common_button_style(asset_server, "com_scroll_button");
    let thumb_img = thumb_style.normal.clone();
    let up_art = chat_arrow_style(asset_server, "chat_arrow_up");
    let up_img = up_art.normal.clone();
    let down_art = chat_arrow_style(asset_server, "chat_arrow_down");
    let down_img = down_art.normal.clone();
    let whisper_style = chat_button_style(asset_server, "chat_whisper_button");
    let hide_style = chat_button_style(asset_server, "chat_hide_button");
    let zoom_style = chat_button_style(asset_server, "chat_zoom");
    let mode_style = chat_button_style(asset_server, "chat_order_button");

    let input_font = fonts.three.clone();
    let whisper_title_font = fonts.nine.clone();
    let penalty_font = fonts.three.clone();

    let (wb_l, wb_t, wb_w, wb_h) = scaled(WHISPER_BTN, s);
    let (hb_l, hb_t, hb_w, hb_h) = scaled(HIDE_BTN, s);
    let (zb_l, zb_t, zb_w, zb_h) = scaled(ZOOM_BTN, s);
    let (mb_l, mb_t, mb_w, mb_h) = scaled(MODE_BTN, s);
    // the popup stacks its five rows plus both caps directly above the button
    let mode_popup_h = (MODE_ROW_H * 5.0 + MODE_CAP_H * 2.0) * s;
    let mode_popup_top = mb_t - mode_popup_h - MODE_POPUP_GAP * s;
    let (ir_l, ir_t, ir_w, ir_h) = scaled(INPUT_ROW, s);
    let (wp_l, wp_t, wp_w, wp_h) = scaled(WHISPER_PANEL, s);
    let (pen_l, pen_t, pen_w, pen_h) = scaled(PENALTY_RECT, s);

    let input_crop = crop(INPUT_BG_CROP);
    let whisper_crop = crop(WHISPER_PANEL_CROP);
    let bg_up_crop = Rect::new(0.0, 0.0, CHAT_BG_SLICE_W, CHAT_BG_SLICE_H);
    let bg_mid_crop = Rect::new(0.0, 4.0, CHAT_BG_SLICE_W, 8.0);
    let bg_down_crop = Rect::new(0.0, 8.0, CHAT_BG_SLICE_W, 12.0);
    let mode_up_crop = bg_up_crop;
    let mode_mid_crop = bg_mid_crop;
    let mode_down_crop = bg_down_crop;

    bsn! {
        ChatRoot
        Name("ChatWindow")
        Node {
            position_type: PositionType::Absolute,
            left: px(WINDOW_MARGIN.0 * s),
            bottom: px(WINDOW_MARGIN.1 * s),
            width: px(WINDOW_W * s),
            flex_direction: FlexDirection::Column,
        }
        GlobalZIndex(50)
        // hoverable (the idle fade un-fades on hover) but never blocking the
        // world behind the window
        Pickable { should_block_lower: false, is_hoverable: true }
        Hovered
        Children [
            // --- header: whisper/hide buttons + tab cells -------------------
            (
                Node {
                    width: percent(100),
                    height: px(HEADER_H * s),
                }
                Pickable::IGNORE
                Children [
                    (
                        image_button(whisper_style, wb_w, wb_h)
                        Node { position_type: PositionType::Absolute, left: px(wb_l), top: px(wb_t) }
                        on(|_: On<Activate>, mut state: ResMut<ChatState>| {
                            state.whisper_panel_open = !state.whisper_panel_open;
                        })
                    ),
                    (
                        image_button(hide_style, hb_w, hb_h)
                        Node { position_type: PositionType::Absolute, left: px(hb_l), top: px(hb_t) }
                        on(|_: On<Activate>, mut state: ResMut<ChatState>| {
                            state.collapsed = !state.collapsed;
                        })
                    ),
                    // tab strip: five fixed 51px cells at 45..300
                    (
                        Node {
                            position_type: PositionType::Absolute,
                            left: px(TAB_X0 * s),
                            top: px(0),
                            width: px(TAB_W * 5.0 * s),
                            height: px(TAB_H * s),
                            flex_direction: FlexDirection::Row,
                        }
                        Pickable::IGNORE
                        Children [
                            (tab_cell(asset_server, fonts, ChatTab::All)),
                            (tab_cell(asset_server, fonts, ChatTab::Party)),
                            (tab_cell(asset_server, fonts, ChatTab::Guild)),
                            (tab_cell(asset_server, fonts, ChatTab::Alliance)),
                            (tab_cell(asset_server, fonts, ChatTab::Academy)),
                        ]
                    ),
                ]
            ),
            // --- body: frame bg, message list, scrollbar --------------------
            (
                ChatBody
                Node {
                    width: percent(100),
                    height: px((LIST_H_SMALL + 13.0) * s),
                }
                Pickable::IGNORE
                Children [
                    (
                        ImageNode { image: {bg_up}, image_mode: NodeImageMode::Stretch, rect: {Some(bg_up_crop)} }
                        Node {
                            position_type: PositionType::Absolute,
                            left: px(BG_X * s), top: px(0),
                            width: px(BG_W * s), height: px(BG_CAP_H * s),
                        }
                        Pickable::IGNORE
                    ),
                    (
                        ImageNode { image: {bg_mid}, image_mode: NodeImageMode::Stretch, rect: {Some(bg_mid_crop)} }
                        Node {
                            position_type: PositionType::Absolute,
                            left: px(BG_X * s), top: px(BG_CAP_H * s), bottom: px(BG_CAP_H * s),
                            width: px(BG_W * s),
                        }
                        Pickable::IGNORE
                    ),
                    (
                        ImageNode { image: {bg_down}, image_mode: NodeImageMode::Stretch, rect: {Some(bg_down_crop)} }
                        Node {
                            position_type: PositionType::Absolute,
                            left: px(BG_X * s), bottom: px(0),
                            width: px(BG_W * s), height: px(BG_CAP_H * s),
                        }
                        Pickable::IGNORE
                    ),
                    // the scrollable list; wheel-scrolling handled by an observer
                    (
                        ChatMessageList
                        Hovered
                        Node {
                            position_type: PositionType::Absolute,
                            left: px(LIST_X * s), top: px(LIST_TOP * s), bottom: px(LIST_BOTTOM * s),
                            width: px(LIST_W * s),
                            flex_direction: FlexDirection::Column,
                            overflow: {Overflow::scroll_y()},
                        }
                        ScrollPosition(Vec2::ZERO)
                        on(|scroll: On<Pointer<Scroll>>,
                            mut list: Query<(&mut ScrollPosition, &ComputedNode), With<ChatMessageList>>,
                            mut state: ResMut<ChatState>| {
                            let Ok((mut pos, computed)) = list.single_mut() else { return };
                            let current = computed.scroll_position.y * computed.inverse_scale_factor;
                            scroll_list_to(&mut pos, computed, &mut state, current - scroll.event.y * WHEEL_STEP);
                        })
                    ),
                    // scrollbar spanning the body's full height:
                    // up arrow / track+thumb / down arrow
                    (
                        Node {
                            position_type: PositionType::Absolute,
                            left: px(0), top: px(0), bottom: px(0),
                            width: px(GUTTER_W * s),
                            flex_direction: FlexDirection::Column,
                        }
                        Pickable::IGNORE
                        Children [
                            (
                                ChatScrollArrow { up: true }
                                Button
                                Hovered
                                ImageNode { image: {up_img}, image_mode: NodeImageMode::Stretch }
                                ImageButtonStyle {
                                    normal: {up_art.normal},
                                    hover: {up_art.hover},
                                    press: {up_art.press},
                                    disable: {up_art.disable},
                                }
                                Node {
                                    width: px(GUTTER_W * s),
                                    height: px(GUTTER_W * s),
                                }
                                on(|_: On<Activate>,
                                    mut list: Query<(&mut ScrollPosition, &ComputedNode), With<ChatMessageList>>,
                                    mut state: ResMut<ChatState>| {
                                    scroll_list_by(&mut list, &mut state, -ARROW_STEP);
                                })
                            ),
                            // No track art: a CIFVerticalScroll is only a
                            // thumb + two arrows (ifverticalscroll.txt), so
                            // the chat backdrop shows through behind the thumb.
                            (
                                ChatScrollTrack
                                Node {
                                    width: px(GUTTER_W * s),
                                    flex_grow: 1.0,
                                }
                                Pickable::IGNORE
                                Children [
                                    (
                                        ChatScrollThumb
                                        // Button + Hovered + ImageButtonStyle feed
                                        // update_image_button_visuals for the
                                        // hover/press art (the widget button's
                                        // Activate is simply unused here)
                                        Button
                                        Hovered
                                        ImageNode { image: {thumb_img}, image_mode: NodeImageMode::Stretch }
                                        ImageButtonStyle {
                                            normal: {thumb_style.normal},
                                            hover: {thumb_style.hover},
                                            press: {thumb_style.press},
                                        }
                                        Node {
                                            position_type: PositionType::Absolute,
                                            left: px(0), top: px(0),
                                            width: px(GUTTER_W * hud_scale()),
                                            height: px(GUTTER_W * hud_scale()),
                                        }
                                        on(|drag: On<Pointer<DragStart>>,
                                            mut commands: Commands,
                                            list: Query<&ComputedNode, With<ChatMessageList>>| {
                                            let Ok(computed) = list.single() else { return };
                                            let current = computed.scroll_position.y * computed.inverse_scale_factor;
                                            commands.entity(drag.entity).insert(ChatThumbDrag(current));
                                        })
                                        on(|drag: On<Pointer<Drag>>,
                                            starts: Query<&ChatThumbDrag>,
                                            track: Query<&ComputedNode, With<ChatScrollTrack>>,
                                            mut list: Query<(&mut ScrollPosition, &ComputedNode), With<ChatMessageList>>,
                                            mut state: ResMut<ChatState>| {
                                            let Ok(ChatThumbDrag(start)) = starts.get(drag.entity) else { return };
                                            let Ok(track) = track.single() else { return };
                                            let Ok((mut pos, computed)) = list.single_mut() else { return };
                                            let track_h = track.size.y * track.inverse_scale_factor;
                                            if track_h <= 0.0 { return }
                                            let content_h = computed.content_size.y * computed.inverse_scale_factor;
                                            // thumb px -> scroll px: linear with ratio content/track
                                            let delta = drag.event.distance.y * content_h / track_h;
                                            scroll_list_to(&mut pos, computed, &mut state, start + delta);
                                        })
                                    ),
                                ]
                            ),
                            (
                                ChatScrollArrow { up: false }
                                Button
                                Hovered
                                ImageNode { image: {down_img}, image_mode: NodeImageMode::Stretch }
                                ImageButtonStyle {
                                    normal: {down_art.normal},
                                    hover: {down_art.hover},
                                    press: {down_art.press},
                                    disable: {down_art.disable},
                                }
                                Node {
                                    width: px(GUTTER_W * s),
                                    height: px(GUTTER_W * s),
                                }
                                on(|_: On<Activate>,
                                    mut list: Query<(&mut ScrollPosition, &ComputedNode), With<ChatMessageList>>,
                                    mut state: ResMut<ChatState>| {
                                    scroll_list_by(&mut list, &mut state, ARROW_STEP);
                                })
                            ),
                        ]
                    ),
                ]
            ),
            // --- bottom row: zoom button + input ----------------------------
            (
                ChatBottomRow
                Node {
                    width: percent(100),
                    height: px(BOTTOM_H * s),
                    margin: {UiRect::top(Val::Px(BOTTOM_GAP * hud_scale()))},
                }
                Pickable::IGNORE
                Children [
                    (
                        image_button(zoom_style, zb_w, zb_h)
                        Node { position_type: PositionType::Absolute, left: px(zb_l), top: px(zb_t) }
                        on(|_: On<Activate>, mut state: ResMut<ChatState>| {
                            state.expanded = !state.expanded;
                        })
                    ),
                    // GDR_CHAT_STA_PENALTY — outside the window to the right,
                    // hidden until the server restricts chat (0x302D)
                    (
                        ChatPenaltyLabel
                        label("", penalty_font, TAB_FONT_SIZE * hud_scale())
                        TextColor(PENALTY_COLOR)
                        Node {
                            position_type: PositionType::Absolute,
                            left: px(pen_l), top: px(pen_t),
                            width: px(pen_w), height: px(pen_h),
                            display: Display::None,
                        }
                        Pickable::IGNORE
                    ),
                    // GDR_CHAT_MODE_BTN + its CreateChatMode dropdown body
                    (
                        ChatModeButton
                        image_button(mode_style, mb_w, mb_h)
                        Node { position_type: PositionType::Absolute, left: px(mb_l), top: px(mb_t) }
                        on(|_: On<Activate>, mut state: ResMut<ChatState>| {
                            state.mode_open = !state.mode_open;
                        })
                    ),
                    (
                        ChatModePopup
                        Node {
                            position_type: PositionType::Absolute,
                            left: px(mb_l), top: px(mode_popup_top),
                            width: px(MODE_W * s),
                            height: px(mode_popup_h),
                            flex_direction: FlexDirection::Column,
                            display: Display::None,
                        }
                        GlobalZIndex(51)
                        Children [
                            (
                                ImageNode { image: {mode_bg_up}, image_mode: NodeImageMode::Stretch, rect: {Some(mode_up_crop)} }
                                Node { width: percent(100), height: px(MODE_CAP_H * s) }
                                Pickable::IGNORE
                            ),
                            (
                                ImageNode { image: {mode_bg_mid}, image_mode: NodeImageMode::Stretch, rect: {Some(mode_mid_crop)} }
                                Node {
                                    width: percent(100),
                                    height: px(MODE_ROW_H * 5.0 * s),
                                    flex_direction: FlexDirection::Column,
                                }
                                Children [
                                    (mode_row(fonts, ui_strings, ChatTab::All)),
                                    (mode_row(fonts, ui_strings, ChatTab::Party)),
                                    (mode_row(fonts, ui_strings, ChatTab::Guild)),
                                    (mode_row(fonts, ui_strings, ChatTab::Alliance)),
                                    (mode_row(fonts, ui_strings, ChatTab::Academy)),
                                ]
                            ),
                            (
                                ImageNode { image: {mode_bg_down}, image_mode: NodeImageMode::Stretch, rect: {Some(mode_down_crop)} }
                                Node { width: percent(100), height: px(MODE_CAP_H * s) }
                                Pickable::IGNORE
                            ),
                        ]
                    ),
                    (
                        ChatInputRow
                        ImageNode { image: {atlas_input}, image_mode: NodeImageMode::Stretch, rect: {Some(input_crop)} }
                        Node {
                            position_type: PositionType::Absolute,
                            left: px(ir_l), top: px(ir_t),
                            width: px(ir_w), height: px(ir_h),
                            padding: {UiRect::horizontal(Val::Px(6.0 * hud_scale()))},
                        }
                        Children [
                            (
                                text_input(input_font, 0)
                                ChatInputBox
                            ),
                        ]
                    ),
                ]
            ),
            // --- whisper-partner panel (floats above the tab bar) -----------
            (
                ChatWhisperPanel
                ImageNode { image: {atlas_whisper}, image_mode: NodeImageMode::Stretch, rect: {Some(whisper_crop)} }
                Node {
                    position_type: PositionType::Absolute,
                    left: px(wp_l), top: px(wp_t),
                    width: px(wp_w), height: px(wp_h),
                    display: Display::None,
                    flex_direction: FlexDirection::Column,
                    padding: {UiRect::axes(Val::Px(10.0 * hud_scale()), Val::Px(8.0 * hud_scale()))},
                }
                Children [
                    (
                        label(&whisper_title, whisper_title_font, TAB_FONT_SIZE * hud_scale())
                        TextColor(Color::WHITE)
                        Node { height: px(14.0 * hud_scale()) }
                    ),
                    (
                        ChatWhisperEntries
                        Node {
                            width: percent(100),
                            flex_grow: 1.0,
                            flex_direction: FlexDirection::Column,
                            overflow: {Overflow::clip()},
                        }
                        Pickable::IGNORE
                    ),
                ]
            ),
        ]
    }
}

/// One fixed-width tab cell: `chat_tab` background (with its baked lamp
/// recess), the lamp overlay at its absolute resinfo offset inside that
/// recess, and a centered label.
fn tab_cell(asset_server: &AssetServer, fonts: &FontAssets, tab: ChatTab) -> impl Scene {
    let cell_bg: Handle<Image> = asset_server.load(TAB_BG);
    let lamp: Handle<Image> = asset_server.load(lamp_texture(tab));
    let font = fonts.nine.clone();
    let text = tab.label();

    let art_crop = Rect::new(0.0, 0.0, TAB_ART.0, TAB_ART.1);

    bsn! {
        ChatTabCell { tab: {tab} }
        Button
        Hovered
        ImageNode { image: {cell_bg}, image_mode: NodeImageMode::Stretch, rect: {Some(art_crop)} }
        Node {
            width: px(TAB_W * hud_scale()),
            height: percent(100),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
        }
        on(|activate: On<Activate>, cells: Query<&ChatTabCell>, mut state: ResMut<ChatState>| {
            let Ok(cell) = cells.get(activate.entity) else { return };
            if state.active_tab != cell.tab {
                state.active_tab = cell.tab;
                state.stick_to_bottom = true;
                // The tab strip and the mode dropdown are separate controls in
                // the data, but reading a channel and then typing into it is
                // one gesture: moving the tab moves the send mode with it,
                // which is exactly what this window did before the dropdown
                // existed. The dropdown is how you type into a channel you are
                // not currently reading.
                state.mode = cell.tab;
            }
        })
        Children [
            (
                ChatTabLamp { tab: {tab} }
                ImageNode {
                    image: {lamp},
                    image_mode: NodeImageMode::Stretch,
                }
                Node {
                    position_type: PositionType::Absolute,
                    left: px(LAMP_OFFSET.0 * hud_scale()),
                    top: px(LAMP_OFFSET.1 * hud_scale()),
                    width: px(LAMP_SIZE.0 * hud_scale()),
                    height: px(LAMP_SIZE.1 * hud_scale()),
                }
                Pickable::IGNORE
            ),
            (
                ChatTabLabel { tab: {tab} }
                label(text, font, TAB_FONT_SIZE * hud_scale())
            ),
        ]
    }
}

/// Text key of one chat-mode entry. The dropdown's own file carries no
/// string keys (`Text=""` on every control), but the vocabulary is in the
/// text table: `UIIT_CTL_CHAT_ALL` = "All" plus the four-key
/// `UIIT_CTL_CHATMENU_*` run — "#Party", "@Guild", "%Union", "&Academy",
/// which are the sigil captions this dropdown selects with (textuisystem.txt;
/// the whole run is exactly four entries, so the mode list is these five and
/// no more).
fn mode_label_key(tab: ChatTab) -> &'static str {
    match tab {
        ChatTab::All => "UIIT_CTL_CHAT_ALL",
        ChatTab::Party => "UIIT_CTL_CHATMENU_PARTY",
        ChatTab::Guild => "UIIT_CTL_CHATMENU_GUILD",
        ChatTab::Alliance => "UIIT_CTL_CHATMENU_ALLY",
        ChatTab::Academy => "UIIT_CTL_CHATMENU_TRAININGCAMP",
    }
}

/// One dropdown entry: a 136x20 clickable row carrying the mode's label.
fn mode_row(fonts: &FontAssets, ui_strings: &ClientUiStrings, tab: ChatTab) -> impl Scene {
    let font = fonts.nine.clone();
    // the table's captions carry the channel sigils, same as the tab strip
    let text = ui_strings
        .get_or(mode_label_key(tab), tab.label())
        .to_string();

    bsn! {
        ChatModeRow { tab: {tab} }
        Button
        Hovered
        Node {
            width: px(MODE_W * hud_scale()),
            height: px(MODE_ROW_H * hud_scale()),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
        }
        on(|activate: On<Activate>, rows: Query<&ChatModeRow>, mut state: ResMut<ChatState>| {
            let Ok(row) = rows.get(activate.entity) else { return };
            state.mode = row.tab;
            state.mode_open = false;
        })
        Children [
            (
                ChatModeRowLabel { tab: {tab} }
                label(&text, font, TAB_FONT_SIZE * hud_scale())
            ),
        ]
    }
}

fn lamp_texture(tab: ChatTab) -> &'static str {
    match tab {
        ChatTab::All => "media://interface/chattingwnd/chat_lamp_all.ddj",
        ChatTab::Party => "media://interface/chattingwnd/chat_lamp_party.ddj",
        ChatTab::Guild => "media://interface/chattingwnd/chat_lamp_guild.ddj",
        ChatTab::Alliance => "media://interface/chattingwnd/chat_lamp_commerce.ddj",
        ChatTab::Academy => "media://interface/chattingwnd/chat_lamp_probation.ddj",
    }
}

/// Active-tab label colors: the RGB of each lamp's resinfo `FontColor`, which
/// is stored **ARGB** — take components 2-4, not 1-3. All/Party/Guild happen to
/// be identical under either reading (their alpha and red are both 255), which
/// is why only the last two ever looked wrong.
fn tab_label_color(tab: ChatTab) -> Color {
    match tab {
        // ifchatviewer.txt:127  255,255,255,255
        ChatTab::All => Color::WHITE,
        // :108  255,255,245,122
        ChatTab::Party => Color::srgb_u8(255, 245, 122),
        // :89   255,255,186,77
        ChatTab::Guild => Color::srgb_u8(255, 186, 77),
        // :70   255,239,153,255
        ChatTab::Alliance => Color::srgb_u8(239, 153, 255),
        // :50   255,239,153,255 (GDR_STATIC_APPRENTICE)
        ChatTab::Academy => Color::srgb_u8(239, 153, 255),
    }
}

fn chat_button_style(asset_server: &AssetServer, stem: &str) -> ImageButtonStyle {
    ImageButtonStyle {
        normal: asset_server.load(format!("media://interface/chattingwnd/{stem}.ddj")),
        hover: asset_server.load(format!("media://interface/chattingwnd/{stem}_focus.ddj")),
        press: asset_server.load(format!("media://interface/chattingwnd/{stem}_press.ddj")),
        ..Default::default()
    }
}

/// Scroll-arrow style: like [`chat_button_style`] but with the `_disable`
/// frame the arrows ship (`chat_arrow_{up,down}_disable.ddj`).
fn chat_arrow_style(asset_server: &AssetServer, stem: &str) -> ImageButtonStyle {
    ImageButtonStyle {
        disable: asset_server.load(format!("media://interface/chattingwnd/{stem}_disable.ddj")),
        ..chat_button_style(asset_server, stem)
    }
}

fn common_button_style(asset_server: &AssetServer, stem: &str) -> ImageButtonStyle {
    ImageButtonStyle {
        normal: asset_server.load(format!("media://interface/ifcommon/{stem}.ddj")),
        hover: asset_server.load(format!("media://interface/ifcommon/{stem}_focus.ddj")),
        press: asset_server.load(format!("media://interface/ifcommon/{stem}_press.ddj")),
        ..Default::default()
    }
}

fn scaled(rect: (f32, f32, f32, f32), s: f32) -> (f32, f32, f32, f32) {
    (rect.0 * s, rect.1 * s, rect.2 * s, rect.3 * s)
}

/// (x0, y0, x1, y1) pixel crop -> `ImageNode::rect`.
fn crop(r: (f32, f32, f32, f32)) -> Rect {
    Rect::new(r.0, r.1, r.2, r.3)
}

// --- Scrolling helpers ------------------------------------------------------

/// Clamp-and-set the list scroll (logical px) and refresh stick-to-bottom.
fn scroll_list_to(
    pos: &mut ScrollPosition,
    computed: &ComputedNode,
    state: &mut ChatState,
    new_y: f32,
) {
    let max = (computed.content_size.y - computed.size.y).max(0.0) * computed.inverse_scale_factor;
    let y = new_y.clamp(0.0, max);
    pos.0.y = y;
    state.stick_to_bottom = y >= max - 1.0;
}

fn scroll_list_by(
    list: &mut Query<(&mut ScrollPosition, &ComputedNode), With<ChatMessageList>>,
    state: &mut ChatState,
    delta: f32,
) {
    let Ok((mut pos, computed)) = list.single_mut() else {
        return;
    };
    let current = computed.scroll_position.y * computed.inverse_scale_factor;
    scroll_list_to(&mut pos, computed, state, current + delta);
}

// --- Update systems ---------------------------------------------------------

/// Run condition (cheap pre-gate): the history or some state changed, or the
/// list was just (re)spawned. [`refresh_chat_list`] narrows it further — state
/// changes every wheel tick (`stick_to_bottom`), which must not rebuild rows.
pub fn chat_needs_refresh(
    history: Res<ChatHistory>,
    state: Res<ChatState>,
    fresh: Query<(), Added<ChatMessageList>>,
) -> bool {
    history.is_changed() || state.is_changed() || !fresh.is_empty()
}

/// Rebuild the message rows for the active tab (newest at the bottom).
pub fn refresh_chat_list(
    mut commands: Commands,
    history: Res<ChatHistory>,
    state: Res<ChatState>,
    fonts: Res<FontAssets>,
    colors: Res<ChatColors>,
    ui_strings: Res<ClientUiStrings>,
    vitals: Res<PlayerVitals>,
    list: Query<Entity, With<ChatMessageList>>,
    fresh: Query<(), Added<ChatMessageList>>,
    mut last_tab: Local<Option<ChatTab>>,
) {
    let tab_changed = *last_tab != Some(state.active_tab);
    if !history.is_changed() && !tab_changed && fresh.is_empty() {
        return;
    }
    *last_tab = Some(state.active_tab);
    let Ok(list) = list.single() else {
        return;
    };
    let s = hud_scale();
    let font = fonts.three.clone();
    let whisper_from = ui_strings.get_or(WHISPER_FROM_KEY, WHISPER_FROM_FALLBACK);
    let whisper_to = ui_strings.get_or(WHISPER_TO_KEY, WHISPER_TO_FALLBACK);

    let visible: Vec<_> = history
        .iter()
        .filter(|line| state.active_tab.shows(line.kind))
        .collect();
    let skip = visible.len().saturating_sub(MAX_RENDERED_LINES);

    commands.entity(list).despawn_related::<Children>();
    commands.entity(list).with_children(|parent| {
        for line in visible.into_iter().skip(skip) {
            let row = (
                ChatMessageRow,
                Text(line.display_with(whisper_from, whisper_to)),
                TextFont {
                    font: font.clone().into(),
                    font_size: FontSize::Px(MESSAGE_FONT_SIZE * s),
                    ..default()
                },
                TextColor(line.kind.color(&colors)),
                TextLayout::new(Justify::Left, LineBreak::WordBoundary),
                Node {
                    width: percent(100.0),
                    flex_shrink: 0.0,
                    ..default()
                },
            );
            // clicking a line with a player sender starts a whisper to them
            // (except your own lines — you can't whisper yourself)
            match &line.sender {
                Some(name)
                    if line.kind.sender_is_whisper_target()
                        && !name.eq_ignore_ascii_case(&vitals.name) =>
                {
                    let name = name.clone();
                    parent.spawn((row, Button, Hovered::default())).observe(
                        move |_: On<Activate>,
                              mut focus: ResMut<InputFocus>,
                              mut state: ResMut<ChatState>,
                              mut input: Query<
                            (Entity, &mut EditableText),
                            With<ChatInputBox>,
                        >| {
                            super::input::prefill_whisper_input(
                                &name, &mut focus, &mut state, &mut input,
                            );
                        },
                    );
                }
                _ => {
                    parent.spawn((row, Pickable::IGNORE));
                }
            }
        }
    });
}

/// Keep the list glued to the newest line while `stick_to_bottom` holds.
/// `f32::MAX` is fine — the layout pass clamps and writes the real value back.
pub fn apply_chat_scroll(
    history: Res<ChatHistory>,
    state: Res<ChatState>,
    mut list: Query<&mut ScrollPosition, With<ChatMessageList>>,
) {
    if !state.stick_to_bottom {
        return;
    }
    if !(history.is_changed() || state.is_changed()) {
        return;
    }
    for mut pos in list.iter_mut() {
        pos.0.y = f32::MAX;
    }
}

/// Mark a scroll arrow `InteractionDisabled` when it can't scroll any further
/// (or there is nothing to scroll), so the widget button stops emitting
/// `Activate`. The art swap follows from that: the arrows carry an
/// `ImageButtonStyle` whose `disable` slot is the `chat_arrow_*_disable.ddj`
/// frame, and ui_v2's shared `update_image_button_visuals` paints it.
pub fn update_chat_scroll_arrows(
    mut commands: Commands,
    list: Query<&ComputedNode, With<ChatMessageList>>,
    arrows: Query<(Entity, &ChatScrollArrow, Has<InteractionDisabled>)>,
) {
    let Ok(list) = list.single() else {
        return;
    };
    let max_scroll = (list.content_size.y - list.size.y).max(0.0);
    let at_top = list.scroll_position.y <= 0.5;
    let at_bottom = list.scroll_position.y >= max_scroll - 0.5;
    let nothing_to_scroll = max_scroll <= 0.5;

    for (entity, arrow, was_disabled) in arrows.iter() {
        let disabled = nothing_to_scroll || if arrow.up { at_top } else { at_bottom };
        if disabled == was_disabled {
            continue;
        }
        if disabled {
            commands.entity(entity).insert(InteractionDisabled);
        } else {
            commands.entity(entity).remove::<InteractionDisabled>();
        }
    }
}

/// Place the fixed-size square thumb (vanilla style) from the list's scroll
/// geometry; docked at the top while the content still fits the viewport.
pub fn update_chat_scroll_thumb(
    list: Query<&ComputedNode, With<ChatMessageList>>,
    track: Query<&ComputedNode, With<ChatScrollTrack>>,
    mut thumb: Query<&mut Node, With<ChatScrollThumb>>,
) {
    let (Ok(list), Ok(track)) = (list.single(), track.single()) else {
        return;
    };
    let Ok(mut node) = thumb.single_mut() else {
        return;
    };

    let content = list.content_size.y;
    let viewport = list.size.y;
    let track_h = track.size.y * track.inverse_scale_factor;
    let thumb_h = GUTTER_W * hud_scale();
    let progress = if content > viewport {
        (list.scroll_position.y / (content - viewport)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let top = progress * (track_h - thumb_h).max(0.0);
    // compare-before-write so an idle scrollbar doesn't dirty the layout
    let new_top = px(top);
    if node.top != new_top {
        node.top = new_top;
    }
}

/// Tint tab cells/lamps/labels by the active tab.
/// Show/hide the chat-mode dropdown and mark the selected entry. The rows are
/// tinted like the tab strip (the selected one in its channel colour, the rest
/// in the shared inactive grey) — the data gives the dropdown no per-entry
/// colours, so it borrows the tab's, which are the same five channels.
pub fn update_chat_mode_popup(
    state: Res<ChatState>,
    mut popups: Query<&mut Node, With<ChatModePopup>>,
    mut labels: Query<(&ChatModeRowLabel, &mut TextColor)>,
    fresh: Query<(), Added<ChatModePopup>>,
) {
    if !state.is_changed() && fresh.is_empty() {
        return;
    }
    for mut node in popups.iter_mut() {
        node.display = if state.mode_open {
            Display::Flex
        } else {
            Display::None
        };
    }
    for (row, mut color) in labels.iter_mut() {
        color.0 = if row.tab == state.mode {
            tab_label_color(row.tab)
        } else {
            INACTIVE_LABEL
        };
    }
}

pub fn update_chat_tab_visuals(
    state: Res<ChatState>,
    mut cells: Query<(&ChatTabCell, &mut ImageNode), (Without<ChatTabLamp>, Without<ChatTabLabel>)>,
    mut lamps: Query<(&ChatTabLamp, &mut ImageNode), Without<ChatTabCell>>,
    mut labels: Query<(&ChatTabLabel, &mut TextColor)>,
    fresh: Query<(), Added<ChatTabCell>>,
    mut last_tab: Local<Option<ChatTab>>,
) {
    if *last_tab == Some(state.active_tab) && fresh.is_empty() {
        return;
    }
    *last_tab = Some(state.active_tab);
    for (cell, mut image) in cells.iter_mut() {
        image.color = if cell.tab == state.active_tab {
            Color::WHITE
        } else {
            INACTIVE_TINT
        };
    }
    for (lamp, mut image) in lamps.iter_mut() {
        image.color = if lamp.tab == state.active_tab {
            Color::WHITE
        } else {
            INACTIVE_TINT
        };
    }
    for (label, mut color) in labels.iter_mut() {
        color.0 = if label.tab == state.active_tab {
            tab_label_color(label.tab)
        } else {
            INACTIVE_LABEL
        };
    }
}

/// Apply the hide/zoom/whisper-panel modes to node display/heights. The input
/// row is always visible (only its *focus* is Enter-toggled).
pub fn apply_chat_window_mode(
    state: Res<ChatState>,
    mut body: Query<
        &mut Node,
        (
            With<ChatBody>,
            Without<ChatBottomRow>,
            Without<ChatWhisperPanel>,
        ),
    >,
    mut bottom: Query<
        &mut Node,
        (
            With<ChatBottomRow>,
            Without<ChatBody>,
            Without<ChatWhisperPanel>,
        ),
    >,
    mut whisper: Query<
        &mut Node,
        (
            With<ChatWhisperPanel>,
            Without<ChatBody>,
            Without<ChatBottomRow>,
        ),
    >,
    fresh: Query<(), Added<ChatBody>>,
    mut last_modes: Local<Option<(bool, bool, bool)>>,
) {
    let modes = (state.collapsed, state.expanded, state.whisper_panel_open);
    if *last_modes == Some(modes) && fresh.is_empty() {
        return;
    }
    *last_modes = Some(modes);
    let list_h = if state.expanded {
        LIST_H_LARGE
    } else {
        LIST_H_SMALL
    };
    for mut node in body.iter_mut() {
        node.height = px((list_h + 13.0) * hud_scale());
        node.display = if state.collapsed {
            Display::None
        } else {
            Display::Flex
        };
    }
    for mut node in bottom.iter_mut() {
        node.display = if state.collapsed {
            Display::None
        } else {
            Display::Flex
        };
    }
    for mut node in whisper.iter_mut() {
        node.display = if state.whisper_panel_open && !state.collapsed {
            Display::Flex
        } else {
            Display::None
        };
    }
}

/// Idle fade: without chat activity the window chrome fades out over
/// [`FADE_DURATION_SECS`] after [`FADE_AFTER_SECS`] — leaving only the bare
/// message text visible — and snaps back on any activity: a new line, Enter,
/// tab/button clicks (all of which touch the history or the state) or hovering
/// the window area. Applied by walking the subtree and scaling the alpha of
/// every `ImageNode`/`TextColor` (all chat art uses alpha-1 tints, so setting
/// the alpha directly is lossless); [`ChatMessageRow`] texts are exempt.
///
/// **Not original behaviour** — the v1.188 client fades nothing on idle and
/// offers a manual transparency slider instead, so this runs only when
/// `chat.idle_fade` is set (docs/re/ui/hud-chat.md §6-13, §9-U9).
#[allow(clippy::too_many_arguments)]
pub fn fade_chat_window(
    config: Res<ClientConfig>,
    time: Res<Time>,
    history: Res<ChatHistory>,
    state: Res<ChatState>,
    roots: Query<(Entity, &Hovered), With<ChatRoot>>,
    children: Query<&Children>,
    mut images: Query<&mut ImageNode>,
    mut texts: Query<&mut TextColor, Without<ChatMessageRow>>,
    mut last_activity: Local<f64>,
    mut applied: Local<Option<f32>>,
) {
    if !config.chat.idle_fade {
        return;
    }
    let now = time.elapsed_secs_f64();
    let hovered = roots.iter().any(|(_, hovered)| hovered.get());
    if hovered || history.is_changed() || state.is_changed() || *last_activity == 0.0 {
        *last_activity = now;
    }
    let idle = (now - *last_activity) as f32;
    let alpha = if state.input_open || idle <= FADE_AFTER_SECS {
        1.0
    } else {
        (1.0 - (idle - FADE_AFTER_SECS) / FADE_DURATION_SECS).clamp(0.0, 1.0)
    };
    if *applied == Some(alpha) {
        return;
    }
    *applied = Some(alpha);

    for (root, _) in roots.iter() {
        let mut stack = vec![root];
        while let Some(entity) = stack.pop() {
            if let Ok(mut image) = images.get_mut(entity) {
                image.color.set_alpha(alpha);
            }
            if let Ok(mut text) = texts.get_mut(entity) {
                text.0.set_alpha(alpha);
            }
            if let Ok(kids) = children.get(entity) {
                stack.extend(kids.iter());
            }
        }
    }
}

/// Drive `GDR_CHAT_STA_PENALTY`: the remaining restriction seconds while the
/// server's 0x302D mute runs, hidden otherwise.
pub fn update_chat_penalty(
    time: Res<Time>,
    state: Res<ChatState>,
    ui_strings: Res<ClientUiStrings>,
    mut labels: Query<(&mut Text, &mut Node), With<ChatPenaltyLabel>>,
) {
    let remaining = state
        .restricted_until
        .map(|until| until - time.elapsed_secs_f64());
    let text = match remaining {
        Some(secs) if secs > 0.0 => Some(format_restriction(
            ui_strings.get_or(CANT_CHATTING_KEY, CANT_CHATTING_FALLBACK),
            secs.ceil() as u32,
        )),
        _ => None,
    };
    for (mut label, mut node) in labels.iter_mut() {
        match &text {
            Some(text) => {
                if label.0 != *text {
                    label.0.clone_from(text);
                }
                if node.display != Display::Flex {
                    node.display = Display::Flex;
                }
            }
            None => {
                if node.display != Display::None {
                    node.display = Display::None;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ifchatviewer.txt` ships both `#ifdef` branches for the input box and
    /// `UI_UPDATE_2009_FIRST` is defined (`Media/config/define.txt:17`), so the
    /// live rect is the 2009 one. We used to transcribe the `#else` branch.
    #[test]
    fn input_row_uses_the_2009_ifdef_branch() {
        assert_eq!(INPUT_ROW, (38.0, 0.0, 360.0, 20.0));
        // the legacy branch started at 18 and ran 381 wide; the 20px the 2009
        // branch gives up there is exactly GDR_CHAT_MODE_BTN (:343, 18,378,20,20)
        assert_eq!(INPUT_ROW.0 - 18.0, 20.0);
        // the 2009 box also stops 1px short of the legacy one's right edge
        assert_eq!(INPUT_ROW.0 + INPUT_ROW.2, 398.0);
        assert_eq!(18.0 + 381.0, 399.0);
    }

    /// The chat-mode button fills the gap the 2009 input rect leaves, and the
    /// dropdown body is the `CreateChatMode` section's geometry: 136 wide,
    /// 4px caps, one 20px row per entry.
    #[test]
    fn chat_mode_dropdown_matches_its_resinfo_section() {
        // GDR_CHAT_MODE_BTN 18,378,20,20 — bottom-row local y 0
        assert_eq!(MODE_BTN, (18.0, 0.0, 20.0, 20.0));
        assert_eq!(MODE_BTN.0 + MODE_BTN.2, INPUT_ROW.0);
        // GDR_CHAT_MODE_BG_UP 0,0,136,4 / _BG_DOWN 0,24,136,4 / rows 136x20
        assert_eq!(MODE_W, 136.0);
        assert_eq!(MODE_CAP_H, 4.0);
        assert_eq!(MODE_ROW_H, 20.0);
        // the section's own stack: up cap, one row, down cap = the 0,24 offset
        // the lower cap is authored at
        assert_eq!(MODE_CAP_H + MODE_ROW_H, 24.0);
    }

    /// The dropdown file carries no strings (`Text=""` everywhere); its five
    /// entries are the text table's `UIIT_CTL_CHAT_ALL` plus the complete
    /// four-key `UIIT_CTL_CHATMENU_*` run.
    #[test]
    fn chat_mode_entries_come_from_the_chatmenu_key_run() {
        assert_eq!(mode_label_key(ChatTab::All), "UIIT_CTL_CHAT_ALL");
        assert_eq!(mode_label_key(ChatTab::Party), "UIIT_CTL_CHATMENU_PARTY");
        assert_eq!(mode_label_key(ChatTab::Guild), "UIIT_CTL_CHATMENU_GUILD");
        assert_eq!(mode_label_key(ChatTab::Alliance), "UIIT_CTL_CHATMENU_ALLY");
        assert_eq!(
            mode_label_key(ChatTab::Academy),
            "UIIT_CTL_CHATMENU_TRAININGCAMP"
        );
    }

    /// The five lamp rects are the only data anchor for the code-drawn tab
    /// strip: x-run 50/101/152/203/254 (pitch 51), each 4x8, all at y=6.
    #[test]
    fn tab_strip_geometry_follows_the_lamp_run() {
        let lamp_x = [50.0, 101.0, 152.0, 203.0, 254.0];
        for (i, x) in lamp_x.iter().enumerate() {
            let cell_x = TAB_X0 + TAB_W * i as f32;
            assert_eq!(x - cell_x, LAMP_OFFSET.0, "lamp {i} offset in its cell");
        }
        // pitch equals the cell width, and the strip ends well short of 399
        assert_eq!(lamp_x[1] - lamp_x[0], TAB_W);
        assert_eq!(TAB_X0 + TAB_W * 5.0, 300.0);
        assert_eq!(LAMP_SIZE, (4.0, 8.0));
        // chat_tab.ddj is 52x20 with a transparent right column and 2 bottom
        // rows -> the visible art is exactly one cell wide
        assert_eq!(TAB_ART, (TAB_W, 18.0));
    }

    /// The lamp `FontColor`s are ARGB; reading components 1-3 instead of 2-4
    /// silently worked for All/Party/Guild (alpha and red are both 255) and
    /// broke Alliance, while Academy was invented outright.
    #[test]
    fn tab_label_colors_are_the_argb_rgb_components() {
        assert_eq!(tab_label_color(ChatTab::All), Color::WHITE);
        assert_eq!(
            tab_label_color(ChatTab::Party),
            Color::srgb_u8(255, 245, 122)
        );
        assert_eq!(
            tab_label_color(ChatTab::Guild),
            Color::srgb_u8(255, 186, 77)
        );
        // both carry FontColor 255,239,153,255
        assert_eq!(
            tab_label_color(ChatTab::Alliance),
            Color::srgb_u8(239, 153, 255)
        );
        assert_eq!(
            tab_label_color(ChatTab::Academy),
            Color::srgb_u8(239, 153, 255)
        );
    }

    /// `GDR_WHISPERLIST` is `16,-152,141,153`; the window rect's x is 0.
    #[test]
    fn transcribed_rects_match_the_resinfo() {
        assert_eq!(WHISPER_PANEL, (16.0, -152.0, 141.0, 153.0));
        assert_eq!(WINDOW_MARGIN.0, 0.0);
        assert_eq!(WINDOW_W, 399.0);
        // collapsed window = 138 total between board top 546 and underbar 684:
        // header + the body's 13px of bg caps/insets + gap + bottom row
        let chrome = HEADER_H + 13.0 + BOTTOM_GAP + BOTTOM_H;
        assert_eq!(LIST_H_SMALL + chrome, 138.0);
    }

    /// Each of the five tabs maps to its own lamp texture; Alliance uses
    /// `commerce` and Academy uses `probation` (there is no
    /// `chat_lamp_apprenticeship.ddj`).
    #[test]
    fn lamp_textures_are_distinct_and_use_the_shipped_names() {
        let all = [
            ChatTab::All,
            ChatTab::Party,
            ChatTab::Guild,
            ChatTab::Alliance,
            ChatTab::Academy,
        ];
        let paths: Vec<&str> = all.iter().copied().map(lamp_texture).collect();
        let mut unique = paths.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), paths.len(), "lamp textures must be distinct");
        assert!(lamp_texture(ChatTab::Alliance).ends_with("chat_lamp_commerce.ddj"));
        assert!(lamp_texture(ChatTab::Academy).ends_with("chat_lamp_probation.ddj"));
    }

    /// `GDR_CHAT_STA_PENALTY` is a 160x20 static at window `404,380` — level
    /// with the input row (y 378) and deliberately **outside** the 399-wide
    /// window, so it is stored bottom-row local at y 2.
    #[test]
    fn penalty_static_matches_gdr_chat_sta_penalty() {
        assert_eq!(PENALTY_RECT, (404.0, 2.0, 160.0, 20.0));
        assert_eq!(PENALTY_RECT.1 + INPUT_ROW.1, 380.0 - 378.0);
        assert!(
            PENALTY_RECT.0 > WINDOW_W,
            "the readout sits outside the window"
        );
        assert_eq!(PENALTY_COLOR, Color::srgb_u8(160, 247, 153));
    }
}
