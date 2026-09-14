//! Guild war and siege authority: the war lifecycle (0x7110 start, 0x7112 end,
//! 0x7114 reward and their acks), the siege-authority update 0x70FF, and the
//! guild-hostility toggle 0x30EF.
//!
//! Idea: two of the three war acks are the shared guild ack form again; the
//! reward ack is not, and it is the one field in this family that a *server*
//! decompile names outright. `0x30EF` is the interesting one — it looks like a
//! per-entity flag and is not: the handler keys a **global guild-id set**, so
//! one packet changes the relation to every member of that guild at once.
//!
//! Layouts and evidence: `docs/net-guild-war.md`.

use bevy::prelude::Message;

use crate::agent::guild::guild_op_ack;

use sro_macro::ByteSize;
use sro_macro::Deserialize;
use sro_macro::SerializationError;
use sro_macro::Serialize;
use sro_macro_derive::*;

/// The one guild-war error code recovered from both sides of the wire: the
/// client special-cases it with `UIIT_CTL_GUILDWAR_NOTCOMPENSATION` and the
/// server writes it when the compensation value is `<= 0`
/// (`FUN_005c72a0 @5c72d2`).
pub const GUILD_WAR_NO_COMPENSATION: u16 = 0x4C45;

/// 0x30EF — server → client: add or remove a guild from the client's
/// **guild-relation set**.
///
/// Despite the inherited name "entity update", the handler `FUN_0088a770` keys a
/// global red-black tree by *guild id*, not by entity uid: the same set is
/// queried elsewhere as `FUN_00469e00(it, guild + 0x30)`, and `+0x30` is where
/// the guild id lives (it is the `%u` in the `"G%u_%u_%u.crb"` crest filename).
/// So one packet affects every member of that guild.
///
/// `flag == 1` inserts, `flag == 0` erases. Whether membership means "hostile"
/// or "friendly" is [U] — the two consumer functions gate name-tag and targeting
/// behaviour, but their branch sense is not readable without their full context.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct GuildRelationUpdate {
    /// 1 = add to the set, 0 = remove.
    pub flag: u8,
    pub guild_id: u32,
}

impl GuildRelationUpdate {
    pub fn is_add(&self) -> bool {
        self.flag == 1
    }
}

/// 0x70FF — client → server: update a member's siege authority.
/// Builder `FUN_0081f9e0@23`, body `b4 b1`.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct SiegeAuthorityUpdateRequest {
    /// [U] — verbatim; the builder gives the width, nothing names it.
    pub unk_u32_00: u32,
    /// [U] — verbatim.
    pub unk_u8_00: u8,
}

/// 0x7110 — client → server: declare a guild war.
/// Builder `FUN_00824a30@45`, body `strS b1 b4 b1 b4`.
///
/// The only request in the guild family with a mixed body, and the only one that
/// names anything: the string is the opposing guild, which is how the original's
/// war dialog addresses it. The four scalars are [U].
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct GuildWarStartRequest {
    pub target_guild: String,
    /// [U] — verbatim.
    pub unk_u8_00: u8,
    /// [U] — verbatim.
    pub unk_u32_00: u32,
    /// [U] — verbatim.
    pub unk_u8_01: u8,
    /// [U] — verbatim.
    pub unk_u32_01: u32,
}

/// 0x7112 — client → server: end a guild war. Builder `FUN_00820540@23`, `b4`.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct GuildWarEndRequest {
    /// [U] — verbatim.
    pub unk_u32_00: u32,
}

/// 0x7114 — client → server: claim the war compensation.
/// Builder `FUN_00820600@23`, `b4`.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct GuildWarRewardRequest {
    /// [U] — verbatim.
    pub unk_u32_00: u32,
}

guild_op_ack! {
    /// 0xB110 — ack for [`GuildWarStartRequest`], handler `FUN_00881ec0`.
    ///
    /// Its tail runs **unconditionally**: the war-declaration dialog is torn
    /// down whether the request succeeded or failed. A consumer should close the
    /// dialog on both arms rather than only on success.
    GuildWarStartAck
}

guild_op_ack! {
    /// 0xB112 — ack for [`GuildWarEndRequest`], handler `FUN_00881f30`.
    GuildWarEndAck
}

/// 0xB114 — ack for [`GuildWarRewardRequest`], handler `FUN_00885ae0`.
///
/// The one war ack with a payload, and the one field in this family that a
/// server decompile names outright: `FUN_005c72a0` loads the compensation value,
/// refuses with [`GUILD_WAR_NO_COMPENSATION`] when it is `<= 0`, and otherwise
/// writes `u8 result = 1` followed by that `u32`. Reader and writer agree,
/// including the error constant.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct GuildWarRewardAck {
    pub result: u8,
    #[sro_packet(when = "result == 1")]
    pub compensation: Option<u32>,
    #[sro_packet(when = "result != 1")]
    pub error_code: Option<u16>,
}

impl GuildWarRewardAck {
    pub fn is_success(&self) -> bool {
        self.result == 1
    }

    /// The refusal the original words itself
    /// (`UIIT_CTL_GUILDWAR_NOTCOMPENSATION`) instead of sending it to the
    /// generic error box.
    pub fn is_no_compensation(&self) -> bool {
        self.error_code == Some(GUILD_WAR_NO_COMPENSATION)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::{BufMut, Bytes, BytesMut};

    /// Five fixed bytes, and the `u32` is a guild id — not an entity uid.
    #[test]
    fn the_relation_update_is_a_flag_and_a_guild_id() {
        let wire = Bytes::from_static(&[0x01, 0x2A, 0x00, 0x00, 0x00]);
        let decoded = GuildRelationUpdate::try_from(wire.clone()).unwrap();
        assert!(decoded.is_add());
        assert_eq!(decoded.guild_id, 0x2A);
        assert_eq!(Bytes::from(decoded), wire);

        let removal = Bytes::from_static(&[0x00, 0x2A, 0x00, 0x00, 0x00]);
        assert!(!GuildRelationUpdate::try_from(removal).unwrap().is_add());
    }

    /// The war declaration is the family's only mixed body: a name first, then
    /// four scalars in the builder's order.
    #[test]
    fn the_war_declaration_leads_with_the_opposing_guild_name() {
        let mut body = BytesMut::new();
        body.put_u16_le(5);
        body.put_slice(b"Rival");
        body.put_u8(1);
        body.put_u32_le(2);
        body.put_u8(3);
        body.put_u32_le(4);
        let wire = body.freeze();

        let decoded = GuildWarStartRequest::try_from(wire.clone()).unwrap();
        assert_eq!(decoded.target_guild, "Rival");
        assert_eq!(
            (
                decoded.unk_u8_00,
                decoded.unk_u32_00,
                decoded.unk_u8_01,
                decoded.unk_u32_01
            ),
            (1, 2, 3, 4)
        );
        assert_eq!(Bytes::from(decoded), wire);
    }

    /// 5 bytes carrying the compensation on success, 3 on refusal — and the
    /// refusal the original words itself is recognised by name.
    #[test]
    fn the_reward_ack_carries_the_compensation_or_the_no_compensation_code() {
        let mut ok = BytesMut::new();
        ok.put_u8(1);
        ok.put_u32_le(5000);
        let ok = ok.freeze();
        let decoded = GuildWarRewardAck::try_from(ok.clone()).unwrap();
        assert_eq!(decoded.compensation, Some(5000));
        assert!(!decoded.is_no_compensation());
        assert_eq!(Bytes::from(decoded), ok);

        let refused = Bytes::from_static(&[0x02, 0x45, 0x4C]);
        let decoded = GuildWarRewardAck::try_from(refused.clone()).unwrap();
        assert!(decoded.is_no_compensation());
        assert_eq!(decoded.compensation, None);
        assert_eq!(Bytes::from(decoded), refused);
    }

    /// Start and end acks are the shared cluster form.
    #[test]
    fn the_war_lifecycle_acks_are_the_shared_form() {
        let ok = Bytes::from_static(&[0x01]);
        assert!(GuildWarStartAck::try_from(ok.clone()).unwrap().is_success());
        assert!(GuildWarEndAck::try_from(ok).unwrap().is_success());

        let refused = Bytes::from_static(&[0x02, 0x0E, 0x1C]);
        let ack = GuildWarStartAck::try_from(refused.clone()).unwrap();
        assert_eq!(ack.error_code, Some(0x1C0E));
        assert_eq!(Bytes::from(ack), refused);
    }

    /// The two single-`u32` requests round-trip at four bytes.
    #[test]
    fn the_end_and_reward_requests_are_a_single_u32() {
        let wire = Bytes::from_static(&[0x07, 0x00, 0x00, 0x00]);
        assert_eq!(
            GuildWarEndRequest::try_from(wire.clone())
                .unwrap()
                .unk_u32_00,
            7
        );
        assert_eq!(GuildWarRewardRequest::try_from(wire).unwrap().unk_u32_00, 7);
    }
}
