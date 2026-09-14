//! Guild storage (guild warehouse) session state: the client half of the
//! 0x7250 / 0x7251 / 0x7252 / 0xB250 / 0x3253 / 0x3255 / 0x3254 family.
//!
//! Idea: this is the guild analogue of `hud::storage`, with one structural
//! difference that decides the shape of the code — **the item stream is a
//! reply, not a push the server volunteers**. The original's 0xB250 success
//! arm sends 0x7252 itself (`sro_client.exe@0088f630` → `@00820980`,
//! `docs/net-guild-storage-0x7250.md`), so a consumer that opens and then
//! waits for 0x3253 waits forever. Hence [`on_guild_storage_response`] is the
//! system that asks for the contents.
//!
//! The rest mirrors the personal warehouse deliberately: the 0x3255 chunks are
//! meaningless alone and accumulate in [`GuildStorageDataBuffer`] until the
//! 0x3254 marker parses them, and the three handlers are `.chain()`ed —
//! unordered, `end` parses before `chunk` has filled the buffer, which is the
//! exact defect `docs/net-storage-0x3047-0x3049.md:162-165` records for the
//! personal family.
//!
//! **No window.** `GDR_GUILDSTORAGEROOM` is explicitly not ported
//! (`net-storage-0x3047-0x3049.md:227`), so this decodes into the model and
//! stops there — that is the doc's own minimal first step. What *is* visible
//! is the refusal path, which is where the player otherwise sees nothing.
//!
//! Nothing here is capture-verified: no `packet_dump/*.log` exists for any of
//! the seven opcodes and go-sro implements none of them.

use bevy::prelude::*;

use packets::agent::guild::GuildPermissions;
use packets::agent::guild_storage::{
    parse_guild_storage_items, GuildStorageCloseRequest, GuildStorageDataBegin,
    GuildStorageDataChunk, GuildStorageDataEnd, GuildStorageListRequest, GuildStorageOpenRequest,
    GuildStorageResponse,
};
use packets::{hexdump, Packet};

use crate::net::connection::SilkroadConnection;
use crate::plugins::hud::chat::model::{ChatHistory, ChatLine};
use crate::plugins::hud::npc_dialog::model::{NpcDialogState, OpenGuildStorage};
use crate::plugins::hud::system_message::model::format_template;
use crate::plugins::net::agent::AgentConnection;
use crate::plugins::net::character_info::CharacterInfo;
use crate::plugins::net::entities::NetworkId;
use crate::plugins::net::guild::GuildRoster;
use crate::plugins::net::inventory::Inventory;
use crate::plugins::player::Player;
use crate::plugins::textdata::{ClientItemData, ClientUiStrings};

/// "At the moment guild member %s is using guild storage, so it cannot be
/// opened." — the original formats exactly this key with the name the 0xB250
/// in-use arm carries (`textuisystem.txt:1423`).
pub const IN_USE_KEY: &str = "UIIT_MSG_GUILD_WAREHOUSE_USE";
pub const IN_USE_FALLBACK: &str =
    "At the moment guild member %s is using guild storage, so it cannot be opened.";

/// "Guild level must be 2 or higher to use guild storage."
/// (`textuisystem.txt:1424`) — the gate the dialog label itself advertises
/// ("Use guild storage. (level 2 or above)").
pub const LEVEL_KEY: &str = "UIIT_MSG_GUILD_WAREHOUSE_LIMIT";
pub const LEVEL_FALLBACK: &str = "Guild level must be 2 or higher to use guild storage.";

/// "You are not authorized." (`textuisystem.txt:1406`) — the guild family's own
/// permission refusal, reused for the `Storage = 8` bit rather than inventing a
/// storage-specific string.
pub const DENIED_KEY: &str = "UIIT_MSG_GUILDERR_PERMISSION_DENIED";
pub const DENIED_FALLBACK: &str = "You are not authorized.";

/// The guild level the client's own string names as the minimum
/// (`UIIT_MSG_GUILD_WAREHOUSE_LIMIT`, and the dialog label "(level 2 or
/// above)"). Sourced, not chosen.
pub const MIN_GUILD_LEVEL: u8 = 2;

/// The 0x3255 chunks between a 0x3253 begin and its 0x3254 end.
#[derive(Resource, Default)]
pub struct GuildStorageDataBuffer(pub Vec<u8>);

#[derive(Clone, Debug, PartialEq)]
pub struct GuildStorageSession {
    /// The warehouse NPC the session belongs to — the 0x7251 close needs the
    /// same id the 0x7250 open carried.
    pub npc: Entity,
    pub npc_id: u32,
}

/// The guild warehouse as the server last described it. Model only: there is
/// no window to render it yet (see the module note).
#[derive(Resource, Default)]
pub struct GuildStorageState {
    pub session: Option<GuildStorageSession>,
    /// Slots + the guild account's gold, mirrored ONLY from server packets,
    /// reusing [`Inventory`] exactly like the personal warehouse does.
    pub items: Inventory,
    /// The item stream arrived for this session.
    pub synced: bool,
}

/// Why an open attempt did not reach the wire. Split out so the gate is
/// testable without a Bevy `App` and without a connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuildStorageGate {
    Allowed,
    /// The player is in no guild, or is not in the pushed roster.
    NotAMember,
    /// In the guild, without the `Storage = 8` bit.
    NotAuthorized,
    /// Guild level below [`MIN_GUILD_LEVEL`].
    GuildLevelTooLow,
}

/// The client-side gate the original advertises in its own strings: guild
/// membership, then the level, then the `Storage` permission bit
/// (`GUILD_PERMISSION_STORAGE = 8`, `xBot/…/SRGuildMember.cs:35`).
///
/// Deviation, stated (ADR-0009): the *server* is authoritative here and would
/// refuse anyway. The client pre-check exists because the refusal it would send
/// back is an uncaptured error code we could not map to a message, whereas
/// these three strings are in the player's own data.
pub fn gate(level: Option<u8>, permissions: Option<GuildPermissions>) -> GuildStorageGate {
    let (Some(level), Some(permissions)) = (level, permissions) else {
        return GuildStorageGate::NotAMember;
    };
    if level < MIN_GUILD_LEVEL {
        return GuildStorageGate::GuildLevelTooLow;
    }
    if !permissions.can_use_storage() {
        return GuildStorageGate::NotAuthorized;
    }
    GuildStorageGate::Allowed
}

/// The dialog's "Use guild storage." option: check the gate, then ask the
/// server to open (0x7250).
#[allow(clippy::too_many_arguments)]
pub fn open_guild_storage(
    mut requests: MessageReader<OpenGuildStorage>,
    npcs: Query<&NetworkId>,
    players: Query<&CharacterInfo, With<Player>>,
    roster: Res<GuildRoster>,
    ui_strings: Res<ClientUiStrings>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    mut state: ResMut<GuildStorageState>,
    mut buffer: ResMut<GuildStorageDataBuffer>,
    mut history: ResMut<ChatHistory>,
) {
    for OpenGuildStorage { npc } in requests.read() {
        let Ok(network_id) = npcs.get(*npc) else {
            warn!("guild storage: OpenGuildStorage for an entity without a network id");
            continue;
        };
        let name = players.single().ok().and_then(|info| info.name.clone());
        let permissions = name.as_deref().and_then(|n| roster.permissions_for(n));
        match gate(roster.level(), permissions) {
            GuildStorageGate::Allowed => {}
            GuildStorageGate::GuildLevelTooLow => {
                history.push(ChatLine::system(
                    ui_strings.get_or(LEVEL_KEY, LEVEL_FALLBACK),
                ));
                continue;
            }
            GuildStorageGate::NotAuthorized | GuildStorageGate::NotAMember => {
                history.push(ChatLine::system(
                    ui_strings.get_or(DENIED_KEY, DENIED_FALLBACK),
                ));
                continue;
            }
        }
        state.session = Some(GuildStorageSession {
            npc: *npc,
            npc_id: network_id.0,
        });
        state.items = Inventory::default();
        state.synced = false;
        buffer.0.clear();
        let Ok(conn) = conn.single() else {
            continue;
        };
        info!("guild storage: opening (0x7250) at npc {}", network_id.0);
        let request = GuildStorageOpenRequest {
            npc_unique_id: network_id.0,
        };
        if let Err(e) = conn.get_sender().send(Packet::from(request).into()) {
            error!("network: failed to send GuildStorageOpenRequest: {}", e.0);
        }
    }
}

/// 0xB250 — the ack, and the packet that makes this family different: on
/// success the original **immediately sends 0x7252**, and the item stream is
/// the answer to that. On the in-use arm it names the member holding the
/// guild-wide lock.
pub fn on_guild_storage_response(
    mut reader: MessageReader<GuildStorageResponse>,
    ui_strings: Res<ClientUiStrings>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    state: Res<GuildStorageState>,
    mut history: ResMut<ChatHistory>,
) {
    for msg in reader.read() {
        if !msg.is_success() {
            match msg.lock_holder() {
                Some(holder) => {
                    let text = format_template(ui_strings.get_or(IN_USE_KEY, IN_USE_FALLBACK), &[holder]);
                    warn!("guild storage: in use by '{holder}' (0xB250 0x4C48)");
                    history.push(ChatLine::system(text));
                }
                None => warn!(
                    "guild storage: open refused (0xB250 result {}, error {:#06X}) — code table is uncaptured, record it in docs/net-guild-storage-0x7250.md",
                    msg.result,
                    msg.error_code.unwrap_or_default()
                ),
            }
            continue;
        }
        let Some(session) = state.session.as_ref() else {
            warn!("guild storage: unsolicited 0xB250 success — no open session, not listing");
            continue;
        };
        let Ok(conn) = conn.single() else {
            continue;
        };
        // [U] which value the original echoes: its 0xB250 arm stores a `u32`
        // read from `FUN_00778b70()` (a client-side getter, NOT a field of the
        // packet — 0xB250's success arm carries no body) and 0x7252 sends
        // that. The NPC id we opened with is the only u32 this side of the
        // exchange has; resolving observation is one `packet_dump/0x7252`
        // capture from the original client against a live guild warehouse.
        info!(
            "guild storage: requesting contents (0x7252, storage_id = npc {})",
            session.npc_id
        );
        let request = GuildStorageListRequest {
            storage_id: session.npc_id,
        };
        if let Err(e) = conn.get_sender().send(Packet::from(request).into()) {
            error!("network: failed to send GuildStorageListRequest: {}", e.0);
        }
    }
}

/// 0x3253 — the stream begins and carries the guild account's gold.
pub fn on_guild_storage_begin(
    mut reader: MessageReader<GuildStorageDataBegin>,
    mut state: ResMut<GuildStorageState>,
    mut buffer: ResMut<GuildStorageDataBuffer>,
) {
    for msg in reader.read() {
        info!("guild storage: data begin (0x3253), gold {}", msg.gold);
        if !msg.tail.is_empty() {
            debug!(
                "guild storage: 0x3253 has an unexpected tail {} — capture for decode",
                hexdump(&msg.tail, 24)
            );
        }
        state.items.gold = msg.gold;
        buffer.0.clear();
    }
}

/// 0x3255 — accumulate; the item section can span several packets.
pub fn on_guild_storage_chunk(
    mut reader: MessageReader<GuildStorageDataChunk>,
    mut buffer: ResMut<GuildStorageDataBuffer>,
) {
    for msg in reader.read() {
        buffer.0.extend_from_slice(&msg.raw);
    }
}

/// 0x3254 — the accumulated chunks are complete: parse the item section.
pub fn on_guild_storage_end(
    mut reader: MessageReader<GuildStorageDataEnd>,
    item_data: Res<ClientItemData>,
    mut state: ResMut<GuildStorageState>,
    mut buffer: ResMut<GuildStorageDataBuffer>,
) {
    for _ in reader.read() {
        let raw = std::mem::take(&mut buffer.0);
        if raw.is_empty() {
            warn!("guild storage: 0x3254 end with no 0x3255 chunks — storage stays unsynced");
            continue;
        }
        match parse_guild_storage_items(&raw, &*item_data) {
            Ok((size, items)) => {
                info!(
                    "guild storage: data end (0x3254) — size {size}, {} items",
                    items.len()
                );
                let gold = state.items.gold;
                let mut slots = vec![None; size as usize];
                for item in items {
                    let index = item.slot as usize;
                    if index >= slots.len() {
                        slots.resize(index + 1, None);
                    }
                    slots[index] = Some(item);
                }
                state.items = Inventory {
                    slots,
                    avatar_slots: Vec::new(),
                    gold,
                };
                state.synced = true;
            }
            Err(e) => warn!(
                "guild storage: item section parse failed ({e:?}) — {} bytes: {} — capture for decode",
                raw.len(),
                hexdump(&raw, 64)
            ),
        }
    }
}

/// Guild storage is a **guild-wide exclusive lock** — that is why the family
/// has a close opcode at all (`0xB250` error `0x4C48` names whoever holds it).
/// So leaving the NPC's dialog must send 0x7251; a session we forget to close
/// locks the warehouse for the whole guild.
pub fn close_guild_storage_with_dialog(
    dialog: Res<NpcDialogState>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    mut state: ResMut<GuildStorageState>,
) {
    let Some(session) = state.session.as_ref() else {
        return;
    };
    if dialog.npc() == Some(session.npc) {
        return;
    }
    let npc_id = session.npc_id;
    state.session = None;
    state.synced = false;
    let Ok(conn) = conn.single() else {
        return;
    };
    info!("guild storage: closing (0x7251) at npc {npc_id}");
    let request = GuildStorageCloseRequest {
        npc_unique_id: npc_id,
    };
    if let Err(e) = conn.get_sender().send(Packet::from(request).into()) {
        error!("network: failed to send GuildStorageCloseRequest: {}", e.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three refusals the client's own strings distinguish. A missing
    /// roster is NOT "no permissions": the player may simply not be in a
    /// guild, and the original has a different string for each.
    #[test]
    fn the_gate_separates_membership_level_and_permission() {
        assert_eq!(gate(None, None), GuildStorageGate::NotAMember);
        assert_eq!(
            gate(Some(5), None),
            GuildStorageGate::NotAMember,
            "in a guild whose roster does not list us is still not a member"
        );
        assert_eq!(
            gate(Some(1), Some(GuildPermissions(GuildPermissions::ALL))),
            GuildStorageGate::GuildLevelTooLow,
            "UIIT_MSG_GUILD_WAREHOUSE_LIMIT: level 2 or higher"
        );
        assert_eq!(
            gate(Some(2), Some(GuildPermissions(GuildPermissions::JOIN))),
            GuildStorageGate::NotAuthorized
        );
        assert_eq!(
            gate(Some(2), Some(GuildPermissions(GuildPermissions::STORAGE))),
            GuildStorageGate::Allowed
        );
        // the master sentinel is every bit set, so it passes without the
        // named bit being special-cased
        assert_eq!(
            gate(Some(2), Some(GuildPermissions(GuildPermissions::MASTER))),
            GuildStorageGate::Allowed
        );
    }

    /// The 0x3255 chunks are meaningless alone. Drive the real systems in
    /// their registered order with a section split across two packets: an
    /// unordered (or unbuffered) consumer decodes "0 bytes" here — the exact
    /// defect `docs/net-storage-0x3047-0x3049.md:162-165` records for the
    /// personal family.
    #[test]
    fn the_guild_item_stream_is_buffered_across_chunks_before_it_parses() {
        use bytes::Bytes;

        let mut app = App::new();
        app.add_message::<GuildStorageDataBegin>()
            .add_message::<GuildStorageDataChunk>()
            .add_message::<GuildStorageDataEnd>()
            .init_resource::<GuildStorageState>()
            .init_resource::<GuildStorageDataBuffer>()
            .init_resource::<ClientItemData>()
            .add_systems(
                Update,
                (
                    on_guild_storage_begin,
                    on_guild_storage_chunk,
                    on_guild_storage_end,
                )
                    .chain(),
            );

        app.world_mut().write_message(GuildStorageDataBegin {
            gold: 1000,
            tail: Bytes::new(),
        });
        // capacity 60, count 0 — split so the end marker cannot parse a
        // complete section from either half on its own
        app.world_mut().write_message(GuildStorageDataChunk {
            raw: Bytes::from_static(&[60]),
        });
        app.world_mut().write_message(GuildStorageDataChunk {
            raw: Bytes::from_static(&[0]),
        });
        app.world_mut().write_message(GuildStorageDataEnd);
        app.update();

        let state = app.world().resource::<GuildStorageState>();
        assert!(state.synced, "the section parsed from the joined chunks");
        assert_eq!(state.items.size(), 60);
        assert_eq!(state.items.gold, 1000, "0x3253 carries the guild gold");
        assert!(
            app.world()
                .resource::<GuildStorageDataBuffer>()
                .0
                .is_empty(),
            "the buffer is consumed, so a second stream cannot inherit it"
        );
    }

    /// The in-use refusal is the one 0xB250 arm the original special-cases,
    /// and it is only useful *with* the name filled in — an unformatted
    /// template would show the player a literal `%s`.
    #[test]
    fn the_in_use_refusal_names_the_lock_holder() {
        let text = format_template(IN_USE_FALLBACK, &["Grunt"]);
        assert!(text.contains("Grunt"), "{text}");
        assert!(!text.contains("%s"), "{text}");
    }
}
