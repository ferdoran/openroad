//! Quest reward tables: `refqusetreward.txt` (note the original's typo) and
//! `refquestrewarditems.txt`.
//!
//! Idea: the reward UI's mode is a **data** switch, not a UI variant. Column 7
//! of `refqusetreward.txt` is `SelectionCnt`: `0` in 678 of the 695 data rows
//! (receive every listed item) and `1` in 17 (pick exactly one); the corpus
//! never carries a value above 1. `refquestrewarditems.txt` then lists the
//! per-quest reward rows — for a `SelectionCnt=1` quest those rows are the
//! candidate pool (at most 14 in this corpus), for a `SelectionCnt=0` quest
//! they are all granted.
//!
//! Only the three columns the doc pins are consumed: quest id, item codename
//! and the count. Neither file has a header row, the other columns of
//! `refqusetreward.txt` (gold/exp-looking numbers, `xxx` placeholders) are
//! unlabelled in the data, so they are deliberately not interpreted here.
//! `refquestrewarditems.txt` also carries the literal comment line
//! `//DBtoMedia`, which is why a line count of that file is one more than its
//! row count (`docs/re/ui/quest-reward-window.md` §3).

use std::collections::HashMap;

/// `SelectionCnt` column index in `refqusetreward.txt`.
const SELECTION_CNT_COL: usize = 7;
/// `refquestrewarditems.txt`: quest id, item codename, count.
const ITEM_QUEST_COL: usize = 0;
const ITEM_CODE_COL: usize = 3;
const ITEM_COUNT_COL: usize = 7;

/// Per-quest `SelectionCnt` (`true` = the player picks exactly one).
#[derive(Debug, Clone, Default)]
pub struct QuestRewardModes(pub HashMap<u32, bool>);

/// Per-quest reward rows, in file order.
#[derive(Debug, Clone, Default)]
pub struct QuestRewardItems(pub HashMap<u32, Vec<QuestRewardItem>>);

#[derive(Debug, Clone, PartialEq)]
pub struct QuestRewardItem {
    pub codename: String,
    /// Column 7. `1` for every pick-one candidate in this corpus and larger
    /// for stacked consumables (28 HP potions on `QNO_CH_POTION_1`), so it
    /// reads as a quantity — **[S]**, the column is unnamed in the data.
    pub count: u32,
}

impl QuestRewardModes {
    pub fn parse(content: &str) -> Self {
        let mut map = HashMap::new();
        for line in content.lines() {
            let cols: Vec<&str> = line.split('\t').collect();
            let Some(quest) = cols.first().and_then(|c| c.trim().parse::<u32>().ok()) else {
                continue;
            };
            let Some(selection) = cols
                .get(SELECTION_CNT_COL)
                .and_then(|c| c.trim().parse::<u32>().ok())
            else {
                continue;
            };
            map.insert(quest, selection > 0);
        }
        QuestRewardModes(map)
    }

    /// `true` when the quest's reward is a choose-one pick.
    pub fn choose_one(&self, quest: u32) -> bool {
        self.0.get(&quest).copied().unwrap_or(false)
    }
}

impl QuestRewardItems {
    pub fn parse(content: &str) -> Self {
        let mut map: HashMap<u32, Vec<QuestRewardItem>> = HashMap::new();
        for line in content.lines() {
            let cols: Vec<&str> = line.split('\t').collect();
            // the `//DBtoMedia` comment line has one column and no quest id
            let Some(quest) = cols
                .get(ITEM_QUEST_COL)
                .and_then(|c| c.trim().parse::<u32>().ok())
            else {
                continue;
            };
            let Some(codename) = cols.get(ITEM_CODE_COL).map(|c| c.trim()) else {
                continue;
            };
            if codename.is_empty() || codename == "xxx" {
                continue;
            }
            let count = cols
                .get(ITEM_COUNT_COL)
                .and_then(|c| c.trim().parse::<u32>().ok())
                .unwrap_or(1);
            map.entry(quest).or_default().push(QuestRewardItem {
                codename: codename.to_string(),
                count,
            });
        }
        QuestRewardItems(map)
    }

    pub fn items(&self, quest: u32) -> &[QuestRewardItem] {
        self.0.get(&quest).map(Vec::as_slice).unwrap_or(&[])
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// Real rows from `refqusetreward.txt`: `QNO_CH_SMITH_1` (id 3) is a
    /// receive-all quest and `QNO_CH_SHAMAN_1` (id 9) is one of the 17
    /// pick-one quests — the two shapes the mode switch has to separate.
    #[test]
    fn selection_cnt_decides_the_mode() {
        let content = "3\tQNO_CH_SMITH_1\t1\t1\t0\t0\t0\t0\t0\t0\t205\t270\r\n\
                       9\tQNO_CH_SHAMAN_1\t1\t0\t1\t1\t1\t1\t0\t0\t0\t0\r\n\
                       //comment";
        let modes = QuestRewardModes::parse(content);
        assert_eq!(modes.0.len(), 2);
        assert!(!modes.choose_one(3), "678 quests receive every item");
        assert!(modes.choose_one(9), "17 quests pick exactly one");
        // an unknown quest is not a picker
        assert!(!modes.choose_one(12345));
    }

    /// Real rows from `refquestrewarditems.txt`, including the `//DBtoMedia`
    /// comment line that makes the file's line count one more than its row
    /// count — a distinction `docs/re/systems/quest.md:113` got wrong.
    #[test]
    fn reward_items_skip_the_comment_line() {
        let content = "6\tQNO_CH_POTION_1\t0\tITEM_ETC_HP_POTION_01\txxx\txxx\t0\t28\t0\txxx\r\n\
                       9\tQNO_CH_SHAMAN_1\t0\tITEM_CH_SWORD_02_B\txxx\txxx\t0\t1\t0\txxx\r\n\
                       9\tQNO_CH_SHAMAN_1\t0\tITEM_CH_BLADE_02_B\txxx\txxx\t0\t1\t0\txxx\r\n\
                       //DBtoMedia";
        let items = QuestRewardItems::parse(content);
        assert_eq!(items.0.len(), 2, "the comment line is not a row");
        assert_eq!(
            items.items(6),
            [QuestRewardItem {
                codename: "ITEM_ETC_HP_POTION_01".into(),
                count: 28,
            }]
        );
        // candidates keep file order
        let shaman: Vec<&str> = items
            .items(9)
            .iter()
            .map(|item| item.codename.as_str())
            .collect();
        assert_eq!(shaman, ["ITEM_CH_SWORD_02_B", "ITEM_CH_BLADE_02_B"]);
        assert_eq!(items.items(999), []);
    }
}
