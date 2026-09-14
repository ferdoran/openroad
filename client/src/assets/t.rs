// Terrain lightmap (JMXVMAPT). SRO bakes per-region terrain lighting — static sun plus cast
// shadows from map geometry — into two payloads: a 96×96 per-tile byte grid (originally used to
// gate whether dynamic objects cast terrain shadows) and a DDS texture holding the high-res baked
// lighting the terrain splat shader samples (see terrain_splat.wgsl). The DDS tail is byte-identical
// to JMXVDDJ minus its signature, so it decodes through the shared `ddj::dds_buffer_to_image` path.
// We expose the decoded texture as a labeled `lightmap` sub-asset; the tile grid is parsed but not
// yet consumed.
// https://github.com/DummkopfOfHachtenduden/SilkroadDoc/wiki/JMXVMAPT

use bevy::asset::{io::Reader, Asset, AssetLoader, Handle, LoadContext};
use bevy::image::Image;
use bevy::log::warn;
use bevy::reflect::TypePath;
use thiserror::Error;

use crate::assets::ddj::dds_buffer_to_image;

const SIGNATURE_LEN: usize = 12;
/// 96×96 per-tile light bytes covering the region's 1920×1920 units.
const TILE_GRID_LEN: usize = 96 * 96;
/// lightMapBufferSize (i32) + lightMapTextureType (i32) preceding the DDS payload.
const TEXTURE_META_LEN: usize = 8;

#[derive(Asset, TypePath)]
pub struct JMXVMAPT {
    /// 96×96 per-tile light bytes. Parsed and retained but not yet consumed (its original role was
    /// gating dynamic object terrain shadows).
    #[allow(dead_code)]
    pub tile_light: Vec<u8>,
    /// Baked terrain lighting texture decoded from the embedded DDS. `None` when the region ships
    /// no lightmap or it failed to decode; the terrain material then falls back to no modulation.
    pub lightmap: Option<Handle<Image>>,
}

#[derive(Error, Debug)]
pub enum TLoaderError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("file too small for JMXVMAPT header")]
    Truncated,
    #[error("unexpected signature: {0:?}")]
    Signature(String),
}

/// Split view of a parsed file: the tile grid (owned) and the raw DDS payload (borrowed from the
/// input buffer). Kept separate from the labeled-asset step so it can be unit-tested without a
/// `LoadContext`.
struct Parsed<'a> {
    tile_light: Vec<u8>,
    dds_bytes: Option<&'a [u8]>,
}

fn parse_bytes(buf: &[u8]) -> Result<Parsed<'_>, TLoaderError> {
    let header_end = SIGNATURE_LEN + TILE_GRID_LEN;
    if buf.len() < header_end {
        return Err(TLoaderError::Truncated);
    }
    let signature = String::from_utf8_lossy(&buf[..SIGNATURE_LEN]);
    if !signature.starts_with("JMXVMAPT") {
        return Err(TLoaderError::Signature(signature.into_owned()));
    }
    let tile_light = buf[SIGNATURE_LEN..header_end].to_vec();

    // Tail: lightMapBufferSize (i32), lightMapTextureType (i32), then the DDS bytes. Only the DDS
    // payload is needed for decoding; the size/type mirror the DDJ header. Take the rest of the
    // file — the DDS reader consumes exactly what it needs.
    let dds_bytes = buf
        .get(header_end + TEXTURE_META_LEN..)
        .filter(|rest| !rest.is_empty());

    // `lightMapBufferSize` is inclusive of its own 8 header bytes, so the DDS
    // payload is `TextureLength - 8` (the SilkroadDoc wiki omits the +8;
    // srodevs-docs has it right). Taking "rest of file" above is correct on
    // every shipped file, but comparing the two is the only way to notice a
    // truncated tail.
    if let Some(rest) = dds_bytes {
        let declared = buf
            .get(header_end..header_end + 4)
            .and_then(|b| <[u8; 4]>::try_from(b).ok())
            .map(i32::from_le_bytes)
            .unwrap_or(0);
        let expected = declared as i64 - TEXTURE_META_LEN as i64;
        if expected > 0 && expected != rest.len() as i64 {
            warn!(
                "t: lightmap payload is {} bytes but the header declares {expected} \
                 (TextureLength {declared} counts its own 8 header bytes)",
                rest.len()
            );
        }
    }

    Ok(Parsed {
        tile_light,
        dds_bytes,
    })
}

#[derive(Default, TypePath)]
pub struct TLoader;

impl AssetLoader for TLoader {
    type Asset = JMXVMAPT;
    type Settings = ();
    type Error = TLoaderError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await?;
        let parsed = parse_bytes(&buf)?;
        let lightmap = parsed
            .dds_bytes
            .and_then(dds_buffer_to_image)
            .map(|image| load_context.add_labeled_asset("lightmap".to_string(), image));
        Ok(JMXVMAPT {
            tile_light: parsed.tile_light,
            lightmap,
        })
    }

    fn extensions(&self) -> &[&str] {
        &["t"]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(sig: &[u8; 12]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(sig);
        v.extend(std::iter::repeat_n(0u8, TILE_GRID_LEN));
        v
    }

    #[test]
    fn parses_tile_grid_and_dds_tail() {
        let mut buf = header(b"JMXVMAPT1001");
        buf.extend_from_slice(&[0u8; TEXTURE_META_LEN]); // size + type
        buf.extend_from_slice(&[0xDD, 0x53]); // stand-in DDS bytes
        let parsed = parse_bytes(&buf).expect("should parse");
        assert_eq!(parsed.tile_light.len(), TILE_GRID_LEN);
        assert_eq!(parsed.dds_bytes, Some(&[0xDD, 0x53][..]));
    }

    #[test]
    fn no_dds_tail_yields_none() {
        let buf = header(b"JMXVMAPT1001");
        let parsed = parse_bytes(&buf).expect("header-only still parses");
        assert_eq!(parsed.tile_light.len(), TILE_GRID_LEN);
        assert_eq!(parsed.dds_bytes, None);
    }

    #[test]
    fn rejects_truncated_and_bad_signature() {
        assert!(matches!(
            parse_bytes(&[0u8; 16]),
            Err(TLoaderError::Truncated)
        ));
        let bad = header(b"NOTAMAPTFILE");
        assert!(matches!(parse_bytes(&bad), Err(TLoaderError::Signature(_))));
    }

    /// Diagnostic (same pattern as `probe_splat_scale_census` in `m/mod.rs`):
    /// census of the embedded DDS pixel format and alpha-channel content of
    /// every region's `.t` lightmap in Map.pk2. The mobile port decodes its
    /// terrain lightmap as RGBM (`rgb * a * hdrScale`); whether SRO's `.t`
    /// alpha carries such an exponent — or anything at all — was never
    /// recorded (see docs/rendering-mobile-shader-comparison.md §"The
    /// lightmap is RGBM-encoded" and `sample_lightmap` in
    /// terrain_splat.wgsl, which discards `.a`). Run:
    /// cargo test -p client probe_t_lightmap_format_census -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic; needs real assets/Map.pk2"]
    fn probe_t_lightmap_format_census() {
        use std::collections::BTreeMap;
        use std::path::PathBuf;

        let archive = bevy_pk2::prelude::Archive::configured(&PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../assets/Map.pk2"
        )));

        let mut regions = 0u32;
        let mut no_dds = 0u32;
        let mut formats: BTreeMap<String, u32> = BTreeMap::new();
        let mut tex_types: BTreeMap<i32, u32> = BTreeMap::new();
        // per-format: (global alpha min, global alpha max, files with
        // non-constant alpha, sample paths of such files)
        let mut alpha: BTreeMap<String, (u8, u8, u32, Vec<String>)> = BTreeMap::new();

        for z in 0..=255u32 {
            for x in 0..=255u32 {
                let path = PathBuf::from(format!("{z}/{x}.t"));
                let Some(data) = archive.read_file_bytes(&path) else {
                    continue;
                };
                let Ok(parsed) = parse_bytes(&data) else {
                    println!("parse failure: {z}/{x}.t");
                    continue;
                };
                regions += 1;
                let header_end = SIGNATURE_LEN + TILE_GRID_LEN;
                if let Some(ty) = data.get(header_end + 4..header_end + 8) {
                    *tex_types
                        .entry(i32::from_le_bytes(ty.try_into().unwrap()))
                        .or_default() += 1;
                }
                let Some(mut dds_bytes) = parsed.dds_bytes else {
                    no_dds += 1;
                    continue;
                };
                let Ok(dds) = ddsfile::Dds::read(&mut dds_bytes) else {
                    println!("DDS read failure: {z}/{x}.t");
                    continue;
                };
                let format = dds
                    .get_d3d_format()
                    .map(|f| format!("{f:?}"))
                    .unwrap_or_else(|| format!("fourcc/unknown {:?}", dds.header.spf.fourcc));
                *formats.entry(format.clone()).or_default() += 1;

                // Alpha census. Uncompressed 32-bit formats store BGRA per
                // texel (alpha = byte 3); DXT2-5 blocks lead with 8 alpha
                // bytes per 16-byte block (interpolated formats: alpha0 and
                // alpha1 endpoints are bytes 0 and 1). DXT1 has no alpha
                // channel worth measuring. Data includes all mip levels,
                // which is fine for a constant-or-not verdict.
                let alphas: Box<dyn Iterator<Item = u8>> = match format.as_str() {
                    "A8R8G8B8" | "X8R8G8B8" => {
                        Box::new(dds.data.chunks_exact(4).map(|texel| texel[3]))
                    }
                    "DXT4" | "DXT5" => Box::new(
                        dds.data
                            .chunks_exact(16)
                            .flat_map(|block| [block[0], block[1]]),
                    ),
                    "DXT2" | "DXT3" => Box::new(
                        dds.data
                            .chunks_exact(16)
                            .flat_map(|block| block[..8].to_vec())
                            .flat_map(|b| [b & 0x0f, b >> 4]),
                    ),
                    _ => Box::new(std::iter::empty()),
                };
                let mut min = u8::MAX;
                let mut max = u8::MIN;
                for a in alphas {
                    min = min.min(a);
                    max = max.max(a);
                }
                if min <= max {
                    let entry = alpha
                        .entry(format)
                        .or_insert((u8::MAX, u8::MIN, 0, Vec::new()));
                    entry.0 = entry.0.min(min);
                    entry.1 = entry.1.max(max);
                    if min != max {
                        entry.2 += 1;
                        if entry.3.len() < 5 {
                            entry.3.push(format!("{z}/{x}.t (alpha {min}..{max})"));
                        }
                    }
                }
            }
        }

        println!("{regions} .t files parsed, {no_dds} without a DDS tail");
        for (ty, count) in &tex_types {
            println!("lightMapTextureType {ty}: {count} files");
        }
        for (format, count) in &formats {
            println!("format {format}: {count} files");
        }
        for (format, (min, max, varying, samples)) in &alpha {
            println!(
                "format {format}: alpha range {min}..{max}, \
                 {varying} files with non-constant alpha"
            );
            for s in samples {
                println!("  {s}");
            }
        }
    }
}
