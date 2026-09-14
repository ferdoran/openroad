// Idea: this is a `StandardMaterial` extension (see `water_hq_material.rs`), not a
// standalone material, so Bevy's built-in screen-space specular transmission still renders
// whatever is really behind the water (refracted by `ior`/`thickness` on the base material)
// and normal PBR lighting/shadowing still applies unmodified. We only bolt on the parts that
// turn flat transmissive glass into *moving water*:
//   1. Scroll the base diffuse UVs over time (animated surface).
//   2. Sample a real (procedurally generated, not SRO game data — see
//      `tools/src/bin/gen_water_normal`) tangent-space ripple normal map at two independently
//      scrolling octaves, blend their slopes, and rebuild a world-space normal from that.
//      Because the water mesh is a flat, axis-aligned XZ plane, tangent-space (u, v,
//      out-of-plane) maps directly onto world (x, z, y) — no per-vertex tangent attribute or
//      full TBN matrix needed.
//   3. Fresnel-blend the final lit/refracted color toward a real screen-space reflection:
//      bevy 0.19's physically-based SSR raymarcher (`bevy_pbr::raymarch`, the same DDA marcher
//      the built-in `ScreenSpaceReflections` uses) walks the camera's depth prepass along the
//      reflected ray, and on a hit the reflected color is sampled from the view transmission
//      texture — the snapshot of the opaque scene (terrain, objects, sky) that transmissive
//      materials get for free. The built-in SSR component can't be used directly because it
//      only shades pixels present in the *deferred gbuffer* and runs before the transmissive
//      phase this water renders in; calling the raymarcher here sidesteps both while keeping
//      the see-through refraction. Rays that miss (offscreen geometry, grazing angles) fall
//      back to the flat sky tint. Requires `DepthPrepass` on the camera (see camera.rs);
//      without a depth prepass this compiles down to the sky-tint-only fallback. Msaa does
//      *not* need to be off: this material shader is compiled through the normal mesh
//      pipeline, not bevy's built-in SSR pipeline, so `USE_DEPTH_SAMPLERS` (which would force
//      a non-multisampled `textureSampleLevel` call) is never defined here — the raymarcher's
//      `textureLoad`-based fallback path handles a multisampled depth prepass texture fine by
//      reading MSAA sample 0.
// The low-graphics tier intentionally skips all of this (see `water.wgsl`).
#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
    mesh_view_bindings::{globals, view_transmission_texture, view_transmission_sampler},
}

#ifdef DEPTH_PREPASS
#import bevy_pbr::mesh_view_bindings::depth_prepass_texture
#import bevy_pbr::view_transformations::position_world_to_ndc
#import bevy_pbr::raymarch::{
    depth_ray_march_new_from_depth,
    depth_ray_march_from_cs,
    depth_ray_march_to_ws_dir,
    depth_ray_march_march,
}
#endif  // DEPTH_PREPASS

struct WaterHqSettings {
    sky_tint: vec4<f32>,
    scroll_speed_a: vec2<f32>,
    scroll_speed_b: vec2<f32>,
    ripple_tiling: f32,
    distortion_strength: f32,
    fresnel_power: f32,
    _padding: f32,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100)
var<uniform> water_settings: WaterHqSettings;
@group(#{MATERIAL_BIND_GROUP}) @binding(101)
var normal_map: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(102)
var normal_map_sampler: sampler;

#ifdef DEPTH_PREPASS
// March parameters mirror `ScreenSpaceReflections::default()` (10 linear + 5 bisection steps,
// secant refinement). The assumed surface thickness is in world units, so unlike bevy's
// meter-scale 0.25 default it's scaled up for this game's world (one terrain block = 320
// units); too thin and rays leak through terrain, too thick and reflections smear under
// ledges.
const SSR_LINEAR_STEPS: u32 = 10u;
const SSR_BISECTION_STEPS: u32 = 5u;
const SSR_DEPTH_THICKNESS: f32 = 4.0;

// Reflected scene color for the ray `P_world + t*R_world`, or `fallback` (the sky tint) if
// the ray leaves the screen or hits nothing. Hits sample the view transmission texture — the
// opaque-scene snapshot the transmissive phase already gets — so reflections and refraction
// read from the same, consistent image of the world.
fn reflect_scene(R_world: vec3<f32>, P_world: vec3<f32>, fallback: vec3<f32>) -> vec3<f32> {
    let depth_size = vec2<f32>(textureDimensions(depth_prepass_texture));

    var raymarch = depth_ray_march_new_from_depth(depth_size);
    depth_ray_march_from_cs(&raymarch, position_world_to_ndc(P_world));
    depth_ray_march_to_ws_dir(&raymarch, normalize(R_world));
    raymarch.linear_steps = SSR_LINEAR_STEPS;
    raymarch.bisection_steps = SSR_BISECTION_STEPS;
    raymarch.use_secant = true;
    raymarch.depth_thickness_linear_z = SSR_DEPTH_THICKNESS;
    raymarch.jitter = 1.0;
    raymarch.march_behind_surfaces = false;

    let result = depth_ray_march_march(&raymarch);
    if (result.hit) {
        return textureSampleLevel(view_transmission_texture, view_transmission_sampler, result.hit_uv, 0.0).rgb;
    }
    return fallback;
}
#endif  // DEPTH_PREPASS

@fragment
fn fragment(
    vertex_output: VertexOutput,
    @builtin(front_facing) is_front: bool,
) -> FragmentOutput {
    // WGSL forbids `var in = in;` (a body-level declaration can't shadow a parameter of the
    // same name — naga: "redefinition of `in`"), so the parameter gets a different name and
    // `in` is declared as the mutable copy, same as Bevy's own pbr.wgsl does it.
    var in = vertex_output;
    let t = globals.time;
    in.uv = in.uv + water_settings.scroll_speed_a * t;

    var pbr_input = pbr_input_from_standard_material(in, is_front);
    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);

    // Two octaves of the same tileable normal map, scrolling independently so their ripples
    // don't lock together into an obviously repeating pattern. Combine by summing the
    // tangent-space slopes (xy) and rebuilding the z from that, which is the standard way to
    // blend two normal-map samples (cheaper and closer to correct than blending the raw
    // vectors or the colors).
    let ripple_uv = in.uv * water_settings.ripple_tiling + water_settings.scroll_speed_b * t;
    let n1 = textureSample(normal_map, normal_map_sampler, in.uv).xyz * 2.0 - 1.0;
    let n2 = textureSample(normal_map, normal_map_sampler, ripple_uv).xyz * 2.0 - 1.0;
    let slope = (n1.xy + n2.xy) * water_settings.distortion_strength;
    let tangent_normal = normalize(vec3(slope, 1.0));

    // Flat, axis-aligned XZ water plane: tangent-space (u, v, out-of-plane) is world (x, z,
    // up), where "up" is the mesh's own (unperturbed) normal rather than a hardcoded +Y — that
    // keeps this correct even for the back face (`prepare_world_normal` flips world_normal to
    // (0,-1,0) there) instead of only working when viewed from above. Only the *lit* normal
    // (`N`) is perturbed; `world_normal` itself stays flat for shadow sampling, same as a real
    // normal map would leave it.
    let up = pbr_input.world_normal;
    pbr_input.N = normalize(up * tangent_normal.z + vec3(tangent_normal.x, 0.0, tangent_normal.y));

    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);

    // Fresnel: reflections dominate at grazing angles, refraction when looking straight down.
    let n_dot_v = max(dot(pbr_input.N, pbr_input.V), 0.0001);
    let fresnel = pow(1.0 - n_dot_v, water_settings.fresnel_power) * water_settings.sky_tint.a;
#ifdef DEPTH_PREPASS
    let reflect_dir = reflect(-pbr_input.V, pbr_input.N);
    let reflected = reflect_scene(reflect_dir, pbr_input.world_position.xyz, water_settings.sky_tint.rgb);
#else
    // No depth prepass on this camera — fall back to the flat sky tint.
    let reflected = water_settings.sky_tint.rgb;
#endif
    out.color = vec4(mix(out.color.rgb, reflected, fresnel), out.color.a);

    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
