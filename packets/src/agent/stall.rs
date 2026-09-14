//! Player stall / private shop ("grocery stall"): the owner's create/destroy and
//! edit requests, the viewer's enter/buy/leave path, and the entity pushes that
//! show a stall in the world.
//!
//! **Spec-derived, not capture-verified.** No `packet_dump/` sample exists for any
//! opcode in this family. Byte-level notes, per-field [V]/[S]/[U] tags and the
//! resolving capture for each unknown live in `docs/net-stall-0x30B7.md`.
//!
//! Sourcing note: the doc reads the original through xBot, which is **not** on this
//! machine, so those citations cannot be re-checked here. Two sources that are on
//! disk carried this instead — the go-sro agent server
//! (`sro-refs/go-sro-agent-server/handler/stall/`), which the doc does cite, and
//! `sro-refs/SilkroadDoc-wiki` (`AGENT_STALL_*.md`, `StallAction.md`,
//! `StallUpdateType.md`, `StallErrorCode.md`), which it does not. Several fields
//! here are `[S]` in the doc — go-sro emits them while xBot has them commented out;
//! those are wired, because dropping a field the server sends desyncs everything
//! after it. The wiki additionally supplies four bodies the doc marks `[U]`; each
//! is noted at its type.
//!
//! All 14 opcodes in scope are wired. The `0x70B3`/`0xB0B3` stall-talk pair is out
//! of scope (it belongs to a separate doc — note that the cross-reference in
//! `docs/net-stall-0x30B7.md` currently points at a section carrying no body).
//!
//! ## Why the item-bearing pushes keep a raw tail
//!
//! Stall inventory streams as a **0xFF-sentinel-terminated** list — the row's
//! leading stall-slot byte doubles as the terminator — and each row embeds the
//! shared item body, whose width depends on the client's itemdata tables. The
//! derive has no sentinel list mode, and `Deserialize` cannot take an
//! `ItemClassResolver`, so `0xB0BA` and `0x30B7` keep their rows raw behind
//! resolver-taking accessors, as `parse_storage_items` and
//! `InventoryOperationResult::pickup_item` already do.

use std::io::Cursor;

use bevy::prelude::Message;
use bytes::{BufMut, Bytes, BytesMut};

use crate::agent::character_data::{InventoryItem, ItemClassResolver};

use sro_macro::ByteSize;
use sro_macro::Deserialize;
use sro_macro::SerializationError;
use sro_macro::Serialize;
use sro_macro_derive::*;

/// `SRTypes.StallUpdate` — what a 0x70BA / 0xB0BA carries.
pub const STALL_UPDATE_ITEM_UPDATE: u8 = 1;
pub const STALL_UPDATE_ITEM_ADDED: u8 = 2;
pub const STALL_UPDATE_ITEM_REMOVED: u8 = 3;
/// Marked unused in the original's enum and an empty case in go-sro, but the wiki
/// gives it a one-byte body on both directions.
pub const STALL_UPDATE_FLEA_MARKET_MODE: u8 = 4;
pub const STALL_UPDATE_STATE: u8 = 5;
pub const STALL_UPDATE_NOTE: u8 = 6;
pub const STALL_UPDATE_TITLE: u8 = 7;

/// 0x30B7's leading action byte.
pub const STALL_ACTION_EXIT: u8 = 1;
pub const STALL_ACTION_ENTER: u8 = 2;
pub const STALL_ACTION_BUY: u8 = 3;

/// Terminates a stall item list, in the row's stall-slot position.
const STALL_ROW_SENTINEL: u8 = 0xFF;

// --- C→S requests ---------------------------------------------------------

/// 0x70B1 — open a stall with this title.
///
/// The original caps the title at 63 characters, which is client-side input
/// policy rather than wire framing, so it is not enforced here.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct StallCreateRequest {
    pub title: String,
}

/// 0x70B2 — close the stall. Empty body.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct StallDestroyRequest;

/// 0x70B4 — buy the item in `stall_slot` from the stall being viewed.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct StallBuyRequest {
    pub stall_slot: u8,
}

/// 0x70B5 — leave the stall being viewed. Empty body.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct StallLeaveRequest;

/// 0x70BA — the owner edits the stall, discriminated by a leading type byte.
///
/// A derived enum is safe here where it would not be for a server push: this is
/// client→server only, so the client only ever *constructs* it and an unmatched
/// tag — which would fail decoding — cannot arise. Same shape as
/// `InventoryOperationRequest`.
///
/// The trailing `unknown0` is present on the three item types and on State, and
/// **absent** on Note and Title — adding it there would desync those two.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub enum StallUpdateRequest {
    #[sro_packet(value = 1)]
    ItemUpdate {
        stall_slot: u8,
        quantity: u16,
        price: u64,
        unknown0: u16,
    },
    #[sro_packet(value = 2)]
    ItemAdded {
        stall_slot: u8,
        inventory_slot: u8,
        quantity: u16,
        price: u64,
        /// Written as a literal `1` by the original.
        flea_market_tid_group: u32,
        unknown0: u16,
    },
    #[sro_packet(value = 3)]
    ItemRemoved { stall_slot: u8, unknown0: u16 },
    /// Values 1 and 2 are accepted with no observable effect; anything above 3 is
    /// refused with error `0x3C2B`.
    #[sro_packet(value = 4)]
    FleaMarketMode { mode: u8 },
    #[sro_packet(value = 5)]
    State { is_open: u8, unknown0: u16 },
    #[sro_packet(value = 6)]
    Note { note: String },
    #[sro_packet(value = 7)]
    Title { title: String },
}

// --- S→C simple responses -------------------------------------------------

/// 0xB0B1 — create ack. On success the server also pushes a separate 0x30B8.
///
/// `result` is a raw byte with a conditional error tail, not a bool: the failure
/// case carries a `u16` error code (`StallErrorCode`). The RE doc has no row for
/// that branch — go-sro cannot reveal it because its handler only ever writes
/// `1` — but SilkroadDoc-wiki documents it on all three simple acks. Modelling
/// this as a bare success byte would truncate every real error. Same shape as
/// `StorageDataResponse`.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct StallCreateResponse {
    pub result: u8,
    #[sro_packet(when = "result == 2")]
    pub error_code: Option<u16>,
}

/// 0xB0B2 — destroy ack. On success the server also pushes a separate 0x30B9.
/// See [`StallCreateResponse`] for the `result` shape.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct StallDestroyResponse {
    pub result: u8,
    #[sro_packet(when = "result == 2")]
    pub error_code: Option<u16>,
}

/// 0xB0B5 — leave ack. See [`StallCreateResponse`] for the `result` shape.
///
/// That leading byte is [S] in the RE doc — the original's parser reads nothing at
/// all and just fires its callback, while go-sro emits it. Wired because a byte the
/// server sends must be consumed; `packet_dump/0xB0B5.log` settles it.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct StallLeaveResponse {
    pub result: u8,
    #[sro_packet(when = "result == 2")]
    pub error_code: Option<u16>,
}

/// 0xB0B4 — buy ack.
///
/// The RE doc calls this a fully commented-out stub and defers it as [U]. It is
/// documented after all: SilkroadDoc-wiki and the silkroad-docs corpus agree
/// independently on `result`, then the bought slot on success or a `u16` error code
/// otherwise (e.g. `15406` stall is not open). Note the failure test is `!= 1`
/// here, not `== 2` — that is how the source branches it, and the two differ for
/// any other result value.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct StallBuyResponse {
    pub result: u8,
    #[sro_packet(when = "result == 1")]
    pub stall_slot: Option<u8>,
    #[sro_packet(when = "result != 1")]
    pub error_code: Option<u16>,
}

// --- S→C entity pushes ----------------------------------------------------

/// 0x30B8 — a stall appeared on `unique_id`.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct EntityStallCreate {
    pub unique_id: u32,
    pub title: String,
    /// The stall's avatar/model. go-sro's default is `0`.
    pub decoration_id: u32,
}

/// 0x30B9 — the stall on `unique_id` is gone.
///
/// `error_code` is [S]: the original comments the trailing `u16` out, go-sro emits
/// it (as `0`). `packet_dump/0x30B9.log` settles the width and values.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct EntityStallDestroy {
    pub unique_id: u32,
    pub error_code: u16,
}

/// 0x30BB — the stall on `unique_id` was renamed.
///
/// Title edits arrive here, not on 0xB0BA — go-sro routes them to this opcode and
/// the original's 0xB0BA title branch reads no text.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct EntityStallTitleUpdate {
    pub unique_id: u32,
    pub title: String,
}

// --- Stall item rows ------------------------------------------------------

/// One row of a stall's inventory listing.
///
/// The row's leading stall-slot byte plus the shared item body is exactly our
/// [`InventoryItem`] — its `slot` field holds the **stall** slot here — so no item
/// parsing is written for this family.
#[derive(Clone, Debug, PartialEq)]
pub struct StallItemRow {
    /// `item.slot` is the stall slot; the rest is the shared item body.
    pub item: InventoryItem,
    /// Where the item sits in the owner's inventory.
    pub inventory_slot: u8,
    pub quantity: u16,
    pub price: u64,
}

/// SRO `ascii`: u16 byte-length prefix + bytes, the framing the derive uses for
/// `String`. Needed because these two packets are hand-written.
fn read_ascii(cursor: &mut Cursor<&[u8]>) -> Result<String, SerializationError> {
    let len = u16::read_from(cursor)? as usize;
    let mut bytes = vec![0u8; len];
    std::io::Read::read_exact(cursor, &mut bytes)?;
    String::from_utf8(bytes).map_err(|_| {
        SerializationError::IoError(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "stall string is not valid UTF-8",
        ))
    })
}

fn put_ascii(buf: &mut BytesMut, value: &str) {
    buf.put_u16_le(value.len() as u16);
    buf.extend_from_slice(value.as_bytes());
}

/// Read rows until the 0xFF sentinel. `None` on any malformed row — a short row
/// shifts every later one, so a partial list is worse than none.
fn read_rows(
    cursor: &mut Cursor<&[u8]>,
    resolver: &impl ItemClassResolver,
) -> Option<Vec<StallItemRow>> {
    let mut rows = Vec::new();
    loop {
        let position = cursor.position() as usize;
        // Peek the slot byte: it doubles as the list terminator, so it must not be
        // consumed unless it really is the sentinel.
        if *cursor.get_ref().get(position)? == STALL_ROW_SENTINEL {
            cursor.set_position(position as u64 + 1);
            return Some(rows);
        }
        rows.push(StallItemRow {
            item: InventoryItem::read_with(cursor, resolver).ok()?,
            inventory_slot: u8::read_from(cursor).ok()?,
            quantity: u16::read_from(cursor).ok()?,
            price: u64::read_from(cursor).ok()?,
        });
    }
}

/// Decode a 0xFF-terminated row list held as raw bytes.
fn decode_rows(raw: &Bytes, resolver: &impl ItemClassResolver) -> Option<Vec<StallItemRow>> {
    read_rows(&mut Cursor::new(&raw[..]), resolver)
}

// --- 0x30B7 SERVER_STALL_ENTITY_ACTION ------------------------------------

/// 0x30B7 — something happened at a stall: a viewer entered or left, or a purchase
/// went through.
///
/// Hand-written: the buy arm ends in a sentinel-terminated item list, which no
/// derive list mode can express.
///
/// The RE doc marks the enter/exit bodies [U] because the original comments the
/// trailing id out. SilkroadDoc-wiki documents both as carrying `u32 UniqueID`, so
/// they are wired rather than left blind.
#[derive(Message, Clone, Debug, PartialEq)]
pub enum StallEntityAction {
    /// A viewer left the stall.
    Exit { unique_id: u32 },
    /// A viewer entered the stall.
    Enter { unique_id: u32 },
    /// A purchase completed; the rows are the stall's remaining listing.
    Buy {
        stall_slot: u8,
        buyer_name: String,
        /// The 0xFF-terminated rows, undecoded — read with [`Self::rows`].
        raw_rows: Bytes,
    },
    /// An action code neither source describes; the bytes are kept intact.
    Unknown { action: u8, tail: Bytes },
}

impl StallEntityAction {
    /// Decode the buy arm's item rows. `None` for the other arms, and on any
    /// malformed row.
    pub fn rows(&self, resolver: &impl ItemClassResolver) -> Option<Vec<StallItemRow>> {
        match self {
            StallEntityAction::Buy { raw_rows, .. } => decode_rows(raw_rows, resolver),
            _ => None,
        }
    }
}

impl TryFrom<Bytes> for StallEntityAction {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, SerializationError> {
        let mut cursor = Cursor::new(&value[..]);
        let action = u8::read_from(&mut cursor)?;
        match action {
            STALL_ACTION_EXIT => Ok(StallEntityAction::Exit {
                unique_id: u32::read_from(&mut cursor)?,
            }),
            STALL_ACTION_ENTER => Ok(StallEntityAction::Enter {
                unique_id: u32::read_from(&mut cursor)?,
            }),
            STALL_ACTION_BUY => {
                let stall_slot = u8::read_from(&mut cursor)?;
                let buyer_name = read_ascii(&mut cursor)?;
                let read = cursor.position() as usize;
                Ok(StallEntityAction::Buy {
                    stall_slot,
                    buyer_name,
                    raw_rows: value.slice(read..),
                })
            }
            _ => Ok(StallEntityAction::Unknown {
                action,
                tail: value.slice(1..),
            }),
        }
    }
}

impl From<StallEntityAction> for Bytes {
    fn from(p: StallEntityAction) -> Self {
        let mut buf = BytesMut::new();
        match p {
            StallEntityAction::Exit { unique_id } => {
                buf.put_u8(STALL_ACTION_EXIT);
                buf.put_u32_le(unique_id);
            }
            StallEntityAction::Enter { unique_id } => {
                buf.put_u8(STALL_ACTION_ENTER);
                buf.put_u32_le(unique_id);
            }
            StallEntityAction::Buy {
                stall_slot,
                buyer_name,
                raw_rows,
            } => {
                buf.put_u8(STALL_ACTION_BUY);
                buf.put_u8(stall_slot);
                put_ascii(&mut buf, &buyer_name);
                buf.extend_from_slice(&raw_rows);
            }
            StallEntityAction::Unknown { action, tail } => {
                buf.put_u8(action);
                buf.extend_from_slice(&tail);
            }
        }
        buf.freeze()
    }
}

// --- 0xB0B3 SERVER_STALL_TALK_RESPONSE ------------------------------------

/// `result == 1` on the two response opcodes of this family; anything else is
/// followed by a `StallErrorCode` u16.
const STALL_RESULT_OK: u8 = 1;

/// 0xB0B3 — the snapshot a viewer receives on entering a remote stall (#759).
///
/// **Sourcing.** The body is the one `docs/net-stall-0x30B7.md` §7 records as
/// existing but never inlines: *"result, uid u32, message, isOpen,
/// fleaMarketMode, the 0xFF-terminated rows, then `peopleCount u8` + uid
/// array"* (wiki `AGENT_STALL_TALK.md`). Field **order** is that record; the
/// two flag widths are `[S]` — a one-byte flag is what `fleaMarketMode`
/// already is on 0x70BA/0xB0BA (§2), and no source states otherwise. The
/// failure arm follows the family's own shape (`u16` error code, as on
/// 0xB0B1/0xB0B2/0xB0B5).
///
/// Hand-written for the same reason as [`StallEntityAction`]: the row list is
/// sentinel-terminated **and** something follows it, so the tail cannot be
/// decoded without the item resolver. [`Self::snapshot`] does that in one pass.
#[derive(Message, Clone, Debug, PartialEq)]
pub enum StallTalkResponse {
    /// The stall opened for us.
    Success {
        /// The stall owner's entity.
        unique_id: u32,
        /// The owner's greeting ("note").
        message: String,
        /// Whether the stall is open for business (`[S]` u8).
        is_open: bool,
        /// The `FleaMarketMode` byte of §2 (`[S]` u8 here).
        flea_market_mode: u8,
        /// The 0xFF-terminated rows **plus** the trailing viewer list,
        /// undecoded — read with [`Self::snapshot`].
        raw_tail: Bytes,
    },
    /// `StallErrorCode` (see `docs/net-stall-0x30B7.md` §7's error list). The
    /// `result` byte is kept verbatim rather than normalised to 2: the family's
    /// other acks show only that `!= 1` means failure, so re-encoding some
    /// other value as 2 would invent a discrimination the sources do not make.
    Failure { result: u8, error_code: u16 },
}

/// What a successful 0xB0B3 carries after its header: the listing and who else
/// is looking at it.
#[derive(Clone, Debug, PartialEq)]
pub struct StallSnapshot {
    pub rows: Vec<StallItemRow>,
    /// `peopleCount u8` followed by that many `u32` unique ids.
    pub viewers: Vec<u32>,
}

impl StallTalkResponse {
    /// Decode the tail: the item rows, then the viewer list. `None` on any
    /// malformed row — a short row shifts the viewer count too, so a partial
    /// answer would be worse than none.
    pub fn snapshot(&self, resolver: &impl ItemClassResolver) -> Option<StallSnapshot> {
        let StallTalkResponse::Success { raw_tail, .. } = self else {
            return None;
        };
        let mut cursor = Cursor::new(&raw_tail[..]);
        let rows = read_rows(&mut cursor, resolver)?;
        let count = u8::read_from(&mut cursor).ok()? as usize;
        let mut viewers = Vec::with_capacity(count);
        for _ in 0..count {
            viewers.push(u32::read_from(&mut cursor).ok()?);
        }
        Some(StallSnapshot { rows, viewers })
    }
}

impl TryFrom<Bytes> for StallTalkResponse {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, SerializationError> {
        let mut cursor = Cursor::new(&value[..]);
        let result = u8::read_from(&mut cursor)?;
        if result != STALL_RESULT_OK {
            return Ok(StallTalkResponse::Failure {
                result,
                error_code: u16::read_from(&mut cursor)?,
            });
        }
        let unique_id = u32::read_from(&mut cursor)?;
        let message = read_ascii(&mut cursor)?;
        let is_open = u8::read_from(&mut cursor)? != 0;
        let flea_market_mode = u8::read_from(&mut cursor)?;
        let read = cursor.position() as usize;
        Ok(StallTalkResponse::Success {
            unique_id,
            message,
            is_open,
            flea_market_mode,
            raw_tail: value.slice(read..),
        })
    }
}

impl From<StallTalkResponse> for Bytes {
    fn from(p: StallTalkResponse) -> Self {
        let mut buf = BytesMut::new();
        match p {
            StallTalkResponse::Success {
                unique_id,
                message,
                is_open,
                flea_market_mode,
                raw_tail,
            } => {
                buf.put_u8(STALL_RESULT_OK);
                buf.put_u32_le(unique_id);
                put_ascii(&mut buf, &message);
                buf.put_u8(u8::from(is_open));
                buf.put_u8(flea_market_mode);
                buf.extend_from_slice(&raw_tail);
            }
            StallTalkResponse::Failure { result, error_code } => {
                buf.put_u8(result);
                buf.put_u16_le(error_code);
            }
        }
        buf.freeze()
    }
}

// --- 0xB0BA SERVER_STALL_UPDATE_RESPONSE ----------------------------------

/// The per-type body of a 0xB0BA.
#[derive(Clone, Debug, PartialEq)]
pub enum StallUpdateAck {
    /// Type 1. The trailing `error_code` is [S] — commented out in the original,
    /// emitted by go-sro.
    ItemUpdate {
        stall_slot: u8,
        quantity: u16,
        price: u64,
        error_code: u16,
    },
    /// Types 2 (added) and 3 (removed) — which of the two is in `update_type`.
    /// The rows are the stall's whole listing after the edit.
    ItemList {
        error_code: u16,
        /// The 0xFF-terminated rows, undecoded — read with
        /// [`StallUpdateResponse::rows`].
        raw_rows: Bytes,
    },
    /// Type 4.
    FleaMarketMode { mode: u8 },
    /// Type 5. `stall_network_result` is the value the client sent, echoed back.
    State {
        is_open: u8,
        stall_network_result: u16,
    },
    /// Type 6.
    Note { note: String },
    /// Type 7 — no payload: title edits arrive on 0x30BB instead.
    Title,
    /// An update type neither source describes; the bytes are kept intact.
    Unknown { tail: Bytes },
}

/// 0xB0BA — the owner's stall edit was applied (or refused).
///
/// Hand-written for the same reason as [`StallEntityAction`]: the add/remove arms
/// end in a sentinel-terminated item list.
#[derive(Message, Clone, Debug, PartialEq)]
pub struct StallUpdateResponse {
    pub result: u8,
    /// See the `STALL_UPDATE_*` constants.
    pub update_type: u8,
    pub body: StallUpdateAck,
}

impl StallUpdateResponse {
    /// Decode the add/remove arms' item rows. `None` for the other types, and on
    /// any malformed row.
    pub fn rows(&self, resolver: &impl ItemClassResolver) -> Option<Vec<StallItemRow>> {
        match &self.body {
            StallUpdateAck::ItemList { raw_rows, .. } => decode_rows(raw_rows, resolver),
            _ => None,
        }
    }
}

impl TryFrom<Bytes> for StallUpdateResponse {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, SerializationError> {
        let mut cursor = Cursor::new(&value[..]);
        let result = u8::read_from(&mut cursor)?;
        let update_type = u8::read_from(&mut cursor)?;
        let body = match update_type {
            STALL_UPDATE_ITEM_UPDATE => StallUpdateAck::ItemUpdate {
                stall_slot: u8::read_from(&mut cursor)?,
                quantity: u16::read_from(&mut cursor)?,
                price: u64::read_from(&mut cursor)?,
                error_code: u16::read_from(&mut cursor)?,
            },
            STALL_UPDATE_ITEM_ADDED | STALL_UPDATE_ITEM_REMOVED => {
                let error_code = u16::read_from(&mut cursor)?;
                let read = cursor.position() as usize;
                StallUpdateAck::ItemList {
                    error_code,
                    raw_rows: value.slice(read..),
                }
            }
            STALL_UPDATE_FLEA_MARKET_MODE => StallUpdateAck::FleaMarketMode {
                mode: u8::read_from(&mut cursor)?,
            },
            STALL_UPDATE_STATE => StallUpdateAck::State {
                is_open: u8::read_from(&mut cursor)?,
                stall_network_result: u16::read_from(&mut cursor)?,
            },
            STALL_UPDATE_NOTE => StallUpdateAck::Note {
                note: read_ascii(&mut cursor)?,
            },
            STALL_UPDATE_TITLE => StallUpdateAck::Title,
            _ => StallUpdateAck::Unknown {
                tail: value.slice(2..),
            },
        };
        Ok(StallUpdateResponse {
            result,
            update_type,
            body,
        })
    }
}

impl From<StallUpdateResponse> for Bytes {
    fn from(p: StallUpdateResponse) -> Self {
        let mut buf = BytesMut::new();
        buf.put_u8(p.result);
        buf.put_u8(p.update_type);
        match p.body {
            StallUpdateAck::ItemUpdate {
                stall_slot,
                quantity,
                price,
                error_code,
            } => {
                buf.put_u8(stall_slot);
                buf.put_u16_le(quantity);
                buf.put_u64_le(price);
                buf.put_u16_le(error_code);
            }
            StallUpdateAck::ItemList {
                error_code,
                raw_rows,
            } => {
                buf.put_u16_le(error_code);
                buf.extend_from_slice(&raw_rows);
            }
            StallUpdateAck::FleaMarketMode { mode } => buf.put_u8(mode),
            StallUpdateAck::State {
                is_open,
                stall_network_result,
            } => {
                buf.put_u8(is_open);
                buf.put_u16_le(stall_network_result);
            }
            StallUpdateAck::Note { note } => put_ascii(&mut buf, &note),
            StallUpdateAck::Title => {}
            StallUpdateAck::Unknown { tail } => buf.extend_from_slice(&tail),
        }
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

    fn ascii(s: &str) -> Vec<u8> {
        let mut out = (s.len() as u16).to_le_bytes().to_vec();
        out.extend_from_slice(s.as_bytes());
        out
    }

    /// One stall row: the leading byte is the STALL slot (it becomes
    /// `InventoryItem::slot`), and the inventory slot is the trailing byte.
    fn row(stall_slot: u8, inventory_slot: u8, quantity: u16, price: u64) -> Vec<u8> {
        let mut out = vec![stall_slot];
        out.extend_from_slice(&0u32.to_le_bytes()); // RentInfo: rent_type = None
        out.extend_from_slice(&5000u32.to_le_bytes()); // ref_id
        out.extend_from_slice(&1u16.to_le_bytes()); // expendable stack, inside the item body
        out.push(inventory_slot);
        out.extend_from_slice(&quantity.to_le_bytes());
        out.extend_from_slice(&price.to_le_bytes());
        out
    }

    // --- C→S ---------------------------------------------------------------

    #[test]
    fn create_request_carries_just_the_title() {
        let req = StallCreateRequest {
            title: "Cheap gear".to_string(),
        };
        let wire: Bytes = req.clone().into();

        assert_eq!(&wire[..], &ascii("Cheap gear")[..]);
        assert_eq!(StallCreateRequest::try_from(wire).unwrap(), req);
    }

    #[test]
    fn destroy_and_leave_requests_are_empty() {
        let destroy: Bytes = StallDestroyRequest.into();
        let leave: Bytes = StallLeaveRequest.into();

        assert!(destroy.is_empty());
        assert!(leave.is_empty());
    }

    #[test]
    fn buy_request_is_a_single_slot_byte() {
        let wire: Bytes = StallBuyRequest { stall_slot: 4 }.into();

        assert_eq!(&wire[..], &[4]);
    }

    /// Each update type round-trips at its own length — the trailing `unknown0` is
    /// on the item types and State, and must NOT be on Note or Title.
    #[test]
    fn every_update_request_variant_roundtrips() {
        let cases: Vec<(StallUpdateRequest, usize)> = vec![
            (
                StallUpdateRequest::ItemUpdate {
                    stall_slot: 1,
                    quantity: 5,
                    price: 900,
                    unknown0: 0,
                },
                1 + 1 + 2 + 8 + 2,
            ),
            (
                StallUpdateRequest::ItemAdded {
                    stall_slot: 1,
                    inventory_slot: 13,
                    quantity: 5,
                    price: 900,
                    flea_market_tid_group: 1,
                    unknown0: 0,
                },
                1 + 1 + 1 + 2 + 8 + 4 + 2,
            ),
            (
                StallUpdateRequest::ItemRemoved {
                    stall_slot: 1,
                    unknown0: 0,
                },
                1 + 1 + 2,
            ),
            (StallUpdateRequest::FleaMarketMode { mode: 1 }, 1 + 1),
            (
                StallUpdateRequest::State {
                    is_open: 1,
                    unknown0: 0,
                },
                1 + 1 + 2,
            ),
            (
                StallUpdateRequest::Note {
                    note: "hi".to_string(),
                },
                1 + 2 + 2,
            ),
            (
                StallUpdateRequest::Title {
                    title: "hi".to_string(),
                },
                1 + 2 + 2,
            ),
        ];

        for (req, expected_len) in cases {
            let wire: Bytes = req.clone().into();
            assert_eq!(wire.len(), expected_len, "wrong length for {req:?}");
            assert_eq!(StallUpdateRequest::try_from(wire).unwrap(), req);
        }
    }

    // --- S→C simple acks ---------------------------------------------------

    /// The three simple acks share a shape the RE doc has no row for: a failure
    /// carries a u16 error code.
    #[test]
    fn simple_acks_read_the_error_code_only_on_failure() {
        let ok = StallCreateResponse::try_from(Bytes::from_static(&[1])).unwrap();
        assert_eq!(ok.result, 1);
        assert_eq!(ok.error_code, None);

        let mut wire = vec![2u8];
        wire.extend_from_slice(&0x3C2Bu16.to_le_bytes());
        let failed = StallCreateResponse::try_from(Bytes::from(wire.clone())).unwrap();
        assert_eq!(failed.error_code, Some(0x3C2B));
        let back: Bytes = failed.into();
        assert_eq!(&back[..], &wire[..]);

        // Destroy and leave use the identical shape.
        assert_eq!(
            StallDestroyResponse::try_from(Bytes::from(wire.clone()))
                .unwrap()
                .error_code,
            Some(0x3C2B)
        );
        assert_eq!(
            StallLeaveResponse::try_from(Bytes::from(wire))
                .unwrap()
                .error_code,
            Some(0x3C2B)
        );
    }

    /// 0xB0B4 branches on `!= 1`, not `== 2`.
    #[test]
    fn buy_response_carries_the_slot_on_success_and_an_error_otherwise() {
        let ok = StallBuyResponse::try_from(Bytes::from_static(&[1, 6])).unwrap();
        assert_eq!(ok.stall_slot, Some(6));
        assert_eq!(ok.error_code, None);

        let mut wire = vec![2u8];
        wire.extend_from_slice(&15406u16.to_le_bytes());
        let failed = StallBuyResponse::try_from(Bytes::from(wire.clone())).unwrap();
        assert_eq!(failed.stall_slot, None);
        assert_eq!(failed.error_code, Some(15406));
        let back: Bytes = failed.into();
        assert_eq!(&back[..], &wire[..]);
    }

    // --- S→C entity pushes -------------------------------------------------

    #[test]
    fn entity_stall_create_reads_title_and_decoration() {
        let mut wire = 777u32.to_le_bytes().to_vec();
        wire.extend_from_slice(&ascii("Wares"));
        wire.extend_from_slice(&3847u32.to_le_bytes());

        let decoded = EntityStallCreate::try_from(Bytes::from(wire.clone())).unwrap();

        assert_eq!(decoded.unique_id, 777);
        assert_eq!(decoded.title, "Wares");
        assert_eq!(decoded.decoration_id, 3847);
        let back: Bytes = decoded.into();
        assert_eq!(&back[..], &wire[..]);
    }

    /// The trailing u16 is [S] — go-sro emits it, xBot comments it out. Dropping it
    /// would leave two bytes unconsumed.
    #[test]
    fn entity_stall_destroy_consumes_the_trailing_error_code() {
        let mut wire = 777u32.to_le_bytes().to_vec();
        wire.extend_from_slice(&0u16.to_le_bytes());

        let decoded = EntityStallDestroy::try_from(Bytes::from(wire.clone())).unwrap();

        assert_eq!(decoded.unique_id, 777);
        assert_eq!(decoded.error_code, 0);
        let back: Bytes = decoded.into();
        assert_eq!(&back[..], &wire[..]);
    }

    #[test]
    fn entity_stall_title_update_roundtrips() {
        let mut wire = 777u32.to_le_bytes().to_vec();
        wire.extend_from_slice(&ascii("Renamed"));

        let decoded = EntityStallTitleUpdate::try_from(Bytes::from(wire.clone())).unwrap();

        assert_eq!(decoded.title, "Renamed");
        let back: Bytes = decoded.into();
        assert_eq!(&back[..], &wire[..]);
    }

    // --- 0x30B7 ------------------------------------------------------------

    /// Enter/exit carry a viewer id, which the RE doc marks [U].
    #[test]
    fn entity_action_enter_and_exit_carry_the_viewer_id() {
        let mut enter = vec![STALL_ACTION_ENTER];
        enter.extend_from_slice(&4242u32.to_le_bytes());
        assert_eq!(
            StallEntityAction::try_from(Bytes::from(enter)).unwrap(),
            StallEntityAction::Enter { unique_id: 4242 }
        );

        let mut exit = vec![STALL_ACTION_EXIT];
        exit.extend_from_slice(&4242u32.to_le_bytes());
        assert_eq!(
            StallEntityAction::try_from(Bytes::from(exit)).unwrap(),
            StallEntityAction::Exit { unique_id: 4242 }
        );
    }

    #[test]
    fn entity_action_buy_reads_the_buyer_and_the_remaining_rows() {
        let mut wire = vec![STALL_ACTION_BUY, 3];
        wire.extend_from_slice(&ascii("Buyer"));
        wire.extend_from_slice(&row(0, 11, 5, 900));
        wire.push(STALL_ROW_SENTINEL);

        let decoded = StallEntityAction::try_from(Bytes::from(wire.clone())).unwrap();

        let rows = decoded.rows(&expendable()).expect("rows should decode");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].item.slot, 0); // the STALL slot
        assert_eq!(rows[0].inventory_slot, 11);
        assert_eq!(rows[0].quantity, 5);
        assert_eq!(rows[0].price, 900);
        assert_eq!(rows[0].item.ref_id, 5000);
        assert_eq!(
            rows[0].item.data,
            ItemTypeData::Expendable {
                stack_count: 1,
                assimilation_prob: None,
                mag_params: vec![],
            }
        );

        let back: Bytes = decoded.into();
        assert_eq!(&back[..], &wire[..]);
    }

    #[test]
    fn an_unknown_entity_action_keeps_its_raw_tail() {
        let wire = vec![9u8, 0xAA, 0xBB];

        let decoded = StallEntityAction::try_from(Bytes::from(wire.clone())).unwrap();

        assert_eq!(
            decoded,
            StallEntityAction::Unknown {
                action: 9,
                tail: Bytes::from_static(&[0xAA, 0xBB]),
            }
        );
        let back: Bytes = decoded.into();
        assert_eq!(&back[..], &wire[..]);
    }

    // --- 0xB0BA ------------------------------------------------------------

    #[test]
    fn update_response_item_update_reads_its_trailing_error_code() {
        let mut wire = vec![1u8, STALL_UPDATE_ITEM_UPDATE, 2];
        wire.extend_from_slice(&7u16.to_le_bytes());
        wire.extend_from_slice(&1234u64.to_le_bytes());
        wire.extend_from_slice(&0u16.to_le_bytes());

        let decoded = StallUpdateResponse::try_from(Bytes::from(wire.clone())).unwrap();

        assert_eq!(
            decoded.body,
            StallUpdateAck::ItemUpdate {
                stall_slot: 2,
                quantity: 7,
                price: 1234,
                error_code: 0,
            }
        );
        assert!(decoded.rows(&expendable()).is_none());
        let back: Bytes = decoded.into();
        assert_eq!(&back[..], &wire[..]);
    }

    /// Add and remove both carry a leading error code and then the whole listing.
    #[test]
    fn update_response_add_reads_the_row_list() {
        let mut wire = vec![1u8, STALL_UPDATE_ITEM_ADDED];
        wire.extend_from_slice(&0u16.to_le_bytes());
        wire.extend_from_slice(&row(0, 11, 5, 900));
        wire.extend_from_slice(&row(1, 12, 1, 50));
        wire.push(STALL_ROW_SENTINEL);

        let decoded = StallUpdateResponse::try_from(Bytes::from(wire.clone())).unwrap();

        let rows = decoded.rows(&expendable()).expect("rows should decode");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].item.slot, 0);
        assert_eq!(rows[1].item.slot, 1);
        assert_eq!(rows[1].inventory_slot, 12);
        assert_eq!(rows[1].price, 50);
        let back: Bytes = decoded.into();
        assert_eq!(&back[..], &wire[..]);
    }

    /// The sentinel is written even for an empty listing, so a bare 0xFF is a
    /// well-formed empty list — not a missing one.
    #[test]
    fn an_empty_row_list_is_just_the_sentinel() {
        let mut wire = vec![1u8, STALL_UPDATE_ITEM_REMOVED];
        wire.extend_from_slice(&0u16.to_le_bytes());
        wire.push(STALL_ROW_SENTINEL);

        let decoded = StallUpdateResponse::try_from(Bytes::from(wire.clone())).unwrap();

        assert_eq!(decoded.rows(&expendable()), Some(vec![]));
        let back: Bytes = decoded.into();
        assert_eq!(&back[..], &wire[..]);
    }

    /// A list that never reaches its sentinel is malformed — every later row would
    /// be shifted, so no partial list is returned.
    #[test]
    fn a_row_list_without_its_sentinel_is_none() {
        let mut wire = vec![1u8, STALL_UPDATE_ITEM_ADDED];
        wire.extend_from_slice(&0u16.to_le_bytes());
        wire.extend_from_slice(&row(0, 11, 5, 900));
        // sentinel deliberately omitted

        let decoded = StallUpdateResponse::try_from(Bytes::from(wire)).unwrap();

        assert!(decoded.rows(&expendable()).is_none());
    }

    /// An unresolvable ref id costs the whole list, by design — its record width
    /// would be unknown. The raw bytes survive for a retry.
    #[test]
    fn rows_are_none_while_itemdata_is_unresolvable() {
        let mut wire = vec![1u8, STALL_UPDATE_ITEM_ADDED];
        wire.extend_from_slice(&0u16.to_le_bytes());
        wire.extend_from_slice(&row(0, 11, 5, 900));
        wire.push(STALL_ROW_SENTINEL);

        let decoded = StallUpdateResponse::try_from(Bytes::from(wire)).unwrap();

        assert!(decoded.rows(&MockResolver(ItemClass::Unknown)).is_none());
    }

    #[test]
    fn update_response_state_note_and_title_roundtrip() {
        let mut state = vec![1u8, STALL_UPDATE_STATE, 1];
        state.extend_from_slice(&1u16.to_le_bytes());
        let decoded = StallUpdateResponse::try_from(Bytes::from(state.clone())).unwrap();
        assert_eq!(
            decoded.body,
            StallUpdateAck::State {
                is_open: 1,
                stall_network_result: 1,
            }
        );
        let back: Bytes = decoded.into();
        assert_eq!(&back[..], &state[..]);

        let mut note = vec![1u8, STALL_UPDATE_NOTE];
        note.extend_from_slice(&ascii("open now"));
        let decoded = StallUpdateResponse::try_from(Bytes::from(note.clone())).unwrap();
        assert_eq!(
            decoded.body,
            StallUpdateAck::Note {
                note: "open now".to_string(),
            }
        );
        let back: Bytes = decoded.into();
        assert_eq!(&back[..], &note[..]);

        // Title carries no payload here — it arrives on 0x30BB instead.
        let title = vec![1u8, STALL_UPDATE_TITLE];
        let decoded = StallUpdateResponse::try_from(Bytes::from(title.clone())).unwrap();
        assert_eq!(decoded.body, StallUpdateAck::Title);
        let back: Bytes = decoded.into();
        assert_eq!(&back[..], &title[..]);
    }

    #[test]
    fn update_response_flea_market_mode_reads_one_byte() {
        let wire = vec![1u8, STALL_UPDATE_FLEA_MARKET_MODE, 2];

        let decoded = StallUpdateResponse::try_from(Bytes::from(wire.clone())).unwrap();

        assert_eq!(decoded.body, StallUpdateAck::FleaMarketMode { mode: 2 });
        let back: Bytes = decoded.into();
        assert_eq!(&back[..], &wire[..]);
    }

    /// An unrecognised update type must keep its bytes rather than fail the packet.
    #[test]
    fn an_unknown_update_type_keeps_its_raw_tail() {
        let wire = vec![1u8, 99, 0xAA];

        let decoded = StallUpdateResponse::try_from(Bytes::from(wire.clone())).unwrap();

        assert_eq!(decoded.update_type, 99);
        assert_eq!(
            decoded.body,
            StallUpdateAck::Unknown {
                tail: Bytes::from_static(&[0xAA]),
            }
        );
        let back: Bytes = decoded.into();
        assert_eq!(&back[..], &wire[..]);
    }

    // --- 0xB0B3 stall talk (#759) ------------------------------------------

    /// The snapshot a viewer gets on entering a stall: header, two rows, the
    /// sentinel, then the viewer list. The header widths are `[S]`; the row
    /// shape and the sentinel are `[V]` and shared with 0x30B7/0xB0BA.
    #[test]
    fn stall_talk_success_decodes_its_rows_and_viewer_list() {
        let mut wire = vec![1u8]; // result = ok
        wire.extend_from_slice(&7777u32.to_le_bytes()); // owner unique id
        wire.extend_from_slice(&ascii("Welcome!")); // the owner's note
        wire.push(1); // is_open
        wire.push(0); // flea market mode
        wire.extend_from_slice(&row(1, 13, 5, 900));
        wire.extend_from_slice(&row(2, 14, 1, 12_500));
        wire.push(0xFF); // end of rows
        wire.push(2); // peopleCount
        wire.extend_from_slice(&4242u32.to_le_bytes());
        wire.extend_from_slice(&4243u32.to_le_bytes());

        let decoded = StallTalkResponse::try_from(Bytes::from(wire.clone())).unwrap();
        let StallTalkResponse::Success {
            unique_id,
            ref message,
            is_open,
            flea_market_mode,
            ..
        } = decoded
        else {
            panic!("expected the success arm, got {decoded:?}");
        };
        assert_eq!(unique_id, 7777);
        assert_eq!(message, "Welcome!");
        assert!(is_open);
        assert_eq!(flea_market_mode, 0);

        let snapshot = decoded.snapshot(&expendable()).expect("tail decodes");
        assert_eq!(snapshot.rows.len(), 2);
        assert_eq!(snapshot.rows[0].inventory_slot, 13);
        assert_eq!(snapshot.rows[1].price, 12_500);
        assert_eq!(snapshot.viewers, vec![4242, 4243]);

        // and it round-trips byte-for-byte
        let re: Bytes = decoded.into();
        assert_eq!(&re[..], &wire[..]);
    }

    /// An empty stall is the sentinel immediately, with nobody watching — the
    /// case where an off-by-one in the tail parse would show up as a phantom
    /// row or a panic.
    #[test]
    fn stall_talk_handles_an_empty_listing_and_no_viewers() {
        let mut wire = vec![1u8];
        wire.extend_from_slice(&1u32.to_le_bytes());
        wire.extend_from_slice(&ascii(""));
        wire.push(0); // closed
        wire.push(0);
        wire.push(0xFF);
        wire.push(0); // peopleCount

        let decoded = StallTalkResponse::try_from(Bytes::from(wire)).unwrap();
        let snapshot = decoded.snapshot(&expendable()).expect("tail decodes");
        assert!(snapshot.rows.is_empty());
        assert!(snapshot.viewers.is_empty());
    }

    /// Failure carries the family's `StallErrorCode`, exactly like the three
    /// simple acks — and no snapshot.
    #[test]
    fn stall_talk_failure_is_an_error_code() {
        let mut wire = vec![2u8];
        wire.extend_from_slice(&0x3C2Bu16.to_le_bytes());
        let decoded = StallTalkResponse::try_from(Bytes::from(wire.clone())).unwrap();

        assert_eq!(
            decoded,
            StallTalkResponse::Failure {
                result: 2,
                error_code: 0x3C2B,
            }
        );
        assert!(decoded.snapshot(&expendable()).is_none());
        let re: Bytes = decoded.into();
        assert_eq!(&re[..], &wire[..]);
    }

    /// A truncated tail must yield `None`, never a partial listing: a short row
    /// shifts the viewer count as well.
    #[test]
    fn stall_talk_rejects_a_truncated_tail() {
        let mut wire = vec![1u8];
        wire.extend_from_slice(&1u32.to_le_bytes());
        wire.extend_from_slice(&ascii("x"));
        wire.push(1);
        wire.push(0);
        let mut short_row = row(1, 13, 5, 900);
        short_row.truncate(4);
        wire.extend_from_slice(&short_row);

        let decoded = StallTalkResponse::try_from(Bytes::from(wire)).unwrap();
        assert!(decoded.snapshot(&expendable()).is_none());
    }
}
