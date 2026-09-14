use bevy::app::App;
use bevy::prelude::*;

use crate::plugins::net::gateway::shard_list::{on_shardlist_ping_response, on_shardlist_response};
use crate::plugins::net::gateway::systems::{init_gateway_service, poll_gateway_connection};
use crate::scenes::SceneState;

pub struct GatewayPlugin;

impl Plugin for GatewayPlugin {
    fn build(&self, app: &mut App) {
        // The shard-list ping is no longer requested on scene-enter: the connect
        // is asynchronous now, so `poll_gateway_connection` fires it the moment
        // the connection is established instead.
        app.add_systems(OnEnter(SceneState::Loading), init_gateway_service)
            .add_systems(
                Update,
                (
                    poll_gateway_connection,
                    on_shardlist_ping_response,
                    on_shardlist_response,
                ),
            );
    }
}
