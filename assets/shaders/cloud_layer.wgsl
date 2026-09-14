// Cloud layer disc (see `plugins/skybox.rs`): scrolling cloud texture on a flat disc
// floating above the camera, with a radial alpha fade so the layer dissolves into the
// sky instead of ending in an edge. The fade is computed per-fragment from the planar
// UVs rather than baked into vertex alpha: the rim quads span tens of thousands of
// units nearly edge-on, and perspective-correct vertex interpolation compresses a
// vertex-alpha fade into a few pixels at the far end — a visible line. The tint comes
// from the environment profile's diffuse color (white at noon, warm at dusk, near-black
// at night), since an unlit cloud would otherwise glow through the night.
#import bevy_pbr::forward_io::VertexOutput
#import bevy_pbr::mesh_view_bindings::{view, globals}
#import bevy_core_pipeline::tonemapping::tone_mapping

struct CloudSettings {
    tint: vec4<f32>,
    // UV/sec; the scroll offset is this times the global clock. Computed here
    // rather than advanced by a system so the material stays immutable — see
    // the Rust-side field doc.
    scroll_speed: vec2<f32>,
    // UV repeats across the disc's diameter; the UVs span [0, uv_tiles].
    uv_tiles: f32,
    // Fraction of the radius that stays fully opaque before the fade to the rim begins.
    inner: f32,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0)
var<uniform> settings: CloudSettings;
@group(#{MATERIAL_BIND_GROUP}) @binding(1)
var cloud_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2)
var cloud_sampler: sampler;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    // The un-scrolled UVs are planar over the disc, so they double as the radial
    // coordinate: 0 at the center, 1 at the rim.
    let radial = length(in.uv / settings.uv_tiles - vec2(0.5)) * 2.0;
    let fade = 1.0 - smoothstep(settings.inner, 1.0, radial);
    let scroll = settings.scroll_speed * globals.time;
    var color = textureSample(cloud_texture, cloud_sampler, in.uv + scroll) * settings.tint;
    color.a = color.a * fade;
#ifdef TONEMAP_IN_SHADER
    color = tone_mapping(color, view.color_grading);
#endif
    return color;
}
