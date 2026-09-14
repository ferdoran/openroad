//! Player-to-player exchange (trade) wire opcodes: the window's lifecycle
//! (0x3085–0x3088), the peer's staged gold/items (0x3089 / 0x308C) and the
//! confirm/approve/exit request-response pairs (0x708x / 0xB08x).
//!
//! **Spec-derived, not capture-verified.** No `packet_dump/` sample exists for
//! any of these, so the layouts come from statically reading the original
//! client's parser/builder. Byte-level notes, per-field [V]/[S]/[U] tags and the
//! resolving capture for each unknown live in `docs/net-exchange-0x3085.md`.
//!
//! Two neighbours deliberately live elsewhere:
//! - the **invitation** (0x7081 / 0xB081) belongs to `docs/net-invite-0x3080.md`;
//! - **your own** staging is not an exchange opcode at all — it rides
//!   `0x7034`/`0xB034` sub-ops `InventoryToExchange` / `ExchangeToInventory` /
//!   `InventoryGoldToExchange`, which are themselves still missing from
//!   [`InventoryOperationRequest`](crate::agent::inventory::InventoryOperationRequest).
//!   A working trade window needs both halves; this issue is the exchange half.

use bevy::prelude::Message;
use bytes::Bytes;

use crate::agent::character_data::{InventoryItem, ItemClassResolver};

use sro_macro::ByteSize;
use sro_macro::Deserialize;
use sro_macro::SerializationError;
use sro_macro::Serialize;
use sro_macro_derive::*;

/// 0x3085 — server → client: the trade window opened against `partner_unique_id`.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct ExchangeStarted {
    pub partner_unique_id: u32,
}

/// 0x3086 — server → client: the *partner* pressed confirm. Empty body.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct ExchangePlayerConfirmed;

/// 0x3087 — server → client: the trade went through. Empty body.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct ExchangeCompleted;

/// 0x3088 — server → client: the trade was called off. Empty body.
///
/// The original's parser reads nothing here, but a trailing `u16` reason code
/// may exist — [U], resolving capture `packet_dump/0x3088.log`. Decoding
/// tolerates one (trailing bytes are ignored) but does not surface it; adding
/// the field is a one-line change once a capture shows it is really there.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct ExchangeCanceled;

/// 0x3089 — server → client: the gold the *partner* has staged.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct ExchangeGoldUpdate {
    /// [U] — semantics unknown; resolving capture `packet_dump/0x3089.log`.
    pub unk_byte01: u8,
    pub gold: u64,
}

/// 0x308C — server → client: the items the *partner* has staged.
///
/// The list is kept as a raw tail because each entry's body is
/// class-dependent: the width of an item record is only known once its
/// `ref_id` has been resolved through an itemdata lookup, which the wire types
/// cannot do. This mirrors how the 0xB034 pickup payload is handled
/// ([`InventoryOperationResult::pickup_item`](crate::agent::inventory::InventoryOperationResult::pickup_item))
/// — decode on demand via [`Self::items`].
///
/// The original's parser early-returns when the updated player is *you*, so on
/// the wire this always describes the peer; your own staging is acked through
/// 0xB034 instead.
#[derive(Message, Clone, Debug, PartialEq)]
pub struct ExchangeItemsUpdate {
    pub player_unique_id: u32,
    pub item_count: u8,
    /// `item_count` records, each a slot byte followed by a class-dependent
    /// item block. See [`Self::items`].
    pub tail: Bytes,
}

impl ExchangeItemsUpdate {
    /// Decode the staged list, given an itemdata class resolver.
    ///
    /// Each record is a slot byte followed by the shared item block — exactly
    /// what [`InventoryItem::read_with`] consumes — so the list is read by
    /// calling it `item_count` times. Returns `None` if any record fails,
    /// because a partial list would silently misrepresent what the peer is
    /// offering.
    ///
    /// Note the entry carries **no** separate exchange-slot byte: the original
    /// reads one only inside a branch its own early return already made
    /// unreachable, so the partner-side layout is slot-then-item. Whether a
    /// self-targeted 0x308C with a second slot byte exists is [U].
    pub fn items(&self, resolver: &impl ItemClassResolver) -> Option<Vec<InventoryItem>> {
        let mut cursor = std::io::Cursor::new(self.tail.as_ref());
        let mut items = Vec::with_capacity(self.item_count as usize);
        for _ in 0..self.item_count {
            items.push(InventoryItem::read_with(&mut cursor, resolver).ok()?);
        }
        Some(items)
    }
}

impl TryFrom<Bytes> for ExchangeItemsUpdate {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, SerializationError> {
        let short = || {
            SerializationError::IoError(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "0x308C body too short",
            ))
        };
        let player_unique_id =
            u32::from_le_bytes(value.get(0..4).ok_or_else(short)?.try_into().unwrap());
        Ok(ExchangeItemsUpdate {
            player_unique_id,
            item_count: *value.get(4).ok_or_else(short)?,
            tail: value.slice(5..),
        })
    }
}

impl From<ExchangeItemsUpdate> for Bytes {
    fn from(p: ExchangeItemsUpdate) -> Self {
        let mut buf = bytes::BytesMut::new();
        bytes::BufMut::put_u32_le(&mut buf, p.player_unique_id);
        bytes::BufMut::put_u8(&mut buf, p.item_count);
        buf.extend_from_slice(&p.tail);
        buf.freeze()
    }
}

/// 0x7082 — client → server: confirm (lock in) my side. Empty body.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct ExchangeConfirmRequest;

/// 0x7083 — client → server: approve the trade. Empty body.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct ExchangeApproveRequest;

/// 0x7084 — client → server: back out of the window.
///
/// Empty body is **inferred** — the original has no builder for this opcode, so
/// [S] rather than [V]. It matches its siblings and its ack carries only a
/// success flag, but a capture is what settles it.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct ExchangeExitRequest;

/// 0xB082 — server → client: ack for [`ExchangeConfirmRequest`].
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct ExchangeConfirmResponse {
    pub success: bool,
}

/// 0xB083 — server → client: ack for [`ExchangeApproveRequest`].
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct ExchangeApproveResponse {
    pub success: bool,
}

/// 0xB084 — server → client: ack for [`ExchangeExitRequest`].
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct ExchangeExitResponse {
    pub success: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::character_data::ItemClass;

    struct MockResolver(ItemClass);
    impl ItemClassResolver for MockResolver {
        fn item_class(&self, _ref_id: u32) -> ItemClass {
            self.0
        }
    }

    /// The window's lifecycle opcodes are empty bodies; they must round-trip to
    /// nothing rather than consuming a byte that is not there.
    #[test]
    fn the_empty_lifecycle_bodies_round_trip() {
        let empty = Bytes::new();
        assert_eq!(
            Bytes::from(ExchangePlayerConfirmed::try_from(empty.clone()).unwrap()),
            empty
        );
        assert_eq!(
            Bytes::from(ExchangeCompleted::try_from(empty.clone()).unwrap()),
            empty
        );
        assert_eq!(
            Bytes::from(ExchangeCanceled::try_from(empty.clone()).unwrap()),
            empty
        );
        assert_eq!(
            Bytes::from(ExchangeConfirmRequest::try_from(empty.clone()).unwrap()),
            empty
        );
    }

    /// 0x3088 may carry a trailing u16 reason ([U]). Decoding must tolerate one
    /// rather than failing the packet, even though the field is not surfaced.
    #[test]
    fn a_cancel_with_a_trailing_reason_still_decodes() {
        assert!(ExchangeCanceled::try_from(Bytes::from_static(&[0x07, 0x00])).is_ok());
    }

    #[test]
    fn the_started_and_gold_bodies_round_trip() {
        let wire = Bytes::from_static(&[0x80, 0xAB, 0x01, 0x00]);
        let decoded = ExchangeStarted::try_from(wire.clone()).unwrap();
        assert_eq!(decoded.partner_unique_id, 0x1AB80);
        assert_eq!(Bytes::from(decoded), wire);

        // unk byte then u64 gold
        let wire = Bytes::from_static(&[0x01, 0xE8, 0x03, 0, 0, 0, 0, 0, 0]);
        let decoded = ExchangeGoldUpdate::try_from(wire.clone()).unwrap();
        assert_eq!((decoded.unk_byte01, decoded.gold), (1, 1000));
        assert_eq!(Bytes::from(decoded), wire);
    }

    #[test]
    fn the_bool_acks_round_trip_both_ways() {
        for (byte, expected) in [(1u8, true), (0u8, false)] {
            let wire = Bytes::copy_from_slice(&[byte]);
            let decoded = ExchangeConfirmResponse::try_from(wire.clone()).unwrap();
            assert_eq!(decoded.success, expected);
            assert_eq!(Bytes::from(decoded), wire);
            assert_eq!(
                ExchangeApproveResponse::try_from(wire.clone())
                    .unwrap()
                    .success,
                expected
            );
            assert_eq!(
                ExchangeExitResponse::try_from(wire).unwrap().success,
                expected
            );
        }
    }

    /// 0x308C keeps its list as a raw tail because an item record's width is
    /// only knowable after resolving its `ref_id`. This drives the accessor
    /// with two expendable records to prove the tail really is
    /// slot-then-item-block repeated, with no exchange-slot byte between them.
    #[test]
    fn the_staged_item_list_decodes_through_the_resolver() {
        let mut body: Vec<u8> = 0x1AB80u32.to_le_bytes().to_vec();
        body.push(2); // item_count
        for (slot, stack) in [(13u8, 5u16), (14u8, 50u16)] {
            body.push(slot);
            body.extend(0u32.to_le_bytes()); // rent_type 0 -> no tail
            body.extend(11623u32.to_le_bytes()); // ref_id
            body.extend(stack.to_le_bytes()); // expendable stack_count
        }
        let wire = Bytes::from(body);

        let decoded = ExchangeItemsUpdate::try_from(wire.clone()).unwrap();
        assert_eq!(decoded.player_unique_id, 0x1AB80);
        assert_eq!(decoded.item_count, 2);

        let items = decoded
            .items(&MockResolver(ItemClass::Expendable { tid3: 0, tid4: 0 }))
            .expect("both records resolve");
        assert_eq!(items.len(), 2);
        assert_eq!((items[0].slot, items[1].slot), (13, 14));
        assert_eq!(items[1].ref_id, 11623);
        assert_eq!(Bytes::from(decoded), wire);
    }

    /// A truncated tail must yield `None` rather than a partial list — half a
    /// roster would misrepresent what the peer is actually offering.
    #[test]
    fn a_truncated_staged_list_is_none_not_partial() {
        let mut body: Vec<u8> = 1u32.to_le_bytes().to_vec();
        body.push(2); // claims two records...
        body.push(13); // ...but only one, and truncated
        body.extend(0u32.to_le_bytes());
        let decoded = ExchangeItemsUpdate::try_from(Bytes::from(body)).unwrap();

        assert!(decoded
            .items(&MockResolver(ItemClass::Expendable { tid3: 0, tid4: 0 }))
            .is_none());
    }
}
