// Per-texel metallic sheen for SRO EnvMap resources (weapons, metal armor).
//
// These resources' diffuse alpha channel is the original engine's
// environment-map mask, not transparency (see `SroResource::alpha_is_sheen`):
// alpha 1.0 = polished metal, alpha 0.0 = matte leather/cloth. The base
// `StandardMaterial` is opaque — except for cutout resources, whose base is
// Mask(1/255) so the stock depth-prepass/shadow shaders discard the cutout
// texels too (an opaque base would write prepass depth there, blocking
// everything behind the discarded fragments into black holes). Either way
// the sampled alpha reaches this fragment untouched (no alpha_discard runs
// in `pbr_input_from_standard_material`); everything else (skinning,
// shadows, prepass, light probes) is the stock PBR pipeline.
//
// Both sheen passes reproduce the original client's exact mechanism,
// recovered from sro_client.exe (CRTModEnvMap::Apply) and Data.pk2
// shader/vertexshader{0,2}spec.c: a grayscale sphere map is sampled by a
// SPHERE MAP of the vertex normal (`oT0 = (normal x envMatrix).xy + 0.5`)
// and MODULATED by a tint color (the engine's TFACTOR). Because the
// coordinate is the view-space normal, the highlight sweeps across the
// surface as the weapon/camera turns, just like the original. The
// always-on base pass uses the chrome probe
// (prim/mtrl/etc/spheremap_gray.ddj) added into the ALBEDO before the
// lighting multiply (the exe's stage order — see below), so it reads as
// lit metal, not a glow; the optional alchemy-enhancement shine
// (`shine_color.a > 0`) uses the per-tier highlight/streak texture with
// the pulsing two-color TFACTOR, added post-lighting in scene-linear HDR
// so it punches through tonemapping like an emissive.
#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing},
    mesh_view_bindings::{globals, view},
}

struct SheenSettings {
    strength: f32,
    shiny_roughness: f32,
    reflectance: f32,
    // discard texels whose alpha (the sheen mask) is below this; the
    // original's EnvMap alpha-test flag (GREATEREQUAL ref 1, so cutouts
    // are alpha == 0 exactly); 0 disables
    alpha_cutout: f32,
    // rgb = TFACTOR tint (scene-linear), a = intensity; a == 0 disables it
    shine_color: vec4<f32>,
    // second color the glow pulses toward (the original's two-color glow)
    shine_color2: vec4<f32>,
    // extra scroll (uv/sec) added to the sphere coordinate; 0 = view-driven
    shine_scroll: vec2<f32>,
    // color1<->color2 pulses per second; 0 = steady color1
    shine_pulse: f32,
    // intensity of the base EnvMap chrome-probe reflection; 0 disables
    env_strength: f32,
    // always-on rim light (rgb = color, a = strength; 0 disables) with
    // its fresnel exponent — the mobile port applies this to every
    // character/equipment material (see sro_rim.wgsl for the original
    // selection-only variant this mirrors)
    rim_color: vec4<f32>,
    rim_power: f32,
    // sharpening exponent on the enhancement-streak highlight sample
    // (mobile "streamerPOW"); 1 = faithful soft streak
    shine_pow: f32,
    // rim application: 0 = absolute add, 1 = relative (lit x (1 + rim)),
    // which survives HDR exposure and the tonemap shoulder
    rim_mode: f32,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100)
var<uniform> sheen_settings: SheenSettings;
@group(#{MATERIAL_BIND_GROUP}) @binding(101)
var shine_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(102)
var shine_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(103)
var env_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(104)
var env_sampler: sampler;

@fragment
fn fragment(vertex_output: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var in = vertex_output;
    var pbr_input = pbr_input_from_standard_material(in, is_front);

    // the sampled diffuse alpha is the sheen mask
    let mask = saturate(pbr_input.material.base_color.a);
    // per-resource cutout: some EnvMap items punch shapes out of a shared
    // atlas with exact-zero alpha (e.g. glaive blades)
    if sheen_settings.alpha_cutout > 0.0 && mask < sheen_settings.alpha_cutout {
        discard;
    }
    pbr_input.material.metallic = mask * sheen_settings.strength;
    pbr_input.material.perceptual_roughness =
        mix(pbr_input.material.perceptual_roughness, sheen_settings.shiny_roughness, mask);
    pbr_input.material.reflectance =
        mix(pbr_input.material.reflectance, vec3<f32>(sheen_settings.reflectance), mask);
    // the alpha carried its mask duty; render fully opaque
    pbr_input.material.base_color.a = 1.0;

    // the original's env-map coordinate: the surface normal in view
    // space as a sphere map (normal.xy * 0.5 + 0.5), so the reflection
    // clings to curvature and shifts with the camera like real gloss
    let n_view = normalize((view.view_from_world * vec4<f32>(pbr_input.N, 0.0)).xyz);
    let sphere = n_view.xy * 0.5 + 0.5;

    // base EnvMap pass, exactly the exe's stage order (CRTModEnvMap::Apply
    // @0xc83690): stage0 MODULATE(spheremap, TFACTOR = 0.5 gray from the
    // ModData floats), stage1 MODULATEALPHA_ADDCOLOR = albedo + alpha x
    // stage0, stage2 MODULATE2X with the vertex lighting. The chrome term
    // joins the ALBEDO before the lighting multiply — it brightens under
    // sun and vanishes in shadow instead of glowing like an emissive. The
    // min() reproduces the fixed-function per-stage saturate: the summed
    // albedo never exceeds 1, so bright texels can't over-glow. Sampled
    // .rgb (the shipped probe is grayscale, so this equals the old .r
    // read; a colored probe would carry through). A mobile-port "metal
    // boost" (albedo tint, extra gain, replace mode) was tried 2026-08-09/10
    // and reverted by decision: with no metallicity channel in 1.188 data
    // it bled onto every EnvMap resource — most visibly the garment paper
    // talismans (see rendering-mobile-shader-comparison.md gap #3).
    let env = textureSample(env_texture, env_sampler, sphere).rgb;
    pbr_input.material.base_color = vec4<f32>(
        min(pbr_input.material.base_color.rgb + env * mask * sheen_settings.env_strength, vec3<f32>(1.0)),
        pbr_input.material.base_color.a,
    );

    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);

    if sheen_settings.shine_color.a > 0.0 {
        let scrolled = sphere + globals.time * sheen_settings.shine_scroll;
        // the original glow ping-pong-interpolates between two TFACTOR
        // colors (CRTModProgEquipPow); triangle wave 0..1..0
        let t = abs(fract(globals.time * sheen_settings.shine_pulse * 0.5) * 2.0 - 1.0);
        let tint = mix(sheen_settings.shine_color, sheen_settings.shine_color2, t);
        // grayscale radial highlight x tint (the exe's MODULATE); metal
        // texels (high mask) flash a bit brighter than matte ones. The
        // pow() is the mobile port's streamerPOW sharpening: it narrows
        // the soft gradient into a crisp travelling band (shine_pow 1 =
        // the faithful unsharpened streak)
        let highlight = pow(textureSample(shine_texture, shine_sampler, scrolled).r, sheen_settings.shine_pow);
        let shine = tint.rgb * tint.a * highlight * (0.6 + 0.4 * mask);
        out.color = vec4<f32>(out.color.rgb + shine, out.color.a);
    }

    // always-on rim (mobile-port parity): the identical fresnel term as
    // sro_rim.wgsl, so weapons/armor match the character body's rim; the
    // selection highlight overrides these fields with its stronger color.
    // rim_mode 1 = relative (lit x (1 + rim)) — the mobile 0.353 is a
    // fraction of their flat LDR lighting, so an absolute add of it into
    // our scene-linear HDR (~1.3-3.5 in daylight) vanishes in the tonemap
    // shoulder; the relative form keeps the ratio at any exposure.
    if sheen_settings.rim_color.a > 0.0 {
        let facing = saturate(dot(normalize(pbr_input.N), normalize(pbr_input.V)));
        let fresnel = pow(1.0 - facing, sheen_settings.rim_power);
        let rim = sheen_settings.rim_color.rgb * sheen_settings.rim_color.a * fresnel;
        let rim_absolute = out.color.rgb + rim;
        let rim_relative = out.color.rgb * (vec3<f32>(1.0) + rim);
        out.color = vec4<f32>(
            mix(rim_absolute, rim_relative, sheen_settings.rim_mode),
            out.color.a,
        );
    }

    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
