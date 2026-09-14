use bevy::asset::Asset;
use bevy::color::Srgba;
use bevy::math::{Vec2, Vec4};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::{Handle, Image};
use bevy::reflect::TypePath;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

use crate::plugins::skybox::SKY_COLOR_HEX;

// Idea: high-quality (default/high graphics) water is a `StandardMaterial` extended with a
// custom fragment shader (`water_hq.wgsl`), so Bevy's built-in screen-space specular
// transmission keeps doing the heavy lifting of actually rendering what's behind the water,
// refracted by `ior`/`thickness` on `base`. The extension only bolts on the parts that make
// flat transmissive glass read as *moving water*: a scrolling diffuse, a real (procedurally
// generated, see `tools/src/bin/gen_water_normal`) tangent-space ripple normal map sampled at
// two independently scrolling octaves, and a Fresnel blend toward a sky-colored tint at
// grazing angles to fake a reflection. See the shader file itself for the per-step breakdown.
// This mirrors Bevy's own `examples/3d/ssr.rs` water demo (`ExtendedMaterial<StandardMaterial,
// Water>` + a scrolling normal map), minus that example's `ScreenSpaceReflections` + deferred
// rendering — deferred is a renderer-wide switch that would also drop `specular_transmission`
// support, so real reflections are a separate, deliberate follow-up rather than bundled here.
//
// The low-graphics tier (`graphics.water.quality: low`) deliberately does not use this: it
// keeps the flat, non-transmissive `LowQualityWaterMaterial` in `water_material.rs`, which
// has neither the transmissive pass's per-frame main-texture copy nor the SSR march, and
// which participates in the depth prepass so it gets early-Z.
pub type HighQualityWaterMaterial =
    ExtendedMaterial<bevy::prelude::StandardMaterial, WaterExtension>;

#[derive(Asset, AsBindGroup, TypePath, Debug, Clone)]
pub struct WaterExtension {
    #[uniform(100)]
    pub settings: WaterHqSettings,
    /// Procedurally generated tileable ripple normal map (`assets/textures/water_normal.png`,
    /// see `tools/src/bin/gen_water_normal`) — not an SRO game asset, since the original
    /// client's water textures don't include a real normal map to reuse. Must be loaded with
    /// `is_srgb = false` like any normal map.
    #[texture(101)]
    #[sampler(102)]
    pub normal_map: Handle<Image>,
}

/// Std140 layout, field order must match the `WaterHqSettings` struct in `water_hq.wgsl`.
#[derive(Clone, Copy, Debug, ShaderType)]
pub struct WaterHqSettings {
    /// Reflection tint blended in at grazing angles; alpha is the max blend strength.
    pub sky_tint: Vec4,
    /// UV/sec scroll of the base diffuse texture (the visible water surface).
    pub scroll_speed_a: Vec2,
    /// UV/sec scroll of the second, ripple-only sample.
    pub scroll_speed_b: Vec2,
    /// UV scale applied before `scroll_speed_b`, kept non-integer to avoid the second normal
    /// map octave lining up with the first and canceling itself out.
    pub ripple_tiling: f32,
    /// Multiplier on the combined tangent-space slope from both normal map octaves before
    /// it's rebuilt into a world-space normal — turn down for calmer water.
    pub distortion_strength: f32,
    /// Fresnel exponent; higher values narrow the reflection tint to shallower angles.
    pub fresnel_power: f32,
    _padding: f32,
}

impl Default for WaterHqSettings {
    fn default() -> Self {
        let sky = Srgba::hex(SKY_COLOR_HEX).unwrap();
        Self {
            sky_tint: Vec4::new(sky.red, sky.green, sky.blue, 0.55),
            scroll_speed_a: Vec2::new(0.008, 0.004),
            scroll_speed_b: Vec2::new(-0.013, 0.02),
            ripple_tiling: 2.3,
            distortion_strength: 1.2,
            fresnel_power: 4.0,
            _padding: 0.0,
        }
    }
}

impl MaterialExtension for WaterExtension {
    fn fragment_shader() -> ShaderRef {
        "shaders/water_hq.wgsl".into()
    }

    /// The water must stay out of the depth prepass: its fragment shader raymarches that very
    /// prepass for screen-space reflections, and if the water wrote its own depth there, every
    /// reflection ray would immediately hit the water surface it started from (and the lakebed
    /// would vanish from the refraction, since transmission also depends on the water not
    /// occluding what's behind it).
    fn enable_prepass() -> bool {
        false
    }
}
