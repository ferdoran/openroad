//! Chat packets (0x7025 request / 0xB025 response / 0x3026 update / 0x302D
//! restriction).
//!
//! Layouts follow the SilkroadDoc AGENT_CHAT pages, with encodings matched to
//! the go-sro server this client plays against
//! (github.com/ferdoran/go-sro, agent-server/handler/chat): **every string is
//! a u16 byte-count prefix + UTF-8 bytes** — go-sro's packet framework has no
//! UTF-16 writer, so the "chat text is unicode" note from the vSRO docs does
//! not apply here. Other quirks:
//!   * `ChatRequest` carries a receiver name only for whispers — a
//!     `when`-conditional field with no presence flag;
//!   * `ChatUpdate`'s sender is a u32 unique id for proximity channels
//!     (All/AllGM/NPC), a name string for the rest, and absent for notices —
//!     modeled as a discriminated enum on the leading chat-type byte. Unknown
//!     chat types fail decode with `UnknownVariation` and are logged/dropped
//!     by the receive pipeline.

use bevy::prelude::Message;

use sro_macro::ByteSize;
use sro_macro::Deserialize;
use sro_macro::SerializationError;
use sro_macro::Serialize;
use sro_macro_derive::*;

/// Chat channel ids on the wire (`chat_type` bytes).
pub mod chat_type {
    pub const ALL: u8 = 1;
    pub const PM: u8 = 2;
    pub const ALL_GM: u8 = 3;
    pub const PARTY: u8 = 4;
    pub const GUILD: u8 = 5;
    pub const GLOBAL: u8 = 6;
    pub const NOTICE: u8 = 7;
    pub const STALL: u8 = 9;
    pub const UNION: u8 = 11;
    pub const NPC: u8 = 13;
    pub const ACADEMY: u8 = 16;
}

/// 0x7025 — client → server chat send. `chat_index` is a client-side rolling
/// counter echoed back in [`ChatResponse`] to correlate results.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct ChatRequest {
    pub chat_type: u8,
    pub chat_index: u8,
    #[sro_packet(when = "chat_type == 2")]
    pub receiver: Option<String>,
    pub message: String,
}

/// 0xB025 — server → client result for a [`ChatRequest`]. `result == 1` is
/// success; `result == 2` carries an error code.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct ChatResponse {
    pub result: u8,
    #[sro_packet(when = "result == 2")]
    pub error: Option<u16>,
    pub chat_type: u8,
    pub chat_index: u8,
}

/// 0x3026 — server → client incoming chat line, discriminated on the leading
/// chat-type byte (values match [`chat_type`]).
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub enum ChatUpdate {
    #[sro_packet(value = 1)]
    All { sender_id: u32, message: String },
    #[sro_packet(value = 2)]
    Pm { sender: String, message: String },
    #[sro_packet(value = 3)]
    AllGm { sender_id: u32, message: String },
    #[sro_packet(value = 4)]
    Party { sender: String, message: String },
    #[sro_packet(value = 5)]
    Guild { sender: String, message: String },
    #[sro_packet(value = 6)]
    Global { sender: String, message: String },
    #[sro_packet(value = 7)]
    Notice { message: String },
    #[sro_packet(value = 9)]
    Stall { sender: String, message: String },
    #[sro_packet(value = 11)]
    Union { sender: String, message: String },
    #[sro_packet(value = 13)]
    Npc { sender_id: u32, message: String },
    #[sro_packet(value = 16)]
    Academy { sender: String, message: String },
}

/// 0x302D — server → client chat restriction (mute) for `seconds`.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct ChatRestriction {
    pub seconds: u8,
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::*;

    #[test]
    fn chat_request_all_roundtrips() {
        let request = ChatRequest {
            chat_type: chat_type::ALL,
            chat_index: 7,
            receiver: None,
            message: "hi".to_string(),
        };
        assert_eq!(request.byte_size(), 6);
        let bytes: Bytes = request.clone().into();
        assert_eq!(bytes.as_ref(), &[0x01, 0x07, 0x02, 0x00, b'h', b'i']);
        assert_eq!(ChatRequest::try_from(bytes).unwrap(), request);
    }

    #[test]
    fn chat_request_pm_carries_receiver_without_flag_byte() {
        let request = ChatRequest {
            chat_type: chat_type::PM,
            chat_index: 3,
            receiver: Some("Foo".to_string()),
            message: "hi".to_string(),
        };
        assert_eq!(request.byte_size(), 11);
        let bytes: Bytes = request.clone().into();
        assert_eq!(
            bytes.as_ref(),
            &[
                0x02, 0x03, // type, index
                0x03, 0x00, b'F', b'o', b'o', // receiver
                0x02, 0x00, b'h', b'i', // message
            ]
        );
        assert_eq!(ChatRequest::try_from(bytes).unwrap(), request);
    }

    // go-sro's length prefix counts UTF-8 *bytes*, not characters.
    #[test]
    fn chat_request_utf8_len_counts_bytes() {
        let request = ChatRequest {
            chat_type: chat_type::ALL,
            chat_index: 0,
            receiver: None,
            message: "안녕".to_string(),
        };
        assert_eq!(request.byte_size(), 10);
        let bytes: Bytes = request.clone().into();
        assert_eq!(
            bytes.as_ref(),
            &[0x01, 0x00, 0x06, 0x00, 0xEC, 0x95, 0x88, 0xEB, 0x85, 0x95]
        );
        assert_eq!(ChatRequest::try_from(bytes).unwrap(), request);
    }

    #[test]
    fn chat_update_all_carries_unique_id() {
        let update = ChatUpdate::All {
            sender_id: 0x0403_0201,
            message: "hi".to_string(),
        };
        assert_eq!(update.byte_size(), 9);
        let bytes: Bytes = update.clone().into();
        assert_eq!(
            bytes.as_ref(),
            &[0x01, 0x01, 0x02, 0x03, 0x04, 0x02, 0x00, b'h', b'i']
        );
        assert_eq!(ChatUpdate::try_from(bytes).unwrap(), update);
    }

    #[test]
    fn chat_update_party_carries_name() {
        let update = ChatUpdate::Party {
            sender: "Foo".to_string(),
            message: "hi".to_string(),
        };
        assert_eq!(update.byte_size(), 10);
        let bytes: Bytes = update.clone().into();
        assert_eq!(
            bytes.as_ref(),
            &[0x04, 0x03, 0x00, b'F', b'o', b'o', 0x02, 0x00, b'h', b'i']
        );
        assert_eq!(ChatUpdate::try_from(bytes).unwrap(), update);
    }

    #[test]
    fn chat_update_notice_has_no_sender() {
        let update = ChatUpdate::Notice {
            message: "hi".to_string(),
        };
        assert_eq!(update.byte_size(), 5);
        let bytes: Bytes = update.clone().into();
        assert_eq!(bytes.as_ref(), &[0x07, 0x02, 0x00, b'h', b'i']);
        assert_eq!(ChatUpdate::try_from(bytes).unwrap(), update);
    }

    #[test]
    fn chat_update_unknown_type_fails_decode() {
        let bytes = Bytes::from_static(&[0x2A, 0x00, 0x00]);
        assert!(ChatUpdate::try_from(bytes).is_err());
    }

    #[test]
    fn chat_response_success_has_no_error() {
        let response = ChatResponse {
            result: 1,
            error: None,
            chat_type: chat_type::ALL,
            chat_index: 7,
        };
        assert_eq!(response.byte_size(), 3);
        let bytes: Bytes = response.clone().into();
        assert_eq!(bytes.as_ref(), &[0x01, 0x01, 0x07]);
        assert_eq!(ChatResponse::try_from(bytes).unwrap(), response);
    }

    #[test]
    fn chat_response_error_carries_code() {
        let response = ChatResponse {
            result: 2,
            error: Some(0x0003),
            chat_type: chat_type::PM,
            chat_index: 9,
        };
        assert_eq!(response.byte_size(), 5);
        let bytes: Bytes = response.clone().into();
        assert_eq!(bytes.as_ref(), &[0x02, 0x03, 0x00, 0x02, 0x09]);
        assert_eq!(ChatResponse::try_from(bytes).unwrap(), response);
    }

    #[test]
    fn chat_restriction_roundtrips() {
        let restriction = ChatRestriction { seconds: 30 };
        assert_eq!(restriction.byte_size(), 1);
        let bytes: Bytes = restriction.clone().into();
        assert_eq!(bytes.as_ref(), &[30]);
        assert_eq!(ChatRestriction::try_from(bytes).unwrap(), restriction);
    }

    // Not a wire packet: pins the derive's UTF-16 (`size = 2`) support —
    // the length prefix must count UTF-16 code units, not Rust bytes —
    // for future vSRO-compatible packets, since no live packet uses it now.
    #[derive(Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
    struct WideString {
        #[sro_packet(size = 2)]
        text: String,
    }

    #[test]
    fn utf16_strings_count_code_units() {
        let wide = WideString {
            text: "안녕".to_string(),
        };
        assert_eq!(wide.byte_size(), 6);
        let bytes: Bytes = wide.clone().into();
        assert_eq!(bytes.as_ref(), &[0x02, 0x00, 0x48, 0xC5, 0x55, 0xB1]);
        assert_eq!(WideString::try_from(bytes).unwrap(), wide);
    }
}
