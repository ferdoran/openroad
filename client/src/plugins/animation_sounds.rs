//! Animation-linked sound effects from the `.bsr` mod palette (Sound
//! ModData, `0x0005 0000`).
//!
//! Idea: the original binds SFX to *animation keytimes*, not to gameplay
//! events — a mob's death thud is "347 ms into the die clip of the default
//! group", a footstep is "N ms into the run clip" (`docs/re/formats/
//! moddata-unhandled.md` §3: 615 files, 45,086 tracks). The .bsr loader
//! keeps those tracks per (group, animation type) in
//! [`AnimationSounds`](crate::commands::AnimationSounds); this module is the
//! player.
//!
//! Rather than scheduling timers when an animation starts (what the particle
//! side does, [`sync_animation_effects`](crate::plugins::effects::spawn)),
//! this samples the [`AnimationPlayer`]'s own seek time each frame and fires
//! a track when the clip's playhead crosses its keytime. That is what makes
//! a **looping** clip work: run/walk animations repeat, so their footsteps
//! have to fire once per lap, and a one-shot timer at clip start would play
//! them exactly once.
//!
//! Playback is **positional** (#774): the sound entity is a child of the
//! emitter, so it inherits its `GlobalTransform`, and it plays through
//! rodio's spatial mixer against the [`SpatialListener`] on the game camera.
//! The distance curve comes from the mod palette's own DS3D parameters —
//! `[3] = 10.0` / `[4] = 100.0`, constant across the corpus
//! (`docs/re/formats/moddata-unhandled.md` §3) — mapped onto the mixer by
//! [`SPATIAL_SCALE`]. An emitter without a `GlobalTransform` falls back to the
//! old non-positional spawn rather than warning once per sound.
//!
//! Two deviations (ADR-0009), both deliberate:
//!
//! * **Every animation sound is 3D, not just the flagged ones.** The original
//!   marks only 533 of its 22,051 sound buffers `0x90` (CTRL3D|CTRLVOLUME);
//!   the rest are `0xC0` (CTRLPAN|CTRLVOLUME, 2D). Those flags live in the
//!   Sound entry's 11-dword config header, which our track scanner
//!   (`assets::bsr::bsr::parse_sound_mods`) deliberately does not walk — it
//!   anchors on the innermost track. Playing a mob's footstep from the mob is
//!   what a player expects; honouring the per-entry flag needs the scanner to
//!   retain the header first and is a follow-up, not a reason to stay flat.
//! * **The rolloff is inverse-square, not DirectSound's inverse.** rodio 0.22
//!   attenuates each ear by `min(1/d², 1)` in scaled units and that is not
//!   configurable (`rodio::source::Spatial::set_positions`). With
//!   [`SPATIAL_SCALE`] the sound is at full level inside the palette's 10-unit
//!   min distance — exactly the DS3D min-distance semantics — and is 40 dB
//!   down at its 100-unit max distance where DirectSound would be 20 dB down.
//!   Quieter far away, same near field.

use bevy::animation::graph::AnimationNodeIndex;
use bevy::animation::{ActiveAnimation, AnimationPlayer};
use bevy::app::{App, Plugin, Update};
use bevy::asset::AssetServer;
use bevy::audio::{AudioPlayer, PlaybackSettings, SpatialScale};
use bevy::ecs::hierarchy::ChildOf;
use bevy::prelude::{
    Commands, Component, Entity, GlobalTransform, Name, Query, Res, Transform, Without,
};

use crate::commands::{AnimationLibrary, AnimationSounds};
use crate::plugins::animation_culling::PausedAnimationGraph;
use crate::plugins::settings::options::GameOptions;

/// DS3D minimum distance of every 3D sound entry in the mod palette: inside
/// it the sound plays at full level. `[3] = 10.0`, constant over the whole
/// corpus (`docs/re/formats/moddata-unhandled.md` §3).
pub const SOUND_MIN_DISTANCE: f32 = 10.0;

/// DS3D maximum distance, `[4] = 100.0`, equally constant. Not a hard cut-off
/// here — it is the distance at which the original considered a sound spent,
/// and it is what [`spatial_gain`] is documented against.
pub const SOUND_MAX_DISTANCE: f32 = 100.0;

/// World units → mixer units. rodio holds a source at full volume while its
/// scaled distance is <= 1, so scaling by `1 / SOUND_MIN_DISTANCE` puts the
/// unity-gain radius exactly on the palette's min distance instead of on an
/// invented number.
pub const SPATIAL_SCALE: f32 = 1.0 / SOUND_MIN_DISTANCE;

/// Distance between the listener's ears, in world units. SRO world units are
/// decimetres (`docs/formats/textdata-itemdata.md`: weapon range is in
/// decimetres; a character is ~18 units tall), so 2.0 units = 20 cm — human
/// ear separation. rodio uses the gap only to normalize the left/right
/// panning term, so this sets how wide the stereo image is, not the volume.
pub const LISTENER_EAR_GAP: f32 = 2.0;

/// Gain rodio's spatial mixer ends up applying at `distance` world units,
/// ignoring the small left/right panning term: `min(1/d_scaled², 1)` with
/// `d_scaled = distance * SPATIAL_SCALE`
/// (`rodio-0.22.2/src/source/spatial.rs:set_positions`).
///
/// It exists so the curve this module ships is stated and testable rather
/// than being an emergent property of two libraries' defaults.
pub fn spatial_gain(distance: f32) -> f32 {
    let scaled = (distance.max(0.0) * SPATIAL_SCALE).max(f32::MIN_POSITIVE);
    (1.0 / (scaled * scaled)).min(1.0)
}

/// Playhead bookkeeping for [`AnimationSounds`]: which clip was playing last
/// frame and how far into it we already fired tracks.
#[derive(Component, Default)]
pub struct AnimationSoundCursor {
    node: Option<AnimationNodeIndex>,
    last_ms: u32,
}

/// Whether a track at `key_time_ms` becomes due this frame.
///
/// `previous_ms` is the playhead position the last frame fired up to, and
/// `now_ms` the current one. A clip that just started (or changed) has no
/// previous position and fires everything up to the current playhead, which
/// is the 0 ms tracks in practice. A wrapped playhead (`now < previous`)
/// means the clip looped, so the tail of the lap and the head of the new one
/// are both due.
pub fn sound_track_is_due(previous_ms: Option<u32>, now_ms: u32, key_time_ms: u32) -> bool {
    match previous_ms {
        None => key_time_ms <= now_ms,
        Some(previous_ms) if now_ms >= previous_ms => {
            key_time_ms > previous_ms && key_time_ms <= now_ms
        }
        Some(previous_ms) => key_time_ms > previous_ms || key_time_ms <= now_ms,
    }
}

/// Fires the tracks of the playing animation whose keytime the playhead
/// crossed this frame.
pub fn play_animation_sounds(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    options: Res<GameOptions>,
    // `Without<PausedAnimationGraph>` is the same distance gate
    // `AnimationCullingPlugin` applies to the animation itself. That plugin
    // freezes a distant rig by removing its `AnimationGraphHandle`, but leaves
    // `AnimationPlayer` in place — so without this filter the loop below still
    // ran the `library.entries` scan, with an `is_playing_animation` lookup per
    // entry, for every animated prop out to the terrain *unload* boundary, and
    // found nothing every time. A culled rig is not animating, so it has no
    // keytimes to cross.
    mut wrappers: Query<
        (
            Entity,
            &AnimationPlayer,
            &AnimationLibrary,
            &AnimationSounds,
            &mut AnimationSoundCursor,
            Option<&GlobalTransform>,
        ),
        Without<PausedAnimationGraph>,
    >,
) {
    let playback = options.audio.fx_playback();

    for (entity, player, library, sounds, mut cursor, emitter_transform) in &mut wrappers {
        let playing = library
            .entries
            .iter()
            .find(|entry| player.is_playing_animation(entry.node));
        let Some(entry) = playing else {
            cursor.node = None;
            continue;
        };
        let Some(active) = player.animation(entry.node) else {
            cursor.node = None;
            continue;
        };

        let now_ms = seek_time_ms(active);
        let previous_ms = (cursor.node == Some(entry.node)).then_some(cursor.last_ms);
        cursor.node = Some(entry.node);
        cursor.last_ms = now_ms;

        // muted FX still advances the cursor, so unmuting does not replay
        // everything the clip passed while it was silent
        let Some(playback) = playback else { continue };
        let Some(tracks) = sounds.0.get(&(entry.group.clone(), entry.anim_type)) else {
            continue;
        };

        // Positional only when the emitter has a transform to inherit; the
        // sound entity needs its own `Transform` for propagation to give it a
        // `GlobalTransform` at all, and rodio warns and plays from the origin
        // when a spatial source has none.
        let playback = PlaybackSettings {
            spatial: emitter_transform.is_some(),
            spatial_scale: Some(SpatialScale::new(SPATIAL_SCALE)),
            ..playback
        };

        for track in tracks {
            if !sound_track_is_due(previous_ms, now_ms, track.key_time_ms) {
                continue;
            }
            commands.spawn((
                AudioPlayer::new(asset_server.load(format!("data://{}", track.path))),
                playback,
                Transform::default(),
                Name::new(format!("anim sound: {}", track.path)),
                ChildOf(entity),
            ));
        }
    }
}

/// The active animation's playhead in milliseconds.
fn seek_time_ms(active: &ActiveAnimation) -> u32 {
    (active.seek_time().max(0.0) * 1000.0) as u32
}

pub struct AnimationSoundsPlugin;

impl Plugin for AnimationSoundsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, play_animation_sounds);
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// A clip that just started fires its 0 ms tracks and nothing later.
    #[test]
    fn sound_track_fires_on_animation_start() {
        assert!(sound_track_is_due(None, 0, 0));
        assert!(!sound_track_is_due(None, 0, 347));
        // a clip entered mid-way (blend/seek) does not replay its whole past
        // twice: everything up to the playhead fires exactly once
        assert!(sound_track_is_due(None, 400, 347));
        assert!(!sound_track_is_due(Some(400), 500, 347));
    }

    /// The tombstone's death thud (347 ms) fires exactly once per lap of a
    /// looping clip — the reason this is a playhead crossing and not a timer
    /// started at clip start.
    #[test]
    fn sound_track_is_due_once_per_loop() {
        assert!(!sound_track_is_due(Some(100), 300, 347));
        assert!(sound_track_is_due(Some(300), 400, 347));
        assert!(!sound_track_is_due(Some(400), 900, 347));
        // playhead wrapped (1000 ms clip): the new lap crosses 347 again
        assert!(sound_track_is_due(Some(900), 400, 347));
        // ... and a track before the wrap point is not fired twice
        assert!(!sound_track_is_due(Some(900), 100, 347));
        assert!(sound_track_is_due(Some(900), 100, 950));
    }

    /// The distance curve, at the four distances the palette actually names.
    /// Origin of 10/100: the DS3D min/max distance dwords of every Sound
    /// ModData entry (`docs/re/formats/moddata-unhandled.md` §3).
    #[test]
    fn spatial_gain_is_flat_inside_the_min_distance() {
        assert_eq!(spatial_gain(0.0), 1.0);
        assert_eq!(spatial_gain(SOUND_MIN_DISTANCE / 2.0), 1.0);
        assert_eq!(spatial_gain(SOUND_MIN_DISTANCE), 1.0);
    }

    #[test]
    fn spatial_gain_falls_off_inverse_square_beyond_it() {
        // 2x the min distance -> a quarter of the power
        assert!((spatial_gain(2.0 * SOUND_MIN_DISTANCE) - 0.25).abs() < 1e-6);
        // at the palette's max distance the sound is 40 dB down (0.01), the
        // stated deviation from DirectSound's inverse curve (0.1 there)
        assert!((spatial_gain(SOUND_MAX_DISTANCE) - 0.01).abs() < 1e-6);
        // and it keeps falling past it — no cut-off, no revival
        assert!(spatial_gain(2.0 * SOUND_MAX_DISTANCE) < spatial_gain(SOUND_MAX_DISTANCE));
    }

    #[test]
    fn spatial_gain_is_monotonic_and_bounded() {
        let mut previous = f32::INFINITY;
        for step in 0..40 {
            let gain = spatial_gain(step as f32 * 10.0);
            assert!((0.0..=1.0).contains(&gain), "gain {gain} out of range");
            assert!(gain <= previous, "gain rose at {step}");
            previous = gain;
        }
        // a negative/garbage distance is clamped, never NaN or infinite
        assert_eq!(spatial_gain(-5.0), 1.0);
    }
}
