// https://github.com/DummkopfOfHachtenduden/SilkroadDoc/wiki/JMXVNVM
use crate::assets::nvm::height_map::HeightMap;
use crate::assets::nvm::map_object::MapObjectList;
use crate::assets::nvm::nav_cell_quad::NavCellQuadList;
use crate::assets::nvm::nav_edge_global::NavEdgeGlobalList;
use crate::assets::nvm::nav_edge_internal::NavEdgeInternalList;
use crate::assets::nvm::plane_height_map::PlaneHeightMap;
use crate::assets::nvm::plane_type::PlaneType;
use crate::assets::nvm::plane_type_map::PlaneTypeMap;
use crate::assets::nvm::tile_map::TileMap;
use crate::util::buf_ext::BufExt;
use bevy::prelude::*;
use bevy::reflect::TypePath;
use std::io::Cursor;

mod edge_direction;
pub(crate) mod height_map;
mod link_edge;
pub mod loader;
mod map_object;
pub(crate) mod nav_cell_quad;
mod nav_edge_global;
mod nav_edge_internal;
pub(crate) mod nav_mesh_edge_flag;
mod nav_mesh_obj_inst;
mod plane_height_map;
mod plane_type;
mod plane_type_map;
mod tile_map;

#[derive(TypePath, Clone, Asset)]
#[allow(dead_code)]
pub struct JMXVNVM {
    pub object_list: MapObjectList,

    pub quad_cell_list: NavCellQuadList,

    pub global_edge_list: NavEdgeGlobalList,

    pub internal_edge_list: NavEdgeInternalList,

    pub tile_map: TileMap,
    pub height_map: HeightMap,
    pub plane_type_map: PlaneTypeMap,
    pub plane_height_map: PlaneHeightMap,
}

/// Size of the fixed trailer: tile map (96×96 × 8 B), height map (97×97 × 4 B),
/// plane type map and plane height map. Everything before it is
/// variable-length, so this is only checkable once the prefix is consumed.
const FIXED_TRAILER_LEN: u64 = 96 * 96 * 8 + 97 * 97 * 4 + 36 + 144;

impl TryFrom<&[u8]> for JMXVNVM {
    type Error = NvmParseError;

    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        let mut cursor = Cursor::new(bytes);
        // skip signature
        let signature = cursor.get_fixed_size_string(12);
        if signature != "JMXVNVM 1000" {
            return Err(NvmParseError::BadSignature(signature));
        }
        //assert_eq!(cursor.position(), 12);

        // ObjectList
        let object_list = MapObjectList::from(&mut cursor);

        //RTNavMeshCellQuad (Cells/Nodes)
        let quad_cell_list = NavCellQuadList::from(&mut cursor);

        //RTNavMeshEdgeGlobal (GlobalEdges, OutlineEdges)
        let global_edge_list = NavEdgeGlobalList::from(&mut cursor);

        //RTNavMeshEdgeInternal (InlineEdges)
        let internal_edge_list = NavEdgeInternalList::from(&mut cursor);

        // A legacy variant (101 files in Data.pk2, region ids 0x11a5-0x1fae)
        // carries 4-byte tile records and sometimes no plane maps, under the
        // *same* signature — so only the trailer length tells it apart. Its
        // short trailer used to run `HeightMap::from` off the end of the buffer
        // and abort the client. None of those regions is active in
        // `mapinfo.mfo` and terrain streaming is gated on that bitmask, so this
        // is a robustness guard rather than a live crash path.
        let remaining = bytes.len() as u64 - cursor.position();
        if remaining < FIXED_TRAILER_LEN {
            return Err(NvmParseError::UnsupportedTileLayout {
                remaining,
                expected: FIXED_TRAILER_LEN,
            });
        }

        //TileMap (96 * 96)
        let tile_map = TileMap::from(&mut cursor);

        //HeightMap (97 * 97)
        let height_map = HeightMap::from(&mut cursor);

        //PlaneTypeMap
        let plane_type_map = PlaneTypeMap::from(&mut cursor);

        //PlaneHeightMap
        let plane_height_map = PlaneHeightMap::from(&mut cursor);

        //EOF

        Ok(Self {
            object_list,
            quad_cell_list,
            global_edge_list,
            internal_edge_list,
            tile_map,
            height_map,
            plane_type_map,
            plane_height_map,
        })
    }
}

/// Why a `.nvm` could not be parsed. Returning these instead of aborting lets
/// Bevy's asset pipeline log and skip the file.
#[derive(thiserror::Error, Debug)]
pub enum NvmParseError {
    #[error("not a JMXVNVM 1000 navmesh (signature {0:?})")]
    BadSignature(String),
    #[error(
        "unsupported legacy tile layout: {remaining} trailing bytes, expected at least {expected}"
    )]
    UnsupportedTileLayout { remaining: u64, expected: u64 },
}

/// Plane (water/ice) map grid: 6x6 cells over a [`REGION_SIZE`]-wide region, so
/// one cell spans [`PLANE_CELL_SIZE`] and aligns with the region's 6x6 `.m` map
/// blocks. Stored x-fast, z-slow, matching the height map's convention.
const PLANE_CELLS_PER_SIDE: usize = 6;
/// World-space side length of one plane cell (a sixth of the region).
const PLANE_CELL_SIZE: f32 = 1920.0 / PLANE_CELLS_PER_SIDE as f32;

impl JMXVNVM {
    /// Index into the 6x6 plane maps for a region-local (x, z) in [0, 1920].
    fn plane_index(x: f32, z: f32) -> usize {
        let last = PLANE_CELLS_PER_SIDE - 1;
        let cx = ((x / PLANE_CELL_SIZE) as usize).min(last);
        let cz = ((z / PLANE_CELL_SIZE) as usize).min(last);
        cz * PLANE_CELLS_PER_SIDE + cx
    }

    /// The height an actor stands at over region-local (x, z), in the region's
    /// own vertical frame (the caller adds the region origin's y).
    ///
    /// This is the terrain height, except over a frozen plane (`Ice` /
    /// `WaterIce`) where the ice sheet is the surface: there the height is
    /// raised to the ice plane wherever the terrain — the lake bed — lies below
    /// it, and left as the terrain where land pokes up through the ice (a
    /// shore). Plain `Water` is *not* lifted: it is unwalkable, not a surface.
    ///
    /// Without this, an actor over a frozen lake walks the lake bed *underneath*
    /// the rendered ice instead of on top of it (the Karakoram bug).
    pub fn walkable_height_at(&self, x: f32, z: f32) -> f32 {
        let terrain = self.height_map.height_at(x, z);
        let i = Self::plane_index(x, z);
        // `.get`, not indexing: a real `.nvm` always carries the full 6x6 maps,
        // but a mesh built without them (a test fixture, a truncated stream)
        // must fall back to the terrain rather than panic.
        match self.plane_type_map.0.get(i) {
            Some(plane) if plane.is_ice() => {
                terrain.max(self.plane_height_map.0.get(i).copied().unwrap_or(terrain))
            }
            _ => terrain,
        }
    }

    /// Whether region-local (x, z) is covered by an ice sheet. An ice cell is
    /// walkable regardless of the terrain cell openness underneath it — you can
    /// cross a frozen lake even where its bed is deep, closed water.
    pub fn is_ice_at(&self, x: f32, z: f32) -> bool {
        self.plane_type_map
            .0
            .get(Self::plane_index(x, z))
            .is_some_and(PlaneType::is_ice)
    }
}

#[allow(dead_code)]
pub struct NavMesh;

#[cfg(test)]
mod tests {
    use super::*;

    /// A region whose whole surface is one plane type at one plane height, over
    /// flat terrain — enough to exercise the ice lift without a real `.nvm`.
    fn nvm_with_plane(terrain: f32, plane: PlaneType, plane_height: f32) -> JMXVNVM {
        JMXVNVM {
            object_list: Default::default(),
            quad_cell_list: NavCellQuadList {
                items: Vec::new(),
                open_cell_count: 0,
            },
            global_edge_list: Default::default(),
            internal_edge_list: Default::default(),
            tile_map: Default::default(),
            height_map: HeightMap::flat(terrain),
            plane_type_map: PlaneTypeMap(vec![plane; PLANE_CELLS_PER_SIDE * PLANE_CELLS_PER_SIDE]),
            plane_height_map: PlaneHeightMap(vec![
                plane_height;
                PLANE_CELLS_PER_SIDE * PLANE_CELLS_PER_SIDE
            ]),
        }
    }

    #[test]
    fn ice_lifts_a_mover_off_the_lake_bed() {
        // Lake bed at 516 under an ice sheet at 800: stand on the ice.
        let nvm = nvm_with_plane(516.0, PlaneType::WaterIce, 800.0);
        assert_eq!(nvm.walkable_height_at(960.0, 960.0), 800.0);
        assert!(nvm.is_ice_at(960.0, 960.0));
    }

    #[test]
    fn land_poking_through_the_ice_stays_terrain() {
        // A shore at 910 above the 800 ice plane: walk on the land.
        let nvm = nvm_with_plane(910.0, PlaneType::Ice, 800.0);
        assert_eq!(nvm.walkable_height_at(960.0, 960.0), 910.0);
    }

    #[test]
    fn plain_water_is_not_a_surface() {
        // Water is not walkable ground; height stays the terrain (lake bed).
        let nvm = nvm_with_plane(516.0, PlaneType::Water, 800.0);
        assert_eq!(nvm.walkable_height_at(960.0, 960.0), 516.0);
        assert!(!nvm.is_ice_at(960.0, 960.0));
    }

    #[test]
    fn plane_index_covers_the_region_corners() {
        assert_eq!(JMXVNVM::plane_index(0.0, 0.0), 0);
        assert_eq!(JMXVNVM::plane_index(1919.0, 1919.0), 35);
        // Clamps a position exactly on / past the far border into the last cell.
        assert_eq!(JMXVNVM::plane_index(1920.0, 1920.0), 35);
    }

    /// Empty object/cell/edge prefix: the variable-length part of a navmesh
    /// with nothing in it, followed by `trailer_len` bytes of trailer.
    fn nvm_bytes(signature: &str, trailer_len: usize) -> Vec<u8> {
        let mut data = signature.as_bytes().to_vec();
        data.resize(12, 0);
        data.extend(0i16.to_le_bytes()); // object count
        data.extend(0i32.to_le_bytes()); // total cell count
        data.extend(0i32.to_le_bytes()); // open cell count
        data.extend(0i32.to_le_bytes()); // global edge count
        data.extend(0i32.to_le_bytes()); // internal edge count
        data.extend(std::iter::repeat_n(0u8, trailer_len));
        data
    }

    /// #272: the parser used to `panic!` here, which aborts the client rather
    /// than letting the asset pipeline skip one bad file.
    #[test]
    fn a_foreign_signature_is_an_error_not_a_panic() {
        let data = nvm_bytes("JMXVBMS 0110", FIXED_TRAILER_LEN as usize);

        let Err(err) = JMXVNVM::try_from(data.as_slice()) else {
            panic!("a foreign signature must not parse");
        };
        assert!(matches!(err, NvmParseError::BadSignature(_)), "{err:?}");
    }

    /// The 101 legacy-variant files in Data.pk2 carry the *same* signature and
    /// only differ in their trailer, so the length check is what catches them.
    /// Before it, the short trailer ran `HeightMap::from` off the end of the
    /// buffer and aborted the client.
    #[test]
    fn a_short_legacy_trailer_is_an_error_not_a_panic() {
        let data = nvm_bytes("JMXVNVM 1000", FIXED_TRAILER_LEN as usize - 180);

        let Err(err) = JMXVNVM::try_from(data.as_slice()) else {
            panic!("a short trailer must not parse");
        };
        assert!(
            matches!(err, NvmParseError::UnsupportedTileLayout { .. }),
            "{err:?}"
        );
    }

    /// ...and a full-length trailer still parses.
    #[test]
    fn a_full_trailer_still_parses() {
        let data = nvm_bytes("JMXVNVM 1000", FIXED_TRAILER_LEN as usize);

        assert!(JMXVNVM::try_from(data.as_slice()).is_ok());
    }
}
