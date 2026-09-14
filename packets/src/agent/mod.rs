pub mod academy;
pub mod alchemy;
pub mod barena;
pub mod character_data;
pub mod chat;
pub mod exchange;
pub mod guild;
pub mod guild_leadership;
pub mod guild_storage;
pub mod guild_union;
pub mod guild_war;
pub mod ingame;
pub mod inventory;
pub mod lobby;
pub mod mail;
pub mod party;
pub mod pet;
pub mod quest;
pub mod siege;
pub mod stall;
pub mod storage;

use bevy::prelude::Message;

use sro_macro::ByteSize;
use sro_macro::Deserialize;
use sro_macro::SerializationError;
use sro_macro::Serialize;
use sro_macro_derive::*;

pub mod prelude {
    pub use crate::agent::academy::*;
    pub use crate::agent::alchemy::*;
    pub use crate::agent::barena::*;
    pub use crate::agent::chat::*;
    pub use crate::agent::exchange::*;
    pub use crate::agent::guild::*;
    pub use crate::agent::guild_leadership::*;
    pub use crate::agent::guild_storage::*;
    pub use crate::agent::guild_union::*;
    pub use crate::agent::guild_war::*;
    pub use crate::agent::ingame::*;
    pub use crate::agent::inventory::*;
    pub use crate::agent::lobby::*;
    pub use crate::agent::mail::*;
    pub use crate::agent::party::*;
    pub use crate::agent::pet::*;
    pub use crate::agent::quest::*;
    pub use crate::agent::siege::*;
    pub use crate::agent::stall::*;
    pub use crate::agent::storage::*;
    pub use crate::agent::*;
}

#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug)]
pub struct AgentLoginRequest {
    pub token: u32,
    pub username: String,
    pub password: String,
    pub content_id: u8,
    pub mac_address: [u8; 6],
}

#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug)]
pub struct AgentLoginResponse {
    pub result: u8,
    // Decoded as a raw u8, not a typed enum: an unknown/new error code must
    // never crash the client. A live vSRO server emitted code 3 (already
    // connected, on an admin re-login while a prior session lingered), which the
    // old typed enum had no variant for and panicked the deserializer on.
    #[sro_packet(when = "result == 2")]
    pub error_code: Option<u8>,
}

/// Human-readable name for an agent-auth error code (`AgentLoginResponse` with
/// `result == 2`). Kept as a lookup rather than a wire enum so unknown codes
/// degrade to "unknown" instead of failing deserialization.
pub fn describe_agent_auth_error(code: u8) -> &'static str {
    match code {
        3 => "already connected",
        4 => "server full",
        5 => "IP limit reached",
        _ => "unknown error",
    }
}
