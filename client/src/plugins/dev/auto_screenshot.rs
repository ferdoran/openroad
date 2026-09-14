//! Idea: headless-friendly visual debugging. When `OPENROAD_SCREENSHOT=/path/prefix` is set,
//! this plugin saves periodic screenshots of the primary window via bevy's built-in
//! `Screenshot` entity and exits the app after the last one. This lets an agent (or CI) launch
//! the real client, capture what actually renders, and inspect the PNGs — no human at the
//! window, no compositor-specific capture tools. Inert unless the env var is set.

use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};

const SHOT_TIMES_SECS: [f32; 3] = [2.0, 2.8, 3.6];
const EXIT_TIME_SECS: f32 = 4.1;

pub struct AutoScreenshotPlugin;

impl Plugin for AutoScreenshotPlugin {
    fn build(&self, app: &mut App) {
        let Ok(prefix) = std::env::var("OPENROAD_SCREENSHOT") else {
            return;
        };
        app.insert_resource(AutoScreenshot { prefix, taken: 0 });
        app.add_systems(Update, take_screenshots);
    }
}

#[derive(Resource)]
struct AutoScreenshot {
    prefix: String,
    taken: usize,
}

fn take_screenshots(
    mut commands: Commands,
    time: Res<Time>,
    mut state: ResMut<AutoScreenshot>,
    mut exit: MessageWriter<AppExit>,
) {
    let elapsed = time.elapsed_secs();
    if state.taken < SHOT_TIMES_SECS.len() && elapsed >= SHOT_TIMES_SECS[state.taken] {
        let path = format!("{}_{}.png", state.prefix, state.taken);
        info!("auto-screenshot -> {path}");
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
        state.taken += 1;
    }
    if elapsed >= EXIT_TIME_SECS {
        exit.write(AppExit::Success);
    }
}
