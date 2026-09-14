//! Particle-effect selection for rare ("Seal of …") items.
//!
//! Ground-drop sparkles by item category (`item_drop_{equip,acc}_rare[_eu].efp`).
//! The *equipped* aura instead comes from the real `ItemRare.txt` table (effect
//! path + scale + weapon bone) — see [`crate::plugins::textdata::ClientRareEffects`].

use crate::assets::textdata::itemdata::ItemDataRow;

/// The ground-drop sparkle for a rare item (every rare item has one).
pub fn drop_effect_path(row: &ItemDataRow) -> &'static str {
    let eu = row.country() == Some(1);
    match row.type_ids() {
        // accessories (earring/necklace/ring)
        Some((3, 1, 5, _)) if eu => "particles://system/item_drop_acc_rare_eu.efp",
        Some((3, 1, 5, _)) => "particles://system/item_drop_acc_rare.efp",
        // any other equipment
        Some((3, 1, _, _)) if eu => "particles://system/item_drop_equip_rare_eu.efp",
        Some((3, 1, _, _)) => "particles://system/item_drop_equip_rare.efp",
        // non-equipment rare item
        _ => "particles://system/item_drop_rare.efp",
    }
}

// (The equipped-aura effect/scale/bone now come from `ItemRare.txt`, not a
// class/tier heuristic — see `ClientRareEffects` and `apply_rare_aura`.)
