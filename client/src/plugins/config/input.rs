//! Mouse-scheme knob for the camera/shortcut split.
//!
//! Idea: vanilla's Input pane offers a **two-state radio** — the wheel either
//! changes the view or uses a shortcut, and the right button always takes the
//! other role (`textuisystem.txt` 917/918, `docs/re/ui/options-controls.md`
//! §3). That choice is persisted as SROptionSet id **3101**
//! (`isMouseShortcutSwapped`). openroad's own scheme — wheel zooms, right
//! button orbits — is *neither* of those states: it puts both devices on the
//! camera and leaves no device for shortcuts. Per ADR-0009 that deviation is
//! allowed but must be **named**, which is what this setting does: it selects
//! between our scheme and letting id 3101 decide.

use serde::Deserialize;

/// Which mouse scheme the follow camera obeys.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum MouseScheme {
    /// **Non-original (openroad).** Wheel zooms *and* the right button orbits,
    /// so both devices drive the camera and neither is free for shortcuts.
    /// The default, because it is the scheme our gameplay code is built around
    /// and the "use shortcut" half of vanilla's pair has no implementation yet
    /// (`docs/re/ui/options-controls.md` §9).
    #[default]
    ZoomOrbit,
    /// Vanilla's two-state pair, selected by the persisted SROptionSet id 3101:
    /// exactly one device changes the view and the other is reserved for
    /// shortcut use.
    Vanilla,
}

/// Input behaviour knobs.
#[derive(Deserialize, Debug, Clone, Default)]
#[serde(default)]
pub struct InputSettings {
    /// See [`MouseScheme`]. `zoom_orbit` (default) keeps openroad's scheme;
    /// `vanilla` hands the wheel/right-button roles to option id 3101.
    pub mouse_scheme: MouseScheme,
}
