use std::collections::HashMap;
use std::io::Cursor;

use bevy::asset::Asset;
use bevy::asset::RenderAssetUsages;
use bevy::camera::primitives::Aabb;
use bevy::log::warn_once;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::{Component, Mesh, Vec3};
use bevy::reflect::TypePath;
use bytes::Buf;

use crate::assets::read_str_and_jump;

pub mod block_mesh;
pub mod block_splat_material;
pub mod loader;
mod tile_residency;

#[allow(dead_code)]
const TERRAIN_NUM_VERTICES: usize = 97 * 97;
#[allow(dead_code)]
const MAX_TEXTURE_COUNT: u32 = 30;

#[derive(Clone, Component)]
pub struct TerrainBlock {
    pub x: i32,
    pub z: i32,
    pub flag: i32,
    /// Environment profile id from the block header — see `environment.ifo` (JMXVENVI)
    /// and `plugins/environment`.
    pub environment_id: u16,
    pub water_type: WaterType,
    pub vertices: Vec<MapVertex>,
    pub tiles: Vec<MapTile>,
    pub aabb: Aabb,
}

#[derive(Default, Copy, Clone)]
pub enum WaterType {
    #[default]
    None,
    Water(u8, f32),
    Ice(f32),
}

#[derive(Debug, Clone, Copy)]
pub struct MapVertex {
    pub x: i32,
    pub z: i32,
    pub height: f32,
    pub texture_id: u16,
    pub splat_scale: u8,
    pub splat_offset: u8,
    pub brightness: u8,
}

impl MapVertex {
    pub fn to_vec3(&self) -> Vec3 {
        Vec3::new(self.x as f32 * -20.0, self.height, self.z as f32 * 20.0)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct MapTile {
    pub min: u32,
    pub max: u32,
    /// The tile's raw `u16` flag word. Previously the two bytes were read as
    /// discarded `u8`s and `min`/`max` were synthesised in their place, so the
    /// "blocked manually" bit was unrecoverable. Bit meanings beyond that are
    /// UNKNOWN (docs/re/formats/terrain-block-family.md).
    pub flag: u16,
}

#[derive(TypePath, Clone, Asset)]
pub struct JMXVMAPM {
    pub format: String,
    pub version: String,
    // [char, 12]
    pub blocks: Vec<TerrainBlock>,
}

impl From<&[u8]> for JMXVMAPM {
    fn from(bytes: &[u8]) -> Self {
        // println!("Bytes {:?}", bytes.len());
        let mut cursor = Cursor::new(bytes);
        let format = read_str_and_jump(&mut cursor, 8);
        let version = read_str_and_jump(&mut cursor, 4);
        debug_assert_eq!(cursor.position(), 12);
        // println!("Format {:?} Version {:?}", format, version);
        let mut blocks = Vec::with_capacity(6 * 6); // 6*6
        for z_block in 0..6 {
            for x_block in 0..6 {
                // println!("Block {:?}x{:?}", x_block, z_block);
                // Header
                // LE like every other read in this family; `_ne` was correct
                // only because every supported target is little-endian.
                let flag = cursor.get_u32_le();
                let environment_id = cursor.get_u16_le();

                let mut map_vertices = Vec::with_capacity(17 * 17);
                // Per block. These used to live outside both loops, so the
                // extent accumulated across the whole file; the x/z halves
                // happened to come out identical for every block (each block's
                // local vertex grid spans the same 0..320) and the y halves
                // were overwritten below, so the result was right by
                // construction - but only by construction.
                let mut min_vertex = Vec3::ZERO;
                let mut max_vertex = Vec3::ZERO;

                if x_block == 0 && z_block == 0 {
                    debug_assert_eq!(18, cursor.position());
                }
                // println!("Before cells position {:?}", cursor.position());

                for z in 0..17 {
                    for x in 0..17 {
                        let height = cursor.get_f32_le();

                        let flags = cursor.get_u16_le();
                        let texture_id = flags & 0b0000_0011_1111_1111;
                        // let texture_id = 603;
                        let splat_scale = ((flags & 0b1111_1100_0000_0000) >> 10) as u8;
                        let splat_offset = ((flags & 0b0001_1100_0000_0000) >> 10) as u8;
                        let brightness = cursor.get_u8();
                        // Log, never abort: this used to be an assert_eq!, which
                        // panicked the loader on 5 shipped files (Map/0/{0,1,2}.m,
                        // Map/1/{0,1}.m) that carry a valid signature but a
                        // non-36-block body.
                        if splat_offset > 0 {
                            warn_once!("m: non-zero splat offset: {}", splat_offset);
                        }
                        // debug!("V[{}, {}] vertex texture {} splat_scale= {:?} brightness={}", x_block*17 + x, z_block*17+z, texture_id, splat_scale, brightness);

                        let map_vertex = MapVertex {
                            x,
                            z,
                            height,
                            texture_id,
                            brightness,
                            splat_scale,
                            splat_offset,
                        };

                        let v = Vec3::new((x * 20) as f32, height, (z * 20) as f32);
                        min_vertex = min_vertex.min(v);
                        max_vertex = max_vertex.max(v);
                        // println!("{:?}", map_vertex);
                        map_vertices.push(map_vertex);
                    }
                }
                if x_block == 0 && z_block == 0 {
                    debug_assert_eq!(18 + 7 * 17 * 17, cursor.position());
                }
                // println!("After cells position {:?}", cursor.position());
                let water_type = cursor.get_i8();
                let water_wave_type = cursor.get_u8();
                let water_height = cursor.get_f32_le();

                let water_type = match water_type {
                    0 => WaterType::Water(water_wave_type, water_height),
                    1 => WaterType::Ice(water_height),
                    -1 | _ => WaterType::None,
                };

                let mut map_tiles = Vec::with_capacity(16 * 16);
                for z in 0..16 {
                    for x in 0..16 {
                        // The two bytes are one u16 flag word, not a max/min
                        // pair. `min`/`max` are this tile's corner vertex
                        // indices into the 17x17 grid, derived from its
                        // position - they were never in the file.
                        let flag = cursor.get_u16_le();

                        let min: u32 = z * 16 + x;
                        let max: u32 = min + 17;

                        map_tiles.push(MapTile { min, max, flag });
                    }
                }

                let max_height = cursor.get_f32_le();
                let min_height = cursor.get_f32_le();

                min_vertex.y = min_height;
                max_vertex.y = max_height;

                cursor.set_position(cursor.position() + 20); // reserved

                let block = TerrainBlock {
                    x: x_block,
                    z: z_block,
                    flag: flag as i32,
                    environment_id,
                    water_type,
                    vertices: map_vertices,
                    tiles: map_tiles,
                    aabb: Aabb::from_min_max(min_vertex, max_vertex),
                };
                blocks.push(block);
            }
        }

        JMXVMAPM {
            format,
            version,
            blocks,
        }
    }
}

impl From<&JMXVMAPM> for Mesh {
    fn from(value: &JMXVMAPM) -> Self {
        let vertices_capacity = 6 * 6 * 17 * 17;
        let mut vertices: Vec<[f32; 3]> = Vec::with_capacity(vertices_capacity);
        let mut brightnesses: Vec<f32> = Vec::with_capacity(vertices_capacity);
        let mut uv: Vec<[f32; 2]> = Vec::with_capacity(vertices_capacity);
        let mut indices = Vec::with_capacity(16 * 16 * 36 * 6); // per triangle 3 indices
        let mut texture_indices = Vec::with_capacity(vertices_capacity);
        let mut splat_scales = Vec::with_capacity(vertices_capacity);

        let mut id: u32 = 0;
        let mut texture_id_map = HashMap::new();
        for block in &value.blocks {
            let bx = (block.x * 16) as f32;
            let bz = (block.z * 16) as f32;
            for v in &block.vertices {
                let y = if f32::is_nan(v.height) { 0.0 } else { v.height };
                vertices.push([((v.x as f32) + bx) * 20.0, y, ((v.z as f32) + bz) * 20.0]);
                brightnesses.push(v.brightness as f32 / 255.0);
                if !texture_id_map.contains_key(&v.texture_id) {
                    texture_id_map.insert(v.texture_id, id);
                    id += 1;
                }
                let tex_index = texture_id_map.get(&v.texture_id).expect("i failed");
                // Only bits 13-15 of the vertex flag field carry the code (all observed
                // codes are multiples of 8 — Map.pk2 census 2026-07-30), making it a
                // power-of-two exponent: scale = 0.25 * 2^(code/8). The old 24 → 0.125 /
                // 32 → 0.0625 guesses inverted the progression.
                // Playtest verdict 2026-08-10: the vertex "Scale" field does
                // not drive tiling — every code matches vanilla at a constant
                // 0.25 (one repeat per 80 world units); the field's meaning
                // is UNKNOWN (docs/formats/mapm-jmxvmapm.md).
                let splat_scale = 0.25;

                splat_scales.push(splat_scale);
                let max = 16.0 * splat_scale;
                let a = v.x as f32 / max;
                let b = v.z as f32 / max;

                uv.push([a, b]);
                texture_indices.push(*tex_index as i32);
            }

            let bi = (block.x + block.z * 6) as u32;
            let start_index = bi * 17 * 17;
            for z in 0..16 {
                for x in 0..16 {
                    let i0 = start_index + x + z * 17;
                    let i1 = i0 + 1;
                    let i2 = i0 + 17;
                    let i3 = i2 + 1;

                    indices.push(i1);
                    indices.push(i2);
                    indices.push(i3);

                    indices.push(i0);
                    indices.push(i2);
                    indices.push(i1);
                }
            }
        }

        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::RENDER_WORLD,
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0., 1., 0.]; vertices.len()]);
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, vertices);
        mesh.insert_indices(Indices::U32(indices));
        mesh
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Diagnostic (same pattern as `probe_bsr_mod_palette` in `bsr.rs`):
    /// histogram every vertex's 6-bit `splat_scale` code across all regions in
    /// Map.pk2, and print per-region cluster centroids in teleport form for
    /// the rarer codes. All five codes are now calibrated (the factor wraps
    /// mod 3 after code 16 — see `docs/formats/mapm-jmxvmapm.md`); this stays
    /// useful for finding in-game comparison spots. Run:
    /// cargo test -p client probe_splat_scale_census -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic; needs real assets/Map.pk2"]
    fn probe_splat_scale_census() {
        use bevy::asset::io::AssetReader;
        use futures_lite::AsyncReadExt;
        use std::collections::BTreeMap;

        let archive = bevy_pk2::prelude::Archive::configured(&PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../assets/Map.pk2"
        )));
        let mut histogram: BTreeMap<u8, u64> = BTreeMap::new();
        let mut samples: BTreeMap<u8, Vec<String>> = BTreeMap::new();
        // (code, region x, region z) -> (vertex count, Σ local x, Σ local z,
        // Σ height): where to LOOK in game for the vanilla-vs-openroad tiling
        // comparison of the unverified codes. The centroid + mean height are
        // printed in server-teleport form (region id, region-local x/y/z) so
        // a spot can be pasted straight into the dev teleport window.
        let mut by_region: BTreeMap<(u8, u32, u32), (u64, f64, f64, f64)> = BTreeMap::new();
        // tile id -> per-code vertex counts: tests whether the scale code is
        // a PER-TEXTURE property (each tile id always carries one code) or
        // genuinely per-vertex. If it's per-texture, applying it per-texel
        // to the dominant texture (as the shader does) is the wrong model —
        // each texture should tile at its own constant factor, which also
        // removes the hard tiling seams where the code changes.
        let mut code_by_tile: BTreeMap<u16, BTreeMap<u8, u64>> = BTreeMap::new();
        let mut regions = 0u32;
        let mut parse_failures = 0u32;
        for z in 0..=255u32 {
            for x in 0..=255u32 {
                let path = PathBuf::from(format!("{z}/{x}.m"));
                let Ok(mut reader) = bevy::tasks::block_on(archive.read(&path)) else {
                    continue;
                };
                let mut data = Vec::new();
                bevy::tasks::block_on(reader.read_to_end(&mut data)).expect("read");
                // the parser asserts on layout surprises (e.g. splat_offset
                // bits); one odd region must not kill the whole census
                let map = match std::panic::catch_unwind(|| JMXVMAPM::from(data.as_slice())) {
                    Ok(map) => map,
                    Err(_) => {
                        parse_failures += 1;
                        println!("parse failure: region {x}x{z}");
                        continue;
                    }
                };
                regions += 1;
                for block in &map.blocks {
                    for v in &block.vertices {
                        *histogram.entry(v.splat_scale).or_default() += 1;
                        *code_by_tile
                            .entry(v.texture_id)
                            .or_default()
                            .entry(v.splat_scale)
                            .or_default() += 1;
                        if !matches!(v.splat_scale, 0 | 8 | 16) {
                            let entry = by_region.entry((v.splat_scale, x, z)).or_default();
                            entry.0 += 1;
                            entry.1 += (block.x * 320 + v.x * 20) as f64;
                            entry.2 += (block.z * 320 + v.z * 20) as f64;
                            entry.3 += v.height as f64;
                            let s = samples.entry(v.splat_scale).or_default();
                            if s.len() < 5 {
                                s.push(format!(
                                    "region {x}x{z} block {},{} vertex {},{}",
                                    block.x, block.z, v.x, v.z
                                ));
                            }
                        }
                    }
                }
            }
        }
        println!("{regions} regions parsed, {parse_failures} parse failures");
        for (code, count) in &histogram {
            println!("splat_scale code {code:2}: {count} vertices");
        }
        for (code, locations) in &samples {
            println!("code {code} sample locations:");
            for l in locations {
                println!("  {l}");
            }
        }
        // Every region carrying an unverified code, biggest clusters first,
        // with the SRO in-game coordinate of the region center
        // (game x = (xSector - 135) * 192, game y = (zSector - 92) * 192) and
        // the cluster centroid in server-teleport form (region id +
        // region-local x/y/z — see dev/teleport.rs).
        // Per-texture / per-vertex verdict: how many tile ids carry exactly
        // one code, and what the exceptions look like.
        let single: usize = code_by_tile
            .values()
            .filter(|codes| codes.len() == 1)
            .count();
        let multi: Vec<_> = code_by_tile
            .iter()
            .filter(|(_, codes)| codes.len() > 1)
            .collect();
        println!(
            "tile-id <-> code correlation: {} tile ids total, {single} carry exactly one code, {} carry several",
            code_by_tile.len(),
            multi.len()
        );
        for (tile, codes) in multi.iter().take(30) {
            let total: u64 = codes.values().sum();
            let spread: Vec<String> = codes
                .iter()
                .map(|(code, count)| format!("{code}:{count}"))
                .collect();
            println!(
                "  tile {tile} ({total} vertices): codes {}",
                spread.join(" ")
            );
        }
        // which tiles the rare codes appear on
        for probe_code in [24u8, 32u8] {
            let tiles: Vec<String> = code_by_tile
                .iter()
                .filter_map(|(tile, codes)| {
                    codes
                        .get(&probe_code)
                        .map(|count| format!("{tile}:{count}"))
                })
                .collect();
            println!("code {probe_code} appears on tiles: {}", tiles.join(" "));
        }
        let mut regions_by_count: Vec<_> = by_region.into_iter().collect();
        regions_by_count.sort_by_key(|&(_, (count, ..))| std::cmp::Reverse(count));
        println!("regions carrying unverified codes (all, biggest first):");
        for ((code, x, z), (count, sum_x, sum_z, sum_h)) in &regions_by_count {
            let game_x = (*x as i32 - 135) * 192 + 96;
            let game_y = (*z as i32 - 92) * 192 + 96;
            let n = *count as f64;
            println!(
                "  code {code}: region {x}x{z} ({count} vertices) ~ in-game ({game_x}, {game_y}) \
                 teleport: region {} x {:.0} y {:.0} z {:.0}",
                z * 256 + x,
                sum_x / n,
                sum_h / n,
                sum_z / n,
            );
        }
    }

    /// Build a minimal but structurally valid `.m`: 36 blocks, each with a
    /// 17x17 vertex grid, water block, 16x16 tile grid, height pair and 20
    /// reserved bytes.
    fn synthetic_m(block_flag: u32, tile_flag: u16, heights: &[(f32, f32)]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(b"JMXVMAPM");
        b.extend_from_slice(b"1000");
        for i in 0..36usize {
            b.extend_from_slice(&block_flag.to_le_bytes());
            b.extend_from_slice(&7u16.to_le_bytes()); // environment id
            for _ in 0..(17 * 17) {
                b.extend_from_slice(&1.0f32.to_le_bytes()); // height
                b.extend_from_slice(&0u16.to_le_bytes()); // texture/splat flags
                b.push(0); // brightness
            }
            b.push(0xFF); // water type -1 => None
            b.push(0); // wave type
            b.extend_from_slice(&0.0f32.to_le_bytes()); // water height
            for _ in 0..(16 * 16) {
                b.extend_from_slice(&tile_flag.to_le_bytes());
            }
            let (min_h, max_h) = heights[i % heights.len()];
            b.extend_from_slice(&max_h.to_le_bytes());
            b.extend_from_slice(&min_h.to_le_bytes());
            b.extend_from_slice(&[0u8; 20]); // reserved
        }
        b
    }

    /// The tile word is a real `u16` flag, not a discarded max/min byte pair —
    /// the "blocked manually" bit used to be unrecoverable (#288).
    #[test]
    fn tile_flags_survive_parsing() {
        let bytes = synthetic_m(0, 0xBEEF, &[(0.0, 1.0)]);
        let map = JMXVMAPM::from(bytes.as_slice());
        assert_eq!(map.blocks.len(), 36);
        for block in &map.blocks {
            assert_eq!(block.tiles.len(), 16 * 16);
            assert!(block.tiles.iter().all(|t| t.flag == 0xBEEF));
        }
        // the derived corner indices are unchanged
        assert_eq!(map.blocks[0].tiles[0].min, 0);
        assert_eq!(map.blocks[0].tiles[0].max, 17);
    }

    /// `Block.Flag` was hardcoded to 0, so the corpus's 2 Culled blocks were
    /// invisible to the terrain plugin.
    #[test]
    fn block_flag_is_kept() {
        let map = JMXVMAPM::from(synthetic_m(1, 0, &[(0.0, 1.0)]).as_slice());
        assert!(map.blocks.iter().all(|b| b.flag == 1));
    }

    /// Each block's AABB must describe that block, not an extent accumulated
    /// over the file. Two distinct height ranges must produce two distinct
    /// boxes, with the same block-local x/z span.
    #[test]
    fn each_block_gets_its_own_aabb() {
        let map = JMXVMAPM::from(synthetic_m(0, 0, &[(-5.0, 5.0), (100.0, 200.0)]).as_slice());
        let first = map.blocks[0].aabb;
        let second = map.blocks[1].aabb;
        assert_eq!(first.min().y, -5.0);
        assert_eq!(first.max().y, 5.0);
        assert_eq!(second.min().y, 100.0);
        assert_eq!(second.max().y, 200.0);
        // block-local horizontal extent: 16 steps of 20 units
        assert_eq!(first.min().x, 0.0);
        assert_eq!(first.max().x, 320.0);
        assert_eq!(second.max().x, 320.0);
    }

    /// A non-zero splat offset used to `assert_eq!` and abort the loader on 5
    /// shipped files; it must only warn.
    #[test]
    fn a_non_zero_splat_offset_does_not_panic() {
        let mut bytes = synthetic_m(0, 0, &[(0.0, 1.0)]);
        // first vertex of the first block: set bits 10..12 of its flag word
        let flag_at = 12 + 4 + 2 + 4;
        bytes[flag_at..flag_at + 2].copy_from_slice(&0b0000_0100_0000_0000u16.to_le_bytes());
        let map = JMXVMAPM::from(bytes.as_slice());
        assert_eq!(map.blocks.len(), 36, "parses despite the offset");
        assert_eq!(map.blocks[0].vertices[0].splat_offset, 1);
    }
}
