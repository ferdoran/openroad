//! Guild storage (guild warehouse) packets: the open/close/list requests
//! 0x7250 / 0x7251 / 0x7252, the 0xB250 ack, and the three-part server push
//! 0x3253 begin / 0x3255 data / 0x3254 end.
//!
//! Idea: this family is the guild analogue of the personal warehouse in
//! [`crate::agent::storage`] — same begin/chunk/end push, same shared item
//! block — with one structural difference that decides the shape of the code
//! here: **the item stream is a reply, not a push the server volunteers.**
//! The original's 0xB250 handler (`sro_client.exe@0088f630`) stores the u32 it
//! is handed and immediately sends 0x7252 with it; the 0x3253/0x3255/0x3254
//! stream answers *that* request. So a consumer must send the list request on
//! the success arm rather than waiting for a push.
//!
//! Layouts: `docs/net-guild-storage-0x7250.md`. The three push opcodes are
//! transcribed from xBot-WinForms (`PacketParser.cs:1504-1528`) and
//! independently confirmed against the original's handlers; the two the doc
//! calls xBot-blind (0x7250 body, 0xB250 body) are read out of the original
//! client (`FUN_00820840`, `FUN_0088f630`, #266). No `packet_dump/*.log`
//! exists for any of the seven opcodes, so nothing here is capture-verified —
//! which is why every decoder degrades instead of erroring where it can.

use bevy::prelude::Message;
use bytes::{BufMut, Bytes, BytesMut};
use std::io::Cursor;

use crate::agent::character_data::{parse_item_section, InventoryItem, ItemClassResolver};

use sro_macro::ByteSize;
use sro_macro::Deserialize;
use sro_macro::SerializationError;
use sro_macro::Serialize;
use sro_macro_derive::*;

/// The guild permission bit that gates opening and using guild storage
/// (`xBot/Game/Objects/Guild/SRGuildMember.cs:35`, `[Flags] Permissions`).
pub const GUILD_PERMISSION_STORAGE: u32 = 8;

/// The one 0xB250 error code the original special-cases: guild storage is a
/// guild-wide exclusive lock, and this arm names the member holding it
/// (`FUN_0088f630`, formatted into `UIIT_MSG_GUILD_WAREHOUSE_USE`).
pub const GUILD_STORAGE_IN_USE: u16 = 0x4C48;

/// 0x7250 — client → server "open this NPC's guild storage".
///
/// One `u32` and nothing else: `FUN_00820840` writes a single 4-byte field and
/// picks the opcode from a bool, so open and close share a body. Note the
/// contrast with the personal opener 0x703C, which carries a trailing unknown
/// byte — do not copy that byte here.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct GuildStorageOpenRequest {
    pub npc_unique_id: u32,
}

/// 0x7251 — client → server "close the guild storage". Same builder, same
/// body as [`GuildStorageOpenRequest`] (`FUN_00820840` with `is_open == 0`).
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct GuildStorageCloseRequest {
    pub npc_unique_id: u32,
}

/// 0x7252 — client → server "send me the contents". The `u32` is the value the
/// 0xB250 success arm handed us, echoed back (`FUN_00820980` reads it from the
/// session object at `+0x7c8`, where the 0xB250 handler had just stored it).
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct GuildStorageListRequest {
    pub storage_id: u32,
}

/// 0xB250 — server → client ack for [`GuildStorageOpenRequest`].
///
/// `result == 1` is success and carries nothing further; anything else carries
/// a `u16` error code, and the in-use code [`GUILD_STORAGE_IN_USE`] adds the
/// holder's name. That conditional-on-a-conditional is the handler's own shape
/// (`FUN_0088f630`), not a guess.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct GuildStorageResponse {
    pub result: u8,
    #[sro_packet(when = "result != 1")]
    pub error_code: Option<u16>,
    #[sro_packet(when = "error_code == Some(GUILD_STORAGE_IN_USE)")]
    pub holder_name: Option<String>,
}

impl GuildStorageResponse {
    pub fn is_success(&self) -> bool {
        self.result == 1
    }

    /// The member currently holding the guild-wide storage lock, when the
    /// refusal was the in-use one.
    pub fn lock_holder(&self) -> Option<&str> {
        match self.error_code {
            Some(GUILD_STORAGE_IN_USE) => self.holder_name.as_deref(),
            _ => None,
        }
    }
}

/// 0x3253 — guild-storage push begin; carries the guild account's gold.
///
/// Decoded tolerantly like its personal twin [`crate::agent::storage::StorageDataBegin`]:
/// a body shorter than 8 bytes keeps everything in `tail` instead of erroring,
/// so a layout surprise from an uncaptured opcode degrades to a log line.
#[derive(Message, Clone, Debug, PartialEq)]
pub struct GuildStorageDataBegin {
    pub gold: u64,
    pub tail: Bytes,
}

impl TryFrom<Bytes> for GuildStorageDataBegin {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, SerializationError> {
        match value.get(0..8) {
            Some(head) => Ok(GuildStorageDataBegin {
                gold: u64::from_le_bytes(head.try_into().unwrap()),
                tail: value.slice(8..),
            }),
            None => Ok(GuildStorageDataBegin {
                gold: 0,
                tail: value,
            }),
        }
    }
}

impl From<GuildStorageDataBegin> for Bytes {
    fn from(p: GuildStorageDataBegin) -> Self {
        let mut buf = BytesMut::new();
        buf.put_u64_le(p.gold);
        buf.extend_from_slice(&p.tail);
        buf.freeze()
    }
}

/// 0x3255 — one chunk of the guild-storage item section. Chunks are
/// meaningless alone: they accumulate until 0x3254 says the buffer is
/// complete (`PacketParser.cs:1511-1514` appends the whole body verbatim).
#[derive(Message, Clone, Debug, PartialEq)]
pub struct GuildStorageDataChunk {
    pub raw: Bytes,
}

impl TryFrom<Bytes> for GuildStorageDataChunk {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, SerializationError> {
        Ok(GuildStorageDataChunk { raw: value })
    }
}

impl From<GuildStorageDataChunk> for Bytes {
    fn from(p: GuildStorageDataChunk) -> Self {
        p.raw
    }
}

/// 0x3254 — guild-storage push end: an empty "chunks complete, decode now"
/// marker. The original's handler reads no fields at all (`FUN_0088e690`) and
/// xBot ignores its packet argument (`PacketParser.cs:1515-1528`).
#[derive(Message, Clone, Debug, Default, PartialEq)]
pub struct GuildStorageDataEnd;

impl TryFrom<Bytes> for GuildStorageDataEnd {
    type Error = SerializationError;
    fn try_from(_: Bytes) -> Result<Self, SerializationError> {
        Ok(GuildStorageDataEnd)
    }
}

impl From<GuildStorageDataEnd> for Bytes {
    fn from(_: GuildStorageDataEnd) -> Self {
        Bytes::new()
    }
}

/// Parse the concatenated 0x3255 chunks: `capacity u8, count u8`, then `count`
/// records of `slot u8` + the shared item block. Byte-identical to the
/// personal warehouse's section, so it goes through the same parser rather
/// than a second decoder.
pub fn parse_guild_storage_items(
    raw: &[u8],
    resolver: &impl ItemClassResolver,
) -> Result<(u8, Vec<InventoryItem>), SerializationError> {
    let mut cursor = Cursor::new(raw);
    parse_item_section(&mut cursor, resolver)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::character_data::{ItemClass, ItemTypeData};

    #[test]
    fn open_and_close_share_one_body_without_the_personal_trailer() {
        let open = GuildStorageOpenRequest { npc_unique_id: 185 };
        let bytes: Bytes = open.clone().into();
        // four bytes, no trailing unknown: FUN_00820840 writes one u32
        assert_eq!(bytes.as_ref(), &[0xB9, 0x00, 0x00, 0x00]);
        assert_eq!(GuildStorageOpenRequest::try_from(bytes).unwrap(), open);

        let close = GuildStorageCloseRequest { npc_unique_id: 185 };
        let bytes: Bytes = close.clone().into();
        assert_eq!(bytes.as_ref(), &[0xB9, 0x00, 0x00, 0x00]);
        assert_eq!(GuildStorageCloseRequest::try_from(bytes).unwrap(), close);
    }

    #[test]
    fn list_request_echoes_the_storage_id() {
        let list = GuildStorageListRequest { storage_id: 0xDEAD };
        let bytes: Bytes = list.clone().into();
        assert_eq!(bytes.as_ref(), &[0xAD, 0xDE, 0x00, 0x00]);
        assert_eq!(GuildStorageListRequest::try_from(bytes).unwrap(), list);
    }

    #[test]
    fn response_reads_success_generic_error_and_the_in_use_holder() {
        // success: one byte, nothing else
        let ack = GuildStorageResponse::try_from(Bytes::from_static(&[0x01])).unwrap();
        assert!(ack.is_success());
        assert_eq!(ack.error_code, None);
        assert_eq!(ack.lock_holder(), None);

        // generic refusal: result + code, no name
        let ack = GuildStorageResponse::try_from(Bytes::from_static(&[0x02, 0x0E, 0x1C])).unwrap();
        assert!(!ack.is_success());
        assert_eq!(ack.error_code, Some(0x1C0E));
        assert_eq!(ack.holder_name, None);
        assert_eq!(ack.lock_holder(), None);

        // in-use refusal (0x4C48): the holder's name follows as strS
        let mut body = BytesMut::new();
        body.put_u8(0x02);
        body.put_u16_le(GUILD_STORAGE_IN_USE);
        body.put_u16_le(4);
        body.put_slice(b"Kong");
        let raw = body.freeze();
        let ack = GuildStorageResponse::try_from(raw.clone()).unwrap();
        assert_eq!(ack.error_code, Some(GUILD_STORAGE_IN_USE));
        assert_eq!(ack.lock_holder(), Some("Kong"));
        let back: Bytes = ack.into();
        assert_eq!(back, raw);
    }

    #[test]
    fn begin_decodes_guild_gold_tolerantly() {
        let bytes = Bytes::from_static(&[0xE8, 0x03, 0, 0, 0, 0, 0, 0]);
        let begin = GuildStorageDataBegin::try_from(bytes.clone()).unwrap();
        assert_eq!(begin.gold, 1000);
        assert!(begin.tail.is_empty());
        let back: Bytes = begin.into();
        assert_eq!(back, bytes);

        // short body: no error, everything stays in the tail
        let begin = GuildStorageDataBegin::try_from(Bytes::from_static(&[0x01, 0x02])).unwrap();
        assert_eq!(begin.gold, 0);
        assert_eq!(&begin.tail[..], &[0x01, 0x02]);
    }

    #[test]
    fn end_marker_is_empty_in_both_directions() {
        assert_eq!(
            GuildStorageDataEnd::try_from(Bytes::from_static(&[])).unwrap(),
            GuildStorageDataEnd
        );
        // the original reads no fields, so a body (if one ever arrives) is
        // ignored rather than fatal
        assert_eq!(
            GuildStorageDataEnd::try_from(Bytes::from_static(&[0xFF])).unwrap(),
            GuildStorageDataEnd
        );
        let back: Bytes = GuildStorageDataEnd.into();
        assert!(back.is_empty());
    }

    struct MockResolver;
    impl ItemClassResolver for MockResolver {
        fn item_class(&self, _ref_id: u32) -> ItemClass {
            ItemClass::Expendable { tid3: 0, tid4: 0 }
        }
    }

    #[test]
    fn guild_chunks_concatenate_into_an_item_section() {
        // capacity 60, count 1: slot 3, rent 0, ref id 4, stack 2 — split
        // across two 0x3255 packets to exercise the accumulate-then-parse flow
        let first = GuildStorageDataChunk::try_from(Bytes::from_static(&[60, 1, 3, 0, 0])).unwrap();
        let second =
            GuildStorageDataChunk::try_from(Bytes::from_static(&[0, 0, 0x04, 0, 0, 0, 0x02, 0]))
                .unwrap();
        let mut buf = Vec::new();
        buf.extend_from_slice(&first.raw);
        buf.extend_from_slice(&second.raw);

        let (capacity, items) = parse_guild_storage_items(&buf, &MockResolver).expect("parse");
        assert_eq!(capacity, 60);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].slot, 3);
        assert_eq!(items[0].ref_id, 4);
        assert_eq!(
            items[0].data,
            ItemTypeData::Expendable {
                stack_count: 2,
                assimilation_prob: None,
                mag_params: Vec::new()
            }
        );

        // a truncated body fails the parse instead of panicking
        assert!(parse_guild_storage_items(&[5, 9], &MockResolver).is_err());
    }
}
