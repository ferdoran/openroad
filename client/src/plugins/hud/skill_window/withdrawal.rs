//! Cyclical Growth (`GDR_SKILLWITHDRAWAL:CIFSkillWithdrawal`, `ginterface.txt:1007`,
//! id 43, `595,262,388,435`, `interface\frame\mframe_wnd_`, title
//! `UIIT_STT_CIRCULATION_SYSTEM` → **"Cyclical Growth System"** despite the key
//! name) — the SP-recovery window, unbuilt until now (#317,
//! `docs/re/ui/hud-skill-window.md` §3/§6-6).
//!
//! Idea: the skill page's right-press has been deliberately inert since the
//! window was written ("withdrawal waits for the removal-box UI",
//! `ui.rs:1762`), because levelling a skill *down* on a stray right-click is
//! the one interaction a skill tree must not have. This module is that UI: a
//! spinner over one learned ladder — pick the level to keep, read what the
//! withdrawal returns, then confirm — driven by the same
//! [`crate::plugins::skills::book::withdraw_to`] the SP accounting already
//! uses.
//!
//! Every rect below is `ifskillremovalbox.txt`, verbatim and window-relative,
//! converted once by [`wd`] — the same discipline the practice box uses, so a
//! transcription error shows up against the file instead of hiding in a magic
//! number.
//!
//! **Deviation 1 — the opener.** The data's own opener lives in `ifskill.txt`'s
//! `Withdrawal` section, whose rects the unit doc does not transcribe, so we
//! do not invent one: the window opens from the skill page's already-reserved
//! right-press on a learned cell.
//!
//! **Deviation 2 — `GDR_SKLRB_BGTILE` is not drawn.** The tile's rect is
//! transcribed (`16,40,268,188`) but *which* `com_bg_tile_*` it names is not,
//! and the shell already paints a background there. Drawing a guessed tile
//! would be an unsourced value; the rect is kept as the content origin, which
//! is the part the file does state.
//!
//! **UNKNOWN, and rendered as such — the gold cost.** `GDR_SKLRB_NEEDMONEY`
//! exists, but no formula for it does: `skills/book.rs:13` already records that
//! our withdrawal "refunds the full SP sum with no gold cost", and the withdraw
//! opcode itself is unwired. The required-amount row therefore shows `-`, not a
//! number we made up, and confirming applies the local refund and logs that no
//! packet is sent.

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui_widgets::{Activate, Button};

use crate::assets::textdata::skilldata::SkillData;
use crate::assets::FontAssets;
use crate::plugins::hud::game_window::{self, GameWindow, WindowGeometry};
use crate::plugins::hud::scale::hud_scale;
use crate::plugins::hud::underbar::model::PlayerProgress;
use crate::plugins::net::inventory::Inventory;
use crate::plugins::player::Player;
use crate::plugins::skills::book::{withdraw_to, SkillBook, SkillGroupIndex};
use crate::plugins::textdata::{ClientSkillData, ClientTextNames, ClientUiStrings};

/// `ginterface.txt:1007` — the host rect's extent (its origin is the vanilla
/// screen position, which our shell anchors itself).
const OUTER: (f32, f32) = (388.0, 435.0);
/// `GDR_SKLRB_BGTILE 16,40,268,188` — the tile the file authors first, and the
/// origin every other rect is measured against (deviation 2).
const ORIGIN: (f32, f32) = (16.0, 40.0);

/// `ifskillremovalbox.txt`, verbatim (window-relative).
const SKILLICON: (f32, f32, f32, f32) = (28.0, 80.0, 32.0, 32.0);
const SKILLNAME: (f32, f32, f32, f32) = (88.0, 78.0, 125.0, 13.0);
const CURRENTLEVEL: (f32, f32, f32, f32) = (217.0, 78.0, 30.0, 13.0);
/// The level the character keeps — the value the two arrows spin. Reading
/// `[S]`: it is the only numeric static the arrows sit next to (they are at
/// x 141, between this rect's right edge at 135 and `WITHDRAW_LEV`'s left edge
/// at 180), and `WITHDRAWED_LEVEL` right of the "Withdrawn Lv." caption is
/// then the count that follows from it.
const TARGETLEVEL: (f32, f32, f32, f32) = (85.0, 104.0, 50.0, 13.0);
const BTN_RECOVER: (f32, f32, f32, f32) = (141.0, 98.0, 19.0, 12.0);
const BTN_DOWNGRADE: (f32, f32, f32, f32) = (141.0, 110.0, 19.0, 12.0);
const WITHDRAW_LEV: (f32, f32, f32, f32) = (180.0, 105.0, 65.0, 13.0);
const WITHDRAWED_LEVEL: (f32, f32, f32, f32) = (249.0, 105.0, 20.0, 13.0);
const TOTALPOINT: (f32, f32, f32, f32) = (135.0, 133.0, 156.0, 24.0);
const NEEDMONEY: (f32, f32, f32, f32) = (102.0, 158.0, 156.0, 24.0);
const CURRENTMONEY: (f32, f32, f32, f32) = (102.0, 183.0, 156.0, 24.0);
const GOLD_NEED: (f32, f32, f32, f32) = (257.0, 167.0, 30.0, 13.0);
const GOLD_HAVE: (f32, f32, f32, f32) = (257.0, 188.0, 30.0, 13.0);
/// `GDR_SKLRB_DECOBOX 7,62,0,0` — a zero rect means "draw the art at its
/// native size", so the node carries a position and no extent.
const DECOBOX: (f32, f32) = (7.0, 62.0);
const BTN_OK: (f32, f32, f32, f32) = (77.0, 212.0, 76.0, 24.0);
const BTN_CANCEL: (f32, f32, f32, f32) = (158.0, 212.0, 76.0, 24.0);

const ART: &str = "media://interface/";
/// Screen anchor (right, top) in physical px — ours; the vanilla `595,262`
/// origin is a 1024x768 screen position and our window is not that size.
const ANCHOR: (f32, f32) = (420.0, 90.0);

/// Window-relative resinfo rect -> content-relative.
const fn wd(r: (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
    (r.0 - ORIGIN.0, r.1 - ORIGIN.1, r.2, r.3)
}

/// The open Cyclical Growth window: which ladder, and the level to keep.
#[derive(Resource, Default)]
pub struct SkillWithdrawalState {
    /// `group_id` of the ladder being edited; `None` = window closed.
    pub group: Option<i32>,
    /// Level the character keeps after confirming (0 = unlearn entirely).
    pub target: u8,
}

impl SkillWithdrawalState {
    /// Open on `group`, starting at the level already learned (nothing
    /// withdrawn yet — the window opens on a no-op, as a spinner should).
    pub fn open(&mut self, group: i32, learned: u8) {
        self.group = Some(group);
        self.target = learned;
    }

    pub fn close(&mut self) {
        self.group = None;
    }
}

#[derive(Component)]
pub struct SkillWithdrawalRoot;

/// The two `ub_*_arrow` spinner buttons; `+1` recovers a level, `-1`
/// withdraws one more.
#[derive(Component)]
struct WithdrawalStep(i8);

/// SP that comes back for withdrawing `learned - target` rungs — the same sum
/// [`withdraw_to`] refunds, computed without mutating anything so the window
/// can show it before the player commits.
pub fn refund_preview(
    index: &SkillGroupIndex,
    skill_data: &SkillData,
    group: i32,
    learned: u8,
    target: u8,
) -> u32 {
    if target >= learned {
        return 0;
    }
    let Some(ladder) = index.ladders.get(&group) else {
        return 0;
    };
    ladder[target as usize..(learned as usize).min(ladder.len())]
        .iter()
        .filter_map(|id| skill_data.get(id))
        .map(|row| row.sp_cost())
        .sum()
}

/// Spawn/despawn the window to match [`SkillWithdrawalState`].
#[allow(clippy::too_many_arguments)]
pub fn apply_skill_withdrawal(
    state: Res<SkillWithdrawalState>,
    book: Res<SkillBook>,
    index: Res<SkillGroupIndex>,
    skill_data: Res<ClientSkillData>,
    names: Res<ClientTextNames>,
    ui_strings: Res<ClientUiStrings>,
    inventories: Query<&Inventory, With<Player>>,
    existing: Query<Entity, With<SkillWithdrawalRoot>>,
    fonts: Res<FontAssets>,
    asset_server: Res<AssetServer>,
    cam_query: Query<Entity, With<Camera2d>>,
    mut commands: Commands,
) {
    if !state.is_changed() && !book.is_changed() {
        return;
    }
    for entity in existing.iter() {
        commands.entity(entity).despawn();
    }
    let Some(group) = state.group else {
        return;
    };
    let Ok(camera) = cam_query.single() else {
        return;
    };
    let Some(data) = skill_data.data() else {
        return;
    };
    let learned = book.learned_level(group);
    let target = state.target.min(learned);
    let refund = refund_preview(&index, data, group, learned, target);
    let gold = inventories.iter().next().map(|inv| inv.gold).unwrap_or(0);

    // the ladder's own rung tells us name and icon; an unlearned ladder has
    // nothing to withdraw and the window should not be up for it
    let rung = index
        .ladders
        .get(&group)
        .and_then(|ladder| ladder.get(learned.saturating_sub(1) as usize))
        .and_then(|id| data.get(id));
    let (icon, name) = match rung {
        Some(row) => (
            row.icon_path(),
            row.name_key()
                .and_then(|key| names.name(key))
                .unwrap_or(row.code_name())
                .to_string(),
        ),
        None => (None, String::new()),
    };

    let s = hud_scale();
    let title = ui_strings
        .get_or("UIIT_STT_CIRCULATION_SYSTEM", "Cyclical Growth System")
        .to_string();
    let window: GameWindow = game_window::spawn_game_window_with(
        &mut commands,
        &asset_server,
        &fonts,
        camera,
        &title,
        WindowGeometry {
            outer: OUTER,
            content_at: ORIGIN,
        },
        None,
        ANCHOR,
        s,
        game_window::GameWindowStyle::default(),
    );
    commands
        .entity(window.root)
        .insert((SkillWithdrawalRoot, GlobalZIndex(46)));
    commands
        .entity(window.expect_close_button())
        .observe(on_cancel);

    let node = |r: (f32, f32, f32, f32)| {
        let (x, y, w, h) = wd(r);
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(x * s),
            top: Val::Px(y * s),
            width: Val::Px(w * s),
            height: Val::Px(h * s),
            ..default()
        }
    };
    let font = TextFont {
        font: fonts.two.clone().into(),
        font_size: FontSize::Px(12.0 * s),
        ..default()
    };

    commands.entity(window.content).with_children(|content| {
        // rec_return_bar.ddj at its native size (a zero rect in the data)
        content.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px((DECOBOX.0 - ORIGIN.0) * s),
                top: Val::Px((DECOBOX.1 - ORIGIN.1) * s),
                ..default()
            },
            ImageNode {
                image: asset_server.load(format!("{ART}recycle/rec_return_bar.ddj")),
                ..default()
            },
            Pickable::IGNORE,
        ));

        if let Some(icon) = icon {
            content.spawn((
                node(SKILLICON),
                ImageNode {
                    image: asset_server.load(icon),
                    image_mode: NodeImageMode::Stretch,
                    ..default()
                },
                Pickable::IGNORE,
            ));
        }

        let mut label = |rect: (f32, f32, f32, f32), text: String| {
            content.spawn((
                Text::new(text),
                font.clone(),
                TextColor(Color::WHITE),
                node(rect),
                Pickable::IGNORE,
            ));
        };
        label(SKILLNAME, name);
        label(CURRENTLEVEL, learned.to_string());
        label(TARGETLEVEL, target.to_string());
        label(
            WITHDRAW_LEV,
            ui_strings
                .get_or("UIIT_STT_CIRCULATION_WITHDRAW_LEV", "Withdrawn Lv.")
                .to_string(),
        );
        label(WITHDRAWED_LEVEL, (learned - target).to_string());
        label(
            TOTALPOINT,
            format!(
                "{}: {refund}",
                ui_strings.get_or("UIIT_STT_CIRCULATION_TOTAL_WITHDRAW_SP", "SP Withdrawn")
            ),
        );
        // the gold cost is UNKNOWN (module header) — a dash, not a number
        label(
            NEEDMONEY,
            format!(
                "{}: -",
                ui_strings.get_or("UIIT_STT_CIRCULATION_NEEDMONEY", "Required Amount")
            ),
        );
        label(
            CURRENTMONEY,
            format!(
                "{}: {}",
                ui_strings.get_or("UIIT_STT_CIRCULATION_CURRENTMONEY", "Amount Posessed"),
                gold
            ),
        );
        let gold_caption = ui_strings.get_or("UIIT_STT_GOLD", "Gold").to_string();
        label(GOLD_NEED, gold_caption.clone());
        label(GOLD_HAVE, gold_caption);

        // the two spinner arrows (underbar art, as the file names it)
        for (rect, step, art) in [
            (BTN_RECOVER, 1i8, "ub_up_arrow"),
            (BTN_DOWNGRADE, -1i8, "ub_down_arrow"),
        ] {
            content
                .spawn((
                    WithdrawalStep(step),
                    Button,
                    Hovered::default(),
                    node(rect),
                    ImageNode {
                        image: asset_server.load(format!("{ART}underbar/{art}.ddj")),
                        image_mode: NodeImageMode::Stretch,
                        ..default()
                    },
                ))
                .observe(on_step);
        }

        for (rect, key, fallback, confirm) in [
            (
                BTN_OK,
                "UIIT_STT_CIRCULATION_WITHDRAW_SKILL",
                "Skill Edit",
                true,
            ),
            (
                BTN_CANCEL,
                "UIIT_STT_CIRCULATION_CANCEL_WITHDRAW",
                "Cancel",
                false,
            ),
        ] {
            let mut button = content.spawn((
                Button,
                Hovered::default(),
                node(rect),
                ImageNode {
                    image: asset_server.load(format!("{ART}ifcommon/com_button.ddj")),
                    image_mode: NodeImageMode::Stretch,
                    ..default()
                },
            ));
            if confirm {
                button.observe(on_confirm);
            } else {
                button.observe(on_cancel);
            }
            button.with_children(|button| {
                button.spawn((
                    Text::new(ui_strings.get_or(key, fallback).to_string()),
                    font.clone(),
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
}

/// An arrow press: clamp into `0..=learned`, so the spinner can never ask for
/// a withdrawal the ladder cannot serve.
fn on_step(
    activate: On<Activate>,
    steps: Query<&WithdrawalStep>,
    book: Res<SkillBook>,
    mut state: ResMut<SkillWithdrawalState>,
) {
    let Ok(step) = steps.get(activate.entity) else {
        return;
    };
    let Some(group) = state.group else {
        return;
    };
    let learned = book.learned_level(group);
    let next = state.target as i16 + step.0 as i16;
    state.target = next.clamp(0, learned as i16) as u8;
}

/// Confirm: apply the local refund. **No packet** — the withdraw opcode is
/// unwired (`docs/re/ui/hud-skill-window.md` §6-6), and inventing one would be
/// worse than not sending it.
fn on_confirm(
    _: On<Activate>,
    index: Res<SkillGroupIndex>,
    skill_data: Res<ClientSkillData>,
    mut book: ResMut<SkillBook>,
    mut progress: ResMut<PlayerProgress>,
    mut state: ResMut<SkillWithdrawalState>,
) {
    let Some(group) = state.group else {
        return;
    };
    let Some(data) = skill_data.data() else {
        return;
    };
    let target = state.target;
    let refund = withdraw_to(&mut book, &index, data, &mut progress, group, target);
    info!("skills: withdrew group {group} to level {target}, +{refund} SP (no send opcode known)");
    state.close();
}

fn on_cancel(_: On<Activate>, mut state: ResMut<SkillWithdrawalState>) {
    state.close();
}

/// Close the window when the skill page closes — it is a satellite of that
/// page, not a window of its own.
pub fn close_withdrawal_with_skill_window(
    skill_window: Res<super::model::SkillWindowState>,
    mut state: ResMut<SkillWithdrawalState>,
) {
    if skill_window.is_changed() && !skill_window.open && state.group.is_some() {
        state.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every rect is `ifskillremovalbox.txt`'s, and the conversion to the
    /// content node is the one subtraction in [`wd`].
    #[test]
    fn the_rects_are_the_authored_ones() {
        assert_eq!(OUTER, (388.0, 435.0));
        assert_eq!(ORIGIN, (16.0, 40.0));
        assert_eq!(SKILLICON, (28.0, 80.0, 32.0, 32.0));
        assert_eq!(BTN_OK, (77.0, 212.0, 76.0, 24.0));
        assert_eq!(BTN_CANCEL, (158.0, 212.0, 76.0, 24.0));
        // com_button is 76x24 art, and the two buttons abut the same row
        assert_eq!(BTN_OK.1, BTN_CANCEL.1);
        assert_eq!(wd(SKILLICON), (12.0, 40.0, 32.0, 32.0));
    }

    /// The spinner's reading `[S]`: the arrows sit between the level being
    /// spun and the "Withdrawn Lv." caption, which is what makes
    /// `TARGETLEVEL` the spun value rather than `WITHDRAWED_LEVEL`.
    #[test]
    fn the_arrows_sit_between_the_two_level_readouts() {
        assert!(TARGETLEVEL.0 + TARGETLEVEL.2 <= BTN_RECOVER.0);
        assert!(BTN_RECOVER.0 + BTN_RECOVER.2 <= WITHDRAW_LEV.0);
        // the two arrows stack on one 12px pitch, `_up` above `_down`
        assert_eq!(BTN_RECOVER.0, BTN_DOWNGRADE.0);
        assert_eq!(BTN_DOWNGRADE.1 - BTN_RECOVER.1, BTN_RECOVER.3);
    }

    /// Everything the window draws stays inside the authored host.
    #[test]
    fn every_rect_fits_the_host() {
        for (x, y, w, h) in [
            SKILLICON,
            SKILLNAME,
            CURRENTLEVEL,
            TARGETLEVEL,
            BTN_RECOVER,
            BTN_DOWNGRADE,
            WITHDRAW_LEV,
            WITHDRAWED_LEVEL,
            TOTALPOINT,
            NEEDMONEY,
            CURRENTMONEY,
            GOLD_NEED,
            GOLD_HAVE,
            BTN_OK,
            BTN_CANCEL,
        ] {
            assert!(
                x >= ORIGIN.0 && y >= ORIGIN.1,
                "{x},{y} is outside the tile"
            );
            assert!(x + w <= OUTER.0, "{x}+{w} overflows the host width");
            assert!(y + h <= OUTER.1, "{y}+{h} overflows the host height");
        }
    }

    /// The window opens on a no-op and the spinner cannot leave `0..=learned`.
    #[test]
    fn the_spinner_opens_on_a_no_op_and_clamps() {
        let mut state = SkillWithdrawalState::default();
        state.open(7, 3);
        assert_eq!((state.group, state.target), (Some(7), 3));
        // clamping is what `on_step` applies; the arithmetic it uses:
        let learned = 3i16;
        for (from, step, expect) in [(3i16, -1i8, 2), (0, -1, 0), (3, 1, 3), (1, 1, 2)] {
            assert_eq!((from + step as i16).clamp(0, learned), expect);
        }
    }
}
