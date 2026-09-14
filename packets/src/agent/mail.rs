//! Mail/memo send (0x7309) and the consignment / "avatar market" flow (0x750E
//! list request, 0xB508 register response, 0xB509 unregister response).
//!
//! **Spec-derived, not capture-verified.** No `packet_dump/` sample exists for
//! any opcode in this family. Byte-level notes, per-field [V]/[S]/[U] tags and the
//! resolving capture for each unknown live in
//! `docs/net-mail-consignment-0x7309.md`.
//!
//! Sourcing note: that doc reads the original through xBot, which is **not** on
//! this machine, so its citations cannot be re-checked here. The layouts below are
//! therefore taken from `sro-refs/SilkroadDoc-wiki`
//! (`AGENT_CONSIGNMENT_REGISTER.md`, `AGENT_CONSIGNMENT_UNREGISTER.md`,
//! `AGENT_CONSIGNMENT_LIST.md`), an independent client-binary RE that agrees with
//! the doc on every field order and width — and additionally documents a
//! `result == 2` error branch the doc omits on both responses, plus the
//! `sale_status` enum the doc marks [U]. Where they differ, the checkable source
//! wins; the divergences are recorded in the doc.
//!
//! Three siblings are deliberately **not** wired, per this issue's acceptance
//! criteria: `0xB309` (mail/memo send response), `0x7508` and `0x7509` (the
//! consignment register/unregister *requests*). See
//! `docs/protocol/opcodes.md` for each one's resolving read.

use std::io::{Cursor, Read};

use bevy::prelude::Message;
use bytes::{BufMut, Bytes, BytesMut};

use crate::agent::character_data::{InventoryItem, ItemClassResolver};

use sro_macro::ByteSize;
use sro_macro::Deserialize;
use sro_macro::SerializationError;
use sro_macro::Serialize;
use sro_macro_derive::*;

/// The SRO result convention shared by both consignment responses: 1 succeeded,
/// 2 failed and carries a `u16` error code.
pub const CONSIGNMENT_RESULT_SUCCESS: u8 = 1;
pub const CONSIGNMENT_RESULT_ERROR: u8 = 2;

/// `ConsignmentSaleStatus : byte` (SilkroadDoc-wiki `ConsignmentSaleStatus.md`).
/// The RE doc marks these semantics [U]; this resolves them. Kept as constants,
/// not a wire enum, so an unlisted value cannot fail the packet.
pub const CONSIGNMENT_SALE_REGISTERED: u8 = 0;
pub const CONSIGNMENT_SALE_SOLD: u8 = 1;
pub const CONSIGNMENT_SALE_EXPIRED: u8 = 2;
pub const CONSIGNMENT_SALE_DELETED: u8 = 255;

/// 0x750E — client → server: send me the consignment listing. Empty body.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct ConsignmentListRequest;

/// One registered-listing record in 0xB508 — a fixed 30-byte record.
#[derive(Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct ConsignmentRegisteredItem {
    /// The inventory slot the listed item left.
    pub slot_inventory: u8,
    /// See the `CONSIGNMENT_SALE_*` constants.
    pub sale_status: u8,
    /// The market-side id of the listing (the wiki's `PersonalID`).
    pub slot_consignment: u32,
    /// Ref-object id of the listed item.
    pub item_id: u32,
    pub gold_deposited: u64,
    pub gold_selling_fee: u64,
    /// Listing expiry, epoch seconds.
    pub end_date: u32,
}

/// 0xB508 — server → client: the register request's outcome, with the resulting
/// listing rows on success.
///
/// `result` is a raw byte, not a bool: the failure case carries a `u16` error
/// code, and a bool would both discard it and fail to round-trip (any non-1 byte
/// decodes as `false` and re-encodes as `0`). Same shape as `StorageDataResponse`
/// and `ItemRepairResponse`.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct ConsignmentRegisterResponse {
    pub result: u8,
    #[sro_packet(when = "result == 1")]
    pub item_count: Option<u8>,
    #[sro_packet(list_type = "by-size-field", size_field = "item_count.unwrap_or(0)")]
    pub listings: Vec<ConsignmentRegisteredItem>,
    /// `ConsignmentErrorCodes` (a `u16`; the wiki enumerates the values against
    /// client disassembly addresses). Kept raw so an unlisted code is not fatal.
    #[sro_packet(when = "result == 2")]
    pub error: Option<u16>,
}

impl ConsignmentRegisterResponse {
    pub fn succeeded(&self) -> bool {
        self.result == CONSIGNMENT_RESULT_SUCCESS
    }
}

/// One returned-item record in 0xB509: the freed market slot plus the item as it
/// goes back to inventory.
///
/// The item is the shared item body, and our [`InventoryItem`] already covers it
/// exactly — its leading `slot` **is** this record's inventory slot, and
/// everything after it is the same rent + ref-id + subtype blob the inventory and
/// storage families read. So no item parsing is written here.
#[derive(Clone, Debug, PartialEq)]
pub struct ConsignmentReturnedItem {
    /// The market-side id of the listing being cancelled (`PersonalID`).
    pub slot_consignment: u32,
    /// Destination inventory slot + the item body.
    pub item: InventoryItem,
}

/// 0xB509 — server → client: the unregister request's outcome; on success the
/// items come back to the inventory.
///
/// Hand-written with the record list kept raw. Each record's length depends on the
/// item's class, which lives in the client's itemdata tables — so decoding needs
/// an [`ItemClassResolver`] that `TryFrom<Bytes>` has no way to receive. Read the
/// records with [`Self::records`] once itemdata is available; the same deferral
/// `parse_storage_items` and `InventoryOperationResult::pickup_item` use.
#[derive(Message, Clone, Debug, PartialEq)]
pub struct ConsignmentUnregisterResponse {
    pub result: u8,
    /// Present on success.
    pub item_count: Option<u8>,
    /// Present on failure — see [`ConsignmentRegisterResponse::error`].
    pub error: Option<u16>,
    /// The `item_count` undecoded records; read them with [`Self::records`].
    pub raw: Bytes,
}

impl ConsignmentUnregisterResponse {
    pub fn succeeded(&self) -> bool {
        self.result == CONSIGNMENT_RESULT_SUCCESS
    }

    /// Decode the returned items. `None` if the records do not fit the layout.
    ///
    /// Note the resolver must actually be loaded: an unresolvable ref id is a hard
    /// error by design (its record width would be unknown), so calling this before
    /// itemdata is ready yields `None` for the whole list rather than a partial
    /// one. Keep [`Self::raw`] and retry if that happens.
    pub fn records(
        &self,
        resolver: &impl ItemClassResolver,
    ) -> Option<Vec<ConsignmentReturnedItem>> {
        let count = self.item_count?;
        let mut cursor = Cursor::new(&self.raw[..]);
        let mut records = Vec::with_capacity(count as usize);
        for _ in 0..count {
            records.push(ConsignmentReturnedItem {
                slot_consignment: u32::read_from(&mut cursor).ok()?,
                item: InventoryItem::read_with(&mut cursor, resolver).ok()?,
            });
        }
        Some(records)
    }
}

impl TryFrom<Bytes> for ConsignmentUnregisterResponse {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, SerializationError> {
        let mut cursor = Cursor::new(&value[..]);
        let result = u8::read_from(&mut cursor)?;
        let item_count = if result == CONSIGNMENT_RESULT_SUCCESS {
            Some(u8::read_from(&mut cursor)?)
        } else {
            None
        };
        let error = if result == CONSIGNMENT_RESULT_ERROR {
            Some(u16::read_from(&mut cursor)?)
        } else {
            None
        };
        let read = cursor.position() as usize;
        Ok(ConsignmentUnregisterResponse {
            result,
            item_count,
            error,
            raw: value.slice(read..),
        })
    }
}

impl From<ConsignmentUnregisterResponse> for Bytes {
    fn from(p: ConsignmentUnregisterResponse) -> Self {
        let mut buf = BytesMut::new();
        buf.put_u8(p.result);
        if let Some(count) = p.item_count {
            buf.put_u8(count);
        }
        if let Some(error) = p.error {
            buf.put_u16_le(error);
        }
        buf.extend_from_slice(&p.raw);
        buf.freeze()
    }
}

/// 0x7309 — client → server: send a mail/memo.
///
/// **Knowingly incomplete, and shaped so a caller cannot miss that.** The only
/// builder in the original writes just these two strings, and what follows them is
/// [U]. The RE doc asserts the remainder is a recipient name, a mail-type
/// discriminator and attachments (gold + item slots); that is not established —
/// SilkroadDoc-wiki names this opcode pair `AGENT_COMMUNITY_MEMO_SEND`, i.e. the
/// memo feature rather than the attachment-bearing mail system, and lists only
/// title + message. So the tail's *contents* are unknown, not merely unread.
///
/// Rather than a two-field struct that looks complete and would silently send a
/// truncated body, the remainder is a `tail` the caller must supply — a
/// verified-head-plus-raw-tail shape. (`ItemUseRequest` used to be the sibling
/// example; it became a class-discriminated enum in #454, because there the
/// classes and their tails *are* known from the original's builders.) A real send is not
/// possible from this type alone until `packet_dump/0x7309.log` resolves it.
///
/// ⚠️ `title`/`message` are user-authored free text carried as `String`, i.e.
/// UTF-8, while the wire is cp1252. Inbound, a high byte fails the packet (the
/// tree-wide derived-string property `party.rs` documents); outbound, the length
/// prefix counts UTF-8 bytes, so a non-ASCII character would ship mojibake. Both
/// are moot while nothing sends this, but neither is fixed here.
#[derive(Message, Clone, Debug, PartialEq)]
pub struct MailSendRequest {
    pub title: String,
    pub message: String,
    /// Everything after `message` — [U], pending `packet_dump/0x7309.log`.
    pub tail: Bytes,
}

/// SRO `Ascii`: u16 byte-length prefix + bytes, the framing the derive uses for
/// `String`.
fn read_ascii(cursor: &mut Cursor<&[u8]>) -> Result<String, SerializationError> {
    let len = u16::read_from(cursor)? as usize;
    let mut bytes = vec![0u8; len];
    cursor.read_exact(&mut bytes)?;
    String::from_utf8(bytes).map_err(|_| {
        SerializationError::IoError(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "0x7309 string is not valid UTF-8",
        ))
    })
}

fn put_ascii(buf: &mut BytesMut, value: &str) {
    buf.put_u16_le(value.len() as u16);
    buf.extend_from_slice(value.as_bytes());
}

impl TryFrom<Bytes> for MailSendRequest {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, SerializationError> {
        let mut cursor = Cursor::new(&value[..]);
        let title = read_ascii(&mut cursor)?;
        let message = read_ascii(&mut cursor)?;
        let read = cursor.position() as usize;
        Ok(MailSendRequest {
            title,
            message,
            tail: value.slice(read..),
        })
    }
}

impl From<MailSendRequest> for Bytes {
    fn from(p: MailSendRequest) -> Self {
        let mut buf = BytesMut::new();
        put_ascii(&mut buf, &p.title);
        put_ascii(&mut buf, &p.message);
        buf.extend_from_slice(&p.tail);
        buf.freeze()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::character_data::{ItemClass, ItemTypeData};

    struct MockResolver(ItemClass);

    impl ItemClassResolver for MockResolver {
        fn item_class(&self, _ref_id: u32) -> ItemClass {
            self.0
        }
    }

    fn expendable() -> MockResolver {
        MockResolver(ItemClass::Expendable { tid3: 1, tid4: 1 })
    }

    #[test]
    fn consignment_list_request_has_an_empty_body() {
        let wire: Bytes = ConsignmentListRequest.into();

        assert!(wire.is_empty());
        assert_eq!(
            ConsignmentListRequest::try_from(wire).unwrap(),
            ConsignmentListRequest
        );
    }

    /// One 30-byte record, the width the layout claims.
    #[test]
    fn register_response_reads_a_listing_row() {
        let mut wire = vec![CONSIGNMENT_RESULT_SUCCESS, 1];
        wire.push(7); // slot_inventory
        wire.push(CONSIGNMENT_SALE_REGISTERED);
        wire.extend_from_slice(&4321u32.to_le_bytes()); // slot_consignment
        wire.extend_from_slice(&11_000u32.to_le_bytes()); // item_id
        wire.extend_from_slice(&500_000u64.to_le_bytes()); // gold_deposited
        wire.extend_from_slice(&25_000u64.to_le_bytes()); // gold_selling_fee
        wire.extend_from_slice(&1_700_000_000u32.to_le_bytes()); // end_date

        // header(2) + one fixed record(30)
        assert_eq!(wire.len(), 32);

        let decoded = ConsignmentRegisterResponse::try_from(Bytes::from(wire.clone())).unwrap();

        assert!(decoded.succeeded());
        assert_eq!(decoded.item_count, Some(1));
        assert_eq!(decoded.error, None);
        assert_eq!(decoded.listings.len(), 1);
        let row = &decoded.listings[0];
        assert_eq!(row.slot_inventory, 7);
        assert_eq!(row.sale_status, CONSIGNMENT_SALE_REGISTERED);
        assert_eq!(row.slot_consignment, 4321);
        assert_eq!(row.item_id, 11_000);
        assert_eq!(row.gold_deposited, 500_000);
        assert_eq!(row.gold_selling_fee, 25_000);
        assert_eq!(row.end_date, 1_700_000_000);

        let back: Bytes = decoded.into();
        assert_eq!(&back[..], &wire[..]);
    }

    /// The failure case carries a u16 error code, which is why `result` is a raw
    /// byte and not a bool — a bool would drop the code and re-encode 2 as 0.
    #[test]
    fn register_response_failure_carries_the_error_code_and_roundtrips() {
        let mut wire = vec![CONSIGNMENT_RESULT_ERROR];
        wire.extend_from_slice(&0x7008u16.to_le_bytes());

        let decoded = ConsignmentRegisterResponse::try_from(Bytes::from(wire.clone())).unwrap();

        assert!(!decoded.succeeded());
        assert_eq!(decoded.error, Some(0x7008));
        assert_eq!(decoded.item_count, None);
        assert!(decoded.listings.is_empty());

        let back: Bytes = decoded.into();
        assert_eq!(&back[..], &wire[..]);
    }

    /// A success with nothing registered: the count is present but zero.
    #[test]
    fn register_response_success_with_no_rows_reads_an_empty_list() {
        let decoded = ConsignmentRegisterResponse::try_from(Bytes::from_static(&[1, 0])).unwrap();

        assert_eq!(decoded.item_count, Some(0));
        assert!(decoded.listings.is_empty());
        assert_eq!(decoded.error, None);
    }

    /// The record is `u32` + exactly one `InventoryItem`, whose leading `slot` is
    /// the destination inventory slot.
    #[test]
    fn unregister_response_reads_returned_items_through_the_resolver() {
        let mut wire = vec![CONSIGNMENT_RESULT_SUCCESS, 1];
        wire.extend_from_slice(&4321u32.to_le_bytes()); // slot_consignment
        wire.push(9); // InventoryItem::slot (the destination inventory slot)
        wire.extend_from_slice(&0u32.to_le_bytes()); // RentInfo: rent_type = None
        wire.extend_from_slice(&5000u32.to_le_bytes()); // ref_id
        wire.extend_from_slice(&40u16.to_le_bytes()); // expendable stack

        let decoded = ConsignmentUnregisterResponse::try_from(Bytes::from(wire.clone())).unwrap();

        assert!(decoded.succeeded());
        assert_eq!(decoded.item_count, Some(1));

        let records = decoded
            .records(&expendable())
            .expect("records should decode");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].slot_consignment, 4321);
        assert_eq!(records[0].item.slot, 9);
        assert_eq!(records[0].item.ref_id, 5000);
        assert_eq!(
            records[0].item.data,
            ItemTypeData::Expendable {
                stack_count: 40,
                assimilation_prob: None,
                mag_params: vec![],
            }
        );

        let back: Bytes = decoded.into();
        assert_eq!(&back[..], &wire[..]);
    }

    #[test]
    fn unregister_response_failure_carries_the_error_code_and_roundtrips() {
        let mut wire = vec![CONSIGNMENT_RESULT_ERROR];
        wire.extend_from_slice(&0x701Fu16.to_le_bytes());

        let decoded = ConsignmentUnregisterResponse::try_from(Bytes::from(wire.clone())).unwrap();

        assert!(!decoded.succeeded());
        assert_eq!(decoded.error, Some(0x701F));
        assert_eq!(decoded.item_count, None);
        assert!(decoded.records(&expendable()).is_none());

        let back: Bytes = decoded.into();
        assert_eq!(&back[..], &wire[..]);
    }

    /// A truncated record must not yield half a list — a short record shifts every
    /// later one.
    #[test]
    fn a_truncated_returned_item_list_is_none_not_partial() {
        let mut wire = vec![CONSIGNMENT_RESULT_SUCCESS, 2]; // claims two, supplies one
        wire.extend_from_slice(&4321u32.to_le_bytes());
        wire.push(9);
        wire.extend_from_slice(&0u32.to_le_bytes());
        wire.extend_from_slice(&5000u32.to_le_bytes());
        wire.extend_from_slice(&40u16.to_le_bytes());

        let decoded = ConsignmentUnregisterResponse::try_from(Bytes::from(wire)).unwrap();

        assert_eq!(decoded.item_count, Some(2));
        assert!(decoded.records(&expendable()).is_none());
    }

    /// An unresolvable ref id is a hard error by design (the record width would be
    /// unknown), so an unloaded itemdata table costs the whole list, not a partial
    /// one. The raw bytes survive for a retry.
    #[test]
    fn returned_items_are_none_while_itemdata_is_unresolvable() {
        let mut wire = vec![CONSIGNMENT_RESULT_SUCCESS, 1];
        wire.extend_from_slice(&4321u32.to_le_bytes());
        wire.push(9);
        wire.extend_from_slice(&0u32.to_le_bytes());
        wire.extend_from_slice(&5000u32.to_le_bytes());
        wire.extend_from_slice(&40u16.to_le_bytes());

        let decoded = ConsignmentUnregisterResponse::try_from(Bytes::from(wire)).unwrap();

        assert!(decoded.records(&MockResolver(ItemClass::Unknown)).is_none());
        assert!(!decoded.raw.is_empty());
    }

    #[test]
    fn mail_send_request_roundtrips_its_verified_head() {
        let req = MailSendRequest {
            title: "Hello".to_string(),
            message: "Some text".to_string(),
            tail: Bytes::new(),
        };
        let wire: Bytes = req.clone().into();

        // 2+5 title, 2+9 message
        assert_eq!(wire.len(), 18);
        assert_eq!(MailSendRequest::try_from(wire).unwrap(), req);
    }

    /// Everything past `message` is preserved rather than dropped, so a future
    /// capture can be decoded without losing the unknown tail.
    #[test]
    fn mail_send_request_preserves_the_unverified_tail() {
        let mut wire = 1u16.to_le_bytes().to_vec();
        wire.push(b'a');
        wire.extend_from_slice(&1u16.to_le_bytes());
        wire.push(b'b');
        wire.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);

        let decoded = MailSendRequest::try_from(Bytes::from(wire.clone())).unwrap();

        assert_eq!(decoded.title, "a");
        assert_eq!(decoded.message, "b");
        assert_eq!(&decoded.tail[..], &[0xDE, 0xAD, 0xBE, 0xEF]);

        let back: Bytes = decoded.into();
        assert_eq!(&back[..], &wire[..]);
    }

    #[test]
    fn mail_send_request_rejects_a_truncated_head() {
        assert!(MailSendRequest::try_from(Bytes::from_static(&[5, 0, b'x'])).is_err());
    }
}
