//! Auto-potion window state + the server's persisted threshold settings.
//!
//! Idea: CHARACTER_DATA 0x3013's tail already carries this account's
//! auto-potion configuration — `auto_hp: u16`, `auto_mp: u16`,
//! `auto_universal: u16`, `auto_potion_delay: u8`
//! (`packets/src/agent/character_data.rs:481-484`) — and nothing read it, so a
//! returning player's thresholds were parsed and dropped every login
//! (`docs/re/ui/autopotion-window.md` §6: a dead wire, not a data-blocked
//! stub). [`AutoPotionSettings`] is that wire state, seeded once per join the
//! same way `underbar::model` seeds the quickslots, and logged on arrival so a
//! live capture surfaces real values (both in-tree captures are all zeros).
//!
//! The window renders it **read-only**: the C→S "set auto potion" opcode is
//! UNKNOWN — it appears in no doc and there are zero outbound samples — so
//! there is nothing an edit could be sent to.

use bevy::prelude::*;

use packets::agent::character_data::PlayerExtras;

use crate::plugins::hud::chat::model::ChatState;
use crate::plugins::net::character_info::CharacterInfo;
use crate::plugins::player::Player;
use crate::plugins::settings::keymap::KEY_AUTO_POTION;
use crate::plugins::settings::options::GameOptions;

/// The thresholds are percentages of the HP/MP gauge:
/// `UIIT_STT_MACROPOTION_CONTENTS` reads "Set the % of the HP/MP gage to the
/// preferred value, then auto potion recovery will be in use." The 14px
/// one-glyph unit static (`GDR_AUTOPOTION_SLOT_DATA_UNIT_STA`) and the empty
/// MIN/CENTER/MAX labels around the slider corroborate a 0..=100 range with a
/// midpoint mark. The wire type is a `u16`, which does not bound it — values
/// above 100 are UNKNOWN and only clamped for display.
pub const MAX_PERCENT: u8 = 100;

/// Open/closed state of the auto-potion window.
///
/// Opened by the `KeyAutoPotion` shortcut ([`toggle_autopotion_window`]), by
/// vanilla's own opener — the under-bar MENU popup's "Auto Potion (T)" row
/// (`underbar/menu_popup.rs`) — by the shell's close button, and by the offline
/// preview scene. The shortcut is a second entry point to the same state, not a
/// replacement for the menu row.
#[derive(Resource, Default)]
pub struct AutoPotionState {
    pub open: bool,
}

/// Toggle with the `KeyAutoPotion` shortcut (unless the chat input is capturing
/// keys).
///
/// **Vanilla's default for this action is known and is `T`** —
/// `Media/server_dep/silkroad/textdata/textuisystem.txt:2271` (UTF-16LE) reads
/// `UIIT_STT_TOGGLE_AUTOPOTION … Auto Potion (T)`, one of fourteen defaults
/// spelled out in `:2250-2274`; the neighbouring `_CONTENTS_MENU_*` family
/// carries the same labels with `%s` at the key position, which is what makes
/// the parenthesised letter the binding rather than decoration.
///
/// It is deliberately **not** set here anyway. Filling in the documented
/// defaults is #657's job and covers 28 of the 32 actions at once, including
/// the four this tree already binds; doing one of them early in a window PR
/// would land a keymap change under a HUD ticket and pre-empt that review.
/// Until then the action resolves through `GameOptions::key_for` and is bound
/// in the options window's Key Map tab, exactly like `KeyAlchemy` (3027) and
/// `KeyCOSInfo` (3016).
pub fn toggle_autopotion_window(
    keys: Res<ButtonInput<KeyCode>>,
    chat: Res<ChatState>,
    options: Res<GameOptions>,
    mut state: ResMut<AutoPotionState>,
) {
    let Some(key) = options.key_for(KEY_AUTO_POTION) else {
        return;
    };
    if keys.just_pressed(key) && !chat.input_open {
        state.open = !state.open;
    }
}

/// The four vanilla rows of the panel, in layout order. Only HP and MP carry a
/// slider; the abnormal-status row is a checkbox plus two source pickers and
/// the delay row is a spin control.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoPotionRow {
    Hp,
    Mp,
    /// `auto_universal` — the "Abnormal status" row (universal pills).
    Universal,
    /// `auto_potion_delay` — the potion-use delay. Its **unit is UNKNOWN**
    /// (the row has no unit static), so the raw number is displayed.
    Delay,
}

/// The server's auto-potion configuration, as last received in 0x3013.
///
/// Percentages are clamped into `0..=MAX_PERCENT`; `delay` is the raw wire
/// value. A row's checkbox is its on/off switch, so `0` reads as "off".
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AutoPotionSettings {
    pub hp_percent: u8,
    pub mp_percent: u8,
    pub universal_percent: u8,
    pub delay: u8,
}

impl AutoPotionSettings {
    /// Map the 0x3013 tail onto the panel's rows.
    ///
    /// The two combo boxes per row pick the potion *source* (Belt / Quick slot)
    /// rather than what to cure, and the wire carries only one number per row —
    /// so the source selection is not on the wire at all and stays UNKNOWN.
    pub fn from_extras(extras: &PlayerExtras) -> Self {
        Self {
            hp_percent: clamp_percent(extras.auto_hp),
            mp_percent: clamp_percent(extras.auto_mp),
            universal_percent: clamp_percent(extras.auto_universal),
            delay: extras.auto_potion_delay,
        }
    }

    /// The number a row displays.
    pub fn value(&self, row: AutoPotionRow) -> u8 {
        match row {
            AutoPotionRow::Hp => self.hp_percent,
            AutoPotionRow::Mp => self.mp_percent,
            AutoPotionRow::Universal => self.universal_percent,
            AutoPotionRow::Delay => self.delay,
        }
    }

    /// Whether a row's checkbox is ticked. The per-row checkbox is the on/off
    /// switch and the server sends `0` for a disabled row, so zero is off.
    pub fn row_enabled(&self, row: AutoPotionRow) -> bool {
        self.value(row) != 0
    }
}

/// Clamp a wire threshold into the documented percentage range.
fn clamp_percent(raw: u16) -> u8 {
    raw.min(MAX_PERCENT as u16) as u8
}

/// Whether any percentage row arrived above the documented range.
fn any_percent_out_of_range(extras: &PlayerExtras) -> bool {
    [extras.auto_hp, extras.auto_mp, extras.auto_universal]
        .iter()
        .any(|raw| *raw > MAX_PERCENT as u16)
}

/// Seed the settings from the 0x3013 character data — the only source there is,
/// since the settings live on the server and the client cannot write them back.
/// Runs on a fresh `CharacterInfo` (a (re)join replaces the whole record), and
/// logs the raw tail so a live capture can confirm the percentage reading.
pub fn seed_autopotion_from_character_info(
    fresh: Query<&CharacterInfo, (With<Player>, Added<CharacterInfo>)>,
    mut settings: ResMut<AutoPotionSettings>,
) {
    for info in fresh.iter() {
        let Some(extras) = info.extras.as_ref() else {
            continue;
        };
        info!(
            "autopotion: server settings (raw) hp {} mp {} universal {} delay {}",
            extras.auto_hp, extras.auto_mp, extras.auto_universal, extras.auto_potion_delay
        );
        // One warning per join, not per frame: this system only sees a fresh
        // record.
        if any_percent_out_of_range(extras) {
            warn!(
                "autopotion: threshold above {}% on the wire (hp {} mp {} universal {}) — the \
                 reading of such values is UNKNOWN, clamping for display",
                MAX_PERCENT, extras.auto_hp, extras.auto_mp, extras.auto_universal
            );
        }
        *settings = AutoPotionSettings::from_extras(extras);
    }
}

#[cfg(test)]
mod test {
    use super::*;

    use crate::plugins::settings::keymap::action;

    /// A `PlayerExtras` carrying just the four auto-potion fields; every other
    /// field is irrelevant here (see `packets/src/agent/character_data.rs:468`).
    fn extras(hp: u16, mp: u16, universal: u16, delay: u8) -> PlayerExtras {
        PlayerExtras {
            pvp_state: 0,
            transport_flag: 0,
            in_combat: 0,
            transport_id: None,
            pvp_flag: 0,
            guide_flag: 0,
            jid: 0,
            gm: false,
            activation_flag: 0,
            hotkeys: Vec::new(),
            auto_hp: hp,
            auto_mp: mp,
            auto_universal: universal,
            auto_potion_delay: delay,
            blocked_whispers: Vec::new(),
            unk_u32: 0,
            unk_u8: 0,
        }
    }

    /// The four wire fields map one-to-one onto the four panel rows:
    /// `auto_hp`/`auto_mp` onto the two `CIFAutoPotionSlot` sliders,
    /// `auto_universal` onto the abnormal-status row and `auto_potion_delay`
    /// onto the delay spin (`ifautopotion.txt` sections `AbnormalSlot` /
    /// `PotionDelaySlot`).
    #[test]
    fn wire_fields_map_onto_the_four_rows() {
        let settings = AutoPotionSettings::from_extras(&extras(60, 45, 30, 4));
        assert_eq!(settings.value(AutoPotionRow::Hp), 60);
        assert_eq!(settings.value(AutoPotionRow::Mp), 45);
        assert_eq!(settings.value(AutoPotionRow::Universal), 30);
        assert_eq!(settings.value(AutoPotionRow::Delay), 4);
    }

    /// `UIIT_STT_MACROPOTION_CONTENTS` bounds the thresholds at 100%, the wire
    /// type does not — so out-of-range values are clamped rather than trusted.
    /// The delay is NOT a percentage (no unit static on that row), so it passes
    /// through unclamped.
    #[test]
    fn percentages_clamp_at_a_hundred_but_the_delay_does_not() {
        let settings = AutoPotionSettings::from_extras(&extras(255, 1000, 101, 200));
        assert_eq!(settings.hp_percent, MAX_PERCENT);
        assert_eq!(settings.mp_percent, MAX_PERCENT);
        assert_eq!(settings.universal_percent, MAX_PERCENT);
        assert_eq!(settings.delay, 200);
        assert!(any_percent_out_of_range(&extras(255, 0, 0, 0)));
        assert!(!any_percent_out_of_range(&extras(100, 100, 100, 255)));
    }

    /// The per-row checkbox is the on/off switch
    /// (`UIIT_STT_MACROPOTION_CONTENTS`: "To not use auto recovery for a
    /// certain category, remove the check from the check box"), so a zero
    /// threshold reads as a disabled row — which is what both in-tree captures
    /// contain.
    #[test]
    fn zero_reads_as_a_disabled_row() {
        let off = AutoPotionSettings::from_extras(&extras(0, 0, 0, 0));
        assert_eq!(off, AutoPotionSettings::default());
        for row in [
            AutoPotionRow::Hp,
            AutoPotionRow::Mp,
            AutoPotionRow::Universal,
            AutoPotionRow::Delay,
        ] {
            assert!(!off.row_enabled(row), "{row:?} should read as off");
        }
        let on = AutoPotionSettings::from_extras(&extras(1, 0, 0, 0));
        assert!(on.row_enabled(AutoPotionRow::Hp));
        assert!(!on.row_enabled(AutoPotionRow::Mp));
    }

    /// The window was built, registered and unreachable: `open` was set only by
    /// its own close button and the preview scene. With 3024 bound the shortcut
    /// now flips it — and, like its two siblings, it must stay quiet while the
    /// chat input is capturing keys, or typing "t" in chat would open windows.
    #[test]
    fn the_shortcut_toggles_the_window_but_not_while_chat_is_capturing() {
        let mut app = App::new();
        app.init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ChatState>()
            .init_resource::<GameOptions>()
            .init_resource::<AutoPotionState>()
            .add_systems(Update, toggle_autopotion_window);

        // #662 landed the documented defaults, so 3024 now resolves to `T`
        // out of the box (`keymap.rs:177-181`, textuisystem L2271). This test
        // binds it explicitly anyway, so it asserts *this* window's behaviour
        // rather than the contents of a table it does not own.
        assert!(app
            .world_mut()
            .resource_mut::<GameOptions>()
            .bind_key(KEY_AUTO_POTION, KeyCode::KeyT));

        // `reset`, not `clear`: `clear()` drops only the just_pressed/just_released
        // sets, and `press()` records a just_pressed **only when the key was not
        // already in `pressed`** (`bevy_input::ButtonInput::press`), so a held key
        // makes a clear+press pair register nothing at all and the toggle never
        // fires. `reset` removes the held state too, which is what "a fresh
        // keypress" means here. There is no `InputPlugin` in this app to clear the
        // sets for us, so the fixture has to do it.
        let press = |app: &mut App| {
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .reset(KeyCode::KeyT);
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .press(KeyCode::KeyT);
            app.update();
        };

        press(&mut app);
        assert!(app.world().resource::<AutoPotionState>().open);
        press(&mut app);
        assert!(!app.world().resource::<AutoPotionState>().open);

        app.world_mut().resource_mut::<ChatState>().input_open = true;
        press(&mut app);
        assert!(!app.world().resource::<AutoPotionState>().open);
    }

    /// The window resolves its shortcut through the shared 32-action registry,
    /// not through a key constant of its own — which is what lets the keymap
    /// defaults land without touching this module.
    ///
    /// **This deliberately no longer pins `default_key`.** It used to assert
    /// `key_for(3024) == None`, described in its own comment as "a checkpoint,
    /// not a prohibition" against a drive-by default slipping in under a HUD
    /// ticket. #662 is now landing exactly that default — `T`, sourced from
    /// `textuisystem.txt:2271` — which is the flip that comment invited. So the
    /// checkpoint has served its purpose and is retired rather than fought:
    /// re-asserting `None` would break #662's merge, and asserting
    /// `Some(KeyCode::KeyT)` would both duplicate #662's own test and couple
    /// this window to the contents of a table it does not own. What remains
    /// this module's business is that it points at the right row.
    #[test]
    fn the_shortcut_resolves_through_the_shared_action_registry() {
        assert_eq!(KEY_AUTO_POTION, 3024);
        let registered = action(KEY_AUTO_POTION).expect("3024 is a registry action");
        assert_eq!(registered.name, "KeyAutoPotion");
    }
}
