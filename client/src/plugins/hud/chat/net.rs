//! Chat packet consumers: 0x3026 lines into the history, 0xB025 acks moving
//! pending own-messages in, 0x302D restriction notices.
//!
//! Sender resolution: proximity channels (All/AllGM/NPC) only carry a unique
//! id — resolved through [`NetworkEntities`] to the spawned entity's `Name`.
//! Own echoes are dropped here — on **every** channel — because own messages
//! enter the history via the 0xB025 ack path (`ChatState::pending`). The two
//! halves need two different joins: the proximity channels give a `u32` sender
//! id to compare against the local `NetworkId`, while party/guild/union/…
//! give only a name, so those compare against the local `DisplayName`.

use bevy::prelude::*;

use packets::agent::chat::{ChatResponse, ChatRestriction, ChatUpdate};
use packets::agent::ingame::NoticeUpdate;

use crate::plugins::hud::toast::{ShowToast, ToastKind};
use crate::plugins::net::entities::{DisplayName, NetworkEntities, NetworkId};
use crate::plugins::player::Player;
use crate::plugins::textdata::{ClientCharacterData, ClientTextNames, ClientUiStrings};

use super::model::{
    format_restriction, ChatHistory, ChatLine, ChatLineKind, ChatState, CANT_CHATTING_FALLBACK,
    CANT_CHATTING_KEY,
};

/// Apply 0x3026: append incoming chat lines to the shared history.
pub fn on_chat_update(
    mut reader: MessageReader<ChatUpdate>,
    entities: Res<NetworkEntities>,
    names: Query<&Name>,
    player: Query<(&NetworkId, Option<&DisplayName>), With<Player>>,
    mut history: ResMut<ChatHistory>,
    mut state: ResMut<ChatState>,
    mut toasts: MessageWriter<ShowToast>,
) {
    for msg in reader.read() {
        // raw-variant log to diagnose server-side channel quirks (e.g. local
        // chat coming back as another chat type)
        debug!("chat: update {:?}", msg);
        let own = player.single().ok();
        let own_id = own.map(|(id, _)| id.0);
        let own_name = own.and_then(|(_, name)| name).map(|name| name.0.as_str());
        let resolve = |id: u32| -> String {
            entities
                .get(id)
                .and_then(|entity| names.get(entity).ok())
                .map(|name| name.as_str().to_string())
                .unwrap_or_else(|| format!("{id}"))
        };

        // The named channels are the other half of the own-echo rule the
        // proximity arm below applies. They carry a sender *name* and no id, so
        // the join is the local player's `DisplayName` — the same name join the
        // quick-party board and the map markers already use. Without this the
        // sender saw their own line twice (once from the ack, once from the
        // server's echo) while everyone else saw it once.
        if let Some(sender) = named_sender(msg) {
            if own_name == Some(sender) {
                continue;
            }
        }

        let line = match msg {
            ChatUpdate::All { sender_id, message }
            | ChatUpdate::AllGm { sender_id, message }
            | ChatUpdate::Npc { sender_id, message } => {
                // own proximity messages come back through the 0xB025 ack
                if Some(*sender_id) == own_id {
                    continue;
                }
                let kind = match msg {
                    ChatUpdate::AllGm { .. } => ChatLineKind::AllGm,
                    ChatUpdate::Npc { .. } => ChatLineKind::Npc,
                    _ => ChatLineKind::All,
                };
                ChatLine {
                    kind,
                    sender: Some(resolve(*sender_id)),
                    text: message.clone(),
                }
            }
            ChatUpdate::Notice { message } => {
                // Chat type 7 is the server's global notice, the one push that
                // has no sender and belongs on screen rather than only in the
                // log — so it also feeds `GDR_NOTICE`. The binding is [S]: all
                // three CIFNotify blocks carry `Text=""` and nothing in the
                // data names their strings (`docs/re/ui/notify-toast-widgets.md`
                // §3.4). It stays in the chat history either way.
                toasts.write(ShowToast::new(ToastKind::Notice, message.clone()));
                ChatLine {
                    kind: ChatLineKind::Notice,
                    sender: None,
                    text: message.clone(),
                }
            }
            ChatUpdate::Pm { sender, message } => {
                // `/Reply` (textuisystem L672-675) answers the newest whisper,
                // so the incoming-whisper path is where that target is
                // recorded — the vocabulary parser has no other source for it.
                state.last_whisper_from = Some(sender.clone());
                ChatLine {
                    kind: ChatLineKind::WhisperFrom,
                    sender: Some(sender.clone()),
                    text: message.clone(),
                }
            }
            ChatUpdate::Party { sender, message } => named(ChatLineKind::Party, sender, message),
            ChatUpdate::Guild { sender, message } => named(ChatLineKind::Guild, sender, message),
            ChatUpdate::Global { sender, message } => named(ChatLineKind::Global, sender, message),
            ChatUpdate::Stall { sender, message } => named(ChatLineKind::Stall, sender, message),
            ChatUpdate::Union { sender, message } => named(ChatLineKind::Union, sender, message),
            ChatUpdate::Academy { sender, message } => {
                named(ChatLineKind::Academy, sender, message)
            }
        };
        history.push(line);
    }
}

/// The sender name of a channel that carries one, or `None` for the channels
/// keyed by id (proximity), addressed to us (whisper), or sent by nobody
/// (notice).
///
/// Whispers are deliberately absent: an incoming whisper's `sender` is the
/// *other* party, never us, and our own outgoing whisper is a `WhisperTo` line
/// the ack already parks.
fn named_sender(msg: &ChatUpdate) -> Option<&str> {
    match msg {
        ChatUpdate::Party { sender, .. }
        | ChatUpdate::Guild { sender, .. }
        | ChatUpdate::Global { sender, .. }
        | ChatUpdate::Stall { sender, .. }
        | ChatUpdate::Union { sender, .. }
        | ChatUpdate::Academy { sender, .. } => Some(sender.as_str()),
        _ => None,
    }
}

fn named(kind: ChatLineKind, sender: &str, message: &str) -> ChatLine {
    ChatLine {
        kind,
        sender: Some(sender.to_string()),
        text: message.to_string(),
    }
}

/// Apply 0xB025: success moves the pending own-line into the history; a
/// failure only logs (no chat line) for now.
pub fn on_chat_response(
    mut reader: MessageReader<ChatResponse>,
    mut state: ResMut<ChatState>,
    mut history: ResMut<ChatHistory>,
) {
    for msg in reader.read() {
        let pending = state.pending.remove(&msg.chat_index);
        if msg.result == 1 {
            if let Some(line) = pending {
                history.push(line);
            }
        } else {
            warn!(
                "chat: send rejected (type {:#04x} index {} error {:#06x})",
                msg.chat_type,
                msg.chat_index,
                msg.error.unwrap_or(0)
            );
        }
    }
}

/// Apply 0x302D: surface the mute and block sending until it lapses.
pub fn on_chat_restriction(
    mut reader: MessageReader<ChatRestriction>,
    time: Res<Time>,
    ui_strings: Res<ClientUiStrings>,
    mut state: ResMut<ChatState>,
    mut history: ResMut<ChatHistory>,
) {
    for msg in reader.read() {
        state.restricted_until = Some(time.elapsed_secs_f64() + msg.seconds as f64);
        // vanilla's own wording, `UIIT_STT_CANT_CHATTING` (textuisystem
        // L3065, "Chat Restricted: %d seconds") — the readout control it
        // belongs to is `GDR_CHAT_STA_PENALTY`, spawned by the chat window.
        history.push(ChatLine::system(format_restriction(
            ui_strings.get_or(CANT_CHATTING_KEY, CANT_CHATTING_FALLBACK),
            msg.seconds as u32,
        )));
    }
}

/// Apply 0x300C: the unique-monster notices become a system line and a notice
/// toast.
///
/// Idea: the original renders these through `FUN_008c9c30(L"<textdata key>")`
/// and those key strings did not survive into the decompilation
/// (`docs/net-misc-0x2113.md` §0x300C, "Unknowns"). So the *routing* is cloned —
/// a global, senderless announcement, which is what `ChatUpdate::Notice`
/// already does — while the **wording is a stated deviation**: our own English
/// sentence around the localized monster name, until a string xref pass
/// recovers the original key. The name itself is not invented: `ref_id` is a
/// ref-data object id (the original hands it to its ref lookup), so it resolves
/// through characterdata's `NameStrID` exactly like the target window names a
/// mob. An unresolvable id degrades to the raw number rather than dropping the
/// announcement.
///
/// Codes other than the two unique ones carry no recorded field widths, so they
/// are logged and ignored — the original's `default:` arm does nothing either.
pub fn on_notice_update(
    mut reader: MessageReader<NoticeUpdate>,
    char_data: Res<ClientCharacterData>,
    names: Res<ClientTextNames>,
    mut history: ResMut<ChatHistory>,
    mut toasts: MessageWriter<ShowToast>,
) {
    for msg in reader.read() {
        let monster = |ref_id: u32| -> String {
            char_data
                .get(&(ref_id as i32))
                .and_then(|row| row.name_key())
                .and_then(|key| names.name(key))
                .map(str::to_string)
                .unwrap_or_else(|| ref_id.to_string())
        };

        let Some(text) = notice_line(msg, monster) else {
            continue;
        };

        toasts.write(ShowToast::new(ToastKind::Notice, text.clone()));
        history.push(ChatLine::system(text));
    }
}

/// The wording for one notice, or `None` for a code we do not render.
///
/// Split out from the system so the two sentences can be tested without a
/// characterdata table: `monster` is whatever resolves a ref id to a name.
fn notice_line(msg: &NoticeUpdate, monster: impl Fn(u32) -> String) -> Option<String> {
    match msg {
        NoticeUpdate::UniqueAppeared { ref_id } => {
            Some(format!("{} has appeared.", monster(*ref_id)))
        }
        NoticeUpdate::UniqueKilled { ref_id, player } => {
            Some(format!("{} was killed by {player}.", monster(*ref_id)))
        }
        other => {
            debug!("notice: unhandled code {:#06x} ({other:?})", other.code());
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    /// The named channels are exactly the ones whose sender the server gives
    /// us as a name — the set the own-echo filter has to cover.
    #[test]
    fn only_the_channels_with_a_name_have_a_named_sender() {
        for msg in [
            ChatUpdate::Party {
                sender: "Ahri".into(),
                message: "hi".into(),
            },
            ChatUpdate::Guild {
                sender: "Ahri".into(),
                message: "hi".into(),
            },
            ChatUpdate::Union {
                sender: "Ahri".into(),
                message: "hi".into(),
            },
            ChatUpdate::Academy {
                sender: "Ahri".into(),
                message: "hi".into(),
            },
            ChatUpdate::Global {
                sender: "Ahri".into(),
                message: "hi".into(),
            },
            ChatUpdate::Stall {
                sender: "Ahri".into(),
                message: "hi".into(),
            },
        ] {
            assert_eq!(named_sender(&msg), Some("Ahri"), "{msg:?} carries a name");
        }

        // the proximity channels are keyed by id, and a whisper's sender is
        // the *other* party — filtering on it would eat incoming whispers
        for msg in [
            ChatUpdate::All {
                sender_id: 1,
                message: "hi".into(),
            },
            ChatUpdate::Npc {
                sender_id: 1,
                message: "hi".into(),
            },
            ChatUpdate::Pm {
                sender: "Ahri".into(),
                message: "hi".into(),
            },
            ChatUpdate::Notice {
                message: "hi".into(),
            },
        ] {
            assert_eq!(named_sender(&msg), None, "{msg:?} has no name to filter on");
        }
    }

    /// The defect: the sender saw their own party line twice — once from the
    /// 0xB025 ack that parks it, once from the server's own echo — while
    /// everybody else saw it once. The pair of assertions is what proves the
    /// fix drops one and not both.
    #[test]
    fn our_own_named_line_is_dropped_and_everyone_elses_is_kept() {
        use crate::plugins::hud::chat::model::ChatLineKind;

        let mut app = App::new();
        app.init_resource::<NetworkEntities>()
            .init_resource::<ChatHistory>()
            .init_resource::<ChatState>()
            .add_message::<ChatUpdate>()
            .add_message::<ShowToast>()
            .add_systems(Update, on_chat_update);
        app.world_mut()
            .spawn((Player, NetworkId(42), DisplayName("Ahri".into())));

        app.world_mut().write_message(ChatUpdate::Party {
            sender: "Ahri".into(),
            message: "hi there".into(),
        });
        app.world_mut().write_message(ChatUpdate::Party {
            sender: "priavte".into(),
            message: "hi".into(),
        });
        app.update();

        let history = app.world().resource::<ChatHistory>();
        let party: Vec<&ChatLine> = history
            .iter()
            .filter(|line| line.kind == ChatLineKind::Party)
            .collect();
        assert_eq!(party.len(), 1, "our own echo survived");
        assert_eq!(party[0].sender.as_deref(), Some("priavte"));
    }

    /// The two captured spawn lines name the monster, not the raw ref id.
    #[test]
    fn a_captured_unique_spawn_becomes_a_named_announcement() {
        let notice = NoticeUpdate::try_from(Bytes::from_static(&[0x05, 0x0c, 0x43, 0x95, 0, 0]))
            .expect("capture decodes");

        let line = notice_line(&notice, |id| {
            assert_eq!(id, 38211);
            "Tiger Girl".to_string()
        });

        assert_eq!(line.as_deref(), Some("Tiger Girl has appeared."));
    }

    /// An unresolvable ref id still announces — degrading to the number beats
    /// swallowing the notice.
    #[test]
    fn an_unresolvable_ref_id_degrades_to_the_number() {
        let notice = NoticeUpdate::UniqueAppeared { ref_id: 38211 };
        let line = notice_line(&notice, |id| id.to_string());
        assert_eq!(line.as_deref(), Some("38211 has appeared."));
    }

    /// A kill names the killer.
    #[test]
    fn a_unique_kill_names_the_killer() {
        let notice = NoticeUpdate::UniqueKilled {
            ref_id: 38211,
            player: "Hunter".to_string(),
        };
        let line = notice_line(&notice, |_| "Tiger Girl".to_string());
        assert_eq!(line.as_deref(), Some("Tiger Girl was killed by Hunter."));
    }

    /// The captured 0x0C18 code has no recorded meaning, so it renders nothing
    /// rather than an invented sentence.
    #[test]
    fn an_unrecorded_notice_code_renders_nothing() {
        let notice =
            NoticeUpdate::try_from(Bytes::from_static(&[0x18, 0x0c, 0x02, 0x03])).expect("decodes");
        assert_eq!(notice_line(&notice, |id| id.to_string()), None);
    }
}
