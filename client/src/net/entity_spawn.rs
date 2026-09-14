//! Decodes the vSRO 1.88 group-spawn payload (0x3019) into entities to spawn or
//! despawn.
//!
//! Idea: the payload is a `count`-long list (count from the preceding
//! `GroupEntitySpawnBegin`) of variable-length, type-dependent records — a
//! player record embeds an equipment ref-id list whose per-item width depends on
//! whether each item is equipment, which needs itemdata. That is the same reason
//! `CharacterDataBody` is a raw passthrough, so the whole payload is carried
//! unparsed and decoded here, where the client's characterdata/itemdata tables
//! live. Each record starts with a `ref_id` (u32); its type (player / NPC /
//! monster / dropped item) is resolved from that id against those tables, which
//! selects the per-record layout. Because records are variable-length and
//! back-to-back, a mis-parse can't recover the next record boundary, so on any
//! unknown type or short read we stop the batch and return what parsed so far
//! (fail-safe, mirroring `character_data.rs`) rather than desync.
//!
//! Layout is calibrated against a live vSRO 1.88 capture: the player and
//! NPC records (incl. the `tag + u8 count + option bytes` interaction list) are
//! verified; the monster rarity tail and the dropped-item branch are still
//! cross-referenced with skrillax and pending a capture.
//!
//! COS (pet/mount) records and the mounted-player conditional are [S]
//! spec-derived from xBot `PacketParser.cs:735-807` (no capture yet — promoted
//! to [V] by CAPTURE_LIST F3/F7/F8): a COS record is the NPC record plus a
//! tid4-dependent owner tail, and a riding player inserts a `u32` mount uid
//! between its state flags — both used to desync the whole batch.

use packets::agent::character_data::{EntityState, ItemClass, ItemClassResolver};
use packets::hexdump;

use crate::net::reader::{GuildTag, Reader};
use crate::plugins::textdata::{ClientCharacterData, ClientItemData, ClientTeleport};
use packets::agent::pet::CosKind;

pub use packets::agent::character_data::SpawnPosition;

/// The kind of a spawned entity, with the per-kind extras the spawn system uses.
#[derive(Debug, Clone, PartialEq)]
pub enum SpawnKind {
    /// A remote player character; `equipment` are the equipped/avatar item
    /// `(ref id, +N opt level)` pairs to attach. `riding_uid` is the unique id
    /// of the COS the player is mounted on (`None` when on foot).
    Player {
        equipment: Vec<(u32, u8)>,
        riding_uid: Option<u32>,
    },
    Npc,
    Monster,
    /// A COS (pet/mount/transport). `pet_name` is the owner-given name
    /// (attack/pick pets only); `owner_uid` links back to the summoner
    /// (absent on ride-only horses, which carry no owner tail at all).
    Cos {
        kind: CosKind,
        pet_name: Option<String>,
        owner_name: Option<String>,
        owner_uid: Option<u32>,
    },
    /// A dropped item; `amount` is the gold pile size (gold only).
    Item {
        amount: Option<u32>,
    },
    /// A world structure (teleport gate building) — spawns as an invisible
    /// click anchor.
    Structure,
}

/// One entity to spawn.
#[derive(Debug, Clone, PartialEq)]
pub struct SpawnedEntity {
    pub ref_id: u32,
    pub unique_id: u32,
    pub position: SpawnPosition,
    /// The character's name (players only; `None` otherwise).
    pub name: Option<String>,
    /// The player's guild, or `None` for a guildless player and every
    /// non-player kind. The block is always on the wire for players — a
    /// guildless one just sends an empty name.
    pub guild: Option<GuildTag>,
    /// Life/motion/body states and movement speeds (characters only; `None`
    /// for dropped items, which carry no state block).
    pub state: Option<EntityState>,
    /// The per-instance rarity byte at the end of a monster record (`None` for
    /// every other kind). go-sro fills it from characterdata's rarity column;
    /// the spawn system logs a mismatch but keys the badge off characterdata.
    pub spawn_rarity: Option<u8>,
    /// Interaction option ids of a talkable NPC (empty for everything else) —
    /// the entries of its talk dialog (talk/store/storage/teleport…, see
    /// `plugins::cursor::interactions::npcs::TalkOption`).
    pub talk_options: Vec<u8>,
    pub kind: SpawnKind,
}

/// The outcome of decoding a group-spawn payload.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GroupSpawnParse {
    pub spawns: Vec<SpawnedEntity>,
    pub despawns: Vec<u32>,
    /// Ref ids the batch had to skip: present in the payload, in no client
    /// table, so unrenderable. Kept for diagnostics — the entities *around*
    /// them are in `spawns` (#426).
    pub unresolved: Vec<u32>,
}

/// What a record's leading `ref_id` resolves to, which selects the record parser.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RefType {
    Player,
    Monster,
    Npc,
    /// A COS (characterdata `1/2/3/tid4`) — NPC record plus an owner tail.
    Cos {
        kind: CosKind,
    },
    /// A dropped item, with the sub-shape needed to parse its record.
    Item {
        equipment: bool,
        gold: bool,
    },
    /// A gate building (teleportbuilding.txt ref) — a fixed structure record,
    /// not a character body.
    Structure,
    /// Not found in any table — record layout unknown, so the batch must stop.
    Unknown,
}

/// Resolves entity/item ref ids to their type. Abstracted so the parser is
/// unit-testable without the real textdata resources (see [`TextdataResolver`]).
pub trait RefResolver {
    /// The type of an *entity* ref id (the record's leading id).
    fn resolve(&self, ref_id: u32) -> RefType;
    /// Whether an *item* ref id (inside a player's equipment list) is equipment,
    /// which decides its extra optimization-level byte.
    fn item_is_equipment(&self, ref_id: u32) -> bool;
}

/// [`RefResolver`] backed by the loaded characterdata/itemdata tables, plus
/// the teleport table for gate buildings (whose refs live ONLY in
/// teleportbuilding.txt — the Jangan dimensional gate, ref 2094, has no
/// characterdata row and used to abort the whole spawn batch as `Unknown`).
pub struct TextdataResolver<'a> {
    pub char_data: &'a ClientCharacterData,
    pub item_data: &'a ClientItemData,
    pub teleport: &'a ClientTeleport,
}

impl RefResolver for TextdataResolver<'_> {
    fn resolve(&self, ref_id: u32) -> RefType {
        let id = ref_id as i32;
        if let Some(item) = self.item_data.get(&id) {
            return RefType::Item {
                equipment: item.is_equipment(),
                gold: item.is_gold(),
            };
        }
        if let Some(ch) = self.char_data.get(&id) {
            if ch.is_player() {
                return RefType::Player;
            }
            if ch.is_monster() {
                return RefType::Monster;
            }
            if let Some(kind) = ch.cos_kind() {
                return RefType::Cos { kind };
            }
            // is_npc, plus any remaining uncategorized character-family row
            // (quest clones 1/2/3/6..8, …), parses with the NPC record layout.
            return RefType::Npc;
        }
        if self.teleport.is_gate_ref(id) {
            return RefType::Structure;
        }
        RefType::Unknown
    }

    fn item_is_equipment(&self, ref_id: u32) -> bool {
        self.item_data
            .get(&(ref_id as i32))
            .map(|i| i.is_equipment())
            .unwrap_or(false)
    }
}

/// The itemdata classification used by the CHARACTER_DATA parser in the
/// `packets` crate (the table lives here in the client, so the parser takes
/// this lookup as a trait).
impl ItemClassResolver for TextdataResolver<'_> {
    fn item_class(&self, ref_id: u32) -> ItemClass {
        match self
            .item_data
            .get(&(ref_id as i32))
            .and_then(|i| i.type_ids())
        {
            Some((3, 1, _, _)) => ItemClass::Equipment,
            Some((3, 2, tid3, tid4)) => ItemClass::Container { tid3, tid4 },
            Some((3, 3, tid3, tid4)) => ItemClass::Expendable { tid3, tid4 },
            _ => ItemClass::Unknown,
        }
    }
}

/// Decode a group-spawn payload. `spawning` is the `kind` from
/// `GroupEntitySpawnBegin` (spawn vs despawn); `count` is its entity count.
/// Never panics: a short or unrecognized record stops the batch and returns the
/// records parsed so far.
pub fn parse_group_spawn(
    raw: &[u8],
    spawning: bool,
    count: u16,
    resolver: &impl RefResolver,
) -> GroupSpawnParse {
    let mut out = GroupSpawnParse::default();
    let mut r = Reader::new(raw);

    if !spawning {
        for i in 0..count {
            match r.u32() {
                Some(id) => out.despawns.push(id),
                None => {
                    bevy::log::warn!("entity_spawn: despawn list truncated at {}/{}", i, count);
                    break;
                }
            }
        }
        return out;
    }

    let mut i = 0;
    while i < count {
        let record_start = r.pos();
        match parse_spawn_record(&mut r, resolver) {
            Some(entity) => out.spawns.push(entity),
            None => {
                // An unclassifiable ref id has no derivable record width, so
                // the whole batch used to end here — losing every entity
                // behind it (#426). Try to derive the width from the payload
                // instead and carry on without that one entity (it has no
                // characterdata row, so it could not be rendered anyway).
                let unresolved = raw
                    .get(record_start..record_start + 4)
                    .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
                    .filter(|id| resolver.resolve(*id) == RefType::Unknown);
                if let Some(ref_id) = unresolved {
                    if let Some(resume) =
                        recover_unknown_record(raw, record_start, count - i - 1, resolver)
                            .filter(|resume| r.seek(*resume).is_some())
                    {
                        bevy::log::warn!(
                            "entity_spawn: record {}/{} ref_id={} is in no client table; \
                             skipped its {} bytes (width derived from the payload) and kept \
                             the rest of the batch",
                            i,
                            count,
                            ref_id,
                            resume - record_start,
                        );
                        out.unresolved.push(ref_id);
                        i += 1;
                        continue;
                    }
                }
                // Rich diagnostics so a live capture pins the record layout: the
                // ref id, what it resolved to, where the record began, how far the
                // parse got, and the record's own bytes (not just the head).
                let ref_id = raw
                    .get(record_start..record_start + 4)
                    .map(|b| u32::from_le_bytes(b.try_into().unwrap()));
                let ref_type = ref_id.map(|id| resolver.resolve(id));
                bevy::log::warn!(
                    "entity_spawn: could not parse spawn record {}/{} ref_id={:?} type={:?}; \
                     record started at {}, parse stopped at {}. record bytes: {}",
                    i,
                    count,
                    ref_id,
                    ref_type,
                    record_start,
                    r.pos(),
                    hexdump(raw.get(record_start..).unwrap_or(&[]), 192),
                );
                break;
            }
        }
        i += 1;
    }
    out
}

/// Derive the width of a record whose ref id is in no client table, so the
/// batch can continue behind it.
///
/// Idea: the ref id cannot be classified (the server's characterdata has rows
/// this client's tables lack — live refs 9251/9252/36030 sit in `npcpos.txt`
/// but in no `characterdata_*.txt`), so the record's layout is unknown. Its
/// *width* is still decidable from the payload: assume the record's body is
/// `n` bytes, parse the records that must follow it, and keep `n` only if they
/// all parse and land exactly on the end of the payload — a group-spawn body
/// holds whole records and nothing else (verified over
/// `packet_dump/0x3019.log`: every spawn payload is consumed to its last byte
/// by its `GroupEntitySpawnBegin` count). A wrong `n` shifts every following
/// record, so it practically never survives that. Accepted only when exactly
/// one `n` does; anything else keeps the batch-abort behaviour.
fn recover_unknown_record(
    raw: &[u8],
    record_start: usize,
    remaining: u16,
    resolver: &impl RefResolver,
) -> Option<usize> {
    let body_start = record_start.checked_add(4)?;
    let max_body = raw.len().checked_sub(body_start)?;
    let mut found = None;
    for body_len in 0..=max_body {
        let resume = body_start + body_len;
        let mut probe = Reader::new(raw);
        probe.seek(resume)?;
        if (0..remaining).any(|_| parse_spawn_record(&mut probe, resolver).is_none()) {
            continue;
        }
        if probe.pos() != raw.len() {
            continue;
        }
        if found.is_some() {
            // Two widths both explain the payload: no basis to pick one.
            return None;
        }
        found = Some(resume);
    }
    found
}

fn parse_spawn_record(r: &mut Reader, resolver: &impl RefResolver) -> Option<SpawnedEntity> {
    let ref_id = r.u32()?;
    match resolver.resolve(ref_id) {
        RefType::Player => parse_player(r, ref_id, resolver),
        RefType::Monster => parse_character(r, ref_id, true),
        RefType::Npc => parse_character(r, ref_id, false),
        RefType::Cos { kind } => parse_cos(r, ref_id, kind),
        RefType::Item { equipment, gold } => parse_item(r, ref_id, equipment, gold),
        RefType::Structure => parse_structure(r, ref_id),
        RefType::Unknown => None,
    }
}

/// Gate-building record — 36 bytes, captured 40× in `packet_dump/0x3019.log`
/// (Jangan dimensional gate, always its own count-1 batch):
/// `2e080000 0c000000 a861 00c09c44 0000c0c0 00c0ab44 0000` + 12-byte tail
/// `01 00 00 01 00 00 00 00 00 00 00 00` = ref 2094, uid 12, region 25000,
/// (1254, -6, 1374), heading 0 — exactly the teleportbuilding.txt row's
/// position. The tail's layout is unknown (no movement/state block fits in
/// 12 bytes) and is skipped whole; the constant `01 00 00 01` prefix will
/// flag any variant via the batch-abort diagnostics if other gates differ.
fn parse_structure(r: &mut Reader, ref_id: u32) -> Option<SpawnedEntity> {
    let unique_id = r.u32()?;
    let position = r.position()?;
    r.skip(12)?;
    Some(SpawnedEntity {
        ref_id,
        unique_id,
        position,
        name: None,
        guild: None,
        state: None,
        spawn_rarity: None,
        talk_options: Vec::new(),
        kind: SpawnKind::Structure,
    })
}

fn parse_player(r: &mut Reader, ref_id: u32, resolver: &impl RefResolver) -> Option<SpawnedEntity> {
    // scale, hwan_level, pvp_cape, auto_invest_xp, base_inventory_size
    r.skip(5)?;
    let mut equipment = Vec::new();
    read_item_list(r, resolver, &mut equipment)?; // inventory (equipped gear)
    let _avatar_slots = r.u8()?;
    read_item_list(r, resolver, &mut equipment)?; // avatar items
    let _has_mask = r.u8()?;

    let unique_id = r.u32()?;
    let position = r.position()?;
    r.skip_movement(position.region)?;
    let state = r.character_state()?;
    let name = r.string()?;
    // job_type, job_level, pk_state
    r.skip(3)?;
    // A mounted player inserts its COS's unique id between the riding and
    // scroll flags ([S] xBot PacketParser.cs:744-747; the old flat skip(8)
    // desynced the batch by 4 bytes whenever a rider was in view).
    let riding = r.u8()? != 0;
    let _in_combat = r.u8()?;
    let riding_uid = if riding { Some(r.u32()?) } else { None };
    // scroll, interact, unk
    r.skip(3)?;
    // guildless players send an empty name here, not an absent block
    let guild = r.guild()?;
    // equipment_cooldown, pk_flag(0xFF)
    r.skip(2)?;

    Some(SpawnedEntity {
        ref_id,
        unique_id,
        position,
        name: Some(name),
        guild: (!guild.name.is_empty()).then_some(guild),
        state: Some(state),
        spawn_rarity: None,
        talk_options: Vec::new(),
        kind: SpawnKind::Player {
            equipment,
            riding_uid,
        },
    })
}

/// Read a `u8`-count-prefixed equipment list, appending each item's
/// `(ref id, opt level)` to `out`. Equipment items carry a trailing
/// enhancement (+N opt) byte; non-equipment (avatar attachments without
/// one) records 0.
fn read_item_list(
    r: &mut Reader,
    resolver: &impl RefResolver,
    out: &mut Vec<(u32, u8)>,
) -> Option<()> {
    let count = r.u8()?;
    for _ in 0..count {
        let item_ref = r.u32()?;
        let opt_level = if resolver.item_is_equipment(item_ref) {
            r.u8()?
        } else {
            0
        };
        out.push((item_ref, opt_level));
    }
    Some(())
}

fn parse_character(r: &mut Reader, ref_id: u32, is_monster: bool) -> Option<SpawnedEntity> {
    let unique_id = r.u32()?;
    let position = r.position()?;
    r.skip_movement(position.region)?;
    let state = r.character_state()?;
    // Interaction options: a tag byte (0 = none), and for a talkable entity a
    // `u8` option count followed by that many 1-byte option ids. (Verified
    // against a live capture: tag=2, count=4, then 4 option bytes.)
    let mut talk_options = Vec::new();
    if r.u8()? != 0 {
        let option_count = r.u8()?;
        for _ in 0..option_count {
            talk_options.push(r.u8()?);
        }
    }
    let spawn_rarity = if is_monster { Some(r.u8()?) } else { None };
    Some(SpawnedEntity {
        ref_id,
        unique_id,
        position,
        name: None,
        guild: None,
        state: Some(state),
        spawn_rarity,
        talk_options,
        kind: if is_monster {
            SpawnKind::Monster
        } else {
            SpawnKind::Npc
        },
    })
}

/// COS record ([S] xBot `PacketParser.cs:791-807`): the shared NPC head
/// (uid, position, movement, state, talk options), then an owner tail selected
/// by the ref's characterdata tid4 — the wire carries **no** subtype byte.
/// Ride-only horses (tid4 1) have no tail; everything else ends in
/// `OwnerUniqueID`, with pets prefixing their given name, pick pets omitting
/// the PVP-state byte and guild guards inserting an `OwnerObjectID`. Routing
/// these rows through the plain NPC parse (the pre-COS behavior) left the tail
/// unconsumed and desynced every later record in the batch.
fn parse_cos(r: &mut Reader, ref_id: u32, kind: CosKind) -> Option<SpawnedEntity> {
    let unique_id = r.u32()?;
    let position = r.position()?;
    r.skip_movement(position.region)?;
    let state = r.character_state()?;
    // The talk-options block is shared with NPCs (a summoned COS sends tag 0).
    let mut talk_options = Vec::new();
    if r.u8()? != 0 {
        let option_count = r.u8()?;
        for _ in 0..option_count {
            talk_options.push(r.u8()?);
        }
    }
    let mut pet_name = None;
    let mut owner_name = None;
    let mut owner_uid = None;
    if kind != CosKind::Vehicle {
        if matches!(kind, CosKind::GrowthPet | CosKind::GrabPet) {
            pet_name = Some(r.string()?).filter(|n| !n.is_empty());
        }
        owner_name = Some(r.string()?).filter(|n| !n.is_empty());
        let _job_type = r.u8()?;
        if kind != CosKind::GrabPet {
            let _pvp_state = r.u8()?;
        }
        if kind == CosKind::Fellow {
            let _owner_obj_id = r.u32()?;
        }
        owner_uid = Some(r.u32()?);
    }
    Some(SpawnedEntity {
        ref_id,
        unique_id,
        position,
        name: None,
        guild: None,
        state: Some(state),
        spawn_rarity: None,
        talk_options,
        kind: SpawnKind::Cos {
            kind,
            pet_name,
            owner_name,
            owner_uid,
        },
    })
}

/// Dropped item record. Layout follows skrillax's `ItemSpawnData` (owner as a
/// flagged optional), corrected against live vSRO 1.188 captures
/// (`packet_dump/0x3015.log`, 2026-07-24): the drop-source tail is present on
/// **every** dropped item, gold included —
/// `ref u32, [amount u32 | upgrade u8], uid u32, pos, owner flag(+u32),
/// rarity u8, drop source u8, dropper uid u32`.
fn parse_item(r: &mut Reader, ref_id: u32, equipment: bool, gold: bool) -> Option<SpawnedEntity> {
    let mut amount = None;
    if equipment {
        r.skip(1)?; // upgrade / opt level
    } else if gold {
        amount = Some(r.u32()?);
    }
    let unique_id = r.u32()?;
    let position = r.position()?;
    // owner: flag byte + optional owner id
    if r.u8()? != 0 {
        r.skip(4)?;
    }
    // The drop's rarity class, kept rather than skipped: it is the only thing
    // on the wire that says whether the pile is a plain, a "blue" (magic-option)
    // or a Seal-grade drop. A ground drop carries NO item body — no opt level,
    // no `mag_params` — so this byte is the sole source for the drop label's
    // colour. Values beyond 0/1 are UNKNOWN and are carried through unmapped
    // rather than guessed at.
    let rarity = r.u8()?;
    r.skip(5)?; // drop source (u8) + dropper uid (u32)
    Some(SpawnedEntity {
        ref_id,
        unique_id,
        position,
        name: None,
        guild: None,
        state: None,
        spawn_rarity: Some(rarity),
        talk_options: Vec::new(),
        kind: SpawnKind::Item { amount },
    })
}

#[cfg(test)]
mod test {
    use super::*;
    use std::collections::HashMap;

    /// A resolver driven by an explicit ref-id → type map (unknown ids resolve
    /// to `Unknown`).
    struct MockResolver {
        types: HashMap<u32, RefType>,
    }

    impl RefResolver for MockResolver {
        fn resolve(&self, ref_id: u32) -> RefType {
            self.types.get(&ref_id).copied().unwrap_or(RefType::Unknown)
        }
        fn item_is_equipment(&self, ref_id: u32) -> bool {
            matches!(
                self.types.get(&ref_id),
                Some(RefType::Item {
                    equipment: true,
                    ..
                })
            )
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
        /// A standing position + movement + character-state block with no buffs.
        fn pos_move_state(self, region: u16, x: f32, y: f32, z: f32, heading: u16) -> Self {
            self.u16(region)
                .f32(x)
                .f32(y)
                .f32(z)
                .u16(heading)
                // movement: standing (has_dest=0, move_type, unk, heading)
                .u8(0)
                .u8(1)
                .u8(0)
                .u16(heading)
                // character-state: life/unk/motion/body + 3 speeds + 0 buffs
                .u8(1)
                .u8(0)
                .u8(0)
                .u8(0)
                .f32(16.0)
                .f32(50.0)
                .f32(0.0)
                .u8(0)
        }
    }

    fn resolver(entries: &[(u32, RefType)]) -> MockResolver {
        MockResolver {
            types: entries.iter().copied().collect(),
        }
    }

    #[test]
    fn parses_player_with_mixed_equipment() {
        const PLAYER_REF: u32 = 1907;
        const SWORD: u32 = 100; // equipment -> carries opt byte
        const NECKLACE: u32 = 200; // non-equipment -> no opt byte
        let res = resolver(&[
            (PLAYER_REF, RefType::Player),
            (
                SWORD,
                RefType::Item {
                    equipment: true,
                    gold: false,
                },
            ),
            (
                NECKLACE,
                RefType::Item {
                    equipment: false,
                    gold: false,
                },
            ),
        ]);

        let body = Body::default()
            .u32(PLAYER_REF)
            // scale, hwan, pvp_cape, autoxp, base_inv_size
            .u8(0)
            .u8(0)
            .u8(0)
            .u8(1)
            .u8(45)
            // inventory: 2 items
            .u8(2)
            .u32(SWORD)
            .u8(3) // equipment: opt byte present
            .u32(NECKLACE) // non-equipment: no opt byte
            // avatar slots + 0 avatar items + has_mask
            .u8(5)
            .u8(0)
            .u8(0)
            // unique id + position/movement/state
            .u32(352808)
            .pos_move_state(0x60A8, 1058.0, -7.68, 1426.0, 12268)
            // name
            .string("Remote")
            // job_type, job_level, pk_state, transport, in_combat, scroll, interact, unk
            .raw(&[0, 1, 0xFF, 0, 0, 0, 0, 0])
            // guild: empty name, guild id, empty nick, 3×u32 + 2×u8
            .string("")
            .u32(0)
            .string("")
            .raw(&[0u8; 14])
            // equipment_cooldown, pk_flag
            .u8(0)
            .u8(0xFF)
            .0;

        let parsed = parse_group_spawn(&body, true, 1, &res);
        assert_eq!(parsed.despawns.len(), 0);
        assert_eq!(parsed.spawns.len(), 1, "player should parse");
        let e = &parsed.spawns[0];
        assert_eq!(e.ref_id, PLAYER_REF);
        assert_eq!(e.unique_id, 352808);
        assert_eq!(e.name.as_deref(), Some("Remote"));
        // guildless: the block is present but its name is empty
        assert_eq!(e.guild, None);
        assert_eq!(e.position.region, 0x60A8);
        assert_eq!(e.position.x, 1058.0);
        assert_eq!(
            e.kind,
            SpawnKind::Player {
                // the sword's +3 opt byte is captured; non-equipment has none
                equipment: vec![(SWORD, 3), (NECKLACE, 0)],
                riding_uid: None,
            }
        );
        let state = e.state.as_ref().expect("characters carry a state block");
        assert_eq!(state.walk_speed, 16.0);
        assert_eq!(state.run_speed, 50.0);
    }

    #[test]
    fn parses_npc_and_monster() {
        const NPC_REF: u32 = 1900;
        const MOB_REF: u32 = 2000;
        let res = resolver(&[(NPC_REF, RefType::Npc), (MOB_REF, RefType::Monster)]);

        // NPC then monster in one batch, so the NPC's length must be exact for
        // the monster to parse.
        let body = Body::default()
            .u32(NPC_REF)
            .u32(11)
            .pos_move_state(0x60A8, 1.0, 2.0, 3.0, 0)
            .u8(0) // talk flag: none
            .u32(MOB_REF)
            .u32(22)
            .pos_move_state(0x60A8, 4.0, 5.0, 6.0, 0)
            .u8(0) // talk flag: none
            .u8(1) // rarity (monster only)
            .0;

        let parsed = parse_group_spawn(&body, true, 2, &res);
        assert_eq!(parsed.spawns.len(), 2);
        assert_eq!(parsed.spawns[0].unique_id, 11);
        assert_eq!(parsed.spawns[0].kind, SpawnKind::Npc);
        assert_eq!(parsed.spawns[0].spawn_rarity, None);
        assert_eq!(parsed.spawns[1].unique_id, 22);
        assert_eq!(parsed.spawns[1].kind, SpawnKind::Monster);
        assert_eq!(parsed.spawns[1].spawn_rarity, Some(1));
    }

    #[test]
    fn parses_talkable_npc_with_options() {
        // A talkable NPC carries interaction options: tag=2, then a u8 count and
        // that many 1-byte option ids (values from a live capture). A second NPC
        // follows so the option list's length must be consumed exactly.
        const NPC_REF: u32 = 1900;
        let res = resolver(&[(NPC_REF, RefType::Npc)]);
        let body = Body::default()
            .u32(NPC_REF)
            .u32(99)
            .pos_move_state(0x60A8, 1.0, 2.0, 3.0, 0)
            .u8(2) // interact tag: talk
            .u8(4) // option count
            .raw(&[0x01, 0x02, 0x04, 0x20]) // 4 option ids
            .u32(NPC_REF)
            .u32(100)
            .pos_move_state(0x60A8, 4.0, 5.0, 6.0, 0)
            .u8(0) // second NPC: no options
            .0;

        let parsed = parse_group_spawn(&body, true, 2, &res);
        assert_eq!(
            parsed.spawns.len(),
            2,
            "the option list must be consumed exactly"
        );
        assert_eq!(parsed.spawns[0].unique_id, 99);
        assert_eq!(parsed.spawns[0].talk_options, vec![0x01, 0x02, 0x04, 0x20]);
        assert_eq!(parsed.spawns[1].unique_id, 100);
        assert!(parsed.spawns[1].talk_options.is_empty());
    }

    #[test]
    fn parses_gold_drop() {
        const GOLD_REF: u32 = 3000;
        let res = resolver(&[(
            GOLD_REF,
            RefType::Item {
                equipment: false,
                gold: true,
            },
        )]);
        // mirrors a live 0x3015 gold line: gold carries the drop-source tail
        let body = Body::default()
            .u32(GOLD_REF)
            .u32(5000) // amount
            .u32(77) // unique id
            .u16(0x60A8)
            .f32(1.0)
            .f32(2.0)
            .f32(3.0)
            .u16(0) // position
            .u8(1) // owner flag: present
            .u32(4) // owner jid
            .u8(0) // rarity
            .u8(5) // drop source
            .u32(0x147A8) // dropper uid
            .0;
        let parsed = parse_group_spawn(&body, true, 1, &res);
        assert_eq!(parsed.spawns.len(), 1);
        assert_eq!(parsed.spawns[0].unique_id, 77);
        assert_eq!(
            parsed.spawns[0].kind,
            SpawnKind::Item { amount: Some(5000) }
        );
        // the drop's rarity byte is kept, not skipped: it is the only source
        // for the drop label's colour (a ground drop carries no item body)
        assert_eq!(parsed.spawns[0].spawn_rarity, Some(0));
    }

    #[test]
    fn parses_despawn_list() {
        let res = resolver(&[]);
        let body = Body::default().u32(10).u32(20).u32(30).0;
        let parsed = parse_group_spawn(&body, false, 3, &res);
        assert_eq!(parsed.despawns, vec![10, 20, 30]);
        assert_eq!(parsed.spawns.len(), 0);
    }

    #[test]
    fn unknown_ref_stops_batch_safely() {
        // First record is a good NPC; second ref id is unknown -> stop after one.
        const NPC_REF: u32 = 1900;
        let res = resolver(&[(NPC_REF, RefType::Npc)]);
        let body = Body::default()
            .u32(NPC_REF)
            .u32(11)
            .pos_move_state(0x60A8, 1.0, 2.0, 3.0, 0)
            .u8(0)
            .u32(999999) // unknown ref
            .raw(&[0u8; 40])
            .0;
        let parsed = parse_group_spawn(&body, true, 2, &res);
        assert_eq!(
            parsed.spawns.len(),
            1,
            "keeps the record parsed before the bad one"
        );
        assert_eq!(parsed.spawns[0].unique_id, 11);
    }

    /// Verbatim `packet_dump/0x3019.log` payload (2026-08-11 09:43:45.339Z,
    /// 98 bytes) whose `GroupEntitySpawnBegin` of the same instant is
    /// `01 02 00` — spawn, 2 records. Record 0 is ref 9252 (in `npcpos.txt`,
    /// in NO `characterdata_*.txt` of either of the user's Media.pk2 — the
    /// ids jump 8984 -> 9264), record 1 is ref 3861 NPC_CH_EVENT_KISAENG1.
    /// Each record is 49 bytes.
    const LIVE_UNKNOWN_NPC_BATCH: &[u8] = &[
        0x24, 0x24, 0x00, 0x00, 0x6b, 0x01, 0x00, 0x00, 0xa8, 0x61, 0xec, 0x51, 0x74, 0x44, 0x7f,
        0x6f, 0x02, 0xc2, 0x52, 0xe8, 0x35, 0x44, 0xb4, 0xc0, 0x00, 0x01, 0x00, 0xb4, 0xc0, 0x01,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xc8, 0x42,
        0x00, 0x02, 0x01, 0x12, 0x15, 0x0f, 0x00, 0x00, 0x6d, 0x01, 0x00, 0x00, 0xa8, 0x61, 0xc3,
        0xb5, 0x89, 0x44, 0x7f, 0x6f, 0x02, 0xc2, 0x33, 0x93, 0x55, 0x44, 0xff, 0x3f, 0x00, 0x01,
        0x00, 0xff, 0x3f, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0xc8, 0x42, 0x00, 0x02, 0x01, 0x02,
    ];

    #[test]
    fn live_unknown_ref_keeps_the_rest_of_the_batch() {
        // #426: the unclassifiable record used to end the batch, so the NPC
        // *behind* it never spawned. Its width (49 bytes) is derivable from
        // the payload — it is the only one that lets the second record parse
        // and land exactly on the last byte — so the batch survives.
        let res = resolver(&[(3861, RefType::Npc)]);
        let parsed = parse_group_spawn(LIVE_UNKNOWN_NPC_BATCH, true, 2, &res);

        assert_eq!(parsed.unresolved, vec![9252]);
        assert_eq!(parsed.spawns.len(), 1, "the record behind the unknown one");
        let npc = &parsed.spawns[0];
        assert_eq!(npc.ref_id, 3861);
        assert_eq!(npc.unique_id, 0x16d);
        assert_eq!(npc.position.region, 25000);
        assert_eq!(npc.kind, SpawnKind::Npc);
    }

    #[test]
    fn unknown_ref_still_stops_the_batch_when_no_width_fits() {
        // The skip is only allowed when a width makes the remaining records
        // parse to the payload's last byte. Cut the capture's last byte off
        // and none does, so the batch must abort as before instead of
        // inventing a boundary.
        let res = resolver(&[(3861, RefType::Npc)]);
        let truncated = &LIVE_UNKNOWN_NPC_BATCH[..LIVE_UNKNOWN_NPC_BATCH.len() - 1];
        let parsed = parse_group_spawn(truncated, true, 2, &res);

        assert!(parsed.unresolved.is_empty());
        assert!(parsed.spawns.is_empty());
    }

    #[test]
    fn truncated_body_fails_safe() {
        const NPC_REF: u32 = 1900;
        let res = resolver(&[(NPC_REF, RefType::Npc)]);
        // ref id present but the rest of the record is cut off.
        let body = Body::default().u32(NPC_REF).u16(5).0;
        let parsed = parse_group_spawn(&body, true, 1, &res);
        assert_eq!(parsed.spawns.len(), 0);
        assert_eq!(parsed.despawns.len(), 0);
    }

    #[test]
    fn parses_captured_gate_building_record() {
        // verbatim packet_dump/0x3019.log payload (Jangan dimensional gate,
        // ref 2094 = STORE_CH_GATE, captured 40x as its own count-1 batch)
        const GATE_REF: u32 = 2094;
        let body: &[u8] = &[
            0x2e, 0x08, 0x00, 0x00, // ref 2094
            0x0c, 0x00, 0x00, 0x00, // uid 12
            0xa8, 0x61, // region 25000
            0x00, 0xc0, 0x9c, 0x44, // x 1254.0
            0x00, 0x00, 0xc0, 0xc0, // y -6.0
            0x00, 0xc0, 0xab, 0x44, // z 1374.0
            0x00, 0x00, // heading 0
            0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, // 12-byte unknown tail
        ];
        let res = resolver(&[(GATE_REF, RefType::Structure)]);
        let parsed = parse_group_spawn(body, true, 1, &res);
        assert_eq!(parsed.spawns.len(), 1);
        let gate = &parsed.spawns[0];
        assert_eq!(gate.ref_id, GATE_REF);
        assert_eq!(gate.unique_id, 12);
        assert_eq!(gate.position.region, 25000);
        assert_eq!(
            (gate.position.x, gate.position.y, gate.position.z),
            (1254.0, -6.0, 1374.0)
        );
        assert_eq!(gate.kind, SpawnKind::Structure);
    }

    /// The shared NPC head of a COS record (uid + position/movement/state +
    /// empty talk block), ready for the kind-specific owner tail.
    fn cos_head(ref_id: u32, uid: u32) -> Body {
        Body::default()
            .u32(ref_id)
            .u32(uid)
            .pos_move_state(0x60A8, 1.0, 2.0, 3.0, 0)
            .u8(0) // talk flag: none
    }

    /// A trailing NPC record used to prove the COS record before it ended
    /// exactly on its boundary.
    fn follower_npc(body: Body, ref_id: u32, uid: u32) -> Body {
        body.u32(ref_id)
            .u32(uid)
            .pos_move_state(0x60A8, 4.0, 5.0, 6.0, 0)
            .u8(0)
    }

    /// Ride-only horses (tid4 1) carry no owner tail at all — a follower
    /// record right after the talk block must still parse.
    #[test]
    fn parses_horse_cos_without_owner_tail() {
        const HORSE_REF: u32 = 2137; // COS_C_HORSE1
        const NPC_REF: u32 = 1900;
        let res = resolver(&[
            (
                HORSE_REF,
                RefType::Cos {
                    kind: CosKind::Vehicle,
                },
            ),
            (NPC_REF, RefType::Npc),
        ]);
        let body = follower_npc(cos_head(HORSE_REF, 41), NPC_REF, 42).0;
        let parsed = parse_group_spawn(&body, true, 2, &res);
        assert_eq!(parsed.spawns.len(), 2, "horse record must end after talk");
        assert_eq!(
            parsed.spawns[0].kind,
            SpawnKind::Cos {
                kind: CosKind::Vehicle,
                pet_name: None,
                owner_name: None,
                owner_uid: None,
            }
        );
        assert_eq!(parsed.spawns[1].kind, SpawnKind::Npc);
    }

    /// Transports (tid4 2): OwnerName, Job, PVPState, OwnerUniqueID.
    #[test]
    fn parses_transport_cos_owner_tail() {
        const TRANSPORT_REF: u32 = 2184; // COS_T_HORSE1
        const NPC_REF: u32 = 1900;
        let res = resolver(&[
            (
                TRANSPORT_REF,
                RefType::Cos {
                    kind: CosKind::Transport,
                },
            ),
            (NPC_REF, RefType::Npc),
        ]);
        let body = follower_npc(
            cos_head(TRANSPORT_REF, 51)
                .string("Owner") // owner name
                .u8(0) // job type
                .u8(0xFF) // pvp state
                .u32(0x0102_0304), // owner uid
            NPC_REF,
            52,
        )
        .0;
        let parsed = parse_group_spawn(&body, true, 2, &res);
        assert_eq!(parsed.spawns.len(), 2);
        assert_eq!(
            parsed.spawns[0].kind,
            SpawnKind::Cos {
                kind: CosKind::Transport,
                pet_name: None,
                owner_name: Some("Owner".into()),
                owner_uid: Some(0x0102_0304),
            }
        );
    }

    /// Attack pets (tid4 3) prefix their given name; pick pets (tid4 4) skip
    /// the PVP-state byte; guild guards (tid4 5) insert an OwnerObjectID.
    #[test]
    fn parses_pet_and_guard_cos_tails() {
        const ATTACK_REF: u32 = 6106; // COS_P_WOLF
        const PICK_REF: u32 = 10365; // COS_P_RABBIT
        const GUARD_REF: u32 = 2100;
        let res = resolver(&[
            (
                ATTACK_REF,
                RefType::Cos {
                    kind: CosKind::GrowthPet,
                },
            ),
            (
                PICK_REF,
                RefType::Cos {
                    kind: CosKind::GrabPet,
                },
            ),
            (
                GUARD_REF,
                RefType::Cos {
                    kind: CosKind::Fellow,
                },
            ),
        ]);
        let body = cos_head(ATTACK_REF, 61)
            .string("Rex") // pet name
            .string("Owner")
            .u8(0) // job
            .u8(0xFF) // pvp
            .u32(1001) // owner uid
            // pick pet: name + owner, job, NO pvp byte, owner uid
            .raw(&cos_head(PICK_REF, 62).0)
            .string("Hoppel")
            .string("Owner")
            .u8(0)
            .u32(1001)
            // guild guard: no pet name, owner, job, pvp, owner OBJECT id, owner uid
            .raw(&cos_head(GUARD_REF, 63).0)
            .string("GuildMate")
            .u8(0)
            .u8(0xFF)
            .u32(777) // owner object id
            .u32(1002)
            .0;
        let parsed = parse_group_spawn(&body, true, 3, &res);
        assert_eq!(parsed.spawns.len(), 3, "all three tails must be exact");
        assert_eq!(
            parsed.spawns[0].kind,
            SpawnKind::Cos {
                kind: CosKind::GrowthPet,
                pet_name: Some("Rex".into()),
                owner_name: Some("Owner".into()),
                owner_uid: Some(1001),
            }
        );
        assert_eq!(
            parsed.spawns[1].kind,
            SpawnKind::Cos {
                kind: CosKind::GrabPet,
                pet_name: Some("Hoppel".into()),
                owner_name: Some("Owner".into()),
                owner_uid: Some(1001),
            }
        );
        assert_eq!(
            parsed.spawns[2].kind,
            SpawnKind::Cos {
                kind: CosKind::Fellow,
                pet_name: None,
                owner_name: Some("GuildMate".into()),
                owner_uid: Some(1002),
            }
        );
    }

    /// A truncated COS owner tail fails safe instead of panicking.
    #[test]
    fn truncated_cos_tail_fails_safe() {
        const TRANSPORT_REF: u32 = 2184;
        let res = resolver(&[(
            TRANSPORT_REF,
            RefType::Cos {
                kind: CosKind::Transport,
            },
        )]);
        let body = cos_head(TRANSPORT_REF, 51).string("Owner").u8(0).0;
        let parsed = parse_group_spawn(&body, true, 1, &res);
        assert_eq!(parsed.spawns.len(), 0);
    }

    /// Build a one-player spawn body whose guild block carries `guild_name` and
    /// `nick`. Guildless players send the same block with an empty name — the
    /// block is never omitted (go-sro `WriteGuild`, corroborated by the vSRO
    /// client parser; see `Reader::guild`). `riding_uid` inserts the mounted
    /// player's conditional COS uid between the state flags.
    fn player_body(
        player_ref: u32,
        guild_name: &str,
        nick: &str,
        riding_uid: Option<u32>,
    ) -> Vec<u8> {
        let mut body = Body::default()
            .u32(player_ref)
            // scale, hwan, pvp_cape, autoxp, base_inv_size
            .raw(&[0u8; 5])
            // 0 inventory items, 0 avatar slots, 0 avatar items, has_mask
            .u8(0)
            .u8(0)
            .u8(0)
            .u8(0)
            .u32(352808)
            .pos_move_state(0x60A8, 1058.0, -7.68, 1426.0, 12268)
            .string("Remote")
            // job_type, job_level, pk_state
            .raw(&[0, 1, 0xFF])
            // riding flag, in_combat, [riding uid]
            .u8(riding_uid.is_some() as u8)
            .u8(0);
        if let Some(uid) = riding_uid {
            body = body.u32(uid);
        }
        body
            // scroll, interact, unk
            .raw(&[0, 0, 0])
            // guild block
            .string(guild_name)
            .u32(7)
            .string(nick)
            .raw(&[0u8; 14])
            // equipment_cooldown, pk_flag
            .u8(0)
            .u8(0xFF)
            .0
    }

    fn player_body_with_guild(player_ref: u32, guild_name: &str, nick: &str) -> Vec<u8> {
        player_body(player_ref, guild_name, nick, None)
    }

    /// A mounted player inserts a `u32` mount uid; the old flat skip desynced
    /// the batch, so a follower record proves the boundary ([S] until F7).
    #[test]
    fn parses_mounted_player_and_keeps_the_boundary() {
        const PLAYER_REF: u32 = 1907;
        const NPC_REF: u32 = 1900;
        let res = resolver(&[(PLAYER_REF, RefType::Player), (NPC_REF, RefType::Npc)]);
        let mut body = player_body(PLAYER_REF, "", "", Some(4711));
        body.extend_from_slice(&follower_npc(Body::default(), NPC_REF, 99).0);

        let parsed = parse_group_spawn(&body, true, 2, &res);
        assert_eq!(parsed.spawns.len(), 2, "rider + follower must both parse");
        assert_eq!(
            parsed.spawns[0].kind,
            SpawnKind::Player {
                equipment: vec![],
                riding_uid: Some(4711),
            }
        );
        assert_eq!(parsed.spawns[1].unique_id, 99);
    }

    /// The guild block used to be skipped wholesale (#30). A guilded player's
    /// name and granted nick now reach the spawn record.
    #[test]
    fn parses_the_guild_block_of_a_guilded_player() {
        const PLAYER_REF: u32 = 1907;
        let res = resolver(&[(PLAYER_REF, RefType::Player)]);
        let body = player_body_with_guild(PLAYER_REF, "Ironclad", "Quartermaster");

        let parsed = parse_group_spawn(&body, true, 1, &res);
        assert_eq!(parsed.spawns.len(), 1, "guilded player should parse");
        let guild = parsed.spawns[0]
            .guild
            .as_ref()
            .expect("guilded player carries a guild tag");
        assert_eq!(guild.name, "Ironclad");
        assert_eq!(guild.granted_nick, "Quartermaster");
        // the record must still end exactly where it did before
        assert_eq!(parsed.spawns[0].name.as_deref(), Some("Remote"));
        assert_eq!(parsed.spawns[0].unique_id, 352808);
    }

    /// An empty guild name is guildless, not a guild called "" — otherwise
    /// every player would draw a blank second nameplate line.
    #[test]
    fn empty_guild_name_parses_as_guildless() {
        const PLAYER_REF: u32 = 1907;
        let res = resolver(&[(PLAYER_REF, RefType::Player)]);
        let body = player_body_with_guild(PLAYER_REF, "", "");

        let parsed = parse_group_spawn(&body, true, 1, &res);
        assert_eq!(parsed.spawns.len(), 1);
        assert_eq!(parsed.spawns[0].guild, None);
    }

    /// The guild block is variable-length, so reading it must leave the record
    /// boundary intact: a second record after a guilded one still parses.
    #[test]
    fn guild_block_does_not_desync_the_next_record() {
        const PLAYER_REF: u32 = 1907;
        let res = resolver(&[(PLAYER_REF, RefType::Player)]);
        let mut body = player_body_with_guild(PLAYER_REF, "Ironclad", "Quartermaster");
        body.extend_from_slice(&player_body_with_guild(PLAYER_REF, "", ""));

        let parsed = parse_group_spawn(&body, true, 2, &res);
        assert_eq!(parsed.spawns.len(), 2, "both records should parse");
        assert_eq!(
            parsed.spawns[0].guild.as_ref().map(|g| g.name.as_str()),
            Some("Ironclad")
        );
        assert_eq!(parsed.spawns[1].guild, None);
    }
}
