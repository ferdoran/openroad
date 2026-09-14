//! Image-based environment reflections: a tiny generated sky/ground cubemap
//! attached to each scene camera as a `GeneratedEnvironmentMapLight`, so
//! reflective surfaces have something to reflect. The original engine's
//! EnvMap pass (sphere-map highlight, see `assets/bmt/sheen.rs`) and the
//! flag-0x4 sun-specular materials both need this — without it, metallic
//! sheen texels read near-black and reflectance shows nothing. The original
//! modulated its EnvMap highlight by the global scene ENVIRONMENT color;
//! that's the future hook for `apply_environment` to drive this light's
//! intensity/tint per ENVI profile (not built yet — constant for now).

use bevy::asset::RenderAssetUsages;
use bevy::light::{EnvironmentMapLight, GeneratedEnvironmentMapLight};
use bevy::prelude::*;
use bevy::render::render_resource::{
    Extent3d, TextureDimension, TextureFormat, TextureViewDescriptor, TextureViewDimension,
};

/// cd/m²; calibrate together with the sheen material constants.
pub const SKY_REFLECTION_INTENSITY: f32 = 1200.0;

/// Frames to keep filtering after bevy first produces the baked
/// `EnvironmentMapLight`, before freezing (see [`freeze_baked_env_maps`]). A
/// full GPU filter pass converges in a single frame for our static source; the
/// margin only covers blue-noise / source-cube upload latency.
const BAKE_SETTLE_FRAMES: u8 = 8;

/// Builds the sky-reflection light component for a camera. The 8×8 flat-color
/// cube (horizon around, sky above, ground below) is deliberately tiny — it is
/// filtered once into diffuse/specular cubemaps and then frozen (see
/// [`freeze_baked_env_maps`]); it is *not* a resolution the renderer should
/// pay for every frame.
pub fn sky_reflection_env_light(images: &mut Assets<Image>) -> GeneratedEnvironmentMapLight {
    const SIZE: u32 = 8; // must be a power of two for the generate pass
                         // cube face order +X -X +Y -Y +Z -Z
    const FACES: [[u8; 4]; 6] = [
        [110, 130, 160, 255],
        [110, 130, 160, 255],
        [140, 175, 230, 255],
        [60, 50, 40, 255],
        [110, 130, 160, 255],
        [110, 130, 160, 255],
    ];
    let data: Vec<u8> = FACES
        .iter()
        .flat_map(|color| std::iter::repeat_n(*color, (SIZE * SIZE) as usize))
        .flatten()
        .collect();
    let mut image = Image::new(
        Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 6,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::Cube),
        ..default()
    });
    GeneratedEnvironmentMapLight {
        environment_map: images.add(image),
        intensity: SKY_REFLECTION_INTENSITY,
        ..default()
    }
}

/// Countdown parking a camera's probe for a few frames after its baked
/// `EnvironmentMapLight` appears, so the GPU filter passes converge before we
/// freeze it.
#[derive(Component)]
pub struct EnvMapBakeCountdown(u8);

/// Bake-once for the sky reflection probe. `GeneratedEnvironmentMapLight`
/// re-filters its source cubemap into the diffuse/specular IBL maps *every
/// frame* with no change detection — profiling put this at ~9 ms/frame, the
/// single largest render cost, even though our source is a constant flat-color
/// cube. Once bevy has inserted the filtered `EnvironmentMapLight` and a few
/// frames have let the compute passes settle, we strip
/// `GeneratedEnvironmentMapLight`: bevy's `SyncComponent` on-remove hook drops
/// the render-world `RenderEnvironmentMap`, so the per-frame filtering stops
/// while the baked `EnvironmentMapLight` keeps lighting reflective surfaces.
/// When the probe is later made to follow the ENVI day cycle, re-add the
/// component for one settle window on each change instead of leaving it on.
pub fn freeze_baked_env_maps(
    mut commands: Commands,
    newly_baked: Query<
        Entity,
        (
            With<GeneratedEnvironmentMapLight>,
            Added<EnvironmentMapLight>,
        ),
    >,
    mut settling: Query<(Entity, &mut EnvMapBakeCountdown)>,
) {
    for entity in &newly_baked {
        commands
            .entity(entity)
            .insert(EnvMapBakeCountdown(BAKE_SETTLE_FRAMES));
    }
    for (entity, mut countdown) in &mut settling {
        match countdown.0.checked_sub(1) {
            Some(next) => countdown.0 = next,
            None => {
                commands
                    .entity(entity)
                    .remove::<GeneratedEnvironmentMapLight>()
                    .remove::<EnvMapBakeCountdown>();
            }
        }
    }
}
