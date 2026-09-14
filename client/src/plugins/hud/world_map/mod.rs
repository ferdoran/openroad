pub mod model;
pub mod ui;

use bevy::prelude::*;

/// Self-registration for the world-map window (#558). The HUD registry holds one line per
/// window, so two windows landing in the same lap no longer collide on it.
pub struct WorldMapPlugin;

impl Plugin for WorldMapPlugin {
    fn build(&self, app: &mut App) {
        use crate::scenes::SceneState;

        app.init_resource::<model::WorldMapState>()
            .init_resource::<model::MapMarkers>()
            .init_resource::<model::WorldMapFollow>()
            .add_systems(OnExit(SceneState::GameWorld), ui::cleanup_world_map)
            // The Dungeons test scene runs the map widgets (dungeon floor
            // tiles / floor map verification) without the rest of the HUD.
            .add_systems(OnExit(SceneState::Dungeons), ui::cleanup_world_map)
            .add_systems(
                Update,
                (
                    model::toggle_world_map
                        .run_if(not(crate::plugins::settings::keymap::text_field_focused)),
                    model::sync_party_markers,
                    model::publish_static_location_markers,
                    ui::sync_world_map_window,
                    ui::update_marker_labels,
                    ui::update_world_map_arrow,
                    ui::update_follow_label,
                    ui::hide_failed_tiles,
                )
                    .run_if(super::map_scenes),
            );
    }
}
