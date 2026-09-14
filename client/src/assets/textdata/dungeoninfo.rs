//! Dungeon id → `.dof` path table from `Data.pk2:dungeon/dungeoninfo.txt`
//! (note: `data://`, not the usual `media://` textdata tree). One row:
//! `service \t dungeon-id \t "Dungeon\path\file.dof"`. The dungeon's region
//! id is `0x8000 | dungeon-id` (bit 15 = dungeon flag); ids collide in the
//! DOF headers themselves, so this table is the authoritative mapping.

use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct DungeonEntry {
    pub id: u16,
    /// `0x8000 | id`.
    pub region_id: u16,
    /// Path as written in the file, e.g. `Dungeon\wchina\Dunhwang_Cv.dof`.
    pub dof_path: String,
}

impl DungeonEntry {
    /// The `data://` asset path for this dungeon's `.dof` (PK2 lookups are
    /// case-insensitive; slashes normalized).
    pub fn asset_path(&self) -> String {
        format!("data://{}", self.dof_path.replace('\\', "/").to_lowercase())
    }

    /// Short display name (`dunhwang_cv`), for pickers.
    pub fn name(&self) -> &str {
        let file = self.dof_path.rsplit('\\').next().unwrap_or(&self.dof_path);
        file.strip_suffix(".dof").unwrap_or(file)
    }
}

/// Keyed by dungeon id, iteration ordered by id.
#[derive(Debug, Clone, Default)]
pub struct DungeonInfo(pub BTreeMap<u16, DungeonEntry>);

impl DungeonInfo {
    pub fn parse(content: &str) -> Self {
        let map = content
            .lines()
            .filter_map(parse_line)
            .map(|e| (e.id, e))
            .collect();
        DungeonInfo(map)
    }

    pub fn by_region(&self, region_id: u16) -> Option<&DungeonEntry> {
        self.0.get(&(region_id & 0x7FFF))
    }

    pub fn entries(&self) -> impl Iterator<Item = &DungeonEntry> {
        self.0.values()
    }
}

fn parse_line(line: &str) -> Option<DungeonEntry> {
    let fields: Vec<&str> = line.split('\t').collect();
    if *fields.first()? != "1" {
        return None;
    }
    let id = fields.get(1)?.trim().parse::<u16>().ok()?;
    let dof_path = fields.get(2)?.trim().trim_matches('"');
    if dof_path.is_empty() {
        return None;
    }
    Some(DungeonEntry {
        id,
        region_id: 0x8000 | id,
        dof_path: dof_path.to_string(),
    })
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn parses_rows() {
        let content = "1\t1\t\"Dungeon\\wchina\\Dunhwang_Cv.dof\"\r\n\
                       1\t7\t\"Dungeon\\china\\jinsi_floor01.dof\"\r\n\
                       0\t9\t\"Dungeon\\wchina\\event.dof\"\r\n\
                       malformed";
        let info = DungeonInfo::parse(content);
        assert_eq!(info.0.len(), 2);

        let dh = info.by_region(0x8001).unwrap();
        assert_eq!(dh.id, 1);
        assert_eq!(dh.region_id, 0x8001);
        assert_eq!(dh.dof_path, r"Dungeon\wchina\Dunhwang_Cv.dof");
        assert_eq!(dh.asset_path(), "data://dungeon/wchina/dunhwang_cv.dof");
        assert_eq!(dh.name(), "Dunhwang_Cv");

        // -32761 as u16 = 0x8007 = jinsi_floor01
        let jinsi = info.by_region((-32761i32) as u16).unwrap();
        assert_eq!(jinsi.name(), "jinsi_floor01");

        // service-0 rows are skipped
        assert!(info.by_region(0x8009).is_none());
        // ordered iteration by id
        let ids: Vec<u16> = info.entries().map(|e| e.id).collect();
        assert_eq!(ids, vec![1, 7]);
    }
}
