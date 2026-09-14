//! Party wire opcodes: party data/update (0x3065, 0x3864), the create/leave/kick
//! requests (0x7060/0x7061/0x7063) and the party-match family (0x706x / 0xB06x).
//!
//! **Spec-derived, not capture-verified.** No `packet_dump/` sample exists for
//! any opcode in this family, so every layout here comes from statically reading
//! the original client's parser/builder (xBot `PacketParser.cs`/`PacketBuilder.cs`)
//! cross-checked against go-sro's handlers. Byte-level notes, per-field [V]/[S]/[U]
//! tags and the resolving capture for each unknown live in
//! `docs/net-party-0x3065.md`. Three opcodes (0x706A, 0xB069, 0xB06A) have **no**
//! original-client code at all and are go-sro-shaped only — flagged per struct.
//!
//! # The presence-mask correction
//!
//! The one place that is **not** spec-derived is the record framing, and it is
//! the load-bearing one. `docs/re/net/inbound/party.md` decompiled the original's
//! own handlers — `FUN_00883cc0` (0x3065), `FUN_00886b10` (0x3864),
//! `FUN_00884660` (0x706D) and the shared member reader `FUN_00883620` — and they
//! agree on a shape neither xBot nor go-sro states outright: **both the roster
//! header and every member record begin with a presence bitmask, and only the
//! fields whose bit is set are on the wire.**
//!
//! A record with *all* bits set is byte-for-byte what the older fixed-layout
//! model read, which is exactly why the bug hid: the full record is the common
//! case, so the fixed reading survived every desk check. A partial record —
//! which is the normal shape of a 0x3864 delta — mis-slices from the first
//! absent field onward and desynchronises the rest of the packet.
//!
//! Three consequences worth stating because they contradict older notes:
//!
//! - the "opaque 9 bytes" of [`PartyData`] are resolved, and with them the
//!   **party leader** (`master_join_id`), which the client-side roster used to
//!   declare UNKNOWN;
//! - `unk_byte07` (a byte xBot reads in the 0x3864 flavour of the record) **does
//!   not exist** — the binary has no such read, so the two record flavours
//!   collapse into the single [`PartyMemberCore`];
//! - [`PartyMemberUpdate`]'s `kind` is a **bitmask**, not an enum, so combined
//!   masks like `0x24` (level *and* hp/mp) are ordinary traffic.
//!
//! The invite path (0x7062 / 0xB060 / 0x3080) is deliberately not here; see
//! `docs/net-invite-0x3080.md`.
//!
//! ⚠️ Free-text caveat: the derive decodes `String` with `from_utf8`, which
//! **fails the whole packet** on an invalid byte, whereas the original reads
//! cp1252 and never fails. `title`, `name` and `guild_name` here are all
//! user-authored, so a non-UTF-8 byte drops the event. That is a pre-existing
//! tree-wide property of the derived string path (the hand-written paths use a
//! lossy reader for exactly this reason — see `character_data::read_string`),
//! not something this module introduces; it is called out because party titles
//! are a likelier place to meet it than most.

use std::io::Cursor;

use bevy::prelude::Message;
use bytes::{BufMut, Bytes, BytesMut};

use crate::agent::character_data::{ItemClass, ItemClassResolver};

use sro_macro::ByteSize;
use sro_macro::Deserialize;
use sro_macro::SerializationError;
use sro_macro::Serialize;
use sro_macro_derive::*;

/// `SRParty.Purpose`: what the party advertises itself for.
pub const PARTY_PURPOSE_HUNTING: u8 = 0;
pub const PARTY_PURPOSE_QUEST: u8 = 1;
pub const PARTY_PURPOSE_TRADER: u8 = 2;
pub const PARTY_PURPOSE_THIEF: u8 = 3;

/// `SRParty.Setup` — a `[Flags]` bitfield, not an enum, so it is modelled as a
/// newtype rather than a derived enum (an unknown value must not fail the
/// packet). Corroborated by go-sro `model/party_setting.go`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PartySetup(pub u8);

impl PartySetup {
    pub const EXP_SHARED: u8 = 0x01;
    pub const ITEM_SHARED: u8 = 0x02;
    pub const ANYONE_CAN_INVITE: u8 = 0x04;

    pub fn is_exp_shared(&self) -> bool {
        self.0 & Self::EXP_SHARED != 0
    }

    pub fn is_item_shared(&self) -> bool {
        self.0 & Self::ITEM_SHARED != 0
    }

    pub fn anyone_can_invite(&self) -> bool {
        self.0 & Self::ANYONE_CAN_INVITE != 0
    }

    /// Max members: sharing EXP raises the cap from 4 to 8 (`SRParty.cs:12`).
    pub fn capacity(&self) -> u8 {
        if self.is_exp_shared() {
            8
        } else {
            4
        }
    }
}

/// One byte packing both bars in 10% steps (`SRPartyMember.cs:14-15`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PartyHpMp(pub u8);

impl PartyHpMp {
    pub fn hp_percent(&self) -> u8 {
        (self.0 & 0x0F) * 10
    }

    pub fn mp_percent(&self) -> u8 {
        (self.0 >> 4) * 10
    }
}

/// Member position in a dungeon region: full 32-bit coordinates.
#[derive(Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct PartyPositionDungeon {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

/// Member position in an overworld region: 16-bit region-local coordinates.
#[derive(Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct PartyPositionWorld {
    pub x: u16,
    pub y: u16,
    pub z: u16,
}

/// The presence bits of a member record (`FUN_00883620`), and of the field
/// groups a 0x3864 type-6 update selects with the *same* numbering.
///
/// Modelled as a newtype rather than a derived enum for the same reason
/// [`PartySetup`] is: bits combine, and an unknown bit must not fail the packet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PartyMemberMask(pub u8);

impl PartyMemberMask {
    /// `name` *and* `model_id` — one bit gates both reads.
    pub const NAME: u8 = 0x01;
    pub const LEVEL: u8 = 0x02;
    pub const HP_MP: u8 = 0x04;
    /// Both mastery ids.
    pub const MASTERIES: u8 = 0x08;
    pub const MEMBER_ID: u8 = 0x10;
    /// `region`, the coordinates *and* the trailing `u32` — one bit gates all of
    /// it, which is why `position_tail` is not its own concept.
    pub const POSITION: u8 = 0x20;
    pub const GUILD: u8 = 0x40;
    pub const FLAG: u8 = 0x80;
    /// Every field present — the shape a full roster push uses, and the one the
    /// pre-mask fixed-layout model happened to read correctly.
    pub const ALL: u8 = 0xFF;

    pub fn has(&self, bit: u8) -> bool {
        self.0 & bit != 0
    }
}

/// One party-member record, as read by the original's shared `FUN_00883620` —
/// the parser behind 0x3065's roster, 0x3864's type-2/type-6 deltas *and*
/// 0x706D's applicant block.
///
/// Every field is optional because every field is gated by `presence`; a delta
/// carries only what changed. Consumers fold these onto a stored member rather
/// than replacing it (see the client's `PartyRoster::apply_update`).
///
/// The position is region-gated exactly as elsewhere in the protocol: the high
/// bit of `region` marks a dungeon, which widens the coordinates from u16 to i32.
#[derive(Serialize, Deserialize, ByteSize, Clone, Debug, Default, PartialEq)]
pub struct PartyMemberCore {
    /// See [`PartyMemberMask`]. Read first; every field below hangs off it.
    pub presence: u8,
    /// Account/character id (JID).
    #[sro_packet(when = "presence & 0x10 != 0")]
    pub member_id: Option<u32>,
    #[sro_packet(when = "presence & 0x01 != 0")]
    pub name: Option<String>,
    #[sro_packet(when = "presence & 0x01 != 0")]
    pub model_id: Option<u32>,
    #[sro_packet(when = "presence & 0x02 != 0")]
    pub level: Option<u8>,
    /// See [`PartyHpMp`] for the nibble packing.
    #[sro_packet(when = "presence & 0x04 != 0")]
    pub hp_mp: Option<u8>,
    #[sro_packet(when = "presence & 0x20 != 0")]
    pub region: Option<u16>,
    #[sro_packet(when = "presence & 0x20 != 0 && region.is_some_and(|r| r & 0x8000 != 0)")]
    pub position_dungeon: Option<PartyPositionDungeon>,
    #[sro_packet(when = "presence & 0x20 != 0 && region.is_some_and(|r| r & 0x8000 == 0)")]
    pub position_world: Option<PartyPositionWorld>,
    /// The `u32` at record+0x54, read under the *same* bit as the position. Its
    /// meaning has no reachable name in the binary — UNKNOWN, kept because
    /// dropping it would desynchronise everything after it.
    #[sro_packet(when = "presence & 0x20 != 0")]
    pub position_tail: Option<u32>,
    #[sro_packet(when = "presence & 0x40 != 0")]
    pub guild_name: Option<String>,
    /// The `u8` at record+0x41 — also UNKNOWN, also load-bearing for framing.
    #[sro_packet(when = "presence & 0x80 != 0")]
    pub flag: Option<u8>,
    #[sro_packet(when = "presence & 0x08 != 0")]
    pub mastery_primary: Option<u32>,
    #[sro_packet(when = "presence & 0x08 != 0")]
    pub mastery_secondary: Option<u32>,
}

impl PartyMemberCore {
    pub fn mask(&self) -> PartyMemberMask {
        PartyMemberMask(self.presence)
    }

    pub fn hp_mp(&self) -> Option<PartyHpMp> {
        self.hp_mp.map(PartyHpMp)
    }
}

/// 0x3065 — server → client full party roster.
///
/// The header used to be nine opaque bytes here, because xBot reads it
/// `[u32][u32][u8 purpose]` while go-sro writes `[u8 0xFF][u32 party_number]
/// [u32 master_jid]` and naming it either way would have encoded a guess. The
/// original's own handler `FUN_00883cc0` settles it in go-sro's favour, with
/// the twist that the leading byte is a **presence flag** rather than the
/// constant `0xFF` go-sro happens to send: bit 0 gates the leader id and the
/// setup byte, bit 1 gates the count and the roster. go-sro's `0xFF` sets both,
/// which is why the byte counts coincided and xBot's misalignment never showed
/// (its "purpose" is really the top byte of `master_join_id`, hence always 0).
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct PartyData {
    /// Bit 0 — party info follows. Bit 1 — the roster follows.
    pub presence: u8,
    pub party_number: u32,
    /// The leader's JID. The original compares it against the local player's own
    /// JID to decide whether to show the "I am leader" state.
    #[sro_packet(when = "presence & 0x01 != 0")]
    pub master_join_id: Option<u32>,
    /// See [`PartySetup`].
    #[sro_packet(when = "presence & 0x01 != 0")]
    pub setup: Option<u8>,
    #[sro_packet(when = "presence & 0x02 != 0")]
    pub member_count: Option<u8>,
    #[sro_packet(list_type = "by-size-field", size_field = "member_count.unwrap_or(0)")]
    pub members: Vec<PartyMemberCore>,
}

impl PartyData {
    /// The sharing flags, or the empty set when this push carries no party info
    /// (`presence` bit 0 clear) — a roster-only update must not be read as
    /// "sharing turned off".
    pub fn setup(&self) -> PartySetup {
        PartySetup(self.setup.unwrap_or(0))
    }

    /// Whether bit 0 named the party's own attributes.
    pub fn has_party_info(&self) -> bool {
        self.presence & 0x01 != 0
    }

    /// Whether bit 1 named the roster. Distinguishes "the party has no members"
    /// (impossible) from "this push did not carry the roster" (routine).
    pub fn has_roster(&self) -> bool {
        self.presence & 0x02 != 0
    }
}

/// 0x3864 — server → client party delta, discriminated by a leading update type.
///
/// Modelled with `when`-conditional fields rather than a derived enum so an
/// unrecognised update type decodes to "no payload" instead of failing the
/// packet — go-sro documents types 1/2/3/6/9 but xBot only handles four of them,
/// and 9 (new master) has no known body at all.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct PartyUpdate {
    /// 1 dismissed · 2 joined · 3 left/kicked · 6 member update · 9 new master.
    pub update_type: u8,
    #[sro_packet(when = "update_type == 2")]
    pub joined: Option<PartyMemberCore>,
    #[sro_packet(when = "update_type == 3 || update_type == 6")]
    pub member_id: Option<u32>,
    /// Type 6's payload is the **same** mask record as everything else in this
    /// family — the original calls `FUN_00883620` here too. It used to be its
    /// own `PartyMemberUpdate` type testing `kind` for equality, which read
    /// nothing at all for a combined mask such as `0x24` (level *and* hp/mp).
    #[sro_packet(when = "update_type == 6")]
    pub member_update: Option<PartyMemberCore>,
}

/// 0x306E — client → server answer to an incoming 0x706D join request. Sits in
/// the 0x3xxx range but is genuinely client → server.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct PartyMatchJoinResponse {
    pub request_id: u32,
    pub join_id: u32,
    /// 1 accept, 0 decline.
    pub accept: u8,
}

/// 0x7060 — client → server: start a party. Whether `unique_id` is the invitee
/// or the sender is [U] (go-sro's handler is an empty stub).
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct PartyCreationRequest {
    pub unique_id: u32,
    /// See [`PartySetup`].
    pub setup: u8,
}

/// 0x7061 — client → server: leave the party. Empty body.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct PartyLeave;

/// 0x7063 — client → server: kick a member by JID.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct PartyKickRequest {
    pub join_id: u32,
}

/// 0x7069 — client → server: advertise a party in the match list. The second
/// u32 is written 0 by both the original client and go-sro; its purpose is [U].
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct PartyMatchCreationRequest {
    /// 0 when creating.
    pub party_number: u32,
    pub unknown: u32,
    pub setup: u8,
    pub purpose: u8,
    pub level_min: u8,
    pub level_max: u8,
    pub title: String,
}

/// 0x706A — client → server: edit an existing match entry.
///
/// **SPEC, unverified**: the original client has an enum entry but no builder,
/// so this shape comes from go-sro's `matching_update_handler` alone. It is the
/// 0x7069 body with a real party number. Resolve with `packet_dump/0x706A.log`.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct PartyMatchEditedRequest {
    pub party_number: u32,
    pub unknown: u32,
    pub setup: u8,
    pub purpose: u8,
    pub level_min: u8,
    pub level_max: u8,
    pub title: String,
}

/// 0x706B — client → server: withdraw a match entry.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct PartyMatchDeleteRequest {
    pub number: u32,
}

/// 0x706C — client → server: request one page of the match list.
///
/// The original client reuses this number for `CLIENT_PET_DESTROY`; both are
/// C→S and only the match-list sender is realised in its builder, so we take the
/// party-match meaning. [U] if a pet-destroy capture ever contradicts it.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct PartyMatchListRequest {
    pub page_index: u8,
}

/// 0x706D — server → client: somebody asks to join our advertised party.
///
/// Named `…Notify` because the *request* in the same opcode is the C→S half,
/// which is not registered (see below).
///
/// This opcode is **bidirectional** with two different bodies: C→S is a bare
/// `u32 number`, S→C is this record. `packets!` maps one type per opcode, so
/// only the inbound direction is registered — an unparsed inbound packet is the
/// failure that actually costs us something, whereas the outbound request is not
/// sent by anything yet.
///
/// The trailing block used to be modelled flat as
/// `unk_byte01, join_id_repeated, name`, which is right only when the applicant's
/// mask happens to be `0x11` and drops everything after the name otherwise. The
/// original's handler `FUN_00884660` reads five `u32`s, one `u8`, and then hands
/// the rest to `FUN_00883620` — the same member-record parser 0x3065 and 0x3864
/// use. So the applicant arrives as an ordinary [`PartyMemberCore`], which is
/// also where the dialog's level / guild fields come from when the mask names
/// them.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct PartyMatchJoinNotify {
    pub request_id: u32,
    pub join_id: u32,
    pub match_number: u32,
    pub mastery_primary: u32,
    pub mastery_secondary: u32,
    pub unk_byte00: u8,
    /// The applicant, as a presence-masked member record.
    pub applicant: PartyMemberCore,
}

/// The advertised entry echoed back by 0xB069 and 0xB06A on success.
///
/// One type for both because the original delegates both handlers to the same
/// reader, `FUN_00883780`, whose seven fields match go-sro's record exactly —
/// which is what promoted these two structs from [S] to [V].
#[derive(Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct PartyMatchForm {
    pub party_number: u32,
    pub unknown: u32,
    pub setup: u8,
    pub purpose: u8,
    pub level_min: u8,
    pub level_max: u8,
    pub title: String,
}

/// 0xB069 — server → client: result of advertising a party.
///
/// `result` is the discriminator, not a spare byte: `1` means the record
/// follows, `2` a `u16` error code. Reading the record unconditionally made a
/// real failure (a 3-byte body) run the derive off the end, so the packet was
/// dropped and the error never surfaced.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct PartyMatchCreationResponse {
    pub result: u8,
    #[sro_packet(when = "result == 1")]
    pub entry: Option<PartyMatchForm>,
    /// Branch on `!= 1`, not `== 2`: an unexpected result must read as an error
    /// rather than as a truncated record.
    #[sro_packet(when = "result != 1")]
    pub error_code: Option<u16>,
}

/// 0xB06A — server → client: result of editing a match entry. Same shape and
/// same reader as [`PartyMatchCreationResponse`]; only the completion message
/// the original shows differs (modify vs record).
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct PartyMatchEditedResponse {
    pub result: u8,
    #[sro_packet(when = "result == 1")]
    pub entry: Option<PartyMatchForm>,
    #[sro_packet(when = "result != 1")]
    pub error_code: Option<u16>,
}

/// 0xB06B — server → client: the withdrawn entry's number, or why it failed.
///
/// `result` was a `bool` here, which deserializes as `read_u8() == 1` — so a
/// real failure (`02 <u16 code>`) decoded as a bland "not successful" and threw
/// the code away.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct PartyMatchDeleteResponse {
    pub result: u8,
    #[sro_packet(when = "result == 1")]
    pub number: Option<u32>,
    #[sro_packet(when = "result != 1")]
    pub error_code: Option<u16>,
}

/// One advertised party in the 0xB06C list. `race_type` is byte-exact against
/// go-sro's `countryType` but its semantics (China/Europe?) are [U].
#[derive(Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct PartyMatchEntry {
    pub number: u32,
    /// When the entry was advertised — a **unix timestamp**, seconds, [V].
    ///
    /// go-sro calls this field `masterJID` and openroad inherited the name, but
    /// a capture of a real vSRO 1.188 server refutes it. Two registrations in
    /// one session: `packet_dump/c2s/0x7069.log` sent at 12:32:59Z and
    /// 12:34:09Z, and the listings that came back
    /// (`packet_dump/0xb06c.log`) carry `0x6A8C39FB` and `0x6A8C3A41` — which
    /// decode as exactly those two instants, and differ by exactly the 70
    /// seconds between them. A join id would not.
    ///
    /// This matters beyond the name: it is the field one reaches for to decide
    /// which listing is your own party, and it can never answer that.
    pub registered_at: u32,
    pub master_name: String,
    pub race_type: u8,
    pub member_count: u8,
    pub setup: u8,
    pub purpose: u8,
    pub level_min: u8,
    pub level_max: u8,
    pub title: String,
}

/// The populated half of a 0xB06C response. Nested rather than flattened
/// because the whole block hangs off one `has_data` flag — the original's parser
/// opens a single `if` around all of it — and because a count that drives a
/// sized list cannot itself be conditional.
#[derive(Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct PartyMatchListPage {
    pub page_count: u8,
    pub page_index: u8,
    pub party_count: u8,
    #[sro_packet(list_type = "by-size-field", size_field = "party_count")]
    pub parties: Vec<PartyMatchEntry>,
}

/// 0xB06C — server → client: one page of the party-match list. Everything after
/// `has_data` is absent when there is nothing to list.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct PartyMatchListResponse {
    pub has_data: bool,
    #[sro_packet(when = "has_data")]
    pub page: Option<PartyMatchListPage>,
}

// --- The family remainder: the four acks/pushes the seed never carried -----
//
// Idea: openroad's party family was seeded from xBot's `Agent.cs` enum, which
// declares these four but dispatches none of them, so they arrived as opcode
// numbers with no bodies (#760, #451). Their layouts are not invented here:
// SilkroadDoc-wiki documents `0xB060`, `0xB062`, `0xB06D` and `0x3068` field by
// field, and go-sro's opcode table names three of them the same way
// (`network/opcode/party.go:7,20` — `PartyCreateResponse`,
// `PartyMemberCountResponse`). Only field *layouts* are taken from the wiki —
// it carries no licence, so it is read for facts and never for code or text
// (AGENTS.md, licence table).
//
// **Attribution correction, stated because it contradicts a merged doc.**
// `docs/net-invite-0x3080.md:100,146` files `0xB060` as the *party-invite* ack
// with an UNKNOWN body. It is the **create** ack: the wiki pairs `0x7060` with
// `0xB060` (`AGENT_PARTY_CREATE`) and `0x7062` with `0xB062`
// (`AGENT_PARTY_INVITE`), and go-sro's table agrees. The invite ack that doc
// wanted is `0xB062`, modelled below. The doc lives under `docs/` and this lane
// does not rewrite RE units (runbook §5), so the correction is written here and
// on the issue.

/// 0xB060 — ack for `0x7060` create.
///
/// `result` is a raw byte with two different tails, not a bool: success carries
/// the leader's **JID**, failure a `u16` error code (11276 request denied,
/// 11280 no response, 11288 already in a party, 11301 registered in party
/// matching). Modelling it as a success bool would truncate every real error,
/// the same trap as `StallCreateResponse`.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct PartyCreateResponse {
    pub result: u8,
    #[sro_packet(when = "result == 1")]
    pub leader_join_id: Option<u32>,
    #[sro_packet(when = "result == 2")]
    pub error_code: Option<u16>,
}

/// 0xB062 — ack for `0x7062` invite. Success carries nothing: the invitation
/// itself travels as the separate `0x3080` popup
/// (`docs/net-invite-0x3080.md`), so this ack only says whether the request was
/// accepted for delivery.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct PartyInviteResponse {
    pub result: u8,
    #[sro_packet(when = "result == 2")]
    pub error_code: Option<u16>,
}

/// 0xB06D — ack for `0x706D` party-match join.
///
/// Note the two tails are **both** `u16` and mean different things: on success
/// it is a `PartyMatchingJoinResult` (the same enum `PartyMatchJoinResponse`
/// answers), on failure an error code (11292 = no such party). The failure test
/// is `!= 1`, not `== 2` — that is how the source branches it, and the two
/// differ for every other result value.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct PartyMatchJoinAck {
    pub result: u8,
    #[sro_packet(when = "result == 1")]
    pub join_result: Option<u16>,
    #[sro_packet(when = "result != 1")]
    pub error_code: Option<u16>,
}

/// 0x3068 — an item dropped by the party was distributed to a member.
///
/// Hand-written for the same reason as the stall rows: the tail's *width*
/// depends on the item's class in the client's own itemdata
/// (`TID2 == 1` → one `opt_level` byte, `TID2 == 2` → nothing at all,
/// `TID2 == 3` → a `u16` quantity), and `Deserialize` cannot take an
/// [`ItemClassResolver`]. The tail is therefore kept raw behind a
/// resolver-taking accessor, exactly like `StallEntityAction::rows`.
#[derive(Message, Clone, Debug, PartialEq)]
pub struct PartyDistribution {
    /// The member who received it.
    pub join_id: u32,
    pub ref_item_id: u32,
    /// The class-dependent tail, undecoded — read with [`Self::detail`].
    pub raw_tail: Bytes,
}

/// The class-dependent tail of a 0x3068.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartyDistributionDetail {
    /// `TID2 == 1` (equipment): the item's plus level.
    OptLevel(u8),
    /// `TID2 == 2` (containers/COS): the original triggers no message and
    /// writes no tail.
    None,
    /// `TID2 == 3` (expendables): how many.
    Quantity(u16),
}

impl PartyDistribution {
    /// Decode the tail against the client's itemdata. `None` when the class is
    /// unknown (the tail's width is then unknowable, and guessing it is what
    /// forged a fake record in `InventoryItem` once already) or the tail is
    /// short.
    pub fn detail(&self, resolver: &impl ItemClassResolver) -> Option<PartyDistributionDetail> {
        match resolver.item_class(self.ref_item_id) {
            ItemClass::Equipment => self
                .raw_tail
                .first()
                .copied()
                .map(PartyDistributionDetail::OptLevel),
            ItemClass::Container { .. } => Some(PartyDistributionDetail::None),
            ItemClass::Expendable { .. } => {
                let bytes: [u8; 2] = self.raw_tail.get(..2)?.try_into().ok()?;
                Some(PartyDistributionDetail::Quantity(u16::from_le_bytes(bytes)))
            }
            ItemClass::Unknown => None,
        }
    }
}

impl TryFrom<Bytes> for PartyDistribution {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, SerializationError> {
        let mut cursor = Cursor::new(&value[..]);
        let join_id = u32::read_from(&mut cursor)?;
        let ref_item_id = u32::read_from(&mut cursor)?;
        let read = cursor.position() as usize;
        Ok(PartyDistribution {
            join_id,
            ref_item_id,
            raw_tail: value.slice(read..),
        })
    }
}

impl From<PartyDistribution> for Bytes {
    fn from(p: PartyDistribution) -> Self {
        let mut buf = BytesMut::new();
        buf.put_u32_le(p.join_id);
        buf.put_u32_le(p.ref_item_id);
        buf.extend_from_slice(&p.raw_tail);
        buf.freeze()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    /// A member record with every presence bit set, written in the original's
    /// read order (`FUN_00883620`). This is the shape the pre-mask fixed-layout
    /// model used to read, so it doubles as the compatibility anchor.
    fn full_member_bytes() -> Vec<u8> {
        let mut b: Vec<u8> = vec![PartyMemberMask::ALL];
        b.extend(0x1234u32.to_le_bytes()); // 0x10 member_id
        b.extend(2u16.to_le_bytes()); // 0x01 name
        b.extend(b"Ax");
        b.extend(1907u32.to_le_bytes()); // 0x01 model_id
        b.push(40); // 0x02 level
        b.push(0x3A); // 0x04 hp 100 %, mp 30 %
        b.extend(0x0100u16.to_le_bytes()); // 0x20 region, overworld
        b.extend(11u16.to_le_bytes());
        b.extend(22u16.to_le_bytes());
        b.extend(33u16.to_le_bytes());
        b.extend(0xDEADBEEFu32.to_le_bytes()); // 0x20 position tail
        b.extend(3u16.to_le_bytes()); // 0x40 guild name
        b.extend(b"Guo");
        b.push(6); // 0x80 flag
        b.extend(100u32.to_le_bytes()); // 0x08 masteries
        b.extend(200u32.to_le_bytes());
        b
    }

    /// The compatibility anchor: a full-mask record decodes every field and
    /// re-serialises to the identical bytes. If the mask rewrite had changed the
    /// *order* of any read, this is what would catch it — the full record is the
    /// only shape both the old and the new model agree on.
    #[test]
    fn a_full_mask_record_round_trips_every_field() {
        let wire = Bytes::from(full_member_bytes());

        let decoded = PartyMemberCore::try_from(wire.clone()).unwrap();

        assert_eq!(decoded.member_id, Some(0x1234));
        assert_eq!(decoded.name.as_deref(), Some("Ax"));
        assert_eq!(decoded.model_id, Some(1907));
        assert_eq!(decoded.level, Some(40));
        assert_eq!(decoded.hp_mp().unwrap().hp_percent(), 100);
        assert_eq!(decoded.hp_mp().unwrap().mp_percent(), 30);
        assert_eq!(
            decoded.position_world,
            Some(PartyPositionWorld {
                x: 11,
                y: 22,
                z: 33
            })
        );
        assert!(decoded.position_dungeon.is_none());
        assert_eq!(decoded.position_tail, Some(0xDEAD_BEEF));
        assert_eq!(decoded.guild_name.as_deref(), Some("Guo"));
        assert_eq!(decoded.flag, Some(6));
        assert_eq!(decoded.mastery_primary, Some(100));
        assert_eq!(decoded.mastery_secondary, Some(200));

        let back: Bytes = decoded.into();
        assert_eq!(back, wire);
    }

    /// The whole point of the rewrite: a record carrying one field is two bytes
    /// on the wire, not a truncated full record. The old fixed-layout model read
    /// the mask as `unk_byte01` and then demanded a `u32` member id that is not
    /// there, so every partial delta desynchronised the rest of the packet.
    #[test]
    fn a_partial_mask_record_reads_only_what_the_mask_names() {
        let wire = Bytes::from_static(&[PartyMemberMask::HP_MP, 0x5A]);

        let decoded = PartyMemberCore::try_from(wire.clone()).unwrap();

        assert_eq!(decoded.hp_mp, Some(0x5A));
        assert!(decoded.member_id.is_none());
        assert!(decoded.name.is_none());
        assert!(decoded.level.is_none());
        assert!(decoded.position_world.is_none());
        assert!(decoded.mastery_primary.is_none());
        let back: Bytes = decoded.into();
        assert_eq!(back, wire);
    }

    /// Bits combine. `0x24` — level *and* hp/mp in one update — is ordinary
    /// traffic that the previous `kind == 2` / `kind == 4` equality tests read as
    /// nothing at all.
    #[test]
    fn a_combined_mask_reads_both_fields() {
        let wire = Bytes::from_static(&[PartyMemberMask::LEVEL | PartyMemberMask::HP_MP, 42, 0x5A]);

        let decoded = PartyMemberCore::try_from(wire.clone()).unwrap();

        assert_eq!(decoded.level, Some(42));
        assert_eq!(decoded.hp_mp, Some(0x5A));
        let back: Bytes = decoded.into();
        assert_eq!(back, wire);
    }

    /// The position bit gates the region, the coordinates *and* the trailing
    /// u32 — and the region's high bit widens the coordinates from u16 to i32.
    #[test]
    fn the_position_bit_widens_the_coordinates_inside_a_dungeon() {
        let mut b: Vec<u8> = vec![PartyMemberMask::POSITION];
        b.extend(0x8001u16.to_le_bytes()); // high bit set -> dungeon
        b.extend((-5i32).to_le_bytes());
        b.extend(6i32.to_le_bytes());
        b.extend(7i32.to_le_bytes());
        b.extend(0u32.to_le_bytes()); // the tail rides the same bit
        let wire = Bytes::from(b);

        let decoded = PartyMemberCore::try_from(wire.clone()).unwrap();

        assert_eq!(
            decoded.position_dungeon,
            Some(PartyPositionDungeon { x: -5, y: 6, z: 7 })
        );
        assert!(decoded.position_world.is_none());
        assert_eq!(decoded.position_tail, Some(0));
        let back: Bytes = decoded.into();
        assert_eq!(back, wire);
    }

    /// 0x3065's header is mask-gated too, and bit 0 is what carries the leader —
    /// the field the client-side roster used to declare UNKNOWN.
    #[test]
    fn party_data_reads_the_leader_and_the_roster_under_their_own_bits() {
        let mut b: Vec<u8> = vec![0x03]; // both bits
        b.extend(77u32.to_le_bytes()); // party number
        b.extend(0xABCDu32.to_le_bytes()); // master join id
        b.push(PartySetup::EXP_SHARED | PartySetup::ITEM_SHARED);
        b.push(1); // member count
        b.extend(full_member_bytes());
        let wire = Bytes::from(b);

        let decoded = PartyData::try_from(wire.clone()).unwrap();

        assert!(decoded.has_party_info() && decoded.has_roster());
        assert_eq!(decoded.party_number, 77);
        assert_eq!(decoded.master_join_id, Some(0xABCD));
        assert!(decoded.setup().is_exp_shared() && decoded.setup().is_item_shared());
        assert_eq!(decoded.setup().capacity(), 8);
        assert_eq!(decoded.members.len(), 1);
        assert_eq!(decoded.members[0].mastery_secondary, Some(200));
        let back: Bytes = decoded.into();
        assert_eq!(back, wire);
    }

    /// A roster-only push (bit 1 alone) has no leader and no setup byte. Under
    /// the old `[u8; 9]` header those five bytes came out of the roster instead,
    /// mis-framing the first member.
    #[test]
    fn party_data_without_the_info_bit_has_no_leader_or_setup() {
        let mut b: Vec<u8> = vec![0x02];
        b.extend(77u32.to_le_bytes());
        b.push(0); // member count
        let wire = Bytes::from(b);

        let decoded = PartyData::try_from(wire.clone()).unwrap();

        assert!(!decoded.has_party_info());
        assert_eq!(decoded.master_join_id, None);
        assert_eq!(decoded.setup, None);
        // and the empty flag set is not "sharing was turned off"
        assert_eq!(decoded.setup(), PartySetup(0));
        assert!(decoded.members.is_empty());
        let back: Bytes = decoded.into();
        assert_eq!(back, wire);
    }

    /// Info-only (bit 0 alone): no count byte, no records.
    #[test]
    fn party_data_without_the_roster_bit_carries_no_count() {
        let mut b: Vec<u8> = vec![0x01];
        b.extend(77u32.to_le_bytes());
        b.extend(0xABCDu32.to_le_bytes());
        b.push(PartySetup::EXP_SHARED);
        let wire = Bytes::from(b);

        let decoded = PartyData::try_from(wire.clone()).unwrap();

        assert!(!decoded.has_roster());
        assert_eq!(decoded.member_count, None);
        assert!(decoded.members.is_empty());
        let back: Bytes = decoded.into();
        assert_eq!(back, wire);
    }

    /// An unrecognised 0x3864 update type must decode to "no payload" rather
    /// than failing the packet — go-sro documents a type 9 the original client
    /// does not handle at all.
    #[test]
    fn an_unknown_party_update_type_is_not_fatal() {
        let decoded = PartyUpdate::try_from(Bytes::from_static(&[9])).unwrap();

        assert_eq!(decoded.update_type, 9);
        assert!(decoded.joined.is_none());
        assert!(decoded.member_id.is_none());
        assert!(decoded.member_update.is_none());
    }

    /// Type 3 carries only the leaving member's id; type 6 adds the *same* mask
    /// record every other party opcode uses — and no `unk_byte07`, a byte the
    /// original's reader never reads.
    #[test]
    fn party_update_reads_the_payload_its_type_selects() {
        let mut body: Vec<u8> = vec![3];
        body.extend(0xABCDu32.to_le_bytes());
        let decoded = PartyUpdate::try_from(Bytes::from(body)).unwrap();
        assert_eq!(decoded.member_id, Some(0xABCD));
        assert!(decoded.member_update.is_none());

        let mut body: Vec<u8> = vec![6];
        body.extend(0xABCDu32.to_le_bytes());
        body.push(PartyMemberMask::HP_MP);
        body.push(0x5A);
        let wire = Bytes::from(body);
        let decoded = PartyUpdate::try_from(wire.clone()).unwrap();
        let update = decoded.member_update.clone().unwrap();
        assert_eq!(update.hp_mp, Some(0x5A));
        assert!(update.level.is_none());
        assert!(update.region.is_none());
        let back: Bytes = decoded.into();
        assert_eq!(back, wire);

        // type 2 is the full record, with nothing extra in front of the masteries
        let mut body: Vec<u8> = vec![2];
        body.extend(full_member_bytes());
        let wire = Bytes::from(body);
        let decoded = PartyUpdate::try_from(wire.clone()).unwrap();
        assert_eq!(decoded.joined.clone().unwrap().name.as_deref(), Some("Ax"));
        let back: Bytes = decoded.into();
        assert_eq!(back, wire);
    }

    /// The match list's whole body hangs off one flag.
    #[test]
    fn an_empty_match_list_is_just_the_flag() {
        let wire = Bytes::from_static(&[0]);
        let decoded = PartyMatchListResponse::try_from(wire.clone()).unwrap();

        assert!(!decoded.has_data);
        assert!(decoded.page.is_none());
        let back: Bytes = decoded.into();
        assert_eq!(back, wire);
    }

    /// ...and round-trips a populated page.
    #[test]
    fn a_populated_match_list_round_trips() {
        let mut body: Vec<u8> = vec![1, 2, 0, 1]; // has_data, page_count, page_index, party_count
        body.extend(77u32.to_le_bytes()); // number
        body.extend(0x1234u32.to_le_bytes()); // registered_at
        body.extend(3u16.to_le_bytes());
        body.extend(b"Bob");
        body.extend([0, 4, PartySetup::EXP_SHARED, PARTY_PURPOSE_QUEST, 10, 80]);
        body.extend(2u16.to_le_bytes());
        body.extend(b"hi");
        let wire = Bytes::from(body);

        let decoded = PartyMatchListResponse::try_from(wire.clone()).unwrap();

        let page = decoded.page.clone().unwrap();
        assert_eq!(page.parties.len(), 1);
        assert_eq!(page.parties[0].master_name, "Bob");
        assert_eq!(page.parties[0].title, "hi");
        assert!(PartySetup(page.parties[0].setup).is_exp_shared());
        let back: Bytes = decoded.into();
        assert_eq!(back, wire);
    }

    /// 0xB06B is a three-state result, not a bool: a failure carries a `u16`
    /// code that a `success: bool` silently discarded.
    #[test]
    fn the_delete_response_keeps_its_error_code() {
        let wire = Bytes::from_static(&[1, 0x2A, 0, 0, 0]);
        let decoded = PartyMatchDeleteResponse::try_from(wire.clone()).unwrap();
        assert_eq!(decoded.number, Some(42));
        assert_eq!(decoded.error_code, None);
        let back: Bytes = decoded.into();
        assert_eq!(back, wire);

        let wire = Bytes::from_static(&[2, 0x1C, 0x2C]);
        let decoded = PartyMatchDeleteResponse::try_from(wire.clone()).unwrap();
        assert_eq!(decoded.number, None);
        assert_eq!(decoded.error_code, Some(11292));
        let back: Bytes = decoded.into();
        assert_eq!(back, wire);
    }

    /// 0xB069 / 0xB06A: a failure is three bytes. Reading the seven record
    /// fields unconditionally ran the derive off the end, so the packet was
    /// dropped and the player was told nothing.
    #[test]
    fn the_match_form_acks_decode_both_arms() {
        let mut body: Vec<u8> = vec![1];
        body.extend(77u32.to_le_bytes());
        body.extend(0u32.to_le_bytes());
        body.extend([PartySetup::EXP_SHARED, PARTY_PURPOSE_HUNTING, 10, 80]);
        body.extend(2u16.to_le_bytes());
        body.extend(b"hi");
        let wire = Bytes::from(body);
        let decoded = PartyMatchCreationResponse::try_from(wire.clone()).unwrap();
        assert_eq!(decoded.entry.clone().unwrap().title, "hi");
        assert_eq!(decoded.error_code, None);
        let back: Bytes = decoded.into();
        assert_eq!(back, wire);

        let wire = Bytes::from_static(&[2, 0x0C, 0x2C]);
        let decoded = PartyMatchEditedResponse::try_from(wire.clone()).unwrap();
        assert!(decoded.entry.is_none());
        assert_eq!(decoded.error_code, Some(11276));
        let back: Bytes = decoded.into();
        assert_eq!(back, wire);
    }

    /// Sharing EXP raises the member cap from 4 to 8.
    #[test]
    fn the_setup_bitfield_decodes_independently() {
        let both = PartySetup(PartySetup::EXP_SHARED | PartySetup::ITEM_SHARED);
        assert!(both.is_exp_shared() && both.is_item_shared());
        assert!(!both.anyone_can_invite());
        assert_eq!(both.capacity(), 8);
        assert_eq!(PartySetup(PartySetup::ITEM_SHARED).capacity(), 4);
    }

    /// 0xB060 is the CREATE ack, not the invite ack: success carries the
    /// leader's JID and failure a u16 code, so a bare success bool would eat
    /// every real error (and `docs/net-invite-0x3080.md` files it under the
    /// wrong request — see the module note).
    #[test]
    fn the_create_ack_carries_a_jid_on_success_and_a_code_on_failure() {
        let wire = Bytes::from_static(&[1, 0x2A, 0, 0, 0]);
        let decoded = PartyCreateResponse::try_from(wire.clone()).unwrap();
        assert_eq!(decoded.leader_join_id, Some(42));
        assert_eq!(decoded.error_code, None);
        let back: Bytes = decoded.into();
        assert_eq!(back, wire);

        // 11288 = already in another party
        let wire = Bytes::from_static(&[2, 0x18, 0x2C]);
        let decoded = PartyCreateResponse::try_from(wire.clone()).unwrap();
        assert_eq!(decoded.error_code, Some(11288));
        assert_eq!(decoded.leader_join_id, None);
        let back: Bytes = decoded.into();
        assert_eq!(back, wire);
    }

    /// 0xB062's success case is empty on purpose: the invitation itself is the
    /// separate 0x3080 popup.
    #[test]
    fn the_invite_ack_is_empty_on_success() {
        let decoded = PartyInviteResponse::try_from(Bytes::from_static(&[1])).unwrap();
        assert_eq!(decoded.result, 1);
        assert_eq!(decoded.error_code, None);

        let wire = Bytes::from_static(&[2, 0x0C, 0x2C]);
        let decoded = PartyInviteResponse::try_from(wire.clone()).unwrap();
        assert_eq!(decoded.error_code, Some(11276));
        let back: Bytes = decoded.into();
        assert_eq!(back, wire);
    }

    /// 0xB06D branches on `result == 1`, not `== 2`: both tails are u16 and
    /// they mean different things, so a result of 3 must read as an ERROR and
    /// not as a join result.
    #[test]
    fn the_match_join_ack_branches_on_result_one_not_two() {
        let wire = Bytes::from_static(&[1, 2, 0]);
        let decoded = PartyMatchJoinAck::try_from(wire.clone()).unwrap();
        assert_eq!(decoded.join_result, Some(2));
        assert_eq!(decoded.error_code, None);
        let back: Bytes = decoded.into();
        assert_eq!(back, wire);

        // 11292 = cannot find corresponding party
        let wire = Bytes::from_static(&[3, 0x1C, 0x2C]);
        let decoded = PartyMatchJoinAck::try_from(wire.clone()).unwrap();
        assert_eq!(decoded.join_result, None);
        assert_eq!(decoded.error_code, Some(11292));
        let back: Bytes = decoded.into();
        assert_eq!(back, wire);
    }

    /// 0x706D's tail is the shared member record, so the applicant's guild and
    /// level arrive whenever the server's mask names them — the flat
    /// `unk_byte01, join_id_repeated, name` model was right only for mask 0x11
    /// and dropped everything after the name.
    #[test]
    fn the_join_notify_ends_in_a_member_record() {
        let mut body: Vec<u8> = Vec::new();
        for value in [7u32, 8, 9, 100, 200] {
            body.extend(value.to_le_bytes());
        }
        body.push(0); // unk_byte00
        body.extend(full_member_bytes());
        let wire = Bytes::from(body);

        let decoded = PartyMatchJoinNotify::try_from(wire.clone()).unwrap();

        assert_eq!(decoded.request_id, 7);
        assert_eq!(decoded.join_id, 8);
        assert_eq!(decoded.match_number, 9);
        assert_eq!(decoded.applicant.name.as_deref(), Some("Ax"));
        assert_eq!(decoded.applicant.guild_name.as_deref(), Some("Guo"));
        assert_eq!(decoded.applicant.level, Some(40));
        let back: Bytes = decoded.into();
        assert_eq!(back, wire);
    }

    /// 0x3068's tail width is a property of the ITEM, not of the packet: one
    /// byte for equipment, nothing for a container, two bytes for an
    /// expendable — and unknown when the class is unknown, because a guessed
    /// width is how a fake record gets forged.
    #[test]
    fn the_distribution_tail_is_read_against_the_item_class() {
        struct Fixed(ItemClass);
        impl ItemClassResolver for Fixed {
            fn item_class(&self, _ref_id: u32) -> ItemClass {
                self.0
            }
        }

        let mut body = 7u32.to_le_bytes().to_vec();
        body.extend(4242u32.to_le_bytes());
        body.push(9); // opt level, or the low byte of a quantity
        body.push(0);
        let wire = Bytes::from(body);

        let decoded = PartyDistribution::try_from(wire.clone()).unwrap();
        assert_eq!(decoded.join_id, 7);
        assert_eq!(decoded.ref_item_id, 4242);
        assert_eq!(
            decoded.detail(&Fixed(ItemClass::Equipment)),
            Some(PartyDistributionDetail::OptLevel(9))
        );
        assert_eq!(
            decoded.detail(&Fixed(ItemClass::Expendable { tid3: 0, tid4: 0 })),
            Some(PartyDistributionDetail::Quantity(9))
        );
        assert_eq!(
            decoded.detail(&Fixed(ItemClass::Container { tid3: 0, tid4: 0 })),
            Some(PartyDistributionDetail::None)
        );
        assert_eq!(
            decoded.detail(&Fixed(ItemClass::Unknown)),
            None,
            "an unknown class must not guess the tail's width"
        );

        let back: Bytes = decoded.into();
        assert_eq!(back, wire);
    }
}
