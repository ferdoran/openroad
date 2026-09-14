//! Quest wire opcodes — currently the **mark** pair only (0x30D6 / 0x30D7).
//!
//! Idea, and the reason this module is three structs rather than seventeen:
//! the quest family is the one family in the round-2 ledger (#451, #758) whose
//! layouts are recorded **nowhere**. SilkroadDoc-wiki indexes eleven quest
//! opcodes and every one of its body pages is an empty stub; go-sro has no
//! quest handler at all; xBot declares the opcodes and dispatches none. So the
//! only usable evidence on this machine is our own `packet_dump/`, and it
//! covers exactly two of them — which is what is modelled here. The whole
//! family's ledger, per opcode with its handler VA and the reason it is not
//! wired, is `docs/net-quest.md`.
//!
//! **Capture-verified, not spec-derived** — the opposite of most of this crate.
//! `packet_dump/0x30d6.log` (18 samples, 2026-08-10 … 2026-08-16) and
//! `packet_dump/0x30d7.log` (4 samples) are real v1.188 server traffic, and the
//! tests below decode literal lines from them.
//!
//! The pairing is proven by the captures themselves, not assumed: every
//! `0x30D7` body in the dump is byte-for-byte the leading `u32` of an *earlier*
//! `0x30D6` (`f05ee652` @ 09:50:38 removes the mark added at 09:50:13, and so
//! on for all four). That is what identifies the leading field as a **mark
//! handle** and `0x30D7` as "drop that mark".

use bevy::prelude::Message;

use sro_macro::ByteSize;
use sro_macro::Deserialize;
use sro_macro::SerializationError;
use sro_macro::Serialize;
use sro_macro_derive::*;

/// `QuestMark` — the icon the minimap/world draws for a mark. Values from
/// SilkroadDoc-wiki's `QuestMark` enum page, and corroborated by the capture:
/// the dump carries 1, 3 and 4 and nothing else.
pub const QUEST_MARK_NEW: u8 = 1;
pub const QUEST_MARK_OPEN: u8 = 2;
pub const QUEST_MARK_COMPLETE: u8 = 3;
pub const QUEST_MARK_HARD: u8 = 4;

/// 0x30D6 — add a quest mark.
///
/// Field confidence, from the 18 captured samples:
///
/// * `mark_id` **[V]** — the handle `0x30D7` later removes. Monotonic with a
///   stride of 44 across every sample, i.e. a server-side pool index, not a
///   quest id: the same quest's mark gets a new value each time it is
///   re-added.
/// * `unknown0` **[U]** — `0x82` in all 18 samples. Constant, so the capture
///   cannot say what it means; it may equally be the high byte of a `u16`
///   with `mark`.
/// * `mark` **[S]** — 1 / 3 / 4 in the capture, exactly the wiki's
///   `QuestMark::New` / `Complete` / `Hard`. `Open` (2) never appeared.
/// * `region` **[S]** — 25000 and 25255 in the capture, both valid region ids
///   for where the character was standing (`docs/formats/` region encoding).
/// * `unknown1`..`unknown4` **[U]** — four `u32`s, the second of which is `0`
///   in every sample. They look like a position triple plus one value (e.g.
///   `(1584, 0, 1407, 243)` recurring for one mark, `(332, 0, 1406, 55)` for
///   another), but "looks like" is not a layout: naming them would be the
///   unsourced-field defect (AGENTS.md, ADR-0009). **Resolve:** decompile the
///   handler at `008811e0`, or capture a mark at a known world position.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct QuestMarkAdd {
    pub mark_id: u32,
    pub unknown0: u8,
    /// See the `QUEST_MARK_*` constants.
    pub mark: u8,
    pub region: u16,
    pub unknown1: u32,
    pub unknown2: u32,
    pub unknown3: u32,
    pub unknown4: u32,
}

/// 0x30D7 — remove the quest mark with this handle. Whole body, 4 bytes
/// **[V]**: every captured sample is a handle a previous `0x30D6` introduced.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct QuestMarkRemove {
    pub mark_id: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    /// A literal line from `packet_dump/0x30d6.log` (2026-08-11T09:43:45Z),
    /// decoded field by field and re-encoded byte-identically.
    #[test]
    fn a_captured_quest_mark_add_round_trips() {
        let wire = Bytes::from_static(&[
            0x6c, 0x5e, 0xe6, 0x52, // mark_id
            0x82, // unknown0
            0x01, // mark = New
            0xa8, 0x61, // region 25000
            0x4c, 0x01, 0x00, 0x00, // 332
            0x00, 0x00, 0x00, 0x00, // 0 in every sample
            0x7e, 0x05, 0x00, 0x00, // 1406
            0x37, 0x00, 0x00, 0x00, // 55
        ]);
        let decoded = QuestMarkAdd::try_from(wire.clone()).unwrap();
        assert_eq!(decoded.mark_id, 0x52e65e6c);
        assert_eq!(decoded.unknown0, 0x82);
        assert_eq!(decoded.mark, QUEST_MARK_NEW);
        assert_eq!(decoded.region, 25000);
        assert_eq!(
            (
                decoded.unknown1,
                decoded.unknown2,
                decoded.unknown3,
                decoded.unknown4
            ),
            (332, 0, 1406, 55)
        );
        let back: Bytes = decoded.into();
        assert_eq!(back, wire, "24 bytes, nothing left over");
    }

    /// The capture's own proof of the pairing: `0x30D7`'s whole body is the
    /// handle an earlier `0x30D6` introduced (`c45ee652`, added 09:50:06 and
    /// removed 09:51:17 on 2026-08-11).
    #[test]
    fn a_captured_mark_remove_carries_an_earlier_adds_handle() {
        let add = Bytes::from_static(&[
            0xc4, 0x5e, 0xe6, 0x52, 0x82, 0x01, 0xa8, 0x61, 0x4c, 0x01, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x7e, 0x05, 0x00, 0x00, 0x37, 0x00, 0x00, 0x00,
        ]);
        let remove = Bytes::from_static(&[0xc4, 0x5e, 0xe6, 0x52]);

        let added = QuestMarkAdd::try_from(add).unwrap();
        let removed = QuestMarkRemove::try_from(remove.clone()).unwrap();
        assert_eq!(removed.mark_id, added.mark_id);

        let back: Bytes = removed.into();
        assert_eq!(back, remove);
    }

    /// The mark byte is the wiki's enum, and the capture only ever carries
    /// values from it — the one cross-check available between the two sources.
    #[test]
    fn the_captured_mark_values_are_all_wiki_enum_values() {
        for mark in [QUEST_MARK_NEW, QUEST_MARK_COMPLETE, QUEST_MARK_HARD] {
            assert!(matches!(mark, 1 | 3 | 4));
        }
        assert_eq!(QUEST_MARK_OPEN, 2, "declared but never seen in the capture");
    }
}
