//! The one HUD scale multiplier.
//!
//! # The idea
//!
//! Vanilla authors every HUD rect in a fixed 1:1 window space sized for the
//! era's 1024x768–1280x1024 screens, so transcribing those rects verbatim
//! gives a HUD that is pixel-exact and unreadably small on a modern display.
//! OpenRoad therefore multiplies the transcribed geometry by one uniform
//! factor. The factor is **ours, not the original's** (ADR-0009: a stated
//! deliberate improvement, not a transcribed value), and until #620 it was
//! restated as an anonymous `const *_SCALE: f32 = 1.5` in 25 modules — 25
//! places that each read like data and none of which a user could change.
//!
//! # Why a process global rather than `Res<ClientConfig>`
//!
//! The multiplier is consumed inside the windows' *pure layout helpers*
//! (`fn tab(..) -> impl Bundle`, `fn label(..)`, rect math) which take no
//! system parameters at all. Threading a `f32` through every one of them is a
//! far larger diff than the value is worth, and it would still not be a single
//! source. So the value lives here, is seeded and refreshed from
//! `config.hud.hud_scale` by [`apply_hud_scale`] on the same
//! `resource_changed::<ClientConfig>` path every other live setting uses
//! (`plugins::settings::live`), and is read with [`hud_scale`].
//!
//! A change lands on the next spawn of a surface: the windows compute their
//! geometry while they are built, so already-open windows keep the scale they
//! were built with until they are reopened.

use bevy::prelude::*;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::plugins::config::ClientConfig;
use crate::plugins::settings::live::config_changed;

/// The factor the whole HUD was authored against, and the default of
/// `config.hud.hud_scale`. Not from the original's data — see the module doc.
pub const DEFAULT_HUD_SCALE: f32 = 1.5;

/// `f32` has no atomic, so the bits are. Relaxed ordering is right here: the
/// value is a single independent scalar, written by one system on the main
/// schedule and read by spawn code; there is no other state whose visibility
/// has to be ordered against it.
static HUD_SCALE: AtomicU32 = AtomicU32::new(DEFAULT_HUD_SCALE.to_bits());

/// The current HUD scale multiplier. Every HUD surface multiplies its
/// transcribed geometry by this and nothing else.
pub fn hud_scale() -> f32 {
    f32::from_bits(HUD_SCALE.load(Ordering::Relaxed))
}

/// Set the multiplier. A non-finite or non-positive value would collapse or
/// NaN out every rect in the HUD, so it is rejected with a message instead of
/// being clamped silently to some other invented number.
pub fn set_hud_scale(scale: f32) {
    if !scale.is_finite() || scale <= 0.0 {
        warn!("ignoring hud.hud_scale = {scale}: it must be finite and > 0");
        return;
    }
    HUD_SCALE.store(scale.to_bits(), Ordering::Relaxed);
}

/// Seeds the global at boot and follows later `config.yaml` edits — one path
/// for both, because `resource_changed` is true on the frame the resource is
/// inserted (`plugins::settings::live`).
pub fn apply_hud_scale(config: Res<ClientConfig>) {
    set_hud_scale(config.hud.hud_scale);
}

pub struct HudScalePlugin;

impl Plugin for HudScalePlugin {
    fn build(&self, app: &mut App) {
        // PreUpdate, so a fresh value is in place before any window spawned
        // this frame reads it.
        app.add_systems(PreUpdate, apply_hud_scale.run_if(config_changed));
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// The point of #620: the multiplier exists once. A module that
    /// re-declares its own `1.5` is back to a constant a config value cannot
    /// reach, which is the defect this issue is about — so the tree is
    /// scanned for that shape rather than trusted.
    #[test]
    fn no_module_declares_its_own_hud_scale_constant() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                // This file is where the one value is allowed to live.
                if path.file_name().and_then(|f| f.to_str()) == Some("scale.rs") {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                for (line, text) in text.lines().enumerate() {
                    let text = text.trim();
                    if (text.starts_with("const ") || text.starts_with("pub const "))
                        && text.contains("SCALE: f32 = 1.5")
                    {
                        offenders.push(format!("{}:{}", path.display(), line + 1));
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "these sites re-declare the HUD scale instead of calling \
             `hud_scale()`, so `config.hud.hud_scale` cannot reach them: {offenders:?}"
        );
    }

    /// The knob has to move the number every surface multiplies by — and a
    /// garbage value in `config.yaml` must not NaN out the whole HUD. Both
    /// live in **one** test on purpose: the value under test is a process
    /// global and cargo runs tests in threads, so two tests writing it would
    /// race each other.
    #[test]
    fn the_scale_follows_a_valid_value_and_refuses_an_invalid_one() {
        let restore = hud_scale();
        set_hud_scale(2.25);
        assert_eq!(hud_scale(), 2.25);
        for bad in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            set_hud_scale(bad);
            assert_eq!(hud_scale(), 2.25, "{bad} was accepted");
        }
        set_hud_scale(restore);
    }
}
