// Idea: the low graphics tier's water — a `StandardMaterial` extension (see
// `water_material.rs`) whose only addition is a time-scrolled base-color UV. Everything else
// is stock PBR, which is the point: the HQ tier (`water_hq.wgsl`) buys refraction, ripple
// normals and screen-space reflections at a per-pixel cost this tier exists to avoid.
//
// The scroll reads `globals.time` rather than a CPU-updated uniform so the material stays
// immutable and its bind group is never re-prepared (see the note in `water_material.rs`).
#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    mesh_view_bindings::globals,
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
}

struct WaterLowSettings {
    scroll_speed: vec2<f32>,
    _padding: vec2<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100)
var<uniform> water_settings: WaterLowSettings;

@fragment
fn fragment(
    in: VertexOutput,
    @builtin(front_facing) is_front: bool,
) -> FragmentOutput {
    // Scroll before the standard material reads the UVs, so the base color texture (and any
    // other UV-driven channel the material carries) animates as one surface.
    var scrolled = in;
    scrolled.uv = in.uv + water_settings.scroll_speed * globals.time;

    var pbr_input = pbr_input_from_standard_material(scrolled, is_front);
    // Honours the base material's AlphaMode; the water is authored as Blend, so this keeps
    // the lakebed visible through the surface.
    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);

    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    // Fog, tonemapping and the rest of the post-lighting chain, so this tier still sits in
    // the same atmosphere as the terrain around it.
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
