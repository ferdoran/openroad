use bevy::asset::Asset;
use std::collections::HashMap;
use std::path::{PathBuf, MAIN_SEPARATOR_STR};
use std::str::FromStr;

use bevy::reflect::TypePath;
use lazy_static::lazy_static;
use regex::Regex;

#[derive(TypePath, Asset)]
pub struct ObjectInfoIndex(pub HashMap<u32, ObjectInfo>);

#[allow(dead_code)]
pub struct ObjectInfo {
    pub id: u32,
    /// The row's `0x`-prefixed flag word. Kept as the full `u32`: the corpus
    /// only ever uses 0 or 1 (`{0: 2590, 1: 177}`), but the field is 8 hex
    /// digits wide and its other bits have no documented meaning yet.
    pub flag: u32,
    pub path: PathBuf,
}

lazy_static! {
    // The flag is HEX (`tile.rs` gets this right); a decimal-only class made
    // any hex letter fail the match, and the value was then parsed base-10.
    static ref OBJECT_INFO_REGEX: Regex =
        Regex::new(r#"^(?P<id>\d{05})\s0x(?P<flag>[0-9a-fA-F]{8})\s"(?P<path>.*)"$"#)
            .expect("invalid object info regex");
}

impl ObjectInfo {
    /// Parse one `object.ifo` row, or `None` if it does not match — a
    /// malformed row is skipped by the caller rather than aborting the load.
    pub fn parse(line: &str) -> Option<Self> {
        let m = OBJECT_INFO_REGEX.captures(line)?;
        let id = u32::from_str(m.name("id")?.as_str()).ok()?;
        let flag = u32::from_str_radix(m.name("flag")?.as_str(), 16).ok()?;
        let path = m
            .name("path")?
            .as_str()
            .replace(r"\\", r"\")
            .replace(r"\", MAIN_SEPARATOR_STR);
        Some(Self {
            id,
            flag,
            path: PathBuf::from(path),
        })
    }

    /// Whether this object is a compound (`.cpd`) rather than a single mesh.
    pub fn is_compound(&self) -> bool {
        self.path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e == "cpd")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real `object.ifo` row.
    #[test]
    fn parses_a_shipped_row() {
        let info = ObjectInfo::parse(r#"00001 0x00000001 "res\bldg\china\dungeon\snake.bsr""#)
            .expect("parses");
        assert_eq!(info.id, 1);
        assert_eq!(info.flag, 1);
        assert!(info.path.to_string_lossy().ends_with("snake.bsr"));
        assert!(!info.is_compound());
    }

    /// The flag is hex. A decimal-only class made any hex letter fail the
    /// match, and the digits were then parsed base-10 — so `0x00000010` used
    /// to mean 10 rather than 16 (#284).
    #[test]
    fn the_flag_is_parsed_as_hex() {
        let info = ObjectInfo::parse(r#"00002 0x0000001A "res\a.bsr""#).expect("hex parses");
        assert_eq!(info.flag, 0x1A);
        let ten = ObjectInfo::parse(r#"00003 0x00000010 "res\b.bsr""#).expect("parses");
        assert_eq!(ten.flag, 0x10, "hex 10 is sixteen, not ten");
    }

    /// The corpus only uses 0 and 1, so the shipped data decodes identically
    /// under the old and new readings — this pins that no behaviour changed
    /// for real files.
    #[test]
    fn the_shipped_flag_values_are_unchanged() {
        assert_eq!(
            ObjectInfo::parse(r#"00004 0x00000000 "res\c.bsr""#)
                .unwrap()
                .flag,
            0
        );
        assert_eq!(
            ObjectInfo::parse(r#"00005 0x00000001 "res\d.bsr""#)
                .unwrap()
                .flag,
            1
        );
    }

    /// A malformed row is skipped, not a panic.
    #[test]
    fn a_malformed_row_is_none() {
        assert!(ObjectInfo::parse("not an object row").is_none());
        assert!(ObjectInfo::parse(r#"00001 0xZZZZZZZZ "res\a.bsr""#).is_none());
        assert!(ObjectInfo::parse("").is_none());
    }

    /// `.cpd` rows are compounds; a path with no extension must not panic.
    #[test]
    fn compound_detection_tolerates_a_missing_extension() {
        let cpd = ObjectInfo::parse(r#"00006 0x00000000 "res\thing.cpd""#).unwrap();
        assert!(cpd.is_compound());
        let bare = ObjectInfo::parse(r#"00007 0x00000000 "res\noext""#).unwrap();
        assert!(!bare.is_compound());
    }
}
