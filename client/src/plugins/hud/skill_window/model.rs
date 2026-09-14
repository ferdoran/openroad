//! Skill window state: open/close toggle, tab selection, the skill-icon
//! drag carry, the practice (level-up confirmation) prompt, and quickslot
//! synchronization.

use bevy::prelude::*;

use crate::plugins::hud::chat::model::ChatState;
use crate::plugins::hud::underbar::model::{PlayerProgress, QuickSlots, SlotAction};
use crate::plugins::settings::keymap::KEY_SKILL;
use crate::plugins::settings::options::GameOptions;
use crate::plugins::skills::book::{SkillBook, SkillGroupIndex};
use crate::plugins::textdata::ClientSkillData;

/// Which skill tree the window shows — CH characters never see EU skills
/// and vice versa. Set by whoever spawns the local character (the Skills
/// scene's spawner, the game scene's join).
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq, Debug)]
pub enum SkillTreeRace {
    #[default]
    Chinese,
    European,
}

impl SkillTreeRace {
    /// Top-level tab labels (mastery table col 6 groups the masteries under
    /// these): CH Weapon/Force, EU Physical/Magical/Assist.
    pub fn top_tabs(self) -> &'static [&'static str] {
        match self {
            SkillTreeRace::Chinese => &["Weapon", "Force"],
            SkillTreeRace::European => &["Physical", "Magical", "Assist"],
        }
    }

    /// Whether a mastery id belongs to this race's tree (CH 257-276, EU 513+;
    /// GM 289 is neither).
    pub fn owns_mastery(self, id: u32) -> bool {
        match self {
            SkillTreeRace::Chinese => id < 289,
            SkillTreeRace::European => id >= 512,
        }
    }
}

/// A pending level-up awaiting confirmation in the practice box.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PracticeAction {
    /// Learn the next rung of a skill ladder (the rung's skilldata id).
    LearnSkill { group_id: i32, skill_id: i32 },
    /// Raise a mastery by one level.
    RaiseMastery { mastery: u32 },
}

/// Open/close + tab state of the skill window (the S key).
#[derive(Resource, Default)]
pub struct SkillWindowState {
    pub open: bool,
    /// Selected top-level tab (mastery table col 6).
    pub top_tab: u8,
    /// Selected mastery within the top tab; `None` picks the first.
    pub mastery: Option<u32>,
    /// An open practice (confirmation) box, if any.
    pub prompt: Option<PracticeAction>,
    /// Preserved scroll offset of the branch-row list, so a learn/level-up
    /// rebuild keeps the view where it was (reset on tab/mastery change).
    pub scroll_y: f32,
}

/// A skill icon being carried (skilldata id). From the window it's a
/// vanilla click-carry toward the underbar (survives stray releases;
/// Escape/right-click cancels); lifted *from* an underbar slot it's a
/// hold-drag — releasing outside a slot discards it (vanilla drag-out).
/// Parallel to the inventory's `InventoryState.drag`, sharing the same
/// `DragGhost` cursor icon.
#[derive(Resource, Default)]
pub struct SkillDrag {
    pub skill: Option<u32>,
    pub from_underbar: bool,
}

/// The `KeySkill` shortcut toggles the window (unless the chat input is capturing
/// keys). Rebindable via the Key Map tab; S by default.
pub fn toggle_skill_window(
    keys: Res<ButtonInput<KeyCode>>,
    chat: Res<ChatState>,
    options: Res<GameOptions>,
    mut state: ResMut<SkillWindowState>,
) {
    let Some(key) = options.key_for(KEY_SKILL) else {
        return;
    };
    if keys.just_pressed(key) && !chat.input_open {
        state.open = !state.open;
        if !state.open {
            state.prompt = None;
        }
    }
}

/// Repaint when the book/SP/tab state feeding the window changes.
pub fn skill_window_needs_refresh(
    state: Res<SkillWindowState>,
    race: Res<SkillTreeRace>,
    book: Res<SkillBook>,
    index: Res<SkillGroupIndex>,
    progress: Res<PlayerProgress>,
    fresh: Query<(), Added<super::ui::SkillWindowRoot>>,
) -> bool {
    state.is_changed()
        || race.is_changed()
        || book.is_changed()
        || index.is_changed()
        || progress.is_changed()
        || !fresh.is_empty()
}

/// Keep quickslot skill references consistent with the book: leveling a
/// skill upgrades its slots to the new rung (like vanilla), withdrawing it
/// entirely clears them.
pub fn sync_quickslots_with_book(
    book: Res<SkillBook>,
    skill_data: Res<ClientSkillData>,
    mut quickslots: ResMut<QuickSlots>,
) {
    if !book.is_changed() {
        return;
    }
    let remap = |action: &Option<SlotAction>| -> Option<Option<SlotAction>> {
        let Some(SlotAction::Skill { ref_id }) = action else {
            return None;
        };
        let group = skill_data.get(&(*ref_id as i32))?.group_id();
        if group == 0 {
            return None;
        }
        match book.learned.get(&group) {
            Some(learned) if learned.skill_id == *ref_id as i32 => None,
            Some(learned) => Some(Some(SlotAction::Skill {
                ref_id: learned.skill_id as u32,
            })),
            None => Some(None),
        }
    };
    // compute first so an unchanged frame doesn't dirty the underbar refresh
    let slot_changes: Vec<(usize, Option<SlotAction>)> = quickslots
        .slots
        .iter()
        .enumerate()
        .filter_map(|(idx, action)| remap(action).map(|new| (idx, new)))
        .collect();
    let special_change = remap(&quickslots.special);
    if slot_changes.is_empty() && special_change.is_none() {
        return;
    }
    for (idx, new_action) in slot_changes {
        quickslots.slots[idx] = new_action;
    }
    if let Some(new_special) = special_change {
        quickslots.special = new_special;
    }
}

/// The mastery's abbreviation used in the `UIIT_STT_MASTERY_GROUP_<abbr>_<n>`
/// branch-series tooltip keys (CH masteries only; EU has no such keys).
pub fn mastery_group_abbrev(id: u32) -> Option<&'static str> {
    Some(match id {
        257 => "VI",   // Bicheon
        258 => "HS",   // Heuksal
        259 => "PA",   // Pacheon
        273 => "HAN",  // Cold
        274 => "PUNG", // Lightning
        275 => "HWA",  // Fire
        276 => "KI",   // Force
        _ => return None,
    })
}

/// Display name for a mastery: its masterydata `name_key` resolved through
/// textuisystem, falling back to the well-known v1.188 names when either
/// table is missing (offline preview scenes).
pub fn mastery_name<'a>(
    id: u32,
    masteries: &'a crate::plugins::textdata::ClientMasteryData,
    strings: &'a crate::plugins::textdata::ClientUiStrings,
) -> &'a str {
    masteries
        .get(id)
        .and_then(|info| strings.get(&info.name_key))
        .unwrap_or_else(|| mastery_display_name(id))
}

/// Hardcoded fallback names for [`mastery_name`], for when textuisystem or
/// masterydata has not loaded.
pub fn mastery_display_name(id: u32) -> &'static str {
    match id {
        257 => "Bicheon",
        258 => "Heuksal",
        259 => "Pacheon",
        273 => "Cold",
        274 => "Lightning",
        275 => "Fire",
        276 => "Force",
        289 => "GM",
        513 => "Warrior",
        514 => "Wizard",
        515 => "Rogue",
        516 => "Warlock",
        517 => "Bard",
        518 => "Cleric",
        _ => "Mastery",
    }
}
