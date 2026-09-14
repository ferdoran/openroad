use bevy::prelude::{Component, Resource};

pub(crate) mod plugin;
pub(crate) mod shard_list;
pub(crate) mod systems;

/// Marker class
#[derive(Component)]
pub struct GatewayConnection;

/// Observable status of the (now asynchronous) gateway connect, so the UI can
/// show a connecting state and a clean failure message instead of freezing
/// while the worker thread runs the connect + handshake.
#[derive(Resource, Default)]
pub enum GatewayConnectionStatus {
    #[default]
    Idle,
    Connecting,
    Connected,
    Failed(String),
}
