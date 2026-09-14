//! Semantic option model built on top of the raw [`OptionRecord`] TLV decode.
//!
//! [`GameOptions`] groups the settings the way the original client's option
//! tabs do (Video / Audio / Setting / KeyMap in `OptionSet.csv`). Homogeneous,
//! indexed groups — the graphic-quality sliders and the keymap — are kept as
//! id-keyed maps rather than dozens of hand-named fields (several CSV slots are
//! unnamed anyway); the distinct scalar settings get real names.
//!
//! `Default` is openroad's own baseline for the scalar settings: their shipped
//! values are UNKNOWN here (we never have a `SROptionSet.dat` to read), so they
//! are sensible starting values, not reverse-engineered constants. The **keymap**
//! group is the exception — fourteen shipped shortcuts are printed literally in
//! the user's `textuisystem.txt` (L2250-2274), so [`super::keymap::KEY_ACTIONS`]
//! carries data-sourced defaults for the actions that run names (#657).

use std::collections::BTreeMap;

use bevy::audio::Volume;
use bevy::prelude::{PlaybackSettings, Resource};
use serde::{Deserialize, Serialize};

use super::sroptionset::{OptionRecord, OptionValue};
use super::window_positions::WindowPositions;

/// One of the two graphics profiles the client stores (Graphic 1 / Graphic 2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphicProfile {
    /// `Type` (ids 501 / 601) — raw enum value, meaning UNKNOWN.
    #[serde(default)]
    pub display_type: u8,
    /// `Brightness` (ids 502 / 602) — raw slider value, scale UNKNOWN.
    #[serde(default)]
    pub brightness: u8,
    /// `WindowResolutionWidth` (ids 503 / 603).
    #[serde(default = "default_width")]
    pub width: u32,
    /// `WindowResolutionHeight` (ids 504 / 604).
    #[serde(default = "default_height")]
    pub height: u32,
    /// Graphic-quality sliders keyed by id (1..=15 / 101..=115); see the CSV
    /// in `docs/formats/sroptionset.md` for each slot's meaning.
    #[serde(default)]
    pub quality: BTreeMap<u16, u16>,
}

fn default_width() -> u32 {
    1920
}
fn default_height() -> u32 {
    1080
}

impl Default for GraphicProfile {
    fn default() -> Self {
        Self {
            display_type: 0,
            brightness: 0,
            width: default_width(),
            height: default_height(),
            quality: BTreeMap::new(),
        }
    }
}

/// The three camera view modes of the original's Camera options pane.
///
/// Idea: this is **not** an openroad invention, which is worth stating because
/// `docs/re/ui/options-camera.md` §9 records "what each mode does geometrically"
/// as UNKNOWN on the grounds that `ifoption_camera.txt` does not define it. The
/// tree does not — but `textuisystem.txt` does, in the two description lines the
/// pane itself renders next to each radio:
///
/// * `UIIT_STT_SIGHT_FREE_DESC1/2` — "Mouse oriented camera control" /
///   "Operates on multidirectional angle control and mouse movement"
/// * `UIIT_STT_SIGHT_THIRD_PERSON_DESC1/2` — "Keyboard oriented camera control" /
///   **"Camera angle is fixed behind the character"**
/// * `UIIT_STT_SIGHT_QUATER_VIEW_DESC1` — **"The height is fixed to this
///   perspective."**, confirmed by `UIIT_STT_CHANGED_SIGHT_QUARTER_VIEW_DESC`
///   ("Camera angle has been set to Fixed height point")
///
/// So each mode is defined by *which orbit axis it takes away*: free takes
/// none, third-person fixes yaw behind the character, quarter fixes pitch.
/// That is a transcription; only the exact fixed pitch value is ours
/// (see `camera::QUARTER_VIEW_PITCH`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SightMode {
    /// `UIIT_STT_SIGHT_FREE` — "Free Movement View". Yaw and pitch both follow
    /// the drag. Default because it is what openroad already did, so adopting
    /// the pane does not silently change anybody's camera; the original's own
    /// default is UNKNOWN (no `resinfo/` option tree carries a default value).
    #[default]
    Free,
    /// `UIIT_STT_SIGHT_THIRD_PERSON` — "Third Person View". Yaw is pinned
    /// behind the character; pitch and zoom still respond.
    ThirdPerson,
    /// `UIIT_STT_SIGHT_QUATER_VIEW` — "Quarter Angle View". Pitch is fixed;
    /// yaw and zoom still respond. (The original misspells "Quater" in the key
    /// and spells it "QUARTER" in the pane's `_DESC` key — both spellings are
    /// in the shipped string table.)
    Quarter,
}

impl SightMode {
    /// The three radios in pane order (top to bottom by `Rect` y in
    /// `ifoption_camera.txt`: 45 / 98 / 152).
    pub const ALL: [SightMode; 3] = [SightMode::Free, SightMode::ThirdPerson, SightMode::Quarter];
}

/// Camera view mode (`GDR_OPTION_WND_CAMERA`, #379).
///
/// **Stated non-original storage.** `docs/re/ui/options-camera.md` §9 leaves it
/// UNKNOWN whether any `SROptionSet` id covers the sight mode — our parser
/// lumps `2001..=2028` (`sroptionset.rs`) with none broken out — so no id is
/// invented here. It rides the `user_settings.yaml` path with the rest of
/// [`GameOptions`] instead, which is what makes the radio survive a restart.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CameraOptions {
    #[serde(default)]
    pub sight: SightMode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoOptions {
    #[serde(default)]
    pub graphic1: GraphicProfile,
    #[serde(default)]
    pub graphic2: GraphicProfile,
    /// `isWindowMode` (id 2015) — windowed vs fullscreen, as a **session-only**
    /// override of `config.yaml`'s `window_settings.mode`.
    ///
    /// `None` means "follow the config", and `#[serde(skip)]` means it can
    /// never be anything else at boot: the field is neither read from nor
    /// written to `user_settings.yaml`, so every launch starts from
    /// `config.yaml` and an in-session toggle lasts only for that session.
    ///
    /// **Stated deviation from the original**, which persists id 2015 across
    /// restarts: `config.yaml` is openroad's single authority for the window,
    /// and a persisted copy here is what silently overrode it (the bug this
    /// replaced — a stale `window_mode: false` in `user_settings.yaml`, or
    /// merely the `bool` default, forced borderless fullscreen on every boot
    /// no matter what the config said). An older file's `window_mode:` key is
    /// deliberately *not* aliased: it is ignored as an unknown key, which is
    /// what retires the stale value without a migration.
    ///
    /// It still takes part in [`GameOptions`]'s `PartialEq`, so flipping it
    /// does trigger a `persistence::save_on_change` write — harmless, since
    /// the field is simply absent from the emitted YAML.
    #[serde(skip)]
    pub window_mode_override: Option<bool>,
}

impl Default for VideoOptions {
    fn default() -> Self {
        Self {
            graphic1: GraphicProfile::default(),
            graphic2: GraphicProfile::default(),
            window_mode_override: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioOptions {
    /// Volume sliders (ids 1001..=1003) — raw slider values, scale UNKNOWN.
    #[serde(default = "default_volume")]
    pub bgm_volume: u32,
    #[serde(default = "default_volume")]
    pub fx_volume: u32,
    #[serde(default = "default_volume")]
    pub env_volume: u32,
    /// Per-channel on/off checkboxes (ids 1004..=1006).
    #[serde(default = "default_true")]
    pub bgm_enabled: bool,
    #[serde(default = "default_true")]
    pub fx_enabled: bool,
    #[serde(default = "default_true")]
    pub env_enabled: bool,
}

fn default_volume() -> u32 {
    100
}
fn default_true() -> bool {
    true
}

impl Default for AudioOptions {
    fn default() -> Self {
        Self {
            bgm_volume: default_volume(),
            fx_volume: default_volume(),
            env_volume: default_volume(),
            bgm_enabled: true,
            fx_enabled: true,
            env_enabled: true,
        }
    }
}

impl AudioOptions {
    /// `PlaybackSettings` for looping BGM, or `None` when BGM is muted.
    pub fn bgm_playback(&self) -> Option<PlaybackSettings> {
        self.bgm_enabled
            .then(|| PlaybackSettings::LOOP.with_volume(Self::gain(self.bgm_volume)))
    }

    /// The same looping BGM settings, but **always** produced — muted BGM is
    /// expressed as a `paused` sink rather than as a missing entity.
    ///
    /// That distinction is what makes the audio group live (#647): a track
    /// that was never spawned because BGM happened to be off at scene entry
    /// cannot start playing when the user turns BGM on, whereas a paused sink
    /// can (`apply_background_music_options`).
    pub fn bgm_playback_settings(&self) -> PlaybackSettings {
        PlaybackSettings {
            paused: !self.bgm_enabled,
            ..PlaybackSettings::LOOP.with_volume(self.bgm_gain())
        }
    }

    /// The BGM gain on its own, independent of the enable toggle: an apply
    /// system needs the intended volume even while the sink is paused.
    pub fn bgm_gain(&self) -> Volume {
        Self::gain(self.bgm_volume)
    }

    /// `PlaybackSettings` for a one-shot sound effect, or `None` when FX is muted.
    pub fn fx_playback(&self) -> Option<PlaybackSettings> {
        self.fx_enabled
            .then(|| PlaybackSettings::DESPAWN.with_volume(Self::gain(self.fx_volume)))
    }

    /// Looping settings for a zone-ambience bed, always produced: like BGM
    /// (#647) a muted environment channel is a *paused* sink, so turning the
    /// Environment row back on starts the bed that was already there
    /// (`plugins::zone_ambience::apply_zone_ambience_options`).
    pub fn env_playback_settings(&self) -> PlaybackSettings {
        PlaybackSettings {
            paused: !self.env_enabled,
            ..PlaybackSettings::LOOP.with_volume(self.env_gain())
        }
    }

    /// The environment gain on its own, for the same reason as `bgm_gain`.
    pub fn env_gain(&self) -> Volume {
        Self::gain(self.env_volume)
    }

    /// `PlaybackSettings` for a one-shot ambient, or `None` while the
    /// environment channel is muted. A one-shot is not worth pausing — it is
    /// simply not spawned, and its schedule keeps running so unmuting picks
    /// the next one up.
    pub fn env_oneshot(&self) -> Option<PlaybackSettings> {
        self.env_enabled
            .then(|| PlaybackSettings::DESPAWN.with_volume(self.env_gain()))
    }

    /// `bgm_volume`/`fx_volume` are read as 0-100 percent — openroad's own
    /// scale, since the original client's SROptionSet slider range (ids
    /// 1001..=1003) is UNKNOWN (`docs/formats/sroptionset.md:54`). The clamp
    /// keeps a hand-edited `config.yaml` from amplifying past unity gain,
    /// which is the mastered file level; the slider itself is the user's
    /// control, so 100 is not lowered to some "safer" default.
    fn gain(volume: u32) -> Volume {
        Volume::Linear((volume as f32 / 100.0).clamp(0.0, 1.0))
    }
}

/// The `Setting` tab toggles (ids 2001..=2028, excluding 2015 which is video).
/// Kept id-keyed because many CSV slots are unnamed; see the doc table.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GameplayOptions {
    #[serde(default)]
    pub toggles: BTreeMap<u16, bool>,
}

/// Custom-shortcut key bindings. Values are raw Win32 VK codes (u32); the
/// VK -> Bevy `KeyCode` translation is a later UI concern.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct KeyMapOptions {
    /// id (3001..=3099) -> Win32 VK code.
    #[serde(default)]
    pub bindings: BTreeMap<u16, u32>,
    /// `isMouseShortcutSwapped` (id 3101).
    #[serde(default)]
    pub mouse_shortcut_swapped: bool,
}

/// openroad's live, persisted player options.
#[derive(Resource, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GameOptions {
    #[serde(default)]
    pub video: VideoOptions,
    #[serde(default)]
    pub audio: AudioOptions,
    #[serde(default)]
    pub gameplay: GameplayOptions,
    #[serde(default)]
    pub keymap: KeyMapOptions,
    /// The Camera pane's sight mode (#379). Not in `OptionSet.csv`'s tab
    /// grouping because no id for it is identified — see [`CameraOptions`].
    #[serde(default)]
    pub camera: CameraOptions,
    /// Where the player left each of the ten windows the original persists
    /// (#302). Not an `OptionSet.csv` group at all — vanilla keeps this in its
    /// own `wndpos.dat`, which we deliberately never read or write; see
    /// [`super::window_positions`].
    #[serde(default)]
    pub windows: WindowPositions,
}

impl GameOptions {
    /// Fold decoded [`OptionRecord`]s onto a default set. Known ids map to their
    /// semantic field; unknown ids (and value/id type mismatches) are ignored.
    pub fn from_records(records: &[OptionRecord]) -> Self {
        let mut opts = Self::default();
        for rec in records {
            opts.apply(*rec);
        }
        opts
    }

    fn apply(&mut self, rec: OptionRecord) {
        let OptionRecord { id, value } = rec;
        match id {
            1..=15 => {
                if let OptionValue::U16(v) = value {
                    self.video.graphic1.quality.insert(id, v);
                }
            }
            101..=115 => {
                if let OptionValue::U16(v) = value {
                    self.video.graphic2.quality.insert(id, v);
                }
            }
            501 => set_u8(&mut self.video.graphic1.display_type, value),
            502 => set_u8(&mut self.video.graphic1.brightness, value),
            503 => set_u32(&mut self.video.graphic1.width, value),
            504 => set_u32(&mut self.video.graphic1.height, value),
            601 => set_u8(&mut self.video.graphic2.display_type, value),
            602 => set_u8(&mut self.video.graphic2.brightness, value),
            603 => set_u32(&mut self.video.graphic2.width, value),
            604 => set_u32(&mut self.video.graphic2.height, value),
            1001 => set_u32(&mut self.audio.bgm_volume, value),
            1002 => set_u32(&mut self.audio.fx_volume, value),
            1003 => set_u32(&mut self.audio.env_volume, value),
            1004 => set_bool(&mut self.audio.bgm_enabled, value),
            1005 => set_bool(&mut self.audio.fx_enabled, value),
            1006 => set_bool(&mut self.audio.env_enabled, value),
            // An imported `SROptionSet.dat` *is* an explicit choice, so it
            // becomes `Some` — but still only for the session it is imported
            // in, like any other flip of this field.
            2015 => set_opt_bool(&mut self.video.window_mode_override, value),
            2001..=2028 => {
                if let OptionValue::Bool(v) = value {
                    self.gameplay.toggles.insert(id, v);
                }
            }
            3101 => set_bool(&mut self.keymap.mouse_shortcut_swapped, value),
            3001..=3099 => {
                if let OptionValue::U32(v) = value {
                    self.keymap.bindings.insert(id, v);
                }
            }
            _ => {}
        }
    }
}

fn set_u8(slot: &mut u8, value: OptionValue) {
    if let OptionValue::U8(v) = value {
        *slot = v;
    }
}
fn set_u32(slot: &mut u32, value: OptionValue) {
    if let OptionValue::U32(v) = value {
        *slot = v;
    }
}
fn set_bool(slot: &mut bool, value: OptionValue) {
    if let OptionValue::Bool(v) = value {
        *slot = v;
    }
}
/// Like [`set_bool`], for a tri-state slot where `None` means "unset". A
/// record that is present in the file is an explicit choice, so it lands as
/// `Some`; a record of the wrong type leaves the slot untouched rather than
/// turning "unset" into a guessed value.
fn set_opt_bool(slot: &mut Option<bool>, value: OptionValue) {
    if let OptionValue::Bool(v) = value {
        *slot = Some(v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #379's acceptance is explicitly "changes behaviour **and** survives a
    /// restart". The restart half is this: the sight mode has to go through the
    /// same `user_settings.yaml` round-trip `persistence.rs` performs, with a
    /// stable spelling, or the radio silently resets on every launch.
    #[test]
    fn the_sight_mode_round_trips_through_the_settings_yaml() {
        for mode in SightMode::ALL {
            let mut options = GameOptions::default();
            options.camera.sight = mode;

            let text = serde_yaml::to_string(&options).expect("options serialize");
            let back: GameOptions = serde_yaml::from_str(&text).expect("options deserialize");

            assert_eq!(back.camera.sight, mode, "{mode:?} did not survive the file");
        }
    }

    /// A `user_settings.yaml` written before #379 has no `camera:` key at all.
    /// It must still load — and land on the mode that is what openroad did
    /// before the pane existed, not on whatever happens to be first.
    #[test]
    fn a_settings_file_without_a_camera_group_still_loads_as_free() {
        let older: GameOptions =
            serde_yaml::from_str("video: {}\naudio: {}\n").expect("an older file still loads");

        assert_eq!(older.camera.sight, SightMode::Free);
    }

    /// The serialized spelling is a file format, so it is pinned rather than
    /// left to whatever `Debug` happens to print.
    #[test]
    fn the_sight_mode_is_stored_under_its_snake_case_name() {
        let text = serde_yaml::to_string(&CameraOptions {
            sight: SightMode::ThirdPerson,
        })
        .expect("serialize");

        assert!(
            text.contains("third_person"),
            "unexpected on-disk spelling: {text}"
        );
    }

    #[test]
    fn from_records_maps_known_ids_and_ignores_unknown() {
        let records = vec![
            OptionRecord {
                id: 1,
                value: OptionValue::U16(42),
            },
            OptionRecord {
                id: 502,
                value: OptionValue::U8(9),
            },
            OptionRecord {
                id: 503,
                value: OptionValue::U32(2560),
            },
            OptionRecord {
                id: 1001,
                value: OptionValue::U32(70),
            },
            OptionRecord {
                id: 1004,
                value: OptionValue::Bool(false),
            },
            OptionRecord {
                id: 2015,
                value: OptionValue::Bool(true),
            },
            OptionRecord {
                id: 2002,
                value: OptionValue::Bool(false),
            },
            OptionRecord {
                id: 3001,
                value: OptionValue::U32(0x41),
            },
            OptionRecord {
                id: 3101,
                value: OptionValue::Bool(true),
            },
            OptionRecord {
                id: 9999, // unknown -> ignored
                value: OptionValue::U32(1),
            },
        ];
        let o = GameOptions::from_records(&records);
        assert_eq!(o.video.graphic1.quality.get(&1), Some(&42));
        assert_eq!(o.video.graphic1.brightness, 9);
        assert_eq!(o.video.graphic1.width, 2560);
        assert_eq!(o.audio.bgm_volume, 70);
        assert!(!o.audio.bgm_enabled);
        assert_eq!(o.video.window_mode_override, Some(true));
        assert_eq!(o.gameplay.toggles.get(&2002), Some(&false));
        assert_eq!(o.keymap.bindings.get(&3001), Some(&0x41));
        assert!(o.keymap.mouse_shortcut_swapped);
    }

    #[test]
    fn yaml_roundtrips_non_default_values() {
        let mut o = GameOptions::default();
        o.audio.bgm_volume = 33;
        o.keymap.bindings.insert(3001, 0x42);
        o.gameplay.toggles.insert(2001, false);
        let yaml = serde_yaml::to_string(&o).expect("serialize");
        let back: GameOptions = serde_yaml::from_str(&yaml).expect("deserialize");
        assert_eq!(o, back);
    }

    /// The window mode is `config.yaml`'s to decide, so the session override
    /// must not ride the file in either direction. Both halves are asserted
    /// because either one alone would let the old bug back in: a written key
    /// would be read back next launch, and a *read* key resurrects the stale
    /// `window_mode: false` that every existing `user_settings.yaml` already
    /// carries.
    #[test]
    fn the_window_mode_override_never_touches_the_settings_yaml() {
        let mut o = GameOptions::default();
        o.video.window_mode_override = Some(true);
        let yaml = serde_yaml::to_string(&o).expect("serialize");
        assert!(
            !yaml.contains("window_mode"),
            "a session-only override must not be written: {yaml}"
        );

        // The legacy spelling every pre-fix file on disk has.
        let back: GameOptions =
            serde_yaml::from_str("video:\n  window_mode: false\n").expect("legacy doc");
        assert_eq!(
            back.video.window_mode_override, None,
            "a stored window_mode must be ignored, not honoured"
        );
    }

    #[test]
    fn empty_document_is_all_defaults() {
        let o: GameOptions = serde_yaml::from_str("{}\n").expect("empty map");
        assert_eq!(o, GameOptions::default());
    }

    #[test]
    fn partial_document_falls_back_to_default() {
        let o: GameOptions =
            serde_yaml::from_str("audio:\n  bgm_volume: 10\n").expect("partial doc");
        assert_eq!(o.audio.bgm_volume, 10);
        // a field missing from the present section keeps its default
        assert!(o.audio.fx_enabled);
        // absent top-level sections default wholesale
        assert_eq!(o.video, VideoOptions::default());
        assert_eq!(o.keymap, KeyMapOptions::default());
    }

    #[test]
    fn default_volume_is_unity_gain() {
        let audio = AudioOptions::default();
        assert_eq!(audio.bgm_playback().unwrap().volume.to_linear(), 1.0);
        assert_eq!(audio.fx_playback().unwrap().volume.to_linear(), 1.0);
    }

    #[test]
    fn zero_volume_is_silent() {
        let mut audio = AudioOptions::default();
        audio.bgm_volume = 0;
        assert_eq!(audio.bgm_playback().unwrap().volume.to_linear(), 0.0);
    }

    #[test]
    fn half_volume_is_half_gain() {
        let mut audio = AudioOptions::default();
        audio.fx_volume = 50;
        assert_eq!(audio.fx_playback().unwrap().volume.to_linear(), 0.5);
    }

    #[test]
    fn disabled_channel_plays_nothing() {
        let mut audio = AudioOptions::default();
        audio.bgm_enabled = false;
        audio.fx_enabled = false;
        assert!(audio.bgm_playback().is_none());
        assert!(audio.fx_playback().is_none());
    }

    #[test]
    fn over_100_volume_clamps_to_unity_gain() {
        let mut audio = AudioOptions::default();
        audio.bgm_volume = 150;
        assert_eq!(audio.bgm_playback().unwrap().volume.to_linear(), 1.0);
    }

    /// Muted BGM is a *paused sink*, not a missing entity (#647): the sink has
    /// to exist, and to remember the intended volume, or turning BGM back on
    /// mid-scene would have nothing to unpause.
    #[test]
    fn muted_bgm_still_produces_paused_playback_settings_at_the_set_volume() {
        let audio = AudioOptions {
            bgm_enabled: false,
            bgm_volume: 40,
            ..AudioOptions::default()
        };
        let settings = audio.bgm_playback_settings();
        assert!(settings.paused, "muted BGM spawns paused");
        assert_eq!(settings.volume.to_linear(), 0.4);
        assert_eq!(audio.bgm_gain().to_linear(), 0.4);
        assert!(audio.bgm_playback().is_none(), "the old gate is unchanged");

        let on = AudioOptions {
            bgm_enabled: true,
            bgm_volume: 40,
            ..AudioOptions::default()
        };
        assert!(!on.bgm_playback_settings().paused);
    }
}
