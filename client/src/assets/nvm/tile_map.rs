use bytes::Buf;
use std::io::Cursor;

const TILE_MAP_SIZE: usize = 96 * 96;

#[derive(Clone)]
#[allow(dead_code)]
pub struct Tile {
    pub(crate) cell_id: u32,
    pub(crate) flag: u16,
    texture_id: i16,
}

#[derive(Default, Clone)]
#[allow(dead_code)]
pub struct TileMap(pub Vec<Tile>);

impl TileMap {
    /// Whether the 20-unit tile covering a region-local position is flagged
    /// blocked. `None` if the position is outside the 96x96 grid.
    ///
    /// Independent of the quad cells, and known to agree with them exactly (see
    /// [`NavCellQuadList`](crate::assets::nvm::nav_cell_quad::NavCellQuadList)),
    /// which makes it a useful cross-check on the region-local coordinate
    /// mapping: the two can only disagree if the position handed in is wrong.
    pub fn blocked_at(&self, local: bevy::math::Vec2) -> Option<bool> {
        let (x, z) = ((local.x / 20.0) as i32, (local.y / 20.0) as i32);
        if !(0..96).contains(&x) || !(0..96).contains(&z) {
            return None;
        }
        self.0.get((z * 96 + x) as usize).map(|t| t.flag & 1 != 0)
    }
}

impl From<&mut Cursor<&[u8]>> for TileMap {
    fn from(cursor: &mut Cursor<&[u8]>) -> Self {
        let mut tile_map: Vec<Tile> = Vec::with_capacity(TILE_MAP_SIZE);
        for _ in 0..TILE_MAP_SIZE {
            let cell_id = cursor.get_u32_le(); // The `QuadCell` this `Tile` belongs to
            let flag = cursor.get_u16_le(); // 1 = Blocked, everything else is kinda unknown. Split into 2 bytes?
            let texture_id = cursor.get_i16_le(); // ID from TextureIndex (Tile2D.ifo) (replaced at runtime with SoundFlag for foot-step sounds)
            tile_map.push(Tile {
                cell_id,
                flag,
                texture_id,
            });
        }
        Self(tile_map)
    }
}
