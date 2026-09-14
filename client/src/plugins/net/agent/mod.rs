use crate::net::connection::SilkroadConnection;
use bevy::prelude::{Bundle, Component};

/// Marker class
#[derive(Component)]
pub struct AgentConnection;

#[derive(Bundle)]
pub struct AgentConnectionBundle {
    pub(crate) conn: SilkroadConnection,
    pub agent_connection: AgentConnection,
}

impl AgentConnectionBundle {
    pub(crate) fn new(conn: SilkroadConnection) -> Self {
        Self {
            conn,
            agent_connection: AgentConnection,
        }
    }
}
