//! Quest reward picker (`quest_re.2dt`, root `CNIFQuestReward` id 188).
//!
//! Idea: this window is **4th-gen only** — there is no classic counterpart, and
//! in particular `resinfo/ifquestreward.txt` is NOT this window: it is a paper
//! scroll with a PML body and an Abandon button, no slots at all
//! (`docs/re/ui/quest-reward-window.md` §3, which corrects
//! `docs/re/systems/quest.md:127,205`). So everything here comes from
//! `res_ui/quest_re.2dt` (48 entries, `4 + 48*976 = 46852` B).
//!
//! The finding that shapes the code is that **choose-one vs receive-all is a
//! DATA mode, not a UI variant**: `refqusetreward.txt` column 7 `SelectionCnt`
//! is 0 for 678 quests (take every listed item) and 1 for 17 (pick exactly
//! one), never higher. Nothing in openroad read that column before
//! (`assets/textdata/questreward.rs` now does), and a window that assumed one
//! mode would be wrong for the other set.
//!
//! Two things the authored data says that a tidy implementation would erase:
//!
//! * the 20 slot x positions are **hand-nudged, not a pitch** — row 1 runs
//!   579/617/652/688/723/760/796/833/868/903 (deltas 38,35,36,35,37,36,37,35,35
//!   around a nominal 36) and row 2 differs from it by a pixel in six columns.
//!   Both rows are transcribed verbatim rather than generated from a pitch.
//! * the grid **over-provisions**: 20 slots for a candidate pool that maxes out
//!   at 14 rows in this corpus, which is why nothing here scrolls.
//!
//! Not built, deliberately: the four `CNIFGoldSlot` currency rows (what they
//! enumerate is UNKNOWN — §9 — so they would be four invented labels) and the
//! id-40 scroll (nothing to scroll past 20 slots). The confirm step belongs to
//! `corfim.2dt`, a separate root and a separate ticket.

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui_widgets::{Activate, Button};

use crate::assets::FontAssets;
use crate::plugins::hud::game_window::{abs_node, spawn_game_window};
use crate::plugins::hud::scale::hud_scale;
use crate::plugins::textdata::{
    ClientItemData, ClientItemIndex, ClientQuestRewards, ClientUiStrings,
};

/// Authored root rect `554,195,425,456`. The chrome shell derives its outer
/// size from the content, and the authored `int_window_` interior
/// (`566,237,401,402`) reproduces the width exactly — 401 — so the content
/// size is that interior, and the derived outer height lands 2px under the
/// authored 456.
const CONTENT_SIZE: (f32, f32) = (401.0, 402.0);
/// Authored root origin, used to carry the absolute 2dt rects into
/// content-local space (`abs - root - chrome inset`, see [`content_x`]).
const ROOT: (f32, f32) = (554.0, 195.0);
/// Where the window sits on screen: right/top anchored like the other HUD
/// windows. Authored x 554 on a 1024-wide canvas leaves 45px to the right
/// edge (1024 - 554 - 425); the top is the authored 195.
const ANCHOR_RIGHT_TOP: (f32, f32) = (45.0, 195.0);

/// Slot size, authored on every one of the 20 `CIFSlotWithHelpEx` entries.
const SLOT: f32 = 32.0;
/// Authored slot x positions, verbatim (see the module note — this is a
/// hand-nudged grid, not a pitch).
const ROW1_XS: [f32; 10] = [
    579.0, 617.0, 652.0, 688.0, 723.0, 760.0, 796.0, 833.0, 868.0, 903.0,
];
const ROW2_XS: [f32; 10] = [
    579.0, 616.0, 652.0, 687.0, 724.0, 760.0, 795.0, 831.0, 868.0, 903.0,
];
const ROW1_Y: f32 = 514.0;
const ROW2_Y: f32 = 550.0;

/// `GDR`-less 4th-gen button entry: `711,601,112,24` on `com_mid_button02.ddj`,
/// `Text=UIIT_STT_QUEST_REWARD` ("Reward").
const BUTTON_RECT: (f32, f32, f32, f32) = (711.0, 601.0, 112.0, 24.0);
const BUTTON_DDJ: &str = "media://interface/ifcommon/com_mid_button02.ddj";

/// Slot backdrop + selection ring: the 2dt authors no slot art and has no
/// colour key, so both are openroad conventions (the picker needs the chosen
/// slot to be distinguishable, and per the doc's WCAG note not by colour
/// alone — hence a border, not just a tint).
const SLOT_BG: Color = Color::srgba(0.0, 0.0, 0.0, 0.55);
const SELECTED_BORDER: Color = Color::srgb_u8(255, 226, 123);

#[derive(Component)]
pub struct QuestRewardWindow;
/// A reward slot with its index into the quest's reward rows.
#[derive(Component)]
pub struct QuestRewardSlot(pub usize);

/// Open quest and, in pick-one mode, the chosen candidate.
#[derive(Resource, Default)]
pub struct QuestRewardState {
    pub quest: Option<u32>,
    pub selected: Option<usize>,
}

/// Convert an authored absolute 2dt x/y into content-local space: the content
/// container sits at the chrome inset inside the root.
fn content_x(abs: f32) -> f32 {
    abs - ROOT.0
        - (crate::plugins::hud::game_window::FRAME_VIS_SIDE
            + crate::plugins::hud::game_window::CHROME_PAD)
}
fn content_y(abs: f32) -> f32 {
    abs - ROOT.1 - crate::plugins::hud::game_window::CONTENT_TOP
}

/// Spawn/despawn the window to match [`QuestRewardState`].
#[allow(clippy::too_many_arguments)]
pub fn sync_quest_reward_window(
    state: Res<QuestRewardState>,
    rewards: Res<ClientQuestRewards>,
    item_index: Res<ClientItemIndex>,
    item_data: Res<ClientItemData>,
    ui_strings: Res<ClientUiStrings>,
    fonts: Res<FontAssets>,
    asset_server: Res<AssetServer>,
    cameras: Query<Entity, With<Camera2d>>,
    windows: Query<Entity, With<QuestRewardWindow>>,
    mut commands: Commands,
) {
    if !state.is_changed() {
        return;
    }
    let open = windows.iter().next();
    let Some(quest) = state.quest else {
        if let Some(window) = open {
            commands.entity(window).despawn();
        }
        return;
    };
    if let Some(window) = open {
        commands.entity(window).despawn();
    }
    let Some(camera) = cameras.iter().next() else {
        return;
    };

    let window = spawn_game_window(
        &mut commands,
        &asset_server,
        &fonts,
        camera,
        ui_strings.get_or("UIIT_STT_QUEST", "Quest"),
        CONTENT_SIZE,
        ANCHOR_RIGHT_TOP,
        hud_scale(),
    );
    commands.entity(window.root).insert(QuestRewardWindow);
    commands.entity(window.expect_close_button()).observe(
        |_: On<Activate>, mut state: ResMut<QuestRewardState>| {
            state.quest = None;
            state.selected = None;
        },
    );

    let choose_one = rewards.choose_one(quest);
    let items = rewards.items(quest);
    let slots: Vec<(f32, f32)> = ROW1_XS
        .iter()
        .map(|x| (*x, ROW1_Y))
        .chain(ROW2_XS.iter().map(|x| (*x, ROW2_Y)))
        .collect();

    commands.entity(window.content).with_children(|content| {
        for (index, (x, y)) in slots.into_iter().enumerate() {
            let item = items.get(index);
            let mut slot = content.spawn((
                QuestRewardSlot(index),
                Button,
                Hovered::default(),
                Node {
                    // a 2px ring so the pick is not signalled by colour alone
                    border: UiRect::all(Val::Px(2.0)),
                    ..abs_node((content_x(x), content_y(y), SLOT, SLOT), hud_scale())
                },
                BackgroundColor(SLOT_BG),
                BorderColor::all(if choose_one && state.selected == Some(index) {
                    SELECTED_BORDER
                } else {
                    Color::NONE
                }),
            ));
            slot.observe(on_slot_activate);
            if let Some(item) = item {
                if let Some(icon) = item_index
                    .id(&item.codename)
                    .and_then(|id| item_data.get(&id))
                    .and_then(|row| row.icon_path())
                {
                    slot.insert(ImageNode {
                        image: asset_server.load(icon),
                        image_mode: NodeImageMode::Stretch,
                        ..default()
                    });
                }
            }
        }

        // the Reward button: enabled only once a pick-one quest has a pick
        content
            .spawn((
                Button,
                Hovered::default(),
                abs_node(
                    (
                        content_x(BUTTON_RECT.0),
                        content_y(BUTTON_RECT.1),
                        BUTTON_RECT.2,
                        BUTTON_RECT.3,
                    ),
                    hud_scale(),
                ),
                ImageNode {
                    image: asset_server.load(BUTTON_DDJ),
                    image_mode: NodeImageMode::Stretch,
                    ..default()
                },
            ))
            .observe(on_reward_activate)
            .with_children(|button| {
                button.spawn((
                    Text::new(
                        ui_strings
                            .get_or("UIIT_STT_QUEST_REWARD", "Reward")
                            .to_string(),
                    ),
                    TextFont {
                        font: fonts.nine.clone().into(),
                        font_size: FontSize::Px(9.0 * hud_scale()),
                        ..default()
                    },
                    TextColor(Color::WHITE),
                    TextLayout::justify(Justify::Center),
                    Node {
                        width: Val::Percent(100.0),
                        align_self: AlignSelf::Center,
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
            });
    });
}

/// Clicking a slot picks it — but only in pick-one mode. In receive-all mode
/// every listed item is granted, so a "selection" would be meaningless.
fn on_slot_activate(
    activate: On<Activate>,
    slots: Query<&QuestRewardSlot>,
    rewards: Res<ClientQuestRewards>,
    mut state: ResMut<QuestRewardState>,
) {
    let Ok(QuestRewardSlot(index)) = slots.get(activate.entity) else {
        return;
    };
    let Some(quest) = state.quest else {
        return;
    };
    if !rewards.choose_one(quest) || *index >= rewards.items(quest).len() {
        return;
    }
    state.selected = Some(*index);
}

/// Accept the reward — which in the original does **not** complete the flow:
/// it raises the confirm dialog (`corfim.2dt`, its own root id 189 and its own
/// module, #663), which is what actually asks "Will you take this reward?".
/// The picker's state is left untouched so Cancel can come back to it with the
/// selection intact; nothing is sent either way, because the opcode that
/// carries the choice back is UNKNOWN (doc §9).
fn on_reward_activate(
    _: On<Activate>,
    rewards: Res<ClientQuestRewards>,
    state: Res<QuestRewardState>,
    mut confirm: ResMut<super::quest_reward_confirm::QuestRewardConfirmState>,
) {
    let Some(quest) = state.quest else {
        return;
    };
    if rewards.choose_one(quest) && state.selected.is_none() {
        debug!("quest reward: quest {quest} needs a pick before accepting");
        return;
    }
    confirm.open = true;
}

pub fn cleanup_quest_reward_window(
    mut commands: Commands,
    windows: Query<Entity, With<QuestRewardWindow>>,
    mut state: ResMut<QuestRewardState>,
) {
    for window in windows.iter() {
        commands.entity(window).despawn();
    }
    *state = QuestRewardState::default();
}

#[cfg(test)]
mod test {
    use super::*;

    /// The slot x lists are the authored numbers, not a generated pitch: the
    /// deltas are hand-nudged (38,35,36,35,37,36,37,35,35 in row 1) and the
    /// two rows disagree by a pixel in six columns. Generating them from 36
    /// would silently "correct" the data.
    #[test]
    fn the_slot_grid_is_hand_nudged_not_a_pitch() {
        assert_eq!(ROW1_XS.len() + ROW2_XS.len(), 20, "ids 301-320");
        let deltas: Vec<f32> = ROW1_XS.windows(2).map(|w| w[1] - w[0]).collect();
        assert_eq!(
            deltas,
            [38.0, 35.0, 36.0, 35.0, 37.0, 36.0, 37.0, 35.0, 35.0]
        );
        assert!(
            deltas.iter().any(|d| *d != 36.0),
            "a uniform 36 pitch would not reproduce the authored row"
        );
        let differing = ROW1_XS
            .iter()
            .zip(ROW2_XS.iter())
            .filter(|(a, b)| a != b)
            .count();
        assert_eq!(differing, 5, "the two rows are not the same numbers");
        // both rows start and end at the same x, which is what makes the
        // middle jitter visible as authoring noise rather than a shift
        assert_eq!((ROW1_XS[0], ROW1_XS[9]), (ROW2_XS[0], ROW2_XS[9]));
    }

    /// The authored absolutes have to land inside the content box once the
    /// root origin and the chrome inset are removed.
    #[test]
    fn authored_absolutes_map_into_the_content_box() {
        // first slot 579,514 -> 13,283 relative to the content origin
        assert_eq!((content_x(579.0), content_y(514.0)), (13.0, 283.0));
        assert_eq!(content_y(ROW2_Y) - content_y(ROW1_Y), 36.0, "row pitch");
        for x in ROW1_XS.iter().chain(ROW2_XS.iter()) {
            assert!(content_x(*x) >= 0.0 && content_x(*x) + SLOT <= CONTENT_SIZE.0);
        }
        let (bx, by, bw, bh) = BUTTON_RECT;
        assert!(content_x(bx) + bw <= CONTENT_SIZE.0);
        assert!(content_y(by) + bh <= CONTENT_SIZE.1);
    }

    /// The content width is the authored `int_window_` interior, which is how
    /// the derived outer size reproduces the authored root width of 425.
    #[test]
    fn the_content_size_reproduces_the_authored_root_width() {
        use crate::plugins::hud::game_window::outer_size;
        assert_eq!(CONTENT_SIZE.0, 401.0, "int_window_ 566,237,401,402");
        assert_eq!(outer_size(CONTENT_SIZE).0, 425.0, "authored root width");
        // the height lands 2px under the authored 456 — stated, not hidden
        assert_eq!(outer_size(CONTENT_SIZE).1, 454.0);
    }

    /// 20 slots for a pool that never exceeds 14 rows: the grid over-provisions
    /// and nothing has to scroll, which is why the id-40 scroll is not built.
    #[test]
    fn the_grid_over_provisions_the_largest_pool() {
        const MAX_OBSERVED_POOL: usize = 14;
        assert!(ROW1_XS.len() + ROW2_XS.len() > MAX_OBSERVED_POOL);
    }
}

/// Self-registration for the quest reward picker (#345) (#558). The HUD registry holds one line per
/// window, so two windows landing in the same lap no longer collide on it.
pub struct QuestRewardPlugin;

impl Plugin for QuestRewardPlugin {
    fn build(&self, app: &mut App) {
        use crate::scenes::SceneState;

        use super::quest_reward_confirm::{
            cleanup_quest_reward_confirm, sync_quest_reward_confirm, QuestRewardConfirmState,
        };

        app.init_resource::<QuestRewardState>()
            .init_resource::<QuestRewardConfirmState>()
            .add_systems(
                OnExit(SceneState::GameWorld),
                (cleanup_quest_reward_window, cleanup_quest_reward_confirm),
            )
            // the live world plus the offline preview scene (there is no
            // reward opcode yet — doc §9)
            .add_systems(
                Update,
                (sync_quest_reward_window, sync_quest_reward_confirm).run_if(
                    in_state(SceneState::GameWorld).or_else(in_state(SceneState::UiTesting)),
                ),
            );
    }
}
