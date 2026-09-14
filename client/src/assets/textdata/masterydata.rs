//! skillmasterydata.txt — the mastery reference table (id → name key, icons,
//! skill-window placement). A single UTF-16LE file, tab-separated, one row
//! per mastery. Column semantics verified against the v1.188 corpus:
//! col 0 id, 2 name key (`UIIT_STT_*`), 4 tooltip key, 5 category key
//! (`UIIT_CTL_WEAPON_SKILL`, `UIIT_CTL_EU_SKILLWND_PHYSICAL`, ...),
//! 6 window column, 7 window group (CH weapon = 0, CH force = 1/2, EU = 3),
//! 11/12 icon + focused icon. CH masteries are ids 257–276, GM is 289,
//! EU masteries are 513+.

use std::collections::HashMap;
use std::ops::Deref;

/// One skillmasterydata.txt row.
#[derive(Debug, Clone)]
pub struct MasteryInfo {
    pub id: u32,
    /// Parsed-but-unconsumed until textuisystem.txt (the `UIIT_*` string
    /// table) is loaded — the window falls back to hardcoded names.
    #[allow(dead_code)]
    pub name_key: String,
    /// Parsed-but-unconsumed until mastery tooltips land.
    #[allow(dead_code)]
    pub tooltip_key: String,
    /// Skill-window section header key (weapon/force/physical/...).
    /// Parsed-but-unconsumed until textuisystem.txt is loaded.
    #[allow(dead_code)]
    pub category_key: String,
    /// Column within the window group (CH: 0 = weapon, 1 = force).
    pub tab: u8,
    /// Window group / row block (CH weapon = 0, CH force = 1/2, EU = 3).
    pub group: u8,
    pub icon: Option<String>,
    /// Parsed-but-unconsumed until mastery selection highlighting lands.
    #[allow(dead_code)]
    pub focus_icon: Option<String>,
}

/// Which skill tree a mastery belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MasteryRace {
    Chinese,
    European,
    Gm,
}

impl MasteryInfo {
    pub fn race(&self) -> MasteryRace {
        match self.id {
            289 => MasteryRace::Gm,
            512.. => MasteryRace::European,
            _ => MasteryRace::Chinese,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct MasteryData(pub HashMap<u32, MasteryInfo>);

impl Deref for MasteryData {
    type Target = HashMap<u32, MasteryInfo>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

// Unlike skilldata's icon column, the path here already starts with `icon\`.
fn icon_path(raw: &str) -> Option<String> {
    let raw = raw.trim();
    (!raw.is_empty() && raw != "xxx")
        .then(|| format!("media://{}", raw.replace('\\', "/").to_lowercase()))
}

impl MasteryData {
    pub fn parse(content: &str) -> Self {
        let rows = content
            .lines()
            .map(|l| l.split('\t').map(str::trim).collect::<Vec<_>>())
            .filter(|row| row.len() > 12)
            .filter_map(|row| {
                let id = row[0].trim_start_matches('\u{feff}').parse::<u32>().ok()?;
                Some((
                    id,
                    MasteryInfo {
                        id,
                        name_key: row[2].to_string(),
                        tooltip_key: row[4].to_string(),
                        category_key: row[5].to_string(),
                        tab: row[6].parse().unwrap_or(0),
                        group: row[7].parse().unwrap_or(0),
                        icon: icon_path(row[11]),
                        focus_icon: icon_path(row[12]),
                    },
                ))
            })
            .collect();
        MasteryData(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rows_and_race_split() {
        let content = "\u{feff}257\t비천검법101\tUIIT_STT_MASTERY_VI\t10\tUIIT_STT_MASTERY_VI_EXPLANATION\tUIIT_CTL_WEAPON_SKILL\t0\t0\t2\t3\t0\ticon\\skillmastery\\china\\mastery_sword.ddj\ticon\\skillmastery\\china\\mastery_sword_focus.ddj\n\
            289\tGM\tUIIT_STT_MASTERY_GM\t4\tUIIT_STT_MASTERY_GM_EXPLANATION\tUIIT_STT_MASTERY_GM\t2\t2\t0\t0\t0\ticon\\a.ddj\ticon\\a.ddj\n\
            513\t워리어201\tUIIT_STT_WARRIOR\t12\tUIIT_STT_WARRIOR_TT_DESC\tUIIT_CTL_EU_SKILLWND_PHYSICAL\t0\t3\t7\t8\t9\ticon\\skillmastery\\europe\\eu_warrior.ddj\txxx\n\
            short\trow\n";
        let data = MasteryData::parse(content);
        assert_eq!(data.len(), 3);

        let sword = &data[&257];
        assert_eq!(sword.name_key, "UIIT_STT_MASTERY_VI");
        assert_eq!(sword.category_key, "UIIT_CTL_WEAPON_SKILL");
        assert_eq!(sword.tab, 0);
        assert_eq!(sword.group, 0);
        assert_eq!(
            sword.icon.as_deref(),
            Some("media://icon/skillmastery/china/mastery_sword.ddj")
        );
        assert_eq!(sword.race(), MasteryRace::Chinese);

        let warrior = &data[&513];
        assert_eq!(warrior.race(), MasteryRace::European);
        assert_eq!(warrior.group, 3);
        assert_eq!(warrior.focus_icon, None);

        assert_eq!(data[&289].race(), MasteryRace::Gm);
    }
}
