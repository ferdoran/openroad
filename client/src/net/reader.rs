//! Bounds-checked little-endian reader for the client-side decoding of the
//! group-spawn payload (0x3019), whose records need the itemdata/characterdata
//! tables to select their layout (see `entity_spawn.rs`). The wire-block
//! shapes themselves (position, character state) are defined in
//! `packets::agent::character_data`; this reader just decodes them
//! `Option`-fashion so a malformed or truncated record fails safe (and keeps a
//! byte offset for diagnostics) instead of erroring through `std::io`.

use bevy::prelude::Component;
use packets::agent::character_data::{ActiveBuff, EntityState, SpawnPosition};

/// A player's guild affiliation, from the spawn record's guild block. Attached
/// to the spawned entity so the nameplate can draw the guild line under the
/// name.
///
/// Only constructed for a **non-empty** guild name: guildless players are on
/// the wire as an empty string, not as an absent block.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
pub struct GuildTag {
    pub name: String,
    /// The guild-granted nickname. Parsed and carried because the record
    /// provides it; no consumer yet.
    pub granted_nick: String,
}

/// Bounds-checked little-endian reader.
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// The current read offset into the buffer.
    pub fn pos(&self) -> usize {
        self.pos
    }

    /// Move the read offset to `pos` (used to resume behind a record whose
    /// width was derived, see `entity_spawn::recover_unknown_record`).
    pub fn seek(&mut self, pos: usize) -> Option<()> {
        (pos <= self.buf.len()).then(|| self.pos = pos)
    }

    pub fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let slice = self.buf.get(self.pos..end)?;
        self.pos = end;
        Some(slice)
    }

    pub fn skip(&mut self, n: usize) -> Option<()> {
        self.take(n).map(|_| ())
    }

    pub fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    pub fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    pub fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub fn f32(&mut self) -> Option<f32> {
        Some(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    /// A u16-length-prefixed string (lossy UTF-8; SRO names are single-byte).
    pub fn string(&mut self) -> Option<String> {
        let len = self.u16()? as usize;
        let bytes = self.take(len)?;
        Some(String::from_utf8_lossy(bytes).into_owned())
    }

    pub fn position(&mut self) -> Option<SpawnPosition> {
        Some(SpawnPosition {
            region: self.u16()?,
            x: self.f32()?,
            y: self.f32()?,
            z: self.f32()?,
            heading: self.u16()?,
        })
    }

    /// Consume the movement block: `has_dest:u8, move_type:u8`, then either a
    /// destination (region, and x/y/z when the destination region > 0) or a
    /// standing turn (`u8, heading:u16`). The destination coordinates are u16
    /// in the overworld and i32 in a dungeon, keyed by the entity's *current*
    /// position region — read immediately before this block — not the
    /// destination region (`docs/re/systems/dungeon-teleport-in.md`).
    pub fn skip_movement(&mut self, current_region: u16) -> Option<()> {
        let has_dest = self.u8()? != 0;
        let _move_type = self.u8()?;
        if has_dest {
            let region = self.u16()?;
            if region > 0 {
                let coord_width = if current_region & 0x8000 != 0 { 4 } else { 2 };
                self.skip(3 * coord_width)?; // x, y, z
            }
        } else {
            self.skip(3)?; // unknown byte + heading:u16
        }
        Some(())
    }

    /// Read the character-state block: `life, unk, motion, body` (4×u8),
    /// walk/run/berserk speeds (3×f32), then a `u8` buff count and that many
    /// 8-byte active-buff entries.
    pub fn character_state(&mut self) -> Option<EntityState> {
        let life_state = self.u8()?;
        let unk = self.u8()?;
        let motion_state = self.u8()?;
        let body_state = self.u8()?;
        let walk_speed = self.f32()?;
        let run_speed = self.f32()?;
        let hwan_speed = self.f32()?;
        let buff_count = self.u8()?;
        let mut buffs = Vec::with_capacity(buff_count as usize);
        for _ in 0..buff_count {
            buffs.push(ActiveBuff {
                ref_skill_id: self.u32()?,
                duration: self.u32()?,
            });
        }
        Some(EntityState {
            life_state,
            unk,
            motion_state,
            body_state,
            walk_speed,
            run_speed,
            hwan_speed,
            buffs,
        })
    }

    /// Read the guild block: name string, guild id (u32), member-nick string,
    /// then 14 bytes — crest revision, union id, union crest revision and two
    /// flag bytes — that nothing consumes yet.
    ///
    /// The block is **always present**, guild or not: a guildless player sends
    /// a zero-length name and the same zeroed tail, so this is not conditional
    /// on membership. `None` (a short read) aborts the record like every other
    /// parse failure here.
    ///
    /// UNVERIFIED against real bytes — `packet_dump/0x3019.log` holds no player
    /// spawn record at all (`docs/net-captured-opcodes.md`), so the layout
    /// comes from go-sro's `WriteGuild` (the server these dumps were captured
    /// against), corroborated field-for-field by the vSRO client-side parser.
    /// Closing it needs the guilded-player capture, `docs/re/CAPTURE_LIST.md`
    /// row A5.
    pub fn guild(&mut self) -> Option<GuildTag> {
        let name = self.string()?;
        self.skip(4)?; // guild id
        let granted_nick = self.string()?;
        self.skip(14)?; // crest rev, union id, union crest rev, 2×u8 flags
        Some(GuildTag { name, granted_nick })
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// A trailing sentinel byte proves exactly the right number of coordinate
    /// bytes was consumed for each region kind.
    #[test]
    fn skip_movement_widths() {
        // Overworld current region: 3 × u16 destination coords.
        let mut b: Vec<u8> = vec![1, 0];
        b.extend_from_slice(&0x60A8u16.to_le_bytes());
        b.extend_from_slice(&[0; 6]);
        b.push(0xEE);
        let mut r = Reader::new(&b);
        r.skip_movement(0x60A8).unwrap();
        assert_eq!(r.u8(), Some(0xEE));

        // Dungeon current region: 3 × i32 destination coords.
        let mut b: Vec<u8> = vec![1, 0];
        b.extend_from_slice(&0x8001u16.to_le_bytes());
        b.extend_from_slice(&[0; 12]);
        b.push(0xEE);
        let mut r = Reader::new(&b);
        r.skip_movement(0x8001).unwrap();
        assert_eq!(r.u8(), Some(0xEE));

        // Destination region 0: no coordinate triple at all.
        let mut b: Vec<u8> = vec![1, 0];
        b.extend_from_slice(&0u16.to_le_bytes());
        b.push(0xEE);
        let mut r = Reader::new(&b);
        r.skip_movement(0x8001).unwrap();
        assert_eq!(r.u8(), Some(0xEE));

        // Standing turn: source byte + heading, independent of region kind.
        let b: Vec<u8> = vec![0, 1, 1, 0xEC, 0x2F, 0xEE];
        let mut r = Reader::new(&b);
        r.skip_movement(0x8001).unwrap();
        assert_eq!(r.u8(), Some(0xEE));
    }
}
