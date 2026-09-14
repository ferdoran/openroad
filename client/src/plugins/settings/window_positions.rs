//! Which windows the original persists, and where the player left ours.
//!
//! Idea: `wndpos.dat` is the original's window-position store — a 96-byte file
//! holding **ten** `i32` (x, y) pairs in a fixed, hardcoded order
//! (`docs/formats/wndpos.md`, `docs/re/ui/wndpos-persistence.md` §2/§3). We
//! reproduce the *set* and its order, because that is the fidelity-bearing
//! part: it says which windows the original considered worth remembering, and
//! it is not the same as the set of windows it lets you drag — 17 further
//! `mframe_wnd_`-framed windows are movable and **not** persisted
//! (`wndpos-persistence.md` §4, which refutes the "absent from wndpos =>
//! not user-movable" reading).
//!
//! What we deliberately do **not** reproduce is the file. Our persistence is
//! import-only by design (`persistence.rs`), so this rides
//! `user_settings.yaml` with the rest of [`GameOptions`] and never reads or
//! writes a real `wndpos.dat`.
//!
//! The stored pair is **our** anchor, not the original's. Vanilla stores a
//! left/top pair against a saved resolution stamp; our shell is right/top
//! anchored (`hud/game_window.rs`), so storing left/top would mean converting
//! through a viewport width at both ends and inventing a rounding rule for a
//! file we never emit. Storing what the shell actually uses keeps the round
//! trip exact. Both components stay **signed** — a window may sit partly past
//! the right edge, exactly as slot 0's `y = -8` shows the original allows.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The ten slots of `wndpos.dat`, in the file's own order
/// (`wndpos-persistence.md` §2 — array index x ginterface cross-check).
///
/// Serialised by name so the YAML stays readable and stable if the order is
/// ever re-derived; the numeric order is the original's, and is asserted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum WndPosSlot {
    /// `GDR_MAINPOPUP` — one tabbed window in the original, so its slot covers
    /// inventory/character/skill/quest together (we still ship those apart).
    MainPopup,
    Store,
    StorageRoom,
    Exchange,
    WorldMap,
    CosWnd,
    GameGuide,
    AlchemyBox,
    AutoPotion,
    ExtQuickSlot,
}

impl WndPosSlot {
    /// The ten slots in `wndpos.dat` order.
    pub const ALL: [WndPosSlot; 10] = [
        WndPosSlot::MainPopup,
        WndPosSlot::Store,
        WndPosSlot::StorageRoom,
        WndPosSlot::Exchange,
        WndPosSlot::WorldMap,
        WndPosSlot::CosWnd,
        WndPosSlot::GameGuide,
        WndPosSlot::AlchemyBox,
        WndPosSlot::AutoPotion,
        WndPosSlot::ExtQuickSlot,
    ];

    /// Array index in the original file (0-9).
    pub fn index(self) -> usize {
        WndPosSlot::ALL
            .iter()
            .position(|slot| *slot == self)
            .expect("ALL covers every variant")
    }
}

/// Saved anchors, keyed by slot. Empty until the player drags something.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WindowPositions(BTreeMap<WndPosSlot, (f32, f32)>);

impl WindowPositions {
    /// The saved `(right, top)` anchor of `slot`, in physical pixels.
    pub fn get(&self, slot: WndPosSlot) -> Option<(f32, f32)> {
        self.0.get(&slot).copied()
    }

    /// Record where the player left the window. Returns whether anything
    /// changed, so a caller can avoid touching [`GameOptions`] (and therefore
    /// the settings file) on a drag that ended where it started.
    pub fn set(&mut self, slot: WndPosSlot, anchor: (f32, f32)) -> bool {
        if self.0.get(&slot) == Some(&anchor) {
            return false;
        }
        self.0.insert(slot, anchor);
        true
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// The order is the file's, not alphabetical or aesthetic
    /// (`wndpos-persistence.md` §2, byte-verified against the sample at §3).
    #[test]
    fn the_slot_order_is_the_files_order() {
        assert_eq!(WndPosSlot::ALL.len(), 10);
        assert_eq!(WndPosSlot::MainPopup.index(), 0);
        assert_eq!(WndPosSlot::Store.index(), 1);
        assert_eq!(WndPosSlot::StorageRoom.index(), 2);
        assert_eq!(WndPosSlot::Exchange.index(), 3);
        assert_eq!(WndPosSlot::WorldMap.index(), 4);
        assert_eq!(WndPosSlot::CosWnd.index(), 5);
        assert_eq!(WndPosSlot::GameGuide.index(), 6);
        assert_eq!(WndPosSlot::AlchemyBox.index(), 7);
        assert_eq!(WndPosSlot::AutoPotion.index(), 8);
        assert_eq!(WndPosSlot::ExtQuickSlot.index(), 9);
    }

    /// Signed both ways: the original's own sample stores `y = -8` in slot 0,
    /// so a store that refused negatives would corrupt a legal position.
    #[test]
    fn positions_round_trip_including_negatives() {
        let mut positions = WindowPositions::default();
        assert!(positions.set(WndPosSlot::MainPopup, (80.0, -8.0)));
        assert_eq!(positions.get(WndPosSlot::MainPopup), Some((80.0, -8.0)));
        // an unchanged write reports no change, so it cannot churn the file
        assert!(!positions.set(WndPosSlot::MainPopup, (80.0, -8.0)));
        assert!(positions.set(WndPosSlot::MainPopup, (81.0, -8.0)));
        // untouched slots stay absent rather than defaulting to an origin
        assert_eq!(positions.get(WndPosSlot::WorldMap), None);
    }

    /// The YAML is keyed by name, so re-deriving the order later cannot
    /// silently re-map a saved position onto the wrong window.
    #[test]
    fn the_yaml_is_keyed_by_slot_name() {
        let mut positions = WindowPositions::default();
        positions.set(WndPosSlot::WorldMap, (12.0, 34.0));
        let yaml = serde_yaml::to_string(&positions).expect("serialises");
        assert!(yaml.contains("WorldMap"), "{yaml}");
        let back: WindowPositions = serde_yaml::from_str(&yaml).expect("round trips");
        assert_eq!(back.get(WndPosSlot::WorldMap), Some((12.0, 34.0)));
    }
}
