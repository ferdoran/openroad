pub mod model;
pub mod net;
pub mod owner;
pub mod ui;

use bevy::prelude::*;

use crate::scenes::SceneState;

/// Self-registration for the player stall window (#558 registry, #779).
pub struct StallPlugin;

impl Plugin for StallPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<model::StallState>()
            .init_resource::<owner::RequestedStallTitle>()
            .add_message::<owner::OpenStallCommand>()
            .add_systems(OnExit(SceneState::GameWorld), ui::cleanup_stall)
            .add_systems(PostUpdate, ui::despawn_closing_stall)
            .add_systems(
                Update,
                (
                    // server-driven throughout: the window opens on the
                    // server's enter broadcast and closes on the leave ack,
                    // never on a click (#780, docs/re/systems/stall.md §3)
                    owner::on_open_stall_command,
                    owner::on_stall_create_response,
                    owner::on_stall_destroy_response,
                    owner::on_stall_update_response,
                    owner::on_entity_stall_title_update,
                    owner::on_entity_stall_destroy,
                    net::on_stall_entity_action,
                    net::on_stall_buy_response,
                    net::on_stall_leave_response,
                    ui::spawn_stall_window.run_if(ui::open_pending_stall),
                    ui::refresh_stall_window,
                )
                    .chain()
                    .run_if(super::hud_scenes),
            );
    }
}
