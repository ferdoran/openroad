//! Pet / COS wire opcodes: the summon blob (0x30C8), its delta (0x30C9), the
//! command/mount/settings acks (0xB0C5 / 0xB0C6 / 0xB0CB / 0xB116 / 0xB117 /
//! 0xB420) and the client commands that drive them (0x70C5 action, 0x70C6
//! terminate, 0x70CB mount, 0x7116 unsummon, 0x7117 rename, 0x7420 settings).
//!
//! **Binary-derived.** Every layout here is read off the original 1.188 client's
//! own parser/builder functions and, where one exists, cross-checked against the
//! vSRO server's writer — `docs/re/net/inbound/pet-cos.md` and
//! `docs/re/net/outbound/pet-cos.md` carry the per-field evidence, the handler
//! and builder VAs, and the [V]/[S]/[U] tags. Those two supersede the older
//! xBot-derived `docs/net-pet-0x30C8.md`, which is wrong in four places this
//! module used to inherit (0x30C9's missing arms, 0xB0CB's conditional riding
//! uid, 0xB420's conditional settings word, 0x70CB's body).
//!
//! Captures exist for part of the family and are the tie-breaker where they
//! reach: `packet_dump/{0x30c8,0x30c9,0xb0cb}.log` and
//! `packet_dump/c2s/{0x70c5,0x70cb}.log`. **Attack-pet samples landed
//! 2026-08-18/19** (a Grey Wolf, ref 6106) and they settle the growth block —
//! see [`CosGrowth`], whose three fields were `[U]` by name until then. The
//! pick-pet settings word and 0xB0C5 are still binary-only, pending
//! CAPTURE_LIST group F.
//!
//! `0x706C CLIENT_PET_DESTROY` is deliberately **not** wired: it shares its
//! number with `PartyMatchListRequest` (`party.rs`) and one opcode maps to one
//! type.
//!
//! ## Why 0x30C8 keeps a raw tail
//!
//! The COS kind that selects the body's layout is **not a byte in the body** —
//! the original derives it from the ref object's refdata `tid` word and then
//! gates the body on it (`FUN_009a1900`, see [`CosKind`]). `Deserialize` takes
//! no context, so the packet stops after the two header fields and the body is
//! read through [`PetData::body`] once the caller has resolved the kind — the
//! same split that keeps `CharacterDataBody` raw (`ingame.rs:9-16`) and
//! `InventoryOperationResult::pickup_item` resolver-driven (`inventory.rs:412`).
//! A wrong guess then costs a `None`, not a failed packet.
//!
//! 0x30C8's body used to be three fixed xBot-derived tail structs picked by a
//! `PET_TYPE_*` byte. The recovered parser (#545) shows a single grammar whose
//! optional blocks are gated per kind, so the tails could not express it: a
//! transport is not a shorter pet, it is the same record with four gates shut.
//!
//! ⚠️ Free-text caveat: the derive decodes `String` with `from_utf8`, so a
//! cp1252/CP949 byte in a COS name makes [`PetData::body`] return `None`. That
//! is the tree-wide derived-string property `party.rs:15-22` documents; here it
//! is contained to one accessor instead of dropping the packet, because the
//! header is parsed separately.

use std::io::Cursor;

use bevy::prelude::Message;
use bytes::{BufMut, Bytes, BytesMut};

use crate::agent::character_data::{InventoryItem, ItemClassResolver};
use crate::agent::ingame::is_dungeon;

use sro_macro::ByteSize;
use sro_macro::Deserialize;
use sro_macro::SerializationError;
use sro_macro::Serialize;
use sro_macro_derive::*;

/// Which COS this is. **It is not a field on the wire.**
///
/// `FUN_009a1900` derives it from the entity's refdata `tid` word — all five
/// arms share `tid & 2 != 0 && (tid & 0x1c) == 4 && (tid & 0x60) == 0x40 &&
/// (tid & 0x780) == 0x180` and differ only in `tid & 0xF800`, i.e. `TypeID4`
/// (`:98-130`) — and stores the result at `record+0x14`, which every later
/// gate in the body then tests. So the caller resolves the kind from the COS
/// ref-object row and hands it to [`PetData::body`]; the packet cannot know it.
///
/// The variant docs carry both numbers because the recovered evidence uses
/// both: `TypeID4` is the refdata nibble, `kind` is the client's internal byte.
/// Source: `docs/re/net/inbound/pet-cos.md` §0x30C8.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CosKind {
    /// `TypeID4` 1 → kind 0. Transport / vehicle. [S]
    Vehicle,
    /// `TypeID4` 2 → kind 1. The second transport class. [S]
    Transport,
    /// `TypeID4` 3 → kind 3. Growth ("attack") pet: the only kind carrying the
    /// growth block, and it is named. [S]
    GrowthPet,
    /// `TypeID4` 4 → kind 2. Grab / pick pet: named, no growth block. [S]
    GrabPet,
    /// `TypeID4` 5 → kind 4. Mercenary / fellow. [V] — `FUN_0099ddb0` is
    /// `kind == 4` and is exactly the gate that logs `Create Mercenary
    /// Interface` at `00811500:23`.
    Fellow,
}

impl CosKind {
    /// From the refdata `TypeID4` nibble.
    pub fn from_type_id4(type_id4: u8) -> Option<Self> {
        Some(match type_id4 {
            1 => CosKind::Vehicle,
            2 => CosKind::Transport,
            3 => CosKind::GrowthPet,
            4 => CosKind::GrabPet,
            5 => CosKind::Fellow,
            _ => return None,
        })
    }

    /// From the whole refdata `tid` word, the way the original does it
    /// (`tid & 0xF800`, i.e. `TypeID4 << 11`).
    pub fn from_tid(tid: u16) -> Option<Self> {
        Self::from_type_id4(((tid & 0xF800) >> 11) as u8)
    }

    /// The client's internal kind byte (`record+0x14`) — the value the body's
    /// gates are written against, kept so the gates below read like the
    /// original's.
    pub fn kind_byte(self) -> u8 {
        match self {
            CosKind::Vehicle => 0,
            CosKind::Transport => 1,
            CosKind::GrabPet => 2,
            CosKind::GrowthPet => 3,
            CosKind::Fellow => 4,
        }
    }

    /// `kind in {2, 3}` — the two kinds the original treats as *pets*: they
    /// carry a name and the trailing `u8`.
    /// Whether the local player can board this COS. The two transport kinds
    /// are the rideable ones — pets and the mercenary follow on foot
    /// (`docs/re/systems/mount.md`).
    pub fn is_rideable(self) -> bool {
        matches!(self, CosKind::Vehicle | CosKind::Transport)
    }

    pub fn is_pet(self) -> bool {
        matches!(self, CosKind::GrabPet | CosKind::GrowthPet)
    }
}

/// The COS inventory is paged **28 slots per page**: the body's slot byte goes
/// through `FUN_0099dbd0(slot) = FUN_0099dba0(slot / 0x1c, slot % 0x1c)`
/// (`FUN_009a1900:245-248`). [V]
pub const COS_INVENTORY_PAGE_SLOTS: u8 = 28;

/// A full hunger bar. HGP is a per-10,000 value, and this is what the original
/// substitutes for the kinds that carry no growth block (`FUN_009a1900:155`).
///
/// It used to live here as `COS_GROWTH_SCALE_DEFAULT`, "a per-mille/percent
/// scalar `[S]`", and a second copy of the same 10,000 sat in the client as
/// `HGP_FULL`. They were always the same number: see [`CosGrowth::hgp`] for how
/// the field was identified. [V]
///
/// The server's own low-hunger check reads against it —
/// `value * 100 / 10000.0 < 30.0` (`server-dec/004fb8c0`), the
/// `UIIT_MSG_COSPETERR_HGP_30LOW` warning.
pub const COS_HGP_FULL: u16 = 10_000;

/// 0x30C9 update discriminators — the seven arms of the original's own switch
/// (`FUN_008aa340`, `docs/re/net/inbound/pet-cos.md` §0x30C9). [V]
pub const PET_UPDATE_UNSUMMONED: u8 = 1;
pub const PET_UPDATE_BAG: u8 = 2;
pub const PET_UPDATE_EXP: u8 = 3;
pub const PET_UPDATE_HUNGRY: u8 = 4;
pub const PET_UPDATE_RENAMED: u8 = 5;
/// Arm 6's `u32` is read and immediately discarded by the original, so the
/// field has a width but no known meaning. [U]
pub const PET_UPDATE_UNKNOWN_6: u8 = 6;
pub const PET_UPDATE_MODEL_CHANGED: u8 = 7;

/// The COS command codes carried by 0x70C5's action byte.
///
/// 1 and 8 are the two the original's builders emit
/// (`docs/re/net/outbound/pet-cos.md` §0x70C5). 2 and 8 are independently [V]
/// from the **server** side — `server-dec/004ecbc0_FUN_004ecbc0.c:68-86`
/// branches on both when it writes the 0xB0C5 error ack. 4 (turn-in-place, a
/// `u16` heading) comes from builders `009eb470`/`009eb6e0`.
///
/// 9 and 11 appear in no binary we have; they are Hyperbot's `CosCommandType`
/// (`packetEnums.hpp:459-465`) and are therefore [S]. Sending one is a probe —
/// 0xB0C5 echoes the action byte back, so the server's verdict is legible.
pub const PET_ACTION_MOVEMENT: u8 = 1;
pub const PET_ACTION_ATTACK: u8 = 2;
pub const PET_ACTION_TURN: u8 = 4;
pub const PET_ACTION_ITEM_PICKUP: u8 = 8;
pub const PET_ACTION_FOLLOW: u8 = 9;
pub const PET_ACTION_CHARM: u8 = 11;

/// The only movement sub-type the original builds: move to a position
/// (`PacketBuilder.cs:194`).
const MOVEMENT_TO_POSITION: u8 = 1;

/// `SRAttackPet.Settings : uint [Flags]` (`SRAttackPet.cs:34-39`). A newtype, not
/// an enum, so an unknown bit combination cannot fail the packet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AttackPetSettings(pub u32);

impl AttackPetSettings {
    pub const OFFENSIVE: u32 = 1;

    pub fn is_offensive(&self) -> bool {
        self.0 & Self::OFFENSIVE != 0
    }
}

/// `SRPickPet.Settings : uint [Flags]` (`SRPickPet.cs:18-27`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PickPetSettings(pub u32);

impl PickPetSettings {
    pub const GOLD: u32 = 1;
    pub const EQUIPMENT: u32 = 2;
    pub const OTHER_ITEMS: u32 = 4;
    pub const GRAB_ALL_ITEMS: u32 = 64;
    pub const ENABLED: u32 = 128;

    pub fn grabs_gold(&self) -> bool {
        self.0 & Self::GOLD != 0
    }

    pub fn grabs_equipment(&self) -> bool {
        self.0 & Self::EQUIPMENT != 0
    }

    pub fn grabs_other_items(&self) -> bool {
        self.0 & Self::OTHER_ITEMS != 0
    }

    pub fn grabs_all_items(&self) -> bool {
        self.0 & Self::GRAB_ALL_ITEMS != 0
    }

    pub fn is_enabled(&self) -> bool {
        self.0 & Self::ENABLED != 0
    }
}

/// The growth-pet block — present only for [`CosKind::GrowthPet`]
/// (`FUN_009a1900:141-152`).
///
/// **All three fields are named `[V]` as of 2026-08-19.** They used to be
/// `unk_c`/`unk_d`/`scale_e`, because the binary-derived doc recovered the
/// widths and the gates but left the meanings `[U]`. Three sources close it:
///
/// 1. **xBot's attack-pet tail** (`docs/net-pet-0x30C8.md:118-128`) names off 16
///    `Exp u64`, off 24 `Level u8`, off 25 `HGP u16` — exactly this
///    u64/u8/u16 triple. That file carries a `SUPERSEDED` banner for five
///    *layout* errors, but its own closing line reserves its field names, and
///    they are what settle this.
/// 2. **The offsets, cross-matched between the two handlers.** `FUN_009a1900`
///    stores these at `record[0x14]/[0x15]` (= byte `+0x50`), `record+0x12` and
///    `record+0x10`. `FUN_008aa340` (0x30C9) adds arm 3's exp delta to
///    `pet+0x50/+0x54`, runs its level-up loop over `FUN_00937f20(pet+0x12)`,
///    and arm 4 writes its HGP `u16` to `+0x10`. Three offsets, three matches,
///    same record.
/// 3. **A capture** — `packet_dump/0x30c8.log`, a Grey Wolf (ref 6106) on the
///    user's own server, which is the fixture in this module's tests.
#[derive(Serialize, Deserialize, ByteSize, Clone, Copy, Debug, PartialEq)]
pub struct CosGrowth {
    /// Running EXP total, `u64` at `:141-144` → `pet+0x50/+0x54`. Signed deltas
    /// from 0x30C9 arm 3 move it, so it goes *down* when the pet dies. [V]
    pub exp: u64,
    /// The pet's own level, `u8` at `:145-148` → `pet+0x12`, and the index the
    /// level table `FUN_00937f20` is read at. Not the same thing as the growth
    /// stage's characterdata `Lvl`, though the ladder rows happen to agree. [V]
    pub level: u8,
    /// Hunger (HGP), `u16` at `:149-152` → `pet+0x10`, the same slot 0x30C9
    /// arm 4 writes. Per-10,000, so [`COS_HGP_FULL`] is a full bar — which is
    /// also why the original substitutes 10,000 when the block is absent. [V]
    pub hgp: u16,
}

/// Just the `u16 len + ASCII` name, so the derive reads the string.
#[derive(Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
struct CosName {
    name: String,
}

/// The 0x30C8 body behind the two header fields — the grammar `FUN_009a1900`
/// actually reads, with every optional part carrying the gate that produces it.
///
/// Names are `[U]` where the original names nothing; the widths and the gates
/// are `[V]` (`docs/re/net/inbound/pet-cos.md` §0x30C8).
#[derive(Clone, Debug, PartialEq)]
pub struct CosBody {
    /// HP at summon time, `:132-135` — fed to both `FUN_009c7870` and
    /// `FUN_009c78a0`, i.e. current *and* maximum. xBot names off 8 `HP` on all
    /// three tails, and the captures agree: a horse reports 983 with `unk_b`
    /// also 983, and the Grey Wolf reports 360, its characterdata `MaxHP`. [V]
    pub hp: u32,
    /// `:136-139`, stored at `record[3]` and `entity+0x454`. **The one field of
    /// this body still `[U]`**: it is 360 on two of the four captured wolf
    /// summons and 0 on the other two, which fits neither HP nor MaxHP. xBot
    /// calls it `unkUInt01` too. Do not seed a bar from it.
    pub unk_b: u32,
    /// Growth block — `kind == 3` only.
    pub growth: Option<CosGrowth>,
    /// `:167-177` — only for the two *pet* `TypeID4`s (3 and 4); the original
    /// substitutes 0 when absent. [U]
    pub unk_f: Option<u32>,
    /// `kind in {2, 3}` only (`:178-189`).
    pub name: Option<String>,
    /// Slot capacity; always read (`:190-193`).
    pub inventory_size: u8,
    /// The carried items, `u8 slot` + the shared item record — the same reader
    /// the inventory and 0x30C9 type-2 use. Present only when
    /// `inventory_size != 0` (`:237-255`).
    pub items: Vec<InventoryItem>,
    /// The **owning character's entity uid** (`kind not in {0, 4}`,
    /// `:263-272`).
    ///
    /// **[V]** 2026-08-27: every `0x30C8` blob in the capture carries the value
    /// `0x00031527`, and the `0x3020` at the same login reports the local
    /// player's uid as exactly that. Holds for every blob in the dump.
    /// *(Was `[U]`.)*
    pub unk_g: Option<u32>,
    /// The **summon scroll's inventory slot** (`kind in {2, 3}`, `:282-286`;
    /// the original uses it as `H - 13`, i.e. as a bag index).
    ///
    /// **[V]** 2026-08-27: it reads `0x17` on every blob while the scroll sat
    /// in slot 0x17 and `0x41` on every blob after it moved to 0x41 — matching
    /// the slot byte of the `0x3040` COS-state pushes exactly. The `- 13` is
    /// `BAG_FIRST_SLOT`, so the original converts it to a bag-relative index.
    /// *(Was `[U]`.)*
    pub unk_h: Option<u8>,
}

impl CosBody {
    /// Hunger, with the original's substitution for the kinds that carry no
    /// growth block (`FUN_009a1900:155`) — they do not starve, so a full bar.
    pub fn hgp_or_full(&self) -> u16 {
        self.growth.map_or(COS_HGP_FULL, |growth| growth.hgp)
    }
}

/// 0x30C8 — server → client cos data, pushed after a summon.
///
/// Only the two header fields are typed; the body needs the COS kind, which is
/// **not on the wire** (see [`CosKind`]), so it is read through
/// [`Self::body`] once the caller has resolved the ref object. A wrong guess
/// then costs a `None`, not a failed packet — the same split that keeps
/// `CharacterDataBody` raw (`ingame.rs:9-16`).
#[derive(Message, Clone, Debug, PartialEq)]
pub struct PetData {
    /// The COS entity's unique id (`FUN_009a1900:73-78`). [V]
    pub unique_id: u32,
    /// The COS ref-object id — what the refdata lookup `FUN_0093f630` keys on,
    /// and therefore where the caller gets [`CosKind`] from (`:79-85`). [V]
    pub ref_obj_id: u32,
    /// The kind-dependent remainder; read with [`Self::body`].
    pub tail: Bytes,
}

impl PetData {
    /// Decode the body for a caller-resolved [`CosKind`].
    ///
    /// `None` when the bytes do not fit the grammar. The trailing parts are
    /// tolerated as absent rather than fatal: the original only reads them when
    /// the uid is new to its COS map (`:204`), so a re-send is *under-read* by
    /// the client itself — do not treat a body that stops early as malformed
    /// (#455).
    pub fn body(&self, kind: CosKind, resolver: &impl ItemClassResolver) -> Option<CosBody> {
        let mut cursor = Cursor::new(&self.tail[..]);
        let hp = u32::read_from(&mut cursor).ok()?;
        let unk_b = u32::read_from(&mut cursor).ok()?;
        let growth = match kind {
            CosKind::GrowthPet => Some(CosGrowth::read_from(&mut cursor).ok()?),
            _ => None,
        };
        // Gated on TypeID4 3|4 — the two pet kinds, i.e. the same set as the
        // name and the trailing byte.
        let unk_f = kind
            .is_pet()
            .then(|| u32::read_from(&mut cursor))
            .transpose()
            .ok()?;
        let name = kind
            .is_pet()
            .then(|| CosName::read_from(&mut cursor).map(|wire| wire.name))
            .transpose()
            .ok()?;
        let inventory_size = u8::read_from(&mut cursor).ok()?;

        // Everything below is what the original skips on a re-summon, so a
        // body that ends here is legal.
        let mut items = Vec::new();
        if inventory_size != 0 {
            let Ok(item_count) = u8::read_from(&mut cursor) else {
                return Some(CosBody {
                    hp,
                    unk_b,
                    growth,
                    unk_f,
                    name,
                    inventory_size,
                    items,
                    unk_g: None,
                    unk_h: None,
                });
            };
            for _ in 0..item_count {
                // A truncated record shifts every later one, so half a list is
                // worse than no list.
                items.push(InventoryItem::read_with(&mut cursor, resolver).ok()?);
            }
        }
        let unk_g = (!matches!(kind, CosKind::Vehicle | CosKind::Fellow))
            .then(|| u32::read_from(&mut cursor).ok())
            .flatten();
        let unk_h = kind
            .is_pet()
            .then(|| u8::read_from(&mut cursor).ok())
            .flatten();
        Some(CosBody {
            hp,
            unk_b,
            growth,
            unk_f,
            name,
            inventory_size,
            items,
            unk_g,
            unk_h,
        })
    }
}

impl TryFrom<Bytes> for PetData {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, SerializationError> {
        let short = || {
            SerializationError::IoError(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "0x30C8 body too short",
            ))
        };
        let unique_id = u32::from_le_bytes(value.get(0..4).ok_or_else(short)?.try_into().unwrap());
        let ref_obj_id = u32::from_le_bytes(value.get(4..8).ok_or_else(short)?.try_into().unwrap());
        Ok(PetData {
            unique_id,
            ref_obj_id,
            tail: value.slice(8..),
        })
    }
}

impl From<PetData> for Bytes {
    fn from(p: PetData) -> Self {
        let mut buf = BytesMut::new();
        buf.put_u32_le(p.unique_id);
        buf.put_u32_le(p.ref_obj_id);
        buf.extend_from_slice(&p.tail);
        buf.freeze()
    }
}

/// The arm-specific tail of a [`PetUpdate`].
///
/// An unrecognised `update_type` becomes [`Self::Unknown`] with its bytes
/// intact rather than failing the packet — the original ignores arms it does
/// not know, and so must we.
#[derive(Clone, Debug, PartialEq)]
pub enum PetUpdatePayload {
    /// Arm 1 — the COS is gone. Empty body.
    Unsummoned,
    /// Arm 2 — a whole-bag refresh. The item records need the itemdata
    /// resolver, exactly like 0x30C8's, so they stay raw here and are read
    /// through [`PetUpdate::bag_items`].
    Bag {
        /// [S] — stored at `pet+0x18`, i.e. the same slot capacity 0x30C8's
        /// `inventory_size` carries.
        slot_capacity: u8,
        item_count: u8,
        items: Bytes,
    },
    /// Arm 3 — exp gained or **lost**. Signed on purpose: the original picks
    /// `UIIT_MSG_COSPET_LOST_EXP` on `delta < 0` and `..._GAIN_EXP` otherwise,
    /// which is how a pet's death shows up here.
    Exp {
        delta: i64,
        /// The entity the exp came from; `0` means "no source" and is what the
        /// original tests before it places the floating text.
        source_unique_id: u32,
    },
    /// Arm 4 — hunger, per-10 000 (`HGP_FULL`).
    Hunger { hgp: u16 },
    /// Arm 5 — the server pushing a new name, including after a successful
    /// 0x7117 (whose own ack is empty).
    Renamed { name: String },
    /// Arm 6 — a `u32` the original reads and drops on the floor. Kept so the
    /// body length stays right. [U]
    Unknown6 { value: u32 },
    /// Arm 7 — the growth-stage model swap. The server writes a **ref-object
    /// id**, not a model id.
    ModelChanged { new_ref_obj_id: u32 },
    /// Any arm the original does not know either.
    Unknown { tail: Bytes },
}

/// 0x30C9 — server → client cos delta, discriminated by `update_type`
/// (`FUN_008aa340`; `docs/re/net/inbound/pet-cos.md` §0x30C9).
///
/// Hand-written rather than derived for the same reason [`PetData`] is: arm 2
/// carries item records whose shape depends on the loaded itemdata, which
/// `Deserialize` has no way to reach.
///
/// All seven arms' widths are [V] — the client's reader and, for arms 1/2/4/7,
/// the vSRO server's writers agree. Arm 3 in particular is **not** [U]: this
/// module used to claim its body was unresolvable without a capture, but
/// `FUN_008aa340` reads `{i64, u32}` plainly.
#[derive(Message, Clone, Debug, PartialEq)]
pub struct PetUpdate {
    pub unique_id: u32,
    /// 1 unsummoned · 2 bag · 3 exp · 4 hungry · 5 renamed · 6 [U] · 7 model.
    pub update_type: u8,
    pub payload: PetUpdatePayload,
}

impl PetUpdate {
    /// Arm 4's value, for callers that only care about hunger.
    pub fn hgp(&self) -> Option<u16> {
        match self.payload {
            PetUpdatePayload::Hunger { hgp } => Some(hgp),
            _ => None,
        }
    }

    /// Arm 7's ref-object id.
    pub fn new_ref_obj_id(&self) -> Option<u32> {
        match self.payload {
            PetUpdatePayload::ModelChanged { new_ref_obj_id } => Some(new_ref_obj_id),
            _ => None,
        }
    }

    /// Decode arm 2's item records against the loaded itemdata.
    ///
    /// `None` when this is not a bag update or the records do not fit — a
    /// truncated record shifts every later one, so half a list is worse than
    /// no list (the same contract as [`PetData::body`]'s item loop).
    pub fn bag_items(&self, resolver: &impl ItemClassResolver) -> Option<Vec<InventoryItem>> {
        let PetUpdatePayload::Bag {
            item_count, items, ..
        } = &self.payload
        else {
            return None;
        };
        let mut cursor = Cursor::new(&items[..]);
        let mut out = Vec::with_capacity(*item_count as usize);
        for _ in 0..*item_count {
            out.push(InventoryItem::read_with(&mut cursor, resolver).ok()?);
        }
        Some(out)
    }
}

impl TryFrom<Bytes> for PetUpdate {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, SerializationError> {
        let mut cursor = Cursor::new(&value[..]);
        let unique_id = u32::read_from(&mut cursor)?;
        let update_type = u8::read_from(&mut cursor)?;
        // Where the arm tail starts; arm 2 hands the remainder to the resolver.
        let tail = value.slice(5.min(value.len())..);
        let payload = match update_type {
            PET_UPDATE_UNSUMMONED => PetUpdatePayload::Unsummoned,
            PET_UPDATE_BAG => {
                let slot_capacity = u8::read_from(&mut cursor)?;
                let item_count = u8::read_from(&mut cursor)?;
                PetUpdatePayload::Bag {
                    slot_capacity,
                    item_count,
                    items: value.slice(7.min(value.len())..),
                }
            }
            PET_UPDATE_EXP => PetUpdatePayload::Exp {
                delta: i64::read_from(&mut cursor)?,
                source_unique_id: u32::read_from(&mut cursor)?,
            },
            PET_UPDATE_HUNGRY => PetUpdatePayload::Hunger {
                hgp: u16::read_from(&mut cursor)?,
            },
            PET_UPDATE_RENAMED => PetUpdatePayload::Renamed {
                name: CosName::read_from(&mut cursor)?.name,
            },
            PET_UPDATE_UNKNOWN_6 => PetUpdatePayload::Unknown6 {
                value: u32::read_from(&mut cursor)?,
            },
            PET_UPDATE_MODEL_CHANGED => PetUpdatePayload::ModelChanged {
                new_ref_obj_id: u32::read_from(&mut cursor)?,
            },
            _ => PetUpdatePayload::Unknown { tail },
        };
        Ok(PetUpdate {
            unique_id,
            update_type,
            payload,
        })
    }
}

impl From<PetUpdate> for Bytes {
    fn from(p: PetUpdate) -> Self {
        let mut buf = BytesMut::new();
        buf.put_u32_le(p.unique_id);
        buf.put_u8(p.update_type);
        match p.payload {
            PetUpdatePayload::Unsummoned => {}
            PetUpdatePayload::Bag {
                slot_capacity,
                item_count,
                items,
            } => {
                buf.put_u8(slot_capacity);
                buf.put_u8(item_count);
                buf.extend_from_slice(&items);
            }
            PetUpdatePayload::Exp {
                delta,
                source_unique_id,
            } => {
                buf.put_i64_le(delta);
                buf.put_u32_le(source_unique_id);
            }
            PetUpdatePayload::Hunger { hgp } => buf.put_u16_le(hgp),
            PetUpdatePayload::Renamed { name } => CosName { name }.serialize_to(&mut buf),
            PetUpdatePayload::Unknown6 { value } => buf.put_u32_le(value),
            PetUpdatePayload::ModelChanged { new_ref_obj_id } => buf.put_u32_le(new_ref_obj_id),
            PetUpdatePayload::Unknown { tail } => buf.extend_from_slice(&tail),
        }
        buf.freeze()
    }
}

/// 0xB0CB — server → client mount/dismount ack (`FUN_008a7720`;
/// `docs/re/net/inbound/pet-cos.md` §0xB0CB). 10 bytes on success, 3 on error.
///
/// `riding_unique_id` is **unconditional**: the original's three reads happen
/// back-to-back before any branch, and both vSRO server writers — `004ec640`
/// (mount) and `004ec750` (dismount) — emit the transport uid on both paths.
/// This module used to gate it on `is_mounting`, inheriting an xBot error, and
/// so consumed 6 bytes of the 10-byte dismount: `packet_dump/0xb0cb.log` line 2
/// is `0173e4010000d5e40100` with `is_mounting = 0` and a trailing uid we threw
/// away exactly when the rider needed unlinking.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct PetPlayerMounted {
    pub success: bool,
    #[sro_packet(when = "success")]
    pub player_unique_id: Option<u32>,
    #[sro_packet(when = "success")]
    pub is_mounting: Option<bool>,
    #[sro_packet(when = "success")]
    pub riding_unique_id: Option<u32>,
    /// The mount-message category is `0x0E` for this opcode. Value space [U].
    #[sro_packet(when = "!success")]
    pub error_code: Option<u16>,
}

/// 0xB420 — server → client settings-change ack (`FUN_008a7d20`;
/// `docs/re/net/inbound/pet-cos.md` §0xB420). 10 bytes on success, 3 on error.
///
/// `settings` is **unconditional** on the success path — the original's three
/// reads are not branched, and its two asserts name the `settings_type` arms
/// `pCOSInfo->IsGoldPet()` (1) and `IsCashPet()` (2), which both store the same
/// field. Gating it on `settings_type == 1` (an xBot reading this module used
/// to carry) left 4 bytes unconsumed on every cash-pet ack.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct PetSettingsChangeResponse {
    pub success: bool,
    #[sro_packet(when = "success")]
    pub unique_id: Option<u32>,
    /// 1 = gold pet, 2 = cash pet — **not** a pet-class discriminator.
    #[sro_packet(when = "success")]
    pub settings_type: Option<u8>,
    /// [`AttackPetSettings`] or [`PickPetSettings`] depending on the COS.
    #[sro_packet(when = "success")]
    pub settings: Option<u32>,
    /// COS error category `0x0C`. Value space [U].
    #[sro_packet(when = "!success")]
    pub error_code: Option<u16>,
}

/// 0x7420 — client → server: change a COS's settings.
///
/// Shape from the **server's** reader, not from a client builder (the original
/// has none): `server-dec/00511d10_FUN_00511d10.c:14-18` reads `u32, u8, u32`
/// in that order (`docs/re/net/inbound/pet-cos.md:741`).
///
/// `settings_type` is **not** an "attack pet vs other" discriminator — per the
/// server's own asserts it selects gold pet (1) vs cash pet (2), and both write
/// the same settings field (`pet-cos.md:739-740`). The bit layout of `settings`
/// is [`PickPetSettings`] / [`AttackPetSettings`] depending on the COS.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct PetSettingsChangeRequest {
    pub unique_id: u32,
    pub settings_type: u8,
    pub settings: u32,
}

/// The `settings_type` of a normal (gold) pet — the cash-shop pet is 2
/// (`docs/re/net/inbound/pet-cos.md:739-740`).
pub const PET_SETTINGS_TYPE_GOLD: u8 = 1;

/// 0x7116 — client → server unsummon (builder `0081e8d0`, 4 bytes. [V]).
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct PetUnsummonRequest {
    pub unique_id: u32,
}

/// 0xB116 — the unsummon ack (`FUN_008a7b30`). Success is **empty**: the COS
/// itself goes away via 0x30C9 arm 1, not through this packet.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct PetUnsummonResponse {
    pub success: bool,
    /// COS error category `0x0C`. The server pushes small ints (3, 4) here, so
    /// the value space is likely 1-byte-ish despite the `u16` width. [S]
    #[sro_packet(when = "!success")]
    pub error_code: Option<u16>,
}

/// 0x70C6 — client → server terminate (builder `0081e810`, 4 bytes. [V]).
///
/// Distinct from [`PetUnsummonRequest`]: unsummon puts a COS away, terminate
/// ends it (the original's `UIIT_STT_COS_CLEAN` "Terminated" action).
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct PetTerminateRequest {
    pub unique_id: u32,
}

/// 0xB0C6 — the terminate ack (`FUN_008a7a70`). Carries **no** cos uid; the
/// success body is empty. Note the original's guard is `result != 1`, not
/// `== 2`, so any non-1 byte takes the error path — which is exactly how
/// `bool` deserializes here (`u8 == 1`).
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct PetTerminateResponse {
    pub success: bool,
    #[sro_packet(when = "!success")]
    pub error_code: Option<u16>,
}

/// 0x7117 — client → server rename (builder `007a4170`, module
/// `IFCOSInfo.cpp`. [V]).
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct PetRenameRequest {
    pub unique_id: u32,
    pub name: String,
}

/// 0xB117 — the rename ack (`FUN_008a7b80`). Success is **empty**, so the new
/// name reaches the client only as [`PetUpdatePayload::Renamed`] — do not
/// apply a rename optimistically.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct PetRenameResponse {
    pub success: bool,
    #[sro_packet(when = "!success")]
    pub error_code: Option<u16>,
}

/// 0xB0C5 — the ack for a 0x70C5 COS command (`FUN_00872590`;
/// `docs/re/net/inbound/pet-cos.md` §0xB0C5). 7 / 11 / 9 / 13 bytes.
///
/// Note the field order: the error code sits **between** the action echo and
/// the cos uid, so `unique_id` is at offset 2 on success and 4 on failure.
/// Both the client's reader and the server's writer (`004ecbc0:68-86`) agree,
/// and that writer is also what makes [`PET_ACTION_ATTACK`] and
/// [`PET_ACTION_ITEM_PICKUP`] [V] rather than bot-sourced.
///
/// Actions other than 8 read no tail, so a server that appended one for e.g.
/// `Movement` would desync — the original would too. [V]
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct PetActionResponse {
    pub success: bool,
    /// Echo of the action byte we sent — see the `PET_ACTION_*` consts.
    pub action: u8,
    #[sro_packet(when = "!success")]
    pub error_code: Option<u16>,
    pub unique_id: u32,
    /// The grabbed item's GID, present only on the pick-up action. The
    /// original names it `dwGIDItem` in a `DropItemManager.cpp` assert.
    #[sro_packet(when = "action == 8")]
    pub item_gid: Option<u32>,
}

/// 0x30E7 — "you have wandered too far" (`FUN_00883b10`). One reason byte;
/// the distances the original shows (100, 30) are **client-side literals**,
/// not wire fields.
///
/// Not COS-only despite living here: reason 1 is the job trade cart, reason 2
/// is a quest monster (`docs/re/systems/job.md`). Reasons other than those two
/// are ignored by the original. [V]
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct StuckDistanceWarning {
    pub reason: u8,
}

/// [`StuckDistanceWarning::reason`] — too far from the job trade cart.
pub const STUCK_REASON_TRADE_CART: u8 = 1;
/// [`StuckDistanceWarning::reason`] — too far from the quest monster.
pub const STUCK_REASON_QUEST_MONSTER: u8 = 2;

/// 0x70CB — client → server mount/dismount toggle.
///
/// The builder does exist after all: `0081eb50` writes `b1 b4` — a **byte
/// first**, then the uid (`docs/re/net/outbound/pet-cos.md` §0x70CB, [V]).
/// This struct was previously an explicit `[U]` guess of a bare `{u32}`, which
/// is both the wrong width and the wrong order; `packet_dump/c2s/0x70cb.log`
/// records the 4-byte probes we sent before the builder was recovered.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct PetMountRequest {
    /// 1 = board, 0 = dismount — the same sense as `PetPlayerMounted`'s
    /// `is_mounting`, which is what the server echoes back. [S]: the width and
    /// position are [V] from the builder, the value mapping is the mirror of
    /// the ack.
    pub mount_state: u8,
    pub cos_unique_id: u32,
}

/// 0x70C5 — client → server cos command envelope: `pet_unique_id`, an action
/// byte, then an action-specific tail.
///
/// Hand-written because the movement tail's coordinate *width* depends on a
/// prior field (the region), which the derive cannot express — the same reason
/// `MovementRequest` is hand-written (`ingame.rs:137-140`).
///
/// The original's builder emits only the two known actions
/// (`PacketBuilder.cs:181-208`, `:381-399`); anything else keeps its raw tail so
/// an unknown command is never mis-sliced.
#[derive(Message, Clone, Debug, PartialEq)]
pub enum PetActionRequest {
    /// `action = 1`. `x`/`y`/`z` are raw region-local units in **wire order**,
    /// i.e. `y` is the height component — the same field naming as
    /// [`crate::agent::ingame::MovementRequest`]. The doc's "X, Z, Y" is xBot's
    /// Z-up axis naming for that identical order, not a different order.
    Movement {
        pet_unique_id: u32,
        region: u16,
        x: i32,
        y: i32,
        z: i32,
    },
    /// `action = 2` — send the COS at a target. The 9-byte `b4 b1 b4` shape of
    /// builders `00791d40`/`008a5fd0`/`008afb50`; those are the *same*
    /// functions that build the player's own 0x7074, which is the mechanism by
    /// which a commanded COS takes over the ordinary attack path. [V]
    Attack {
        pet_unique_id: u32,
        target_unique_id: u32,
    },
    /// `action = 4` — turn in place. `b4 b1 b2` (builders `009eb470`,
    /// `009eb6e0`); the `u16` is a heading. [V]
    Turn { pet_unique_id: u32, heading: u16 },
    /// `action = 8`.
    ItemPickUp {
        pet_unique_id: u32,
        item_unique_id: u32,
    },
    /// `action = 9` — resume following the owner. **[S]**: no builder in any
    /// binary we have, only Hyperbot's `CosCommandType`. Modelled as a bare
    /// `{uid, action}` because a follow order has nothing else to carry; the
    /// 0xB0C5 echo is what will confirm or refute it.
    Follow { pet_unique_id: u32 },
    /// Any other action code, or a movement sub-type the original never builds.
    Unknown {
        pet_unique_id: u32,
        action: u8,
        tail: Bytes,
    },
}

/// One coordinate component: 2 bytes in the overworld, 4 in a dungeon.
fn read_coord(cursor: &mut Cursor<&[u8]>, region: u16) -> Result<i32, SerializationError> {
    if is_dungeon(region) {
        i32::read_from(cursor)
    } else {
        Ok(i16::read_from(cursor)? as i32)
    }
}

fn put_coord(buf: &mut BytesMut, region: u16, value: i32) {
    if is_dungeon(region) {
        buf.put_i32_le(value);
    } else {
        buf.put_i16_le(value as i16);
    }
}

impl TryFrom<Bytes> for PetActionRequest {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, SerializationError> {
        let mut cursor = Cursor::new(&value[..]);
        let pet_unique_id = u32::read_from(&mut cursor)?;
        let action = u8::read_from(&mut cursor)?;
        let unknown = |action| PetActionRequest::Unknown {
            pet_unique_id,
            action,
            tail: value.slice(5..),
        };
        match action {
            PET_ACTION_MOVEMENT => {
                if u8::read_from(&mut cursor)? != MOVEMENT_TO_POSITION {
                    return Ok(unknown(action));
                }
                let region = u16::read_from(&mut cursor)?;
                Ok(PetActionRequest::Movement {
                    pet_unique_id,
                    region,
                    x: read_coord(&mut cursor, region)?,
                    y: read_coord(&mut cursor, region)?,
                    z: read_coord(&mut cursor, region)?,
                })
            }
            PET_ACTION_ATTACK => Ok(PetActionRequest::Attack {
                pet_unique_id,
                target_unique_id: u32::read_from(&mut cursor)?,
            }),
            PET_ACTION_TURN => Ok(PetActionRequest::Turn {
                pet_unique_id,
                heading: u16::read_from(&mut cursor)?,
            }),
            PET_ACTION_ITEM_PICKUP => Ok(PetActionRequest::ItemPickUp {
                pet_unique_id,
                item_unique_id: u32::read_from(&mut cursor)?,
            }),
            PET_ACTION_FOLLOW => Ok(PetActionRequest::Follow { pet_unique_id }),
            _ => Ok(unknown(action)),
        }
    }
}

impl From<PetActionRequest> for Bytes {
    fn from(p: PetActionRequest) -> Self {
        let mut buf = BytesMut::new();
        match p {
            PetActionRequest::Movement {
                pet_unique_id,
                region,
                x,
                y,
                z,
            } => {
                buf.put_u32_le(pet_unique_id);
                buf.put_u8(PET_ACTION_MOVEMENT);
                buf.put_u8(MOVEMENT_TO_POSITION);
                buf.put_u16_le(region);
                put_coord(&mut buf, region, x);
                put_coord(&mut buf, region, y);
                put_coord(&mut buf, region, z);
            }
            PetActionRequest::Attack {
                pet_unique_id,
                target_unique_id,
            } => {
                buf.put_u32_le(pet_unique_id);
                buf.put_u8(PET_ACTION_ATTACK);
                buf.put_u32_le(target_unique_id);
            }
            PetActionRequest::Turn {
                pet_unique_id,
                heading,
            } => {
                buf.put_u32_le(pet_unique_id);
                buf.put_u8(PET_ACTION_TURN);
                buf.put_u16_le(heading);
            }
            PetActionRequest::ItemPickUp {
                pet_unique_id,
                item_unique_id,
            } => {
                buf.put_u32_le(pet_unique_id);
                buf.put_u8(PET_ACTION_ITEM_PICKUP);
                buf.put_u32_le(item_unique_id);
            }
            PetActionRequest::Follow { pet_unique_id } => {
                buf.put_u32_le(pet_unique_id);
                buf.put_u8(PET_ACTION_FOLLOW);
            }
            PetActionRequest::Unknown {
                pet_unique_id,
                action,
                tail,
            } => {
                buf.put_u32_le(pet_unique_id);
                buf.put_u8(action);
                buf.extend_from_slice(&tail);
            }
        }
        buf.freeze()
    }
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

    /// A `packet_dump` hex line, whitespace-tolerant so a fixture can be laid
    /// out field by field.
    fn hex(s: &str) -> Vec<u8> {
        let digits: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
        digits
            .chunks(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }

    /// `u16` length prefix + bytes, the derive's `String` framing.
    fn ascii(s: &str) -> Vec<u8> {
        let mut out = (s.len() as u16).to_le_bytes().to_vec();
        out.extend_from_slice(s.as_bytes());
        out
    }

    fn pet_data(tail: Vec<u8>) -> PetData {
        let mut wire = 0x1234_5678u32.to_le_bytes().to_vec();
        wire.extend_from_slice(&2001u32.to_le_bytes());
        wire.extend_from_slice(&tail);
        PetData::try_from(Bytes::from(wire)).unwrap()
    }

    #[test]
    fn pet_data_splits_the_header_from_the_raw_tail_and_roundtrips() {
        let decoded = pet_data(vec![0xAA, 0xBB]);

        assert_eq!(decoded.unique_id, 0x1234_5678);
        assert_eq!(decoded.ref_obj_id, 2001);
        assert_eq!(&decoded.tail[..], &[0xAA, 0xBB]);

        let back: Bytes = decoded.clone().into();
        assert_eq!(PetData::try_from(back).unwrap(), decoded);
    }

    #[test]
    fn pet_data_header_shorter_than_eight_bytes_is_an_error() {
        assert!(PetData::try_from(Bytes::from_static(&[1, 2, 3, 4, 5, 6, 7])).is_err());
    }

    /// The kind is a refdata lookup, never a wire byte — the whole reason
    /// [`PetData::body`] takes it as an argument (#545).
    #[test]
    fn the_kind_comes_from_the_refdata_tid_not_the_wire() {
        // tid & 0xF800 == TypeID4 << 11; the low bits are the shared COS
        // predicate and do not select the arm.
        assert_eq!(CosKind::from_tid(0x0800 | 0x0184), Some(CosKind::Vehicle));
        assert_eq!(CosKind::from_tid(0x1000 | 0x0184), Some(CosKind::Transport));
        assert_eq!(CosKind::from_tid(0x1800 | 0x0184), Some(CosKind::GrowthPet));
        assert_eq!(CosKind::from_tid(0x2000 | 0x0184), Some(CosKind::GrabPet));
        assert_eq!(CosKind::from_tid(0x2800 | 0x0184), Some(CosKind::Fellow));
        assert_eq!(CosKind::from_tid(0x0000), None);
        // The internal kind byte the body's gates are written against.
        assert_eq!(CosKind::GrowthPet.kind_byte(), 3);
        assert_eq!(CosKind::GrabPet.kind_byte(), 2);
        assert!(CosKind::GrowthPet.is_pet() && CosKind::GrabPet.is_pet());
        assert!(!CosKind::Vehicle.is_pet() && !CosKind::Fellow.is_pet());
    }

    /// The growth pet is the arm that exercises every gated block at once:
    /// growth block, `unk_f`, name, item list, `unk_g`, `unk_h`.
    #[test]
    fn growth_pet_body_reads_every_gated_block() {
        let mut tail = 11u32.to_le_bytes().to_vec(); // hp
        tail.extend_from_slice(&22u32.to_le_bytes()); // unk_b
        tail.extend_from_slice(&123_456u64.to_le_bytes()); // exp
        tail.push(14); // level
        tail.extend_from_slice(&5_000u16.to_le_bytes()); // hgp
        tail.extend_from_slice(&33u32.to_le_bytes()); // F
        tail.extend_from_slice(&ascii("Wolfie")); // name
        tail.push(6); // inventory_size
        tail.push(1); // item_count
        tail.push(3); // slot
        tail.extend_from_slice(&0u32.to_le_bytes()); // RentInfo::None
        tail.extend_from_slice(&5000u32.to_le_bytes()); // ref_id
        tail.extend_from_slice(&25u16.to_le_bytes()); // stack
        tail.extend_from_slice(&44u32.to_le_bytes()); // G
        tail.push(55); // H

        let resolver = MockResolver(ItemClass::Expendable { tid3: 1, tid4: 1 });
        let body = pet_data(tail)
            .body(CosKind::GrowthPet, &resolver)
            .expect("growth-pet body should decode");

        assert_eq!(body.hp, 11);
        assert_eq!(body.unk_b, 22);
        assert_eq!(
            body.growth,
            Some(CosGrowth {
                exp: 123_456,
                level: 14,
                hgp: 5_000,
            })
        );
        assert_eq!(body.hgp_or_full(), 5_000);
        assert_eq!(body.unk_f, Some(33));
        assert_eq!(body.name.as_deref(), Some("Wolfie"));
        assert_eq!(body.inventory_size, 6);
        assert_eq!(body.items.len(), 1);
        assert_eq!(body.items[0].slot, 3);
        assert_eq!(body.items[0].ref_id, 5000);
        assert_eq!(body.unk_g, Some(44));
        assert_eq!(body.unk_h, Some(55));
    }

    /// The capture that named the growth block.
    ///
    /// `packet_dump/0x30c8.log`, 2026-08-19 — a Grey Wolf (ref 6106,
    /// `COS_P_WOLF_001`) on the user's own vSRO server, 39 bytes with every one
    /// accounted for. Two numbers make it self-checking:
    ///
    /// * `hgp = 9932` is 99.32 % of [`COS_HGP_FULL`], the only reading of those
    ///   two bytes that lands in range at all.
    /// * `exp = 77` is what `0x30c9.log` independently predicts for this pet —
    ///   the previous summon took three arm-3 deltas of `+26` and then
    ///   `ffffffffffffffff` (**-1**, the death loss), and `3 * 26 - 1 = 77`.
    ///   That also confirms arm 3's delta is signed.
    ///
    /// `hp = 360` equals characterdata `MaxHP` for ref 6106, and `unk_b` is 0
    /// here while two other captures of the same pet have it at 360 — which is
    /// exactly why it keeps its `[U]` name.
    #[test]
    fn the_grey_wolf_capture_decodes_field_for_field() {
        let wire = Bytes::from(hex(
            "d16c0200 da170000 68010000 00000000 4d00000000000000 01 cc26 \
                 00000000 0000 00 6f6c0200 17",
        ));
        let data = PetData::try_from(wire).expect("39-byte capture parses");
        assert_eq!(data.unique_id, 0x0002_6cd1);
        assert_eq!(data.ref_obj_id, 6106);

        let resolver = MockResolver(ItemClass::Expendable { tid3: 1, tid4: 1 });
        let body = data
            .body(CosKind::GrowthPet, &resolver)
            .expect("a growth-pet body");

        assert_eq!(body.hp, 360);
        assert_eq!(body.unk_b, 0);
        assert_eq!(
            body.growth,
            Some(CosGrowth {
                exp: 77,
                level: 1,
                hgp: 9_932,
            })
        );
        // 99.32 % — the reading that names the field.
        assert!(body.hgp_or_full() < COS_HGP_FULL);
        assert_eq!(body.unk_f, Some(0)); // settings: not offensive
        assert_eq!(body.name.as_deref(), Some("")); // never renamed
        assert_eq!(body.inventory_size, 0);
        assert_eq!(body.unk_g, Some(0x0002_6c6f)); // the owning player
        assert_eq!(body.unk_h, Some(0x17));
    }

    /// The same bytes read as a vehicle are a *different* record: no growth
    /// block, no `unk_f`, no name, no `unk_g`/`unk_h`. This is what the old
    /// three fixed tails could not express — the gates are per kind, and the
    /// kind is not in the packet.
    #[test]
    fn a_vehicle_body_has_none_of_the_pet_only_blocks() {
        let mut tail = 120u32.to_le_bytes().to_vec(); // A
        tail.extend_from_slice(&300u32.to_le_bytes()); // B
        tail.push(0); // inventory_size -> no list

        let resolver = MockResolver(ItemClass::Expendable { tid3: 1, tid4: 1 });
        let body = pet_data(tail)
            .body(CosKind::Vehicle, &resolver)
            .expect("vehicle body should decode");

        assert_eq!((body.hp, body.unk_b), (120, 300));
        assert_eq!(body.growth, None);
        assert_eq!(body.unk_f, None);
        assert_eq!(body.name, None);
        assert_eq!(body.inventory_size, 0);
        assert!(body.items.is_empty());
        // kind 0 is one of the two kinds without the trailing u32.
        assert_eq!(body.unk_g, None);
        assert_eq!(body.unk_h, None);
        // No growth block -> the kind does not starve, so a full bar.
        assert_eq!(body.hgp_or_full(), COS_HGP_FULL);
    }

    /// A transport (kind 1) does carry the trailing `u32` — `kind not in {0, 4}`
    /// — but still no name and no growth block.
    #[test]
    fn a_transport_body_carries_only_the_trailing_u32() {
        let mut tail = 1u32.to_le_bytes().to_vec();
        tail.extend_from_slice(&2u32.to_le_bytes());
        tail.push(0); // inventory_size
        tail.extend_from_slice(&99u32.to_le_bytes()); // G

        let resolver = MockResolver(ItemClass::Expendable { tid3: 1, tid4: 1 });
        let body = pet_data(tail)
            .body(CosKind::Transport, &resolver)
            .expect("transport body should decode");

        assert_eq!(body.name, None);
        assert_eq!(body.unk_g, Some(99));
        assert_eq!(body.unk_h, None);
    }

    /// The original only reads past `inventory_size` when the uid is new to its
    /// COS map (`FUN_009a1900:204`), so a re-summon body that stops right there
    /// is legal and must not be rejected (#455).
    #[test]
    fn a_body_that_stops_after_inventory_size_is_still_valid() {
        let mut tail = 11u32.to_le_bytes().to_vec();
        tail.extend_from_slice(&22u32.to_le_bytes());
        tail.extend_from_slice(&123_456u64.to_le_bytes());
        tail.push(14);
        tail.extend_from_slice(&5_000u16.to_le_bytes());
        tail.extend_from_slice(&33u32.to_le_bytes());
        tail.extend_from_slice(&ascii("Wolfie"));
        tail.push(6); // inventory_size, and then nothing

        let resolver = MockResolver(ItemClass::Expendable { tid3: 1, tid4: 1 });
        let body = pet_data(tail)
            .body(CosKind::GrowthPet, &resolver)
            .expect("an under-read body is legal");

        assert_eq!(body.name.as_deref(), Some("Wolfie"));
        assert_eq!(body.inventory_size, 6);
        assert!(body.items.is_empty());
        assert_eq!(body.unk_g, None);
        assert_eq!(body.unk_h, None);
    }

    /// A truncated item must not yield half a list — a short record shifts every
    /// later one, so the whole body fails.
    #[test]
    fn a_truncated_item_list_is_none_not_partial() {
        let mut tail = 0u32.to_le_bytes().to_vec();
        tail.extend_from_slice(&0u32.to_le_bytes());
        tail.extend_from_slice(&0u32.to_le_bytes()); // F
        tail.extend_from_slice(&ascii("Grabby"));
        tail.push(6); // inventory_size
        tail.push(2); // claims two items, supplies one
        tail.push(3);
        tail.extend_from_slice(&0u32.to_le_bytes());
        tail.extend_from_slice(&5000u32.to_le_bytes());
        tail.extend_from_slice(&25u16.to_le_bytes());

        let resolver = MockResolver(ItemClass::Expendable { tid3: 1, tid4: 1 });
        assert!(pet_data(tail).body(CosKind::GrabPet, &resolver).is_none());
    }

    /// Reading a body with the wrong kind costs a `None` or a nonsense record,
    /// never a failed packet — which is why the kind is resolved outside the
    /// decoder.
    #[test]
    fn a_short_body_returns_none_instead_of_failing_the_packet() {
        let resolver = MockResolver(ItemClass::Expendable { tid3: 1, tid4: 1 });
        let data = pet_data(vec![0x01, 0x02, 0x03]);

        assert!(data.body(CosKind::GrowthPet, &resolver).is_none());
        assert!(data.body(CosKind::Vehicle, &resolver).is_none());
    }

    #[test]
    fn pet_update_hungry_carries_only_the_hunger_value() {
        let mut wire = 42u32.to_le_bytes().to_vec();
        wire.push(PET_UPDATE_HUNGRY);
        wire.extend_from_slice(&650u16.to_le_bytes());

        let decoded = PetUpdate::try_from(Bytes::from(wire)).unwrap();

        assert_eq!(decoded.unique_id, 42);
        assert_eq!(decoded.payload, PetUpdatePayload::Hunger { hgp: 650 });
        assert_eq!(decoded.hgp(), Some(650));
        assert_eq!(decoded.new_ref_obj_id(), None);
    }

    #[test]
    fn pet_update_model_changed_carries_the_new_ref_object() {
        let mut wire = 42u32.to_le_bytes().to_vec();
        wire.push(PET_UPDATE_MODEL_CHANGED);
        wire.extend_from_slice(&2010u32.to_le_bytes());

        let decoded = PetUpdate::try_from(Bytes::from(wire)).unwrap();

        assert_eq!(decoded.new_ref_obj_id(), Some(2010));
        assert_eq!(decoded.hgp(), None);
    }

    /// Arm 3 is `{i64, u32}` — the widths this module once declared [U]. The
    /// delta is **signed**: a pet losing exp on death is the negative case.
    #[test]
    fn pet_update_exp_reads_a_signed_delta_and_its_source() {
        let mut wire = 42u32.to_le_bytes().to_vec();
        wire.push(PET_UPDATE_EXP);
        wire.extend_from_slice(&(-1500i64).to_le_bytes());
        wire.extend_from_slice(&909u32.to_le_bytes());

        let decoded = PetUpdate::try_from(Bytes::from(wire.clone())).unwrap();

        assert_eq!(
            decoded.payload,
            PetUpdatePayload::Exp {
                delta: -1500,
                source_unique_id: 909,
            }
        );
        assert_eq!(&Bytes::from(decoded)[..], &wire[..]);
    }

    /// Arm 5 is how a rename actually reaches the client — 0xB117's success
    /// body is empty.
    #[test]
    fn pet_update_rename_reads_the_new_name() {
        let mut wire = 42u32.to_le_bytes().to_vec();
        wire.push(PET_UPDATE_RENAMED);
        wire.extend_from_slice(&4u16.to_le_bytes());
        wire.extend_from_slice(b"Nuri");

        let decoded = PetUpdate::try_from(Bytes::from(wire.clone())).unwrap();

        assert_eq!(
            decoded.payload,
            PetUpdatePayload::Renamed {
                name: "Nuri".to_string(),
            }
        );
        assert_eq!(&Bytes::from(decoded)[..], &wire[..]);
    }

    /// Arm 2 keeps its item records raw because their width depends on the
    /// loaded itemdata — the same split 0x30C8's body uses.
    #[test]
    fn pet_update_bag_reads_capacity_and_defers_the_items() {
        let mut wire = 42u32.to_le_bytes().to_vec();
        wire.push(PET_UPDATE_BAG);
        wire.push(28); // slot capacity
        wire.push(0); // no items
        let decoded = PetUpdate::try_from(Bytes::from(wire)).unwrap();

        let PetUpdatePayload::Bag {
            slot_capacity,
            item_count,
            ..
        } = decoded.payload
        else {
            panic!("expected a bag arm");
        };
        assert_eq!((slot_capacity, item_count), (28, 0));
        let resolver = MockResolver(ItemClass::Expendable { tid3: 1, tid4: 1 });
        assert_eq!(decoded.bag_items(&resolver).unwrap(), Vec::new());
    }

    /// Arm 6's u32 is discarded by the original but still occupies the body,
    /// and an arm neither side knows must keep its bytes rather than fail.
    #[test]
    fn pet_update_unknown_arms_keep_their_bytes() {
        let mut wire = 42u32.to_le_bytes().to_vec();
        wire.push(PET_UPDATE_UNKNOWN_6);
        wire.extend_from_slice(&7u32.to_le_bytes());
        let decoded = PetUpdate::try_from(Bytes::from(wire.clone())).unwrap();
        assert_eq!(decoded.payload, PetUpdatePayload::Unknown6 { value: 7 });
        assert_eq!(&Bytes::from(decoded)[..], &wire[..]);

        let mut wire = 42u32.to_le_bytes().to_vec();
        wire.push(99);
        wire.extend_from_slice(&[0xAA, 0xBB]);
        let decoded = PetUpdate::try_from(Bytes::from(wire.clone())).unwrap();
        assert_eq!(decoded.update_type, 99);
        assert_eq!(&Bytes::from(decoded)[..], &wire[..]);
    }

    #[test]
    fn pet_update_unsummoned_has_an_empty_body() {
        let mut wire = 42u32.to_le_bytes().to_vec();
        wire.push(PET_UPDATE_UNSUMMONED);

        let decoded = PetUpdate::try_from(Bytes::from(wire)).unwrap();

        assert_eq!(decoded.payload, PetUpdatePayload::Unsummoned);
    }

    #[test]
    fn pet_player_mounted_reads_the_ridden_pet_when_mounting() {
        let mut wire = vec![1u8]; // success
        wire.extend_from_slice(&777u32.to_le_bytes());
        wire.push(1); // is_mounting
        wire.extend_from_slice(&888u32.to_le_bytes());

        let decoded = PetPlayerMounted::try_from(Bytes::from(wire)).unwrap();

        assert!(decoded.success);
        assert_eq!(decoded.player_unique_id, Some(777));
        assert_eq!(decoded.is_mounting, Some(true));
        assert_eq!(decoded.riding_unique_id, Some(888));
    }

    /// The regression this module shipped with: dismounting still carries the
    /// transport uid. These are the exact bytes of `packet_dump/0xb0cb.log`
    /// line 2 — 10 bytes of which we used to consume 6.
    #[test]
    fn pet_player_mounted_reads_the_ridden_pet_when_dismounting_too() {
        let wire =
            Bytes::from_static(&[0x01, 0x73, 0xe4, 0x01, 0x00, 0x00, 0xd5, 0xe4, 0x01, 0x00]);

        let decoded = PetPlayerMounted::try_from(wire.clone()).unwrap();

        assert_eq!(decoded.player_unique_id, Some(0x0001_e473));
        assert_eq!(decoded.is_mounting, Some(false));
        assert_eq!(decoded.riding_unique_id, Some(0x0001_e4d5));
        // and every one of the ten bytes is accounted for
        assert_eq!(&Bytes::from(decoded)[..], &wire[..]);
    }

    #[test]
    fn pet_player_mounted_failure_carries_an_error_code() {
        let decoded = PetPlayerMounted::try_from(Bytes::from_static(&[2, 0x0d, 0x30])).unwrap();

        assert!(!decoded.success);
        assert_eq!(decoded.player_unique_id, None);
        assert_eq!(decoded.riding_unique_id, None);
        assert_eq!(decoded.error_code, Some(0x300d));
    }

    /// The `u32` is present on **both** settings types — gating it on type 1
    /// left 4 bytes unread on every cash-pet ack.
    #[test]
    fn pet_settings_response_reads_the_settings_word_for_either_type() {
        for settings_type in [PET_SETTINGS_TYPE_GOLD, 2] {
            let mut wire = vec![1u8];
            wire.extend_from_slice(&55u32.to_le_bytes());
            wire.push(settings_type);
            wire.extend_from_slice(&AttackPetSettings::OFFENSIVE.to_le_bytes());

            let decoded = PetSettingsChangeResponse::try_from(Bytes::from(wire.clone())).unwrap();

            assert_eq!(decoded.unique_id, Some(55));
            assert_eq!(decoded.settings_type, Some(settings_type));
            assert!(AttackPetSettings(decoded.settings.unwrap()).is_offensive());
            assert_eq!(&Bytes::from(decoded)[..], &wire[..]);
        }
    }

    #[test]
    fn pet_settings_response_failure_carries_an_error_code() {
        let decoded =
            PetSettingsChangeResponse::try_from(Bytes::from_static(&[2, 0x11, 0x4c])).unwrap();

        assert!(!decoded.success);
        assert_eq!(decoded.settings, None);
        assert_eq!(decoded.error_code, Some(0x4c11));
    }

    /// 0xB0C5's error code sits **between** the action echo and the cos uid,
    /// so the uid's offset moves with `result`. All four sizes must round-trip.
    #[test]
    fn pet_action_response_places_the_uid_after_the_error_code() {
        // success, non-pick action: 7 bytes
        let mut wire = vec![1u8, PET_ACTION_ATTACK];
        wire.extend_from_slice(&55u32.to_le_bytes());
        let decoded = PetActionResponse::try_from(Bytes::from(wire.clone())).unwrap();
        assert!(decoded.success);
        assert_eq!(decoded.unique_id, 55);
        assert_eq!(decoded.item_gid, None);
        assert_eq!(&Bytes::from(decoded)[..], &wire[..]);

        // success, pick action: 11 bytes, the trailing GID present
        let mut wire = vec![1u8, PET_ACTION_ITEM_PICKUP];
        wire.extend_from_slice(&55u32.to_le_bytes());
        wire.extend_from_slice(&909u32.to_le_bytes());
        let decoded = PetActionResponse::try_from(Bytes::from(wire.clone())).unwrap();
        assert_eq!(decoded.item_gid, Some(909));
        assert_eq!(&Bytes::from(decoded)[..], &wire[..]);

        // error on attack: 9 bytes, uid pushed to offset 4 — the server's own
        // 0x3009 case (server-dec/004ecbc0)
        let mut wire = vec![2u8, PET_ACTION_ATTACK];
        wire.extend_from_slice(&0x3009u16.to_le_bytes());
        wire.extend_from_slice(&55u32.to_le_bytes());
        let decoded = PetActionResponse::try_from(Bytes::from(wire.clone())).unwrap();
        assert!(!decoded.success);
        assert_eq!(decoded.error_code, Some(0x3009));
        assert_eq!(decoded.unique_id, 55);
        // Length, not bytes: `success: bool` re-serializes the failure marker
        // as 0 rather than the wire's 2 (the tree-wide `u8 == 1` bool rule),
        // which costs nothing on an S→C packet we never send. What matters is
        // that all nine bytes were consumed in the right order.
        assert_eq!(Bytes::from(decoded).len(), wire.len());

        // error on pick: 13 bytes — error 0x4414, and the GID still follows
        let mut wire = vec![2u8, PET_ACTION_ITEM_PICKUP];
        wire.extend_from_slice(&0x4414u16.to_le_bytes());
        wire.extend_from_slice(&55u32.to_le_bytes());
        wire.extend_from_slice(&909u32.to_le_bytes());
        let decoded = PetActionResponse::try_from(Bytes::from(wire.clone())).unwrap();
        assert_eq!(decoded.error_code, Some(0x4414));
        assert_eq!(decoded.unique_id, 55);
        assert_eq!(decoded.item_gid, Some(909));
        assert_eq!(Bytes::from(decoded).len(), wire.len());
    }

    /// The three COS acks whose success body is empty. `PetTerminateResponse`
    /// additionally takes the error path on **any** non-1 byte.
    #[test]
    fn empty_success_cos_acks_roundtrip() {
        let ok = PetUnsummonResponse::try_from(Bytes::from_static(&[1])).unwrap();
        assert!(ok.success && ok.error_code.is_none());
        let err = PetUnsummonResponse::try_from(Bytes::from_static(&[2, 3, 0])).unwrap();
        assert_eq!(err.error_code, Some(3));

        let ok = PetRenameResponse::try_from(Bytes::from_static(&[1])).unwrap();
        assert!(ok.success && ok.error_code.is_none());

        let err = PetTerminateResponse::try_from(Bytes::from_static(&[7, 4, 0])).unwrap();
        assert!(!err.success);
        assert_eq!(err.error_code, Some(4));
    }

    #[test]
    fn pet_rename_request_roundtrips() {
        let req = PetRenameRequest {
            unique_id: 55,
            name: "Nuri".to_string(),
        };
        let wire: Bytes = req.clone().into();

        assert_eq!(wire.len(), 4 + 2 + 4);
        assert_eq!(PetRenameRequest::try_from(wire).unwrap(), req);
    }

    #[test]
    fn stuck_distance_warning_is_one_reason_byte() {
        let decoded = StuckDistanceWarning::try_from(Bytes::from_static(&[1])).unwrap();
        assert_eq!(decoded.reason, STUCK_REASON_TRADE_CART);
    }

    #[test]
    fn pet_unsummon_request_roundtrips() {
        let req = PetUnsummonRequest { unique_id: 4242 };
        let wire: Bytes = req.clone().into();

        assert_eq!(&wire[..], &4242u32.to_le_bytes());
        assert_eq!(PetUnsummonRequest::try_from(wire).unwrap(), req);
    }

    /// Builder `0081eb50` writes `b1 b4` — the state byte comes **first**.
    #[test]
    fn pet_mount_request_puts_the_state_byte_first() {
        let req = PetMountRequest {
            mount_state: 1,
            cos_unique_id: 4711,
        };
        let wire: Bytes = req.clone().into();

        assert_eq!(wire.len(), 5);
        assert_eq!(wire[0], 1);
        assert_eq!(&wire[1..], &4711u32.to_le_bytes());
        assert_eq!(PetMountRequest::try_from(wire).unwrap(), req);
    }

    /// The three 9-byte `{uid, action, u32}` commands and the bare follow.
    #[test]
    fn pet_action_commands_roundtrip() {
        let attack = PetActionRequest::Attack {
            pet_unique_id: 55,
            target_unique_id: 909,
        };
        let wire: Bytes = attack.clone().into();
        assert_eq!(wire.len(), 9);
        assert_eq!(wire[4], PET_ACTION_ATTACK);
        assert_eq!(PetActionRequest::try_from(wire).unwrap(), attack);

        let turn = PetActionRequest::Turn {
            pet_unique_id: 55,
            heading: 0x4000,
        };
        let wire: Bytes = turn.clone().into();
        assert_eq!(wire.len(), 7);
        assert_eq!(PetActionRequest::try_from(wire).unwrap(), turn);

        let follow = PetActionRequest::Follow { pet_unique_id: 55 };
        let wire: Bytes = follow.clone().into();
        assert_eq!(wire.len(), 5);
        assert_eq!(PetActionRequest::try_from(wire).unwrap(), follow);
    }

    /// Overworld regions use 2-byte coordinates.
    #[test]
    fn pet_movement_uses_short_coords_in_the_overworld() {
        let req = PetActionRequest::Movement {
            pet_unique_id: 9,
            region: 0x60A8,
            x: 1058,
            y: -8,
            z: 1426,
        };
        let wire: Bytes = req.clone().into();

        // uid(4) + action(1) + movement_type(1) + region(2) + 3x i16
        assert_eq!(wire.len(), 14);
        assert_eq!(PetActionRequest::try_from(wire).unwrap(), req);
    }

    /// Dungeon regions widen them to 4 bytes (`is_dungeon`, bit 15).
    #[test]
    fn pet_movement_uses_int_coords_in_a_dungeon() {
        let req = PetActionRequest::Movement {
            pet_unique_id: 9,
            region: 0x8001,
            x: 100_000,
            y: -20,
            z: 250_000,
        };
        let wire: Bytes = req.clone().into();

        // uid(4) + action(1) + movement_type(1) + region(2) + 3x i32
        assert_eq!(wire.len(), 20);
        assert_eq!(PetActionRequest::try_from(wire).unwrap(), req);
    }

    #[test]
    fn pet_item_pickup_roundtrips() {
        let req = PetActionRequest::ItemPickUp {
            pet_unique_id: 9,
            item_unique_id: 0x0BAD_F00D,
        };
        let wire: Bytes = req.clone().into();

        assert_eq!(wire.len(), 9);
        assert_eq!(PetActionRequest::try_from(wire).unwrap(), req);
    }

    /// An action code no builder in any source emits keeps its bytes instead
    /// of being mis-sliced into a typed shape. (Action 4 used to stand in for
    /// "unknown" here; it is [`PetActionRequest::Turn`] now.)
    #[test]
    fn an_unknown_pet_action_keeps_its_raw_tail() {
        let mut wire = 9u32.to_le_bytes().to_vec();
        wire.push(42); // none of the six known action codes
        wire.extend_from_slice(&[0xDE, 0xAD]);

        let decoded = PetActionRequest::try_from(Bytes::from(wire.clone())).unwrap();

        assert_eq!(
            decoded,
            PetActionRequest::Unknown {
                pet_unique_id: 9,
                action: 42,
                tail: Bytes::from_static(&[0xDE, 0xAD]),
            }
        );
        let back: Bytes = decoded.into();
        assert_eq!(&back[..], &wire[..]);
    }

    /// A movement sub-type other than move-to-position is [U], so it degrades to
    /// `Unknown` with the sub-type byte preserved rather than being decoded.
    #[test]
    fn an_unbuilt_movement_subtype_degrades_to_unknown() {
        let mut wire = 9u32.to_le_bytes().to_vec();
        wire.push(PET_ACTION_MOVEMENT);
        wire.push(2); // not MOVEMENT_TO_POSITION
        wire.extend_from_slice(&[0x01, 0x02]);

        let decoded = PetActionRequest::try_from(Bytes::from(wire.clone())).unwrap();

        assert_eq!(
            decoded,
            PetActionRequest::Unknown {
                pet_unique_id: 9,
                action: PET_ACTION_MOVEMENT,
                tail: Bytes::from_static(&[2, 0x01, 0x02]),
            }
        );
        let back: Bytes = decoded.into();
        assert_eq!(&back[..], &wire[..]);
    }
}
