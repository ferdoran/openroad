//! COS (companion) runtime state: what the summon packets say about the COS the
//! player currently has out.
//!
//! Idea: `0x30C8 PetData` / `0x30C9 PetUpdate` were already decoded by the
//! `packets` crate and then **thrown away** — nothing in `client/` read either
//! (`docs/re/ui/cos-pet-window.md` §7, #656). This resource is the consumer: it
//! keeps the wire's own types rather than a flattened copy, so the info page
//! renders what the server actually said and nothing more.
//!
//! The one thing the wire does **not** carry is *which kind* of COS this is.
//! `0x30C8`'s body is kind-dependent, and the original resolves the kind from
//! refdata, not from the packet (#545) — so the kind comes from characterdata's
//! `TypeID4` via [`ChardataRow::cos_kind`](crate::assets::textdata::characterdata),
//! which is the single implementation of that gate.
//!
//! **More than one COS can be out at once.** A player may ride a horse while a
//! growth pet fights and a pick pet loots, so this resource holds a list keyed
//! by unique id rather than the single slot it started as; every consumer that
//! wants "the pet" asks for a kind.

use bevy::prelude::*;

use packets::agent::character_data::InventoryItem;
use packets::agent::inventory::{
    InventoryOperationResponse, InventoryOperationResult, PET_PICKUP_SLOT_NONE,
};
use packets::agent::pet::{
    CosBody, CosGrowth, CosKind, PetData, PetUpdate, PetUpdatePayload, PET_UPDATE_UNSUMMONED,
};

use crate::plugins::net::inventory::Inventory;
use crate::plugins::player::Player;
use crate::plugins::textdata::{ClientCharacterData, ClientItemData, ClientLevelData};

/// HGP is stored per-10,000. Re-exported from `packets` rather than declared
/// here: this used to be a second copy of the same 10,000 that the packet crate
/// carried as `COS_GROWTH_SCALE_DEFAULT`, before the growth block's `u16` was
/// identified as hunger (see [`packets::agent::pet::CosGrowth`]).
pub use packets::agent::pet::COS_HGP_FULL as HGP_FULL;

/// The COS the player currently has summoned, as the wire described it.
#[derive(Debug, Clone, PartialEq)]
pub struct Cos {
    /// `0x30C8` header: the COS entity's unique id.
    pub unique_id: u32,
    /// `0x30C8` header: the refdata object id the kind was resolved from.
    pub ref_obj_id: u32,
    /// Resolved from refdata, never from the packet (#545).
    pub kind: CosKind,
    /// The kind-dependent body, kept whole rather than copied field by field.
    pub body: CosBody,
    /// Hunger, per-10,000. Seeded from `0x30C8`'s growth block and then moved
    /// by `0x30C9` arm 4. `None` only for the kinds that carry no growth block
    /// *and* have had no arm 4 yet — a pick pet before its first hunger tick.
    pub hgp: Option<u16>,
    /// Exp **within the current level**, not a running total — the same thing
    /// the server stores as `_CharCos.ExpOffset`. Seeded from `0x30C8`'s growth
    /// block, then moved by `0x30C9` arm 3 through [`apply_pet_exp_gain`],
    /// which subtracts each level's threshold as it is crossed. Unsigned: arm
    /// 3's delta is signed and a death drains this, but it floors at 0.
    pub exp: u64,
    /// The pet's own level. Seeded from the growth block, then advanced locally
    /// by the exp loop — see [`apply_pet_exp_gain`] for why that is the client's
    /// job here and not the server's. `None` for the kinds that carry no growth
    /// block at all (everything but the attack pet).
    pub level: Option<u8>,
}

impl Cos {
    /// HGP as a 0.0..=1.0 fraction, from the per-10,000 wire value.
    pub fn hgp_fraction(&self) -> Option<f32> {
        self.hgp
            .map(|hgp| (hgp as f32 / HGP_FULL as f32).clamp(0.0, 1.0))
    }

    /// The pet's own name, which only the two named kinds carry.
    ///
    /// An unnamed pet sends a **zero-length string**, not an absent field
    /// (`packet_dump/0x30c8.log`'s Grey Wolf has `0000` where the length goes),
    /// so the empty case is filtered here rather than at each caller — every
    /// one of them already read this `Option` as "is it named?".
    pub fn name(&self) -> Option<&str> {
        self.body
            .name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
    }

    /// The settings word (`0x30C8`'s `unk_f`), which both pet kinds carry and
    /// which the setup page and the AI-mode toggle both read.
    pub fn settings(&self) -> u32 {
        self.body.unk_f.unwrap_or(0)
    }
}

/// Every COS the player currently has out.
///
/// A list, not a slot: a horse and a pet coexist routinely, and a pick pet may
/// be out alongside an attack pet. Ordered by summon, so `first_of_kind` gives
/// the oldest of a kind — which is the one the single-COS HUD pages mean.
#[derive(Resource, Default)]
pub struct CosState {
    pub cos: Vec<Cos>,
}

impl CosState {
    pub fn get(&self, unique_id: u32) -> Option<&Cos> {
        self.cos.iter().find(|cos| cos.unique_id == unique_id)
    }

    pub fn get_mut(&mut self, unique_id: u32) -> Option<&mut Cos> {
        self.cos.iter_mut().find(|cos| cos.unique_id == unique_id)
    }

    pub fn remove(&mut self, unique_id: u32) -> Option<Cos> {
        let index = self.cos.iter().position(|cos| cos.unique_id == unique_id)?;
        Some(self.cos.remove(index))
    }

    /// The oldest summoned COS of `kind`.
    pub fn first_of_kind(&self, kind: CosKind) -> Option<&Cos> {
        self.cos.iter().find(|cos| cos.kind == kind)
    }

    /// The COS the pet windows act on: the attack pet if one is out, else the
    /// pick pet. Vehicles have their own HUD (`cos_status`) and never drive the
    /// pet pages.
    pub fn active_pet(&self) -> Option<&Cos> {
        self.first_of_kind(CosKind::GrowthPet)
            .or_else(|| self.first_of_kind(CosKind::GrabPet))
    }
}

/// Resolve a COS ref id to its [`CosKind`] through characterdata's `TypeID4`.
///
/// Thin wrapper over the one implementation of the tid gate — this module used
/// to carry a second copy of it.
pub fn cos_kind(char_data: &ClientCharacterData, ref_obj_id: u32) -> Option<CosKind> {
    char_data.get(&(ref_obj_id as i32))?.cos_kind()
}

/// Apply `0x30C8`: a COS was summoned (or re-sent).
pub fn on_pet_data(
    mut reader: MessageReader<PetData>,
    char_data: Res<ClientCharacterData>,
    item_data: Res<ClientItemData>,
    mut state: ResMut<CosState>,
) {
    for msg in reader.read() {
        let Some(kind) = cos_kind(&char_data, msg.ref_obj_id) else {
            warn!(
                "cos: no characterdata COS row for ref {} — summon ignored",
                msg.ref_obj_id
            );
            continue;
        };
        let Some(body) = msg.body(kind, &*item_data) else {
            warn!("cos: 0x30C8 body for ref {} does not parse", msg.ref_obj_id);
            continue;
        };
        let held = state.remove(msg.unique_id);
        let (hgp, exp, level) = summon_seed(body.growth, held.as_ref());
        state.cos.push(Cos {
            unique_id: msg.unique_id,
            ref_obj_id: msg.ref_obj_id,
            kind,
            body,
            hgp,
            exp,
            level,
        });
    }
}

/// `(hgp, exp, level)` for a COS that just arrived on `0x30C8`, given whatever
/// we already held for the same unique id.
///
/// **The growth block wins.** It used to be the other way round — written when
/// the block's three scalars were `[U]` and 0x30C8 was assumed to carry a stale
/// summon-time snapshot. The captures say otherwise: the Grey Wolf's re-summon
/// on 2026-08-19 reports `exp = 77`, the running total its *previous* summon
/// ended on (three `0x30C9` arm-3 gains of +26, then the -1 death loss). So
/// 0x30C8 is the authoritative push, and the held values only survive for the
/// kinds that carry no block at all — a pick pet's HGP, which reaches us solely
/// through arm 4.
fn summon_seed(growth: Option<CosGrowth>, held: Option<&Cos>) -> (Option<u16>, u64, Option<u8>) {
    match growth {
        Some(growth) => (Some(growth.hgp), growth.exp, Some(growth.level)),
        None => (
            held.and_then(|cos| cos.hgp),
            held.map_or(0, |cos| cos.exp),
            held.and_then(|cos| cos.level),
        ),
    }
}

/// `0x30C9` arm 3 — the pet's exp gain, and the level-up it may trigger.
///
/// **The client levels the pet, not the server.** That is the original's own
/// design: `FUN_008aa340`'s arm 3 *"drives a level-up loop over
/// `FUN_00937f20(pet+0x12)`"*, while arm 7's handler swaps the model and the
/// refdata and never touches `pet+0x12`. Arm 7 is the level-up *notification*
/// (a growth stage is one characterdata row per level, so levelling up is a
/// model swap) but it carries no level for us to read, and arm 3's body is
/// `{i64 delta, u32 source}` with no level field either.
///
/// This is the opposite call to the one the character bar makes, and the
/// difference is real: `0x3056` carries an explicit level-up flag, so
/// `hud/underbar/model.rs::apply_exp_gain` can defer to the server and must
/// (#209 — private servers run custom requirements the client cannot know).
/// Arm 3 offers nothing to defer to, so the client curve is not a *worse* guess
/// than the server's word, it is the only mechanism there is.
///
/// **Verified against `packet_dump/0x30c9.log`**, the 2026-08-19T22:25 session
/// (uid `626e0200`, `COS_P_WOLF_001`): seeded at the summon's own level 1 /
/// 76 exp, the seven captured deltas reproduce the three arm-7 model swaps
/// (refs 6108/6109/6110 = stages 3, 4, 5) exactly. Two rival readings die on
/// the first kill — resetting exp to 0 per level lands on stage 2, and leveldata
/// column 5 lands on stage 8.
///
/// Only a **gain** walks the curve. A pet loses exp when it dies (the same
/// capture has a `-1`) but never a level, which is the rule the character path
/// already follows (`underbar/model.rs`, #306).
fn apply_pet_exp_gain(level: u8, exp: u64, delta: i64, level_data: &ClientLevelData) -> (u8, u64) {
    // Floors at 0 rather than wrapping: a bogus server value must not panic a
    // debug build (#218), and the offset it mirrors is unsigned.
    let mut exp = exp.saturating_add_signed(delta);
    let mut level = level;
    if delta <= 0 {
        return (level, exp);
    }
    // A `while`, not an `if`: the capture's first kill crosses two levels at
    // once, and the server sent a single arm 7 for it.
    while let Some(required) = level_data.max_exp(level) {
        if required == 0 || exp < required {
            break;
        }
        exp -= required;
        level = level.saturating_add(1);
    }
    // Past the last leveldata row `max_exp` is `None` and the loop stops there,
    // leaving the surplus to accumulate rather than spinning.
    (level, exp)
}

/// Apply `0x30C9`. Every arm the original acts on is acted on here; the two it
/// ignores ([`PetUpdatePayload::Unknown6`] and unrecognised types) are dropped
/// deliberately rather than by omission.
///
/// Note what is *not* here: the bag refresh (arm 2) needs the itemdata
/// resolver, so it runs in [`apply_cos_bag_refresh`] where that resource is
/// available.
pub fn on_pet_update(
    mut reader: MessageReader<PetUpdate>,
    level_data: Res<ClientLevelData>,
    mut state: ResMut<CosState>,
) {
    for msg in reader.read() {
        if msg.update_type == PET_UPDATE_UNSUMMONED {
            state.remove(msg.unique_id);
            continue;
        }
        let Some(cos) = state.get_mut(msg.unique_id) else {
            continue;
        };
        match &msg.payload {
            PetUpdatePayload::Hunger { hgp } => cos.hgp = Some(*hgp),
            // The server sends a *delta*, not a total, and it is signed. The
            // level moves here too — see `apply_pet_exp_gain`.
            PetUpdatePayload::Exp { delta, .. } => {
                let (level, exp) =
                    apply_pet_exp_gain(cos.level.unwrap_or(1), cos.exp, *delta, &level_data);
                cos.exp = exp;
                // A kind with no growth block has no level to advance, and
                // inventing one here would put a number in the window that the
                // wire never offered.
                if cos.level.is_some() {
                    cos.level = Some(level);
                }
            }
            PetUpdatePayload::Renamed { name } => cos.body.name = Some(name.clone()),
            // The growth-stage model swap — and the server's level-up
            // notification, though it carries no level: the ladder has one
            // characterdata row per level, so a level-up *is* a new ref object.
            // The level itself is advanced by the arm-3 loop above, which is
            // where the original puts it too. Do not snap it from this ref:
            // arm 7 arrives *before* the arm 3 that caused it (all three
            // captured pairs share a millisecond, arm 7 first), so snapping
            // would set the level early and then let the delta land without
            // its threshold subtracted. `plugins::cos` respawns the entity's
            // model from the same message.
            PetUpdatePayload::ModelChanged { new_ref_obj_id } => cos.ref_obj_id = *new_ref_obj_id,
            _ => {}
        }
    }
}

/// Apply `0x30C9` arm 2: the server replacing a pet's whole bag.
///
/// Separate from [`on_pet_update`] only because the item records are decoded
/// against the loaded itemdata, the same split `0x30C8`'s body uses.
pub fn apply_cos_bag_refresh(
    mut reader: MessageReader<PetUpdate>,
    item_data: Res<ClientItemData>,
    mut state: ResMut<CosState>,
) {
    for msg in reader.read() {
        let PetUpdatePayload::Bag { slot_capacity, .. } = &msg.payload else {
            continue;
        };
        let Some(items) = msg.bag_items(&*item_data) else {
            warn!(
                "cos bag: 0x30C9 arm 2 records for {} do not parse — capture needed",
                msg.unique_id
            );
            continue;
        };
        let Some(cos) = state.get_mut(msg.unique_id) else {
            continue;
        };
        cos.body.inventory_size = *slot_capacity;
        cos.body.items = items;
    }
}

impl Cos {
    /// The pet bag entry in `slot`, if any. The wire sends the bag as a list
    /// of slot-tagged records (`CosBody::items`), not as a dense array, so
    /// every bag op resolves its slot by search rather than by index.
    pub fn bag_get(&self, slot: u8) -> Option<&InventoryItem> {
        self.body.items.iter().find(|item| item.slot == slot)
    }

    /// Remove and return the entry in `slot`.
    pub fn bag_take(&mut self, slot: u8) -> Option<InventoryItem> {
        let index = self.body.items.iter().position(|item| item.slot == slot)?;
        Some(self.body.items.remove(index))
    }

    /// Place `item` at its own `slot`, replacing whatever was there. The
    /// server's acks are authoritative post-state (the same rule
    /// `Inventory::gain_item` follows), so this replaces rather than merges.
    pub fn bag_put(&mut self, item: InventoryItem) {
        self.bag_take(item.slot);
        self.body.items.push(item);
    }

    /// Move a bag entry between two of the pet's own slots (op 25). Whole-slot
    /// relocate-or-swap, exactly like [`crate::plugins::net::inventory::Inventory::apply_move`]:
    /// the ack echoes a quantity, but the authoritative server relocates whole
    /// slots, and splitting here would drift from it.
    pub fn bag_move(&mut self, source: u8, target: u8) {
        if source == target {
            return;
        }
        let Some(mut moved) = self.bag_take(source) else {
            return;
        };
        if let Some(mut swapped) = self.bag_take(target) {
            swapped.slot = source;
            self.body.items.push(swapped);
        }
        moved.slot = target;
        self.body.items.push(moved);
    }
}

/// Apply the five pick-pet bag ops of `0xB034`
/// (`docs/re/systems/pet-pick-cos.md` §3) to the summoned COS's bag — and, for
/// the three that cross the container boundary, to the player's own inventory.
///
/// Both sides are applied **here**, in one system, on purpose: ops 26 and 27
/// carry only two slot numbers, so the moved item record is known **only** to
/// the container it is leaving. Splitting this across `hud::inventory` and
/// `hud::cos` would mean one of the two halves guessing at an item it cannot
/// see. `hud::inventory::model::on_inventory_operation_response` therefore
/// ignores the pet ops and says so.
///
/// Every op is gated on the COS unique id: an ack for a COS we do not hold is
/// not ours to apply.
pub fn apply_cos_bag_ops(
    mut reader: MessageReader<InventoryOperationResponse>,
    item_data: Res<ClientItemData>,
    mut state: ResMut<CosState>,
    mut inventories: Query<&mut Inventory, With<Player>>,
) {
    for msg in reader.read() {
        let Some(op) = msg.operation.as_ref() else {
            continue;
        };
        let Some(cos_unique_id) = cos_bag_op_target(op) else {
            continue;
        };
        let Some(cos) = state.get_mut(cos_unique_id) else {
            continue;
        };
        match op {
            InventoryOperationResult::GroundToPet { .. } => match op.pet_pickup_item(&*item_data) {
                Some(item) => cos.bag_put(item),
                None => warn!("cos bag: op 17 item payload not understood — capture 0xB034"),
            },
            InventoryOperationResult::PetToPet { source, target, .. } => {
                cos.bag_move(*source, *target);
            }
            InventoryOperationResult::PetToInventory {
                pet_slot,
                inventory_slot,
                ..
            } => {
                if let Some(mut item) = cos.bag_take(*pet_slot) {
                    item.slot = *inventory_slot;
                    for mut inventory in inventories.iter_mut() {
                        inventory.gain_item(item.clone());
                    }
                }
            }
            InventoryOperationResult::InventoryToPet {
                inventory_slot,
                pet_slot,
                ..
            } => {
                for mut inventory in inventories.iter_mut() {
                    if let Some(mut item) = inventory.take_slot(*inventory_slot) {
                        item.slot = *pet_slot;
                        cos.bag_put(item);
                    }
                }
            }
            // The grab passed straight through the pet into the character's
            // inventory, so the pet bag does not change. On the
            // [`PET_PICKUP_SLOT_NONE`] sentinel nothing landed anywhere —
            // that is the "inventory full" case, not an item we lost.
            InventoryOperationResult::GroundToPetToInventory { slot, .. } => {
                if *slot == PET_PICKUP_SLOT_NONE {
                    debug!("cos bag: op 28 sentinel — grabbed item did not fit the inventory");
                } else if let Some(item) = op.pet_pickup_item(&*item_data) {
                    for mut inventory in inventories.iter_mut() {
                        inventory.gain_item(item.clone());
                    }
                } else {
                    warn!("cos bag: op 28 item payload not understood — capture 0xB034");
                }
            }
            _ => {}
        }
    }
}

/// The COS a bag op names, or `None` for the ops that are not COS ops.
fn cos_bag_op_target(op: &InventoryOperationResult) -> Option<u32> {
    match op {
        InventoryOperationResult::GroundToPet { cos_unique_id, .. }
        | InventoryOperationResult::PetToPet { cos_unique_id, .. }
        | InventoryOperationResult::PetToInventory { cos_unique_id, .. }
        | InventoryOperationResult::InventoryToPet { cos_unique_id, .. }
        | InventoryOperationResult::GroundToPetToInventory { cos_unique_id, .. } => {
            Some(*cos_unique_id)
        }
        _ => None,
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use packets::agent::character_data::{ItemTypeData, RentInfo};

    fn bag_item(slot: u8, ref_id: u32) -> InventoryItem {
        InventoryItem {
            slot,
            rent: RentInfo::default(),
            ref_id,
            data: ItemTypeData::Expendable {
                stack_count: 1,
                assimilation_prob: None,
                mag_params: Vec::new(),
            },
        }
    }

    /// The wolf's own re-summon, in miniature: 0x30C8 comes back carrying the
    /// running totals, and it must overwrite what the deltas left behind rather
    /// than lose to it. Before this rule the page kept showing whatever the
    /// last session had accumulated, and never showed HGP at all.
    #[test]
    fn a_growth_block_overrides_what_the_deltas_built_up() {
        let mut held = pick_pet(Vec::new());
        held.hgp = Some(5_000);
        held.exp = 12;
        held.level = Some(9);

        let growth = CosGrowth {
            exp: 77,
            level: 1,
            hgp: 9_932,
        };
        assert_eq!(
            summon_seed(Some(growth), Some(&held)),
            (Some(9_932), 77, Some(1))
        );
        // ...and with nothing held at all, the block is still the whole seed —
        // this is the case that used to leave `hgp: None` and an empty gauge.
        assert_eq!(summon_seed(Some(growth), None), (Some(9_932), 77, Some(1)));
    }

    /// A pick pet carries no growth block, so its hunger only ever reaches us
    /// through 0x30C9 arm 4 — a re-send must not wipe it back to `None`.
    #[test]
    fn a_kind_without_a_growth_block_keeps_what_the_deltas_gave_it() {
        let mut held = pick_pet(Vec::new());
        held.hgp = Some(4_200);
        held.exp = 5;

        assert_eq!(summon_seed(None, Some(&held)), (Some(4_200), 5, None));
        // A first summon of such a kind simply has nothing to show.
        assert_eq!(summon_seed(None, None), (None, 0, None));
    }

    /// leveldata column 1 for the levels the capture below walks, verbatim from
    /// the user's own `Media.pk2` (row "1 118" = 118 exp to go from 1 to 2).
    fn leveldata() -> ClientLevelData {
        use crate::assets::textdata::leveldata::LevelData;
        let mut data = LevelData::default();
        for (level, exp) in [
            (1u8, 118u64),
            (2, 470),
            (3, 1_058),
            (4, 1_880),
            (5, 2_938),
            (6, 5_640),
            (7, 9_048),
        ] {
            data.exp.insert(level, exp);
        }
        ClientLevelData::from_data(data)
    }

    /// **The capture that proves the whole rule.**
    ///
    /// `packet_dump/0x30c9.log`, 2026-08-19T22:25, uid `626e0200`: the pet is
    /// summoned by `0x30c8.log` at level 1 with 76 exp, then takes seven arm-3
    /// deltas in 44 seconds. Three of them are preceded by an arm 7 naming a new
    /// ref object, and the growth ladder has one characterdata row per level —
    /// 6106 is `COS_P_WOLF_001`, so ref 6108 is stage 3, 6109 is 4, 6110 is 5.
    ///
    /// Replaying the deltas through the loop has to land on those three stages
    /// at exactly those three kills, and it does. Nothing about this is tuned:
    /// the thresholds are the shipped table and the deltas are the wire's.
    #[test]
    fn the_captured_kill_sequence_reproduces_the_servers_own_level_ups() {
        let table = leveldata();
        // (delta, the ref the server's arm 7 announced just before it)
        let kills = [
            (639i64, Some(6108u32)),
            (610, None),
            (610, Some(6109)),
            (596, None),
            (596, None),
            (596, Some(6110)),
            (582, None),
        ];
        const LADDER_BASE: u32 = 6106; // COS_P_WOLF_001 is level 1

        let (mut level, mut exp) = (1u8, 76u64);
        let mut seen = Vec::new();
        for (delta, arm7) in kills {
            (level, exp) = apply_pet_exp_gain(level, exp, delta, &table);
            if let Some(ref_obj_id) = arm7 {
                assert_eq!(
                    u32::from(level),
                    ref_obj_id - LADDER_BASE + 1,
                    "the loop disagrees with the server's own model swap"
                );
            }
            seen.push((level, exp));
        }
        assert_eq!(
            seen,
            vec![
                (3, 127), // one kill, two levels — and the server sent ONE arm 7
                (3, 737),
                (4, 289),
                (4, 885),
                (4, 1_481),
                (5, 197),
                (5, 779),
            ]
        );
    }

    /// The two readings this capture kills, spelled out so neither comes back.
    ///
    /// Both die on the very first kill, where 76 + 639 has to clear level 1's
    /// 118 *and* level 2's 470 and still land inside level 3.
    #[test]
    fn the_refuted_readings_miss_the_first_kill() {
        let table = leveldata();
        let (level, exp) = apply_pet_exp_gain(1, 76, 639, &table);
        assert_eq!((level, exp), (3, 127));

        // Reset-to-0 per level would stop at 2: the surplus has to survive the
        // first threshold to pay for the second.
        assert_ne!(level, 2);
        // leveldata column 5 (the reference mob EXP yield: 24, 47, 71, ...) is
        // far too shallow — it would run this single kill to level 8.
        let mut col5 = crate::assets::textdata::leveldata::LevelData::default();
        for (l, e) in [
            (1u8, 24u64),
            (2, 47),
            (3, 71),
            (4, 94),
            (5, 118),
            (6, 141),
            (7, 165),
        ] {
            col5.exp.insert(l, e);
        }
        let (col5_level, _) = apply_pet_exp_gain(1, 76, 639, &ClientLevelData::from_data(col5));
        assert_eq!(col5_level, 8, "recorded so the refutation stays visible");
    }

    /// A pet loses exp when it dies but never a level — the capture's own `-1`
    /// left the pet at level 1 and the next summon reported 77 - 1 = 76. Same
    /// rule the character bar follows (`underbar/model.rs`, #306).
    #[test]
    fn a_loss_drains_exp_without_ever_de_levelling() {
        let table = leveldata();
        assert_eq!(apply_pet_exp_gain(1, 77, -1, &table), (1, 76));
        // A loss bigger than the offset floors at 0 rather than wrapping (#218)
        // — and still does not touch the level.
        assert_eq!(apply_pet_exp_gain(4, 10, -9_999, &table), (4, 0));
    }

    /// Past the last leveldata row there is no threshold left, so the loop stops
    /// instead of spinning and the surplus simply accumulates.
    #[test]
    fn a_level_off_the_end_of_the_table_stops_the_loop() {
        let table = leveldata(); // only levels 1..=7
        assert_eq!(apply_pet_exp_gain(7, 0, 1_000_000, &table), (8, 990_952));
        assert_eq!(apply_pet_exp_gain(8, 0, 1_000_000, &table), (8, 1_000_000));
    }

    fn pick_pet(items: Vec<InventoryItem>) -> Cos {
        Cos {
            unique_id: 0x539,
            ref_obj_id: 2,
            kind: CosKind::GrabPet,
            body: CosBody {
                hp: 0,
                unk_b: 0,
                growth: None,
                unk_f: None,
                name: None,
                inventory_size: 8,
                items,
                unk_g: None,
                unk_h: None,
            },
            hgp: None,
            exp: 0,
            level: None,
        }
    }

    /// Op 25 is the one pet op that stays inside the bag, and it is a
    /// whole-slot relocate-or-swap — the same rule `Inventory::apply_move`
    /// follows, so the client cannot drift from the authoritative server.
    #[test]
    fn cos_bag_move_relocates_and_swaps_whole_slots() {
        let mut cos = pick_pet(vec![bag_item(0, 11), bag_item(3, 22)]);

        // relocate into an empty slot
        cos.bag_move(0, 5);
        assert!(cos.bag_get(0).is_none());
        assert_eq!(cos.bag_get(5).map(|i| i.ref_id), Some(11));

        // swap with an occupied one
        cos.bag_move(5, 3);
        assert_eq!(cos.bag_get(3).map(|i| i.ref_id), Some(11));
        assert_eq!(cos.bag_get(5).map(|i| i.ref_id), Some(22));
        assert_eq!(cos.body.items.len(), 2);

        // a move from an empty slot changes nothing
        cos.bag_move(7, 1);
        assert_eq!(cos.body.items.len(), 2);
        assert!(cos.bag_get(1).is_none());
    }

    /// Ops 26/27 carry no item record, only two slot numbers — so the bag
    /// hands the record over on the way out and takes one on the way in, and
    /// the record's own `slot` follows the container it lands in.
    #[test]
    fn cos_bag_take_and_put_carry_the_record_across_containers() {
        let mut cos = pick_pet(vec![bag_item(2, 33)]);

        // op 26: out of the pet bag, into inventory slot 13
        let mut moved = cos.bag_take(2).expect("slot 2 holds an item");
        assert!(cos.bag_get(2).is_none());
        moved.slot = 13;
        assert_eq!(moved.ref_id, 33);

        // op 27: back in at pet slot 4, replacing nothing
        moved.slot = 4;
        cos.bag_put(moved);
        assert_eq!(cos.bag_get(4).map(|i| i.ref_id), Some(33));
        assert_eq!(cos.body.items.len(), 1);

        // a put over an occupied slot replaces it rather than duplicating it
        cos.bag_put(bag_item(4, 44));
        assert_eq!(cos.bag_get(4).map(|i| i.ref_id), Some(44));
        assert_eq!(cos.body.items.len(), 1);
    }

    /// Only the five COS ops name a COS; everything else on 0xB034 must fall
    /// through this system untouched (the storage/store ops have their own
    /// consumers, and applying them here would double-apply them).
    #[test]
    fn only_the_five_cos_ops_target_a_cos_bag() {
        use bytes::Bytes;
        assert_eq!(
            cos_bag_op_target(&InventoryOperationResult::PetToPet {
                cos_unique_id: 0x539,
                source: 1,
                target: 2,
                amount: 1,
            }),
            Some(0x539)
        );
        assert_eq!(
            cos_bag_op_target(&InventoryOperationResult::GroundToPetToInventory {
                cos_unique_id: 7,
                slot: PET_PICKUP_SLOT_NONE,
                tail: Bytes::new(),
            }),
            Some(7)
        );
        assert_eq!(
            cos_bag_op_target(&InventoryOperationResult::InventoryToStorage {
                source: 1,
                target: 2
            }),
            None
        );
        assert_eq!(
            cos_bag_op_target(&InventoryOperationResult::Unknown {
                op: 42,
                tail: Bytes::new()
            }),
            None
        );
    }

    /// The wire's "unnamed" is a zero-length string, not an absent field, so
    /// the `Option` this returns has to mean *named* rather than *present*.
    /// Every window read it as the former while it meant the latter, which is
    /// why an unnamed pet rendered a blank instead of "No name".
    #[test]
    fn an_empty_wire_name_reads_as_unnamed() {
        let mut cos = pick_pet(Vec::new());

        cos.body.name = Some(String::new()); // the Grey Wolf capture's `0000`
        assert_eq!(cos.name(), None);
        cos.body.name = Some("   ".into());
        assert_eq!(cos.name(), None);
        cos.body.name = None; // a kind that carries no name field at all
        assert_eq!(cos.name(), None);

        cos.body.name = Some("Rex".into());
        assert_eq!(cos.name(), Some("Rex"));
    }

    /// The server's own 30% threshold, expressed against our fraction.
    #[test]
    fn hgp_is_a_per_ten_thousand_value() {
        let cos = |hgp| Cos {
            unique_id: 1,
            ref_obj_id: 2,
            kind: CosKind::GrowthPet,
            body: CosBody {
                hp: 0,
                unk_b: 0,
                growth: None,
                unk_f: None,
                name: None,
                inventory_size: 0,
                items: Vec::new(),
                unk_g: None,
                unk_h: None,
            },
            hgp,
            exp: 0,
            level: None,
        };
        assert_eq!(cos(None).hgp_fraction(), None);
        assert_eq!(cos(Some(HGP_FULL)).hgp_fraction(), Some(1.0));
        assert_eq!(cos(Some(3_000)).hgp_fraction(), Some(0.3));
        // out-of-range values clamp rather than overdrawing the gauge
        assert_eq!(cos(Some(u16::MAX)).hgp_fraction(), Some(1.0));
    }
}
