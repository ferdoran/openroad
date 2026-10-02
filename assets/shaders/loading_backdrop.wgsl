// Blurred, darkened backdrop for the loading screens.
//
// Idea: the loading art is 1024x768 (4:3) and the window is not, so the art is
// drawn *contained* (whole painting, nothing cropped) and this shader fills
// what is left over — the same picture, blown up to cover the window, blurred
// out of focus and dimmed, so the painting ends in its own colours instead of
// in black bars. That is a deliberate deviation from the original (ADR-0009):
// the v1.188 client only ever ran 4:3 and has nothing to say about the leftover
// area. See `scenes::loading_screen::BACKDROP_BLUR` for the parameters.
//
// Method, and why it is not a plain wide gather: a disc of 24 taps over a
// radius of ~9% of the height is ~70 texels apart on a 1024x768 painting, and
// these paintings are full of hard silhouettes — a fixed tap pattern at that
// spacing ghosts them into a fan of copies. Two things fix that at 25 samples:
//   * the mip level whose texels are about the tap spacing does the averaging
//     between the taps, wherever the texture has one, and
//   * the tap spiral is rotated by a per-pixel hash, which turns the residual
//     structure into fine, static grain instead of coherent ghosts. This is
//     what makes the effect independent of the mip chain: 18 of the 40 vanilla
//     loading backgrounds are DXT5 with a single mip level (the .ddj loader
//     passes compressed payloads to the GPU as they are), so a mip-only blur
//     would look right on 22 of them and ghost on the other 18.
//
// The node this draws into is the 4:3 *cover* box of `fit_design_surfaces` and
// the art is 4:3 too, so UV space and screen space share one aspect ratio: a
// circular tap offset in UV stays circular on screen.

#import bevy_ui::ui_vertex_output::UiVertexOutput

// x = blur radius in UV (= fraction of the box height), y = linear brightness
@group(1) @binding(0) var<uniform> settings: vec4<f32>;
@group(1) @binding(1) var art: texture_2d<f32>;
@group(1) @binding(2) var art_sampler: sampler;

const TAPS: i32 = 24;
// golden angle: successive taps of the spiral land as far apart as possible
const GOLDEN_ANGLE: f32 = 2.3999632;

// Interleaved gradient noise (Jimenez, "Next Generation Post Processing"):
// one dot product and a fract, and its pattern is fine enough that the grain
// it leaves reads as film grain rather than as a texture of its own.
fn hash_angle(p: vec2<f32>) -> f32 {
    let magic = vec3(0.06711056, 0.00583715, 52.9829189);
    return fract(magic.z * fract(dot(p, magic.xy))) * 6.2831853;
}

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    let radius = settings.x;
    let brightness = settings.y;
    let dims = vec2<f32>(textureDimensions(art, 0));

    // the level whose texels are about half the blur radius, clamped to what
    // this texture actually has (see the header: 18 of 40 have only level 0)
    let radius_texels = max(radius * dims.y, 1.0);
    let levels = f32(textureNumLevels(art)) - 1.0;
    let lod = clamp(log2(radius_texels) - 1.0, 0.0, levels);

    let phase = hash_angle(in.position.xy);
    // address mode is Repeat for every .ddj (main.rs), so clamp here or the
    // taps near the border wrap the opposite edge of the painting into frame
    var acc = textureSampleLevel(art, art_sampler, clamp(in.uv, vec2(0.0), vec2(1.0)), lod).rgb;
    for (var i = 0; i < TAPS; i = i + 1) {
        let t = (f32(i) + 0.5) / f32(TAPS);
        // sqrt(t): equal-area steps, so the taps cover the disc evenly
        let r = radius * sqrt(t);
        let a = f32(i) * GOLDEN_ANGLE + phase;
        let uv = clamp(in.uv + vec2(cos(a), sin(a)) * r, vec2(0.0), vec2(1.0));
        acc += textureSampleLevel(art, art_sampler, uv, lod).rgb;
    }
    return vec4(acc / f32(TAPS + 1) * brightness, 1.0);
}
