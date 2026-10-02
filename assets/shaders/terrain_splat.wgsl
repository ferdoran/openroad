#import bevy_pbr::{
    pbr_types::{
        STANDARD_MATERIAL_FLAGS_DOUBLE_SIDED_BIT,
        STANDARD_MATERIAL_FLAGS_FOG_ENABLED_BIT,
        pbr_input_new,
        PbrInput
    },
    pbr_bindings,
    prepass_utils,
    mesh_functions::{get_world_from_local, mesh_normal_local_to_world, mesh_position_local_to_world, mesh_tangent_local_to_world},
    skinning::{skin_model, skin_normals},
    morph::morph,
    view_transformations::position_world_to_clip,
    forward_io::{Vertex, VertexOutput, FragmentOutput},
    pbr_functions::{
        apply_normal_mapping,
        apply_pbr_lighting,
        alpha_discard,
        calculate_view,
        main_pass_post_lighting_processing,
        prepare_world_normal,
        calculate_tbn_mikktspace,
        sample_texture
    },
    mesh_view_bindings::{view, lights},
    mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT,
    shadows::fetch_directional_shadow,
    mesh_bindings::mesh,
}
#import bevy_core_pipeline::tonemapping::tone_mapping

@vertex
fn vertex(
    @builtin(vertex_index) index: u32,
    vertex_no_morph: Vertex
) -> VertexOutput {
    var out: VertexOutput;
#ifdef MORPH_TARGETS
    var vertex = morph_vertex(vertex_no_morph);
#else
    var vertex = vertex_no_morph;
#endif

#ifdef SKINNED
    var model = skin_model(vertex.joint_indices, vertex.joint_weights);
#else
    // Use vertex_no_morph.instance_index instead of vertex.instance_index to work around a wgpu dx12 bug.
    // See https://github.com/gfx-rs/naga/issues/2416 .
    var model = get_world_from_local(vertex_no_morph.instance_index);
#endif

#ifdef VERTEX_NORMALS
#ifdef SKINNED
    out.world_normal = skin_normals(model, vertex.normal);
#else
    out.world_normal = mesh_normal_local_to_world(
        vertex.normal,
        // Use vertex_no_morph.instance_index instead of vertex.instance_index to work around a wgpu dx12 bug.
        // See https://github.com/gfx-rs/naga/issues/2416
        vertex_no_morph.instance_index
    );
#endif
#endif

#ifdef VERTEX_POSITIONS
    out.world_position = mesh_position_local_to_world(model, vec4<f32>(vertex.position, 1.0));
    out.position = position_world_to_clip(out.world_position.xyz);
#endif

#ifdef VERTEX_UVS
    out.uv = vertex.uv;
#endif

#ifdef VERTEX_TANGENTS
    out.world_tangent = mesh_tangent_local_to_world(
        model,
        vertex.tangent,
        // Use vertex_no_morph.instance_index instead of vertex.instance_index to work around a wgpu dx12 bug.
        // See https://github.com/gfx-rs/naga/issues/2416
        vertex_no_morph.instance_index
    );
#endif

#ifdef VERTEX_COLORS
    out.color = vertex.color;
#endif

#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    // Use vertex_no_morph.instance_index instead of vertex.instance_index to work around a wgpu dx12 bug.
    // See https://github.com/gfx-rs/naga/issues/2416
    out.instance_index = vertex_no_morph.instance_index;
#endif
    return out;
}

// Ground splat rendering merges a region's whole 6x6 grid of 320-unit terrain blocks into one
// mesh + one material draw call (see `load_terrain_system` / `block_mesh::merge_block_meshes`).
//
// Each map vertex names exactly ONE ground tile (`JMXVMAPM`'s 10-bit texture_id) plus a tiling
// scale; the blend between neighbouring tiles is the bilinear interpolation of those per-vertex
// choices across the quad. This shader evaluates that directly: it reads the tile id at the 4
// vertices surrounding the fragment and weights each by its bilinear factor.
//
// The previous one-hot-weight-texture design (see git history) cost one weight sample per tile
// per fragment and capped the group at however many one-hot channels the material could bind.
// The 4-corner gather has neither property: 4 texel reads regardless of tile count, one
// `tile_map` layer per region.
//
// `tile_map` packs the region's 6x6 blocks of 17x17 vertices into a single 102x102 texture:
// texel (block_col*17 + i, block_row*17 + j) is that block's vertex (i, j). Blocks keep their
// own duplicated edge vertices, so a fragment never gathers across a block boundary and the
// per-block behaviour of the old layout is reproduced exactly. `.g` = splat scale code always.
// `.r` differs by draw path (see the two branches below): the `Material`-based path (default)
// stores this region's *local* tile index (its position in
// `TerrainBlockSplatMaterial::used_tiles`, capped at `REGION_TILE_SLOT_COUNT` because that path
// binds one atlas copy PER region); the hand-rolled-pipeline path
// (`client/src/plugins/map/terrain/render/`, feature `terrain_hand_rolled_pipeline`) binds the
// atlas exactly ONCE, globally, so it has no per-region size pressure and `.r` is simply the
// map's raw 10-bit tile id, indexing `tile_atlas` directly.
const REGION_SIZE: f32 = 1920.0;      // 6 blocks * 320 units
const BLOCK_SIZE: f32 = 320.0;
const VERTEX_SPACING: f32 = 20.0;     // 17 vertices per block edge => 16 intervals of 20
const BLOCK_VERTS: u32 = 17u;

// Two bind-group layouts for the same bindings, chosen by which draw path is active
// (`TerrainPipeline::specialize` pushes the `TERRAIN_HAND_ROLLED_PIPELINE` def; the stock
// `Material`/`MaterialPlugin` path never does). Only the *shape* differs — groups vs. one group,
// and the atlas size — every `var` name and all the fragment logic below is identical either way.
#ifdef TERRAIN_HAND_ROLLED_PIPELINE
// Per-region group: the two textures that are genuinely unique per region. Everything else this
// shader reads is identical for every terrain draw and lives in the global group below instead —
// see client/src/assets/m/block_splat_material.rs's REGION_TILE_SLOT_COUNT doc comment for why
// that split needed a hand-rolled pipeline (Bevy's `Material` trait has no supported way to bind
// an extra, globally-shared group).
@group(3) @binding(0) var tile_map: texture_2d<u32>;
@group(3) @binding(1) var lightmap_tex: texture_2d<f32>;

// Global group: bound exactly once, shared by every terrain region's draw. `tile_atlas` covers
// the map format's entire 10-bit id space (see `TILE_SLOT_COUNT` in block_splat_material.rs) —
// no per-region cap needed since there is only ever one copy of this array, not one per region.
@group(4) @binding(0) var clamp_sampler: sampler;
@group(4) @binding(1) var tile_atlas: binding_array<texture_2d<f32>, 1024>;
@group(4) @binding(2) var tile_sampler: sampler;
@group(4) @binding(3) var<storage> ambient_ratio: vec4<f32>;
@group(4) @binding(4) var<storage> terrain_params: array<vec4<f32>, 3>;
#else
@group(#{MATERIAL_BIND_GROUP}) @binding(0)
var tile_map: texture_2d<u32>;
// Clamp+filtering sampler, used for the lightmap (`tile_map` is only ever `textureLoad`ed).
@group(#{MATERIAL_BIND_GROUP}) @binding(1)
var clamp_sampler: sampler;
// This region's own ground tiles only — up to REGION_TILE_SLOT_COUNT (block_splat_material.rs)
// distinct textures, indexed by the *local* index `tile_map` carries (see the header comment
// above), not the map's raw 10-bit tile id. Each region's bind group fills only as many of these
// slots as it actually uses (`TerrainBlockSplatMaterial::used_tiles`); the rest are the shared
// fallback texture.
@group(#{MATERIAL_BIND_GROUP}) @binding(2)
var tile_atlas: binding_array<texture_2d<f32>, 64>;
@group(#{MATERIAL_BIND_GROUP}) @binding(3)
var tile_sampler: sampler;
// Per-channel ratio of SRO's terrain ambient to the global (object) ambient — one GPU
// buffer shared by ALL splat materials, updated in place by `TerrainAmbientRatioPlugin`
// (see `TerrainAmbientRatio` in block_splat_material.rs). w unused. Storage, not uniform:
// wgpu forbids mixing binding arrays (`tile_atlas` above) with uniform buffers in one group.
@group(#{MATERIAL_BIND_GROUP}) @binding(4)
var<storage> ambient_ratio: vec4<f32>;
// Baked per-region terrain lightmap (JMXVMAPT, see assets/t.rs): SRO's pre-baked static sun and
// cast shadows on the ground. Sampled at region-local UV with `clamp_sampler` (binding 1).
// A white 1x1 texture stands in for regions with no `.t`, making the multiply a no-op.
@group(#{MATERIAL_BIND_GROUP}) @binding(5)
var lightmap_tex: texture_2d<f32>;
// Global terrain render params, one shared buffer like `ambient_ratio` (see
// `TerrainRenderParams` in block_splat_material.rs):
//   [0].xy = lightmap UV scale, [0].zw = lightmap UV offset — identity (1,1,0,0) or the
//            V-flip calibration (1,-1,0,1), a data change instead of a shader edit;
//   [1].x  = lighting mode: 0 = dynamic PBR (current), 2 = fully baked albedo × lightmap
//            (mobile port / original). 1 used to be a third "flat baked" mode and is gone
//            (see TerrainLightingMode); any non-zero value reads as fully baked;
//   [1].yzw, [2].xy = ground-tile repeat factors for splat-scale codes 0/8/16/24/32
//            (live calibration sliders in the render-debug panel);
//   [2].z  = lightmap strength: 1 = vanilla baked shadows, 0 = PBR mode (the
//            environment plugin's hotkey-N switch swaps them for cascaded shadow
//            maps). Note: only the dynamic lighting mode substitutes real shadows —
//            baked bypasses apply_pbr_lighting, so strength 0 there just
//            leaves the ground unshaded;
//   [2].w  = extra sun-shadow darkening (vanilla player-shadow boost, 0 = off/PBR):
//            multiplies the lit ground by 1-[2].w where the shadow map is fully
//            occluded, sampled manually below so it works in every lighting mode.
@group(#{MATERIAL_BIND_GROUP}) @binding(6)
var<storage> terrain_params: array<vec4<f32>, 3>;
#endif

@fragment
fn fragment(
    @builtin(front_facing) is_front: bool,
    in: VertexOutput,
) -> FragmentOutput {
    // Recover the fragment's position local to the region purely via modulo, the same trick the
    // original single-block shader used with `% 320.0` — it works regardless of *any* ancestor
    // translation because the mesh spans exactly one region. An earlier version tried to recover
    // this by subtracting the group's own Transform.translation, which forgot about the *region*
    // entity's own world-space translation stacked on top — that made the block lookup come out
    // constant almost everywhere (every block showing the same tile data). Modulo sidesteps
    // needing to know either offset at all. World X is negated because region entities are
    // X-mirrored.
    let region_x = (((-in.world_position.x) % REGION_SIZE) + REGION_SIZE) % REGION_SIZE;
    let region_z = ((in.world_position.z % REGION_SIZE) + REGION_SIZE) % REGION_SIZE;

    // Screen-space derivatives of the ground-plane coordinate, taken from the CONTINUOUS world
    // position instead of the wrapped region-local coordinate: the two differ only by a
    // per-region-constant offset, so in the interior the derivatives are identical — but at
    // region borders the modulo wrap makes dpdx jump by ±REGION_SIZE within a straddling 2x2
    // pixel quad, driving every derivative-based sample to the lowest mip and drawing a thin
    // dashed line along the border. Computed here, in uniform control flow, because dpdx/dpdy
    // are derivative ops.
    let ground = vec2(-in.world_position.x, in.world_position.z);
    let dground_dx = dpdx(ground);
    let dground_dy = dpdy(ground);

    var color = sample_splat(region_x, region_z, dground_dx, dground_dy);
    // Modulate the ground albedo by the baked lightmap before lighting: shadowed ground stays dark
    // under any dynamic sun/ambient, adding SRO's static baked shadows as an albedo/occlusion layer.
    // [2].z is lightmap_enabled as a hard 0.0/1.0 (never fractional — see
    // `TerrainRenderParams::to_render_params`), off whenever `render_mode: pbr` is active. A real
    // branch is safe here despite `sample_lightmap` calling `textureSampleGrad`: unlike an
    // implicit-derivative `textureSample`, Grad takes its derivatives as explicit arguments
    // (computed once above, in uniform control flow), so it carries none of WGSL's
    // non-uniform-control-flow restriction — the same property `sample_splat`'s own fast path
    // already relies on (see its comment). Skipping the call when disabled avoids a lightmap
    // texture fetch on every terrain fragment whose result would otherwise just be discarded.
    var lightmap = vec3(1.0);
    if terrain_params[2].z > 0.0 {
        lightmap = sample_lightmap(region_x, region_z, dground_dx, dground_dy);
    }
    color = vec4(color.rgb * lightmap, color.a);

    // prepare pbr input
    var pbr_input = prepare_pbr(in, is_front, color);
    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);
    // Lighting model A/B (`terrain_params[1].x`, uniform across the draw). Both the mobile
    // port and (probably) the original render ground as albedo × baked lightmap with no
    // dynamic response at all, so both variants stay selectable for the playtest
    // (docs/rendering-mobile-shader-comparison.md, gap #6). A third one existed here
    // ("flat baked": base_color × ambient tint × exposure, no N·L) and was removed — it
    // scaled the whole ground by the ambient term alone, which in `render_mode: pbr` is
    // 30x smaller than in vanilla (`pbr_ambient_brightness` 100 vs `ambient_brightness`
    // 3000) and additionally arrives without the lightmap, so the ground went black
    // (ferdoran/openroad#5).
    // Fog is unaffected: it lives in main_pass_post_lighting_processing below.
    let lighting_mode = terrain_params[1].x;
    var out: FragmentOutput;
    if lighting_mode < 0.5 {
        // dynamic (current): full PBR sun + ambient over the baked lightmap
        out.color = apply_pbr_lighting(pbr_input);
        // SRO gives terrain its own ambient color, separate from the object ambient Bevy just
        // applied as `lights.ambient_color * diffuse`. Re-weight that term by the ratio of the
        // two; a ratio of 1 (the default, and whenever the environment system is off) is a
        // no-op. `view.exposure` is required: apply_pbr_lighting multiplies all lighting by it
        // at the end, and this correction is added after — without it the term is orders of
        // magnitude too bright.
        let ambient_correction = (ambient_ratio.rgb - vec3(1.0)) * lights.ambient_color.rgb * pbr_input.material.base_color.rgb * view.exposure;
        out.color = vec4(out.color.rgb + ambient_correction, out.color.a);
    } else {
        // fully baked: the lightmap (already multiplied into base_color above) is the
        // entire lighting, like the original fixed-function ground
        out.color = pbr_input.material.base_color;
    }
    // Vanilla player-shadow boost ([2].w): inside a cascaded shadow, apply_pbr_lighting
    // only removes the sun's *direct* term, and with SRO's ambient magnitudes (comparable
    // to the sun, see EnvironmentSettings::ambient_brightness) that reads far too faint.
    // Darken the lit result by the sun's shadow factor directly — a stand-in for the
    // original client's dark projected player shadow (in vanilla mode only the player
    // casts). Sampled manually instead of relying on apply_pbr_lighting so the
    // baked mode gets the player shadow too. terrain_params is a
    // storage buffer (formally non-uniform to WGSL), but the block stays legal:
    // fetch_directional_shadow's hardware path uses textureSampleCompareLevel,
    // which needs no derivatives and is allowed in non-uniform control flow.
    let shadow_strength = terrain_params[2].w;
    if shadow_strength > 0.0 {
        let view_z = dot(vec4(
            view.view_from_world[0].z,
            view.view_from_world[1].z,
            view.view_from_world[2].z,
            view.view_from_world[3].z,
        ), in.world_position);
        var shadow = 1.0;
        for (var i = 0u; i < lights.n_directional_lights; i = i + 1u) {
            if (lights.directional_lights[i].flags & DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) == 0u {
                continue;
            }
            shadow = min(
                shadow,
                fetch_directional_shadow(i, in.world_position, in.world_normal, view_z, in.position.xy),
            );
        }
        out.color = vec4(out.color.rgb * mix(1.0 - shadow_strength, 1.0, shadow), out.color.a);
    }
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}

fn prepare_pbr(in: VertexOutput, is_front: bool, color: vec4<f32>) -> PbrInput {
    // prepare pbr input
    var pbr_input = pbr_input_new();
    // pbr_input_new() zeroes the MESH flags; copy the instance flags like
    // bevy's pbr_input_from_vertex_output does, or apply_pbr_lighting never
    // sees MESH_FLAGS_SHADOW_RECEIVER_BIT and terrain silently ignores every
    // shadow map (characters cast onto objects but not the ground). Only the
    // dynamic lighting mode consumes this; baked bypasses PBR lighting.
    pbr_input.flags = mesh[in.instance_index].flags;
    pbr_input.material.base_color = color;
//
//    // alpha discard
    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);
//
    let double_sided = (pbr_input.material.flags & STANDARD_MATERIAL_FLAGS_DOUBLE_SIDED_BIT) != 0u;
    pbr_input.frag_coord = in.position;
    pbr_input.world_position = in.world_position;
    pbr_input.world_normal = prepare_world_normal(
        in.world_normal,
        double_sided,
        is_front,
    );
    pbr_input.material.perceptual_roughness = 0.9;
    pbr_input.material.reflectance = vec3(0.1);
    pbr_input.material.metallic = 0.0;
    // pbr_input_new() leaves fog off by default; main_pass_post_lighting_processing()
    // only calls apply_fog() when this bit is set.
    pbr_input.material.flags |= STANDARD_MATERIAL_FLAGS_FOG_ENABLED_BIT;
//    pbr_input.material.ior = 1.5;
//    pbr_input.material.thickness = 1.0;
//    pbr_input.material.diffuse_transmission = 0.0;
//    pbr_input.material.specular_transmission = 0.0;
//
    pbr_input.is_orthographic = view.clip_from_view[3].w == 1.0;
#ifdef LOAD_PREPASS_NORMALS
    pbr_input.N = prepass_utils::prepass_normal(in.position, 0u);
#else
    pbr_input.N = normalize(pbr_input.world_normal);
#endif

    pbr_input.V = calculate_view(in.world_position, pbr_input.is_orthographic);
    return pbr_input;
}

// Each material's lightmap covers exactly one region, so the region-local coordinates the
// fragment already derived map straight to UV. NOTE: V (world_z) orientation vs. the DDS row
// order is calibration-sensitive; flip `v` here if the baked shadows come out mirrored along Z.
fn sample_lightmap(region_x: f32, region_z: f32, duv_dx: vec2<f32>, duv_dy: vec2<f32>) -> vec3<f32> {
    let region = REGION_SIZE;
    // scale/offset from the shared params buffer, so the pending V-flip
    // calibration is a data change (see the binding-6 comment above)
    let uv = vec2(region_x / region, region_z / region) * terrain_params[0].xy + terrain_params[0].zw;
    // textureSampleGrad with the continuous ground-plane derivatives, NOT textureSample: the
    // region-local uv wraps 1→0 exactly at region borders, where implicit derivatives explode and
    // pick the lowest mip (the DDS decodes with a mip chain) — that was the dashed line along every
    // region border. Not textureSampleLevel(0) either: the low-res lightmap stretched over 1920
    // units minifies heavily at distance and would alias without mips. The 1x1 white fallback for
    // regions without a `.t` is unaffected.
    return textureSampleGrad(
        lightmap_tex,
        clamp_sampler,
        uv,
        duv_dx / region * terrain_params[0].xy,
        duv_dy / region * terrain_params[0].xy,
    ).rgb;
}

// Bilinear blend of the ground tiles named by the 4 map vertices surrounding this fragment.
//
// `duv_dx`/`duv_dy` are the ground-plane derivatives precomputed in `fragment()` — from the
// CONTINUOUS world position, not the wrapped region-local coordinate, so they don't blow up
// where the modulo wraps at region borders (which forced the lowest mip and drew dashed border
// lines). Passing them in also keeps every tile fetch a `textureSampleGrad`, so the uniform-tile
// fast path below is legal: branching around an *implicit*-derivative `textureSample` would be
// undefined per WGSL's uniformity rules, since derivatives need every fragment in a 2x2 quad to
// run the same fetch.
fn sample_splat(region_x: f32, region_z: f32, duv_dx: vec2<f32>, duv_dy: vec2<f32>) -> vec4<f32> {
    // Which block, and where inside it. Vertices sit every 20 units, 17 per block edge, and the
    // fragment is strictly inside the block (region_x < REGION_SIZE => local < BLOCK_SIZE), so
    // `i0` lands in 0..15 and `i0 + 1` never leaves the block's own 17 vertices. That is what
    // makes the packed 102x102 `tile_map` reproduce the old per-block layers exactly.
    let block_col = min(u32(floor(region_x / BLOCK_SIZE)), 5u);
    let block_row = min(u32(floor(region_z / BLOCK_SIZE)), 5u);
    let local_x = region_x - f32(block_col) * BLOCK_SIZE;
    let local_z = region_z - f32(block_row) * BLOCK_SIZE;

    let fx = local_x / VERTEX_SPACING;
    let fz = local_z / VERTEX_SPACING;
    let i0 = min(u32(floor(fx)), 15u);
    let j0 = min(u32(floor(fz)), 15u);
    let tx = fx - f32(i0);
    let tz = fz - f32(j0);

    let base = vec2<u32>(block_col * BLOCK_VERTS + i0, block_row * BLOCK_VERTS + j0);
    let v00 = textureLoad(tile_map, base, 0);
    let v10 = textureLoad(tile_map, base + vec2<u32>(1u, 0u), 0);
    let v01 = textureLoad(tile_map, base + vec2<u32>(0u, 1u), 0);
    let v11 = textureLoad(tile_map, base + vec2<u32>(1u, 1u), 0);

    // BLOCK-local, not region-local: `sample` turns this into the tile's uv, and the authored
    // mapping restarts the tiling pattern at every block edge (`Mesh::from(&JMXVMAPM)` maps each
    // block's 17 vertices over `16 * splat_scale`). For splat scales 0.25/0.5/1 the two choices
    // differ by a whole number of repeats and would look identical, but at scale 2 and 4 they do
    // not — block-local is what the original client does.
    let uv = vec2(local_x, local_z);

    // Overwhelmingly the common case (median 8 distinct tiles per region, and most quads sit
    // well inside one tile's area): all four corners name the same tile, so the blend collapses
    // to a single sample.
    if v00.r == v10.r && v00.r == v01.r && v00.r == v11.r
        && v00.g == v10.g && v00.g == v01.g && v00.g == v11.g {
        return sample(1.0, v00.g, v00.r, uv, duv_dx, duv_dy);
    }

    let w00 = (1.0 - tx) * (1.0 - tz);
    let w10 = tx * (1.0 - tz);
    let w01 = (1.0 - tx) * tz;
    let w11 = tx * tz;

    var color = vec4(0.0, 0.0, 0.0, 0.0);
    // Each corner carries its own tiling scale, so a tile used at different scales across a
    // region still renders correctly — that used to be approximated by a single per-texel scale
    // read from the dominant corner.
    if w00 > 0.0 { color += sample(w00, v00.g, v00.r, uv, duv_dx, duv_dy); }
    if w10 > 0.0 { color += sample(w10, v10.g, v10.r, uv, duv_dx, duv_dy); }
    if w01 > 0.0 { color += sample(w01, v01.g, v01.r, uv, duv_dx, duv_dy); }
    if w11 > 0.0 { color += sample(w11, v11.g, v11.r, uv, duv_dx, duv_dy); }

    return color;
}

// `tile` indexes `tile_atlas` directly — the region-local index (0..REGION_TILE_SLOT_COUNT,
// baked into `tile_map` by `pack_tile_map`) on the `Material`-based path, or the map's raw
// 10-bit id on the hand-rolled-pipeline path (see the header comment above `tile_map`'s
// declaration). The index varies per fragment, so this relies on non-uniform indexing of a
// sampled-texture binding array (`SAMPLED_TEXTURE_AND_STORAGE_BUFFER_ARRAY_NON_UNIFORM_INDEXING`)
// — `check_tile_atlas_support` in block_splat_material.rs reports at startup if the device lacks
// it. The `min` clamp is a defensive backstop only: both paths already guarantee every baked
// index is in range.
fn sample(contrib: f32, splat: u32, tile: u32, uv: vec2<f32>, duv_dx: vec2<f32>, duv_dy: vec2<f32>) -> vec4<f32> {
#ifdef TERRAIN_HAND_ROLLED_PIPELINE
    let tex = tile_atlas[min(tile, 1023u)];
#else
    let tex = tile_atlas[min(tile, 63u)];
#endif
    let splat_scale = get_splat_scale(splat);
    // 16, not 4: one texture repeat spans `320 * splat_scale` world units (a 320-unit block
    // has 16 20-unit tiles), so splat_scale 1.0 => exactly one repeat per block (1x1), 0.25 =>
    // 4x4, etc. This matches the documented reference UV mapping in `JMXVMAPM::from`
    // (`assets/m/mod.rs`, `let max = 16.0 * splat_scale`). The old `4.0` tiled 4x too densely;
    // it was only masked before because secondary textures silently fell back to scale 1.0.
    let max_uv = 16.0 * splat_scale;
    let scale = 1.0 / (20.0 * max_uv);
    let uv2 = uv * scale;

    return textureSampleGrad(tex, tile_sampler, uv2, duv_dx * scale, duv_dy * scale) * contrib;
}

// Code -> repeat factor, all five from the shared params buffer so every
// code is live-calibratable (render-debug sliders). Playtest verdict
// 2026-08-10: the vertex "Scale" field does NOT drive tiling — every code
// matches vanilla at a constant 0.25 (one repeat per 80 world units), so
// all five default to 0.25 and the field's real meaning is UNKNOWN. The
// constant also removes the hard tiling seams a varying per-texel factor
// produced. See docs/formats/mapm-jmxvmapm.md.
fn get_splat_scale(scale: u32) -> f32 {
    var actual = terrain_params[1].w; // code 16 = the common default
    switch scale {
        case 0u: {
            actual = terrain_params[1].y;
        }
        case 8u: {
            actual = terrain_params[1].z;
        }
        case 16u, default {
            actual = terrain_params[1].w;
        }
        case 24u: {
            actual = terrain_params[2].x;
        }
        case 32u: {
            actual = terrain_params[2].y;
        }
    }

    return actual;
}

