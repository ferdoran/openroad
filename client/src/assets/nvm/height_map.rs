use bytes::Buf;
use std::io::Cursor;

const HEIGHT_MAP_VERTS_PER_SIDE: usize = 97;
const HEIGHT_MAP_SIZE: usize = HEIGHT_MAP_VERTS_PER_SIDE * HEIGHT_MAP_VERTS_PER_SIDE;
/// World-space distance between adjacent height samples (97 verts spanning a
/// 1920-unit region).
const HEIGHT_MAP_STEP: f32 = 20.0;

#[derive(Clone)]
pub struct HeightMap(Vec<f32>);

impl HeightMap {
    /// A region whose terrain sits at a single height everywhere. Test-only:
    /// real height maps come from the `.nvm` byte stream.
    #[cfg(test)]
    pub(crate) fn flat(height: f32) -> Self {
        Self(vec![height; HEIGHT_MAP_SIZE])
    }

    /// Bilinearly interpolated terrain height at a region-local (x, z) position
    /// in [0, 1920]. Positions outside the region are clamped to its border.
    pub fn height_at(&self, x: f32, z: f32) -> f32 {
        let max_cell = (HEIGHT_MAP_VERTS_PER_SIDE - 2) as f32;
        let fx = (x / HEIGHT_MAP_STEP).clamp(0.0, max_cell + 1.0);
        let fz = (z / HEIGHT_MAP_STEP).clamp(0.0, max_cell + 1.0);
        let x0 = (fx.floor() as usize).min(HEIGHT_MAP_VERTS_PER_SIDE - 2);
        let z0 = (fz.floor() as usize).min(HEIGHT_MAP_VERTS_PER_SIDE - 2);
        let tx = fx - x0 as f32;
        let tz = fz - z0 as f32;
        let h = |xi: usize, zi: usize| self.0[zi * HEIGHT_MAP_VERTS_PER_SIDE + xi];
        let h0 = h(x0, z0) * (1.0 - tx) + h(x0 + 1, z0) * tx;
        let h1 = h(x0, z0 + 1) * (1.0 - tx) + h(x0 + 1, z0 + 1) * tx;
        h0 * (1.0 - tz) + h1 * tz
    }
}

impl From<&mut Cursor<&[u8]>> for HeightMap {
    fn from(cursor: &mut Cursor<&[u8]>) -> Self {
        let mut height_map: Vec<f32> = Vec::with_capacity(HEIGHT_MAP_SIZE);
        for _ in 0..HEIGHT_MAP_SIZE {
            let height = cursor.get_f32_le();
            height_map.push(height);
        }
        Self(height_map)
    }
}
