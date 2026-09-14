use crate::assets::nvm::plane_type::PlaneType;
use bytes::Buf;
use std::io::Cursor;

pub const PLANE_TYPE_MAP_SIZE: usize = 6 * 6;

#[derive(Default, Clone)]
pub struct PlaneTypeMap(pub Vec<PlaneType>);

impl From<&mut Cursor<&[u8]>> for PlaneTypeMap {
    fn from(cursor: &mut Cursor<&[u8]>) -> Self {
        let mut plane_type_map: Vec<PlaneType> = Vec::with_capacity(PLANE_TYPE_MAP_SIZE);
        for _ in 0..PLANE_TYPE_MAP_SIZE {
            let plane_type = PlaneType::from(cursor.get_u8());
            plane_type_map.push(plane_type);
        }
        Self(plane_type_map)
    }
}
