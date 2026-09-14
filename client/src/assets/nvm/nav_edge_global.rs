use crate::assets::nvm::edge_direction::EdgeDirection;
use crate::assets::nvm::nav_mesh_edge_flag::NavMeshEdgeFlag;
use crate::util::buf_ext::BufExt;
use bevy::prelude::Vec2;
use bytes::Buf;
use std::io::Cursor;

#[derive(Clone)]
#[allow(dead_code)]
pub struct NavEdgeGlobal {
    pub(crate) line: (Vec2, Vec2),
    pub(crate) flag: NavMeshEdgeFlag,
    assoc_direction: (EdgeDirection, EdgeDirection),
    assoc_cell: (i16, i16),
    assoc_region: (i16, i16),
}

#[derive(Default, Clone)]
pub struct NavEdgeGlobalList(pub Vec<NavEdgeGlobal>);

impl From<&mut Cursor<&[u8]>> for NavEdgeGlobalList {
    fn from(cursor: &mut Cursor<&[u8]>) -> Self {
        let global_edge_count = cursor.get_i32_le() as usize;
        let mut global_edge_list: Vec<NavEdgeGlobal> = Vec::with_capacity(global_edge_count);
        for _ in 0..global_edge_count {
            // NavLine
            let min = cursor.get_vec2();
            let max = cursor.get_vec2();

            let line = (min, max);

            let flag = NavMeshEdgeFlag(cursor.get_u8()); // see EdgeFlag
            let assoc_direction_0 = cursor.get_i8(); // see EdgeDirection -> index for m_EdgeList
            let assoc_direction_1 = cursor.get_i8(); // see EdgeDirection -> index for m_EdgeList (-1 if Blocked)

            let assoc_cell_0 = cursor.get_i16_le(); // QuadCell index -> index for pCell.m_CellList
            let assoc_cell_1 = cursor.get_i16_le(); // QuadCell index -> index for pCell.m_CellList (-1 if Blocked)

            let assoc_region_0 = cursor.get_i16_le();
            let assoc_region_1 = cursor.get_i16_le();

            global_edge_list.push(NavEdgeGlobal {
                line,
                flag,
                assoc_direction: (
                    EdgeDirection::from(assoc_direction_0),
                    EdgeDirection::from(assoc_direction_1),
                ),
                assoc_cell: (assoc_cell_0, assoc_cell_1),
                assoc_region: (assoc_region_0, assoc_region_1),
            });
        }
        Self(global_edge_list)
    }
}
