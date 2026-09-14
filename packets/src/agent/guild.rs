//! Guild wire opcodes: the chunked guild-record transfer (0x34B3 / 0x3101 /
//! 0x34B4), the create-guild echo (0xB0F0), the notice edit request (0x70F9)
//! and two capture-gated pushes (0x30FF, 0x38F5).
//!
//! **Spec-derived, not capture-verified.** No `packet_dump/` sample exists for
//! any of these; layouts come from statically reading the original client's
//! parser/builder. Byte-level notes, per-field [V]/[S]/[U] tags and the
//! resolving capture for each unknown live in `docs/net-guild-0x3101.md`.
//!
//! The record arrives **chunked**: BEGIN, then one or more DATA bodies whose
//! payloads concatenate, then END, at which point the original parses the
//! assembled buffer in one go. So [`GuildDataBody`] is a raw passthrough and
//! [`GuildData`] is the parsed shape — the same split the character-data blob
//! uses ([`CharacterDataBody`](crate::agent::ingame::CharacterDataBody)),
//! except that guild records need no itemdata resolver, so [`GuildData`] is a
//! plain derive and [`GuildData::parse`] is all the staging that is required.
//!
//! Guild storage (0x7250/0xB250), guild chat and alliance/union live in their
//! own docs and are not here.

use bevy::prelude::Message;
use bytes::{BufMut, Bytes, BytesMut};

use sro_macro::ByteSize;
use sro_macro::Deserialize;
use sro_macro::SerializationError;
use sro_macro::Serialize;
use sro_macro_derive::*;

/// `SRGuildMember.Permissions` — a `[Flags] uint`, so it is a newtype rather
/// than a derived enum: real servers can set bits this list does not name, and
/// an unknown discriminator would otherwise fail the whole packet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GuildPermissions(pub u32);

impl GuildPermissions {
    pub const JOIN: u32 = 0x0000_0001;
    pub const KICK: u32 = 0x0000_0002;
    pub const UNION_CHAT: u32 = 0x0000_0004;
    pub const STORAGE: u32 = 0x0000_0008;
    pub const NOTICE: u32 = 0x0000_0010;
    /// Everything a non-master can hold.
    pub const ALL: u32 = 0x0000_001F;
    /// The guild master's sentinel — every bit set, not just [`Self::ALL`].
    pub const MASTER: u32 = 0xFFFF_FFFF;

    pub fn can_invite(&self) -> bool {
        self.0 & Self::JOIN != 0
    }

    pub fn can_kick(&self) -> bool {
        self.0 & Self::KICK != 0
    }

    pub fn can_union_chat(&self) -> bool {
        self.0 & Self::UNION_CHAT != 0
    }

    pub fn can_use_storage(&self) -> bool {
        self.0 & Self::STORAGE != 0
    }

    pub fn can_edit_notice(&self) -> bool {
        self.0 & Self::NOTICE != 0
    }

    /// The master sentinel, distinct from merely holding every named right.
    pub fn is_master_sentinel(&self) -> bool {
        self.0 == Self::MASTER
    }
}

/// One roster entry inside the assembled guild record.
#[derive(Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct GuildMember {
    pub member_id: u32,
    pub name: String,
    /// [U] — read verbatim, semantics unknown.
    pub unk_u8_01: u8,
    pub level: u8,
    pub guild_points: u32,
    /// See [`GuildPermissions`].
    pub permissions: u32,
    /// [U] ×3 — read verbatim, semantics unknown.
    pub unk_u32_01: u32,
    pub unk_u32_02: u32,
    pub unk_u32_03: u32,
    pub nickname: String,
    pub model_id: u32,
    pub is_master: bool,
    pub is_offline: bool,
}

impl GuildMember {
    pub fn permissions(&self) -> GuildPermissions {
        GuildPermissions(self.permissions)
    }
}

/// The assembled guild record — the concatenation of every [`GuildDataBody`]
/// between BEGIN and END, and also the tail of a successful [`GuildCreatedData`].
///
/// Not a wire type of its own: nothing carries this as a single packet body,
/// which is why it is parsed via [`Self::parse`] rather than registered.
#[derive(Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct GuildData {
    pub guild_id: u32,
    pub name: String,
    pub level: u8,
    pub guild_points: u32,
    pub notice: String,
    pub message: String,
    /// [U] — read verbatim, semantics unknown.
    pub unk_u32_00: u32,
    /// [U] — read verbatim, semantics unknown.
    pub unk_u8_00: u8,
    pub member_count: u8,
    #[sro_packet(list_type = "by-size-field", size_field = "member_count")]
    pub members: Vec<GuildMember>,
}

impl GuildData {
    /// Parse an assembled record. The original reads exactly to the end of the
    /// last member, so a well-formed buffer leaves no tail.
    pub fn parse(assembled: Bytes) -> Result<Self, SerializationError> {
        Self::try_from(assembled)
    }
}

/// 0x34B3 — server → client: the guild record starts. Marker, empty body.
///
/// The original reads no bytes here. Whether a length or id prefix exists is
/// [U] — resolving capture `packet_dump/0x34B3.log`.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct GuildDataBegin;

/// 0x3101 — server → client: one chunk of the guild record.
///
/// Carried unparsed: a single chunk is not a complete record, so decoding it as
/// [`GuildData`] would fail on every packet but the last. Accumulate the bodies
/// between BEGIN and END, then call [`GuildData::parse`].
#[derive(Message, Clone, Debug, PartialEq)]
pub struct GuildDataBody {
    pub data: Bytes,
}

impl TryFrom<Bytes> for GuildDataBody {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, SerializationError> {
        Ok(GuildDataBody { data: value })
    }
}

impl From<GuildDataBody> for Bytes {
    fn from(p: GuildDataBody) -> Self {
        p.data
    }
}

/// 0x34B4 — server → client: the guild record is complete. Marker, empty body.
///
/// Same [U] as [`GuildDataBegin`] — the original reads no bytes and takes no
/// packet argument; it only closes the accumulator.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct GuildDataEnd;

/// 0x30FF — server → client: a guild activity-log entry.
///
/// Carried unparsed: the original has an enum entry but **no parser at all**,
/// so there is no layout to encode — inventing one would be a guess. Resolving
/// capture `packet_dump/0x30FF.log`.
#[derive(Message, Clone, Debug, PartialEq)]
pub struct GuildPlayerLog {
    pub data: Bytes,
}

impl TryFrom<Bytes> for GuildPlayerLog {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, SerializationError> {
        Ok(GuildPlayerLog { data: value })
    }
}

impl From<GuildPlayerLog> for Bytes {
    fn from(p: GuildPlayerLog) -> Self {
        p.data
    }
}

/// 0x38F5 — server → client: an incremental guild update.
///
/// The original reads `update_type` and then switches on it with an **empty**
/// body for every arm, so only the discriminator is known: 5 = notice,
/// 6 = permissions, 15 = ?. The rest is kept raw rather than guessed — one
/// capture per type (`packet_dump/0x38F5.log`) is what resolves it.
#[derive(Message, Clone, Debug, PartialEq)]
pub struct GuildUpdate {
    pub update_type: u8,
    /// [U] — the per-type payload, unparsed.
    pub tail: Bytes,
}

impl TryFrom<Bytes> for GuildUpdate {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, SerializationError> {
        let update_type = *value.first().ok_or_else(|| {
            SerializationError::IoError(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "empty 0x38F5 body",
            ))
        })?;
        Ok(GuildUpdate {
            update_type,
            tail: value.slice(1..),
        })
    }
}

impl From<GuildUpdate> for Bytes {
    fn from(p: GuildUpdate) -> Self {
        let mut buf = BytesMut::with_capacity(1 + p.tail.len());
        buf.put_u8(p.update_type);
        buf.extend_from_slice(&p.tail);
        buf.freeze()
    }
}

/// 0xB0F0 — server → client: the answer to creating a guild. On success it
/// carries the same record as the chunked transfer, inline.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct GuildCreatedData {
    pub success: bool,
    #[sro_packet(when = "success")]
    pub data: Option<GuildData>,
}

/// 0x70F9 — client → server: edit the guild notice.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct GuildNoticeEditRequest {
    pub title: String,
    pub message: String,
}

/// The guild command acks share **one body form**: `{result:u8}` on success,
/// `{result:u8, error_code:u16}` on refusal. That is not an assumption — the
/// eleven handlers sit in one VA cluster (`00881820`–`00881e70`) and every one
/// of them reads the same two fields in the same order, and the server writers
/// in the `0x005c9xxx` family answer with the single byte `01` on success
/// (`docs/re/net/inbound/guild.md`, net-batch-10/11). Modelling them as one
/// generated form keeps that finding visible instead of copying a struct
/// eleven times.
///
/// Each ack still gets its own type, because the opcode table in
/// [`crate::Packet`] maps one opcode to one type.
macro_rules! guild_op_ack {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        ///
        /// Body: `result:u8` (1 = ok, 2 = error); on `result != 1` a `u16`
        /// error code follows. 1 byte on success, 3 on failure.
        #[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
        pub struct $name {
            pub result: u8,
            #[sro_packet(when = "result != 1")]
            pub error_code: Option<u16>,
        }

        impl $name {
            /// `result == 1`. The original tests for exactly this value and
            /// treats every other value as the error arm.
            pub fn is_success(&self) -> bool {
                self.result == 1
            }
        }
    };
}

/// Re-exported for the union family, whose acks live in the same handler
/// cluster ([`crate::agent::guild_union`]).
pub(crate) use guild_op_ack;

guild_op_ack! {
    /// 0xB0F1 — ack for the guild **disband** request (C→S `0x70F1`), handler
    /// `FUN_00881820`.
    ///
    /// Which of 0xB0F1/0xB0F2 is disband and which is leave is not stated by
    /// either handler — both refresh the same guild window. Two independent
    /// sources decide it the same way: the C→S catalog pairs `0x70F1` with
    /// disband and `0x70F2` with leave/secede, and the server function that
    /// emits the *kick* ack `0xB0F4` emits `0xB0F2` on its other branch
    /// (`FUN_005c8b90 @5c8bbe`, "kick vs leave") — so `0xB0F2` is the
    /// member-side verb and `0xB0F1` is the one left over. Naming is [S],
    /// the layout is [V].
    GuildDisbandAck
}

guild_op_ack! {
    /// 0xB0F2 — ack for the guild **leave/secede** request (C→S `0x70F2`),
    /// handler `FUN_008819e0`. See [`GuildDisbandAck`] for why this one is
    /// leave rather than disband.
    GuildLeaveAck
}

guild_op_ack! {
    /// 0xB0F3 — ack for the guild **invite** (C→S `0x70F3`
    /// [`crate::agent::ingame::GuildInviteRequest`]), handler `FUN_00881950`.
    ///
    /// The success arm is deliberately inert in the original: the inviter
    /// learns nothing here, the invitee gets the `0x3080` petition popup. The
    /// error arm additionally clears the client's "invitation pending" flag.
    GuildInviteAck
}

guild_op_ack! {
    /// 0xB0F4 — ack for **kicking** a member (C→S `0x70F4`), handler
    /// `FUN_00881c60`; SilkroadDoc calls it `AGENT_GUILD_KICK`.
    ///
    /// The success arm does nothing: the roster change arrives out of band via
    /// `0x3100`/`0x38F5`. Server writer `FUN_005c8b90 @5c8bbe` emits the single
    /// byte `01`.
    GuildKickAck
}

guild_op_ack! {
    /// 0xB0F9 — ack for the notice edit ([`GuildNoticeEditRequest`], C→S
    /// `0x70F9`), handler `FUN_00881a70`.
    ///
    /// On success the original shows `UIIT_MSG_GUILD_COMMON_KNOW_REMIND_UPDATE`
    /// and reads nothing further; the server writer `FUN_005c91a0 @5c91d7`
    /// agrees byte-for-byte.
    GuildNoticeEditAck
}

guild_op_ack! {
    /// 0xB0FA — ack for **promote/demote** (C→S `0x70FA`), handler
    /// `FUN_00881bd0`; SilkroadDoc calls it `AGENT_GUILD_PROMOTE`.
    ///
    /// Success refreshes the guild panel and carries no payload. The server
    /// side (`FUN_005c8fd0 @5c90aa`) charges a per-grade cost with the grade
    /// clamped to 2..=5, which is where the C→S grade range comes from — that
    /// request is not modelled here, only its ack.
    GuildPromoteAck
}

guild_op_ack! {
    /// 0xB104 — ack for a member **permission update** (C→S `0x7104`), handler
    /// `FUN_00881cb0`; SilkroadDoc calls it `AGENT_GUILD_UPDATE_PERMISSION`.
    ///
    /// Success is empty; the new bitmask itself arrives inside the guild record
    /// as [`GuildMember::permissions`]. Server writer `FUN_005c9530 @5c9557`.
    GuildPermissionUpdateAck
}

/// 0xB0F6 — ack for the guild **GP donation** (C→S `0x70F6`), handler
/// `FUN_00881ae0`; SilkroadDoc calls it `AGENT_GUILD_DONATE_OBSOLETE`.
///
/// The one member of the cluster whose success arm is not empty: it carries the
/// donated guild points, which the original formats into
/// `UIIT_MSG_GUILD_GP_SUBSCRIPION_RESULT` (the misspelling is the original's).
/// The string literal is resolved inside the decompiled handler, so the `u32`
/// is unambiguously the contributed amount rather than a running total.
/// 5 bytes on success, 3 on failure.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct GuildDonateAck {
    pub result: u8,
    #[sro_packet(when = "result == 1")]
    pub donated_gp: Option<u32>,
    #[sro_packet(when = "result != 1")]
    pub error_code: Option<u16>,
}

impl GuildDonateAck {
    pub fn is_success(&self) -> bool {
        self.result == 1
    }
}

/// 0x3100 — server → client: this entity is no longer in a guild.
///
/// A bare entity id. The client drops the guild association on the spawned
/// object; the server emits one of these per member while disbanding a guild
/// (`FUN_005c3d80` writes exactly one `u32` per member, the handler
/// `FUN_008819b0` reads exactly one).
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct EntityGuildRemove {
    pub entity_id: u32,
}

/// 0x70F0 — client → server: create a guild.
///
/// Builder `FUN_0081f5a0@37`: one `b4` then an ASCII string
/// (`docs/re/net/outbound/guild-union.md`). The leading `u32` is the guild
/// **NPC's** unique id — creation is an NPC dialogue in the original, the same
/// shape the storage opener [`crate::agent::guild_storage::GuildStorageOpenRequest`]
/// uses — but the decompile only shows the width, so the name is [S] and the
/// value's origin is the window's target slot.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct GuildCreateRequest {
    pub npc_unique_id: u32,
    pub name: String,
}

/// 0x70F1 — client → server: disband the guild. Builder `FUN_0081f710@24`,
/// body `b4`.
///
/// The `u32` comes from the guild window's accessor `FUN_00778b70()`
/// (`this+0x628`) — the *same* slot 0x70F2 reads. Whether that slot holds the
/// guild id or the current selection is [U], so the field is not named after a
/// guess; a capture of either request decides it. See [`GuildDisbandAck`] for
/// why this opcode is disband and 0x70F2 is leave.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct GuildDisbandRequest {
    /// [U] — the guild window's `+0x628` slot, verbatim.
    pub unk_u32_00: u32,
}

/// 0x70F2 — client → server: leave/secede from the guild. Builder
/// `FUN_0081f7d0@24`, body `b4` from the same accessor as
/// [`GuildDisbandRequest`].
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct GuildLeaveRequest {
    /// [U] — the guild window's `+0x628` slot, verbatim.
    pub unk_u32_00: u32,
}

/// 0x70F4 — client → server: kick a member, addressed **by name**, not by id.
/// Builder `FUN_00824190@72`, body `strA`.
///
/// The name-addressed form is the notable part: every other membership op in
/// the family carries a `u32`, so a consumer must pass the roster row's name
/// ([`GuildMember::name`]) rather than its `member_id`.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct GuildKickRequest {
    pub member_name: String,
}

/// 0x70FA — client → server: promote/demote a member. Builder
/// `FUN_0081fdc0@24`, body `b4`.
///
/// The ack's server side charges a cost indexed by the new grade with the grade
/// clamped to `2..=5` (`FUN_005c8fd0`), so grades 2..5 are the promotable range;
/// whether this `u32` is that grade or the member id is [U] — one `b4` is all
/// the builder shows.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct GuildPromoteRequest {
    /// [U] — one `u32`, either the target member or the target grade.
    pub unk_u32_00: u32,
}

/// 0x7104 — client → server: the guild permission update, a single byte.
/// Builder `005ace60@59` (an inlined constructor, which is why openroad's
/// sender map never saw it), body `b1`.
///
/// Its ack is [`GuildPermissionUpdateAck`]. One byte cannot be a 32-bit
/// permission mask, so this is a selector, not the mask itself — the mask
/// travels in the guild record. Which selector is [U].
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct GuildPermissionUpdateRequest {
    /// [U] — one byte, verbatim.
    pub unk_u8_00: u8,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ascii(s: &str) -> Vec<u8> {
        let mut out = (s.len() as u16).to_le_bytes().to_vec();
        out.extend(s.as_bytes());
        out
    }

    fn guild_record(members: &[(&str, u32)]) -> Vec<u8> {
        let mut out = 0x2A_u32.to_le_bytes().to_vec(); // guild_id
        out.extend(ascii("Wanderers"));
        out.push(5); // level
        out.extend(1234u32.to_le_bytes()); // guild_points
        out.extend(ascii("notice text"));
        out.extend(ascii("motd"));
        out.extend(0u32.to_le_bytes()); // unk_u32_00
        out.push(0); // unk_u8_00
        out.push(members.len() as u8);
        for (name, perms) in members {
            out.extend(0x1234u32.to_le_bytes()); // member_id
            out.extend(ascii(name));
            out.push(0); // unk_u8_01
            out.push(40); // level
            out.extend(99u32.to_le_bytes()); // guild_points
            out.extend(perms.to_le_bytes());
            out.extend([0u8; 12]); // unk_u32_01..03
            out.extend(ascii("nick"));
            out.extend(1907u32.to_le_bytes()); // model_id
            out.push(u8::from(*perms == GuildPermissions::MASTER));
            out.push(0); // is_offline
        }
        out
    }

    /// The assembled record round-trips, and the roster is sized by the header
    /// count rather than read to EOF.
    #[test]
    fn an_assembled_guild_record_round_trips() {
        let wire = Bytes::from(guild_record(&[
            ("Master", GuildPermissions::MASTER),
            ("Grunt", GuildPermissions::JOIN | GuildPermissions::STORAGE),
        ]));

        let decoded = GuildData::parse(wire.clone()).unwrap();

        assert_eq!(decoded.guild_id, 0x2A);
        assert_eq!(decoded.name, "Wanderers");
        assert_eq!(decoded.notice, "notice text");
        assert_eq!(decoded.members.len(), 2);
        assert_eq!(decoded.members[1].nickname, "nick");
        let back: Bytes = decoded.into();
        assert_eq!(back, wire);
    }

    /// `permissions` is a bitfield, so the master sentinel is every bit set —
    /// not merely holding each named right — and a member can hold an
    /// arbitrary subset.
    #[test]
    fn permissions_decode_as_flags_not_an_enum() {
        let wire = Bytes::from(guild_record(&[
            ("Master", GuildPermissions::MASTER),
            ("Grunt", GuildPermissions::JOIN | GuildPermissions::STORAGE),
        ]));
        let decoded = GuildData::parse(wire).unwrap();

        let master = decoded.members[0].permissions();
        assert!(master.is_master_sentinel());
        assert!(master.can_kick() && master.can_edit_notice());

        let grunt = decoded.members[1].permissions();
        assert!(grunt.can_invite() && grunt.can_use_storage());
        assert!(!grunt.can_kick() && !grunt.can_edit_notice());
        assert!(!grunt.is_master_sentinel());

        // an unnamed bit must not fail decoding
        assert!(!GuildPermissions(0x8000_0000).can_invite());
    }

    /// A chunk is deliberately *not* a record: 0x3101 carries its payload raw
    /// so a partial buffer cannot fail the packet, and the pieces concatenate.
    #[test]
    fn chunks_pass_through_raw_and_concatenate_into_a_record() {
        let full = guild_record(&[("Solo", GuildPermissions::ALL)]);
        let (head, rest) = full.split_at(10);

        let a = GuildDataBody::try_from(Bytes::copy_from_slice(head)).unwrap();
        let b = GuildDataBody::try_from(Bytes::copy_from_slice(rest)).unwrap();

        // neither half is a record on its own
        assert!(GuildData::parse(a.data.clone()).is_err());

        let mut assembled = BytesMut::new();
        assembled.extend_from_slice(&a.data);
        assembled.extend_from_slice(&b.data);
        let decoded = GuildData::parse(assembled.freeze()).unwrap();
        assert_eq!(decoded.members[0].name, "Solo");
    }

    /// 0xB0F0 carries the same record inline, gated on success.
    #[test]
    fn the_create_response_carries_the_record_only_on_success() {
        let failed = Bytes::from_static(&[0]);
        let decoded = GuildCreatedData::try_from(failed.clone()).unwrap();
        assert!(!decoded.success);
        assert!(decoded.data.is_none());
        assert_eq!(Bytes::from(decoded), failed);

        let mut body = vec![1u8];
        body.extend(guild_record(&[("Founder", GuildPermissions::MASTER)]));
        let wire = Bytes::from(body);
        let decoded = GuildCreatedData::try_from(wire.clone()).unwrap();
        assert_eq!(decoded.data.as_ref().unwrap().name, "Wanderers");
        assert_eq!(Bytes::from(decoded), wire);
    }

    /// 0x38F5 keeps its per-type payload raw — the original switches on the
    /// type with an empty body for every arm, so there is nothing to decode.
    #[test]
    fn a_guild_update_keeps_its_unknown_payload() {
        let wire = Bytes::from_static(&[5, 0xAA, 0xBB]);
        let decoded = GuildUpdate::try_from(wire.clone()).unwrap();

        assert_eq!(decoded.update_type, 5);
        assert_eq!(&decoded.tail[..], &[0xAA, 0xBB]);
        assert_eq!(Bytes::from(decoded), wire);

        assert!(GuildUpdate::try_from(Bytes::new()).is_err());
    }

    #[test]
    fn the_notice_edit_request_round_trips() {
        let mut body = ascii("Title");
        body.extend(ascii("Body text"));
        let wire = Bytes::from(body);

        let decoded = GuildNoticeEditRequest::try_from(wire.clone()).unwrap();

        assert_eq!(
            (decoded.title.as_str(), decoded.message.as_str()),
            ("Title", "Body text")
        );
        assert_eq!(Bytes::from(decoded), wire);
    }

    /// The 0xB0Fx cluster is one body form: 1 byte on success, `result` plus a
    /// `u16` code on refusal — and it round-trips in both arms.
    #[test]
    fn a_guild_op_ack_is_one_byte_on_success_and_three_on_refusal() {
        let ok = Bytes::from_static(&[0x01]);
        let decoded = GuildKickAck::try_from(ok.clone()).unwrap();
        assert!(decoded.is_success());
        assert_eq!(decoded.error_code, None);
        assert_eq!(Bytes::from(decoded), ok);

        let refused = Bytes::from_static(&[0x02, 0x33, 0x4C]);
        let decoded = GuildKickAck::try_from(refused.clone()).unwrap();
        assert!(!decoded.is_success());
        assert_eq!(decoded.error_code, Some(0x4C33));
        assert_eq!(Bytes::from(decoded), refused);
    }

    /// Every ack in the cluster reads the same two fields — the point of the
    /// shared form. Decoding the identical wire through each type must agree.
    #[test]
    fn every_ack_in_the_cluster_reads_the_same_two_fields() {
        let refused = Bytes::from_static(&[0x02, 0x0E, 0x1C]);

        macro_rules! same {
            ($($ty:ty),*) => {$({
                let d = <$ty>::try_from(refused.clone()).unwrap();
                assert_eq!((d.result, d.error_code), (0x02, Some(0x1C0E)));
                assert_eq!(Bytes::from(d), refused);
            })*};
        }

        same!(
            GuildDisbandAck,
            GuildLeaveAck,
            GuildInviteAck,
            GuildKickAck,
            GuildNoticeEditAck,
            GuildPromoteAck,
            GuildPermissionUpdateAck
        );
    }

    /// 0xB0F6 is the exception: its success arm carries the donated GP.
    #[test]
    fn the_donate_ack_carries_the_contributed_gp_only_on_success() {
        let mut body = vec![0x01u8];
        body.extend(500u32.to_le_bytes());
        let wire = Bytes::from(body);
        let decoded = GuildDonateAck::try_from(wire.clone()).unwrap();
        assert_eq!(decoded.donated_gp, Some(500));
        assert_eq!(decoded.error_code, None);
        assert_eq!(Bytes::from(decoded), wire);

        let refused = Bytes::from_static(&[0x02, 0x0E, 0x1C]);
        let decoded = GuildDonateAck::try_from(refused.clone()).unwrap();
        assert_eq!(decoded.donated_gp, None);
        assert_eq!(decoded.error_code, Some(0x1C0E));
        assert_eq!(Bytes::from(decoded), refused);
    }

    /// 0x3100 is a bare entity id — four bytes, nothing else.
    #[test]
    fn the_guild_removal_push_is_a_bare_entity_id() {
        let wire = Bytes::from_static(&[0x2A, 0x00, 0x00, 0x00]);
        let decoded = EntityGuildRemove::try_from(wire.clone()).unwrap();
        assert_eq!(decoded.entity_id, 0x2A);
        assert_eq!(Bytes::from(decoded), wire);
    }

    /// The two lifecycle requests are one `u32` each — the same window slot,
    /// so they must encode identically.
    #[test]
    fn the_lifecycle_requests_are_a_single_u32() {
        let wire = Bytes::from_static(&[0x2A, 0x00, 0x00, 0x00]);

        let disband = GuildDisbandRequest::try_from(wire.clone()).unwrap();
        assert_eq!(disband.unk_u32_00, 0x2A);
        assert_eq!(Bytes::from(disband), wire);

        let leave = GuildLeaveRequest::try_from(wire.clone()).unwrap();
        assert_eq!(leave.unk_u32_00, 0x2A);
        assert_eq!(Bytes::from(leave), wire);

        let promote = GuildPromoteRequest::try_from(wire.clone()).unwrap();
        assert_eq!(promote.unk_u32_00, 0x2A);
        assert_eq!(Bytes::from(promote), wire);
    }

    /// 0x70F0 leads with the NPC id and then the guild name.
    #[test]
    fn the_create_request_is_an_id_then_a_name() {
        let mut body = 0x1234u32.to_le_bytes().to_vec();
        body.extend(ascii("Wanderers"));
        let wire = Bytes::from(body);

        let decoded = GuildCreateRequest::try_from(wire.clone()).unwrap();
        assert_eq!(decoded.npc_unique_id, 0x1234);
        assert_eq!(decoded.name, "Wanderers");
        assert_eq!(Bytes::from(decoded), wire);
    }

    /// The kick is addressed by name — no id anywhere in the body.
    #[test]
    fn the_kick_request_addresses_the_member_by_name() {
        let wire = Bytes::from(ascii("Grunt"));
        let decoded = GuildKickRequest::try_from(wire.clone()).unwrap();
        assert_eq!(decoded.member_name, "Grunt");
        assert_eq!(Bytes::from(decoded), wire);
    }

    /// 0x7104 is one byte, so it cannot be carrying the 32-bit mask.
    #[test]
    fn the_permission_update_request_is_a_single_byte() {
        let wire = Bytes::from_static(&[0x03]);
        let decoded = GuildPermissionUpdateRequest::try_from(wire.clone()).unwrap();
        assert_eq!(decoded.unk_u8_00, 0x03);
        assert_eq!(Bytes::from(decoded), wire);
    }
}
