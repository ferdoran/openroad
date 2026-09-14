//! Alchemy box state: the Attribute Grant page's five item slots.
//!
//! Idea: the vanilla alchemy box does not *move* items. A page slot holds a
//! reference to an inventory slot (`CommandID` 0 = the equipment, 1..4 = the
//! stones — `resinfo/ifalchemyenchant.txt`), and the inventory only changes
//! when the server acks a fuse. So placement is pure client state and needs no
//! wire support; only the fuse action does, and that opcode map
//! (`docs/re/systems/alchemy.md`) is still `[S]`-inferred rather than
//! captured, so nothing is sent from here yet.

use bevy::prelude::*;

use crate::plugins::hud::chat::model::ChatState;
use crate::plugins::settings::keymap::KEY_ALCHEMY;
use crate::plugins::settings::options::GameOptions;

/// `GDR_AB_ENCHANT_SLOT_01..04` — the four stone slots next to the equipment
/// slot (`ifalchemyenchant.txt`, `CommandID` 1..4).
pub const STONE_SLOTS: usize = 4;
/// Page-slot index of `GDR_AB_ENCHANT_SLOT_EQUIP` (`CommandID` 0).
pub const EQUIP_SLOT: usize = 0;

/// Open/closed state of the alchemy box plus what sits in its slots.
#[derive(Resource, Default)]
pub struct AlchemyState {
    pub open: bool,
    /// Inventory wire slots, indexed like the vanilla `CommandID`s:
    /// 0 = equipment, 1..=4 = the stone slots.
    slots: [Option<u8>; STONE_SLOTS + 1],
}

impl AlchemyState {
    /// The inventory wire slot shown in page slot `index` (0 = equipment).
    pub fn slot(&self, index: usize) -> Option<u8> {
        self.slots.get(index).copied().flatten()
    }

    /// Put an inventory item into page slot `index`. An item can only sit in
    /// one slot, so placing it again moves it rather than duplicating it —
    /// the slots are references, and two references to one item would let the
    /// player "fuse" a stone with itself.
    pub fn place(&mut self, index: usize, inventory_slot: u8) {
        if index >= self.slots.len() {
            return;
        }
        for slot in self.slots.iter_mut() {
            if *slot == Some(inventory_slot) {
                *slot = None;
            }
        }
        self.slots[index] = Some(inventory_slot);
    }

    /// Empty page slot `index`, returning what was in it.
    pub fn take(&mut self, index: usize) -> Option<u8> {
        self.slots.get_mut(index).and_then(Option::take)
    }

    /// Drop every placement (closing the window releases the references).
    pub fn clear(&mut self) {
        self.slots = [None; STONE_SLOTS + 1];
    }
}

/// Toggle with the `KeyAlchemy` shortcut (unless the chat input is capturing
/// keys). Its vanilla default **is** known: `textuisystem.txt` L2273
/// `UIIT_STT_TOGGLE_ENCHANT` reads `Alchemy ( Y )`, so `keymap.rs` binds `Y`
/// (#657 — this comment previously claimed vanilla ships no default, which is
/// what kept the action unbound). A stored `SROptionSet.dat` binding and the
/// options window's Key Map tab both still override it.
pub fn toggle_alchemy_window(
    keys: Res<ButtonInput<KeyCode>>,
    chat: Res<ChatState>,
    options: Res<GameOptions>,
    mut state: ResMut<AlchemyState>,
) {
    let Some(key) = options.key_for(KEY_ALCHEMY) else {
        return;
    };
    if keys.just_pressed(key) && !chat.input_open {
        state.open = !state.open;
        if !state.open {
            state.clear();
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn alchemy_slots_hold_inventory_references() {
        let mut state = AlchemyState::default();
        state.place(EQUIP_SLOT, 13);
        state.place(1, 20);
        assert_eq!(state.slot(EQUIP_SLOT), Some(13));
        assert_eq!(state.slot(1), Some(20));
        assert_eq!(state.take(1), Some(20));
        assert_eq!(state.slot(1), None);
        assert_eq!(state.take(1), None);
    }

    /// One inventory item cannot occupy two page slots at once.
    #[test]
    fn alchemy_placing_the_same_item_twice_moves_it() {
        let mut state = AlchemyState::default();
        state.place(1, 20);
        state.place(3, 20);
        assert_eq!(state.slot(1), None);
        assert_eq!(state.slot(3), Some(20));
    }

    #[test]
    fn alchemy_clear_releases_every_slot() {
        let mut state = AlchemyState::default();
        state.place(EQUIP_SLOT, 13);
        state.place(4, 21);
        state.clear();
        assert!((0..=STONE_SLOTS).all(|i| state.slot(i).is_none()));
    }

    /// Out-of-range indices are ignored rather than panicking (the UI feeds
    /// these from cell components).
    #[test]
    fn alchemy_out_of_range_slot_is_ignored() {
        let mut state = AlchemyState::default();
        state.place(STONE_SLOTS + 1, 7);
        assert_eq!(state.slot(STONE_SLOTS + 1), None);
        assert_eq!(state.take(STONE_SLOTS + 1), None);
    }
}
