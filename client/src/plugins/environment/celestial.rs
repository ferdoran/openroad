// Idea: the sun and moon are unlit, additively blended quads riding the day-cycle circle
// (the same `(t - 0.25) * TAU` angle that drives the directional light, but *unclamped* —
// the billboard genuinely sets and rises while the light's pitch stays floored above the
// horizon, see `apply_environment`). They sit between the gradient sky cuboid and the
// cloud layers, re-centered on the active camera every frame like the rest of the skybox.
// Textures come from the user's own Map.pk2 (the exact archive layout varies by client
// version), so a missing/failed texture just keeps the billboard hidden with a one-shot
// warning instead of failing the load screen — which is also why these are lazy
// `asset_server.load`s and not part of the `MapsAssets` collection. Additive blending
// makes the textures' black background transparent for free. The sun quad is tinted by
// the profile's SunColor graph in `apply_environment`.

use std::f32::consts::TAU;

use bevy::asset::LoadState;
use bevy::light::NotShadowCaster;
use bevy::prelude::*;

use super::TimeOfDay;
use crate::GameState;

// Map.pk2 keeps these under `sun/`: the sun is authored as lens-flare sprites
// (`lens1`–`lens8`; `lens3` is the main 256x256 disc/glow used here — compositing the
// full flare chain is future work), and the moon as 30 lunar-phase textures
// (`moon01`–`moon30`); a fixed phase is used until there's a day counter to drive it.
const SUN_TEXTURE_PATH: &str = "map://sun/lens3.ddj";
const MOON_TEXTURE_PATH: &str = "map://sun/moon15.ddj";
/// Distance from the camera — deliberately *below* the cloud discs' 14k/17k heights so
/// the transparent-phase sort draws sun and moon after (over) the clouds: additively
/// blended, they read as glowing through the cloud layer instead of being swallowed by
/// it. Terrain still occludes them normally via the depth test.
const CELESTIAL_DISTANCE: f32 = 10_000.0;
const SUN_SIZE: f32 = 1_500.0;
const MOON_SIZE: f32 = 1_100.0;
/// Transparent-phase sort bias: Bevy sorts by view-space depth of the mesh *origin*
/// plus this bias (bigger = drawn later). The cloud discs' origins float almost
/// directly above the camera — nearly zero forward depth — so without biases they
/// always sort "closest" and composite over the sun/moon. The sky stack is pinned to
/// the very back of the phase instead: clouds first (see `spawn_cloud_layer`), then
/// these discs, then all regular world transparents.
pub const CELESTIAL_DEPTH_BIAS: f32 = -900_000.0;
/// Additive discs only ever add their texture onto the sky behind them; at plain white
/// they wash out against anything brighter than a night sky.
pub const SUN_TINT_BOOST: f32 = 4.0;
const MOON_TINT_BOOST: f32 = 3.0;

#[derive(Clone, Copy, PartialEq)]
pub enum CelestialKind {
    Sun,
    Moon,
}

#[derive(Component)]
pub struct CelestialBody {
    pub kind: CelestialKind,
    texture: Handle<Image>,
    warned_missing: bool,
}

/// Material handles so `apply_environment` can tint the sun disc by the SunColor graph.
#[derive(Resource)]
pub struct CelestialMaterials {
    pub sun: Handle<StandardMaterial>,
}

/// Direction from the camera toward the sun for a day-cycle time `t` (unclamped; goes
/// below the horizon at night). The moon is diametrically opposite.
pub fn sun_direction(t: f32) -> Vec3 {
    let theta = (t - 0.25) * TAU;
    Vec3::new(0.0, theta.sin(), theta.cos())
}

pub struct CelestialPlugin;

impl Plugin for CelestialPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(GameState::Game), spawn_celestials)
            .add_systems(
                Update,
                (position_celestials, update_celestial_visibility)
                    .run_if(in_state(GameState::Game)),
            );
    }
}

fn spawn_celestials(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    asset_server: Res<AssetServer>,
) {
    let mut spawn =
        |kind: CelestialKind, path: &str, size: f32, color: Color, name: &'static str| {
            let texture: Handle<Image> = asset_server.load(path.to_string());
            let material = materials.add(StandardMaterial {
                base_color: color,
                base_color_texture: Some(texture.clone()),
                unlit: true,
                alpha_mode: AlphaMode::Add,
                cull_mode: None,
                depth_bias: CELESTIAL_DEPTH_BIAS,
                ..default()
            });
            let material_handle = material.clone();
            commands.spawn((
                Mesh3d(meshes.add(Mesh::from(Rectangle::from_size(Vec2::splat(size))))),
                MeshMaterial3d(material),
                Transform::default(),
                Visibility::Hidden,
                NotShadowCaster,
                CelestialBody {
                    kind,
                    texture,
                    warned_missing: false,
                },
                Name::from(name),
            ));
            material_handle
        };

    let sun_material = spawn(
        CelestialKind::Sun,
        SUN_TEXTURE_PATH,
        SUN_SIZE,
        Color::LinearRgba(LinearRgba::WHITE * SUN_TINT_BOOST),
        "Sun disc",
    );
    spawn(
        CelestialKind::Moon,
        MOON_TEXTURE_PATH,
        MOON_SIZE,
        Color::LinearRgba(LinearRgba::WHITE * MOON_TINT_BOOST),
        "Moon disc",
    );
    commands.insert_resource(CelestialMaterials { sun: sun_material });
}

fn position_celestials(
    camera_query: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    mut bodies: Query<(&CelestialBody, &mut Transform)>,
    tod: Res<TimeOfDay>,
) {
    let Some(camera_position) = camera_query
        .iter()
        .find(|(camera, _)| camera.is_active)
        .map(|(_, transform)| transform.translation())
    else {
        return;
    };

    let sun_dir = sun_direction(tod.t);
    for (body, mut transform) in &mut bodies {
        let dir = match body.kind {
            CelestialKind::Sun => sun_dir,
            CelestialKind::Moon => -sun_dir,
        };
        transform.translation = camera_position + dir * CELESTIAL_DISTANCE;
        // Billboard: the quad's plane faces the camera (cull is off, so orientation of the
        // normal doesn't matter).
        transform.look_at(camera_position, Vec3::Y);
    }
}

/// Shows a body once its texture loaded; a failed load warns once and keeps it hidden,
/// since the PK2 path can differ per client version.
fn update_celestial_visibility(
    asset_server: Res<AssetServer>,
    mut bodies: Query<(&mut CelestialBody, &mut Visibility)>,
) {
    for (mut body, mut visibility) in &mut bodies {
        let load_state = asset_server
            .get_load_state(&body.texture)
            .unwrap_or(LoadState::NotLoaded);
        let loaded = load_state.is_loaded();
        if load_state.is_failed() && !body.warned_missing {
            body.warned_missing = true;
            let path = match body.kind {
                CelestialKind::Sun => SUN_TEXTURE_PATH,
                CelestialKind::Moon => MOON_TEXTURE_PATH,
            };
            warn!(
                "celestial texture '{}' not found in the PK2 archives; billboard stays hidden \
                 (find the real path with: make pk2 list PK2=$SRO_PATH/Map.pk2)",
                path
            );
        }
        let target = if loaded {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        if *visibility != target {
            *visibility = target;
        }
    }
}
