//! The system-message surface (`GDR_SYSTEM_MESSAGE_VIEW`, `CIFSystemMessage`,
//! id 68) — see `docs/re/ui/hud-system-message.md`.
//!
//! `model` owns the message classes and the printf-template formatter; `ui`
//! owns the transcribed geometry and the hosting relation (a SIBLING of the
//! chat board, created next to it in `ginterface.txt`, not a child or a tab of
//! it). The view is deliberately not spawned yet: the
//! original's frame (`0,552,350,126`) is fully contained in the chat board's
//! (`0,546,399,398`), resinfo has no `ZOrder`/`Visible` key to arbitrate them,
//! and whether id 68 is the collapsed-chat body inset 6px or an independently
//! toggled surface is `[S]`/`[U]` (§8-1, §9-U1/U6). Spawning first and deciding
//! later would overlap our collapsed chat on every pixel.

pub mod model;
pub mod ui;

use bevy::prelude::*;

/// Self-registration for the system-message log (#558). The HUD registry holds one line per
/// window, so two windows landing in the same lap no longer collide on it.
pub struct SystemMessagePlugin;

impl Plugin for SystemMessagePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<model::SystemMessageLog>();
    }
}
