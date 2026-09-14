//! World-map definitions from `worldmap_mapinfo.txt` (+ the POI table
//! `worldmap_localinfo.txt`, read as a sibling).
//!
//! Idea: the `#section WLocalmap` rows declare every map the M window can
//! show — row 0 is the tiled world map (one 128px `map_world_<x>x<z>.ddj`
//! tile per 4x4-region block), the kind-1 rows are single-image city/area
//! maps. Each row carries the map's region rectangle plus per-edge offsets in
//! *display units* (du, 1/10 world unit; 1 region = 192 du), from which the
//! du→pixel projection follows (verified against the data: world map =
//! 1/6 px per du, Jangan = 4/3):
//!
//! ```text
//! left_du = x1*192 + off_left        top_du    = z1*192 + off_top
//! right_du = x2*192 + off_right      bottom_du = z2*192 + off_bottom
//! px_per_du = logical_w / (right_du - left_du)
//! px = ((gx_du - left_du) * k,  (top_du - gz_du) * k)
//! ```
//!
//! A kind-1 row whose region rectangle contains the player's region is "the
//! city map to show here" — this is also the client's only town test.
//! localinfo POIs are either text labels (`SN_ZONE_*`) or `xy_*.ddj` icons,
//! keyed on the map id: world-map POIs carry a region + du offset, city-map
//! POIs carry raw pixels on the city image. The `city_*.ddj` icon rows link
//! to their city map (the world map's clickable city buttons). Docs:
//! `docs/formats/worldmap.md`.

use std::collections::HashMap;

use bevy::math::Vec2;

/// Display units per region edge (region = 1920 world units, du = 1/10).
pub const DU_PER_REGION: f32 = 192.0;

/// Dungeon interiors live in a local frame centred on region (128,128) —
/// the `#section Dungeonmap` rects and the `minimap_d` tile names both use
/// these absolute sector coordinates.
pub const DUNGEON_CENTER_REGION: f32 = 128.0;

#[derive(Debug, Clone, Default)]
pub struct WorldMapTable {
    pub maps: Vec<MapDef>,
    pub pois: Vec<PoiDef>,
    /// `#section Dungeonmap`: per dungeon region id, its floor maps sorted by
    /// floor number. Donwhang shares one region across 4 floor rows; jinsi
    /// has one region (and one row) per floor.
    pub dungeon_maps: HashMap<u16, Vec<DungeonMapDef>>,
}

/// One `#section Dungeonmap` row: a dungeon floor's map/tile geometry.
#[derive(Debug, Clone)]
pub struct DungeonMapDef {
    pub map_id: u32,
    /// `B` rows are basements (floors count downward), `F` count up.
    pub basement: bool,
    pub floor_number: u32,
    pub floor_count: u32,
    /// `0x8000 | dungeoninfo id`.
    pub region_id: u16,
    pub tiles_x: u32,
    pub tiles_z: u32,
    pub logical_w: f32,
    pub logical_h: f32,
    /// Inclusive region-cell rect (left/top/right/bottom; top = north =
    /// larger z), in the 128-centred dungeon sector space. Unlike WLocalmap
    /// rows, the covered edges run x1..x2+1 / z2..z1+1. The per-edge offset
    /// columns (19-22) are 0 in 19 of the 23 rows and are not read; the four
    /// Jupiter floors (map ids 2020-2023) do carry values there, so they
    /// project as if the offsets were 0 — see `docs/formats/worldmap.md`.
    pub x1: i32,
    pub z1: i32,
    pub x2: i32,
    pub z2: i32,
    /// Zoom: `logical = tiles · 256 · scale` (holds for every corpus row).
    pub scale: f32,
    /// World-map tile prefix, forward slashes
    /// (`interface/worldmap/dungeon/map_world_donf01_`); tile name =
    /// `{prefix}{x}x{z}.ddj`.
    pub tile_prefix: String,
    pub dungeon_id: u16,
    /// `DH_A01_FLOOR01`-style key — matches the `.dof` floor label and,
    /// lowercased, the `minimap_d` tile filename stem.
    pub floor_string: String,
}

impl DungeonMapDef {
    pub fn left_du(&self) -> f32 {
        self.x1 as f32 * DU_PER_REGION
    }
    pub fn top_du(&self) -> f32 {
        (self.z1 + 1) as f32 * DU_PER_REGION
    }
    pub fn right_du(&self) -> f32 {
        (self.x2 + 1) as f32 * DU_PER_REGION
    }
    pub fn bottom_du(&self) -> f32 {
        self.z2 as f32 * DU_PER_REGION
    }
    pub fn px_per_du(&self) -> f32 {
        self.logical_w / (self.right_du() - self.left_du())
    }
    /// Project global display-unit coordinates onto the floor map image
    /// (pixels, y down from the top edge).
    pub fn project(&self, gx_du: f32, gz_du: f32) -> Vec2 {
        let k = self.px_per_du();
        Vec2::new((gx_du - self.left_du()) * k, (self.top_du() - gz_du) * k)
    }
    /// Whether a point (in dungeon display units) lies on the map.
    pub fn contains_du(&self, gx_du: f32, gz_du: f32) -> bool {
        (self.left_du()..=self.right_du()).contains(&gx_du)
            && (self.bottom_du()..=self.top_du()).contains(&gz_du)
    }
}

/// Raw dungeon-local coordinates as global display units in the 128-centred
/// dungeon sector space.
pub fn dungeon_du(raw_x: f32, raw_z: f32) -> Vec2 {
    Vec2::new(
        DUNGEON_CENTER_REGION * DU_PER_REGION + raw_x / 10.0,
        DUNGEON_CENTER_REGION * DU_PER_REGION + raw_z / 10.0,
    )
}

/// The dungeon-space sector holding a raw dungeon-local coordinate pair.
pub fn dungeon_sector(raw_x: f32, raw_z: f32) -> (i32, i32) {
    (
        DUNGEON_CENTER_REGION as i32 + (raw_x / 1920.0).floor() as i32,
        DUNGEON_CENTER_REGION as i32 + (raw_z / 1920.0).floor() as i32,
    )
}

#[derive(Debug, Clone)]
pub struct MapDef {
    pub id: u32,
    /// false = single image (`texture` is the full path), true = 4x4-region
    /// tiles (`texture` is the `map_world_` prefix).
    pub tiled: bool,
    pub texture: String,
    /// The texture's full extent vs the sub-rect that carries the art (e.g.
    /// Khotan draws into 512x838 of a 512x1024 texture).
    pub tex_w: f32,
    pub tex_h: f32,
    pub used_w: f32,
    pub used_h: f32,
    pub logical_w: f32,
    pub logical_h: f32,
    /// Region rectangle: x1..x2 west→east, z1 (north) down to z2 (south).
    pub x1: i32,
    pub z1: i32,
    pub x2: i32,
    pub z2: i32,
    pub off_left: f32,
    pub off_top: f32,
    pub off_right: f32,
    pub off_bottom: f32,
    /// `UIIT_*` / `SN_*` display key.
    pub name_key: String,
    pub enabled: bool,
}

impl MapDef {
    pub fn left_du(&self) -> f32 {
        self.x1 as f32 * DU_PER_REGION + self.off_left
    }
    pub fn top_du(&self) -> f32 {
        self.z1 as f32 * DU_PER_REGION + self.off_top
    }
    pub fn right_du(&self) -> f32 {
        self.x2 as f32 * DU_PER_REGION + self.off_right
    }
    pub fn bottom_du(&self) -> f32 {
        self.z2 as f32 * DU_PER_REGION + self.off_bottom
    }
    /// Map pixels per display unit.
    pub fn px_per_du(&self) -> f32 {
        self.logical_w / (self.right_du() - self.left_du())
    }
    /// Project global display-unit coordinates onto the map image (pixels,
    /// y down from the image's top edge).
    pub fn project(&self, gx_du: f32, gz_du: f32) -> Vec2 {
        let k = self.px_per_du();
        Vec2::new((gx_du - self.left_du()) * k, (self.top_du() - gz_du) * k)
    }
    /// Whether the player's region falls inside this map's rectangle (the
    /// city-map test; note z1 is the NORTH/larger bound).
    pub fn contains_region(&self, rx: i32, rz: i32) -> bool {
        self.x1 <= rx && rx <= self.x2 && self.z2 <= rz && rz <= self.z1
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PoiKind {
    /// Text label — an `SN_ZONE_*` key into textdataname.
    Label(String),
    /// Icon — an `interface\worldmap\...` ddj path.
    Icon(String),
}

#[derive(Debug, Clone)]
pub struct PoiDef {
    /// The `MapDef.id` this POI belongs to (0 = world map).
    pub map_id: u32,
    pub kind: PoiKind,
    /// City-button link: clicking switches to this map id.
    pub link_map: Option<u32>,
    /// World-map POIs: region + a *map pixel* offset from the region cell's
    /// north-west corner (a region is 32 px on the world map; e.g. the
    /// city_jangan button at region 167x98 + (16, 2) lands centered on
    /// Jangan). City-map POIs: `None`, `px`/`pz` are raw pixels on the city
    /// image.
    pub region: Option<(i32, i32)>,
    pub px: f32,
    pub pz: f32,
    pub w: f32,
    pub h: f32,
}

impl PoiDef {
    /// The POI's position on its map image, in map pixels (the icon/label
    /// anchor point — vanilla draws from here, not centered).
    pub fn map_px(&self, map: &MapDef) -> Vec2 {
        match self.region {
            Some((rx, rz)) => {
                // the region cell's NW corner on the map...
                let corner =
                    map.project(rx as f32 * DU_PER_REGION, (rz + 1) as f32 * DU_PER_REGION);
                // ...plus the offset, already in map pixels
                corner + Vec2::new(self.px, self.pz)
            }
            None => Vec2::new(self.px, self.pz),
        }
    }
}

impl WorldMapTable {
    pub fn map(&self, id: u32) -> Option<&MapDef> {
        self.maps.iter().find(|map| map.id == id)
    }

    /// The enabled city/area map covering a region, if any.
    pub fn city_map_for_region(&self, rx: i32, rz: i32) -> Option<&MapDef> {
        self.maps
            .iter()
            .find(|map| !map.tiled && map.enabled && map.contains_region(rx, rz))
    }

    pub fn pois_for(&self, map_id: u32) -> impl Iterator<Item = &PoiDef> {
        self.pois.iter().filter(move |poi| poi.map_id == map_id)
    }

    /// A dungeon region's floor maps (sorted by floor number), empty if the
    /// data declares none.
    pub fn dungeon_floors(&self, region_id: u16) -> &[DungeonMapDef] {
        self.dungeon_maps
            .get(&region_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// The floor map matching a `.dof` floor label, case-insensitive.
    pub fn dungeon_floor(&self, region_id: u16, floor_string: &str) -> Option<&DungeonMapDef> {
        self.dungeon_floors(region_id)
            .iter()
            .find(|def| def.floor_string.eq_ignore_ascii_case(floor_string))
    }

    pub fn parse(mapinfo: &str, localinfo: &str) -> Self {
        let mut table = WorldMapTable::default();

        // --- worldmap_mapinfo.txt: #section WLocalmap + #section Dungeonmap -
        #[derive(PartialEq)]
        enum Section {
            Other,
            Local,
            Dungeon,
        }
        let mut section = Section::Other;
        for line in mapinfo.lines() {
            let fields: Vec<&str> = line.split('\t').map(str::trim).collect();
            let head = fields[0].trim_start_matches('\u{feff}');
            if head == "#section" {
                section = match fields.get(1).copied() {
                    Some("WLocalmap") => Section::Local,
                    Some("Dungeonmap") => Section::Dungeon,
                    _ => Section::Other,
                };
                continue;
            }
            let num = |index: usize| {
                fields
                    .get(index)
                    .and_then(|f| f.parse::<f32>().ok())
                    .unwrap_or(0.0)
            };
            let int = |index: usize| {
                fields
                    .get(index)
                    .and_then(|f| f.parse::<i32>().ok())
                    .unwrap_or(0)
            };
            match section {
                Section::Local if fields.len() >= 21 => {
                    let (Ok(id), Ok(kind)) = (head.parse::<u32>(), fields[1].parse::<u8>()) else {
                        continue;
                    };
                    table.maps.push(MapDef {
                        id,
                        tiled: kind == 0,
                        texture: fields[3].replace('\\', "/"),
                        tex_w: num(4),
                        tex_h: num(5),
                        used_w: num(6),
                        used_h: num(7),
                        logical_w: num(8),
                        logical_h: num(9),
                        x1: int(10),
                        z1: int(11),
                        x2: int(12),
                        z2: int(13),
                        off_left: num(14),
                        off_top: num(15),
                        off_right: num(16),
                        off_bottom: num(17),
                        name_key: fields[19].to_string(),
                        // the world map row itself carries 0 here; kind-1 rows use it
                        enabled: fields[20] == "1" || kind == 0,
                    });
                }
                Section::Dungeon if fields.len() >= 19 => {
                    let Ok(map_id) = head.parse::<u32>() else {
                        continue;
                    };
                    if fields[18] != "1" {
                        continue; // disabled floor row
                    }
                    let region_id = int(5) as u16;
                    table
                        .dungeon_maps
                        .entry(region_id)
                        .or_default()
                        .push(DungeonMapDef {
                            map_id,
                            basement: fields[2].eq_ignore_ascii_case("B"),
                            floor_number: int(3).max(0) as u32,
                            floor_count: int(4).max(0) as u32,
                            region_id,
                            tiles_x: int(6).max(0) as u32,
                            tiles_z: int(7).max(0) as u32,
                            logical_w: num(8),
                            logical_h: num(9),
                            x1: int(10),
                            z1: int(11),
                            x2: int(12),
                            z2: int(13),
                            scale: num(14),
                            tile_prefix: fields[15].replace('\\', "/"),
                            dungeon_id: int(16) as u16,
                            floor_string: fields[17].to_string(),
                        });
                }
                _ => {}
            }
        }
        for floors in table.dungeon_maps.values_mut() {
            floors.sort_by_key(|def| def.floor_number);
        }

        // --- worldmap_localinfo.txt ----------------------------------------
        for line in localinfo.lines() {
            let fields: Vec<&str> = line.split('\t').map(str::trim).collect();
            if fields.len() < 15 || fields[0].trim_start_matches('\u{feff}') != "1" {
                continue;
            }
            let kind = match fields[2] {
                "1" => PoiKind::Label(fields[3].to_string()),
                "2" => PoiKind::Icon(fields[3].replace('\\', "/")),
                _ => continue,
            };
            let Ok(map_id) = fields[8].parse::<u32>() else {
                continue;
            };
            let link_map = fields[7].parse::<u32>().ok().filter(|&link| link > 0);
            let region = match (fields[9].parse::<i32>(), fields[10].parse::<i32>()) {
                (Ok(rx), Ok(rz)) if rx >= 0 && rz >= 0 => Some((rx, rz)),
                _ => None,
            };
            let num = |index: usize| {
                fields
                    .get(index)
                    .and_then(|f| f.parse::<f32>().ok())
                    .unwrap_or(0.0)
            };
            table.pois.push(PoiDef {
                map_id,
                kind,
                link_map,
                region,
                px: num(11),
                pz: num(12),
                w: num(13),
                h: num(14),
            });
        }
        table
    }
}

#[cfg(test)]
mod test {
    use super::*;

    const MAPINFO: &str = "#section\tWLocalmap\t\t\n\
        0\t0\tW\tinterface\\worldmap\\map\\map_world_\t128\t128\t128\t128\t4224\t1408\t46\t113\t177\t70\t0\t192\t192\t0\txxx\tUIIT_PAG_WORLDMAP\t0\t4x4\t\n\
        1\t1\tJ\tinterface\\worldmap\\map\\map_jangan.ddj\t1024\t1024\t1024\t768\t1024\t768\t166\t99\t170\t96\t96\t96\t96\t96\txxx\tUIIT_STT_JANGAN\t1\txxx\t\n\
        #section\tDungeonmap\t\t\n\
        2001\tKR\tF\t1\t4\t32769\t3\t3\t768\t768\t127\t128\t129\t126\t1.0\tinterface\\worldmap\\dungeon\\map_world_donf01_\t1\tDH_A01_FLOOR01\t1\t0\t0\t0\t0\n\
        2002\tKR\tF\t2\t4\t32769\t3\t3\t768\t768\t127\t128\t129\t126\t1.0\tinterface\\worldmap\\dungeon\\map_world_donf02_\t1\tDH_A01_FLOOR02\t0\t0\t0\t0\t0\n";

    const LOCALINFO: &str = "1\t11001\t1\tSN_ZONE_11001\tJ\tSmith\tlocal\t0\t1\t-1\t-1\t413\t459\t0\t0\t255\t255\t255\t0\t0\t1\txxx\n\
        1\t91001\t2\tinterface\\worldmap\\map\\city_jangan.ddj\tCH\tJangan\tworld\t1\t0\t167\t98\t16\t2\t64\t64\t0\t0\t0\t0\t0\t1\txxx\n";

    #[test]
    fn parses_maps_and_projects() {
        let table = WorldMapTable::parse(MAPINFO, LOCALINFO);
        assert_eq!(table.maps.len(), 2);

        let world = table.map(0).unwrap();
        assert!(world.tiled);
        // 4224 px over regions 46..178 → exactly 1/6 px per du
        assert!((world.px_per_du() - 1.0 / 6.0).abs() < 1e-6);

        let jangan = table.map(1).unwrap();
        assert!(!jangan.tiled);
        // 1024 px over 768 du → 4/3 px per du
        assert!((jangan.px_per_du() - 4.0 / 3.0).abs() < 1e-6);
        // Jangan region 168x97 is inside the city rect, 150x97 is not
        assert!(jangan.contains_region(168, 97));
        assert!(!jangan.contains_region(150, 97));
        assert_eq!(table.city_map_for_region(168, 97).unwrap().id, 1);

        // the city rect center projects into the image
        let center_du = (jangan.left_du() + jangan.right_du()) / 2.0;
        let center = jangan.project(center_du, (jangan.top_du() + jangan.bottom_du()) / 2.0);
        assert!((center.x - 512.0).abs() < 1.0);
        assert!((center.y - 384.0).abs() < 1.0);
    }

    #[test]
    fn parses_dungeonmap_rows() {
        let table = WorldMapTable::parse(MAPINFO, LOCALINFO);
        // Dungeonmap rows never leak into the WLocalmap set.
        assert_eq!(table.maps.len(), 2);

        // Donwhang region 0x8001: floor 1 enabled, floor 2's disabled row
        // dropped.
        let floors = table.dungeon_floors(0x8001);
        assert_eq!(floors.len(), 1);
        let floor = &floors[0];
        assert_eq!(floor.floor_number, 1);
        assert!(!floor.basement);
        assert_eq!(floor.floor_count, 4);
        assert_eq!((floor.tiles_x, floor.tiles_z), (3, 3));
        assert_eq!(
            floor.tile_prefix,
            "interface/worldmap/dungeon/map_world_donf01_"
        );
        assert_eq!(floor.dungeon_id, 1);
        assert_eq!(
            table
                .dungeon_floor(0x8001, "dh_a01_floor01")
                .unwrap()
                .map_id,
            2001
        );

        // Inclusive-cell rect: 3 regions of 256px at scale 1.0 → 4/3 px/du.
        assert!((floor.px_per_du() - 4.0 / 3.0).abs() < 1e-6);
        // The Donwhang arrival (raw 1011, -862) sits in sector (128, 127),
        // inside the floor rect, and projects into the 768px image.
        let du = dungeon_du(1011.0, -862.0);
        assert_eq!(dungeon_sector(1011.0, -862.0), (128, 127));
        assert!(floor.contains_du(du.x, du.y));
        let px = floor.project(du.x, du.y);
        assert!((0.0..=768.0).contains(&px.x) && (0.0..=768.0).contains(&px.y));

        // Unknown region → empty, no panic.
        assert!(table.dungeon_floors(0x8FFF).is_empty());
    }

    #[test]
    fn parses_pois() {
        let table = WorldMapTable::parse(MAPINFO, LOCALINFO);
        assert_eq!(table.pois.len(), 2);

        let city_poi = &table.pois[0];
        assert_eq!(city_poi.map_id, 1);
        assert_eq!(city_poi.region, None);
        let jangan = table.map(1).unwrap();
        assert_eq!(city_poi.map_px(jangan), Vec2::new(413.0, 459.0));

        let button = &table.pois[1];
        assert_eq!(button.map_id, 0);
        assert_eq!(button.link_map, Some(1));
        assert!(matches!(&button.kind, PoiKind::Icon(path) if path.ends_with("city_jangan.ddj")));
        // region 167x98's NW corner is ((167-46)*32, (114-99)*32) = (3872, 480)
        // on the 32-px-per-region world map; + the (16, 2) pixel offset
        let world = table.map(0).unwrap();
        assert_eq!(button.map_px(world), Vec2::new(3888.0, 482.0));
    }
}
