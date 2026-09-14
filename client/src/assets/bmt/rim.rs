//! Fresnel rim material (`sro_rim.wgsl`): standard PBR plus an additive
//! edge glow. Two users share it:
//!  - the always-on subtle rim on character-class meshes (mobile-port
//!    parity, gap #1 in `docs/rendering-mobile-shader-comparison.md`) —
//!    the `.rim` labeled sub-asset built by the `.bmt` loader and picked
//!    at spawn for `res/char|mob|npc|cos|pet2` resources;
//!  - the hover/click selection highlight
//!    (`plugins/cursor/interactions/entity_select.rs`), which overrides
//!    the rim parameters with the stronger selection color.
//! (Sheen materials carry the same rim term inside `sro_sheen.wgsl`
//! instead — bevy allows one extension per `ExtendedMaterial`.)

use bevy::math::Vec4;
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::{Asset, Color, StandardMaterial};
use bevy::reflect::TypePath;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

/// Uniform for `sro_rim.wgsl`; field order must match the shader struct.
#[derive(Clone, Copy, Debug, ShaderType)]
pub struct RimSettings {
    /// rgb = rim tint (scene-linear), a = strength.
    pub color: Vec4,
    /// Fresnel falloff exponent — higher makes the edge thinner.
    pub power: f32,
    /// 0 = absolute add (a scene-linear constant — vanishes against HDR
    /// daylight), 1 = relative (`lit × (1 + rim·fresnel)`), which keeps the
    /// rim-to-lit ratio at any exposure/time of day.
    pub mode: f32,
}

impl Default for RimSettings {
    fn default() -> Self {
        Self {
            color: Vec4::ZERO,
            power: 3.0,
            mode: 0.0,
        }
    }
}

#[derive(Asset, AsBindGroup, TypePath, Debug, Clone, Default)]
pub struct RimExtension {
    #[uniform(100)]
    pub settings: RimSettings,
}

impl MaterialExtension for RimExtension {
    fn fragment_shader() -> ShaderRef {
        "shaders/sro_rim.wgsl".into()
    }
}

/// Standard PBR plus a fresnel rim glow.
pub type SroRimMaterial = ExtendedMaterial<StandardMaterial, RimExtension>;

/// Build the rim uniform from a config color + strengths (the color's own
/// alpha scales the strength, so a fully-opaque hex gives exactly
/// `strength`). `relative` picks the exposure-independent application (see
/// [`RimSettings::mode`]).
pub fn rim_settings(color: Color, strength: f32, power: f32, relative: bool) -> RimSettings {
    let c = color.to_linear();
    RimSettings {
        color: Vec4::new(c.red, c.green, c.blue, c.alpha * strength),
        power,
        mode: if relative { 1.0 } else { 0.0 },
    }
}

#[cfg(test)]
mod tests {
    /// Every shader that replaces bevy's stock PBR fragment has to run
    /// `alpha_discard` itself, because `pbr_input_from_standard_material` only
    /// samples — it does no alpha handling. Missing it both skips the
    /// `AlphaMode::Mask` cutout in the main pass (while the depth prepass still
    /// discards, so the passes disagree) and leaks the raw sampled alpha into
    /// the render target instead of forcing 1.0 for non-blend modes, which
    /// makes an opaque character material read as see-through (#427).
    ///
    /// `sro_sheen.wgsl` is the one deliberate exception: it consumes the
    /// sampled alpha as its sheen mask and sets `base_color.a = 1.0` itself, so
    /// it is asserted on that instead.
    #[test]
    fn character_shaders_handle_alpha_instead_of_leaking_it() {
        const RIM: &str = include_str!("../../../../assets/shaders/sro_rim.wgsl");
        const SHEEN: &str = include_str!("../../../../assets/shaders/sro_sheen.wgsl");

        assert!(
            RIM.contains("alpha_discard(pbr_input.material, pbr_input.material.base_color)"),
            "sro_rim.wgsl must run alpha_discard after pbr_input_from_standard_material"
        );
        assert!(
            RIM.contains("pbr_functions::{alpha_discard,"),
            "sro_rim.wgsl must import alpha_discard"
        );
        assert!(
            SHEEN.contains("pbr_input.material.base_color.a = 1.0"),
            "sro_sheen.wgsl consumes the alpha as its mask, so it must set it to 1.0 itself"
        );
    }
}
