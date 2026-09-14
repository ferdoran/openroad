//! Font-source knob.
//!
//! Idea: two faces can render the HUD, and which one is the *default* is a
//! trade-off rather than a fidelity question.
//!
//! * The **bundled** face is Arimo (OFL, metric-compatible with Arial,
//!   variable `wght`). It is the default because it is what the HUD is designed
//!   against — Regular, Latin, and the same file supplies its own bold.
//! * The user's own `Media.pk2` carries the original's faces
//!   (`Media/fonts/`). They are the original's look and they carry Hangul,
//!   which the bundled face does not.
//!
//! The bundled faces are the *only* fonts this repository may ship, because a
//! GPL-3.0 repo cannot redistribute the original's all-rights-reserved Korean
//! faces (#637).

use serde::Deserialize;

/// Font behaviour knobs.
#[derive(Deserialize, Debug, Clone)]
#[serde(default)]
pub struct FontSettings {
    /// Load the UI face from the user's PK2 (`Media/fonts/기본서체.ttf`)
    /// instead of the bundled Arimo.
    ///
    /// **Defaults to `false`**, which is a deliberate change: the PK2 face
    /// overwrites all four UI slots at startup, so leaving it on made the
    /// bundled face unreachable on any tree that has a PK2 — i.e. every real
    /// install. Turn it on for the original's own typography, or when the
    /// server's item and NPC names are Korean: Arimo has no Hangul, and those
    /// labels render as missing glyphs without the archive's face.
    ///
    /// Either way text never disappears. When the archive has no such file, or
    /// no PK2 is configured at all, the bundled face stays in place and the
    /// failure is logged once.
    pub pk2_faces: bool,
}

impl Default for FontSettings {
    fn default() -> Self {
        Self { pk2_faces: false }
    }
}
