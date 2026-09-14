//! The KeyMap side of [`GameOptions`]: which action each key triggers.
//!
//! Idea: `OptionSet.csv` names 32 rebindable shortcut actions (ids 3001-3035
//! with three gaps) and the option stream stores each as a raw **Win32 VK code**
//! (`docs/formats/sroptionset.md`). Bevy speaks [`KeyCode`], so this module owns
//! the one translation table between the two and resolves "which key is action
//! N bound to" for the gameplay systems. The stored value always wins; a
//! [`KeyAction::default_key`] is only the fallback when nothing is bound.
//!
//! **On defaults.** A real `SROptionSet.dat` always wins — a stored binding
//! overrides everything below, and importing a user's `.dat` is still the only
//! way to learn what *that* player uses. But the shipped defaults are not all
//! unknown: the user's own
//! `Media/server_dep/silkroad/textdata/textuisystem.txt` prints fourteen of them
//! in the label text itself, at L2250-2274 (the file is **UTF-16LE**, which is
//! why an ASCII grep finds nothing there). `Character ( C )`, `Inventory ( I )`,
//! `Skill ( S )` in that run agree with what openroad already bound, and the
//! neighbouring `UIIT_STT_TOGGLE_CONTENTS_MENU_*` family (L2275+) writes the same
//! labels with `%s` where the letter goes — the same UI showing a literal in one
//! place and a live keymap lookup in the other, which is what makes the
//! bracketed letters read as the shipped bindings rather than decoration.
//!
//! So eight further actions get a data-sourced default, each carrying its
//! textuisystem line in a comment. What the data does *not* say stays `None`:
//!
//! * **`Guild ( U )` (L2252) and `Community ( U )` (L2263) claim the same
//!   letter.** Nothing in the data decides it — plausibly the original really
//!   ships both on `U` (the guild page lives inside the community window), or one
//!   label is stale. `OptionSet.csv` has no `KeyGuild` action at all, so the only
//!   one of the pair that is bindable here is `KeyCommunity`, and it is the only
//!   one bound. The collision is recorded, not resolved.
//! * **`UIIT_STT_TOGGLE_WORLD_MAP` (L2258) carries no letter** — its text is
//!   "Whole area map". `KeyWorldMap`'s `M` below is openroad's own pre-existing
//!   binding, kept for behaviour, and is *not* data-sourced.
//! * `Option ( ESC )` (L2265), `System ( Esc )` (L2274) and `Stall network ( F )`
//!   (L2272) name no `OptionSet.csv` action, so there is nothing to bind them to.
//! * The remaining actions have no string evidence and stay unbound. Inventing a
//!   letter for them would look grounded while being guesswork.
//!
//! **Three COS defaults come from the same kind of evidence**: the original
//! prints its own bindings inside the button captions in `textuisystem.txt` —
//! `UIIT_STT_COS_DISEMBARK` = "Dismount (Home)", `UIIT_STT_COS_CLEAN` =
//! "Terminated (PgUp)", `UIIT_STT_COS_AGGRESSIVE` / `_DEFENSIVE` =
//! "Offensive (PgDn)" / "Defensive (PgDn)". `KeyCOSFollow` and the rest stay
//! `None` because no caption names a key for them.

use bevy::input_focus::InputFocus;
use bevy::prelude::*;
use bevy::text::EditableText;

use super::options::GameOptions;

/// Whether a text field currently holds keyboard focus, i.e. the player is
/// typing rather than issuing shortcuts.
///
/// Every keybind toggle already refuses to fire while the *chat* input is open
/// (`ChatState::input_open`), but chat is not the only text field any more: the
/// party-match register dialog has a title box, and typing a name like
/// "Uigur run" into it would otherwise fire Inventory, Skill and the match
/// board itself as the letters went by. This is the general form of that
/// guard — any focused [`EditableText`] — and it composes as a run condition:
/// `.run_if(not(text_field_focused))`.
/// `InputFocus` is optional because it comes from bevy's `InputFocusPlugin`,
/// which the headless test apps do not build — and a run condition panics on a
/// missing `Res` exactly as a system does. No focus resource means nothing is
/// focused, so the keybind fires.
pub fn text_field_focused(
    focus: Option<Res<InputFocus>>,
    fields: Query<(), With<EditableText>>,
) -> bool {
    focus
        .and_then(|focus| focus.get())
        .is_some_and(|entity| fields.contains(entity))
}

/// One rebindable shortcut, as `OptionSet.csv` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyAction {
    /// `OptionSet.csv` id (the option-stream id, 3001..=3035).
    pub id: u16,
    /// The CSV's `Name` column, verbatim.
    pub name: &'static str,
    /// openroad's current binding, where one exists — see the module note.
    pub default_key: Option<KeyCode>,
}

/// The 32 KeyMap actions from `OptionSet.csv`, in id order.
///
/// Ids **3010, 3022 and 3028 do not exist** — the 32-record count is what makes
/// the 681-byte `SROptionSet.dat` arithmetic land, so the gaps are real and not a
/// transcription slip (`docs/formats/sroptionset.md`).
pub const KEY_ACTIONS: [KeyAction; 32] = [
    // Defaults are the textuisystem L2250-2274 literals where the data has one
    // (each cited at its entry); `KeyWorldMap`'s M is openroad's own, and every
    // action the data does not name stays unbound.
    KeyAction {
        id: 3001,
        name: "KeyCharacter",
        default_key: Some(KeyCode::KeyC),
    },
    KeyAction {
        id: 3002,
        name: "KeyInventory",
        default_key: Some(KeyCode::KeyI),
    },
    KeyAction {
        id: 3003,
        name: "KeySkill",
        default_key: Some(KeyCode::KeyS),
    },
    KeyAction {
        id: 3004,
        name: "KeyAction",
        // textuisystem L2254 `UIIT_STT_TOGGLE_ACTION` "Action ( A )"
        default_key: Some(KeyCode::KeyA),
    },
    KeyAction {
        id: 3005,
        name: "KeyParty",
        // textuisystem L2250 `UIIT_STT_TOGGLE_PARTY` "Party ( P )"
        default_key: Some(KeyCode::KeyP),
    },
    KeyAction {
        id: 3006,
        name: "KeyQuest",
        // textuisystem L2262 `UIIT_STT_TOGGLE_QUEST` "Quest ( Q )"
        default_key: Some(KeyCode::KeyQ),
    },
    KeyAction {
        id: 3007,
        name: "KeyCommunity",
        // textuisystem L2263 `UIIT_STT_TOGGLE_COMMUNITY` "Community ( U )".
        // L2252 `Guild ( U )` claims the same letter; OptionSet.csv has no
        // KeyGuild action, so this is the only bindable half of that collision
        // (see the module note) — the collision is recorded, not resolved.
        default_key: Some(KeyCode::KeyU),
    },
    KeyAction {
        id: 3008,
        name: "KeyWorldMap",
        default_key: Some(KeyCode::KeyM),
    },
    KeyAction {
        id: 3009,
        name: "KeyBerserkerMode",
        default_key: None,
    },
    KeyAction {
        id: 3011,
        name: "KeyHelp",
        default_key: None,
    },
    KeyAction {
        id: 3012,
        name: "KeyViewDropItem",
        default_key: None,
    },
    KeyAction {
        id: 3013,
        name: "KeyMouseQuickSlot",
        default_key: None,
    },
    KeyAction {
        id: 3014,
        name: "KeySitStand",
        default_key: None,
    },
    KeyAction {
        id: 3015,
        name: "KeyAutoPickup",
        default_key: None,
    },
    KeyAction {
        id: 3016,
        name: "KeyCOSInfo",
        default_key: None,
    },
    // Board/dismount is one toggle in the original (there is no separate
    // dismount action), and its caption names the key: "Dismount (Home)".
    KeyAction {
        id: 3017,
        name: "KeyCOSRide",
        default_key: Some(KeyCode::Home),
    },
    // "Terminated (PgUp)".
    KeyAction {
        id: 3018,
        name: "KeyCOSRelease",
        default_key: Some(KeyCode::PageUp),
    },
    KeyAction {
        id: 3019,
        name: "KeyCOSFollow",
        default_key: None,
    },
    KeyAction {
        id: 3020,
        name: "KeyCOSAttack",
        default_key: None,
    },
    // Offensive/defensive share one toggle, and both captions name "(PgDn)".
    KeyAction {
        id: 3021,
        name: "KeyCOSAIType",
        default_key: Some(KeyCode::PageDown),
    },
    KeyAction {
        id: 3023,
        name: "KeyReplyWhisper",
        default_key: None,
    },
    KeyAction {
        id: 3024,
        name: "KeyAutoPotion",
        // textuisystem L2271 `UIIT_STT_TOGGLE_AUTOPOTION` "Auto Potion (T)"
        default_key: Some(KeyCode::KeyT),
    },
    KeyAction {
        id: 3025,
        name: "KeyCOSSelection",
        default_key: None,
    },
    KeyAction {
        id: 3026,
        name: "KeyPartyMatch",
        // textuisystem L2251 `UIIT_STT_TOGGLE_PARTYMATCH` "Party Matching(E)"
        default_key: Some(KeyCode::KeyE),
    },
    KeyAction {
        id: 3027,
        name: "KeyAlchemy",
        // textuisystem L2273 `UIIT_STT_TOGGLE_ENCHANT` "Alchemy ( Y )"
        default_key: Some(KeyCode::KeyY),
    },
    KeyAction {
        id: 3029,
        name: "KeyTargetEnemy",
        default_key: None,
    },
    KeyAction {
        id: 3030,
        name: "KeyTargetRecent",
        default_key: None,
    },
    KeyAction {
        id: 3031,
        name: "KeyTargetSupport",
        default_key: None,
    },
    KeyAction {
        id: 3032,
        name: "KeyTargetSee",
        default_key: None,
    },
    KeyAction {
        id: 3033,
        name: "KeyAcademy",
        // textuisystem L2253 `UIIT_CTL_TC_SHORTKEY_L` "Academy ( L )"
        default_key: Some(KeyCode::KeyL),
    },
    KeyAction {
        id: 3034,
        name: "KeyHideFriends",
        default_key: None,
    },
    KeyAction {
        id: 3035,
        name: "KeyHideEnemies",
        default_key: None,
    },
];

/// The option ids openroad actually consumes today. Kept next to the migrated
/// call sites' ids so a rename cannot silently unbind a window.
pub const KEY_CHARACTER: u16 = 3001;
/// Opens the COS/companion window (`docs/re/ui/cos-pet-window.md`).
pub const KEY_COS_INFO: u16 = 3016;
pub const KEY_INVENTORY: u16 = 3002;
pub const KEY_SKILL: u16 = 3003;
/// Opens the party roster page (`docs/re/ui/hud-party-window.md`). `P` by
/// default; the dev hotkeys that used to claim `P` unconditionally are behind
/// `dev_tools` now, so the binding is actually reachable.
pub const KEY_PARTY: u16 = 3005;
/// Opens the party-matching board (`docs/re/ui/hud-party-matching.md`).
pub const KEY_PARTY_MATCH: u16 = 3026;
pub const KEY_WORLD_MAP: u16 = 3008;
pub const KEY_ALCHEMY: u16 = 3027;
/// Held (not tapped) while the player wants every dropped item in range
/// labelled — `docs/re/ui/hud-nameplates.md` §1.
pub const KEY_VIEW_DROP_ITEM: u16 = 3012;
/// Opens the auto-potion configuration window
/// (`docs/re/ui/autopotion-window.md`).
pub const KEY_AUTO_POTION: u16 = 3024;
/// Board/dismount toggle (the COS command bar's first cell).
pub const KEY_COS_RIDE: u16 = 3017;
/// Dismiss the summon ("Terminated").
pub const KEY_COS_RELEASE: u16 = 3018;
/// Order the COS to follow. Unbound by default — no caption names a key.
pub const KEY_COS_FOLLOW: u16 = 3019;
/// Send the attack pet at the selected target. Unbound by default, and that is
/// the evidence-correct choice: `UIIT_STT_COS_ATTACK` is plain "Attack", where
/// the three bound COS commands all name their key in the caption itself
/// ("Terminated (PgUp)", "Offensive (PgDn)", "Dismount (Home)").
pub const KEY_COS_ATTACK: u16 = 3020;
/// Offensive/defensive toggle.
pub const KEY_COS_AI_TYPE: u16 = 3021;

/// Win32 VK code ↔ [`KeyCode`]. One table, both directions, so they cannot drift.
///
/// Letters and digits are their ASCII uppercase values (`VK_A == 0x41`), which is
/// what the option stream stores. Only keys a player could plausibly bind are
/// listed; a press outside this table cannot be persisted and is refused at the
/// capture site rather than stored as a value we could not read back.
const VK_TABLE: &[(u32, KeyCode)] = &[
    (0x08, KeyCode::Backspace),
    (0x09, KeyCode::Tab),
    (0x0D, KeyCode::Enter),
    (0x13, KeyCode::Pause),
    (0x14, KeyCode::CapsLock),
    (0x20, KeyCode::Space),
    (0x21, KeyCode::PageUp),
    (0x22, KeyCode::PageDown),
    (0x23, KeyCode::End),
    (0x24, KeyCode::Home),
    (0x25, KeyCode::ArrowLeft),
    (0x26, KeyCode::ArrowUp),
    (0x27, KeyCode::ArrowRight),
    (0x28, KeyCode::ArrowDown),
    (0x2D, KeyCode::Insert),
    (0x2E, KeyCode::Delete),
    (0x30, KeyCode::Digit0),
    (0x31, KeyCode::Digit1),
    (0x32, KeyCode::Digit2),
    (0x33, KeyCode::Digit3),
    (0x34, KeyCode::Digit4),
    (0x35, KeyCode::Digit5),
    (0x36, KeyCode::Digit6),
    (0x37, KeyCode::Digit7),
    (0x38, KeyCode::Digit8),
    (0x39, KeyCode::Digit9),
    (0x41, KeyCode::KeyA),
    (0x42, KeyCode::KeyB),
    (0x43, KeyCode::KeyC),
    (0x44, KeyCode::KeyD),
    (0x45, KeyCode::KeyE),
    (0x46, KeyCode::KeyF),
    (0x47, KeyCode::KeyG),
    (0x48, KeyCode::KeyH),
    (0x49, KeyCode::KeyI),
    (0x4A, KeyCode::KeyJ),
    (0x4B, KeyCode::KeyK),
    (0x4C, KeyCode::KeyL),
    (0x4D, KeyCode::KeyM),
    (0x4E, KeyCode::KeyN),
    (0x4F, KeyCode::KeyO),
    (0x50, KeyCode::KeyP),
    (0x51, KeyCode::KeyQ),
    (0x52, KeyCode::KeyR),
    (0x53, KeyCode::KeyS),
    (0x54, KeyCode::KeyT),
    (0x55, KeyCode::KeyU),
    (0x56, KeyCode::KeyV),
    (0x57, KeyCode::KeyW),
    (0x58, KeyCode::KeyX),
    (0x59, KeyCode::KeyY),
    (0x5A, KeyCode::KeyZ),
    (0x60, KeyCode::Numpad0),
    (0x61, KeyCode::Numpad1),
    (0x62, KeyCode::Numpad2),
    (0x63, KeyCode::Numpad3),
    (0x64, KeyCode::Numpad4),
    (0x65, KeyCode::Numpad5),
    (0x66, KeyCode::Numpad6),
    (0x67, KeyCode::Numpad7),
    (0x68, KeyCode::Numpad8),
    (0x69, KeyCode::Numpad9),
    (0x6A, KeyCode::NumpadMultiply),
    (0x6B, KeyCode::NumpadAdd),
    (0x6D, KeyCode::NumpadSubtract),
    (0x6E, KeyCode::NumpadDecimal),
    (0x6F, KeyCode::NumpadDivide),
    (0x70, KeyCode::F1),
    (0x71, KeyCode::F2),
    (0x72, KeyCode::F3),
    (0x73, KeyCode::F4),
    (0x74, KeyCode::F5),
    (0x75, KeyCode::F6),
    (0x76, KeyCode::F7),
    (0x77, KeyCode::F8),
    (0x78, KeyCode::F9),
    (0x79, KeyCode::F10),
    (0x7A, KeyCode::F11),
    (0x7B, KeyCode::F12),
    (0xBA, KeyCode::Semicolon),
    (0xBB, KeyCode::Equal),
    (0xBC, KeyCode::Comma),
    (0xBD, KeyCode::Minus),
    (0xBE, KeyCode::Period),
    (0xBF, KeyCode::Slash),
    (0xC0, KeyCode::Backquote),
    (0xDB, KeyCode::BracketLeft),
    (0xDC, KeyCode::Backslash),
    (0xDD, KeyCode::BracketRight),
    (0xDE, KeyCode::Quote),
];

/// A stored VK code as a Bevy key, or `None` if it is outside [`VK_TABLE`].
pub fn vk_to_keycode(vk: u32) -> Option<KeyCode> {
    VK_TABLE
        .iter()
        .find_map(|&(code, key)| (code == vk).then_some(key))
}

/// A Bevy key as the VK code the option stream stores, or `None` if it has no
/// representation there — such a key cannot be persisted, so it is not accepted.
pub fn keycode_to_vk(key: KeyCode) -> Option<u32> {
    VK_TABLE
        .iter()
        .find_map(|&(code, k)| (k == key).then_some(code))
}

/// Metadata for one action id.
pub fn action(id: u16) -> Option<&'static KeyAction> {
    KEY_ACTIONS.iter().find(|a| a.id == id)
}

impl GameOptions {
    /// Which key currently triggers action `id`: the stored binding if there is a
    /// readable one, else the action's default, else unbound.
    ///
    /// A stored VK code outside [`VK_TABLE`] falls back to the default rather than
    /// leaving the action dead — an unreadable binding is a data problem, not a
    /// reason to lose the shortcut.
    pub fn key_for(&self, id: u16) -> Option<KeyCode> {
        self.keymap
            .bindings
            .get(&id)
            .copied()
            .and_then(vk_to_keycode)
            .or_else(|| action(id).and_then(|a| a.default_key))
    }

    /// Bind `key` to action `id`. Returns `false` (and changes nothing) when the
    /// key has no VK representation, so the caller can reject the capture.
    pub fn bind_key(&mut self, id: u16, key: KeyCode) -> bool {
        match keycode_to_vk(key) {
            Some(vk) => {
                self.keymap.bindings.insert(id, vk);
                true
            }
            None => false,
        }
    }

    /// Drop the stored binding for `id`, so [`Self::key_for`] falls back to the
    /// action's default.
    pub fn reset_key(&mut self, id: u16) {
        self.keymap.bindings.remove(&id);
    }

    /// Every other action currently resolving to the same key as `id`.
    ///
    /// Two actions on one key is a real state the option stream can hold (and the
    /// UI must surface), not something to silently repair.
    pub fn key_conflicts(&self, id: u16) -> Vec<u16> {
        let Some(key) = self.key_for(id) else {
            return Vec::new();
        };
        KEY_ACTIONS
            .iter()
            .filter(|a| a.id != id && self.key_for(a.id) == Some(key))
            .map(|a| a.id)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The trap that came with the party-title field: typing a name like
    /// "Uigur run" would otherwise fire Inventory, Skill and the match board
    /// itself as the letters went by, because every toggle gated only on the
    /// *chat* input being open.
    #[test]
    fn a_focused_text_field_blocks_the_keybinds_and_nothing_else_does() {
        let mut app = App::new();
        app.init_resource::<InputFocus>();

        let field = app.world_mut().spawn(EditableText::new("")).id();
        let plain = app.world_mut().spawn_empty().id();

        let focused = |app: &mut App| {
            app.world_mut()
                .run_system_cached(text_field_focused)
                .expect("the condition must be callable")
        };

        // nothing focused
        assert!(!focused(&mut app));

        // a focused entity that is not a text field is not typing
        app.world_mut()
            .resource_mut::<InputFocus>()
            .set(plain, bevy::input_focus::FocusCause::Navigated);
        assert!(!focused(&mut app));

        // ...and one that is, is
        app.world_mut()
            .resource_mut::<InputFocus>()
            .set(field, bevy::input_focus::FocusCause::Navigated);
        assert!(focused(&mut app));

        app.world_mut().resource_mut::<InputFocus>().clear();
        assert!(!focused(&mut app));
    }

    /// The headless apps do not build bevy's `InputFocusPlugin`, and a run
    /// condition panics on a missing `Res` exactly as a system does. No focus
    /// resource means nothing is focused, so the keybind fires.
    #[test]
    fn a_world_without_the_focus_resource_does_not_block_keybinds() {
        let mut app = App::new();
        assert!(!app
            .world_mut()
            .run_system_cached(text_field_focused)
            .expect("the condition must survive a missing InputFocus"));
    }

    /// The gaps are load-bearing: 32 records is what makes the documented
    /// 681-byte option-stream arithmetic work.
    #[test]
    fn the_action_table_has_32_entries_and_skips_the_three_absent_ids() {
        assert_eq!(KEY_ACTIONS.len(), 32);
        for missing in [3010u16, 3022, 3028] {
            assert!(action(missing).is_none(), "{missing} should not exist");
        }
        // ids are unique and ascending
        let ids: Vec<u16> = KEY_ACTIONS.iter().map(|a| a.id).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(ids, sorted);
    }

    /// Letters and digits are ASCII uppercase in the option stream — the same
    /// assumption `options.rs`'s own round-trip test encodes with `0x41`.
    #[test]
    fn vk_translation_round_trips_both_ways() {
        assert_eq!(vk_to_keycode(0x41), Some(KeyCode::KeyA));
        assert_eq!(keycode_to_vk(KeyCode::KeyA), Some(0x41));
        for &(vk, key) in VK_TABLE {
            assert_eq!(vk_to_keycode(vk), Some(key));
            assert_eq!(keycode_to_vk(key), Some(vk));
        }
    }

    /// The table must be injective in both directions, or a rebind could resolve
    /// to a different key than it stored.
    #[test]
    fn the_vk_table_has_no_duplicate_entries() {
        let mut vks: Vec<u32> = VK_TABLE.iter().map(|&(vk, _)| vk).collect();
        let before = vks.len();
        vks.sort_unstable();
        vks.dedup();
        assert_eq!(vks.len(), before, "duplicate VK code");

        let mut keys: Vec<String> = VK_TABLE.iter().map(|&(_, k)| format!("{k:?}")).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), before, "duplicate KeyCode");
    }

    /// Every action openroad consumes must resolve out of the box, so migrating
    /// the hardcoded call sites cannot silently disable a window.
    #[test]
    fn the_four_consumed_actions_default_to_todays_keys() {
        let opts = GameOptions::default();
        assert_eq!(opts.key_for(KEY_CHARACTER), Some(KeyCode::KeyC));
        assert_eq!(opts.key_for(KEY_INVENTORY), Some(KeyCode::KeyI));
        assert_eq!(opts.key_for(KEY_SKILL), Some(KeyCode::KeyS));
        assert_eq!(opts.key_for(KEY_WORLD_MAP), Some(KeyCode::KeyM));
    }

    /// The three COS keys the original prints inside its own button captions
    /// ("Dismount (Home)", "Terminated (PgUp)", "Offensive/Defensive (PgDn)")
    /// — transcribed data, not invented defaults (see the module note).
    #[test]
    fn the_cos_keys_named_by_vanilla_captions_are_bound() {
        let opts = GameOptions::default();
        assert_eq!(opts.key_for(KEY_COS_RIDE), Some(KeyCode::Home));
        assert_eq!(opts.key_for(KEY_COS_RELEASE), Some(KeyCode::PageUp));
        assert_eq!(opts.key_for(KEY_COS_AI_TYPE), Some(KeyCode::PageDown));
        // No caption names a key for follow, so it stays unbound.
        assert_eq!(opts.key_for(KEY_COS_FOLLOW), None);
    }

    /// Everything the data does not name stays unbound: eight data-sourced
    /// defaults (textuisystem L2250-2274) plus `C`/`I`/`S`, which that same run
    /// confirms, plus openroad's own `M` for the world map and the three COS
    /// keys the vanilla captions print = 15 bound, 17 not. Inventing a letter
    /// for the other 17 would look authoritative.
    #[test]
    fn actions_without_a_grounded_default_stay_unbound() {
        let opts = GameOptions::default();
        let bound = KEY_ACTIONS
            .iter()
            .filter(|a| opts.key_for(a.id).is_some())
            .count();
        assert_eq!(bound, 15);
        assert_eq!(KEY_ACTIONS.len() - bound, 17);
    }

    /// The defaults that come from the user's own textdata, pinned against the
    /// lines they were read from (`textuisystem.txt`, UTF-16LE, 5377 lines).
    #[test]
    fn data_sourced_defaults_match_their_textuisystem_lines() {
        let opts = GameOptions::default();
        // L2250 "Party ( P )", L2251 "Party Matching(E)", L2254 "Action ( A )"
        assert_eq!(opts.key_for(3005), Some(KeyCode::KeyP));
        assert_eq!(opts.key_for(3026), Some(KeyCode::KeyE));
        assert_eq!(opts.key_for(3004), Some(KeyCode::KeyA));
        // L2262 "Quest ( Q )", L2271 "Auto Potion (T)", L2273 "Alchemy ( Y )"
        assert_eq!(opts.key_for(3006), Some(KeyCode::KeyQ));
        assert_eq!(opts.key_for(3024), Some(KeyCode::KeyT));
        assert_eq!(opts.key_for(KEY_ALCHEMY), Some(KeyCode::KeyY));
        // L2253 "Academy ( L )"
        assert_eq!(opts.key_for(3033), Some(KeyCode::KeyL));
    }

    /// The acceptance's hard rule: `Guild ( U )` (L2252) and `Community ( U )`
    /// (L2263) claim the same letter and nothing in the data decides it, so at
    /// most one action may hold `U` — and the world map, whose label carries no
    /// letter at all (L2258 "Whole area map"), must not have been given one.
    #[test]
    fn the_u_collision_binds_at_most_one_action_and_the_world_map_gets_no_letter() {
        let opts = GameOptions::default();
        let on_u = KEY_ACTIONS
            .iter()
            .filter(|a| opts.key_for(a.id) == Some(KeyCode::KeyU))
            .count();
        assert!(on_u <= 1, "the U collision must not be resolved silently");
        // openroad's own pre-existing M, explicitly not a data-sourced letter
        assert_eq!(opts.key_for(KEY_WORLD_MAP), Some(KeyCode::KeyM));
    }

    #[test]
    fn a_stored_binding_overrides_the_default_and_reset_restores_it() {
        let mut opts = GameOptions::default();
        assert!(opts.bind_key(KEY_INVENTORY, KeyCode::F5));
        assert_eq!(opts.key_for(KEY_INVENTORY), Some(KeyCode::F5));

        opts.reset_key(KEY_INVENTORY);
        assert_eq!(opts.key_for(KEY_INVENTORY), Some(KeyCode::KeyI));
    }

    /// A key with no VK code cannot be stored, so the bind is refused outright
    /// rather than writing a value we could not read back.
    #[test]
    fn a_key_outside_the_vk_table_is_refused() {
        let mut opts = GameOptions::default();
        assert!(!opts.bind_key(KEY_INVENTORY, KeyCode::ContextMenu));
        assert!(opts.keymap.bindings.get(&KEY_INVENTORY).is_none());
        assert_eq!(opts.key_for(KEY_INVENTORY), Some(KeyCode::KeyI));
    }

    /// An unreadable stored VK falls back to the default instead of killing the
    /// shortcut.
    #[test]
    fn an_unreadable_stored_vk_falls_back_to_the_default() {
        let mut opts = GameOptions::default();
        opts.keymap.bindings.insert(KEY_SKILL, 0xFFFF);

        assert_eq!(opts.key_for(KEY_SKILL), Some(KeyCode::KeyS));
    }

    #[test]
    fn binding_two_actions_to_one_key_is_reported_as_a_conflict() {
        let mut opts = GameOptions::default();
        assert!(opts.key_conflicts(KEY_INVENTORY).is_empty());

        assert!(opts.bind_key(KEY_INVENTORY, KeyCode::KeyS));

        assert_eq!(opts.key_conflicts(KEY_INVENTORY), vec![KEY_SKILL]);
        assert_eq!(opts.key_conflicts(KEY_SKILL), vec![KEY_INVENTORY]);
    }

    /// Unbound actions all resolve to `None`; that must not read as 28 mutual
    /// conflicts.
    #[test]
    fn unbound_actions_do_not_conflict_with_each_other() {
        let opts = GameOptions::default();

        assert!(opts.key_conflicts(3004).is_empty());
        assert!(opts.key_conflicts(3035).is_empty());
    }
}
