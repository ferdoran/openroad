//! Parser for `server_dep/silkroad/textdata/skillgroup.txt` (Media.pk2).
//!
//! Idea: this table is the authoritative display order of a mastery's skill
//! branches (the "series" rows in the skill window). Each row is one branch of
//! one mastery, with a **Row** column (col 4) giving its top-to-bottom order
//! and an icon (col 6, `skillgroup\<race>\pack_<concept>.ddj`) whose `concept`
//! stem is the only link back to the skills — skilldata carries no branch id.
//!
//! Skills join a branch by that concept: the branch concept (e.g. `sword_smash`,
//! `earth`) appears as a substring of the skill's `basic_group` codename
//! (`SKILL_CH_SWORD_SMASH_A`, `SKILL_EU_WIZARD_EARTHA_AREA_A`). Matching the
//! **longest** such concept within the mastery resolves the overlaps
//! (`sword_shield` vs `sword_shieldpd`, `stealth` vs `stealth_expert`).
//!
//! Tab-separated, UTF-16LE with BOM like the other row tables; `//` comments.

use bevy::prelude::Resource;
use std::collections::HashMap;

/// One branch of one mastery from skillgroup.txt.
#[derive(Debug, Clone)]
pub struct SkillGroupBranch {
    /// Lowercased icon stem (`pack_` and `.ddj` stripped) — the join key.
    pub concept: String,
    /// Display order within the mastery (col 4).
    pub row: u32,
    /// Branch icon asset path, `icon/skillgroup/<race>/pack_<concept>.ddj`
    /// (col 6, backslashes normalized).
    pub icon: String,
}

/// Parsed skillgroup.txt: per mastery, its branches in file order.
#[derive(Resource, Debug, Default, Clone)]
pub struct SkillGroupTable {
    by_mastery: HashMap<u32, Vec<SkillGroupBranch>>,
}

/// The icon-stem concept of an icon path, e.g.
/// `skillgroup\china\pack_sword_smash.ddj` → `sword_smash`. `None` for the
/// empty/`xxx` placeholders.
fn icon_concept(icon: &str) -> Option<String> {
    let base = icon.rsplit(['\\', '/']).next().unwrap_or(icon);
    let stem = base.strip_suffix(".ddj").unwrap_or(base);
    let concept = stem.strip_prefix("pack_").unwrap_or(stem).to_lowercase();
    (!concept.is_empty() && concept != "xxx").then_some(concept)
}

impl SkillGroupTable {
    pub fn parse(content: &str) -> Self {
        let mut by_mastery: HashMap<u32, Vec<SkillGroupBranch>> = HashMap::new();
        for line in content.lines() {
            let line = line
                .trim_end_matches(['\r', '\t'])
                .trim_start_matches('\u{feff}');
            if line.is_empty() || line.starts_with("//") {
                continue;
            }
            let row: Vec<&str> = line.split('\t').collect();
            let field = |i: usize| row.get(i).map(|s| s.trim()).unwrap_or("");
            if field(0) != "1" {
                continue;
            }
            let (Ok(mastery), Ok(order)) = (field(2).parse::<u32>(), field(4).parse::<u32>())
            else {
                continue;
            };
            let Some(concept) = icon_concept(field(6)) else {
                continue;
            };
            let icon = format!("icon/{}", field(6).replace('\\', "/"));
            by_mastery
                .entry(mastery)
                .or_default()
                .push(SkillGroupBranch {
                    concept,
                    row: order,
                    icon,
                });
        }
        Self { by_mastery }
    }

    /// The branch a skill series belongs to, matched by the longest branch
    /// concept that occurs in the series' `basic_group` (case-insensitive).
    /// `None` when no branch matches (rare oddball series).
    pub fn branch_for(&self, mastery: u32, basic_group: &str) -> Option<&SkillGroupBranch> {
        let key = basic_group.to_lowercase();
        self.by_mastery
            .get(&mastery)?
            .iter()
            .filter(|branch| key.contains(branch.concept.as_str()))
            .max_by_key(|branch| branch.concept.len())
    }

    /// Display row (col 4) of a skill series; `None` sorts it last.
    pub fn row_for(&self, mastery: u32, basic_group: &str) -> Option<u32> {
        self.branch_for(mastery, basic_group).map(|b| b.row)
    }

    pub fn len(&self) -> usize {
        self.by_mastery.values().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.by_mastery.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> SkillGroupTable {
        // mastery 257 (sword): the shield/shieldpd overlap; mastery 514
        // (wizard): element-only concepts with a class-prefixed basic_group.
        let content = "\u{feff}//\tSkillName\tSkillID\tPriority\tRefName\tRow\tUIIT\tIcon\n\
            1\t\t257\tref\t0\tU_VI_0\tskillgroup\\china\\pack_sword_smash.ddj\n\
            1\t\t257\tref\t2\tU_VI_2\tskillgroup\\china\\pack_sword_shield.ddj\n\
            1\t\t257\tref\t7\tU_VI_7\tskillgroup\\china\\pack_sword_shieldpd.ddj\n\
            1\t\t514\tref\t2\tU_EA_2\tskillgroup\\europe\\pack_earth.ddj\n\
            0\t\t257\tref\t9\tU_VI_9\tskillgroup\\china\\pack_sword_disabled.ddj\n";
        SkillGroupTable::parse(content)
    }

    #[test]
    fn maps_series_to_row() {
        let t = sample();
        assert_eq!(t.row_for(257, "SKILL_CH_SWORD_SMASH_A"), Some(0));
        // longest match wins: shieldpd must not collapse onto shield (row 2)
        assert_eq!(t.row_for(257, "SKILL_CH_SWORD_SHIELD_A"), Some(2));
        assert_eq!(t.row_for(257, "SKILL_CH_SWORD_SHIELDPD_A"), Some(7));
        // EU: element-only concept inside a class-prefixed codename
        assert_eq!(t.row_for(514, "SKILL_EU_WIZARD_EARTHA_AREA_A"), Some(2));
        // disabled row (col 0 != 1) skipped; unmatched series → None
        assert_eq!(t.row_for(257, "SKILL_CH_SWORD_KNOCKDOWN_A"), None);
        // authoritative icon path (col 6, backslashes normalized) of the match
        assert_eq!(
            t.branch_for(257, "SKILL_CH_SWORD_SHIELDPD_A")
                .map(|b| b.icon.as_str()),
            Some("icon/skillgroup/china/pack_sword_shieldpd.ddj")
        );
    }
}
