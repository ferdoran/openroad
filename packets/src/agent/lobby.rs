use bevy::prelude::Message;
use sro_macro::ByteSize;
use sro_macro::Deserialize;
use sro_macro::SerializationError;
use sro_macro::Serialize;
use sro_macro_derive::*;

#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug)]
pub struct CharacterSelectionActionRequest {
    pub action: CharacterSelectionAction,
    /// Actions that target an existing slot by name (Delete, CheckName,
    /// Restore) carry just the name — identical wire shape, no presence flag.
    /// Create carries its name inside [`CharacterCreate`] instead.
    ///
    /// go-sro reads exactly one string for all three
    /// (`handler/lobby/char_selection_action_handler.go:141-146`). Leaving
    /// `Restore` out sent a bare `[05]` with the name silently dropped.
    #[sro_packet(
        when = "action == CharacterSelectionAction::Delete || action == CharacterSelectionAction::CheckName || action == CharacterSelectionAction::Restore"
    )]
    pub name: Option<String>,
    /// New-character payload, present only for `Create`.
    #[sro_packet(when = "action == CharacterSelectionAction::Create")]
    pub create: Option<CharacterCreate>,
}

/// New-character creation body (0x7007 `action == Create`).
///
/// EXPERIMENTAL: outbound requests never reach `packet_dump/` (we can't
/// capture our own send), so the field set/order is transcribed from the
/// public v1.188 spec (skrillax `silkroad-protocol`, SilkroadDoc) via
/// `docs/net-char-select-0x7007.md` and not yet capture-verified. The wire is
/// the name, the starter body model, the body scale, then a fixed set of four
/// starter-item ref-obj-ids (chest/pants/boots/weapon) — not a length-prefixed
/// list.
#[derive(Serialize, Deserialize, ByteSize, Clone, Debug)]
pub struct CharacterCreate {
    /// Candidate character name.
    pub name: String,
    /// Starter body `CharacterData.txt` ref-obj-id (selects race/gender/model).
    pub ref_obj_id: u32,
    /// Body scale (0..=255).
    pub scale: u8,
    /// Starter chest-armor ref-obj-id.
    pub chest: u32,
    /// Starter leg-armor ref-obj-id.
    pub pants: u32,
    /// Starter boots ref-obj-id.
    pub boots: u32,
    /// Starter weapon ref-obj-id.
    pub weapon: u32,
}

#[derive(Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
#[repr(u8)]
pub enum CharacterSelectionAction {
    #[sro_packet(value = 1)]
    Create,
    #[sro_packet(value = 2)]
    List,
    #[sro_packet(value = 3)]
    Delete,
    #[sro_packet(value = 4)]
    CheckName,
    #[sro_packet(value = 5)]
    Restore,
    #[sro_packet(value = 9)]
    ShowJobSpread,
    #[sro_packet(value = 0x10)]
    AssignJob,
}

#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug)]
pub struct CharacterSelectionActionResponse {
    pub action: CharacterSelectionAction,
    pub result: u8,
    #[sro_packet(when = "result == 1 && action == CharacterSelectionAction::List")]
    pub characters: Option<CharacterData>,
    #[sro_packet(when = "result == 2")]
    pub error_code: Option<u16>,
}

#[derive(Serialize, Deserialize, ByteSize, Clone, Debug)]
pub struct CharacterData {
    pub count: u8,
    #[sro_packet(list_type = "by-size-field", size_field = "count")]
    pub characters: Vec<LobbyCharacter>,
}
#[derive(Serialize, Deserialize, ByteSize, Clone, Debug)]
pub struct LobbyCharacter {
    pub ref_obj_id: u32,
    pub name: String,
    pub scale: u8,
    pub level: u8,
    pub exp_offset: u64,
    pub str: u16,
    pub int: u16,
    pub stat_points: u16,
    pub hp: u32,
    pub mp: u32,
    pub is_deleting: bool,
    #[sro_packet(when = "is_deleting")]
    pub deletion_time: Option<u32>,
    pub guild_member_class: u8,
    pub is_guild_rename_required: bool,
    #[sro_packet(when = "is_guild_rename_required")]
    pub current_guild_name: Option<String>,
    pub academy_member_class: u8,
    pub char_items: Vec<LobbyItem>,
    pub avatar_items: Vec<LobbyItem>,
}

#[derive(Serialize, Deserialize, ByteSize, Clone, Debug)]
pub struct LobbyItem {
    pub id: u32,
    pub plus: u8,
}

/// 0x7001 — asks the agent server to enter the world with the given
/// character. The server answers with [`CharacterJoinResponse`] and then
/// starts the ingame spawn sequence (character data, position, ... — not
/// implemented yet).
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug)]
pub struct CharacterJoinRequest {
    pub character_name: String,
}

/// 0xB001 — result of [`CharacterJoinRequest`] (1 = ok, 2 = error).
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug)]
pub struct CharacterJoinResponse {
    pub result: u8,
    #[sro_packet(when = "result == 2")]
    pub error: Option<u16>,
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::*;

    // `when`-conditional fields must serialize without the presence flag a
    // plain Option carries — these tests pin the exact wire bytes.
    #[test]
    fn delete_request_carries_name_without_flag_byte() {
        let request = CharacterSelectionActionRequest {
            action: CharacterSelectionAction::Delete,
            name: Some("Foo".to_string()),
            create: None,
        };
        assert_eq!(request.byte_size(), 6);
        let bytes: Bytes = request.into();
        assert_eq!(bytes.as_ref(), &[0x03, 0x03, 0x00, b'F', b'o', b'o']);
    }

    /// #202: Restore (action 5) carries the name in exactly the same shape as
    /// Delete — go-sro reads one string for Delete/CheckName/Restore alike.
    #[test]
    fn restore_request_carries_name_without_flag_byte() {
        let request = CharacterSelectionActionRequest {
            action: CharacterSelectionAction::Restore,
            name: Some("Foo".to_string()),
            create: None,
        };
        assert_eq!(request.byte_size(), 6);
        let bytes: Bytes = request.into();
        assert_eq!(bytes.as_ref(), &[0x05, 0x03, 0x00, b'F', b'o', b'o']);
    }

    #[test]
    fn list_request_is_a_single_action_byte() {
        let request = CharacterSelectionActionRequest {
            action: CharacterSelectionAction::List,
            name: None,
            create: None,
        };
        assert_eq!(request.byte_size(), 1);
        let bytes: Bytes = request.into();
        assert_eq!(bytes.as_ref(), &[0x02]);
    }

    #[test]
    fn check_name_request_carries_name_without_flag_byte() {
        let request = CharacterSelectionActionRequest {
            action: CharacterSelectionAction::CheckName,
            name: Some("Foo".to_string()),
            create: None,
        };
        assert_eq!(request.byte_size(), 6);
        let bytes: Bytes = request.into();
        assert_eq!(bytes.as_ref(), &[0x04, 0x03, 0x00, b'F', b'o', b'o']);
    }

    #[test]
    fn create_request_serializes_name_model_scale_and_starter_items() {
        let request = CharacterSelectionActionRequest {
            action: CharacterSelectionAction::Create,
            name: None,
            create: Some(CharacterCreate {
                name: "Foo".to_string(),
                ref_obj_id: 0x04030201,
                scale: 0x2A,
                chest: 0x11111111,
                pants: 0x22222222,
                boots: 0x33333333,
                weapon: 0x44444444,
            }),
        };
        assert_eq!(request.byte_size(), 27);
        let bytes: Bytes = request.into();
        assert_eq!(
            bytes.as_ref(),
            &[
                0x01, // action = Create
                0x03, 0x00, b'F', b'o', b'o', // name = "Foo"
                0x01, 0x02, 0x03, 0x04, // ref_obj_id (LE)
                0x2A, // scale
                0x11, 0x11, 0x11, 0x11, // chest
                0x22, 0x22, 0x22, 0x22, // pants
                0x33, 0x33, 0x33, 0x33, // boots
                0x44, 0x44, 0x44, 0x44, // weapon
            ]
        );
    }

    #[test]
    fn join_request_serializes_name() {
        let request = CharacterJoinRequest {
            character_name: "Foo".to_string(),
        };
        assert_eq!(request.byte_size(), 5);
        let bytes: Bytes = request.into();
        assert_eq!(bytes.as_ref(), &[0x03, 0x00, b'F', b'o', b'o']);
    }

    #[test]
    fn join_response_success_has_no_error() {
        let response = CharacterJoinResponse::try_from(Bytes::from_static(&[0x01])).unwrap();
        assert_eq!(response.result, 1);
        assert!(response.error.is_none());
    }

    #[test]
    fn join_response_error_reads_code() {
        let response =
            CharacterJoinResponse::try_from(Bytes::from_static(&[0x02, 0x03, 0x04])).unwrap();
        assert_eq!(response.result, 2);
        assert_eq!(response.error, Some(0x0403));
    }
}
