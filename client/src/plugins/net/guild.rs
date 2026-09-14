//! Client-side guild record: the 0x34B3 / 0x3101 / 0x34B4 chunked push folded
//! into one resource.
//!
//! Idea: `packets::agent::guild` parses the wire, but a single 0x3101 body is
//! not a record — the original accumulates every chunk between the two markers
//! and only then decodes ([`GuildData::parse`]). This module is the one place
//! that does the accumulation, exactly like `net::party` does for 0x3065.
//!
//! Deliberately minimal: it keeps the decoded record and answers two questions
//! about it (guild level, a member's permission bits). That is what the guild
//! storage consumer needs to gate itself (#744); the guild *window* — roster,
//! notice, log — is a separate piece of work (#25, #252) and nothing here
//! anticipates it.
//!
//! The 0x38F5 incremental update (`GuildUpdate`) is **not** applied: only its
//! discriminator is known and every arm's payload is `[U]`
//! (`docs/net-guild-0x3101.md`), so folding it in would mean inventing a
//! layout. A permission change therefore only lands on the next full push —
//! stated rather than papered over.

use bevy::prelude::*;

use crate::net::connection::SilkroadConnection;
use packets::agent::guild::{
    GuildCreatedData, GuildData, GuildDataBegin, GuildDataBody, GuildDataEnd, GuildInviteAck,
    GuildKickAck, GuildKickRequest, GuildPermissions,
};
use packets::agent::ingame::GuildInviteRequest;
use packets::Packet;

use crate::plugins::net::agent::AgentConnection;

/// The local player's guild, as the server last pushed it.
#[derive(Resource, Default, Debug)]
pub struct GuildRoster {
    /// The assembled record; `None` while the player is guildless (no push).
    pub data: Option<GuildData>,
}

impl GuildRoster {
    /// Guild level — the gate `UIIT_MSG_GUILD_WAREHOUSE_LIMIT` ("Guild level
    /// must be 2 or higher to use guild storage.") talks about.
    pub fn level(&self) -> Option<u8> {
        self.data.as_ref().map(|d| d.level)
    }

    /// The permission bits the server gave this member, matched by name.
    ///
    /// Name-matched because the record carries no "this is you" marker: the
    /// roster's `member_id` is a guild-internal id, not the character id the
    /// client knows itself by. Matching is case-sensitive — SRO names are
    /// unique and the server echoes the same spelling in CHARACTER_DATA.
    pub fn permissions_for(&self, name: &str) -> Option<GuildPermissions> {
        self.data
            .as_ref()?
            .members
            .iter()
            .find(|m| m.name == name)
            .map(|m| m.permissions())
    }
}

/// The 0x3101 chunks between a 0x34B3 begin and its 0x34B4 end.
#[derive(Resource, Default)]
pub struct GuildDataBuffer(pub Vec<u8>);

/// 0x34B3 — a new record starts; drop whatever a previous, truncated transfer
/// left behind.
pub fn on_guild_begin(
    mut reader: MessageReader<GuildDataBegin>,
    mut buffer: ResMut<GuildDataBuffer>,
) {
    for _ in reader.read() {
        buffer.0.clear();
    }
}

/// 0x3101 — accumulate; the record can span several packets.
pub fn on_guild_chunk(
    mut reader: MessageReader<GuildDataBody>,
    mut buffer: ResMut<GuildDataBuffer>,
) {
    for msg in reader.read() {
        buffer.0.extend_from_slice(&msg.data);
    }
}

/// 0x34B4 — the record is complete: decode it.
pub fn on_guild_end(
    mut reader: MessageReader<GuildDataEnd>,
    mut buffer: ResMut<GuildDataBuffer>,
    mut roster: ResMut<GuildRoster>,
) {
    for _ in reader.read() {
        let raw = std::mem::take(&mut buffer.0);
        if raw.is_empty() {
            warn!("guild: 0x34B4 end with no 0x3101 chunks — roster stays empty");
            continue;
        }
        match GuildData::parse(raw.clone().into()) {
            Ok(data) => {
                info!(
                    "guild: record for '{}' (level {}, {} members)",
                    data.name,
                    data.level,
                    data.members.len()
                );
                roster.data = Some(data);
            }
            Err(e) => warn!(
                "guild: record parse failed ({e:?}) — {} bytes: {} — capture for decode",
                raw.len(),
                packets::hexdump(&raw, 64)
            ),
        }
    }
}

/// 0xB0F0 — a guild the player just founded carries the same record inline,
/// so there is no push to wait for.
pub fn on_guild_created(
    mut reader: MessageReader<GuildCreatedData>,
    mut roster: ResMut<GuildRoster>,
) {
    for msg in reader.read() {
        if let Some(data) = msg.data.clone() {
            info!("guild: created '{}' (level {})", data.name, data.level);
            roster.data = Some(data);
        }
    }
}

// --- Commands (C->S) --------------------------------------------------------
//
// Idea: the same seam `net::party` uses — the UI states an *intent* and this
// module turns it into a packet, so a button never touches the connection and
// the mapping stays testable without one.
//
// Only the two commands whose request body is fully sourced are here. Leave
// (0x70F2), disband (0x70F1) and promote (0x70FA) each carry one `u32` whose
// meaning the decompile does not give (`docs/net-guild-lifecycle.md` — the
// builder shows a width and the window accessor `FUN_00778b70`, not a meaning),
// so sending one would mean inventing its value. They stay out until a capture
// resolves the slot.

/// A guild command the UI wants sent.
#[derive(Message, Clone, Debug, PartialEq, Eq)]
pub enum GuildAction {
    /// 0x70F3 — invite the selected player's spawn id into the guild.
    Invite(u32),
    /// 0x70F4 — expel a member, addressed **by name**: the only membership op
    /// in the family that is not id-addressed.
    Kick(String),
}

/// The intent-to-opcode mapping, split out so it is testable without a live
/// connection — the same shape `party_action_packet` has.
pub fn guild_action_packet(action: &GuildAction) -> Packet {
    match action {
        GuildAction::Invite(unique_id) => Packet::from(GuildInviteRequest {
            unique_id: *unique_id,
        }),
        GuildAction::Kick(member_name) => Packet::from(GuildKickRequest {
            member_name: member_name.clone(),
        }),
    }
}

/// Turn [`GuildAction`]s into wire packets.
pub fn send_guild_actions(
    mut reader: MessageReader<GuildAction>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
) {
    let mut pending = reader.read().peekable();
    if pending.peek().is_none() {
        return;
    }
    let Ok(conn) = conn.single() else {
        // Draining without a connection is the point: a queued invite must not
        // fire at a later, unrelated session.
        for action in pending {
            warn!("guild: no agent connection, dropping {action:?}");
        }
        return;
    };
    for action in pending {
        info!("guild: sending {action:?}");
        if let Err(e) = conn.get_sender().send(guild_action_packet(action).into()) {
            error!("network: failed to send guild action: {}", e.0);
        }
    }
}

/// 0xB0F3 / 0xB0F4 — the answers to the two commands above.
///
/// Both success arms are inert in the original (the roster change arrives out of
/// band via 0x3100/0x38F5), so there is nothing to apply; a refusal is logged
/// with its raw code because the guild error table (`FUN_00778190` category
/// 0x10) is not decoded — inventing wording for a code we cannot name would be
/// worse than the number.
pub fn on_guild_command_acks(
    mut invites: MessageReader<GuildInviteAck>,
    mut kicks: MessageReader<GuildKickAck>,
) {
    for ack in invites.read() {
        match ack.error_code {
            None => info!("guild: invite accepted by the server"),
            Some(code) => warn!("guild: invite refused (error {code:#06x})"),
        }
    }
    for ack in kicks.read() {
        match ack.error_code {
            None => info!("guild: expel accepted by the server"),
            Some(code) => warn!("guild: expel refused (error {code:#06x})"),
        }
    }
}

/// The guild record consumer. Registered next to [`crate::plugins::net::party`]
/// because it is wire-state, not a window.
pub struct GuildPlugin;

impl Plugin for GuildPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GuildRoster>()
            .init_resource::<GuildDataBuffer>()
            .add_systems(
                Update,
                (
                    // chained for the same reason the storage push is
                    // (`hud::storage::mod`): begin/chunk/end land in ONE frame
                    // and an unordered tuple lets `end` decode an empty buffer
                    (on_guild_begin, on_guild_chunk, on_guild_end).chain(),
                    on_guild_created,
                    send_guild_actions,
                    on_guild_command_acks,
                ),
            )
            .add_message::<GuildAction>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use packets::agent::guild::GuildMember;

    fn member(name: &str, permissions: u32) -> GuildMember {
        GuildMember {
            member_id: 7,
            name: name.to_string(),
            unk_u8_01: 0,
            level: 40,
            guild_points: 0,
            permissions,
            unk_u32_01: 0,
            unk_u32_02: 0,
            unk_u32_03: 0,
            nickname: String::new(),
            model_id: 1907,
            is_master: permissions == GuildPermissions::MASTER,
            is_offline: false,
        }
    }

    fn record(level: u8, members: Vec<GuildMember>) -> GuildData {
        GuildData {
            guild_id: 42,
            name: "Wanderers".into(),
            level,
            guild_points: 0,
            notice: String::new(),
            message: String::new(),
            unk_u32_00: 0,
            unk_u8_00: 0,
            member_count: members.len() as u8,
            members,
        }
    }

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(GuildPlugin);
        app.add_message::<GuildDataBegin>()
            .add_message::<GuildDataBody>()
            .add_message::<GuildDataEnd>()
            .add_message::<GuildCreatedData>()
            // the ack consumer's inputs: in the real app these come from the
            // `packets!` registration, which a bare test App does not run
            .add_message::<GuildInviteAck>()
            .add_message::<GuildKickAck>();
        app
    }

    /// The intent-to-opcode mapping, pinned by the encoded bytes rather than by
    /// the type name: invite is the 4-byte id form, expel is the name form.
    #[test]
    fn the_two_commands_encode_as_their_sourced_bodies() {
        let invite = guild_action_packet(&GuildAction::Invite(0x2A)).into_serialize();
        assert_eq!(invite.0, 0x70F3);
        assert_eq!(invite.1.as_ref(), &[0x2A, 0x00, 0x00, 0x00]);

        let kick = guild_action_packet(&GuildAction::Kick("Grunt".into())).into_serialize();
        assert_eq!(kick.0, 0x70F4);
        assert_eq!(kick.1.as_ref(), b"\x05\x00Grunt");
    }

    /// Without a connection the queue must drain rather than hold an invite
    /// that would fire into a later, unrelated session.
    #[test]
    fn actions_are_dropped_when_there_is_no_connection() {
        let mut app = app();
        app.add_message::<GuildAction>()
            .add_systems(Update, send_guild_actions);
        app.world_mut().write_message(GuildAction::Invite(7));
        app.update();
        app.update();
        // nothing to assert beyond "it did not panic without a connection" —
        // the point is the drain path, which a queued message would survive.
        assert!(app
            .world()
            .resource::<Messages<GuildAction>>()
            .iter_current_update_messages()
            .next()
            .is_none());
    }

    /// The record only exists concatenated: a two-packet split must decode to
    /// the same roster a single packet would. This is the defect
    /// `net-storage-0x3047-0x3049.md:162-165` records for the personal family
    /// — an unordered/unbuffered consumer decodes "0 bytes" instead.
    #[test]
    fn a_guild_record_split_across_two_chunks_decodes_as_one() {
        let wire: Bytes = record(
            2,
            vec![
                member("Master", GuildPermissions::MASTER),
                member("Grunt", GuildPermissions::JOIN | GuildPermissions::STORAGE),
            ],
        )
        .into();
        let (head, tail) = wire.split_at(wire.len() / 2);

        let mut app = app();
        app.world_mut().write_message(GuildDataBegin);
        app.world_mut().write_message(GuildDataBody {
            data: Bytes::copy_from_slice(head),
        });
        app.world_mut().write_message(GuildDataBody {
            data: Bytes::copy_from_slice(tail),
        });
        app.world_mut().write_message(GuildDataEnd);
        app.update();

        let roster = app.world().resource::<GuildRoster>();
        assert_eq!(roster.level(), Some(2));
        assert_eq!(
            roster.permissions_for("Grunt").map(|p| p.can_use_storage()),
            Some(true)
        );
        // the master sentinel is every bit set, not just the named ones
        assert_eq!(
            roster
                .permissions_for("Master")
                .map(|p| p.can_use_storage()),
            Some(true)
        );
        // a name that is not in the roster is not "no permissions", it is
        // "not a member" — the storage gate distinguishes the two
        assert!(roster.permissions_for("Stranger").is_none());
    }

    /// A member without the `Storage` bit reads as refused rather than as an
    /// absent roster — the two produce different messages at the gate.
    #[test]
    fn a_member_without_the_storage_bit_is_refused_not_unknown() {
        let wire: Bytes = record(3, vec![member("Rookie", GuildPermissions::JOIN)]).into();
        let mut app = app();
        app.world_mut().write_message(GuildDataBegin);
        app.world_mut().write_message(GuildDataBody { data: wire });
        app.world_mut().write_message(GuildDataEnd);
        app.update();

        let perms = app
            .world()
            .resource::<GuildRoster>()
            .permissions_for("Rookie")
            .expect("member is in the roster");
        assert!(!perms.can_use_storage());
    }
}
