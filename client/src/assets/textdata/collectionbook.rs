//! Collection-book tables (`collectionbook_theme.txt` + `collectionbook_item.txt`).
//!
//! Idea: the talisman collection book has no server-side content at all — the
//! whole book ships in two textdata tables that the original's own loader reads
//! together, so they are parsed together here (the theme file is the entry
//! point and pulls its item sibling in, the way the shop/teleport chains do).
//!
//! ```text
//! theme: 1  1  THEME_GOD_TOGUI_RED_BLOOD  <KR>  SN_…  SN_…_TT_DESC  8
//! item:  1  ITEM_TALISMAN_TOGUI_RED_TEARS  <KR>  THEME_GOD_TOGUI_RED_BLOOD  1  1
//!           SN_…_TT_DESC_STORY  icon\item\etc\talisman_togui_red_te.ddj
//! ```
//!
//! Theme columns: service flag · theme index · **code** · Korean name · name key
//! · description key · item count. Item columns: service flag · **code** ·
//! Korean name · **theme code** · theme index · **slot index (1-based)** · story
//! key · icon.
//!
//! The two tables are self-consistent and that is what settles the layout
//! question: 4 themes × 8 items = 32 rows exactly, so the window's 4×2 grid of
//! 88×128 slots is *one theme's complete set*, not a scrolled view.
//!
//! Codes are the tables' own **strings** — the original has no numeric theme id,
//! so none is invented here. All display strings resolve through the `SN_*` keys
//! against `textdata_object.txt`, which openroad already loads as part of the
//! `textdataname.txt` shard set.

/// One collection theme (a page of the book).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionTheme {
    /// 1-based authored order.
    pub index: u32,
    /// `THEME_*` code — the identity the item rows join on.
    pub code: String,
    /// `SN_*` key of the theme's display name.
    pub name_key: String,
    /// `SN_*_TT_DESC` key of its description.
    pub desc_key: String,
    /// Items in the theme; authored 8 for all four.
    pub item_count: u32,
}

/// One talisman.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionItem {
    pub code: String,
    /// `THEME_*` code this talisman belongs to.
    pub theme_code: String,
    /// **1-based** slot within the theme's grid.
    pub slot: u32,
    /// `SN_*_TT_DESC_STORY` key of the story text.
    pub story_key: String,
    /// Icon path, media-relative (backslashes normalized).
    pub icon: String,
}

#[derive(Debug, Clone, Default)]
pub struct CollectionBookTable {
    pub themes: Vec<CollectionTheme>,
    pub items: Vec<CollectionItem>,
}

impl CollectionBookTable {
    pub fn parse(themes: &str, items: &str) -> Self {
        let themes = themes
            .lines()
            .filter_map(|line| {
                let f: Vec<&str> = line.trim_end_matches('\r').split('\t').collect();
                if f.len() < 7 || f[0].trim_start_matches('\u{feff}').trim() != "1" {
                    return None;
                }
                Some(CollectionTheme {
                    index: f[1].trim().parse().ok()?,
                    code: f[2].trim().to_string(),
                    name_key: f[4].trim().to_string(),
                    desc_key: f[5].trim().to_string(),
                    item_count: f[6].trim().parse().ok()?,
                })
            })
            .collect();
        let items = items
            .lines()
            .filter_map(|line| {
                let f: Vec<&str> = line.trim_end_matches('\r').split('\t').collect();
                if f.len() < 8 || f[0].trim_start_matches('\u{feff}').trim() != "1" {
                    return None;
                }
                Some(CollectionItem {
                    code: f[1].trim().to_string(),
                    theme_code: f[3].trim().to_string(),
                    slot: f[5].trim().parse().ok()?,
                    story_key: f[6].trim().to_string(),
                    icon: f[7].trim().replace('\\', "/"),
                })
            })
            .collect();
        Self { themes, items }
    }

    /// The theme's items in authored slot order (1-based slots, so index 0 is
    /// slot 1). Missing slots stay `None` rather than shifting the grid.
    pub fn theme_items(&self, theme: &CollectionTheme) -> Vec<Option<&CollectionItem>> {
        let mut slots = vec![None; theme.item_count as usize];
        for item in self.items.iter().filter(|i| i.theme_code == theme.code) {
            if let Some(cell) = slots.get_mut(item.slot.saturating_sub(1) as usize) {
                *cell = Some(item);
            }
        }
        slots
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// Real rows, byte-for-byte from the decoded tables (Korean columns kept).
    const THEMES: &str = "1\t1\tTHEME_GOD_TOGUI_RED_BLOOD\t토귀마을 - 붉은피의 환영\tSN_THEME_GOD_TOGUI_RED_BLOOD\tSN_THEME_GOD_TOGUI_RED_BLOOD_TT_DESC\t8\r\n\
                          1\t2\tTHEME_GOD_FLAME_BURNING_ABYSS\t화염산 - 불타는 심연\tSN_THEME_GOD_FLAME_BURNING_ABYSS\tSN_THEME_GOD_FLAME_BURNING_ABYSS_TT_DESC\t8";
    const ITEMS: &str = "1\tITEM_TALISMAN_TOGUI_RED_TEARS\t붉은 눈물\tTHEME_GOD_TOGUI_RED_BLOOD\t1\t1\tSN_ITEM_TALISMAN_TOGUI_RED_TEARS_TT_DESC_STORY\ticon\\item\\etc\\talisman_togui_red_te.ddj\r\n\
                         1\tITEM_TALISMAN_TOGUI_TOGUI_MASK\t토귀 가면\tTHEME_GOD_TOGUI_RED_BLOOD\t1\t3\tSN_ITEM_TALISMAN_TOGUI_TOGUI_MASK_TT_DESC_STORY\ticon\\item\\etc\\talisman_togui_togui_m.ddj\r\n\
                         1\tITEM_TALISMAN_FLAME_X\t불\tTHEME_GOD_FLAME_BURNING_ABYSS\t2\t1\tSN_X_TT_DESC_STORY\ticon\\item\\etc\\x.ddj";

    #[test]
    fn parses_themes_and_items_with_string_codes() {
        let table = CollectionBookTable::parse(THEMES, ITEMS);
        assert_eq!(table.themes.len(), 2);
        assert_eq!(table.items.len(), 3);

        let first = &table.themes[0];
        assert_eq!(first.code, "THEME_GOD_TOGUI_RED_BLOOD");
        assert_eq!(first.name_key, "SN_THEME_GOD_TOGUI_RED_BLOOD");
        assert_eq!(first.desc_key, "SN_THEME_GOD_TOGUI_RED_BLOOD_TT_DESC");
        assert_eq!(first.item_count, 8);
        // the icon path is normalized so it can be joined onto media://
        assert_eq!(
            table.items[0].icon,
            "icon/item/etc/talisman_togui_red_te.ddj"
        );
    }

    /// Slots are 1-based and a theme's grid is `item_count` long: a gap must
    /// stay a gap, or every talisman after it would show in the wrong cell.
    #[test]
    fn theme_items_are_placed_by_their_authored_slot() {
        let table = CollectionBookTable::parse(THEMES, ITEMS);
        let slots = table.theme_items(&table.themes[0]);
        assert_eq!(slots.len(), 8);
        assert_eq!(
            slots[0].map(|i| i.code.as_str()),
            Some("ITEM_TALISMAN_TOGUI_RED_TEARS")
        );
        // authored slot 3 lands in index 2, and slot 2 stays empty
        assert!(slots[1].is_none());
        assert_eq!(
            slots[2].map(|i| i.code.as_str()),
            Some("ITEM_TALISMAN_TOGUI_TOGUI_MASK")
        );
        // the other theme's item does not bleed in
        assert!(slots[3..].iter().all(|s| s.is_none()));
        assert_eq!(
            table.theme_items(&table.themes[1])[0].map(|i| i.slot),
            Some(1)
        );
    }
}
