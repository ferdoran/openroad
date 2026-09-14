//! Academy ("Training Camp") wire opcodes: the notice edit 0x7477, the
//! matching-board list request 0x747D and its 0xB47D ack. 0x3C81 is
//! deliberately absent — see below.
//!
//! Idea: this family was documented as *capture-gated* because xBot decodes
//! none of it. That premise is retired: the original client's own builders and
//! handlers are the source, and `CMsgStreamBuffer` reads/writes exactly one
//! field per call, so the layouts fall out of the decompiles
//! (`docs/re/net/outbound/academy-arena.md`, `docs/re/net/inbound/academy.md`).
//! What the decompiles cannot supply is *meaning*: a width and an order are
//! `[V]`, a field's name is `[S]`, and a branch's semantics stay `[U]` until a
//! capture exists. This module types what is `[V]` and refuses to name what is
//! not — the 0xB47D record block is kept as raw bytes rather than decoded into
//! invented field names.
//!
//! **0x3C81 (SERVER_ACADEMY_DATA) is not here on purpose.** Its handler
//! (`sro_client.exe@FUN_008986c0`) contains no read call at all: the original
//! consumes zero bytes of it, so there is no layout to model. It is listed in
//! the client's known-and-ignored opcodes instead
//! (`client/src/plugins/net/plugin.rs`).
//!
//! Layouts: `docs/net-academy-0x3C81.md`.

use bevy::prelude::Message;
use bytes::{BufMut, Bytes, BytesMut};

use sro_macro::ByteSize;
use sro_macro::Deserialize;
use sro_macro::SerializationError;
use sro_macro::Serialize;
use sro_macro_derive::*;

/// 0x7477 — client → server: edit the academy notice.
///
/// Builder `FUN_00824560` writes `strS strS` and nothing else
/// (`docs/re/net/outbound/academy-arena.md:212`). Which string is the title
/// and which the body is `[S]`, taken from the guild notice edit 0x70F9, whose
/// builder writes the same pair in the same order.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct AcademyNoticeEditRequest {
    pub title: String,
    pub notice: String,
}

/// 0x747D — client → server: fetch a page of the academy matching board.
///
/// Builder `FUN_0081d3b0` writes exactly one byte
/// (`docs/re/net/outbound/academy-arena.md:282`). That the byte is a page
/// index is `[S]`, by symmetry with the party match list, which pages the same
/// way — the decompile proves only the width.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct AcademyMatchListRequest {
    pub page: u8,
}

/// 0xB47D — server → client: the matching-board page.
///
/// Read trace of `FUN_00890ab0` (`docs/re/net/inbound/academy.md:486-511`):
/// one byte at depth 1, then **three** bytes at depth 2 followed by a record
/// loop at depth 4, with a `u16` read on the other depth-2 branch. So the
/// shape is the family's usual `result` + success/failure arms — but the three
/// header bytes and the record fields have no names anywhere in the corpus,
/// and no `packet_dump/0xb47d.log` exists to bind them.
///
/// Therefore: the arms are typed, the bytes inside them are not renamed.
/// [`Self::records`] is the undecoded remainder, and the per-record widths
/// (`4 4 1 4 1 1 4 strS 4 1 4 4`) stay in the RE doc until a capture can say
/// what they mean. Inventing twelve field names here is exactly the unsourced
/// value the project's doctrine forbids.
#[derive(Message, Clone, Debug, PartialEq)]
pub struct AcademyMatchListResponse {
    pub result: u8,
    /// Success arm only: the three bytes the handler reads before its record
    /// loop (`local_d8`, `local_d4`, `local_d9`, L61-63). Which is the page
    /// index, which the page count and which the record count is `[U]`.
    pub header: Option<[u8; 3]>,
    /// Success arm only: the record block, undecoded (see the type docs).
    pub records: Bytes,
    /// Failure arm only: the `u16` read at L191.
    pub error_code: Option<u16>,
}

impl AcademyMatchListResponse {
    /// The original branches on `result == 1`; every other value takes the
    /// error arm (the shared ack shape of the whole academy family).
    pub fn is_success(&self) -> bool {
        self.result == 1
    }
}

impl TryFrom<Bytes> for AcademyMatchListResponse {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, SerializationError> {
        let Some(&result) = value.first() else {
            // An empty body is not a decode error we can act on; report the
            // shape we saw rather than failing the whole receive loop.
            return Ok(AcademyMatchListResponse {
                result: 0,
                header: None,
                records: Bytes::new(),
                error_code: None,
            });
        };
        let rest = value.slice(1..);
        if result == 1 {
            let header = rest.get(0..3).map(|h| [h[0], h[1], h[2]]);
            let records = if header.is_some() {
                rest.slice(3..)
            } else {
                rest
            };
            Ok(AcademyMatchListResponse {
                result,
                header,
                records,
                error_code: None,
            })
        } else {
            let error_code = rest.get(0..2).map(|b| u16::from_le_bytes([b[0], b[1]]));
            Ok(AcademyMatchListResponse {
                result,
                header: None,
                records: Bytes::new(),
                error_code,
            })
        }
    }
}

impl From<AcademyMatchListResponse> for Bytes {
    fn from(p: AcademyMatchListResponse) -> Self {
        let mut buf = BytesMut::new();
        buf.put_u8(p.result);
        if let Some(header) = p.header {
            buf.put_slice(&header);
        }
        buf.extend_from_slice(&p.records);
        if let Some(code) = p.error_code {
            buf.put_u16_le(code);
        }
        buf.freeze()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notice_edit_writes_two_length_prefixed_strings() {
        let request = AcademyNoticeEditRequest {
            title: "hi".to_string(),
            notice: "there".to_string(),
        };
        let bytes: Bytes = request.clone().into();
        assert_eq!(
            bytes.as_ref(),
            &[2, 0, b'h', b'i', 5, 0, b't', b'h', b'e', b'r', b'e']
        );
        assert_eq!(AcademyNoticeEditRequest::try_from(bytes).unwrap(), request);
    }

    #[test]
    fn match_list_request_is_a_single_byte() {
        let request = AcademyMatchListRequest { page: 3 };
        let bytes: Bytes = request.clone().into();
        assert_eq!(bytes.as_ref(), &[3]);
        assert_eq!(AcademyMatchListRequest::try_from(bytes).unwrap(), request);
    }

    #[test]
    fn match_list_response_splits_the_two_arms_without_naming_the_records() {
        // success: result, three header bytes, then the record block whole
        let raw = Bytes::from_static(&[1, 0, 2, 5, 0xAA, 0xBB]);
        let ack = AcademyMatchListResponse::try_from(raw.clone()).unwrap();
        assert!(ack.is_success());
        assert_eq!(ack.header, Some([0, 2, 5]));
        assert_eq!(&ack.records[..], &[0xAA, 0xBB]);
        assert_eq!(ack.error_code, None);
        let back: Bytes = ack.into();
        assert_eq!(back, raw);

        // failure: result, u16 error, no records
        let raw = Bytes::from_static(&[2, 0x0E, 0x1C]);
        let ack = AcademyMatchListResponse::try_from(raw.clone()).unwrap();
        assert!(!ack.is_success());
        assert_eq!(ack.error_code, Some(0x1C0E));
        assert!(ack.records.is_empty());
        let back: Bytes = ack.into();
        assert_eq!(back, raw);
    }

    #[test]
    fn match_list_response_degrades_instead_of_erroring() {
        // no capture exists for this opcode, so a body that does not match the
        // read trace must not kill the receive loop
        let ack = AcademyMatchListResponse::try_from(Bytes::new()).unwrap();
        assert_eq!(ack.result, 0);
        assert_eq!(ack.header, None);

        // success arm too short for the three header bytes: keep them raw
        let ack = AcademyMatchListResponse::try_from(Bytes::from_static(&[1, 9])).unwrap();
        assert_eq!(ack.header, None);
        assert_eq!(&ack.records[..], &[9]);
    }
}
