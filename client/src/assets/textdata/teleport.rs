//! Teleporter network, from `teleportdata.txt` + `teleportlink.txt`.
//!
//! Idea: teleportdata declares every teleporter (an id, the characterdata ref
//! id of the NPC or gate building that owns it, an `SN_ZONE_*` display key
//! and the arrival region/position); teleportlink declares the directed
//! source → destination edges with an optional level gate. The table is
//! keyed both ways: owner ref id → teleporter (to find the teleporter of a
//! clicked NPC or gate) and teleporter id → info (to name the destinations).
//! Gate *buildings* have no characterdata rows — their codenames and display
//! names come from teleportbuilding.txt (`buildings`), and the spawn path
//! turns their spawn records into invisible click anchors.
//!
//! Corpus notes (v1.188 Media): teleportlink has 23 columns and **col 3 is
//! the gold fee** — an earlier version of this note said the table carried no
//! fee at all because cols 4-6 (which *are* all-zero) were read one place to
//! the left. 45 of 231 rows are priced, and the prices are semantically
//! obvious: every continent-gate hop is 5,000 (`GATE_CH`↔`GATE_WC`↔`GATE_KT`↔
//! `GATE_CA`↔`GATE_EU`), every ferry/flyship/tunnel hop is 500, and the two
//! `GATE_SD*` rows charge 40,000 to Jangan / 30,000 to Hotan. Col 7 is a link type code
//! (2×255, 0×59, 1×50, 4×1); only type 1 rows carry a level range in cols
//! 8/9 (min/max, min 0 = ungated). 114 of 260 teleportdata rows use owner
//! ref 0 (dungeon/arena gates) — indexed by nobody, or they'd all collide on
//! the same key.

use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct TeleportTable {
    /// Owner (NPC/building) characterdata ref id → teleporter id.
    pub by_owner_ref: HashMap<i32, u32>,
    pub info: HashMap<u32, TeleportInfo>,
    /// Source teleporter id → outgoing links.
    pub links: HashMap<u32, Vec<TeleportLink>>,
    /// Gate-building ref id → building row (teleportbuilding.txt — these
    /// refs have no characterdata rows; the spawn path names them from here
    /// and the dialog resolves npcchat speech via the codename).
    pub buildings: HashMap<i32, GateBuilding>,
}

#[derive(Debug, Clone)]
pub struct TeleportInfo {
    pub codename: String,
    /// `SN_ZONE_*` display key (textdataname).
    pub name_key: String,
    /// Owning NPC/building characterdata ref; 0 for standalone circle-area
    /// gates (dungeon/arena entrances).
    pub owner_ref: i32,
    /// Arrival/placement region. Negative textdata values (dungeon regions,
    /// bit 15) wrap into the u16 id space like `zonenames.rs` does.
    pub region: u16,
    /// Placement/arrival point, region-local for overworld rows and raw
    /// dungeon-local for dungeon rows.
    pub position: bevy::math::Vec3,
    /// Trigger radius of the circle area on the ground.
    pub radius: f32,
}

#[derive(Debug, Clone)]
pub struct GateBuilding {
    /// `STORE_CH_GATE`-style codename — the npcchat/speech lookup key.
    pub codename: String,
    /// `SN_NPC_*_GATE` display key.
    pub name_key: String,
}

#[derive(Debug, Clone)]
pub struct TeleportLink {
    pub destination: u32,
    /// Minimum level, when the link declares one (type-1 rows, min > 0).
    pub min_level: Option<u16>,
    /// Gold charged for the hop (`teleportlink.txt` col 3), `0` = free.
    /// The board renders it through `UIIT_CTL_TELEPORT_RESULT` and picks
    /// `_FREE_RESULT` when this is 0.
    pub fee: u64,
}

impl TeleportTable {
    /// `teleportdata.txt`: `service|id|codename|owner_ref|name_strid|region|x|y|z|…`
    /// `teleportlink.txt`: `service|source|dest|0…|type|min|max|…`
    /// `teleportbuilding.txt`: `service|ref id|codename|kr|xxx|name_strid|…`
    pub fn parse(data: &str, links: &str, buildings: &str) -> Self {
        let mut table = TeleportTable::default();
        for line in buildings.lines() {
            let fields: Vec<&str> = line.split('\t').collect();
            if fields.len() < 6 || fields[0].trim_start_matches('\u{feff}') != "1" {
                continue;
            }
            if let Ok(owner) = fields[1].parse::<i32>() {
                table.buildings.insert(
                    owner,
                    GateBuilding {
                        codename: fields[2].to_string(),
                        name_key: fields[5].to_string(),
                    },
                );
            }
        }
        for line in data.lines() {
            let fields: Vec<&str> = line.split('\t').collect();
            if fields.len() < 9 || fields[0].trim_start_matches('\u{feff}') != "1" {
                continue;
            }
            let (Ok(id), Ok(owner)) = (fields[1].parse::<u32>(), fields[3].parse::<i32>()) else {
                continue;
            };
            let region = fields[5].parse::<i32>().unwrap_or(0) as u16;
            if owner != 0 {
                table.by_owner_ref.insert(owner, id);
            }
            let coord = |i: usize| {
                fields
                    .get(i)
                    .and_then(|f| f.parse::<f32>().ok())
                    .unwrap_or(0.0)
            };
            table.info.insert(
                id,
                TeleportInfo {
                    codename: fields[2].to_string(),
                    name_key: fields[4].to_string(),
                    owner_ref: owner,
                    region,
                    position: bevy::math::Vec3::new(coord(6), coord(7), coord(8)),
                    radius: coord(9),
                },
            );
        }
        for line in links.lines() {
            let fields: Vec<&str> = line.split('\t').collect();
            if fields.len() < 10 || fields[0].trim_start_matches('\u{feff}') != "1" {
                continue;
            }
            let (Ok(source), Ok(destination)) =
                (fields[1].parse::<u32>(), fields[2].parse::<u32>())
            else {
                continue;
            };
            let min_level = (fields[7] == "1")
                .then(|| fields[8].parse().ok().filter(|&min| min > 0))
                .flatten();
            let fee = fields[3].trim().parse::<u64>().unwrap_or(0);
            table.links.entry(source).or_default().push(TeleportLink {
                destination,
                min_level,
                fee,
            });
        }
        table
    }

    /// The outgoing destinations of the teleporter owned by `owner_ref`.
    /// `None` when the ref owns no teleporter OR the teleporter has no
    /// outgoing links (7 real teleporters are link-less — offering them a
    /// teleport line would open an empty window).
    pub fn destinations(&self, owner_ref: i32) -> Option<(u32, &[TeleportLink])> {
        let id = *self.by_owner_ref.get(&owner_ref)?;
        let links = self.links.get(&id).map(Vec::as_slice).unwrap_or(&[]);
        (!links.is_empty()).then_some((id, links))
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn parses_teleporters_and_links() {
        let data = "1\t1\tGATE_CH\t2094\tSN_ZONE_22001\t25000\t969\t0\t1369\t150\t1\t0\t1\n\
                    1\t3\tGATE_NPC_CH_FERRY\t2011\tSN_ZONE_21002\t24993\t560\t140\t1460\t200\t0\t0\t1\n\
                    1\t8\tGATE_TD\t2197\tSN_ZONE_TD\t23962\t0\t0\t0\t0\t0\t0\t1\n\
                    1\t9\tGATE_JINSI\t0\tSN_ZONE_JINSI\t0\t0\t0\t0\t0\t0\t0\t1\n";
        // real 23-column rows: type 2 (plain, priced 5000), type 1 with min 0
        // (ungated, free), type 1 with min 90, and a disabled row
        let plain = "\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0";
        let links = format!(
            "1\t1\t3\t5000\t0\t0\t0\t2\t0\t0{plain}\n\
             1\t1\t8\t0\t0\t0\t0\t1\t0\t999{plain}\n\
             1\t1\t175\t500\t0\t0\t0\t1\t90\t999{plain}\n\
             0\t1\t9\t0\t0\t0\t0\t2\t0\t0{plain}\n"
        );
        let buildings = "1\t2094\tSTORE_CH_GATE\t????\txxx\tSN_NPC_CH_GATE\txxx\t0\n";
        let table = TeleportTable::parse(data, &links, buildings);
        let gate = table.buildings.get(&2094).unwrap();
        assert_eq!(gate.codename, "STORE_CH_GATE");
        assert_eq!(gate.name_key, "SN_NPC_CH_GATE");
        let (id, dests) = table.destinations(2094).unwrap();
        assert_eq!(id, 1);
        assert_eq!(dests.len(), 3);
        assert_eq!(dests[0].destination, 3);
        assert_eq!(dests[0].min_level, None);
        // type-1 with min 0 must NOT surface a "[Lv 0+]" gate
        assert_eq!(dests[1].min_level, None);
        assert_eq!(dests[2].min_level, Some(90));
        // col 3 is the gold fee, not another zero column: 5,000 for the
        // continent hop, free for the second link, 500 for the gated one
        assert_eq!(dests[0].fee, 5000);
        assert_eq!(dests[1].fee, 0);
        assert_eq!(dests[2].fee, 500);
        assert_eq!(table.info[&3].name_key, "SN_ZONE_21002");
        // position + radius parse (row 1: 969/0/1369, r=150)
        let info = &table.info[&1];
        assert_eq!(info.position, bevy::math::Vec3::new(969.0, 0.0, 1369.0));
        assert_eq!(info.radius, 150.0);
        assert_eq!(info.owner_ref, 2094);
        assert_eq!(table.info[&9].owner_ref, 0);
        // disabled link rows are skipped
        assert!(!dests.iter().any(|link| link.destination == 9));
        // a teleporter with no outgoing links offers no destinations
        assert!(table.destinations(2197).is_none());
        // owner ref 0 rows (dungeon gates) never index a teleporter
        assert!(table.destinations(0).is_none());
    }
}
