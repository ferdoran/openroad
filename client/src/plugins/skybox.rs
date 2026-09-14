use std::f32::consts::TAU;

use crate::plugins::map::assets::MapsAssets;
use crate::GameState;
use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor};
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, MeshVertexBufferLayoutRef, PrimitiveTopology};
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Face, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
};
use bevy::shader::ShaderRef;

pub struct SkyboxPlugin;

#[derive(Component)]
struct Skybox;

/// Extra offset applied on top of the camera position by `follow_active_camera` —
/// the cloud discs float this far above the camera.
#[derive(Component)]
struct SkyboxOffset(Vec3);

/// Clouds are rendered as two textured layers scrolling at different speeds/scales
/// over the gradient sky cuboid, mimicking the original client's parallax cloud effect.
/// Cloud layer material handles, exposed so `plugins/environment` can drive the layers'
/// tint and opacity from the profile's cloud graphs over the day cycle (unlit white
/// clouds would otherwise glow at full brightness through the night). `near` shows
/// cloud1.ddj (ENVI cloud_near_alpha), `far` shows cloud99.ddj (cloud_far_alpha).
#[derive(Resource)]
pub struct CloudMaterials {
    pub near: Handle<CloudMaterial>,
    pub far: Handle<CloudMaterial>,
}

/// Uniform block of `shaders/cloud_layer.wgsl` — see there for the field semantics.
#[derive(ShaderType, Clone, Copy, Debug)]
pub struct CloudSettings {
    pub tint: Vec4,
    /// UV/sec. The offset is `scroll_speed * globals.time`, evaluated in the
    /// shader: advancing an offset from a system instead meant a
    /// `materials.get_mut` on both layers every frame forever, and a mutated
    /// material re-prepares its bind group — one of the few unconditional
    /// per-frame asset writes left in the tree. Cloud drift is monotonic, so
    /// unlike an effect node's `age` (see `perf-future-levers.md` §2) there is
    /// no loop reset or pause a global clock would lose.
    pub scroll_speed: Vec2,
    pub uv_tiles: f32,
    pub inner: f32,
}

/// Scrolling cloud texture with a per-fragment radial rim fade (see the shader for why
/// the fade cannot live in vertex alpha). Unlit and unfogged like the sky behind it.
#[derive(Asset, TypePath, AsBindGroup, Clone)]
pub struct CloudMaterial {
    #[uniform(0)]
    pub settings: CloudSettings,
    #[texture(1)]
    #[sampler(2)]
    pub texture: Handle<Image>,
    /// Sort-order tie breaker between the two nearly co-located layers.
    pub depth_bias: f32,
}

impl Material for CloudMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/cloud_layer.wgsl".into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }

    fn depth_bias(&self) -> f32 {
        self.depth_bias
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        // A horizontal disc seen from below; nothing worth culling.
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

#[derive(Resource)]
pub struct SkyboxMaterial(pub Handle<SkyGradientMaterial>);

/// Vertical sky gradient (see `shaders/sky_gradient.wgsl`): horizon (bottom) → zenith
/// (top), plus the scene's distance-fog color that the lowest band blends into so the
/// fully-fogged terrain horizon meets the sky seamlessly. Colors in linear RGB. Driven
/// at runtime by `plugins/environment` from the ENVI SkyTopColor/SkyBottomColor/FogColor
/// graphs.
#[derive(ShaderType, Clone, Copy, Debug)]
pub struct SkyGradient {
    pub top_color: Vec4,
    pub bottom_color: Vec4,
    pub fog_color: Vec4,
    /// x = procedural star intensity (0..1, from the ENVI NightIntensity graph);
    /// yzw unused.
    pub params: Vec4,
}

#[derive(Asset, TypePath, AsBindGroup, Clone)]
pub struct SkyGradientMaterial {
    #[uniform(0)]
    pub gradient: SkyGradient,
}

impl Material for SkyGradientMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/sky_gradient.wgsl".into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Opaque
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        // Render only inner faces while the camera is inside the cube.
        descriptor.primitive.cull_mode = Some(Face::Front);
        Ok(())
    }
}

/// The default sky color in linear RGB, as a gradient uniform color.
pub fn default_sky_gradient_color() -> Vec4 {
    let sky = LinearRgba::from(Srgba::hex(SKY_COLOR_HEX).unwrap());
    Vec4::new(sky.red, sky.green, sky.blue, 1.0)
}

/// The whole default gradient: flat sky color with the default fog color at the horizon.
pub fn default_sky_gradient() -> SkyGradient {
    let sky = default_sky_gradient_color();
    let fog = LinearRgba::from(crate::plugins::map::terrain::rendering::FOG_COLOR);
    SkyGradient {
        top_color: sky,
        bottom_color: sky,
        fog_color: Vec4::new(fog.red, fog.green, fog.blue, 1.0),
        params: Vec4::ZERO,
    }
}

const CLOUD_LAYER_NEAR_SCROLL_SPEED: Vec2 = Vec2::new(0.006, 0.0015);
const CLOUD_LAYER_FAR_SCROLL_SPEED: Vec2 = Vec2::new(0.0025, 0.0006);

/// Flat sky color, exposed so the high-quality water shader's faked reflection tint
/// (see `water_hq_material.rs`) can match it instead of drifting out of sync.
pub const SKY_COLOR_HEX: &str = "9FC3DD";

impl Plugin for SkyboxPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<SkyGradientMaterial>::default())
            .add_plugins(MaterialPlugin::<CloudMaterial>::default())
            .add_systems(OnEnter(GameState::Game), setup)
            .add_systems(
                Update,
                follow_active_camera.run_if(in_state(GameState::Game)),
            );
    }
}

pub fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut cloud_materials: ResMut<Assets<CloudMaterial>>,
    mut sky_materials: ResMut<Assets<SkyGradientMaterial>>,
    mut images: ResMut<Assets<Image>>,
    maps_assets: Res<MapsAssets>,
) {
    // Cloud textures scroll via a moving UV offset, so they need to wrap at the edges
    // instead of the default clamp-to-edge, or the animation would streak once it
    // moves past the [0, 1] range.
    for handle in [&maps_assets.cloud_layer_1, &maps_assets.cloud_layer_2] {
        if let Some(mut image) = images.get_mut(handle) {
            image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
                address_mode_u: ImageAddressMode::Repeat,
                address_mode_v: ImageAddressMode::Repeat,
                ..default()
            });
        }
    }

    // sky
    let skybox_material = sky_materials.add(SkyGradientMaterial {
        gradient: default_sky_gradient(),
    });
    commands.insert_resource(SkyboxMaterial(skybox_material.clone()));
    // Wide enough (110k half extent) that the 90k/95k cloud discs stay fully inside:
    // the opaque cube writes depth, so anything poking through its walls gets clipped
    // along the cube's silhouette. Corners reach ~190k, still inside the 200k far plane.
    commands.spawn((
        Mesh3d(meshes.add(Mesh::from(Cuboid::default()))),
        MeshMaterial3d(skybox_material),
        Transform::from_scale(Vec3::splat(220_000.0)),
        Visibility::default(),
        NotShadowCaster,
        Skybox,
        Name::from("Skybox"),
    ));

    // Large negative depth biases pin the cloud layers to the very back of the
    // transparent phase (drawn before the sun/moon at CELESTIAL_DEPTH_BIAS and before
    // every regular world transparent): the discs' origins float almost directly above
    // the camera with near-zero forward depth, so the default view-depth sort would
    // otherwise place them in front of everything.
    let far = spawn_cloud_layer(
        &mut commands,
        &mut meshes,
        &mut cloud_materials,
        maps_assets.cloud_layer_2.clone(),
        95_000.0,
        17_000.0,
        CLOUD_LAYER_FAR_SCROLL_SPEED,
        -1_000_100.0,
        "CloudLayerFar",
    );
    let near = spawn_cloud_layer(
        &mut commands,
        &mut meshes,
        &mut cloud_materials,
        maps_assets.cloud_layer_1.clone(),
        90_000.0,
        14_000.0,
        CLOUD_LAYER_NEAR_SCROLL_SPEED,
        -1_000_000.0,
        "CloudLayerNear",
    );
    commands.insert_resource(CloudMaterials { near, far });
}

/// Flat cloud disc; the rim fade happens per-fragment in the shader, so the mesh is a
/// plain triangle fan. UVs are planar so the texture tiles/scrolls like the old cube's
/// top face did (`uv_tiles` ≈ one repeat per ~90k world units, matching the old scale).
fn cloud_disc_mesh(radius: f32, uv_tiles: f32) -> Mesh {
    const SEGMENTS: usize = 48;

    let mut positions = vec![[0.0f32, 0.0, 0.0]];
    let mut uvs = vec![[0.5 * uv_tiles, 0.5 * uv_tiles]];
    for i in 0..SEGMENTS {
        let angle = i as f32 / SEGMENTS as f32 * TAU;
        let (x, z) = (angle.cos() * radius, angle.sin() * radius);
        positions.push([x, 0.0, z]);
        uvs.push([
            (x / (2.0 * radius) + 0.5) * uv_tiles,
            (z / (2.0 * radius) + 0.5) * uv_tiles,
        ]);
    }

    let n = SEGMENTS as u32;
    let mut indices: Vec<u32> = Vec::with_capacity(SEGMENTS * 3);
    for i in 0..n {
        // Winding is irrelevant; the material culls nothing.
        indices.extend([0, 1 + i, 1 + (i + 1) % n]);
    }

    let normals = vec![[0.0f32, -1.0, 0.0]; positions.len()];
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    .with_inserted_indices(Indices::U32(indices))
}

#[allow(clippy::too_many_arguments)]
fn spawn_cloud_layer(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<CloudMaterial>,
    texture: Handle<Image>,
    radius: f32,
    height: f32,
    scroll_speed: Vec2,
    depth_bias: f32,
    name: &'static str,
) -> Handle<CloudMaterial> {
    // See the call site for the depth_bias scheme; the two layers' biases also order
    // the near layer over the far one.
    let uv_tiles = 2.0 * radius / 90_000.0;
    let material = materials.add(CloudMaterial {
        settings: CloudSettings {
            tint: Vec4::ONE,
            scroll_speed,
            uv_tiles,
            inner: 0.55,
        },
        texture,
        depth_bias,
    });
    let handle = material.clone();
    commands.spawn((
        Mesh3d(meshes.add(cloud_disc_mesh(radius, uv_tiles))),
        MeshMaterial3d(material),
        Transform::default(),
        Visibility::default(),
        NotShadowCaster,
        Skybox,
        SkyboxOffset(Vec3::Y * height),
        Name::from(name),
    ));
    handle
}

fn follow_active_camera(
    mut skybox_query: Query<(&mut Transform, Option<&SkyboxOffset>), With<Skybox>>,
    camera_query: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
) {
    let Some(camera_translation) = camera_query
        .iter()
        .find(|(camera, _)| camera.is_active)
        .map(|(_, transform)| transform.translation())
    else {
        return;
    };

    for (mut skybox_transform, offset) in &mut skybox_query {
        skybox_transform.translation =
            camera_translation + offset.map_or(Vec3::ZERO, |offset| offset.0);
    }
}
