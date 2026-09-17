use std::cmp::max;
use std::io::Cursor;

use bevy::asset::RenderAssetUsages;
use bevy::asset::{io::Reader, Asset, AssetLoader, LoadContext};
use bevy::color::Srgba;
use bevy::image::{CompressedImageFormats, ImageSampler, ImageType};
use bevy::log::warn;
use bevy::prelude::Image;
use bevy::reflect::TypePath;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use byteorder::{LittleEndian, ReadBytesExt};
use bytes::{Buf, Bytes};
use ddsfile::*;
use num_enum::TryFromPrimitive;
use std::default::Default;
use std::fmt::Debug;
use thiserror::Error;

use crate::util::buf_ext::BufExt;

// https://github.com/DummkopfOfHachtenduden/SilkroadDoc/wiki/JMXVDDJ
#[derive(TypePath, Asset)]
#[allow(dead_code)]
pub struct JMXVDDJ {
    signature: String,
    //[char; 12],
    texture_buffer_size: i32,
    texture_type: D3dResourceType,
    texture_buffer: Vec<u8>,
    is_terrain_texture: bool,
    dds: Dds,
}

/// The 12-byte container signature every real DDJ starts with.
const DDJ_SIGNATURE: &str = "JMXVDDJ 1000";
/// Header bytes before the embedded DDS: signature + size + resource type.
const DDJ_HEADER_LEN: usize = 20;

impl TryFrom<&mut Bytes> for JMXVDDJ {
    type Error = DDJLoaderErrors;

    /// Parse the container, rejecting anything that is not a DDJ instead of
    /// panicking on it. `Media/res_ui/nifenchantwnd.ddj` is the reason this is
    /// fallible: it carries a `.ddj` extension but is a 2DT/UI window
    /// definition (`48 00 00 00 "CNIFEnchantWnd"`, i.e. 0x48 = **72** entries),
    /// so both the resource-type conversion and the DDS parse used to abort
    /// the process.
    ///
    /// It must stay skipped even once a 2DT UI loader exists, and not because
    /// of its extension: that file is the **pre-fix revision** of
    /// `res_ui/nifenchantwnd.2dt`. The two differ in 638 bytes across 11
    /// entries, all rects — most tellingly the Type-2 slot grid (ids 62-69),
    /// which is broken in the `.ddj` (573/621/669/717 and a stray 467) and
    /// clean in the `.2dt` (509/557/605/653 on both rows, pitch 48). Loading
    /// it would resurrect a bug the original's own authors had already
    /// corrected (#476).
    fn try_from(value: &mut Bytes) -> Result<Self, Self::Error> {
        if value.len() < DDJ_HEADER_LEN {
            return Err(DDJLoaderErrors::Truncated(value.len()));
        }
        let signature = value.get_fixed_size_string(12);
        if signature.trim_end_matches('\0') != DDJ_SIGNATURE {
            return Err(DDJLoaderErrors::NotADdj(signature));
        }
        let texture_buffer_size = value.get_i32_le();
        let raw_type = value.get_i32_le();
        let texture_type = D3dResourceType::try_from(raw_type)
            .map_err(|_| DDJLoaderErrors::UnknownResourceType(raw_type))?;
        let mut texture_buffer = &value[..];
        let texture_buffer_vec = texture_buffer.to_vec();

        // `texture_buffer_size` is deliberately not trusted: it is `+8`-inclusive
        // and wrong (too large by 0x100/0x10000/0x10100) in 358 of 37,551 corpus
        // files whose payloads are intact, so the rest of the buffer is used.
        let dds =
            Dds::read(&mut texture_buffer).map_err(|e| DDJLoaderErrors::Dds(format!("{e:?}")))?;

        Ok(JMXVDDJ {
            signature,
            texture_buffer_size,
            texture_type,
            texture_buffer: texture_buffer_vec,
            is_terrain_texture: false,
            dds,
        })
    }
}

impl JMXVDDJ {
    /// `srgb` = treat the pixel data as sRGB-encoded color (the default for
    /// albedo/diffuse textures). Pass `false` for non-color intensity maps
    /// (the sheen chrome probe, the +N enhancement streak): sampling those
    /// through an sRGB view decodes their stored mid-gray 128 to 0.216
    /// linear instead of the intended 0.5, halving the whole term (see
    /// `DdjSettings::non_color`).
    pub fn to_image(&self, srgb: bool) -> Option<Image> {
        //println!("Data size {:?}", self.dds.data.len());
        //println!("Pixels {:?}", self.dds.data.len() / 2);
        //println!("WxH {:?}", self.dds.header.width * self.dds.header.height);

        let usages = RenderAssetUsages::RENDER_WORLD;

        match self.dds.get_d3d_format() {
            None if self.is_palettized() => self.decoded_image(usages, srgb),
            None => None,
            Some(
                D3DFormat::A1R5G5B5
                | D3DFormat::R5G6B5
                | D3DFormat::X8R8G8B8
                | D3DFormat::A4R4G4B4
                | D3DFormat::X1R5G5B5,
            ) => self.decoded_image(usages, srgb),
            // Keep the historical cutout workaround until native BC1 sampling is
            // validated on the regression assets/backends. BC1 itself supports alpha;
            // this is a compatibility policy, not a format limitation.
            Some(D3DFormat::DXT1)
                if !self.is_terrain_texture
                    && dxt1_has_alpha(
                        &self.dds.data,
                        self.dds.header.width,
                        self.dds.header.height,
                    ) =>
            {
                self.decoded_image(usages, srgb)
            }
            Some(_f) => {
                // Everything else (the DXTn family) goes to the GPU compressed.
                // A format bevy cannot decode is logged and skipped — never a
                // panic: one bad file must not take the process down.
                Image::from_buffer(
                    &self.texture_buffer,
                    ImageType::Extension("dds"),
                    CompressedImageFormats::all(),
                    srgb,
                    ImageSampler::Default,
                    usages,
                )
                .map_err(|e| warn!("ddj: undecodable texture ({_f:?}): {e}"))
                .ok()
            }
        }
    }

    /// Decode each authored mip independently. Only single-level sources generate
    /// missing levels; partial authored chains keep their original level count.
    /// Validate the complete layout first so malformed data cannot silently become
    /// a different image or panic in the legacy pixel converters.
    fn decoded_image(&self, usages: RenderAssetUsages, srgb: bool) -> Option<Image> {
        let (width, height) = (self.dds.header.width, self.dds.header.height);
        if width == 0 || height == 0 {
            return None;
        }
        let levels = self.dds.get_num_mipmap_levels().max(1);
        if levels > 32 - width.max(height).leading_zeros() {
            return None;
        }
        let format = self.dds.get_d3d_format();
        let palette = self.is_palettized();
        let bpp = match format {
            Some(D3DFormat::X8R8G8B8) => 4usize,
            _ if palette => 1,
            _ => 2,
        };
        let bc1 = format == Some(D3DFormat::DXT1);
        // Uncompressed mip rows are tightly packed in these DDJ paths. Reject
        // padded mip-0 layouts instead of misreading their row padding as texels.
        if !bc1
            && self
                .dds
                .header
                .pitch
                .is_some_and(|pitch| pitch as usize != width as usize * bpp)
        {
            return None;
        }
        let mut offset = if palette { P8_PALETTE_BYTES } else { 0 };
        let mut decoded = Vec::new();
        for level in 0..levels {
            let (w, h) = ((width >> level).max(1), (height >> level).max(1));
            let len = if bc1 {
                (w as usize)
                    .div_ceil(4)
                    .checked_mul((h as usize).div_ceil(4))?
                    .checked_mul(8)?
            } else {
                (w as usize).checked_mul(h as usize)?.checked_mul(bpp)?
            };
            let end = offset.checked_add(len)?;
            let bytes = self.dds.data.get(offset..end)?;
            let rgba = match format {
                _ if palette => {
                    let mut plane = self.dds.data.get(..P8_PALETTE_BYTES)?.to_vec();
                    plane.extend_from_slice(bytes);
                    p8_to_rgba8(&plane, w, h)?
                }
                Some(D3DFormat::DXT1) => decode_dxt1_rgba8(bytes, w, h),
                Some(D3DFormat::A1R5G5B5) => a1r5g5b5_to_rgba8(bytes),
                Some(D3DFormat::R5G6B5) => r5g6b5_to_rgba8(bytes),
                Some(D3DFormat::X8R8G8B8) => x8r8g8b8_to_rgba8(bytes),
                Some(D3DFormat::A4R4G4B4) => a4r4g4b4_to_rgba8(bytes),
                Some(D3DFormat::X1R5G5B5) => x1r5g5b5_to_rgba8(bytes),
                _ => return None,
            };
            decoded.extend(rgba);
            offset = end;
        }
        if levels == 1 {
            Some(rgba8_to_image(width, height, &decoded, usages, srgb))
        } else {
            Some(rgba8_levels_to_image(
                width, height, levels, decoded, usages, srgb,
            ))
        }
    }

    /// Whether the embedded DDS declares an 8-bit palette. `ddsfile` maps
    /// `PALETTEINDEXED8` onto no `D3DFormat`, so this reads the raw pixel-format
    /// flags: `DDPF_PALETTEINDEXED8` (0x20) with 8 bits per pixel.
    fn is_palettized(&self) -> bool {
        const DDPF_PALETTEINDEXED8: u32 = 0x20;
        self.dds.header.spf.flags.bits() & DDPF_PALETTEINDEXED8 != 0
            && self.dds.header.spf.rgb_bit_count == Some(8)
    }
}

/// Decode a bare DDS byte buffer into an `Image`, reusing the same format handling as
/// `JMXVDDJ::to_image`. The terrain lightmap format (JMXVMAPT, `assets/t.rs`) embeds a DDS
/// payload with the identical `size`/`type`/DDS-bytes tail as JMXVDDJ, so it decodes through
/// here rather than duplicating the format match. `is_terrain_texture` is false: lightmaps are
/// GPU-only and are not foliage tint sources.
pub fn dds_buffer_to_image(dds_bytes: &[u8]) -> Option<Image> {
    let mut cursor = dds_bytes;
    let dds = Dds::read(&mut cursor).ok()?;
    JMXVDDJ {
        signature: String::new(),
        texture_buffer_size: dds_bytes.len() as i32,
        // Unused by `to_image` (only the decoded `dds` + raw bytes matter); a plain 2D texture.
        texture_type: D3dResourceType::Texture,
        texture_buffer: dds_bytes.to_vec(),
        is_terrain_texture: false,
        dds,
    }
    .to_image(true)
}

/// A minimal, deliberately synthetic 1x1 magenta `.ddj` file. Substituted by
/// `plugins::assets::fallback_reader::FallbackAssetReader` for a `.ddj` path
/// the archive doesn't have, so a missing texture becomes a visibly-a-
/// placeholder image instead of a `LoadState::Failed` that hangs
/// `bevy_asset_loader`'s `AssetCollection` gate forever (see ADR/PR notes on
/// the stuck-loading-screen bug). Magenta, not Bevy's own default white
/// handle, so a substituted asset is distinguishable on screen from an
/// unset `Handle<Image>`.
///
/// Built from the same container layout this module already documents
/// (`DDJ_SIGNATURE` + `DDJ_HEADER_LEN`) rather than a byte literal, so it
/// stays correct if that layout ever changes. Every step here is checked
/// against a fixed 1x1 `X8R8G8B8` shape, so the `expect`s cannot fail.
pub fn placeholder_ddj_bytes() -> Vec<u8> {
    let mut dds = Dds::new_d3d(NewD3dParams {
        height: 1,
        width: 1,
        depth: None,
        format: D3DFormat::X8R8G8B8,
        mipmap_levels: Some(1),
        caps2: None,
    })
    .expect("a 1x1 X8R8G8B8 DDS header is always constructible");
    // X8R8G8B8, little-endian: magenta (r=255, g=0, b=255).
    dds.data = vec![0xFF, 0x00, 0xFF, 0x00];
    let mut dds_bytes = Vec::new();
    dds.write(&mut dds_bytes)
        .expect("writing an in-memory DDS buffer cannot fail");

    let mut bytes = Vec::with_capacity(DDJ_HEADER_LEN + dds_bytes.len());
    bytes.extend_from_slice(DDJ_SIGNATURE.as_bytes());
    bytes.extend_from_slice(&(dds_bytes.len() as i32).to_le_bytes());
    bytes.extend_from_slice(&(D3dResourceType::Texture as i32).to_le_bytes());
    bytes.extend_from_slice(&dds_bytes);
    bytes
}

/// CPU decode of the uncompressed D3D formats SRO ships (A1R5G5B5, R5G6B5,
/// X8R8G8B8, A8R8G8B8) to `(width, height, RGBA8 mip 0)`. Returns `None`
/// for compressed (DXTn) and unknown formats — callers decode those with a
/// DDS-capable image library. Bevy-free counterpart of `JMXVDDJ::to_image`
/// for standalone tools (bsr2glb).
pub fn dds_to_rgba8(dds_bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let mut cursor = dds_bytes;
    let dds = Dds::read(&mut cursor).ok()?;
    let (width, height) = (dds.header.width, dds.header.height);
    // mip 0 only; tolerate short source data like `JMXVDDJ::to_image`
    let mip0 = |bpp: u32| {
        let len = ((width * height * bpp) as usize).min(dds.data.len());
        &dds.data[..len]
    };
    // Palettized DDS maps to no `D3DFormat`, so it is handled before the match.
    const DDPF_PALETTEINDEXED8: u32 = 0x20;
    if dds.header.spf.flags.bits() & DDPF_PALETTEINDEXED8 != 0
        && dds.header.spf.rgb_bit_count == Some(8)
    {
        return p8_to_rgba8(&dds.data, width, height).map(|rgba| (width, height, rgba));
    }
    let rgba = match dds.get_d3d_format()? {
        D3DFormat::A1R5G5B5 => a1r5g5b5_to_rgba8(mip0(2)),
        D3DFormat::R5G6B5 => r5g6b5_to_rgba8(mip0(2)),
        D3DFormat::X8R8G8B8 => x8r8g8b8_to_rgba8(mip0(4)),
        D3DFormat::A8R8G8B8 => a8r8g8b8_to_rgba8(mip0(4)),
        D3DFormat::A4R4G4B4 => a4r4g4b4_to_rgba8(mip0(2)),
        D3DFormat::X1R5G5B5 => x1r5g5b5_to_rgba8(mip0(2)),
        _ => return None,
    };
    Some((width, height, rgba))
}

#[allow(dead_code)]
fn dxt1_to_rgba8(dds: &Dds) -> Vec<u8> {
    let bytes: &[u8] = &dds.data;
    let _capacity = max(1, (dds.header.width + 3) / 4) * max(1, (dds.header.height + 3) / 4) * 8;
    // @todo: observe if value is different as 1048576?
    //let mut out: Vec<u8> = Vec::new();
    let mut out: Vec<u8> = Vec::with_capacity(1048576);
    let mut cursor = Cursor::new(bytes);
    (0..1).for_each(|level| {
        let size = dds.header.width / (1 << level);
        let level_data = read_mipmap(size, &mut cursor);
        out.extend_from_slice(&level_data);
    });
    // if let Some(_mipmaps) = dds.header.mip_map_count {
    //     (0..1).for_each(|level| {
    //         let size = dds.header.width / (1<<level);
    //         let level_data = read_mipmap(size, &mut cursor);
    //         out.extend_from_slice(&level_data);
    //     })
    // }
    // assert_eq!(out.len(), 1048576);
    out
}

#[allow(dead_code)]
fn read_mipmap<T: Buf>(size: u32, buffer: &mut T) -> Vec<u8> {
    let len = if size >= 4 { (size * size) / 2 } else { 8 };
    let mut out: Vec<u8> = Vec::with_capacity((size * size) as usize * 4);
    (0..out.capacity()).for_each(|_| {
        out.push(0);
    });
    let mut i = 0;

    while i < len as usize {
        let y_offset = i / (size as usize * 2);
        // i = 0 => 0
        // i = 8 => 4
        // i = 16 => 8
        // i = 24 => 12
        // i = 2048 => 0
        // i = 2056 => 4
        // rgba8 has 4 bytes/px, dxt1 has 8 bytes / 16px
        // => rgba 64 bytes/16px
        // 1 dxt1 block = 4 pixel = 16 bytes in rgba
        // 1 dxt1 row has 'width(in px) / 4' blocks
        // 1 dxt1 block = 8 bytes
        // 1
        let x_offset = i % (size as usize * 2);
        let c0 = buffer.get_u16_le();
        let c1 = buffer.get_u16_le();

        let (col_0, col_1, col_2, col_3) = if c0 > c1 {
            let col_0 = u16_to_r5g6b5_color(c0);
            let col_1 = u16_to_r5g6b5_color(c1);
            let col_2 = col_0 * (2.0 / 3.0) + col_1 * (1.0 / 3.0);
            let col_3 = col_0 * (1.0 / 3.0) + col_1 * (2.0 / 3.0);
            (col_0, col_1, col_2, col_3)
        } else {
            let col_0 = u16_to_r5g5b5a1_color(c0);
            let col_1 = u16_to_r5g5b5a1_color(c1);
            let col_2 = col_0 * 0.5 + col_1 * 0.5;
            let col_3 = Srgba::rgba_u8(0, 0, 0, 0);
            (col_0, col_1, col_2, col_3)
        };

        let one = buffer.get_u8();
        let two = buffer.get_u8();
        let three = buffer.get_u8();
        let four = buffer.get_u8();

        let flags = [
            u8_color_flags(one),
            u8_color_flags(two),
            u8_color_flags(three),
            u8_color_flags(four),
        ];

        for v in 0..4 {
            let f = flags[v];
            for w in 0..4 {
                let col = match f[w] {
                    0b00 => col_0,
                    0b01 => col_1,
                    0b10 => col_2,
                    0b11 => col_3,
                    _ => panic!("invalid interpolated color"),
                };

                if size > 3 {
                    let a = (y_offset * 4 + v) * (size as usize * 4) + (x_offset * 2) + w * 4 + 0;
                    let b = (y_offset * 4 + v) * (size as usize * 4) + (x_offset * 2) + w * 4 + 1;
                    let c = (y_offset * 4 + v) * (size as usize * 4) + (x_offset * 2) + w * 4 + 2;
                    let d = (y_offset * 4 + v) * (size as usize * 4) + (x_offset * 2) + w * 4 + 3;

                    out[a] = (col.red * 255.0) as u8;
                    out[b] = (col.green * 255.0) as u8;
                    out[c] = (col.blue * 255.0) as u8;
                    out[d] = 0xff;
                } else if v == 3 {
                    let (a, b, c, d) = if size == 2 {
                        let a = (y_offset * 4) * (size as usize * 4) + (x_offset * 2) + w * 4 + 0;
                        let b = (y_offset * 4) * (size as usize * 4) + (x_offset * 2) + w * 4 + 1;
                        let c = (y_offset * 4) * (size as usize * 4) + (x_offset * 2) + w * 4 + 2;
                        let d = (y_offset * 4) * (size as usize * 4) + (x_offset * 2) + w * 4 + 3;
                        (a, b, c, d)
                    } else {
                        let a = (y_offset * 4) * (size as usize * 4) + (x_offset * 2) + 0;
                        let b = (y_offset * 4) * (size as usize * 4) + (x_offset * 2) + 1;
                        let c = (y_offset * 4) * (size as usize * 4) + (x_offset * 2) + 2;
                        let d = (y_offset * 4) * (size as usize * 4) + (x_offset * 2) + 3;
                        (a, b, c, d)
                    };
                    out[a] = (col.red * 255.0) as u8;
                    out[b] = (col.green * 255.0) as u8;
                    out[c] = (col.blue * 255.0) as u8;
                    out[d] = 0xff;
                }
            }
        }

        i += 8;
    }
    out
}

#[allow(dead_code)]
fn u8_color_flags(n: u8) -> [u8; 4] {
    let mut flags = [0u8; 4];
    flags[0] = n & 0b11;
    flags[1] = (n >> 2) & 0b11;
    flags[2] = (n >> 4) & 0b11;
    flags[3] = (n >> 6) & 0b11;
    flags
}

#[allow(dead_code)]
fn u16_to_r5g6b5_color(n: u16) -> Srgba {
    // let r = (n & 0b_1111_1000_0000_0000) >> 11;
    // let g = (n & 0b_0000_0111_1110_0000) >> 5;
    // let b = (n & 0b_0000_0000_0001_1111);

    // let r = ((n & 0b11111_000000_00000) >> 8) | 0b00000111;
    // let g = ((n & 0b00000_111111_00000) >> 5) | 0b00000111;
    // let b = ((n & 0b00000_000000_11111) << 3) | 0b00000111;

    // let r = ((n & 0b11111_000000_00000) >> 8) | 0b00000111;
    // let g = ((n & 0b00000_111111_00000) >> 3) | 0b00000111;
    // let b = ((n & 0b00000_000000_11111) << 3) | 0b00000111;
    let mut r = (n & 0xf800) >> 8;
    r |= r >> 5;
    let mut g = (n & 0x7e0) >> 3;
    g |= g >> 6;
    let mut b = (n & 0x1f) << 3;
    b |= b >> 5;
    Srgba::rgba_u8((r) as u8, (g) as u8, (b) as u8, 0xff)
}

#[allow(dead_code)]
fn u16_to_r5g5b5a1_color(word: u16) -> Srgba {
    // let r = (n & 0b_1111_1000_0000_0000) >> 11;
    // let g = (n & 0b_0000_0111_1110_0000) >> 5;
    // let b = (n & 0b_0000_0000_0001_1111);

    // let r = ((n & 0b11111_000000_00000) >> 8) | 0b00000111;
    // let g = ((n & 0b00000_111111_00000) >> 5) | 0b00000111;
    // let b = ((n & 0b00000_000000_11111) << 3) | 0b00000111;

    // let r = ((n & 0b11111_000000_00000) >> 8) | 0b00000111;
    // let g = ((n & 0b00000_111111_00000) >> 3) | 0b00000111;
    // let b = ((n & 0b00000_000000_11111) << 3) | 0b00000111;
    let a: u16 = if word & 0b1_00000_00000_00000 > 0 {
        0xff
    } else {
        0x00
    };
    let r = ((word & 0b0_11111_00000_00000) >> 7) | 0b00000111;
    let g = ((word & 0b0_00000_11111_00000) >> 2) | 0b00000111;
    let b = ((word & 0b0_00000_00000_11111) << 3) | 0b00000111;
    // let mut r = ((n & 0xf800) >> 8);
    // r |= r >> 5;
    // let mut g = ((n & 0x7e0) >> 3);
    // g |= g >> 6;
    // let mut b = ((n & 0x1f) << 3);
    // b |= b >> 5;
    Srgba::rgba_u8((r) as u8, (g) as u8, (b) as u8, a as u8)
}

/// Whether a DXT1/BC1 mip-0 payload uses any punch-through (1-bit-alpha)
/// blocks — those with `color0 <= color1`, whose 4th palette index is a
/// transparent-black texel. Such textures need the alpha honored.
fn dxt1_has_alpha(data: &[u8], width: u32, height: u32) -> bool {
    let blocks = width.div_ceil(4) as usize * height.div_ceil(4) as usize;
    (0..blocks).any(|i| {
        data.get(i * 8..i * 8 + 4).is_some_and(|b| {
            let c0 = u16::from_le_bytes([b[0], b[1]]);
            let c1 = u16::from_le_bytes([b[2], b[3]]);
            c0 <= c1
        })
    })
}

/// CPU-decode a DXT1/BC1 mip-0 payload to RGBA8, honoring the 1-bit alpha:
/// punch-through blocks (`color0 <= color1`) make their 4th index fully
/// transparent. This preserves the historical cutout workaround (e.g. gold
/// coin drops) while native BC1 is revalidated on the affected assets and
/// backends; one-bit alpha is supported by the format itself.
pub fn decode_dxt1_rgba8(data: &[u8], width: u32, height: u32) -> Vec<u8> {
    let expand = |c: u16| -> [u8; 3] {
        let r = ((c >> 11) & 0x1f) as u32;
        let g = ((c >> 5) & 0x3f) as u32;
        let b = (c & 0x1f) as u32;
        [
            (r * 255 / 31) as u8,
            (g * 255 / 63) as u8,
            (b * 255 / 31) as u8,
        ]
    };
    let (w, h) = (width as usize, height as usize);
    let mut out = vec![0u8; w * h * 4];
    let bw = width.div_ceil(4) as usize;
    let bh = height.div_ceil(4) as usize;
    for by in 0..bh {
        for bx in 0..bw {
            let off = (by * bw + bx) * 8;
            let Some(block) = data.get(off..off + 8) else {
                continue;
            };
            let c0 = u16::from_le_bytes([block[0], block[1]]);
            let c1 = u16::from_le_bytes([block[2], block[3]]);
            let idx = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
            let (e0, e1) = (expand(c0), expand(c1));
            let lerp = |a: u8, b: u8, num: u32, den: u32| {
                ((a as u32 * (den - num) + b as u32 * num) / den) as u8
            };
            let punch = c0 <= c1;
            // palette[3] is transparent in punch-through mode
            let palette: [[u8; 4]; 4] = if punch {
                [
                    [e0[0], e0[1], e0[2], 255],
                    [e1[0], e1[1], e1[2], 255],
                    [
                        lerp(e0[0], e1[0], 1, 2),
                        lerp(e0[1], e1[1], 1, 2),
                        lerp(e0[2], e1[2], 1, 2),
                        255,
                    ],
                    [0, 0, 0, 0],
                ]
            } else {
                [
                    [e0[0], e0[1], e0[2], 255],
                    [e1[0], e1[1], e1[2], 255],
                    [
                        lerp(e0[0], e1[0], 1, 3),
                        lerp(e0[1], e1[1], 1, 3),
                        lerp(e0[2], e1[2], 1, 3),
                        255,
                    ],
                    [
                        lerp(e0[0], e1[0], 2, 3),
                        lerp(e0[1], e1[1], 2, 3),
                        lerp(e0[2], e1[2], 2, 3),
                        255,
                    ],
                ]
            };
            for t in 0..16 {
                let ci = ((idx >> (t * 2)) & 3) as usize;
                let (px, py) = (bx * 4 + t % 4, by * 4 + t / 4);
                if px < w && py < h {
                    let o = (py * w + px) * 4;
                    out[o..o + 4].copy_from_slice(&palette[ci]);
                }
            }
        }
    }
    out
}

fn rgba8_to_image(
    width: u32,
    height: u32,
    texture_data: &[u8],
    usages: RenderAssetUsages,
    srgb: bool,
) -> Image {
    if texture_data.is_empty() {
        warn!("ZERO LEN TEXTURE DATA");
    }
    // Build a full mip chain (the hand-decoded formats otherwise render
    // full-res at every distance). `Image::new`/`new_fill` assert mip0-sized
    // data, so the chain goes in via `new_uninit` + descriptor, the same way
    // Bevy's own DDS loader does it.
    let (mip_levels, data) = if srgb {
        crate::util::mips::rgba8_srgb_mip_chain(width, height, texture_data)
    } else {
        crate::util::mips::rgba8_mip_chain(width, height, texture_data)
    };
    rgba8_levels_to_image(width, height, mip_levels, data, usages, srgb)
}

fn rgba8_levels_to_image(
    width: u32,
    height: u32,
    mip_levels: u32,
    data: Vec<u8>,
    usages: RenderAssetUsages,
    srgb: bool,
) -> Image {
    let mut image = Image::new_uninit(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        if srgb {
            TextureFormat::Rgba8UnormSrgb
        } else {
            TextureFormat::Rgba8Unorm
        },
        usages,
    );
    image.texture_descriptor.mip_level_count = mip_levels;
    image.data = Some(data);
    image
}

#[allow(dead_code)]
fn rgba8_to_image_uncompressed(
    width: u32,
    height: u32,
    texture_data: &Vec<u8>,
    usages: RenderAssetUsages,
) -> Image {
    //println!("Buffer Size {:?}", texture_data.len());
    Image::new_fill(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &texture_data,
        TextureFormat::Rgba8Unorm,
        usages,
    )
}

#[allow(dead_code)]
pub fn a8r8g8b8_to_rgba8(bytes: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len() * 2);
    let mut i = 0;
    let mut rdr = Cursor::new(bytes);

    while i < bytes.len() {
        let word = rdr.read_u32::<LittleEndian>().expect("i failed");

        // => 1 Byte = 8 Bits => 1 Pixel = 16 Bits = 2 Bytes
        // r5g6b5 =>
        //   r      g     b
        // 00000 000|000 00000
        let a = (word & 0xff000000) >> 24;
        let r = (word & 0x00ff0000) >> 16;
        let g = (word & 0x0000ff00) >> 8;
        let b = word & 0x000000ff;

        out.push(r as u8);
        out.push(g as u8);
        out.push(b as u8);
        out.push(a as u8);

        i += 4;
    }
    out
}

pub fn r5g6b5_to_rgba8(bytes: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len() * 2);
    let mut i = 0;
    let mut rdr = Cursor::new(bytes);

    while i < bytes.len() {
        let word = rdr.read_u16::<LittleEndian>().expect("i failed");

        // => 1 Byte = 8 Bits => 1 Pixel = 16 Bits = 2 Bytes
        // r5g6b5 =>
        //   r      g     b
        // 00000 000|000 00000
        let r = ((word & 0b11111_000000_00000) >> 8) | 0b00000111;
        let g = ((word & 0b00000_111111_00000) >> 3) | 0b00000111;
        let b = ((word & 0b00000_000000_11111) << 3) | 0b00000111;

        out.push(r as u8);
        out.push(g as u8);
        out.push(b as u8);
        out.push(0xff as u8);

        i += 2;
    }
    out
}

/// `A4R4G4B4` -> RGBA8. Each 4-bit channel is expanded by replicating its
/// nibble (`v * 0x11`), which maps 0x0 -> 0 and 0xF -> 255 exactly. SRO ships
/// 6 of these; bevy cannot decode the format at all.
pub fn a4r4g4b4_to_rgba8(bytes: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len() * 2);
    for chunk in bytes.chunks_exact(2) {
        let word = u16::from_le_bytes([chunk[0], chunk[1]]);
        // a    r    g    b
        // 0000 0000 0000 0000
        let a = ((word >> 12) & 0xF) as u8;
        let r = ((word >> 8) & 0xF) as u8;
        let g = ((word >> 4) & 0xF) as u8;
        let b = (word & 0xF) as u8;
        out.push(r * 0x11);
        out.push(g * 0x11);
        out.push(b * 0x11);
        out.push(a * 0x11);
    }
    out
}

/// `X1R5G5B5` -> RGBA8: the same layout as `A1R5G5B5` but the top bit is
/// padding, not alpha, so every texel is opaque. SRO ships 2 of these; bevy
/// cannot decode the format at all.
pub fn x1r5g5b5_to_rgba8(bytes: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len() * 2);
    for chunk in bytes.chunks_exact(2) {
        let word = u16::from_le_bytes([chunk[0], chunk[1]]);
        let r = (((word >> 10) & 0x1F) as u8) << 3;
        let g = (((word >> 5) & 0x1F) as u8) << 3;
        let b = ((word & 0x1F) as u8) << 3;
        // replicate the top 3 bits into the low ones so 0x1F maps to 0xFF
        out.push(r | (r >> 5));
        out.push(g | (g >> 5));
        out.push(b | (b >> 5));
        out.push(0xFF);
    }
    out
}

/// Number of palette entries in a `PALETTEINDEXED8` DDS, each BGRA8.
const P8_PALETTE_ENTRIES: usize = 256;
const P8_PALETTE_BYTES: usize = P8_PALETTE_ENTRIES * 4;

/// `PALETTEINDEXED8` -> RGBA8. The 1024-byte palette (256 x BGRA8) sits
/// directly after the DDS header, ahead of the index plane; the corpus size
/// arithmetic (`1024 + sum(level w*h)`) confirms that layout exactly.
///
/// Returns `None` if the buffer cannot hold a palette plus one full index
/// plane, so a malformed file is skipped rather than read out of bounds.
pub fn p8_to_rgba8(data: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    let pixels = (width as usize).checked_mul(height as usize)?;
    if data.len() < P8_PALETTE_BYTES + pixels {
        warn!(
            "ddj: palettized texture too small ({} < {})",
            data.len(),
            P8_PALETTE_BYTES + pixels
        );
        return None;
    }
    let (palette, indices) = data.split_at(P8_PALETTE_BYTES);
    let mut out: Vec<u8> = Vec::with_capacity(pixels * 4);
    for &index in &indices[..pixels] {
        let entry = &palette[index as usize * 4..index as usize * 4 + 4];
        // stored BGRA, emitted RGBA
        out.push(entry[2]);
        out.push(entry[1]);
        out.push(entry[0]);
        out.push(entry[3]);
    }
    Some(out)
}

pub fn a1r5g5b5_to_rgba8(bytes: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len() * 2);
    let mut i = 0;
    let mut rdr = Cursor::new(bytes);

    while i < bytes.len() {
        let word = rdr.read_u16::<LittleEndian>().expect("i failed");

        // => 1 Byte = 8 Bits => 1 Pixel = 16 Bits = 2 Bytes
        // a1r5g5b5 =>
        // a   r      g     b
        // 0 00000 00|000 00000
        let a: u16 = if word & 0b1_00000_00000_00000 > 0 {
            0xff
        } else {
            0x00
        };
        let r = ((word & 0b0_11111_00000_00000) >> 7) | 0b00000111;
        let g = ((word & 0b0_00000_11111_00000) >> 2) | 0b00000111;
        let b = ((word & 0b0_00000_00000_11111) << 3) | 0b00000111;

        out.push(r as u8);
        out.push(g as u8);
        out.push(b as u8);
        out.push(a as u8);

        i += 2;
    }
    out
}

pub fn x8r8g8b8_to_rgba8(bytes: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len() * 2);
    let mut i = 0;
    let mut rdr = Cursor::new(bytes);

    while i < bytes.len() {
        let word = rdr.read_u32::<LittleEndian>().expect("i failed");

        // X8R8G8B8 	A 32-bit RGB pixel format in which 8 bits are reserved for each color.
        // see https://learn.microsoft.com/en-us/previous-versions/windows/desktop/bb153349(v=vs.85)
        let r = (word & 0x00ff0000) >> 16;
        let g = (word & 0x0000ff00) >> 8;
        let b = word & 0x000000ff;

        out.push(r as u8);
        out.push(g as u8);
        out.push(b as u8);
        out.push(0xff as u8);

        i += 4;
    }
    out
}

/*
fn decode_dds() {
    let header = DDS::parse_header(buf)?;

    let mut data_buf = Vec::new();
    buf.read_to_end(&mut data_buf)?;

    let layers = match header.compression {
        Compression::None => {
            DDS::decode_uncompressed(&header, &mut data_buf)
        }
        Compression::DXT1 | Compression::DXT2| Compression::DXT3 | Compression::DXT4 | Compression::DXT5 => {
            DDS::decode_dxt(&header, &mut data_buf)
        }
        _ => {
            let compression_type = String::from_utf8_lossy(&header.fourcc[..]);
            return Err(format!("The compression type `{}` is unsupported!", compression_type).into());
        }
    };

    Ok(DDS {
        header,
        layers,
    })
}*/

#[derive(Debug, Eq, PartialEq, TryFromPrimitive)]
#[repr(i32)]
// see https://learn.microsoft.com/en-us/windows/win32/direct3d9/d3dresourcetype
pub enum D3dResourceType {
    Surface = 1,
    Volume = 2,
    Texture = 3,
    VolumeTexture = 4,
    CubeTexture = 5,
    VertexBuffer = 6,
    IndexBuffer = 7,
    // D3DRTYPE_FORCE_DWORD = 0x7fffffff, not used?
}

#[derive(Error, Debug)]
pub enum DDJLoaderErrors {
    #[error("Problem opening file")]
    PROBLEM,
    #[error("{0}")]
    IO(std::io::Error),
    /// Shorter than the 20-byte container header.
    #[error("truncated DDJ: {0} bytes")]
    Truncated(usize),
    /// Wrong magic — a non-DDJ file carrying a `.ddj` extension.
    #[error("not a DDJ (signature {0:?})")]
    NotADdj(String),
    #[error("unknown D3D resource type {0}")]
    UnknownResourceType(i32),
    #[error("embedded DDS is unreadable: {0}")]
    Dds(String),
    /// A pixel format nothing in the pipeline can decode.
    #[error("unsupported pixel format: {0}")]
    UnsupportedFormat(String),
}

/// Per-load settings for `.ddj` textures.
///
/// `non_color`: load as linear (`Rgba8Unorm`/non-sRGB) instead of the sRGB
/// default. For intensity maps — the sheen chrome probe
/// (`spheremap_gray.ddj`) and the +N enhancement streak textures — whose
/// stored bytes ARE the reflection intensity: through an sRGB view their
/// mid-gray 128 decodes to 0.216 linear instead of 0.5, halving the term
/// (and inverting the sheen replace-mode math). Caveat: bevy keys loaded
/// assets per (path, settings-defaults) — the first load's settings win for
/// a given plain path — which is harmless here because these paths are only
/// ever loaded as intensity maps.
#[derive(serde::Serialize, serde::Deserialize, Default, Clone, Copy)]
pub struct DdjSettings {
    pub non_color: bool,
}

#[derive(bevy::reflect::TypePath)]
pub struct DDJLoader {
    tile_tints: crate::assets::tile_tint::TerrainTileTints,
}

impl bevy::prelude::FromWorld for DDJLoader {
    fn from_world(world: &mut bevy::prelude::World) -> Self {
        world.init_resource::<crate::assets::tile_tint::TerrainTileTints>();
        Self {
            tile_tints: world
                .resource::<crate::assets::tile_tint::TerrainTileTints>()
                .clone(),
        }
    }
}

impl AssetLoader for DDJLoader {
    type Asset = Image;
    type Settings = DdjSettings;
    type Error = DDJLoaderErrors;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        settings: &Self::Settings,
        load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut buf = Vec::new();
        reader
            .read_to_end(&mut buf)
            .await
            .map_err(|e| DDJLoaderErrors::IO(e))?;
        let bytes = &buf;
        let mut ddj_file = match JMXVDDJ::try_from(&mut Bytes::copy_from_slice(bytes)) {
            Ok(ddj) => ddj,
            Err(e) => {
                // Log and skip: a mis-named or malformed file (e.g.
                // res_ui/nifenchantwnd.ddj, which is a UI window definition)
                // must not take the process down.
                warn!("ddj: {:?}: {e}", load_context.path());
                return Err(e);
            }
        };
        ddj_file.is_terrain_texture = load_context
            .path()
            .path()
            .to_str()
            .map_or(false, |s| s.contains("tile2d"));
        // One line per *distinct* source mip count over the whole run: the
        // DXT path passes the source mip chain straight through to the GPU,
        // so tiles authored without mips would render full-res at every
        // distance — this makes that visible. (Static, not Local: the
        // loader runs concurrently on the async task pool.)
        if ddj_file.is_terrain_texture {
            static SEEN_MIP_COUNTS: std::sync::Mutex<std::collections::BTreeSet<u32>> =
                std::sync::Mutex::new(std::collections::BTreeSet::new());
            let mips = ddj_file.dds.get_num_mipmap_levels();
            if SEEN_MIP_COUNTS.lock().unwrap().insert(mips) {
                bevy::log::info!(
                    "terrain tile source mips = {mips} (format {:?}), first seen: {}",
                    ddj_file.dds.get_d3d_format(),
                    load_context.path().path().display()
                );
            }
        }
        match ddj_file.to_image(!settings.non_color) {
            None => Err(DDJLoaderErrors::PROBLEM),
            Some(image) => {
                if ddj_file.is_terrain_texture {
                    self.tile_tints
                        .derive(load_context.path().clone_owned(), &image);
                }
                // let mut sampler_desc = ImageSamplerDescriptor {
                //     address_mode_u: ImageAddressMode::Repeat,
                //     address_mode_v: ImageAddressMode::Repeat,
                //     ..default()
                // };
                // let sampler = ImageSampler::Descriptor(sampler_desc);
                // image.sampler = sampler;
                Ok(image)
            }
        }
    }

    fn extensions(&self) -> &[&str] {
        &["ddj"]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    fn synthetic_ddj(format: D3DFormat, levels: u32) -> JMXVDDJ {
        let dds = Dds::new_d3d(NewD3dParams {
            height: 4,
            width: 4,
            depth: None,
            format,
            mipmap_levels: Some(levels),
            caps2: None,
        })
        .unwrap();
        JMXVDDJ {
            signature: DDJ_SIGNATURE.to_owned(),
            texture_buffer_size: 0,
            texture_type: D3dResourceType::Texture,
            texture_buffer: Vec::new(),
            is_terrain_texture: false,
            dds,
        }
    }

    /// Optional corpus measurement uses only our parsers and reports aggregates.
    /// It does not render or claim process/GPU memory or timing improvements.
    #[test]
    #[ignore = "reads the user's PK2s; requires local config.yaml key and assets"]
    fn measure_rendering_texture_corpus() {
        use bevy_pk2::prelude::{Archive, Pk2Key};
        use std::path::Path;
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let key = Pk2Key::resolve_from(&root.join("config.yaml")).unwrap();
        for archive_name in ["Map.pk2", "Data.pk2", "Media.pk2"] {
            let archive = Archive::open(root.join("assets").join(archive_name), &key).unwrap();
            let mut buckets = [(0usize, 0usize, 0usize); 2];
            let (mut rejected, mut uncompressed) = (0usize, 0usize);
            for (path, entry) in archive.root.get_all_entries() {
                if !entry.is_file()
                    || !path
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("ddj"))
                {
                    continue;
                }
                let bytes = archive.read_file_bytes(&path).unwrap();
                let Ok(mut ddj) = JMXVDDJ::try_from(&mut Bytes::from(bytes)) else {
                    continue;
                };
                let terrain = path
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .contains("tile2d");
                ddj.is_terrain_texture = terrain;
                let format = ddj.dds.get_d3d_format();
                let fallback = format == Some(D3DFormat::DXT1)
                    && !terrain
                    && dxt1_has_alpha(&ddj.dds.data, ddj.dds.header.width, ddj.dds.header.height);
                let hand_decoded = matches!(
                    format,
                    Some(
                        D3DFormat::A1R5G5B5
                            | D3DFormat::R5G6B5
                            | D3DFormat::X8R8G8B8
                            | D3DFormat::A4R4G4B4
                            | D3DFormat::X1R5G5B5
                    )
                ) || ddj.is_palettized();
                if !terrain && !fallback && !hand_decoded {
                    continue;
                }
                let Some(image) = ddj.to_image(true) else {
                    rejected += 1;
                    continue;
                };
                if terrain || fallback {
                    let bucket = &mut buckets[usize::from(!terrain)];
                    bucket.0 += 1;
                    bucket.1 += ddj.dds.data.len();
                    bucket.2 += image.data.as_ref().unwrap().len();
                }
                uncompressed += usize::from(hand_decoded);
            }
            for (name, (count, source, uploaded)) in
                ["terrain", "BC1-cutout"].into_iter().zip(buckets)
            {
                eprintln!("{archive_name}: {name} textures={count}, source bytes={source}, uploaded bytes={uploaded}");
            }
            eprintln!("{archive_name}: hand-decoded={uncompressed}, rejected={rejected}");
            assert_eq!(
                rejected, 0,
                "supported texture layouts regressed in {archive_name}"
            );
        }
    }

    #[test]
    fn decoded_bc1_keeps_authored_mips_and_both_endpoint_modes() {
        let mut source = synthetic_ddj(D3DFormat::DXT1, 3);
        let punch = [0, 0, 255, 255, 0xE4, 0xE4, 0xE4, 0xE4];
        let red = [0, 0xF8, 0, 0, 0, 0, 0, 0];
        let blue = [0x1F, 0, 0, 0, 0, 0, 0, 0];
        source.dds.data = [punch, red, blue].concat();
        let image = source.to_image(true).unwrap();
        assert_eq!(image.texture_descriptor.mip_level_count, 3);
        let data = image.data.unwrap();
        assert_eq!(data.len(), (16 + 4 + 1) * 4);
        assert_eq!(&data[12..16], &[0, 0, 0, 0]);
        assert!(data[64..80].chunks_exact(4).all(|p| p == [255, 0, 0, 255]));
        assert_eq!(&data[80..], &[0, 0, 255, 255]);
    }

    #[test]
    fn decoded_partial_chain_is_preserved_and_truncation_is_rejected() {
        let mut source = synthetic_ddj(D3DFormat::A4R4G4B4, 2);
        source.dds.data = vec![255; 32];
        source.dds.data.extend([0, 0xF0].repeat(4));
        let image = source.to_image(true).unwrap();
        assert_eq!(image.texture_descriptor.mip_level_count, 2);
        assert!(image.data.unwrap()[64..]
            .chunks_exact(4)
            .all(|p| p == [0, 0, 0, 255]));
        source.dds.data.pop();
        assert!(source.to_image(true).is_none());
    }

    #[test]
    fn dxt1_punchthrough_texels_are_transparent() {
        // punch-through block (color0 <= color1); all 16 indices = 3 (0xFFFFFFFF)
        let block = [0, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF];
        assert!(dxt1_has_alpha(&block, 4, 4));
        let rgba = decode_dxt1_rgba8(&block, 4, 4);
        assert!(
            rgba.chunks(4).all(|px| px[3] == 0),
            "punch-through index-3 texels must be transparent"
        );
    }

    #[test]
    fn dxt1_opaque_block_stays_opaque() {
        // opaque block (color0 > color1); all indices = 0
        let block = [0xFF, 0xFF, 0x00, 0x00, 0, 0, 0, 0];
        assert!(!dxt1_has_alpha(&block, 4, 4));
        let rgba = decode_dxt1_rgba8(&block, 4, 4);
        assert!(
            rgba.chunks(4).all(|px| px[3] == 255),
            "opaque-mode texels must stay opaque"
        );
    }

    /// `Media/res_ui/nifenchantwnd.ddj` carries a `.ddj` extension but is a
    /// 2DT/UI window definition. It used to hit `expect("i failed")` on the
    /// resource-type conversion and abort the process; it must now be a typed
    /// error the loader can log and skip.
    ///
    /// The head doubles as a 2DT entry-count check: `0x48` = 72 entries, which
    /// is exactly what `res_ui/nifenchantwnd.2dt` carries
    /// (70,276 B = 4 + 72x976), confirming the two files are revisions of one
    /// descriptor rather than unrelated blobs (#476).
    #[test]
    fn a_non_ddj_file_is_rejected_not_panicked_on() {
        // real head of that file: length prefix + the class name
        let mut raw = Bytes::from_static(b"\x48\x00\x00\x00CNIFEnchantWnd\x00\x00");
        assert_eq!(u32::from_le_bytes([0x48, 0, 0, 0]), 72);
        match JMXVDDJ::try_from(&mut raw) {
            Err(DDJLoaderErrors::NotADdj(sig)) => assert!(!sig.starts_with("JMXVDDJ")),
            other => panic!("expected NotADdj, got {other:?}", other = other.err()),
        }
    }

    /// Anything shorter than the 20-byte container header must not be indexed
    /// past its end.
    #[test]
    fn a_truncated_file_is_rejected() {
        let mut raw = Bytes::from_static(b"JMXVDDJ 1");
        assert!(matches!(
            JMXVDDJ::try_from(&mut raw),
            Err(DDJLoaderErrors::Truncated(9))
        ));
    }

    /// `FallbackAssetReader` substitutes these bytes for a missing `.ddj`
    /// file; they must round-trip through the same container parse and
    /// pixel decode a real file would, all the way to a valid 1x1 `Image`,
    /// or the "fix" would just move the stuck-loading-screen bug one layer
    /// down instead of closing it.
    #[test]
    fn placeholder_ddj_bytes_decode_to_a_1x1_magenta_image() {
        let bytes = placeholder_ddj_bytes();
        let ddj = JMXVDDJ::try_from(&mut Bytes::from(bytes)).expect("must parse as a DDJ");
        let image = ddj.to_image(true).expect("must decode to an image");
        assert_eq!(image.texture_descriptor.size.width, 1);
        assert_eq!(image.texture_descriptor.size.height, 1);
        assert_eq!(image.data.as_deref(), Some([255, 0, 255, 255].as_slice()));
    }

    /// 4-bit channels expand by nibble replication, so 0xF -> 255 exactly and
    /// 0x0 -> 0 (a plain `<< 4` would cap white at 240).
    #[test]
    fn a4r4g4b4_expands_nibbles_to_full_range() {
        // 0xFFFF = opaque white, 0x0000 = transparent black, 0xFF00 = opaque red
        let bytes = [0xFF, 0xFF, 0x00, 0x00, 0x00, 0xFF];
        let rgba = a4r4g4b4_to_rgba8(&bytes);
        assert_eq!(&rgba[0..4], &[255, 255, 255, 255]);
        assert_eq!(&rgba[4..8], &[0, 0, 0, 0]);
        assert_eq!(&rgba[8..12], &[255, 0, 0, 255]);
        // a mid nibble replicates rather than shifting: 0x8 -> 0x88, not 0x80
        let mid = a4r4g4b4_to_rgba8(&[0x00, 0xF8]);
        assert_eq!(&mid[0..4], &[0x88, 0, 0, 255]);
    }

    /// X1R5G5B5's top bit is padding, not alpha: every texel is opaque even
    /// when that bit is 0 (which `A1R5G5B5` would decode as transparent).
    #[test]
    fn x1r5g5b5_ignores_the_top_bit_and_stays_opaque() {
        // 0x7FFF = white with the pad bit clear; 0xFFFF = white with it set
        let bytes = [0xFF, 0x7F, 0xFF, 0xFF];
        let rgba = x1r5g5b5_to_rgba8(&bytes);
        assert_eq!(&rgba[0..4], &[255, 255, 255, 255]);
        assert_eq!(&rgba[4..8], &[255, 255, 255, 255]);
    }

    /// P8 indexes a 1024-byte BGRA palette that precedes the index plane.
    #[test]
    fn p8_indexes_the_leading_bgra_palette() {
        let mut data = vec![0u8; 1024];
        // entry 1 = BGRA(0x10, 0x20, 0x30, 0x40) -> RGBA(0x30, 0x20, 0x10, 0x40)
        data[4..8].copy_from_slice(&[0x10, 0x20, 0x30, 0x40]);
        // entry 2 = opaque white
        data[8..12].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);
        data.extend_from_slice(&[1, 2, 0, 1]); // 2x2 index plane
        let rgba = p8_to_rgba8(&data, 2, 2).expect("decodes");
        assert_eq!(&rgba[0..4], &[0x30, 0x20, 0x10, 0x40]);
        assert_eq!(&rgba[4..8], &[0xFF, 0xFF, 0xFF, 0xFF]);
        assert_eq!(rgba.len(), 4 * 4);
    }

    /// A palettized file too small for its own palette + index plane must be
    /// skipped, never read out of bounds.
    #[test]
    fn p8_rejects_a_buffer_that_cannot_hold_the_plane() {
        let data = vec![0u8; 1024 + 3];
        assert!(p8_to_rgba8(&data, 2, 2).is_none());
    }
}
