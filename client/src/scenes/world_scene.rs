//! The offline world **sandbox** (`make run world`) — a dev scene, not the
//! game scene.
//!
//! Idea: load a terrain region around a hardcoded spawn point with a player
//! and a camera, and nothing else. It deliberately spawns **no UI**: the
//! HUD's fidelity target is `game_scene.rs` / [`SceneState::GameWorld`], the
//! state entered from character selection, and the surfaces that do show up
//! here (inventory plus the debug camera panel and the two egui dev windows)
//! are injected by their own plugins, three of the four being dev tooling.
//!
//! That absence is a scoping decision, not a gap, and the reason is worth
//! stating once: **the original has no per-scene UI composition at all**.
//! v1.188 executes a single global composition list (`ginterface.txt`, 84
//! live windows) once at client init and never partitions UI by scene — so
//! "which UI does the world scene spawn" is a question the original does not
//! pose. Per-scene gating is entirely an openroad convention (see
//! `docs/re/ui/scene-world.md`), and any HUD work belongs in the game scene.

use bevy::prelude::*;
use std::f32::consts::PI;

use crate::assets::mfo::JMXVMFO;
use crate::plugins::camera::{
    despawn_cinematic_camera, spawn_player_camera, CinematicCamera, CinematicCamera2, DebugCamera,
    PlayerCamera,
};
use crate::plugins::map::assets::MapsAssets;
use crate::plugins::map::terrain::preload_terrain_region;
use crate::plugins::map::terrain::Terrain;
use crate::plugins::player::{Player, PlayerPlugin};
use crate::plugins::world_origin::{set_world_origin, WorldOrigin};
use crate::scenes::SceneState;
use crate::util::region::RegionIdExt;

pub struct WorldScenePlugin;

#[allow(dead_code)]
const KARAKORAM: Vec3 = Vec3 {
    x: -129.0 * 1920.0,
    y: 1100.0,
    z: 91.0 * 1920.0,
};
#[allow(dead_code)]
const JANGAN: Vec3 = Vec3 {
    x: -167.0 * 1920.0,
    y: 350.0,
    z: 97.0 * 1920.0,
};

#[allow(dead_code)]
const INTRO_CAM_BASE: Vec3 = Vec3::new(-81.0 * 1920.0, 0.0, 105.0 * 1920.0);
// const INTRO_CAM_BASE: Vec3 = Vec3::new(-66.0 * 1920.0, 0.0, 190.0 * 1920.0);

// this is the offset in code, but it does not match exactly
// const INTRO_CAM_OFFSET: Vec3 = Vec3::new(-60.0, -15.0, 700.0);
#[allow(dead_code)]
const INTRO_CAM_OFFSET: Vec3 = Vec3::new(-56.0, -12.0, 697.6);

pub struct SpawnPoints;
#[allow(dead_code)]
impl SpawnPoints {
    pub fn jangan() -> Vec3 {
        Vec3 {
            x: -323526.03,
            y: -32.608875,
            z: 187275.28,
        }
    }

    pub const fn karakoram() -> Vec3 {
        return KARAKORAM;
    }

    pub fn intro_cam() -> Vec3 {
        return INTRO_CAM_BASE + INTRO_CAM_OFFSET;
    }
    pub const fn intro_cam_base() -> Vec3 {
        return INTRO_CAM_BASE;
    }

    pub fn intro_cam_rotation() -> Quat {
        Quat::from_euler(EulerRot::XYZ, -0.1, PI - 3.0, 0.0)
    }
}

impl Plugin for WorldScenePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(PlayerPlugin)
            .add_systems(
                OnEnter(SceneState::WorldSandbox),
                (
                    enter_play_mode,
                    set_origin_to_spawn_point,
                    spawn_player_camera,
                    despawn_cinematic_camera::<CinematicCamera>,
                    despawn_cinematic_camera::<CinematicCamera2>,
                    setup,
                    preload_starting_area,
                )
                    .chain(),
            )
            .add_systems(
                OnExit(SceneState::WorldSandbox),
                (
                    despawn_cinematic_camera::<PlayerCamera>,
                    despawn_cinematic_camera::<DebugCamera>,
                ),
            );
    }
}

/// The World sandbox starts in the third-person follow camera, not the free-fly
/// one (#199).
///
/// `AppMode` defaults to `DebugMode` (`main.rs`), and `spawn_player_camera`
/// activates whichever camera the current mode names — so this scene used to
/// open flying a `DebugCamera` with WASD while the character stood still, with
/// nothing on screen naming the `Tab` that switches (`plugins/dev/mod.rs`
/// `switch_mode`). Selecting `PlayMode` on entry is the smaller of the issue's
/// two options and mirrors what the Skills test scene already does
/// (`scenes/testing/skills.rs`); `Tab` still opts back into fly mode, and
/// nothing else changes — `GameWorld` forces the follow camera on its own path.
fn enter_play_mode(mut next_mode: ResMut<NextState<crate::AppMode>>) {
    next_mode.set(crate::AppMode::PlayMode);
}

/// Anchor the floating world origin on the spawn point so everything in the
/// world scene lives at small render-space coordinates (see `world_origin`).
/// Must run before the camera/player placement and terrain systems.
pub fn set_origin_to_spawn_point(
    mut origin: ResMut<WorldOrigin>,
    mut terrain: Query<&mut Transform, With<Terrain>>,
) {
    set_world_origin(SpawnPoints::jangan(), &mut origin, &mut terrain);
}

/// set up a simple 3D scene
pub fn setup(
    mut commands: Commands,
    origin: Res<WorldOrigin>,
    config: Res<crate::plugins::config::ClientConfig>,
    mut camera_query: Query<
        (Entity, &mut Transform),
        (With<Camera>, Without<Player>, Without<CinematicCamera>),
    >,
    mut player_query: Query<&mut Transform, (With<Player>, Without<Camera>)>,
) {
    // Ambient light is set up once in `map::setup_lighting`.
    let start = origin.to_render(SpawnPoints::jangan());

    let Ok(mut player_transform) = player_query.single_mut() else {
        return;
    };
    player_transform.translation = start;

    for cam in camera_query.iter_mut() {
        commands.entity(cam.0).insert((
            crate::plugins::map::terrain::rendering::fog(&config.graphics.fog),
            // ScreenSpaceAmbientOcclusionBundle::default(),
        ));
    }
}

/// Prototype for region-based preloading: keeps a fixed area around the Jangan spawn
/// point permanently loaded (see `terrain::preload_terrain_region`), instead of letting
/// it stream in/out with the rest of the world like every other region does. The map
/// format itself has no notion of named zones/instances, so `RADIUS` here just stands in
/// for wherever a real area boundary would come from.
///
/// Kept deliberately small for now: block mesh/material building is still fully
/// synchronous (see `load_terrain_system`), so every region in this radius gets built in
/// one go the moment its `.m` data loads — too large a radius here will stall the game on
/// scene entry rather than during normal streaming.
pub fn preload_starting_area(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    map_infos: Res<Assets<JMXVMFO>>,
    maps_assets: Res<MapsAssets>,
    origin: Res<WorldOrigin>,
) {
    const RADIUS: i32 = 3;

    let Some(map_info) = map_infos.get(&maps_assets.map_info) else {
        return;
    };
    if map_info.map_width == 0 || map_info.map_height == 0 {
        return;
    }

    let (center_x, center_z) = SpawnPoints::jangan().to_x_z();
    let min_x = (center_x as i32 - RADIUS).max(0) as u8;
    let max_x = (center_x as i32 + RADIUS).min(map_info.map_width as i32 - 1) as u8;
    let min_z = (center_z as i32 - RADIUS).max(0) as u8;
    let max_z = (center_z as i32 + RADIUS).min(map_info.map_height as i32 - 1) as u8;

    preload_terrain_region(
        &mut commands,
        &asset_server,
        map_info,
        &origin,
        min_x,
        max_x,
        min_z,
        max_z,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Entering the World sandbox must queue `PlayMode`, so `switch_camera`
    /// activates the follow camera instead of the free-fly one. Without this the
    /// scene opens on a `DebugCamera` and the only way out is an undiscoverable
    /// `Tab` (#199).
    #[test]
    fn entering_the_world_sandbox_queues_play_mode() {
        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin)
            .init_state::<crate::AppMode>();

        // the app-wide default, from main.rs — this is what made the sandbox fly
        assert_eq!(
            *app.world().resource::<State<crate::AppMode>>().get(),
            crate::AppMode::DebugMode
        );

        app.add_systems(Update, enter_play_mode);
        // `StateTransition` runs before `Update` in `Main`, so the queued state
        // lands on the following frame — two ticks, not one.
        app.update();
        app.update();

        assert_eq!(
            *app.world().resource::<State<crate::AppMode>>().get(),
            crate::AppMode::PlayMode
        );
    }
}
