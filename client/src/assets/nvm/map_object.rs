use crate::assets::nvm::link_edge::{LinkEdge, LinkEdges};
use crate::assets::nvm::nav_mesh_obj_inst::NavMeshObjInst;
use bevy::prelude::Vec3;
use bytes::Buf;
use std::io::Cursor;

#[derive(Clone)]
#[allow(dead_code)]
pub struct MapObject {
    asset_id: i32,
    local_position: Vec3,
    object_type: i16,
    local_yaw: f32,
    local_uid: i16,
    short_0: i16,
    is_big: bool,
    is_struct: bool,
    region_id: u16, // rid
}

#[derive(Default, Clone)]
#[allow(dead_code)]
pub struct MapObjectList(pub Vec<NavMeshObjInst>);

impl From<&mut Cursor<&[u8]>> for MapObjectList {
    fn from(cursor: &mut Cursor<&[u8]>) -> Self {
        let object_count = cursor.get_i16_le() as usize;
        let mut object_list: Vec<NavMeshObjInst> = Vec::with_capacity(object_count);
        for _ in 0..object_count {
            // MapObject
            let asset_id = cursor.get_i32_le(); // used to look up the asset (.bsr/.cpd) from MapObjectIndex (object.ifo)
            let x = cursor.get_f32_le();
            let y = cursor.get_f32_le();
            let z = cursor.get_f32_le();
            let local_position = Vec3::new(x, y, z);
            let object_type = cursor.get_i16_le(); // -1 = Static, 0 = SkinedNavMesh?
            let local_yaw = cursor.get_f32_le(); // Rotation around the Y axis (height)
            let local_uid = cursor.get_i16_le(); // UID within region
                                                 // WorldUID = (obj->RegionID << 16) | obj->LocalUID
            let short_0 = cursor.get_i16_le();

            // Two independent 0/1 bytes, not one nibble-masked i16. Read as a
            // single LE i16 both nibble tests hit the *low* byte — IsBig — so
            // `is_big` (high nibble of a 0/1 value) was false for all 1,328
            // flagged objects, `is_struct` (low nibble) silently received
            // IsBig, and the real IsStruct byte was never read at all.
            let is_big = cursor.get_u8() != 0;
            let is_struct = cursor.get_u8() != 0;

            let region_id = cursor.get_u16_le(); // RegionID this object belongs to (origin region)

            let object = MapObject {
                asset_id,
                local_position,
                object_type,
                local_yaw,
                local_uid,
                short_0,
                is_big,
                is_struct,
                region_id,
            };

            let link_edge_count = cursor.get_u16_le() as usize;

            let mut link_edges: Vec<LinkEdge> = Vec::with_capacity(link_edge_count);
            for _ in 0..link_edge_count {
                // Values of `-1` here are temporarily invalid. Picture 3 regions (left, center, right) with an object owned by them each. The left and right
                // object are connected to the center object. All 3 objects will be included in every regions object list because of the linkage regardless of
                // bounding box intersection with the region.
                let linked_obj_id = cursor.get_i16_le();
                let linked_obj_edge_id = cursor.get_i16_le();
                let edge_id = cursor.get_i16_le();

                let link_edge = LinkEdge {
                    linked_obj_edge_id,
                    linked_obj_id,
                    edge_id,
                };
                link_edges.push(link_edge);
            }

            // create NavMeshObjInst
            let nav_mesh_obj_inst = NavMeshObjInst {
                object,
                link_edges: LinkEdges(link_edges),
            };
            object_list.push(nav_mesh_obj_inst);
        }
        Self(object_list)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// #272: `IsBig` and `IsStruct` are two independent 0/1 bytes, exactly as
    /// the `.o`/`.o2` parsers already read the same pair. Reading them as one
    /// LE i16 and nibble-masking put both tests on the low byte, so `is_big`
    /// was always false and `is_struct` received `IsBig`.
    #[test]
    fn is_big_and_is_struct_are_separate_bytes() {
        let mut data: Vec<u8> = 1i16.to_le_bytes().to_vec(); // objectCount
        data.extend(7i32.to_le_bytes()); // assetId
        data.extend(0.0f32.to_le_bytes()); // x
        data.extend(0.0f32.to_le_bytes()); // y
        data.extend(0.0f32.to_le_bytes()); // z
        data.extend((-1i16).to_le_bytes()); // type = Static
        data.extend(0.0f32.to_le_bytes()); // yaw
        data.extend(3i16.to_le_bytes()); // localUID
        data.extend(0i16.to_le_bytes()); // short0
        data.push(1); // IsBig
        data.push(0); // IsStruct
        data.extend(0x1234u16.to_le_bytes()); // regionId
        data.extend(0u16.to_le_bytes()); // linkEdgeCount

        let mut cursor = Cursor::new(data.as_slice());
        let list = MapObjectList::from(&mut cursor);

        let object = &list.0[0].object;
        assert!(object.is_big, "IsBig=1 must survive");
        assert!(!object.is_struct, "IsStruct=0 must not pick up IsBig");
        assert_eq!(
            object.region_id, 0x1234,
            "the following field stays aligned"
        );
    }

    /// ...and the other way round, so neither byte can be standing in for the
    /// other.
    #[test]
    fn is_struct_is_read_from_its_own_byte() {
        let mut data: Vec<u8> = 1i16.to_le_bytes().to_vec();
        data.extend(7i32.to_le_bytes());
        data.extend([0u8; 12]); // x/y/z
        data.extend((-1i16).to_le_bytes());
        data.extend(0.0f32.to_le_bytes());
        data.extend(3i16.to_le_bytes());
        data.extend(0i16.to_le_bytes());
        data.push(0); // IsBig
        data.push(1); // IsStruct
        data.extend(0x1234u16.to_le_bytes());
        data.extend(0u16.to_le_bytes());

        let mut cursor = Cursor::new(data.as_slice());
        let list = MapObjectList::from(&mut cursor);

        let object = &list.0[0].object;
        assert!(!object.is_big);
        assert!(object.is_struct);
        assert_eq!(object.region_id, 0x1234);
    }
}
