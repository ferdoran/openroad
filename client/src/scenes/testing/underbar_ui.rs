//! Idea: an offline preview of the underbar with mock quickslots and
//! progression numbers, so the pixel layout, gauge fills, paging and the
//! arrow indicator can be iterated without logging into a live server. Run
//! with `SCENE=ui_testing`. The underbar's own Update systems (refresh, keys,
//! buttons) already run in `SceneState::UiTesting`, so this only spawns the
//! bar and the fake numbers. Ref ids are vanilla skilldata/itemdata rows, so
//! the icons are real; casting logs but sends nothing (no connection).

use bevy::prelude::*;

use crate::plugins::hud::underbar::model::{PlayerProgress, QuickSlots, SlotAction};
use crate::plugins::hud::underbar::ui::spawn_underbar;
use crate::scenes::SceneState;

pub struct UnderbarUiPreviewPlugin;

impl Plugin for UnderbarUiPreviewPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            OnEnter(SceneState::UiTesting),
            (spawn_underbar, seed_mock_data).chain(),
        );
    }
}

// vanilla skilldata_5000.txt active skills / itemdata potion
const CH_COLD_GIGONGTA: u32 = 90; // SKILL_CH_COLD_GIGONGTA_A_01
const CH_LIGHTNING_GIGONGTA: u32 = 107; // SKILL_CH_LIGHTNING_GIGONGTA_A_01
const CH_FIRE_GIGONGTA: u32 = 124; // SKILL_CH_FIRE_GIGONGTA_A_01
const HP_POTION: u32 = 5; // ITEM_ETC_HP_POTION_01

fn seed_mock_data(mut quickslots: ResMut<QuickSlots>, mut progress: ResMut<PlayerProgress>) {
    let mut slots = QuickSlots::default();
    slots.slots[0] = Some(SlotAction::Skill {
        ref_id: CH_COLD_GIGONGTA,
    });
    slots.slots[1] = Some(SlotAction::Skill {
        ref_id: CH_LIGHTNING_GIGONGTA,
    });
    slots.slots[2] = Some(SlotAction::Skill {
        ref_id: CH_FIRE_GIGONGTA,
    });
    slots.slots[9] = Some(SlotAction::Item { ref_id: HP_POTION });
    // page 2 content to exercise the arrows + page digit
    slots.slots[10] = Some(SlotAction::Skill {
        ref_id: CH_FIRE_GIGONGTA,
    });
    slots.special = Some(SlotAction::Skill {
        ref_id: CH_COLD_GIGONGTA,
    });
    slots.armed = Some(0);
    *quickslots = slots;

    *progress = PlayerProgress {
        level: 42,
        exp_offset: 46_500_000, // leveldata 42: mid-level
        skill_exp: 260,
        skill_points: 12_345,
    };
}
