//! Chat input: the vanilla Enter toggle and channel-prefix parsing.
//!
//! Idea: Enter opens (and focuses) the input row, Enter again parses + sends
//! and closes it; Esc aborts. A leading channel char reroutes the message —
//! `#` party, `@` guild, `%` union, `&` academy, `$name msg` whisper —
//! otherwise the active tab decides. Sent lines don't echo immediately: they
//! park in [`ChatState::pending`] under the request's rolling `chat_index`
//! and enter the history when the 0xB025 ack confirms (see `net.rs`).

use bevy::input_focus::{FocusCause, InputFocus};
use bevy::prelude::*;
use bevy::text::{EditableText, TextEdit};
use bevy::ui_widgets::{Activate, Button};

use packets::agent::chat::{chat_type, ChatRequest};
use packets::Packet;

use crate::assets::FontAssets;
use crate::net::connection::SilkroadConnection;
use crate::plugins::gm::SlashCommand;
use crate::plugins::hud::player_mini_info::PlayerVitals;
use crate::plugins::hud::stall::owner::OpenStallCommand;
use crate::plugins::net::agent::AgentConnection;
use crate::plugins::net::entities::{DisplayName, NetworkId, RemoteEntity};
use crate::plugins::net::party::{PartyAction, PartyRoster};
use crate::plugins::textdata::ClientUiStrings;

use super::model::{ChatHistory, ChatLine, ChatLineKind, ChatState, ChatTab};
use super::ui::{ChatInputBox, ChatWhisperEntries};
use crate::plugins::hud::scale::hud_scale;

/// A parsed outgoing message.
#[derive(Debug, PartialEq)]
pub struct Outgoing {
    pub chat_type: u8,
    pub receiver: Option<String>,
    pub message: String,
}

/// Channel-prefix parsing; `None` when nothing sendable remains (empty
/// message, or a whisper without recipient/message). `mode` is the chat-mode
/// dropdown's selection (`GDR_CHAT_MODE_*`), which the tab strip keeps in
/// step, so an unprefixed line goes where the dropdown says.
pub fn parse_outgoing(text: &str, mode: ChatTab) -> Option<Outgoing> {
    let text = text.trim();
    let (chat_type, receiver, message) = match text.chars().next()? {
        '#' => (chat_type::PARTY, None, &text[1..]),
        '@' => (chat_type::GUILD, None, &text[1..]),
        '%' => (chat_type::UNION, None, &text[1..]),
        '&' => (chat_type::ACADEMY, None, &text[1..]),
        '$' => {
            let rest = &text[1..];
            let (name, message) = rest.split_once(' ')?;
            if name.is_empty() {
                return None;
            }
            (chat_type::PM, Some(name.to_string()), message)
        }
        _ => (mode.default_chat_type(), None, text),
    };
    let message = message.trim();
    if message.is_empty() {
        return None;
    }
    Some(Outgoing {
        chat_type,
        receiver,
        message: message.to_string(),
    })
}

/// The original's whisper chat-commands, verbatim from the user's PK2
/// (`Media/server_dep/silkroad/textdata/textuisystem.txt`):
/// `UIIT_STT_CHAT_COMMAND_SECRET_TALK` L668 `/Whisper`,
/// `_WHISPER1` L676 `/whisper`, `_WHISPER2` L677 `/W`,
/// `_WHISPER3`-`_WHISPER5` L678-680 `/w`.
///
/// The table lists `/W` and `/w` as separate entries, so the original matches
/// case-sensitively and so do we — the four distinct spellings below are the
/// whole vocabulary for this command.
const WHISPER_COMMANDS: [&str; 4] = ["/Whisper", "/whisper", "/W", "/w"];

/// `UIIT_STT_CHAT_COMMAND_REPLY1`-`_REPLY4`, textuisystem L672-675: answer the
/// newest whisper without retyping the name.
const REPLY_COMMANDS: [&str; 4] = ["/Reply", "/r", "/re", "/R"];

/// The rest of the original's vocabulary, verbatim from the same table. Every
/// one of these needs a request path we do not have (party/guild/friend/trade
/// invites, stall, sit, PvP proposal, block lists), so they are **named here
/// rather than implemented**: a command that parses and then silently does
/// nothing is the dead wire this file is trying to remove. Reaching one of
/// them answers with `UIIT_CHATERR_INVALID_COMMAND` and logs the command, so
/// the player gets feedback and the gap stays visible.
///
/// L663-671, L681-692 and L4584-4589 in reading order:
const UNSUPPORTED_COMMANDS: [&str; 18] = [
    "/Acquire",                // L663 _FINDUSER
    "/Exchange",               // L664 _TRADE
    "/Fight",                  // L665 _PVP_PROPOSAL
    "/SitDown",                // L670 _SITDOWN
    "/invite",                 // L681 _FRIEND_INVITE1
    "/friend",                 // L682 _FRIEND_INVITE2
    "/join",                   // L684 _GUILD_INVITE1
    "/guild",                  // L685 _GUILD_INVITE2
    "/trade",                  // L686 _TRADE2
    "/ban",                    // L687 _BLOCK
    "/unban",                  // L688 _ADMISSION
    "/mute",                   // L689 _WHISPER_BLOCK
    "/unmute",                 // L690 _WHISPER_ADMISSION
    "/f",                      // L691 _FRIEND_INVITE
    "/g",                      // L692 _GUILD_INVITE
    "/BlockFriendInvitation",  // L4584 _FRIEND_BLOCK
    "/AcceptFriendInvitation", // L4585 _FRIEND_ADMISSION
    "/BlockGuildInvitation",   // L4586 _GUILD_BLOCK
];

/// The three remaining L4587-4589 entries, split out only because the array
/// above is already at the table's reading order boundary.
const UNSUPPORTED_COMMANDS_TAIL: [&str; 3] = [
    "/AcceptGuildInvitation", // L4587 _GUILD_ADMISSION
    "/BlockTradeInvitation",  // L4588 _TRADE_BLOCK
    "/AcceptTradeInvitation", // L4589 _TRADE_ADMISSION
];

/// The four party verbs from the same table, split out of
/// [`UNSUPPORTED_COMMANDS`] because they now have a request path
/// (`net::party::PartyAction`). Two name a player (L666 `_PARTY_INVITE`,
/// L683 `_PARTY_INVITE2`, L667 `_PARTY_EXPEL`) and one takes no argument
/// (L671 `_PARTY_EXIT`).
///
/// Invite resolves the name against the *visible* entities rather than sending
/// it: the original's own builder writes a `u32 uniqueID`, never a name
/// (`docs/re/systems/gameinvite-0x3080-assembly.md` §3), so it must resolve
/// locally too. Expel resolves against the party roster, which carries member
/// names. A name that resolves to nothing is reported, not sent as id 0.
const PARTY_INVITE_COMMANDS: [&str; 2] = ["/InviteToParty", "/party"];
const PARTY_EXPEL_COMMAND: &str = "/BanishFromParty";
const PARTY_LEAVE_COMMAND: &str = "/LeaveTheParty";

/// `UIIT_STT_CHAT_COMMAND_PARTY_*` needs a target and did not get one, or the
/// name matched nobody. Our wording — the original's own "no such user" string
/// is not bound to these commands by any source we have.
pub const NO_SUCH_PLAYER_FALLBACK: &str = "No such player nearby.";
pub const NO_SUCH_MEMBER_FALLBACK: &str = "No party member by that name.";

/// `UIIT_STT_CHAT_COMMAND_STREETSTORE`, textuisystem L669 — the one command
/// this file has moved OUT of [`UNSUPPORTED_COMMANDS`], because `0x70B1` now
/// gives it the request path it was missing (#781).
const STALL_COMMAND: &str = "/Stall";

/// `UIIT_CHATERR_INVALID_COMMAND`, textuisystem L1730.
pub const INVALID_COMMAND_KEY: &str = "UIIT_CHATERR_INVALID_COMMAND";
pub const INVALID_COMMAND_FALLBACK: &str = "Invalid chat command.";

/// How a `/…` line is routed.
#[derive(Debug, PartialEq)]
pub enum SlashRoute {
    /// A chat command that carries a message to the server (a whisper).
    Chat(Outgoing),
    /// A chat command with nothing sendable left (`/w Foo` without a message,
    /// or `/r` before any whisper arrived). Dropped, exactly like the
    /// `$`-prefixed form.
    Incomplete,
    /// In the original's vocabulary, but openroad has no request path for it.
    /// Answered with `UIIT_CHATERR_INVALID_COMMAND` and logged — never
    /// swallowed, because a parsed command with no effect is a dead wire.
    Unsupported,
    /// Not in the chat vocabulary — a client-side GM command.
    Gm,
    /// One of the four party verbs. The name is still unresolved here:
    /// `route_slash` is a pure parser with no view of the world or the roster,
    /// so resolution happens at the call site.
    Party(PartySlash),
    /// `/Stall [title]` (`UIIT_STT_CHAT_COMMAND_STREETSTORE`, L669): open a
    /// player stall. It left [`SlashRoute::Unsupported`] the moment its
    /// request path existed — `0x70B1 StallCreateRequest{title}` (#781).
    /// `None` means the command carried no title and the vanilla default
    /// applies.
    OpenStall(Option<String>),
}

/// A parsed party command, before its name (if any) is resolved to an id.
#[derive(Debug, PartialEq, Eq)]
pub enum PartySlash {
    /// Ask a nearby player into the party.
    Invite(String),
    /// Expel a party member.
    Expel(String),
    /// Leave the party — no argument.
    Leave,
}

/// Idea: the original ships a localizable slash-command vocabulary
/// (`UIIT_STT_CHAT_COMMAND_*`, textuisystem L663-692 and L4584-4589) that is
/// resolved *before* anything else looks at the leading `/`. We implement the
/// two commands that need no new request path — whisper and reply — name the
/// rest as [`SlashRoute::Unsupported`], and leave everything outside the
/// vocabulary to the GM dispatcher.
///
/// `last_whisper_from` is [`ChatState::last_whisper_from`], recorded on the
/// incoming-whisper path in `net.rs`; it is the only source for `/Reply`'s
/// target.
pub fn route_slash(text: &str, last_whisper_from: Option<&str>) -> SlashRoute {
    let text = text.trim();
    let (command, rest) = match text.split_once(' ') {
        Some(split) => split,
        None => (text, ""),
    };
    let rest = rest.trim_start();

    if WHISPER_COMMANDS.contains(&command) {
        let Some((name, message)) = rest.split_once(' ') else {
            return SlashRoute::Incomplete;
        };
        let message = message.trim();
        if name.is_empty() || message.is_empty() {
            return SlashRoute::Incomplete;
        }
        return SlashRoute::Chat(Outgoing {
            chat_type: chat_type::PM,
            receiver: Some(name.to_string()),
            message: message.to_string(),
        });
    }

    if REPLY_COMMANDS.contains(&command) {
        let message = rest.trim();
        // No partner yet, or nothing to say: nothing sendable, same as above.
        let (Some(name), false) = (last_whisper_from, message.is_empty()) else {
            return SlashRoute::Incomplete;
        };
        return SlashRoute::Chat(Outgoing {
            chat_type: chat_type::PM,
            receiver: Some(name.to_string()),
            message: message.to_string(),
        });
    }

    if command == PARTY_LEAVE_COMMAND {
        return SlashRoute::Party(PartySlash::Leave);
    }
    // The two name-taking verbs: a missing name is Incomplete, exactly like a
    // whisper without a message — parsed, nothing to send.
    if PARTY_INVITE_COMMANDS.contains(&command) || command == PARTY_EXPEL_COMMAND {
        let name = rest.trim();
        if name.is_empty() {
            return SlashRoute::Incomplete;
        }
        return SlashRoute::Party(if command == PARTY_EXPEL_COMMAND {
            PartySlash::Expel(name.to_string())
        } else {
            PartySlash::Invite(name.to_string())
        });
    }

    if command == STALL_COMMAND {
        let title = rest.trim();
        return SlashRoute::OpenStall((!title.is_empty()).then(|| title.to_string()));
    }

    if UNSUPPORTED_COMMANDS.contains(&command) || UNSUPPORTED_COMMANDS_TAIL.contains(&command) {
        return SlashRoute::Unsupported;
    }

    SlashRoute::Gm
}

/// The optimistic own-line parked until the server ack.
fn own_line(outgoing: &Outgoing, own_name: &str) -> ChatLine {
    let (kind, sender) = match outgoing.chat_type {
        chat_type::PM => (
            ChatLineKind::WhisperTo,
            outgoing.receiver.clone().unwrap_or_default(),
        ),
        chat_type::PARTY => (ChatLineKind::Party, own_name.to_string()),
        chat_type::GUILD => (ChatLineKind::Guild, own_name.to_string()),
        chat_type::UNION => (ChatLineKind::Union, own_name.to_string()),
        chat_type::ACADEMY => (ChatLineKind::Academy, own_name.to_string()),
        _ => (ChatLineKind::All, own_name.to_string()),
    };
    ChatLine {
        kind,
        sender: Some(sender),
        text: outgoing.message.clone(),
    }
}

/// Enter/Esc handling for the input row (vanilla toggle behavior).
#[allow(clippy::too_many_arguments)]
pub fn handle_chat_enter(
    keys: Res<ButtonInput<KeyCode>>,
    enter_consumed: Res<crate::plugins::hud::focus::EnterConsumed>,
    time: Res<Time>,
    mut focus: ResMut<InputFocus>,
    mut state: ResMut<ChatState>,
    mut input: Query<(Entity, &mut EditableText), With<ChatInputBox>>,
    vitals: Res<PlayerVitals>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    ui_strings: Res<ClientUiStrings>,
    mut history: ResMut<ChatHistory>,
    mut slash_commands: MessageWriter<SlashCommand>,
    nearby: Query<(&DisplayName, &NetworkId, &RemoteEntity)>,
    roster: Res<PartyRoster>,
    mut party: MessageWriter<PartyAction>,
    mut open_stall: MessageWriter<OpenStallCommand>,
) {
    let Ok((entity, mut editable)) = input.single_mut() else {
        return;
    };

    // The input row is always visible; "open" = the editable text is focused.
    // The user can also focus it by clicking, so derive the state from the
    // actual focus (keeps the Esc-menu guard and the idle fade in sync).
    let focused = focus.get() == Some(entity);
    if state.input_open != focused {
        state.input_open = focused;
    }

    // A dialog that confirmed on this press already claimed it
    // (`hud::focus::confirm_focused_dialog_on_enter`, ordered before this).
    // Without the check, one Enter would both dismiss the dialog and drop the
    // player into the chat input underneath it.
    let enter = crate::plugins::hud::focus::enter_pressed(&keys) && !enter_consumed.0;
    let escape = keys.just_pressed(KeyCode::Escape);
    if !enter && !escape {
        return;
    }

    if !focused {
        if enter && !state.collapsed {
            state.input_open = true;
            focus.set(entity, FocusCause::Navigated);
        }
        return;
    }

    let close = |state: &mut ChatState, focus: &mut InputFocus| {
        state.input_open = false;
        if focus.get() == Some(entity) {
            focus.clear();
        }
    };

    if escape {
        close(&mut state, &mut focus);
        return;
    }
    if editable.is_composing() {
        return;
    }

    let text = editable.value().to_string();
    // A leading '/' is either one of the original's chat commands (`/w Foo hi`
    // is a whisper and must reach the server) or a client-side GM command
    // (e.g. /invisible). Resolve the chat vocabulary first, then fall back to
    // the GM handler.
    let outgoing = if let Some(gm_command) = text.trim().strip_prefix('/') {
        match route_slash(&text, state.last_whisper_from.as_deref()) {
            SlashRoute::Chat(outgoing) => Some(outgoing),
            SlashRoute::Incomplete => None,
            SlashRoute::Unsupported => {
                // In the original's vocabulary, but we have no request path for
                // it. Say so instead of dropping it silently — and keep the
                // command in the log so the gap stays findable.
                warn!("chat: {} is not implemented yet", text.trim());
                history.push(ChatLine::system(
                    &ui_strings.get_plain_or(INVALID_COMMAND_KEY, INVALID_COMMAND_FALLBACK),
                ));
                editable.clear();
                close(&mut state, &mut focus);
                return;
            }
            SlashRoute::Party(command) => {
                // Resolution failures are reported, never sent as id 0.
                let resolved = match &command {
                    PartySlash::Leave => Some(PartyAction::Leave),
                    PartySlash::Invite(name) => nearby
                        .iter()
                        .find(|(display, _, kind)| {
                            **kind == RemoteEntity::Player && display.0 == *name
                        })
                        .map(|(_, id, _)| PartyAction::Invite(id.0)),
                    // Both halves are presence-masked, so a member the server
                    // has not named — or not given an id — cannot be kicked by
                    // name; that resolves to the "no such member" line rather
                    // than to a kick aimed at id 0.
                    PartySlash::Expel(name) => roster
                        .members
                        .iter()
                        .find(|member| member.name.as_deref() == Some(name.as_str()))
                        .and_then(|member| member.member_id)
                        .map(PartyAction::Kick),
                };
                match resolved {
                    Some(action) => {
                        party.write(action);
                    }
                    None => {
                        let fallback = match command {
                            PartySlash::Expel(_) => NO_SUCH_MEMBER_FALLBACK,
                            _ => NO_SUCH_PLAYER_FALLBACK,
                        };
                        warn!("chat: {} resolved to nobody", text.trim());
                        history.push(ChatLine::system(fallback));
                    }
                }
                editable.clear();
                close(&mut state, &mut focus);
                return;
            }
            SlashRoute::OpenStall(title) => {
                open_stall.write(OpenStallCommand { title });
                editable.clear();
                close(&mut state, &mut focus);
                return;
            }
            SlashRoute::Gm => {
                let gm_command = gm_command.trim();
                if !gm_command.is_empty() {
                    slash_commands.write(SlashCommand(gm_command.to_string()));
                }
                editable.clear();
                close(&mut state, &mut focus);
                return;
            }
        }
    } else {
        parse_outgoing(&text, state.mode)
    };
    let Some(outgoing) = outgoing else {
        editable.clear();
        close(&mut state, &mut focus);
        return;
    };

    // whispering yourself is a no-op (and go-sro would happily deliver it)
    if outgoing.chat_type == chat_type::PM
        && outgoing
            .receiver
            .as_deref()
            .is_some_and(|receiver| receiver.eq_ignore_ascii_case(&vitals.name))
    {
        warn!("chat: not sending, cannot whisper yourself");
        editable.clear();
        close(&mut state, &mut focus);
        return;
    }

    if let Some(until) = state.restricted_until {
        let now = time.elapsed_secs_f64();
        if now < until {
            warn!(
                "chat: not sending, restricted for {} more seconds",
                (until - now).ceil() as u64
            );
            editable.clear();
            close(&mut state, &mut focus);
            return;
        }
    }

    let Ok(conn) = conn.single() else {
        warn!("chat: not sending, no agent connection");
        editable.clear();
        close(&mut state, &mut focus);
        return;
    };
    let chat_index = state.next_chat_index;
    state.next_chat_index = state.next_chat_index.wrapping_add(1);
    let request = ChatRequest {
        chat_type: outgoing.chat_type,
        chat_index,
        receiver: outgoing.receiver.clone(),
        message: outgoing.message.clone(),
    };
    if let Err(e) = conn.get_sender().send(Packet::from(request).into()) {
        error!("network: failed to send ChatRequest: {}", e.0);
    } else {
        state
            .pending
            .insert(chat_index, own_line(&outgoing, &vitals.name));
    }
    editable.clear();
    close(&mut state, &mut focus);
}

/// Rebuild the whisper-partner rows (recent distinct names, newest first,
/// never yourself); clicking one prefills `$name ` and opens the input.
pub fn refresh_whisper_panel(
    mut commands: Commands,
    history: Res<ChatHistory>,
    state: Res<ChatState>,
    fonts: Res<FontAssets>,
    vitals: Res<PlayerVitals>,
    entries: Query<Entity, With<ChatWhisperEntries>>,
    mut was_open: Local<bool>,
) {
    // rebuild only when the panel (re)opens or the history changes while open
    let just_opened = state.whisper_panel_open && !*was_open;
    *was_open = state.whisper_panel_open;
    if !state.whisper_panel_open || !(history.is_changed() || just_opened) {
        return;
    }
    let Ok(container) = entries.single() else {
        return;
    };

    let mut partners: Vec<String> = Vec::new();
    for line in history.iter().collect::<Vec<_>>().into_iter().rev() {
        let is_whisper = matches!(
            line.kind,
            ChatLineKind::WhisperFrom | ChatLineKind::WhisperTo
        );
        if !is_whisper {
            continue;
        }
        if let Some(name) = &line.sender {
            if !name.eq_ignore_ascii_case(&vitals.name) && !partners.iter().any(|p| p == name) {
                partners.push(name.clone());
            }
        }
        if partners.len() >= 8 {
            break;
        }
    }

    let font = fonts.nine.clone();
    commands.entity(container).despawn_related::<Children>();
    commands.entity(container).with_children(|parent| {
        for name in partners {
            let display = name.clone();
            parent
                .spawn((
                    Text(display),
                    TextFont {
                        font: font.clone().into(),
                        font_size: FontSize::Px(11.0 * hud_scale()),
                        ..default()
                    },
                    TextColor(Color::WHITE),
                    TextLayout::new(Justify::Left, LineBreak::NoWrap),
                    Node {
                        width: percent(100.0),
                        height: px(14.0 * hud_scale()),
                        flex_shrink: 0.0,
                        ..default()
                    },
                    Button,
                ))
                .observe(
                    move |_: On<Activate>,
                          mut focus: ResMut<InputFocus>,
                          mut state: ResMut<ChatState>,
                          mut input: Query<(Entity, &mut EditableText), With<ChatInputBox>>| {
                        state.whisper_panel_open = false;
                        prefill_whisper_input(&name, &mut focus, &mut state, &mut input);
                    },
                );
        }
    });
}

/// Prefill the input with `$name ` (caret at the end) and focus it, starting
/// a whisper to `name`. Shared by the whisper-panel rows and clicks on sender
/// names in the message list.
pub fn prefill_whisper_input(
    name: &str,
    focus: &mut InputFocus,
    state: &mut ChatState,
    input: &mut Query<(Entity, &mut EditableText), With<ChatInputBox>>,
) {
    let Ok((entity, mut editable)) = input.single_mut() else {
        return;
    };
    // SelectAll + Insert instead of `clear()` + Insert: `clear()` leaves
    // parley's selection at a stale byte index, and a same-frame Insert then
    // panics inside apply_text_edits.
    editable.queue_edit(TextEdit::SelectAll);
    editable.queue_edit(TextEdit::Insert(format!("${name} ").into()));
    state.input_open = true;
    focus.set(entity, FocusCause::Navigated);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #293: an unprefixed line follows the chat-mode dropdown, not the tab
    /// being read — that is the whole point of `GDR_CHAT_MODE_*`.
    #[test]
    fn plain_text_uses_the_selected_chat_mode() {
        let out = parse_outgoing("hello", ChatTab::Guild).unwrap();
        assert_eq!(out.chat_type, chat_type::GUILD);
        assert_eq!(out.receiver, None);
        assert_eq!(out.message, "hello");
    }

    #[test]
    fn prefixes_override_the_chat_mode() {
        for (prefix, expected) in [
            ('#', chat_type::PARTY),
            ('@', chat_type::GUILD),
            ('%', chat_type::UNION),
            ('&', chat_type::ACADEMY),
        ] {
            let out = parse_outgoing(&format!("{prefix}hi"), ChatTab::All).unwrap();
            assert_eq!(out.chat_type, expected);
            assert_eq!(out.message, "hi");
        }
    }

    #[test]
    fn whisper_splits_recipient_and_message() {
        let out = parse_outgoing("$Foo hi there", ChatTab::All).unwrap();
        assert_eq!(out.chat_type, chat_type::PM);
        assert_eq!(out.receiver.as_deref(), Some("Foo"));
        assert_eq!(out.message, "hi there");
    }

    #[test]
    fn empty_or_incomplete_input_is_rejected() {
        assert_eq!(parse_outgoing("", ChatTab::All), None);
        assert_eq!(parse_outgoing("   ", ChatTab::All), None);
        assert_eq!(parse_outgoing("#", ChatTab::All), None);
        assert_eq!(parse_outgoing("#   ", ChatTab::All), None);
        assert_eq!(parse_outgoing("$Foo", ChatTab::All), None);
        assert_eq!(parse_outgoing("$ hi", ChatTab::All), None);
    }

    /// Regression for #293: `/whisper Foo hi` used to hit the GM dispatcher
    /// and never reached the server.
    #[test]
    fn whisper_slash_commands_route_to_the_server() {
        for command in WHISPER_COMMANDS {
            let SlashRoute::Chat(out) = route_slash(&format!("{command} Foo hi there"), None)
            else {
                panic!("{command} was not routed to chat");
            };
            assert_eq!(out.chat_type, chat_type::PM, "{command}");
            assert_eq!(out.receiver.as_deref(), Some("Foo"), "{command}");
            assert_eq!(out.message, "hi there", "{command}");
        }
    }

    /// textuisystem.txt L677/L678 list `/W` and `/w` as separate entries, so
    /// matching is case-sensitive and the vocabulary is exactly these four.
    #[test]
    fn whisper_command_vocabulary_is_the_data_one() {
        assert_eq!(WHISPER_COMMANDS, ["/Whisper", "/whisper", "/W", "/w"]);
        assert_eq!(route_slash("/WHISPER Foo hi", None), SlashRoute::Gm);
    }

    /// Everything outside the chat vocabulary stays with the GM dispatcher;
    /// an incomplete whisper is dropped instead of leaking to it. `/party` is
    /// in the vocabulary (L683) and now has a request path, so it routes to
    /// `Party` — never to `Gm` (#606, then #32).
    #[test]
    fn non_chat_slash_commands_stay_with_the_gm_dispatcher() {
        assert_eq!(route_slash("/invisible", None), SlashRoute::Gm);
        assert_eq!(
            route_slash("/party come", None),
            SlashRoute::Party(PartySlash::Invite("come".to_string()))
        );
        assert_eq!(route_slash("/w Foo", None), SlashRoute::Incomplete);
        assert_eq!(route_slash("/w Foo   ", None), SlashRoute::Incomplete);
        assert_eq!(route_slash("/w", None), SlashRoute::Incomplete);
    }

    /// #606: `/Reply` and its three spellings (textuisystem L672-675) answer
    /// the newest whisper, whose sender `net.rs` records in
    /// `ChatState::last_whisper_from`.
    #[test]
    fn reply_commands_target_the_last_whisper_partner() {
        for command in REPLY_COMMANDS {
            let SlashRoute::Chat(out) = route_slash(&format!("{command} hi there"), Some("Foo"))
            else {
                panic!("{command} was not routed to chat");
            };
            assert_eq!(out.chat_type, chat_type::PM, "{command}");
            assert_eq!(out.receiver.as_deref(), Some("Foo"), "{command}");
            // the whole rest is the message — a reply carries no name
            assert_eq!(out.message, "hi there", "{command}");
        }
    }

    /// Without a partner there is nothing to send, so the line is dropped
    /// rather than guessed at or leaked to the GM dispatcher.
    #[test]
    fn reply_without_a_partner_or_message_is_dropped() {
        assert_eq!(route_slash("/r hi", None), SlashRoute::Incomplete);
        assert_eq!(route_slash("/r", Some("Foo")), SlashRoute::Incomplete);
        assert_eq!(route_slash("/r    ", Some("Foo")), SlashRoute::Incomplete);
    }

    /// The vocabulary is the data's, transcribed verbatim: 4 whisper + 4 reply
    /// spellings, `/Stall` (implemented since #781) and 25 further commands =
    /// the 34 distinct strings of textuisystem L663-692 and L4584-4589 (36
    /// keys, whose values collapse to 34 distinct commands because
    /// `_WHISPER3`-`_WHISPER5` all spell `/w`). The total is a property of the
    /// data and must not move when a command gains a request path — only its
    /// column does.
    #[test]
    fn the_vocabulary_is_the_whole_data_table() {
        assert_eq!(WHISPER_COMMANDS.len(), 4);
        assert_eq!(REPLY_COMMANDS, ["/Reply", "/r", "/re", "/R"]);
        // 26 without a request path, minus the four party verbs and `/Stall`,
        // which now have one and moved to their own constants.
        assert_eq!(
            UNSUPPORTED_COMMANDS.len() + UNSUPPORTED_COMMANDS_TAIL.len(),
            21
        );
        assert_eq!(PARTY_INVITE_COMMANDS.len(), 2);
        let party_commands = [PARTY_EXPEL_COMMAND, PARTY_LEAVE_COMMAND];
        let mut all: Vec<&str> = WHISPER_COMMANDS
            .iter()
            .chain(REPLY_COMMANDS.iter())
            .chain([STALL_COMMAND].iter())
            .chain(UNSUPPORTED_COMMANDS.iter())
            .chain(UNSUPPORTED_COMMANDS_TAIL.iter())
            .chain(PARTY_INVITE_COMMANDS.iter())
            .chain(party_commands.iter())
            .copied()
            .collect();
        assert_eq!(all.len(), 34);
        all.sort_unstable();
        let mut unique = all.clone();
        unique.dedup();
        assert_eq!(all, unique, "no command may appear in two roles");
        assert!(
            all.iter().all(|c| c.starts_with('/') && c.len() > 1),
            "every entry is a slash command: {all:?}"
        );
    }

    /// A command the data ships but openroad cannot perform must never be
    /// swallowed: it answers with `UIIT_CHATERR_INVALID_COMMAND` (L1730) and
    /// is logged. This is the regression against re-creating a dead wire.
    #[test]
    fn vocabulary_commands_without_a_request_path_are_not_swallowed() {
        for command in UNSUPPORTED_COMMANDS
            .iter()
            .chain(UNSUPPORTED_COMMANDS_TAIL.iter())
        {
            assert_eq!(
                route_slash(command, Some("Foo")),
                SlashRoute::Unsupported,
                "{command}"
            );
            assert_eq!(
                route_slash(&format!("{command} Bar"), Some("Foo")),
                SlashRoute::Unsupported,
                "{command} with an argument"
            );
        }
        assert_eq!(INVALID_COMMAND_KEY, "UIIT_CHATERR_INVALID_COMMAND");
    }

    /// The four party verbs route to [`SlashRoute::Party`] with the name still
    /// unresolved — resolution needs the world (invite) or the roster (expel),
    /// neither of which `route_slash` can see.
    #[test]
    fn the_party_verbs_parse_into_an_unresolved_command() {
        assert_eq!(
            route_slash("/LeaveTheParty", None),
            SlashRoute::Party(PartySlash::Leave)
        );
        // leave takes no argument, and a trailing one does not change it
        assert_eq!(
            route_slash("/LeaveTheParty now", None),
            SlashRoute::Party(PartySlash::Leave)
        );
        for command in PARTY_INVITE_COMMANDS {
            assert_eq!(
                route_slash(&format!("{command} Alice"), None),
                SlashRoute::Party(PartySlash::Invite("Alice".to_string())),
                "{command}"
            );
            // no name is nothing to send, exactly like an empty whisper
            assert_eq!(
                route_slash(command, None),
                SlashRoute::Incomplete,
                "{command}"
            );
        }
        assert_eq!(
            route_slash("/BanishFromParty Bob", None),
            SlashRoute::Party(PartySlash::Expel("Bob".to_string()))
        );
        assert_eq!(
            route_slash("/BanishFromParty   ", None),
            SlashRoute::Incomplete
        );
    }

    /// `/Stall` is the one command that left [`UNSUPPORTED_COMMANDS`]: its
    /// request path (`0x70B1`) now exists, and this file's own rule is that a
    /// command with a path is wired rather than answered with
    /// `UIIT_CHATERR_INVALID_COMMAND` (#781).
    #[test]
    fn the_stall_command_routes_to_a_stall_open_and_no_longer_reads_unsupported() {
        assert_eq!(
            route_slash("/Stall Cheap potions", None),
            SlashRoute::OpenStall(Some("Cheap potions".into()))
        );
        // no title -> the caller fills in the vanilla default
        assert_eq!(route_slash("/Stall", None), SlashRoute::OpenStall(None));
        assert_eq!(route_slash("/Stall   ", None), SlashRoute::OpenStall(None));
        // the original's table is case-sensitive (see WHISPER_COMMANDS), so
        // the lowercase spelling is not in the vocabulary at all
        assert_eq!(route_slash("/stall", None), SlashRoute::Gm);
        assert!(!UNSUPPORTED_COMMANDS.contains(&"/Stall"));
        assert!(!UNSUPPORTED_COMMANDS_TAIL.contains(&"/Stall"));
    }
}
