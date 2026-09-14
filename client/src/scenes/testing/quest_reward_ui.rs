//! Idea: an offline preview of the quest reward picker. The Update systems are
//! already gated to also run in `SceneState::UiTesting`, so this only opens the
//! window on a real quest id.
//!
//! `QNO_CH_SHAMAN_1` (quest 9) is one of the **17** `SelectionCnt=1` quests and
//! carries a 14-row candidate pool, so the preview exercises the pick-one mode
//! and the full pool against real `refqusetreward.txt` /
//! `refquestrewarditems.txt` data. Run with `SCENE=ui_testing`.

use bevy::prelude::*;

use crate::plugins::hud::quest_reward::QuestRewardState;
use crate::scenes::SceneState;

pub struct QuestRewardUiPreviewPlugin;

impl Plugin for QuestRewardUiPreviewPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(SceneState::UiTesting), open_reward_preview);
    }
}

/// `QNO_CH_SHAMAN_1`, `refqusetreward.txt` row id 9.
const PREVIEW_QUEST: u32 = 9;

fn open_reward_preview(mut state: ResMut<QuestRewardState>) {
    state.quest = Some(PREVIEW_QUEST);
    state.selected = None;
}
