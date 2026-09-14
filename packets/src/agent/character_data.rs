//! The CHARACTER_DATA (0x3013) body: wire structs and the staged parser.
//!
//! Idea: the blob is one large record whose layout is transcribed from the
//! go-sro server's write calls (`char_selection_join_handler.go`,
//! `packetutils_item.go`); see `docs/net-character-data-0x3013.md`. Every
//! section that is expressible on wire data alone is a derive-based struct
//! (`when`-conditionals for rent info / transport, `break`-lists for
//! masteries/skills, `by-size-field` for quests). The one thing the derives
//! cannot express is the inventory: an item's sub-record shape depends on its
//! itemdata type, which lives in the *client's* data tables — so the item
//! branch is selected through the [`ItemClassResolver`] trait and the whole
//! record is parsed by [`parse_character_info`] instead of a single derive
//! (which also enables the staged fail-safe below). That is why the packet
//! itself (`CharacterDataBody` in `ingame.rs`) still carries raw bytes.
//!
//! Each section is a stage; the first read error stops the forward pass and
//! records the stage name, so a layout drift degrades to a partial result
//! instead of a desync. Because a mis-parsed variable-length middle would
//! silently shift the tail, two guards protect it: the tail's position block
//! must be plausible, and the forward-parsed unique id is cross-checked
//! against the id from `CelestialPosition` (0x3020). On disagreement the
//! variable middle is discarded and the tail is rebuilt via the anchor scan —
//! find the known unique id's little-endian bytes and read the position that
//! immediately follows (go-sro writes `UniqueID` then `WritePosition`), then
//! continue with movement/state/name from there. The spawn position and the
//! movement speeds therefore never depend on the riskiest parsing.
//!
//! Anchor verified against a live vSRO 1.88 capture (offset 501: unique_id
//! `0x56228` → region `0x60A8`, x 1058.0, y -7.68, z 1426.0 — a Jangan spawn).

use std::io::{Cursor, Read};

use byteorder::ReadBytesExt;

use sro_macro::ByteSize;
use sro_macro::Deserialize;
use sro_macro::SerializationError;
use sro_macro::Serialize;
use sro_macro_derive::*;

/// Wire class of an inventory item, selecting its sub-record shape (mirrors
/// go-sro's `WriteInventoryItem` branches over the itemdata `TypeID2..4`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ItemClass {
    /// `TID2 == 1`: weapons, armor, accessories, shields.
    Equipment,
    /// `TID2 == 2`: summon-scroll containers (COS pets, transformation
    /// monsters, magic cubes); the sub-shape depends on `TID3`/`TID4`.
    Container { tid3: u32, tid4: u32 },
    /// `TID2 == 3`: consumables.
    Expendable { tid3: u32, tid4: u32 },
    /// Not an item or missing type columns — record layout unknown.
    Unknown,
}

/// Classifies an item ref id via the game's itemdata table. The table lives in
/// the client, so the parser takes the lookup as a trait instead of the data.
pub trait ItemClassResolver {
    fn item_class(&self, ref_id: u32) -> ItemClass;
}

/// The fixed stat block at the head of the record (go-sro
/// `WriteCharDataToPacket`, preceded by the 4-byte server time).
#[derive(Serialize, Deserialize, ByteSize, Clone, Copy, Debug, PartialEq)]
pub struct CharacterStats {
    pub server_time: u32,
    pub ref_id: u32,
    pub scale: u8,
    pub level: u8,
    pub max_level: u8,
    pub exp: u64,
    pub skill_exp: u32,
    pub gold: u64,
    pub skill_points: u32,
    pub stat_points: u16,
    pub berserk_points: u8,
    pub unk_u32: u32,
    pub hp: u32,
    pub mp: u32,
    pub auto_invest_exp: u8,
    pub daily_pk: u8,
    pub total_pk: u16,
    pub pk_penalty: u32,
    pub berserk_level: u8,
    pub free_pvp: u8,
}

/// Item rent info: a type selector and its type-dependent fields (go-sro
/// `WriteRentInfo`).
#[derive(Serialize, Deserialize, ByteSize, Clone, Copy, Debug, Default, PartialEq)]
pub struct RentInfo {
    pub rent_type: u32,
    #[sro_packet(when = "rent_type == 1 || rent_type == 2 || rent_type == 3")]
    pub can_delete: Option<u16>,
    #[sro_packet(when = "rent_type == 2 || rent_type == 3")]
    pub can_recharge: Option<u16>,
    #[sro_packet(when = "rent_type == 1 || rent_type == 3")]
    pub period_begin: Option<u32>,
    #[sro_packet(when = "rent_type == 1 || rent_type == 3")]
    pub period_end: Option<u32>,
    #[sro_packet(when = "rent_type == 2")]
    pub meter_rate: Option<u32>,
    #[sro_packet(when = "rent_type == 3")]
    pub packing_time: Option<u32>,
}

/// A magic ("blue") parameter on an item.
#[derive(Serialize, Deserialize, ByteSize, Clone, Copy, Debug, PartialEq)]
pub struct MagicParam {
    pub kind: u32,
    pub value: u32,
}

/// A socket or advanced-elixir binding option on an equipment item.
#[derive(Serialize, Deserialize, ByteSize, Clone, Copy, Debug, PartialEq)]
pub struct BindingOption {
    pub slot: u8,
    pub id: u32,
    pub value: u32,
}

/// The equipment item sub-record: enhancement, variance, durability, magic
/// params, then the two tagged binding-option blocks (sockets, adv. elixirs).
#[derive(Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct EquipmentData {
    pub opt_level: u8,
    pub variance: u64,
    pub durability: u32,
    pub mag_params: Vec<MagicParam>,
    /// Binding-option tag, `1` = sockets.
    pub socket_tag: u8,
    pub sockets: Vec<BindingOption>,
    /// Binding-option tag, `2` = advanced elixirs.
    pub elixir_tag: u8,
    pub adv_elixirs: Vec<BindingOption>,
}

/// `SRCoS.State` (`Game/Objects/Item/SRCoS.cs:43-49`), the COS container's
/// summon state. It arrives in the item record and is updated in place by
/// `0x3040` mask bit `0x40`.
///
/// A scroll that has never been summoned has no pet behind it yet, so the
/// server writes the state byte and nothing else.
pub const COS_STATE_NEVER_SUMMONED: u8 = 1;
pub const COS_STATE_SUMMONED: u8 = 2;
pub const COS_STATE_UNSUMMONED: u8 = 3;
/// The pet behind the scroll is dead. It cannot be summoned again until it is
/// revived — a `0x704C` use answers `0xB04C` error
/// [`ITEM_USE_ERROR_COS_NOT_SUMMONABLE`](crate::agent::inventory::ITEM_USE_ERROR_COS_NOT_SUMMONABLE)
/// (observed live 2026-08-18).
pub const COS_STATE_DEAD: u8 = 4;

/// The class-dependent part of an inventory item. Selected via
/// [`ItemClassResolver`], which is why [`InventoryItem`] is not a plain derive.
#[derive(Debug, Clone, PartialEq)]
pub enum ItemTypeData {
    Equipment(EquipmentData),
    /// COS pet container (`TID3 == 1`). Everything past `state` is absent
    /// while the scroll has never been summoned — see [`COS_STATE_NEVER_SUMMONED`].
    CosPet {
        state: u8,
        cos_ref_id: Option<u32>,
        name: Option<String>,
        /// Present for rentable pets (`TID4 == 2`) that carry a pet.
        rent_seconds: Option<u32>,
        unk: Option<u8>,
    },
    /// Transformation-monster scroll (`TID3 == 2`): the mask's ref id.
    TransformScroll {
        mask_ref_id: u32,
    },
    /// Magic cube (`TID3 == 3`): number of contained elixirs.
    MagicCube {
        elixir_count: u32,
    },
    Expendable {
        stack_count: u16,
        /// Magic/attribute stones (`TID3 == 11`, `TID4 ∈ {1, 2}`).
        assimilation_prob: Option<u8>,
        /// Gacha cards carry their own magic-param list.
        mag_params: Vec<MagicParam>,
    },
    /// A *known-class* item whose sub-type falls outside go-sro's
    /// `WriteContainerItem` switch — the server writes NO body for those, so
    /// the zero-byte record is real. An *unresolvable class*
    /// (`ItemClass::Unknown`) is NOT this case: the server knew a category
    /// and wrote its body (live-falsified 2026-08-11 — go-sro granted
    /// starter-kit expendable 46551, absent from every known client itemdata,
    /// with a `stack:u16` body; the old zero-byte assumption shifted every
    /// later record and forged a structurally-valid fake tail). `read_body`
    /// now fails fast on `ItemClass::Unknown` instead — unless a resync
    /// proved the body's width from the whole blob (`recovered_body`), in
    /// which case the skipped bytes land here too, uninterpreted.
    Unknown,
}

/// One inventory item: slot, rent info, ref id, and the class-dependent body.
#[derive(Debug, Clone, PartialEq)]
pub struct InventoryItem {
    pub slot: u8,
    pub rent: RentInfo,
    pub ref_id: u32,
    pub data: ItemTypeData,
}

impl InventoryItem {
    /// Read one item record; the resolver supplies the itemdata class that
    /// selects the sub-record shape (go-sro `WriteInventoryItem`).
    pub fn read_with<T: Read + ReadBytesExt>(
        reader: &mut T,
        resolver: &impl ItemClassResolver,
    ) -> Result<Self, SerializationError> {
        let slot = u8::read_from(reader)?;
        let rent = RentInfo::read_from(reader)?;
        let ref_id = u32::read_from(reader)?;
        let data = ItemTypeData::read_body(reader, resolver.item_class(ref_id), ref_id)?;
        Ok(InventoryItem {
            slot,
            rent,
            ref_id,
            data,
        })
    }
}

impl ItemTypeData {
    /// The COS container's summon state, for the two callers that care what a
    /// scroll's pet is doing (the inventory's tint and its tooltip).
    pub fn cos_state(&self) -> Option<u8> {
        match self {
            ItemTypeData::CosPet { state, .. } => Some(*state),
            _ => None,
        }
    }

    /// Whether this is a COS scroll whose pet is dead — the state that makes it
    /// unusable until revived.
    pub fn cos_is_dead(&self) -> bool {
        self.cos_state() == Some(COS_STATE_DEAD)
    }

    /// Read the class-dependent body that follows an item's slot/rent/ref
    /// header (go-sro `WriteInventoryItem`'s routing). `ref_id` is context
    /// for the unresolvable-class error.
    fn read_body<T: Read + ReadBytesExt>(
        reader: &mut T,
        class: ItemClass,
        ref_id: u32,
    ) -> Result<Self, SerializationError> {
        let data = match class {
            ItemClass::Equipment => ItemTypeData::Equipment(EquipmentData::read_from(reader)?),
            ItemClass::Container { tid3, tid4 } => match tid3 {
                1 => {
                    // xBot `PacketParser.cs:486-494` writes the four trailing
                    // fields only once a pet exists behind the scroll, so a
                    // freshly bought one is a single state byte. Reading them
                    // unconditionally desynced the rest of the item list.
                    let state = u8::read_from(reader)?;
                    let summoned = state != COS_STATE_NEVER_SUMMONED;
                    ItemTypeData::CosPet {
                        state,
                        cos_ref_id: if summoned {
                            Some(u32::read_from(reader)?)
                        } else {
                            None
                        },
                        name: if summoned {
                            Some(read_string(reader)?)
                        } else {
                            None
                        },
                        rent_seconds: if summoned && tid4 == 2 {
                            Some(u32::read_from(reader)?)
                        } else {
                            None
                        },
                        unk: if summoned {
                            Some(u8::read_from(reader)?)
                        } else {
                            None
                        },
                    }
                }
                2 => ItemTypeData::TransformScroll {
                    mask_ref_id: u32::read_from(reader)?,
                },
                3 => ItemTypeData::MagicCube {
                    elixir_count: u32::read_from(reader)?,
                },
                // go-sro's WriteContainerItem switch writes nothing for other
                // TID3 values — a zero-byte body (see ItemTypeData::Unknown)
                _ => ItemTypeData::Unknown,
            },
            ItemClass::Expendable { tid3, tid4 } => {
                let stack_count = u16::read_from(reader)?;
                let assimilation_prob = if tid3 == 11 && (tid4 == 1 || tid4 == 2) {
                    Some(u8::read_from(reader)?)
                } else {
                    None
                };
                // Gacha cards only (`TID3 == 14`, WIN/LOSE via TID4). go-sro's
                // source has `TypeID3 == 14 || TypeID4 == 2`, but a live
                // capture proved its server data never hits the `TID4 == 2`
                // arm for ordinary expendables: an MP potion (TID3 1, TID4 2)
                // arrived stack-only, and taking that arm swallowed 120
                // phantom mag-param bytes and derailed the whole section.
                let mag_params = if tid3 == 14 {
                    let count = u8::read_from(reader)?;
                    let mut params = Vec::with_capacity(count as usize);
                    for _ in 0..count {
                        params.push(MagicParam::read_from(reader)?);
                    }
                    params
                } else {
                    Vec::new()
                };
                ItemTypeData::Expendable {
                    stack_count,
                    assimilation_prob,
                    mag_params,
                }
            }
            ItemClass::Unknown => {
                // The server wrote a body of unknown width for this record —
                // pretending it is zero bytes would shift every following
                // record (see the variant doc). Fail the section instead.
                return Err(SerializationError::UnknownVariation(
                    ref_id as usize,
                    "unresolvable item ref — record width unknown",
                ));
            }
        };
        Ok(data)
    }
}

#[derive(Serialize, Deserialize, ByteSize, Clone, Copy, Debug, PartialEq)]
pub struct Mastery {
    pub id: u32,
    pub level: u8,
}

#[derive(Serialize, Deserialize, ByteSize, Clone, Copy, Debug, PartialEq)]
pub struct KnownSkill {
    pub id: u32,
    /// Raw wire byte; 1 = enabled on live servers.
    pub enabled: u8,
}

/// The mastery + skill section: both are `1 → entry, 2 → end` tagged lists
/// (the derive's `break` list type), each preceded by an unknown byte.
#[derive(Serialize, Deserialize, ByteSize, Clone, Debug, Default, PartialEq)]
pub struct MasterySkillSection {
    pub unk_begin: u8,
    #[sro_packet(list_type = "break")]
    pub masteries: Vec<Mastery>,
    pub unk_mid: u8,
    #[sro_packet(list_type = "break")]
    pub skills: Vec<KnownSkill>,
}

/// The quest section head: completed quest ids plus the active-quest count.
/// Active quest records have an unverified shape, so the parser only proceeds
/// when the count is zero (go-sro sends none).
#[derive(Serialize, Deserialize, ByteSize, Clone, Debug, Default, PartialEq)]
pub struct QuestSection {
    pub completed_count: u16,
    #[sro_packet(list_type = "by-size-field", size_field = "completed_count")]
    pub completed: Vec<u32>,
    pub active_count: u8,
}

/// The collection-book head; entry shape unverified, so the parser only
/// proceeds when no theme was started (go-sro sends none).
#[derive(Serialize, Deserialize, ByteSize, Clone, Copy, Debug, Default, PartialEq)]
pub struct CollectionBook {
    pub unk: u8,
    pub started_theme_count: u32,
}

/// SRO position as written by go-sro's `WritePosition`: a region id,
/// region-local float coordinates and a heading. Shared by CHARACTER_DATA and
/// the entity-spawn records.
#[derive(Serialize, Deserialize, ByteSize, Clone, Copy, Debug, PartialEq)]
pub struct SpawnPosition {
    pub region: u16,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub heading: u16,
}

/// The movement block: either a destination or a standing turn. The
/// destination triple is 2 bytes per component in the overworld and 4 bytes in
/// a dungeon, keyed by the entity's *current* `SpawnPosition.region` — not the
/// `dest_region` field that precedes it (xBot/go-sro rule; the dest region can
/// legitimately be 0). The derive can't branch on external context, so this is
/// read via [`EntityMovement::read_with_region`]. The `dest_region > 0`
/// presence gate matches our captures; xBot reads the triple unconditionally —
/// an unresolved source conflict (`docs/re/systems/dungeon-teleport-in.md` §9).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EntityMovement {
    pub has_destination: bool,
    pub move_type: u8,
    pub dest_region: Option<u16>,
    pub dest_x: Option<i32>,
    pub dest_y: Option<i32>,
    pub dest_z: Option<i32>,
    /// Standing only: 0 = spinning, 1 = sky-/key-walking.
    pub source: Option<u8>,
    pub angle: Option<u16>,
}

impl EntityMovement {
    /// Read the movement block; `current_region` (the entity's own position
    /// region, read just before this block) selects the coordinate width.
    pub fn read_with_region<T: Read + ReadBytesExt>(
        reader: &mut T,
        current_region: u16,
    ) -> Result<Self, SerializationError> {
        let has_destination = u8::read_from(reader)? != 0;
        let move_type = u8::read_from(reader)?;
        let mut movement = EntityMovement {
            has_destination,
            move_type,
            ..Default::default()
        };
        if has_destination {
            let dest_region = u16::read_from(reader)?;
            movement.dest_region = Some(dest_region);
            if dest_region > 0 {
                let mut coord = || -> Result<i32, SerializationError> {
                    if super::ingame::is_dungeon(current_region) {
                        i32::read_from(reader)
                    } else {
                        Ok(u16::read_from(reader)? as i32)
                    }
                };
                movement.dest_x = Some(coord()?);
                movement.dest_y = Some(coord()?);
                movement.dest_z = Some(coord()?);
            }
        } else {
            movement.source = Some(u8::read_from(reader)?);
            movement.angle = Some(u16::read_from(reader)?);
        }
        Ok(movement)
    }
}

/// An active buff in the character-state block.
#[derive(Serialize, Deserialize, ByteSize, Clone, Copy, Debug, PartialEq)]
pub struct ActiveBuff {
    pub ref_skill_id: u32,
    pub duration: u32,
}

/// The character-state block shared by spawn records and CHARACTER_DATA:
/// life/motion/body states, the three movement speeds, and active buffs.
#[derive(Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct EntityState {
    pub life_state: u8,
    pub unk: u8,
    pub motion_state: u8,
    pub body_state: u8,
    /// Movement speeds in game units per second.
    pub walk_speed: f32,
    pub run_speed: f32,
    pub hwan_speed: f32,
    pub buffs: Vec<ActiveBuff>,
}

/// The job block following the character name.
#[derive(Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct JobInfo {
    pub job_name: String,
    pub job_type: u8,
    pub job_level: u8,
    pub job_exp: u32,
    pub job_contribution: u32,
    pub job_reward: u32,
}

/// A hotkey-bar entry (vSRO shape; go-sro sends none).
#[derive(Serialize, Deserialize, ByteSize, Clone, Copy, Debug, PartialEq)]
pub struct Hotkey {
    pub slot: u8,
    pub kind: u8,
    pub data: u32,
}

/// Account/session flags at the end of the record.
#[derive(Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct PlayerExtras {
    pub pvp_state: u8,
    pub transport_flag: u8,
    pub in_combat: u8,
    /// Present when riding a transport.
    #[sro_packet(when = "transport_flag != 0")]
    pub transport_id: Option<u32>,
    pub pvp_flag: u8,
    pub guide_flag: u64,
    pub jid: u32,
    pub gm: bool,
    pub activation_flag: u8,
    pub hotkeys: Vec<Hotkey>,
    pub auto_hp: u16,
    pub auto_mp: u16,
    pub auto_universal: u16,
    pub auto_potion_delay: u8,
    pub blocked_whispers: Vec<String>,
    pub unk_u32: u32,
    pub unk_u8: u8,
}

/// The spawn essentials extracted from a CHARACTER_DATA body: the unique id
/// and the position it precedes (plus the record-head ref id).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParsedCharacterData {
    pub unique_id: u32,
    pub ref_id: u32,
    pub region: u16,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub heading: u16,
}

/// Everything extracted from a CHARACTER_DATA body. Each section is `None`
/// when its stage did not parse; `spawn`/`state`/`name` may still be present
/// via the anchor fallback even when the forward pass died earlier.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedCharacterInfo {
    pub stats: Option<CharacterStats>,
    pub inventory_size: Option<u8>,
    pub inventory: Option<Vec<InventoryItem>>,
    pub avatar_items: Option<Vec<InventoryItem>>,
    pub masteries: Option<Vec<Mastery>>,
    pub skills: Option<Vec<KnownSkill>>,
    pub completed_quests: Option<Vec<u32>>,
    pub spawn: Option<ParsedCharacterData>,
    pub movement: Option<EntityMovement>,
    pub state: Option<EntityState>,
    pub name: Option<String>,
    pub job: Option<JobInfo>,
    pub extras: Option<PlayerExtras>,
    /// Every stage parsed and the reader consumed the whole blob — the layout
    /// is fully pinned for this server.
    pub fully_parsed: bool,
    /// Name of the first stage that failed to parse, for diagnostics.
    pub failed_stage: Option<&'static str>,
    /// How far (in bytes) the forward pass got, for diagnostics.
    pub forward_parsed_to: usize,
    /// One entry per item record the forward pass started to read (main and
    /// avatar inventory alike, in read order). Pure diagnostics: on a stage
    /// failure the first `Unknown`-class entry usually marks where the stream
    /// drifted — the item *before* it read the wrong number of body bytes.
    pub item_trace: Vec<ItemReadTrace>,
    /// Set when an item section stopped on a record whose width could not be
    /// derived; the items before it are still in `inventory`/`avatar_items`.
    pub item_stop: Option<ItemSectionStop>,
    /// Set when such a record was crossed by a validated resync instead
    /// (`item_stop` is then `None`): the ref id and the body width the resync
    /// proved. Diagnostics — the record itself is in the inventory with an
    /// [`ItemTypeData::Unknown`] body.
    pub recovered_body: Option<RecoveredItemBody>,
}

/// An unresolvable record whose body width was recovered by resync.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RecoveredItemBody {
    pub ref_id: u32,
    pub body_len: usize,
}

/// An item section's result: what was decoded, plus where it stopped if a
/// record could not be decoded.
///
/// Idea (#455): the original client asserts `cursor != end` only in a debug
/// build (`MsgStreamBuffer.h:0xba`); a shipping build force-consumes the
/// remainder and carries on. So consuming fewer bytes than the body is the
/// original's own behaviour, and a section that cannot derive one record's
/// width must return the records it *did* read rather than nothing (#425: an
/// unresolvable ref id threw away 25 already-decoded items).
#[derive(Debug, Clone, Default)]
pub struct ItemSection {
    /// Slot capacity of the section (the `size` byte).
    pub size: u8,
    /// The items decoded before the section ended or stopped.
    pub items: Vec<InventoryItem>,
    /// `Some` when a record could not be decoded; everything after it in the
    /// packet is unreliable, so the caller must stop its forward pass.
    pub stopped_at: Option<ItemSectionStop>,
}

/// Where an item section stopped short: the record index, its start offset in
/// the blob, and the ref id whose class could not be resolved.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ItemSectionStop {
    pub index: u8,
    pub offset: usize,
    pub ref_id: u32,
}

/// Diagnostic record of one item-record read attempt: where it started, its
/// header fields, the class the resolver picked, and where the read ended
/// (header end if the body errored).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ItemReadTrace {
    pub start_offset: usize,
    pub end_offset: usize,
    pub slot: u8,
    pub rent_type: u32,
    pub ref_id: u32,
    pub class: ItemClass,
}

/// Region-local coordinates span one 1920-unit tile; a real spawn is never at
/// region 0, and its height stays within a sane band. Used to reject a stray
/// byte sequence that happens to look like a position during the scan.
fn plausible(region: u16, x: f32, y: f32, z: f32) -> bool {
    region != 0 && sane_coord(x) && sane_coord(z) && y.is_finite() && y.abs() <= 5000.0
}

/// A region-local horizontal coordinate lies in `[0, 1920]`. Reject sub-unit
/// magnitudes (subnormals and other near-zero floats): real spawn coordinates
/// are whole-ish values in the hundreds, and admitting tiny floats lets random
/// blob bytes pass as a position when scanning without a known unique id.
fn sane_coord(v: f32) -> bool {
    v == 0.0 || (v.is_normal() && (1.0..=1920.0).contains(&v))
}

fn read_u32_at(raw: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_le_bytes(raw.get(off..off + 4)?.try_into().ok()?))
}

/// A u16-length-prefixed string, read lossily (SRO names are single-byte and
/// not always valid UTF-8; the derive's strict `String` handling is fine for
/// the job block, but the character name must survive any encoding).
fn read_string<T: Read + ReadBytesExt>(reader: &mut T) -> Result<String, SerializationError> {
    let len = u16::read_from(reader)? as usize;
    let mut bytes = vec![0u8; len];
    reader.read_exact(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Decode a `SpawnPosition` at `off` and lift it into a [`ParsedCharacterData`]
/// if plausible.
fn read_position_at(
    raw: &[u8],
    off: usize,
    unique_id: u32,
    ref_id: u32,
) -> Option<ParsedCharacterData> {
    let mut cursor = Cursor::new(raw.get(off..)?);
    let pos = SpawnPosition::read_from(&mut cursor).ok()?;
    if !plausible(pos.region, pos.x, pos.y, pos.z) {
        return None;
    }
    Some(ParsedCharacterData {
        unique_id,
        ref_id,
        region: pos.region,
        x: pos.x,
        y: pos.y,
        z: pos.z,
        heading: pos.heading,
    })
}

/// Find the spawn position + unique id in a CHARACTER_DATA body. Scans for a
/// `u32` immediately followed by a plausible position block, taking that `u32`
/// as the unique id — the layout go-sro writes (`UniqueID` then
/// `WritePosition`).
///
/// `expected_unique_id` optionally pins the scan to a known id (from
/// `CelestialPosition`, when it arrived first) so a coincidental earlier
/// position can't win; pass `None` to accept the first plausible position and
/// read its unique id straight from the blob. Returns `None` if no plausible
/// position (matching the id, if given) is found.
///
/// Production code goes through [`parse_character_info`] (which uses the scan
/// as its fallback); this stays as the standalone anchor primitive for tests
/// and diagnostics.
#[allow(dead_code)]
pub fn parse_character_data(
    raw: &[u8],
    expected_unique_id: Option<u32>,
) -> Option<ParsedCharacterData> {
    scan_character_data(raw, expected_unique_id).map(|(parsed, _)| parsed)
}

/// The scan behind [`parse_character_data`], additionally returning the offset
/// just past the position block so the caller can keep reading the tail
/// (movement, state, name) from there.
fn scan_character_data(
    raw: &[u8],
    expected_unique_id: Option<u32>,
) -> Option<(ParsedCharacterData, usize)> {
    // The character's ref id sits at the start of the record (after the 4-byte
    // server time); cosmetic here, so default to 0 if the body is truncated.
    let ref_id = read_u32_at(raw, 4).unwrap_or(0);

    // id (4 bytes) + position block (u16 + 3×f32 + u16 = 16 bytes).
    const AFTER_POSITION: usize = 4 + 16;

    let mut found: Option<(ParsedCharacterData, usize)> = None;
    let mut off = 0;
    while off + 4 <= raw.len() {
        let Some(candidate_id) = read_u32_at(raw, off) else {
            break;
        };
        // When the id is known, only test positions that follow *that* id.
        if expected_unique_id.is_none_or(|want| want == candidate_id) {
            if let Some(parsed) = read_position_at(raw, off + 4, candidate_id, ref_id) {
                // With a known id, the first id-matching position is the answer.
                if expected_unique_id.is_some() {
                    return Some((parsed, off + AFTER_POSITION));
                }
                // Without one, we're inferring the id from the position's
                // neighbour, so accept it only if the whole blob yields a single
                // plausible position — a second match means we can't tell the
                // real spawn from a coincidence, and the caller should wait for
                // the id from `CelestialPosition` instead.
                if found.is_some() {
                    return None;
                }
                found = Some((parsed, off + AFTER_POSITION));
            }
        }
        off += 1;
    }
    found
}

/// Fully parse a CHARACTER_DATA body: forward pass per the go-sro write order,
/// cross-checked against the anchor scan (see the module doc). Never fails
/// outright — absent sections are `None` and `failed_stage`/`forward_parsed_to`
/// say where and why the forward pass stopped.
pub fn parse_character_info(
    raw: &[u8],
    expected_unique_id: Option<u32>,
    resolver: &impl ItemClassResolver,
) -> ParsedCharacterInfo {
    let mut info = ParsedCharacterInfo::default();
    let mut cursor = Cursor::new(raw);
    forward_parse(&mut cursor, resolver, &mut info, None);
    info.forward_parsed_to = cursor.position() as usize;

    // A record of unresolvable width stops the forward pass where it stands,
    // which throws away every section behind it (#425: 710 of 1296 bytes).
    // Try to resync across it — see `recover_item_stop`.
    if let Some(stop) = info.item_stop {
        if let Some(recovered) = recover_item_stop(raw, expected_unique_id, resolver, stop) {
            return recovered;
        }
    }

    // The forward tail is trusted when its position was plausible (checked
    // while parsing) and its unique id matches the known one, if any.
    let forward_ok = match (&info.spawn, expected_unique_id) {
        (Some(spawn), Some(id)) => spawn.unique_id == id,
        (Some(_), None) => true,
        (None, _) => false,
    };
    if forward_ok {
        info.fully_parsed = info.failed_stage.is_none() && info.forward_parsed_to == raw.len();
        return info;
    }

    // Forward pass died before the tail or disagreed with the known id: the
    // variable middle can't be trusted. Keep the fixed-offset stats and the
    // diagnostics, rebuild the tail from the anchor scan. Exception: when BOTH
    // item sections parsed (the failure hit masteries or later), two
    // consecutive structurally-valid sections make a misaligned inventory very
    // unlikely — keep it so one drifted late section doesn't blank the items.
    // (Not on a tail id mismatch though — then the whole blob is suspect.)
    // A record whose class is unresolvable is never read (`read_body` fails
    // fast), so an `Unknown` entry can only be the *last* one in the trace, and
    // it is exactly the record the section stopped on. Every record before it
    // ended where the next began, so a stopped-short section's items are as
    // trustworthy as a completed one's (#455) — an `Unknown` anywhere earlier
    // would mean a later width was guessed, and voids the whole section.
    let drifted = info
        .item_trace
        .split_last()
        .map(|(_, before)| before)
        .unwrap_or(&[])
        .iter()
        .any(|t| t.class == ItemClass::Unknown);
    let items_trusted = info.failed_stage.is_some()
        && (info.avatar_items.is_some() || info.item_stop.is_some())
        && !drifted;
    let failed_stage = info.failed_stage.or(Some("tail id mismatch"));
    let mut fallback = ParsedCharacterInfo {
        stats: info.stats,
        inventory_size: if items_trusted {
            info.inventory_size
        } else {
            None
        },
        inventory: if items_trusted { info.inventory } else { None },
        avatar_items: if items_trusted {
            info.avatar_items
        } else {
            None
        },
        failed_stage,
        forward_parsed_to: info.forward_parsed_to,
        item_trace: info.item_trace,
        item_stop: info.item_stop,
        ..Default::default()
    };
    if let Some((spawn, after_position)) = scan_character_data(raw, expected_unique_id) {
        let current_region = spawn.region;
        fallback.spawn = Some(spawn);
        let mut cursor = Cursor::new(raw);
        cursor.set_position(after_position as u64);
        if let Ok(movement) = EntityMovement::read_with_region(&mut cursor, current_region) {
            if let Ok(state) = EntityState::read_from(&mut cursor) {
                if state_plausible(&state) {
                    fallback.movement = Some(movement);
                    fallback.state = Some(state);
                    if let Ok(name) = read_string(&mut cursor) {
                        if name_plausible(&name) {
                            fallback.name = Some(name);
                        }
                    }
                }
            }
        }
    }
    fallback
}

/// The anchor-continued tail has no id to validate against, so gate the state
/// block on sane speeds instead (go-sro defaults: walk 16, run 50, hwan 100).
fn state_plausible(state: &EntityState) -> bool {
    let sane = |v: f32| v.is_finite() && (0.0..=1000.0).contains(&v);
    sane(state.walk_speed) && state.run_speed > 0.0 && sane(state.run_speed)
}

fn name_plausible(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && !name.chars().any(|c| c.is_control())
}

/// Widest body a resync will attribute to an unresolvable record. The search
/// is bounded only for cost — every candidate is validated against the whole
/// blob, so the bound decides how big a record we can recover, never whether a
/// recovery is correct. 64 bytes covers every body go-sro's
/// `WriteInventoryItem` writes short of a magic-param-laden equipment record.
const MAX_RECOVERED_BODY: usize = 64;

/// Skip instruction for the one record whose class could not be resolved:
/// consume `body_len` bytes at `offset` and carry on.
#[derive(Debug, Clone, Copy)]
struct UnknownBody {
    /// Start offset of the *record* (its slot byte), as reported by
    /// [`ItemSectionStop::offset`].
    offset: usize,
    body_len: usize,
}

/// Resync across a record whose width we cannot derive.
///
/// Idea: we cannot classify the ref id (the server's item table has rows the
/// client's itemdata does not — live refs 46551 and 23273), but we can *test*
/// a width: assume the body is `n` bytes, replay the whole forward pass, and
/// keep `n` only if every following section then parses and the pass consumes
/// the blob to the last byte with the expected unique id in the tail. A wrong
/// `n` shifts every later record, so it practically never survives that. The
/// width is accepted only when it is the unique one that does — an ambiguous
/// blob keeps today's stop-and-report behaviour rather than guessing.
fn recover_item_stop(
    raw: &[u8],
    expected_unique_id: Option<u32>,
    resolver: &impl ItemClassResolver,
    stop: ItemSectionStop,
) -> Option<ParsedCharacterInfo> {
    let mut found: Option<ParsedCharacterInfo> = None;
    for body_len in 0..=MAX_RECOVERED_BODY.min(raw.len().saturating_sub(stop.offset)) {
        let mut candidate = ParsedCharacterInfo::default();
        let mut cursor = Cursor::new(raw);
        forward_parse(
            &mut cursor,
            resolver,
            &mut candidate,
            Some(UnknownBody {
                offset: stop.offset,
                body_len,
            }),
        );
        candidate.forward_parsed_to = cursor.position() as usize;
        let tail_ok = match (&candidate.spawn, expected_unique_id) {
            (Some(spawn), Some(id)) => spawn.unique_id == id,
            (Some(_), None) => true,
            (None, _) => false,
        };
        if candidate.failed_stage.is_some() || candidate.forward_parsed_to != raw.len() || !tail_ok
        {
            continue;
        }
        if found.is_some() {
            // Two widths both explain the whole blob: we cannot tell them
            // apart, so recover nothing.
            return None;
        }
        candidate.fully_parsed = true;
        candidate.recovered_body = Some(RecoveredItemBody {
            ref_id: stop.ref_id,
            body_len,
        });
        found = Some(candidate);
    }
    found
}

/// Run the staged forward pass, filling `info` section by section. Stops at
/// the first stage that fails, recording its name. `unknown_body` carries a
/// resync attempt's width for the one unresolvable record (see
/// [`recover_item_stop`]); `None` is the plain first pass.
fn forward_parse(
    cursor: &mut Cursor<&[u8]>,
    resolver: &impl ItemClassResolver,
    info: &mut ParsedCharacterInfo,
    unknown_body: Option<UnknownBody>,
) {
    macro_rules! stage {
        ($name:literal, $expr:expr) => {
            match $expr {
                Ok(v) => v,
                Err(_) => {
                    info.failed_stage = Some($name);
                    return;
                }
            }
        };
    }

    info.stats = Some(stage!("stats", CharacterStats::read_from(cursor)));

    let inventory = stage!(
        "inventory",
        read_item_section(cursor, resolver, &mut info.item_trace, unknown_body)
    );
    info.inventory_size = Some(inventory.size);
    info.inventory = Some(inventory.items);
    if let Some(stop) = inventory.stopped_at {
        // The section handed back what it read; the rest of the blob sits
        // behind a record of unknown width, so the forward pass ends here.
        info.item_stop = Some(stop);
        info.failed_stage = Some("inventory");
        return;
    }
    let avatar = stage!(
        "avatar inventory",
        read_item_section(cursor, resolver, &mut info.item_trace, unknown_body)
    );
    info.avatar_items = Some(avatar.items);
    if let Some(stop) = avatar.stopped_at {
        info.item_stop = Some(stop);
        info.failed_stage = Some("avatar inventory");
        return;
    }

    let mastery_skills = stage!("masteries", MasterySkillSection::read_from(cursor));
    info.masteries = Some(mastery_skills.masteries);
    info.skills = Some(mastery_skills.skills);

    let quests = stage!("quests", QuestSection::read_from(cursor));
    info.completed_quests = Some(quests.completed);
    // Active quest records have an unverified shape; go-sro sends none.
    stage!("active quests", nonzero_guard(quests.active_count as u32));
    let collection = stage!("collection book", CollectionBook::read_from(cursor));
    stage!(
        "collection book",
        nonzero_guard(collection.started_theme_count)
    );

    let ref_id = info.stats.as_ref().map(|s| s.ref_id).unwrap_or(0);
    info.spawn = Some(stage!("spawn position", read_tail_spawn(cursor, ref_id)));
    let current_region = info.spawn.as_ref().map(|s| s.region).unwrap_or(0);
    info.movement = Some(stage!(
        "movement",
        EntityMovement::read_with_region(cursor, current_region)
    ));
    info.state = Some(stage!("state", EntityState::read_from(cursor)));
    info.name = Some(stage!("name", read_string(cursor)));
    info.job = Some(stage!("job", JobInfo::read_from(cursor)));
    info.extras = Some(stage!("extras", PlayerExtras::read_from(cursor)));
}

/// A guard stage for counts whose record shape is unknown: fine while zero.
fn nonzero_guard(count: u32) -> Result<(), SerializationError> {
    if count == 0 {
        Ok(())
    } else {
        Err(SerializationError::UnknownVariation(
            count as usize,
            "unverified record count",
        ))
    }
}

/// An inventory section: `size:u8, count:u8`, then `count` items. Used for
/// both the main and the avatar inventory (same wire shape). Every record
/// started is logged into `trace` (header first, end offset patched after the
/// body) so a mid-section drift can be pinned from a live run's log.
fn read_item_section(
    cursor: &mut Cursor<&[u8]>,
    resolver: &impl ItemClassResolver,
    trace: &mut Vec<ItemReadTrace>,
    unknown_body: Option<UnknownBody>,
) -> Result<ItemSection, SerializationError> {
    let size = u8::read_from(cursor)?;
    let count = u8::read_from(cursor)?;
    if count > size {
        // More items than slots: we lost the layout.
        return Err(SerializationError::UnknownVariation(
            count as usize,
            "inventory count",
        ));
    }
    let mut section = ItemSection {
        size,
        items: Vec::with_capacity(count as usize),
        stopped_at: None,
    };
    for index in 0..count {
        let start_offset = cursor.position() as usize;
        let slot = u8::read_from(cursor)?;
        let rent = RentInfo::read_from(cursor)?;
        let ref_id = u32::read_from(cursor)?;
        let class = resolver.item_class(ref_id);
        trace.push(ItemReadTrace {
            start_offset,
            end_offset: cursor.position() as usize,
            slot,
            rent_type: rent.rent_type,
            ref_id,
            class,
        });
        // A resync attempt supplies the width of the one record whose class
        // could not be resolved; everything else routes normally.
        let resync =
            unknown_body.filter(|ub| ub.offset == start_offset && class == ItemClass::Unknown);
        let data = match resync {
            Some(ub) => {
                let mut skipped = vec![0u8; ub.body_len];
                cursor.read_exact(&mut skipped)?;
                Ok(ItemTypeData::Unknown)
            }
            None => ItemTypeData::read_body(cursor, class, ref_id),
        };
        let data = match data {
            Ok(data) => data,
            // A record whose *width* we cannot derive (unknown item class) is a
            // short read, not a corrupt stream: keep the items already decoded
            // and mark where we stopped. Running out of bytes (`IoError`) is a
            // long read and still fails the section.
            Err(err @ SerializationError::IoError(_)) => return Err(err),
            Err(_) => {
                section.stopped_at = Some(ItemSectionStop {
                    index,
                    offset: start_offset,
                    ref_id,
                });
                return Ok(section);
            }
        };
        if let Some(entry) = trace.last_mut() {
            entry.end_offset = cursor.position() as usize;
        }
        section.items.push(InventoryItem {
            slot,
            rent,
            ref_id,
            data,
        });
    }
    Ok(section)
}

/// Public entry to the item-section parser for other packets that carry the
/// same `size:u8, count:u8, items…` shape (the storage list 0x3049). Discards
/// the per-record trace the character-data parser keeps.
pub fn parse_item_section(
    cursor: &mut Cursor<&[u8]>,
    resolver: &impl ItemClassResolver,
) -> Result<(u8, Vec<InventoryItem>), SerializationError> {
    let mut trace = Vec::new();
    let section = read_item_section(cursor, resolver, &mut trace, None)?;
    Ok((section.size, section.items))
}

/// The tail's `unique_id + position`; the position must be plausible, which is
/// what validates that the variable middle was parsed with the right widths.
fn read_tail_spawn(
    cursor: &mut Cursor<&[u8]>,
    ref_id: u32,
) -> Result<ParsedCharacterData, SerializationError> {
    let unique_id = u32::read_from(cursor)?;
    let pos = SpawnPosition::read_from(cursor)?;
    if !plausible(pos.region, pos.x, pos.y, pos.z) {
        return Err(SerializationError::UnknownVariation(
            pos.region as usize,
            "implausible spawn position",
        ));
    }
    Ok(ParsedCharacterData {
        unique_id,
        ref_id,
        region: pos.region,
        x: pos.x,
        y: pos.y,
        z: pos.z,
        heading: pos.heading,
    })
}

#[cfg(test)]
mod test {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn movement_dest_coords_are_short_in_overworld() {
        // has_dest, move_type, dest_region 0x60A8, then x/y/z as u16.
        let mut b: Vec<u8> = vec![1, 0];
        b.extend_from_slice(&0x60A8u16.to_le_bytes());
        for v in [100u16, 50, 200] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.push(0xEE); // sentinel: must not be consumed
        let mut cursor = Cursor::new(b.as_slice());
        let m = EntityMovement::read_with_region(&mut cursor, 0x60A8).unwrap();
        assert_eq!(m.dest_region, Some(0x60A8));
        assert_eq!(
            (m.dest_x, m.dest_y, m.dest_z),
            (Some(100), Some(50), Some(200))
        );
        assert_eq!(cursor.position() as usize, b.len() - 1);
    }

    #[test]
    fn movement_dest_coords_are_int_in_dungeon() {
        // Current position region selects the width (bit 15 set → i32), and
        // dungeon-local destinations are large and can be negative
        // (GATE_JINSI_02x01_03 = -10468, 0, 5596 in teleportdata).
        let mut b: Vec<u8> = vec![1, 0];
        b.extend_from_slice(&0x8007u16.to_le_bytes());
        for v in [-10468i32, 0, 5596] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.push(0xEE);
        let mut cursor = Cursor::new(b.as_slice());
        let m = EntityMovement::read_with_region(&mut cursor, 0x8007).unwrap();
        assert_eq!(
            (m.dest_x, m.dest_y, m.dest_z),
            (Some(-10468), Some(0), Some(5596))
        );
        assert_eq!(cursor.position() as usize, b.len() - 1);
    }

    #[test]
    fn movement_dest_region_zero_suppresses_coords() {
        let mut b: Vec<u8> = vec![1, 0];
        b.extend_from_slice(&0u16.to_le_bytes());
        b.push(0xEE);
        let mut cursor = Cursor::new(b.as_slice());
        let m = EntityMovement::read_with_region(&mut cursor, 0x8001).unwrap();
        assert_eq!(m.dest_region, Some(0));
        assert_eq!(m.dest_x, None);
        assert_eq!(cursor.position(), 4);
    }

    #[test]
    fn movement_standing_turn() {
        let b: Vec<u8> = vec![0, 1, 1, 0xEC, 0x2F];
        let mut cursor = Cursor::new(b.as_slice());
        let m = EntityMovement::read_with_region(&mut cursor, 0x60A8).unwrap();
        assert!(!m.has_destination);
        assert_eq!(m.source, Some(1));
        assert_eq!(m.angle, Some(0x2FEC));
    }

    /// Build a body: some prefix, the unique id, then the position block. The
    /// filler is `0xFF` (whose f32 reads are inf/NaN) so it never forms a
    /// coincidental plausible position — keeping the hint-less scan unambiguous.
    fn body(unique_id: u32, region: u16, x: f32, y: f32, z: f32, heading: u16) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&0u32.to_le_bytes()); // server time
        b.extend_from_slice(&1919u32.to_le_bytes()); // ref id
        b.extend_from_slice(&[0xFF; 20]); // arbitrary variable-length middle
        b.extend_from_slice(&unique_id.to_le_bytes());
        b.extend_from_slice(&region.to_le_bytes());
        b.extend_from_slice(&x.to_le_bytes());
        b.extend_from_slice(&y.to_le_bytes());
        b.extend_from_slice(&z.to_le_bytes());
        b.extend_from_slice(&heading.to_le_bytes());
        b.extend_from_slice(&[0xFF; 30]); // trailing name/guild/etc.
        b
    }

    #[test]
    fn anchors_on_unique_id_and_reads_position() {
        // Values from the real vSRO 1.88 capture (a Jangan spawn).
        let parsed = parse_character_data(
            &body(352808, 0x60A8, 1058.0, -7.68, 1426.0, 12268),
            Some(352808),
        )
        .expect("should parse");
        assert_eq!(parsed.unique_id, 352808);
        assert_eq!(parsed.ref_id, 1919);
        assert_eq!(parsed.region, 0x60A8);
        assert_eq!(parsed.x, 1058.0);
        assert_eq!(parsed.y, -7.68);
        assert_eq!(parsed.z, 1426.0);
        assert_eq!(parsed.heading, 12268);
    }

    #[test]
    fn reads_unique_id_from_blob_without_hint() {
        // No `CelestialPosition` id available: an unambiguous body lets the scan
        // recover both the unique id and the position straight from the blob.
        let parsed = parse_character_data(&body(352808, 0x60A8, 800.0, -10.0, 700.0, 100), None)
            .expect("should parse");
        assert_eq!(parsed.unique_id, 352808);
        assert_eq!(parsed.region, 0x60A8);
        assert_eq!(parsed.x, 800.0);
        assert_eq!(parsed.z, 700.0);
    }

    #[test]
    fn hintless_scan_rejects_ambiguous_body() {
        // Two plausible positions and no id to disambiguate: the scan must bail
        // (the caller falls back to the id from `CelestialPosition`). With the
        // id given, the matching one is still found.
        let mut b = Vec::new();
        b.extend_from_slice(&0u32.to_le_bytes()); // server time
        b.extend_from_slice(&1919u32.to_le_bytes()); // ref id
        b.extend_from_slice(&111u32.to_le_bytes()); // id A
        b.extend_from_slice(&0x60A8u16.to_le_bytes());
        b.extend_from_slice(&800.0f32.to_le_bytes());
        b.extend_from_slice(&(-10.0f32).to_le_bytes());
        b.extend_from_slice(&700.0f32.to_le_bytes());
        b.extend_from_slice(&100u16.to_le_bytes());
        b.extend_from_slice(&222u32.to_le_bytes()); // id B
        b.extend_from_slice(&0x60A8u16.to_le_bytes());
        b.extend_from_slice(&900.0f32.to_le_bytes());
        b.extend_from_slice(&(-10.0f32).to_le_bytes());
        b.extend_from_slice(&600.0f32.to_le_bytes());
        b.extend_from_slice(&100u16.to_le_bytes());

        assert!(parse_character_data(&b, None).is_none());
        let parsed = parse_character_data(&b, Some(222)).expect("id B resolves");
        assert_eq!(parsed.unique_id, 222);
        assert_eq!(parsed.x, 900.0);
    }

    #[test]
    fn none_when_id_absent() {
        let b = body(352808, 0x60A8, 1058.0, -7.68, 1426.0, 12268);
        assert!(parse_character_data(&b, Some(999999)).is_none());
    }

    #[test]
    fn skips_coincidental_match_with_implausible_position() {
        // A stray copy of the id followed by garbage (region 0), then the real
        // id + position. The plausibility check must skip the decoy.
        let mut b = Vec::new();
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&1919u32.to_le_bytes());
        b.extend_from_slice(&352808u32.to_le_bytes()); // decoy
        b.extend_from_slice(&[0u8; 16]); // region 0 -> implausible
        b.extend_from_slice(&352808u32.to_le_bytes()); // real
        b.extend_from_slice(&0x60A8u16.to_le_bytes());
        b.extend_from_slice(&1058.0f32.to_le_bytes());
        b.extend_from_slice(&(-7.68f32).to_le_bytes());
        b.extend_from_slice(&1426.0f32.to_le_bytes());
        b.extend_from_slice(&12268u16.to_le_bytes());
        let parsed = parse_character_data(&b, Some(352808)).expect("should skip decoy");
        assert_eq!(parsed.region, 0x60A8);
        assert_eq!(parsed.z, 1426.0);
    }

    // --- forward parser -----------------------------------------------------

    /// Item classes by ref id; unknown ids resolve to `Unknown`.
    struct MockResolver {
        classes: HashMap<u32, ItemClass>,
    }

    impl ItemClassResolver for MockResolver {
        fn item_class(&self, ref_id: u32) -> ItemClass {
            self.classes
                .get(&ref_id)
                .copied()
                .unwrap_or(ItemClass::Unknown)
        }
    }

    /// A byte builder mirroring the server's little-endian writes.
    #[derive(Default)]
    struct Body(Vec<u8>);
    impl Body {
        fn u8(mut self, v: u8) -> Self {
            self.0.push(v);
            self
        }
        fn u16(mut self, v: u16) -> Self {
            self.0.extend_from_slice(&v.to_le_bytes());
            self
        }
        fn u32(mut self, v: u32) -> Self {
            self.0.extend_from_slice(&v.to_le_bytes());
            self
        }
        fn u64(mut self, v: u64) -> Self {
            self.0.extend_from_slice(&v.to_le_bytes());
            self
        }
        fn f32(mut self, v: f32) -> Self {
            self.0.extend_from_slice(&v.to_le_bytes());
            self
        }
        fn string(mut self, s: &str) -> Self {
            self.0.extend_from_slice(&(s.len() as u16).to_le_bytes());
            self.0.extend_from_slice(s.as_bytes());
            self
        }
        fn raw(mut self, bytes: &[u8]) -> Self {
            self.0.extend_from_slice(bytes);
            self
        }
    }

    const UNIQUE_ID: u32 = 352808;
    const REF_ID: u32 = 1907;
    const SWORD: u32 = 3632; // equipment
    const PILLS: u32 = 5; // expendable
    const MP_POTION: u32 = 24457; // expendable, TID4 = 2 like all MP potions
    const GACHA_CARD: u32 = 30000; // expendable, TID3 = 14: carries mag params
                                   // Real refs from the user's v1.188 itemdata: ITEM_COS_P_RABBIT_SCROLL is
                                   // TID 3/2/1/2 (rentable pick pet — the only COS arm with a rent field),
                                   // ITEM_COS_P_FLUTE is 3/2/1/1 (growth pet, no rent field).
    const PET_SCROLL_RENTABLE: u32 = 10365;
    const PET_SCROLL_GROWTH: u32 = 7488;

    fn resolver() -> MockResolver {
        MockResolver {
            classes: [
                (SWORD, ItemClass::Equipment),
                (PILLS, ItemClass::Expendable { tid3: 1, tid4: 1 }),
                (MP_POTION, ItemClass::Expendable { tid3: 1, tid4: 2 }),
                (GACHA_CARD, ItemClass::Expendable { tid3: 14, tid4: 2 }),
                (
                    PET_SCROLL_RENTABLE,
                    ItemClass::Container { tid3: 1, tid4: 2 },
                ),
                (PET_SCROLL_GROWTH, ItemClass::Container { tid3: 1, tid4: 1 }),
            ]
            .into_iter()
            .collect(),
        }
    }

    /// The stat block go-sro writes: server time + `WriteCharDataToPacket`.
    fn stats_block(b: Body) -> Body {
        b.u32(12345) // server time
            .u32(REF_ID)
            .u8(0) // scale
            .u8(7) // level
            .u8(9) // max level
            .u64(5000) // exp
            .u32(300) // skill exp
            .u64(1_000_000_000) // gold
            .u32(100_000) // skill points
            .u16(15) // stat points
            .u8(0) // berserk points
            .u32(0) // unknown
            .u32(720) // hp
            .u32(410) // mp
            .u8(1) // auto invest exp
            .u8(0) // daily pk
            .u16(0) // total pk
            .u32(0) // pk penalty
            .u8(0) // berserk level
            .u8(0) // free pvp
    }

    /// A full go-sro-shaped body: stats, 2 inventory items (1 equipment with a
    /// mag param + socket, 1 expendable), empty avatar inventory, 2 masteries,
    /// 1 skill, 1 completed quest, empty collection book, then the tail.
    fn full_body() -> Vec<u8> {
        full_body_with_autopotion(0, 0, 0, 0)
    }

    /// `full_body()` with the four 0x3013 auto-potion tail fields set. Both
    /// in-tree captures carry zeros there (see
    /// `real_0x3013_extras_tail_carries_the_autopotion_settings`), so the only
    /// way to prove the fields are read at the right offsets — and not just
    /// zero-by-accident — is to vary them.
    fn full_body_with_autopotion(hp: u16, mp: u16, universal: u16, delay: u8) -> Vec<u8> {
        let b = stats_block(Body::default())
            // inventory: size 45, 2 items
            .u8(45)
            .u8(2)
            // item 1: equipment in slot 6, no rent
            .u8(6)
            .u32(0)
            .u32(SWORD)
            .u8(3) // opt level
            .u64(0xABCD) // variance
            .u32(10) // durability
            .u8(1) // 1 mag param
            .u32(1701)
            .u32(42)
            .u8(1) // binding tag: socket
            .u8(1) // 1 socket
            .u8(0)
            .u32(9001)
            .u32(7)
            .u8(2) // binding tag: advanced elixir
            .u8(0) // none
            // item 2: expendable stack of 50 in slot 13, no rent
            .u8(13)
            .u32(0)
            .u32(PILLS)
            .u16(50)
            // avatar inventory: size 5, empty
            .u8(5)
            .u8(0)
            // masteries: begin byte, 2 entries, terminator, unknown byte
            .u8(0)
            .u8(1)
            .u32(0x101)
            .u8(4)
            .u8(1)
            .u32(0x102)
            .u8(0)
            .u8(2)
            .u8(0)
            // skills: 1 entry, terminator
            .u8(1)
            .u32(70)
            .u8(1)
            .u8(2)
            // quests: 1 completed, 0 active
            .u16(1)
            .u32(1)
            .u8(0)
            // collection book: unknown byte + 0 started themes
            .u8(0)
            .u32(0)
            // tail: unique id + position
            .u32(UNIQUE_ID)
            .u16(0x60A8)
            .f32(1058.0)
            .f32(-7.68)
            .f32(1426.0)
            .u16(12268)
            // movement: standing (has_dest=0, type, source, angle)
            .u8(0)
            .u8(1)
            .u8(0)
            .u16(12268)
            // state: life/unk/motion/body + walk/run/hwan + 0 buffs
            .u8(2)
            .u8(0)
            .u8(0)
            .u8(0)
            .f32(16.0)
            .f32(50.0)
            .f32(100.0)
            .u8(0)
            // name + job block
            .string("Hero")
            .string("") // job name
            .u8(0) // job type
            .u8(1) // job level
            .u32(0) // job exp
            .u32(0) // job contribution
            .u32(0) // job reward
            // extras
            .u8(0) // pvp state
            .u8(0) // transport flag
            .u8(0) // in combat
            .u8(0xFF) // pvp flag
            .u64(0) // guide flag
            .u32(77) // jid
            .u8(1) // gm
            .u8(0) // activation flag
            .u8(0) // hotkey count
            .u16(hp) // auto hp
            .u16(mp) // auto mp
            .u16(universal) // auto universal
            .u8(delay) // auto potion delay
            .u8(0) // blocked whisper count
            .u32(0x00010001)
            .u8(0);
        b.0
    }

    #[test]
    fn forward_parses_full_go_sro_body() {
        let raw = full_body();
        let info = parse_character_info(&raw, Some(UNIQUE_ID), &resolver());

        assert_eq!(info.failed_stage, None);
        assert!(info.fully_parsed, "reader must consume the whole blob");
        assert_eq!(info.forward_parsed_to, raw.len());

        let stats = info.stats.expect("stats");
        assert_eq!(stats.ref_id, REF_ID);
        assert_eq!(stats.level, 7);
        assert_eq!(stats.exp, 5000);
        assert_eq!(stats.gold, 1_000_000_000);
        assert_eq!(stats.skill_points, 100_000);
        assert_eq!(stats.hp, 720);
        assert_eq!(stats.mp, 410);

        assert_eq!(info.inventory_size, Some(45));
        let inv = info.inventory.expect("inventory");
        assert_eq!(inv.len(), 2);
        assert_eq!(inv[0].slot, 6);
        assert_eq!(inv[0].ref_id, SWORD);
        assert_eq!(inv[0].rent.rent_type, 0);
        let ItemTypeData::Equipment(eq) = &inv[0].data else {
            panic!("sword must parse as equipment");
        };
        assert_eq!(eq.opt_level, 3);
        assert_eq!(eq.variance, 0xABCD);
        assert_eq!(eq.durability, 10);
        assert_eq!(
            eq.mag_params,
            vec![MagicParam {
                kind: 1701,
                value: 42
            }]
        );
        assert_eq!(eq.sockets.len(), 1);
        assert_eq!(eq.sockets[0].id, 9001);
        assert_eq!(eq.adv_elixirs.len(), 0);
        assert_eq!(inv[1].ref_id, PILLS);
        assert_eq!(
            inv[1].data,
            ItemTypeData::Expendable {
                stack_count: 50,
                assimilation_prob: None,
                mag_params: vec![],
            }
        );
        assert_eq!(info.avatar_items.as_deref(), Some(&[][..]));

        let masteries = info.masteries.expect("masteries");
        assert_eq!(masteries.len(), 2);
        assert_eq!(
            masteries[0],
            Mastery {
                id: 0x101,
                level: 4
            }
        );
        let skills = info.skills.expect("skills");
        assert_eq!(skills, vec![KnownSkill { id: 70, enabled: 1 }]);
        assert_eq!(info.completed_quests, Some(vec![1]));

        let spawn = info.spawn.expect("spawn");
        assert_eq!(spawn.unique_id, UNIQUE_ID);
        assert_eq!(spawn.region, 0x60A8);
        assert_eq!(spawn.x, 1058.0);

        let movement = info.movement.expect("movement");
        assert!(!movement.has_destination);
        assert_eq!(movement.angle, Some(12268));

        let state = info.state.expect("state");
        assert_eq!(state.walk_speed, 16.0);
        assert_eq!(state.run_speed, 50.0);
        assert_eq!(state.hwan_speed, 100.0);
        assert!(state.buffs.is_empty());

        assert_eq!(info.name.as_deref(), Some("Hero"));
        assert_eq!(info.job.expect("job").job_level, 1);
        let extras = info.extras.expect("extras");
        assert_eq!(extras.jid, 77);
        assert!(extras.gm);
        assert_eq!(extras.pvp_flag, 0xFF);
        assert_eq!(extras.transport_id, None);
    }

    #[test]
    fn forward_parse_works_without_known_id() {
        // Before CelestialPosition arrives there is no expected id; a forward
        // parse whose tail position is plausible is trusted on its own.
        let raw = full_body();
        let info = parse_character_info(&raw, None, &resolver());
        assert!(info.fully_parsed);
        assert_eq!(info.spawn.expect("spawn").unique_id, UNIQUE_ID);
        assert_eq!(info.state.expect("state").run_speed, 50.0);
    }

    /// Live capture (packet_dump/0x3013.log), see the test using it.
    const FRESH_CHAR_0X3013_HEX: &str = "1a2e828d740700002201010000000000000000000000000000000000000000a086010000000000000000c8000000c800\
         0000010000000000000000006d1300000000001703000000000000000000000027000000000100020001000000008303\
         000000000000000000000027000000000100020002000000005f03000000000000000000000027000000000100020003\
         00000000cb0300000000000000000000002700000000010002000400000000a703000000000000000000000027000000\
         00010002000500000000ef03000000000000000000000027000000000100020006000000006b00000000000000000000\
         00004500000000010002000700000000fb0000000000000000000000002e000000000100020009000000002b07000000\
         00000000000000000000000000010002000a000000004f0700000000000000000000000000000000010002000b000000\
         00070700000000000000000000000000000000010002000c000000000707000000000000000000000000000000000100\
         02000d00000000855f0000e8030e00000000895f0000e8030f00000000815f000014001000000000805f000014001100\
         0000007e5f000014001200000000d7b50000010013000000005d5f000000000000000000000000000000000100020005\
         000001010100000001020100000001030100000001110100000001120100000001130100000001140100000002000201\
         0001000000000000000000beab0100a862496873440000a041de12dc4394e600010094e6000000009a99994101007042\
         0000c84200050050656e697300000001000000000000000000000000000000ff5700a004000000000200000001000000\
         000000000000000100010000";

    /// Live capture (packet_dump/0x3013.log), see the test using it.
    const OLD_CHAR_0X3013_HEX: &str = "1a2e2117730700002211117f520100000000006f0100009804000000000000bea4010000000200000000c50000002503\
         0000010000000000000000006d1b00000000001f01000000000000000000000030000000000100020001000000008b01\
         00000000000000000000002f000000000100020002000000006701000000000000000000000030000000000100020003\
         00000000d30100000000000000000000003000000000010002000400000000af01000000000000000000000030000000\
         00010002000500000000f70100000000000000000000003000000000010002000600000000b800000001a01d35100200\
         000031000000000100020009000000002b0700000000000000000000000000000000010002000a000000004f07000000\
         00000000000000000000000000010002000b00000000070700000000000000000000000000000000010002000c000000\
         00070700000000000000000000000000000000010002000d00000000855f0000e7030e00000000895f0000e7030f0000\
         0000815f00000d001000000000805f00000f0011000000007e5f0000140012000000000b0f0000080013000000005d5f\
         000000000000000000000000000000000100020014000000003700000001001500000000de1b00000100160000000047\
         0000000000000000000000003c00000000010002001700000000eb1b000001001800000000fb00000000000000000000\
         00002e000000000100020019000000000b00000001001a000000003e00000014001b000000005e04000001ed88303000\
         0000003800000000010002001c0000000004000000010005000001010100000001020100000001030100000001110100\
         00000112010000000113010000110114010000000200018e00000001015f0500000102010001000000000000000000a2\
         ab0100a8618f376f447e6f02c27961ae44328b000100328b0000000000008041000048420000c8420004003132333400\
         000001000000000000000000000000000000ff5702a0050000000002000000010701014aeb0300640000000000000000\
         0100010000";

    fn hex_bytes(hex: &str) -> Vec<u8> {
        let clean: String = hex.chars().filter(|c| c.is_ascii_hexdigit()).collect();
        clean
            .as_bytes()
            .chunks(2)
            .map(|c| u8::from_str_radix(std::str::from_utf8(c).unwrap(), 16).unwrap())
            .collect()
    }

    /// The client-side classes of every ref in the fresh char's blob,
    /// mirroring the user's v1.188 itemdata — WITHOUT the go-sro starter-kit
    /// ref 46551 (absent from every known client itemdata).
    fn fresh_char_resolver(with_46551: bool) -> MockResolver {
        let mut classes = HashMap::new();
        for eq in [
            791, 899, 863, 971, 935, 1007, 107, 251, 1835, 1871, 1799, 24413,
        ] {
            classes.insert(eq, ItemClass::Equipment);
        }
        for exp in [24446, 24448, 24449, 24453, 24457] {
            classes.insert(exp, ItemClass::Expendable { tid3: 1, tid4: 1 });
        }
        if with_46551 {
            classes.insert(46551, ItemClass::Expendable { tid3: 1, tid4: 1 });
        }
        MockResolver { classes }
    }

    #[test]
    fn live_fresh_char_unresolvable_starter_item_is_resynced() {
        // The 2026-08-11 live capture that falsified the zero-byte-Unknown
        // assumption: go-sro grants starter-kit ref 46551 (slot 0x12,
        // expendable stack 1) which no client itemdata can route.
        //
        // #425: stopping there threw away every section behind the offender.
        // The resync must instead *prove* the body width from the blob — the
        // only width that lets the rest of the packet parse to its last byte
        // with the expected id in the tail is the real 2-byte stack — and then
        // deliver the whole packet. The offender itself stays uninterpreted
        // (`ItemTypeData::Unknown`): its class is still unknown, only its
        // width is now known. Reference: the same blob parsed with a resolver
        // that routes 46551.
        let raw = hex_bytes(FRESH_CHAR_0X3013_HEX);
        let info = parse_character_info(&raw, Some(0x1ABBE), &fresh_char_resolver(false));

        assert_eq!(info.failed_stage, None);
        assert!(info.fully_parsed);
        assert_eq!(info.item_stop, None);
        assert_eq!(
            info.recovered_body,
            Some(RecoveredItemBody {
                ref_id: 46551,
                body_len: 2,
            })
        );

        let complete = parse_character_info(&raw, Some(0x1ABBE), &fresh_char_resolver(true));
        let reference = complete.inventory.expect("reference inventory");
        let items = info.inventory.expect("inventory");
        assert_eq!(items.len(), reference.len());
        for (got, want) in items.iter().zip(reference.iter()) {
            assert_eq!((got.slot, got.ref_id), (want.slot, want.ref_id));
            if got.ref_id == 46551 {
                // Width recovered, body deliberately not interpreted.
                assert_eq!(got.data, ItemTypeData::Unknown);
            } else {
                assert_eq!(got.data, want.data);
            }
        }
        // The offender is no longer the end of the packet: everything behind
        // it arrives too.
        assert_eq!(info.avatar_items, complete.avatar_items);
        assert_eq!(info.masteries, complete.masteries);
        assert_eq!(info.skills, complete.skills);
        assert_eq!(info.job, complete.job);
        let spawn = info.spawn.expect("spawn");
        assert_eq!(spawn.unique_id, 0x1ABBE);
        assert_eq!(spawn.region, 0x62A8);
        assert_eq!(info.name.as_deref(), Some("Penis"));
    }

    #[test]
    fn resync_declines_when_no_width_explains_the_blob() {
        // The resync is only allowed to cross a record when a width makes the
        // *whole* blob parse. Cut the capture's last byte off and no width can
        // — so the parse must fall back to today's behaviour: stop on the
        // offender, keep the records read before it, and salvage the tail via
        // the anchor scan. It must never pick a width just to get past.
        let mut raw = hex_bytes(FRESH_CHAR_0X3013_HEX);
        raw.pop();
        let info = parse_character_info(&raw, Some(0x1ABBE), &fresh_char_resolver(false));

        assert_eq!(info.recovered_body, None);
        assert_eq!(info.failed_stage, Some("inventory"));
        let complete = parse_character_info(&raw, Some(0x1ABBE), &fresh_char_resolver(true))
            .inventory
            .expect("reference inventory");
        let stop_index = complete
            .iter()
            .position(|i| i.ref_id == 46551)
            .expect("offender is in the reference parse");
        assert!(
            stop_index > 0,
            "capture must have items before the offender"
        );
        assert_eq!(info.inventory.as_deref(), Some(&complete[..stop_index]));
        let stop = info.item_stop.expect("stop recorded");
        assert_eq!(stop.ref_id, 46551);
        assert_eq!(stop.index as usize, stop_index);
        let offender = info.item_trace.last().expect("offender traced");
        assert_eq!(offender.class, ItemClass::Unknown);
        assert_eq!(offender.slot, 0x12);
        let spawn = info.spawn.expect("anchor spawn");
        assert_eq!(spawn.unique_id, 0x1ABBE);
        assert_eq!(spawn.region, 0x62A8);
    }

    #[test]
    fn live_fresh_char_parses_fully_once_the_ref_resolves() {
        // Same bytes, resolver additionally routes 46551 as a plain
        // expendable: the whole blob parses to the byte count and the real
        // chest (ref 899) sits in equip slot 1 — proving the fix is about
        // classification, not offsets.
        let raw = hex_bytes(FRESH_CHAR_0X3013_HEX);
        let info = parse_character_info(&raw, Some(0x1ABBE), &fresh_char_resolver(true));

        assert!(info.fully_parsed, "failed at {:?}", info.failed_stage);
        let items = info.inventory.expect("inventory");
        assert_eq!(items.len(), 19);
        let chest = items
            .iter()
            .find(|i| i.slot == 1)
            .expect("slot 1 populated");
        assert_eq!(chest.ref_id, 899);
        let starter = items
            .iter()
            .find(|i| i.ref_id == 46551)
            .expect("starter item");
        assert!(matches!(
            starter.data,
            ItemTypeData::Expendable { stack_count: 1, .. }
        ));
        assert_eq!(info.name.as_deref(), Some("Penis"));
    }

    #[test]
    fn unroutable_item_fails_the_section() {
        // An unresolvable ref has a server-written body of UNKNOWN width
        // (live capture 2026-08-11: go-sro starter-kit expendable 46551,
        // absent from every known client itemdata, carried a stack:u16 —
        // reading it as zero bytes shifted every later record and forged a
        // fake tail). The section must stop there, keeping the offender in
        // the trace and returning only the records it actually read (#455).
        let b = Body::default()
            .u8(45)
            .u8(2)
            // item 1: unroutable ref in slot 10
            .u8(10)
            .u32(0)
            .u32(999_999)
            // item 2 (never reached): expendable stack of 50 in slot 13
            .u8(13)
            .u32(0)
            .u32(PILLS)
            .u16(50);
        let mut cursor = Cursor::new(b.0.as_slice());
        let mut trace = Vec::new();
        let section = read_item_section(&mut cursor, &resolver(), &mut trace, None)
            .expect("short read is tolerated");
        assert!(section.items.is_empty(), "the offender is the first record");
        let stop = section.stopped_at.expect("stop recorded");
        assert_eq!(stop.index, 0);
        assert_eq!(stop.ref_id, 999_999);
        let offender = trace.last().expect("trace keeps the offender");
        assert_eq!(offender.ref_id, 999_999);
        assert_eq!(offender.class, ItemClass::Unknown);
    }

    #[test]
    fn a_truncated_item_body_still_fails_the_section() {
        // The tolerance is for a *short* read (we consumed less than the body
        // because a width was unknown). A *long* read — the parser wanting
        // more bytes than the body has — is still an error, so a cut-off
        // equipment record must not degrade to a partial section.
        let b = Body::default().u8(45).u8(1).u8(10).u32(0).u32(SWORD).u8(3); // equipment body cut off after the opt level
        let mut cursor = Cursor::new(b.0.as_slice());
        let mut trace = Vec::new();
        assert!(read_item_section(&mut cursor, &resolver(), &mut trace, None).is_err());
    }

    #[test]
    fn tid4_2_expendable_is_stack_only() {
        // Live-capture regression: an MP potion (TID3 1, TID4 2) is a plain
        // stack-only record — the mag-param list belongs to gacha cards
        // (TID3 14) only. Reading a param list here swallowed the next item's
        // slot byte as a count and derailed the whole section.
        let b = Body::default()
            .u8(45)
            .u8(3)
            // item 1: MP potion stack of 1000 — nothing after the stack
            .u8(14)
            .u32(0)
            .u32(MP_POTION)
            .u16(1000)
            // item 2: gacha card with one mag param
            .u8(15)
            .u32(0)
            .u32(GACHA_CARD)
            .u16(1)
            .u8(1)
            .u32(7)
            .u32(99)
            // item 3: plain pills, proving alignment survived
            .u8(16)
            .u32(0)
            .u32(PILLS)
            .u16(20);
        let mut cursor = Cursor::new(b.0.as_slice());
        let mut trace = Vec::new();
        let items = read_item_section(&mut cursor, &resolver(), &mut trace, None)
            .expect("section")
            .items;
        assert_eq!(items.len(), 3);
        assert!(matches!(
            items[0].data,
            ItemTypeData::Expendable {
                stack_count: 1000,
                ..
            }
        ));
        match &items[1].data {
            ItemTypeData::Expendable { mag_params, .. } => {
                assert_eq!(mag_params.len(), 1);
                assert_eq!(mag_params[0].kind, 7);
                assert_eq!(mag_params[0].value, 99);
            }
            other => panic!("gacha card mis-parsed: {other:?}"),
        }
        assert_eq!(items[2].slot, 16);
        assert_eq!(cursor.position() as usize, b.0.len());
    }

    #[test]
    fn never_summoned_pet_scroll_is_state_only() {
        // #301: a freshly bought pet scroll (SRCoS.State::NeverSummoned == 1)
        // has no pet behind it, so the server stops after the state byte.
        // Reading modelID/name/rent/unk anyway consumed 11 bytes of the next
        // record and desynced the remainder of the inventory list.
        let b = Body::default()
            .u8(45)
            .u8(3)
            // item 1: unused rentable scroll — one byte of body
            .u8(20)
            .u32(0)
            .u32(PET_SCROLL_RENTABLE)
            .u8(1)
            // item 2: unused growth scroll — likewise state-only
            .u8(21)
            .u32(0)
            .u32(PET_SCROLL_GROWTH)
            .u8(1)
            // item 3: plain pills, proving alignment survived
            .u8(22)
            .u32(0)
            .u32(PILLS)
            .u16(20);
        let mut cursor = Cursor::new(b.0.as_slice());
        let mut trace = Vec::new();
        let items = read_item_section(&mut cursor, &resolver(), &mut trace, None)
            .expect("section")
            .items;
        assert_eq!(items.len(), 3);
        for unused in &items[0..2] {
            assert_eq!(
                unused.data,
                ItemTypeData::CosPet {
                    state: COS_STATE_NEVER_SUMMONED,
                    cos_ref_id: None,
                    name: None,
                    rent_seconds: None,
                    unk: None,
                },
                "slot {} over-read past the state byte",
                unused.slot
            );
        }
        assert_eq!(items[2].slot, 22);
        assert_eq!(cursor.position() as usize, b.0.len());
    }

    #[test]
    fn summoned_pet_scroll_reads_the_full_tail() {
        // The other side of the #301 gate: once a pet exists the four trailing
        // fields are present, and the rent stamp only for the rentable TID4 2
        // scroll — the growth scroll (TID4 1) must not consume one.
        let b = Body::default()
            .u8(45)
            .u8(3)
            // item 1: summoned rentable pet, carries the rent stamp
            .u8(20)
            .u32(0)
            .u32(PET_SCROLL_RENTABLE)
            .u8(2)
            .u32(1_907)
            .string("Bunny")
            .u32(1_700_000_000)
            .u8(0)
            // item 2: growth pet — tail present, no rent stamp. State 0 is
            // what the go-sro test peer writes (`WriteContainerItem`'s
            // `WriteByte(0) // TODO COS State`) and is outside SRCoS.State,
            // so the gate must test `!= NeverSummoned` rather than whitelist
            // the summoned states — otherwise #93 interop regresses.
            .u8(21)
            .u32(0)
            .u32(PET_SCROLL_GROWTH)
            .u8(0)
            .u32(2_120)
            .string("Piggy")
            .u8(0)
            // item 3: plain pills, proving alignment survived
            .u8(22)
            .u32(0)
            .u32(PILLS)
            .u16(20);
        let mut cursor = Cursor::new(b.0.as_slice());
        let mut trace = Vec::new();
        let items = read_item_section(&mut cursor, &resolver(), &mut trace, None)
            .expect("section")
            .items;
        assert_eq!(items.len(), 3);
        assert_eq!(
            items[0].data,
            ItemTypeData::CosPet {
                state: 2,
                cos_ref_id: Some(1_907),
                name: Some("Bunny".to_string()),
                rent_seconds: Some(1_700_000_000),
                unk: Some(0),
            }
        );
        assert_eq!(
            items[1].data,
            ItemTypeData::CosPet {
                state: 0,
                cos_ref_id: Some(2_120),
                name: Some("Piggy".to_string()),
                rent_seconds: None,
                unk: Some(0),
            }
        );
        assert_eq!(items[2].slot, 22);
        assert_eq!(cursor.position() as usize, b.0.len());
    }

    #[test]
    fn garbage_middle_falls_back_to_anchor_tail() {
        // Unparseable bytes mid-inventory derail the forward pass (here: 0xFF
        // filler garbles the avatar section), but stats (fixed offsets) and
        // the anchor-scanned tail must still land — most importantly the
        // movement speeds.
        let b = stats_block(Body::default())
            .u8(45)
            .u8(1)
            .u8(6)
            .u32(0)
            .u32(999_999) // unknown item ref -> zero-body record
            .raw(&[0xFF; 24]) // filler the forward pass cannot interpret
            .u32(UNIQUE_ID)
            .u16(0x60A8)
            .f32(1058.0)
            .f32(-7.68)
            .f32(1426.0)
            .u16(12268)
            .u8(0)
            .u8(1)
            .u8(0)
            .u16(12268)
            .u8(2)
            .u8(0)
            .u8(0)
            .u8(0)
            .f32(16.0)
            .f32(50.0)
            .f32(100.0)
            .u8(0)
            .string("Hero");
        let info = parse_character_info(&b.0, Some(UNIQUE_ID), &resolver());

        // the unresolvable ref now fails the inventory stage itself
        assert_eq!(info.failed_stage, Some("inventory"));
        assert!(!info.fully_parsed);
        assert_eq!(info.stats.expect("stats").hp, 720);
        // the section stopped on its very first record, so it yields an empty
        // list (#455) rather than nothing — and nothing after it is parsed
        assert_eq!(info.inventory.as_deref(), Some(&[][..]));
        assert_eq!(info.item_stop.expect("stop recorded").ref_id, 999_999);
        assert_eq!(info.masteries, None);
        let spawn = info.spawn.expect("anchor must find the spawn");
        assert_eq!(spawn.unique_id, UNIQUE_ID);
        assert_eq!(spawn.x, 1058.0);
        let state = info.state.expect("anchor tail must yield the state");
        assert_eq!(state.walk_speed, 16.0);
        assert_eq!(state.run_speed, 50.0);
        assert_eq!(info.name.as_deref(), Some("Hero"));
    }

    #[test]
    fn mismatched_forward_id_drops_middle_stages() {
        // The forward pass succeeds but its unique id disagrees with the known
        // one: the variable middle cannot be trusted and is discarded (only the
        // fixed-offset stats survive); the anchor finds nothing for the real id.
        let raw = full_body();
        let info = parse_character_info(&raw, Some(999_999), &resolver());

        assert_eq!(info.failed_stage, Some("tail id mismatch"));
        assert!(!info.fully_parsed);
        assert!(info.stats.is_some());
        assert_eq!(info.inventory, None);
        assert_eq!(info.masteries, None);
        assert_eq!(info.spawn, None);
        assert_eq!(info.state, None);
    }

    #[test]
    fn derive_sections_roundtrip() {
        // The derive-based wire structs must serialize back to the exact bytes
        // they were read from (validates the break-list and `when` usage).
        let section = MasterySkillSection {
            unk_begin: 0,
            masteries: vec![Mastery {
                id: 0x101,
                level: 4,
            }],
            unk_mid: 0,
            skills: vec![KnownSkill { id: 70, enabled: 1 }],
        };
        let bytes: bytes::Bytes = section.clone().into();
        let decoded = MasterySkillSection::try_from(bytes).unwrap();
        assert_eq!(decoded, section);

        let extras = PlayerExtras {
            pvp_state: 0,
            transport_flag: 1,
            in_combat: 0,
            transport_id: Some(42),
            pvp_flag: 0xFF,
            guide_flag: 0,
            jid: 77,
            gm: true,
            activation_flag: 0,
            hotkeys: vec![Hotkey {
                slot: 1,
                kind: 2,
                data: 3,
            }],
            auto_hp: 0,
            auto_mp: 0,
            auto_universal: 0,
            auto_potion_delay: 0,
            blocked_whispers: vec!["Spammer".to_string()],
            unk_u32: 0x00010001,
            unk_u8: 0,
        };
        let bytes: bytes::Bytes = extras.clone().into();
        let decoded = PlayerExtras::try_from(bytes).unwrap();
        assert_eq!(decoded, extras);
    }

    /// The 38-byte `PlayerExtras` tail of a real `CHARACTER_DATA 0x3013`
    /// capture (`packet_dump/0x3013.log` line 1, 2026-08-10T18:19:22Z, 735-byte
    /// body; the tail begins right after the job block that follows the
    /// character name). `packet_dump/` is gitignored, so the bytes are inlined
    /// here as the fixture.
    ///
    /// This is the dead-wire regression guard for #332: the four auto-potion
    /// fields sit between the hotkey list and the blocked-whisper list, so any
    /// drift in the surrounding variable-length sections silently shifts them.
    /// Asserting the whole tail — including that the reader consumes exactly
    /// 38 bytes — pins that layout against a real server's bytes.
    const REAL_EXTRAS_TAIL: [u8; 38] = [
        0x00, 0x00, 0x00, 0xff, 0x57, 0x02, 0xa0, 0x05, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00,
        0x00, 0x01, 0x07, 0x01, 0x01, 0x4a, 0xeb, 0x03, 0x00, 0x64, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00,
    ];

    #[test]
    fn real_0x3013_extras_tail_carries_the_autopotion_settings() {
        let bytes = bytes::Bytes::from_static(&REAL_EXTRAS_TAIL);
        let extras = PlayerExtras::try_from(bytes).expect("real 0x3013 tail must parse");

        // The variable-length neighbours that fix the four fields' offsets.
        assert_eq!(extras.jid, 2);
        assert!(extras.gm);
        assert_eq!(extras.hotkeys.len(), 1);
        assert_eq!(extras.blocked_whispers.len(), 0);

        // The dead wire itself. This capture has auto-potion switched off, so
        // all four read zero — which is exactly the state the window must show
        // rather than substituting a local default.
        assert_eq!(extras.auto_hp, 0);
        assert_eq!(extras.auto_mp, 0);
        assert_eq!(extras.auto_universal, 0);
        assert_eq!(extras.auto_potion_delay, 0);

        // Serializing back must reproduce the capture byte for byte: proof the
        // reader consumed all 38 bytes and mis-attributed none of them.
        let round: bytes::Bytes = extras.into();
        assert_eq!(round.as_ref(), &REAL_EXTRAS_TAIL[..]);
    }

    #[test]
    fn autopotion_settings_survive_the_full_forward_parse() {
        // #332: the four fields were parsed and dropped. They arrive at the
        // very end of the record, behind every variable-length section, so
        // this drives them through `parse_character_info` rather than through
        // `PlayerExtras` alone.
        let raw = full_body_with_autopotion(60, 45, 30, 4);
        let info = parse_character_info(&raw, Some(UNIQUE_ID), &resolver());

        assert_eq!(info.failed_stage, None);
        assert!(info.fully_parsed);
        let extras = info.extras.expect("extras");
        assert_eq!(extras.auto_hp, 60);
        assert_eq!(extras.auto_mp, 45);
        assert_eq!(extras.auto_universal, 30);
        assert_eq!(extras.auto_potion_delay, 4);
    }
}
