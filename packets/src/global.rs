use bevy::prelude::Message;
use bytes::{Buf, BufMut, Bytes, BytesMut};

use sro_macro::ByteSize;
use sro_macro::Deserialize;
use sro_macro::SerializationError;
use sro_macro::Serialize;
use sro_macro_derive::*;

#[derive(Message, Serialize, Deserialize, ByteSize, Debug, Clone)]
pub struct GlobalStateUpdate {
    pub update_flag: u8,
    #[sro_packet(when = "update_flag & 1 != 0")]
    pub server_body: Option<ServerBody>,
    #[sro_packet(when = "update_flag & 2 != 0")]
    pub server_cord: Option<ServerCord>,
}

#[derive(Message, Serialize, Deserialize, ByteSize, Debug, Clone)]
pub struct ServerBody {
    pub unknown_byte: u8,
    #[sro_packet(list_type = "break")]
    pub server_bodies: Vec<ServerBodyEntry>,
}

#[derive(Message, Serialize, Deserialize, ByteSize, Debug, Clone)]
pub struct ServerCord {
    pub unknown_byte: u8,
    #[sro_packet(list_type = "break")]
    pub server_cords: Vec<ServerCordEntry>,
}

#[derive(Message, Serialize, Deserialize, ByteSize, Debug, Clone)]
pub struct ServerBodyEntry {
    pub id: u16,
    pub state: u32,
}

#[derive(Message, Serialize, Deserialize, ByteSize, Debug, Clone)]
pub struct ServerCordEntry {
    pub id: u32,
    pub state: u32,
}

#[derive(Message, Serialize, Deserialize, ByteSize, Debug, Clone)]
pub struct GlobalStateRequest {
    pub update_flag: u8,
    #[sro_packet(when = "update_flag & 1 != 0")]
    pub server_body: Option<ServerBodyRequest>,
    #[sro_packet(when = "update_flag & 2 != 0")]
    pub server_cord: Option<ServerCordRequest>,
}

#[derive(Message, Serialize, Deserialize, ByteSize, Debug, Clone)]
pub struct ServerBodyRequest {
    pub unknown_byte: u8,
    #[sro_packet(list_type = "break")]
    pub server_bodies: Vec<ServerBodyRequestEntry>,
}

#[derive(Message, Serialize, Deserialize, ByteSize, Debug, Clone)]
pub struct ServerCordRequest {
    pub unknown_byte: u8,
    #[sro_packet(list_type = "break")]
    pub server_cords: Vec<ServerCordRequestEntry>,
}

#[derive(Message, Serialize, Deserialize, ByteSize, Debug, Clone)]
pub struct ServerBodyRequestEntry {
    pub id: u16,
}

#[derive(Message, Serialize, Deserialize, ByteSize, Debug, Clone)]
pub struct ServerCordRequestEntry {
    pub id: u32,
}

#[derive(Message, Serialize, Deserialize, ByteSize, Debug, Clone)]
pub struct KeepAlive;

/// 0x2113 — `GLOBAL_XTRAP_IDENTIFICATION`, the anti-cheat challenge/response the
/// server drives during a live session. One opcode, both directions.
///
/// The framing is `u8 kind`, `u8 sub`, `u8[0x400]` in **both** directions — 1026
/// bytes, fixed. The original parses it inline in its receive loop rather than
/// through a registration table (`sro_client.exe 00842ff0:44-89`): it reads
/// `kind`, and only for `kind == 1` reads `sub` plus the 1024-byte challenge;
/// only `sub == 1` produces a reply.
///
/// **The 1024-byte blob itself stays opaque.** It is an XTrap challenge; per the
/// workspace security rules it is handled as pure data and the module is not
/// reversed. So the body is kept whole — one opcode maps to one type used for
/// both directions, a decode preserves the challenge exactly, and an encode emits
/// whatever bytes were set. [`Self::kind`]/[`Self::sub`] expose the two framing
/// bytes for logging; [`Self::stub_reply`] builds the one outbound body we have.
#[derive(Message, Clone, Debug, PartialEq)]
pub struct XTrapIdentification {
    pub raw: Bytes,
}

impl XTrapIdentification {
    /// The `kind` byte (`00842ff0:50`). `None` for an empty body.
    pub fn kind(&self) -> Option<u8> {
        self.raw.first().copied()
    }

    /// The `sub` byte (`00842ff0:52`), which the original only reads when
    /// `kind == 1`. `None` for a body shorter than two bytes.
    pub fn sub(&self) -> Option<u8> {
        self.raw.get(1).copied()
    }

    /// The reply body used by the clientless reference proxy: `02 02` followed by
    /// 1024 zero bytes, 1026 total — the same length the original writes
    /// (`00842ff0:75-77`).
    ///
    /// This is a **stated deviation**, not a valid XTrap signature: the original
    /// returns a blob computed by the anti-cheat module from the challenge
    /// (`FUN_00b78fd0(challenge, out, 0x13)`, `00842ff0:55`), which we do not
    /// reimplement. A server with XTrap stubbed or disabled accepts the stub, but
    /// sending it is non-original behaviour, so a caller that wires this into the
    /// receive loop must put it behind a config flag. Nothing sends it today.
    pub fn stub_reply() -> Self {
        let mut buf = BytesMut::with_capacity(1026);
        buf.put_u8(2);
        buf.put_u8(2);
        buf.put_bytes(0, 1024);
        XTrapIdentification { raw: buf.freeze() }
    }
}

/// 0x2001 — module identification, the first packet on every Silkroad connection
/// and the only one exchanged before anything else is meaningful.
///
/// Idea: both directions carry a single `u16`-length-prefixed ASCII name and no
/// more. The client announces `SR_Client` (`004ce8c0:16-27`: opcode `0x2001`,
/// the name, then one zero byte) and the server answers with its own name, which
/// the original compares literally against `GatewayServer`, `AgentServer` and
/// `DownloadServer` (`004ce9d0:62,91,138`) to decide what kind of peer it is
/// talking to — see [`PeerKind`].
///
/// The trailing byte is modelled as optional on purpose: the original writes it
/// on the C→S side, but there is no S→C capture (this packet is exchanged during
/// connection setup, before the dump-capable receive loop runs), so requiring it
/// inbound would be an unverified assumption that could reject a real server.
#[derive(Message, Debug, Clone, PartialEq)]
pub struct ModuleIdentification {
    /// The module name. `u16`-length-prefixed, ASCII, no NUL terminator.
    pub module: String,
    /// One trailing byte, `0` in the original's C→S frame (`004ce8c0:19,26-27`).
    /// Its meaning is `[U]` — the original hardcodes zero and never reads it back.
    pub tail: Option<u8>,
}

/// The peer kinds the original client recognizes by name in the `0x2001` reply
/// (`004ce9d0:62,91,138`). Anything else stays [`PeerKind::Unknown`] rather than
/// being guessed at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerKind {
    Gateway,
    Agent,
    Download,
    Unknown,
}

impl ModuleIdentification {
    /// The C→S frame the original sends: `SR_Client` plus the trailing zero.
    pub fn client() -> Self {
        ModuleIdentification {
            module: "SR_Client".to_owned(),
            tail: Some(0),
        }
    }

    /// Classify a server's announced name the way the original does.
    pub fn peer_kind(&self) -> PeerKind {
        match self.module.as_str() {
            "GatewayServer" => PeerKind::Gateway,
            "AgentServer" => PeerKind::Agent,
            "DownloadServer" => PeerKind::Download,
            _ => PeerKind::Unknown,
        }
    }
}

impl TryFrom<Bytes> for ModuleIdentification {
    type Error = SerializationError;
    fn try_from(mut value: Bytes) -> Result<Self, SerializationError> {
        if value.len() < 2 {
            return Err(SerializationError::IoError(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "0x2001 body is shorter than its length prefix",
            )));
        }
        let len = value.get_u16_le() as usize;
        if value.len() < len {
            return Err(SerializationError::IoError(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "0x2001 module name is shorter than its length prefix",
            )));
        }
        let module = String::from_utf8(value.split_to(len).to_vec())?;
        let tail = value.first().copied();
        Ok(ModuleIdentification { module, tail })
    }
}

impl From<ModuleIdentification> for Bytes {
    fn from(p: ModuleIdentification) -> Self {
        let mut buf = BytesMut::with_capacity(p.module.len() + 3);
        buf.put_u16_le(p.module.len() as u16);
        buf.put_slice(p.module.as_bytes());
        if let Some(tail) = p.tail {
            buf.put_u8(tail);
        }
        buf.freeze()
    }
}

impl TryFrom<Bytes> for XTrapIdentification {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, SerializationError> {
        Ok(XTrapIdentification { raw: value })
    }
}

impl From<XTrapIdentification> for Bytes {
    fn from(p: XTrapIdentification) -> Self {
        p.raw
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reply shape the reference proxy sends: `02 02` + 1024 zeros.
    #[test]
    fn xtrap_stub_reply_is_two_flags_and_1024_zero_bytes() {
        let wire: Bytes = XTrapIdentification::stub_reply().into();

        assert_eq!(wire.len(), 1026);
        assert_eq!(&wire[..2], &[2, 2]);
        assert!(wire[2..].iter().all(|&b| b == 0));
    }

    /// A real captured challenge (packet_dump/0x2113.log, 2026-08-10) must survive
    /// decode+encode byte-for-byte — the blob is opaque, so anything else would be
    /// losing anti-cheat data we are not allowed to interpret.
    ///
    /// The wire body is 1026: `kind`, `sub`, 1024 challenge bytes
    /// (`00842ff0:50-53`). The dump lines are 1028 because they were captured
    /// before the frame-body fix (#449) and still carry two bytes of our own
    /// blowfish padding — `1026 + 4` rounded to the 8-byte block is `1032`, minus
    /// the 4-byte header.
    #[test]
    fn xtrap_challenge_roundtrips_a_captured_body_untouched() {
        // First 16 bytes of a captured challenge, then zero-fill to the wire
        // length: kind 01, sub 01, then the blob (selector 01 + the entropy).
        let mut body = vec![
            0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xd4, 0x33, 0x9a, 0x1e,
            0x1c, 0x3d,
        ];
        body.resize(1026, 0);
        let captured = Bytes::from(body);

        let decoded = XTrapIdentification::try_from(captured.clone()).unwrap();
        assert_eq!(decoded.raw.len(), 1026);
        assert_eq!(decoded.kind(), Some(1));
        assert_eq!(decoded.sub(), Some(1));

        let back: Bytes = decoded.into();
        assert_eq!(back, captured);
    }

    /// The exact C→S bytes the original writes at `004ce8c0:16-27`: the `u16`
    /// length, `SR_Client`, and one trailing zero — 12 bytes, no NUL in the name.
    #[test]
    fn module_identification_client_frame_is_binary_exact() {
        let wire: Bytes = ModuleIdentification::client().into();

        assert_eq!(
            &wire[..],
            &[0x09, 0x00, b'S', b'R', b'_', b'C', b'l', b'i', b'e', b'n', b't', 0x00]
        );
    }

    /// A server reply names the peer; the three names the original compares
    /// against map to a [`PeerKind`], anything else stays `Unknown`. The trailing
    /// byte is optional in both directions because no S→C capture exists.
    #[test]
    fn module_identification_reads_peer_names_with_or_without_the_tail() {
        for (name, expected) in [
            ("GatewayServer", PeerKind::Gateway),
            ("AgentServer", PeerKind::Agent),
            ("DownloadServer", PeerKind::Download),
            ("SomethingElse", PeerKind::Unknown),
        ] {
            let mut with_tail = BytesMut::new();
            with_tail.put_u16_le(name.len() as u16);
            with_tail.put_slice(name.as_bytes());
            let without_tail = with_tail.clone().freeze();
            with_tail.put_u8(0);

            let decoded = ModuleIdentification::try_from(with_tail.freeze()).unwrap();
            assert_eq!(decoded.module, name);
            assert_eq!(decoded.tail, Some(0));
            assert_eq!(decoded.peer_kind(), expected);

            let decoded = ModuleIdentification::try_from(without_tail.clone()).unwrap();
            assert_eq!(decoded.module, name);
            assert_eq!(decoded.tail, None);
            assert_eq!(decoded.peer_kind(), expected);
            let back: Bytes = decoded.into();
            assert_eq!(back, without_tail);
        }
    }

    /// The macro must route `0x2001` to this type in both directions, so the
    /// ledger and the dump-side name table see it.
    #[test]
    fn module_identification_is_wired_to_0x2001() {
        let wire: Bytes = ModuleIdentification::client().into();

        let packet = crate::Packet::deserialize(0x2001, wire.clone()).unwrap();
        let (opcode, back) = packet.into_serialize();

        assert_eq!(opcode, 0x2001);
        assert_eq!(back, wire);
    }

    /// A truncated body is an error, not a panic or a silently empty name.
    #[test]
    fn module_identification_rejects_a_truncated_body() {
        for body in [
            Bytes::from_static(&[0x01]),
            Bytes::from_static(&[0x09, 0x00, b'S', b'R']),
        ] {
            assert!(ModuleIdentification::try_from(body).is_err());
        }
    }

    /// The framing bytes must be readable without touching the blob, and absent
    /// rather than invented when the body is too short.
    #[test]
    fn xtrap_kind_and_sub_read_the_two_framing_bytes() {
        let reply = XTrapIdentification::stub_reply();
        assert_eq!(reply.kind(), Some(2));
        assert_eq!(reply.sub(), Some(2));

        let short = XTrapIdentification::try_from(Bytes::from_static(&[0x01])).unwrap();
        assert_eq!(short.kind(), Some(1));
        assert_eq!(short.sub(), None);

        let empty = XTrapIdentification::try_from(Bytes::new()).unwrap();
        assert_eq!(empty.kind(), None);
    }

    /// An empty or short body must not fail to decode — the challenge layout is
    /// unknown, so no length is assumed.
    #[test]
    fn xtrap_makes_no_assumption_about_body_length() {
        for body in [Bytes::new(), Bytes::from_static(&[0x01])] {
            let decoded = XTrapIdentification::try_from(body.clone()).unwrap();
            let back: Bytes = decoded.into();
            assert_eq!(back, body);
        }
    }
}
