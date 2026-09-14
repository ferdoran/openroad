use bytes::Bytes;
use packets::global::ModuleIdentification;

use crate::net::frame::SilkroadFrame;

pub(crate) mod blowfish;
mod codec;
pub mod connection;
pub(crate) mod crc;
pub mod entity_spawn;
pub mod frame;
mod handshake;
pub mod reader;
pub mod security;
pub(crate) mod sequence;

fn module_identification_frame() -> SilkroadFrame {
    let payload: Bytes = ModuleIdentification::client().into();
    SilkroadFrame::Packet {
        opcode: 0x2001,
        encrypted: 1,
        crc: 0,
        count: 0,
        data: payload,
    }
}
