//! Zone background music: the `effectenvsnd.txt` track of the zone the player
//! is standing in, played from `Music.pk2` on the BGM channel (EP-22.1).
//!
//! Idea: the zone-sound table (`assets::textdata::zonesound`, #770) already
//! fans many map sectors into one *zone*, so the track is keyed on the **zone
//! name**, never on the region id. That is the whole trick: walking from
//! sector (167,96) to (167,97) is still 장안, so the music must not restart —
//! only a change of zone swaps the track. `JMXVENVI`'s own Day/NightBGM
//! strings are not the source; they are empty in all 60 shipped profiles and
//! `docs/formats/envi-jmxvenvi.md:20-23` marks them obsolete, "handled by
//! regioninfo.txt & effectenvsnd.txt".
//!
//! Stated openroad decisions (ADR-0009):
//!
//! * **The swap is a hard cut.** The original's fade
//!   behaviour is not in our data — `effectenvsnd.txt` carries no fade column
//!   and no envelope — so a crossfade duration would be an invented number,
//!   while a cut is the one behaviour the data does describe (one track per
//!   zone, no overlap).
//! * A zone with no `effectenvsnd` entry (보상인던, and every off-table
//!   position) **keeps** the current track instead of falling silent, and says
//!   so once per crossing in the log. Silence would otherwise be indis-
//!   tinguishable from a bug.
//! * Muted BGM is a *paused sink*, not a missing entity, exactly as the intro
//!   does (#647): the track is spawned with
//!   [`AudioOptions::bgm_playback_settings`] so turning BGM on mid-zone starts
//!   it, and [`apply_zone_bgm_options`] carries the live slider/checkbox onto
//!   the playing sink.

use bevy::prelude::*;

use crate::assets::textdata::zonesound::ZoneSound;
use crate::plugins::hud::minimap::MinimapDungeonContext;
use crate::plugins::map::terrain::REGION_SIZE;
use crate::plugins::player::Player;
use crate::plugins::settings::options::GameOptions;
use crate::plugins::textdata::ClientZoneSounds;
use crate::plugins::world_origin::WorldOrigin;
use crate::scenes::SceneState;

/// Marker for the entity carrying the current zone's looping BGM.
#[derive(Component)]
pub struct ZoneBackgroundMusic;

/// Which region was last examined and which zone is currently sounding.
///
/// The region id is the *change detector* (cheap, once per sector crossing);
/// the zone name is the *identity of the track*, which is what keeps the music
/// running across sector crossings inside one zone.
#[derive(Resource, Default)]
pub struct ZoneBgmState {
    pub region: Option<u16>,
    pub zone: Option<String>,
}

/// What a crossing into `zone` should do to the music, given the zone whose
/// track is currently playing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZoneBgmAction {
    /// Same zone, no zone entry, or no track for it — leave the music alone.
    Keep,
    /// Cut to another zone's track.
    Swap { zone: String, track: String },
}

/// The zone-change decision, pure so it can be tested headlessly.
///
/// `playing` is the zone name whose track is sounding right now. Note the
/// three distinct "do nothing" cases fold into one: still inside the same
/// zone, walked into a zone the table does not cover, or walked into a zone
/// with no BGM of its own.
pub fn zone_bgm_action(playing: Option<&str>, zone: Option<&ZoneSound>) -> ZoneBgmAction {
    let Some(zone) = zone else {
        return ZoneBgmAction::Keep;
    };
    if playing == Some(zone.name.as_str()) {
        return ZoneBgmAction::Keep;
    }
    match zone.bgm_asset_path() {
        Some(track) => ZoneBgmAction::Swap {
            zone: zone.name.clone(),
            track,
        },
        None => ZoneBgmAction::Keep,
    }
}

/// Sector-local SRO coordinates of an SRO-space position, in the same
/// `0..REGION_SIZE` space as the `RECT` bounds of `regioninfo.txt`.
///
/// The X axis is negated first because that is what
/// [`overworld_region_id`](crate::plugins::hud::region_banner::overworld_region_id)
/// packs: SRO X grows the opposite way from render X (`world_origin`).
fn sector_local(sro: Vec3) -> (f32, f32) {
    (
        (-sro.x).rem_euclid(REGION_SIZE),
        sro.z.rem_euclid(REGION_SIZE),
    )
}

/// Swaps the zone track when the player crosses into a *different zone*.
///
/// The region id gate keeps this to one table lookup per sector crossing;
/// everything else is the pure [`zone_bgm_action`].
pub fn update_zone_bgm(
    zone_sounds: Res<ClientZoneSounds>,
    origin: Res<WorldOrigin>,
    options: Res<GameOptions>,
    asset_server: Res<AssetServer>,
    dungeon: Option<Res<MinimapDungeonContext>>,
    player: Query<&Transform, With<Player>>,
    playing: Query<Entity, With<ZoneBackgroundMusic>>,
    mut state: ResMut<ZoneBgmState>,
    mut commands: Commands,
) {
    // In a dungeon the region id comes from the dungeon context and every
    // claim in the table is `ALL`, so there is no in-sector position to apply.
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
    if state.region == Some(region) {
        return;
    }
    state.region = Some(region);

    let zone = match local {
        Some((x, z)) => zone_sounds.zone_at(region, x, z),
        None => zone_sounds.zone_for_region(region),
    };
    // Bound before the match so the immutable borrow of `state` ends here.
    let action = zone_bgm_action(state.zone.as_deref(), zone);
    match action {
        ZoneBgmAction::Keep => {
            if zone.is_none_or(|zone| zone.bgm.is_none()) {
                // once per crossing, not per frame: the region gate above
                info!(
                    "region {region} has no zone BGM ({}), keeping the current track",
                    zone.map_or("no zone entry", |zone| zone.name.as_str())
                );
            }
        }
        ZoneBgmAction::Swap { zone, track } => {
            info!("zone {zone}: background music {track}");
            // Hard cut, not a crossfade: no fade duration exists anywhere in
            // the zone-sound data, so a fade time would be invented (see the
            // module note, ADR-0009).
            for entity in playing.iter() {
                commands.entity(entity).despawn();
            }
            commands.spawn((
                AudioPlayer::new(asset_server.load(track.clone())),
                options.audio.bgm_playback_settings(),
                ZoneBackgroundMusic,
                Name::from("Zone Background Music"),
            ));
            state.zone = Some(zone);
        }
    }
}

/// Carries the live BGM slider/checkbox onto the playing zone track (#647).
///
/// Runs in the module that owns the consumer and is gated on the options
/// resource changing, never computed in `Plugin::build`.
pub fn apply_zone_bgm_options(
    options: Res<GameOptions>,
    mut sinks: Query<&mut AudioSink, With<ZoneBackgroundMusic>>,
) {
    for mut sink in sinks.iter_mut() {
        sink.set_volume(options.audio.bgm_gain());
        if options.audio.bgm_enabled {
            sink.play();
        } else {
            sink.pause();
        }
    }
}

/// Leaving the world stops the zone music and forgets the zone, so re-entering
/// starts the right track instead of believing it is still playing.
fn cleanup_zone_bgm(
    playing: Query<Entity, With<ZoneBackgroundMusic>>,
    mut state: ResMut<ZoneBgmState>,
    mut commands: Commands,
) {
    for entity in playing.iter() {
        commands.entity(entity).despawn();
    }
    *state = ZoneBgmState::default();
}

/// Self-registration (#558). Live game world only: the offline scenes have no
/// player position and never cross a zone boundary.
pub struct ZoneBgmPlugin;

impl Plugin for ZoneBgmPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ZoneBgmState>()
            .add_systems(OnExit(SceneState::GameWorld), cleanup_zone_bgm)
            .add_systems(
                Update,
                update_zone_bgm.run_if(in_state(SceneState::GameWorld)),
            )
            .add_systems(
                PreUpdate,
                apply_zone_bgm_options.run_if(crate::plugins::settings::live::options_changed),
            );
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::assets::textdata::zonesound::ZoneSoundTable;

    /// Real rows from the user's `Media.pk2` (2026-08-16): 장안 owns sectors
    /// (167,96) and (167,97), 돈황던젼 is the Z=128 dungeon row, and 보상인던
    /// is the one `regioninfo` zone with no `effectenvsnd` block at all.
    const REGIONINFO: &str = "#TOWN\t장안\t\t\t\t\t\r\n\
        167\t96\tALL\t\t\t\t\r\n\
        167\t97\tALL\t\t\t\t\r\n\
        #FIELD\t장안필드\t\t\t\t\t\r\n\
        168\t97\tALL\t\t\t\t\r\n\
        #FIELD\t보상인던\tfort_dungeon\t\t\t\t\r\n\
        17\t128\tALL\t\t\t\t\r\n";

    const EFFECTENVSND: &str = "<1>\t장안\t\t\t\t\r\n\
        \t\"Jangan_Town.ogg\"\t\t\t\t\r\n\
        \t<2>\t낮\t\t\t\r\n\
        \t\t\t<3>\t\"day_wind.wav\"\t0~0\r\n\
        <1>\t장안필드\t\t\t\t\r\n\
        \t\"China_Field.ogg\"\t\t\t\t\r\n\
        \t<2>\t낮\t\t\t\r\n\
        \t\t\t<3>\t\"day_wind.wav\"\t0~0\r\n";

    fn table() -> ZoneSoundTable {
        ZoneSoundTable::parse(REGIONINFO, EFFECTENVSND)
    }

    /// The acceptance criterion: crossing a *sector* boundary inside 장안 must
    /// not restart the track, crossing into 장안필드 must swap it.
    #[test]
    fn sector_crossing_inside_a_zone_keeps_the_track_but_a_zone_change_swaps_it() {
        let table = table();
        let jangan_a = table.zone_for_region((96 << 8) | 167);
        let jangan_b = table.zone_for_region((97 << 8) | 167);
        let field = table.zone_for_region((97 << 8) | 168);

        // entering the world: nothing playing yet -> start Jangan's track
        assert_eq!(
            zone_bgm_action(None, jangan_a),
            ZoneBgmAction::Swap {
                zone: "장안".into(),
                track: "music://jangan_town.ogg".into(),
            }
        );
        // (167,96) -> (167,97): different region id, same zone, same track
        assert_eq!(zone_bgm_action(Some("장안"), jangan_b), ZoneBgmAction::Keep);
        // (167,97) -> (168,97): different zone, cut to its track
        assert_eq!(
            zone_bgm_action(Some("장안"), field),
            ZoneBgmAction::Swap {
                zone: "장안필드".into(),
                track: "music://china_field.ogg".into(),
            }
        );
    }

    /// A zone the table does not cover, and the one zone that has sectors but
    /// no `effectenvsnd` block, both keep whatever is playing rather than
    /// falling silent.
    #[test]
    fn a_zone_without_a_track_keeps_the_current_music() {
        let table = table();
        // 보상인던 = dungeoninfo id 17 = 0x8011, sectors but no sound entry
        let silent = table.zone_for_region(0x8011).expect("zone exists");
        assert!(silent.bgm.is_none());
        assert_eq!(
            zone_bgm_action(Some("장안"), Some(silent)),
            ZoneBgmAction::Keep
        );
        // a region no zone claims at all
        assert_eq!(zone_bgm_action(Some("장안"), None), ZoneBgmAction::Keep);
        // ... and with nothing playing it stays silent instead of erroring
        assert_eq!(zone_bgm_action(None, None), ZoneBgmAction::Keep);
    }

    /// The sector-local offsets handed to the `RECT` lookup: SRO X is negated
    /// (the region-id convention) and both axes wrap into `0..REGION_SIZE`.
    #[test]
    fn sector_local_matches_the_region_id_convention() {
        // sector (167,96): sro.x = -167.5 * 1920, sro.z = 96.25 * 1920
        let sro = Vec3::new(-167.5 * REGION_SIZE, 0.0, 96.25 * REGION_SIZE);
        let (x, z) = sector_local(sro);
        assert!((x - 0.5 * REGION_SIZE).abs() < 0.5);
        assert!((z - 0.25 * REGION_SIZE).abs() < 0.5);
        assert_eq!(
            crate::plugins::hud::region_banner::overworld_region_id(-sro.x, sro.z),
            Some((96 << 8) | 167)
        );
    }
}
