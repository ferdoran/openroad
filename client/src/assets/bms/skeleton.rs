use bytes::Buf;

use crate::assets::bms::BmsVersion;
use crate::util::buf_ext::BufExt;

/// Bytes per bone influence: `u8` index + `u16` weight.
const INFLUENCE_LEN: usize = 3;

pub struct MeshBones {
    pub bones: Vec<String>,
    pub bone_data: Vec<BoneData>,
}

#[derive(Clone)]
pub struct BoneData {
    pub index1: u8,
    pub weight1: u16,
    pub index2: u8,
    pub weight2: u16,
}

impl MeshBones {
    /// Reads the skin section. `version` selects the per-vertex influence
    /// stride — see [`BmsVersion::influences`].
    ///
    /// Returns `None` when the section is absent (`bone_count == 0`) or the
    /// buffer is too short for what the counts claim; the caller treats both
    /// as "this mesh has no usable skinning" rather than failing the load.
    pub fn from<T: Buf>(buf: &mut T, vertex_count: usize, version: BmsVersion) -> Option<Self> {
        if buf.remaining() < 4 {
            return None;
        }
        let bone_count = buf.get_u32_le();
        if bone_count == 0 {
            return None;
        }

        let mut bones = Vec::new();
        for _ in 0..bone_count {
            // Each name is a u32 length prefix + that many bytes; a bogus
            // count would otherwise index past the chunk and panic.
            if buf.remaining() < 4 {
                return None;
            }
            let len = u32::from_le_bytes(buf.chunk()[..4].try_into().ok()?) as usize;
            if buf.remaining() < 4 + len {
                return None;
            }
            bones.push(buf.get_double_len_string());
        }

        let stride = INFLUENCE_LEN * version.influences();
        if buf.remaining() < vertex_count * stride {
            return None;
        }
        let bone_data = (0..vertex_count)
            .map(|_| {
                let data = BoneData {
                    index1: buf.get_u8(),
                    weight1: buf.get_u16_le(),
                    index2: buf.get_u8(),
                    weight2: buf.get_u16_le(),
                };
                // 0109's influences 3 and 4 are `(0xFF, 0)` padding in every
                // sampled vertex (2,965/2,965) — consumed for the stride, not
                // blended.
                buf.advance(stride - 2 * INFLUENCE_LEN);
                data
            })
            .collect();

        Some(Self { bones, bone_data })
    }
}
