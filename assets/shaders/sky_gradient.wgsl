// Gradient sky for the flat skybox cuboid (see `plugins/skybox.rs`): blends from a
// horizon (bottom) color to a zenith (top) color by the view direction's elevation,
// with the ramp compressed toward the horizon so it reads as a haze band rather than a
// linear sweep across the whole dome. All colors come from the JMXVENVI environment
// profiles, sampled by time of day. The lowest band (and everything below the horizon)
// blends into the scene's distance-fog color: terrain at the far plane is fully fogged,
// so the sky must meet it in exactly that color or the horizon shows a hard seam — the
// old StandardMaterial sky got this "for free" by being entirely beyond the fog far
// plane and thus rendering as pure fog color everywhere. Tonemapped in-shader so it
// matches the StandardMaterial-based look it replaces.
#import bevy_pbr::forward_io::VertexOutput
#import bevy_pbr::mesh_view_bindings::view
#import bevy_core_pipeline::tonemapping::tone_mapping

struct SkyGradient {
    top_color: vec4<f32>,
    bottom_color: vec4<f32>,
    fog_color: vec4<f32>,
    // x = star intensity (0..1, from the ENVI NightIntensity graph); yzw unused.
    params: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0)
var<uniform> gradient: SkyGradient;

fn hash31(p: vec3<f32>) -> f32 {
    var q = fract(p * 0.1031);
    q += dot(q, q.yzx + 33.33);
    return fract((q.x + q.y) * q.z);
}

// Procedural star field (our own — the game archives ship no star texture): the view
// direction is scaled into a 3D grid, a small fraction of cells hold one jittered star
// point, and brightness falls off with the fragment's distance to it. Purely
// direction-based, so the stars are fixed on the sky dome; the cloud layers draw on
// top and occlude them naturally.
fn stars(dir: vec3<f32>, intensity: f32) -> f32 {
    if (intensity <= 0.001 || dir.y <= 0.0) {
        return 0.0;
    }
    let d = dir * 220.0;
    let cell = floor(d);
    let h = hash31(cell);
    if (h < 0.94) {
        return 0.0;
    }
    let jitter = vec3(hash31(cell + 1.7), hash31(cell + 4.3), hash31(cell + 9.1)) - 0.5;
    let dist = length(fract(d) - 0.5 - jitter * 0.6);
    // Per-star brightness variation from the same hash that selected the cell.
    let brightness = (h - 0.94) / 0.06;
    // Fade out toward the horizon like real stars do behind haze.
    return brightness * smoothstep(0.12, 0.0, dist) * smoothstep(0.0, 0.15, dir.y) * intensity;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    // The cuboid is centered on the camera every frame, so the fragment's world position
    // relative to the camera is the view direction.
    let dir = normalize(in.world_position.xyz - view.world_position);
    let t = pow(clamp(dir.y, 0.0, 1.0), 0.45);
    var color = mix(gradient.bottom_color.rgb, gradient.top_color.rgb, t);
    let horizon = smoothstep(0.0, 0.08, dir.y);
    color = mix(gradient.fog_color.rgb, color, horizon);
    color += vec3(stars(dir, gradient.params.x));
    var out = vec4(color, 1.0);
#ifdef TONEMAP_IN_SHADER
    out = tone_mapping(out, view.color_grading);
#endif
    return out;
}
