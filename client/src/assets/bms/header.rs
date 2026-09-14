use bytes::Buf;

use crate::assets::bms::BmsLoaderError;
use crate::util::buf_ext::BufExt;

/// The twelve `u32` offsets plus `sub_prime_count`/`vertex_flag`/
/// `unknown_uint_2` that precede the two length-prefixed strings.
const FIXED_FIELDS: usize = 15 * 4;

#[allow(dead_code)]
pub struct Header {
    pub vertex_offset: u32,
    pub skin_offset: u32,
    pub face_offset: u32,
    pub cloth_vertex_offset: u32,
    pub cloth_edge_offset: u32,
    pub bounding_box_offset: u32,
    pub occlusion_portals: u32,
    pub navmesh_offset: u32,
    pub skinned_navmesh_offset: u32,
    pub unknown_offset: u32,
    pub unknown_uint: u32,
    pub nav_flag: u32,
    pub sub_prime_count: u32,
    pub vertex_flag: u32,
    pub unknown_uint_2: u32,
    pub name: String,
    pub material: String,
    pub unknown_uint_3: u32,
}

/// Reads a length-prefixed (`u32`) string without trusting the prefix.
fn read_string<T: Buf + BufExt>(
    value: &mut T,
    what: &'static str,
) -> Result<String, BmsLoaderError> {
    if value.remaining() < 4 {
        return Err(BmsLoaderError::Truncated(what));
    }
    let len = u32::from_le_bytes(
        value.chunk()[..4]
            .try_into()
            .map_err(|_| BmsLoaderError::Truncated(what))?,
    ) as usize;
    if value.remaining() < 4 + len {
        return Err(BmsLoaderError::Truncated(what));
    }
    Ok(value.get_double_len_string())
}

impl Header {
    /// Reads the header, rejecting a file that ends inside it rather than
    /// indexing past the buffer (`get_fixed_size_string` slices `chunk()`
    /// directly, so a bogus length prefix used to panic here).
    pub fn read<T: Buf + BufExt>(value: &mut T) -> Result<Self, BmsLoaderError> {
        if value.remaining() < FIXED_FIELDS {
            return Err(BmsLoaderError::Truncated("header"));
        }
        Ok(Self {
            vertex_offset: value.get_u32_le(),
            skin_offset: value.get_u32_le(),
            face_offset: value.get_u32_le(),
            cloth_vertex_offset: value.get_u32_le(),
            cloth_edge_offset: value.get_u32_le(),
            bounding_box_offset: value.get_u32_le(),
            occlusion_portals: value.get_u32_le(),
            navmesh_offset: value.get_u32_le(),
            skinned_navmesh_offset: value.get_u32_le(),
            unknown_offset: value.get_u32_le(),
            unknown_uint: value.get_u32_le(),
            nav_flag: value.get_u32_le(),
            sub_prime_count: value.get_u32_le(),
            vertex_flag: value.get_u32_le(),
            unknown_uint_2: value.get_u32_le(),
            name: read_string(value, "header name")?,
            material: read_string(value, "header material")?,
            unknown_uint_3: {
                if value.remaining() < 4 {
                    return Err(BmsLoaderError::Truncated("header"));
                }
                value.get_u32_le()
            },
        })
    }
}
