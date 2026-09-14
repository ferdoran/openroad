//! Skill-cooldown overlays: while a skill in [`SkillCooldowns`] is not ready,
//! its icon — underbar quickslot and skill-window cell alike — is covered by
//! the original's clock sweep with the remaining whole seconds centered on it.
//! One per-frame system owns BOTH surfaces.
//!
//! **The sweep is the original's own art, one frame per draw.**
//! `interface/skill/skill_delay.ddj` is a 256x256 sheet that tiles as a 16x16
//! grid of 16x16-px frames; 236 of the 256 cells carry ink (0..=235) and their
//! opaque-pixel counts fall monotonically 256 -> 12, i.e. frame 0 is a full
//! black disc and frame 235 a 2 px sliver at twelve o'clock
//! (`docs/re/ui/hud-cooldown.md` §3, measured from the file). So the animation
//! is an *index*, not a shader: the remaining fraction of the skill's
//! `Action_ReuseDelay` picks one of the 236 frames, and `ImageNode::rect`
//! samples it out of the sheet — bevy's documented one-off alternative to a
//! `TextureAtlas`, which keeps this to two fields on an existing node instead
//! of a new atlas asset.
//!
//! This replaces a flat blue multiplicative tint (#369): the tint was our own
//! visual language and the wedge is the client's.
//!
//! Deviation: the sheet's frames are 16x16 while the surfaces they cover are
//! 28 px (skill window) and 32 px (quickslot), so the frame is stretched to the
//! icon instead of drawn at its authored size — a 16 px disc centred on a 32 px
//! icon would leave the icon's corners uncovered, which is the one thing a
//! cooldown overlay must not do.
//!
//! Still ours, not the original's: the remaining seconds are TTF text rather
//! than the `effect/icon/cool_time_0..9.ddj` glyph strip (build step 3 of #369).

use bevy::prelude::*;

use crate::plugins::hud::skill_window::ui::SkillCell;
use crate::plugins::hud::underbar::model::{QuickSlots, SlotAction};
use crate::plugins::hud::underbar::ui::{UbSlotCell, UbSpecialSlotCell};
use crate::plugins::skills::cast::SkillCooldowns;
use crate::plugins::textdata::ClientSkillData;

/// The original's cooldown sweep sheet (256x256, 16x16 frames of 16x16 px).
pub const WEDGE_ART: &str = "media://interface/skill/skill_delay.ddj";
/// Frames per row/column in that sheet.
const WEDGE_GRID: usize = 16;
/// One frame's edge, in source pixels.
const WEDGE_FRAME_PX: f32 = 16.0;
/// Cells 0..=235 carry ink; the remaining 20 are fully transparent.
pub const WEDGE_FRAMES: usize = 236;

/// Which sweep frame shows `fraction` of the cooldown still to run.
///
/// Frame 0 is the full disc, so a just-started cooldown (`1.0`) is 0 and a
/// nearly-finished one (`0.0`) is the last inked frame, 235.
pub fn wedge_frame(fraction_remaining: f32) -> usize {
    // NaN (a zero-length delay divided into itself) is treated as "just
    // started": the full disc is the safe end of the ramp, since it can only
    // ever be too cautious for one frame.
    let fraction = if fraction_remaining.is_nan() {
        1.0
    } else {
        fraction_remaining.clamp(0.0, 1.0)
    };
    let last = WEDGE_FRAMES - 1;
    (((1.0 - fraction) * last as f32).round() as usize).min(last)
}

/// The source rect of sweep frame `index`, row-major in the 16x16 grid.
pub fn wedge_rect(index: usize) -> Rect {
    let index = index.min(WEDGE_FRAMES - 1);
    let x = (index % WEDGE_GRID) as f32 * WEDGE_FRAME_PX;
    let y = (index / WEDGE_GRID) as f32 * WEDGE_FRAME_PX;
    Rect::new(x, y, x + WEDGE_FRAME_PX, y + WEDGE_FRAME_PX)
}

/// The overlay's image node, for the two surfaces that spawn one.
///
/// Stretched, not centred — see the deviation note in the module comment.
pub fn wedge_image(asset_server: &AssetServer) -> ImageNode {
    ImageNode {
        image: asset_server.load(WEDGE_ART),
        image_mode: NodeImageMode::Stretch,
        rect: Some(wedge_rect(0)),
        ..default()
    }
}

/// The flex-centered wrapper over an icon that holds the countdown text;
/// hidden while the skill is ready.
#[derive(Component)]
pub struct CooldownOverlay;

/// The countdown seconds text inside a [`CooldownOverlay`].
#[derive(Component)]
pub struct CooldownText;

/// Remaining cooldown of `skill_id`, `None` when ready/absent.
fn remaining(cooldowns: &SkillCooldowns, now: f64, skill_id: u32) -> Option<f32> {
    cooldowns
        .0
        .get(&skill_id)
        .map(|&at| (at - now) as f32)
        .filter(|r| *r > 0.0)
}

/// Drive one icon's sweep + countdown from a remaining-time reading.
///
/// `total` is the skill's full `Action_ReuseDelay`; without it (skilldata not
/// loaded, or a skill the table does not know) there is no fraction to sweep,
/// so the overlay shows the full disc rather than a guessed position.
fn apply(
    remaining: Option<f32>,
    total: Option<f32>,
    cell_children: &Children,
    overlays: &mut Query<(&mut Visibility, &mut ImageNode, &Children), With<CooldownOverlay>>,
    texts: &mut Query<&mut Text, With<CooldownText>>,
) {
    for child in cell_children.iter() {
        let Ok((mut visibility, mut wedge, overlay_children)) = overlays.get_mut(child) else {
            continue;
        };
        let target = match remaining {
            Some(_) => Visibility::Inherited,
            None => Visibility::Hidden,
        };
        if *visibility != target {
            *visibility = target;
        }
        if let Some(remaining) = remaining {
            let fraction = total
                .filter(|total| *total > 0.0)
                .map(|total| remaining / total)
                .unwrap_or(1.0);
            let rect = Some(wedge_rect(wedge_frame(fraction)));
            if wedge.rect != rect {
                wedge.rect = rect;
            }
            let seconds = format!("{}", remaining.ceil() as u32);
            for text_child in overlay_children.iter() {
                if let Ok(mut text) = texts.get_mut(text_child) {
                    if text.0 != seconds {
                        text.0 = seconds.clone();
                    }
                }
            }
        }
    }
}

/// Per-frame cooldown display over both skill surfaces. Accepted edge
/// (cosmetic): leveling a skill mid-cooldown re-keys its slot to the new
/// rung id, orphaning the running display until the old entry lapses.
#[allow(clippy::type_complexity)]
pub fn update_cooldown_overlays(
    time: Res<Time>,
    cooldowns: Res<SkillCooldowns>,
    quickslots: Res<QuickSlots>,
    skill_data: Res<ClientSkillData>,
    ub_cells: Query<(&UbSlotCell, &Children)>,
    special_cells: Query<&Children, (With<UbSpecialSlotCell>, Without<UbSlotCell>)>,
    window_cells: Query<(&SkillCell, &Children), Without<CooldownOverlay>>,
    mut overlays: Query<(&mut Visibility, &mut ImageNode, &Children), With<CooldownOverlay>>,
    mut texts: Query<&mut Text, With<CooldownText>>,
) {
    let now = time.elapsed_secs_f64();
    // the sweep's denominator: skilldata col 14 Action_ReuseDelay, the same
    // value the cooldown was started from
    let total_secs = |skill_id: u32| -> Option<f32> {
        skill_data
            .get(&(skill_id as i32))
            .map(|row| row.reuse_delay_ms() as f32 / 1000.0)
            .filter(|secs| *secs > 0.0)
    };
    let slot_remaining = |action: Option<SlotAction>| -> (Option<f32>, Option<f32>) {
        match action {
            Some(SlotAction::Skill { ref_id }) => {
                (remaining(&cooldowns, now, ref_id), total_secs(ref_id))
            }
            _ => (None, None),
        }
    };

    // underbar: the overlay is a child of the (image-less) hit-area cell
    for (cell, children) in ub_cells.iter() {
        let (left, total) = slot_remaining(quickslots.visible(cell.0));
        apply(left, total, children, &mut overlays, &mut texts);
    }
    for children in special_cells.iter() {
        let (left, total) = slot_remaining(quickslots.special);
        apply(left, total, children, &mut overlays, &mut texts);
    }

    // skill window: only learned cells are eligible — an unlearned cell shows
    // the gray/locked-book art and has no cooldown to run
    for (cell, children) in window_cells.iter() {
        let Some(learned_id) = cell.learned_id else {
            continue;
        };
        let left = remaining(&cooldowns, now, learned_id as u32);
        apply(
            left,
            total_secs(learned_id as u32),
            children,
            &mut overlays,
            &mut texts,
        );
    }
}

/// Self-registration for the quickslot/skill cooldown overlays (#558). The HUD registry holds one line per
/// window, so two windows landing in the same lap no longer collide on it.
pub struct CooldownPlugin;

impl Plugin for CooldownPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, update_cooldown_overlays.run_if(super::hud_scenes));
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// #369: the sweep is an index into the original's own 236-frame sheet, so
    /// the two ends have to land exactly — frame 0 is the full disc (nothing of
    /// the cooldown has run) and frame 235 the last sliver (it is about to end).
    /// A fraction outside 0..=1 (a replayed cooldown longer than the table's
    /// delay, say) must clamp rather than index past the inked cells.
    #[test]
    fn wedge_frames_run_from_the_full_disc_to_the_last_sliver() {
        assert_eq!(wedge_frame(1.0), 0);
        assert_eq!(wedge_frame(0.0), WEDGE_FRAMES - 1);
        // half-way is 117.5 of 235, and `round` takes it away from zero
        assert_eq!(wedge_frame(0.5), 118);
        assert_eq!(wedge_frame(2.0), 0, "over 100% remaining clamps to full");
        assert_eq!(wedge_frame(-1.0), WEDGE_FRAMES - 1);
        assert_eq!(wedge_frame(f32::NAN), 0, "NaN shows the full disc");

        // as time runs down the frame index only ever rises
        let mut previous = 0;
        for step in 0..=100 {
            let fraction = 1.0 - step as f32 / 100.0;
            let frame = wedge_frame(fraction);
            assert!(frame >= previous, "frame went backwards at {fraction}");
            assert!(frame < WEDGE_FRAMES);
            previous = frame;
        }
    }

    /// The frames are read out of a 256x256 sheet as a row-major 16x16 grid of
    /// 16x16 cells; an off-by-one here samples a neighbouring frame, which looks
    /// like a jittering clock rather than a crash.
    #[test]
    fn wedge_rects_tile_the_sheet_row_major() {
        assert_eq!(wedge_rect(0), Rect::new(0.0, 0.0, 16.0, 16.0));
        assert_eq!(wedge_rect(15), Rect::new(240.0, 0.0, 256.0, 16.0));
        assert_eq!(wedge_rect(16), Rect::new(0.0, 16.0, 16.0, 32.0));
        // the last inked cell: 235 = row 14, column 11
        assert_eq!(
            wedge_rect(WEDGE_FRAMES - 1),
            Rect::new(176.0, 224.0, 192.0, 240.0)
        );
        // and nothing ever samples outside the sheet
        for index in 0..WEDGE_FRAMES {
            let rect = wedge_rect(index);
            assert!(rect.max.x <= 256.0 && rect.max.y <= 256.0);
            assert_eq!(rect.max - rect.min, Vec2::splat(WEDGE_FRAME_PX));
        }
        // an out-of-range index is pinned to the last inked frame rather than
        // sampling one of the 20 empty cells
        assert_eq!(wedge_rect(WEDGE_FRAMES), wedge_rect(WEDGE_FRAMES - 1));
    }
}
