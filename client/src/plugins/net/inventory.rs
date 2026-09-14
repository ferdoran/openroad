//! The player's inventory as a slot-indexed component.
//!
//! Idea: the inventory window (and later trade/storage UIs) should not dig
//! through the raw `CharacterInfo` record — when CHARACTER_DATA (0x3013)
//! resolves, the game scene distills its item list into this component:
//! one `Option<InventoryItem>` per wire slot (0..12 equipment, 13+ bag), so
//! empty slots are first-class and lookups are O(1). Server-confirmed moves
//! (0xB034) and gold updates (0x304E) mutate it in place; change detection on
//! the component drives the UI refresh.

use bevy::prelude::*;

use packets::agent::character_data::{InventoryItem, ItemTypeData, ParsedCharacterInfo};

/// Wire slots 0..12 are the equipment slots; the bag starts at 13.
pub const EQUIP_SLOT_COUNT: u8 = 13;
/// Wire slot holding the equipped weapon. Capture-verified against real
/// `packet_dump/0x3052.log` bodies (see the durability test below, where slot
/// 6 is the weapon wearing down over the capture).
pub const WEAPON_SLOT: u8 = 6;
/// Wire slot holding the equipped ammunition (and the shield — vanilla shares
/// the secondary hole between them, which is why `equip_slots` sends both here).
pub const AMMO_SLOT: u8 = 7;
pub const BAG_FIRST_SLOT: u8 = EQUIP_SLOT_COUNT;
/// The inventory window shows the bag as 4x8 pages.
pub const SLOTS_PER_PAGE: u8 = 32;

/// The vanilla base inventory size, used when 0x3013's size byte is missing.
const DEFAULT_SIZE: u8 = 45;

/// The highest slot a 0x3052 durability push may name. The original's handler
/// bails out on `0x6f < slot` (`sro_client.exe@008adf50:62`) before touching
/// either of its item arrays, so a push above it is silently dropped.
pub const MAX_DURABILITY_SLOT: u8 = 0x6F;

/// The avatar inventory's slot count (go-sro hardcodes 5 on the wire).
pub const AVATAR_SLOT_COUNT: u8 = 5;

#[derive(Component, Clone, Debug, Default)]
pub struct Inventory {
    /// One entry per wire slot; `None` is an empty slot.
    pub slots: Vec<Option<InventoryItem>>,
    /// The separate avatar inventory (0x3013's second item section),
    /// slot-indexed like `slots`. Display-only: go-sro implements no avatar
    /// equip/unequip operations.
    pub avatar_slots: Vec<Option<InventoryItem>>,
    pub gold: u64,
}

impl Inventory {
    /// Distill the parsed CHARACTER_DATA record: allocate `inventory_size`
    /// slots (growing past it if the server sent an out-of-range slot) and
    /// place each item at its slot.
    pub fn from_character(parsed: &ParsedCharacterInfo) -> Self {
        let size = parsed
            .inventory_size
            .unwrap_or(DEFAULT_SIZE)
            .max(EQUIP_SLOT_COUNT);
        let mut slots = vec![None; size as usize];
        for item in parsed.inventory.iter().flatten() {
            let index = item.slot as usize;
            if index >= slots.len() {
                slots.resize(index + 1, None);
            }
            slots[index] = Some(item.clone());
        }
        // One line per bag item at join. Item-use (#215/#454) is addressed by
        // *wire slot* + packed type_id, and every live experiment so far had to
        // guess both; this makes the two numbers the request needs readable
        // straight from the log instead of re-deriving them from a hex dump.
        for item in parsed.inventory.iter().flatten() {
            debug!(
                "inventory: slot {} = ref {} ({:?})",
                item.slot, item.ref_id, item.data
            );
        }
        let mut avatar_slots = vec![None; AVATAR_SLOT_COUNT as usize];
        for item in parsed.avatar_items.iter().flatten() {
            let index = item.slot as usize;
            if index >= avatar_slots.len() {
                avatar_slots.resize(index + 1, None);
            }
            avatar_slots[index] = Some(item.clone());
        }
        Self {
            slots,
            avatar_slots,
            gold: parsed.stats.map(|stats| stats.gold).unwrap_or_default(),
        }
    }

    pub fn get_avatar(&self, slot: u8) -> Option<&InventoryItem> {
        self.avatar_slots.get(slot as usize)?.as_ref()
    }

    pub fn size(&self) -> u8 {
        self.slots.len().min(u8::MAX as usize) as u8
    }

    pub fn get(&self, slot: u8) -> Option<&InventoryItem> {
        self.slots.get(slot as usize)?.as_ref()
    }

    /// Number of bag pages the inventory window can flip through.
    pub fn page_count(&self) -> u8 {
        let bag_slots = (self.size().saturating_sub(BAG_FIRST_SLOT)) as u16;
        (bag_slots.div_ceil(SLOTS_PER_PAGE as u16) as u8).max(1)
    }

    /// Apply a server-confirmed pickup (0xB034 op 6): place the fully-parsed
    /// item at its slot. The server sends the authoritative post-pickup slot
    /// state (verified: repeated expendable pickups carry the growing slot
    /// total, not a delta), so this replaces rather than merges.
    pub fn gain_item(&mut self, item: InventoryItem) {
        // The slot comes straight off the wire, and one caller supplies it from
        // a pet-bag op (`hud/cos/state.rs`) whose slot field is the least
        // capture-verified of the family. A gain into the equipment band is
        // therefore far more likely to be a decode error than a real event —
        // apply it (the server is authoritative) but never silently.
        if item.slot < BAG_FIRST_SLOT {
            warn!(
                "inventory: gain of ref {} names EQUIPMENT slot {} — capture the 0xB034 \
                 for decode; the bag starts at {BAG_FIRST_SLOT}",
                item.ref_id, item.slot
            );
        }
        let index = item.slot as usize;
        if index >= self.slots.len() {
            self.slots.resize(index + 1, None);
        }
        self.slots[index] = Some(item);
    }

    /// Apply a server-confirmed move: relocate, **merge**, or swap.
    ///
    /// This used to mirror go-sro exactly — always relocate or swap whole
    /// slots, ignoring the echoed `amount`, because go-sro's `MoveItems` has no
    /// stack merging. **That premise no longer holds**: the server in use is
    /// not go-sro (it accepts the op-7 drop go-sro ships no handler for, and it
    /// performs two-handed auto-unequips with chained ack records), and
    /// dropping a stack onto an identical one visibly did nothing because the
    /// two simply traded places.
    ///
    /// So a move onto a slot holding the **same `ref_id`** now merges, capped
    /// at the item's `max_stack`, with any overflow left behind in the source.
    ///
    /// `[S]`, and deliberately loud about it: a merge and a swap produce a
    /// byte-identical ack, and no `0x3040` quantity push ever follows a move on
    /// this server, so the wire cannot confirm the outcome either way. The
    /// merge is logged with its resulting counts so a disagreement shows up in
    /// the log rather than as a silently wrong bag. Any later authoritative
    /// packet — a 0x3040 quantity update, a fresh 0x3013 snapshot — overwrites
    /// this, which is the intended correction path.
    pub fn apply_move(
        &mut self,
        source: u8,
        target: u8,
        amount: u16,
        item_data: &crate::plugins::textdata::ClientItemData,
    ) {
        if self.try_merge(source, target, amount, item_data) {
            return;
        }
        // An UNEQUIP never swaps. The server accepts an equip→bag move only
        // into an EMPTY bag slot (go-sro `MoveItems`, the same rule the UI's
        // `bag_panel_drop` picks its target by), so a `Move` ack whose source
        // is an equipment slot can only ever mean "relocate". Swapping there
        // pushed the bag slot's occupant back into the equipment band, which
        // is how a potion ended up drawn in the weapon hole after unequipping
        // a sword: `refresh_inventory` reads nothing but this model.
        let unequip = source < BAG_FIRST_SLOT;
        let (source, target) = (source as usize, target as usize);
        if source == target || source >= self.slots.len() {
            return;
        }
        if target >= self.slots.len() {
            self.slots.resize(target + 1, None);
        }
        let Some(mut moved) = self.slots[source].take() else {
            return;
        };
        moved.slot = target as u8;
        if let Some(mut swapped) = self.slots[target].take() {
            if unequip {
                // Our model already disagrees with the server's: it acked a
                // move into a slot we believe is occupied. Keep the occupant
                // where the server put it rather than inventing an equip.
                warn!(
                    "inventory: unequip {source} -> {target} landed on an occupied slot \
                     (ref {}) — local state had drifted; not swapping into the equipment band",
                    swapped.ref_id
                );
            } else {
                swapped.slot = source as u8;
                self.slots[source] = Some(swapped);
            }
        }
        self.slots[target] = Some(moved);
    }

    /// Combine two stacks of the same item, if that is what this move is.
    ///
    /// Returns `true` when it handled the move, so [`apply_move`] falls through
    /// to its relocate/swap for everything else. A move is a merge only when
    /// both slots hold the same `ref_id` **and** the item is stackable
    /// (`max_stack > 1`) — an equipment swap between two identical swords is
    /// not a merge, and neither is anything involving the equipment band.
    ///
    /// [`apply_move`]: Self::apply_move
    fn try_merge(
        &mut self,
        source: u8,
        target: u8,
        amount: u16,
        item_data: &crate::plugins::textdata::ClientItemData,
    ) -> bool {
        if source == target || source < BAG_FIRST_SLOT || target < BAG_FIRST_SLOT {
            return false;
        }
        let (Some(from), Some(into)) = (self.get(source), self.get(target)) else {
            return false;
        };
        if from.ref_id != into.ref_id {
            return false;
        }
        let Some(max_stack) = item_data
            .get(&(from.ref_id as i32))
            .and_then(|row| row.max_stack())
            .filter(|max| *max > 1)
        else {
            return false;
        };
        let (Some(from_count), Some(into_count)) = (stack_count(from), stack_count(into)) else {
            return false;
        };

        // The server echoes the whole source stack as `amount`; clamp to what
        // we believe is actually there so a stale model cannot invent items.
        let offered = amount.min(from_count);
        let space = (max_stack as u16).saturating_sub(into_count);
        let moved = offered.min(space);
        if moved == 0 {
            return false;
        }
        let left = from_count - moved;
        info!(
            "inventory: merged {moved} of ref {} from slot {source} into {target} \
             ({into_count} -> {}, {left} left behind) [S]",
            from.ref_id,
            into_count + moved,
        );
        set_stack(self.slots[target as usize].as_mut(), into_count + moved);
        if left == 0 {
            self.slots[source as usize] = None;
        } else {
            set_stack(self.slots[source as usize].as_mut(), left);
        }
        true
    }

    /// Remove and return the whole slot — the counterpart of
    /// [`Self::gain_item`] for a server-confirmed move *out* of the bag whose
    /// ack carries only slot numbers (the pick-pet ops 26/27,
    /// `docs/re/systems/pet-pick-cos.md` §3): the receiving container has to
    /// be handed the record, because the packet does not repeat it.
    pub fn take_slot(&mut self, slot: u8) -> Option<InventoryItem> {
        self.slots.get_mut(slot as usize)?.take()
    }

    /// Apply a server-confirmed removal (store sell): shrink the slot's stack
    /// by `amount`, clearing the slot when it empties (non-stackables always
    /// clear).
    pub fn remove_amount(&mut self, slot: u8, amount: u16) {
        let Some(entry) = self.slots.get_mut(slot as usize) else {
            return;
        };
        let Some(item) = entry else {
            return;
        };
        if let packets::agent::character_data::ItemTypeData::Expendable { stack_count, .. } =
            &mut item.data
        {
            if *stack_count > amount {
                *stack_count -= amount;
                return;
            }
        }
        *entry = None;
    }

    /// Apply 0x3052: the server's authoritative durability for one slot. Only
    /// equipment carries durability; a push naming any other slot is ignored
    /// rather than guessed at.
    ///
    /// The wire slot is a single flat space, which our `slots` already is: the
    /// original's handler (`sro_client.exe@008adf50:29,62-63`) splits it into
    /// its own two arrays — `slot < 0x0D` indexes the equipment array, `0x0D..
    /// 0x6F` the inventory array as `slot - 0x0D` — and **ignores anything
    /// above `0x6F`** (`:62`, the `0x6f < slot` bail). The bias is that split,
    /// not a wire field, so only the upper bound is a real rule to keep.
    pub fn set_durability(&mut self, slot: u8, durability: u32) {
        if slot > MAX_DURABILITY_SLOT {
            return;
        }
        let Some(Some(item)) = self.slots.get_mut(slot as usize) else {
            return;
        };
        if let packets::agent::character_data::ItemTypeData::Equipment(equipment) = &mut item.data {
            equipment.durability = durability;
        }
    }

    /// Apply 0x3040 update type 8: the slot's new stack total (absolute, not a
    /// delta). Quantity 0 means the stack was consumed and the slot is now
    /// empty.
    pub fn set_stack_count(&mut self, slot: u8, quantity: u16) {
        let Some(entry) = self.slots.get_mut(slot as usize) else {
            return;
        };
        if quantity == 0 {
            *entry = None;
            return;
        }
        let Some(item) = entry else {
            return;
        };
        if let packets::agent::character_data::ItemTypeData::Expendable { stack_count, .. } =
            &mut item.data
        {
            *stack_count = quantity;
        }
    }

    /// Apply 0x3040 update type 0x40: the COS container's new summon state
    /// (`SRCoS.State`: 1 never summoned, 2 summoned, 3 unsummoned, 4 dead).
    pub fn set_cos_state(&mut self, slot: u8, new_state: u8) {
        let Some(Some(item)) = self.slots.get_mut(slot as usize) else {
            return;
        };
        if let packets::agent::character_data::ItemTypeData::CosPet { state, .. } = &mut item.data {
            *state = new_state;
        }
    }

    /// Apply 0x3092: the bag's new slot count. Grow only — SRO has no
    /// bag-shrink operation, and honouring one would silently drop the items
    /// the server still believes we hold.
    pub fn set_capacity(&mut self, capacity: u8) {
        let size = (capacity as usize).max(EQUIP_SLOT_COUNT as usize);
        if size > self.slots.len() {
            self.slots.resize(size, None);
        }
    }

    /// Wire slots (equipment + bag) holding **repairable** damaged equipment —
    /// durability below the item's rolled max, on an item that can be repaired
    /// at all. Drives the repair buttons + repair-all application.
    pub fn damaged_slots(
        &self,
        item_data: &crate::plugins::textdata::ClientItemData,
        magic_options: &crate::plugins::textdata::ClientMagicOptions,
    ) -> Vec<u8> {
        self.slots
            .iter()
            .flatten()
            .filter(|item| {
                let packets::agent::character_data::ItemTypeData::Equipment(eq) = &item.data else {
                    return false;
                };
                if !is_repairable(eq, magic_options) {
                    return false;
                }
                item_data
                    .get(&(item.ref_id as i32))
                    .and_then(|row| max_durability(row, eq))
                    .is_some_and(|max| eq.durability < max)
            })
            .map(|item| item.slot)
            .collect()
    }

    /// Restore a slot's equipment durability to its rolled max (a confirmed
    /// 0xB03E repair).
    pub fn repair_slot(&mut self, slot: u8, item_data: &crate::plugins::textdata::ClientItemData) {
        let Some(Some(item)) = self.slots.get_mut(slot as usize) else {
            return;
        };
        let ref_id = item.ref_id as i32;
        let packets::agent::character_data::ItemTypeData::Equipment(eq) = &mut item.data else {
            return;
        };
        if let Some(max) = item_data
            .get(&ref_id)
            .and_then(|row| max_durability(row, eq))
        {
            eq.durability = max;
        }
    }
}

/// The stack size of an item, or `None` for a class that has no stack.
///
/// Split out because "is this stackable" and "how many are there" are the same
/// question asked of two `ItemTypeData` variants, and the merge path has to ask
/// it of both slots before touching either.
fn stack_count(item: &InventoryItem) -> Option<u16> {
    match &item.data {
        ItemTypeData::Expendable { stack_count, .. } => Some(*stack_count),
        _ => None,
    }
}

/// Write a new stack size, ignoring an item that has no stack.
fn set_stack(item: Option<&mut InventoryItem>, count: u16) {
    if let Some(ItemTypeData::Expendable { stack_count, .. }) = item.map(|item| &mut item.data) {
        *stack_count = count;
    }
}

/// Whether an equipment instance can be repaired at all.
///
/// "Cannot be repaired" is not an itemdata column — it is the blue option
/// `MATTR_NOT_REPARABLE`, carried per instance in `mag_params`
/// (`assets/textdata/magicoption.rs`). Before this, the repair flow read only
/// durability, so a not-repairable item was offered for repair, priced, and
/// paid for; the server was the only thing that knew better.
///
/// An option id the archive does not define cannot be classified, so it is
/// treated as harmless — refusing to repair an item because of an unknown blue
/// would be worse than the bug this fixes.
pub fn is_repairable(
    eq: &packets::agent::character_data::EquipmentData,
    magic_options: &crate::plugins::textdata::ClientMagicOptions,
) -> bool {
    !eq.mag_params
        .iter()
        .filter_map(|param| magic_options.get(param.kind))
        .any(|info| info.is_penalty())
}

/// The rolled maximum durability of an equipment item: itemdata's durability
/// bound pair (cols 63/64) interpolated by the item's 5-bit variance slot 0
/// (durability is slot 0 for weapons AND armor/shields; accessories carry no
/// durability) — the same white-stat math the inventory tooltip renders.
pub fn max_durability(
    row: &crate::assets::textdata::itemdata::ItemDataRow,
    eq: &packets::agent::character_data::EquipmentData,
) -> Option<u32> {
    use crate::assets::textdata::itemdata::StatRange;
    let (_, _, tid3, _) = row.type_ids()?;
    if tid3 == 5 {
        return None; // accessories
    }
    let (lower, upper) = row.stat_range(StatRange::Durability)?;
    let factor = (eq.variance & 0x1F) as f32 / 31.0;
    Some((lower + (upper - lower) * factor).round() as u32)
}

#[cfg(test)]
mod test {
    use super::*;
    use packets::agent::character_data::{ItemTypeData, RentInfo};

    fn item(slot: u8, ref_id: u32, stack: Option<u16>) -> InventoryItem {
        InventoryItem {
            slot,
            rent: RentInfo::default(),
            ref_id,
            data: match stack {
                Some(stack_count) => ItemTypeData::Expendable {
                    stack_count,
                    assimilation_prob: None,
                    mag_params: vec![],
                },
                None => ItemTypeData::TransformScroll { mask_ref_id: 0 },
            },
        }
    }

    fn inventory(size: u8, items: Vec<InventoryItem>) -> Inventory {
        let mut slots = vec![None; size as usize];
        for i in items {
            let slot = i.slot as usize;
            slots[slot] = Some(i);
        }
        Inventory {
            slots,
            avatar_slots: vec![],
            gold: 0,
        }
    }

    fn stack_at(inv: &Inventory, slot: u8) -> Option<u16> {
        match inv.get(slot)?.data {
            ItemTypeData::Expendable { stack_count, .. } => Some(stack_count),
            _ => None,
        }
    }

    /// Itemdata that knows nothing. Merging needs `max_stack`, so with this the
    /// merge path always declines and `apply_move` keeps its relocate/swap —
    /// which is exactly what the non-merge tests below want to exercise, and
    /// also the production fail-safe for an item whose row is missing.
    fn no_item_data() -> crate::plugins::textdata::ClientItemData {
        crate::plugins::textdata::ClientItemData::default()
    }

    /// Itemdata declaring one stackable ref, so the merge path can engage.
    fn stackable(ref_id: i32, max_stack: u32) -> crate::plugins::textdata::ClientItemData {
        use crate::assets::textdata::itemdata::{ItemData, ItemDataRow};
        let mut fields = vec![String::new(); 60];
        fields[57] = max_stack.to_string(); // ItemdataFields::MaxStack
        crate::plugins::textdata::ClientItemData::from_data(ItemData(
            std::collections::HashMap::from([(ref_id, ItemDataRow(fields))]),
        ))
    }

    #[test]
    fn page_count_from_size() {
        assert_eq!(Inventory::default().page_count(), 1);
        assert_eq!(inventory(45, vec![]).page_count(), 1); // 32 bag slots
        assert_eq!(inventory(77, vec![]).page_count(), 2);
        assert_eq!(inventory(109, vec![]).page_count(), 3);
    }

    #[test]
    fn apply_move_relocates_and_swaps() {
        let mut inv = inventory(45, vec![item(13, 100, None), item(14, 200, None)]);
        inv.apply_move(13, 20, 0, &no_item_data());
        assert_eq!(inv.get(13), None);
        assert_eq!(inv.get(20).unwrap().ref_id, 100);
        assert_eq!(inv.get(20).unwrap().slot, 20);
        inv.apply_move(20, 14, 0, &no_item_data()); // different item -> swap
        assert_eq!(inv.get(14).unwrap().ref_id, 100);
        assert_eq!(inv.get(20).unwrap().ref_id, 200);
    }

    /// The unequip direction never swaps. go-sro accepts an equip→bag move
    /// only into an empty slot, so an ack naming an occupied one means our
    /// model drifted — and swapping there is what drew a bag item's icon in
    /// the weapon hole after unequipping a sword.
    #[test]
    fn unequipping_onto_an_occupied_bag_slot_leaves_the_equipment_slot_empty() {
        let mut inv = inventory(45, vec![item(6, 100, None), item(20, 200, None)]);

        inv.apply_move(6, 20, 1, &no_item_data());

        assert_eq!(inv.get(6), None, "the weapon slot must not gain an item");
        assert_eq!(inv.get(20).unwrap().ref_id, 100);
        assert_eq!(inv.get(20).unwrap().slot, 20);
    }

    /// ...but the equip direction still does: putting a second weapon on
    /// sends the worn one back to the bag slot it came from.
    #[test]
    fn equipping_over_a_worn_item_still_swaps_it_back_into_the_bag() {
        let mut inv = inventory(45, vec![item(6, 100, None), item(20, 200, None)]);

        inv.apply_move(20, 6, 1, &no_item_data());

        assert_eq!(inv.get(6).unwrap().ref_id, 200);
        assert_eq!(inv.get(6).unwrap().slot, 6);
        assert_eq!(inv.get(20).unwrap().ref_id, 100);
        assert_eq!(inv.get(20).unwrap().slot, 20);
    }

    /// Dropping a stack onto an identical one combines them.
    ///
    /// This test used to be `apply_move_swaps_stacks_like_go_sro` and asserted
    /// the exact opposite — that same-item stacks merely trade places, because
    /// go-sro's `swapItems` never merges. The server in use is not go-sro, and
    /// to the player a swap is indistinguishable from the drag doing nothing.
    #[test]
    fn same_item_stacks_merge_instead_of_swapping() {
        let data = stackable(5, 100);
        let mut inv = inventory(45, vec![item(13, 5, Some(50)), item(15, 5, Some(10))]);

        inv.apply_move(13, 15, 50, &data);

        assert_eq!(stack_at(&inv, 15), Some(60), "the target absorbs the stack");
        assert_eq!(inv.get(13), None, "an emptied source slot is cleared");
    }

    /// A merge stops at `max_stack`; the remainder stays where it was rather
    /// than being silently destroyed or overflowing the target.
    #[test]
    fn a_merge_past_the_stack_cap_leaves_the_rest_behind() {
        let data = stackable(5, 100);
        let mut inv = inventory(45, vec![item(13, 5, Some(50)), item(15, 5, Some(80))]);

        inv.apply_move(13, 15, 50, &data);

        assert_eq!(stack_at(&inv, 15), Some(100), "capped at max_stack");
        assert_eq!(stack_at(&inv, 13), Some(30), "the overflow stays put");
        assert_eq!(inv.get(13).unwrap().slot, 13);
    }

    /// Only *identical* items merge. Two different stackables still swap, which
    /// is what a player rearranging their bag expects.
    #[test]
    fn different_items_still_swap() {
        let data = stackable(5, 100);
        let mut inv = inventory(45, vec![item(13, 5, Some(50)), item(15, 9, Some(10))]);

        inv.apply_move(13, 15, 50, &data);

        assert_eq!(inv.get(15).unwrap().ref_id, 5);
        assert_eq!(inv.get(13).unwrap().ref_id, 9);
    }

    /// An unstackable item never merges, however many the ack claims — two
    /// identical swords are two swords, not a stack of two.
    #[test]
    fn identical_unstackables_swap_rather_than_merge() {
        let data = stackable(5, 1); // max_stack 1
        let mut inv = inventory(45, vec![item(13, 5, Some(1)), item(15, 5, Some(1))]);

        inv.apply_move(13, 15, 1, &data);

        assert!(inv.get(13).is_some(), "both slots must still hold an item");
        assert_eq!(stack_at(&inv, 15), Some(1));
    }

    /// The equipment band is never a merge target: an equip or unequip that
    /// happens to involve identical items has to keep its relocate/swap
    /// behaviour, or a weapon would be absorbed into a bag stack.
    #[test]
    fn the_equipment_band_never_merges() {
        let data = stackable(5, 100);
        let mut inv = inventory(45, vec![item(6, 5, Some(1)), item(20, 5, Some(1))]);

        inv.apply_move(6, 20, 1, &data);

        assert_eq!(inv.get(6), None, "unequip relocates, as before");
        assert_eq!(stack_at(&inv, 20), Some(1), "no absorption happened");
    }

    /// A missing itemdata row means `max_stack` is unknown, and an unknown cap
    /// must not be guessed — the move falls back to the old relocate/swap.
    #[test]
    fn an_unknown_item_falls_back_to_swapping() {
        let mut inv = inventory(45, vec![item(13, 5, Some(50)), item(15, 5, Some(10))]);

        inv.apply_move(13, 15, 50, &no_item_data());

        assert_eq!(stack_at(&inv, 13), Some(10));
        assert_eq!(stack_at(&inv, 15), Some(50));
    }

    fn equipment(slot: u8, durability: u32) -> InventoryItem {
        InventoryItem {
            slot,
            rent: RentInfo::default(),
            ref_id: 1,
            data: ItemTypeData::Equipment(packets::agent::character_data::EquipmentData {
                opt_level: 0,
                variance: 0,
                durability,
                mag_params: vec![],
                socket_tag: 0,
                sockets: vec![],
                elixir_tag: 0,
                adv_elixirs: vec![],
            }),
        }
    }

    fn durability_at(inv: &Inventory, slot: u8) -> Option<u32> {
        match &inv.get(slot)?.data {
            ItemTypeData::Equipment(equipment) => Some(equipment.durability),
            _ => None,
        }
    }

    /// 0x3052, real `packet_dump/0x3052.log` bodies: slot 6 is the weapon and
    /// slot 1 the chest, both wearing down over the capture.
    #[test]
    fn durability_push_updates_the_equipment_slot() {
        let mut inv = inventory(45, vec![equipment(6, 68), equipment(1, 47)]);

        inv.set_durability(6, 67);
        inv.set_durability(1, 46);

        assert_eq!(durability_at(&inv, 6), Some(67));
        assert_eq!(durability_at(&inv, 1), Some(46));
    }

    /// The original's handler bails on `0x6f < slot` before it touches an item
    /// array (`sro_client.exe@008adf50:62`), so a push above that band is
    /// dropped rather than folded into the flat slot space.
    #[test]
    fn durability_push_above_the_original_slot_band_is_ignored() {
        let mut inv = inventory(0x80, vec![equipment(6, 49), equipment(0x70, 49)]);

        inv.set_durability(0x70, 99);
        inv.set_durability(6, 99);

        assert_eq!(durability_at(&inv, 0x70), Some(49));
        assert_eq!(durability_at(&inv, 6), Some(99));
    }

    /// A durability push naming an empty or non-equipment slot is ignored —
    /// the original casts hard there, we must not panic or corrupt the slot.
    #[test]
    fn durability_push_ignores_a_slot_without_equipment() {
        let mut inv = inventory(45, vec![item(13, 10, Some(5))]);

        inv.set_durability(13, 99);
        inv.set_durability(44, 99);

        assert_eq!(stack_at(&inv, 13), Some(5));
        assert!(inv.get(44).is_none());
    }

    /// 0x3040 type 8 carries the absolute new stack total; 0 means the stack
    /// was consumed and the slot is now empty.
    #[test]
    fn quantity_push_sets_the_stack_and_zero_clears_it() {
        let mut inv = inventory(45, vec![item(13, 10, Some(5)), item(14, 10, Some(3))]);

        inv.set_stack_count(13, 9);
        inv.set_stack_count(14, 0);

        assert_eq!(stack_at(&inv, 13), Some(9));
        assert!(inv.get(14).is_none());
    }

    /// 0x3092 grows the bag. It must never shrink it: SRO has no bag-shrink,
    /// and honouring one would drop items the server still believes we hold.
    #[test]
    fn capacity_push_grows_the_bag_but_never_shrinks_it() {
        let mut inv = inventory(45, vec![item(44, 10, Some(1))]);

        inv.set_capacity(77);
        assert_eq!(inv.size(), 77);

        inv.set_capacity(45);
        assert_eq!(inv.size(), 77);
        assert_eq!(stack_at(&inv, 44), Some(1));
    }
}
