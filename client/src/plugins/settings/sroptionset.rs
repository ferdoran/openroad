//! Dumb TLV decode of the original client's `SROptionSet.dat` (the player's
//! saved in-game options). See `docs/formats/sroptionset.md`.
//!
//! The format is NOT self-describing: each record is `id:u16` +
//! `unk_ushort0:u16` (always 0) followed by a value whose *width is chosen by
//! the id* (`value_width`). An unknown id therefore makes the rest of the
//! stream unparseable, so we stop gracefully and return what was decoded so
//! far. Every read is length-guarded — untrusted file input never panics.

use bevy::log::warn;
use bytes::{Buf, Bytes};

/// Opaque 9-byte header (`u32`, `u8`, `u32`). Read but not interpreted — it
/// carries no option data, so the fields exist only to document the layout.
#[allow(dead_code)]
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SroOptionHeader {
    pub unk_uint0: u32,
    pub unk_byte0: u8,
    pub unk_uint1: u32,
}

/// One decoded option value. Which variant a record produces is fixed by its
/// id (see `value_width` / `is_bool_id`), not encoded in the stream.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OptionValue {
    U8(u8),
    U16(u16),
    U32(u32),
    Bool(bool),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OptionRecord {
    pub id: u16,
    pub value: OptionValue,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SroOptionSet {
    pub header: SroOptionHeader,
    pub records: Vec<OptionRecord>,
}

/// Value width in bytes for a known option id, or `None` when the id is
/// unknown — which makes the rest of the stream unparseable. Ranges mirror the
/// id table in `docs/formats/sroptionset.md`.
fn value_width(id: u16) -> Option<usize> {
    match id {
        1..=15 | 101..=115 => Some(2),    // graphic quality sliders (u16)
        501 | 502 | 601 | 602 => Some(1), // type / brightness (u8)
        503 | 504 | 603 | 604 => Some(4), // window res W/H (u32)
        1001..=1003 => Some(4),           // BGM / FX / Env volume (u32)
        1004..=1006 => Some(1),           // volume on/off (bool)
        2001..=2028 => Some(1),           // gameplay / display / name / camera toggles (bool)
        3101 => Some(1),                  // mouse-shortcut-swap (bool)
        3001..=3099 => Some(4),           // KeyMap VK codes (u32)
        _ => None,                        // unknown id -> width unknown -> STOP
    }
}

/// Whether a width-1 id decodes to `Bool` (a checkbox) rather than a raw `U8`
/// (`Type` / `Brightness`, ids 501/502/601/602).
fn is_bool_id(id: u16) -> bool {
    matches!(id, 1004..=1006 | 2001..=2028 | 3101)
}

/// Decode a whole `SROptionSet.dat`. Returns the partial result (defaulting to
/// an empty set) on any short/unknown input — this never panics or errors.
pub fn parse_sroptionset(data: &[u8]) -> SroOptionSet {
    let mut buf = Bytes::copy_from_slice(data);
    if buf.remaining() < 9 {
        warn!(
            "[sroptionset] input too short for 9-byte header ({} bytes)",
            data.len()
        );
        return SroOptionSet::default();
    }
    let header = SroOptionHeader {
        unk_uint0: buf.get_u32_le(),
        unk_byte0: buf.get_u8(),
        unk_uint1: buf.get_u32_le(),
    };

    let mut records = Vec::new();
    // Need id (2) + unk_ushort0 (2) before a record can even be attempted; a
    // trailing 1-3 byte remnant is ignored (the original loop stops near EOF).
    while buf.remaining() >= 4 {
        let id = buf.get_u16_le();
        let _unk_ushort0 = buf.get_u16_le(); // always 0; NOT a skip length
        let Some(width) = value_width(id) else {
            warn!(
                "[sroptionset] unknown option id {id}; stopping after {} record(s)",
                records.len()
            );
            break;
        };
        if buf.remaining() < width {
            warn!(
                "[sroptionset] truncated value for id {id} (need {width}, have {})",
                buf.remaining()
            );
            break;
        }
        let value = match width {
            1 if is_bool_id(id) => OptionValue::Bool(buf.get_u8() != 0),
            1 => OptionValue::U8(buf.get_u8()),
            2 => OptionValue::U16(buf.get_u16_le()),
            _ => OptionValue::U32(buf.get_u32_le()),
        };
        records.push(OptionRecord { id, value });
    }

    SroOptionSet { header, records }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal valid stream: 9-byte header + one record per width class.
    fn fixture() -> Vec<u8> {
        let mut b = Vec::new();
        // header: u32, u8, u32
        b.extend_from_slice(&1u32.to_le_bytes());
        b.push(7u8);
        b.extend_from_slice(&0u32.to_le_bytes());
        // id 1 -> u16 (graphic slider)
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(&42u16.to_le_bytes());
        // id 501 -> u8 (type)
        b.extend_from_slice(&501u16.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        b.push(3u8);
        // id 503 -> u32 (window width)
        b.extend_from_slice(&503u16.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(&1920u32.to_le_bytes());
        // id 1004 -> bool (bgm on/off)
        b.extend_from_slice(&1004u16.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        b.push(1u8);
        b
    }

    #[test]
    fn parses_header_and_every_width_class() {
        let set = parse_sroptionset(&fixture());
        assert_eq!(
            set.header,
            SroOptionHeader {
                unk_uint0: 1,
                unk_byte0: 7,
                unk_uint1: 0,
            }
        );
        assert_eq!(
            set.records,
            vec![
                OptionRecord {
                    id: 1,
                    value: OptionValue::U16(42)
                },
                OptionRecord {
                    id: 501,
                    value: OptionValue::U8(3)
                },
                OptionRecord {
                    id: 503,
                    value: OptionValue::U32(1920)
                },
                OptionRecord {
                    id: 1004,
                    value: OptionValue::Bool(true)
                },
            ]
        );
    }

    #[test]
    fn truncated_value_returns_partial_without_panic() {
        let mut b = fixture();
        // append a known 4-byte-value record (id 3001) but with only 1 value byte
        b.extend_from_slice(&3001u16.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        b.push(0xEE);
        let set = parse_sroptionset(&b);
        // the four complete records survive; the truncated one is dropped
        assert_eq!(set.records.len(), 4);
    }

    #[test]
    fn unknown_id_stops_parsing() {
        let mut b = fixture();
        b.extend_from_slice(&9999u16.to_le_bytes()); // no width -> STOP
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes());
        let set = parse_sroptionset(&b);
        assert_eq!(set.records.len(), 4);
    }

    #[test]
    fn too_short_header_yields_empty_set() {
        assert_eq!(parse_sroptionset(&[0, 1, 2, 3]), SroOptionSet::default());
    }
}
