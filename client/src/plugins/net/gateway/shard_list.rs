use bevy::prelude::{error, info, warn, Commands, MessageReader, Query, Resource, With};

use packets::gateway::{ShardListPingResponse, ShardListRequest, ShardListResponse};
use packets::Packet;

use crate::net::connection::SilkroadConnection;
use crate::plugins::net::gateway::GatewayConnection;

#[derive(Resource)]
pub struct ShardList(pub ShardListResponse);

pub(crate) fn on_shardlist_ping_response(
    mut events: MessageReader<ShardListPingResponse>,
    query: Query<&mut SilkroadConnection, With<GatewayConnection>>,
) {
    match events.read().next() {
        None => return,
        Some(res) => {
            // The packet is a counted list, so an empty one is legal (and what
            // a gateway with no farm configured sends) — not an error arm.
            if res.farms.is_empty() {
                warn!("[ShardListPingResponse]: the gateway advertised no farms");
            }
            for farm in &res.farms {
                info!(
                    "[ShardListPingResponse]: farm_id = {}, ip = {}",
                    farm.id, farm.ip
                );
            }

            let Ok(conn) = query.single() else {
                return;
            };
            let sender = conn.get_sender();
            let frame = Packet::from(ShardListRequest {}).into();

            if let Err(e) = sender.send(frame) {
                error!("failed to send frame: {}", e.0);
            }
        }
    }
}

pub(crate) fn on_shardlist_response(
    mut events: MessageReader<ShardListResponse>,
    mut commands: Commands,
) {
    if let Some(res) = events.read().next() {
        info!(
            "[ShardListResponse]: farms = {:?}, shards = {:?}",
            res.farms, res.shards
        );
        commands.insert_resource(ShardList(res.clone()));
    }
}
