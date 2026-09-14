//! Action-window command table (`actionwnddata.txt`).
//!
//! Idea: `resinfo/ifaction.txt` authors 52 empty slots — every one has
//! `Text=""`, `HelpString=""` and `DDJ=""` — so the panel's *content* is not in
//! the layout at all. It is in this textdata table, which is what binds an
//! action to its icon, its tooltip string and its slot:
//!
//! ```text
//! 1000  1  앉기서기  UIIT_CTL_TOG_SIT_STAND_TT  icon\action\icon_cha_sit.ddj  1  0
//! //1005 1 자동선택  UIIT_CTL_AUTOSELECT_TT     icon\action\icon_cha_autotarget.ddj  1  5
//! 4000  1  인사      UIIT_CTL_EMOT_GREETING_TT  icon\action\emot_act_greeting.ddj    4  0
//! ```
//!
//! Columns: command id · service flag · Korean name · textuisystem key · icon ·
//! **group** (1 = character control, 4 = emote) · **slot index within the
//! group**. Three rows are commented out with `//` (1005 auto-select, 1011
//! helper state, 1013 stall network), and each of their slot indices is reused
//! by the next live row — which is why the live indices form a gapless 0..=14
//! run even though the ids skip.
//!
//! Two consequences worth stating, because they contradict what the layout
//! alone suggests:
//!
//! 1. **The identity of a slot is `(group, index)`, never the command id.**
//!    `ifaction.txt`'s own `CommandID=` key repeats across panels (the value `7`
//!    sits on three different blocks), so it cannot be a global action id.
//! 2. **The table is newer than the layout.** `ifaction.txt` assigns command ids
//!    only up to 1010/4006; this table adds 1012, 1014-1017 and 5000. So the
//!    runtime source of truth for what a slot *is* is this file, and the
//!    layout's `CommandID` values are stale authoring residue.

/// One authored action-window command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionCommand {
    /// The `CommandID` the original sends for this action.
    pub command_id: u32,
    /// `false` for the three `//`-commented rows — kept, because they are the
    /// only record of what the holes in the id run were.
    pub live: bool,
    /// textuisystem key for the tooltip/label.
    pub text_key: String,
    /// Icon path, media-relative (`icon/action/…`, backslashes normalized).
    pub icon: String,
    /// 1 = character control grid, 4 = emote/action grid.
    pub group: u8,
    /// Slot index inside the group's grid.
    pub slot: u8,
}

#[derive(Debug, Clone, Default)]
pub struct ActionWndData(pub Vec<ActionCommand>);

impl ActionWndData {
    pub fn parse(content: &str) -> Self {
        let rows = content
            .lines()
            .filter_map(|line| {
                let fields: Vec<&str> = line.trim_end_matches('\r').split('\t').collect();
                if fields.len() < 7 {
                    return None;
                }
                let id = fields[0].trim().trim_start_matches('\u{feff}');
                let live = !id.starts_with("//");
                let command_id = id.trim_start_matches('/').parse::<u32>().ok()?;
                Some(ActionCommand {
                    command_id,
                    live,
                    text_key: fields[3].trim().to_string(),
                    icon: fields[4].trim().replace('\\', "/"),
                    group: fields[5].trim().parse().ok()?,
                    slot: fields[6].trim().parse().ok()?,
                })
            })
            .collect();
        Self(rows)
    }

    /// The live command occupying `index` of `group`, if any. Commented-out
    /// rows are skipped: their slot is taken by the live row that follows.
    pub fn slot(&self, group: u8, index: u8) -> Option<&ActionCommand> {
        self.0
            .iter()
            .find(|row| row.live && row.group == group && row.slot == index)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// Real rows, byte-for-byte from `Media/server_dep/silkroad/textdata/
    /// actionwnddata.txt` (decoded from UTF-16), including the commented 1005.
    const FIXTURE: &str = "1000\t1\t앉기서기\tUIIT_CTL_TOG_SIT_STAND_TT\ticon\\action\\icon_cha_sit.ddj\t1\t0\r\n\
                           //1005\t1\t자동선택\tUIIT_CTL_AUTOSELECT_TT\ticon\\action\\icon_cha_autotarget.ddj\t1\t5\r\n\
                           1006\t1\t교환\tUIIT_CTL_EXCHANGE_TT\ticon\\action\\icon_cha_exchange.ddj\t1\t5\r\n\
                           4000\t1\t인사\tUIIT_CTL_EMOT_GREETING_TT\ticon\\action\\emot_act_greeting.ddj\t4\t0\r\n\
                           5000\t1\tCOS애교\tUIIT_STT_COSNEWUI_CHARMS\ticon\\action\\cos_cmd_charm.ddj\t4\t7\r\n\
                           garbage";

    #[test]
    fn parses_action_commands_and_keeps_the_commented_rows() {
        let table = ActionWndData::parse(FIXTURE);
        assert_eq!(table.0.len(), 5, "the trailing garbage line is dropped");

        let sit = &table.0[0];
        assert_eq!(sit.command_id, 1000);
        assert!(sit.live);
        assert_eq!(sit.text_key, "UIIT_CTL_TOG_SIT_STAND_TT");
        // backslashes are normalized so the path can be joined to media://
        assert_eq!(sit.icon, "icon/action/icon_cha_sit.ddj");
        assert_eq!((sit.group, sit.slot), (1, 0));

        // 1005 is commented out and 1006 reuses its slot — the reason the live
        // control indices are gapless while the ids are not.
        assert!(!table.0[1].live);
        assert_eq!(table.0[1].command_id, 1005);
        assert_eq!(table.0[1].slot, table.0[2].slot);
    }

    /// A slot resolves to the LIVE row, never to the commented-out one that
    /// shares its index, and the two groups do not bleed into each other.
    #[test]
    fn slot_lookup_skips_commented_rows_and_is_group_local() {
        let table = ActionWndData::parse(FIXTURE);
        assert_eq!(table.slot(1, 5).map(|c| c.command_id), Some(1006));
        assert_eq!(table.slot(1, 0).map(|c| c.command_id), Some(1000));
        // group 4 index 0 is the greeting emote, not the control at index 0
        assert_eq!(table.slot(4, 0).map(|c| c.command_id), Some(4000));
        // the COS charm sits in the emote grid (group 4), id 5000
        assert_eq!(table.slot(4, 7).map(|c| c.command_id), Some(5000));
        assert_eq!(table.slot(1, 7), None);
        assert_eq!(table.slot(4, 5), None);
    }
}
