//! NPC dialog window layout + interaction.
//!
//! Idea: the conversation lives in the shared `game_window` shell (mframe
//! chrome, title band with the NPC's name, close (X) button, drag-to-move)
//! carrying vanilla's nested `npc_conversation_window_` frame: the shell is
//! `GDR_NPCWINDOW` 386x451 and the conversation frame `GDR_NW_NPCTALK`
//! 364x391 sits inside it at 11,48, both straight from the data.
//! Inside it sits vanilla's single scrolling talk box with its matched
//! 16px scrollbar. The box holds a flex column:
//! the wrapped speech (npcchat greeting/talk strings out of
//! textquest_speech&name, with an `SN_<codename>_BS/_PS` fallback for NPCs
//! without an npcchat row) and the option lines directly beneath it —
//! derived from DATA (shop table / teleporter table / speech presence), not
//! the spawn record's unreliable option bits (those are only debug-logged).
//! The window rebuilds from [`NpcDialogState`] on every state change (a
//! dragged position survives) and tears down via the deferred
//! `DialogClosing` PostUpdate despawn so the closing click never falls
//! through to click-to-move.

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui::{ComputedNode, Overflow, ScrollPosition};
use bevy::ui_widgets::{Activate, Button};
use bevy::window::PrimaryWindow;

use crate::assets::FontAssets;
use crate::net::connection::SilkroadConnection;
use crate::plugins::cursor::interactions::npcs::TalkOption;
use crate::plugins::hud::game_window;
use crate::plugins::hud::npc_dialog::model::{
    DialogPage, NpcDialogState, OpenGuildStorage, OpenStorage, OpenStore, OpenTeleport,
};
use crate::plugins::hud::scale::hud_scale;
use crate::plugins::net::agent::AgentConnection;
use crate::plugins::net::entities::{CharacterRef, DisplayName, NetworkId, NpcTalkOptions};
use crate::plugins::textdata::{
    ClientCharacterData, ClientNpcChat, ClientShops, ClientSpeechText, ClientTeleport,
    ClientUiStrings,
};
use crate::plugins::ui_v2::style::ImageButtonStyle;

/// Both of vanilla's frames, as authored. The `mframe_wnd_` shell
/// `GDR_NPCWINDOW` is `200,200,386,451` (`ginterface.txt:495`) and it nests a
/// dedicated `npc_conversation_window_` 9-slice `GDR_NW_NPCTALK` at
/// `11,48,364,391` (`if_npcwindow.txt:6-24`).
///
/// Drawing only the shell is what used to leave a published **+2 / -8
/// residual**: deriving the outer size from the 364x391 content through the
/// shell's own `(12,36,12,12)` margins gives 388x443, while the data says
/// 386x451 with margins `(11,48,11,12)`. The 12 px of top inset that the
/// derivation could not account for *was* the missing inner frame, so now both
/// numbers are taken from the data and there is no residual left to publish.
const OUTER_SIZE: (f32, f32) = (386.0, 451.0);
const INNER_FRAME_RECT: (f32, f32, f32, f32) = (11.0, 48.0, 364.0, 391.0);
/// The inner frame's 8 pieces are 20x20 corners with 128-long mids.
const INNER_FRAME_CORNER: f32 = 20.0;
const INNER_FRAME_DIR: &str = "media://interface/npc/npc_conversation_window_";
const CONTENT_W: f32 = INNER_FRAME_RECT.2;
const CONTENT_H: f32 = INNER_FRAME_RECT.3;
/// `if_npctalk.txt`, rects local to the 364x391 conversation frame:
/// `GDR_NT_TALKBOX:CIFTextBox` `23,23,302,343` and its matched
/// `GDR_NPCTALK_SCROLL:CIFVerticalScroll` `336,27,16,320`
/// (23 + 302 + 11 = 336).
const TALKBOX: (f32, f32, f32, f32) = (23.0, 23.0, 302.0, 343.0);
const SCROLL_RECT: (f32, f32, f32, f32) = (336.0, 27.0, 16.0, 320.0);
/// One arrow click scrolls two option lines' worth.
const SCROLL_ARROW_STEP: f32 = 40.0;
/// First-open screen placement (physical px; vanilla sits at the left edge).
const WINDOW_LEFT_PX: f32 = 16.0;
const WINDOW_TOP_PX: f32 = 72.0;

const SPEECH_COLOR: Color = Color::srgb(0.92, 0.92, 0.88);
const OPTION_COLOR: Color = Color::srgb_u8(150, 220, 120);
const OPTION_HOVER_COLOR: Color = Color::srgb_u8(255, 240, 160);
const OPTION_DISABLED_COLOR: Color = Color::srgb(0.5, 0.5, 0.5);

#[derive(Component)]
pub struct NpcDialogRoot;

/// The scrolling talk box (`GDR_NT_TALKBOX`) and its scrollbar parts.
#[derive(Component)]
pub struct NpcSpeechScroll;

#[derive(Component)]
pub struct NpcScrollTrack;

#[derive(Component)]
pub struct NpcScrollThumb;

/// Deferred despawn marker (PostUpdate), so the click that closed the dialog
/// doesn't fall through to click-to-move.
#[derive(Component)]
pub struct DialogClosing;

/// What clicking a dialog line does.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum DialogAction {
    Option(TalkOption),
    BackToOptions,
    EndConversation,
}

/// An interactive dialog text line (hover tint).
#[derive(Component)]
pub struct DialogLine {
    enabled: bool,
}

/// Rebuild the dialog window whenever the session state changes.
#[allow(clippy::too_many_arguments)]
pub fn sync_dialog_window(
    state: Res<NpcDialogState>,
    existing: Query<(Entity, &Node), With<NpcDialogRoot>>,
    npcs: Query<(
        Option<&DisplayName>,
        Option<&CharacterRef>,
        Option<&NpcTalkOptions>,
    )>,
    char_data: Res<ClientCharacterData>,
    npc_chat: Res<ClientNpcChat>,
    speech: Res<ClientSpeechText>,
    ui_strings: Res<ClientUiStrings>,
    shops: Res<ClientShops>,
    teleport: Res<ClientTeleport>,
    fonts: Res<FontAssets>,
    asset_server: Res<AssetServer>,
    cam_query: Query<Entity, With<Camera2d>>,
    primary_window: Query<&Window, With<PrimaryWindow>>,
    mut commands: Commands,
) {
    if !state.is_changed() {
        return;
    }
    // page switches rebuild the window — keep a dragged position; the first
    // open sits at vanilla's left-side spot (right-anchor math)
    let mut anchor: Option<(f32, f32)> = None;
    for (entity, node) in existing.iter() {
        if let (Val::Px(right), Val::Px(top)) = (node.right, node.top) {
            anchor = Some((right, top));
        }
        commands.entity(entity).insert(DialogClosing);
    }
    let NpcDialogState::Open { npc, page } = *state else {
        return;
    };
    let Ok(camera) = cam_query.single() else {
        warn!("npc dialog: no 2d camera to attach to");
        return;
    };
    let s = hud_scale();
    let anchor = anchor.unwrap_or_else(|| {
        let outer_w = OUTER_SIZE.0;
        let screen_w = primary_window.single().map(|w| w.width()).unwrap_or(1280.0);
        (
            (screen_w - outer_w * s - WINDOW_LEFT_PX).max(0.0),
            WINDOW_TOP_PX,
        )
    });

    let (display_name, char_ref, talk_options) = npcs.get(npc).unwrap_or((None, None, None));
    let name = display_name.map(|n| n.0.clone()).unwrap_or_default();
    let row = char_ref.and_then(|r| char_data.get(&(r.0 as i32)));
    // gate buildings have no characterdata row — their codename (the
    // npcchat/speech lookup key, e.g. STORE_CH_GATE) comes from
    // teleportbuilding.txt instead
    let codename: Option<&str> = row
        .map(|row| row.code_name().as_str())
        .or_else(|| char_ref.and_then(|r| teleport.owner_codename(r.0 as i32)));
    let chat_entry = codename.and_then(|code| npc_chat.get(code));
    // NPCs without an npcchat row usually still have speech strings under
    // the conventional SN_<codename>_BS/_PS keys
    let synthesized = |suffix: &str| codename.map(|code| format!("SN_{code}{suffix}"));
    let greeting_text = chat_entry
        .and_then(|entry| speech.get(&entry.greeting_key))
        .or_else(|| {
            synthesized("_BS")
                .as_deref()
                .and_then(|key| speech.get(key))
        })
        .unwrap_or("");
    // msg2 (the `_PS` half) is declared by all 395 `npcchat.txt` rows but has
    // **no** row in v1.188 `textquest_speech&name.txt` (453 `_BS` rows, 0
    // `_PS`), so this lookup is empty for every NPC in this data drop. The
    // synthesis is kept — the convention is real and a later drop may populate
    // it — and the Talk page falls back to msg1 instead of rendering blank.
    let raw_talk_text = chat_entry
        .and_then(|entry| speech.get(&entry.talk_key))
        .or_else(|| {
            synthesized("_PS")
                .as_deref()
                .and_then(|key| speech.get(key))
        })
        .unwrap_or("");
    let talk_text = talk_page_text(raw_talk_text, greeting_text);
    let speech_text = match page {
        DialogPage::Options => greeting_text,
        DialogPage::Talk => talk_text,
    }
    .replace("\\n", "\n");

    // What this NPC actually offers, derived from DATA — the spawn record's
    // option bits proved unreliable (guards advertise trade-ish bits) and are
    // only logged for future reverse-engineering.
    let has_shop = codename
        .and_then(|code| shops.shop_for_npc(code))
        .is_some_and(|layout| !layout.pages.is_empty());
    let teleporter = char_ref.and_then(|r| teleport.destinations(r.0 as i32));
    let is_ferry = teleporter.is_some_and(|(id, _)| {
        teleport
            .info(id)
            .is_some_and(|info| info.codename.contains("FERRY"))
    });
    if let Some(options) = talk_options {
        debug!(
            "npc dialog: spawn option bits {:02x?} vs derived shop={has_shop} teleport={} ",
            options.0,
            teleporter.is_some()
        );
    }

    let text_font = |size: f32| TextFont {
        font: fonts.two.clone().into(),
        font_size: FontSize::Px(size * s),
        ..default()
    };

    let window = game_window::spawn_game_window_with(
        &mut commands,
        &asset_server,
        &fonts,
        camera,
        if name.is_empty() { "NPC" } else { &name },
        game_window::WindowGeometry {
            outer: OUTER_SIZE,
            content_at: (INNER_FRAME_RECT.0, INNER_FRAME_RECT.1),
        },
        Some(game_window::InnerFrame {
            dir: INNER_FRAME_DIR,
            rect: INNER_FRAME_RECT,
            corner: INNER_FRAME_CORNER,
        }),
        anchor,
        s,
        game_window::GameWindowStyle::default(),
    );
    commands
        .entity(window.root)
        .insert((NpcDialogRoot, GlobalZIndex(45)));
    commands
        .entity(window.expect_close_button())
        .observe(on_close_button);

    commands.entity(window.content).with_children(|window| {
        // content: vanilla's single CIFTextBox (GDR_NT_TALKBOX 23,23,302,343)
        // holding the wrapped speech and, directly beneath it, the option
        // lines — the resinfo grammar has no list control here, the options
        // live in the same text box (shared class with the chat viewers).
        // The name lives in the shell's title band. Scrolls, with the matched
        // GDR_NPCTALK_SCROLL gutter to its right.
        let (box_x, box_y, box_w, box_h) = TALKBOX;
        let content_node = Node {
            position_type: PositionType::Absolute,
            left: Val::Px(box_x * s),
            top: Val::Px(box_y * s),
            width: Val::Px(box_w * s),
            height: Val::Px(box_h * s),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(6.0 * s),
            overflow: Overflow::scroll_y(),
            ..default()
        };
        window
            .spawn((NpcSpeechScroll, Hovered::default(), content_node))
            .observe(
                |mut scroll: On<Pointer<Scroll>>,
                 mut lists: Query<(&mut ScrollPosition, &ComputedNode), With<NpcSpeechScroll>>| {
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
            .with_children(|content| {
                content.spawn((
                    Text::new(speech_text),
                    text_font(9.0),
                    TextColor(SPEECH_COLOR),
                    Node {
                        width: Val::Percent(100.0),
                        margin: UiRect::bottom(Val::Px(12.0 * s)),
                        ..default()
                    },
                    Pickable::IGNORE,
                ));

                let ui = |key: &str, fallback: &str| ui_strings.get_or(key, fallback).to_string();
                let mut lines: Vec<(String, DialogAction, bool)> = Vec::new();
                match page {
                    DialogPage::Options => {
                        // data-derived options: only offer what the tables
                        // actually back (label always matches the action)
                        if !talk_text.is_empty() {
                            lines.push((
                                ui(
                                    "UIIT_STT_NPC_CHATTING_WND_TALKSTART",
                                    "Talk to this person.",
                                ),
                                DialogAction::Option(TalkOption::Talk),
                                true,
                            ));
                        }
                        if has_shop {
                            lines.push((
                                ui("UIIT_STT_NPC_CHATTING_WND_SHOP", "Trade in the shop."),
                                DialogAction::Option(TalkOption::Store),
                                true,
                            ));
                        }
                        if teleporter.is_some() {
                            let label = if is_ferry {
                                ui("UIIT_STT_NPC_CHATTING_WND_SHIP_MOVE", "Cross by ship.")
                            } else {
                                ui("UIIT_CTL_TELEPORT_TARGET", "Select teleport area")
                            };
                            lines.push((label, DialogAction::Option(TalkOption::Teleport), true));
                        }
                        // data-derived like shop/teleport: warehouse keepers
                        // follow the NPC_*_WAREHOUSE* codename convention
                        // (same style of gate as the FERRY test above)
                        let has_storage = codename.is_some_and(|c| c.contains("WAREHOUSE"));
                        if has_storage {
                            lines.push((
                                ui(
                                    "UIIT_STT_NPC_CHATTING_WND_STOREHOUSE",
                                    "Deposit into storage.",
                                ),
                                DialogAction::Option(TalkOption::Storage),
                                true,
                            ));
                            // The guild warehouse is the same NPC, a second
                            // option. Offered unconditionally like the label
                            // itself reads — "(level 2 or above)" — because the
                            // guild/level/permission refusals each have their
                            // own string and are stated at the click
                            // (`hud::guild_storage::model::gate`), not by
                            // hiding the line.
                            lines.push((
                                ui(
                                    "UIIT_CTL_GUILD_WAREHOUSE",
                                    "Use guild storage. (level 2 or above)",
                                ),
                                DialogAction::Option(TalkOption::GuildStorage),
                                true,
                            ));
                        }
                    }
                    DialogPage::Talk => {
                        lines.push((
                            ui("UIIT_STT_NPC_CHATTING_WND_TALKEND2", "End."),
                            DialogAction::BackToOptions,
                            true,
                        ));
                    }
                }
                lines.push((
                    ui("UIIT_STT_NPC_CHATTING_WND_TALKEND", "End conversation."),
                    DialogAction::EndConversation,
                    true,
                ));

                for (label, action, enabled) in lines {
                    let color = if enabled {
                        OPTION_COLOR
                    } else {
                        OPTION_DISABLED_COLOR
                    };
                    let mut line = content.spawn((
                        DialogLine { enabled },
                        action,
                        Button,
                        Hovered::default(),
                        Text::new(label),
                        text_font(9.5),
                        TextColor(color),
                        Node::default(),
                    ));
                    if enabled {
                        line.observe(on_dialog_line);
                    }
                }
            });

        // GDR_NPCTALK_SCROLL 336,27,16,320 — up/down arrows over the
        // com_scroll track with a thumb mirroring the box's scroll fraction
        // (same recipe as the skill window's row list).
        let (bar_x, bar_y, bar_w, bar_h) = SCROLL_RECT;
        let scroll_by = |delta: f32| {
            move |_: On<Activate>,
                  mut lists: Query<(&mut ScrollPosition, &ComputedNode), With<NpcSpeechScroll>>| {
                let Ok((mut pos, computed)) = lists.single_mut() else {
                    return;
                };
                let max = ((computed.content_size().y - computed.size().y)
                    * computed.inverse_scale_factor())
                .max(0.0);
                pos.y = (pos.y + delta).clamp(0.0, max);
            }
        };
        window
            .spawn(Node {
                position_type: PositionType::Absolute,
                left: Val::Px(bar_x * s),
                top: Val::Px(bar_y * s),
                width: Val::Px(bar_w * s),
                height: Val::Px(bar_h * s),
                flex_direction: FlexDirection::Column,
                ..default()
            })
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
                                width: Val::Px(bar_w * s),
                                height: Val::Px(bar_w * s),
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
                        NpcScrollTrack,
                        Node {
                            width: Val::Px(bar_w * s),
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
                            NpcScrollThumb,
                            Node {
                                position_type: PositionType::Absolute,
                                left: Val::Px(0.0),
                                top: Val::Px(0.0),
                                width: Val::Px(bar_w * s),
                                height: Val::Px(bar_w * s),
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

/// Mirror the talk box's scroll fraction onto the scrollbar thumb.
pub fn update_npc_dialog_scroll_thumb(
    lists: Query<&ComputedNode, With<NpcSpeechScroll>>,
    tracks: Query<&ComputedNode, With<NpcScrollTrack>>,
    mut thumbs: Query<&mut Node, With<NpcScrollThumb>>,
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
        ((track.size().y - SCROLL_RECT.2 * hud_scale()) * track.inverse_scale_factor()).max(0.0);
    thumb.top = Val::Px(travel * fraction);
}

/// The Talk page's text: msg2 when the data has it, msg1 otherwise.
///
/// `npcchat.txt` declares both `SN_<codename>_BS` (msg1) and `_PS` (msg2) for
/// all 395 rows, but v1.188's `textquest_speech&name.txt` carries 453 `_BS`
/// rows and **zero** `_PS` rows, so msg2 resolves empty for every NPC and the
/// Talk page used to render nothing at all.
fn talk_page_text<'a>(talk: &'a str, greeting: &'a str) -> &'a str {
    if talk.is_empty() {
        greeting
    } else {
        talk
    }
}

/// The title-band (X) button = "End conversation.".
fn on_close_button(
    _: On<Activate>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    ids: Query<&NetworkId>,
    mut state: ResMut<NpcDialogState>,
) {
    if let Some(npc) = state.npc() {
        super::model::send_close(&conn, ids.get(npc).ok());
    }
    *state = NpcDialogState::Closed;
}

/// Hover tint for the interactive lines.
pub fn tint_dialog_lines(
    mut lines: Query<(&DialogLine, &Hovered, &mut TextColor), Changed<Hovered>>,
) {
    for (line, hovered, mut color) in lines.iter_mut() {
        if !line.enabled {
            continue;
        }
        color.0 = if hovered.get() {
            OPTION_HOVER_COLOR
        } else {
            OPTION_COLOR
        };
    }
}

/// Dispatch a clicked dialog line.
#[allow(clippy::too_many_arguments)]
fn on_dialog_line(
    activate: On<Activate>,
    actions: Query<&DialogAction>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    ids: Query<&NetworkId>,
    mut state: ResMut<NpcDialogState>,
    mut store: MessageWriter<OpenStore>,
    mut teleport: MessageWriter<OpenTeleport>,
    mut storage: MessageWriter<OpenStorage>,
    mut guild_storage: MessageWriter<OpenGuildStorage>,
) {
    let Ok(action) = actions.get(activate.entity) else {
        return;
    };
    let Some(npc) = state.npc() else {
        return;
    };
    match action {
        DialogAction::Option(TalkOption::Talk) => {
            *state = NpcDialogState::Open {
                npc,
                page: DialogPage::Talk,
            };
        }
        DialogAction::Option(TalkOption::Store) => {
            store.write(OpenStore { npc });
        }
        DialogAction::Option(TalkOption::Teleport) => {
            teleport.write(OpenTeleport { npc });
        }
        DialogAction::Option(TalkOption::Storage) => {
            storage.write(OpenStorage { npc });
        }
        DialogAction::Option(TalkOption::GuildStorage) => {
            guild_storage.write(OpenGuildStorage { npc });
        }
        DialogAction::BackToOptions => {
            *state = NpcDialogState::Open {
                npc,
                page: DialogPage::Options,
            };
        }
        DialogAction::EndConversation => {
            super::model::send_close(&conn, ids.get(npc).ok());
            *state = NpcDialogState::Closed;
        }
    }
}

/// PostUpdate: actually despawn windows marked [`DialogClosing`].
pub fn despawn_closing_dialogs(
    closing: Query<Entity, With<DialogClosing>>,
    mut commands: Commands,
) {
    for entity in closing.iter() {
        commands.entity(entity).despawn();
    }
}

/// Vanilla: the shop/storage window swaps IN PLACE OF the dialog. The dialog
/// window is only hidden, never state-closed — those sessions live inside the
/// talk session and `close_*_with_dialog` would end them — so the dialog
/// reappears when the shop/storage closes back into the conversation.
pub fn hide_dialog_while_store_open(
    store: Res<crate::plugins::hud::store::model::StoreState>,
    storage: Res<crate::plugins::hud::storage::model::StorageState>,
    dialog: Res<NpcDialogState>,
    mut dialogs: Query<&mut Visibility, With<NpcDialogRoot>>,
) {
    // Hide the dialog only while the CURRENTLY-OPEN NPC's own shop/storage window
    // is up (it swaps in place of the dialog). A session left over from a
    // different NPC must never hide a freshly opened dialog (#217).
    let open_npc = dialog.npc();
    let hide = store
        .session
        .as_ref()
        .is_some_and(|s| open_npc == Some(s.npc))
        || storage
            .session
            .as_ref()
            .is_some_and(|s| open_npc == Some(s.npc));
    let wanted = if hide {
        Visibility::Hidden
    } else {
        Visibility::Inherited
    };
    for mut visibility in dialogs.iter_mut() {
        visibility.set_if_neq(wanted);
    }
}

/// OnExit cleanup.
pub fn cleanup_npc_dialog(
    mut commands: Commands,
    windows: Query<Entity, With<NpcDialogRoot>>,
    mut state: ResMut<NpcDialogState>,
) {
    for entity in windows.iter() {
        commands.entity(entity).despawn();
    }
    *state = NpcDialogState::Closed;
}

#[cfg(test)]
mod test {
    use super::*;

    /// Both frames now come from the data: the shell is `GDR_NPCWINDOW`'s
    /// `386x451` and the nested conversation frame is `GDR_NW_NPCTALK`'s
    /// `364x391` at `11,48`. The old +2 / -8 residual came from *deriving*
    /// the outer size (388x443) instead of reading it, so this test pins the
    /// authored numbers and the margins they imply — there is no residual.
    #[test]
    fn both_authored_frames_are_modelled_with_no_residual() {
        assert_eq!(OUTER_SIZE, (386.0, 451.0));
        assert_eq!(INNER_FRAME_RECT, (11.0, 48.0, 364.0, 391.0));
        assert_eq!((CONTENT_W, CONTENT_H), (364.0, 391.0));
        // margins the two rects imply: (left, top, right, bottom)
        let (x, y, w, h) = INNER_FRAME_RECT;
        assert_eq!(
            (x, y, OUTER_SIZE.0 - x - w, OUTER_SIZE.1 - y - h),
            (11.0, 48.0, 11.0, 12.0)
        );
        // the derivation this replaces, kept as the reason it was replaced
        assert_eq!(
            game_window::outer_size((CONTENT_W, CONTENT_H)),
            (388.0, 443.0)
        );
        assert_ne!(game_window::outer_size((CONTENT_W, CONTENT_H)), OUTER_SIZE);
    }

    /// The talkbox and the scrollbar clear the nested frame's 20 px border,
    /// which is why the frame can be drawn under them without covering them.
    #[test]
    fn the_talk_box_clears_the_nested_frames_border() {
        assert_eq!(INNER_FRAME_CORNER, 20.0);
        assert!(TALKBOX.0 >= INNER_FRAME_CORNER);
        assert!(TALKBOX.1 >= INNER_FRAME_CORNER);
    }

    /// `if_npctalk.txt` — `GDR_NT_TALKBOX` `23,23,302,343` and
    /// `GDR_NPCTALK_SCROLL` `336,27,16,320` are a matched pair
    /// (23 + 302 + 11 = 336), both inside the 364x391 frame.
    #[test]
    fn talk_box_and_scrollbar_match_if_npctalk_rects() {
        assert_eq!(TALKBOX, (23.0, 23.0, 302.0, 343.0));
        assert_eq!(SCROLL_RECT, (336.0, 27.0, 16.0, 320.0));
        assert_eq!(TALKBOX.0 + TALKBOX.2 + 11.0, SCROLL_RECT.0);
        assert!(SCROLL_RECT.0 + SCROLL_RECT.2 <= CONTENT_W);
        assert!(TALKBOX.1 + TALKBOX.3 <= CONTENT_H);
        assert!(SCROLL_RECT.1 + SCROLL_RECT.3 <= CONTENT_H);
    }

    /// v1.188 declares `SN_*_PS` (msg2) for all 395 `npcchat.txt` rows and
    /// gives it text in none of them, so the Talk page rendered blank for
    /// every NPC. Keep the lookup, fall back to msg1.
    #[test]
    fn talk_page_falls_back_to_msg1_when_ps_is_empty() {
        assert_eq!(
            talk_page_text("", "Welcome, traveller."),
            "Welcome, traveller."
        );
        assert_eq!(
            talk_page_text("Let me tell you.", "Welcome."),
            "Let me tell you."
        );
        assert_eq!(talk_page_text("", ""), "");
    }
}
