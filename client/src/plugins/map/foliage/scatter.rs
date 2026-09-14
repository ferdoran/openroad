//! Deterministic foliage scatter + merge.
//!
//! Foliage is not authored per instance: tile2d.ifo only says "this tile gets
//! N tufts of model M". Placement therefore has to be *derived* — and derived
//! identically every time, so grass reappears in the same spots when a region
//! streams back in. A splitmix64 stream seeded purely from
//! (region, block, tile-or-cell, layer, pair) provides that; per tuft the
//! stream is consumed in a fixed order (cell pick where applicable, jitter-x,
//! jitter-z, yaw[, variant]).
//!
//! All tufts of a block are baked into one mesh per material (positions
//! pre-transformed, block-local space) instead of spawning per-tuft entities:
//! a fully grassy block can carry thousands of tufts, and per-entity
//! transform/visibility/extraction cost at that count dwarfs the one-time
//! vertex copy. Block-local space is the same frame the merged ground mesh
//! uses (`merge_block_meshes`: vertex (x,z) at (x*20, height, z*20)), so a
//! child of the block entity with an identity transform sits exactly on the
//! ground, mirrored group transform included — only the triangle winding has
//! to compensate for the mirror, exactly like the terrain mesh itself.

use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::camera::primitives::Aabb;
use bevy::math::{Quat, Vec3};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::Mesh;

use crate::assets::m::{TerrainBlock, WaterType};
use crate::util::mesh::reverse_winding_u32;

use super::{FoliageLibrary, FoliageModel, NativeModelState};

/// Cells per block side (16 cells of 20 units over 17 vertices).
const CELLS_PER_SIDE: usize = 16;
/// World units per cell.
const CELL_SIZE: f32 = 20.0;

/// Scatter stream layer ids — native and pack draws must never correlate.
pub const LAYER_NATIVE: u8 = 0;
pub const LAYER_PACK: u8 = 1;

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Uniform in [0, 1).
fn unit_f32(state: &mut u64) -> f32 {
    (splitmix64(state) >> 40) as f32 / (1u64 << 24) as f32
}

/// Placement seed: purely positional so scatter survives region reloads.
/// The `cell_idx` slot carries the cell index for per-cell streams (pack) and
/// the 10-bit tile id for the block-level native streams — both fit u16 and
/// the layer byte keeps the two spaces from colliding.
fn cell_seed(region: u16, block_idx: u8, cell_idx: u16, layer: u8, pair: u8) -> u64 {
    (region as u64) << 40
        | (block_idx as u64) << 32
        | (cell_idx as u64) << 16
        | (layer as u64) << 8
        | pair as u64
}

/// One merged foliage mesh for one material of one block.
pub struct FoliageBuild {
    pub material: bevy::asset::Handle<bevy::pbr::StandardMaterial>,
    pub mesh: Mesh,
    pub aabb: Aabb,
    pub tufts: u32,
}

/// Environment tint inputs of one block build. Colors are baked as vertex
/// colors (linear Float32x4 — bevy's `ATTRIBUTE_COLOR` accepts only its
/// canonical format) — one flat color per tuft: the underlying tile's
/// average color lerped in by the per-layer blend, darkened by the region's
/// baked per-cell light.
pub struct TintContext<'a> {
    /// Linear-space average color per grass tile id.
    pub tile_tints: &'a HashMap<u16, bevy::math::Vec3>,
    pub blend_native: f32,
    pub blend_pack: f32,
    /// The region's 96×96 `JMXVMAPT::tile_light` grid, if resident — one
    /// byte per 20×20 cell, row-major over the whole region.
    pub tile_light: Option<&'a [u8]>,
    /// 0 = off; 1 = full linear darkening toward the byte value.
    pub light_strength: f32,
}

impl TintContext<'static> {
    /// No-op tint (white, no baked light) — for tests and Off-tint configs.
    pub fn neutral() -> Self {
        static EMPTY: std::sync::OnceLock<HashMap<u16, Vec3>> = std::sync::OnceLock::new();
        Self {
            tile_tints: EMPTY.get_or_init(HashMap::new),
            blend_native: 0.0,
            blend_pack: 0.0,
            tile_light: None,
            light_strength: 0.0,
        }
    }
}

#[derive(Default)]
struct MergeBuffers {
    material: Option<bevy::asset::Handle<bevy::pbr::StandardMaterial>>,
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    colors: Vec<[f32; 4]>,
    indices: Vec<u32>,
    tufts: u32,
}

/// Scatter every active layer over `block` and return one merged mesh per
/// distinct material. Pure function of its inputs — byte-identical output for
/// identical inputs (the determinism the streaming teardown/rebuild relies on).
///
/// Native count semantics: `count` tufts per fully covered 320×320 **block**,
/// scaled by the tile's actual coverage (`covered_cells / 256`). The
/// per-20×20-cell reading was rejected in playtest as ~256× too crowded vs
/// the original; the exact exe unit remains unverified, so `native_density`
/// stays the correction knob (1.0 = the per-block reading).
#[allow(clippy::too_many_arguments)]
pub fn build_block_foliage(
    block: &TerrainBlock,
    region: u16,
    library: &FoliageLibrary,
    native_active: bool,
    native_density: f32,
    pack_active: bool,
    pack_density: f32,
    reverse_winding: bool,
    tint: &TintContext,
) -> Vec<FoliageBuild> {
    let block_idx = (block.z * 6 + block.x).clamp(0, 255) as u8;
    let mut merged: HashMap<bevy::asset::AssetId<bevy::pbr::StandardMaterial>, MergeBuffers> =
        HashMap::new();

    // group the block's cells by tile id (recipes only): native placement is
    // per (tile, block), so tufts pick a random covered cell
    let mut covered: HashMap<u16, Vec<(usize, usize)>> = HashMap::new();
    for cz in 0..CELLS_PER_SIDE {
        for cx in 0..CELLS_PER_SIDE {
            let tile_id = block.vertices[cz * 17 + cx].texture_id;
            if library.tile_recipes.contains_key(&tile_id) {
                covered.entry(tile_id).or_default().push((cx, cz));
            }
        }
    }
    let mut tile_ids: Vec<u16> = covered.keys().copied().collect();
    tile_ids.sort_unstable(); // deterministic iteration order

    for tile_id in tile_ids {
        let cells = &covered[&tile_id];
        let recipe = &library.tile_recipes[&tile_id];
        let tile_avg = tint.tile_tints.get(&tile_id).copied().unwrap_or(Vec3::ONE);
        let native_base = Vec3::ONE.lerp(tile_avg, tint.blend_native.clamp(0.0, 1.0));
        let pack_base = Vec3::ONE.lerp(tile_avg, tint.blend_pack.clamp(0.0, 1.0));
        let coverage = cells.len() as f32 / (CELLS_PER_SIDE * CELLS_PER_SIDE) as f32;

        if native_active {
            for (pair_idx, (model_id, count)) in recipe.pairs.iter().enumerate() {
                let Some(NativeModelState::Ready(model)) = library.native.get(model_id) else {
                    continue;
                };
                let tufts = (*count as f32 * native_density * coverage).round() as u32;
                // the tile id (10-bit) rides in the seed's cell slot: the
                // stream is per (block, tile, pair) now that placement is
                // block-level rather than cell-level
                let mut rng = cell_seed(
                    region,
                    block_idx,
                    tile_id,
                    LAYER_NATIVE,
                    pair_idx.min(255) as u8,
                );
                for _ in 0..tufts {
                    let (cx, cz) = cells[(splitmix64(&mut rng) % cells.len() as u64) as usize];
                    let rgb = native_base * cell_light_factor(tint, block, cx, cz);
                    stamp_tuft(block, cx, cz, model, rgb, &mut rng, &mut merged);
                }
            }
        }

        // pack layer keeps absolute per-cell density (non-original content,
        // documented as tufts per 20x20 cell)
        if pack_active && recipe.pack_eligible && !library.pack.is_empty() {
            for (cx, cz) in cells {
                let (cx, cz) = (*cx, *cz);
                let cell_idx = (cz * CELLS_PER_SIDE + cx) as u16;
                let tufts = pack_density.round() as u32;
                let mut rng = cell_seed(region, block_idx, cell_idx, LAYER_PACK, 0);
                let rgb = pack_base * cell_light_factor(tint, block, cx, cz);
                for _ in 0..tufts {
                    let variant = (splitmix64(&mut rng) % library.pack.len() as u64) as usize;
                    let model = &library.pack[variant];
                    stamp_tuft(block, cx, cz, model, rgb, &mut rng, &mut merged);
                }
            }
        }
    }

    merged
        .into_values()
        .filter(|b| !b.indices.is_empty())
        .map(|mut b| {
            if reverse_winding {
                reverse_winding_u32(&mut b.indices);
            }
            let (mut min, mut max) = (Vec3::MAX, Vec3::MIN);
            for p in &b.positions {
                min = min.min(Vec3::from(*p));
                max = max.max(Vec3::from(*p));
            }
            let mut mesh = Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::RENDER_WORLD,
            );
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, b.positions);
            mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, b.normals);
            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, b.uvs);
            // Float32x4 — bevy's ATTRIBUTE_COLOR only accepts its canonical
            // format (insert_attribute panics on Unorm8x4)
            mesh.insert_attribute(
                Mesh::ATTRIBUTE_COLOR,
                bevy::mesh::VertexAttributeValues::Float32x4(b.colors),
            );
            mesh.insert_indices(Indices::U32(b.indices));
            FoliageBuild {
                material: b.material.expect("merged buffer always has a material"),
                mesh,
                aabb: Aabb::from_min_max(min, max),
                tufts: b.tufts,
            }
        })
        .collect()
}

/// Stamp one tuft of `model` into the per-material merge buffers. Consumes the
/// rng in fixed order (jx, jz, yaw) — keep in sync with the determinism test.
/// `rgb` is the cell's linear environment tint; each part multiplies in its
/// own authored `color_factor` and lands as a flat Unorm8x4 vertex color
/// (alpha 255 — vertex color feeds the alpha mask, so it must stay opaque).
fn stamp_tuft(
    block: &TerrainBlock,
    cx: usize,
    cz: usize,
    model: &FoliageModel,
    rgb: Vec3,
    rng: &mut u64,
    merged: &mut HashMap<bevy::asset::AssetId<bevy::pbr::StandardMaterial>, MergeBuffers>,
) {
    let jx = unit_f32(rng) * CELL_SIZE;
    let jz = unit_f32(rng) * CELL_SIZE;
    let yaw = unit_f32(rng) * std::f32::consts::TAU;

    let y = cell_height(block, cx, cz, jx / CELL_SIZE, jz / CELL_SIZE);
    // authored underwater terrain gets no grass
    match block.water_type {
        WaterType::Water(_, level) | WaterType::Ice(level) if y < level => return,
        _ => {}
    }
    let offset = Vec3::new(cx as f32 * CELL_SIZE + jx, y, cz as f32 * CELL_SIZE + jz);
    let rot = Quat::from_rotation_y(yaw);

    for part in &model.parts {
        let buffers = merged.entry(part.material.id()).or_default();
        if buffers.material.is_none() {
            buffers.material = Some(part.material.clone());
        }
        let tinted = (rgb * part.color_factor).clamp(Vec3::ZERO, Vec3::ONE);
        // alpha stays 1.0: the vertex color participates in the alpha mask
        let color = [tinted.x, tinted.y, tinted.z, 1.0];
        let base = buffers.positions.len() as u32;
        for p in &part.positions {
            buffers.positions.push((rot * *p + offset).to_array());
        }
        for n in &part.normals {
            buffers.normals.push((rot * *n).to_array());
        }
        buffers.uvs.extend_from_slice(&part.uvs);
        buffers
            .colors
            .extend(std::iter::repeat_n(color, part.positions.len()));
        buffers
            .indices
            .extend(part.indices.iter().map(|i| i + base));
    }
    if let Some(part) = model.parts.first() {
        if let Some(buffers) = merged.get_mut(&part.material.id()) {
            buffers.tufts += 1;
        }
    }
}

/// Baked-light darkening for a cell from the region's 96×96 `tile_light`
/// grid (one byte per 20×20 cell): linear pull toward the byte value by
/// `light_strength`. Unverified original semantics — the strength knob and
/// visual calibration live in config (`graphics.foliage.tint.baked_light`).
fn cell_light_factor(tint: &TintContext, block: &TerrainBlock, cx: usize, cz: usize) -> f32 {
    let Some(grid) = tint.tile_light else {
        return 1.0;
    };
    let strength = tint.light_strength.clamp(0.0, 1.0);
    if strength <= 0.0 {
        return 1.0;
    }
    let gx = block.x.clamp(0, 5) as usize * CELLS_PER_SIDE + cx;
    let gz = block.z.clamp(0, 5) as usize * CELLS_PER_SIDE + cz;
    let Some(byte) = grid.get(gz * 96 + gx) else {
        return 1.0;
    };
    let v = *byte as f32 / 255.0;
    1.0 - strength * (1.0 - v)
}

/// Bilinear ground height inside cell (cx, cz) at fraction (fx, fz).
fn cell_height(block: &TerrainBlock, cx: usize, cz: usize, fx: f32, fz: f32) -> f32 {
    let h00 = block.vertices[cz * 17 + cx].height;
    let h10 = block.vertices[cz * 17 + cx + 1].height;
    let h01 = block.vertices[(cz + 1) * 17 + cx].height;
    let h11 = block.vertices[(cz + 1) * 17 + cx + 1].height;
    let h0 = h00 + (h10 - h00) * fx;
    let h1 = h01 + (h11 - h01) * fx;
    h0 + (h1 - h0) * fz
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::m::MapVertex;
    use crate::plugins::map::foliage::{FoliagePartGeometry, TileRecipe};
    use bevy::asset::Handle;
    use bevy::mesh::VertexAttributeValues;

    fn flat_block(tile_id: u16) -> TerrainBlock {
        let mut vertices = Vec::with_capacity(17 * 17);
        for z in 0..17 {
            for x in 0..17 {
                vertices.push(MapVertex {
                    x,
                    z,
                    height: 5.0,
                    texture_id: tile_id,
                    splat_scale: 0,
                    splat_offset: 0,
                    brightness: 0,
                });
            }
        }
        TerrainBlock {
            x: 2,
            z: 3,
            flag: 0,
            environment_id: 0,
            water_type: WaterType::None,
            vertices,
            tiles: Vec::new(),
            aabb: Default::default(),
        }
    }

    fn quad_model(material: Handle<bevy::pbr::StandardMaterial>) -> FoliageModel {
        FoliageModel {
            parts: vec![FoliagePartGeometry {
                positions: vec![
                    Vec3::new(-1.0, 0.0, 0.0),
                    Vec3::new(1.0, 0.0, 0.0),
                    Vec3::new(1.0, 2.0, 0.0),
                    Vec3::new(-1.0, 2.0, 0.0),
                ],
                normals: vec![Vec3::Z; 4],
                uvs: vec![[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]],
                indices: vec![0, 1, 2, 0, 2, 3],
                material,
                color_factor: Vec3::ONE,
            }],
        }
    }

    fn test_library() -> FoliageLibrary {
        let material = Handle::default();
        let mut library = FoliageLibrary::default();
        library
            .native
            .insert(757, NativeModelState::Ready(quad_model(material)));
        library.tile_recipes.insert(
            7,
            TileRecipe {
                pairs: vec![(757, 4)],
                pack_eligible: true,
            },
        );
        library
    }

    fn positions(build: &FoliageBuild) -> Vec<[f32; 3]> {
        match build.mesh.attribute(Mesh::ATTRIBUTE_POSITION).unwrap() {
            VertexAttributeValues::Float32x3(v) => v.clone(),
            _ => panic!("positions are Float32x3"),
        }
    }

    fn neutral_build(
        block: &TerrainBlock,
        region: u16,
        library: &FoliageLibrary,
        reverse_winding: bool,
    ) -> Vec<FoliageBuild> {
        build_block_foliage(
            block,
            region,
            library,
            true,
            1.0,
            false,
            0.0,
            reverse_winding,
            &TintContext::neutral(),
        )
    }

    #[test]
    fn scatter_is_deterministic() {
        let block = flat_block(7);
        let library = test_library();
        let a = neutral_build(&block, 0x1234, &library, true);
        let b = neutral_build(&block, 0x1234, &library, true);
        assert_eq!(a.len(), 1);
        assert_eq!(positions(&a[0]), positions(&b[0]));
    }

    #[test]
    fn scatter_count_matches_recipe() {
        let block = flat_block(7);
        let library = test_library();
        let builds = neutral_build(&block, 0x1234, &library, false);
        // per-block semantics: fully covered block, count 4 -> 4 tufts total
        assert_eq!(builds[0].tufts, 4);
        assert_eq!(positions(&builds[0]).len(), 4 * 4);
    }

    #[test]
    fn scatter_count_scales_with_coverage() {
        // half the cells carry the grass tile -> half the authored count
        let mut block = flat_block(7);
        for v in block.vertices.iter_mut() {
            if v.x >= 8 {
                v.texture_id = 0; // no recipe
            }
        }
        let library = test_library();
        let builds = neutral_build(&block, 0x1234, &library, false);
        assert_eq!(builds[0].tufts, 2);
        // and every tuft sits over a covered cell (x < 8 cells -> x < 160.0)
        assert!(positions(&builds[0]).iter().all(|p| p[0] < 8.0 * 20.0));
    }

    #[test]
    fn different_regions_scatter_differently() {
        let block = flat_block(7);
        let library = test_library();
        let a = neutral_build(&block, 0x1111, &library, false);
        let b = neutral_build(&block, 0x2222, &library, false);
        assert_ne!(positions(&a[0]), positions(&b[0]));
    }

    #[test]
    fn tufts_sit_on_the_ground() {
        let block = flat_block(7);
        let library = test_library();
        let builds = neutral_build(&block, 0, &library, false);
        // quad_model roots are at y=0 relative to the ground; flat height 5.0
        let ys: Vec<f32> = positions(&builds[0]).iter().map(|p| p[1]).collect();
        assert!(ys.iter().all(|y| (5.0..=7.0).contains(y)));
    }

    #[test]
    fn tile_tint_lands_in_vertex_colors() {
        let block = flat_block(7);
        let library = test_library();
        let mut tints = HashMap::new();
        tints.insert(7u16, Vec3::new(0.5, 0.25, 0.0));
        let tint = TintContext {
            tile_tints: &tints,
            blend_native: 1.0,
            blend_pack: 0.0,
            tile_light: None,
            light_strength: 0.0,
        };
        let builds = build_block_foliage(&block, 0, &library, true, 1.0, false, 0.0, false, &tint);
        let colors = match builds[0].mesh.attribute(Mesh::ATTRIBUTE_COLOR).unwrap() {
            VertexAttributeValues::Float32x4(v) => v.clone(),
            _ => panic!("colors are Float32x4"),
        };
        assert!(!colors.is_empty());
        assert!(colors.iter().all(|c| *c == [0.5, 0.25, 0.0, 1.0]));
    }

    #[test]
    fn tile_light_darkens_the_tint() {
        let block = flat_block(7);
        let library = test_library();
        let tints = HashMap::new();
        // whole-region grid at half brightness, full strength
        let grid = vec![127u8; 96 * 96];
        let tint = TintContext {
            tile_tints: &tints,
            blend_native: 0.0,
            blend_pack: 0.0,
            tile_light: Some(&grid),
            light_strength: 1.0,
        };
        let builds = build_block_foliage(&block, 0, &library, true, 1.0, false, 0.0, false, &tint);
        let colors = match builds[0].mesh.attribute(Mesh::ATTRIBUTE_COLOR).unwrap() {
            VertexAttributeValues::Float32x4(v) => v.clone(),
            _ => panic!("colors are Float32x4"),
        };
        let expected = 127.0 / 255.0;
        assert!(colors
            .iter()
            .all(|c| (c[0] - expected).abs() < 1e-5 && c[3] == 1.0));
    }
}
