use bevy::asset::RenderAssetUsages;
use bevy::math::Vec3;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::Mesh;
use std::collections::HashMap;

use crate::assets::m::{TerrainBlock, JMXVMAPM};
use crate::util::mesh::reverse_winding_u32;

/// Canonical block winding: correct under a *non-mirrored* (positive-determinant)
/// placement, matching the convention raw `.bms` data uses. `BLOCK_INDICES` is
/// authored for the mirrored (negative-determinant) terrain placement, i.e. it is
/// already the reversed form — so the canonical winding is its per-triangle
/// reverse. The shared winding rule (`needs_winding_reversal`) then reverses this
/// again for mirrored placements, reproducing `BLOCK_INDICES` exactly.
pub fn canonical_block_indices() -> Vec<u32> {
    let mut indices = BLOCK_INDICES.to_vec();
    reverse_winding_u32(&mut indices);
    indices
}

pub const BLOCK_INDICES: [u32; 1536] = [
    17, 0, 18, 18, 0, 1, 18, 1, 19, 19, 1, 2, 19, 2, 20, 20, 2, 3, 20, 3, 21, 21, 3, 4, 21, 4, 22,
    22, 4, 5, 22, 5, 23, 23, 5, 6, 23, 6, 24, 24, 6, 7, 24, 7, 25, 25, 7, 8, 25, 8, 26, 26, 8, 9,
    26, 9, 27, 27, 9, 10, 27, 10, 28, 28, 10, 11, 28, 11, 29, 29, 11, 12, 29, 12, 30, 30, 12, 13,
    30, 13, 31, 31, 13, 14, 31, 14, 32, 32, 14, 15, 32, 15, 33, 33, 15, 16, 34, 17, 35, 35, 17, 18,
    35, 18, 36, 36, 18, 19, 36, 19, 37, 37, 19, 20, 37, 20, 38, 38, 20, 21, 38, 21, 39, 39, 21, 22,
    39, 22, 40, 40, 22, 23, 40, 23, 41, 41, 23, 24, 41, 24, 42, 42, 24, 25, 42, 25, 43, 43, 25, 26,
    43, 26, 44, 44, 26, 27, 44, 27, 45, 45, 27, 28, 45, 28, 46, 46, 28, 29, 46, 29, 47, 47, 29, 30,
    47, 30, 48, 48, 30, 31, 48, 31, 49, 49, 31, 32, 49, 32, 50, 50, 32, 33, 51, 34, 52, 52, 34, 35,
    52, 35, 53, 53, 35, 36, 53, 36, 54, 54, 36, 37, 54, 37, 55, 55, 37, 38, 55, 38, 56, 56, 38, 39,
    56, 39, 57, 57, 39, 40, 57, 40, 58, 58, 40, 41, 58, 41, 59, 59, 41, 42, 59, 42, 60, 60, 42, 43,
    60, 43, 61, 61, 43, 44, 61, 44, 62, 62, 44, 45, 62, 45, 63, 63, 45, 46, 63, 46, 64, 64, 46, 47,
    64, 47, 65, 65, 47, 48, 65, 48, 66, 66, 48, 49, 66, 49, 67, 67, 49, 50, 68, 51, 69, 69, 51, 52,
    69, 52, 70, 70, 52, 53, 70, 53, 71, 71, 53, 54, 71, 54, 72, 72, 54, 55, 72, 55, 73, 73, 55, 56,
    73, 56, 74, 74, 56, 57, 74, 57, 75, 75, 57, 58, 75, 58, 76, 76, 58, 59, 76, 59, 77, 77, 59, 60,
    77, 60, 78, 78, 60, 61, 78, 61, 79, 79, 61, 62, 79, 62, 80, 80, 62, 63, 80, 63, 81, 81, 63, 64,
    81, 64, 82, 82, 64, 65, 82, 65, 83, 83, 65, 66, 83, 66, 84, 84, 66, 67, 85, 68, 86, 86, 68, 69,
    86, 69, 87, 87, 69, 70, 87, 70, 88, 88, 70, 71, 88, 71, 89, 89, 71, 72, 89, 72, 90, 90, 72, 73,
    90, 73, 91, 91, 73, 74, 91, 74, 92, 92, 74, 75, 92, 75, 93, 93, 75, 76, 93, 76, 94, 94, 76, 77,
    94, 77, 95, 95, 77, 78, 95, 78, 96, 96, 78, 79, 96, 79, 97, 97, 79, 80, 97, 80, 98, 98, 80, 81,
    98, 81, 99, 99, 81, 82, 99, 82, 100, 100, 82, 83, 100, 83, 101, 101, 83, 84, 102, 85, 103, 103,
    85, 86, 103, 86, 104, 104, 86, 87, 104, 87, 105, 105, 87, 88, 105, 88, 106, 106, 88, 89, 106,
    89, 107, 107, 89, 90, 107, 90, 108, 108, 90, 91, 108, 91, 109, 109, 91, 92, 109, 92, 110, 110,
    92, 93, 110, 93, 111, 111, 93, 94, 111, 94, 112, 112, 94, 95, 112, 95, 113, 113, 95, 96, 113,
    96, 114, 114, 96, 97, 114, 97, 115, 115, 97, 98, 115, 98, 116, 116, 98, 99, 116, 99, 117, 117,
    99, 100, 117, 100, 118, 118, 100, 101, 119, 102, 120, 120, 102, 103, 120, 103, 121, 121, 103,
    104, 121, 104, 122, 122, 104, 105, 122, 105, 123, 123, 105, 106, 123, 106, 124, 124, 106, 107,
    124, 107, 125, 125, 107, 108, 125, 108, 126, 126, 108, 109, 126, 109, 127, 127, 109, 110, 127,
    110, 128, 128, 110, 111, 128, 111, 129, 129, 111, 112, 129, 112, 130, 130, 112, 113, 130, 113,
    131, 131, 113, 114, 131, 114, 132, 132, 114, 115, 132, 115, 133, 133, 115, 116, 133, 116, 134,
    134, 116, 117, 134, 117, 135, 135, 117, 118, 136, 119, 137, 137, 119, 120, 137, 120, 138, 138,
    120, 121, 138, 121, 139, 139, 121, 122, 139, 122, 140, 140, 122, 123, 140, 123, 141, 141, 123,
    124, 141, 124, 142, 142, 124, 125, 142, 125, 143, 143, 125, 126, 143, 126, 144, 144, 126, 127,
    144, 127, 145, 145, 127, 128, 145, 128, 146, 146, 128, 129, 146, 129, 147, 147, 129, 130, 147,
    130, 148, 148, 130, 131, 148, 131, 149, 149, 131, 132, 149, 132, 150, 150, 132, 133, 150, 133,
    151, 151, 133, 134, 151, 134, 152, 152, 134, 135, 153, 136, 154, 154, 136, 137, 154, 137, 155,
    155, 137, 138, 155, 138, 156, 156, 138, 139, 156, 139, 157, 157, 139, 140, 157, 140, 158, 158,
    140, 141, 158, 141, 159, 159, 141, 142, 159, 142, 160, 160, 142, 143, 160, 143, 161, 161, 143,
    144, 161, 144, 162, 162, 144, 145, 162, 145, 163, 163, 145, 146, 163, 146, 164, 164, 146, 147,
    164, 147, 165, 165, 147, 148, 165, 148, 166, 166, 148, 149, 166, 149, 167, 167, 149, 150, 167,
    150, 168, 168, 150, 151, 168, 151, 169, 169, 151, 152, 170, 153, 171, 171, 153, 154, 171, 154,
    172, 172, 154, 155, 172, 155, 173, 173, 155, 156, 173, 156, 174, 174, 156, 157, 174, 157, 175,
    175, 157, 158, 175, 158, 176, 176, 158, 159, 176, 159, 177, 177, 159, 160, 177, 160, 178, 178,
    160, 161, 178, 161, 179, 179, 161, 162, 179, 162, 180, 180, 162, 163, 180, 163, 181, 181, 163,
    164, 181, 164, 182, 182, 164, 165, 182, 165, 183, 183, 165, 166, 183, 166, 184, 184, 166, 167,
    184, 167, 185, 185, 167, 168, 185, 168, 186, 186, 168, 169, 187, 170, 188, 188, 170, 171, 188,
    171, 189, 189, 171, 172, 189, 172, 190, 190, 172, 173, 190, 173, 191, 191, 173, 174, 191, 174,
    192, 192, 174, 175, 192, 175, 193, 193, 175, 176, 193, 176, 194, 194, 176, 177, 194, 177, 195,
    195, 177, 178, 195, 178, 196, 196, 178, 179, 196, 179, 197, 197, 179, 180, 197, 180, 198, 198,
    180, 181, 198, 181, 199, 199, 181, 182, 199, 182, 200, 200, 182, 183, 200, 183, 201, 201, 183,
    184, 201, 184, 202, 202, 184, 185, 202, 185, 203, 203, 185, 186, 204, 187, 205, 205, 187, 188,
    205, 188, 206, 206, 188, 189, 206, 189, 207, 207, 189, 190, 207, 190, 208, 208, 190, 191, 208,
    191, 209, 209, 191, 192, 209, 192, 210, 210, 192, 193, 210, 193, 211, 211, 193, 194, 211, 194,
    212, 212, 194, 195, 212, 195, 213, 213, 195, 196, 213, 196, 214, 214, 196, 197, 214, 197, 215,
    215, 197, 198, 215, 198, 216, 216, 198, 199, 216, 199, 217, 217, 199, 200, 217, 200, 218, 218,
    200, 201, 218, 201, 219, 219, 201, 202, 219, 202, 220, 220, 202, 203, 221, 204, 222, 222, 204,
    205, 222, 205, 223, 223, 205, 206, 223, 206, 224, 224, 206, 207, 224, 207, 225, 225, 207, 208,
    225, 208, 226, 226, 208, 209, 226, 209, 227, 227, 209, 210, 227, 210, 228, 228, 210, 211, 228,
    211, 229, 229, 211, 212, 229, 212, 230, 230, 212, 213, 230, 213, 231, 231, 213, 214, 231, 214,
    232, 232, 214, 215, 232, 215, 233, 233, 215, 216, 233, 216, 234, 234, 216, 217, 234, 217, 235,
    235, 217, 218, 235, 218, 236, 236, 218, 219, 236, 219, 237, 237, 219, 220, 238, 221, 239, 239,
    221, 222, 239, 222, 240, 240, 222, 223, 240, 223, 241, 241, 223, 224, 241, 224, 242, 242, 224,
    225, 242, 225, 243, 243, 225, 226, 243, 226, 244, 244, 226, 227, 244, 227, 245, 245, 227, 228,
    245, 228, 246, 246, 228, 229, 246, 229, 247, 247, 229, 230, 247, 230, 248, 248, 230, 231, 248,
    231, 249, 249, 231, 232, 249, 232, 250, 250, 232, 233, 250, 233, 251, 251, 233, 234, 251, 234,
    252, 252, 234, 235, 252, 235, 253, 253, 235, 236, 253, 236, 254, 254, 236, 237, 255, 238, 256,
    256, 238, 239, 256, 239, 257, 257, 239, 240, 257, 240, 258, 258, 240, 241, 258, 241, 259, 259,
    241, 242, 259, 242, 260, 260, 242, 243, 260, 243, 261, 261, 243, 244, 261, 244, 262, 262, 244,
    245, 262, 245, 263, 263, 245, 246, 263, 246, 264, 264, 246, 247, 264, 247, 265, 265, 247, 248,
    265, 248, 266, 266, 248, 249, 266, 249, 267, 267, 249, 250, 267, 250, 268, 268, 250, 251, 268,
    251, 269, 269, 251, 252, 269, 252, 270, 270, 252, 253, 270, 253, 271, 271, 253, 254, 272, 255,
    273, 273, 255, 256, 273, 256, 274, 274, 256, 257, 274, 257, 275, 275, 257, 258, 275, 258, 276,
    276, 258, 259, 276, 259, 277, 277, 259, 260, 277, 260, 278, 278, 260, 261, 278, 261, 279, 279,
    261, 262, 279, 262, 280, 280, 262, 263, 280, 263, 281, 281, 263, 264, 281, 264, 282, 282, 264,
    265, 282, 265, 283, 283, 265, 266, 283, 266, 284, 284, 266, 267, 284, 267, 285, 285, 267, 268,
    285, 268, 286, 286, 268, 269, 286, 269, 287, 287, 269, 270, 287, 270, 288, 288, 270, 271,
];

impl JMXVMAPM {
    pub fn to_block_meshes(&self) -> Vec<Mesh> {
        self.blocks.iter().map(|block| Mesh::from(block)).collect()
    }
}

impl From<&TerrainBlock> for Mesh {
    fn from(block: &TerrainBlock) -> Self {
        let mut vertices = Vec::with_capacity(17 * 17);
        //let mut indices = Vec::with_capacity(16 * 16 * 6);
        let indices = BLOCK_INDICES.to_vec();
        let mut normals = Vec::with_capacity(17 * 17);
        let mut texture_ids = Vec::with_capacity(block.vertices.len());
        let mut texture_id_map = HashMap::with_capacity(17 * 17);
        let mut id: i32 = 0;

        for z in 0..17 {
            for x in 0..17 {
                let v = &block.vertices[z * 17 + x];
                let vx = x * 20;
                let vz = z * 20;
                let position = [vx as f32, v.height, vz as f32];
                vertices.push(position);
                if !texture_id_map.contains_key(&v.texture_id) {
                    texture_id_map.insert(v.texture_id, id);
                    id += 1;
                }
                let tex_index = texture_id_map.get(&v.texture_id).expect("i failed");
                texture_ids.push(*tex_index);

                let _size_factor = 1.0 / 1920.0;

                let current = Vec3::from(position);
                let mut normal = Vec3::ZERO;

                calc_normals(block, z, x, vx, vz, current, &mut normal);
                normals.push(normal.normalize().to_array());
            }
        }

        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::RENDER_WORLD,
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, vertices);
        mesh.insert_indices(Indices::U32(indices));
        // Note: this single-block path (currently unused) still computes normals from the
        // block alone; the live `merge_block_meshes` path uses `region_normal`, which sees
        // across block borders.

        // let normals = mesh.attribute_mut(Mesh::ATTRIBUTE_NORMAL)
        //     .unwrap()
        //     .as_float3()
        //     .unwrap();
        //
        // let normals = normals.iter().map(|n| {
        //     (-Vec3::new(n[0], n[1], n[2])).to_array()
        // }).collect::<Vec<_>>();
        // mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);

        mesh
    }
}

/// Height of a vertex on the region-wide 97x97 grid (6x6 blocks of 17x17 vertices, where
/// adjacent blocks share their edge vertices), or `None` outside the region. This is what
/// lets normals see across block borders: the vertex one step past a block's edge lives in
/// the neighboring block, but on the region grid it is just `gx ± 1`.
fn region_vertex_height(map: &JMXVMAPM, gx: i32, gz: i32) -> Option<f32> {
    if !(0..=96).contains(&gx) || !(0..=96).contains(&gz) {
        return None;
    }
    let (bx, x) = if gx == 96 {
        (5, 16)
    } else {
        (gx / 16, gx % 16)
    };
    let (bz, z) = if gz == 96 {
        (5, 16)
    } else {
        (gz / 16, gz % 16)
    };
    let block = &map.blocks[(bz * 6 + bx) as usize];
    Some(block.vertices[(z * 17 + x) as usize].height)
}

/// Vertex normal on the region grid: the same sum of the four surrounding triangle face
/// normals `calc_normals` uses (identical winding, so interior vertices are unchanged),
/// but with neighbor heights taken from the whole region instead of a single block — no
/// more normal seams at block borders. Vertices on the region's outer edge still lack
/// neighbors (they live in a different `JMXVMAPM`), so region borders keep their seam.
fn region_normal(map: &JMXVMAPM, gx: i32, gz: i32) -> Vec3 {
    let current = Vec3::new(
        0.0,
        region_vertex_height(map, gx, gz).expect("vertex inside region"),
        0.0,
    );
    let w = region_vertex_height(map, gx - 1, gz).map(|h| Vec3::new(-20.0, h, 0.0));
    let e = region_vertex_height(map, gx + 1, gz).map(|h| Vec3::new(20.0, h, 0.0));
    let n = region_vertex_height(map, gx, gz - 1).map(|h| Vec3::new(0.0, h, -20.0));
    let s = region_vertex_height(map, gx, gz + 1).map(|h| Vec3::new(0.0, h, 20.0));

    let mut normal = Vec3::ZERO;
    if let (Some(w), Some(n)) = (w, n) {
        normal += calc_surface_normal(current, n, w);
    }
    if let (Some(w), Some(s)) = (w, s) {
        normal += calc_surface_normal(current, w, s);
    }
    if let (Some(e), Some(n)) = (e, n) {
        normal += calc_surface_normal(current, e, n);
    }
    if let (Some(e), Some(s)) = (e, s) {
        normal += calc_surface_normal(current, s, e);
    }
    normal.normalize()
}

/// Merges a rectangular group of blocks into one mesh (one draw call instead of one per
/// block). `blocks` is `(block, dx, dz)` in row-major group order, where `(dx, dz)` is the
/// block's offset *within the group*, in world units (`(block.x - group_origin_x) * 320.0`,
/// same for z) — i.e. positions are tiled at multiples of 320 starting from `(0,0)` for the
/// group's origin block, exactly like a single block's own local mesh space today.
/// `map` must be the region the blocks belong to; it provides the cross-block neighbor
/// heights for `region_normal`.
///
/// The caller is expected to give the merge-group *entity* the same
/// `Transform{translation: (group_origin_x * -320.0, 0.0, group_origin_z * 320.0), scale:
/// (-1.0, 1.0, 1.0)}` shape that an individual block's entity uses today, so the group's
/// mirroring is handled by Bevy's normal model-matrix machinery — vertex positions and
/// normals here are computed exactly as `impl From<&TerrainBlock> for Mesh` does, just
/// tiled across multiple blocks instead of baking a per-block offset into positions.
///
/// `reverse_winding` should come from the shared rule applied to that placement transform
/// (`needs_winding_reversal`): the mesh is built in canonical winding and reversed when the
/// placement is mirrored, so a `scale.x = -1` group renders exactly as it did before.
pub fn merge_block_meshes(
    map: &JMXVMAPM,
    blocks: &[(&TerrainBlock, f32, f32)],
    reverse_winding: bool,
) -> Mesh {
    let mut vertices = Vec::with_capacity(blocks.len() * 17 * 17);
    let mut normals = Vec::with_capacity(blocks.len() * 17 * 17);
    let mut indices = Vec::with_capacity(blocks.len() * BLOCK_INDICES.len());
    let canonical = canonical_block_indices();

    for (block_index, (block, dx, dz)) in blocks.iter().enumerate() {
        for z in 0..17 {
            for x in 0..17 {
                let v = &block.vertices[z * 17 + x];
                let vx = x * 20;
                let vz = z * 20;
                vertices.push([dx + vx as f32, v.height, dz + vz as f32]);

                let gx = block.x * 16 + x as i32;
                let gz = block.z * 16 + z as i32;
                normals.push(region_normal(map, gx, gz).to_array());
            }
        }

        let base = (block_index * 17 * 17) as u32;
        indices.extend(canonical.iter().map(|i| i + base));
    }

    if reverse_winding {
        reverse_winding_u32(&mut indices);
    }

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, vertices);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

fn calc_normals(
    block: &TerrainBlock,
    z: usize,
    x: usize,
    vx: usize,
    vz: usize,
    current: Vec3,
    mut normal_result: &mut Vec3,
) {
    if x > 0 {
        let w = Vec3::new(
            (vx - 20) as f32,
            block.vertices[z * 17 + x - 1].height,
            vz as f32,
        );
        calc_vertical_normals(block, z, x, vx, vz, current, &mut normal_result, w, false);
    }

    if x < 16 {
        let e = Vec3::new(
            (vx + 20) as f32,
            block.vertices[z * 17 + x + 1].height,
            vz as f32,
        );
        calc_vertical_normals(block, z, x, vx, vz, current, &mut normal_result, e, true);
    }
}

fn calc_vertical_normals(
    block: &TerrainBlock,
    z: usize,
    x: usize,
    vx: usize,
    vz: usize,
    current: Vec3,
    normal_result: &mut Vec3,
    h: Vec3,
    is_east: bool,
) {
    if z > 0 {
        let n = Vec3::new(
            vx as f32,
            block.vertices[(z - 1) * 17 + x].height,
            (vz - 20) as f32,
        );
        if is_east {
            *normal_result += calc_surface_normal(current, h, n);
        } else {
            *normal_result += calc_surface_normal(current, n, h);
        }
    }
    if z < 16 {
        let s = Vec3::new(
            vx as f32,
            block.vertices[(z + 1) * 17 + x].height,
            (vz + 20) as f32,
        );
        if is_east {
            *normal_result += calc_surface_normal(current, s, h);
        } else {
            *normal_result += calc_surface_normal(current, h, s);
        }
    }
}

// use BLOCK_INDICES instead
#[allow(dead_code)]
pub fn create_indices() -> Vec<u32> {
    let mut indices = Vec::with_capacity(16 * 16 * 6);

    for z in 0..16 {
        for x in 0..16 {
            let i0: u32 = x + z * 17;
            let i1: u32 = i0 + 1;
            let i2: u32 = i0 + 17;
            let i3: u32 = i2 + 1;
            /*

               i0      i1


               i2      i3

            */
            indices.push(i2);
            indices.push(i0);
            indices.push(i3);

            indices.push(i3);
            indices.push(i0);
            indices.push(i1);
        }
    }

    indices
}

fn calc_surface_normal(t1: Vec3, t2: Vec3, t3: Vec3) -> Vec3 {
    let v1 = t2 - t1;
    let v2 = t3 - t1;

    v1.cross(v2).normalize()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::m::{MapVertex, WaterType};
    use bevy::camera::primitives::Aabb;

    /// Builds a full 6x6-block region whose vertex heights come from `height(gx, gz)`
    /// on the region-wide 97x97 grid, so adjacent blocks share edge heights exactly.
    fn test_region(height: impl Fn(i32, i32) -> f32) -> JMXVMAPM {
        let mut blocks = Vec::with_capacity(36);
        for bz in 0..6 {
            for bx in 0..6 {
                let mut vertices = Vec::with_capacity(17 * 17);
                for z in 0..17i32 {
                    for x in 0..17i32 {
                        vertices.push(MapVertex {
                            x,
                            z,
                            height: height(bx * 16 + x, bz * 16 + z),
                            texture_id: 0,
                            splat_scale: 0,
                            splat_offset: 0,
                            brightness: 0,
                        });
                    }
                }
                blocks.push(TerrainBlock {
                    x: bx,
                    z: bz,
                    flag: 0,
                    environment_id: 0,
                    water_type: WaterType::None,
                    vertices,
                    tiles: Vec::new(),
                    aabb: Aabb::from_min_max(Vec3::ZERO, Vec3::ONE),
                });
            }
        }
        JMXVMAPM {
            format: "JMXVMAPM".into(),
            version: "1000".into(),
            blocks,
        }
    }

    #[test]
    fn region_vertex_height_spans_blocks_and_shared_edges() {
        let map = test_region(|gx, gz| (gx * 1000 + gz) as f32);
        // Interior of the first block, a shared block edge (gx=16 lives in both block 0
        // and block 1), and the region's far corner.
        assert_eq!(region_vertex_height(&map, 3, 5), Some(3005.0));
        assert_eq!(region_vertex_height(&map, 16, 8), Some(16008.0));
        assert_eq!(region_vertex_height(&map, 96, 96), Some(96096.0));
        // Outside the region there is no data.
        assert_eq!(region_vertex_height(&map, -1, 0), None);
        assert_eq!(region_vertex_height(&map, 0, 97), None);
    }

    #[test]
    fn planar_slope_has_seamless_normals_across_block_borders() {
        // On a plane every triangle shares one normal, so every vertex normal — including
        // those on block borders, which previously only saw one block's half of the
        // neighborhood — must equal the plane normal.
        let (a, b) = (3.0, -7.0);
        let map = test_region(|gx, gz| a * gx as f32 + b * gz as f32);
        // Heights step by (a, b) per 20-unit grid step => normal ∝ (-a, 20, -b).
        let expected = Vec3::new(-a, 20.0, -b).normalize();
        for gz in 0..97 {
            for gx in 0..97 {
                let normal = region_normal(&map, gx, gz);
                assert!(
                    normal.abs_diff_eq(expected, 1e-5),
                    "normal at ({gx}, {gz}) is {normal:?}, expected {expected:?}"
                );
            }
        }
    }
}
