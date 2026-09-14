//! UI string table (`textuisystem.txt`): maps `UIIT_*` (and `UIC_*`/`UIO_*`…)
//! keys referenced by resinfo layouts and other textdata tables to their
//! localized display strings. Rows are `enabled \t KEY \t <language columns>`;
//! the v1.188 data has the English text in the last (9th) column, so we take
//! the last non-empty column like the other string tables.

use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct UiSystemText(pub HashMap<String, String>);

impl UiSystemText {
    /// Parse the tab-separated file content (already decoded from UTF-16).
    pub fn parse(content: &str) -> Self {
        let entries = content
            .lines()
            .filter_map(|line| {
                let fields: Vec<&str> = line.split('\t').collect();
                if fields.len() < 3 || fields[0].trim_start_matches('\u{feff}') != "1" {
                    return None;
                }
                let key = fields[1].trim();
                if key.is_empty() {
                    return None;
                }
                let value = fields[2..]
                    .iter()
                    .rev()
                    .map(|f| f.trim())
                    .find(|f| !f.is_empty())?;
                Some((key.to_string(), value.to_string()))
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
    fn parses_ui_strings() {
        let content = "1\tUIIT_STT_INVENTORY\t\t\t\t\t\t\tInventory\r\n\
                       1\tUIIT_STT_WORLDMAP\t\t\t\t\t\t\tWorld map\r\n\
                       0\tUIIT_STT_DISABLED\t\t\t\t\t\t\tNope\r\n\
                       malformed";
        let strings = UiSystemText::parse(content);
        assert_eq!(strings.get("UIIT_STT_INVENTORY"), Some("Inventory"));
        assert_eq!(strings.get("UIIT_STT_WORLDMAP"), Some("World map"));
        assert_eq!(strings.get("UIIT_STT_DISABLED"), None);
        assert_eq!(strings.0.len(), 2);
    }
}
