use bevy::math::{Vec2, Vec3};
use bytes::Buf;

use crate::assets::nvm::nav_mesh_edge_flag::NavMeshEdgeFlag;
use crate::util::buf_ext::BufExt;

/// [`NavEdge::src_cell`]/[`NavEdge::dst_cell`] value meaning "no cell on this
/// side" — the edge borders the outside of the walkable area.
///
/// Documents the format; nothing consumes it since blocking stopped depending
/// on which side an edge's cell is on (see `plugins/nav/edges.rs`).
#[allow(dead_code)]
pub const NO_CELL: u16 = 0xFFFF;

/// `header.nav_flag` bits controlling which optional event bytes are present.
const NAV_FLAG_EDGE_EVENTS: u32 = 1 << 0;
const NAV_FLAG_CELL_EVENTS: u32 = 1 << 1;
const NAV_FLAG_EVENT_LIST: u32 = 1 << 2;

/// Object-level nav mesh from a `.bms` mesh file: the walkable triangle mesh of
/// a map object (bridge deck, stairs, building interior), in object-local space.
/// Instanced into world regions by the `.nvm` object list.
///
/// Layout: https://github.com/DummkopfOfHachtenduden/SilkroadDoc/wiki/JMXVBMS
pub struct BmsNavMesh {
    pub vertices: Vec<NavVertex>,
    /// Walkable triangles ("object ground").
    pub cells: Vec<NavCell>,
    /// Border edges of the walkable area (entrances, drop-offs, walls).
    pub outline_edges: Vec<NavEdge>,
    /// Edges between two cells inside the walkable area.
    pub inline_edges: Vec<NavEdge>,
    /// Event zone names referenced by cells/edges via their `event_zone` byte.
    pub events: Vec<String>,
    pub outline_lookup: OutlineLookupGrid,
    /// Object-local AABB of the nav vertices. Distinct from
    /// [`JMXVBMS::bounding_box`](super::mesh::JMXVBMS::bounding_box), which is
    /// the *visual* mesh's box read from the file header: a map object's nav
    /// mesh routinely extends well past its visual geometry (a bridge's deck
    /// reaches beyond the planks that are drawn), so a query that culled nav
    /// triangles against the visual box rejected reachable ground. Every nav
    /// query rejects against this box instead.
    pub bounds: (Vec3, Vec3),
}

pub struct NavVertex {
    pub position: Vec3,
    /// Index of the outline bisector at this vertex (used by the original
    /// client for sliding along outlines); meaning per SilkroadDoc.
    pub bisector_index: u8,
}

pub struct NavCell {
    /// Indices into [`BmsNavMesh::vertices`].
    pub vertices: [u16; 3],
    pub flag: u16,
    pub event_zone: Option<u8>,
}

pub struct NavEdge {
    /// Indices into [`BmsNavMesh::vertices`].
    pub src_vertex: u16,
    pub dst_vertex: u16,
    /// Indices into [`BmsNavMesh::cells`]; `0xFFFF` when there is no cell on
    /// that side (outline edges).
    pub src_cell: u16,
    pub dst_cell: u16,
    /// Same flag semantics as the terrain nav mesh edges.
    pub flag: NavMeshEdgeFlag,
    pub event_zone: Option<u8>,
}

/// Spatial index over [`BmsNavMesh::outline_edges`]: which outline edges cross
/// each grid cell, for fast point/segment queries in object-local XZ space.
pub struct OutlineLookupGrid {
    pub origin: Vec2,
    pub width: u32,
    pub height: u32,
    /// `width * height` cells, row-major; each holds outline edge indices.
    pub cells: Vec<Vec<u16>>,
}

impl BmsNavMesh {
    pub fn parse<T: Buf + BufExt>(cursor: &mut T, nav_flag: u32) -> Self {
        let vertex_count = cursor.get_u32_le() as usize;
        let mut vertices = Vec::with_capacity(vertex_count);
        for _ in 0..vertex_count {
            vertices.push(NavVertex {
                position: cursor.get_vec3(),
                bisector_index: cursor.get_u8(),
            });
        }

        let cell_count = cursor.get_u32_le() as usize;
        let mut cells = Vec::with_capacity(cell_count);
        for _ in 0..cell_count {
            cells.push(NavCell {
                vertices: [
                    cursor.get_u16_le(),
                    cursor.get_u16_le(),
                    cursor.get_u16_le(),
                ],
                flag: cursor.get_u16_le(),
                event_zone: (nav_flag & NAV_FLAG_CELL_EVENTS != 0).then(|| cursor.get_u8()),
            });
        }

        let outline_edges = parse_edges(cursor, nav_flag);
        let inline_edges = parse_edges(cursor, nav_flag);

        let mut events = Vec::new();
        if nav_flag & NAV_FLAG_EVENT_LIST != 0 {
            let event_count = cursor.get_u32_le() as usize;
            events.reserve(event_count);
            for _ in 0..event_count {
                events.push(cursor.get_double_len_string());
            }
        }

        let bounds = vertices.iter().fold(
            (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
            |(lo, hi), v| (lo.min(v.position), hi.max(v.position)),
        );

        let origin = cursor.get_vec2();
        let width = cursor.get_u32_le();
        let height = cursor.get_u32_le();
        let grid_cell_count = cursor.get_u32_le() as usize;
        let mut grid_cells = Vec::with_capacity(grid_cell_count);
        for _ in 0..grid_cell_count {
            let outline_count = cursor.get_u32_le() as usize;
            let mut outlines = Vec::with_capacity(outline_count);
            for _ in 0..outline_count {
                outlines.push(cursor.get_u16_le());
            }
            grid_cells.push(outlines);
        }

        Self {
            vertices,
            cells,
            outline_edges,
            inline_edges,
            events,
            outline_lookup: OutlineLookupGrid {
                origin,
                width,
                height,
                cells: grid_cells,
            },
            bounds,
        }
    }

    /// Object-local corner positions of a cell's triangle.
    pub fn cell_triangle(&self, cell: &NavCell) -> (Vec3, Vec3, Vec3) {
        (
            self.vertices[cell.vertices[0] as usize].position,
            self.vertices[cell.vertices[1] as usize].position,
            self.vertices[cell.vertices[2] as usize].position,
        )
    }
}

fn parse_edges<T: Buf + BufExt>(cursor: &mut T, nav_flag: u32) -> Vec<NavEdge> {
    let edge_count = cursor.get_u32_le() as usize;
    let mut edges = Vec::with_capacity(edge_count);
    for _ in 0..edge_count {
        edges.push(NavEdge {
            src_vertex: cursor.get_u16_le(),
            dst_vertex: cursor.get_u16_le(),
            src_cell: cursor.get_u16_le(),
            dst_cell: cursor.get_u16_le(),
            flag: NavMeshEdgeFlag(cursor.get_u8()),
            event_zone: (nav_flag & NAV_FLAG_EDGE_EVENTS != 0).then(|| cursor.get_u8()),
        });
    }
    edges
}
