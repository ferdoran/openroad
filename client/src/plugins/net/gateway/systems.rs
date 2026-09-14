use bevy::log::{error, info, warn};
use bevy::prelude::{Commands, Entity, Name, Query, Res, ResMut, With};

use packets::gateway::ShardListPingRequest;
use packets::Packet;

use crate::net::connection::{PendingConnection, SilkroadConnection};
use crate::plugins::config::division::DivisionInfo;
use crate::plugins::config::ClientConfig;
use crate::plugins::net::gateway::{GatewayConnection, GatewayConnectionStatus};
use crate::plugins::net::plugin::NetworkState;

/// Kicks off the gateway connect on a worker thread. Does nothing if a gateway
/// connection already exists or is still being established (the
/// [`GatewayConnection`] marker covers both), so it can double as a "reconnect
/// if needed" system (e.g. when re-entering the login form after a successful
/// login despawned the connection).
///
/// The blocking connect + handshake runs off-thread via
/// [`SilkroadConnection::connect_async`]; [`poll_gateway_connection`] delivers
/// the finished connection to the ECS, so the app never freezes here.
pub(crate) fn init_gateway_service(
    existing: Query<(), With<GatewayConnection>>,
    config: Option<Res<ClientConfig>>,
    division: Res<DivisionInfo>,
    mut status: ResMut<GatewayConnectionStatus>,
    mut commands: Commands,
) {
    if !existing.is_empty() {
        return;
    }

    let Some(config) = config else {
        warn!("client config does not exist");
        return;
    };

    let Some(address) = config.network_settings.resolve_gateway(&division) else {
        warn!(
            "no gateway address: config.yaml names none and Media.pk2's \
             divisioninfo.txt/gateport.txt could not supply one"
        );
        return;
    };

    let pending = SilkroadConnection::connect_async(address.as_str());
    commands.spawn((pending, GatewayConnection, Name::from("GatewayService")));
    *status = GatewayConnectionStatus::Connecting;
    info!("connecting to gateway service ...");
}

/// Delivers an in-flight gateway connect to the ECS. On success the established
/// [`SilkroadConnection`] replaces the [`PendingConnection`] on the same entity
/// and the initial shard-list ping is sent right away — driving it off
/// connection-establishment (rather than scene-enter) is what keeps it correct
/// now that the connect is asynchronous. On failure the entity is despawned and
/// the error is surfaced via [`GatewayConnectionStatus`].
pub(crate) fn poll_gateway_connection(
    query: Query<(Entity, &PendingConnection), With<GatewayConnection>>,
    mut network_state: ResMut<NetworkState>,
    mut status: ResMut<GatewayConnectionStatus>,
    mut commands: Commands,
) {
    for (entity, pending) in query.iter() {
        let Some(result) = pending.poll() else {
            continue;
        };
        match result {
            Ok(conn) => {
                // The connection is fully established (handshake done, socket
                // switched to non-blocking) before it ever reaches the ECS, so
                // the first `receive_packets` tick that sees it cannot race the
                // handshake.
                let sender = conn.get_sender();
                commands
                    .entity(entity)
                    .remove::<PendingConnection>()
                    .insert(conn);
                network_state.gateway = true;
                *status = GatewayConnectionStatus::Connected;
                info!("initialized gateway service");

                let frame = Packet::from(ShardListPingRequest).into();
                if let Err(e) = sender.send(frame) {
                    error!("failed to request shard list: {}", e.0);
                }
            }
            Err(e) => {
                error!("failed to init gateway service: {}", e);
                *status =
                    GatewayConnectionStatus::Failed(format!("Failed to connect to server: {e}"));
                commands.entity(entity).despawn();
            }
        }
    }
}
