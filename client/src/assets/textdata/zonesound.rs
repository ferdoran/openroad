//! Zone sound tables: `regioninfo.txt` (which sectors make up a zone) joined
//! with `effectenvsnd.txt` (what that zone sounds like) into one region-id
//! lookup — the data half of EP-22.1.
//!
//! The idea: neither file alone can answer "what should be playing here".
//! `regioninfo.txt` is the navmesh zone table — a `#TOWN`/`#FIELD` header
//! naming a zone, followed by the sector rows it owns; `effectenvsnd.txt`
//! keys off the *same zone name* and carries the BGM track plus the day/night
//! ambient list. So this module parses both and fans the sector rows into a
//! map from packed region id to a zone entry, which is what a playback system
//! can actually query. Both files are CP949 with no BOM (`decode.rs`).
//!
//! Two things the raw rows do not say out loud, both measured in the user's
//! own `Media.pk2` (2026-08-16) and written up in
//! `docs/formats/textdata-regioninfo-effectenvsnd.md`:
//!
//! * A sector row is either `ALL` (the zone owns the whole sector) or `RECT`
//!   plus four sector-local bounds. 66 sectors are claimed twice; in every one
//!   of them exactly one claimant is `ALL` and the others are `RECT`, so the
//!   `RECT` is the specific claim carved out of the `ALL` background. That is
//!   the resolution order [`ZoneSoundTable::zone_at`] implements.
//! * The sector-Z column reaches 128, one past the 0..=127 overworld grid.
//!   Those 17 rows are dungeons: packed the usual way (`z << 8 | x`) they are
//!   exactly `0x8000 | id` of `dungeoninfo.txt`, and the names line up
//!   (x=1 -> `Dunhwang_Cv`, x=2..4 -> `jinsi_floor06..04`, x=10..16 -> the
//!   Egypt caves). So one packing covers overworld and dungeon zones alike and
//!   no special case is needed here.

use std::collections::HashMap;

/// Side length of one map sector in SRO units. Deliberately a local copy of
/// `plugins::map::terrain::REGION_SIZE`: `assets::textdata` is part of the
/// crate's parser-only lib surface (`client/src/lib.rs`) and must not
/// reference `plugins`.
const SECTOR_SIZE: f32 = 1920.0;

/// `#TOWN` or `#FIELD` — the two block kinds `regioninfo.txt` uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoneKind {
    Town,
    Field,
}

/// Which `<2>` section of `effectenvsnd.txt` an ambient list came from.
///
/// The file labels them 낮 (day) and 밤 (night). NOTE: in the shipped v1.188
/// table the two lists are frequently swapped relative to their labels (the
/// 낮/day list of 도적마을 opens with `night_wind.wav`, its 밤/night list with
/// `day_wind.wav`). We reproduce the file's own labelling and do not
/// "correct" it — the filenames are not authoritative, the section header is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeOfDay {
    Day,
    Night,
}

/// One `<3>` row: a `.wav` and how often it repeats.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ambient {
    /// File name as written, e.g. `donhwang_wind04.wav`.
    pub file: String,
    /// Lower bound of the repeat interval in seconds (`a` of `a~b`).
    pub min_secs: u32,
    /// Upper bound of the repeat interval in seconds (`b` of `a~b`).
    pub max_secs: u32,
}

impl Ambient {
    /// `0~0` marks the continuous bed of the zone (loop it); anything else is
    /// a one-shot that fires again after a random delay in `min..=max`.
    pub fn is_continuous(&self) -> bool {
        self.min_secs == 0 && self.max_secs == 0
    }

    /// Asset path of the `.wav` in the mounted `data://` source (#772).
    ///
    /// The directory is not in the table — it was resolved against the user's
    /// own `Data.pk2`: **all 40 distinct names of `effectenvsnd.txt` that
    /// exist in the archive live in `prim/snd/env/` and nowhere else** (35 are
    /// present; the five `kk_*.wav` of 카라코람 are not shipped, which is data,
    /// not a bug). Lowercased because PK2 lookups normalize that way
    /// (`bevy_pk2/src/pk2/archive.rs:70`).
    pub fn asset_path(&self) -> String {
        format!("data://prim/snd/env/{}", self.file.to_lowercase())
    }
}

/// A zone: its `regioninfo.txt` identity plus its `effectenvsnd.txt` sounds.
#[derive(Debug, Clone)]
pub struct ZoneSound {
    /// Zone name, the join key between the two files (CP949, usually Hangul).
    pub name: String,
    pub kind: ZoneKind,
    /// Optional third header column (`donwhang`, `jinsi`, ...); empty for 35
    /// of the 43 blocks, so it is a hint, not an identifier.
    pub code: Option<String>,
    /// BGM track from `effectenvsnd.txt`, e.g. `Jangan_Town.ogg`. `None` when
    /// the zone has no `effectenvsnd` entry at all.
    pub bgm: Option<String>,
    pub day: Vec<Ambient>,
    pub night: Vec<Ambient>,
}

// Read by the playback half (#771); the loader itself only builds the table.
#[allow(dead_code)]
impl ZoneSound {
    /// Asset path of the BGM track in the mounted `music://` source. PK2
    /// lookups are case-insensitive; the archive stores the tracks lowercase.
    pub fn bgm_asset_path(&self) -> Option<String> {
        self.bgm
            .as_ref()
            .map(|f| format!("music://{}", f.to_lowercase()))
    }

    pub fn ambients(&self, when: TimeOfDay) -> &[Ambient] {
        match when {
            TimeOfDay::Day => &self.day,
            TimeOfDay::Night => &self.night,
        }
    }
}

/// Sector-local bounds of a `RECT` claim, in SRO units (0..=[`SECTOR_SIZE`]).
///
/// The four columns are two corners: column 2/4 march monotonically upward
/// across the six RECT rows of sector (49,90), which is what identifies them
/// as one axis' min/max. Which corner axis is X and which is Z is **not**
/// stated by the file; we read them as `x_min z_min x_max z_max` because
/// columns 1/2 of every sector row are already X/Z in that order. The
/// observation that would settle it: stand in Alexandria at the sector-(49,90)
/// boundary and check whether the zone flips to 델타지역 along X or along Z.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SectorRect {
    pub x_min: f32,
    pub z_min: f32,
    pub x_max: f32,
    pub z_max: f32,
}

impl SectorRect {
    fn contains(&self, x: f32, z: f32) -> bool {
        (self.x_min..=self.x_max).contains(&x) && (self.z_min..=self.z_max).contains(&z)
    }
}

/// One sector row: which zone claims it, and over the whole sector (`None`)
/// or only a rectangle of it.
#[derive(Debug, Clone)]
struct SectorClaim {
    zone: usize,
    rect: Option<SectorRect>,
}

/// The joined table. Keyed by the packed region id
/// (`plugins::hud::region_banner::overworld_region_id`: X in bits 0-7, Z in
/// 8-14, and Z=128 setting bit 15 for dungeons — see the module comment).
#[derive(Debug, Clone, Default)]
pub struct ZoneSoundTable {
    zones: Vec<ZoneSound>,
    sectors: HashMap<u16, Vec<SectorClaim>>,
    /// Zone names present in one file but not the other, for a one-line log.
    pub unmatched_zones: Vec<String>,
}

#[allow(dead_code)]
impl ZoneSoundTable {
    /// Parse `regioninfo.txt` (already decoded) joined with `effectenvsnd.txt`.
    pub fn parse(regioninfo: &str, effectenvsnd: &str) -> Self {
        let mut sounds = parse_effectenvsnd(effectenvsnd);
        let mut zones: Vec<ZoneSound> = Vec::new();
        let mut sectors: HashMap<u16, Vec<SectorClaim>> = HashMap::new();
        let mut unmatched_zones = Vec::new();

        for line in regioninfo.lines() {
            let fields: Vec<&str> = line.split('\t').map(str::trim).collect();
            let first = fields.first().copied().unwrap_or_default();
            if first.is_empty() {
                continue;
            }
            if let Some(kind) = zone_kind(first) {
                let Some(name) = fields.get(1).filter(|n| !n.is_empty()) else {
                    continue;
                };
                let sound = sounds.remove(*name);
                if sound.is_none() {
                    unmatched_zones.push((*name).to_string());
                }
                let (bgm, day, night) = sound.unwrap_or_default();
                zones.push(ZoneSound {
                    name: (*name).to_string(),
                    kind,
                    code: fields
                        .get(2)
                        .filter(|c| !c.is_empty())
                        .map(|c| (*c).to_string()),
                    bgm,
                    day,
                    night,
                });
                continue;
            }
            // A sector row before any header has no zone to belong to.
            let Some(zone) = zones.len().checked_sub(1) else {
                continue;
            };
            let (Some(x), Some(z)) = (
                first.parse::<u16>().ok(),
                fields.get(1).and_then(|f| f.parse::<u16>().ok()),
            ) else {
                continue;
            };
            let Some(region_id) = pack_region_id(x, z) else {
                continue;
            };
            let rect = match fields.get(2).copied() {
                Some("ALL") => None,
                Some("RECT") => match parse_rect(&fields) {
                    Some(rect) => Some(rect),
                    None => continue,
                },
                _ => continue,
            };
            sectors
                .entry(region_id)
                .or_default()
                .push(SectorClaim { zone, rect });
        }

        // Whatever is left over keys off a zone name regioninfo never names.
        unmatched_zones.extend(sounds.into_keys());
        unmatched_zones.sort();

        ZoneSoundTable {
            zones,
            sectors,
            unmatched_zones,
        }
    }

    pub fn zones(&self) -> &[ZoneSound] {
        &self.zones
    }

    /// The zone at a position: `RECT` claims win over the `ALL` background of
    /// the same sector (see the module comment). `local_x`/`local_z` are
    /// sector-local SRO units (0..=[`SECTOR_SIZE`]).
    pub fn zone_at(&self, region_id: u16, local_x: f32, local_z: f32) -> Option<&ZoneSound> {
        let claims = self.sectors.get(&region_id)?;
        let hit = claims
            .iter()
            .find(|c| c.rect.is_some_and(|r| r.contains(local_x, local_z)))
            .or_else(|| claims.iter().find(|c| c.rect.is_none()))?;
        self.zones.get(hit.zone)
    }

    /// The zone owning a whole sector, ignoring the in-sector bounds — for
    /// callers that only have a region id (a region-change event, a test).
    /// Prefers the `ALL` claim, which is the sector's background zone.
    pub fn zone_for_region(&self, region_id: u16) -> Option<&ZoneSound> {
        let claims = self.sectors.get(&region_id)?;
        let hit = claims
            .iter()
            .find(|c| c.rect.is_none())
            .or_else(|| claims.first())?;
        self.zones.get(hit.zone)
    }
}

fn zone_kind(field: &str) -> Option<ZoneKind> {
    match field {
        "#TOWN" => Some(ZoneKind::Town),
        "#FIELD" => Some(ZoneKind::Field),
        _ => None,
    }
}

/// X in bits 0-7, Z in bits 8-14; Z=128 is the dungeon flag (bit 15).
fn pack_region_id(x: u16, z: u16) -> Option<u16> {
    if x > 255 || z > 128 {
        return None;
    }
    Some((z << 8) | x)
}

fn parse_rect(fields: &[&str]) -> Option<SectorRect> {
    let v: Vec<f32> = fields
        .get(3..7)?
        .iter()
        .filter_map(|f| f.parse::<f32>().ok())
        .collect();
    let [x_min, z_min, x_max, z_max] = v[..] else {
        return None;
    };
    if [x_min, z_min, x_max, z_max]
        .iter()
        .any(|c| !(0.0..=SECTOR_SIZE).contains(c))
    {
        return None;
    }
    Some(SectorRect {
        x_min,
        z_min,
        x_max,
        z_max,
    })
}

type ZoneSounds = (Option<String>, Vec<Ambient>, Vec<Ambient>);

/// `<1>` zone / bare `"track.ogg"` / `<2>` day-night section / `<3>` ambient.
fn parse_effectenvsnd(content: &str) -> HashMap<String, ZoneSounds> {
    let mut out: HashMap<String, ZoneSounds> = HashMap::new();
    let mut zone: Option<String> = None;
    let mut when = TimeOfDay::Day;

    for line in content.lines() {
        let fields: Vec<&str> = line
            .split('\t')
            .map(str::trim)
            .filter(|f| !f.is_empty())
            .collect();
        let Some(first) = fields.first().copied() else {
            continue;
        };
        match first {
            "<1>" => {
                zone = fields.get(1).map(|n| (*n).to_string());
                if let Some(name) = &zone {
                    out.entry(name.clone()).or_default();
                }
                when = TimeOfDay::Day;
            }
            "<2>" => {
                // 낮 = day, 밤 = night; anything else keeps the current section.
                when = match fields.get(1).copied() {
                    Some("낮") => TimeOfDay::Day,
                    Some("밤") => TimeOfDay::Night,
                    _ => when,
                };
            }
            "<3>" => {
                let (Some(zone), Some(file), Some(interval)) =
                    (zone.as_ref(), fields.get(1), fields.get(2))
                else {
                    continue;
                };
                let file = file.trim_matches('"').trim();
                let Some((min, max)) = interval.split_once('~') else {
                    continue;
                };
                let (Ok(min_secs), Ok(max_secs)) = (min.trim().parse(), max.trim().parse()) else {
                    continue;
                };
                let ambient = Ambient {
                    file: file.to_string(),
                    min_secs,
                    max_secs,
                };
                let entry = out.entry(zone.clone()).or_default();
                match when {
                    TimeOfDay::Day => entry.1.push(ambient),
                    TimeOfDay::Night => entry.2.push(ambient),
                }
            }
            // The BGM is the only bare quoted field, on the line after `<1>`.
            _ if first.starts_with('"') => {
                if let Some(zone) = zone.as_ref() {
                    out.entry(zone.clone()).or_default().0 =
                        Some(first.trim_matches('"').trim().to_string());
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod test {
    use super::*;

    /// Real rows, verbatim from the user's `Media.pk2` (2026-08-16): the
    /// Jangan town block, the Donwhang dungeon `#FIELD` (sector Z=128), and
    /// the Alexandria/Delta pair that shares sector (49,90) as ALL + RECT.
    const REGIONINFO: &str = "#TOWN\t장안\t\t\t\t\t\r\n\
        167\t96\tRECT\t0\t1600\t1920\t1920\r\n\
        167\t97\tALL\t\t\t\t\r\n\
        \t\t\t\t\t\t\r\n\
        #FIELD\t돈황던젼\tdonwhang\t\t\t\t\r\n\
        1\t128\tALL\t\t\t\t\r\n\
        #TOWN\t알렉산드리아\t\t\t\t\t\r\n\
        49\t90\tALL\t\t\t\t\r\n\
        #FIELD\t델타지역\t\t\t\t\t\r\n\
        49\t90\tRECT\t480\t0\t1920\t640\r\n";

    const EFFECTENVSND: &str = "<1>\t장안\t\t\t\t\r\n\
        \t\"Jangan_Town.ogg\"\t\t\t\t\r\n\
        \t<2>\t낮\t\t\t\r\n\
        \t\t\t<3>\t\"night_wind.wav\"\t0~0\r\n\
        \t\t\t<3>\t\"donhwang_wind04.wav\"\t15~40\r\n\
        \t<2>\t밤\t\t\t\r\n\
        \t\t\t<3>\t\"day_wind.wav\"\t0~0\r\n\
        \t\t\t\t\t\r\n\
        <1>\t돈황던젼\t\t\t\t\r\n\
        \t\"Donwhang_Dungeon.ogg\"\t\t\t\t\r\n\
        \t<2>\t낮\t\t\t\r\n\
        \t\t\t<3>\t\"dd_wind_01.wav\"  \t10~15\r\n";

    fn table() -> ZoneSoundTable {
        ZoneSoundTable::parse(REGIONINFO, EFFECTENVSND)
    }

    #[test]
    fn joins_hangul_zone_name_to_its_bgm_and_ambients() {
        let table = table();
        // 167 + (97 << 8) — the packing of `overworld_region_id`.
        let jangan = table.zone_for_region((97 << 8) | 167).unwrap();
        assert_eq!(jangan.name, "장안");
        assert_eq!(jangan.kind, ZoneKind::Town);
        assert_eq!(jangan.code, None);
        assert_eq!(jangan.bgm.as_deref(), Some("Jangan_Town.ogg"));
        assert_eq!(
            jangan.bgm_asset_path().as_deref(),
            Some("music://jangan_town.ogg")
        );
        assert_eq!(
            jangan.ambients(TimeOfDay::Day),
            &[
                Ambient {
                    file: "night_wind.wav".into(),
                    min_secs: 0,
                    max_secs: 0
                },
                Ambient {
                    file: "donhwang_wind04.wav".into(),
                    min_secs: 15,
                    max_secs: 40
                },
            ]
        );
        assert!(jangan.ambients(TimeOfDay::Day)[0].is_continuous());
        assert!(!jangan.ambients(TimeOfDay::Day)[1].is_continuous());
        assert_eq!(jangan.ambients(TimeOfDay::Night).len(), 1);
        assert_eq!(jangan.ambients(TimeOfDay::Night)[0].file, "day_wind.wav");
    }

    #[test]
    fn rect_row_is_honoured_and_beats_the_all_claim_of_the_same_sector() {
        let table = table();
        let jangan_rect = (96 << 8) | 167;
        // The Jangan RECT covers z 1600..1920 of sector (167,96) only.
        assert_eq!(
            table.zone_at(jangan_rect, 500.0, 1700.0).unwrap().name,
            "장안"
        );
        assert!(table.zone_at(jangan_rect, 500.0, 100.0).is_none());

        // Sector (49,90): Alexandria owns all of it, Delta carves out a RECT.
        let shared = (90 << 8) | 49;
        assert_eq!(
            table.zone_at(shared, 100.0, 100.0).unwrap().name,
            "알렉산드리아"
        );
        assert_eq!(
            table.zone_at(shared, 1000.0, 300.0).unwrap().name,
            "델타지역"
        );
        // The region-id-only lookup takes the ALL claim, the sector background.
        assert_eq!(table.zone_for_region(shared).unwrap().name, "알렉산드리아");
    }

    #[test]
    fn sector_z_128_packs_to_the_dungeoninfo_region_id() {
        let table = table();
        // `dungeoninfo.txt` id 1 = Dungeon\wchina\Dunhwang_Cv.dof.
        let dungeon = table.zone_for_region(0x8001).unwrap();
        assert_eq!(dungeon.name, "돈황던젼");
        assert_eq!(dungeon.code.as_deref(), Some("donwhang"));
        assert_eq!(dungeon.bgm.as_deref(), Some("Donwhang_Dungeon.ogg"));
        // Trailing spaces inside the quoted `<3>` name are stripped.
        assert_eq!(dungeon.ambients(TimeOfDay::Day)[0].file, "dd_wind_01.wav");
        assert_eq!(pack_region_id(1, 128), Some(0x8001));
        assert_eq!(pack_region_id(256, 96), None);
    }

    #[test]
    fn zones_without_a_sound_entry_are_reported_not_dropped() {
        let table = table();
        assert_eq!(table.zones().len(), 4);
        // 알렉산드리아 and 델타지역 have no effectenvsnd block in this excerpt.
        assert_eq!(table.unmatched_zones, vec!["델타지역", "알렉산드리아"]);
        let silent = table.zones().iter().find(|z| z.name == "델타지역").unwrap();
        assert!(silent.bgm.is_none());
        assert!(silent.bgm_asset_path().is_none());
    }
}
