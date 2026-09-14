//! Storage (warehouse) packets: the open request 0x703C and the three-part
//! server push 0x3047 begin / 0x3049 data / 0x3048 end.
//!
//! Layouts transcribed from xBot-WinForms (`xBot/Game/PacketBuilder.cs`
//! `RequestStorageData`, `xBot/Game/PacketParser.cs` `StorageDataBegin` /
//! `StorageData` / `StorageDataEnd`, `xBot/Network/Agent.cs` opcode table) —
//! 2026-08-08, capture-verified against the reference server the same day;
//! the 0xB03C ack and its once-per-session refusal were dump-verified
//! 2026-08-10. See `docs/net-storage-0x3047-0x3049.md`.
//!
//! The push mirrors CHARACTER_DATA's begin/body/end shape with one twist: the
//! **data can arrive as several 0x3049 packets that must be concatenated**
//! before parsing (xBot appends every chunk into one buffer and only decodes
//! it on the end marker). The concatenated body is an ordinary item section,
//! so it goes through the same resolver-driven parser as CHARACTER_DATA.

use bevy::prelude::Message;
use bytes::{BufMut, Bytes, BytesMut};
use std::io::Cursor;

use crate::agent::character_data::{parse_item_section, InventoryItem, ItemClassResolver};

use sro_macro::ByteSize;
use sro_macro::Deserialize;
use sro_macro::SerializationError;
use sro_macro::Serialize;
use sro_macro_derive::*;

/// 0x703C — client → server "open this NPC's storage". The trailing zero byte
/// is what xBot sends; its meaning is unknown (possibly a storage-kind
/// selector, given the separate guild-storage opcode 0x7250).
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct StorageDataRequest {
    pub npc_unique_id: u32,
    pub unknown: u8,
}

impl StorageDataRequest {
    pub fn new(npc_unique_id: u32) -> Self {
        Self {
            npc_unique_id,
            unknown: 0,
        }
    }
}

/// 0xB03C — server → client ack for [`StorageDataRequest`]. `result == 1` is
/// success; `result == 2` carries an error code. Capture-verified 2026-08-10:
/// the server sends the 0x3047/0x3049/0x3048 push only **once per character
/// session** — every repeat request is answered `02 0E1C` (error 0x1C0E), so
/// the client must cache the storage contents across window reopens.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct StorageDataResponse {
    pub result: u8,
    #[sro_packet(when = "result == 2")]
    pub error: Option<u16>,
}

/// 0x3047 — storage push begin; carries the storage account's gold.
///
/// Decoded tolerantly: a body shorter than 8 bytes keeps everything in `tail`
/// instead of erroring, so a layout surprise degrades to a log line.
#[derive(Message, Clone, Debug, PartialEq)]
pub struct StorageDataBegin {
    pub gold: u64,
    pub tail: Bytes,
}

impl TryFrom<Bytes> for StorageDataBegin {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, SerializationError> {
        match value.get(0..8) {
            Some(head) => Ok(StorageDataBegin {
                gold: u64::from_le_bytes(head.try_into().unwrap()),
                tail: value.slice(8..),
            }),
            None => Ok(StorageDataBegin {
                gold: 0,
                tail: value,
            }),
        }
    }
}

impl From<StorageDataBegin> for Bytes {
    fn from(p: StorageDataBegin) -> Self {
        let mut buf = BytesMut::new();
        buf.put_u64_le(p.gold);
        buf.extend_from_slice(&p.tail);
        buf.freeze()
    }
}

/// 0x3049 — one chunk of the storage item section. Chunks are meaningless
/// alone: the client appends them and parses once 0x3048 arrives.
#[derive(Message, Clone, Debug, PartialEq)]
pub struct StorageDataChunk {
    pub raw: Bytes,
}

impl TryFrom<Bytes> for StorageDataChunk {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, SerializationError> {
        Ok(StorageDataChunk { raw: value })
    }
}

impl From<StorageDataChunk> for Bytes {
    fn from(p: StorageDataChunk) -> Self {
        p.raw
    }
}

/// 0x3048 — storage push end: the accumulated chunks are now complete.
#[derive(Message, Clone, Debug, Default, PartialEq)]
pub struct StorageDataEnd;

impl TryFrom<Bytes> for StorageDataEnd {
    type Error = SerializationError;
    fn try_from(_: Bytes) -> Result<Self, SerializationError> {
        Ok(StorageDataEnd)
    }
}

impl From<StorageDataEnd> for Bytes {
    fn from(_: StorageDataEnd) -> Self {
        Bytes::new()
    }
}

/// Parse the concatenated 0x3049 chunks: `size u8, count u8`, then `count`
/// item records in the CHARACTER_DATA shape (`slot u8, rent, ref id,
/// class-dependent body` — xBot's `ItemParsing` reads exactly those fields).
pub fn parse_storage_items(
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
    fn open_request_roundtrips() {
        let request = StorageDataRequest::new(185);
        let bytes: Bytes = request.clone().into();
        assert_eq!(bytes.as_ref(), &[0xB9, 0x00, 0x00, 0x00, 0x00]);
        assert_eq!(StorageDataRequest::try_from(bytes).unwrap(), request);
    }

    #[test]
    fn data_ack_decodes_success_and_error() {
        // success ack: result 1, no error code
        let ack = StorageDataResponse::try_from(Bytes::from_static(&[0x01])).unwrap();
        assert_eq!(ack.result, 1);
        assert_eq!(ack.error, None);

        // capture 2026-08-10: repeat request within a session → 02 0E1C
        let ack = StorageDataResponse::try_from(Bytes::from_static(&[0x02, 0x0E, 0x1C])).unwrap();
        assert_eq!(ack.result, 2);
        assert_eq!(ack.error, Some(0x1C0E));
    }

    #[test]
    fn begin_decodes_gold_tolerantly() {
        let bytes = Bytes::from_static(&[0xE8, 0x03, 0, 0, 0, 0, 0, 0]);
        let begin = StorageDataBegin::try_from(bytes.clone()).unwrap();
        assert_eq!(begin.gold, 1000);
        assert!(begin.tail.is_empty());
        let back: Bytes = begin.into();
        assert_eq!(back, bytes);

        // longer body keeps the remainder raw
        let begin = StorageDataBegin::try_from(Bytes::from_static(&[1, 0, 0, 0, 0, 0, 0, 0, 0xAA]))
            .unwrap();
        assert_eq!(begin.gold, 1);
        assert_eq!(&begin.tail[..], &[0xAA]);

        // short body: no error, everything stays in the tail
        let begin = StorageDataBegin::try_from(Bytes::from_static(&[0x01, 0x02])).unwrap();
        assert_eq!(begin.gold, 0);
        assert_eq!(&begin.tail[..], &[0x01, 0x02]);
    }

    struct MockResolver;
    impl ItemClassResolver for MockResolver {
        fn item_class(&self, _ref_id: u32) -> ItemClass {
            ItemClass::Expendable { tid3: 0, tid4: 0 }
        }
    }

    #[test]
    fn chunks_concatenate_into_an_item_section() {
        // size 60, count 1: slot 3, rent 0, ref id 4, stack 2 — split across
        // two chunks to exercise the accumulate-then-parse flow
        let first = StorageDataChunk::try_from(Bytes::from_static(&[60, 1, 3, 0, 0])).unwrap();
        let second =
            StorageDataChunk::try_from(Bytes::from_static(&[0, 0, 0x04, 0, 0, 0, 0x02, 0]))
                .unwrap();
        let mut buf = Vec::new();
        buf.extend_from_slice(&first.raw);
        buf.extend_from_slice(&second.raw);

        let (size, items) = parse_storage_items(&buf, &MockResolver).expect("parse");
        assert_eq!(size, 60);
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

        // a garbage body fails the parse instead of panicking
        assert!(parse_storage_items(&[5, 9], &MockResolver).is_err());
    }
}
