//! Options -> Audio pane (`OptionsTab::Audio`): three volume groups.
//!
//! Idea: this is the most completely specified pane in the options window and
//! the last empty one. `ifoption_audio.txt` declares 18 blocks = **3 groups x
//! 6 controls**, with the group suffixes `_BGM` / `_EFF` / `_ENV` and a
//! group pitch of exactly 64px:
//!
//! | control | class | BGM | EFF | ENV |
//! |---|---|---|---|---|
//! | frame | `CIFFrame` | `11,10,342,62` | `11,74,..` | `11,138,..` |
//! | label | `CIFStatic` | `18,14,326,12` | `18,78,..` | `18,142,..` |
//! | track art | `CIFStatic` (`opt_volume.ddj`) | `15,37` | `15,101` | `15,165` |
//! | slider | `CIFHScroll_Option` | `38,42,207,16` | `38,106,..` | `38,170,..` |
//! | mute button | `CIFButton` | `275,41,44,12` | `275,105,..` | `275,169,..` |
//! | checkbox | `CIFCheckBox` | `325,42,16,16` | `325,106,..` | `325,170,..` |
//!
//! Note what is *not* here: there is no voice channel, and no `*VOICE*` key
//! exists anywhere in `textuisystem.txt`. Three is the whole feature, so a
//! fourth would be non-original behaviour (`docs/re/ui/options-audio.md` §8.5).
//!
//! **All three channels are live.** `AudioOptions::bgm_playback_settings` and
//! `fx_playback` are read at every playback site (#373/#478) and by
//! `apply_background_music_options` (#647); `env_playback_settings` /
//! `env_gain` / `env_oneshot` drive the zone ambience
//! (`plugins::zone_ambience`, #772). Environment used to render dimmed and
//! inert here because nothing read it — that is no longer true, so the row is
//! drawn like the other two.
//!
//! Stated deviations (ADR-0009):
//!
//! 1. The value range is **ours**. No `Min`/`Max`/`Step` key exists in the
//!    tree's 16-key grammar and `define.txt` carries no volume symbols
//!    (`options-audio.md` §9), so 0..=100 is picked to match `AudioOptions`'
//!    existing `u32` percent scale — a choice, not a transcription.
//! 2. `CIFFrame` draws a nine-slice border from the `opt_inner_box_*` kit
//!    (eight DDJs). We draw the frame's `342x62` rect as a plain bordered
//!    panel; the art is not transcribed.
//! 3. The whole row drags the slider and the whole mute strip toggles it:
//!    vanilla's 44x12 button and 16x16 checkbox are both under the WCAG 2.2 AA
//!    24x24 target minimum (§8.7). No vanilla geometry moves.

use bevy::ecs::relationship::RelatedSpawnerCommands;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui_widgets::{Activate, Button};

use crate::plugins::settings::options::{AudioOptions, GameOptions};
use crate::plugins::textdata::ClientUiStrings;

/// `interface\ifcommon\com_radiobutton_off.ddj` — vanilla uses the *radio*
/// asset for a mute toggle. That is the original's own choice, not a slip
/// (`docs/re/ui/options-audio.md` §3).
const CHECK_OFF: &str = "media://interface/ifcommon/com_radiobutton_off.ddj";
const CHECK_ON: &str = "media://interface/ifcommon/com_radiobutton_on.ddj";
/// `interface\option\opt_volume.ddj`, the slider track art (248x28).
const TRACK_ART: &str = "media://interface/option/opt_volume.ddj";

/// Group pitch, verified identical across all six control columns.
const GROUP_PITCH: f32 = 64.0;
/// First group's y for each control, pane-local. Groups 2 and 3 are these
/// plus one and two [`GROUP_PITCH`]s.
const FRAME_XYWH: (f32, f32, f32, f32) = (11.0, 10.0, 342.0, 62.0);
const LABEL_XYWH: (f32, f32, f32, f32) = (18.0, 14.0, 326.0, 12.0);
const TRACK_ART_XY: (f32, f32) = (15.0, 37.0);
const SLIDER_XYWH: (f32, f32, f32, f32) = (38.0, 42.0, 207.0, 16.0);
const MUTE_XYWH: (f32, f32, f32, f32) = (275.0, 41.0, 44.0, 12.0);
const CHECK_XYWH: (f32, f32, f32, f32) = (325.0, 42.0, 16.0, 16.0);

/// Our range, not the data's (see the module comment). Matches the `u32`
/// percent scale `AudioOptions` already stores and `gain()` already divides by.
const VOLUME_MAX: f32 = 100.0;

const LABEL_COLOR: Color = Color::srgb_u8(255, 255, 255);
/// Dimmed, exactly as `options_game.rs` dims a row nothing reads yet.
const INERT_COLOR: Color = Color::srgb_u8(128, 128, 128);
const FRAME_BORDER: Color = Color::srgba(1.0, 1.0, 1.0, 0.18);
const FILL_COLOR: Color = Color::srgb_u8(240, 217, 165);
const FILL_INERT: Color = Color::srgb_u8(96, 96, 96);

/// One of the original's three audio channels.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum AudioChannel {
    /// `UIIT_STT_BGMSETTING`, ids 19/16/13.
    Bgm,
    /// `UIIT_STT_EFFSETTING`, ids 20/17/14.
    Effect,
    /// `UIIT_STT_ENVIRONMENT`, ids 21/18/15. Consumed by the zone ambience
    /// (`plugins::zone_ambience`, #772).
    Environment,
}

impl AudioChannel {
    /// Top to bottom, which is also `_BGM` / `_EFF` / `_ENV`.
    pub(crate) const ALL: [AudioChannel; 3] = [
        AudioChannel::Bgm,
        AudioChannel::Effect,
        AudioChannel::Environment,
    ];

    fn label(self) -> (&'static str, &'static str) {
        match self {
            AudioChannel::Bgm => ("UIIT_STT_BGMSETTING", "Background Music"),
            AudioChannel::Effect => ("UIIT_STT_EFFSETTING", "Sound Effect"),
            AudioChannel::Environment => ("UIIT_STT_ENVIRONMENT", "Environment"),
        }
    }

    /// Whether a change to this channel reaches any audio in this build.
    ///
    /// All three are `true` since #772 gave the environment channel its
    /// consumer; the flag stays because the pane's dimming path is what tells
    /// a future dead row apart from a live one.
    pub(crate) fn is_live(self) -> bool {
        true
    }

    fn volume(self, audio: &AudioOptions) -> u32 {
        match self {
            AudioChannel::Bgm => audio.bgm_volume,
            AudioChannel::Effect => audio.fx_volume,
            AudioChannel::Environment => audio.env_volume,
        }
    }

    fn set_volume(self, audio: &mut AudioOptions, value: u32) {
        let value = value.min(VOLUME_MAX as u32);
        match self {
            AudioChannel::Bgm => audio.bgm_volume = value,
            AudioChannel::Effect => audio.fx_volume = value,
            AudioChannel::Environment => audio.env_volume = value,
        }
    }

    pub(crate) fn enabled(self, audio: &AudioOptions) -> bool {
        match self {
            AudioChannel::Bgm => audio.bgm_enabled,
            AudioChannel::Effect => audio.fx_enabled,
            AudioChannel::Environment => audio.env_enabled,
        }
    }

    fn toggle(self, audio: &mut AudioOptions) {
        match self {
            AudioChannel::Bgm => audio.bgm_enabled = !audio.bgm_enabled,
            AudioChannel::Effect => audio.fx_enabled = !audio.fx_enabled,
            AudioChannel::Environment => audio.env_enabled = !audio.env_enabled,
        }
    }
}

/// The filled part of a channel's track.
#[derive(Component, Clone, Copy)]
pub(crate) struct VolumeFill(pub AudioChannel);

/// A channel's mute checkbox image.
#[derive(Component, Clone, Copy)]
pub(crate) struct MuteBox(pub AudioChannel);

/// The volume a drag started from, so the drag is relative and never jumps.
#[derive(Component, Clone, Copy)]
struct DragStartVolume(u32);

fn offset(base: (f32, f32, f32, f32), index: usize) -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: Val::Px(base.0),
        top: Val::Px(base.1 + GROUP_PITCH * index as f32),
        width: Val::Px(base.2),
        height: Val::Px(base.3),
        ..default()
    }
}

/// Build the Audio pane into an already-positioned pane node.
pub(crate) fn spawn_audio_pane(
    pane: &mut RelatedSpawnerCommands<ChildOf>,
    asset_server: &AssetServer,
    font: &Handle<Font>,
    ui_strings: &ClientUiStrings,
    options: &GameOptions,
) {
    let track: Handle<Image> = asset_server.load(TRACK_ART);
    let off: Handle<Image> = asset_server.load(CHECK_OFF);
    let on: Handle<Image> = asset_server.load(CHECK_ON);

    for (index, channel) in AudioChannel::ALL.into_iter().enumerate() {
        let live = channel.is_live();
        let text_color = if live { LABEL_COLOR } else { INERT_COLOR };

        let mut frame = offset(FRAME_XYWH, index);
        frame.border = UiRect::all(Val::Px(1.0));
        pane.spawn((frame, BorderColor::all(FRAME_BORDER), Pickable::IGNORE));

        let (key, english) = channel.label();
        pane.spawn((
            Text::new(ui_strings.get_or(key, english).to_string()),
            TextFont {
                font: font.clone().into(),
                font_size: FontSize::Px(11.0),
                ..default()
            },
            TextColor(text_color),
            offset(LABEL_XYWH, index),
            Pickable::IGNORE,
        ));

        pane.spawn((
            ImageNode {
                image: track.clone(),
                ..default()
            },
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(TRACK_ART_XY.0),
                top: Val::Px(TRACK_ART_XY.1 + GROUP_PITCH * index as f32),
                ..default()
            },
            Pickable::IGNORE,
        ));

        spawn_slider(pane, channel, index, live, &options.audio);
        spawn_mute(
            pane, font, ui_strings, channel, index, live, &off, &on, options,
        );
    }
}

/// The `CIFHScroll_Option` track. Vanilla drags a thumb; we drag the whole
/// 207x16 track, which is the same gesture with a target big enough to hit.
fn spawn_slider(
    pane: &mut RelatedSpawnerCommands<ChildOf>,
    channel: AudioChannel,
    index: usize,
    live: bool,
    audio: &AudioOptions,
) {
    let mut slider = pane.spawn((
        offset(SLIDER_XYWH, index),
        Pickable::default(),
        Hovered::default(),
    ));
    slider.observe(
        move |_: On<Pointer<DragStart>>,
              options: Res<GameOptions>,
              mut commands: Commands,
              sliders: Query<Entity, With<VolumeTrack>>| {
            let start = channel.volume(&options.audio);
            for entity in &sliders {
                commands.entity(entity).insert(DragStartVolume(start));
            }
        },
    );
    slider.observe(
        move |drag: On<Pointer<Drag>>,
              starts: Query<&DragStartVolume>,
              mut options: ResMut<GameOptions>| {
            let Ok(start) = starts.get(drag.entity) else {
                return;
            };
            let travelled = drag.event.distance.x / SLIDER_XYWH.2 * VOLUME_MAX;
            let next = (start.0 as f32 + travelled).clamp(0.0, VOLUME_MAX);
            channel.set_volume(&mut options.audio, next.round() as u32);
        },
    );
    slider.insert(VolumeTrack(channel));
    slider.with_children(|track| {
        track.spawn((
            VolumeFill(channel),
            Node {
                width: Val::Percent(fill_percent(channel, audio)),
                height: Val::Percent(100.0),
                ..default()
            },
            BackgroundColor(if live { FILL_COLOR } else { FILL_INERT }),
            Pickable::IGNORE,
        ));
    });
}

/// Marks a draggable track so a `DragStart` can seed every slider's baseline.
#[derive(Component, Clone, Copy)]
struct VolumeTrack(#[allow(dead_code)] AudioChannel);

fn fill_percent(channel: AudioChannel, audio: &AudioOptions) -> f32 {
    (channel.volume(audio) as f32 / VOLUME_MAX * 100.0).clamp(0.0, 100.0)
}

/// The `CIFButton` ("Off") and `CIFCheckBox` are two affordances whose
/// relationship the grammar does not express (§9). We treat them as one
/// control — the strip from the button's left edge to the box's right edge
/// toggles the channel — because two independent mute states for one channel
/// is the reading that cannot be right.
#[allow(clippy::too_many_arguments)]
fn spawn_mute(
    pane: &mut RelatedSpawnerCommands<ChildOf>,
    font: &Handle<Font>,
    ui_strings: &ClientUiStrings,
    channel: AudioChannel,
    index: usize,
    live: bool,
    off: &Handle<Image>,
    on: &Handle<Image>,
    options: &GameOptions,
) {
    let top = MUTE_XYWH.1 + GROUP_PITCH * index as f32;
    let width = CHECK_XYWH.0 + CHECK_XYWH.2 - MUTE_XYWH.0;
    let mut strip = pane.spawn((
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(MUTE_XYWH.0),
            top: Val::Px(top),
            width: Val::Px(width),
            height: Val::Px(CHECK_XYWH.3 + 1.0),
            ..default()
        },
        Button,
        Hovered::default(),
        Pickable::default(),
    ));
    strip.observe(move |_: On<Activate>, mut options: ResMut<GameOptions>| {
        channel.toggle(&mut options.audio);
    });
    strip.with_children(|s| {
        s.spawn((
            Text::new(
                ui_strings
                    .get_or("UIIT_STT_SOUND_ELEMINATE", "Off")
                    .to_string(),
            ),
            TextFont {
                font: font.clone().into(),
                font_size: FontSize::Px(10.0),
                ..default()
            },
            TextColor(if live { LABEL_COLOR } else { INERT_COLOR }),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Px(MUTE_XYWH.2),
                height: Val::Px(MUTE_XYWH.3),
                ..default()
            },
            Pickable::IGNORE,
        ));

        let image = if channel.enabled(&options.audio) {
            off.clone()
        } else {
            on.clone()
        };
        s.spawn((
            MuteBox(channel),
            ImageNode { image, ..default() },
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(CHECK_XYWH.0 - MUTE_XYWH.0),
                top: Val::Px(CHECK_XYWH.1 - MUTE_XYWH.1),
                width: Val::Px(CHECK_XYWH.2),
                height: Val::Px(CHECK_XYWH.3),
                ..default()
            },
            Pickable::IGNORE,
        ));
    });
}

/// Repaint the fills and the mute boxes from `GameOptions`.
///
/// The checkbox is checked when the channel is **muted**: the control the box
/// belongs to is labelled "Off" (`UIIT_STT_SOUND_ELEMINATE`), so a tick means
/// "this channel is off", not "this channel is on".
pub(crate) fn refresh_audio_rows(
    options: Res<GameOptions>,
    asset_server: Res<AssetServer>,
    mut fills: Query<(&VolumeFill, &mut Node)>,
    mut boxes: Query<(&MuteBox, &mut ImageNode)>,
) {
    if !options.is_changed() {
        return;
    }
    for (fill, mut node) in &mut fills {
        node.width = Val::Percent(fill_percent(fill.0, &options.audio));
    }
    for (mute, mut image) in &mut boxes {
        let art = if mute.0.enabled(&options.audio) {
            CHECK_OFF
        } else {
            CHECK_ON
        };
        image.image = asset_server.load(art);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every rect is a transcription of `ifoption_audio.txt`, and the one
    /// property that ties the three groups together is the 64px pitch — it is
    /// identical across all six control columns, which is what makes a single
    /// `offset(base, index)` legitimate instead of eighteen constants.
    #[test]
    fn the_group_pitch_is_the_same_in_every_column() {
        assert_eq!(GROUP_PITCH, 64.0);
        for (base, expected) in [
            (FRAME_XYWH.1, [10.0, 74.0, 138.0]),
            (LABEL_XYWH.1, [14.0, 78.0, 142.0]),
            (TRACK_ART_XY.1, [37.0, 101.0, 165.0]),
            (SLIDER_XYWH.1, [42.0, 106.0, 170.0]),
            (MUTE_XYWH.1, [41.0, 105.0, 169.0]),
            (CHECK_XYWH.1, [42.0, 106.0, 170.0]),
        ] {
            for (index, want) in expected.into_iter().enumerate() {
                assert_eq!(base + GROUP_PITCH * index as f32, want);
            }
        }
    }

    /// Three channels, and only three: there is no voice group in the tree and
    /// no `*VOICE*` key in the string table, so a fourth would be invented
    /// behaviour rather than a missing row.
    #[test]
    fn there_are_exactly_three_channels_and_none_is_a_voice_channel() {
        assert_eq!(AudioChannel::ALL.len(), 3);
        for channel in AudioChannel::ALL {
            let (key, _) = channel.label();
            assert!(!key.contains("VOICE"), "{key} invents a voice channel");
        }
    }

    /// The pane must not claim a channel works before it does — and must not
    /// keep claiming one is dead after it works. Environment became live with
    /// #772 (`plugins::zone_ambience` reads `env_volume`/`env_enabled`), so
    /// all three rows now render live.
    #[test]
    fn every_channel_is_live_now_that_the_environment_one_has_a_consumer() {
        for channel in AudioChannel::ALL {
            assert!(channel.is_live(), "{channel:?} renders dimmed");
        }
    }

    /// Each channel must read and write *its own* field. A copy-paste slip
    /// here would silently cross two channels' volumes.
    #[test]
    fn every_channel_reads_and_writes_its_own_field() {
        for channel in AudioChannel::ALL {
            let mut audio = AudioOptions::default();
            channel.set_volume(&mut audio, 42);

            assert_eq!(channel.volume(&audio), 42);
            for other in AudioChannel::ALL {
                if other != channel {
                    assert_eq!(channel.volume(&audio), 42);
                    assert_ne!(other.volume(&audio), 42, "{other:?} moved with {channel:?}");
                }
            }
        }
    }

    /// The mute toggle is per channel too, and starts from the shipped
    /// "everything on" default.
    #[test]
    fn muting_one_channel_leaves_the_others_playing() {
        for channel in AudioChannel::ALL {
            let mut audio = AudioOptions::default();
            assert!(channel.enabled(&audio), "the default is every channel on");

            channel.toggle(&mut audio);
            assert!(!channel.enabled(&audio));
            for other in AudioChannel::ALL {
                if other != channel {
                    assert!(other.enabled(&audio), "{other:?} followed {channel:?}");
                }
            }
        }
    }

    /// The fill is what the player reads the volume off, so it has to track
    /// the stored value across the whole of our stated 0..=100 range.
    #[test]
    fn the_fill_spans_the_whole_range_and_never_leaves_it() {
        let mut audio = AudioOptions::default();
        for (value, want) in [(0u32, 0.0f32), (50, 50.0), (100, 100.0)] {
            AudioChannel::Bgm.set_volume(&mut audio, value);
            assert_eq!(fill_percent(AudioChannel::Bgm, &audio), want);
        }

        // A value above the range is clamped on the way in, not on the way out.
        AudioChannel::Bgm.set_volume(&mut audio, 500);
        assert_eq!(AudioChannel::Bgm.volume(&audio), 100);
        assert_eq!(fill_percent(AudioChannel::Bgm, &audio), 100.0);
    }
}
