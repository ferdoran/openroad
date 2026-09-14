//! State the stall window renders from (#779).
//!
//! There is no wire in this ticket: the window is driven entirely from this
//! resource, the way `stall_network.rs` handles its own not-yet-known
//! opcodes. The stall wire is modelled in `packets/src/agent/stall.rs` and its
//! remainder is #759 — a layout invented here to "fill in" would be exactly
//! what that ticket exists to prevent.

use bevy::prelude::*;

/// The two states of `GDR_STALL_OWNERSTATE_*`. Both strings and both icons
/// ship (`UIIT_STT_TRADING_NOW` / `UIIT_STT_STALL_MODIFYING`,
/// `stl_condition_icon_01/_02.ddj`); **what drives the swap on the wire is
/// UNKNOWN** (`docs/re/ui/hud-stall-window.md` §9-U4), so it is a local state
/// here and not a decoded field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
// `Modifying` is constructed by the wire half (#759) and by the preview scene;
// the shell renders both states today.
#[allow(dead_code)]
pub enum StallTradingState {
    /// "Trading now" — the stall is open for business.
    #[default]
    Open,
    /// "Modifying" — the owner is editing the stall.
    Modifying,
}

/// One occupied grid cell. Deliberately minimal: the row template
/// (`ifstallslot.txt`) has a slot, a name, a quantity and a price, and every
/// value cell in the tree is `Text=""` — server-authoritative.
#[derive(Debug, Clone, PartialEq)]
pub struct StallRow {
    pub name: String,
    /// Read by the row template's quantity cell once the slot art lands
    /// (`ifstallslot.txt`); kept now so the model matches the wire row.
    #[allow(dead_code)]
    pub quantity: u16,
    pub price: u64,
}

/// The stall window's model.
#[derive(Resource, Debug, Clone, PartialEq)]
pub struct StallState {
    pub open: bool,
    /// True while the stall on screen is **ours** (`0xB0B1` accepted our
    /// create, #781). The window is one shell for both roles — owner-only
    /// controls are runtime-toggled, not a second window
    /// (`docs/re/ui/hud-stall-window.md` §4) — so this flag is what tells a
    /// buy from an edit.
    pub owner: bool,
    /// `UIIT_STT_STALL_DEFAULT_TITLE` is `[%s]'s stall.` — the default is
    /// filled in by whoever knows the owner's name, not here.
    pub title: String,
    /// `UIIT_STT_STALL_DEFAULT_OWNERMSG`, same reasoning.
    pub greeting: String,
    pub trading: StallTradingState,
    /// Ten cells, matching the grid and the wire capacity
    /// (`docs/re/systems/stall.md:78-80`).
    pub slots: Vec<Option<StallRow>>,
}

impl Default for StallState {
    fn default() -> Self {
        Self {
            open: false,
            owner: false,
            title: String::new(),
            greeting: String::new(),
            trading: StallTradingState::Open,
            slots: vec![None; super::ui::STALL_SLOTS],
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// The model's capacity is the grid's capacity is the wire's capacity —
    /// three numbers that must never drift apart.
    #[test]
    fn a_fresh_stall_has_exactly_ten_empty_slots() {
        let state = StallState::default();
        assert_eq!(state.slots.len(), 10);
        assert!(state.slots.iter().all(Option::is_none));
        assert!(!state.open);
        assert_eq!(state.trading, StallTradingState::Open);
    }
}
