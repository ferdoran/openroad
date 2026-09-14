use bevy::app::{App, AppExit};
use bevy::asset::Assets;
use bevy::asset::RenderAssetUsages;
use bevy::math::{vec2, vec3};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::wireframe::{Wireframe, WireframePlugin};
use bevy::prelude::{
    Asset, Camera3d, Commands, LinearRgba, Material, MaterialPlugin, Mesh, Mesh3d, MeshMaterial3d,
    ResMut, StandardMaterial, Startup, Transform, Vec3, Visibility,
};
use bevy::reflect::TypePath;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;
use bevy::DefaultPlugins;

fn main() -> AppExit {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(MaterialPlugin::<CustomMaterial>::default())
        .add_plugins(WireframePlugin::default())
        .add_systems(Startup, triangle)
        .run()
}

fn triangle(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<CustomMaterial>>,
    mut _standard_materials: ResMut<Assets<StandardMaterial>>,
) {
    let mesh: Mesh = Triangle {
        a: vec3(0.0, 0.0, 0.0),
        b: vec3(2.0, 1.0, 0.0),
        c: vec3(3.0, 0.0, 0.0),
    }
    .into();
    let mesh = meshes.add(mesh);

    let material = CustomMaterial {
        color: LinearRgba::BLUE,
    };
    let material = materials.add(material);

    commands
        .spawn((
            Mesh3d(mesh),
            MeshMaterial3d(material.clone()),
            Transform::default(),
            Visibility::default(),
        ))
        .insert(Wireframe);

    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(0.0, 0.0, 5.0).looking_at(Vec3::ZERO, Vec3::Y),
        Visibility::default(),
    ));
}

#[derive(AsBindGroup, Clone, TypePath, Asset)]
struct CustomMaterial {
    #[uniform(0)]
    color: LinearRgba,
}

impl Material for CustomMaterial {
    fn vertex_shader() -> ShaderRef {
        "shader.wgsl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "shader.wgsl".into()
    }
}

struct Triangle {
    pub a: Vec3,
    pub b: Vec3,
    pub c: Vec3,
}

impl Into<Mesh> for Triangle {
    fn into(self) -> Mesh {
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        );
        let position = vec![self.a.to_array(), self.b.to_array(), self.c.to_array()];
        let _uv = vec![vec2(0.0, 0.0), vec2(0.5, 1.0), vec2(1.0, 0.0)];

        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, position);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0., 1., 0.]; 3]);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0., 0.0]; 3]);
        mesh.insert_indices(Indices::U32(vec![0, 2, 1]));
        mesh
    }
}
