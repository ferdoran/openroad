//! Alchemy (elixir / stone / manufacture / dismantle / socket) wire family.
//!
//! Idea: of the twenty opcodes in this block exactly **one** has a published
//! body — dismantle. `docs/re/systems/alchemy.md:44` states it plainly ("the
//! one published layout", `AGENT_ALCHEMY_DISMANTLE.md:1-18`), and the rest of
//! the family is named by two independent opcode catalogs but has **no**
//! recorded field layout anywhere: no `packet_dump/*.log`, no xBot parser
//! ("xBot implements none of these — zero ALCHEMY hits in `Network/Agent.cs`",
//! `alchemy.md:47`), and only call-site byte *counts* in the RE ledger. So
//! this module models dismantle and nothing else, and
//! `docs/net-alchemy.md` carries the other nineteen as a written list with
//! their handler VAs and the reason each stays unwired. Inventing the missing
//! bodies is the one defect the whole RE program exists to avoid (ADR-0009).
//!
//! When those layouts do arrive, the ack shape is already decided by the item
//! half: `character_data::EquipmentData` is byte-for-byte the original's item
//! blob, and every `0xB15x` ack re-emits the mutated item
//! (`alchemy.md:135-137`) — reuse that type rather than adding a second one.

use bevy::prelude::Message;

use sro_macro::ByteSize;
use sro_macro::Deserialize;
use sro_macro::SerializationError;
use sro_macro::Serialize;
use sro_macro_derive::*;

/// 0x7157 — client → server "dismantle these inventory slots".
///
/// `{u8 SlotCount, u8[] Slots}` (`AGENT_ALCHEMY_DISMANTLE.md:1-18`, the
/// family's only published body; builders `sro_client.exe@00820ff0`,
/// `@008259a0`). The slots are inventory slot indices, one byte each.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct AlchemyDismantleRequest {
    pub slot_count: u8,
    #[sro_packet(list_type = "by-size-field", size_field = "slot_count")]
    pub slots: Vec<u8>,
}

impl AlchemyDismantleRequest {
    /// Builds the request from the slots, keeping the count in step with them.
    pub fn new(slots: Vec<u8>) -> Self {
        Self {
            slot_count: slots.len() as u8,
            slots,
        }
    }
}

/// The `result` value that means "refused, an error code follows".
///
/// The published dismantle ack is `{u8 result, if result == 2 u16 errorCode}`,
/// the same `result == 2 → u16` idiom the storage and guild-storage acks use
/// (`agent::storage::StorageDataResponse`). `alchemy.md:45` notes it is the
/// *presumed* shape of every `0xB15x` ack — presumed for the siblings, but
/// published for this one, which is why only this one is modelled here.
pub const ALCHEMY_RESULT_ERROR: u8 = 2;

/// 0xB157 — server → client ack for [`AlchemyDismantleRequest`]
/// (handler `sro_client.exe@00872940`).
///
/// The `AlchemyErrorCode` table the published doc links is a dead page, so the
/// code is carried as a raw `u16` and named nowhere — an UNKNOWN kept as data
/// rather than guessed at (`alchemy.md` §9.7).
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct AlchemyDismantleResponse {
    pub result: u8,
    #[sro_packet(when = "result == ALCHEMY_RESULT_ERROR")]
    pub error_code: Option<u16>,
}

impl AlchemyDismantleResponse {
    pub fn is_success(&self) -> bool {
        self.result != ALCHEMY_RESULT_ERROR
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    /// No `packet_dump/0xb157.log` exists — the server in reach never answered
    /// a dismantle — so both directions are asserted against the published
    /// byte layout instead of a capture, and the test says so.
    #[test]
    fn dismantle_request_is_a_count_prefixed_slot_list() {
        let request = AlchemyDismantleRequest::new(vec![13, 14, 42]);
        assert_eq!(request.slot_count, 3);
        let bytes: Bytes = request.clone().into();
        assert_eq!(bytes.as_ref(), &[0x03, 0x0D, 0x0E, 0x2A]);
        assert_eq!(AlchemyDismantleRequest::try_from(bytes).unwrap(), request);

        // the degenerate body the original can also build: nothing selected
        let empty = AlchemyDismantleRequest::new(Vec::new());
        let bytes: Bytes = empty.clone().into();
        assert_eq!(bytes.as_ref(), &[0x00]);
        assert_eq!(AlchemyDismantleRequest::try_from(bytes).unwrap(), empty);
    }

    /// `{u8 result, if result == 2 u16 errorCode}` — success is one byte, a
    /// refusal carries the (unnamed) code.
    #[test]
    fn dismantle_response_carries_an_error_code_only_on_result_two() {
        let ok = AlchemyDismantleResponse::try_from(Bytes::from_static(&[0x01])).unwrap();
        assert!(ok.is_success());
        assert_eq!(ok.error_code, None);

        let refused =
            AlchemyDismantleResponse::try_from(Bytes::from_static(&[0x02, 0x0E, 0x1C])).unwrap();
        assert!(!refused.is_success());
        assert_eq!(refused.error_code, Some(0x1C0E));
    }
}
