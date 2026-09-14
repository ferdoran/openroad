//! Client-side party roster: 0x3065's full list plus 0x3864's deltas, folded
//! into one resource.
//!
//! Idea: the whole party wire family is already parsed in
//! `packets::agent::party`, but nothing client-side kept it, so every consumer
//! (the world-map markers, the minimap signs, the party window) would have to
//! fold the full-roster packet and its five delta types itself. This module is
//! the single place that does it, and it keeps the wire shapes verbatim: a
//! member is stored as the wire's own [`PartyMemberCore`], and `hp_mp` stays the
//! raw byte the server sent ([`PartyHpMp`] packs both bars into nibbles).
//! Turning that byte into percentages is the view's job — the roster gauges are
//! a crop, not a stretch, so quantisation belongs where it is drawn, not here.
//!
//! # Records are partial, so the fold MERGES
//!
//! Every party record on the wire begins with a presence bitmask and carries
//! only the fields whose bit is set (see `packets::agent::party`'s module note).
//! A full roster push names everything; a 0x3864 delta typically names one
//! field. So a stored member is the **accumulation** of every record seen for
//! that id, not the last one: overwriting the record with a two-byte hp/mp delta
//! would blank the member's name, level, guild and position. `presence` is
//! OR-ed as it goes, so it always says what we actually know about a member.
//!
//! The leader is now modelled — `PartyData`'s header bit 0 carries
//! `master_join_id`, which the original compares against the local player's own
//! JID. That used to be an UNKNOWN here because the header was nine opaque
//! bytes; the binary resolved it.
//!
//! The second half of the file is the outbound side: the four verbs the player
//! can actually trigger (invite / create / leave / kick) and the two acks that
//! answer them. See the section comment there for why one message covers both
//! invite opcodes.

use bevy::prelude::*;

use packets::agent::ingame::PartyInviteRequest;
use packets::agent::party::{
    PartyCreateResponse, PartyCreationRequest, PartyData, PartyInviteResponse, PartyKickRequest,
    PartyLeave, PartyMemberCore, PartySetup, PartyUpdate,
};
use packets::Packet;

use crate::net::connection::SilkroadConnection;
use crate::plugins::hud::chat::model::{ChatHistory, ChatLine};
use crate::plugins::net::agent::AgentConnection;

/// The party the local player is in. Empty when there is no party.
#[derive(Resource, Default, Debug)]
pub struct PartyRoster {
    /// The server's id for this party. 0 when there is none.
    pub party_number: u32,
    /// The leader's JID, when the server has told us — `PartyData`'s header bit
    /// 0 carries it, and a roster-only push does not.
    pub master_join_id: Option<u32>,
    /// `SRParty.Setup` bitfield (EXP/item sharing, invite permission). Drives
    /// the party window's mode readout and the 4-vs-8 capacity.
    pub setup: PartySetup,
    /// Every member the server listed, in wire order. The local player is one
    /// of them — 0x3065 sends the full party, not "the others".
    pub members: Vec<PartyMemberCore>,
}

impl PartyRoster {
    /// Whether the local player is in a party at all.
    pub fn is_active(&self) -> bool {
        !self.members.is_empty()
    }

    /// How many members this party can hold — a *derived* property of the
    /// EXP-share bit, not a separate wire field.
    pub fn capacity(&self) -> u8 {
        self.setup.capacity()
    }

    /// Whether `member_id` leads this party. `false` when the leader is
    /// unknown, which is the honest answer: no crown is better than a wrong one.
    pub fn is_leader(&self, member_id: u32) -> bool {
        self.master_join_id == Some(member_id)
    }

    pub fn member(&self, member_id: u32) -> Option<&PartyMemberCore> {
        self.members.iter().find(|m| m.member_id == Some(member_id))
    }

    fn member_mut(&mut self, member_id: u32) -> Option<&mut PartyMemberCore> {
        self.members
            .iter_mut()
            .find(|m| m.member_id == Some(member_id))
    }

    /// Clear everything — the party is gone.
    fn dismiss(&mut self) {
        self.members.clear();
        self.setup = PartySetup::default();
        self.master_join_id = None;
        self.party_number = 0;
    }

    /// 0x3065 — the authoritative push. Each of its two halves is independently
    /// present: the header bit 0 names the party's attributes, bit 1 names the
    /// roster. A roster-only push must not be read as "sharing was turned off",
    /// and an info-only push must not empty the party.
    pub fn apply_data(&mut self, data: &PartyData) {
        self.party_number = data.party_number;
        if data.has_party_info() {
            self.setup = data.setup();
            self.master_join_id = data.master_join_id;
        }
        if data.has_roster() {
            self.members = data.members.clone();
        }
    }

    /// 0x3864 — delta. An unrecognised update type is a no-op by design: the
    /// packet model decodes it to "no payload" instead of failing, and the
    /// roster must not invent state for it.
    pub fn apply_update(&mut self, update: &PartyUpdate) {
        match update.update_type {
            // 1 — party dismissed.
            1 => self.dismiss(),
            // 2 — member joined. A re-join of a known id merges onto the record
            // we already have rather than replacing it, for the same reason
            // type 6 does: the record is only as complete as its mask.
            2 => {
                if let Some(joined) = update.joined.as_ref() {
                    self.upsert(joined);
                }
            }
            // 3 — member left or was kicked.
            3 => {
                if let Some(member_id) = update.member_id {
                    self.members.retain(|m| m.member_id != Some(member_id));
                }
            }
            // 6 — some member fields changed. The id is in the envelope, not in
            // the record (whose own 0x10 bit is usually clear here).
            6 => {
                if let (Some(member_id), Some(delta)) =
                    (update.member_id, update.member_update.as_ref())
                {
                    if let Some(member) = self.member_mut(member_id) {
                        merge_member(member, delta);
                    }
                }
            }
            // 9 (new master) has no known body, and nothing else is defined.
            _ => {}
        }
    }

    /// Merge a record onto the member it names, or append it as a new one.
    fn upsert(&mut self, record: &PartyMemberCore) {
        match record.member_id.and_then(|id| self.member_mut(id)) {
            Some(slot) => merge_member(slot, record),
            None => self.members.push(record.clone()),
        }
    }
}

/// Fold one presence-masked record onto a stored member.
///
/// Only fields the record's mask actually named are copied — a `None` here
/// means "this record did not mention it", never "it became empty". The
/// position is the one group that moves together, because one mask bit gates
/// the region, the coordinates and the trailing word, and the region's high bit
/// decides which of the two coordinate flavours arrived.
fn merge_member(member: &mut PartyMemberCore, delta: &PartyMemberCore) {
    member.presence |= delta.presence;
    if delta.member_id.is_some() {
        member.member_id = delta.member_id;
    }
    if delta.name.is_some() {
        member.name = delta.name.clone();
        member.model_id = delta.model_id;
    }
    if delta.level.is_some() {
        member.level = delta.level;
    }
    if delta.hp_mp.is_some() {
        member.hp_mp = delta.hp_mp;
    }
    if delta.region.is_some() {
        member.region = delta.region;
        member.position_dungeon = delta.position_dungeon.clone();
        member.position_world = delta.position_world.clone();
        member.position_tail = delta.position_tail;
    }
    if delta.guild_name.is_some() {
        member.guild_name = delta.guild_name.clone();
    }
    if delta.flag.is_some() {
        member.flag = delta.flag;
    }
    if delta.mastery_primary.is_some() {
        member.mastery_primary = delta.mastery_primary;
        member.mastery_secondary = delta.mastery_secondary;
    }
}

pub fn on_party_data(mut reader: MessageReader<PartyData>, mut roster: ResMut<PartyRoster>) {
    for data in reader.read() {
        roster.apply_data(data);
    }
}

pub fn on_party_update(mut reader: MessageReader<PartyUpdate>, mut roster: ResMut<PartyRoster>) {
    for update in reader.read() {
        roster.apply_update(update);
    }
}

// --------------------------------------------------------------------------
// Outbound: the four party verbs
// --------------------------------------------------------------------------
//
// Idea: everything above folds what the server pushes; nothing sent one. The
// original has *two* different "invite" opcodes and picking between them is the
// only decision this half makes: `0x7060` forms a party around the target and
// raises petition type 2 (`PartyCreation`) on them, `0x7062` adds the target to
// a party that already exists and raises type 3 (`PartyInvitation`)
// (`docs/re/systems/gameinvite-0x3080-assembly.md` §3 opcode table,
// `docs/re/notes/party-guild.md:40-41`). One message type therefore carries the
// player's intent ("ask this person into my party") and the sender resolves it
// against the roster, so no caller has to know the split.

/// A party verb the local player triggered. Written by the UI (the target
/// menu today, the party window next), consumed by [`send_party_actions`].
#[derive(Message, Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartyAction {
    /// Ask the player with this **spawn id** into the party. Becomes 0x7060 or
    /// 0x7062 depending on whether we are already in one.
    Invite(u32),
    /// Leave the party — 0x7061, empty body.
    Leave,
    /// Expel the member with this **join id** — 0x7063.
    Kick(u32),
}

/// The `SRParty.Setup` flags a freshly created party is opened with.
///
/// Deliberately the empty set rather than a guessed sharing policy. The
/// original picks these in `ifsetpartymode.txt` (two radio groups plus the
/// "can invite without master status" checkbox,
/// `docs/re/ui/hud-party-window.md` §3g/§3h) *before* the create request goes
/// out, and that dialog does not exist here yet (#32). Sending flags the player
/// never chose would invent a policy — and a visible one, since `EXP_SHARED`
/// alone changes the party's capacity from 4 to 8. This resource is the seam
/// the dialog writes to when it lands.
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct PartyCreateSetup(pub PartySetup);

/// Turn [`PartyAction`]s into wire packets.
pub fn send_party_actions(
    mut reader: MessageReader<PartyAction>,
    roster: Res<PartyRoster>,
    setup: Res<PartyCreateSetup>,
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
            warn!("party: no agent connection, dropping {action:?}");
        }
        return;
    };
    let in_party = roster.is_active();
    for action in pending {
        let packet = party_action_packet(*action, in_party, setup.0);
        info!("party: sending {action:?} (in_party={in_party})");
        if let Err(e) = conn.get_sender().send(packet.into()) {
            error!("network: failed to send party action: {}", e.0);
        }
    }
}

/// The verb-to-opcode rule, split out so the 0x7060/0x7062 fork is testable
/// without a live connection — it is the only decision on this path.
pub fn party_action_packet(action: PartyAction, in_party: bool, setup: PartySetup) -> Packet {
    match action {
        // Already in a party -> 0x7062 adds to it; otherwise 0x7060 forms one
        // around the target (and `setup` is only read on that branch).
        PartyAction::Invite(unique_id) if in_party => {
            Packet::from(PartyInviteRequest { unique_id })
        }
        PartyAction::Invite(unique_id) => Packet::from(PartyCreationRequest {
            unique_id,
            setup: setup.0,
        }),
        PartyAction::Leave => Packet::from(PartyLeave),
        PartyAction::Kick(join_id) => Packet::from(PartyKickRequest { join_id }),
    }
}

/// The error codes this family answers with, as values from the wiki's
/// create/join pages quoted in `docs/net-party.md` §5. Wording is ours: the
/// original's own strings are `textuisystem` keys we cannot bind to a code
/// without a capture (the client resolves them through its category-2 error box,
/// `FUN_00778190(2, code, …)`, `docs/re/net/inbound/party.md:538,578`).
pub fn party_error_text(code: u16) -> Option<&'static str> {
    match code {
        11276 => Some("The party request was declined."),
        11280 => Some("The invitation expired without an answer."),
        11288 => Some("That player is already in another party."),
        11292 => Some("That party no longer exists."),
        11301 => Some("That player has a party registered in party matching."),
        _ => None,
    }
}

/// One line of feedback for an ack, or `None` when it succeeded.
///
/// `0xB060` and `0xB062` share this shape: `result == 1` is success (0xB062
/// carries nothing at all on success, so silence is correct — the party itself
/// arrives as the separate 0x3065/0x3864 push), `result == 2` carries the code.
fn ack_feedback(verb: &str, result: u8, error_code: Option<u16>) -> Option<String> {
    if result == 1 {
        return None;
    }
    Some(match error_code.and_then(party_error_text) {
        Some(text) => text.to_string(),
        // An unmapped code is reported verbatim rather than swallowed: the
        // table above is wiki-derived, not captured, so it will have holes.
        None => format!(
            "Party {verb} failed (code {}).",
            error_code
                .map(|c| c.to_string())
                .unwrap_or_else(|| format!("result {result}"))
        ),
    })
}

/// Push one ack line into the chat log, when there is a chat log.
///
/// `ChatHistory` is a **HUD** resource and this is the **net** layer. The
/// headless netcheck harness builds the networking core with no HUD at all, and
/// Bevy 0.19 does not skip a system whose `ResMut` is missing — it fails
/// parameter validation and panics the schedule, which killed the harness
/// outright. So the dependency is optional by construction: without a HUD the
/// ack still reaches the log, it just has no window to print in.
fn report_ack(history: &mut Option<ResMut<ChatHistory>>, text: String) {
    match history {
        Some(history) => history.push(ChatLine::system(text)),
        None => info!("party (headless): {text}"),
    }
}

/// 0xB060 — ack for our own 0x7060. Success also carries a `u32` whose meaning
/// is [U] (`docs/re/net/inbound/party.md:538`), so it is logged, not modelled.
pub fn on_party_create_response(
    mut reader: MessageReader<PartyCreateResponse>,
    mut history: Option<ResMut<ChatHistory>>,
) {
    for msg in reader.read() {
        info!(
            "party: 0xB060 create ack result={} value={:?} error={:?}",
            msg.result, msg.leader_join_id, msg.error_code
        );
        if let Some(text) = ack_feedback("creation", msg.result, msg.error_code) {
            report_ack(&mut history, text);
        }
    }
}

/// 0xB062 — ack for our own 0x7062.
pub fn on_party_invite_response(
    mut reader: MessageReader<PartyInviteResponse>,
    mut history: Option<ResMut<ChatHistory>>,
) {
    for msg in reader.read() {
        info!(
            "party: 0xB062 invite ack result={} error={:?}",
            msg.result, msg.error_code
        );
        if let Some(text) = ack_feedback("invitation", msg.result, msg.error_code) {
            report_ack(&mut history, text);
        }
    }
}

/// Owns [`PartyRoster`]; part of the networking core so the roster exists
/// wherever party packets can arrive.
pub struct PartyPlugin;

impl Plugin for PartyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PartyRoster>()
            .init_resource::<PartyCreateSetup>()
            .add_message::<PartyAction>()
            .add_systems(
                Update,
                (
                    on_party_data,
                    on_party_update,
                    on_party_create_response,
                    on_party_invite_response,
                    send_party_actions,
                )
                    .chain(),
            );
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use packets::agent::party::{PartyHpMp, PartyMemberMask, PartyPositionWorld};

    /// A member record with every field present — what a full roster push sends.
    pub(crate) fn core(member_id: u32, name: &str, region: u16) -> PartyMemberCore {
        PartyMemberCore {
            presence: PartyMemberMask::ALL,
            member_id: Some(member_id),
            name: Some(name.to_string()),
            model_id: Some(1907),
            level: Some(20),
            // 0x5A = HP nibble 10 -> 100 %, MP nibble 5 -> 50 %.
            hp_mp: Some(0x5A),
            region: Some(region),
            position_dungeon: None,
            position_world: Some(PartyPositionWorld {
                x: 100,
                y: 0,
                z: 200,
            }),
            position_tail: Some(0),
            guild_name: Some(String::new()),
            flag: Some(0),
            mastery_primary: Some(0),
            mastery_secondary: Some(0),
        }
    }

    /// A record naming exactly one field, the shape a 0x3864 delta really has.
    fn delta(mask: u8, apply: impl FnOnce(&mut PartyMemberCore)) -> PartyMemberCore {
        let mut record = PartyMemberCore {
            presence: mask,
            ..Default::default()
        };
        apply(&mut record);
        record
    }

    pub(crate) fn party_data(members: Vec<PartyMemberCore>) -> PartyData {
        PartyData {
            presence: 0x03,
            party_number: 1,
            master_join_id: members.first().and_then(|m| m.member_id),
            setup: Some(PartySetup::EXP_SHARED),
            member_count: Some(members.len() as u8),
            members,
        }
    }

    #[test]
    fn full_roster_replaces_state() {
        let mut roster = PartyRoster::default();
        roster.apply_data(&party_data(vec![
            core(1, "Alice", 25000),
            core(2, "Bob", 25000),
        ]));
        assert_eq!(roster.members.len(), 2);
        assert!(roster.setup.is_exp_shared());
        assert_eq!(roster.capacity(), 8);
        // The first member is the leader in this fixture, and the crown follows
        // the wire field rather than wire order.
        assert!(roster.is_leader(1));
        assert!(!roster.is_leader(2));
        // A second 0x3065 carrying the roster bit is authoritative, not additive.
        roster.apply_data(&party_data(vec![core(3, "Carol", 25000)]));
        assert_eq!(roster.members.len(), 1);
        assert_eq!(roster.members[0].member_id, Some(3));
    }

    /// A push that carries only the party's attributes must not empty the
    /// party, and one that carries only the roster must not reset the sharing
    /// flags. Under the old fixed 9-byte header both halves were always assumed
    /// present.
    #[test]
    fn each_half_of_the_roster_push_is_independent() {
        let mut roster = PartyRoster::default();
        roster.apply_data(&party_data(vec![core(1, "Alice", 25000)]));

        // roster only — sharing and leader survive
        roster.apply_data(&PartyData {
            presence: 0x02,
            party_number: 1,
            master_join_id: None,
            setup: None,
            member_count: Some(2),
            members: vec![core(1, "Alice", 25000), core(2, "Bob", 25000)],
        });
        assert!(roster.setup.is_exp_shared());
        assert_eq!(roster.master_join_id, Some(1));
        assert_eq!(roster.members.len(), 2);

        // info only — the members survive
        roster.apply_data(&PartyData {
            presence: 0x01,
            party_number: 1,
            master_join_id: Some(2),
            setup: Some(PartySetup::ITEM_SHARED),
            member_count: None,
            members: Vec::new(),
        });
        assert_eq!(roster.members.len(), 2);
        assert!(roster.is_leader(2));
        assert_eq!(roster.capacity(), 4, "EXP sharing was switched off");
    }

    #[test]
    fn hp_mp_stays_the_raw_wire_byte() {
        let mut roster = PartyRoster::default();
        roster.apply_data(&party_data(vec![core(1, "Alice", 25000)]));
        assert_eq!(roster.members[0].hp_mp, Some(0x5A));
        assert_eq!(roster.members[0].hp_mp().unwrap().hp_percent(), 100);
        assert_eq!(roster.members[0].hp_mp().unwrap().mp_percent(), 50);
        assert_eq!(Some(PartyHpMp(0x5A)), roster.members[0].hp_mp());
    }

    #[test]
    fn deltas_join_leave_dismiss_and_update() {
        let mut roster = PartyRoster::default();
        roster.apply_data(&party_data(vec![core(1, "Alice", 25000)]));

        // type 2 — joined.
        roster.apply_update(&PartyUpdate {
            update_type: 2,
            joined: Some(core(2, "Bob", 25000)),
            member_id: None,
            member_update: None,
        });
        assert_eq!(roster.members.len(), 2);

        // type 6 — a level-only delta.
        roster.apply_update(&PartyUpdate {
            update_type: 6,
            joined: None,
            member_id: Some(2),
            member_update: Some(delta(PartyMemberMask::LEVEL, |r| r.level = Some(42))),
        });
        assert_eq!(roster.member(2).unwrap().level, Some(42));

        // type 3 — left/kicked.
        roster.apply_update(&PartyUpdate {
            update_type: 3,
            joined: None,
            member_id: Some(2),
            member_update: None,
        });
        assert!(roster.member(2).is_none());

        // An unknown update type must not touch the roster.
        roster.apply_update(&PartyUpdate {
            update_type: 9,
            joined: None,
            member_id: None,
            member_update: None,
        });
        assert_eq!(roster.members.len(), 1);

        // type 1 — dismissed.
        roster.apply_update(&PartyUpdate {
            update_type: 1,
            joined: None,
            member_id: None,
            member_update: None,
        });
        assert!(roster.members.is_empty());
        assert!(!roster.is_active());
        assert_eq!(roster.master_join_id, None);
    }

    /// The regression the presence-mask rewrite exists to prevent: a delta names
    /// one field, so folding it must not blank the rest of the member. Replacing
    /// the stored record — which is what the old code did for type 2, and what
    /// the obvious implementation of type 6 would do — would drop the name, the
    /// guild and the position on every hp/mp tick.
    #[test]
    fn a_partial_delta_merges_instead_of_replacing() {
        let mut roster = PartyRoster::default();
        roster.apply_data(&party_data(vec![core(1, "Alice", 25000)]));

        roster.apply_update(&PartyUpdate {
            update_type: 6,
            joined: None,
            member_id: Some(1),
            member_update: Some(delta(PartyMemberMask::HP_MP, |r| r.hp_mp = Some(0x12))),
        });

        let member = roster.member(1).unwrap();
        assert_eq!(member.hp_mp, Some(0x12), "the delta applied");
        assert_eq!(member.name.as_deref(), Some("Alice"), "the name survived");
        assert_eq!(member.level, Some(20), "the level survived");
        assert!(member.position_world.is_some(), "the position survived");
        assert_eq!(member.model_id, Some(1907), "the model survived");
    }

    /// The position group moves together: one mask bit gates the region, the
    /// coordinates and the trailing word, and a member who walked into a dungeon
    /// must not keep a stale overworld position beside the new dungeon one.
    #[test]
    fn a_position_delta_swaps_the_coordinate_flavour() {
        let mut roster = PartyRoster::default();
        roster.apply_data(&party_data(vec![core(1, "Alice", 25000)]));
        assert!(roster.member(1).unwrap().position_world.is_some());

        roster.apply_update(&PartyUpdate {
            update_type: 6,
            joined: None,
            member_id: Some(1),
            member_update: Some(delta(PartyMemberMask::POSITION, |r| {
                r.region = Some(0x8001);
                r.position_dungeon =
                    Some(packets::agent::party::PartyPositionDungeon { x: 1, y: 2, z: 3 });
            })),
        });

        let member = roster.member(1).unwrap();
        assert_eq!(member.region, Some(0x8001));
        assert!(member.position_dungeon.is_some());
        assert!(
            member.position_world.is_none(),
            "the overworld position must not linger"
        );
    }

    /// The file's one real decision: the same player-facing verb is two
    /// different opcodes, picked by whether a party already exists. Getting
    /// this backwards is silently wrong on the wire (the server answers the
    /// other ack and the invitee sees the other petition type).
    #[test]
    fn invite_picks_create_or_invite_by_roster_state() {
        let uid = 0x0001_60AA;
        // no party yet -> 0x7060 with the setup byte appended
        let (opcode, body) =
            party_action_packet(PartyAction::Invite(uid), false, PartySetup(0)).into_serialize();
        assert_eq!(opcode, 0x7060);
        assert_eq!(&body[..], &[0xAA, 0x60, 0x01, 0x00, 0x00]);

        // in a party -> 0x7062, a bare uid and no setup byte
        let (opcode, body) =
            party_action_packet(PartyAction::Invite(uid), true, PartySetup(0)).into_serialize();
        assert_eq!(opcode, 0x7062);
        assert_eq!(&body[..], &[0xAA, 0x60, 0x01, 0x00]);

        // the setup byte only exists on the create branch
        let (_, body) = party_action_packet(
            PartyAction::Invite(uid),
            false,
            PartySetup(PartySetup::EXP_SHARED | PartySetup::ITEM_SHARED),
        )
        .into_serialize();
        assert_eq!(body[4], 0x03);
    }

    #[test]
    fn leave_is_empty_and_kick_carries_the_join_id() {
        let (opcode, body) =
            party_action_packet(PartyAction::Leave, true, PartySetup(0)).into_serialize();
        assert_eq!(opcode, 0x7061);
        assert!(body.is_empty());

        let (opcode, body) =
            party_action_packet(PartyAction::Kick(0x0201), true, PartySetup(0)).into_serialize();
        assert_eq!(opcode, 0x7063);
        assert_eq!(&body[..], &[0x01, 0x02, 0x00, 0x00]);
    }

    /// A successful ack says nothing (the party arrives as its own push); a
    /// failure always says *something*, even for a code the table misses.
    #[test]
    fn acks_are_silent_on_success_and_never_swallow_a_failure() {
        assert_eq!(ack_feedback("invitation", 1, None), None);
        assert_eq!(
            ack_feedback("invitation", 2, Some(11288)).as_deref(),
            Some("That player is already in another party.")
        );
        // unmapped code -> reported verbatim rather than dropped
        assert_eq!(
            ack_feedback("creation", 2, Some(9999)).as_deref(),
            Some("Party creation failed (code 9999).")
        );
        // a failure result with no code at all still surfaces
        assert!(ack_feedback("creation", 2, None).is_some());
    }

    /// The regression the headless netcheck harness caught: these two systems
    /// live in the **net** layer but reported through `ChatHistory`, a **HUD**
    /// resource. The harness builds networking with no HUD, and Bevy 0.19 does
    /// not skip a system whose `ResMut` is missing — it fails parameter
    /// validation and panics the schedule, so the harness died before login.
    ///
    /// This builds exactly that shape: the ack systems, no HUD resource, a real
    /// ack delivered. It must survive the update.
    #[test]
    fn the_ack_systems_run_without_a_hud() {
        let mut app = App::new();
        app.add_message::<PartyCreateResponse>()
            .add_message::<PartyInviteResponse>()
            .add_systems(Update, (on_party_create_response, on_party_invite_response));
        assert!(
            !app.world().contains_resource::<ChatHistory>(),
            "this test is only meaningful without the HUD resource"
        );

        // a failure arm, so the reporting path is actually taken
        app.world_mut().write_message(PartyCreateResponse {
            result: 2,
            leader_join_id: None,
            error_code: Some(11288),
        });
        app.world_mut().write_message(PartyInviteResponse {
            result: 2,
            error_code: Some(11276),
        });
        app.update();
    }

    /// ...and with a HUD present the same acks really do reach the chat log,
    /// so making the dependency optional did not quietly drop the feedback.
    #[test]
    fn the_ack_systems_still_write_to_the_chat_log_when_there_is_one() {
        let mut app = App::new();
        app.init_resource::<ChatHistory>()
            .add_message::<PartyInviteResponse>()
            .add_systems(Update, on_party_invite_response);
        app.world_mut().write_message(PartyInviteResponse {
            result: 2,
            error_code: Some(11288),
        });
        app.update();
        let history = app.world().resource::<ChatHistory>();
        assert_eq!(history.iter().count(), 1);

        // and a success stays silent — the party arrives as its own push
        app.world_mut().write_message(PartyInviteResponse {
            result: 1,
            error_code: None,
        });
        app.update();
        assert_eq!(app.world().resource::<ChatHistory>().iter().count(), 1);
    }

    /// The codes are the wiki values quoted in `docs/net-party.md` §5 — pinned
    /// so a later edit of the prose cannot silently re-map them.
    #[test]
    fn the_error_table_is_the_documented_five() {
        for code in [11276, 11280, 11288, 11292, 11301] {
            assert!(party_error_text(code).is_some(), "{code} lost its text");
        }
        assert_eq!(party_error_text(0), None);
    }
}
