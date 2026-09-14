use std::net::Ipv4Addr;

use bevy::prelude::Message;

use sro_macro::ByteSize;
use sro_macro::Deserialize;
use sro_macro::SerializationError;
use sro_macro::Serialize;
use sro_macro_derive::*;

#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug)]
pub struct ShardListRequest;

/// 0xA106 — the gateway's answer to the farm ping: a `u8`-counted list of
/// farms, nothing else. The original parses it inline in the login-UI pump as
/// a plain loop (`0086bfc0:142` reads the count, `:146-147` each entry's id
/// and address, `:208` bounds the loop), so a count of `0` is an empty list and
/// the packet ends there — there is no success flag and no error code.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct ShardListPingResponse {
    pub farms: Vec<Farm>,
}

#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct Farm {
    pub id: u8,
    /// Four octets in wire order: the original's dotted-quad formatting at
    /// `0086bfc0:161-162` prints the first wire byte as the first octet.
    pub ip: Ipv4Addr,
}

#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug)]
pub struct ShardListPingRequest;

#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug)]
pub struct ShardListResponse {
    #[sro_packet(list_type = "has-more")]
    pub farms: Vec<FarmDetails>,
    #[sro_packet(list_type = "has-more")]
    pub shards: Vec<Shard>,
}

#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug)]
pub struct FarmDetails {
    pub id: u8,
    pub name: String,
}

#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug)]
pub struct Shard {
    pub id: u16,
    pub name: String,
    pub online_count: u16,
    pub capacity: u16,
    pub is_operating: bool,
    pub farm_id: u8,
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    /// The maintainer's capture, all 9 lines of `packet_dump/0xa106.log`:
    /// `01` count, `14` farm id, `7f 00 00 01` = 127.0.0.1 — the first wire
    /// byte is the first octet (`0086bfc0:161-162`).
    const CAPTURED: &[u8] = &[0x01, 0x14, 0x7f, 0x00, 0x00, 0x01];

    #[test]
    fn captured_single_farm_roundtrips() {
        let decoded =
            ShardListPingResponse::try_from(Bytes::from_static(CAPTURED)).expect("decodes");
        assert_eq!(
            decoded,
            ShardListPingResponse {
                farms: vec![Farm {
                    id: 0x14,
                    ip: Ipv4Addr::new(127, 0, 0, 1),
                }],
            }
        );
        let encoded: Bytes = decoded.into();
        assert_eq!(encoded.as_ref(), CAPTURED);
    }

    #[test]
    fn a_two_farm_body_decodes_to_two_entries() {
        // Synthetic: the maintainer's server only ever has one farm, which is
        // exactly why the old bool-plus-optional model round-tripped.
        let body = Bytes::from_static(&[
            0x02, 0x14, 0x7f, 0x00, 0x00, 0x01, 0x15, 0xc0, 0xa8, 0x01, 0x0a,
        ]);
        let decoded = ShardListPingResponse::try_from(body).expect("decodes");
        assert_eq!(decoded.farms.len(), 2);
        assert_eq!(decoded.farms[1].id, 0x15);
        assert_eq!(decoded.farms[1].ip, Ipv4Addr::new(192, 168, 1, 10));
    }

    #[test]
    fn a_zero_count_is_an_empty_list_not_an_error_code() {
        let decoded =
            ShardListPingResponse::try_from(Bytes::from_static(&[0x00])).expect("decodes");
        assert!(decoded.farms.is_empty());
    }
}
