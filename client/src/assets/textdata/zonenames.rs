//! Area names from `textzonename.txt`: maps a region id (the decimal `u16`
//! whose high byte is the region z and low byte the region x, e.g. 25000 =
//! 0x61A8 = region 168x97 "Jangan") to its localized display name. Rows carry
//! several language columns; we take the last non-empty one (English in the
//! v1.188 data).

use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct ZoneNames(pub HashMap<u16, String>);

impl ZoneNames {
    /// Parse the tab-separated file content (already decoded from UTF-16).
    pub fn parse(content: &str) -> Self {
        let map = content
            .lines()
            .filter_map(parse_line)
            .collect::<HashMap<_, _>>();
        ZoneNames(map)
    }

    /// Display name for a region id, if the data names it.
    pub fn name(&self, region: u16) -> Option<&str> {
        self.0.get(&region).map(String::as_str)
    }
}

/// One row: `service \t region-id \t <language columns...>`. Skips disabled
/// (service != 1) and unnamed rows. Dungeon region ids appear as negative
/// numbers (the region's high bit set), hence the wrapping i32 -> u16 cast.
fn parse_line(line: &str) -> Option<(u16, String)> {
    let fields: Vec<&str> = line.split('\t').collect();
    if *fields.first()? != "1" {
        return None;
    }
    let region = fields.get(1)?.trim().parse::<i32>().ok()? as u16;
    let name = fields[2..]
        .iter()
        .rev()
        .map(|f| f.trim())
        .find(|f| !f.is_empty())?;
    Some((region, name.to_string()))
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn parses_region_rows() {
        let content = "1\t25000\t\t\t\t\t\t\t\tJangan\r\n0\t25001\t\t\t\t\t\t\t\tDisabled\r\n1\t-32767\t\t\t\t\t\t\t\tDungeon\r\nmalformed line";
        let names = ZoneNames::parse(content);
        assert_eq!(names.name(25000), Some("Jangan"));
        // service 0 rows are skipped
        assert_eq!(names.name(25001), None);
        // negative (dungeon) ids wrap into the u16 region id space
        assert_eq!(names.name((-32767i32) as u16), Some("Dungeon"));
        assert_eq!(names.0.len(), 2);
    }
}
