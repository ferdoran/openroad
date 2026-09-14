use crate::util::buf_ext::BufExt;
use bevy::math::Rect;
use bytes::Buf;
use std::io::Cursor;

#[derive(Clone)]
#[allow(dead_code)]
pub struct ObjectIndices(pub Vec<u16>);

#[derive(Clone)]
#[allow(dead_code)]
pub struct NavCellQuad {
    pub(crate) rectangle: Rect,
    pub(crate) object_indices: ObjectIndices,
}

/// The region's quad cells, stored **open-first**: the first
/// [`open_cell_count`](Self::open_cell_count) entries are the walkable ones and
/// everything after them is solid.
///
/// Verified across v1.188 (regions 167x97, 167x98, 166x97): every tile the
/// `TileMap` flags blocked has a `cell_id` at or past `open_cell_count`, and no
/// unflagged tile does — 0 blocked tiles in open cells and 0 free tiles in
/// closed cells, in all three.
///
/// This is the authoritative walkability signal for terrain. The blocked edge
/// lists bound these regions but cannot be relied on alone: wherever their
/// coverage is incomplete, a mover walks into solid ground.
#[derive(Clone)]
#[allow(dead_code)]
pub struct NavCellQuadList {
    pub items: Vec<NavCellQuad>,
    pub open_cell_count: i32,
}

impl NavCellQuadList {
    /// Index of the cell containing a region-local position.
    pub fn cell_at(&self, local: bevy::math::Vec2) -> Option<usize> {
        self.items
            .iter()
            .position(|cell| cell.rectangle.contains(local))
    }

    /// Whether a region-local position is inside walkable space.
    ///
    /// A position no cell covers counts as walkable: coverage is complete in
    /// practice (every tile belongs to a cell), so a miss means a boundary
    /// rounding case, and refusing to move on those would trap movers.
    pub fn is_walkable_at(&self, local: bevy::math::Vec2) -> bool {
        self.cell_at(local)
            .is_none_or(|index| index < self.open_cell_count.max(0) as usize)
    }
}

impl From<&mut Cursor<&[u8]>> for NavCellQuadList {
    fn from(cursor: &mut Cursor<&[u8]>) -> Self {
        //RTNavMeshCellQuad (Cells/Nodes)
        let total_cell_count = cursor.get_i32_le() as usize;
        let open_cell_count = cursor.get_i32_le();
        let mut quad_cell_list: Vec<NavCellQuad> = Vec::with_capacity(total_cell_count);
        for _ in 0..total_cell_count {
            let min = cursor.get_vec2();
            let max = cursor.get_vec2();

            let cell_obj_count = cursor.get_u8() as usize;

            let mut object_indices: Vec<u16> = Vec::with_capacity(cell_obj_count);
            for _ in 0..cell_obj_count {
                let obj_index = cursor.get_u16_le();
                object_indices.push(obj_index);
            }

            quad_cell_list.push(NavCellQuad {
                rectangle: Rect { min, max },
                object_indices: ObjectIndices(object_indices),
            });
        }
        Self {
            items: quad_cell_list,
            open_cell_count,
        }
    }
}
