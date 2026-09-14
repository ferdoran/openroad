//! The shared `msgbox2_window_` modal shell.
//!
//! Idea: every dialog-style popup in the original is **one art image with
//! absolutely-positioned children** — never the bordered `mframe_wnd_` chrome
//! that `hud/game_window.rs` builds. `docs/re/ui/message-box.md` §3 states it
//! after walking all 16 `ifmessagebox.txt` sections plus `ifconfirmbox.txt`,
//! `ifcheckconfirmwnd.txt` and `pscharacterselect.txt`'s `Warning`: every one
//! of them uses a dedicated static image or a tile composite, and none uses
//! `mframe_wnd_`. So this module owns a *shell*, not a frame ring — the
//! dimming scrim, the centred art plate, and the plate's interior arithmetic.
//!
//! Deliberately **not** a rows x cols layout model. The deleted
//! `plugins/ui/message_box.rs` (removed in `d5716e2`) modelled a dialog as a
//! grid; `message-box.md` §8-1 refutes that, because the data authors every
//! child at its own rect inside the plate.
//!
//! Interior arithmetic, from the art and confirmed by an authored rect:
//! `msgbox2_window_left_up.ddj` is **16x40** and `_left_down.ddj` **16x16**
//! (DDS headers, cited in PR #621), so the plate insets its interior by 16 at
//! the sides, 40 at the top and 16 at the bottom. `ifpartymatch.txt` closes it
//! independently: `GDR_PARTYMATCH_REGISTER` is `0,0,314,373` with `_MAIN_BG`
//! `16,40,282,317` — `314 - 32 == 282` and `373 - 40 - 16 == 317`
//! (`docs/re/ui/hud-party-matching.md` §3.10).

use bevy::ecs::relationship::RelatedSpawnerCommands;
use bevy::prelude::*;

/// The 8 pieces of the `msgbox2_window_` ring, measured from their DDS headers:
/// the corners are 16 wide with a 40-tall top row and a 16-tall bottom row, the
/// mid pieces are 64 wide, and the two side pieces are 16x64.
pub const MODAL_FRAME_DIR: &str = "media://interface/messagebox/msgbox2_window_";

/// Side inset of the `msgbox2_window_` plate (`_left_up.ddj` is 16 wide).
pub const MODAL_SIDE: f32 = 16.0;
/// Top inset — the plate's top strip (`_left_up.ddj` is 40 tall).
pub const MODAL_TOP: f32 = 40.0;
/// Bottom inset (`_left_down.ddj` is 16 tall).
pub const MODAL_BOTTOM: f32 = 16.0;

/// Dimming scrim behind a modal.
///
/// **openroad convention, not the original's.** No dialog in the corpus
/// authors a backdrop, and the hosts that place these boxes carry degenerate
/// rects (`pstitle.txt:6` `GDR_CMB_CONFIRMBOX` is `0,0,1,1`), so both the
/// centring and the dimming are ours. It stays because it is also what makes
/// a modal modal: the scrim swallows clicks on the screen behind it.
pub const MODAL_SCRIM: Color = Color::srgba(0.0, 0.0, 0.0, 0.75);

/// Interior rect `(left, top, width, height)` of a plate of the given outer
/// size, in plate-local coordinates.
///
/// Dialogs whose resinfo tree authors child rects in *window* coordinates (the
/// usual case, and both current callers) do not need this — their rects are
/// already absolute inside the plate. It exists so the inset is stated once
/// rather than re-measured per dialog.
pub fn modal_interior(width: f32, height: f32) -> (f32, f32, f32, f32) {
    (
        MODAL_SIDE,
        MODAL_TOP,
        width - 2.0 * MODAL_SIDE,
        height - MODAL_TOP - MODAL_BOTTOM,
    )
}

/// The fullscreen scrim a modal sits on: click-blocking, dimming, and the
/// thing that centres the plate (the plate's `margin: auto` resolves against
/// it).
pub fn modal_scrim() -> impl Scene {
    bsn! {
        Node {
            position_type: PositionType::Absolute,
            width: percent(100),
            height: percent(100),
        }
        BackgroundColor({MODAL_SCRIM})
    }
}

/// The centred art plate. The caller passes the art's **native** size, so the
/// plate is never rescaled — a scaled plate would move every child rect the
/// data authors.
pub fn modal_plate(art: Handle<Image>, width: f32, height: f32) -> impl Scene {
    bsn! {
        ImageNode { image: {art}, image_mode: NodeImageMode::Stretch }
        Node {
            position_type: PositionType::Absolute,
            width: px(width),
            height: px(height),
            margin: {UiRect::all(Val::Auto)},
        }
    }
}

/// The scrim as a plain bundle, for the dialogs built with `commands.spawn`
/// rather than as a BSN scene. Same node, same colour as [`modal_scrim`] —
/// which stays because the scene-built callers compose it with `bsn!`.
pub fn modal_scrim_node() -> impl Bundle {
    (
        Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        },
        BackgroundColor(MODAL_SCRIM),
    )
}

/// A centred plate of the given native size, as a plain bundle. The size is the
/// art's own: a rescaled plate would move every child rect the data authors.
///
/// `Pickable::IGNORE` because the plate is a **positioning container**, not a
/// surface: it carries no art of its own (the frame and the background are
/// separate children), so it has nothing to be clicked. Every other dialog in
/// this tree marks its plate the same way — `choice_confirm`, `petition`,
/// `quest_reward_confirm`, `academy_appraisal` and `appearance_change` — and
/// this helper was the one plate that did not. A click that misses a control
/// then falls through to the scrim, which is what swallows it.
pub fn modal_plate_node(width: f32, height: f32, s: f32) -> impl Bundle {
    (
        Node {
            position_type: PositionType::Absolute,
            width: Val::Px(width * s),
            height: Val::Px(height * s),
            margin: UiRect::all(Val::Auto),
            ..default()
        },
        Pickable::IGNORE,
    )
}

/// Spawn the 8-piece `msgbox2_window_` ring as children of a plate of
/// `width` x `height`, leaving the interior to the caller.
///
/// The two side pieces **tile** vertically rather than stretch. That is
/// measured, not assumed: `msgbox2_window_left_side.ddj` is 16x64 with exactly
/// 16 distinct rows repeating at a period of 16, so stretching it over an
/// arbitrary dialog height would smear a motif the artist authored. It is the
/// same finding, and the same treatment, as `game_window`'s own side pieces.
/// The mid pieces stretch: they span a width the dialog chooses and carry no
/// horizontal period to preserve.
pub fn spawn_modal_frame(
    parent: &mut RelatedSpawnerCommands<ChildOf>,
    asset_server: &AssetServer,
    width: f32,
    height: f32,
    s: f32,
) {
    let inner_w = (width - 2.0 * MODAL_SIDE).max(0.0);
    let inner_h = (height - MODAL_TOP - MODAL_BOTTOM).max(0.0);
    let pieces: [(&str, (f32, f32, f32, f32), bool); 8] = [
        ("left_up", (0.0, 0.0, MODAL_SIDE, MODAL_TOP), false),
        ("mid_up", (MODAL_SIDE, 0.0, inner_w, MODAL_TOP), false),
        (
            "right_up",
            (width - MODAL_SIDE, 0.0, MODAL_SIDE, MODAL_TOP),
            false,
        ),
        ("left_side", (0.0, MODAL_TOP, MODAL_SIDE, inner_h), true),
        (
            "right_side",
            (width - MODAL_SIDE, MODAL_TOP, MODAL_SIDE, inner_h),
            true,
        ),
        (
            "left_down",
            (0.0, height - MODAL_BOTTOM, MODAL_SIDE, MODAL_BOTTOM),
            false,
        ),
        (
            "mid_down",
            (MODAL_SIDE, height - MODAL_BOTTOM, inner_w, MODAL_BOTTOM),
            false,
        ),
        (
            "right_down",
            (
                width - MODAL_SIDE,
                height - MODAL_BOTTOM,
                MODAL_SIDE,
                MODAL_BOTTOM,
            ),
            false,
        ),
    ];
    for (piece, rect, tile_y) in pieces {
        parent.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(rect.0 * s),
                top: Val::Px(rect.1 * s),
                width: Val::Px(rect.2 * s),
                height: Val::Px(rect.3 * s),
                ..default()
            },
            ImageNode {
                image: asset_server.load(format!("{MODAL_FRAME_DIR}{piece}.ddj")),
                image_mode: if tile_y {
                    NodeImageMode::Tiled {
                        tile_x: false,
                        tile_y: true,
                        stretch_value: s,
                    }
                } else {
                    NodeImageMode::Stretch
                },
                ..default()
            },
            Pickable::IGNORE,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The insets are the art's, and one authored rect closes on them from the
    /// other side: `GDR_PARTYMATCH_REGISTER` `0,0,314,373` with `_MAIN_BG`
    /// `16,40,282,317` (`hud-party-matching.md` §3.10).
    #[test]
    fn the_plate_insets_match_the_msgbox2_art() {
        assert_eq!((MODAL_SIDE, MODAL_TOP, MODAL_BOTTOM), (16.0, 40.0, 16.0));
        assert_eq!(modal_interior(314.0, 373.0), (16.0, 40.0, 282.0, 317.0));
    }

    /// The two modals this shell now carries keep their own authored sizes —
    /// the shell owns the chrome, not the geometry (#664 acceptance 2).
    #[test]
    fn the_shell_does_not_own_dialog_sizes() {
        // captcha: 400x180 (`ifconfirmbox.txt` art), delete: 344x192
        // (`warning_delete.ddj`). Both are the caller's, and both are larger
        // than the insets they sit in.
        for (w, h) in [(400.0, 180.0), (344.0, 192.0)] {
            let (left, top, iw, ih) = modal_interior(w, h);
            assert_eq!((left, top), (MODAL_SIDE, MODAL_TOP));
            assert!(iw > 0.0 && ih > 0.0, "{w}x{h} has no interior");
        }
    }
}
