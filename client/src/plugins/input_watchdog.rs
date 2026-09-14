//! macOS stuck-modifier watchdog.
//!
//! Idea: system shortcuts that open an overlay without deactivating the app
//! (screen recording Cmd+Shift+5 being the repro) swallow the modifier
//! key-releases: the window never loses focus, winit only re-syncs per-key
//! modifier state from `flagsChanged` events, and bevy_winit drops the
//! `ModifiersChanged` event that carries the corrected state. `ButtonInput`
//! then reports Cmd/Shift held forever, and `bevy_ui_widgets` text inputs
//! stop inserting characters (their insert arm requires no modifiers held).
//! Whenever bevy believes a modifier is down, poll the OS's live modifier
//! state (`NSEvent modifierFlags`) and release the ones the OS says are up —
//! before `bevy_input_focus` dispatches keyboard input to the focused widget.

use bevy::prelude::*;

pub struct InputWatchdogPlugin;

impl Plugin for InputWatchdogPlugin {
    fn build(&self, app: &mut App) {
        #[cfg(target_os = "macos")]
        app.add_systems(
            PreUpdate,
            macos::release_stuck_modifiers
                .after(bevy::input::InputSystems)
                .before(bevy::input_focus::InputFocusSystems::Dispatch),
        );
        #[cfg(not(target_os = "macos"))]
        let _ = app;
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use bevy::ecs::system::NonSendMarker;
    use bevy::input::keyboard::{Key, KeyCode};
    use bevy::prelude::*;
    use objc2_app_kit::{NSEvent, NSEventModifierFlags};

    /// OS flag ↔ bevy logical/physical keys for each maskable modifier.
    const MODIFIERS: [(NSEventModifierFlags, Key, [KeyCode; 2]); 4] = [
        (
            NSEventModifierFlags::NSEventModifierFlagCommand,
            Key::Super,
            [KeyCode::SuperLeft, KeyCode::SuperRight],
        ),
        (
            NSEventModifierFlags::NSEventModifierFlagShift,
            Key::Shift,
            [KeyCode::ShiftLeft, KeyCode::ShiftRight],
        ),
        (
            NSEventModifierFlags::NSEventModifierFlagControl,
            Key::Control,
            [KeyCode::ControlLeft, KeyCode::ControlRight],
        ),
        (
            NSEventModifierFlags::NSEventModifierFlagOption,
            Key::Alt,
            [KeyCode::AltLeft, KeyCode::AltRight],
        ),
    ];

    pub fn release_stuck_modifiers(
        // AppKit call — keep this system on the main thread
        _main_thread: NonSendMarker,
        mut keycodes: ResMut<ButtonInput<KeyCode>>,
        mut keys: ResMut<ButtonInput<Key>>,
    ) {
        let any_held = MODIFIERS.iter().any(|(_, key, codes)| {
            keys.pressed(key.clone()) || codes.iter().any(|code| keycodes.pressed(*code))
        });
        if !any_held {
            return;
        }
        let os_flags = unsafe { NSEvent::modifierFlags_class() };
        for (flag, key, codes) in MODIFIERS {
            if os_flags.contains(flag) {
                continue;
            }
            for code in codes {
                if keycodes.pressed(code) {
                    debug!("input: releasing stuck modifier {code:?}");
                    keycodes.release(code);
                }
            }
            if keys.pressed(key.clone()) {
                keys.release(key);
            }
        }
    }
}
