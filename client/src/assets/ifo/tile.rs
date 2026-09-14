// tile2d.ifo (JMXV2DTI) entry parser. Each line is
//   `00007 0x0000000a "HMfild" "c_grass_hmfld_01.ddj" {757,64} {1816,36}...`
// with zero or more trailing {objectIfoId,count} 3D-grass pairs. The head is
// matched with a regex whose quoted groups never cross quotes; the brace pairs
// are pulled out of the tail with a second regex so multi-pair lines can't be
// mangled by a single greedy group. See docs/formats/2dti-jmxv2dti.md.
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;

use bevy::asset::Asset;
use bevy::reflect::TypePath;
use lazy_static::lazy_static;
use num_enum::TryFromPrimitive;
use regex::Regex;

/// Terrain surface classification from tile2d.ifo column 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, TryFromPrimitive)]
#[repr(u32)]
#[allow(dead_code)]
pub enum TileType {
    Dirt = 0,
    Sand = 1,
    Ashfield = 2,
    Stone = 3,
    Metal = 4,
    Wood = 5,
    Mud = 6,
    Water = 7,
    DeepWater = 8,
    Snow = 9,
    Grass = 10,
    LongGrass = 11,
    Forest = 12,
    Cloud = 13,
}

#[derive(Clone)]
pub struct TileInfo {
    pub id: u16,
    pub typ: u32,
    #[allow(dead_code)]
    pub category: String,
    pub texture: PathBuf,
    /// 3D-grass pairs: (object.ifo model id, tufts scattered per placement unit).
    pub grass_3d: Vec<(u32, u16)>,
}

impl TileInfo {
    pub fn tile_type(&self) -> Option<TileType> {
        TileType::try_from(self.typ).ok()
    }
}

lazy_static! {
    static ref TILE_HEAD_REGEX: Regex = Regex::new(
        r#"^(?P<id>\d{5})\s+0x(?P<type>[0-9a-fA-F]{8})\s+"(?P<category>[^"]*)"\s+"(?P<path>[^"]*)"(?P<grass>.*)$"#
    )
    .expect("invalid tile head regex");
    static ref GRASS_PAIR_REGEX: Regex =
        Regex::new(r"\{(\d+)\s*,\s*(\d+)\}").expect("invalid grass pair regex");
}

impl From<String> for TileInfo {
    fn from(value: String) -> Self {
        let captures = TILE_HEAD_REGEX
            .captures(value.as_str())
            .unwrap_or_else(|| panic!("failed to parse tile info: {}", value));
        let id = u16::from_str(&captures["id"]).expect("tile id");
        let typ = u32::from_str_radix(&captures["type"], 16).expect("tile type");
        let category = String::from(&captures["category"]);
        let texture = PathBuf::from_str(&captures["path"]).expect("tile texture path");
        let grass_3d = GRASS_PAIR_REGEX
            .captures_iter(&captures["grass"])
            .map(|pair| {
                let model = u32::from_str(&pair[1]).expect("grass model id");
                let count = u16::from_str(&pair[2]).expect("grass count");
                (model, count)
            })
            .collect();
        Self {
            id,
            typ,
            category,
            texture,
            grass_3d,
        }
    }
}

#[derive(TypePath, Asset, Clone)]
pub struct TileInfoIndex {
    pub tiles: HashMap<u16, TileInfo>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_line_without_grass() {
        let info = TileInfo::from(String::from(
            r#"00000 0x00000000 "CJfild" "c_dust_fld_01.ddj""#,
        ));
        assert_eq!(info.id, 0);
        assert_eq!(info.typ, 0);
        assert_eq!(info.tile_type(), Some(TileType::Dirt));
        assert_eq!(info.category, "CJfild");
        assert_eq!(info.texture, PathBuf::from("c_dust_fld_01.ddj"));
        assert!(info.grass_3d.is_empty());
    }

    #[test]
    fn parses_single_grass_pair() {
        let info = TileInfo::from(String::from(
            r#"00007 0x0000000a "HMfild" "c_grass_hmfld_01.ddj" {757,64}"#,
        ));
        assert_eq!(info.id, 7);
        assert_eq!(info.tile_type(), Some(TileType::Grass));
        assert_eq!(info.category, "HMfild");
        assert_eq!(info.grass_3d, vec![(757, 64)]);
    }

    #[test]
    fn parses_multiple_grass_pairs() {
        let info = TileInfo::from(String::from(
            r#"00689 0x0000000a "Masin" "masin_grass_tile.ddj" {3028,60} {3030,30} {3029,15} {3031,29} {2985,7}"#,
        ));
        assert_eq!(
            info.grass_3d,
            vec![(3028, 60), (3030, 30), (3029, 15), (3031, 29), (2985, 7)]
        );
    }

    #[test]
    fn tile_type_conversion() {
        assert_eq!(TileType::try_from(10), Ok(TileType::Grass));
        assert_eq!(TileType::try_from(13), Ok(TileType::Cloud));
        assert!(TileType::try_from(99).is_err());
    }
}
