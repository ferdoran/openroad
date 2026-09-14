//! Selection/hover highlight settings: the fresnel rim colors (AARRGGBB hex)
//! and strengths for the hover/selected highlight, customizable in
//! `config.yaml` without recompiling. Invalid hex warns and falls back to the
//! built-in default at resolve time (mirrors the chat/nameplate color settings).

use bevy::prelude::*;
use serde::Deserialize;

use crate::plugins::config::chat::parse_argb;

#[derive(Deserialize, Debug, Clone, Default)]
pub struct SelectionSettings {
    #[serde(default)]
    pub highlight: HighlightSettings,
    #[serde(default)]
    pub decal: DecalSettings,
}

/// The ground circle drawn under the click-selected target, tinted per entity
/// kind (AARRGGBB hex over the white `select_01.ddj` ring, additive).
#[derive(Deserialize, Debug, Clone)]
#[serde(default)]
pub struct DecalSettings {
    pub monster: String,
    pub npc: String,
    pub player: String,
    /// World-space edge length of the marker.
    pub size: f32,
}

impl Default for DecalSettings {
    fn default() -> Self {
        Self {
            monster: "FFFF7A66".into(), // light red
            npc: "FFB2D9FF".into(),     // light blue (nameplate npc hue)
            player: "FFB2D9FF".into(),
            size: 12.0,
        }
    }
}

/// Parsed decal tints (a resource, like [`HighlightColors`]).
#[derive(Resource, Debug, Clone)]
pub struct SelectionDecalColors {
    pub monster: Color,
    pub npc: Color,
    pub player: Color,
    pub size: f32,
}

/// The palette a default (or absent) `DecalSettings` block resolves to.
///
/// Needed because the resource is now `init_resource`d and (re)filled by an
/// apply system whenever `ClientConfig` changes — see
/// [`crate::plugins::settings::live`] — instead of being derived once in
/// `Plugin::build`, where a later config edit could never reach it.
impl Default for SelectionDecalColors {
    fn default() -> Self {
        DecalSettings::default().resolved()
    }
}

impl DecalSettings {
    pub fn resolved(&self) -> SelectionDecalColors {
        let defaults = DecalSettings::default();
        let parse = |field: &str, value: &str, default: &str| {
            parse_argb(value).unwrap_or_else(|| {
                warn!("config: invalid selection decal color {field}={value:?}, using {default}");
                parse_argb(default).expect("default decal colors are valid")
            })
        };
        SelectionDecalColors {
            monster: parse("monster", &self.monster, &defaults.monster),
            npc: parse("npc", &self.npc, &defaults.npc),
            player: parse("player", &self.player, &defaults.player),
            size: self.size,
        }
    }
}

#[derive(Deserialize, Debug, Clone)]
#[serde(default)]
pub struct HighlightSettings {
    /// Rim color while an entity is merely hovered (AARRGGBB).
    pub hover_rim: String,
    /// Rim color while an entity is the click-selected target (AARRGGBB).
    pub selected_rim: String,
    /// Rim glow intensity (the alpha of the rim color also scales it).
    pub rim_strength: f32,
    /// Fresnel falloff exponent — higher makes the edge thinner.
    pub rim_power: f32,
    /// Neutral emissive lighten added to the whole model while highlighted.
    pub emissive_strength: f32,
    /// Also drive the selection rim on sheen items (weapons/metal armor)
    /// through their material's rim term, so the outline matches the body's
    /// instead of only the emissive lighten. Additive on top of the
    /// existing behavior; off restores the emissive-only sheen highlight.
    pub rim_boost: bool,
}

impl Default for HighlightSettings {
    fn default() -> Self {
        Self {
            hover_rim: "FFCFE4FF".into(),    // soft cool white-blue
            selected_rim: "FFFFD873".into(), // warm gold
            rim_strength: 0.4,
            rim_power: 3.0,
            emissive_strength: 0.06,
            rim_boost: true,
        }
    }
}

/// The parsed, ready-to-use highlight settings (a resource so the highlight
/// system doesn't re-parse hex per apply).
#[derive(Resource, Debug, Clone)]
pub struct HighlightColors {
    pub hover_rim: Color,
    pub selected_rim: Color,
    pub rim_strength: f32,
    pub rim_power: f32,
    pub emissive_strength: f32,
    pub rim_boost: bool,
    /// Rim application mode, mirrored from the global `graphics.rim.mode`
    /// (set by `EntitySelectionPlugin::build`, not from the selection
    /// config): relative rims survive HDR daylight, absolute is the
    /// legacy add.
    pub rim_relative: bool,
}

/// The palette a default (or absent) `HighlightSettings` block resolves to.
///
/// Needed because the resource is now `init_resource`d and (re)filled by an
/// apply system whenever `ClientConfig` changes — see
/// [`crate::plugins::settings::live`] — instead of being derived once in
/// `Plugin::build`, where a later config edit could never reach it.
impl Default for HighlightColors {
    fn default() -> Self {
        HighlightSettings::default().resolved()
    }
}

impl HighlightSettings {
    pub fn resolved(&self) -> HighlightColors {
        let defaults = HighlightSettings::default();
        let parse = |field: &str, value: &str, default: &str| {
            parse_argb(value).unwrap_or_else(|| {
                warn!("config: invalid highlight color {field}={value:?}, using {default}");
                parse_argb(default).expect("default highlight colors are valid")
            })
        };
        HighlightColors {
            hover_rim: parse("hover_rim", &self.hover_rim, &defaults.hover_rim),
            selected_rim: parse("selected_rim", &self.selected_rim, &defaults.selected_rim),
            rim_strength: self.rim_strength,
            rim_power: self.rim_power,
            emissive_strength: self.emissive_strength,
            rim_boost: self.rim_boost,
            // overridden from graphics.rim.mode by EntitySelectionPlugin
            rim_relative: true,
        }
    }
}
