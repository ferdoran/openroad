//! Idea: an offline preview of the world map window — opens on the world map
//! with the preview player anchored in Jangan (same origin as the minimap
//! preview), so the tile grid, POI labels/icons, city buttons, panning and
//! the player arrow can be iterated without a live server. Pressing M
//! re-opens it region-aware (inside Jangan → the city map). Run with
//! `SCENE=ui_testing`.

use bevy::prelude::*;

use crate::plugins::hud::world_map::model::WorldMapState;
use crate::plugins::world_origin::WorldOrigin;
use crate::scenes::SceneState;

pub struct WorldMapUiPreviewPlugin;

impl Plugin for WorldMapUiPreviewPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(SceneState::UiTesting), seed_world_map_preview);
    }
}

fn seed_world_map_preview(mut origin: ResMut<WorldOrigin>, mut state: ResMut<WorldMapState>) {
    // Jangan, region 169x96 (matches the minimap preview's anchor).
    *origin = WorldOrigin(Vec3::new(-(169.0 * 1920.0), 0.0, 96.0 * 1920.0));
    state.current_map = 0;
    state.center_on_player = true;
    state.open = true;
}
