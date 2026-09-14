//! Zone ambience: the `effectenvsnd.txt` day/night ambient lists of the zone
//! the player is standing in, played on the **environment** channel (EP-22.2,
//! #772). Sibling of `zone_bgm` (#771), which plays the same table's BGM.
//!
//! Idea: a zone's ambience is two kinds of sound in one list, and the table
//! distinguishes them with the repeat range it writes after every file:
//!
//! ```text
//!     <2> 낮
//!             <3> "night_wind.wav"      0~0     <- the bed: loop it
//!             <3> "donhwang_wind04.wav" 15~40   <- a one-shot every 15-40 s
//! ```
//!
//! `0~0` is a continuous bed and everything else is a randomly repeating
//! one-shot. That reading is a **stated inference** (ADR-0009), not a
//! documented fact: in every block of the shipped table the `0~0` entries come
//! first and a bed is never ranged, and a "repeat every 0 to 0 seconds" is the
//! only alternative reading — which would be a machine gun, not weather.
//!
//! What is deliberately *not* invented here: the day/night switch reuses the
//! existing world clock (`environment::TimeOfDay`) through the same
//! `sun_direction` the sky uses, so there is exactly one time source in the
//! client; and the zone lookup reuses `ClientZoneSounds` with the same region
//! gate as `zone_bgm`, so a sector crossing inside one zone does not restart
//! the ambience.
//!
//! Deviation (ADR-0009): the ambience is **non-positional**. The table gives a
//! zone, not a source position, so there is nothing to pan against — the
//! positional pass (#774) is about entity-emitted sounds.

use bevy::prelude::*;
use rand::Rng;

use crate::assets::textdata::zonesound::{Ambient, TimeOfDay as AmbientPhase, ZoneSound};
use crate::plugins::environment::celestial::sun_direction;
use crate::plugins::environment::TimeOfDay;
use crate::plugins::hud::minimap::MinimapDungeonContext;
use crate::plugins::map::terrain::REGION_SIZE;
use crate::plugins::player::Player;
use crate::plugins::settings::options::GameOptions;
use crate::plugins::textdata::ClientZoneSounds;
use crate::plugins::world_origin::WorldOrigin;
use crate::scenes::SceneState;

/// Marker for a looping ambience bed of the current zone.
#[derive(Component)]
pub struct ZoneAmbienceBed;

/// A ranged entry of the current zone's list, with the time its next play is
/// due (seconds on `Time::elapsed_secs`).
#[derive(Debug, Clone, PartialEq)]
pub struct ScheduledAmbient {
    pub path: String,
    pub min_secs: u32,
    pub max_secs: u32,
    pub next_at: f32,
}

/// What is currently sounding, and the schedule of the ranged entries.
#[derive(Resource, Default)]
pub struct ZoneAmbienceState {
    /// Change detector for the zone lookup, exactly as in `zone_bgm`.
    pub region: Option<u16>,
    pub zone: Option<String>,
    pub phase: Option<AmbientPhase>,
    pub scheduled: Vec<ScheduledAmbient>,
}

/// Day while the sun is above the horizon, night below — read off the same
/// `sun_direction` curve the sky and the celestial bodies use, so the ambience
/// can never disagree with what the player sees.
pub fn phase_at(t: f32) -> AmbientPhase {
    if sun_direction(t).y > 0.0 {
        AmbientPhase::Day
    } else {
        AmbientPhase::Night
    }
}

/// When the ambience has to be rebuilt: a different zone, or the same zone
/// crossing into its other list. `None` for either side means "nothing is
/// playing"/"no zone here", both of which are changes only if the other side
/// says something else.
pub fn ambience_changed(
    playing: (Option<&str>, Option<AmbientPhase>),
    zone: Option<&ZoneSound>,
    phase: AmbientPhase,
) -> bool {
    playing.0 != zone.map(|z| z.name.as_str()) || playing.1 != Some(phase)
}

/// Next due time of a ranged entry: `now` plus a uniform draw from its own
/// `min..=max` window. `roll` returns a 0..=1 fraction, so the scheduler is
/// testable without a rng.
pub fn next_at(entry_min: u32, entry_max: u32, now: f32, roll: f32) -> f32 {
    let (min, max) = (
        entry_min.min(entry_max) as f32,
        entry_max.max(entry_min) as f32,
    );
    now + min + (max - min) * roll.clamp(0.0, 1.0)
}

/// The ranged entries of a list, scheduled from `now`. Continuous (`0~0`)
/// entries are **not** in here: they are beds, they loop, and a bed that ever
/// gets rescheduled would restart mid-note.
pub fn schedule(
    ambients: &[Ambient],
    now: f32,
    mut roll: impl FnMut() -> f32,
) -> Vec<ScheduledAmbient> {
    ambients
        .iter()
        .filter(|a| !a.is_continuous())
        .map(|a| ScheduledAmbient {
            path: a.asset_path(),
            min_secs: a.min_secs,
            max_secs: a.max_secs,
            next_at: next_at(a.min_secs, a.max_secs, now, roll()),
        })
        .collect()
}

/// Sector-local SRO coordinates, same convention as `zone_bgm::sector_local`
/// (SRO X grows opposite render X).
fn sector_local(sro: Vec3) -> (f32, f32) {
    (
        (-sro.x).rem_euclid(REGION_SIZE),
        sro.z.rem_euclid(REGION_SIZE),
    )
}

/// Restarts the ambience when the player changes zone or the world clock
/// crosses dawn/dusk.
pub fn update_zone_ambience(
    zone_sounds: Res<ClientZoneSounds>,
    origin: Res<WorldOrigin>,
    options: Res<GameOptions>,
    time_of_day: Res<TimeOfDay>,
    time: Res<Time>,
    asset_server: Res<AssetServer>,
    dungeon: Option<Res<MinimapDungeonContext>>,
    player: Query<&Transform, With<Player>>,
    beds: Query<Entity, With<ZoneAmbienceBed>>,
    mut state: ResMut<ZoneAmbienceState>,
    mut commands: Commands,
) {
    let phase = phase_at(time_of_day.t);
    let located = match &dungeon {
        Some(ctx) => Some((ctx.region_id, None)),
        None => player.single().ok().and_then(|tf| {
            let sro = origin.to_sro(tf.translation);
            crate::plugins::hud::region_banner::overworld_region_id(-sro.x, sro.z)
                .map(|region| (region, Some(sector_local(sro))))
        }),
    };
    let Some((region, local)) = located else {
        return;
    };
    // The cheap gate: same sector and same phase cannot change anything.
    if state.region == Some(region) && state.phase == Some(phase) {
        return;
    }
    state.region = Some(region);

    let zone = match local {
        Some((x, z)) => zone_sounds.zone_at(region, x, z),
        None => zone_sounds.zone_for_region(region),
    };
    if !ambience_changed((state.zone.as_deref(), state.phase), zone, phase) {
        return;
    }

    for entity in beds.iter() {
        commands.entity(entity).despawn();
    }
    state.phase = Some(phase);
    state.zone = zone.map(|z| z.name.clone());

    let Some(zone) = zone else {
        state.scheduled.clear();
        return;
    };
    let ambients = zone.ambients(phase);
    let now = time.elapsed_secs();
    let mut rng = rand::rng();
    state.scheduled = schedule(ambients, now, || rng.random::<f32>());

    let beds = ambients.iter().filter(|a| a.is_continuous());
    let mut bed_count = 0;
    for bed in beds {
        commands.spawn((
            AudioPlayer::new(asset_server.load(bed.asset_path())),
            options.audio.env_playback_settings(),
            ZoneAmbienceBed,
            Name::from("Zone Ambience Bed"),
        ));
        bed_count += 1;
    }
    info!(
        "zone {} ({:?}): {bed_count} ambience bed(s), {} scheduled one-shot(s)",
        zone.name,
        phase,
        state.scheduled.len()
    );
}

/// Fires the ranged entries whose window has elapsed and rolls their next one.
pub fn fire_scheduled_ambients(
    time: Res<Time>,
    options: Res<GameOptions>,
    asset_server: Res<AssetServer>,
    mut state: ResMut<ZoneAmbienceState>,
    mut commands: Commands,
) {
    if state.scheduled.is_empty() {
        return;
    }
    let now = time.elapsed_secs();
    // A muted channel still advances the schedule (same reasoning as the
    // animation-sound cursor): unmuting must not dump every missed one-shot.
    let playback = options.audio.env_oneshot();
    let mut rng = rand::rng();
    for entry in state.scheduled.iter_mut() {
        if now < entry.next_at {
            continue;
        }
        if let Some(playback) = playback {
            commands.spawn((
                AudioPlayer::new(asset_server.load(entry.path.clone())),
                playback,
                Name::new(format!("zone ambience: {}", entry.path)),
            ));
        }
        entry.next_at = next_at(entry.min_secs, entry.max_secs, now, rng.random::<f32>());
    }
}

/// Carries the live Environment slider/checkbox onto the playing beds — the
/// row that until now was read by nothing (#772).
pub fn apply_zone_ambience_options(
    options: Res<GameOptions>,
    mut sinks: Query<&mut AudioSink, With<ZoneAmbienceBed>>,
) {
    for mut sink in sinks.iter_mut() {
        sink.set_volume(options.audio.env_gain());
        if options.audio.env_enabled {
            sink.play();
        } else {
            sink.pause();
        }
    }
}

/// Leaving the world silences the ambience and forgets the zone.
fn cleanup_zone_ambience(
    beds: Query<Entity, With<ZoneAmbienceBed>>,
    mut state: ResMut<ZoneAmbienceState>,
    mut commands: Commands,
) {
    for entity in beds.iter() {
        commands.entity(entity).despawn();
    }
    *state = ZoneAmbienceState::default();
}

/// Self-registration (#558). Live game world only, like `zone_bgm`.
pub struct ZoneAmbiencePlugin;

impl Plugin for ZoneAmbiencePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ZoneAmbienceState>()
            .add_systems(OnExit(SceneState::GameWorld), cleanup_zone_ambience)
            .add_systems(
                Update,
                (update_zone_ambience, fire_scheduled_ambients)
                    .run_if(in_state(SceneState::GameWorld)),
            )
            .add_systems(
                PreUpdate,
                apply_zone_ambience_options.run_if(crate::plugins::settings::live::options_changed),
            );
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn ambient(file: &str, min: u32, max: u32) -> Ambient {
        Ambient {
            file: file.to_string(),
            min_secs: min,
            max_secs: max,
        }
    }

    /// The 도적마을 day block of the user's `effectenvsnd.txt`, shape-for-shape:
    /// a `0~0` bed first, then two ranged entries.
    fn day_list() -> Vec<Ambient> {
        vec![
            ambient("night_wind.wav", 0, 0),
            ambient("donhwang_wind04.wav", 15, 40),
            ambient("night_bird01.wav", 30, 60),
        ]
    }

    #[test]
    fn continuous_beds_are_never_scheduled() {
        let scheduled = schedule(&day_list(), 100.0, || 0.5);
        assert_eq!(scheduled.len(), 2);
        assert!(scheduled.iter().all(|s| !s.path.contains("night_wind.wav")));
        assert_eq!(scheduled[0].path, "data://prim/snd/env/donhwang_wind04.wav");
    }

    #[test]
    fn a_ranged_entry_is_due_only_inside_its_own_window() {
        let now = 100.0;
        // the extremes of the draw are exactly the table's own bounds
        assert_eq!(next_at(15, 40, now, 0.0), 115.0);
        assert_eq!(next_at(15, 40, now, 1.0), 140.0);
        assert_eq!(next_at(15, 40, now, 0.5), 127.5);
        // and a garbage roll cannot escape the window
        assert_eq!(next_at(15, 40, now, -3.0), 115.0);
        assert_eq!(next_at(15, 40, now, 7.0), 140.0);

        for roll in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let due = schedule(&day_list(), now, || roll)[0].next_at;
            assert!(
                (now + 15.0..=now + 40.0).contains(&due),
                "{due} out of window"
            );
        }
    }

    #[test]
    fn a_zone_change_and_a_dawn_crossing_both_rebuild_but_a_step_does_not() {
        let zone = ZoneSound {
            name: "도적마을".to_string(),
            kind: crate::assets::textdata::zonesound::ZoneKind::Field,
            code: None,
            bgm: None,
            day: day_list(),
            night: vec![],
        };
        // same zone, same phase -> nothing to do
        assert!(!ambience_changed(
            (Some("도적마을"), Some(AmbientPhase::Day)),
            Some(&zone),
            AmbientPhase::Day
        ));
        // same zone, the sun went down -> rebuild
        assert!(ambience_changed(
            (Some("도적마을"), Some(AmbientPhase::Day)),
            Some(&zone),
            AmbientPhase::Night
        ));
        // another zone -> rebuild
        assert!(ambience_changed(
            (Some("장안"), Some(AmbientPhase::Day)),
            Some(&zone),
            AmbientPhase::Day
        ));
        // walked off the table -> rebuild (into silence)
        assert!(ambience_changed(
            (Some("도적마을"), Some(AmbientPhase::Day)),
            None,
            AmbientPhase::Day
        ));
        // and nothing playing in an off-table region is stable
        assert!(!ambience_changed(
            (None, Some(AmbientPhase::Day)),
            None,
            AmbientPhase::Day
        ));
    }

    /// Day/night follows the sun, not a clock constant: `t` is 0 at midnight
    /// and 0.5 at noon (`environment::TimeOfDay`).
    #[test]
    fn phase_follows_the_suns_own_curve() {
        assert_eq!(phase_at(0.5), AmbientPhase::Day);
        assert_eq!(phase_at(0.0), AmbientPhase::Night);
        assert_eq!(phase_at(0.9), AmbientPhase::Night);
        assert_eq!(phase_at(0.3), AmbientPhase::Day);
    }

    /// The environment channel is what this ticket makes real: muted must be a
    /// *paused* bed (so unmuting resumes it) but no one-shot at all.
    #[test]
    fn muted_environment_pauses_the_bed_and_drops_one_shots() {
        let mut audio = crate::plugins::settings::options::AudioOptions::default();
        assert!(!audio.env_playback_settings().paused);
        assert!(audio.env_oneshot().is_some());

        audio.env_enabled = false;
        assert!(audio.env_playback_settings().paused);
        assert!(audio.env_oneshot().is_none());

        audio.env_enabled = true;
        audio.env_volume = 50;
        assert_eq!(audio.env_gain().to_linear(), 0.5);
    }
}
