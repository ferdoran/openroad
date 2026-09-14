use bevy::asset::Asset;
use bevy::math::Vec2;
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::reflect::TypePath;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

// Idea: the low graphics tier answers the same question as `water_hq_material.rs` — "make a
// flat plane read as moving water" — with the one effect that actually carries that reading
// (a scrolling surface) and none of the ones that cost per-pixel work.
//
// What it deliberately does NOT do, and why each matters on a fill-limited GPU:
//   * No `specular_transmission`. The HQ tier's transmission is what routes it into Bevy's
//     Transmissive phase, and that phase copies the whole main texture into
//     `view_transmission_texture` every frame that any transmissive mesh is visible — a
//     full-resolution copy paid once per frame regardless of how little water is on screen.
//   * No screen-space reflection raymarch. The HQ shader walks the depth prepass ~15 times
//     per water pixel; here there is no march at all, just the lit surface.
//   * No normal-map ripple octaves. Two extra texture samples plus the slope blend, gone.
//
// What remains is a `StandardMaterial` whose base-color UVs scroll with `globals.time`. The
// scroll is computed in the shader rather than by mutating a uniform per frame on purpose:
// mutating a material re-prepares its bind group every frame, which is the exact churn
// `docs/perf-future-levers.md` §2 measures on the effect materials. A plain clock is enough
// here because, unlike an effect node's `age`, water has no loop reset or pause to honour.
//
// Transparency is ordinary alpha blending (`AlphaMode::Blend` on the base material) instead
// of refraction, so the lakebed still shows through — it just does not bend.
pub type LowQualityWaterMaterial =
    ExtendedMaterial<bevy::prelude::StandardMaterial, WaterLowExtension>;

#[derive(Asset, AsBindGroup, TypePath, Debug, Clone)]
pub struct WaterLowExtension {
    #[uniform(100)]
    pub settings: WaterLowSettings,
}

/// Std140 layout; field order must match `WaterLowSettings` in `water.wgsl`.
#[derive(Clone, Copy, Debug, ShaderType)]
pub struct WaterLowSettings {
    /// UV/sec scroll of the base diffuse. Matches the HQ tier's primary octave
    /// (`WaterHqSettings::scroll_speed_a`) so switching tiers does not change how
    /// fast the water appears to move.
    pub scroll_speed: Vec2,
    pub _padding: Vec2,
}

impl Default for WaterLowSettings {
    fn default() -> Self {
        Self {
            scroll_speed: Vec2::new(0.008, 0.004),
            _padding: Vec2::ZERO,
        }
    }
}

impl MaterialExtension for WaterLowExtension {
    fn fragment_shader() -> ShaderRef {
        "shaders/water.wgsl".into()
    }

    // Note the contrast with the HQ tier, which must opt *out* of the prepass because its
    // fragment shader raymarches that very prepass. Nothing here reads it, so the default
    // (participate) stands and the surface gets ordinary depth handling.
}
