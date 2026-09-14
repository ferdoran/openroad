//! Quest-reward CONFIRM dialog — `res_ui/corfim.2dt`, root
//! `CNIFQuestReward_ConfirmWnd` id 189 (the filename is a shipped misspelling
//! of "confirm").
//!
//! Idea: the picker (`quest_reward.rs`, `quest_re.2dt` root 188) is only the
//! first half of the flow. `corfim.2dt` is its own root — no entry in either
//! file references the other — and it is the step that asks
//! *"Will you take this reward?"* before anything is granted. It needs **no
//! packet**, which is why it can be built while the reward opcodes are still
//! UNKNOWN (`docs/re/ui/quest-reward-window.md` §9): this dialog completes the
//! flow *up to* the wire and stops there. Confirm does exactly what
//! `on_reward_activate` used to do on its own — log the decision and close —
//! and Cancel returns to the picker with the selection intact.
//!
//! Geometry is transcribed from `docs/re/ui/corfim-quest-reward-confirm.md`
//! §4.1, which reads the file's 19 entries (`4 + 19*976 = 18548` B). 2DT rects
//! are absolute in one flat design space, so every constant here is the
//! doc's **local** column, i.e. `child - root` against the root
//! `568,293,379,250`.
//!
//! **Deviation (stated, per §5a):** the five reward frames are laid out on a
//! uniform 48px pitch with their slots centred, where the data has 48/48/48/47
//! for the frames, 48/47/49/49 for the slots and a non-constant `+2/+2/+1/+2/+4`
//! slot-inside-frame offset. That jitter is hand-authoring noise, not a design;
//! reproducing it costs ten magic numbers and buys nothing, and it breaks at
//! non-integer UI scales. The measured values stay in the doc, so the choice is
//! auditable. Everything else — the plate, the tiles, the black square, the
//! body line and the two 76x24 buttons — is verbatim.

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui::UiTargetCamera;
use bevy::ui_widgets::{Activate, Button};

use crate::assets::FontAssets;
use crate::plugins::hud::modal_dialog::{MODAL_BOTTOM, MODAL_SCRIM, MODAL_SIDE, MODAL_TOP};
use crate::plugins::hud::quest_reward::QuestRewardState;
use crate::plugins::hud::scale::hud_scale;
use crate::plugins::textdata::{
    ClientItemData, ClientItemIndex, ClientQuestRewards, ClientUiStrings,
};

/// Root rect `568,293,379,250` — only the size is ours to use, since the
/// dialog is centred on the scrim rather than placed at the authored origin
/// (the 2dt canvas is 1024x768 and our window is not).
const PLATE: (f32, f32) = (379.0, 250.0);

/// `msgbox2_window_` art directory; the 8 pieces measure 16x40 / 64x40 /
/// 16x64 / 16x16 in the user's PK2, which is where [`MODAL_SIDE`],
/// [`MODAL_TOP`] and [`MODAL_BOTTOM`] come from.
const PLATE_ART: &str = "media://interface/messagebox/msgbox2_window_";
const ART: &str = "media://interface/";

/// idx 1, id 2 — `com_bg_tile_a.ddj` over the plate's interior.
const BG_A_RECT: (f32, f32, f32, f32) = (16.0, 40.0, 348.0, 196.0);
/// idx 5, id 23 — the `com_blacksquare_` well behind the slot strip.
const BLACK_RECT: (f32, f32, f32, f32) = (58.0, 72.0, 271.0, 60.0);
/// idx 6, id 25 — `com_bg_tile_b.ddj` inside that well.
const BG_B_RECT: (f32, f32, f32, f32) = (60.0, 75.0, 262.0, 53.0);
/// idx 2, id 52 — the body line, `UIIT_STT_QUESTREWARD_MSG02`.
const BODY_RECT: (f32, f32, f32, f32) = (17.0, 152.0, 344.0, 16.0);
/// idx 3/4, ids 7/8 — `com_button.ddj`, authored at its own art size 76x24.
const CONFIRM_RECT: (f32, f32, f32, f32) = (106.0, 192.0, 76.0, 24.0);
const CANCEL_RECT: (f32, f32, f32, f32) = (196.0, 192.0, 76.0, 24.0);

/// Slot strip: five `pt_block02.ddj` frames (40x40, ids 139/21/20/18/19) each
/// holding one 32x32 `CIFSlotWithHelpEx` (ids 26-30).
const SLOT_COUNT: usize = 5;
const FRAME: f32 = 40.0;
const SLOT: f32 = 32.0;
/// First frame's authored x/y; the rest follow [`SLOT_PITCH`] — see the
/// module's deviation note.
const FRAME_ORIGIN: (f32, f32) = (71.0, 83.0);
const SLOT_PITCH: f32 = 48.0;

/// Frame `i`'s local rect.
fn frame_rect(index: usize) -> (f32, f32, f32, f32) {
    (
        FRAME_ORIGIN.0 + SLOT_PITCH * index as f32,
        FRAME_ORIGIN.1,
        FRAME,
        FRAME,
    )
}

/// Slot `i`'s local rect: centred in its frame (the deviation).
fn slot_rect(index: usize) -> (f32, f32, f32, f32) {
    let (x, y, w, h) = frame_rect(index);
    let inset = (w - SLOT) / 2.0;
    (x + inset, y + (h - SLOT) / 2.0, SLOT, SLOT)
}

/// An absolutely-positioned node from a plate-local rect.
fn plate_node((x, y, w, h): (f32, f32, f32, f32)) -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: Val::Px(x * hud_scale()),
        top: Val::Px(y * hud_scale()),
        width: Val::Px(w * hud_scale()),
        height: Val::Px(h * hud_scale()),
        ..default()
    }
}

/// Whether the confirm dialog is up. The payload stays in
/// [`QuestRewardState`] — Cancel must return to the picker with the selection
/// intact, so the choice is not moved here.
#[derive(Resource, Default)]
pub struct QuestRewardConfirmState {
    pub open: bool,
}

#[derive(Component)]
pub struct QuestRewardConfirmDialog;

/// One of the dialog's five mirror slots.
#[derive(Component)]
pub struct ConfirmRewardSlot(pub usize);

/// Spawn/despawn the dialog to match [`QuestRewardConfirmState`].
#[allow(clippy::too_many_arguments)]
pub fn sync_quest_reward_confirm(
    confirm: Res<QuestRewardConfirmState>,
    picker: Res<QuestRewardState>,
    rewards: Res<ClientQuestRewards>,
    item_index: Res<ClientItemIndex>,
    item_data: Res<ClientItemData>,
    ui_strings: Res<ClientUiStrings>,
    fonts: Res<FontAssets>,
    asset_server: Res<AssetServer>,
    cameras: Query<Entity, With<Camera2d>>,
    open: Query<Entity, With<QuestRewardConfirmDialog>>,
    mut commands: Commands,
) {
    if !confirm.is_changed() && !picker.is_changed() {
        return;
    }
    for entity in open.iter() {
        commands.entity(entity).despawn();
    }
    let (true, Some(quest)) = (confirm.open, picker.quest) else {
        return;
    };
    let Some(camera) = cameras.iter().next() else {
        return;
    };

    // The dialog mirrors the chosen reward: in pick-one mode the single
    // selection, otherwise everything the quest grants (capacity 5).
    let items = rewards.items(quest);
    let shown: Vec<usize> = if rewards.choose_one(quest) {
        picker.selected.into_iter().collect()
    } else {
        (0..items.len().min(SLOT_COUNT)).collect()
    };

    let text_font = TextFont {
        font: fonts.nine.clone().into(),
        font_size: FontSize::Px(9.0 * hud_scale()),
        ..default()
    };
    let img = |rect: (f32, f32, f32, f32), path: String| {
        (
            plate_node(rect),
            ImageNode {
                image: asset_server.load(path),
                image_mode: NodeImageMode::Stretch,
                ..default()
            },
            Pickable::IGNORE,
        )
    };

    // Captured out of the button loop so the root can point Enter at Confirm
    // (`hud::focus::HudDialog`).
    let mut confirm_button = None;
    let root = commands
        .spawn((
            QuestRewardConfirmDialog,
            Name::from("Quest Reward Confirm"),
            UiTargetCamera(camera),
            GlobalZIndex(90),
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            BackgroundColor(MODAL_SCRIM),
        ))
        .with_children(|scrim| {
            scrim
                .spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        width: Val::Px(PLATE.0 * hud_scale()),
                        height: Val::Px(PLATE.1 * hud_scale()),
                        margin: UiRect::all(Val::Auto),
                        ..default()
                    },
                    Pickable::IGNORE,
                ))
                .with_children(|plate| {
                    // the msgbox2_window_ 9-slice, 16 at the sides, 40 top,
                    // 16 bottom
                    let (w, h) = PLATE;
                    for ((x, y, pw, ph), piece) in [
                        ((0.0, 0.0, MODAL_SIDE, MODAL_TOP), "left_up"),
                        ((MODAL_SIDE, 0.0, w - 2.0 * MODAL_SIDE, MODAL_TOP), "mid_up"),
                        ((w - MODAL_SIDE, 0.0, MODAL_SIDE, MODAL_TOP), "right_up"),
                        (
                            (0.0, MODAL_TOP, MODAL_SIDE, h - MODAL_TOP - MODAL_BOTTOM),
                            "left_side",
                        ),
                        (
                            (
                                w - MODAL_SIDE,
                                MODAL_TOP,
                                MODAL_SIDE,
                                h - MODAL_TOP - MODAL_BOTTOM,
                            ),
                            "right_side",
                        ),
                        (
                            (0.0, h - MODAL_BOTTOM, MODAL_SIDE, MODAL_BOTTOM),
                            "left_down",
                        ),
                        (
                            (
                                MODAL_SIDE,
                                h - MODAL_BOTTOM,
                                w - 2.0 * MODAL_SIDE,
                                MODAL_BOTTOM,
                            ),
                            "mid_down",
                        ),
                        (
                            (w - MODAL_SIDE, h - MODAL_BOTTOM, MODAL_SIDE, MODAL_BOTTOM),
                            "right_down",
                        ),
                    ] {
                        plate.spawn(img((x, y, pw, ph), format!("{PLATE_ART}{piece}.ddj")));
                    }

                    plate.spawn(img(
                        BG_A_RECT,
                        format!("{ART}ifcommon/bg_tile/com_bg_tile_a.ddj"),
                    ));
                    plate.spawn((
                        plate_node(BLACK_RECT),
                        BackgroundColor(Color::BLACK),
                        Pickable::IGNORE,
                    ));
                    plate.spawn(img(
                        BG_B_RECT,
                        format!("{ART}ifcommon/bg_tile/com_bg_tile_b.ddj"),
                    ));

                    // the five mirror frames and their slots
                    for index in 0..SLOT_COUNT {
                        plate.spawn(img(frame_rect(index), format!("{ART}pet/pt_block02.ddj")));
                        let mut slot =
                            plate.spawn((ConfirmRewardSlot(index), plate_node(slot_rect(index))));
                        if let Some(icon) = shown
                            .get(index)
                            .and_then(|item| items.get(*item))
                            .and_then(|item| item_index.id(&item.codename))
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

                    plate.spawn((
                        Text::new(
                            ui_strings
                                .get_or("UIIT_STT_QUESTREWARD_MSG02", "Will you take this reward?")
                                .to_string(),
                        ),
                        text_font.clone(),
                        TextColor(Color::WHITE),
                        TextLayout::justify(Justify::Center),
                        plate_node(BODY_RECT),
                        Pickable::IGNORE,
                    ));

                    for (rect, key, fallback, is_confirm) in [
                        (CONFIRM_RECT, "UIIT_CTL_CONFIRM", "Confirm", true),
                        (CANCEL_RECT, "UIIT_CTL_CANCEL", "Cancel", false),
                    ] {
                        let mut button = plate.spawn((
                            Button,
                            Hovered::default(),
                            plate_node(rect),
                            ImageNode {
                                image: asset_server.load(format!("{ART}ifcommon/com_button.ddj")),
                                image_mode: NodeImageMode::Stretch,
                                ..default()
                            },
                        ));
                        if is_confirm {
                            button.observe(on_confirm);
                            confirm_button = Some(button.id());
                        } else {
                            button.observe(on_cancel);
                        }
                        button.with_children(|button| {
                            button.spawn((
                                Text::new(ui_strings.get_or(key, fallback).to_string()),
                                text_font.clone(),
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
                    }
                });
        })
        .id();
    if let Some(confirm_button) = confirm_button {
        commands
            .entity(root)
            .insert(crate::plugins::hud::focus::HudDialog { confirm_button });
    }
}

/// Confirm: the decision the picker used to take on its own. Still no packet —
/// which opcode carries the choice back is UNKNOWN (doc §9), and inventing one
/// would be worse than not sending it.
fn on_confirm(
    _: On<Activate>,
    rewards: Res<ClientQuestRewards>,
    mut confirm: ResMut<QuestRewardConfirmState>,
    mut picker: ResMut<QuestRewardState>,
) {
    let Some(quest) = picker.quest else {
        confirm.open = false;
        return;
    };
    if rewards.choose_one(quest) {
        info!(
            "quest reward: quest {quest} confirmed candidate {:?} (no send opcode known)",
            picker.selected
        );
    } else {
        info!("quest reward: quest {quest} confirmed, grants every listed item");
    }
    confirm.open = false;
    picker.quest = None;
    picker.selected = None;
}

/// Cancel: back to the picker, selection intact (acceptance 2).
fn on_cancel(_: On<Activate>, mut confirm: ResMut<QuestRewardConfirmState>) {
    confirm.open = false;
}

pub fn cleanup_quest_reward_confirm(
    mut commands: Commands,
    dialogs: Query<Entity, With<QuestRewardConfirmDialog>>,
    mut confirm: ResMut<QuestRewardConfirmState>,
) {
    for dialog in dialogs.iter() {
        commands.entity(dialog).despawn();
    }
    confirm.open = false;
}

#[cfg(test)]
mod test {
    use super::*;

    /// The authored frame and slot x runs, from the doc's §4.3 table. Kept in
    /// the test rather than in the code: they are what the deviation is
    /// measured against, not what we draw.
    const AUTHORED_FRAME_XS: [f32; 5] = [71.0, 119.0, 167.0, 215.0, 262.0];
    const AUTHORED_SLOT_XS: [f32; 5] = [73.0, 121.0, 168.0, 217.0, 266.0];

    /// The stated deviation, pinned: a uniform 48px pitch, and never more than
    /// 4px from the hand-authored run it replaces.
    #[test]
    fn the_uniform_pitch_stays_within_four_pixels_of_the_authored_run() {
        for index in 0..SLOT_COUNT {
            let ours = frame_rect(index).0;
            let authored = AUTHORED_FRAME_XS[index];
            assert!(
                (ours - authored).abs() <= 4.0,
                "frame {index}: {ours} vs authored {authored}"
            );
            let ours = slot_rect(index).0;
            let authored = AUTHORED_SLOT_XS[index];
            assert!(
                (ours - authored).abs() <= 4.0,
                "slot {index}: {ours} vs authored {authored}"
            );
        }
        // and it really is uniform, which the authored run is not
        let deltas: Vec<f32> = (1..SLOT_COUNT)
            .map(|i| frame_rect(i).0 - frame_rect(i - 1).0)
            .collect();
        assert_eq!(deltas, vec![SLOT_PITCH; SLOT_COUNT - 1]);
    }

    /// Every slot sits inside its own frame, which is the property the
    /// centring deviation has to preserve.
    #[test]
    fn every_slot_sits_inside_its_frame() {
        for index in 0..SLOT_COUNT {
            let (fx, fy, fw, fh) = frame_rect(index);
            let (sx, sy, sw, sh) = slot_rect(index);
            assert!(
                sx >= fx && sx + sw <= fx + fw,
                "slot {index} overflows in x"
            );
            assert!(
                sy >= fy && sy + sh <= fy + fh,
                "slot {index} overflows in y"
            );
        }
    }

    /// Everything the dialog draws is inside the authored plate, and the two
    /// buttons keep their art size (`com_button.ddj` is 76x24).
    #[test]
    fn every_transcribed_rect_fits_the_plate() {
        let mut rects = vec![
            BG_A_RECT,
            BLACK_RECT,
            BG_B_RECT,
            BODY_RECT,
            CONFIRM_RECT,
            CANCEL_RECT,
        ];
        for index in 0..SLOT_COUNT {
            rects.push(frame_rect(index));
            rects.push(slot_rect(index));
        }
        for (x, y, w, h) in rects {
            assert!(x + w <= PLATE.0, "{x}+{w} overflows the plate width");
            assert!(y + h <= PLATE.1, "{y}+{h} overflows the plate height");
        }
        assert_eq!((CONFIRM_RECT.2, CONFIRM_RECT.3), (76.0, 24.0));
        assert_eq!((CANCEL_RECT.2, CANCEL_RECT.3), (76.0, 24.0));
    }

    /// The plate's interior tile is the authored `16,40,348,196`, which is the
    /// shared shell's inset applied to the authored root — origin exactly, and
    /// the size 1x2 larger than `deflate(16,40,16,16)` predicts (doc §4.3,
    /// recorded there as an authored overshoot, not corrected here).
    #[test]
    fn the_background_tile_is_the_authored_rect() {
        assert_eq!(BG_A_RECT, (16.0, 40.0, 348.0, 196.0));
        assert_eq!((BG_A_RECT.0, BG_A_RECT.1), (MODAL_SIDE, MODAL_TOP));
        let predicted = crate::plugins::hud::modal_dialog::modal_interior(PLATE.0, PLATE.1);
        assert_eq!(
            (BG_A_RECT.2 - predicted.2, BG_A_RECT.3 - predicted.3),
            (1.0, 2.0)
        );
    }
}
