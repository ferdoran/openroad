//! Restore and record the positions of the windows the original persists.
//!
//! Idea: `hud/game_window.rs` gives every framed window a title-bar drag, and
//! until now the client threw the result away on despawn — reopening a window
//! always snapped it back to its spawn anchor. The original remembers ten of
//! its windows in `wndpos.dat`; we reproduce that *set* through
//! [`WndPosSlot`] and store the anchor in `user_settings.yaml` instead of the
//! original's file (see `settings::window_positions` for why).
//!
//! A window opts in by carrying [`PersistedWindow`] on its root. Everything
//! else here is two systems:
//!
//! * **restore** — on spawn, apply the saved anchor if there is one. It rides
//!   `Added<PersistedWindow>` rather than a load-time latch, so a window that
//!   opens after the settings file loads still gets its position (#647: the
//!   apply mechanism is change detection, never `Plugin::build`).
//! * **record** — on mouse-button *release*, copy each persisted window's
//!   current anchor into [`GameOptions`]. Writing during the drag would push a
//!   new value every frame, and `persistence::save_on_change` would rewrite
//!   the settings file every frame with it.
//!
//! **The restore is clamped, and that clamp is ours, not the data's.** A saved
//! position can be off-screen — the original's own sample stores `y = -8`, and
//! a resolution change can strand a window far outside the viewport. Since our
//! title bar *is* the drag handle, an off-screen title bar is an unrecoverable
//! window for a player who cannot drag it back (WCAG 2.2 AA, and
//! `docs/re/ui/wndpos-persistence.md` §8 step 5 calls for exactly this). So the
//! restore keeps the title bar reachable. No fidelity is lost: we never load
//! the original's file, so its legal negative offsets never reach this code.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::plugins::settings::options::GameOptions;
use crate::plugins::settings::window_positions::WndPosSlot;

/// Marks a window root whose position is remembered, and which slot it is.
#[derive(Component, Debug, Clone, Copy)]
pub struct PersistedWindow(pub WndPosSlot);

/// How much of a restored window must stay inside the viewport for its title
/// bar to remain grabbable. One title-bar height's worth of chrome.
const MIN_VISIBLE_PX: f32 = 48.0;

pub struct WindowPositionsPlugin;

impl Plugin for WindowPositionsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (restore_window_positions, record_window_positions));
    }
}

/// Clamp a saved `(right, top)` anchor so the title bar stays reachable in a
/// `viewport`-sized window for a window of `size`.
///
/// The root is right/top anchored, so the window spans
/// `x = viewport.0 - right - width ..= viewport.0 - right`.
pub fn clamp_anchor(anchor: (f32, f32), size: (f32, f32), viewport: (f32, f32)) -> (f32, f32) {
    let (right, top) = anchor;
    // at least MIN_VISIBLE_PX of the window inside the left and right edges
    let right_min = MIN_VISIBLE_PX - size.0;
    let right_max = (viewport.0 - MIN_VISIBLE_PX).max(right_min);
    // the title bar sits at the very top of the window, so it must not go
    // above the viewport at all — unlike the side edges there is nothing left
    // to grab once it does
    let top_max = (viewport.1 - MIN_VISIBLE_PX).max(0.0);
    (right.clamp(right_min, right_max), top.clamp(0.0, top_max))
}

/// Apply the saved anchor to a window that just spawned.
fn restore_window_positions(
    options: Res<GameOptions>,
    primary: Query<&Window, With<PrimaryWindow>>,
    mut windows: Query<(&PersistedWindow, &mut Node), Added<PersistedWindow>>,
) {
    if windows.is_empty() {
        return;
    }
    let viewport = primary
        .single()
        .map(|w| (w.width(), w.height()))
        .unwrap_or((1280.0, 720.0));
    for (persisted, mut node) in windows.iter_mut() {
        let Some(anchor) = options.windows.get(persisted.0) else {
            continue;
        };
        let (Val::Px(w), Val::Px(h)) = (node.width, node.height) else {
            continue;
        };
        let (right, top) = clamp_anchor(anchor, (w, h), viewport);
        node.right = Val::Px(right);
        node.top = Val::Px(top);
    }
}

/// Record where the player let go of a window.
fn record_window_positions(
    buttons: Res<ButtonInput<MouseButton>>,
    mut options: ResMut<GameOptions>,
    windows: Query<(&PersistedWindow, &Node)>,
) {
    if !buttons.just_released(MouseButton::Left) {
        return;
    }
    let mut changed = false;
    for (persisted, node) in windows.iter() {
        let (Val::Px(right), Val::Px(top)) = (node.right, node.top) else {
            continue;
        };
        // bypass_change_detection: writing the map is what decides whether the
        // settings file is rewritten, so only a real move may mark it changed
        changed |= options
            .bypass_change_detection()
            .windows
            .set(persisted.0, (right, top));
    }
    if changed {
        options.set_changed();
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// A window saved at a sane position is restored verbatim.
    #[test]
    fn a_sane_anchor_survives_the_clamp() {
        let anchor = clamp_anchor((120.0, 80.0), (400.0, 300.0), (1920.0, 1080.0));
        assert_eq!(anchor, (120.0, 80.0));
    }

    /// A position saved on a wider screen, or dragged past an edge, must not
    /// strand the title bar outside the viewport — our title bar is the only
    /// drag handle, so an unreachable one is an unrecoverable window.
    #[test]
    fn an_offscreen_anchor_keeps_the_title_bar_reachable() {
        let viewport = (800.0, 600.0);
        let size = (400.0, 300.0);
        // saved on a 1920-wide screen: `right` far beyond this viewport
        let (right, _) = clamp_anchor((1600.0, 40.0), size, viewport);
        assert!(right <= viewport.0 - MIN_VISIBLE_PX);
        // dragged off the right edge: still MIN_VISIBLE_PX of window on screen
        let (right, _) = clamp_anchor((-900.0, 40.0), size, viewport);
        assert_eq!(right, MIN_VISIBLE_PX - size.0);
        assert!(viewport.0 - right - size.0 <= viewport.0 - MIN_VISIBLE_PX);
        // above the top edge, and below the bottom edge
        assert_eq!(clamp_anchor((10.0, -50.0), size, viewport).1, 0.0);
        assert_eq!(
            clamp_anchor((10.0, 5000.0), size, viewport).1,
            viewport.1 - MIN_VISIBLE_PX
        );
    }

    /// A viewport smaller than the margin must still produce a finite, ordered
    /// clamp rather than panicking on an inverted range.
    #[test]
    fn a_tiny_viewport_does_not_invert_the_clamp() {
        let anchor = clamp_anchor((10.0, 10.0), (400.0, 300.0), (20.0, 20.0));
        assert!(anchor.0.is_finite() && anchor.1.is_finite());
        assert_eq!(anchor.1, 0.0);
    }
}
