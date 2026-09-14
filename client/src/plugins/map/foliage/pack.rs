//! Pack foliage layer: bundled CC0 grass sprites (assets/foliage/*.png)
//! rendered as cross-quad billboards on Grass/LongGrass-typed tiles. This is
//! NOT original 1.188 content — it only runs when `graphics.foliage.mode` is
//! `pack` or `both`. Each sprite becomes a two-quad tuft model that flows
//! through the same scatter/merge pipeline as the native grass, so the two
//! layers differ only in where their geometry comes from.

use bevy::asset::{Handle, LoadState};
use bevy::image::Image;
use bevy::math::Vec3;
use bevy::pbr::StandardMaterial;
use bevy::prelude::*;

use super::{FoliageLibrary, FoliageModel, FoliagePartGeometry};

/// Card files looked for under `assets/foliage/` (committed CC0 art —
/// composed ambientCG blade-scan tufts, see `assets/foliage/README.md` and
/// `tools/src/bin/gen_foliage_cards.rs`). Missing files are skipped with a
/// warning; if none load, the pack layer stays empty.
const PACK_SPRITES: &[&str] = &[
    "foliage/grass_a.png",
    "foliage/grass_b.png",
    "foliage/grass_c.png",
    "foliage/grass_d.png",
];

/// Tuft size in world units before `graphics.foliage.pack.scale` (SRO scale:
/// ~10 units per meter, so this is roughly a 1m grass tuft).
const TUFT_SIZE: f32 = 10.0;

/// Start loading the pack sprites; called from `init_foliage_library` when the
/// pack layer is active. Resolution happens in [`extract_pack_models`].
pub(super) fn start_loading(library: &mut FoliageLibrary, asset_server: &AssetServer) {
    for path in PACK_SPRITES {
        library.pack_loading.push(asset_server.load::<Image>(*path));
    }
}

/// Turn each loaded sprite into a cross-quad tuft model; drop sprites that
/// fail to load (file not present — the pack art is a manual, optional
/// download). Runs until every pending handle resolved one way or the other;
/// block builds with an active pack layer wait for that.
pub(super) fn extract_pack_models(
    mut library: ResMut<FoliageLibrary>,
    asset_server: Res<AssetServer>,
    config: Res<crate::plugins::config::ClientConfig>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let scale = config.graphics.foliage.pack.scale.max(0.01);
    let mut remaining = Vec::new();
    let pending = std::mem::take(&mut library.pack_loading);
    for handle in pending {
        match asset_server.load_state(&handle) {
            LoadState::Failed(_) => {
                warn!(
                    "foliage: pack sprite {:?} missing (see assets/foliage/README.md); skipping",
                    handle.path()
                );
            }
            LoadState::Loaded => {
                // white base: the cards carry their own color, and the
                // per-tile environment tint arrives as vertex colors
                let material = materials.add(StandardMaterial {
                    base_color: Color::WHITE,
                    base_color_texture: Some(handle.clone()),
                    alpha_mode: AlphaMode::Mask(0.5),
                    // both faces rasterize (cull off), but no double_sided
                    // normal flip — the cards carry up normals so both faces
                    // shade like the ground (see native.rs foliage_material)
                    double_sided: false,
                    cull_mode: None,
                    perceptual_roughness: 1.0,
                    reflectance: 0.0,
                    ..Default::default()
                });
                library
                    .pack
                    .push(cross_quad_model(material, TUFT_SIZE * scale));
            }
            _ => remaining.push(handle),
        }
    }
    library.pack_loading = remaining;
}

/// Two vertical quads crossed at 90°, `size` wide and tall, rooted at y=0.
/// Normals point up so the tuft takes the terrain's lighting instead of
/// showing a lit and a shadow side per quad.
fn cross_quad_model(material: Handle<StandardMaterial>, size: f32) -> FoliageModel {
    let h = size / 2.0;
    let mut positions = Vec::with_capacity(8);
    let mut normals = Vec::with_capacity(8);
    let mut uvs = Vec::with_capacity(8);
    let mut indices = Vec::with_capacity(12);
    for dir in [Vec3::X, Vec3::Z] {
        let base = positions.len() as u32;
        positions.extend([
            dir * -h,
            dir * h,
            dir * h + Vec3::Y * size,
            dir * -h + Vec3::Y * size,
        ]);
        normals.extend([Vec3::Y; 4]);
        uvs.extend([[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]]);
        indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    FoliageModel {
        parts: vec![FoliagePartGeometry {
            positions,
            normals,
            uvs,
            indices,
            material,
            color_factor: Vec3::ONE,
        }],
    }
}
