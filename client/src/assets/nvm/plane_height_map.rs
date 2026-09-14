use bytes::Buf;
use std::io::Cursor;

pub const PLANE_HEIGHT_MAP_SIZE: usize = 6 * 6;

#[derive(Default, Clone)]
pub struct PlaneHeightMap(pub Vec<f32>);

impl From<&mut Cursor<&[u8]>> for PlaneHeightMap {
    fn from(cursor: &mut Cursor<&[u8]>) -> Self {
        let mut plane_height_map: Vec<f32> = Vec::with_capacity(PLANE_HEIGHT_MAP_SIZE);
        for _ in 0..PLANE_HEIGHT_MAP_SIZE {
            let plane_height = cursor.get_f32_le();
            plane_height_map.push(plane_height);
        }
        Self(plane_height_map)
    }
}
