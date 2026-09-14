use bevy::asset::Asset;
use bevy::math::Vec2;
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::StandardMaterial;
use bevy::reflect::TypePath;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

// Idea: TexAni resources (waterfalls, dungeon canal water — every .bsr with
// a TexAni mod, see `SroResource::texani_mods`) continuously scroll their
// material UVs. The original client applies a D3D texture-transform matrix
// scaled by time each frame; in all observed data the matrix is a pure UV
// translation, so this extension keeps the full standard PBR pipeline and
// swaps the fragment shader for one that offsets the UVs by
// `uv_speed * globals.time` before sampling — shader-clock driven, so a
// single shared material animates every instance with zero per-frame CPU
// or bind-group churn (unlike mutating `StandardMaterial::uv_transform`).
pub type SroUvScrollMaterial = ExtendedMaterial<StandardMaterial, UvScrollExtension>;

#[derive(Asset, AsBindGroup, TypePath, Debug, Clone, Default)]
pub struct UvScrollExtension {
    #[uniform(100)]
    pub settings: UvScrollSettings,
}

/// Std140 layout, field order must match the `UvScrollSettings` struct in
/// `sro_uv_scroll.wgsl`.
#[derive(Clone, Copy, Debug, Default, ShaderType)]
pub struct UvScrollSettings {
    /// UV scroll speed in uv/sec, from the TexAni matrix translation
    /// (negative V = the texture flows down the mesh).
    pub uv_speed: Vec2,
    pub _padding: Vec2,
}

impl MaterialExtension for UvScrollExtension {
    fn fragment_shader() -> ShaderRef {
        "shaders/sro_uv_scroll.wgsl".into()
    }
}
