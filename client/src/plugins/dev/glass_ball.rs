use bevy::app::{App, Plugin};
use bevy::asset::Assets;
use bevy::math::Vec3;
use bevy::pbr::StandardMaterial;
use bevy::prelude::{
    default, ButtonInput, Color, Commands, Cylinder, GlobalTransform, KeyCode, Mesh, Mesh3d,
    MeshMaterial3d, Plane3d, Query, Res, ResMut, Sphere, Transform, Update, Visibility, With,
};

use crate::plugins::camera::DebugCamera;

pub struct GlassballPlugin;

impl Plugin for GlassballPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, spawn_glass_ball);
    }
}

fn spawn_glass_ball(
    mut commands: Commands,
    mut material_assets: ResMut<Assets<StandardMaterial>>,
    mut mesh_assets: ResMut<Assets<Mesh>>,
    flycam_query: Query<&GlobalTransform, With<DebugCamera>>,
    button_input: Res<ButtonInput<KeyCode>>,
) {
    if button_input.just_pressed(KeyCode::KeyB) {
        let Ok(transform) = flycam_query.single() else {
            return;
        };
        let _plane = mesh_assets.add(Mesh::from(Plane3d::default()));
        let cylinder = mesh_assets.add(Mesh::from(Cylinder::default()));
        let _icosphere = mesh_assets.add(Mesh::try_from(Sphere { radius: 0.9 }).unwrap());

        commands.spawn((
            Mesh3d(cylinder),
            MeshMaterial3d(material_assets.add(StandardMaterial {
                base_color: Color::WHITE,
                specular_transmission: 0.9,
                diffuse_transmission: 1.0,
                thickness: 1.8,
                ior: 1.5,
                perceptual_roughness: 0.12,
                ..default()
            })),
            Transform::from(transform.clone()).with_scale(Vec3::splat(50.0)),
            Visibility::default(),
        ));
    }
}
