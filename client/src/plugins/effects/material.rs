//! Unlit material for JMXVEFF effect nodes.
//!
//! Materials are immutable and shared per (texture, blend factors): the
//! per-frame diffuse-graph tint is delivered as a packed ARGB `MeshTag`
//! read back out in the shader, so animating it neither re-prepares bind
//! groups nor splits batches. Only TextureSlide nodes get a private
//! instance whose `uv_offset_scale` uniform is animated. The D3D9 blend
//! factors from the effect file are applied verbatim through pipeline
//! specialization; `alpha_mode()` only routes the mesh into the
//! Transparent3d phase for back-to-front sorting.

use bevy::asset::Asset;
use bevy::image::Image;
use bevy::math::Vec4;
use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::{Material, MaterialPipeline, MaterialPipelineKey};
use bevy::prelude::{AlphaMode, Handle};
use bevy::reflect::TypePath;
use bevy::render::render_resource::{
    AsBindGroup, BlendComponent, BlendFactor, BlendOperation, BlendState, RenderPipelineDescriptor,
    SpecializedMeshPipelineError,
};
use bevy::shader::ShaderRef;

use crate::assets::efp::format::EeResource;

/// Scene-linear intensity of additive (dst = ONE) effect nodes. The original
/// D3D9 client applied no boost — additive is `src.rgb * src.a + dst.rgb`, so
/// `1.0` matches the authored data. (An earlier `8.0` HDR fudge over-brightened
/// dense additive stacks to white; dropped per playtest.)
const ADDITIVE_EFFECT_BOOST: f32 = 1.0;

#[derive(Asset, TypePath, AsBindGroup, Clone)]
#[bind_group_data(SroEffectMaterialKey)]
pub struct SroEffectMaterial {
    /// xy = UV offset, zw = UV scale (TextureSlide); default (0, 0, 1, 1).
    #[uniform(0)]
    pub uv_offset_scale: Vec4,
    /// x = color multiplier ([`ADDITIVE_EFFECT_BOOST`] × the color texture-op
    /// scale), y = alpha multiplier (the alpha texture-op scale), z = LDR
    /// additive mode (see [`ldr_mode`]), w unused. Static per material — a
    /// pure function of the blend combo + texture ops, so the shared-material
    /// scheme is unaffected.
    #[uniform(3)]
    pub params: Vec4,
    #[texture(1)]
    #[sampler(2)]
    pub texture: Option<Handle<Image>>,
    /// Raw D3DBLEND constants from the effect file.
    pub src_blend: u32,
    pub dst_blend: u32,
    /// The authored color multiplier before the live additive-intensity
    /// calibration factor (render-debug `effect_additive_intensity`).
    pub base_color_scale: f32,
    /// Emulate the original's 8-bit LDR backbuffer for additive nodes: the
    /// D3D9 client hard-clamped every blend result at 1.0 and quantized
    /// contributions below 1/255 away, so stacked additive plates saturated
    /// at white instead of accumulating into our HDR target (where bloom +
    /// tonemap desaturation wash saturated colors out and keep sub-threshold
    /// wisps visible). Implemented as saturating blending — premultiplied
    /// `rgb·a` with `(OneMinusDst, One)` factors, `dst' = rgb·a·(1−dst)+dst`,
    /// bounded at 1.0 — plus 1/255 output quantization in the shader.
    /// Only affects `dst_blend == 2` combos; toggleable live via the
    /// render-debug `effect_ldr_additive` switch for A/B.
    pub ldr_additive: bool,
}

impl Default for SroEffectMaterial {
    fn default() -> Self {
        Self {
            uv_offset_scale: Vec4::new(0.0, 0.0, 1.0, 1.0),
            // x = color mult, y = alpha mult (both default to no scaling),
            // z = LDR mode 1 (matches the default (5, 2) + ldr_additive).
            params: Vec4::new(ADDITIVE_EFFECT_BOOST, 1.0, 1.0, 0.0),
            texture: None,
            // SrcAlpha / One: the dominant additive combo in the corpus.
            src_blend: 5,
            dst_blend: 2,
            base_color_scale: 1.0,
            ldr_additive: true,
        }
    }
}

impl SroEffectMaterial {
    pub fn from_resource(resource: &EeResource, texture: Option<Handle<Image>>) -> Self {
        // texture_stage = [SrcArg1, SrcArg2, SrcColorOp, DstArg1, DstArg2,
        // DstAlphaOp]; the color/alpha ops carry the D3D MODULATE2X/4X
        // brightness the original client authored (see `texture_op_scale`).
        let color_scale =
            intensity_for_blend(resource.dst_blend) * texture_op_scale(resource.texture_stage[2]);
        let alpha_scale = texture_op_scale(resource.texture_stage[5]);
        let mut material = Self {
            texture,
            params: Vec4::new(color_scale, alpha_scale, 0.0, 0.0),
            src_blend: resource.src_blend,
            dst_blend: resource.dst_blend,
            base_color_scale: color_scale,
            ..Default::default()
        };
        material.set_ldr_additive(material.ldr_additive);
        material
    }

    /// Flips the LDR-additive emulation and keeps the shader-side mode
    /// uniform (`params.z`) in sync. Mutating the flag re-specializes the
    /// pipeline via [`SroEffectMaterialKey`].
    pub fn set_ldr_additive(&mut self, on: bool) {
        self.ldr_additive = on;
        self.params.z = ldr_mode(self.src_blend, self.dst_blend, on);
    }
}

/// The shader's `effect_params.z`: 0 = plain D3D-mapped blending, 1 =
/// saturating LDR additive with SrcAlpha premultiplied into the color, 2 =
/// saturating LDR additive without a premultiply (src factor ONE). Must
/// agree with the blend-factor selection in `specialize` — both derive from
/// this function/`ldr_blend` pair.
fn ldr_mode(src_blend: u32, dst_blend: u32, ldr_additive: bool) -> f32 {
    match (ldr_additive, src_blend, dst_blend) {
        (true, 5, 2) => 1.0, // SrcAlpha/One
        (true, 2, 2) => 2.0, // One/One
        _ => 0.0,            // non-additive or unhandled src factor: unchanged
    }
}

/// Additive combos (destination factor ONE) get the HDR boost; everything
/// else renders at authored brightness.
fn intensity_for_blend(dst_blend: u32) -> f32 {
    if dst_blend == 2 {
        ADDITIVE_EFFECT_BOOST
    } else {
        1.0
    }
}

/// The output multiplier of a fixed-function `D3DTEXTUREOP` color/alpha op. The
/// SRO corpus uses MODULATE (tex*diffuse, ×1), SELECTARG1 (tex, ×1) and the
/// brightened MODULATE2X/4X (and ADDSIGNED2X); only the ×N ops scale the result.
/// https://learn.microsoft.com/en-us/windows/win32/direct3d9/d3dtextureop
fn texture_op_scale(op: u32) -> f32 {
    match op {
        5 => 2.0, // MODULATE2X
        6 => 4.0, // MODULATE4X
        9 => 2.0, // ADDSIGNED2X (approx: the 2x brightening)
        _ => 1.0, // DISABLE / SELECTARG1/2 / MODULATE / ADD ...
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct SroEffectMaterialKey {
    src_blend: u32,
    dst_blend: u32,
    ldr_additive: bool,
}

impl From<&SroEffectMaterial> for SroEffectMaterialKey {
    fn from(material: &SroEffectMaterial) -> Self {
        Self {
            src_blend: material.src_blend,
            dst_blend: material.dst_blend,
            ldr_additive: material.ldr_additive,
        }
    }
}

/// D3DBLEND -> wgpu blend factor.
/// https://learn.microsoft.com/en-us/windows/win32/direct3d9/d3dblend
pub fn d3d_blend_factor(value: u32) -> BlendFactor {
    match value {
        1 => BlendFactor::Zero,
        2 => BlendFactor::One,
        3 => BlendFactor::Src,
        4 => BlendFactor::OneMinusSrc,
        5 => BlendFactor::SrcAlpha,
        6 => BlendFactor::OneMinusSrcAlpha,
        7 => BlendFactor::DstAlpha,
        8 => BlendFactor::OneMinusDstAlpha,
        9 => BlendFactor::Dst,
        10 => BlendFactor::OneMinusDst,
        11 => BlendFactor::SrcAlphaSaturated,
        // 12/13 (BOTHSRCALPHA/BOTHINVSRCALPHA) are src-only constants that
        // also prescribe the destination factor; see `blend_components`. As a
        // lone factor (should not occur as a dst) use the source side.
        12 => BlendFactor::SrcAlpha,
        13 => BlendFactor::OneMinusSrcAlpha,
        14 => BlendFactor::Constant,         // BLENDFACTOR
        15 => BlendFactor::OneMinusConstant, // INVBLENDFACTOR
        other => {
            bevy::log::warn!("unmapped D3DBLEND constant {other}, falling back to One");
            BlendFactor::One
        }
    }
}

/// The (src, dst) wgpu factors for a D3D blend pair, honoring the legacy
/// `BOTHSRCALPHA` (12) / `BOTHINVSRCALPHA` (13) source constants that fix the
/// destination factor implicitly (D3D9: 12 -> dst INVSRCALPHA, 13 -> dst
/// SRCALPHA).
pub fn blend_components(src: u32, dst: u32) -> (BlendFactor, BlendFactor) {
    match src {
        12 => (BlendFactor::SrcAlpha, BlendFactor::OneMinusSrcAlpha),
        13 => (BlendFactor::OneMinusSrcAlpha, BlendFactor::SrcAlpha),
        _ => (d3d_blend_factor(src), d3d_blend_factor(dst)),
    }
}

impl Material for SroEffectMaterial {
    fn vertex_shader() -> ShaderRef {
        "shaders/sro_effect.wgsl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "shaders/sro_effect.wgsl".into()
    }

    #[inline]
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        let key_data = &key.bind_group_data;
        let (src_factor, dst_factor) = if ldr_mode(
            key_data.src_blend,
            key_data.dst_blend,
            key_data.ldr_additive,
        ) != 0.0
        {
            // Saturating LDR-additive emulation: the shader premultiplies
            // the D3D source factor into the color (see `ldr_mode`), so
            // the hardware factor becomes the saturation term.
            (BlendFactor::OneMinusDst, BlendFactor::One)
        } else {
            blend_components(key_data.src_blend, key_data.dst_blend)
        };
        let blend = BlendComponent {
            src_factor,
            dst_factor,
            operation: BlendOperation::Add,
        };
        if let Some(fragment) = descriptor.fragment.as_mut() {
            for target in fragment.targets.iter_mut().flatten() {
                target.blend = Some(BlendState {
                    color: blend,
                    alpha: blend,
                });
            }
        }
        if let Some(depth) = descriptor.depth_stencil.as_mut() {
            depth.depth_write_enabled = Some(false);
        }
        // Effect plates/meshes are authored assuming no backface culling;
        // this also sidesteps winding flips under mirrored (scale.x = -1)
        // character wrappers.
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn additive_nodes_get_the_hdr_boost() {
        assert_eq!(intensity_for_blend(2), ADDITIVE_EFFECT_BOOST); // ONE
        assert_eq!(intensity_for_blend(6), 1.0); // INVSRCALPHA
        assert_eq!(intensity_for_blend(1), 1.0); // ZERO
    }

    #[test]
    fn ldr_mode_covers_additive_combos_only() {
        assert_eq!(ldr_mode(5, 2, true), 1.0); // SrcAlpha/One: premultiply
        assert_eq!(ldr_mode(2, 2, true), 2.0); // One/One: no premultiply
        assert_eq!(ldr_mode(5, 6, true), 0.0); // alpha blend: untouched
        assert_eq!(ldr_mode(3, 2, true), 0.0); // unhandled src factor
        assert_eq!(ldr_mode(5, 2, false), 0.0); // toggled off
    }

    #[test]
    fn set_ldr_additive_keeps_uniform_in_sync() {
        let mut material = SroEffectMaterial::default(); // (5, 2), LDR on
        assert_eq!(material.params.z, 1.0);
        material.set_ldr_additive(false);
        assert_eq!(material.params.z, 0.0);
        material.set_ldr_additive(true);
        assert_eq!(material.params.z, 1.0);
    }

    #[test]
    fn common_corpus_combos_map_to_expected_factors() {
        // (5, 2): additive glow
        assert_eq!(d3d_blend_factor(5), BlendFactor::SrcAlpha);
        assert_eq!(d3d_blend_factor(2), BlendFactor::One);
        // (5, 6): classic alpha blending
        assert_eq!(d3d_blend_factor(6), BlendFactor::OneMinusSrcAlpha);
        assert_eq!(d3d_blend_factor(1), BlendFactor::Zero);
        assert_eq!(d3d_blend_factor(11), BlendFactor::SrcAlphaSaturated);
    }
}
