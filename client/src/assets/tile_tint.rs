//! Derive terrain tints while DDJ pixels are owned by the loader, before Bevy
//! transfers them to its render upload queue. This small shared table outlives
//! foliage libraries and worlds; a revision invalidates baked grass on reload.
use crate::assets::ddj::decode_dxt1_rgba8;
use bevy::asset::AssetPath;
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

#[derive(Resource, Clone, Default)]
pub struct TerrainTileTints(pub Arc<RwLock<TileTintTable>>);

#[derive(Default)]
pub struct TileTintTable {
    pub revision: u64,
    pub entries: HashMap<AssetPath<'static>, TileTint>,
}

pub struct TileTint {
    pub color: Vec3,
    /// Source bytes handed to the render queue, not a claim of measured RSS savings.
    pub source_bytes: usize,
}

impl TerrainTileTints {
    pub fn derive(&self, path: AssetPath<'static>, image: &Image) {
        let color = average_color_linear(image).unwrap_or(Vec3::ONE);
        let source_bytes = image.data.as_ref().map_or(0, Vec::len);
        let mut table = self.0.write().unwrap();
        table.entries.insert(
            path,
            TileTint {
                color,
                source_bytes,
            },
        );
        table.revision += 1;
    }
}

/// Average color of a tile image in linear space, or `None` when the format
/// isn't one the tile pipeline produces.
fn average_color_linear(image: &Image) -> Option<Vec3> {
    let data = image.data.as_ref()?;
    let width = image.texture_descriptor.size.width;
    let height = image.texture_descriptor.size.height;
    match image.texture_descriptor.format {
        TextureFormat::Bc1RgbaUnormSrgb => {
            let mips = image.texture_descriptor.mip_level_count;
            // walk to the deepest mip still >= 4x4: one 8-byte BC1 block
            let mut offset = 0usize;
            let (mut w, mut h) = (width, height);
            for level in 0..mips {
                let lw = (width >> level).max(1);
                let lh = (height >> level).max(1);
                if lw < 4 || lh < 4 {
                    break;
                }
                (w, h) = (lw, lh);
                if lw == 4 || lh == 4 || level == mips - 1 {
                    break;
                }
                offset += (lw.div_ceil(4) * lh.div_ceil(4) * 8) as usize;
            }
            let size = (w.div_ceil(4) * h.div_ceil(4) * 8) as usize;
            let block = data.get(offset..offset + size)?;
            Some(average_rgba8_srgb(&decode_dxt1_rgba8(block, w, h)))
        }
        TextureFormat::Rgba8UnormSrgb => {
            // hand-decoded (non-DXT) tiles: stride over mip 0
            let mip0 = data.get(..(width * height * 4) as usize)?;
            let strided: Vec<u8> = mip0
                .chunks_exact(4)
                .step_by(16)
                .flatten()
                .copied()
                .collect();
            Some(average_rgba8_srgb(&strided))
        }
        _ => None,
    }
}

/// Average sRGB RGBA8 texels in linear space, skipping fully transparent
/// texels (BC1 punch-through holes); falls back to all texels if everything
/// is transparent.
fn average_rgba8_srgb(rgba: &[u8]) -> Vec3 {
    let mut sum = Vec3::ZERO;
    let mut n = 0u32;
    for texel in rgba.chunks_exact(4) {
        if texel[3] == 0 {
            continue;
        }
        sum += srgb_to_linear(texel);
        n += 1;
    }
    if n == 0 {
        for texel in rgba.chunks_exact(4) {
            sum += srgb_to_linear(texel);
            n += 1;
        }
    }
    if n == 0 {
        Vec3::ONE
    } else {
        sum / n as f32
    }
}

fn srgb_to_linear(texel: &[u8]) -> Vec3 {
    let c = Color::srgb_u8(texel[0], texel[1], texel[2]).to_linear();
    Vec3::new(c.red, c.green, c.blue)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tint_outlives_the_pixels_transferred_to_the_render_queue() {
        use bevy::asset::RenderAssetUsages;
        use bevy::render::render_resource::{Extent3d, TextureDimension};
        use bevy::render::{render_asset::RenderAsset, texture::GpuImage};
        let mut image = Image::new_fill(
            Extent3d {
                width: 4,
                height: 4,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            &[64, 128, 192, 255],
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::RENDER_WORLD,
        );
        let tints = TerrainTileTints::default();
        let path = AssetPath::from("tile2d/synthetic.ddj");
        tints.derive(path.clone(), &image);
        let upload = GpuImage::take_gpu_data(&mut image, None).unwrap();
        assert!(image.data.is_none());
        assert_eq!(upload.data.as_ref().unwrap().len(), 64);
        let table = tints.0.read().unwrap();
        assert_eq!(table.entries[&path].source_bytes, 64);
        assert_eq!(table.entries[&path].color, srgb_to_linear(&[64, 128, 192]));
        drop(table);
        // A reload derives from new pixels, even though the old main-world
        // image no longer owns any. Resource clones see the same table.
        let shared = tints.clone();
        let mut replacement = upload;
        replacement.data.as_mut().unwrap().fill(255);
        shared.derive(path.clone(), &replacement);
        let table = tints.0.read().unwrap();
        assert_eq!(table.revision, 2);
        assert_eq!(table.entries[&path].color, Vec3::ONE);
    }

    #[test]
    fn average_skips_transparent_texels() {
        // two texels: opaque mid-gray + fully transparent white
        let rgba = [128, 128, 128, 255, 255, 255, 255, 0];
        let avg = average_rgba8_srgb(&rgba);
        let expected = srgb_to_linear(&[128, 128, 128]);
        assert!((avg - expected).length() < 1e-6);
    }

    #[test]
    fn all_transparent_falls_back_to_all() {
        let rgba = [10, 20, 30, 0, 10, 20, 30, 0];
        let avg = average_rgba8_srgb(&rgba);
        assert!((avg - srgb_to_linear(&[10, 20, 30])).length() < 1e-6);
    }
}
