//! Guild union (alliance) wire opcodes: the three membership requests
//! 0x70FB / 0x70FC / 0x70FD and their acks 0xB0FB / 0xB0FC / 0xB0FD.
//!
//! Idea: the union acks are members of the guild ack cluster
//! (`00881820`-`00881e70`), so they reuse the one body form defined next door
//! in [`crate::agent::guild`] instead of restating it. What is specific to the
//! union is the *request* side, and one shape surprise: **0x70FC has an empty
//! body**, which is easy to mis-file as "layout unknown".
//!
//! Layouts and the handler-VA correction this module depends on are in
//! `docs/net-guild-union.md`. `0x3102` (union roster push) is deliberately
//! absent: its record was never decoded, and the one published reading of it is
//! the misattribution corrected there. No `packet_dump/*.log` exists for any of
//! these opcodes.

use bevy::prelude::Message;
use bytes::{Bytes, BytesMut};

use crate::agent::guild::guild_op_ack;

use sro_macro::ByteSize;
use sro_macro::Deserialize;
use sro_macro::SerializationError;
use sro_macro::Serialize;
use sro_macro_derive::*;

/// 0x70FB — client → server: invite a guild into the union.
/// Builder `FUN_008201b0@53`, body `b4`.
///
/// Target-addressed like the guild invite 0x70F3: the `u32` is the invited
/// guild master's spawned entity id, which is why the original only offers the
/// command on a selected player.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct UnionInviteRequest {
    pub target_unique_id: u32,
}

/// 0x70FD — client → server: expel a guild from the union.
/// Builder `FUN_00820390@23`, body `b4`.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct UnionExpelRequest {
    /// [U] — one `u32`; the builder shows the width, not whether it addresses
    /// the guild or its master.
    pub unk_u32_00: u32,
}

/// 0x70FC — client → server: leave the union. **Empty body.**
///
/// Builder `FUN_008202f0@23` writes no field at all — the server infers the
/// sender's guild. An empty body is a real layout, not a missing one, which is
/// why it is modelled rather than skipped: it is the cheapest possible
/// round-trip anchor for this family.
#[derive(Message, Clone, Debug, Default, PartialEq, Eq)]
pub struct UnionLeaveRequest;

impl TryFrom<Bytes> for UnionLeaveRequest {
    type Error = SerializationError;
    fn try_from(_: Bytes) -> Result<Self, SerializationError> {
        Ok(UnionLeaveRequest)
    }
}

impl From<UnionLeaveRequest> for Bytes {
    fn from(_: UnionLeaveRequest) -> Self {
        BytesMut::new().freeze()
    }
}

guild_op_ack! {
    /// 0xB0FB — ack for [`UnionInviteRequest`], handler `FUN_00881dd0`.
    UnionInviteAck
}

guild_op_ack! {
    /// 0xB0FC — ack for [`UnionLeaveRequest`], handler `FUN_00881e20`
    /// (server writer `FUN_005c9210 @5c9237` emits the single byte `01`).
    UnionLeaveAck
}

guild_op_ack! {
    /// 0xB0FD — ack for [`UnionExpelRequest`], handler `FUN_00881e70`
    /// (server writer `FUN_005c9270 @5c9297`).
    UnionExpelAck
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The empty body is the point: it encodes to nothing and decodes from
    /// nothing, and a stray tail does not turn it into an error.
    #[test]
    fn the_union_leave_request_has_no_body() {
        let wire: Bytes = UnionLeaveRequest.into();
        assert!(wire.is_empty());
        assert_eq!(
            UnionLeaveRequest::try_from(wire).unwrap(),
            UnionLeaveRequest
        );
    }

    /// Invite and expel are one `u32` each, little-endian.
    #[test]
    fn invite_and_expel_are_a_single_u32() {
        let wire = Bytes::from_static(&[0x2A, 0x00, 0x00, 0x00]);

        let invite = UnionInviteRequest::try_from(wire.clone()).unwrap();
        assert_eq!(invite.target_unique_id, 0x2A);
        assert_eq!(Bytes::from(invite), wire);

        let expel = UnionExpelRequest::try_from(wire.clone()).unwrap();
        assert_eq!(expel.unk_u32_00, 0x2A);
        assert_eq!(Bytes::from(expel), wire);
    }

    /// The three acks are the guild cluster's shared form, not a union-specific
    /// one — same success byte, same refusal shape.
    #[test]
    fn the_union_acks_are_the_shared_guild_ack_form() {
        let ok = Bytes::from_static(&[0x01]);
        assert!(UnionInviteAck::try_from(ok.clone()).unwrap().is_success());
        assert!(UnionLeaveAck::try_from(ok.clone()).unwrap().is_success());
        assert!(UnionExpelAck::try_from(ok).unwrap().is_success());

        let refused = Bytes::from_static(&[0x02, 0x0E, 0x1C]);
        let ack = UnionExpelAck::try_from(refused.clone()).unwrap();
        assert!(!ack.is_success());
        assert_eq!(ack.error_code, Some(0x1C0E));
        assert_eq!(Bytes::from(ack), refused);
    }
}
