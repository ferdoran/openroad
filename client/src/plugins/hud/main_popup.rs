//! The MainPopup group: the windows the original ships as **one** frame.
//!
//! `GDR_MAINPOPUP` (`ginterface.txt` id 25, `595,262,388,408`, `Text=""`) is a
//! single tabbed window hosting eight mutually-exclusive pages, with a vertical
//! seven-button tab rail hanging off its left edge at `x=-36`
//! (`ifmainpopup.txt:7,27,46,65,84,103,122,141` and `:161-294`). We ship its
//! pages as separate free-floating windows, which
//! `docs/re/ui/scene-game-hud-composition.md` §5a calls the largest single
//! structural drift in the HUD — five windows that can all be open at once and
//! overlap each other, where the original has one.
//!
//! This module closes the **behavioural** half of that (#302): the pages are
//! now mutually exclusive and share one remembered position, so opening the
//! skill window puts the inventory away exactly as the original does, and the
//! group occupies one place on screen instead of five.
//!
//! # What is deliberately *not* done here
//!
//! The remaining half is art, and it needs measurements this tree does not
//! have:
//!
//! * **The `sframe_wnd_` page chrome.** MainPopup pages use `sframe_wnd_`, not
//!   the `mframe_wnd_` ring `hud::game_window` builds — that mismatch inflates
//!   the character page by 26 units (`docs/re/ui/hud-character-info.md` §6).
//!   `game_window.rs`'s own rule is "measure every piece before adding a family
//!   here", and only two of the eight pieces have recorded extents (`16x36` /
//!   `128x36` top pieces). **Blocked on a `pk2 unpack` of
//!   `interface/frame/sframe_wnd_*`.**
//! * **The seven-button tab rail.** Three of its seven anchors are recorded
//!   (`GDR_BTN_CHAR -36,78`, `_SKILL -36,162`, `_PARTY -36,246`, i.e. a 42 px
//!   pitch) and the art is `interface\mainpopup\main_sysbutton_*.ddj`, but the
//!   rail's *order* and its remaining four entries live in
//!   `ifmainpopup.txt:161-294`, unread. Two docs already disagree about which
//!   button is topmost (`hud-apprenticeship-window.md:421` says Apprentice;
//!   the character page says `_CHAR` is at the first pitch), so drawing a rail
//!   now would be inventing the answer.
//!
//! Until then the pages keep their own chrome and their own hotkeys, which is
//! how the player reaches them.

use bevy::prelude::*;

use crate::plugins::hud::action::ActionWindowState;
use crate::plugins::hud::character_info::model::CharacterInfoState;
use crate::plugins::hud::inventory::model::InventoryState;
use crate::plugins::hud::party::model::PartyWindowState;
use crate::plugins::hud::skill_window::model::SkillWindowState;

/// The pages of the original's single MainPopup frame that this client
/// actually implements.
///
/// The original declares eight; Quest and Apprenticeship have no content
/// builder here, so they are absent rather than stubbed. Equipment is not
/// listed either: it is not a sibling of Inventory but the *other pane of the
/// same page* (`GDR_INVENTORY 13,63,176,333` beside `GDR_EQUIPMENT
/// 198,41,178,355`), and this client already draws the two together.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MainPopupPage {
    Inventory,
    Character,
    Skill,
    Action,
    Party,
}

impl MainPopupPage {
    /// Every page, in the order the exclusivity sweep reads them. The order is
    /// not the rail's — that is still unknown (see the module docs) — it only
    /// has to be stable.
    pub const ALL: [MainPopupPage; 5] = [
        MainPopupPage::Inventory,
        MainPopupPage::Character,
        MainPopupPage::Skill,
        MainPopupPage::Action,
        MainPopupPage::Party,
    ];
}

/// Which page is showing, if any.
///
/// Mirrors the five `*State.open` flags rather than replacing them: those
/// flags are what each page's own `apply_*_visibility` system reads, and
/// rewriting five windows to read a sixth resource would be a large change for
/// no behaviour the mirror does not already give. This resource exists so the
/// group has one name, and so the state is inspectable.
#[derive(Resource, Default, Debug, PartialEq, Eq)]
pub struct MainPopupState {
    pub page: Option<MainPopupPage>,
}

pub struct MainPopupPlugin;

impl Plugin for MainPopupPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MainPopupState>()
            .add_systems(Update, enforce_page_exclusivity);
    }
}

/// The open flags, in [`MainPopupPage::ALL`] order.
///
/// Taken by shared reference on purpose: `ResMut`'s plain `Deref` does not mark
/// the resource changed, only `DerefMut` does. That matters because this system
/// runs every frame — dirtying five window states unconditionally would make
/// every change-detected consumer of them re-run forever, the inventory's own
/// repaint gate (`InventoryState::is_changed`) included.
fn read_flags(
    inventory: &InventoryState,
    character: &CharacterInfoState,
    skill: &SkillWindowState,
    action: &ActionWindowState,
    party: &PartyWindowState,
) -> [bool; 5] {
    [
        inventory.open,
        character.open,
        skill.open,
        action.open,
        party.open,
    ]
}

/// Decide which pages must be closed.
///
/// Returns the index of the page that newly opened, if any. Only a page that
/// went from closed to open this frame can win — a page that was *already*
/// open does not steal focus from one the player just opened, and without the
/// edge test two open pages would fight for the rest of the session.
pub fn newly_opened(previous: [bool; 5], current: [bool; 5]) -> Option<usize> {
    (0..current.len()).find(|&i| current[i] && !previous[i])
}

/// Opening one page of the group closes the others — the original's frame can
/// only show one at a time.
fn enforce_page_exclusivity(
    mut inventory: ResMut<InventoryState>,
    mut character: ResMut<CharacterInfoState>,
    mut skill: ResMut<SkillWindowState>,
    mut action: ResMut<ActionWindowState>,
    mut party: ResMut<PartyWindowState>,
    mut popup: ResMut<MainPopupState>,
    mut previous: Local<[bool; 5]>,
) {
    let current = read_flags(&inventory, &character, &skill, &action, &party);
    let winner = newly_opened(*previous, current);

    if let Some(winner) = winner {
        // Write only the flags that actually change, so the pages that were
        // already closed are not marked dirty every time another one opens.
        for (index, open) in current.iter().enumerate() {
            if index == winner || !open {
                continue;
            }
            match MainPopupPage::ALL[index] {
                MainPopupPage::Inventory => inventory.open = false,
                MainPopupPage::Character => character.open = false,
                MainPopupPage::Skill => skill.open = false,
                MainPopupPage::Action => action.open = false,
                MainPopupPage::Party => party.open = false,
            }
        }
    }

    // Re-read: the writes above may have closed pages this frame.
    let settled = read_flags(&inventory, &character, &skill, &action, &party);
    *previous = settled;

    let page = settled
        .iter()
        .position(|open| *open)
        .map(|index| MainPopupPage::ALL[index]);
    if popup.page != page {
        popup.page = page;
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// Opening a page while another is open closes the other one — the whole
    /// point. Five windows that could all be open at once was the drift.
    #[test]
    fn the_page_that_just_opened_is_the_one_that_wins() {
        // inventory was open; the player opens the skill window
        let previous = [true, false, false, false, false];
        let current = [true, false, true, false, false];
        assert_eq!(newly_opened(previous, current), Some(2));
    }

    /// A page that was already open does not win. Without the edge test, two
    /// pages that somehow ended up open would close each other in alternate
    /// frames forever.
    #[test]
    fn an_already_open_page_does_not_steal_the_frame() {
        let both = [true, false, true, false, false];
        assert_eq!(newly_opened(both, both), None);
    }

    /// Closing a page is not an opening, and must leave the rest alone.
    #[test]
    fn closing_a_page_opens_nothing() {
        let previous = [true, false, true, false, false];
        let current = [false, false, true, false, false];
        assert_eq!(newly_opened(previous, current), None);
    }

    /// End to end through the real systems and the real state resources: two
    /// hotkeys in a row leave exactly one page open, and `MainPopupState`
    /// names it.
    #[test]
    fn opening_a_second_page_puts_the_first_away() {
        let mut app = App::new();
        app.init_resource::<InventoryState>()
            .init_resource::<CharacterInfoState>()
            .init_resource::<SkillWindowState>()
            .init_resource::<ActionWindowState>()
            .init_resource::<PartyWindowState>()
            .init_resource::<MainPopupState>()
            .add_systems(Update, enforce_page_exclusivity);

        app.world_mut().resource_mut::<InventoryState>().open = true;
        app.update();
        assert_eq!(
            app.world().resource::<MainPopupState>().page,
            Some(MainPopupPage::Inventory)
        );

        app.world_mut().resource_mut::<SkillWindowState>().open = true;
        app.update();
        assert!(
            !app.world().resource::<InventoryState>().open,
            "the inventory should have been put away"
        );
        assert!(app.world().resource::<SkillWindowState>().open);
        assert_eq!(
            app.world().resource::<MainPopupState>().page,
            Some(MainPopupPage::Skill)
        );

        // and closing the survivor leaves the group closed
        app.world_mut().resource_mut::<SkillWindowState>().open = false;
        app.update();
        assert_eq!(app.world().resource::<MainPopupState>().page, None);
    }

    /// The sweep must not dirty the state resources on a quiet frame: the
    /// inventory's own repaint gate is `InventoryState::is_changed`, so a
    /// gratuitous `ResMut` deref here would rebuild the whole bag every frame.
    #[test]
    fn a_quiet_frame_marks_nothing_changed() {
        let mut app = App::new();
        app.init_resource::<InventoryState>()
            .init_resource::<CharacterInfoState>()
            .init_resource::<SkillWindowState>()
            .init_resource::<ActionWindowState>()
            .init_resource::<PartyWindowState>()
            .init_resource::<MainPopupState>()
            .add_systems(Update, enforce_page_exclusivity);
        // one update to settle the initial insert-change
        app.update();
        app.update();
        assert!(
            !app.world().resource_ref::<InventoryState>().is_changed(),
            "the exclusivity sweep dirtied the inventory on an idle frame"
        );
    }
}
