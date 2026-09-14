//! Which HUD window Escape closes, and which dialog Enter confirms.
//!
//! Idea: the client had **no notion of window focus at all**. Layering is ~30
//! hand-assigned `GlobalZIndex` literals that never change, and Escape ran a
//! chain in `plugins::system_window` that closed the Esc menu, dropped the
//! target, or opened the Esc menu — and never touched a HUD window, so an open
//! inventory could only be closed by its own (X) or its own hotkey.
//!
//! This adds the smallest thing that fixes that: a **recency stamp** per
//! window. A window is stamped when it opens and re-stamped whenever a click
//! lands anywhere inside it, so "the focused window" is simply the visible one
//! with the highest stamp. No z-order is touched — raising a window on focus
//! would fight the authored ladder, and nothing asked for it.
//!
//! # Escape closes a window by pressing its own (X)
//!
//! [`HudWindow`] stores the window's close **button**, not a closer of its own,
//! and Escape triggers `Activate` on it. Every window already wires that button
//! to whatever closing means for it — clearing a `*State.open` flag, despawning
//! a rebuild-on-change tree, ending a session — so Escape reuses that path
//! instead of duplicating it, and the two cannot drift apart. Opting a window
//! in is one line next to the close-button wiring it already has.
//!
//! # The Escape chain
//!
//! Escape is now a strict priority order, and [`EscConsumed`] is what carries
//! the decision between the stages:
//!
//! 1. **transients** — an armed item, a carried skill, an open drop or practice
//!    prompt. These cancel and consume.
//! 2. **the focused window** (here).
//! 3. **the target**, then the Esc menu (`system_window::toggle_system_window`).
//!
//! Before this, stage 1 did not consume: cancelling an armed item *also* opened
//! the Esc menu in the same press, because nothing told the menu the press had
//! already been used.
//!
//! # Enter confirms a dialog by pressing its own OK
//!
//! [`HudDialog`] is the same trick for the other key: it names the dialog's
//! confirm button, and Enter triggers `Activate` on it. Every one of the ~20
//! dialogs in this tree already wires that button to whatever confirming means
//! for it — sending the request, closing the prompt, answering the petition —
//! so Enter reuses those handlers untouched and cannot drift from what the
//! mouse does. Opting in is one line next to the button the dialog already
//! builds.
//!
//! Enter has its own chain, because it already had an owner: pressing it with
//! no dialog up opens the chat box (`chat::input::handle_chat_enter`). So
//! [`EnterConsumed`] mirrors [`EscConsumed`] and the dialog stage runs first —
//! without that, one press would confirm the dialog *and* drop the player into
//! the chat input, the exact double-consumption `EscConsumed` exists to stop.
//!
//! Dialogs share the window recency stamp rather than a z-order scan: a
//! confirm prompt is almost always the most recently opened thing on screen,
//! which is precisely what the stamp already measures.

use bevy::prelude::*;
use bevy::ui_widgets::Activate;

use crate::plugins::hud::chat::model::ChatState;

/// Marks a window root that Escape may close, and names the control it uses.
#[derive(Component, Debug, Clone, Copy)]
pub struct HudWindow {
    /// The window's own (X). Escape triggers `Activate` here — see the module
    /// docs for why this is a button rather than a closer.
    pub close_button: Entity,
}

/// Marks a dialog root that Enter may confirm, and names the control it uses.
///
/// Sits alongside [`HudWindow`] rather than extending it: a window is a
/// persistent surface with an (X), a dialog is a transient prompt with an OK,
/// and plenty of dialogs have no close button to give `HudWindow` at all. A
/// root may carry both when it genuinely has both controls.
#[derive(Component, Debug, Clone, Copy)]
pub struct HudDialog {
    /// The dialog's confirm/OK/Yes control. Enter triggers `Activate` here —
    /// see the module docs for why this is a button rather than a callback.
    pub confirm_button: Entity,
}

/// Recency of a window: higher is more recently opened or clicked.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct FocusStamp(pub u64);

/// The stamp counter. Monotonic for the session; at one stamp per click it
/// cannot realistically wrap.
#[derive(Resource, Default)]
pub struct HudFocus {
    next: u64,
}

impl HudFocus {
    fn stamp(&mut self) -> FocusStamp {
        self.next += 1;
        FocusStamp(self.next)
    }
}

/// Whether this frame's Escape press has already been used.
///
/// Cleared at the top of every frame rather than by its readers, so a stage
/// that forgets to reset it cannot swallow every future press.
#[derive(Resource, Default)]
pub struct EscConsumed(pub bool);

impl EscConsumed {
    /// Take the press for this frame. Returns `false` if something earlier
    /// already did, which is the caller's cue to do nothing.
    pub fn claim(&mut self) -> bool {
        if self.0 {
            return false;
        }
        self.0 = true;
        true
    }
}

/// Whether this frame's Enter press has already been used.
///
/// The twin of [`EscConsumed`], and cleared the same way. Enter needs one
/// because it already had a default owner — the chat box — so a dialog
/// confirming on Enter must be able to say so before the chat input reads it.
#[derive(Resource, Default)]
pub struct EnterConsumed(pub bool);

impl EnterConsumed {
    /// Take the press for this frame. Returns `false` if something earlier
    /// already did, which is the caller's cue to do nothing.
    pub fn claim(&mut self) -> bool {
        if self.0 {
            return false;
        }
        self.0 = true;
        true
    }
}

/// True on the frame Enter (either one) went down.
pub fn enter_pressed(keys: &ButtonInput<KeyCode>) -> bool {
    keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter)
}

pub struct HudFocusPlugin;

impl Plugin for HudFocusPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<HudFocus>()
            .init_resource::<EscConsumed>()
            .init_resource::<EnterConsumed>()
            .add_systems(First, (clear_esc_consumed, clear_enter_consumed))
            .add_systems(Update, stamp_new_windows)
            // Ahead of the chat box, which owns Enter when no dialog is up.
            .add_systems(
                Update,
                confirm_focused_dialog_on_enter
                    .before(crate::plugins::hud::chat::input::handle_chat_enter),
            )
            // Ordered between the two ends of the Escape chain: after the
            // transients that cancel and consume, before the Esc menu. Stated
            // here rather than at each participant so the whole order is
            // readable in one place.
            .add_systems(
                Update,
                close_focused_window_on_esc
                    .after(crate::plugins::hud::inventory::use_on_item::cancel_armed_item)
                    .after(crate::plugins::hud::inventory::drop_item::cancel_drop_confirm)
                    .before(crate::plugins::system_window::toggle_system_window),
            )
            .add_observer(stamp_clicked_window);
    }
}

fn clear_esc_consumed(mut consumed: ResMut<EscConsumed>) {
    if consumed.0 {
        consumed.0 = false;
    }
}

fn clear_enter_consumed(mut consumed: ResMut<EnterConsumed>) {
    if consumed.0 {
        consumed.0 = false;
    }
}

/// A window that just appeared is the focused one — opening it is the most
/// recent thing the player did with a window.
fn stamp_new_windows(
    fresh: Query<Entity, (Or<(With<HudWindow>, With<HudDialog>)>, Without<FocusStamp>)>,
    mut focus: ResMut<HudFocus>,
    mut commands: Commands,
) {
    for entity in fresh.iter() {
        let stamp = focus.stamp();
        commands.entity(entity).insert(stamp);
    }
}

/// Any press inside a window focuses it. The press is observed globally and
/// walked up the hierarchy, because the thing actually hit is a slot, a button
/// or a label — never the root itself.
fn stamp_clicked_window(
    press: On<Pointer<Press>>,
    parents: Query<&ChildOf>,
    windows: Query<(), Or<(With<HudWindow>, With<HudDialog>)>>,
    mut focus: ResMut<HudFocus>,
    mut commands: Commands,
) {
    let mut entity = press.entity;
    loop {
        if windows.contains(entity) {
            let stamp = focus.stamp();
            commands.entity(entity).insert(stamp);
            return;
        }
        let Ok(parent) = parents.get(entity) else {
            return;
        };
        entity = parent.parent();
    }
}

/// Escape closes the focused window, if one is open.
///
/// "Open" is read off the root's own `Node.display` and `Visibility`: the two
/// window lifecycles in this tree are spawn-once-hidden (toggled through
/// `display`) and rebuild-on-change (the root simply does not exist), and both
/// are covered by asking the roots that exist whether they are drawn.
pub fn close_focused_window_on_esc(
    keys: Res<ButtonInput<KeyCode>>,
    chat: Res<ChatState>,
    options: Query<(), With<crate::plugins::options_window::OptionsWindow>>,
    windows: Query<(Entity, &HudWindow, &FocusStamp, &Node, &Visibility)>,
    mut consumed: ResMut<EscConsumed>,
    mut commands: Commands,
) {
    if !keys.just_pressed(KeyCode::Escape) {
        return;
    }
    // Escape while typing belongs to the chat input, and while the options
    // window is up belongs to that window's own `close_on_esc` — the same two
    // guards `toggle_system_window` carries, for the same reasons.
    if chat.input_open || !options.is_empty() {
        return;
    }
    let Some((_, window, ..)) = windows
        .iter()
        .filter(|(_, _, _, node, visibility)| {
            node.display != Display::None && **visibility != Visibility::Hidden
        })
        .max_by_key(|(_, _, stamp, _, _)| **stamp)
    else {
        return;
    };
    if !consumed.claim() {
        return;
    }
    commands.trigger(Activate {
        entity: window.close_button,
    });
}

/// Enter confirms the focused dialog, if one is open.
///
/// The mirror of [`close_focused_window_on_esc`], down to reading "open" off
/// the root's own `Node.display`/`Visibility` so it covers both window
/// lifecycles in this tree (spawn-once-hidden and rebuild-on-change).
///
/// Claiming [`EnterConsumed`] is what keeps the press from also reaching
/// `chat::input::handle_chat_enter` and opening the chat box underneath the
/// dialog the player just dismissed.
pub fn confirm_focused_dialog_on_enter(
    keys: Res<ButtonInput<KeyCode>>,
    chat: Res<ChatState>,
    options: Query<(), With<crate::plugins::options_window::OptionsWindow>>,
    dialogs: Query<(&HudDialog, &FocusStamp, &Node, &Visibility)>,
    mut consumed: ResMut<EnterConsumed>,
    mut commands: Commands,
) {
    if !enter_pressed(&keys) {
        return;
    }
    // Enter while typing belongs to the chat input, and while the options
    // window is up that window owns the keyboard — the same two guards the
    // Escape stage carries, for the same reasons.
    if chat.input_open || !options.is_empty() {
        return;
    }
    let Some((dialog, ..)) = dialogs
        .iter()
        .filter(|(_, _, node, visibility)| {
            node.display != Display::None && **visibility != Visibility::Hidden
        })
        .max_by_key(|(_, stamp, _, _)| **stamp)
    else {
        return;
    };
    if !consumed.claim() {
        return;
    }
    commands.trigger(Activate {
        entity: dialog.confirm_button,
    });
}

#[cfg(test)]
mod test {
    use super::*;

    /// Stamps are strictly increasing, which is the whole ordering.
    #[test]
    fn stamps_increase_so_the_newest_window_wins() {
        let mut focus = HudFocus::default();
        let first = focus.stamp();
        let second = focus.stamp();
        assert!(second > first);
    }

    /// One press, one consumer. Before this, cancelling an armed item with
    /// Escape *also* opened the Esc menu, because nothing recorded that the
    /// press had been used.
    #[test]
    fn a_press_can_only_be_claimed_once() {
        let mut consumed = EscConsumed::default();
        assert!(consumed.claim(), "the first stage must get the press");
        assert!(!consumed.claim(), "a second stage must not also act");
        assert!(!consumed.claim());
    }

    /// The flag is per-frame: a claim in one frame must not swallow the next
    /// frame's press.
    #[test]
    fn the_claim_does_not_survive_the_frame() {
        let mut app = App::new();
        app.init_resource::<EscConsumed>()
            .add_systems(First, clear_esc_consumed);
        app.world_mut().resource_mut::<EscConsumed>().0 = true;
        app.update();
        assert!(!app.world().resource::<EscConsumed>().0);
    }
}
