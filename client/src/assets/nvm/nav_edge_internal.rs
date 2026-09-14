use crate::assets::nvm::nav_mesh_edge_flag::NavMeshEdgeFlag;
use crate::util::buf_ext::BufExt;
use bevy::prelude::Vec2;
use bytes::Buf;
use std::io::Cursor;

#[derive(Clone)]
#[allow(dead_code)]
pub struct NavEdgeInternal {
    pub(crate) line: (Vec2, Vec2),
    pub(crate) flag: NavMeshEdgeFlag,
    assoc_direction: (u8, u8),
    /// Quad cell indices either side of the edge, into
    /// [`NavCellQuadList::items`](crate::assets::nvm::nav_cell_quad::NavCellQuadList).
    ///
    /// Measured over v1.188: on every one of the 1,586,920 *blocked* internal
    /// edges `.0` is valid and `.1` is `-1` — a blocked edge separates a
    /// walkable cell from void, so there is no far side to stand on. Unblocked
    /// edges have both valid. Nothing reads this today; it is why blocking can
    /// safely ignore the flag's direction (see `plugins/nav/edges.rs`).
    #[allow(dead_code)]
    pub(crate) assoc_cell: (i16, i16),
}

#[derive(Default, Clone)]
pub struct NavEdgeInternalList(pub Vec<NavEdgeInternal>);

impl From<&mut Cursor<&[u8]>> for NavEdgeInternalList {
    fn from(cursor: &mut Cursor<&[u8]>) -> Self {
        let internal_edge_count = cursor.get_i32_le() as usize;
        let mut internal_edge_list: Vec<NavEdgeInternal> = Vec::with_capacity(internal_edge_count);
        for _ in 0..internal_edge_count {
            // NavLine
            let min = cursor.get_vec2();
            let max = cursor.get_vec2();

            let line = (min, max);

            let flag = NavMeshEdgeFlag(cursor.get_u8()); // see EdgeFlag
            let assoc_direction_0 = cursor.get_u8(); // see EdgeDirection -> index for m_EdgeList
            let assoc_direction_1 = cursor.get_u8(); // see EdgeDirection -> index for m_EdgeList (-1 if Blocked)

            let assoc_cell_0 = cursor.get_i16_le(); // QuadCell index -> index for pCell.m_CellList
            let assoc_cell_1 = cursor.get_i16_le(); // QuadCell index -> index for pCell.m_CellList (-1 if Blocked)

            internal_edge_list.push(NavEdgeInternal {
                line,
                flag,
                assoc_direction: (assoc_direction_0, assoc_direction_1),
                assoc_cell: (assoc_cell_0, assoc_cell_1),
            });
        }
        Self(internal_edge_list)
    }
}
