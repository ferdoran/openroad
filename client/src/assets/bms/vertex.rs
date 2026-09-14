use std::path::PathBuf;

use bevy::math::Vec2;
use bevy::prelude::Vec3;
use bytes::Buf;

use crate::assets::bms::BmsLoaderError;
use crate::util::buf_ext::BufExt;

#[derive(Debug)]
pub struct Vertex {
    pub position: Vec3,
    pub normal: Vec3,
    pub uv_0: Vec2,
    pub uv_1: Option<Vec2>,
    pub morphing_data: Option<Vec<u8>>,
    pub float: f32,
    pub int0: u32,
    pub int1: u32,
}

/// Size of the per-vertex morph record carried when `VertexFlag & 0x800`.
///
/// The contents are still UNKNOWN — only the width is established. 36 is the
/// only value in 0..96 that leaves the vertex section ending exactly at
/// `SkinOffset` on all five corpus samples
/// (`Data/Prim/mesh/{cos,mob/event}/event_festival_*.bms`). The previous code
/// consumed 64 (a `copy_to_bytes(32)`, which already advances, plus a second
/// `advance(32)`), which over-ran the section and failed every one of them.
const MORPH_RECORD_LEN: usize = 36;

#[derive(Debug)]
pub struct VertexData {
    pub vertices: Vec<Vertex>,
    pub lightmap_path: Option<PathBuf>,
    pub new_vertex_data: Option<Vec<(Vec3, Vec3)>>,
}

impl VertexData {
    pub fn from<T: Buf + BufExt>(value: &mut T, vertex_flag: u32) -> Result<Self, BmsLoaderError> {
        if value.remaining() < 4 {
            return Err(BmsLoaderError::Truncated("vertex"));
        }
        let vertex_count = value.get_u32_le();
        // Record width is fixed once the flags are known, so a garbage count
        // is rejected before it can reach `Vec::with_capacity` — the corpus
        // has files claiming 1.6 billion vertices (~135 GiB), which aborts
        // the process rather than unwinding.
        let record_len = 12
            + 12
            + 8
            + if vertex_flag & 0x400 != 0 { 8 } else { 0 }
            + if vertex_flag & 0x800 != 0 {
                MORPH_RECORD_LEN
            } else {
                0
            }
            + 12;
        let needed = (vertex_count as usize).saturating_mul(record_len);
        if needed > value.remaining() {
            return Err(BmsLoaderError::ImplausibleCount(
                "vertex",
                vertex_count,
                value.remaining(),
            ));
        }
        let mut vertices = Vec::with_capacity(vertex_count as usize);
        for _ in 0..vertex_count {
            let position = value.get_vec3();
            let normal = value.get_vec3();
            let uv_0 = value.get_vec2();
            let uv_1 = if vertex_flag & 0x400 != 0 {
                Some(value.get_vec2())
            } else {
                None
            };
            let morphing_data = if vertex_flag & 0x800 != 0 {
                Some(value.copy_to_bytes(MORPH_RECORD_LEN).to_vec())
            } else {
                None
            };
            let float = value.get_f32_le();
            let int0 = value.get_u32_le();
            let int1 = value.get_u32_le();
            vertices.push(Vertex {
                position,
                normal,
                uv_0,
                uv_1,
                morphing_data,
                float,
                int0,
                int1,
            });
        }
        let lightmap_path = if vertex_flag & 0x400 != 0 {
            if value.remaining() < 4 {
                return Err(BmsLoaderError::Truncated("lightmap path"));
            }
            let len = u32::from_le_bytes(
                value.chunk()[..4]
                    .try_into()
                    .map_err(|_| BmsLoaderError::Truncated("lightmap path"))?,
            ) as usize;
            if value.remaining() < 4 + len {
                return Err(BmsLoaderError::Truncated("lightmap path"));
            }
            Some(value.get_path_buf_double_len())
        } else {
            None
        };

        let new_vertex_data = if vertex_flag & 0x1000 != 0 {
            if value.remaining() < 4 {
                return Err(BmsLoaderError::Truncated("new vertex data"));
            }
            let count = value.get_u32_le();
            if (count as usize).saturating_mul(24) > value.remaining() {
                return Err(BmsLoaderError::ImplausibleCount(
                    "new vertex",
                    count,
                    value.remaining(),
                ));
            }
            let mut data = Vec::with_capacity(count as usize);
            for _ in 0..count {
                data.push((value.get_vec3(), value.get_vec3()))
            }
            Some(data)
        } else {
            None
        };

        Ok(Self {
            vertices,
            lightmap_path,
            new_vertex_data,
        })
    }
}
