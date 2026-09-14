//! Display-name string table (`textdataname.txt` master -> `textdata_object` /
//! `textdata_equip&skill` shards): maps `SN_*` keys referenced by other
//! textdata tables (e.g. itemdata's NameStrID column) to localized strings.
//! We pick the English column (index 9 in our data), falling back to the last
//! non-empty column since the column count varies between locales.

use bevy::asset::Asset;
use bevy::prelude::TypePath;
use std::collections::HashMap;

#[derive(Asset, TypePath, Debug, Clone, Default)]
pub struct TextdataNames(pub HashMap<String, String>);

impl TextdataNames {
    pub fn parse(content: &str) -> Self {
        let entries = content
            .lines()
            .filter_map(|line| {
                let fields: Vec<&str> = line.split('\t').collect();
                // col 0 = enabled flag, col 1 = key, cols 2.. = languages
                if fields.len() < 3 || fields[0].trim_start_matches('\u{feff}') != "1" {
                    return None;
                }
                let key = fields[1].trim();
                if key.is_empty() || key == "0" {
                    return None;
                }
                let value = fields
                    .get(9)
                    .copied()
                    .filter(|v| !v.trim().is_empty())
                    .or_else(|| {
                        fields[2..]
                            .iter()
                            .rev()
                            .copied()
                            .find(|v| !v.trim().is_empty())
                    })?;
                Some((key.to_string(), value.trim().to_string()))
            })
            .collect();
        Self(entries)
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn parses_english_column() {
        let content =
            "1\tSN_MOB_THIEF_NPC\t\t\t\t\t\t\t\tThief\n1\tSN_ITEM_ETC_GOLD\t\t\t\t\t\t\t\tGold\n";
        let names = TextdataNames::parse(content);
        assert_eq!(names.get("SN_MOB_THIEF_NPC"), Some("Thief"));
        assert_eq!(names.get("SN_ITEM_ETC_GOLD"), Some("Gold"));
        assert_eq!(names.get("SN_MISSING"), None);
    }

    #[test]
    fn falls_back_to_last_non_empty_column() {
        // only 8 columns: no index-9 English column present
        let content = "1\tSN_SHORT\t\u{c774}\u{b984}\t\t\tName\t\t\n";
        let names = TextdataNames::parse(content);
        assert_eq!(names.get("SN_SHORT"), Some("Name"));
    }

    #[test]
    fn skips_disabled_and_malformed_rows() {
        let content =
            "0\tSN_DISABLED\t\t\t\t\t\t\t\tNope\ngarbage line\n1\tSN_EMPTY\t\t\t\t\t\t\t\t\n";
        let names = TextdataNames::parse(content);
        assert!(names.0.is_empty());
    }
}
