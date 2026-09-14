//! Guild leadership: master transfer (0x7103/0xB103), the three election
//! opcodes (0x7105/0x7106/0x7107 and their acks) and the GP-contribution
//! history (0x7501/0xB501).
//!
//! Idea: three of the four acks are members of the guild ack cluster and reuse
//! the one body form from [`crate::agent::guild`]. The two that are not — the
//! election roster 0xB106 and the GP history 0xB501 — are the only bodies in
//! the whole guild family with a repeating record, and both are recovered from
//! the original's reader with a matching server writer or a resolved CRT call,
//! so neither is guessed.
//!
//! The `0xB501` record's leading `u32` is a **`time_t`**: the original feeds it
//! to `FUN_00b46dd4`, which is the CRT's `_localtime32_s` (it memsets a `tm`,
//! calls `___tzset`/`__gmtime32_s` and does the `tm_year + 1900` fix-ups). That
//! is what settles "date or amount" for the two `u32`s in the row.
//!
//! Layouts and evidence: `docs/net-guild-leadership.md`.

use bevy::prelude::Message;
use bytes::{BufMut, Bytes, BytesMut};

use crate::agent::guild::guild_op_ack;

use sro_macro::ByteSize;
use sro_macro::Deserialize;
use sro_macro::SerializationError;
use sro_macro::Serialize;
use sro_macro_derive::*;

/// The one election error code recovered from the binary: the original answers
/// it with `UIIT_MSG_MRELEASEERR_NOTVOTETIME` instead of the generic error box
/// (`FUN_008857b0`). "MRELEASE" is *master release* — the impeachment vote.
pub const GUILD_ELECTION_NOT_VOTE_TIME: u16 = 0x4C33;

// --- C->S -------------------------------------------------------------------

/// 0x7103 — hand the guild master role over. Builder `FUN_0081fe80@23`,
/// body `b4 b4`.
///
/// Its ack shows the original's own name for this: the only `UIIT_*` strings in
/// `FUN_00889680` are `UIIT_MSG_MLEAVE_SUCCESS` (master *leave*) and the
/// message-box title. Which of the two `u32`s is the successor is [U] — the
/// builder shows two four-byte fields and no formatting site names either.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct GuildMasterTransferRequest {
    /// [U] — verbatim.
    pub unk_u32_00: u32,
    /// [U] — verbatim.
    pub unk_u32_01: u32,
}

/// 0x7105 — start the master-release (impeachment) vote.
/// Builder `FUN_0081ff50@23`, body `b4`.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct GuildElectionStartRequest {
    /// [U] — verbatim.
    pub unk_u32_00: u32,
}

/// 0x7106 — stand as a candidate / join the vote.
/// Builder `FUN_00820010@23`, body `b4`.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct GuildElectionParticipateRequest {
    /// [U] — verbatim.
    pub unk_u32_00: u32,
}

/// 0x7107 — cast a vote. Builder `FUN_008200d0@23`, body `b4 b4 b1`.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct GuildElectionVoteRequest {
    /// [U] — verbatim.
    pub unk_u32_00: u32,
    /// [U] — verbatim.
    pub unk_u32_01: u32,
    /// [U] — verbatim.
    pub unk_u8_00: u8,
}

/// 0x7501 — ask for the GP contribution history.
/// Builder `FUN_008206c0@23`, body `b4`.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct GuildGpHistoryRequest {
    /// [U] — verbatim.
    pub unk_u32_00: u32,
}

// --- S->C -------------------------------------------------------------------

guild_op_ack! {
    /// 0xB103 — ack for [`GuildMasterTransferRequest`], handler `FUN_00889680`.
    ///
    /// The success arm carries **no** successor id: any roster change arrives
    /// through `0x38F5`/`0x3100`, so a consumer must not expect one here.
    GuildMasterTransferAck
}

guild_op_ack! {
    /// 0xB105 — ack for [`GuildElectionStartRequest`], handler `FUN_008857b0`.
    /// [`GUILD_ELECTION_NOT_VOTE_TIME`] is the one code it special-cases.
    GuildElectionStartAck
}

guild_op_ack! {
    /// 0xB107 — ack for [`GuildElectionVoteRequest`], handler `FUN_00881d80`.
    ///
    /// Note the VA: `guild-alliance.md` misattributes this handler to `0xB0FB`.
    /// Its error box uses category `0x15`, shared only with 0xB106 — the
    /// binary's own grouping of the election subsystem.
    GuildElectionVoteAck
}

/// One row of the election roster in [`GuildElectionRoster`].
#[derive(Serialize, Deserialize, ByteSize, Clone, Debug, Default, PartialEq, Eq)]
pub struct GuildElectionEntry {
    /// A character id; the original looks it up per row.
    pub uid: u32,
    /// [U] — one byte per row.
    pub flag: u8,
}

/// 0xB106 — ack for [`GuildElectionParticipateRequest`], handler `FUN_00881d00`.
///
/// The only ack in the guild family with a repeating record, and the one whose
/// source module is named by an assert in its callee:
/// `CharacterDependentData_Vote.cpp`. On success the original builds the UI
/// literally called `"GuildMasterElection"`.
///
/// Field-for-field agreement with the server writer `FUN_005e7f00 @5e7f29`:
/// `u8 result=1`, `u32`, then the tail writer emits `u32`, `u8 count`, and per
/// entry `u32` + `u8`. `vote_value` is [U] — deadline or tally.
#[derive(Message, Clone, Debug, PartialEq)]
pub struct GuildElectionRoster {
    pub result: u8,
    pub vote_id: u32,
    /// [U] — a deadline timestamp or a running tally; the assert string that
    /// would name it is not in the string index.
    pub vote_value: u32,
    pub entries: Vec<GuildElectionEntry>,
    pub error_code: Option<u16>,
}

impl GuildElectionRoster {
    pub fn is_success(&self) -> bool {
        self.result == 1
    }
}

impl TryFrom<Bytes> for GuildElectionRoster {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, SerializationError> {
        let mut c = Cursor::new(&value);
        let result = c.u8()?;
        if result != 1 {
            return Ok(GuildElectionRoster {
                result,
                vote_id: 0,
                vote_value: 0,
                entries: Vec::new(),
                error_code: c.u16().ok(),
            });
        }
        let vote_id = c.u32()?;
        let vote_value = c.u32()?;
        let count = c.u8()?;
        let mut entries = Vec::with_capacity(count as usize);
        for _ in 0..count {
            entries.push(GuildElectionEntry {
                uid: c.u32()?,
                flag: c.u8()?,
            });
        }
        Ok(GuildElectionRoster {
            result,
            vote_id,
            vote_value,
            entries,
            error_code: None,
        })
    }
}

impl From<GuildElectionRoster> for Bytes {
    fn from(p: GuildElectionRoster) -> Self {
        let mut buf = BytesMut::new();
        buf.put_u8(p.result);
        if p.result == 1 {
            buf.put_u32_le(p.vote_id);
            buf.put_u32_le(p.vote_value);
            buf.put_u8(p.entries.len() as u8);
            for e in &p.entries {
                buf.put_u32_le(e.uid);
                buf.put_u8(e.flag);
            }
        } else if let Some(code) = p.error_code {
            buf.put_u16_le(code);
        }
        buf.freeze()
    }
}

/// One GP-contribution row.
///
/// `timestamp` is a 32-bit `time_t`: the original passes it to `FUN_00b46dd4`,
/// which the corpus shows to be the CRT's `_localtime32_s`. That is what
/// distinguishes it from `gp`, the amount.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuildGpHistoryEntry {
    pub timestamp: u32,
    pub member_name: String,
    pub gp: u32,
    /// [U] — an index into a client-side label table (`FUN_005b96a0`), i.e. a
    /// textdata artefact rather than a decompile one.
    pub reason: u8,
}

/// 0xB501 — the GP contribution log, handler `FUN_00885c20`; the per-row
/// formatter resolves `UIIT_MSG_GUILD_GP_HISTORY`, which is what names it.
#[derive(Message, Clone, Debug, PartialEq)]
pub struct GuildGpHistoryResponse {
    pub result: u8,
    pub entries: Vec<GuildGpHistoryEntry>,
    pub error_code: Option<u16>,
}

impl GuildGpHistoryResponse {
    pub fn is_success(&self) -> bool {
        self.result == 1
    }
}

impl TryFrom<Bytes> for GuildGpHistoryResponse {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, SerializationError> {
        let mut c = Cursor::new(&value);
        let result = c.u8()?;
        if result != 1 {
            return Ok(GuildGpHistoryResponse {
                result,
                entries: Vec::new(),
                error_code: c.u16().ok(),
            });
        }
        let count = c.u8()?;
        let mut entries = Vec::with_capacity(count as usize);
        for _ in 0..count {
            entries.push(GuildGpHistoryEntry {
                timestamp: c.u32()?,
                member_name: c.string()?,
                gp: c.u32()?,
                reason: c.u8()?,
            });
        }
        Ok(GuildGpHistoryResponse {
            result,
            entries,
            error_code: None,
        })
    }
}

impl From<GuildGpHistoryResponse> for Bytes {
    fn from(p: GuildGpHistoryResponse) -> Self {
        let mut buf = BytesMut::new();
        buf.put_u8(p.result);
        if p.result == 1 {
            buf.put_u8(p.entries.len() as u8);
            for e in &p.entries {
                buf.put_u32_le(e.timestamp);
                buf.put_u16_le(e.member_name.len() as u16);
                buf.extend_from_slice(e.member_name.as_bytes());
                buf.put_u32_le(e.gp);
                buf.put_u8(e.reason);
            }
        } else if let Some(code) = p.error_code {
            buf.put_u16_le(code);
        }
        buf.freeze()
    }
}

struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], SerializationError> {
        let end = self.pos.checked_add(n).ok_or_else(short)?;
        let slice = self.buf.get(self.pos..end).ok_or_else(short)?;
        self.pos = end;
        Ok(slice)
    }
    fn u8(&mut self) -> Result<u8, SerializationError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, SerializationError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32, SerializationError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn string(&mut self) -> Result<String, SerializationError> {
        let len = self.u16()? as usize;
        Ok(String::from_utf8_lossy(self.take(len)?).into_owned())
    }
}

fn short() -> SerializationError {
    SerializationError::IoError(std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        "short guild leadership body",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_leadership_requests_carry_their_builder_widths() {
        let transfer = GuildMasterTransferRequest {
            unk_u32_00: 1,
            unk_u32_01: 2,
        };
        let wire: Bytes = transfer.clone().into();
        assert_eq!(wire.len(), 8);
        assert_eq!(
            GuildMasterTransferRequest::try_from(wire).unwrap(),
            transfer
        );

        let vote = GuildElectionVoteRequest {
            unk_u32_00: 1,
            unk_u32_01: 2,
            unk_u8_00: 3,
        };
        let wire: Bytes = vote.clone().into();
        assert_eq!(wire.len(), 9);
        assert_eq!(GuildElectionVoteRequest::try_from(wire).unwrap(), vote);

        let history = GuildGpHistoryRequest { unk_u32_00: 9 };
        let wire: Bytes = history.clone().into();
        assert_eq!(wire.as_ref(), &[9, 0, 0, 0]);
        assert_eq!(GuildGpHistoryRequest::try_from(wire).unwrap(), history);
    }

    /// The three plain acks are the shared cluster form, including the one
    /// recovered error constant.
    #[test]
    fn the_election_start_ack_carries_the_not_vote_time_code() {
        let refused = Bytes::from_static(&[0x02, 0x33, 0x4C]);
        let ack = GuildElectionStartAck::try_from(refused.clone()).unwrap();
        assert_eq!(ack.error_code, Some(GUILD_ELECTION_NOT_VOTE_TIME));
        assert_eq!(Bytes::from(ack), refused);

        let ok = Bytes::from_static(&[0x01]);
        assert!(GuildMasterTransferAck::try_from(ok.clone())
            .unwrap()
            .is_success());
        assert!(GuildElectionVoteAck::try_from(ok).unwrap().is_success());
    }

    /// 10 + 5*N bytes on success — the shape the server writer emits
    /// field-for-field.
    #[test]
    fn the_election_roster_is_ten_bytes_plus_five_per_entry() {
        let msg = GuildElectionRoster {
            result: 1,
            vote_id: 7,
            vote_value: 0x1234,
            entries: vec![
                GuildElectionEntry { uid: 11, flag: 0 },
                GuildElectionEntry { uid: 22, flag: 1 },
            ],
            error_code: None,
        };
        let wire: Bytes = msg.clone().into();
        assert_eq!(wire.len(), 10 + 5 * 2);
        assert_eq!(GuildElectionRoster::try_from(wire).unwrap(), msg);
    }

    /// The error arm is the canonical three bytes and carries no roster.
    #[test]
    fn a_refused_election_roster_is_three_bytes() {
        let refused = Bytes::from_static(&[0x02, 0x0E, 0x1C]);
        let decoded = GuildElectionRoster::try_from(refused.clone()).unwrap();
        assert!(!decoded.is_success());
        assert_eq!(decoded.error_code, Some(0x1C0E));
        assert!(decoded.entries.is_empty());
        assert_eq!(Bytes::from(decoded), refused);
    }

    /// A GP-history row is `{time_t, name, amount, reason}` — the timestamp
    /// first, which is what the `_localtime32_s` call settles.
    #[test]
    fn a_gp_history_row_leads_with_the_timestamp_and_ends_with_the_reason() {
        let msg = GuildGpHistoryResponse {
            result: 1,
            entries: vec![GuildGpHistoryEntry {
                timestamp: 0x5F00_0000,
                member_name: "Kong".into(),
                gp: 1500,
                reason: 3,
            }],
            error_code: None,
        };
        let wire: Bytes = msg.clone().into();
        // 1 result + 1 count + (4 + 2 + 4 + 4 + 1)
        assert_eq!(wire.len(), 2 + 15);
        assert_eq!(&wire[2..6], &0x5F00_0000u32.to_le_bytes());
        assert_eq!(*wire.last().unwrap(), 3);
        assert_eq!(GuildGpHistoryResponse::try_from(wire).unwrap(), msg);
    }

    /// The client stops reading the rows when its history window is absent —
    /// a client quirk, not an optional wire field. A body that says `count` and
    /// then ends is therefore a short read, not a valid empty log.
    #[test]
    fn a_history_body_that_ends_after_the_count_is_an_error_not_an_empty_log() {
        assert!(GuildGpHistoryResponse::try_from(Bytes::from_static(&[0x01, 0x02])).is_err());
    }
}
