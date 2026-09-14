//! Render synthetic BC1 and decoded RGBA side by side through the real material
//! shaders, then compare readback pixels. No PK2 data or window is needed.
//! Explicitly ignored in ordinary CI: run on each supported graphics backend.
use bevy::asset::RenderAssetUsages;
use bevy::camera::{RenderTarget, ScalingMode};
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::image::{ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use bevy::window::ExitCondition;
use bevy::winit::WinitPlugin;
use client::assets::bmt::sheen::{
    SheenExtension, SheenSettings, SroSheenMaterial, SHEEN_ALPHA_CUTOUT,
};
use client::assets::ddj::decode_dxt1_rgba8;
use std::time::{Duration, Instant};

#[derive(Resource, Default)]
struct Capture(Option<Image>);

// Four levels exercise both BC1 endpoint orderings and all four palette indices.
// Later mips intentionally differ, so accidentally regenerating them is detectable.
fn texture(compressed: bool, lod: u32) -> Image {
    let punch = [0, 0, 255, 255, 0xE4, 0xE4, 0xE4, 0xE4];
    let opaque = [0, 0xF8, 0x1F, 0, 0xE4, 0xE4, 0xE4, 0xE4];
    let blocks = [
        [punch, opaque, opaque, punch].concat(),
        punch.to_vec(),
        opaque.to_vec(),
        punch.to_vec(),
    ];
    let mut image = Image::new_uninit(
        Extent3d {
            width: 8,
            height: 8,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        if compressed {
            TextureFormat::Bc1RgbaUnormSrgb
        } else {
            TextureFormat::Rgba8UnormSrgb
        },
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_descriptor.mip_level_count = 4;
    image.data = Some(
        blocks
            .iter()
            .enumerate()
            .flat_map(|(level, bytes)| {
                if compressed {
                    bytes.clone()
                } else {
                    decode_dxt1_rgba8(bytes, 8 >> level, 8 >> level)
                }
            })
            .collect(),
    );
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        lod_min_clamp: lod as f32,
        lod_max_clamp: lod as f32,
        ..ImageSamplerDescriptor::nearest()
    });
    image
}

#[test]
#[ignore = "requires a graphics adapter; run with --ignored --nocapture on each backend"]
fn native_bc1_matches_decoded_mask_blend_and_sheen() {
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(AssetPlugin {
                file_path: format!("{}/../assets", env!("CARGO_MANIFEST_DIR")),
                ..default()
            })
            .set(WindowPlugin {
                primary_window: None,
                exit_condition: ExitCondition::DontExit,
                ..default()
            })
            .disable::<WinitPlugin>(),
    )
    .add_plugins(MaterialPlugin::<SroSheenMaterial>::default())
    .init_resource::<Capture>();
    app.finish();
    app.cleanup();
    let target = app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(Image::new_target_texture(
            256,
            96,
            TextureFormat::Rgba8UnormSrgb,
            None,
        ));
    app.world_mut().spawn((
        Camera3d::default(),
        RenderTarget::Image(target.clone().into()),
        Camera {
            clear_color: ClearColorConfig::Custom(Color::srgb(1.0, 0.0, 1.0)),
            ..default()
        },
        Projection::Orthographic(OrthographicProjection {
            scaling_mode: ScalingMode::Fixed {
                width: 8.0,
                height: 3.0,
            },
            ..OrthographicProjection::default_3d()
        }),
        Transform::from_xyz(0.0, 0.0, 10.0).looking_at(Vec3::ZERO, Vec3::Y),
        Tonemapping::None,
        Msaa::Off,
    ));
    app.world_mut().spawn((
        DirectionalLight {
            illuminance: 10_000.0,
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::IDENTITY,
    ));
    let mesh = app
        .world_mut()
        .resource_mut::<Assets<Mesh>>()
        .add(Rectangle::new(0.9, 0.9));
    let probe = app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(Image::new_fill(
            Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            &[128, 128, 128, 255],
            TextureFormat::Rgba8Unorm,
            RenderAssetUsages::RENDER_WORLD,
        ));
    for side in 0..2 {
        for lod in 0..4 {
            let image = app
                .world_mut()
                .resource_mut::<Assets<Image>>()
                .add(texture(side == 0, lod));
            for variant in 0..3 {
                let base = StandardMaterial {
                    base_color_texture: Some(image.clone()),
                    alpha_mode: match variant {
                        0 => AlphaMode::Mask(0.5),
                        1 => AlphaMode::Blend,
                        _ => AlphaMode::Mask(SHEEN_ALPHA_CUTOUT),
                    },
                    perceptual_roughness: 1.0,
                    reflectance: 0.0,
                    ..default()
                };
                let transform =
                    Transform::from_xyz((side * 4 + lod) as f32 - 3.5, 1.0 - variant as f32, 0.0);
                if variant == 2 {
                    let material = app
                        .world_mut()
                        .resource_mut::<Assets<SroSheenMaterial>>()
                        .add(SroSheenMaterial {
                            base,
                            extension: SheenExtension {
                                settings: SheenSettings {
                                    alpha_cutout: SHEEN_ALPHA_CUTOUT,
                                    ..default()
                                },
                                env_texture: probe.clone(),
                                shine_texture: probe.clone(),
                            },
                        });
                    app.world_mut().spawn((
                        Mesh3d(mesh.clone()),
                        MeshMaterial3d(material),
                        transform,
                    ));
                } else {
                    let material = app
                        .world_mut()
                        .resource_mut::<Assets<StandardMaterial>>()
                        .add(base);
                    app.world_mut().spawn((
                        Mesh3d(mesh.clone()),
                        MeshMaterial3d(material),
                        transform,
                    ));
                }
            }
        }
    }
    let start = Instant::now();
    let mut frames = 0;
    loop {
        app.update();
        frames += 1;
        if frames % 30 == 0 {
            app.world_mut()
                .spawn(Screenshot::image(target.clone()))
                .observe(
                    |event: On<ScreenshotCaptured>, mut capture: ResMut<Capture>| {
                        capture.0 = Some(event.image.clone());
                    },
                );
        }
        if let Some(image) = app.world_mut().resource_mut::<Capture>().0.take() {
            let data = image.data.unwrap();
            let mut drawn = [[0usize; 4]; 3];
            let mut differing = 0;
            for y in 0..96usize {
                for x in 0..128usize {
                    let a = &data[(y * 256 + x) * 4..][..4];
                    let b = &data[(y * 256 + x + 128) * 4..][..4];
                    // Compare within one sRGB byte's rounding on each side plus
                    // a BC1 interpolation rounding byte; alpha must agree exactly.
                    differing += usize::from(
                        a[..3].iter().zip(&b[..3]).any(|(a, b)| a.abs_diff(*b) > 3) || a[3] != b[3],
                    );
                    if b != [255, 0, 255, 255] {
                        drawn[y / 32][x / 32] += 1;
                    }
                }
            }
            // Require drawn pixels in every material/mip cell, including the
            // black 1x1 mip. Missing draws cannot pass as matching backgrounds.
            if drawn.iter().flatten().all(|n| *n > 50) {
                // In mip 1 each row's final texel is index 3 in punch-through
                // mode. Sample inside that column, away from the quad border.
                for row in 0..3usize {
                    for side in 0..2usize {
                        let x = side * 128 + 59;
                        let y = row * 32 + 16;
                        let pixel = &data[(y * 256 + x) * 4..][..4];
                        assert_eq!(
                            pixel,
                            &[255, 0, 255, 255],
                            "punch-through hole must reveal the background, row {row}, side {side}"
                        );
                    }
                }
                assert_eq!(
                    differing, 0,
                    "{differing} differing pixels across mask/blend/sheen"
                );
                eprintln!("BC1 comparison passed: four mips, mask/blend/sheen, {frames} frames");
                break;
            }
        }
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "material render/readback did not become ready"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
