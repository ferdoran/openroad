//! Post-join in-game packets (self-spawn foundation).
//!
//! After the `CharacterJoinRequest`/`CharacterJoinResponse` handshake
//! (`lobby.rs`), the agent server streams the character into the world:
//! `CharacterDataBegin` → `CharacterData` (the big blob) → `CharacterDataEnd`,
//! plus a `CelestialPosition` carrying the server time and the player's unique
//! id. Once the client has loaded, it answers with `GameReady`.
//!
//! Two shapes here don't fit the derive-based (de)serialization, so both
//! hand-write the `TryFrom<Bytes>` / `From<_> for Bytes` conversions the
//! `packets!` macro relies on:
//!   * `CharacterDataBody` carries its body **unparsed** (`Bytes`) — the vSRO
//!     layout has type-dependent inventory items whose sizes need itemdata,
//!     which lives in the client. The body's wire structs and staged parser
//!     live in `agent::character_data`; the client calls it with its itemdata
//!     lookup (`ItemClassResolver`).
//!   * the framing/ready packets have **empty bodies**.
//!   * the movement packets (`MovementRequest`/`MovementResponse`) have a
//!     coordinate width that depends on the region flag, which the derive can't
//!     express — hand-written below.
//!
//! Also here: the logout flow (`LogoutRequest`/`LogoutResponse`/cancel/success,
//! 0x7005/0xB005/0x7006/0xB006/0x300A) driving the Esc system window's
//! Quit/Restart buttons.

use bevy::prelude::Message;
use bytes::{BufMut, Bytes, BytesMut};

use sro_macro::ByteSize;
use sro_macro::Deserialize;
use sro_macro::SerializationError;
use sro_macro::Serialize;
use sro_macro_derive::*;

/// 0x34A5 — marks the start of the character-data stream. Empty body.
#[derive(Message, Clone, Debug, Default)]
pub struct CharacterDataBegin;

/// 0x3013 — the character-data blob (between `CharacterDataBegin` and
/// `CharacterDataEnd`). Carried unparsed because decoding needs the client's
/// itemdata tables; see `agent::character_data::parse_character_info`. Named
/// `...Body` to avoid the collision with the lobby's `CharacterData` (the
/// char-list container).
#[derive(Message, Clone, Debug)]
pub struct CharacterDataBody {
    pub raw: Bytes,
}

/// 0x34A6 — marks the end of the character-data stream. Empty body. The client
/// treats this as "self-spawn complete" and answers with [`GameReady`].
#[derive(Message, Clone, Debug, Default)]
pub struct CharacterDataEnd;

/// 0x3020 — per-character celestial position sent on entering the world. The
/// `unique_id` is the **local player's** in-world id.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug)]
pub struct CelestialPosition {
    pub unique_id: u32,
    /// `m_wDay`, the in-game **day counter** — not a moon phase. The original's
    /// handler feeds this `u16` straight into
    /// `m_LocalTime.InitTimer(pM->dwRealTime, pM->m_wDay, pM->m_byHour,
    /// pM->m_byMin, 0)`, the source expression carried in its own assert
    /// (`corpus/client-handlers/008a6dc0_FUN_008a6dc0.c:16-31`). The capture
    /// agrees: `packet_dump/0x3020.log` gives 471 then 503 across ~15 h of wall
    /// clock, which no phase index would do.
    pub day: u16,
    pub hour: u8,
    pub minute: u8,
}

/// 0x3027 — periodic celestial (time-of-day) update. No unique id.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug)]
pub struct CelestialUpdate {
    /// `m_wDay`, the in-game day counter — see [`CelestialPosition::day`]. The
    /// `0x3027` handler reads the same three fields without the leading `u32`
    /// (`corpus/client-handlers/008a6e80_FUN_008a6e80.c:20-22`).
    pub day: u16,
    pub hour: u8,
    pub minute: u8,
}

/// 0x34BE — the **real-world** server clock, pushed every ~10 minutes, packed
/// into one `u32`. Not to be confused with the in-game time-of-day clock
/// (`0x3020`/`0x3027`): this is the wall clock the original feeds into a C
/// `tm` and `mktime`.
///
/// The bit layout is read straight off the handler
/// (`corpus/client-handlers/0089a250_FUN_0089a250.c`), which is a single
/// 4-byte read followed by:
///
/// ```text
/// tm_year = (v & 0x3F) + 100      // years since 1900 -> 2000 + (v & 0x3F)
/// tm_mon  = ((v >> 6) & 0x0F) - 1 // wire month is 1-based
/// tm_mday = (v >> 10) & 0x1F
/// tm_hour = (v >> 15) & 0x1F
/// tm_min  = (v >> 20) & 0x3F
/// tm_sec  = v >> 26               // top 6 bits
/// ```
///
/// [V] — the six captured samples in `packet_dump/0x34be.log` decode to
/// 2026-08-10 11:29:22 … 12:19:22, exactly ten minutes apart and matching the
/// dump's own timestamps to the second (the server's clock runs 7 h behind the
/// capture host's UTC). See `docs/net-celestial-0x3020.md`.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct ServerTime {
    pub packed: u32,
}

impl ServerTime {
    /// Full year (the original adds 100 to get `tm_year`, i.e. 1900 + 100 + n).
    pub fn year(&self) -> u16 {
        2000 + (self.packed & 0x3F) as u16
    }

    /// 1-based month, as on the wire (the original subtracts 1 for `tm_mon`).
    pub fn month(&self) -> u8 {
        ((self.packed >> 6) & 0x0F) as u8
    }

    pub fn day(&self) -> u8 {
        ((self.packed >> 10) & 0x1F) as u8
    }

    pub fn hour(&self) -> u8 {
        ((self.packed >> 15) & 0x1F) as u8
    }

    pub fn minute(&self) -> u8 {
        ((self.packed >> 20) & 0x3F) as u8
    }

    pub fn second(&self) -> u8 {
        (self.packed >> 26) as u8
    }
}

/// 0x3012 — client → server "loading finished / game ready". Empty body; the
/// server continues the spawn sequence once it arrives.
#[derive(Message, Clone, Debug, Default)]
pub struct GameReady;

/// 0x34B5 — server → client SERVER_AGENT_GAME_RESET: tear the world down and
/// reload (sent after a teleport commits). Capture-VERIFIED vSRO 1.188
/// (2026-08-07, `packet_dump/0x34b5.log`: `a7 61` = destination region
/// 0x61A7 — exactly where the relog landed). The server then goes COMPLETELY
/// silent (even HP ticks stop) until the client answers with
/// [`GameResetComplete`], after which it replays the CHARACTER_DATA stream
/// and waits for a second [`GameReady`].
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug)]
pub struct GameReset {
    /// Destination region id.
    pub region: u16,
}

/// 0x34B6 — client → server CLIENT_AGENT_GAME_RESET_COMPLETE: the client
/// finished resetting and is ready for the post-teleport replay. Empty body.
#[derive(Message, Clone, Debug, Default)]
pub struct GameResetComplete;

// --- Hand-written wire conversions for the raw / empty bodies ---------------
//
// The `packets!` macro only needs `TryFrom<Bytes>` (decode) and
// `From<Self> for Bytes` (encode) per type; the derive macros generate exactly
// those. Empty and raw bodies provide them directly.

macro_rules! empty_packet {
    ($name:ident) => {
        impl TryFrom<Bytes> for $name {
            type Error = SerializationError;
            fn try_from(_: Bytes) -> Result<Self, Self::Error> {
                Ok($name)
            }
        }
        impl From<$name> for Bytes {
            fn from(_: $name) -> Self {
                Bytes::new()
            }
        }
    };
}

empty_packet!(CharacterDataBegin);
empty_packet!(CharacterDataEnd);
empty_packet!(GameReady);
empty_packet!(GameResetComplete);

impl TryFrom<Bytes> for CharacterDataBody {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, Self::Error> {
        Ok(CharacterDataBody { raw: value })
    }
}

impl From<CharacterDataBody> for Bytes {
    fn from(value: CharacterDataBody) -> Self {
        value.raw
    }
}

// --- Movement (0x7021 request / 0xB021 response) ---------------------------
//
// Coordinates are raw region-local units; on the wire their width depends on the
// region: overworld regions (`region & 0x8000 == 0`) use 2-byte shorts, dungeon
// regions use 4-byte ints. The derive macro can't switch a field's width on a
// prior field, so these are hand-written.

/// Whether a region id denotes a dungeon (4-byte coords) vs the overworld
/// (2-byte coords). Bit 15 is the dungeon flag (go-sro/RSBot convention; xBot's
/// two helpers disagree at 0x7FFF and are not evidence — see
/// `docs/re/systems/dungeon-teleport-in.md`).
pub fn is_dungeon(region: u16) -> bool {
    region & 0x8000 != 0
}

fn short_packet() -> SerializationError {
    SerializationError::IoError(std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        "packet too short",
    ))
}

/// Bounds-checked little-endian reader — the `bytes::Buf` getters panic on
/// underflow, which we must not do on a malformed packet.
struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], SerializationError> {
        let end = self.pos.checked_add(n).ok_or_else(short_packet)?;
        let slice = self.buf.get(self.pos..end).ok_or_else(short_packet)?;
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
    fn u64(&mut self) -> Result<u64, SerializationError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn i16(&mut self) -> Result<i16, SerializationError> {
        Ok(i16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn i32(&mut self) -> Result<i32, SerializationError> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn i64(&mut self) -> Result<i64, SerializationError> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn f32(&mut self) -> Result<f32, SerializationError> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    /// One coordinate component, widened to i32 (short for overworld regions).
    fn coord(&mut self, region: u16) -> Result<i32, SerializationError> {
        if is_dungeon(region) {
            self.i32()
        } else {
            Ok(self.i16()? as i32)
        }
    }
    /// An optional trailing byte (the movement source is not always present).
    fn opt_u8(&mut self) -> Option<u8> {
        let b = self.buf.get(self.pos).copied();
        if b.is_some() {
            self.pos += 1;
        }
        b
    }
}

fn put_coords(buf: &mut BytesMut, region: u16, x: i32, y: i32, z: i32) {
    if is_dungeon(region) {
        buf.put_i32_le(x);
        buf.put_i32_le(y);
        buf.put_i32_le(z);
    } else {
        buf.put_i16_le(x as i16);
        buf.put_i16_le(y as i16);
        buf.put_i16_le(z as i16);
    }
}

/// 0x7021 — client → server move order to a location (click-to-move). `x/y/z`
/// are raw region-local units (a capture showed ×10 scaling made the server
/// wrap the destination several regions over).
#[derive(Message, Clone, Debug, PartialEq)]
pub struct MovementRequest {
    pub region: u16,
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl From<MovementRequest> for Bytes {
    fn from(p: MovementRequest) -> Self {
        let mut buf = BytesMut::new();
        buf.put_u8(1); // 1 = move to a location (vs 0 = turn to an angle)
        buf.put_u16_le(p.region);
        put_coords(&mut buf, p.region, p.x, p.y, p.z);
        buf.freeze()
    }
}

impl TryFrom<Bytes> for MovementRequest {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, Self::Error> {
        let mut r = Reader::new(&value);
        let _kind = r.u8()?; // 1 = location (the only form we build)
        let region = r.u16()?;
        Ok(MovementRequest {
            region,
            x: r.coord(region)?,
            y: r.coord(region)?,
            z: r.coord(region)?,
        })
    }
}

/// 0xB021 — server → client movement update. We surface `unique_id` + the
/// destination (or `angle` when it's a turn-in-place); the trailing source
/// position is parsed to stay in sync but not exposed.
#[derive(Message, Clone, Debug, PartialEq)]
pub struct MovementResponse {
    pub unique_id: u32,
    pub has_destination: bool,
    /// Valid when `has_destination`: region + raw region-local coords.
    pub region: u16,
    pub x: i32,
    pub y: i32,
    pub z: i32,
    /// Valid when `!has_destination`: heading.
    pub angle: u16,
}

impl TryFrom<Bytes> for MovementResponse {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, Self::Error> {
        let mut r = Reader::new(&value);
        let unique_id = r.u32()?;
        let has_destination = r.u8()? != 0;
        let mut out = MovementResponse {
            unique_id,
            has_destination,
            region: 0,
            x: 0,
            y: 0,
            z: 0,
            angle: 0,
        };
        if has_destination {
            out.region = r.u16()?;
            out.x = r.coord(out.region)?;
            out.y = r.coord(out.region)?;
            out.z = r.coord(out.region)?;
        } else {
            let _moving = r.u8()?;
            out.angle = r.u16()?;
        }
        // Optional source position: region, X (short/int), Y (always f32), Z.
        if matches!(r.opt_u8(), Some(1)) {
            let src_region = r.u16()?;
            let _ = r.coord(src_region)?;
            let _ = r.f32()?;
            let _ = r.coord(src_region)?;
        }
        Ok(out)
    }
}

impl From<MovementResponse> for Bytes {
    fn from(p: MovementResponse) -> Self {
        let mut buf = BytesMut::new();
        buf.put_u32_le(p.unique_id);
        buf.put_u8(p.has_destination as u8);
        if p.has_destination {
            buf.put_u16_le(p.region);
            put_coords(&mut buf, p.region, p.x, p.y, p.z);
        } else {
            buf.put_u8(1); // moving
            buf.put_u16_le(p.angle);
        }
        buf.put_u8(0); // has_source = 0
        buf.freeze()
    }
}

/// 0xB023 — server → client absolute position sync for one entity: snap the
/// addressed entity to `region` + region-local float coords + heading. Unlike
/// [`MovementResponse`] (a move *order* whose coordinate width depends on the
/// region), this carries the entity's exact current position as floats — used for
/// teleports/knockback/corrections. Same position shape as the CHARACTER_DATA
/// block, so it needs no hand-written width handling.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct MovementPositionUpdate {
    pub unique_id: u32,
    pub region: u16,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub heading: u16,
}

/// 0xB024 — server → client: an entity turned **in place**. Without this an
/// entity that rotates without moving (an idle turn, or facing an NPC or a
/// target) keeps its old facing until its next move order.
///
/// 6-byte body, taken from the original's parser, which reads a `u32` then a
/// `u16` and stops (`PacketParser.EntityMovementAngle`). `angle` is the same
/// `0..=u16::MAX → 0..2π` heading encoding used everywhere else on the wire,
/// as in [`MovementPositionUpdate`].
///
/// No capture exists yet (`packet_dump/0xb024.log` is absent), so the layout
/// rests on the parser alone and the heading mapping is cross-checked only
/// against our other heading fields — not against live bytes for this opcode.
///
/// The C→S half (0x7024 `CLIENT_CHARACTER_MOVEMENT_ANGLE`) is deliberately
/// **not** modelled: the original has no builder for it and its dispatch arm
/// is an empty stub, so any body would be invented.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct MovementAngleResponse {
    pub unique_id: u32,
    pub angle: u16,
}

/// 0x30D0 — server → client movement-speed change for one entity (buffs, GM
/// speed command, mounts). Layout per go-sro's `EntityUpdateMovementSpeed`
/// writer: unique id + walk/run speeds in game units per second.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct EntitySpeedUpdate {
    pub unique_id: u32,
    pub walk_speed: f32,
    pub run_speed: f32,
}

// --- Vitals / points / stats updates (0x3057 / 0x304E / 0x303D) -------------
//
// These three feed the player mini-info HUD: 0x3057 moves the HP/MP bars,
// 0x304E fills the hwan/berserk pips, and 0x303D is the only packet on the
// wire that carries max HP/MP (neither CHARACTER_DATA nor the bar update do).
// Layouts verified against skrillax's silkroad-protocol (vSRO 1.188).

/// [`EntityBarsUpdate::source`] display hints.
pub const BARS_SOURCE_DAMAGE: u16 = 0x01;
pub const BARS_SOURCE_REGEN: u16 = 0x10;
pub const BARS_SOURCE_LEVEL_UP: u16 = 0x80;

/// [`EntityBarsUpdate::flag`] bits — the body carries one block per set bit.
pub const BARS_FLAG_HP: u8 = 0x01;
pub const BARS_FLAG_MP: u8 = 0x02;
pub const BARS_FLAG_BAD_STATUS: u8 = 0x04;
/// A `u16` of UNKNOWN meaning; carried uninterpreted so the tail stays aligned.
pub const BARS_FLAG_UNKNOWN16: u8 = 0x08;

/// Which [`EntityBarsUpdate::bad_status`] bits carry a trailing `u8` level.
///
/// Recovered from the original's `FUN_009d2220`, which reads one level byte per
/// set bit that also lies in this mask (`docs/re/net/inbound/entity.md`). The
/// level-less bits are exactly the six elemental/DoT states plus Petrify —
/// which is independent corroboration of the SPEC bit ORDER in [`BadStatus`],
/// since two unrelated artifacts have to agree for that partition to line up.
pub const BAD_STATUS_LEVELED: u32 = 0x017F_EFC0;

/// 0x3057 — server → client vitals update for one entity. Values are absolute,
/// not deltas.
///
/// **`flag` is a BITMASK, not an enum.** The original's `FUN_008a9e30` runs
/// four independent `if ((flag & bit) != 0)` blocks in this order: `0x01` HP
/// u32, `0x02` MP u32, `0x08` u16, `0x04` bad-status `u32` mask followed by
/// `popcount(mask & `[`BAD_STATUS_LEVELED`]`)` level bytes
/// (`docs/re/net/inbound/entity.md`).
///
/// This corrects an earlier enum reading (xBot `SRTypes.cs`: HP=1, MP=2,
/// HPMP=3, BadStatus=4, EntityHPMP=5) which happened to round-trip because
/// `flag=5` = `HP|BAD_STATUS` carries two u32s, exactly like `flag=3`'s
/// HP+MP — so the shapes are indistinguishable by LENGTH, only by MEANING.
/// The capture decides it, and it now decides against the enum:
///
/// - `packet_dump/0x3057.log` grew to 870 bodies and **`flag=0x04` now
///   occurs** (`b1640200 0301 04 08000000`) — a value the enum reading calls
///   BadStatus-with-no-HP but which the old code left entirely unparsed. Its
///   body is 11 bytes with **no** trailing level byte, which is precisely what
///   [`BAD_STATUS_LEVELED`] predicts for bit 3 — so that RE-derived rule is now
///   capture-confirmed too.
/// - Its uid `0x000264b1` is a **monster** (in the 0x3019 spawn stream, yields
///   EXP on death), and the mask `0x8` is Burn. For the next 7 s every
///   `flag=05` body for that uid carries `08000000` in the trailing u32 while
///   its HP drains to 0 — i.e. the burn ticking. Under the enum reading that
///   monster was reported as having **MP = 8**.
/// - Every other `flag=05` body in the corpus carries `0` there. As "no bad
///   status" that is unremarkable; as "MP" it was an unexplained constant the
///   old doc comment had to hedge about.
///
/// Body lengths corroborate the bitmask exactly: 11 bytes for flags 1/2/4
/// (one block) and 15 for 3/5 (two blocks), matching the corpus census.
///
/// Decoded by hand rather than by the derive: the level tail's length is a
/// popcount of a value read earlier in the same body, which `#[sro_packet]`
/// has no way to express.
#[derive(Message, Clone, Debug, PartialEq)]
pub struct EntityBarsUpdate {
    pub unique_id: u32,
    /// [`BARS_SOURCE_DAMAGE`] / [`BARS_SOURCE_REGEN`] / [`BARS_SOURCE_LEVEL_UP`].
    pub source: u16,
    pub flag: u8,
    pub hp: Option<u32>,
    pub mp: Option<u32>,
    /// The `0x08` block — meaning UNKNOWN, carried so the tail stays aligned.
    pub unknown16: Option<u16>,
    /// Abnormal-state bitmask ([`BadStatus`]), when `flag & 0x04`.
    pub bad_status: Option<u32>,
    /// One level per set [`BAD_STATUS_LEVELED`] bit, in bit order.
    pub bad_status_levels: Vec<u8>,
}

impl EntityBarsUpdate {
    /// The ailment mask, or 0 when this update carries no bad-status block.
    ///
    /// `None` means "this packet said nothing about ailments", which is NOT the
    /// same as "no ailments" — only a `flag & 0x04` body clears them.
    pub fn bad_status(&self) -> Option<BadStatus> {
        self.bad_status.map(BadStatus)
    }
}

impl TryFrom<Bytes> for EntityBarsUpdate {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, SerializationError> {
        let mut r = Reader::new(&value);
        let unique_id = r.u32()?;
        let source = r.u16()?;
        let flag = r.u8()?;
        // Order is the binary's, not the bit order: 0x08 is read BEFORE 0x04.
        let hp = (flag & BARS_FLAG_HP != 0).then(|| r.u32()).transpose()?;
        let mp = (flag & BARS_FLAG_MP != 0).then(|| r.u32()).transpose()?;
        let unknown16 = (flag & BARS_FLAG_UNKNOWN16 != 0)
            .then(|| r.u16())
            .transpose()?;
        let bad_status = (flag & BARS_FLAG_BAD_STATUS != 0)
            .then(|| r.u32())
            .transpose()?;
        let mut bad_status_levels = Vec::new();
        if let Some(mask) = bad_status {
            // A short/absent tail is tolerated rather than fatal: the level
            // rule is RE-derived and only its no-level case is capture-proven,
            // so a body that ends early yields fewer levels instead of losing
            // the mask that names the ailments.
            for _ in 0..(mask & BAD_STATUS_LEVELED).count_ones() {
                match r.u8() {
                    Ok(level) => bad_status_levels.push(level),
                    Err(_) => break,
                }
            }
        }
        Ok(EntityBarsUpdate {
            unique_id,
            source,
            flag,
            hp,
            mp,
            unknown16,
            bad_status,
            bad_status_levels,
        })
    }
}

impl From<EntityBarsUpdate> for Bytes {
    fn from(p: EntityBarsUpdate) -> Self {
        let mut buf = BytesMut::new();
        buf.put_u32_le(p.unique_id);
        buf.put_u16_le(p.source);
        buf.put_u8(p.flag);
        if let Some(hp) = p.hp {
            buf.put_u32_le(hp);
        }
        if let Some(mp) = p.mp {
            buf.put_u32_le(mp);
        }
        if let Some(unknown) = p.unknown16 {
            buf.put_u16_le(unknown);
        }
        if let Some(mask) = p.bad_status {
            buf.put_u32_le(mask);
        }
        for level in &p.bad_status_levels {
            buf.put_u8(*level);
        }
        buf.freeze()
    }
}

impl ByteSize for EntityBarsUpdate {
    fn byte_size(&self) -> usize {
        7 + self.hp.map_or(0, |_| 4)
            + self.mp.map_or(0, |_| 4)
            + self.unknown16.map_or(0, |_| 2)
            + self.bad_status.map_or(0, |_| 4)
            + self.bad_status_levels.len()
    }
}

/// The 0x3057 abnormal-state bitmask.
///
/// **The bit → ailment mapping is `[S]` SPEC-derived**, from ducksoup's
/// `GlobalEnums.cs` (`docs/re/notes/death-resurrect.md`,
/// `docs/re/DRAFT_LAYOUTS.md`). Only **Burn (`0x8`) is capture-confirmed** —
/// see the [`EntityBarsUpdate`] doc. The ordering is independently corroborated
/// by [`BAD_STATUS_LEVELED`], whose level-less bits land exactly on the six
/// elemental/DoT states plus Petrify; that agreement between the RE'd read rule
/// and the SPEC enum is why the order is trusted enough to act on, but no
/// individual name below except Burn should be called verified.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct BadStatus(pub u32);

/// The ailments [`BadStatus`] can name, in bit order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ailment {
    Freezing,
    Frostbite,
    ElectricShock,
    Burn,
    Poison,
    Zombie,
    Sleep,
    Bind,
    Dull,
    Fear,
    ShortSight,
    Bleed,
    Petrify,
    Darkness,
    Stun,
    Disease,
    Confusion,
    Decay,
    Weaken,
}

impl Ailment {
    /// Every ailment, in bit order — index `n` is bit `1 << n`.
    pub const ALL: [Ailment; 19] = [
        Ailment::Freezing,
        Ailment::Frostbite,
        Ailment::ElectricShock,
        Ailment::Burn,
        Ailment::Poison,
        Ailment::Zombie,
        Ailment::Sleep,
        Ailment::Bind,
        Ailment::Dull,
        Ailment::Fear,
        Ailment::ShortSight,
        Ailment::Bleed,
        Ailment::Petrify,
        Ailment::Darkness,
        Ailment::Stun,
        Ailment::Disease,
        Ailment::Confusion,
        Ailment::Decay,
        Ailment::Weaken,
    ];

    /// This ailment's mask bit.
    pub fn bit(self) -> u32 {
        1 << Ailment::ALL.iter().position(|a| *a == self).unwrap_or(0)
    }

    /// The archive's own name for this ailment's authored debuff effect, as
    /// `battle/status_bad_<name>.efp`.
    ///
    /// **Every name below is a real file**, enumerated from the user's
    /// `Particles.pk2` (26 `battle/status_bad_*` entries). Six are additionally
    /// corroborated by the matching `status_cure_*` set documented in
    /// `docs/re/notes/item-use.md` (`blind`, `burn`, `eshock`, `frostbite`,
    /// `poison`, `zombie`).
    ///
    /// The bit → file pairing is an inference over two independent naming
    /// schemes (the SPEC enum's English names vs. the artists' filenames) and
    /// is **not capture-verified for any bit except Burn**. Where the pairing
    /// is not obvious it returns `None` and that ailment plays no effect —
    /// borrowing a neighbouring file's art would be exactly the unsourced
    /// invention ADR-0009 forbids. Unpaired files, kept here so the next person
    /// does not re-derive the list: `control`, `hide`, `temptation`,
    /// `dark_blaze`, `dark_toxin`, the `_off` counterparts (`icing_off`,
    /// `stone_off` — almost certainly the *removal* animations) and the `_b`/
    /// `_m` icing size variants. A parallel `monster/status_bad_*` set exists
    /// for 8 of these; which set the original picks per entity kind is UNKNOWN,
    /// so only `battle/` is used.
    pub fn archive_name(self) -> Option<&'static str> {
        Some(match self {
            // "icing" is the archive's freeze family; frostbite is its own
            // file, matching the enum's two distinct cold states.
            Ailment::Freezing => "icing_on",
            Ailment::Frostbite => "frostbite",
            Ailment::ElectricShock => "eshock",
            Ailment::Burn => "burn",
            Ailment::Poison => "poison",
            Ailment::Zombie => "zombie",
            Ailment::Sleep => "sleep",
            Ailment::Bind => "root",
            Ailment::Dull => "blunt",
            Ailment::ShortSight => "myopia",
            Ailment::Bleed => "bleeding",
            Ailment::Petrify => "stone_on",
            Ailment::Darkness => "blind",
            Ailment::Stun => "stun",
            Ailment::Disease => "disease",
            Ailment::Confusion => "confusion",
            // No `status_bad_` file reads as fear, decay or weaken. The
            // unclaimed behavioural files (`panic`, `control`, `temptation`)
            // most likely belong to the ailments ABOVE Weaken that this enum
            // does not carry yet — note the icon set has a separate
            // `s_fear_icon` AND `s_panic_icon`, so pairing Fear to
            // `status_bad_panic.efp` would almost certainly be wrong.
            Ailment::Fear | Ailment::Decay | Ailment::Weaken => return None,
        })
    }

    /// The authored HUD icon for this ailment, under `icon/StateOdd/`.
    ///
    /// **Every name is a real file**, enumerated from the user's `Media.pk2`,
    /// and this set is what raises confidence in the SPEC bit ORDER from
    /// "plausible" to "well corroborated": all nineteen bits below pair to a
    /// distinct authored icon, and the last two land on `s_decay_icon` and
    /// `s_weakness_icon` at exactly the positions the enum puts Decay and
    /// Weaken. Three independent artifacts — the enum, the RE'd
    /// [`BAD_STATUS_LEVELED`] partition, and this icon set — agree.
    ///
    /// Names are given lowercase because `bevy_pk2` lowercases every path as
    /// it indexes the archive, so that is what a lookup must use — the
    /// archive's own mixed casing (`S_Burn_Icon.ddj` beside
    /// `s_bleeding_icon.ddj`) never reaches us.
    ///
    /// Unclaimed icons (`s_combustion_icon`, `s_dissociation_icon`,
    /// `s_incubation_icon`, `s_panic_icon`, `s_powerless_icon`) line up with
    /// the ailments above Weaken that this enum does not carry yet.
    pub fn icon_name(self) -> &'static str {
        match self {
            Ailment::Freezing => "s_freeze_icon",
            Ailment::Frostbite => "s_frostbite_icon",
            Ailment::ElectricShock => "s_electricshock_icon",
            Ailment::Burn => "s_burn_icon",
            Ailment::Poison => "s_poisoning_icon",
            Ailment::Zombie => "s_zombi_icon",
            Ailment::Sleep => "s_sleep_icon",
            Ailment::Bind => "s_root_icon",
            Ailment::Dull => "s_blunting_icon",
            Ailment::Fear => "s_fear_icon",
            Ailment::ShortSight => "s_myopia_icon",
            Ailment::Bleed => "s_bleeding_icon",
            Ailment::Petrify => "s_stonecurse_icon",
            Ailment::Darkness => "s_dark_icon",
            Ailment::Stun => "s_stun_icon",
            Ailment::Disease => "s_disease_icon",
            Ailment::Confusion => "s_confusion_icon",
            Ailment::Decay => "s_decay_icon",
            Ailment::Weaken => "s_weakness_icon",
        }
    }

    /// Asset path of [`Self::icon_name`].
    pub fn icon_path(self) -> String {
        format!("media://icon/stateodd/{}.ddj", self.icon_name())
    }
}

impl BadStatus {
    /// Whether any ailment is set.
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Whether `ailment` is set.
    pub fn has(self, ailment: Ailment) -> bool {
        self.0 & ailment.bit() != 0
    }

    /// The ailments this mask names. Bits beyond [`Ailment::ALL`] are ignored
    /// rather than guessed at — the SPEC enum runs out before `u32` does.
    pub fn ailments(self) -> impl Iterator<Item = Ailment> {
        Ailment::ALL.into_iter().filter(move |a| self.has(*a))
    }
}

/// 0x304E — server → client points update for the local player (no unique id).
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub enum CharacterPointsUpdate {
    #[sro_packet(value = 1)]
    Gold { amount: u64, display: u8 },
    #[sro_packet(value = 2)]
    Sp { amount: u32, display: u8 },
    #[sro_packet(value = 3)]
    StatPoints { amount: u16 },
    /// `amount` is the hwan/berserk gauge fill (0–5).
    #[sro_packet(value = 4)]
    Berserk { amount: u8, source: u32 },
}

/// 0x303D — server → client recomputed combat stats for the local player (no
/// unique id), sent after the self-spawn stream and on any stat change. The
/// only source of max HP/MP.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct CharacterStatsUpdate {
    pub phys_attack_min: u32,
    pub phys_attack_max: u32,
    pub mag_attack_min: u32,
    pub mag_attack_max: u32,
    pub phys_defense: u16,
    pub mag_defense: u16,
    pub hit_rate: u16,
    pub parry_rate: u16,
    pub max_hp: u32,
    pub max_mp: u32,
    pub strength: u16,
    pub intelligence: u16,
}

// --- Misc captured world-join server pushes (EXPERIMENTAL) ------------------
//
// Four small S→C pushes seen in a single live world-join capture against the
// maintainer's own vSRO 1.188 server (2026-08-07; see
// docs/net-captured-opcodes.md and PR #179). Bodies were decoded from the
// pre-deserialization bytes in packet_dump/ and cross-referenced against the
// go-sro opcode filter and skrillax's silkroad-protocol. Only empty-list
// branches were observed for the two roster/cooldown packets, so their entry
// shapes are UNVERIFIED and kept as best-effort targets — the empty case (the
// only captured one) round-trips exactly.

/// 0x3153 — SERVER_AGENT_SILK_UPDATE: account silk balances (Joymax premium
/// currency). Three u32 balances, little-endian. Capture-VERIFIED: the 12-byte
/// body `F4 CB 9A 3B 50 C3 00 00 00 00 00 00` decodes to own 1,000,000,500 /
/// gift 50,000 / point 0 on the admin test account. The own/gift/point label
/// order follows the skrillax/SilkroadDoc convention (not on-wire labelled).
/// Confidence: HIGH.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct SilkUpdate {
    /// Regular (purchased) silk.
    pub own: u32,
    /// Gift / premium silk.
    pub gift: u32,
    /// Silk points.
    pub point: u32,
}

/// 0x3809 — SERVER_AGENT_ENVIRONMENT_WEATHER_UPDATE: the zone weather sent on
/// world-enter. Captured body was exactly 2 bytes (`01 B4` → type 1 = clear,
/// intensity 180 in the Jangan start zone). The `weather_type` enum and the
/// `intensity` scale need weather-change captures to confirm; relevant to
/// EP-27 (environment / weather). Confidence: HIGH on name/direction, field
/// semantics SPEC-derived.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct WeatherUpdate {
    /// 1 = clear/fine (only value observed).
    pub weather_type: u8,
    /// Severity / particle amount (only 180 observed).
    pub intensity: u8,
}

/// 0x3305 — SERVER_AGENT_COMMUNITY_FRIEND_INFO: the join-time friend roster
/// (skrillax `FriendList`). The captured body was a single `00` count byte —
/// an empty roster — so only the count-prefixed shell is capture-VERIFIED. The
/// per-entry [`FriendEntry`] record is not capture-backed (no friends to
/// observe) but is read off the original's own parser (#546); the empty case
/// round-trips exactly. Relevant to EP-32 / #154
/// (social / friends). Confidence: HIGH on name/direction, entry layout SPEC.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct FriendListInfo {
    /// Number of roster entries that follow (0 in the only capture).
    pub count: u8,
    #[sro_packet(list_type = "by-size-field", size_field = "count")]
    pub friends: Vec<FriendEntry>,
}

/// One friend roster entry: `u32, u16 len + ASCII, u32, u8` — **four** fields.
///
/// Read straight off the original's parser `FUN_009993b0`
/// (`docs/re/net/inbound/chat-social.md`, 0x3305): it reads id (`:50`), name
/// length (`:51`), the name bytes (`:63`), then a `u32` (`:64`) and a `u8`
/// (`:65`), and hands exactly those four to the roster ctor
/// `FUN_009971c0(id, name, u32, u8)` (`:68`). The roster node it builds is
/// 0x28 bytes with slots for precisely those four, and no fifth.
///
/// This used to carry a `group_id: u16` taken from skrillax' community record.
/// It is not on this wire, so every entry after the first shifted by two bytes
/// as soon as the roster was non-empty (#546). It stayed invisible because
/// the only capture (`packet_dump/0x3305.log`, 5/5 lines `00`) is an empty
/// roster and nothing in `client/src` consumes the list yet.
///
/// UNKNOWN: the *names* of the two trailing scalars. The parser names neither
/// and `FUN_009971c0` is not in the corpus; a model/ref-object id and an
/// online flag are the obvious reading (and the node's `+0x24` is mutated by
/// the status setter `FUN_00997350`), but that is inference, so the layout is
/// closed while the semantics are not.
#[derive(Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct FriendEntry {
    pub char_id: u32,
    pub name: String,
    /// Trailing `u32` — very likely the character's ref-object id. [U]
    pub char_model: u32,
    /// Trailing `u8` — nonzero = online, per the roster node's status slot. [U]
    pub is_online: u8,
}

/// 0x3077 — CharacterFinished: the join-time cooldown replay. Two
/// count-prefixed lists — item cooldowns then skill cooldowns, keyed by ref-id
/// (openroad RE wave A3 / skrillax `item_cooldowns` + `skill_cooldowns`).
/// Captured body was `00 00` — both lists empty (a fresh char with nothing on
/// cooldown) — so only the two-empty-list shell is capture-VERIFIED. The
/// per-entry [`Cooldown`] record is UNVERIFIED (no on-cooldown capture).
/// Relevant to EP-07 (item use / cooldowns). Confidence: MEDIUM (name from
/// openroad RE only; join-timing and the two-list shape are confirmed).
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct CharacterFinished {
    pub item_cooldown_count: u8,
    #[sro_packet(list_type = "by-size-field", size_field = "item_cooldown_count")]
    pub item_cooldowns: Vec<Cooldown>,
    pub skill_cooldown_count: u8,
    #[sro_packet(list_type = "by-size-field", size_field = "skill_cooldown_count")]
    pub skill_cooldowns: Vec<Cooldown>,
}

/// One cooldown entry — UNVERIFIED `{ ref_id, cooldown }` (per the A3 RE
/// prediction); no non-empty capture exists to confirm it.
#[derive(Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct Cooldown {
    pub ref_id: u32,
    pub cooldown: u32,
}

// --- Entity selection (0x7045/0xB045) ---------------------------------------

/// 0x7045 — client → server "select this entity" (the click target of the
/// world-scene selection). The server answers with [`SelectEntityResponse`].
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct SelectEntityRequest {
    pub unique_id: u32,
}

/// 0xB045 — server → client answer to [`SelectEntityRequest`].
///
/// After the `result`/`unique_id` header the body depends on the *target's*
/// type, which is not encoded in the packet — so the tail stays raw and the
/// consumer interprets it against what it knows the entity to be. Per go-sro's
/// `select_entity.go` (the reference server; layouts unverified against vSRO):
///   * monster: `u8` (=1), `u32` current HP (go-sro hardcodes 0), `u8`, `u8`
///   * player:  `u32`, `u8` trader lvl, `u8` hunter lvl, `u8` thief lvl, `u8`
///   * NPC:     go-sro never sends the response at all (handler bug) — the
///     client must treat this packet as optional enrichment, never a gate.
#[derive(Message, Clone, Debug, PartialEq)]
pub struct SelectEntityResponse {
    /// 1 = ok, anything else = failure (tail then carries an error byte).
    pub result: u8,
    /// Echo of the selected unique id (0 on failure).
    pub unique_id: u32,
    /// Raw type-dependent remainder, see above.
    pub tail: Bytes,
}

impl SelectEntityResponse {
    /// Interpret the tail as the monster shape and return the current HP, if
    /// the shape matches and the server filled it in (go-sro sends 0 = unknown).
    pub fn monster_hp(&self) -> Option<u32> {
        if self.result != 1 || self.tail.len() < 5 || self.tail[0] != 1 {
            return None;
        }
        let hp = u32::from_le_bytes(self.tail[1..5].try_into().unwrap());
        (hp > 0).then_some(hp)
    }
}

impl TryFrom<Bytes> for SelectEntityResponse {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, Self::Error> {
        let mut r = Reader::new(&value);
        let result = r.u8()?;
        let unique_id = if result == 1 { r.u32()? } else { 0 };
        Ok(SelectEntityResponse {
            result,
            unique_id,
            tail: value.slice(r.pos..),
        })
    }
}

impl From<SelectEntityResponse> for Bytes {
    fn from(p: SelectEntityResponse) -> Self {
        let mut buf = BytesMut::new();
        buf.put_u8(p.result);
        if p.result == 1 {
            buf.put_u32_le(p.unique_id);
        }
        buf.extend_from_slice(&p.tail);
        buf.freeze()
    }
}

// --- Object action (0x7074/0xB074, 0xB070/0xB071) ---------------------------
//
// The attack / skill-cast exchange. The request starts a *server-driven*
// action loop (the server paths the character into range and repeats basic
// attacks until a Cancel or the target dies); each swing/cast arrives as a
// 0xB070 [`ObjectActionUpdate`] carrying the per-target damage list — the
// 0xB074 ack itself is only accept/reject. Layouts follow skrillax's
// `combat.rs` (reverse-engineered from vSRO 1.188 captures) adjusted to our
// own dumps: our server omits skrillax's extra u32 before `target` (every
// `packet_dump/0xb070.log` line is 20 bytes), and the 0xB074 ack is the
// 2-byte `phase code` shape documented on [`ObjectActionResponse`]. Both
// still decode tolerantly into an `Unknown { result, tail }` fallback for
// unrecognized shapes — consumers must treat those as log-only.

/// 0x7074 — client → server object action: a leading flag byte selects
/// execute (1, followed by an [`ActionCommand`]) or cancel (2, empty).
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub enum ObjectActionRequest {
    #[sro_packet(value = 1)]
    Execute(ActionCommand),
    #[sro_packet(value = 2)]
    Cancel,
}

/// The action selector inside [`ObjectActionRequest::Execute`].
#[derive(Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub enum ActionCommand {
    #[sro_packet(value = 1)]
    Attack(ActionTarget),
    #[sro_packet(value = 2)]
    Pickup(ActionTarget),
    #[sro_packet(value = 4)]
    CastSkill {
        ref_skill_id: u32,
        target: ActionTarget,
    },
}

/// Target selector: a flag byte, then the unique id when targeting an entity.
#[derive(Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub enum ActionTarget {
    #[sro_packet(value = 0)]
    None,
    #[sro_packet(value = 1)]
    Entity { unique_id: u32 },
}

/// Action error codes seen in [`ObjectActionResponse`] / [`ObjectActionUpdate`]
/// failures (vSRO 1.188 via skrillax's `PerformActionError`).
///
/// ⚠️ These are **not** the codes this server sends. The original reads the
/// failure field as a `u16` and hands it straight to its message-box helper
/// (`FUN_008a5fd0:179-180`, `FUN_00880b60:12-14`), and our own captures carry
/// `0x3006`, `0x3010` (0xB070) and `0x4004` (0xB074) — string/notice ids, not
/// small ordinals. The low byte of `0x3006` coinciding with `0x06` here is
/// suggestive but **UNVERIFIED**; nothing maps the two encodings yet, so these
/// two constants stay as the skrillax reference and no code compares against
/// them. Resolving read: the client's message-id table for the id passed as
/// `0xffdbc99b`'s format argument.
pub const ACTION_ERROR_INVALID_TARGET: u16 = 0x06;
pub const ACTION_ERROR_INVALID_DISTANCE: u16 = 0x07;

// --- GM commands (0x7010 / 0xB010) -----------------------------------------

/// 0x7010 — client → server GM command: a **u16 LE sub-command selector**,
/// then per-command **typed** args. Only the account-privileged commands the
/// server honours do anything; a normal account gets a failure [`GmResponse`].
///
/// The original binds each command *name* to its own builder in a registry at
/// `0x00558e00`–`0x00559100`, and each builder writes its fields individually
/// through `FUN_00508fe0(&value, size)`. There is no generic
/// "`u16` + ASCII message" envelope — xBot's `SendGMCommand` writes one, but
/// that is a bot-side helper, not this client, and the `docs/re` builder census
/// agrees: 34 distinct 0x7010 builders, every one `{u16 sub_id, …typed args}`
/// (`docs/re/net/outbound/session-lifecycle.md` §0x7010).
///
/// The two toggles carry no args; the server flips the state and echoes it back
/// as a 0x30BF body-state update rather than in the ack.
#[derive(Message, Clone, Debug, PartialEq)]
pub enum GmCommand {
    /// `/loadmonster` (0x06) — spawn `count` of `ref_id` at the caller.
    /// Builder `0x00547eb0`, 8-byte body `{u16, u32, u8, u8}`. [V]
    ///
    /// This sub-id is why [`GmCommand::MakeItem`] must not use it: 0x06 was
    /// wrongly assigned to MakeItem here (a skrillax-derived `[U]` guess), so
    /// every "make item" we sent was a *monster spawn* one byte short of its
    /// own layout.
    LoadMonster { ref_id: u32, count: u8, rarity: u8 },
    /// `/makeitem` (0x07) — create an item in the GM's inventory. Builder
    /// `0x005483c0`, 7-byte body `{u16, u32, u8}` written at
    /// `0x00548507`–`0x0054853a`. [V]
    ///
    /// `value` stays a neutral name because the one byte means two different
    /// things by item class. The original parses argument 2 with
    /// `swscanf("%d")`, truncates it to its low byte and sends it for both —
    /// clamped to `[1, MaxStack]` when the row is stackable (`TypeID2 == 3`),
    /// passed through untouched for equipment.
    ///
    /// The server's reading was `[U]` here until a live test settled it
    /// (2026-08-18): for **equipment it is the enchantment level**, and for a
    /// stackable it is the quantity — which is what the client's own two-branch
    /// clamp already implied. It is a *request*, not a guarantee: the server
    /// clamps to the item's own ceiling, and a sent `255` came back as `+8`.
    ///
    /// `ref_id` is resolved client-side: the original looks argument 1 up in
    /// the item ref-object table by codename and puts the resulting u32 on the
    /// wire, so a codename never reaches the server.
    MakeItem { ref_id: u32, value: u8 },
    /// `/zoe` **and** `/zoe2` (0x0C) — spawn `count` of a monster. Builders
    /// `0x005481c0` (Zoe) and `0x005516f0` (Zoe2), 7-byte body `{u16,u32,u8}`.
    /// [V]
    ///
    /// **Both commands emit this one sub-id.** `Zoe2` is not a distinct packet:
    /// it is a client-side batching wrapper that splits a large count into
    /// chunks of 200 and paces them, and every chunk is an ordinary `0x000C`.
    /// So the 34 recovered 0x7010 builders cover only 33 distinct sub-ids.
    ///
    /// The monster is named by codename and resolved client-side against the
    /// same unified ref-object map `MakeItem` uses; the gate is `TypeID 1/2/1`
    /// (character / NPC / monster) where `MakeItem`'s is `TypeID1 == 3`.
    ///
    /// That the server then *kills* what it spawned is `[U]` — the client only
    /// builds this body. The suggestion comes from the drain routine's own log
    /// strings (`[CF Kill]`), which is not proof.
    Zoe { ref_id: u32, count: u8 },
    /// `/invisible` (0x0E) — toggle GM invisibility. Builder `0x0053dd80`,
    /// 2-byte body. [V], and the only sub-command we have ever captured
    /// (`packet_dump/c2s/0x7010.log`).
    Invisible,
    /// `/invincible` (0x0F) — toggle GM invincibility. Builder `0x0053dea0`,
    /// 2-byte body. [V]
    Invincible,
}

impl GmCommand {
    /// The u16 sub-command selector. Sourced from the original's own command
    /// registry, not from a bot table — see the variant docs for each VA.
    pub fn code(&self) -> u16 {
        match self {
            GmCommand::LoadMonster { .. } => 0x06,
            GmCommand::MakeItem { .. } => 0x07,
            GmCommand::Zoe { .. } => 0x0C,
            GmCommand::Invisible => 0x0E,
            GmCommand::Invincible => 0x0F,
        }
    }
}

impl TryFrom<Bytes> for GmCommand {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, SerializationError> {
        let mut r = Reader::new(&value);
        match r.u16()? {
            0x06 => Ok(GmCommand::LoadMonster {
                ref_id: r.u32()?,
                count: r.u8()?,
                rarity: r.u8()?,
            }),
            0x07 => Ok(GmCommand::MakeItem {
                ref_id: r.u32()?,
                value: r.u8()?,
            }),
            0x0C => Ok(GmCommand::Zoe {
                ref_id: r.u32()?,
                count: r.u8()?,
            }),
            0x0E => Ok(GmCommand::Invisible),
            0x0F => Ok(GmCommand::Invincible),
            other => Err(SerializationError::UnknownVariation(
                other as usize,
                "GmCommand",
            )),
        }
    }
}

impl From<GmCommand> for Bytes {
    fn from(p: GmCommand) -> Self {
        let mut buf = BytesMut::new();
        buf.put_u16_le(p.code());
        match p {
            GmCommand::LoadMonster {
                ref_id,
                count,
                rarity,
            } => {
                buf.put_u32_le(ref_id);
                buf.put_u8(count);
                buf.put_u8(rarity);
            }
            GmCommand::MakeItem { ref_id, value } => {
                buf.put_u32_le(ref_id);
                buf.put_u8(value);
            }
            GmCommand::Zoe { ref_id, count } => {
                buf.put_u32_le(ref_id);
                buf.put_u8(count);
            }
            GmCommand::Invisible | GmCommand::Invincible => {}
        }
        buf.freeze()
    }
}

/// 0xB010 — server → client GM command result (`FUN_008743a0`;
/// `docs/re/net/inbound/misc-debug.md` §0xb010).
///
/// `result` is 1 = ok / 2 = fail, and the `u16` after it is an **echo of the
/// request's sub-command**, read on *both* arms — not an error code. The proof
/// is that command 0x20 appears in both and yields "SiegeManager MSG Result -
/// Ok." / "…- Fail." from the same value. That echo is what makes this packet
/// a usable probe: it says which [`GmCommand`] the server just judged.
///
/// The per-command payload after the echo is still kept raw. Only a handful of
/// ids carry one (0x01 and 0x19/0x1a a string, 0x04 three u32s), and neither
/// [`GmCommand::MakeItem`] nor [`GmCommand::LoadMonster`] is among them — for
/// those the whole body is the 3-byte head.
///
/// Confirmed against our own capture: every body in `packet_dump/0xb010.log`
/// is `01 0e 00` — ok, echoing the `/invisible` we sent.
#[derive(Message, Clone, Debug, PartialEq)]
pub struct GmResponse {
    pub result: u8,
    /// Echo of the [`GmCommand::code`] this answers.
    pub gm_command_id: u16,
    pub tail: Bytes,
}

impl GmResponse {
    pub fn is_success(&self) -> bool {
        self.result == GM_RESULT_OK
    }
}

/// `result` values the original branches on; anything else reads nothing.
pub const GM_RESULT_OK: u8 = 1;
pub const GM_RESULT_FAIL: u8 = 2;

impl TryFrom<Bytes> for GmResponse {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, SerializationError> {
        let mut r = Reader::new(&value);
        let result = r.u8()?;
        // The original reads the echo on the ok and fail arms alike, and reads
        // nothing at all for any other `result` — so a body that stops here is
        // legal rather than malformed.
        let gm_command_id = r.u16().unwrap_or(0);
        Ok(GmResponse {
            result,
            gm_command_id,
            tail: value.slice(r.pos.min(value.len())..),
        })
    }
}

impl From<GmResponse> for Bytes {
    fn from(p: GmResponse) -> Self {
        let mut buf = BytesMut::new();
        buf.put_u8(p.result);
        buf.put_u16_le(p.gm_command_id);
        buf.extend_from_slice(&p.tail);
        buf.freeze()
    }
}

/// 0xB074 — server → client ack for [`ObjectActionRequest`].
///
/// Capture-VERIFIED vSRO 1.188 (2026-08-06, the whole `packet_dump/0xb074.log`
/// corpus: `01 01`×703, `02 00`×693, `01 02`×150, `02 01`×134, `01 00`×6,
/// `03 xx 04 40`×20). The shape is two bytes, `phase code`:
/// - `01 <code>` — action start ack: code 0 = skill cast accepted (every
///   `01 00` is followed <60 ms by a successful kind-None 0xB070), code 1 =
///   attack accepted, code 2 = **rejected** (server action slot busy — a cast
///   sent mid-auto-attack; no 0xB070 ever follows).
/// - `02 <code>` — the running action ended (0/1 observed; meaning of the
///   code not yet pinned).
/// - `03 <code> <error u16>` — the request was **refused with a message**: the
///   handler reads the same `code` byte, then a `u16` it hands to the
///   message-box helper (`FUN_00880b60:12-14`). Captured seven times as
///   `03 00 04 40`, i.e. code 0, error `0x4004` (#232).
#[derive(Message, Clone, Debug, PartialEq)]
pub enum ObjectActionResponse {
    /// `01 <code>` — start ack; see [`Self::is_rejected`].
    Started { code: u8 },
    /// `02 <code>` — the server-side action loop ended.
    Ended { code: u8 },
    /// `03 <code> <error u16>` — refused with a message box; `error` is the
    /// original's notice id, not a small ordinal (see `ACTION_ERROR_*`).
    Failed { code: u8, error: u16 },
    /// Anything else — kept raw, log-only.
    Unknown { result: u8, tail: Bytes },
}

/// [`ObjectActionResponse::Started`] code: skill cast accepted.
pub const ACTION_START_CAST: u8 = 0;
/// [`ObjectActionResponse::Started`] code: attack accepted.
pub const ACTION_START_ATTACK: u8 = 1;
/// [`ObjectActionResponse::Started`] code: rejected — another action owns the
/// server's action slot.
pub const ACTION_START_REJECTED: u8 = 2;

impl ObjectActionResponse {
    /// A start ack that refused the request (nothing will follow on 0xB070).
    pub fn is_rejected(&self) -> bool {
        matches!(
            self,
            ObjectActionResponse::Started {
                code: ACTION_START_REJECTED
            }
        )
    }
}

impl TryFrom<Bytes> for ObjectActionResponse {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, SerializationError> {
        let mut r = Reader::new(&value);
        let result = r.u8()?;
        let parsed = (|| -> Option<ObjectActionResponse> {
            match result {
                1 => Some(ObjectActionResponse::Started { code: r.u8().ok()? }),
                2 => Some(ObjectActionResponse::Ended { code: r.u8().ok()? }),
                3 => Some(ObjectActionResponse::Failed {
                    code: r.u8().ok()?,
                    error: r.u16().ok()?,
                }),
                _ => None,
            }
        })()
        // A typed read that leaves bytes over means we guessed the wrong
        // shape — fall back to raw rather than silently dropping data.
        .filter(|_| r.pos == value.len());
        Ok(parsed.unwrap_or_else(|| ObjectActionResponse::Unknown {
            result,
            tail: value.slice(1..),
        }))
    }
}

impl From<ObjectActionResponse> for Bytes {
    fn from(p: ObjectActionResponse) -> Self {
        let mut buf = BytesMut::new();
        match p {
            ObjectActionResponse::Started { code } => {
                buf.put_u8(1);
                buf.put_u8(code);
            }
            ObjectActionResponse::Ended { code } => {
                buf.put_u8(2);
                buf.put_u8(code);
            }
            ObjectActionResponse::Failed { code, error } => {
                buf.put_u8(3);
                buf.put_u8(code);
                buf.put_u16_le(error);
            }
            ObjectActionResponse::Unknown { result, tail } => {
                buf.put_u8(result);
                buf.extend_from_slice(&tail);
            }
        }
        buf.freeze()
    }
}

/// One damage value inside a [`SkillPartDamage`] hit.
///
/// The `kind` byte is kept **raw**. The server resolves an outcome enum
/// internally (1 hit / 2 miss / 4 crit / 5 block / 6 parry / 8 defense,
/// `docs/combat-math-server-spec.md` §2) whose numbering is NOT the numbering
/// of this wire byte — on the wire, `2` is the critical marker. Only `1` and
/// `2` have ever been captured (169 hit records, spec §5), so any other value
/// is an outcome we have never seen: collapsing it to a bool would rewrite it
/// as an ordinary hit on the way back out.
///
/// Both fields come out of a single packed little-endian `u32`
/// (`FUN_00a55e00:29, :66-67`): the low byte is this `kind` (the original's
/// "damage state"), the upper **24 bits** are the amount. Reading the byte
/// and then a full `u32` agrees only while the following unnamed field is
/// zero, which it is in every capture we hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DamageValue {
    /// Wire `kind` byte: 1 = standard, 2 = critical, other = UNKNOWN outcome.
    pub kind: u8,
    /// 24-bit on the wire; values above `0xFF_FFFF` cannot be encoded.
    pub amount: u32,
}

impl DamageValue {
    /// Wire kind for an ordinary hit.
    pub const KIND_NORMAL: u8 = 1;
    /// Wire kind for a critical hit ([S]: skrillax `combat.rs`, and the only
    /// non-1 value in our captures).
    pub const KIND_CRITICAL: u8 = 2;

    pub fn is_critical(&self) -> bool {
        self.kind == Self::KIND_CRITICAL
    }

    /// True for a `kind` neither of the two we have ever captured — an
    /// outcome the presentation layer must treat as log-only rather than as
    /// a plain hit.
    pub fn is_unknown_kind(&self) -> bool {
        !matches!(self.kind, Self::KIND_NORMAL | Self::KIND_CRITICAL)
    }
}

/// The 14-byte position tail carried by hit arms 4 and 5
/// (`FUN_00a55e00:41-45` / `:51-57`): a region id plus three `i32`
/// coordinates the original converts to floats at read time. Kept as the
/// wire's integers — no capture of these arms exists yet, so any scaling
/// would be a guess.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HitPosition {
    pub region: u16,
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

/// The payload of one hit record, selected by `flags & 0x7F`.
///
/// The original is a `switch (flags & 0x7f)` over four listed arms plus a
/// catch-all (`FUN_00a55e00:26-34`), not a set of bit tests: arms 0 and 7 are
/// 9 bytes, arms 4 and 5 are 23, and every other value reads nothing at all.
/// A bit test on `0x08` agrees with the catch-all only by luck and reads
/// arms 4/5 fourteen bytes short, desynchronising the rest of the packet
/// (`docs/re/net/inbound/skill-combat.md`, 0xB070).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitEffect {
    /// Arm 0 — plain damage: the packed damage word plus one unnamed `u32`
    /// (zero in all 145 captured `0xb070` lines, kept so a non-zero value
    /// survives a round trip).
    Damage { value: DamageValue, unknown: u32 },
    /// Arms 4 and 5 — arm 0 plus a position tail (knockback / knockdown).
    /// The original stores the two arms in *different* record slots
    /// (`rec+0x1c` vs `rec+0x2c`), so which arm it was is preserved; what
    /// distinguishes them is UNKNOWN.
    Displaced {
        arm: u8,
        value: DamageValue,
        unknown: u32,
        pos: HitPosition,
    },
    /// Arm 7 — the packed damage word plus two unnamed `u16`s. The original
    /// force-clears its killing-blow flag on this arm (`:63`).
    Arm7 {
        value: DamageValue,
        unknown_a: u16,
        unknown_b: u16,
    },
    /// Any other arm: the record is the flag byte alone, no damage is read
    /// and the original zeroes its damage field (`:32-34`).
    NoPayload { arm: u8 },
}

/// One hit against one entity: the `0x80` bit of the flag byte plus the arm
/// payload it selects. The original keeps exactly this split — `0x80` goes to
/// a separate bool at `rec+8` (`FUN_00a55e00:16-24`) while `flags & 0x7F`
/// drives the switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SkillPartDamage {
    /// Wire bit `0x80`. Kept verbatim even on arm 7, where the original
    /// discards it after reading, so the byte round-trips.
    pub killing_blow: bool,
    pub effect: HitEffect,
}

impl SkillPartDamage {
    /// An ordinary arm-0 hit — the shape practically every captured record
    /// takes.
    pub fn hit(value: DamageValue) -> Self {
        SkillPartDamage {
            killing_blow: false,
            effect: HitEffect::Damage { value, unknown: 0 },
        }
    }

    /// An arm-0 hit that killed the target.
    pub fn killing_blow(value: DamageValue) -> Self {
        SkillPartDamage {
            killing_blow: true,
            effect: HitEffect::Damage { value, unknown: 0 },
        }
    }

    pub fn value(&self) -> Option<DamageValue> {
        match self.effect {
            HitEffect::Damage { value, .. }
            | HitEffect::Displaced { value, .. }
            | HitEffect::Arm7 { value, .. } => Some(value),
            HitEffect::NoPayload { .. } => None,
        }
    }

    /// This hit was **avoided by the defender**: [`HIT_ARM_AVOIDED`], the arm
    /// that carries no damage word at all.
    pub fn is_avoided(&self) -> bool {
        matches!(
            self.effect,
            HitEffect::NoPayload {
                arm: HIT_ARM_AVOIDED
            }
        )
    }
}

/// Hit arm 2 — **the defender took no damage**. The record is the flag byte
/// alone; the original reads no damage word and zeroes its damage field
/// (`FUN_00a55e00:32-34`).
///
/// `[V]` from the whole of `packet_dump/0xb070.log` (1121 lines, 987 hit
/// records): 95 arm-2 records, **every one of them on a player-class defender**
/// (the local character plus four party members) and never once on a monster,
/// appearing independently in either instance slot of a two-instance monster
/// skill — `0,0` ×82, `0,2` ×31, `2,0` ×29, i.e. a per-hit roll at ~25%, not a
/// tail filler. Contrast [`HIT_ARM_DEAD_TARGET`].
///
/// **Which** avoidance it is stays `[U]`. The original's internal resolver enum
/// numbers `2 = MISS`, `5 = BLOCK`, `6 = PARRY`, `8 = defense`
/// (`docs/re/gamedata/combat-outcome-resolver.md` §3′), but that enum is not
/// this wire byte's numbering — the wire carries no discriminator and no
/// amount, so miss, parry and block are indistinguishable here. The client
/// presents it as BLOCK (`docs/combat-math-server-spec.md` §5).
pub const HIT_ARM_AVOIDED: u8 = 2;

/// Hit arm 8 — **no hit: the target was already dead**. Also payload-less.
///
/// `[V]` from the same dump: 28 records, **28 of 28** the second instance of a
/// two-instance basic attack whose *first* instance carried the `0x80` killing
/// blow. Unlike [`HIT_ARM_AVOIDED`] it never appears in the first slot and
/// never on a contested roll, so it is a filler, not an outcome, and it
/// correctly produces no popup.
pub const HIT_ARM_DEAD_TARGET: u8 = 8;

/// Damage dealt to a single target entity by one action instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PerEntityDamage {
    /// Unique id of the entity taking the damage.
    pub target: u32,
    /// One entry per damage instance (count = [`DamageContent::instance_count`],
    /// no per-entity prefix on the wire).
    pub hits: Vec<SkillPartDamage>,
}

/// The damage block of an attack-kind [`ObjectActionUpdate`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DamageContent {
    /// Hits per entity (multi-hit skills; 1 for basic attacks).
    pub instance_count: u8,
    /// u8-count-prefixed list of damaged entities.
    pub entities: Vec<PerEntityDamage>,
}

/// What kind of action a 0xB070 update describes (wire byte 0 / 1 / 8).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActionKind {
    /// Self-casts / buffs — no damage payload observed.
    None,
    /// A swing/cast that dealt damage; `None` damage = swing without payload.
    Attack {
        damage: Option<DamageContent>,
    },
    Teleport,
}

/// 0xB070 — server → client: one action instance executes (a basic-attack
/// swing or skill cast). This is where per-hit damage arrives; the matching
/// [`SkillEnd`] later echoes `instance`.
///
/// Layout = skrillax's `PerformActionUpdate` minus its `unknown_4` u32: our
/// live captures (`packet_dump/0xb070.log`, all 20 bytes, e.g.
/// `01 0030 c6980000 90890500 27060000 00000000 00` = self-buff, target 0,
/// kind none) only fit without it. The all-zero tail can't fully disambiguate
/// the two layouts — re-adding the u32 before `target` is the first thing to
/// try if real attack captures land in [`Self::Unknown`].
#[derive(Message, Clone, Debug, PartialEq)]
pub enum ObjectActionUpdate {
    Success {
        /// Observed 0x3000 in captures (skrillax: 0x3002 | 0x3000).
        unknown: u16,
        /// Ref skill id (basic attacks use the weapon's base-attack skill).
        skill_id: u32,
        /// Unique id of the acting entity.
        source: u32,
        /// Cast-instance counter, echoed by [`SkillEnd`].
        instance: u32,
        /// Unique id of the primary target (0 for self-casts).
        target: u32,
        kind: ActionKind,
    },
    /// `02 <error u16>` — the action failed and the original pops a message
    /// box. The handler's non-success branch is a single 2-byte read followed
    /// by `FUN_00778190(4, error, ...)` (`FUN_008a5fd0:178-181`), so the tail
    /// is exactly one `u16` — which is why the captured `02 06 30` / `02 10 30`
    /// used to fall into `Unknown` against the old 1-byte model (#232).
    Failure { error: u16 },
    /// Any shape the typed parse doesn't fit — kept raw, log-only.
    Unknown { result: u8, tail: Bytes },
}

impl ObjectActionUpdate {
    /// Parse the post-`result` body of the success shape; any error or
    /// leftover bytes reject the whole typed read (→ `Unknown`).
    fn parse_success(value: &Bytes) -> Option<ObjectActionUpdate> {
        let mut r = Reader::new(value);
        let _ = r.u8().ok()?; // result, already known == 1
        let unknown = r.u16().ok()?;
        let skill_id = r.u32().ok()?;
        let source = r.u32().ok()?;
        let instance = r.u32().ok()?;
        let target = r.u32().ok()?;
        let kind = match r.u8().ok()? {
            0 => ActionKind::None,
            1 => {
                let damage = if r.pos == value.len() {
                    None
                } else {
                    Some(Self::parse_damage(&mut r)?)
                };
                ActionKind::Attack { damage }
            }
            8 => ActionKind::Teleport,
            _ => return None,
        };
        (r.pos == value.len()).then_some(ObjectActionUpdate::Success {
            unknown,
            skill_id,
            source,
            instance,
            target,
            kind,
        })
    }

    fn parse_damage(r: &mut Reader) -> Option<DamageContent> {
        let instance_count = r.u8().ok()?;
        let entity_count = r.u8().ok()?;
        let mut entities = Vec::with_capacity(entity_count as usize);
        for _ in 0..entity_count {
            let target = r.u32().ok()?;
            let mut hits = Vec::with_capacity(instance_count as usize);
            for _ in 0..instance_count {
                hits.push(Self::parse_hit(r)?);
            }
            entities.push(PerEntityDamage { target, hits });
        }
        Some(DamageContent {
            instance_count,
            entities,
        })
    }

    /// One hit record — `FUN_00a55e00`. The flag byte splits into a
    /// killing-blow bit (`0x80`) and an arm selector (`flags & 0x7F`) whose
    /// arms have four different lengths.
    fn parse_hit(r: &mut Reader) -> Option<SkillPartDamage> {
        let flags = r.u8().ok()?;
        let arm = flags & 0x7F;
        let effect = match arm {
            0 => HitEffect::Damage {
                value: Self::parse_damage_word(r)?,
                unknown: r.u32().ok()?,
            },
            4 | 5 => {
                let value = Self::parse_damage_word(r)?;
                let unknown = r.u32().ok()?;
                let pos = HitPosition {
                    region: r.u16().ok()?,
                    x: r.i32().ok()?,
                    y: r.i32().ok()?,
                    z: r.i32().ok()?,
                };
                HitEffect::Displaced {
                    arm,
                    value,
                    unknown,
                    pos,
                }
            }
            7 => HitEffect::Arm7 {
                value: Self::parse_damage_word(r)?,
                unknown_a: r.u16().ok()?,
                unknown_b: r.u16().ok()?,
            },
            _ => HitEffect::NoPayload { arm },
        };
        Some(SkillPartDamage {
            killing_blow: flags & 0x80 != 0,
            effect,
        })
    }

    /// The packed damage word: `u8 damage state | u24 amount`, LE.
    fn parse_damage_word(r: &mut Reader) -> Option<DamageValue> {
        let word = r.u32().ok()?;
        Some(DamageValue {
            kind: (word & 0xFF) as u8,
            amount: word >> 8,
        })
    }
}

/// Encode one hit record — the inverse of `ObjectActionUpdate::parse_hit`.
fn write_hit(buf: &mut BytesMut, hit: &SkillPartDamage) {
    let arm = match hit.effect {
        HitEffect::Damage { .. } => 0,
        HitEffect::Displaced { arm, .. } | HitEffect::NoPayload { arm } => arm & 0x7F,
        HitEffect::Arm7 { .. } => 7,
    };
    buf.put_u8(arm | if hit.killing_blow { 0x80 } else { 0 });
    let word = |v: &DamageValue| (v.amount & 0x00FF_FFFF) << 8 | v.kind as u32;
    match &hit.effect {
        HitEffect::Damage { value, unknown } => {
            buf.put_u32_le(word(value));
            buf.put_u32_le(*unknown);
        }
        HitEffect::Displaced {
            value,
            unknown,
            pos,
            ..
        } => {
            buf.put_u32_le(word(value));
            buf.put_u32_le(*unknown);
            buf.put_u16_le(pos.region);
            buf.put_i32_le(pos.x);
            buf.put_i32_le(pos.y);
            buf.put_i32_le(pos.z);
        }
        HitEffect::Arm7 {
            value,
            unknown_a,
            unknown_b,
        } => {
            buf.put_u32_le(word(value));
            buf.put_u16_le(*unknown_a);
            buf.put_u16_le(*unknown_b);
        }
        HitEffect::NoPayload { .. } => {}
    }
}

/// Encode an [`ActionKind`] + optional damage block — the shared tail of
/// [`ObjectActionUpdate`] and [`SkillEnd`].
fn write_action_kind(buf: &mut BytesMut, kind: &ActionKind) {
    match kind {
        ActionKind::None => buf.put_u8(0),
        ActionKind::Teleport => buf.put_u8(8),
        ActionKind::Attack { damage } => {
            buf.put_u8(1);
            if let Some(damage) = damage {
                buf.put_u8(damage.instance_count);
                buf.put_u8(damage.entities.len() as u8);
                for entity in &damage.entities {
                    buf.put_u32_le(entity.target);
                    for hit in &entity.hits {
                        write_hit(buf, hit);
                    }
                }
            }
        }
    }
}

impl TryFrom<Bytes> for ObjectActionUpdate {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, Self::Error> {
        let mut r = Reader::new(&value);
        let result = r.u8()?;
        let parsed = match result {
            1 => ObjectActionUpdate::parse_success(&value),
            2 => match r.u16() {
                Ok(error) if r.pos == value.len() => Some(ObjectActionUpdate::Failure { error }),
                _ => None,
            },
            _ => None,
        };
        Ok(parsed.unwrap_or_else(|| ObjectActionUpdate::Unknown {
            result,
            tail: value.slice(1..),
        }))
    }
}

impl From<ObjectActionUpdate> for Bytes {
    fn from(p: ObjectActionUpdate) -> Self {
        let mut buf = BytesMut::new();
        match p {
            ObjectActionUpdate::Success {
                unknown,
                skill_id,
                source,
                instance,
                target,
                kind,
            } => {
                buf.put_u8(1);
                buf.put_u16_le(unknown);
                buf.put_u32_le(skill_id);
                buf.put_u32_le(source);
                buf.put_u32_le(instance);
                buf.put_u32_le(target);
                write_action_kind(&mut buf, &kind);
            }
            ObjectActionUpdate::Failure { error } => {
                buf.put_u8(2);
                buf.put_u16_le(error);
            }
            ObjectActionUpdate::Unknown { result, tail } => {
                buf.put_u8(result);
                buf.extend_from_slice(&tail);
            }
        }
        buf.freeze()
    }
}

/// 0xB071 — server → client: the cast instance ends. Capture-verified vSRO
/// 1.188: `01 <instance u32> <target u32> <kind u8> [DamageContent]` — the
/// same trailing layout as [`ObjectActionUpdate`]. This server delivers
/// TARGETED SKILL DAMAGE here (~0.5 s after the kind-None 0xB070 cast
/// start, roughly the visual hit moment); the 10-byte majority shape is the
/// same layout with target 0, kind 0 (self-buffs / auto-attack ends).
/// Anything else — error shapes, other server builds — lands in
/// [`Self::Unknown`] raw. No Failure variant: none was ever captured.
#[derive(Message, Clone, Debug, PartialEq)]
pub enum SkillEnd {
    Success {
        /// Echoes the [`ObjectActionUpdate`] cast-instance counter.
        instance: u32,
        /// Unique id of the damaged/affected entity, 0 when none.
        target: u32,
        kind: ActionKind,
    },
    Unknown {
        result: u8,
        tail: Bytes,
    },
}

impl TryFrom<Bytes> for SkillEnd {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, Self::Error> {
        let mut r = Reader::new(&value);
        let result = r.u8()?;
        let parsed = (|| -> Option<SkillEnd> {
            if result != 1 {
                return None;
            }
            let instance = r.u32().ok()?;
            let target = r.u32().ok()?;
            let kind = match r.u8().ok()? {
                0 => ActionKind::None,
                1 => {
                    let damage = if r.pos == value.len() {
                        None
                    } else {
                        Some(ObjectActionUpdate::parse_damage(&mut r)?)
                    };
                    ActionKind::Attack { damage }
                }
                8 => ActionKind::Teleport,
                _ => return None,
            };
            Some(SkillEnd::Success {
                instance,
                target,
                kind,
            })
        })()
        // leftover bytes = wrong shape guess; keep raw rather than misread
        .filter(|_| r.pos == value.len());
        Ok(parsed.unwrap_or_else(|| SkillEnd::Unknown {
            result,
            tail: value.slice(1..),
        }))
    }
}

impl From<SkillEnd> for Bytes {
    fn from(p: SkillEnd) -> Self {
        let mut buf = BytesMut::new();
        match p {
            SkillEnd::Success {
                instance,
                target,
                kind,
            } => {
                buf.put_u8(1);
                buf.put_u32_le(instance);
                buf.put_u32_le(target);
                write_action_kind(&mut buf, &kind);
            }
            SkillEnd::Unknown { result, tail } => {
                buf.put_u8(result);
                buf.extend_from_slice(&tail);
            }
        }
        buf.freeze()
    }
}

// --- Skill / mastery learning (0x70A1/0x70A2) -------------------------------
//
// Skill/mastery learning. No longer EXPERIMENTAL (#125, 2026-08-15): the two
// requests are read off the original's builders (0x70A1 `@0081dc60`, 0x70A2
// `@0081dd20`) and both acks are capture-dated (2026-08-05). Only the ERROR
// shapes stay assumed — no capture has produced a non-`01` result, so those
// paths keep their bytes raw and warn.

/// 0x70A1 — client → server "learn this skill" (AGENT_SKILL_LEARN) (the skilldata ref id of the
/// next rung of the ladder).
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct SkillLearnRequest {
    pub ref_skill_id: u32,
}

/// 0xB0A1 — server → client ack for [`SkillLearnRequest`].
///
/// Capture-verified on vSRO 1.188: `01 <ref_skill_id u32>` — the ack is the
/// ONLY learn notification (no character-data refresh follows), so the
/// client applies it to the skill book directly. Error shape assumed
/// `02 <code u16>` by analogy with other acks (uncaptured); anything else
/// lands in [`Self::Unknown`] raw.
#[derive(Message, Clone, Debug, PartialEq)]
pub enum SkillLearnResponse {
    Success { ref_skill_id: u32 },
    Failure(u16),
    Unknown { result: u8, tail: Bytes },
}

impl TryFrom<Bytes> for SkillLearnResponse {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, Self::Error> {
        let mut r = Reader::new(&value);
        let result = r.u8()?;
        let parsed = (|| -> Option<SkillLearnResponse> {
            match result {
                1 => Some(SkillLearnResponse::Success {
                    ref_skill_id: r.u32().ok()?,
                }),
                2 => Some(SkillLearnResponse::Failure(r.u16().ok()?)),
                _ => None,
            }
        })()
        // leftover bytes = wrong shape guess; keep raw rather than misread
        .filter(|_| r.pos == value.len());
        Ok(parsed.unwrap_or_else(|| SkillLearnResponse::Unknown {
            result,
            tail: value.slice(1..),
        }))
    }
}

impl From<SkillLearnResponse> for Bytes {
    fn from(p: SkillLearnResponse) -> Self {
        let mut buf = BytesMut::new();
        match p {
            SkillLearnResponse::Success { ref_skill_id } => {
                buf.put_u8(1);
                buf.put_u32_le(ref_skill_id);
            }
            SkillLearnResponse::Failure(code) => {
                buf.put_u8(2);
                buf.put_u16_le(code);
            }
            SkillLearnResponse::Unknown { result, tail } => {
                buf.put_u8(result);
                buf.extend_from_slice(&tail);
            }
        }
        buf.freeze()
    }
}

/// 0x70A2 — client → server "raise this mastery" (AGENT_SKILL_MASTERY_LEARN) by `amount` levels (the
/// vanilla client always sends 1).
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct MasteryLearnRequest {
    pub mastery_id: u32,
    pub amount: u8,
}

/// 0xB0A2 — server → client ack for [`MasteryLearnRequest`].
///
/// Capture-verified on vSRO 1.188: `01 <mastery_id u32> <new_level u8>`
/// (e.g. `01 01010000 05` = Bicheon raised to 5). Error shape assumed
/// `02 <code u16>` (uncaptured); anything else lands in [`Self::Unknown`].
#[derive(Message, Clone, Debug, PartialEq)]
pub enum MasteryLearnResponse {
    Success { mastery_id: u32, new_level: u8 },
    Failure(u16),
    Unknown { result: u8, tail: Bytes },
}

impl TryFrom<Bytes> for MasteryLearnResponse {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, Self::Error> {
        let mut r = Reader::new(&value);
        let result = r.u8()?;
        let parsed = (|| -> Option<MasteryLearnResponse> {
            match result {
                1 => Some(MasteryLearnResponse::Success {
                    mastery_id: r.u32().ok()?,
                    new_level: r.u8().ok()?,
                }),
                2 => Some(MasteryLearnResponse::Failure(r.u16().ok()?)),
                _ => None,
            }
        })()
        .filter(|_| r.pos == value.len());
        Ok(parsed.unwrap_or_else(|| MasteryLearnResponse::Unknown {
            result,
            tail: value.slice(1..),
        }))
    }
}

impl From<MasteryLearnResponse> for Bytes {
    fn from(p: MasteryLearnResponse) -> Self {
        let mut buf = BytesMut::new();
        match p {
            MasteryLearnResponse::Success {
                mastery_id,
                new_level,
            } => {
                buf.put_u8(1);
                buf.put_u32_le(mastery_id);
                buf.put_u8(new_level);
            }
            MasteryLearnResponse::Failure(code) => {
                buf.put_u8(2);
                buf.put_u16_le(code);
            }
            MasteryLearnResponse::Unknown { result, tail } => {
                buf.put_u8(result);
                buf.extend_from_slice(&tail);
            }
        }
        buf.freeze()
    }
}

// --- Server notice push (0x300C) --------------------------------------------
//
// The discriminator is **one u16**, not `u8 type` + `u8 unk01`. The original's
// handler `FUN_00874ea0` does a single read at `:53` and then
// `switch (code & 0xffff)` over eighteen `0x0Cxx` labels; xBot's second "unk01"
// byte is simply that u16's high byte, constant `0x0C` in every sample — which
// is why it looked like a stable but meaningless field. The capture pins it:
// `05 0c 43 95 00 00` is six bytes, so u16 `0x0C05` + u32 ref id, and a u32
// discriminator would leave two bytes and read the id as 0.
//
// Only the two codes with a recorded meaning are modelled — `0x0C05` (unique
// appeared) and `0x0C06` (unique killed) — plus `0x0C18`, whose two leading
// bytes the capture pins. The other fifteen codes decode to `Raw`: the original's
// `default:` arm reads nothing either, so raw is the faithful behaviour and
// inventing widths for them is exactly the defect ADR-0009 names. See
// docs/net-misc-0x2113.md §0x300C.
//
// The `pos == len` guard is the same one the learn/level acks use: a code whose
// assumed shape does not consume the body exactly falls through to `Raw` rather
// than being misread.

/// `0x0C05` — a unique monster appeared. Capture-verified.
pub const NOTICE_UNIQUE_APPEARED: u16 = 0x0C05;
/// `0x0C06` — a unique monster was killed. Stated by the original's parser
/// (one scalar + one string), never captured.
pub const NOTICE_UNIQUE_KILLED: u16 = 0x0C06;
/// `0x0C18` — meaning unknown; its two leading bytes are capture-pinned.
pub const NOTICE_0C18: u16 = 0x0C18;

/// 0x300C — server → client notice push.
#[derive(Message, Clone, Debug, PartialEq)]
pub enum NoticeUpdate {
    /// [`NOTICE_UNIQUE_APPEARED`] — `ref_id` is a ref-data object id, not a
    /// model id: the original hands it to `FUN_0093f630()`, the ref-data
    /// lookup. Capture-verified: `05 0c 43 95 00 00` → 38211.
    UniqueAppeared { ref_id: u32 },
    /// [`NOTICE_UNIQUE_KILLED`] — same lookup plus the killer's name.
    UniqueKilled { ref_id: u32, player: String },
    /// [`NOTICE_0C18`] — two bytes the capture pins (`18 0c 02 03` → 2, 3) and
    /// then, on a condition the decompile lost, eight more. Since the gate is
    /// [U], whatever follows is kept verbatim rather than gated on a guess.
    Code0C18 { a: u8, b: u8, tail: Bytes },
    /// Any other code — kept whole, code included, because no source records
    /// its field widths.
    Raw { code: u16, tail: Bytes },
}

impl NoticeUpdate {
    /// The wire code this notice carries.
    pub fn code(&self) -> u16 {
        match self {
            NoticeUpdate::UniqueAppeared { .. } => NOTICE_UNIQUE_APPEARED,
            NoticeUpdate::UniqueKilled { .. } => NOTICE_UNIQUE_KILLED,
            NoticeUpdate::Code0C18 { .. } => NOTICE_0C18,
            NoticeUpdate::Raw { code, .. } => *code,
        }
    }
}

impl TryFrom<Bytes> for NoticeUpdate {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, SerializationError> {
        let mut r = Reader::new(&value);
        let code = r.u16()?;
        let parsed = (|| -> Option<NoticeUpdate> {
            match code {
                NOTICE_UNIQUE_APPEARED => Some(NoticeUpdate::UniqueAppeared {
                    ref_id: r.u32().ok()?,
                }),
                NOTICE_UNIQUE_KILLED => {
                    let ref_id = r.u32().ok()?;
                    let len = r.u16().ok()? as usize;
                    let name = r.take(len).ok()?;
                    Some(NoticeUpdate::UniqueKilled {
                        ref_id,
                        player: String::from_utf8_lossy(name).into_owned(),
                    })
                }
                NOTICE_0C18 => {
                    let a = r.u8().ok()?;
                    let b = r.u8().ok()?;
                    let tail = value.slice(r.pos..);
                    r.pos = value.len();
                    Some(NoticeUpdate::Code0C18 { a, b, tail })
                }
                _ => None,
            }
        })()
        .filter(|_| r.pos == value.len());
        Ok(parsed.unwrap_or_else(|| NoticeUpdate::Raw {
            code,
            tail: value.slice(2.min(value.len())..),
        }))
    }
}

impl From<NoticeUpdate> for Bytes {
    fn from(p: NoticeUpdate) -> Self {
        let mut buf = BytesMut::new();
        buf.put_u16_le(p.code());
        match p {
            NoticeUpdate::UniqueAppeared { ref_id } => buf.put_u32_le(ref_id),
            NoticeUpdate::UniqueKilled { ref_id, player } => {
                buf.put_u32_le(ref_id);
                buf.put_u16_le(player.len() as u16);
                buf.extend_from_slice(player.as_bytes());
            }
            NoticeUpdate::Code0C18 { a, b, tail } => {
                buf.put_u8(a);
                buf.put_u8(b);
                buf.extend_from_slice(&tail);
            }
            NoticeUpdate::Raw { tail, .. } => buf.extend_from_slice(&tail),
        }
        buf.freeze()
    }
}

// --- Mastery / skill level-DOWN (0x7202/0x7203, 0xB202/0xB203) --------------
//
// The mirror of the level-UP flow above. Both responses are [V] — read statically
// from the original's parsers — but neither *request* has a builder there, so their
// bodies are spec-derived from the level-UP siblings and stay [U] until a capture
// lands. See docs/net-mastery-teleport-0x7202.md.
//
// The response enums reuse the level-UP shapes verbatim, including the
// `pos == len` guard. That guard matters more here than it does above: the
// original reads no error code on the failure branch, so whether a failure is a
// lone `02` or `02 <code u16>` is unresolved — and the guard makes both safe,
// because a shape that does not consume the body exactly falls through to
// `Unknown` raw instead of being misread.

/// 0x7202 — client → server "lower this skill by one level".
///
/// **[U] body.** The original has no builder for this opcode; the single `u32` is
/// mirrored from the level-UP sibling [`SkillLearnRequest`]. Confirm with
/// `packet_dump/0x7202.log` before anything sends it.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct SkillLevelDownRequest {
    pub ref_skill_id: u32,
}

/// 0xB202 — server → client ack for [`SkillLevelDownRequest`].
///
/// `01 <new_skill_id u32>` on success — a level-down returns the id of the skill at
/// its new, lower level. Failure shape uncaptured; see the section comment.
#[derive(Message, Clone, Debug, PartialEq)]
pub enum MasterySkillLevelDownResponse {
    Success { new_skill_id: u32 },
    Failure(u16),
    Unknown { result: u8, tail: Bytes },
}

impl TryFrom<Bytes> for MasterySkillLevelDownResponse {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, Self::Error> {
        let mut r = Reader::new(&value);
        let result = r.u8()?;
        let parsed = (|| -> Option<MasterySkillLevelDownResponse> {
            match result {
                1 => Some(MasterySkillLevelDownResponse::Success {
                    new_skill_id: r.u32().ok()?,
                }),
                2 => Some(MasterySkillLevelDownResponse::Failure(r.u16().ok()?)),
                _ => None,
            }
        })()
        .filter(|_| r.pos == value.len());
        Ok(
            parsed.unwrap_or_else(|| MasterySkillLevelDownResponse::Unknown {
                result,
                tail: value.slice(1..),
            }),
        )
    }
}

impl From<MasterySkillLevelDownResponse> for Bytes {
    fn from(p: MasterySkillLevelDownResponse) -> Self {
        let mut buf = BytesMut::new();
        match p {
            MasterySkillLevelDownResponse::Success { new_skill_id } => {
                buf.put_u8(1);
                buf.put_u32_le(new_skill_id);
            }
            MasterySkillLevelDownResponse::Failure(code) => {
                buf.put_u8(2);
                buf.put_u16_le(code);
            }
            MasterySkillLevelDownResponse::Unknown { result, tail } => {
                buf.put_u8(result);
                buf.extend_from_slice(&tail);
            }
        }
        buf.freeze()
    }
}

/// 0x7203 — client → server "lower this mastery by one level".
///
/// **[U] body.** No builder in the original. The level-UP sibling
/// [`MasteryLearnRequest`] carries a trailing `amount: u8`; whether the DOWN
/// request does too is unresolved, so it is **not** included here — the doc's build
/// plan defers that byte to a capture rather than assuming it. Resolve with
/// `packet_dump/0x7203.log`.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct MasteryLevelDownRequest {
    pub mastery_id: u32,
}

/// 0xB203 — server → client ack for [`MasteryLevelDownRequest`].
///
/// `01 <mastery_id u32> <new_level u8>` on success — the exact mirror of
/// [`MasteryLearnResponse`]. Failure shape uncaptured; see the section comment.
#[derive(Message, Clone, Debug, PartialEq)]
pub enum MasteryLevelDownResponse {
    Success { mastery_id: u32, new_level: u8 },
    Failure(u16),
    Unknown { result: u8, tail: Bytes },
}

impl TryFrom<Bytes> for MasteryLevelDownResponse {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, Self::Error> {
        let mut r = Reader::new(&value);
        let result = r.u8()?;
        let parsed = (|| -> Option<MasteryLevelDownResponse> {
            match result {
                1 => Some(MasteryLevelDownResponse::Success {
                    mastery_id: r.u32().ok()?,
                    new_level: r.u8().ok()?,
                }),
                2 => Some(MasteryLevelDownResponse::Failure(r.u16().ok()?)),
                _ => None,
            }
        })()
        .filter(|_| r.pos == value.len());
        Ok(parsed.unwrap_or_else(|| MasteryLevelDownResponse::Unknown {
            result,
            tail: value.slice(1..),
        }))
    }
}

impl From<MasteryLevelDownResponse> for Bytes {
    fn from(p: MasteryLevelDownResponse) -> Self {
        let mut buf = BytesMut::new();
        match p {
            MasteryLevelDownResponse::Success {
                mastery_id,
                new_level,
            } => {
                buf.put_u8(1);
                buf.put_u32_le(mastery_id);
                buf.put_u8(new_level);
            }
            MasteryLevelDownResponse::Failure(code) => {
                buf.put_u8(2);
                buf.put_u16_le(code);
            }
            MasteryLevelDownResponse::Unknown { result, tail } => {
                buf.put_u8(result);
                buf.extend_from_slice(&tail);
            }
        }
        buf.freeze()
    }
}

// --- Stat point allocation (0x7050/0x7051) ----------------------------------
//
// Capture-VERIFIED on vSRO 1.188 (2026-08-06): empty 0x7050 requests ack
// `01` on success and `02 7406` when the wallet is empty
// (docs/net-stats-0x7050-0x7051.md). The stat-point balance and the new
// STR/INT arrive separately (0x304E StatPoints, 0x303D stats refresh), so
// the acks carry no payload on success. 0x7051 (INT) shares the shape but
// is uncaptured.

/// 0x7050 — client → server "spend one stat point on strength".
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct IncreaseStrRequest;

/// 0x7051 — client → server "spend one stat point on intelligence".
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct IncreaseIntRequest;

/// 0xB050 — server → client ack for [`IncreaseStrRequest`]. Assumed `01` on
/// success and `02 <code u16>` on error by analogy with the other acks;
/// anything else lands in [`Self::Unknown`] raw.
#[derive(Message, Clone, Debug, PartialEq)]
pub enum IncreaseStrResponse {
    Success,
    Failure(u16),
    Unknown { result: u8, tail: Bytes },
}

/// 0xB051 — server → client ack for [`IncreaseIntRequest`] (same shape as
/// [`IncreaseStrResponse`]).
#[derive(Message, Clone, Debug, PartialEq)]
pub enum IncreaseIntResponse {
    Success,
    Failure(u16),
    Unknown { result: u8, tail: Bytes },
}

macro_rules! stat_ack_wire {
    ($name:ident) => {
        impl TryFrom<Bytes> for $name {
            type Error = SerializationError;
            fn try_from(value: Bytes) -> Result<Self, Self::Error> {
                let mut r = Reader::new(&value);
                let result = r.u8()?;
                let parsed = (|| -> Option<$name> {
                    match result {
                        1 => Some($name::Success),
                        2 => Some($name::Failure(r.u16().ok()?)),
                        _ => None,
                    }
                })()
                // leftover bytes = wrong shape guess; keep raw rather than misread
                .filter(|_| r.pos == value.len());
                Ok(parsed.unwrap_or_else(|| $name::Unknown {
                    result,
                    tail: value.slice(1..),
                }))
            }
        }

        impl From<$name> for Bytes {
            fn from(p: $name) -> Self {
                let mut buf = BytesMut::new();
                match p {
                    $name::Success => buf.put_u8(1),
                    $name::Failure(code) => {
                        buf.put_u8(2);
                        buf.put_u16_le(code);
                    }
                    $name::Unknown { result, tail } => {
                        buf.put_u8(result);
                        buf.extend_from_slice(&tail);
                    }
                }
                buf.freeze()
            }
        }
    };
}

stat_ack_wire!(IncreaseStrResponse);
stat_ack_wire!(IncreaseIntResponse);

// --- NPC talk (0x7046/0xB046, 0x704B/0xB04B) --------------------------------
//
// 0x7046 and 0x704B capture-VERIFIED on vSRO 1.188 (2026-08-06): the talk
// acks `01` plus the echoed talk flag, with errors `02 0500` (out of range /
// unselected) and `02 0b1c` (session already open — cleared by sending the
// close first); the close acks a bare `01` (docs/net-npc-talk-0x7046.md).

/// 0x7046 — client → server "start talking to this NPC" (flag 1 = open the
/// talk dialog).
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct TalkRequest {
    pub unique_id: u32,
    pub talk_flag: u8,
}

/// 0xB046 — server → client ack for [`TalkRequest`]. Only the result byte is
/// interpreted; a success' payload (if any) stays raw until captured.
#[derive(Message, Clone, Debug, PartialEq)]
pub enum TalkResponse {
    Success { tail: Bytes },
    Failure(u16),
    Unknown { result: u8, tail: Bytes },
}

impl TryFrom<Bytes> for TalkResponse {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, Self::Error> {
        let mut r = Reader::new(&value);
        let result = r.u8()?;
        Ok(match result {
            1 => TalkResponse::Success {
                tail: value.slice(1..),
            },
            2 => match r.u16() {
                Ok(code) if r.pos == value.len() => TalkResponse::Failure(code),
                _ => TalkResponse::Unknown {
                    result,
                    tail: value.slice(1..),
                },
            },
            _ => TalkResponse::Unknown {
                result,
                tail: value.slice(1..),
            },
        })
    }
}

impl From<TalkResponse> for Bytes {
    fn from(p: TalkResponse) -> Self {
        let mut buf = BytesMut::new();
        match p {
            TalkResponse::Success { tail } => {
                buf.put_u8(1);
                buf.extend_from_slice(&tail);
            }
            TalkResponse::Failure(code) => {
                buf.put_u8(2);
                buf.put_u16_le(code);
            }
            TalkResponse::Unknown { result, tail } => {
                buf.put_u8(result);
                buf.extend_from_slice(&tail);
            }
        }
        buf.freeze()
    }
}

/// 0x7059 — client → server "make this teleporter my recall point"
/// (`DesignateRecall`). A single u32, verified from the original's builder.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct TeleportRecallRequest {
    pub teleport_unique_id: u32,
}

/// 0xB059 — server → client ack for [`TeleportRecallRequest`].
///
/// **Entire body [U], so nothing is claimed about it.** The opcode is declared in
/// the original's enum but has no dispatch case and no parser, and go-sro has no
/// handler either — there is no source for a layout. The doc guesses
/// `success u8 [+ tail]` "by family analogy"; that guess is not encoded here,
/// because splitting a leading byte off an unknown body would also make an empty
/// body fail to decode. Kept whole and log-only until
/// `packet_dump/0xb059.log` exists.
#[derive(Message, Clone, Debug, PartialEq)]
pub struct TeleportRecallResponse {
    pub raw: Bytes,
}

impl TryFrom<Bytes> for TeleportRecallResponse {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, SerializationError> {
        Ok(TeleportRecallResponse { raw: value })
    }
}

impl From<TeleportRecallResponse> for Bytes {
    fn from(p: TeleportRecallResponse) -> Self {
        p.raw
    }
}

/// 0x705A — client → server "teleport me via this teleporter" (per
/// SilkroadDoc AGENT_TELEPORT_USE). The `kind` discriminator is 2 for a
/// designated-destination teleport; the destination is the teleportdata id as
/// a **u32** — the 2026-08-06 playtest proved the u16 guess wrong the hard
/// way: the server read 2 bytes past the 7-byte body and reset the
/// connection without any 0xB05A.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct TeleportRequest {
    pub npc_unique_id: u32,
    pub kind: u8,
    pub destination_id: u32,
}

/// 0xB05A — server → client ack for [`TeleportRequest`]. Capture-VERIFIED
/// vSRO 1.188 (2026-08-07, the first successful teleport,
/// `packet_dump/0xb05a.log`): a TWO-PHASE ack — `02 01 00` (begin, with the
/// zone-teardown despawns sandwiched after it) then `01` 43 ms later
/// (committed, right before the 0x34B5 GameReset). The earlier
/// `stat_ack_wire!` guess read the `02 …` phase as a Failure, which it
/// plainly is not. Logging-only either way.
#[derive(Message, Clone, Debug, PartialEq)]
pub enum TeleportResponse {
    /// `02 <code u16>` — teleport accepted, teardown starting (code 1
    /// observed; error codes may share this shape — unknown until seen).
    Begin { code: u16 },
    /// `01` — teleport committed; 0x34B5 GameReset follows.
    Committed,
    /// Anything else — kept raw, log-only.
    Unknown { result: u8, tail: Bytes },
}

impl TryFrom<Bytes> for TeleportResponse {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, SerializationError> {
        let mut r = Reader::new(&value);
        let result = r.u8()?;
        let parsed = (|| -> Option<TeleportResponse> {
            match result {
                1 => Some(TeleportResponse::Committed),
                2 => Some(TeleportResponse::Begin {
                    code: r.u16().ok()?,
                }),
                _ => None,
            }
        })()
        .filter(|_| r.pos == value.len());
        Ok(parsed.unwrap_or_else(|| TeleportResponse::Unknown {
            result,
            tail: value.slice(1..),
        }))
    }
}

impl From<TeleportResponse> for Bytes {
    fn from(p: TeleportResponse) -> Self {
        let mut buf = BytesMut::new();
        match p {
            TeleportResponse::Begin { code } => {
                buf.put_u8(2);
                buf.put_u16_le(code);
            }
            TeleportResponse::Committed => buf.put_u8(1),
            TeleportResponse::Unknown { result, tail } => {
                buf.put_u8(result);
                buf.extend_from_slice(&tail);
            }
        }
        buf.freeze()
    }
}

/// 0x705B — client → server "abort the cast I am in the middle of". Empty body.
///
/// The cancel behind `GDR_DI_CANCEL`, the button on the cast/delay gauge. `[V]`
/// from the original client: `FUN_0081ee60` builds it twice, both times
/// `FUN_00841780(0x705b, 0)` with no payload, and the surrounding UI strings are
/// `UIIT_STT_TRANSITION_CANCEL` (the button) and
/// `UIIT_MSG_TRANSITION_CANCEL_RESULT` (the ack's message). See
/// `docs/re/net/outbound/progression-teleport.md`.
///
/// ⚠️ **vSRO never writes the [`TransitionCastingCancelResponse`]**, so a
/// go-sro-derived server is expected to ignore this. Do not read silence as a
/// malformed request — read it as an unimplemented one. Distinct from
/// [`ObjectActionRequest::Cancel`] (`0x7074`, byte `02`), which aborts the
/// object-action loop and has no bearing on a `0x704C` item cast.
#[derive(Message, Clone, Debug, Default)]
pub struct TransitionCastingCancelRequest;

empty_packet!(TransitionCastingCancelRequest);

/// 0xB05B — server → client: result of [`TransitionCastingCancelRequest`].
///
/// `[V]` from the original's handler `FUN_008727a0`: `result == 1` is bare and
/// shows `UIIT_MSG_TRANSITION_CANCEL_RESULT`; `result == 2` carries a `u16` the
/// original reads but never displays. Same shape as [`LogoutCancelResponse`],
/// which is the other "cancel a pending countdown" pair.
///
/// Unobserved in any capture of ours — see the request's note.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug)]
pub struct TransitionCastingCancelResponse {
    pub result: u8,
    #[sro_packet(when = "result == 2")]
    pub error: Option<u16>,
}

/// 0x704B — client → server "close the talk session with this NPC".
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct CloseTalkRequest {
    pub unique_id: u32,
}

/// 0xB04B — server → client ack for [`CloseTalkRequest`] (assumed the common
/// `01` / `02 <code u16>` shape).
#[derive(Message, Clone, Debug, PartialEq)]
pub enum CloseTalkResponse {
    Success,
    Failure(u16),
    Unknown { result: u8, tail: Bytes },
}

stat_ack_wire!(CloseTalkResponse);

/// [`EntityStateUpdate::kind`]: the entity's life state changed.
pub const STATE_KIND_LIFE: u8 = 0;
/// [`EntityStateUpdate::kind`]: the entity's motion state changed.
pub const STATE_KIND_MOTION: u8 = 1;
/// [`EntityStateUpdate::kind`]: the entity's body state changed (invisibility,
/// invincibility, stealth, berserk — skrillax `UpdatedState::Body`).
pub const STATE_KIND_BODY: u8 = 4;
/// Life-state values (kind 0).
pub const LIFE_STATE_ALIVE: u8 = 1;
pub const LIFE_STATE_DEAD: u8 = 2;

/// Motion-state values (kind 1), from live 0x30BF captures: monsters toggling
/// their wander gait send `2` when they start strolling and `3` when they run
/// (`packet_dump/0x30bf.log`: 15 walk vs 41 run lines in one session). The
/// same byte is the `motion_state` of every spawn/state block.
pub const MOTION_STATE_WALK: u8 = 2;
pub const MOTION_STATE_RUN: u8 = 3;

/// A state kind the census found on the wire but no source names: a clean 1/0
/// toggle that appears **only on the local player's uid** (16 of the 120
/// `packet_dump/0x30bf.log` bodies), goes `1` when a fight starts, and whose
/// `→ 0` transition lands in the same millisecond as a death in 6 of 8 cases —
/// i.e. at every captured death. Read as the in-combat flag the spawn record
/// also carries; `[S]` inference, see `docs/net-death-resurrect.md` §4.
pub const STATE_KIND_COMBAT: u8 = 8;

/// Body-state values (kind 4), from skrillax's `BodyState`. The three that
/// hide the entity are grouped in [`body_state_is_invisible`].
pub const BODY_STATE_NONE: u8 = 0;
/// Post-resurrect invulnerability (skrillax: `Untouchable`). All three captured
/// revives set it in the same millisecond as life→alive and clear it after
/// 6.13–6.29 s; it occurs nowhere else in the capture
/// (`docs/net-death-resurrect.md` §2).
pub const BODY_STATE_UNTOUCHABLE: u8 = 2;
pub const BODY_STATE_GM_INVINCIBLE: u8 = 3;
pub const BODY_STATE_GM_INVISIBLE: u8 = 4;
pub const BODY_STATE_STEALTH: u8 = 6;
pub const BODY_STATE_INVISIBLE: u8 = 7;

/// How a hidden entity should be drawn for *somebody else's* character.
///
/// [`body_state_is_invisible`] answers "is this entity hidden", which is the
/// right question for your **own** body — all three values mean you are hidden
/// — but the wrong one for drawing another character, because GM invisibility
/// and stealth are not the same secret. A GM is meant to see other GMs as
/// translucent ghosts; nobody is meant to see a stealthed player at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HiddenRender {
    /// Draw normally.
    Visible,
    /// Draw translucent — GM invisibility, seen by a GM.
    Ghost,
    /// Do not draw.
    Hidden,
}

/// Resolve [`HiddenRender`] for another character's body state.
///
/// **Caveat, and the reason callers must scope this to players.** A census of
/// `packet_dump/0x30bf.log` (2982 bodies, all 6 bytes) finds kind 4 on **217
/// distinct unique ids**, 229 of those events carrying value 4 — and only 4 of
/// those ids ever emit a life state, so most are not monsters in combat. If
/// value 4 really meant GM-invisible for all of them, applying this to every
/// entity would hide two hundred of them from a non-GM. What value 4 means for
/// a **non-player** entity is therefore **UNKNOWN**, and this must be asked
/// only about characters until a capture settles it.
pub fn hidden_render(body_state: u8, viewer_is_gm: bool) -> HiddenRender {
    match body_state {
        // A GM's own invisibility: fellow GMs see the ghost, nobody else sees
        // anything.
        BODY_STATE_GM_INVISIBLE if viewer_is_gm => HiddenRender::Ghost,
        BODY_STATE_GM_INVISIBLE => HiddenRender::Hidden,
        // Stealth and player invisibility are gameplay, not moderation: being
        // a GM does not entitle you to see through them here.
        BODY_STATE_STEALTH | BODY_STATE_INVISIBLE => HiddenRender::Hidden,
        _ => HiddenRender::Visible,
    }
}

/// 0xB0BD — server → client: a buff landed on an entity. Capture-verified
/// vSRO 1.188 (`packet_dump/0xb0bd.log`, fixed 12-byte payloads; the server
/// self-casts skill 39110 on every join and announces it here). No duration
/// on the wire — the client derives it from skilldata's `'dura'` param.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct BuffAdd {
    /// Unique id of the entity the buff applies to.
    pub unique_id: u32,
    pub ref_skill_id: u32,
    /// Instance id echoed by the (assumed) 0xB072 removal.
    pub buff_instance_id: u32,
}

/// 0xB0BD's counterpart, 0xB072 — buffs removed by instance id. The leading
/// byte is a **COUNT, not a result**: the body is a list.
///
/// Read from the original's handler (`sro_client.exe@008a4de0`, table A):
/// `FUN_004f7220(&param_1,1)` takes one byte, then
/// `cVar3 = (char)param_1; while (cVar3 != 0) { cVar3--; FUN_004f7220(&uStack_4,4); … }`
/// consumes exactly that many u32 ids, resolving each to its ref skill id
/// locally (`FUN_00a56620`). The server writer agrees: `0059ecd0` derives the
/// byte from a vector size (`(*(int *)(p+0x2a0) - *(int *)(p+0x29c)) >> 2`)
/// and writes one u32 per element, while the five single-removal writers
/// (`0059bfe0`, `0059f020`, `0059f8b0`, `005a16c0`, `005a1ce0`) hard-code it to 1.
///
/// Every captured line so far is a one-element list (`packet_dump/0xb072.log`,
/// 6 lines 2026-08-11/2026-08-15, e.g. `01 8c030000`) — which is exactly why the
/// earlier "result byte" reading survived: a single-removal capture cannot tell
/// `{result:1, id}` from `{count:1, [id]}`. The decompile can, so a multi-removal
/// push no longer loses every id but the first.
#[derive(Message, Clone, Debug, PartialEq)]
pub struct BuffRemove {
    /// Instance ids to drop, in wire order (count-prefixed, never empty in practice).
    pub buff_instance_ids: Vec<u32>,
}

impl TryFrom<Bytes> for BuffRemove {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, Self::Error> {
        let mut r = Reader::new(&value);
        let count = r.u8()?;
        let mut buff_instance_ids = Vec::with_capacity(count as usize);
        for _ in 0..count {
            buff_instance_ids.push(r.u32()?);
        }
        Ok(BuffRemove { buff_instance_ids })
    }
}

impl From<BuffRemove> for Bytes {
    fn from(p: BuffRemove) -> Self {
        let mut buf = BytesMut::new();
        buf.put_u8(p.buff_instance_ids.len() as u8);
        for id in &p.buff_instance_ids {
            buf.put_u32_le(*id);
        }
        buf.freeze()
    }
}

/// 0x70A7 — client → server: the hwan (jahwan / berserk) activation request.
///
/// One byte. The original's builder writes exactly one
/// (`sro_client.exe@0081e690`: `FUN_00841780(0x70a7,…)` opens the packet at
/// `:21` and `FUN_00508fe0(&stack0x00000004,1)` at `:30` writes the byte it was
/// handed by its caller). **What that byte enumerates is `[U]`** — the corpus
/// has no caller of `FUN_0081e690`, so the value space is unbounded; skrillax
/// is the only source for `1 = berserk`, which is the sole value we send.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct HwanActionRequest {
    pub action: u8,
}

/// The only 0x70A7 action value with a source ([S], skrillax): activate.
pub const HWAN_ACTION_BERSERK: u8 = 1;

impl HwanActionRequest {
    pub fn berserk() -> Self {
        HwanActionRequest {
            action: HWAN_ACTION_BERSERK,
        }
    }
}

/// 0xB0A7 — the server's answer to 0x70A7.
///
/// The handler (`sro_client.exe@008a7a20`) reads one byte, and **only when it
/// is not `1`** reads a `u16` and hands it to the message box
/// (`FUN_00778190(0x1a, code, …)`). So the error code is conditional, not a
/// fixed trailer: a success body is a single byte.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct HwanActionResponse {
    pub result: u8,
    #[sro_packet(when = "result != 1")]
    pub error_code: Option<u16>,
}

impl HwanActionResponse {
    pub fn is_success(&self) -> bool {
        self.result == 1
    }
}

/// 0x30DF — server → client HWANLEVEL: an entity's hwan level changed.
///
/// `sro_client.exe@008a7630` reads a `u32` and a `u8` (`:8-9`), then resolves
/// the `u32` through the object registry (`NetProcessInObject.cpp:0xa7d`)
/// before applying the byte — so the `u32` is a unique id and the `u8` is the
/// level. What the level drives visually (the hwan aura tier) is not modelled
/// on our side yet.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct HwanLevelUpdate {
    pub unique_id: u32,
    pub level: u8,
}

/// Whether a body-state value renders the entity invisible (GM invisible,
/// player invisible, or stealth).
pub fn body_state_is_invisible(body_state: u8) -> bool {
    matches!(
        body_state,
        BODY_STATE_GM_INVISIBLE | BODY_STATE_INVISIBLE | BODY_STATE_STEALTH
    )
}

/// 0x30BF — server → client entity state change. Verified against live vSRO
/// 1.188 captures: `kind` 0 = life state (1 alive, **2 dead — arrives when a
/// monster dies, before its despawn**), `kind` 1 = motion state (2 walk,
/// 3 run — monsters toggling their wander gait). `kind` 4 = body state
/// (invisibility/invincibility/stealth), which the GM `/invisible` toggle
/// produces for the acting entity.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct EntityStateUpdate {
    pub unique_id: u32,
    pub kind: u8,
    pub value: u8,
}

impl EntityStateUpdate {
    /// The entity just died (life state → dead).
    pub fn is_death(&self) -> bool {
        self.kind == STATE_KIND_LIFE && self.value == LIFE_STATE_DEAD
    }

    /// The entity just revived (life state → alive) — the local player's
    /// respawn/get-up signal (#143). SPEC-derived: a live capture must confirm
    /// the server sends this to the reviving player itself (see
    /// docs/re/notes/death-resurrect.md).
    pub fn is_revive(&self) -> bool {
        self.kind == STATE_KIND_LIFE && self.value == LIFE_STATE_ALIVE
    }

    /// `Some(is_invisible)` when this is a body-state update, else `None`.
    pub fn body_invisibility(&self) -> Option<bool> {
        (self.kind == STATE_KIND_BODY).then(|| body_state_is_invisible(self.value))
    }

    /// The raw body-state value when this is a body-state update, else `None`.
    ///
    /// [`Self::body_invisibility`] collapses the value to a yes/no, which is
    /// all your own body needs. Drawing *another* character needs the value
    /// itself, because GM invisibility and stealth are shown differently — see
    /// [`hidden_render`].
    pub fn body_state_value(&self) -> Option<u8> {
        (self.kind == STATE_KIND_BODY).then_some(self.value)
    }

    /// `Some(is_walking)` when this is a motion-state update naming a gait,
    /// else `None`. Only the two observed gaits answer: an unknown motion
    /// value must not be guessed into "running" and silently pick an
    /// animation.
    pub fn motion_walking(&self) -> Option<bool> {
        if self.kind != STATE_KIND_MOTION {
            return None;
        }
        match self.value {
            MOTION_STATE_WALK => Some(true),
            MOTION_STATE_RUN => Some(false),
            _ => None,
        }
    }
}

/// 0x3053 — client → server CLIENT_CHARACTER_AUTORESURRECTION: the death
/// window's resurrect request. Carries a single option byte selecting the
/// resurrect mode. The old empty-body model was flagged SPEC-derived pending a
/// selector check; the xBot 1.188 source settles it — `ResurrectAtPresentPoint`
/// writes exactly `WriteByte(2)` (vSRO xBot `PacketBuilder.cs:498-503`), so the
/// body is one option byte, not empty.
///
/// Two option values are now sourced:
///
/// - `2` [V] present resurrection point — xBot `PacketBuilder.cs:498-503`.
/// - `1` [S] return to the designated resurrection point (town) — the vSRO
///   clientless bot "Vsro Multi Tool" answers the post-death `0x30D2` push with
///   `new Packet(0x3053); WriteUInt8(1)` under the comment *"ress pvp or back
///   to town if dead"* (`tools/Vsro Multi Tool/Clientless_login/Agent.cs:135-141`).
///   Single source, behavioural rather than symbolic, hence [S] not [V] — but it
///   is the only always-available option, and it is what the present-point value
///   is not: `packet_dump/c2s/0x3053.log` shows five `02` sends on 2026-08-15
///   at 10:50:00–10:50:11 that the live server answered with *nothing*, because
///   vSRO gates present-point resurrect (level/scroll).
///
/// The "wait for other player's help" case sends no packet at all (it is the
/// server's default: stay dead), so it has no option byte.
///
/// Direction caveat: 0x3053 sits in the 0x3xxx range this repo otherwise treats
/// as S→C, but death/resurrect is the documented C→S exception (two independent
/// sources agree). See docs/re/notes/death-resurrect.md.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, Default, PartialEq)]
pub struct GetUpRequest {
    pub option: u8,
}

impl GetUpRequest {
    /// Return to the designated resurrection point / town (value `1`).
    pub const RETURN_TO_TOWN: u8 = 1;

    /// Free resurrect at the present resurrection point (xBot value `2`).
    pub const PRESENT_POINT: u8 = 2;

    /// Resurrect at the designated resurrection point (the town return): the
    /// option a dead character always has, whatever its level.
    pub fn return_to_town() -> Self {
        Self {
            option: Self::RETURN_TO_TOWN,
        }
    }

    /// The evidence-confirmed free resurrect (present resurrection point).
    pub fn present_point() -> Self {
        Self {
            option: Self::PRESENT_POINT,
        }
    }
}

/// 0x3054 — server → client: the entity leveled up (play the level-up
/// effect on it). Verified against a live capture: bare unique id.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct EntityLevelUp {
    pub unique_id: u32,
}

/// 0x3011 — server → client: the local character died (drives the death
/// window / respawn UI). One byte, witnessed twice over: the original reads
/// exactly one byte and stops, and all three `packet_dump/0x3011.log` samples
/// are a single `04`.
///
/// UNKNOWN: the value space of [`Self::death_cause`]. Only `0x04` has ever
/// been observed, and the original's own comment merely guesses at it
/// ("4 = Dead by mob?"), so the raw byte is exposed rather than branched on.
/// It is **not** the `LifeState` discriminator carried by spawn/state-update
/// — do not conflate the two.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct CharacterDied {
    pub death_cause: u8,
}

/// 0x304D — server → client: a dropped item's owner-lock has expired, so
/// anyone may pick it up now. Body is the drop entity's unique id.
///
/// Layout is **supported, not verified**: the original has no parser for this
/// opcode (an enum entry only), so the sole witness is one live capture —
/// `packet_dump/0x304d.log` = `aa600100` → `0x000160AA` — which reads cleanly
/// as the `u32` unique id that drop spawns and their owner field both use.
/// With a single sample trailing fields cannot be ruled out; the generated
/// decode ignores a tail, so a longer real body degrades to "unique id only"
/// rather than failing.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct DropUnlocked {
    pub unique_id: u32,
}

/// 0x3056 — server → client experience delta: a gain from a kill, or the
/// negative EXP penalty charged on death (see [`Self::experience`]). Verified against
/// live vSRO 1.188 captures (21-byte bodies, empty tail); a level-up appends
/// a trailing u16 that is the character's **total stat points**, NOT the new
/// level — the values seen (3, 6, 9, 12) are 3×(level-1), i.e. the 3
/// stat-points-per-level award (skrillax mislabels it `new_level`). The level
/// itself is derived from the exp curve (leveldata), so this is exposed only
/// as the stat-point total.
#[derive(Message, Clone, Debug, PartialEq)]
pub struct ReceiveExperience {
    /// Unique id of the entity that provided the experience.
    pub exp_origin: u32,
    /// Experience delta — **signed**: a kill grants a positive value, death
    /// charges the EXP penalty as a negative one (`packet_dump/0x3056.log`
    /// carries `5df3ffffffffffff` = −3235 on both captured deaths, with
    /// `exp_origin` set to the dying player's own uid).
    pub experience: i64,
    /// Skill-experience points gained (400 sp-exp = 1 SP).
    pub sp_exp: u64,
    /// Flag for extra trailing data (0 in all captures so far).
    pub unknown: u8,
    /// Raw remainder, see [`Self::stat_points`].
    pub tail: Bytes,
}

impl ReceiveExperience {
    /// The character's total stat points, present only when this gain caused
    /// a level-up (its presence therefore also flags "a level-up happened").
    /// The client applies this to the stat-point wallet in
    /// `character_info::model::on_experience_stat_points` — required because
    /// some servers (e.g. the reference vSRO) never send a 0x304E `StatPoints`
    /// refresh, so without it the wallet would freeze at the login value.
    pub fn stat_points(&self) -> Option<u16> {
        (self.tail.len() >= 2).then(|| u16::from_le_bytes(self.tail[0..2].try_into().unwrap()))
    }
}

impl TryFrom<Bytes> for ReceiveExperience {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, Self::Error> {
        let mut r = Reader::new(&value);
        Ok(ReceiveExperience {
            exp_origin: r.u32()?,
            experience: r.i64()?,
            sp_exp: r.u64()?,
            unknown: r.u8()?,
            tail: value.slice(r.pos..),
        })
    }
}

impl From<ReceiveExperience> for Bytes {
    fn from(p: ReceiveExperience) -> Self {
        let mut buf = BytesMut::new();
        buf.put_u32_le(p.exp_origin);
        buf.put_i64_le(p.experience);
        buf.put_u64_le(p.sp_exp);
        buf.put_u8(p.unknown);
        buf.extend_from_slice(&p.tail);
        buf.freeze()
    }
}

// --- Invite / petition popup (0x3080) --------------------------------------

/// `SRTypes.PlayerPetition` — which invite the 0x3080 popup is for.
pub const PETITION_EXCHANGE: u8 = 1;
pub const PETITION_PARTY_CREATION: u8 = 2;
pub const PETITION_PARTY_INVITATION: u8 = 3;
pub const PETITION_RESURRECTION: u8 = 4;
pub const PETITION_GUILD: u8 = 5;
pub const PETITION_UNION: u8 = 6;
pub const PETITION_ACADEMY: u8 = 9;

/// `SRParty.Setup` flags carried by the two party petitions.
pub const PARTY_SETUP_EXP_SHARED: u8 = 1;
pub const PARTY_SETUP_ITEM_SHARED: u8 = 2;
pub const PARTY_SETUP_ANYONE_CAN_INVITE: u8 = 4;

/// The petition body the server pushes to the invitee.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvitePetition {
    /// One of the `PETITION_*` constants.
    pub petition: u8,
    /// UNKNOWN whether this is a spawn id or a petition token
    /// (docs/net-invite-0x3080.md §7) — it is echoed by nothing, so the
    /// server correlates the answer by session either way.
    pub unique_id: u32,
    /// Party petitions only (`PARTY_CREATION`/`PARTY_INVITATION`): the
    /// `PARTY_SETUP_*` flags. Capacity is 8 when `EXP_SHARED` is set, else 4.
    pub setup: Option<u8>,
}

impl InvitePetition {
    pub fn is_party(&self) -> bool {
        matches!(
            self.petition,
            PETITION_PARTY_CREATION | PETITION_PARTY_INVITATION
        )
    }
}

/// The invitee's answer. The wire carries no echoed id or type — the server
/// correlates by session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InviteResponse {
    /// `01 01`
    Accept,
    /// `01 00`
    Decline,
    /// `02 0C 2C` — the party-specific decline.
    ///
    /// UNKNOWN whether `0x2C0C` is a constant or a reason parameter
    /// (docs/net-invite-0x3080.md §7).
    DeclineParty,
}

/// 0x3080 — the shared invite/petition popup.
///
/// The opcode is **dual-mapped**: server → client it is
/// `SERVER_PLAYER_PETITION_REQUEST` (the popup on the invitee), client →
/// server it is `CLIENT_PLAYER_INVITATION_RESPONSE` (accept/decline). One
/// opcode services party / exchange / guild / academy / resurrection invites,
/// keyed by the leading `type` byte.
///
/// Because the two directions carry different bodies under one opcode, this
/// enum is the codec for both: decoding always yields [`Self::Petition`] and
/// encoding a [`Self::Response`] emits the answer bytes.
#[derive(Message, Clone, Debug, PartialEq)]
pub enum GameInvite {
    /// S→C.
    Petition(InvitePetition),
    /// C→S.
    Response(InviteResponse),
}

impl TryFrom<Bytes> for GameInvite {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, Self::Error> {
        let mut r = Reader::new(&value);
        let petition = r.u8()?;
        let unique_id = r.u32()?;
        let mut invite = InvitePetition {
            petition,
            unique_id,
            setup: None,
        };
        // the setup byte is party-only; tolerate its absence rather than fail
        if invite.is_party() {
            invite.setup = r.u8().ok();
        }
        Ok(GameInvite::Petition(invite))
    }
}

impl From<GameInvite> for Bytes {
    fn from(p: GameInvite) -> Self {
        let mut buf = BytesMut::new();
        match p {
            GameInvite::Response(InviteResponse::Accept) => buf.extend_from_slice(&[0x01, 0x01]),
            GameInvite::Response(InviteResponse::Decline) => buf.extend_from_slice(&[0x01, 0x00]),
            GameInvite::Response(InviteResponse::DeclineParty) => {
                buf.extend_from_slice(&[0x02, 0x0C, 0x2C])
            }
            // Round-trip only: the client never sends a petition.
            GameInvite::Petition(invite) => {
                buf.put_u8(invite.petition);
                buf.put_u32_le(invite.unique_id);
                if let Some(setup) = invite.setup {
                    buf.put_u8(setup);
                }
            }
        }
        buf.freeze()
    }
}

/// 0x7081 — ask to exchange with an entity; the server answers the target with
/// a 0x3080 petition. The party (0x7062), guild (0x70F3) and academy (0x7472)
/// funnels carry the same single-`u32` body.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct ExchangeInviteRequest {
    pub unique_id: u32,
}

/// 0x7062 — invite an entity to the party.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct PartyInviteRequest {
    pub unique_id: u32,
}

/// 0x70F3 — invite an entity to the guild.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct GuildInviteRequest {
    pub unique_id: u32,
}

/// 0x7472 — invite an entity to the academy.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct AcademyInviteRequest {
    pub unique_id: u32,
}

/// 0xB081 — inviter-side ack that the exchange petition was raised.
///
/// The tail is **selected by the result byte**, which is why this is not a
/// plain derive: `result == 1` carries the target's `u32 uniqueID`, anything
/// else carries a `u16` error code (`docs/re/notes/stall-exchange-storage.md`
/// L173, matching xBot `PacketParser.cs:1213-1221` via
/// `docs/net-invite-0x3080.md` §4). Reading both as one fixed 5-byte body made
/// every *refused* invite fail to decode — a 3-byte body is short, not
/// malformed.
#[derive(Message, Clone, Debug, PartialEq)]
pub enum ExchangeInviteResponse {
    /// The petition reached the target; the window opens on the following
    /// 0x3085.
    Accepted { unique_id: u32 },
    /// The server refused. The code is carried verbatim: no source names its
    /// value space, so interpreting it here would be an invented table.
    Refused { error: u16 },
}

impl TryFrom<Bytes> for ExchangeInviteResponse {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, SerializationError> {
        let short = || {
            SerializationError::IoError(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "0xB081 body too short",
            ))
        };
        let result = *value.first().ok_or_else(short)?;
        if result == EXCHANGE_INVITE_RAISED {
            let uid = value.get(1..5).ok_or_else(short)?;
            Ok(ExchangeInviteResponse::Accepted {
                unique_id: u32::from_le_bytes(uid.try_into().unwrap()),
            })
        } else {
            let code = value.get(1..3).ok_or_else(short)?;
            Ok(ExchangeInviteResponse::Refused {
                error: u16::from_le_bytes(code.try_into().unwrap()),
            })
        }
    }
}

impl From<ExchangeInviteResponse> for Bytes {
    fn from(p: ExchangeInviteResponse) -> Self {
        let mut buf = bytes::BytesMut::new();
        match p {
            ExchangeInviteResponse::Accepted { unique_id } => {
                bytes::BufMut::put_u8(&mut buf, EXCHANGE_INVITE_RAISED);
                bytes::BufMut::put_u32_le(&mut buf, unique_id);
            }
            ExchangeInviteResponse::Refused { error } => {
                bytes::BufMut::put_u8(&mut buf, EXCHANGE_INVITE_REFUSED);
                bytes::BufMut::put_u16_le(&mut buf, error);
            }
        }
        buf.freeze()
    }
}

/// `result` values of [`ExchangeInviteResponse`]. Only 1 is named by a source;
/// 2 is the family's usual "failed" and is what we write when re-encoding a
/// refusal, never something we require on the wire.
pub const EXCHANGE_INVITE_RAISED: u8 = 1;
pub const EXCHANGE_INVITE_REFUSED: u8 = 2;

// --- Logout (0x7005/0xB005/0x7006/0xB006/0x300A) ---------------------------

/// Logout mode sent in [`LogoutRequest`].
pub const LOGOUT_MODE_EXIT: u8 = 1;
pub const LOGOUT_MODE_RESTART: u8 = 2;
/// Logout error codes (in [`LogoutResponse::error`]).
pub const LOGOUT_ERROR_IN_BATTLE: u16 = 0x801;
pub const LOGOUT_ERROR_IN_TELEPORT: u16 = 0x802;

/// 0x7005 — client → server: begin logout. `mode` is [`LOGOUT_MODE_EXIT`] or
/// [`LOGOUT_MODE_RESTART`].
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug)]
pub struct LogoutRequest {
    pub mode: u8,
}

/// 0xB005 — server → client: logout accepted (result 1, with a countdown and
/// the echoed mode) or rejected (result 2, with an error code).
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug)]
pub struct LogoutResponse {
    pub result: u8,
    #[sro_packet(when = "result == 1")]
    pub countdown: Option<u8>,
    #[sro_packet(when = "result == 1")]
    pub mode: Option<u8>,
    #[sro_packet(when = "result == 2")]
    pub error: Option<u16>,
}

/// 0x7006 — client → server: cancel a pending logout. Empty body.
#[derive(Message, Clone, Debug, Default)]
pub struct LogoutCancelRequest;

/// 0xB006 — server → client: result of a cancel request.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug)]
pub struct LogoutCancelResponse {
    pub result: u8,
    #[sro_packet(when = "result == 2")]
    pub error: Option<u16>,
}

/// 0x300A — server → client: the logout countdown elapsed; perform the
/// exit/restart now. Empty body.
#[derive(Message, Clone, Debug, Default)]
pub struct LogoutSuccess;

empty_packet!(LogoutCancelRequest);
empty_packet!(LogoutSuccess);

// --- Group entity spawn/despawn (0x3017 begin / 0x3019 data / 0x3018 end) ----
//
// After the self-spawn stream, the agent server streams every *other* nearby
// entity (remote players, NPCs, monsters, item drops) as a batch: a
// `GroupEntitySpawnBegin` marker (spawn vs despawn + entity count), one
// `GroupEntitySpawnData` packet carrying all `count` records, then a
// `GroupEntitySpawnEnd` marker. (0x3018 is the empty end and 0x3019 the data —
// verified against a live capture.) The per-record layout is version- and
// itemdata-dependent (a player's equipment list writes an extra byte only for
// equipment items), the same reason `CharacterDataBody` is a raw passthrough, so
// the data packet is carried unparsed and decoded in the client where itemdata
// lives (see `client/src/net/entity_spawn.rs`).

/// [`GroupEntitySpawnBegin::kind`]: this batch adds entities.
pub const GROUP_SPAWN: u8 = 1;
/// [`GroupEntitySpawnBegin::kind`]: this batch removes entities.
pub const GROUP_DESPAWN: u8 = 2;

/// 0x3017 — start of a group spawn/despawn batch. `kind` is [`GROUP_SPAWN`] or
/// [`GROUP_DESPAWN`]; `count` is the number of records in the following
/// [`GroupEntitySpawnData`]. (Some servers append trailing "unknown" fields;
/// they are ignored on decode.)
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug)]
pub struct GroupEntitySpawnBegin {
    pub kind: u8,
    pub count: u16,
}

/// 0x3019 — the group spawn/despawn payload: `count` records (per
/// [`GroupEntitySpawnBegin`]). For a spawn each record is a full entity; for a
/// despawn each record is a bare `u32` unique id. Carried unparsed; the client
/// decodes it with itemdata (see `client/src/net/entity_spawn.rs`).
#[derive(Message, Clone, Debug)]
pub struct GroupEntitySpawnData {
    pub raw: Bytes,
}

/// 0x3018 — end of a group spawn/despawn batch. Empty body.
#[derive(Message, Clone, Debug, Default)]
pub struct GroupEntitySpawnEnd;

/// 0x3015 — a single entity entering view outside a group batch (monster
/// respawn, a player walking into range). The body is exactly one
/// [`GroupEntitySpawnData`] spawn record (ref id + type-dependent data), so it
/// is carried unparsed for the same reason: decoding needs the client's
/// itemdata tables.
#[derive(Message, Clone, Debug)]
pub struct SingleEntitySpawn {
    pub raw: Bytes,
}

/// 0x3016 — a single entity leaving view.
#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct SingleEntityDespawn {
    pub unique_id: u32,
}

impl TryFrom<Bytes> for SingleEntitySpawn {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, Self::Error> {
        Ok(SingleEntitySpawn { raw: value })
    }
}

impl From<SingleEntitySpawn> for Bytes {
    fn from(value: SingleEntitySpawn) -> Self {
        value.raw
    }
}

impl TryFrom<Bytes> for GroupEntitySpawnData {
    type Error = SerializationError;
    fn try_from(value: Bytes) -> Result<Self, Self::Error> {
        Ok(GroupEntitySpawnData { raw: value })
    }
}

impl From<GroupEntitySpawnData> for Bytes {
    fn from(value: GroupEntitySpawnData) -> Self {
        value.raw
    }
}

empty_packet!(GroupEntitySpawnEnd);

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn celestial_position_roundtrips() {
        let packet = CelestialPosition {
            unique_id: 0x0A0B0C0D,
            day: 15,
            hour: 13,
            minute: 42,
        };
        let bytes: Bytes = packet.clone().into();
        // u32 LE + u16 LE + u8 + u8
        assert_eq!(&bytes[..], &[0x0D, 0x0C, 0x0B, 0x0A, 0x0F, 0x00, 13, 42]);
        let decoded: CelestialPosition = bytes.try_into().unwrap();
        assert_eq!(decoded.unique_id, packet.unique_id);
        assert_eq!(decoded.day, 15);
        assert_eq!(decoded.hour, 13);
        assert_eq!(decoded.minute, 42);
    }

    /// #213: `0x34BE` was captured six times and never decoded. It is the
    /// real-world server clock packed into one `u32`, per the handler
    /// (`0089a250_FUN_0089a250.c`). Fixture = every line of
    /// `packet_dump/0x34be.log` (2026-08-10), which is what proves the layout:
    /// the decoded minute/second track the dump's own timestamps exactly, ten
    /// minutes apart, across an hour rollover.
    #[test]
    fn server_time_decodes_the_captured_clock_pushes() {
        let captured: [(&[u8; 4], (u16, u8, u8, u8, u8, u8)); 6] = [
            (b"\x1a\xaa\xd5\x59", (2026, 8, 10, 11, 29, 22)),
            (b"\x1a\xaa\x75\x5a", (2026, 8, 10, 11, 39, 22)),
            (b"\x1a\xaa\x15\x5b", (2026, 8, 10, 11, 49, 22)),
            (b"\x1a\xaa\xb5\x5b", (2026, 8, 10, 11, 59, 22)),
            (b"\x1a\x2a\x96\x58", (2026, 8, 10, 12, 9, 22)),
            (b"\x1a\x2a\x36\x59", (2026, 8, 10, 12, 19, 22)),
        ];

        for (wire, (year, month, day, hour, minute, second)) in captured {
            let bytes = Bytes::copy_from_slice(wire);
            let decoded: ServerTime = bytes.clone().try_into().unwrap();
            assert_eq!(
                (
                    decoded.year(),
                    decoded.month(),
                    decoded.day(),
                    decoded.hour(),
                    decoded.minute(),
                    decoded.second()
                ),
                (year, month, day, hour, minute, second),
                "sample {wire:02x?}"
            );
            let back: Bytes = decoded.into();
            assert_eq!(back, bytes, "round-trip {wire:02x?}");
        }
    }

    #[test]
    fn select_entity_response_roundtrips_and_reads_mob_hp() {
        // go-sro mob shape: result=1, unique id, then u8(1) u32(hp) u8 u8.
        let wire = Bytes::from_static(&[1, 0x39, 0x30, 0, 0, 1, 0xA0, 0x0F, 0, 0, 1, 5]);
        let decoded: SelectEntityResponse = wire.clone().try_into().unwrap();
        assert_eq!(decoded.result, 1);
        assert_eq!(decoded.unique_id, 12345);
        assert_eq!(decoded.monster_hp(), Some(4000));
        let back: Bytes = decoded.into();
        assert_eq!(back, wire);

        // go-sro sends hp=0 (stub) — must read as "unknown", not "dead".
        let stub = Bytes::from_static(&[1, 1, 0, 0, 0, 1, 0, 0, 0, 0, 1, 5]);
        let decoded: SelectEntityResponse = stub.try_into().unwrap();
        assert_eq!(decoded.monster_hp(), None);

        // failure shape: result=0 + error byte, no unique id.
        let fail = Bytes::from_static(&[0, 0]);
        let decoded: SelectEntityResponse = fail.try_into().unwrap();
        assert_eq!(decoded.result, 0);
        assert_eq!(decoded.unique_id, 0);
        assert_eq!(decoded.monster_hp(), None);
    }

    #[test]
    fn object_action_request_roundtrips() {
        // cast with an entity target: 01 (execute) 04 (cast) id LE 01 uid LE
        let packet = ObjectActionRequest::Execute(ActionCommand::CastSkill {
            ref_skill_id: 0x0102,
            target: ActionTarget::Entity {
                unique_id: 0x0A0B0C0D,
            },
        });
        let bytes: Bytes = packet.clone().into();
        assert_eq!(
            &bytes[..],
            &[1, 4, 0x02, 0x01, 0, 0, 1, 0x0D, 0x0C, 0x0B, 0x0A]
        );
        let decoded: ObjectActionRequest = bytes.try_into().unwrap();
        assert_eq!(decoded, packet);

        // untargeted cast: 01 04 id LE 00
        let packet = ObjectActionRequest::Execute(ActionCommand::CastSkill {
            ref_skill_id: 3,
            target: ActionTarget::None,
        });
        let bytes: Bytes = packet.clone().into();
        assert_eq!(&bytes[..], &[1, 4, 3, 0, 0, 0, 0]);
        let decoded: ObjectActionRequest = bytes.try_into().unwrap();
        assert_eq!(decoded, packet);

        // cancel: bare 02
        let bytes: Bytes = ObjectActionRequest::Cancel.into();
        assert_eq!(&bytes[..], &[2]);
        let decoded: ObjectActionRequest = bytes.try_into().unwrap();
        assert_eq!(decoded, ObjectActionRequest::Cancel);
    }

    #[test]
    fn object_action_response_decodes_captured_shapes() {
        // the four 2-byte bodies that make up 99% of packet_dump/0xb074.log
        for (wire, expected) in [
            (
                Bytes::from_static(&[1, 0]),
                ObjectActionResponse::Started {
                    code: ACTION_START_CAST,
                },
            ),
            (
                Bytes::from_static(&[1, 1]),
                ObjectActionResponse::Started {
                    code: ACTION_START_ATTACK,
                },
            ),
            (
                Bytes::from_static(&[1, 2]),
                ObjectActionResponse::Started {
                    code: ACTION_START_REJECTED,
                },
            ),
            (
                Bytes::from_static(&[2, 0]),
                ObjectActionResponse::Ended { code: 0 },
            ),
        ] {
            let decoded: ObjectActionResponse = wire.clone().try_into().unwrap();
            assert_eq!(decoded, expected);
            assert_eq!(
                decoded.is_rejected(),
                wire.as_ref() == [1, 2],
                "{wire:?} rejection flag"
            );
            let back: Bytes = decoded.into();
            assert_eq!(back, wire);
        }
    }

    #[test]
    fn object_action_response_unknown_shapes_keep_raw_tail() {
        // over-long or unrecognized phases must land in Unknown and roundtrip
        // untouched. `03 xx 04 40` is no longer among them: #232 decoded it
        // from the handler as `Failed { code, error }`, which is why `03 09 04
        // 40` is asserted below to be typed, not raw.
        for wire in [
            Bytes::from_static(&[2, 0x04, 0x30]),
            Bytes::from_static(&[1]),
            Bytes::from_static(&[4, 9, 4, 0x40]),
        ] {
            let decoded: ObjectActionResponse = wire.clone().try_into().unwrap();
            assert!(
                matches!(decoded, ObjectActionResponse::Unknown { .. }),
                "{wire:?}"
            );
            let back: Bytes = decoded.into();
            assert_eq!(back, wire);
        }

        // the `03` family varies only in its code byte (`03 xx 04 40` ×20 in
        // the 2026-08-06 corpus), and every one of them is now typed
        let decoded: ObjectActionResponse =
            Bytes::from_static(&[3, 9, 4, 0x40]).try_into().unwrap();
        assert_eq!(
            decoded,
            ObjectActionResponse::Failed {
                code: 9,
                error: 0x4004
            }
        );
    }

    #[test]
    fn teleport_ack_and_game_reset_decode_captured_lines() {
        // real packet_dump/0xb05a.log lines (first successful teleport,
        // 2026-08-07): phase 1 `02 01 00`, phase 2 `01`
        let begin = Bytes::from_static(&[0x02, 0x01, 0x00]);
        let decoded: TeleportResponse = begin.clone().try_into().unwrap();
        assert_eq!(decoded, TeleportResponse::Begin { code: 1 });
        let back: Bytes = decoded.into();
        assert_eq!(back, begin);

        let committed = Bytes::from_static(&[0x01]);
        let decoded: TeleportResponse = committed.clone().try_into().unwrap();
        assert_eq!(decoded, TeleportResponse::Committed);
        let back: Bytes = decoded.into();
        assert_eq!(back, committed);

        // real packet_dump/0x34b5.log line: destination region 0x61A7
        let reset = Bytes::from_static(&[0xa7, 0x61]);
        let decoded: GameReset = reset.try_into().unwrap();
        assert_eq!(decoded.region, 0x61A7);

        let complete: Bytes = GameResetComplete.into();
        assert!(complete.is_empty());
    }

    #[test]
    fn get_up_request_carries_the_option_byte() {
        // 0x3053: one option byte. Present-point (free resurrect) = 2, the
        // only value confirmed against xBot (PacketBuilder.cs:498-503).
        let wire: Bytes = GetUpRequest::present_point().into();
        assert_eq!(&wire[..], &[2u8]);
        let decoded: GetUpRequest = wire.try_into().unwrap();
        assert_eq!(decoded, GetUpRequest::present_point());
        assert_eq!(decoded.option, GetUpRequest::PRESENT_POINT);
    }

    /// The town return is the option a dead character always has; its byte is
    /// `1` per the vSRO clientless "Vsro Multi Tool"
    /// (`Clientless_login/Agent.cs:135-141`, "ress pvp or back to town if
    /// dead"). Pinned by a test because the live server silently ignores the
    /// present-point value (`packet_dump/c2s/0x3053.log`, five `02` sends with
    /// no answer), so a regression here is invisible until someone dies.
    #[test]
    fn get_up_request_town_return_is_option_one() {
        let wire: Bytes = GetUpRequest::return_to_town().into();
        assert_eq!(&wire[..], &[1u8]);
        let decoded: GetUpRequest = wire.try_into().unwrap();
        assert_eq!(decoded.option, GetUpRequest::RETURN_TO_TOWN);
        assert_ne!(GetUpRequest::RETURN_TO_TOWN, GetUpRequest::PRESENT_POINT);
    }

    #[test]
    fn buff_remove_decodes_captured_line() {
        // real packet_dump/0xb072.log line (2026-08-11): count 01, instance 0x38c
        let wire = Bytes::from_static(&[0x01, 0x8c, 0x03, 0x00, 0x00]);
        let decoded: BuffRemove = wire.clone().try_into().unwrap();
        assert_eq!(decoded.buff_instance_ids, vec![0x38c]);
        let back: Bytes = decoded.into();
        assert_eq!(back, wire);
    }

    /// The capture cannot separate `{result, id}` from `{count, [id]}` — every
    /// captured line removes exactly one buff. `008a4de0` can: it loops the
    /// leading byte. Pin the multi-element case, which is the one the old
    /// reading silently dropped.
    #[test]
    fn buff_remove_reads_the_leading_byte_as_a_count() {
        let wire = Bytes::from_static(&[
            0x03, 0x8c, 0x03, 0x00, 0x00, 0x27, 0x06, 0x00, 0x00, 0x4c, 0x00, 0x00, 0x00,
        ]);
        let decoded = BuffRemove::try_from(wire.clone()).unwrap();
        assert_eq!(decoded.buff_instance_ids, vec![0x38c, 0x627, 0x4c]);
        let back: Bytes = decoded.into();
        assert_eq!(back, wire);
        // A count that outruns the body is a decode error, not a silent short read.
        assert!(BuffRemove::try_from(Bytes::from_static(&[0x02, 0x8c, 0x03, 0x00, 0x00])).is_err());
    }

    #[test]
    fn object_action_update_decodes_live_self_cast() {
        // real packet_dump/0xb070.log line (self-buff, no target, no damage)
        let wire = Bytes::from_static(&[
            0x01, 0x00, 0x30, 0xC6, 0x98, 0x00, 0x00, 0x90, 0x89, 0x05, 0x00, 0x27, 0x06, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ]);
        let decoded: ObjectActionUpdate = wire.clone().try_into().unwrap();
        assert_eq!(
            decoded,
            ObjectActionUpdate::Success {
                unknown: 0x3000,
                skill_id: 0x98C6,
                source: 0x58990,
                instance: 0x627,
                target: 0,
                kind: ActionKind::None,
            }
        );
        let back: Bytes = decoded.into();
        assert_eq!(back, wire);

        // the matching 0xB071 capture: same instance, no target, kind none
        let wire = Bytes::from_static(&[0x01, 0x27, 0x06, 0, 0, 0, 0, 0, 0, 0]);
        let decoded: SkillEnd = wire.clone().try_into().unwrap();
        assert_eq!(
            decoded,
            SkillEnd::Success {
                instance: 0x627,
                target: 0,
                kind: ActionKind::None,
            }
        );
        let back: Bytes = decoded.into();
        assert_eq!(back, wire);
    }

    #[test]
    fn skill_end_decodes_live_damage_capture() {
        // real packet_dump/0xb071.log line (2026-08-05 19:58:12): the skill
        // cast's damage — killing blow, 151 damage on target 0xc40c
        let wire = Bytes::from_static(&[
            0x01, 0x3B, 0x00, 0x00, 0x00, 0x0C, 0xC4, 0x00, 0x00, 0x01, 0x01, 0x01, 0x0C, 0xC4,
            0x00, 0x00, 0x80, 0x01, 0x97, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ]);
        let decoded: SkillEnd = wire.clone().try_into().unwrap();
        assert_eq!(
            decoded,
            SkillEnd::Success {
                instance: 0x3B,
                target: 0xC40C,
                kind: ActionKind::Attack {
                    damage: Some(DamageContent {
                        instance_count: 1,
                        entities: vec![PerEntityDamage {
                            target: 0xC40C,
                            hits: vec![SkillPartDamage::killing_blow(DamageValue {
                                kind: DamageValue::KIND_NORMAL,
                                amount: 151,
                            })],
                        }],
                    }),
                },
            }
        );
        let back: Bytes = decoded.into();
        assert_eq!(back, wire);

        // junk / truncated bodies stay raw
        for wire in [
            Bytes::from_static(&[0x02, 0x06, 0x30]),
            Bytes::from_static(&[0x01, 0x3B, 0x00]),
        ] {
            let decoded: SkillEnd = wire.clone().try_into().unwrap();
            assert!(matches!(decoded, SkillEnd::Unknown { .. }), "{wire:?}");
            let back: Bytes = decoded.into();
            assert_eq!(back, wire);
        }
    }

    /// Real `packet_dump/0xb070.log` lines (vSRO 1.188, 2026-08-11), one
    /// critical killing blow and one ordinary hit, plus a synthetic line
    /// carrying an outcome byte we have never captured. The census behind the
    /// "only 1 and 2 exist so far" claim is in
    /// `docs/combat-math-server-spec.md` §5.
    #[test]
    fn damage_kind_byte_is_preserved_from_captured_lines() {
        // 01 3002 <skill 40> <src> <inst 0x3ad> <tgt> 01 | 01 01 <tgt>
        // 80 02 1c010000 0000 00  → killing blow, kind 2 (critical), 284
        let wire = Bytes::from_static(&[
            0x01, 0x02, 0x30, 0x28, 0x00, 0x00, 0x00, 0x80, 0xab, 0x01, 0x00, 0xad, 0x03, 0x00,
            0x00, 0xa0, 0x60, 0x01, 0x00, 0x01, 0x01, 0x01, 0xa0, 0x60, 0x01, 0x00, 0x80, 0x02,
            0x1c, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00,
        ]);
        let decoded: ObjectActionUpdate = wire.clone().try_into().unwrap();
        let ObjectActionUpdate::Success {
            kind: ActionKind::Attack {
                damage: Some(damage),
            },
            ..
        } = decoded.clone()
        else {
            panic!("captured attack line did not decode as an attack: {decoded:?}");
        };
        let hit = damage.entities[0].hits[0];
        assert_eq!(
            hit,
            SkillPartDamage::killing_blow(DamageValue {
                kind: DamageValue::KIND_CRITICAL,
                amount: 284,
            })
        );
        assert!(hit.value().unwrap().is_critical());
        assert!(!hit.value().unwrap().is_unknown_kind());
        let back: Bytes = decoded.into();
        assert_eq!(back, wire);

        // same line with the outcome byte the resolver would call PARRY
        // (never captured): it must survive the round trip as itself, not be
        // rewritten into an ordinary hit
        let mut unknown_kind = wire.to_vec();
        unknown_kind[27] = 0x06;
        let unknown_kind = Bytes::from(unknown_kind);
        let decoded: ObjectActionUpdate = unknown_kind.clone().try_into().unwrap();
        let ObjectActionUpdate::Success {
            kind: ActionKind::Attack {
                damage: Some(damage),
            },
            ..
        } = decoded.clone()
        else {
            panic!("unknown-outcome line did not decode as an attack");
        };
        let value = damage.entities[0].hits[0].value().unwrap();
        assert_eq!(value.kind, 0x06);
        assert!(value.is_unknown_kind());
        assert!(!value.is_critical());
        let back: Bytes = decoded.into();
        assert_eq!(back, unknown_kind);
    }

    /// #547 — the per-hit record is a tagged record whose damage is a `u24`
    /// packed behind a state byte, and whose arm is `flags & 0x7F`, not a bit
    /// test (`FUN_00a55e00`, `docs/re/net/inbound/skill-combat.md` 0xB070).
    #[test]
    fn hit_record_damage_is_a_u24_behind_a_state_byte() {
        // real packet_dump/0xb071.log line: one hit, state 1, damage 0x5F = 95
        // — the value the capture's own annotation records.
        let wire = Bytes::from_static(&[
            0x01, 0xb9, 0x03, 0x00, 0x00, 0x80, 0xab, 0x01, 0x00, 0x01, 0x01, 0x01, 0x80, 0xab,
            0x01, 0x00, 0x00, 0x01, 0x5f, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ]);
        let decoded: SkillEnd = wire.clone().try_into().unwrap();
        let SkillEnd::Success {
            kind: ActionKind::Attack {
                damage: Some(damage),
            },
            ..
        } = decoded.clone()
        else {
            panic!("captured damage line did not decode as an attack: {decoded:?}");
        };
        assert_eq!(
            damage.entities[0].hits[0],
            SkillPartDamage::hit(DamageValue {
                kind: DamageValue::KIND_NORMAL,
                amount: 95,
            })
        );
        let back: Bytes = decoded.into();
        assert_eq!(back, wire);

        // the byte after the packed word belongs to the *unnamed* u32, not to
        // the amount: reading a bare u32 for the amount would report
        // 0xFF00005F here. Same line with that byte set.
        let mut with_trailer = wire.to_vec();
        with_trailer[21] = 0xFF;
        let with_trailer = Bytes::from(with_trailer);
        let decoded: SkillEnd = with_trailer.clone().try_into().unwrap();
        let SkillEnd::Success {
            kind: ActionKind::Attack {
                damage: Some(damage),
            },
            ..
        } = decoded.clone()
        else {
            panic!("trailer variant did not decode as an attack");
        };
        assert_eq!(
            damage.entities[0].hits[0],
            SkillPartDamage {
                killing_blow: false,
                effect: HitEffect::Damage {
                    value: DamageValue {
                        kind: DamageValue::KIND_NORMAL,
                        amount: 95,
                    },
                    unknown: 0xFF,
                },
            }
        );
        let back: Bytes = decoded.into();
        assert_eq!(back, with_trailer);
    }

    /// #547 — arms 4 and 5 are 23-byte records; the old `flag & 0x08` test
    /// read them as 9 and desynchronised everything after them.
    #[test]
    fn hit_record_arms_4_and_5_carry_a_position_tail() {
        // captured 0xb071 line above with its single hit rewritten to arm 4
        // (killing blow) + a position tail, and a second 9-byte arm-0 hit
        // appended: a short read of the first record would swallow the second.
        let mut wire = vec![
            0x01, 0xb9, 0x03, 0x00, 0x00, 0x80, 0xab, 0x01, 0x00, 0x01, 0x02, 0x01, 0x80, 0xab,
            0x01, 0x00,
        ];
        wire.extend_from_slice(&[0x84, 0x01, 0x5f, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        wire.extend_from_slice(&[0x2f, 0x00]); // region
        wire.extend_from_slice(&(-125i32).to_le_bytes());
        wire.extend_from_slice(&300i32.to_le_bytes());
        wire.extend_from_slice(&17i32.to_le_bytes());
        wire.extend_from_slice(&[0x00, 0x02, 0x2c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        let wire = Bytes::from(wire);

        let decoded: SkillEnd = wire.clone().try_into().unwrap();
        let SkillEnd::Success {
            kind: ActionKind::Attack {
                damage: Some(damage),
            },
            ..
        } = decoded.clone()
        else {
            panic!("arm-4 line did not decode as an attack: {decoded:?}");
        };
        assert_eq!(
            damage.entities[0].hits,
            vec![
                SkillPartDamage {
                    killing_blow: true,
                    effect: HitEffect::Displaced {
                        arm: 4,
                        value: DamageValue {
                            kind: DamageValue::KIND_NORMAL,
                            amount: 95,
                        },
                        unknown: 0,
                        pos: HitPosition {
                            region: 0x2f,
                            x: -125,
                            y: 300,
                            z: 17,
                        },
                    },
                },
                SkillPartDamage::hit(DamageValue {
                    kind: DamageValue::KIND_CRITICAL,
                    amount: 44,
                }),
            ]
        );
        let back: Bytes = decoded.into();
        assert_eq!(back, wire);

        // arm 8 (and every other unlisted arm) is the flag byte alone
        let abort = Bytes::from_static(&[
            0x01, 0xb9, 0x03, 0x00, 0x00, 0x80, 0xab, 0x01, 0x00, 0x01, 0x01, 0x01, 0x80, 0xab,
            0x01, 0x00, 0x08,
        ]);
        let decoded: SkillEnd = abort.clone().try_into().unwrap();
        let SkillEnd::Success {
            kind: ActionKind::Attack {
                damage: Some(damage),
            },
            ..
        } = decoded.clone()
        else {
            panic!("arm-8 line did not decode as an attack");
        };
        assert_eq!(
            damage.entities[0].hits[0],
            SkillPartDamage {
                killing_blow: false,
                effect: HitEffect::NoPayload { arm: 8 },
            }
        );
        assert!(damage.entities[0].hits[0].value().is_none());
        assert!(!damage.entities[0].hits[0].is_avoided());
        let back: Bytes = decoded.into();
        assert_eq!(back, abort);
    }

    /// A real captured 0xB070: a monster's two-instance skill on the local
    /// character where instance 1 landed 6 damage and instance 2 was
    /// **avoided** — the arm the client now shows as BLOCK. Taken verbatim
    /// from `packet_dump/0xb070.log`; see [`HIT_ARM_AVOIDED`] for why the arm
    /// is a defender outcome and not a filler.
    #[test]
    fn a_captured_avoided_hit_decodes_as_arm_2() {
        let line = Bytes::from_static(&[
            0x01, 0x02, 0x30, 0xc3, 0x00, 0x00, 0x00, 0xa7, 0x62, 0x02, 0x00, 0xa2, 0x1b, 0x00,
            0x00, 0xc7, 0x40, 0x03, 0x00, 0x01, 0x02, 0x01, 0xc7, 0x40, 0x03, 0x00, 0x00, 0x01,
            0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02,
        ]);
        let decoded: ObjectActionUpdate = line.clone().try_into().unwrap();
        let ObjectActionUpdate::Success {
            kind: ActionKind::Attack {
                damage: Some(damage),
            },
            ..
        } = decoded.clone()
        else {
            panic!("captured line did not decode as an attack");
        };
        assert_eq!(damage.instance_count, 2);
        let hits = &damage.entities[0].hits;
        // instance 1: an ordinary 6-damage hit
        assert_eq!(hits[0].value().unwrap().amount, 6);
        assert!(!hits[0].is_avoided());
        // instance 2: avoided — no damage word on the wire at all
        assert_eq!(hits[1].effect, HitEffect::NoPayload { arm: 2 });
        assert!(hits[1].is_avoided());
        assert!(hits[1].value().is_none());
        let back: Bytes = decoded.into();
        assert_eq!(back, line);
    }

    #[test]
    fn object_action_update_attack_damage_roundtrips() {
        // synthetic (skrillax-shaped) basic-attack swing: two damage
        // instances on one target, a critical hit + a killing blow.
        let packet = ObjectActionUpdate::Success {
            unknown: 0x3002,
            skill_id: 70,
            source: 0x58990,
            instance: 0x629,
            target: 0x9001,
            kind: ActionKind::Attack {
                damage: Some(DamageContent {
                    instance_count: 2,
                    entities: vec![PerEntityDamage {
                        target: 0x9001,
                        hits: vec![
                            SkillPartDamage::hit(DamageValue {
                                kind: DamageValue::KIND_CRITICAL,
                                amount: 123,
                            }),
                            SkillPartDamage::killing_blow(DamageValue {
                                kind: DamageValue::KIND_NORMAL,
                                amount: 45,
                            }),
                        ],
                    }],
                }),
            },
        };
        let bytes: Bytes = packet.clone().into();
        let decoded: ObjectActionUpdate = bytes.try_into().unwrap();
        assert_eq!(decoded, packet);

        // swing without a damage block (whiff) and a failure code
        let whiff = ObjectActionUpdate::Success {
            unknown: 0x3002,
            skill_id: 70,
            source: 1,
            instance: 2,
            target: 3,
            kind: ActionKind::Attack { damage: None },
        };
        let bytes: Bytes = whiff.clone().into();
        let decoded: ObjectActionUpdate = bytes.try_into().unwrap();
        assert_eq!(decoded, whiff);

        // #232: the failure tail is a u16, not a u8. Both captured
        // `packet_dump/0xb070.log` failure lines, which used to land in
        // `Unknown`.
        for (wire, error) in [([2u8, 0x06, 0x30], 0x3006u16), ([2, 0x10, 0x30], 0x3010)] {
            let bytes = Bytes::copy_from_slice(&wire);
            let decoded: ObjectActionUpdate = bytes.clone().try_into().unwrap();
            assert_eq!(decoded, ObjectActionUpdate::Failure { error });
            let back: Bytes = decoded.into();
            assert_eq!(back, bytes);
        }

        // A one-byte tail is now the wrong shape and must stay raw rather than
        // be read as a truncated error.
        let short = Bytes::from_static(&[2, 0x07]);
        let decoded: ObjectActionUpdate = short.try_into().unwrap();
        assert!(matches!(
            decoded,
            ObjectActionUpdate::Unknown { result: 2, .. }
        ));
    }

    /// #232: `0xB074 result=3` is `code u8 + error u16` — the handler reads the
    /// same code byte as results 1/2 and then a `u16` for its message box
    /// (`FUN_00880b60:12-14`). Fixture: the seven identical captured lines in
    /// `packet_dump/0xb074.log` (2026-08-12).
    #[test]
    fn object_action_response_decodes_the_captured_refusal() {
        let wire = Bytes::from_static(&[0x03, 0x00, 0x04, 0x40]);
        let decoded: ObjectActionResponse = wire.clone().try_into().unwrap();
        assert_eq!(
            decoded,
            ObjectActionResponse::Failed {
                code: 0,
                error: 0x4004
            }
        );
        let back: Bytes = decoded.into();
        assert_eq!(back, wire);

        // the two shapes that were already modelled still decode
        let started: ObjectActionResponse = Bytes::from_static(&[1, 1]).try_into().unwrap();
        assert_eq!(started, ObjectActionResponse::Started { code: 1 });
        let ended: ObjectActionResponse = Bytes::from_static(&[2, 0]).try_into().unwrap();
        assert_eq!(ended, ObjectActionResponse::Ended { code: 0 });

        // a result=3 body of the wrong length stays raw
        let short: ObjectActionResponse = Bytes::from_static(&[3, 0, 4]).try_into().unwrap();
        assert!(matches!(
            short,
            ObjectActionResponse::Unknown { result: 3, .. }
        ));
    }

    #[test]
    fn buff_add_decodes_live_capture() {
        // real packet_dump/0xb0bd.log line: join-time auto-buff, skill 39110
        let wire = Bytes::from_static(&[
            0x90, 0x89, 0x05, 0x00, 0xC6, 0x98, 0x00, 0x00, 0x27, 0x06, 0x00, 0x00,
        ]);
        let decoded = BuffAdd::try_from(wire.clone()).unwrap();
        assert_eq!(
            decoded,
            BuffAdd {
                unique_id: 0x058990,
                ref_skill_id: 39110,
                buff_instance_id: 0x627,
            }
        );
        let back: Bytes = decoded.into();
        assert_eq!(back, wire);

        // the matching 0xB072 drops that same instance, as a one-element list
        let wire = Bytes::from_static(&[0x01, 0x27, 0x06, 0x00, 0x00]);
        let decoded = BuffRemove::try_from(wire.clone()).unwrap();
        assert_eq!(decoded.buff_instance_ids, vec![0x627]);
        let back: Bytes = decoded.into();
        assert_eq!(back, wire);
    }

    #[test]
    fn learn_acks_decode_live_captures() {
        // real packet_dump/0xb0a1.log line: skill 3 (SWORD_SMASH_A_01) learned
        let wire = Bytes::from_static(&[0x01, 0x03, 0x00, 0x00, 0x00]);
        let decoded: SkillLearnResponse = wire.clone().try_into().unwrap();
        assert_eq!(decoded, SkillLearnResponse::Success { ref_skill_id: 3 });
        let back: Bytes = decoded.into();
        assert_eq!(back, wire);

        // real packet_dump/0xb0a2.log line: Bicheon (257) raised to level 5
        let wire = Bytes::from_static(&[0x01, 0x01, 0x01, 0x00, 0x00, 0x05]);
        let decoded: MasteryLearnResponse = wire.clone().try_into().unwrap();
        assert_eq!(
            decoded,
            MasteryLearnResponse::Success {
                mastery_id: 257,
                new_level: 5,
            }
        );
        let back: Bytes = decoded.into();
        assert_eq!(back, wire);
    }

    #[test]
    fn learn_acks_keep_unexpected_shapes_raw() {
        // assumed error shape decodes; truncated/overlong bodies stay raw
        let decoded: SkillLearnResponse =
            Bytes::from_static(&[0x02, 0x34, 0x12]).try_into().unwrap();
        assert_eq!(decoded, SkillLearnResponse::Failure(0x1234));
        for wire in [
            Bytes::from_static(&[0x01, 0x03, 0x00]),
            Bytes::from_static(&[0x01, 0x03, 0x00, 0x00, 0x00, 0xFF]),
            Bytes::from_static(&[0x03, 0x01]),
        ] {
            let decoded: SkillLearnResponse = wire.clone().try_into().unwrap();
            assert!(
                matches!(decoded, SkillLearnResponse::Unknown { .. }),
                "{wire:?}"
            );
            let back: Bytes = decoded.into();
            assert_eq!(back, wire);
        }
        let decoded: MasteryLearnResponse = Bytes::from_static(&[0x01, 0x01, 0x01, 0x00, 0x00])
            .try_into()
            .unwrap();
        assert!(matches!(decoded, MasteryLearnResponse::Unknown { .. }));
    }

    #[test]
    fn gm_command_invisible_roundtrips() {
        // 0x7010 body = 2-byte LE sub-command; /invisible = 0x000E. These are
        // the only bytes we have ever actually sent (packet_dump/c2s/0x7010).
        let bytes: Bytes = GmCommand::Invisible.into();
        assert_eq!(&bytes[..], &[0x0E, 0x00]);
        assert_eq!(GmCommand::try_from(bytes).unwrap(), GmCommand::Invisible);

        let bytes: Bytes = GmCommand::Invincible.into();
        assert_eq!(&bytes[..], &[0x0F, 0x00]);

        // unknown sub-command fails to decode (not silently mismatched)
        assert!(GmCommand::try_from(Bytes::from_static(&[0x99, 0x00])).is_err());
    }

    /// Pins the recovered sub-id against the regression that motivated it:
    /// `MakeItem` was 0x06, which is **`LoadMonster`** — so every "make item"
    /// was a monster spawn one byte short of that command's own layout. The
    /// bytes below are the real `/Makeitem ITEM_EU_STAFF_11_SET_A_RARE 255`
    /// (ref 25627, equipment, so 255 is not clamped).
    #[test]
    fn gm_make_item_is_sub_command_seven() {
        let bytes: Bytes = GmCommand::MakeItem {
            ref_id: 25627,
            value: 255,
        }
        .into();
        assert_eq!(&bytes[..], &[0x07, 0x00, 0x1B, 0x64, 0x00, 0x00, 0xFF]);
        assert_eq!(
            GmCommand::try_from(bytes).unwrap(),
            GmCommand::MakeItem {
                ref_id: 25627,
                value: 255,
            }
        );
    }

    /// 0x06 is LoadMonster, and its body is 8 bytes — one more than MakeItem's,
    /// which is exactly why sending MakeItem under 0x06 under-ran it.
    #[test]
    fn gm_load_monster_owns_sub_command_six() {
        let command = GmCommand::LoadMonster {
            ref_id: 1907,
            count: 3,
            rarity: 1,
        };
        let bytes: Bytes = command.clone().into();
        assert_eq!(
            &bytes[..],
            &[0x06, 0x00, 0x73, 0x07, 0x00, 0x00, 0x03, 0x01]
        );
        assert_eq!(bytes.len(), 8);
        assert_eq!(GmCommand::try_from(bytes).unwrap(), command);
    }

    /// `Zoe` and `Zoe2` are two *commands* over one sub-id: `Zoe2` is a
    /// client-side batching wrapper, so on the wire there is nothing to tell
    /// them apart. 34 recovered builders, 33 distinct sub-ids.
    #[test]
    fn gm_zoe_is_sub_command_twelve() {
        let command = GmCommand::Zoe {
            ref_id: 1907,
            count: 200,
        };
        let bytes: Bytes = command.clone().into();
        assert_eq!(&bytes[..], &[0x0C, 0x00, 0x73, 0x07, 0x00, 0x00, 0xC8]);
        assert_eq!(bytes.len(), 7);
        assert_eq!(GmCommand::try_from(bytes).unwrap(), command);
    }

    /// The u16 after `result` is an **echo of the sub-command**, read on both
    /// the ok and fail arms — not an error code.
    #[test]
    fn gm_response_decodes_the_command_echo() {
        // every body in packet_dump/0xb010.log is exactly this: ok, /invisible
        let captured = Bytes::from_static(&[0x01, 0x0E, 0x00]);
        let decoded: GmResponse = captured.clone().try_into().unwrap();
        assert!(decoded.is_success());
        assert_eq!(decoded.gm_command_id, 0x000E);
        assert!(decoded.tail.is_empty());
        assert_eq!(Bytes::from(decoded), captured);

        // a refused /makeitem is the same shape with result 2 — which is what
        // an unprivileged account is expected to answer
        let refused = Bytes::from_static(&[0x02, 0x07, 0x00]);
        let decoded: GmResponse = refused.clone().try_into().unwrap();
        assert!(!decoded.is_success());
        assert_eq!(decoded.result, GM_RESULT_FAIL);
        assert_eq!(decoded.gm_command_id, 0x0007);
        assert_eq!(Bytes::from(decoded), refused);

        // a per-command payload after the echo is kept raw
        let with_tail = Bytes::from_static(&[0x01, 0x01, 0x00, 0x68, 0x69]);
        let decoded: GmResponse = with_tail.clone().try_into().unwrap();
        assert_eq!(decoded.gm_command_id, 0x0001);
        assert_eq!(decoded.tail.len(), 2);
        assert_eq!(Bytes::from(decoded), with_tail);

        // `result` alone is legal: the original reads nothing further for a
        // result it does not branch on
        let bare: GmResponse = Bytes::from_static(&[0x09]).try_into().unwrap();
        assert_eq!(bare.gm_command_id, 0);
    }

    #[test]
    fn entity_state_update_body_invisibility() {
        // kind 4 (body), value 4 = GM invisible
        let inv = EntityStateUpdate {
            unique_id: 1,
            kind: STATE_KIND_BODY,
            value: BODY_STATE_GM_INVISIBLE,
        };
        assert_eq!(inv.body_invisibility(), Some(true));
        // kind 4, value 0 = none (visible)
        let vis = EntityStateUpdate {
            unique_id: 1,
            kind: STATE_KIND_BODY,
            value: BODY_STATE_NONE,
        };
        assert_eq!(vis.body_invisibility(), Some(false));
        // non-body update
        let life = EntityStateUpdate {
            unique_id: 1,
            kind: STATE_KIND_LIFE,
            value: LIFE_STATE_DEAD,
        };
        assert_eq!(life.body_invisibility(), None);
        // ...and the raw value, which is what drawing somebody ELSE needs
        assert_eq!(inv.body_state_value(), Some(BODY_STATE_GM_INVISIBLE));
        assert_eq!(vis.body_state_value(), Some(BODY_STATE_NONE));
        assert_eq!(life.body_state_value(), None);
    }

    /// GM invisibility and stealth are not the same secret, which is why the
    /// render needs the value and not `body_state_is_invisible`'s yes/no: a GM
    /// is meant to see another GM's ghost, and nobody is meant to see a
    /// stealthed player.
    #[test]
    fn only_gm_invisibility_is_a_ghost_and_only_for_a_gm() {
        use HiddenRender::*;
        assert_eq!(hidden_render(BODY_STATE_GM_INVISIBLE, true), Ghost);
        assert_eq!(hidden_render(BODY_STATE_GM_INVISIBLE, false), Hidden);

        // being a GM buys no sight of stealth or player invisibility
        for state in [BODY_STATE_STEALTH, BODY_STATE_INVISIBLE] {
            assert_eq!(hidden_render(state, true), Hidden);
            assert_eq!(hidden_render(state, false), Hidden);
        }

        // everything else draws normally, GM or not — including the two states
        // that are about damage rather than sight
        for state in [
            BODY_STATE_NONE,
            BODY_STATE_UNTOUCHABLE,
            BODY_STATE_GM_INVINCIBLE,
        ] {
            assert_eq!(hidden_render(state, true), Visible);
            assert_eq!(hidden_render(state, false), Visible);
        }
    }

    /// The three "you are hidden" values stay grouped for your OWN body, where
    /// they really do all mean the same thing. Pins that the finer split did
    /// not quietly change the coarse one.
    #[test]
    fn your_own_body_still_treats_all_three_alike() {
        for state in [
            BODY_STATE_GM_INVISIBLE,
            BODY_STATE_STEALTH,
            BODY_STATE_INVISIBLE,
        ] {
            assert!(body_state_is_invisible(state));
        }
        for state in [
            BODY_STATE_NONE,
            BODY_STATE_UNTOUCHABLE,
            BODY_STATE_GM_INVINCIBLE,
        ] {
            assert!(!body_state_is_invisible(state));
        }
    }

    /// 0x3057's flag is a BITMASK. Every body below is a real
    /// `packet_dump/0x3057.log` line. This replaces an earlier test that
    /// asserted the opposite (an enum, per xBot `SRTypes.cs`) — see the
    /// [`EntityBarsUpdate`] doc for why the capture now settles it: `flag=0x04`
    /// appears, and its uid is a monster whose trailing u32 tracks a burn.
    ///
    /// Note flags 3 and 5 are byte-identical in LENGTH under either reading
    /// (two u32s), so length can never distinguish them — only meaning can.
    #[test]
    fn entity_bars_update_flag_is_a_bitmask() {
        // flag=1 HP only: monster 0x1a8b5 regenerating to 350
        let wire =
            Bytes::from_static(&[0xB5, 0xA8, 0x01, 0x00, 0x10, 0x00, 0x01, 0x5E, 0x01, 0, 0]);
        let decoded = EntityBarsUpdate::try_from(wire).unwrap();
        assert_eq!((decoded.hp, decoded.mp), (Some(350), None));
        assert_eq!(decoded.bad_status, None);

        // flag=2 MP only: the local player at 1039 MP
        let wire =
            Bytes::from_static(&[0x80, 0xAB, 0x01, 0x00, 0x10, 0x00, 0x02, 0x0F, 0x04, 0, 0]);
        let decoded = EntityBarsUpdate::try_from(wire).unwrap();
        assert_eq!((decoded.hp, decoded.mp), (None, Some(1039)));

        // flag=3 = HP|MP: both present, MP after HP
        let wire = Bytes::from_static(&[
            0xB5, 0xA8, 0x01, 0x00, 0x10, 0x00, 0x03, 0xCE, 0, 0, 0, 0xEF, 0x02, 0, 0,
        ]);
        let decoded = EntityBarsUpdate::try_from(wire).unwrap();
        assert_eq!((decoded.hp, decoded.mp), (Some(206), Some(751)));

        // flag=5 = HP|BAD_STATUS: the trailing u32 is the ailment mask, NOT MP.
        // A healthy monster's is 0 — which is what every such body in the
        // corpus carries, and what the enum reading had to explain as "MP 0".
        let wire = Bytes::from_static(&[
            0x30, 0x61, 0x01, 0x00, 0x01, 0x00, 0x05, 0xBA, 0, 0, 0, 0, 0, 0, 0,
        ]);
        let decoded = EntityBarsUpdate::try_from(wire).unwrap();
        assert_eq!(decoded.unique_id, 0x16130);
        assert_eq!((decoded.hp, decoded.mp), (Some(186), None));
        assert_eq!(decoded.bad_status(), Some(BadStatus(0)));
        assert!(decoded.bad_status().unwrap().is_empty());
    }

    /// The body that settles it: `packet_dump/0x3057.log` line 447, a monster
    /// catching fire. 11 bytes, `flag=0x04`, mask `0x8` = Burn, and — because
    /// bit 3 is NOT in `BAD_STATUS_LEVELED` — no trailing level byte, which is
    /// what makes the RE-derived level rule capture-confirmed.
    ///
    /// Under the old enum reading this monster reported **MP = 8**.
    #[test]
    fn a_burning_monster_decodes_as_burn_not_as_mp() {
        let wire = Bytes::from_static(&[0xB1, 0x64, 0x02, 0x00, 0x03, 0x01, 0x04, 0x08, 0, 0, 0]);
        let decoded = EntityBarsUpdate::try_from(wire.clone()).unwrap();
        assert_eq!(decoded.unique_id, 0x000264B1);
        assert_eq!(decoded.hp, None);
        assert_eq!(decoded.mp, None, "the mask must not be read as MP");
        let status = decoded.bad_status().expect("bad-status block");
        assert!(status.has(Ailment::Burn));
        assert_eq!(status.ailments().collect::<Vec<_>>(), vec![Ailment::Burn]);
        assert!(
            decoded.bad_status_levels.is_empty(),
            "Burn carries no level byte"
        );
        // and it round-trips byte-for-byte
        let back: Bytes = decoded.into();
        assert_eq!(back, wire);
    }

    /// A level-carrying bit pulls one `u8` per set `BAD_STATUS_LEVELED` bit.
    /// Synthetic — no capture has such a bit set yet, so this pins the
    /// structure the RE describes, not an observed body.
    #[test]
    fn leveled_bad_status_bits_pull_one_level_byte_each() {
        // Stun (bit 14) and Bleed (bit 11) are both in the leveled mask; Burn
        // (bit 3) is not, so a mask of all three yields exactly two levels.
        let mask = Ailment::Stun.bit() | Ailment::Bleed.bit() | Ailment::Burn.bit();
        assert_eq!((mask & BAD_STATUS_LEVELED).count_ones(), 2);
        let mut body = vec![0xB1, 0x64, 0x02, 0x00, 0x03, 0x01, 0x04];
        body.extend_from_slice(&mask.to_le_bytes());
        body.extend_from_slice(&[7, 9]);
        let wire = Bytes::from(body);
        let decoded = EntityBarsUpdate::try_from(wire.clone()).unwrap();
        assert_eq!(decoded.bad_status_levels, vec![7, 9]);
        assert_eq!(
            decoded.bad_status().unwrap().ailments().collect::<Vec<_>>(),
            vec![Ailment::Burn, Ailment::Bleed, Ailment::Stun],
            "ailments come back in bit order"
        );
        let back: Bytes = decoded.into();
        assert_eq!(back, wire);
    }

    /// The level-less bits are exactly the six elemental/DoT states plus
    /// Petrify. That partition is what corroborates the SPEC bit ORDER, so if
    /// either the mask constant or the enum order drifts, this catches it.
    #[test]
    fn the_leveled_mask_matches_the_spec_bit_order() {
        let level_less: Vec<Ailment> = Ailment::ALL
            .into_iter()
            .filter(|a| a.bit() & BAD_STATUS_LEVELED == 0)
            .collect();
        assert_eq!(
            level_less,
            vec![
                Ailment::Freezing,
                Ailment::Frostbite,
                Ailment::ElectricShock,
                Ailment::Burn,
                Ailment::Poison,
                Ailment::Zombie,
                Ailment::Petrify,
            ]
        );
    }

    #[test]
    fn motion_updates_name_the_gait() {
        // Verbatim `packet_dump/0x30bf.log` lines: uid 0x1ab9c starts walking,
        // uid 0x16130 starts running (kind 1, values 2 and 3).
        let walk = Bytes::from_static(&[0x9c, 0xab, 0x01, 0x00, 0x01, 0x02]);
        let decoded: EntityStateUpdate = walk.try_into().unwrap();
        assert_eq!(decoded.unique_id, 0x1ab9c);
        assert_eq!(decoded.motion_walking(), Some(true));

        let run = Bytes::from_static(&[0x30, 0x61, 0x01, 0x00, 0x01, 0x03]);
        let decoded: EntityStateUpdate = run.try_into().unwrap();
        assert_eq!(decoded.motion_walking(), Some(false));

        // A life-state update is not a gait, and an unobserved motion value
        // stays unanswered rather than being guessed into a gait.
        let dead = Bytes::from_static(&[0x30, 0x61, 0x01, 0x00, 0x00, 0x02]);
        let decoded: EntityStateUpdate = dead.try_into().unwrap();
        assert_eq!(decoded.motion_walking(), None);
        let unknown = Bytes::from_static(&[0x30, 0x61, 0x01, 0x00, 0x01, 0x09]);
        let decoded: EntityStateUpdate = unknown.try_into().unwrap();
        assert_eq!(decoded.motion_walking(), None);
    }

    #[test]
    fn entity_state_update_decodes_live_captures() {
        // live 0x30bf lines: monster 0x13656 running, then dying
        let run = Bytes::from_static(&[0x56, 0x36, 0x01, 0x00, 0x01, 0x03]);
        let decoded: EntityStateUpdate = run.try_into().unwrap();
        assert_eq!(decoded.unique_id, 0x13656);
        assert_eq!(decoded.kind, STATE_KIND_MOTION);
        assert!(!decoded.is_death());

        let dead = Bytes::from_static(&[0x56, 0x36, 0x01, 0x00, 0x00, 0x02]);
        let decoded: EntityStateUpdate = dead.try_into().unwrap();
        assert!(decoded.is_death());
        assert!(!decoded.is_revive());

        // life → alive (revive): kind 0, value 1
        let alive = Bytes::from_static(&[0x56, 0x36, 0x01, 0x00, 0x00, 0x01]);
        let decoded: EntityStateUpdate = alive.try_into().unwrap();
        assert!(decoded.is_revive());
        assert!(!decoded.is_death());

        // live 0x3054 line: bare uid of the leveling entity
        let levelup = Bytes::from_static(&[0x56, 0xAF, 0x05, 0x00]);
        let decoded: EntityLevelUp = levelup.try_into().unwrap();
        assert_eq!(decoded.unique_id, 0x5AF56);
    }

    /// 0xB024 is a 6-byte body: u32 uid then u16 angle, exactly what the
    /// original's parser reads before it stops.
    ///
    /// No capture exists (`packet_dump/0xb024.log` is absent), so this fixture
    /// is built from that parser layout rather than from live bytes — the
    /// widths and their order are what it pins.
    #[test]
    fn movement_angle_decodes_the_parser_layout() {
        let body = Bytes::from_static(&[0x56, 0xAF, 0x05, 0x00, 0x00, 0x40]);
        let decoded: MovementAngleResponse = body.try_into().unwrap();
        assert_eq!(decoded.unique_id, 0x5AF56);
        // 0x4000 = a quarter turn in the 0..=u16::MAX -> 0..2pi encoding
        assert_eq!(decoded.angle, 0x4000);
    }

    /// The uid must not swallow the angle's low byte: a 6-byte body split
    /// 4+2, not 2+4 or 5+1.
    #[test]
    fn movement_angle_field_widths_do_not_overlap() {
        let body = Bytes::from_static(&[0xFF, 0xFF, 0xFF, 0xFF, 0x34, 0x12]);
        let decoded: MovementAngleResponse = body.try_into().unwrap();
        assert_eq!(decoded.unique_id, u32::MAX);
        assert_eq!(decoded.angle, 0x1234);
    }

    /// 0x3080 S->C: `{type, uid}`, plus a party-only `setup` byte. The
    /// discriminator is `SRTypes.PlayerPetition`.
    #[test]
    fn game_invite_decodes_a_petition() {
        // exchange (1), uid 0x0001_60AA, no setup byte
        let body = Bytes::from_static(&[0x01, 0xAA, 0x60, 0x01, 0x00]);
        let GameInvite::Petition(p) = GameInvite::try_from(body).unwrap() else {
            panic!("decoding always yields a petition")
        };
        assert_eq!(p.petition, PETITION_EXCHANGE);
        assert_eq!(p.unique_id, 0x0001_60AA);
        assert_eq!(p.setup, None, "only party petitions carry setup");
        assert!(!p.is_party());
    }

    /// The party arms carry the extra `setup` byte; capacity is 8 when
    /// EXP_SHARED is set, else 4.
    #[test]
    fn a_party_petition_carries_its_setup_flags() {
        let body = Bytes::from_static(&[0x03, 0x01, 0x00, 0x00, 0x00, 0x05]);
        let GameInvite::Petition(p) = GameInvite::try_from(body).unwrap() else {
            panic!("petition")
        };
        assert_eq!(p.petition, PETITION_PARTY_INVITATION);
        assert!(p.is_party());
        let setup = p.setup.expect("party petitions carry setup");
        assert_eq!(setup & PARTY_SETUP_EXP_SHARED, PARTY_SETUP_EXP_SHARED);
        assert_eq!(
            setup & PARTY_SETUP_ANYONE_CAN_INVITE,
            PARTY_SETUP_ANYONE_CAN_INVITE
        );
        assert_eq!(setup & PARTY_SETUP_ITEM_SHARED, 0);
    }

    /// A truncated party tail must not fail the whole decode - the petition
    /// type and uid are still usable.
    #[test]
    fn a_party_petition_without_its_setup_byte_still_decodes() {
        let body = Bytes::from_static(&[0x02, 0x01, 0x00, 0x00, 0x00]);
        let GameInvite::Petition(p) = GameInvite::try_from(body).unwrap() else {
            panic!("petition")
        };
        assert_eq!(p.petition, PETITION_PARTY_CREATION);
        assert_eq!(p.setup, None);
    }

    /// C->S: the answer carries no echoed id or type; the server correlates by
    /// session. Party decline has its own three-byte form.
    #[test]
    fn invite_responses_serialize_to_the_original_bytes() {
        let accept: Bytes = GameInvite::Response(InviteResponse::Accept).into();
        assert_eq!(&accept[..], &[0x01, 0x01]);
        let decline: Bytes = GameInvite::Response(InviteResponse::Decline).into();
        assert_eq!(&decline[..], &[0x01, 0x00]);
        let party: Bytes = GameInvite::Response(InviteResponse::DeclineParty).into();
        assert_eq!(&party[..], &[0x02, 0x0C, 0x2C]);
    }

    /// The four funnel requests are a bare target uid.
    #[test]
    fn the_invite_funnels_are_a_bare_uid() {
        let uid = 0x0001_60AA;
        let party: Bytes = PartyInviteRequest { unique_id: uid }.into();
        assert_eq!(&party[..], &[0xAA, 0x60, 0x01, 0x00]);
        let exchange: Bytes = ExchangeInviteRequest { unique_id: uid }.into();
        assert_eq!(&exchange[..], &party[..]);
        let guild: Bytes = GuildInviteRequest { unique_id: uid }.into();
        assert_eq!(&guild[..], &party[..]);
        let academy: Bytes = AcademyInviteRequest { unique_id: uid }.into();
        assert_eq!(&academy[..], &party[..]);
    }

    /// 0xB081 inviter-side ack: the result byte selects the tail, so the
    /// refused case is three bytes and must decode as such — reading it as the
    /// success layout is how a refused invite used to look like a broken
    /// packet.
    #[test]
    fn the_exchange_ack_decodes_both_tails() {
        let raised = Bytes::from_static(&[0x01, 0xAA, 0x60, 0x01, 0x00]);
        let ack: ExchangeInviteResponse = raised.clone().try_into().unwrap();
        assert_eq!(
            ack,
            ExchangeInviteResponse::Accepted {
                unique_id: 0x0001_60AA
            }
        );
        assert_eq!(Bytes::from(ack), raised);

        let refused = Bytes::from_static(&[0x02, 0x0C, 0x2C]);
        let ack: ExchangeInviteResponse = refused.clone().try_into().unwrap();
        assert_eq!(ack, ExchangeInviteResponse::Refused { error: 0x2C0C });
        assert_eq!(Bytes::from(ack), refused);

        // a body that stops inside its tail is an error, not a silent zero
        assert!(ExchangeInviteResponse::try_from(Bytes::from_static(&[0x01, 0x00])).is_err());
        assert!(ExchangeInviteResponse::try_from(Bytes::new()).is_err());
    }

    /// Real `packet_dump/0x3011.log` lines. All three captured samples are the
    /// same single byte `04`, and the original's parser reads exactly one byte.
    #[test]
    fn character_died_decodes_live_capture() {
        let body = Bytes::from_static(&[0x04]);
        let decoded: CharacterDied = body.try_into().unwrap();
        assert_eq!(decoded.death_cause, 0x04);
        // the byte is passed through, not interpreted - its value space is
        // UNKNOWN, so any other cause must survive the round trip too
        for cause in [0x00u8, 0x01, 0x7F, 0xFF] {
            let raw = Bytes::copy_from_slice(&[cause]);
            let decoded: CharacterDied = raw.try_into().unwrap();
            assert_eq!(decoded.death_cause, cause);
        }
    }

    /// The single real `packet_dump/0x304d.log` line: `aa600100`.
    #[test]
    fn drop_unlocked_decodes_live_capture() {
        let body = Bytes::from_static(&[0xAA, 0x60, 0x01, 0x00]);
        let decoded: DropUnlocked = body.try_into().unwrap();
        assert_eq!(decoded.unique_id, 0x0001_60AA);
        assert_eq!(decoded.unique_id, 90282);
    }

    /// Only one 0x304D sample exists, so a longer real body is possible. The
    /// decode must ignore a tail rather than fail, leaving the unique id
    /// usable (docs/net-entity-events-0x3011.md marks this [S], not [V]).
    #[test]
    fn drop_unlocked_tolerates_an_unknown_tail() {
        let body = Bytes::from_static(&[0xAA, 0x60, 0x01, 0x00, 0xDE, 0xAD]);
        let decoded: DropUnlocked = body.try_into().unwrap();
        assert_eq!(decoded.unique_id, 0x0001_60AA);
    }

    #[test]
    fn receive_experience_decodes_live_capture() {
        // real packet_dump/0x3056.log line: kill grants 23 exp, 119 sp-exp
        let wire = Bytes::from_static(&[
            0xA8, 0x47, 0x01, 0x00, 0x17, 0, 0, 0, 0, 0, 0, 0, 0x77, 0, 0, 0, 0, 0, 0, 0, 0,
        ]);
        let decoded: ReceiveExperience = wire.clone().try_into().unwrap();
        assert_eq!(decoded.exp_origin, 0x147A8);
        assert_eq!(decoded.experience, 23);
        assert_eq!(decoded.sp_exp, 119);
        assert_eq!(decoded.stat_points(), None);
        let back: Bytes = decoded.into();
        assert_eq!(back, wire);

        // level-up: trailing u16 is the total stat points (here 12 = level 5)
        let wire = Bytes::from_static(&[
            1, 0, 0, 0, 9, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 12, 0,
        ]);
        let decoded: ReceiveExperience = wire.try_into().unwrap();
        assert_eq!(decoded.stat_points(), Some(12));
    }

    #[test]
    fn receive_experience_death_penalty_is_negative() {
        // real packet_dump/0x3056.log death line (09:45:51.937Z): the EXP
        // penalty arrives as a negative i64 on the player's own uid. Read as
        // u64 this is 18446744073709548381, which saturated the underbar's
        // exp offset and walked the level to the leveldata maximum (#306).
        let wire = Bytes::from_static(&[
            0x80, 0xAB, 0x01, 0x00, 0x5D, 0xF3, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0, 0,
            0, 0, 0, 0,
        ]);
        let decoded: ReceiveExperience = wire.clone().try_into().unwrap();
        assert_eq!(decoded.exp_origin, 0x1AB80);
        assert_eq!(decoded.experience, -3235);
        assert_eq!(decoded.sp_exp, 0);
        assert_eq!(decoded.stat_points(), None);
        let back: Bytes = decoded.into();
        assert_eq!(back, wire);
    }

    #[test]
    fn object_action_update_unknown_shapes_keep_raw_tail() {
        for wire in [
            // truncated success body
            Bytes::from_static(&[1, 0, 0x30, 5]),
            // success with trailing garbage after kind=none
            Bytes::from_static(&[
                1, 0, 0x30, 1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, 4, 0, 0, 0, 0, 0xAA,
            ]),
            // unknown result byte
            Bytes::from_static(&[9, 1, 2, 3]),
        ] {
            let decoded: ObjectActionUpdate = wire.clone().try_into().unwrap();
            assert!(
                matches!(decoded, ObjectActionUpdate::Unknown { .. }),
                "{wire:?}"
            );
            let back: Bytes = decoded.into();
            assert_eq!(back, wire);
        }
    }

    #[test]
    fn character_data_is_raw_passthrough() {
        let body = Bytes::from_static(&[1, 2, 3, 4, 5]);
        let packet: CharacterDataBody = body.clone().try_into().unwrap();
        assert_eq!(packet.raw, body);
        let back: Bytes = packet.into();
        assert_eq!(back, body);
    }

    #[test]
    fn empty_packets_serialize_empty() {
        let bytes: Bytes = GameReady.into();
        assert!(bytes.is_empty());
        let _decoded: GameReady = Bytes::new().try_into().unwrap();
    }

    #[test]
    fn movement_request_overworld_uses_shorts() {
        let req = MovementRequest {
            region: 0x60A8,
            x: 10580,
            y: -77,
            z: 14260,
        };
        let bytes: Bytes = req.clone().into();
        // kind(1) + region(u16) + 3× i16
        assert_eq!(bytes.len(), 1 + 2 + 6);
        assert_eq!(bytes[0], 1);
        let decoded: MovementRequest = bytes.try_into().unwrap();
        assert_eq!(decoded, req);
    }

    #[test]
    fn movement_request_dungeon_uses_ints() {
        let req = MovementRequest {
            region: 0x8001,
            x: 123456,
            y: -7,
            z: 654321,
        };
        let bytes: Bytes = req.clone().into();
        // kind(1) + region(u16) + 3× i32
        assert_eq!(bytes.len(), 1 + 2 + 12);
        let decoded: MovementRequest = bytes.try_into().unwrap();
        assert_eq!(decoded, req);
    }

    #[test]
    fn movement_response_destination_roundtrips() {
        let resp = MovementResponse {
            unique_id: 352808,
            has_destination: true,
            region: 0x60A8,
            x: 10580,
            y: -77,
            z: 14260,
            angle: 0,
        };
        let bytes: Bytes = resp.clone().into();
        let decoded: MovementResponse = bytes.try_into().unwrap();
        assert_eq!(decoded, resp);
    }

    #[test]
    fn movement_response_short_body_fails_safe() {
        for len in 0..4 {
            let bytes = Bytes::copy_from_slice(&[0u8; 4][..len]);
            assert!(MovementResponse::try_from(bytes).is_err());
        }
    }

    #[test]
    fn logout_response_success_carries_countdown_and_mode() {
        let ok = LogoutResponse {
            result: 1,
            countdown: Some(5),
            mode: Some(LOGOUT_MODE_RESTART),
            error: None,
        };
        let bytes: Bytes = ok.clone().into();
        // result + countdown + mode, no error, no presence flags
        assert_eq!(&bytes[..], &[1, 5, LOGOUT_MODE_RESTART]);
        let decoded: LogoutResponse = bytes.try_into().unwrap();
        assert_eq!(decoded.countdown, Some(5));
        assert_eq!(decoded.mode, Some(LOGOUT_MODE_RESTART));
        assert_eq!(decoded.error, None);
    }

    #[test]
    fn movement_position_update_roundtrips() {
        let p = MovementPositionUpdate {
            unique_id: 352808,
            region: 0x60A8,
            x: 1058.0,
            y: -7.68,
            z: 1426.0,
            heading: 12268,
        };
        let bytes: Bytes = p.clone().into();
        // u32 + u16 + 3×f32 + u16
        assert_eq!(bytes.len(), 4 + 2 + 12 + 2);
        let decoded: MovementPositionUpdate = bytes.try_into().unwrap();
        assert_eq!(decoded, p);
    }

    #[test]
    fn entity_speed_update_roundtrips() {
        let p = EntitySpeedUpdate {
            unique_id: 352808,
            walk_speed: 16.0,
            run_speed: 50.0,
        };
        let bytes: Bytes = p.clone().into();
        // u32 + 2×f32
        assert_eq!(bytes.len(), 4 + 8);
        let decoded: EntitySpeedUpdate = bytes.try_into().unwrap();
        assert_eq!(decoded, p);
    }

    #[test]
    fn group_spawn_begin_roundtrips() {
        let begin = GroupEntitySpawnBegin {
            kind: GROUP_SPAWN,
            count: 3,
        };
        let bytes: Bytes = begin.clone().into();
        // kind(u8) + count(u16 LE)
        assert_eq!(&bytes[..], &[GROUP_SPAWN, 0x03, 0x00]);
        let decoded: GroupEntitySpawnBegin = bytes.try_into().unwrap();
        assert_eq!(decoded.kind, GROUP_SPAWN);
        assert_eq!(decoded.count, 3);
    }

    #[test]
    fn group_spawn_begin_ignores_trailing_bytes() {
        // Some servers append "unknown" fields after count; decode must not fail.
        let body = Bytes::from_static(&[GROUP_DESPAWN, 0x02, 0x00, 0xAA, 0xBB, 0xCC]);
        let decoded: GroupEntitySpawnBegin = body.try_into().unwrap();
        assert_eq!(decoded.kind, GROUP_DESPAWN);
        assert_eq!(decoded.count, 2);
    }

    #[test]
    fn group_spawn_data_is_raw_passthrough() {
        let body = Bytes::from_static(&[9, 8, 7, 6, 5, 4]);
        let packet: GroupEntitySpawnData = body.clone().try_into().unwrap();
        assert_eq!(packet.raw, body);
        let back: Bytes = packet.into();
        assert_eq!(back, body);
    }

    #[test]
    fn single_spawn_is_raw_passthrough() {
        let body = Bytes::from_static(&[1, 2, 3, 4, 5, 6, 7]);
        let packet: SingleEntitySpawn = body.clone().try_into().unwrap();
        assert_eq!(packet.raw, body);
        let back: Bytes = packet.into();
        assert_eq!(back, body);
    }

    #[test]
    fn single_despawn_roundtrips() {
        let p = SingleEntityDespawn { unique_id: 352808 };
        let bytes: Bytes = p.clone().into();
        assert_eq!(&bytes[..], &[0x28, 0x62, 0x05, 0x00]);
        let decoded: SingleEntityDespawn = bytes.try_into().unwrap();
        assert_eq!(decoded, p);
    }

    #[test]
    fn group_spawn_end_serializes_empty() {
        let bytes: Bytes = GroupEntitySpawnEnd.into();
        assert!(bytes.is_empty());
        let _decoded: GroupEntitySpawnEnd = Bytes::new().try_into().unwrap();
    }

    #[test]
    fn entity_bars_update_hp_only() {
        let p = EntityBarsUpdate {
            unique_id: 352808,
            source: BARS_SOURCE_DAMAGE,
            flag: BARS_FLAG_HP,
            hp: Some(231),
            mp: None,
            unknown16: None,
            bad_status: None,
            bad_status_levels: Vec::new(),
        };
        let bytes: Bytes = p.clone().into();
        // u32 + u16 + u8 + u32, no MP field on the wire
        assert_eq!(
            &bytes[..],
            &[0x28, 0x62, 0x05, 0x00, 0x01, 0x00, 0x01, 231, 0x00, 0x00, 0x00]
        );
        let decoded: EntityBarsUpdate = bytes.try_into().unwrap();
        assert_eq!(decoded, p);
    }

    #[test]
    fn entity_bars_update_both_orders_hp_before_mp() {
        let p = EntityBarsUpdate {
            unique_id: 1,
            source: BARS_SOURCE_REGEN,
            flag: BARS_FLAG_HP | BARS_FLAG_MP,
            hp: Some(0x11223344),
            mp: Some(0x55667788),
            unknown16: None,
            bad_status: None,
            bad_status_levels: Vec::new(),
        };
        let bytes: Bytes = p.clone().into();
        assert_eq!(bytes.len(), 4 + 2 + 1 + 4 + 4);
        assert_eq!(&bytes[7..11], &[0x44, 0x33, 0x22, 0x11]); // HP first
        let decoded: EntityBarsUpdate = bytes.try_into().unwrap();
        assert_eq!(decoded, p);
    }

    /// The `0x08` block is read BEFORE the `0x04` one — the binary's order,
    /// not bit order. A body carrying both pins that, since swapping them
    /// would still consume the same byte count and silently mis-slice.
    #[test]
    fn entity_bars_update_reads_the_unknown16_block_before_bad_status() {
        let p = EntityBarsUpdate {
            unique_id: 1,
            source: BARS_SOURCE_DAMAGE,
            flag: BARS_FLAG_HP | BARS_FLAG_BAD_STATUS | BARS_FLAG_UNKNOWN16,
            hp: Some(0x11223344),
            mp: None,
            unknown16: Some(0xBEEF),
            bad_status: Some(Ailment::Burn.bit()),
            bad_status_levels: Vec::new(),
        };
        let bytes: Bytes = p.clone().into();
        assert_eq!(&bytes[11..13], &[0xEF, 0xBE], "the u16 comes first");
        let decoded: EntityBarsUpdate = bytes.try_into().unwrap();
        assert_eq!(decoded, p);
    }

    /// A truncated level tail must not lose the mask that names the ailments:
    /// the level rule is RE-derived and only its no-level case is captured, so
    /// a short body yields fewer levels rather than failing the whole decode.
    #[test]
    fn a_short_level_tail_keeps_the_mask() {
        let mask = Ailment::Stun.bit() | Ailment::Bleed.bit();
        let mut body = vec![0x01, 0x00, 0x00, 0x00, 0x01, 0x00, BARS_FLAG_BAD_STATUS];
        body.extend_from_slice(&mask.to_le_bytes());
        body.push(3); // only one of the two expected level bytes
        let decoded: EntityBarsUpdate = Bytes::from(body).try_into().unwrap();
        assert_eq!(decoded.bad_status(), Some(BadStatus(mask)));
        assert_eq!(decoded.bad_status_levels, vec![3]);
    }

    #[test]
    fn character_points_berserk_roundtrips() {
        let p = CharacterPointsUpdate::Berserk {
            amount: 5,
            source: 352808,
        };
        let bytes: Bytes = p.clone().into();
        // discriminator u8 + amount u8 + source u32
        assert_eq!(&bytes[..], &[4, 5, 0x28, 0x62, 0x05, 0x00]);
        let decoded: CharacterPointsUpdate = bytes.try_into().unwrap();
        assert_eq!(decoded, p);
    }

    #[test]
    fn character_points_gold_roundtrips() {
        let p = CharacterPointsUpdate::Gold {
            amount: 123_456_789,
            display: 1,
        };
        let bytes: Bytes = p.clone().into();
        assert_eq!(bytes.len(), 1 + 8 + 1);
        let decoded: CharacterPointsUpdate = bytes.try_into().unwrap();
        assert_eq!(decoded, p);
    }

    #[test]
    fn character_stats_update_is_36_bytes() {
        let p = CharacterStatsUpdate {
            phys_attack_min: 10,
            phys_attack_max: 14,
            mag_attack_min: 20,
            mag_attack_max: 26,
            phys_defense: 8,
            mag_defense: 9,
            hit_rate: 25,
            parry_rate: 17,
            max_hp: 244,
            max_mp: 244,
            strength: 21,
            intelligence: 22,
        };
        let bytes: Bytes = p.clone().into();
        assert_eq!(bytes.len(), 36);
        let decoded: CharacterStatsUpdate = bytes.try_into().unwrap();
        assert_eq!(decoded.max_hp, 244);
        assert_eq!(decoded.max_mp, 244);
        assert_eq!(decoded, p);
    }

    #[test]
    fn logout_response_error_carries_code() {
        let err = LogoutResponse {
            result: 2,
            countdown: None,
            mode: None,
            error: Some(LOGOUT_ERROR_IN_BATTLE),
        };
        let bytes: Bytes = err.into();
        assert_eq!(&bytes[..], &[2, 0x01, 0x08]); // result + u16 LE
        let decoded: LogoutResponse = bytes.try_into().unwrap();
        assert_eq!(decoded.error, Some(LOGOUT_ERROR_IN_BATTLE));
        assert_eq!(decoded.countdown, None);
    }

    // --- Captured world-join server pushes (live vSRO 1.188, PR #179) --------

    #[test]
    fn silk_update_decodes_captured_body() {
        // packet_dump/0x3153.log: F4 CB 9A 3B 50 C3 00 00 00 00 00 00
        let body = Bytes::from_static(&[
            0xF4, 0xCB, 0x9A, 0x3B, // own = 1_000_000_500
            0x50, 0xC3, 0x00, 0x00, // gift = 50_000
            0x00, 0x00, 0x00, 0x00, // point = 0
        ]);
        let decoded: SilkUpdate = body.clone().try_into().unwrap();
        assert_eq!(
            decoded,
            SilkUpdate {
                own: 1_000_000_500,
                gift: 50_000,
                point: 0,
            }
        );
        let reencoded: Bytes = decoded.into();
        assert_eq!(reencoded, body);
    }

    #[test]
    fn weather_update_decodes_captured_body() {
        // packet_dump/0x3809.log: 01 B4
        let body = Bytes::from_static(&[0x01, 0xB4]);
        let decoded: WeatherUpdate = body.clone().try_into().unwrap();
        assert_eq!(
            decoded,
            WeatherUpdate {
                weather_type: 1,
                intensity: 180,
            }
        );
        let reencoded: Bytes = decoded.into();
        assert_eq!(reencoded, body);
    }

    #[test]
    fn friend_list_info_decodes_empty_roster() {
        // packet_dump/0x3305.log: 00 (empty roster)
        let body = Bytes::from_static(&[0x00]);
        let decoded: FriendListInfo = body.clone().try_into().unwrap();
        assert_eq!(decoded.count, 0);
        assert!(decoded.friends.is_empty());
        let reencoded: Bytes = decoded.into();
        assert_eq!(reencoded, body);
    }

    /// The record the original's parser reads is `u32, u16 len + ASCII, u32,
    /// u8` — four fields (`FUN_009993b0`, `docs/re/net/inbound/chat-social.md`
    /// §0x3305). We modelled a fifth (`group_id: u16`) that is not on the
    /// wire, so from the *second* entry onwards everything shifted by two
    /// bytes (#546). Two entries is the smallest roster that shows it, which
    /// is why the empty-roster test above never could.
    ///
    /// Synthetic bytes: the only live capture of 0x3305 is an empty roster.
    #[test]
    fn friend_list_info_decodes_two_entries_without_drift() {
        let body = Bytes::from_static(&[
            0x02, // count
            // entry 1: id 0x00000101, "ab", model 7, online 1
            0x01, 0x01, 0x00, 0x00, //
            0x02, 0x00, b'a', b'b', //
            0x07, 0x00, 0x00, 0x00, //
            0x01, //
            // entry 2: id 0x00000202, "xyz", model 8, offline
            0x02, 0x02, 0x00, 0x00, //
            0x03, 0x00, b'x', b'y', b'z', //
            0x08, 0x00, 0x00, 0x00, //
            0x00,
        ]);
        let decoded: FriendListInfo = body.clone().try_into().unwrap();
        assert_eq!(decoded.count, 2);
        assert_eq!(
            decoded.friends,
            vec![
                FriendEntry {
                    char_id: 0x0101,
                    name: "ab".to_string(),
                    char_model: 7,
                    is_online: 1,
                },
                FriendEntry {
                    char_id: 0x0202,
                    name: "xyz".to_string(),
                    char_model: 8,
                    is_online: 0,
                },
            ]
        );
        // The record has no trailing padding: 1 + 2 * (11 + name_len).
        let reencoded: Bytes = decoded.into();
        assert_eq!(reencoded, body);
        assert_eq!(body.len(), 1 + (11 + 2) + (11 + 3));
    }

    #[test]
    fn character_finished_decodes_two_empty_lists() {
        // packet_dump/0x3077.log: 00 00 (no item + no skill cooldowns)
        let body = Bytes::from_static(&[0x00, 0x00]);
        let decoded: CharacterFinished = body.clone().try_into().unwrap();
        assert_eq!(decoded.item_cooldown_count, 0);
        assert_eq!(decoded.skill_cooldown_count, 0);
        assert!(decoded.item_cooldowns.is_empty());
        assert!(decoded.skill_cooldowns.is_empty());
        let reencoded: Bytes = decoded.into();
        assert_eq!(reencoded, body);
    }

    // --- Server notice push (0x300C) — real packet_dump bytes (#267) ---------

    /// packet_dump/0x300c.log line 2: `05 0c 43 95 00 00` — code 0x0C05, ref id
    /// 0x9543 = 38211. Six bytes is what pins the discriminator as a u16: a
    /// second u8 field would have to be part of it.
    #[test]
    fn notice_update_decodes_a_captured_unique_spawn() {
        let body = Bytes::from_static(&[0x05, 0x0c, 0x43, 0x95, 0x00, 0x00]);

        let decoded = NoticeUpdate::try_from(body.clone()).unwrap();

        assert_eq!(decoded, NoticeUpdate::UniqueAppeared { ref_id: 38211 });
        assert_eq!(decoded.code(), NOTICE_UNIQUE_APPEARED);
        let back: Bytes = decoded.into();
        assert_eq!(back, body);
    }

    /// The very next captured line differs only in the ref id (38212), which is
    /// what pins the field as a little-endian u32 rather than a wider/narrower one.
    #[test]
    fn notice_update_reads_the_second_captured_spawn_ref_id() {
        let body = Bytes::from_static(&[0x05, 0x0c, 0x44, 0x95, 0x00, 0x00]);

        assert_eq!(
            NoticeUpdate::try_from(body).unwrap(),
            NoticeUpdate::UniqueAppeared { ref_id: 38212 }
        );
    }

    /// packet_dump/0x300c.log line 1: `18 0c 02 03` — code 0x0C18 with the two
    /// bytes the capture pins, and no tail (both are >= 2, so the conditional
    /// 8-byte run is absent).
    #[test]
    fn notice_update_decodes_the_captured_0c18_code() {
        let body = Bytes::from_static(&[0x18, 0x0c, 0x02, 0x03]);

        let decoded = NoticeUpdate::try_from(body.clone()).unwrap();

        assert_eq!(
            decoded,
            NoticeUpdate::Code0C18 {
                a: 2,
                b: 3,
                tail: Bytes::new(),
            }
        );
        let back: Bytes = decoded.into();
        assert_eq!(back, body);
    }

    /// A code no source decodes keeps its whole body — including the code — so
    /// nothing is invented and nothing is lost.
    #[test]
    fn an_unrecorded_notice_code_is_kept_raw() {
        let body = Bytes::from_static(&[0x16, 0x0c, 0xAA, 0xBB]);

        let decoded = NoticeUpdate::try_from(body.clone()).unwrap();

        assert_eq!(
            decoded,
            NoticeUpdate::Raw {
                code: 0x0C16,
                tail: Bytes::from_static(&[0xAA, 0xBB]),
            }
        );
        let back: Bytes = decoded.into();
        assert_eq!(back, body);
    }

    #[test]
    fn notice_update_reads_the_killer_name_on_the_kill_code() {
        let mut body = vec![0x06, 0x0c];
        body.extend_from_slice(&38211u32.to_le_bytes());
        body.extend_from_slice(&5u16.to_le_bytes());
        body.extend_from_slice(b"Hunter");
        body.truncate(2 + 4 + 2 + 5); // length prefix says 5
        let body = Bytes::from(body);

        let decoded = NoticeUpdate::try_from(body.clone()).unwrap();

        assert_eq!(
            decoded,
            NoticeUpdate::UniqueKilled {
                ref_id: 38211,
                player: "Hunte".to_string(),
            }
        );
        let back: Bytes = decoded.into();
        assert_eq!(back, body);
    }

    /// A spawn body of the wrong length must not be accepted as a spawn.
    #[test]
    fn a_short_notice_body_is_kept_raw_instead_of_misread() {
        let decoded = NoticeUpdate::try_from(Bytes::from_static(&[0x05, 0x0c, 0x43])).unwrap();

        assert!(matches!(
            decoded,
            NoticeUpdate::Raw {
                code: NOTICE_UNIQUE_APPEARED,
                ..
            }
        ));
    }

    // --- Mastery/skill level-down + teleport recall (#260) -------------------

    #[test]
    fn skill_level_down_request_is_a_lone_skill_id() {
        let req = SkillLevelDownRequest {
            ref_skill_id: 0x0102_0304,
        };
        let wire: Bytes = req.clone().into();

        assert_eq!(&wire[..], &0x0102_0304u32.to_le_bytes());
        assert_eq!(SkillLevelDownRequest::try_from(wire).unwrap(), req);
    }

    /// The trailing `amount` byte the level-UP sibling carries is deliberately NOT
    /// mirrored onto the DOWN request — it is unresolved, so the body is 4 bytes.
    #[test]
    fn mastery_level_down_request_omits_the_unresolved_amount_byte() {
        let req = MasteryLevelDownRequest { mastery_id: 257 };
        let wire: Bytes = req.clone().into();

        assert_eq!(wire.len(), 4);
        assert_eq!(MasteryLevelDownRequest::try_from(wire).unwrap(), req);
    }

    #[test]
    fn skill_level_down_response_reads_the_new_skill_id() {
        let mut wire = vec![1u8];
        wire.extend_from_slice(&9001u32.to_le_bytes());

        let decoded = MasterySkillLevelDownResponse::try_from(Bytes::from(wire.clone())).unwrap();

        assert_eq!(
            decoded,
            MasterySkillLevelDownResponse::Success { new_skill_id: 9001 }
        );
        let back: Bytes = decoded.into();
        assert_eq!(&back[..], &wire[..]);
    }

    #[test]
    fn mastery_level_down_response_reads_the_new_level() {
        let mut wire = vec![1u8];
        wire.extend_from_slice(&257u32.to_le_bytes());
        wire.push(4);

        let decoded = MasteryLevelDownResponse::try_from(Bytes::from(wire.clone())).unwrap();

        assert_eq!(
            decoded,
            MasteryLevelDownResponse::Success {
                mastery_id: 257,
                new_level: 4,
            }
        );
        let back: Bytes = decoded.into();
        assert_eq!(&back[..], &wire[..]);
    }

    /// The failure branch is uncaptured: the original reads no error code. Both
    /// candidate shapes must survive — `02 <code>` parses as `Failure`, while a
    /// lone `02` falls through to `Unknown` rather than being misread. That is the
    /// `pos == len` guard doing its job, and it is why cloning the level-UP shape
    /// is safe despite the unknown.
    #[test]
    fn level_down_failure_shapes_both_degrade_safely() {
        let mut with_code = vec![2u8];
        with_code.extend_from_slice(&0x7406u16.to_le_bytes());
        assert_eq!(
            MasteryLevelDownResponse::try_from(Bytes::from(with_code)).unwrap(),
            MasteryLevelDownResponse::Failure(0x7406)
        );

        let lone = Bytes::from_static(&[2]);
        assert_eq!(
            MasteryLevelDownResponse::try_from(lone).unwrap(),
            MasteryLevelDownResponse::Unknown {
                result: 2,
                tail: Bytes::new(),
            }
        );

        // Same for the skill half.
        assert_eq!(
            MasterySkillLevelDownResponse::try_from(Bytes::from_static(&[2])).unwrap(),
            MasterySkillLevelDownResponse::Unknown {
                result: 2,
                tail: Bytes::new(),
            }
        );
    }

    /// A success body of the wrong length must not be accepted as a success.
    #[test]
    fn a_short_level_down_success_body_is_not_read_as_success() {
        let decoded =
            MasteryLevelDownResponse::try_from(Bytes::from_static(&[1, 0x01, 0x01])).unwrap();

        assert!(matches!(
            decoded,
            MasteryLevelDownResponse::Unknown { result: 1, .. }
        ));
    }

    #[test]
    fn teleport_recall_request_is_a_lone_unique_id() {
        let req = TeleportRecallRequest {
            teleport_unique_id: 4242,
        };
        let wire: Bytes = req.clone().into();

        assert_eq!(&wire[..], &4242u32.to_le_bytes());
        assert_eq!(TeleportRecallRequest::try_from(wire).unwrap(), req);
    }

    /// 0xB059 has no parser in any source, so it must not claim a shape — any body,
    /// including an empty one, round-trips untouched instead of failing to decode.
    #[test]
    fn teleport_recall_response_keeps_any_body_whole() {
        for body in [
            Bytes::new(),
            Bytes::from_static(&[1]),
            Bytes::from_static(&[2, 0xAA, 0xBB]),
        ] {
            let decoded = TeleportRecallResponse::try_from(body.clone()).unwrap();
            assert_eq!(decoded.raw, body);
            let back: Bytes = decoded.into();
            assert_eq!(back, body);
        }
    }
    /// 0x70A7 is one byte — the original's builder writes exactly one
    /// (`0081e690:21,30`).
    #[test]
    fn hwan_action_request_is_a_single_byte() {
        let wire: Bytes = HwanActionRequest::berserk().into();
        assert_eq!(&wire[..], &[HWAN_ACTION_BERSERK]);
        assert_eq!(
            HwanActionRequest::try_from(wire).unwrap(),
            HwanActionRequest { action: 1 }
        );
    }

    /// 0xB0A7's error code is conditional: `008a7a20` reads the `u16` only when
    /// the result byte is not `1`, so a success body is a lone byte and a
    /// failure body carries the code.
    #[test]
    fn hwan_action_response_reads_the_error_code_only_on_failure() {
        let ok = HwanActionResponse::try_from(Bytes::from_static(&[0x01])).unwrap();
        assert!(ok.is_success());
        assert_eq!(ok.error_code, None);
        let back: Bytes = ok.into();
        assert_eq!(&back[..], &[0x01]);

        let failed = HwanActionResponse::try_from(Bytes::from_static(&[0x02, 0x34, 0x12])).unwrap();
        assert!(!failed.is_success());
        assert_eq!(failed.error_code, Some(0x1234));
        let back: Bytes = failed.into();
        assert_eq!(&back[..], &[0x02, 0x34, 0x12]);
    }

    /// 0x30DF is `{ u32 unique id, u8 level }` — `008a7630:8-9`, the u32 goes
    /// through the object registry before the byte is applied.
    #[test]
    fn hwan_level_update_is_an_id_and_a_level() {
        let wire = Bytes::from_static(&[0xBE, 0xAB, 0x01, 0x00, 0x03]);
        let decoded = HwanLevelUpdate::try_from(wire.clone()).unwrap();
        assert_eq!(
            decoded,
            HwanLevelUpdate {
                unique_id: 0x1ABBE,
                level: 3,
            }
        );
        let back: Bytes = decoded.into();
        assert_eq!(back, wire);
    }
    /// Real `packet_dump/0x30bf.log` bodies from the 2026-08-12 death capture
    /// (`docs/net-death-resurrect.md`): the four state deltas a death and the
    /// following resurrection produce for the local player `111483`.
    #[test]
    fn death_and_resurrect_state_deltas_decode_from_the_capture() {
        let decode = |hex: &[u8; 6]| {
            EntityStateUpdate::try_from(Bytes::copy_from_slice(hex)).expect("decodes")
        };

        // 13:58:23.185 — combat flag drops in the same millisecond as the death
        let combat_off = decode(&[0x7b, 0xb3, 0x01, 0x00, 0x08, 0x00]);
        assert_eq!(combat_off.unique_id, 111_483);
        assert_eq!(combat_off.kind, STATE_KIND_COMBAT);
        assert_eq!(combat_off.value, 0);
        assert!(!combat_off.is_death(), "kind 8 is not the death signal");

        // 13:58:23.185 — the authoritative death
        let died = decode(&[0x7b, 0xb3, 0x01, 0x00, 0x00, 0x02]);
        assert!(died.is_death());

        // 13:58:24.635 — revive, plus the untouchable window
        let revived = decode(&[0x7b, 0xb3, 0x01, 0x00, 0x00, 0x01]);
        assert!(revived.is_revive());
        let untouchable = decode(&[0x7b, 0xb3, 0x01, 0x00, 0x04, 0x02]);
        assert_eq!(untouchable.value, BODY_STATE_UNTOUCHABLE);
        // it is a body state, but not one that hides the player
        assert_eq!(untouchable.body_invisibility(), Some(false));

        // 13:58:30.920 — cleared 6.29 s later
        let cleared = decode(&[0x7b, 0xb3, 0x01, 0x00, 0x04, 0x00]);
        assert_eq!(cleared.value, BODY_STATE_NONE);
    }

    /// The EXP penalty rides the same opcode as an EXP gain, with a negative
    /// value — `packet_dump/0x3056.log`, both captured deaths.
    #[test]
    fn death_charges_a_negative_experience_delta() {
        let body = Bytes::from_static(&[
            0x80, 0xab, 0x01, 0x00, // exp_origin = the dying player's own uid
            0x5d, 0xf3, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, // experience = -3235
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // sp_exp
            0x00,
        ]);
        let decoded = ReceiveExperience::try_from(body).expect("decodes");
        assert_eq!(decoded.exp_origin, 109_440);
        assert_eq!(decoded.experience, -3235);
        assert_eq!(decoded.stat_points(), None, "no level-up tail on a death");
    }
}
