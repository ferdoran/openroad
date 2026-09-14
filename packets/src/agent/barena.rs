//! Battle Arena operation broadcast — `0x34D2` `SERVER_AGENT_BARENA_OPERATION`.
//!
//! Idea: this opcode is a **tagged union whose tag is one byte and whose body
//! length changes per tag**, so it is decoded as `{op, body}` first and
//! interpreted second. Everything the client needs (which phase the scheduler
//! is broadcasting, and, inside the `0xFF` update sub-stream, the live score
//! and rank rows) is read out of `body` by accessor methods that return
//! `Option` rather than by a derive that would have to guess a fixed layout.
//! That shape is what lets the ops we have never observed pass through intact
//! instead of failing the whole packet.
//!
//! Evidence: `docs/re/systems/battle-arena.md` §3a. Ops `02/03/05/0D/0E` are
//! **capture-verified** against our own `packet_dump/0x34d2.log` (10 lines,
//! two complete 30-minute arena cycles, 2026-08-10) — the fixtures in the
//! tests below are those exact byte strings. The remaining ops and the whole
//! `0xFF` sub-stream are `[S]` from the SilkroadDoc wiki
//! (`AGENT_BARENA_OPERATION.md`, `BArenaUpdate.md`; no-licence class — facts
//! and field layouts only, no code or text copied) and are therefore decoded
//! tolerantly: a short or surprising body yields `None`, never an error.

use bevy::prelude::Message;
use bytes::{BufMut, Bytes, BytesMut};

use sro_macro::SerializationError;

/// `BArenaMatchType` — which population the round is drawn from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BArenaMatchType {
    Random,
    Party,
    Guild,
    Job,
    Disabled,
}

impl BArenaMatchType {
    pub fn from_wire(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Random),
            1 => Some(Self::Party),
            2 => Some(Self::Guild),
            3 => Some(Self::Job),
            4 => Some(Self::Disabled),
            _ => None,
        }
    }
}

/// `BArenaGameTypeMask` — the game modes registration is open for. A mask, not
/// an enum: `RegistrationBegin` advertises the set.
pub const GAME_TYPE_MASK_CTF: u16 = 0x20;
pub const GAME_TYPE_MASK_SCORE: u16 = 0x40;

/// The two arena teams. The wire values are `Red = 0`, `Blue = 1`.
///
/// `[U]` which crest art belongs to which value: the arena ships
/// `interface/guild/gil_arena_{tiger,dragon}_team.ddj`, and nothing in the
/// data or in our capture ties tiger/dragon to red/blue. The mapping is
/// deliberately absent here rather than guessed — resolving observation is one
/// screenshot of a live arena rank board.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BArenaTeam {
    Red,
    Blue,
}

impl BArenaTeam {
    pub fn from_wire(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Red),
            1 => Some(Self::Blue),
            _ => None,
        }
    }
}

/// The operation byte. Named after the ops the spec declares; anything else
/// keeps its raw value in [`BArenaOperation::op`] and is reported as `None`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BArenaOp {
    /// 0x02 — registration is open, and the body names the modes.
    RegistrationBegin,
    /// 0x03 — registration closed.
    RegistrationEnd,
    /// 0x04 — the round is running.
    GameStart,
    /// 0x05 — the round is over. Two-byte body: match type only.
    GameEnd,
    /// 0x08 — the round's length in milliseconds.
    GameStartTime,
    /// 0x09 — win/lose/draw plus the Arena Coin and skill-exp reward.
    GameResult,
    /// 0x0D — "starts in 5 minutes".
    GameStartAlarmIn5,
    /// 0x0E — "starts in 1 minute".
    GameStartAlarmIn1,
    /// 0xFF — the in-match update sub-stream ([`BArenaUpdate`]).
    Update,
}

impl BArenaOp {
    pub fn from_wire(byte: u8) -> Option<Self> {
        match byte {
            0x02 => Some(Self::RegistrationBegin),
            0x03 => Some(Self::RegistrationEnd),
            0x04 => Some(Self::GameStart),
            0x05 => Some(Self::GameEnd),
            0x08 => Some(Self::GameStartTime),
            0x09 => Some(Self::GameResult),
            0x0D => Some(Self::GameStartAlarmIn5),
            0x0E => Some(Self::GameStartAlarmIn1),
            0xFF => Some(Self::Update),
            _ => None,
        }
    }
}

/// One row of the `0x41` scoreboard: who, on which team, with how many points.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BArenaRankEntry {
    pub team: Option<BArenaTeam>,
    pub name: String,
    pub points: u32,
}

/// The `0xFF` update sub-stream, keyed by its own second byte.
#[derive(Clone, Debug, PartialEq)]
pub enum BArenaUpdate {
    /// `0x40` — points the local player just gained.
    GainedPoints(u32),
    /// `0x41` — the live scoreboard: both team totals plus the rank rows the
    /// board displays.
    Scoreboard {
        total_red: u32,
        total_blue: u32,
        ranks: Vec<BArenaRankEntry>,
    },
    /// `0xF0` — the match clock, in milliseconds.
    Countdown { max_time: u32, elapsed: u32 },
    /// A sub-op we have neither captured nor decoded (the `0x80..=0x89` CTF
    /// flag events, and anything else). Its bytes are kept rather than dropped.
    Other { sub_op: u8, raw: Bytes },
}

/// 0x34D2 — the scheduler's arena broadcast.
///
/// `body` is everything after the op byte, kept verbatim so an unknown op
/// survives the trip to the client (and to `packet_dump/0x34d2.log`).
#[derive(Message, Clone, Debug, PartialEq)]
pub struct BArenaOperation {
    pub op: u8,
    pub body: Bytes,
}

/// SRO's string form: a `u16` length in bytes, then the bytes themselves.
fn read_string(raw: &[u8], at: usize) -> Option<(String, usize)> {
    let len = u16::from_le_bytes(raw.get(at..at + 2)?.try_into().ok()?) as usize;
    let start = at + 2;
    let bytes = raw.get(start..start + len)?;
    Some((String::from_utf8_lossy(bytes).into_owned(), start + len))
}

fn read_u32(raw: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(raw.get(at..at + 4)?.try_into().ok()?))
}

impl BArenaOperation {
    /// The named operation, or `None` for a value the spec does not declare.
    pub fn operation(&self) -> Option<BArenaOp> {
        BArenaOp::from_wire(self.op)
    }

    /// The match type every scheduler op except the `0xFF` sub-stream carries
    /// as its first body byte.
    pub fn match_type(&self) -> Option<BArenaMatchType> {
        if self.operation() == Some(BArenaOp::Update) {
            return None;
        }
        BArenaMatchType::from_wire(*self.body.first()?)
    }

    /// The `RegistrationBegin` mode mask. Only that op carries it — this is
    /// the difference our own capture shows between the 4-byte ops and the
    /// 2-byte `GameEnd`.
    pub fn game_type_mask(&self) -> Option<u16> {
        if self.operation() != Some(BArenaOp::RegistrationBegin) {
            return None;
        }
        Some(u16::from_le_bytes(self.body.get(1..3)?.try_into().ok()?))
    }

    /// The `0xFF` sub-stream, decoded. `None` for every other op, and for a
    /// body too short to carry its own sub-op byte.
    pub fn update(&self) -> Option<BArenaUpdate> {
        if self.operation() != Some(BArenaOp::Update) {
            return None;
        }
        let sub_op = *self.body.first()?;
        let raw = &self.body[1..];
        let unknown = || BArenaUpdate::Other {
            sub_op,
            raw: self.body.slice(1..),
        };
        Some(match sub_op {
            0x40 => match read_u32(raw, 0) {
                Some(points) => BArenaUpdate::GainedPoints(points),
                None => unknown(),
            },
            0x41 => match Self::read_scoreboard(raw) {
                Some(update) => update,
                None => unknown(),
            },
            0xF0 => match (read_u32(raw, 0), read_u32(raw, 4)) {
                (Some(max_time), Some(elapsed)) => BArenaUpdate::Countdown { max_time, elapsed },
                _ => unknown(),
            },
            _ => unknown(),
        })
    }

    /// `{TotalRed:u32, TotalBlue:u32, rankCount:u8, rank×{team:u8, name, points:u32}}`.
    ///
    /// A truncated row ends the list instead of failing the packet: the count
    /// byte is uncaptured, so a miscount must not cost us the totals — the
    /// half of this message the score strip needs.
    fn read_scoreboard(raw: &[u8]) -> Option<BArenaUpdate> {
        let total_red = read_u32(raw, 0)?;
        let total_blue = read_u32(raw, 4)?;
        let count = *raw.get(8)? as usize;
        let mut at = 9;
        let mut ranks = Vec::with_capacity(count.min(64));
        for _ in 0..count {
            let Some(&team) = raw.get(at) else { break };
            let Some((name, next)) = read_string(raw, at + 1) else {
                break;
            };
            let Some(points) = read_u32(raw, next) else {
                break;
            };
            at = next + 4;
            ranks.push(BArenaRankEntry {
                team: BArenaTeam::from_wire(team),
                name,
                points,
            });
        }
        Some(BArenaUpdate::Scoreboard {
            total_red,
            total_blue,
            ranks,
        })
    }
}

impl TryFrom<Bytes> for BArenaOperation {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, SerializationError> {
        // An empty body is not a layout we can interpret, but it is also not a
        // reason to drop the connection's packet stream: report it as op 0,
        // which `operation()` reads as unknown.
        match value.first() {
            Some(&op) => Ok(BArenaOperation {
                op,
                body: value.slice(1..),
            }),
            None => Ok(BArenaOperation {
                op: 0,
                body: Bytes::new(),
            }),
        }
    }
}

impl From<BArenaOperation> for Bytes {
    fn from(p: BArenaOperation) -> Self {
        let mut buf = BytesMut::with_capacity(1 + p.body.len());
        buf.put_u8(p.op);
        buf.extend_from_slice(&p.body);
        buf.freeze()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(hex: &str) -> BArenaOperation {
        let bytes: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
        BArenaOperation::try_from(Bytes::from(bytes)).unwrap()
    }

    /// The five lines of `packet_dump/0x34d2.log`, verbatim. This is the whole
    /// capture-verified surface of the opcode: two 30-minute cycles broadcast
    /// exactly these bodies, and the body length varies per op (4/4/4/4/2)
    /// exactly as the spec says it does.
    #[test]
    fn the_captured_cycle_decodes_op_match_type_and_mask() {
        let begin = packet("02004000");
        assert_eq!(begin.operation(), Some(BArenaOp::RegistrationBegin));
        assert_eq!(begin.match_type(), Some(BArenaMatchType::Random));
        assert_eq!(begin.game_type_mask(), Some(GAME_TYPE_MASK_SCORE));

        for (hex, op) in [
            ("03000000", BArenaOp::RegistrationEnd),
            ("0d000000", BArenaOp::GameStartAlarmIn5),
            ("0e000000", BArenaOp::GameStartAlarmIn1),
        ] {
            let p = packet(hex);
            assert_eq!(p.operation(), Some(op), "{hex}");
            assert_eq!(p.match_type(), Some(BArenaMatchType::Random), "{hex}");
            // only RegistrationBegin advertises a mode mask
            assert_eq!(p.game_type_mask(), None, "{hex}");
        }

        // GameEnd is two bytes in the capture: match type and nothing else.
        let end = packet("0500");
        assert_eq!(end.operation(), Some(BArenaOp::GameEnd));
        assert_eq!(end.match_type(), Some(BArenaMatchType::Random));
        assert_eq!(end.body.len(), 1);
    }

    /// The `0x41` scoreboard is what feeds the score strip and the rank board.
    #[test]
    fn the_update_substream_reads_totals_and_rank_rows() {
        let mut body = BytesMut::new();
        body.put_u8(0xFF); // op: update sub-stream
        body.put_u8(0x41); // sub-op: scoreboard
        body.put_u32_le(17);
        body.put_u32_le(9);
        body.put_u8(2);
        for (team, name, points) in [(0u8, "Kong", 11u32), (1, "Sura", 6)] {
            body.put_u8(team);
            body.put_u16_le(name.len() as u16);
            body.put_slice(name.as_bytes());
            body.put_u32_le(points);
        }
        let p = BArenaOperation::try_from(body.freeze()).unwrap();
        assert_eq!(p.operation(), Some(BArenaOp::Update));
        // the sub-stream is not a scheduler op: it carries no match type
        assert_eq!(p.match_type(), None);
        assert_eq!(
            p.update(),
            Some(BArenaUpdate::Scoreboard {
                total_red: 17,
                total_blue: 9,
                ranks: vec![
                    BArenaRankEntry {
                        team: Some(BArenaTeam::Red),
                        name: "Kong".into(),
                        points: 11,
                    },
                    BArenaRankEntry {
                        team: Some(BArenaTeam::Blue),
                        name: "Sura".into(),
                        points: 6,
                    },
                ],
            })
        );
    }

    /// Nothing in this family is allowed to fail the packet: an uncaptured
    /// sub-op, a truncated rank row and an empty body all degrade.
    #[test]
    fn an_uncaptured_or_truncated_body_degrades_instead_of_erroring() {
        // a CTF flag event (0x80..=0x89) — spec'd, never captured, kept raw
        let flag = packet("ff8001");
        assert!(matches!(
            flag.update(),
            Some(BArenaUpdate::Other { sub_op: 0x80, .. })
        ));

        // rankCount says 3, one and a half rows follow: the totals survive
        let mut body = BytesMut::new();
        body.put_u8(0xFF);
        body.put_u8(0x41);
        body.put_u32_le(4);
        body.put_u32_le(5);
        body.put_u8(3);
        body.put_u8(0);
        body.put_u16_le(4);
        body.put_slice(b"Kong");
        body.put_u32_le(1);
        body.put_u8(1); // a second row that stops mid-record
        let p = BArenaOperation::try_from(body.freeze()).unwrap();
        match p.update() {
            Some(BArenaUpdate::Scoreboard {
                total_red,
                total_blue,
                ranks,
            }) => {
                assert_eq!((total_red, total_blue), (4, 5));
                assert_eq!(ranks.len(), 1, "the truncated row is dropped, not faked");
            }
            other => panic!("expected a scoreboard, got {other:?}"),
        }

        // an empty body is op 0 = unknown, and round-trips
        let empty = BArenaOperation::try_from(Bytes::new()).unwrap();
        assert_eq!(empty.operation(), None);
        assert_eq!(empty.update(), None);
        assert_eq!(empty.match_type(), None);
    }

    /// Every captured line round-trips byte-for-byte, which is what keeps the
    /// unknown ops honest: we re-emit what we received.
    #[test]
    fn the_captured_lines_round_trip() {
        for hex in ["02004000", "03000000", "0d000000", "0e000000", "0500"] {
            let p = packet(hex);
            let back: Bytes = p.into();
            let expect: Vec<u8> = (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
                .collect();
            assert_eq!(back.as_ref(), expect.as_slice(), "{hex}");
        }
    }
}
