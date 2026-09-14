// DDJ -> PNG transcode. A .ddj is a 20-byte header (12-byte signature +
// i32 size + i32 type) followed by a raw DDS file. glTF only allows
// png/jpeg images, so the DDS payload is decoded to RGBA8 and re-encoded:
// the `image` crate covers the compressed formats (DXT1/3/5), the client's
// `dds_to_rgba8` covers the uncompressed D3D formats (A1R5G5B5, R5G6B5,
// X8R8G8B8, A8R8G8B8) the image crate rejects. Exotic formats (DXT2/4,
// DX10 headers) fail both paths and yield `None` — callers emit the
// material untextured and warn instead of aborting.

use std::io::Cursor;

use client::assets::ddj::dds_to_rgba8;

const DDJ_HEADER_LEN: usize = 20;

pub fn ddj_to_png(ddj_bytes: &[u8]) -> Option<Vec<u8>> {
    if ddj_bytes.len() <= DDJ_HEADER_LEN {
        return None;
    }
    let dds = &ddj_bytes[DDJ_HEADER_LEN..];

    let rgba = match image::load_from_memory_with_format(dds, image::ImageFormat::Dds) {
        Ok(decoded) => decoded.to_rgba8(),
        Err(_) => {
            let (width, height, data) = dds_to_rgba8(dds)?;
            image::RgbaImage::from_raw(width, height, data)?
        }
    };

    let mut png = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(rgba)
        .write_to(&mut png, image::ImageOutputFormat::Png)
        .ok()?;
    Some(png.into_inner())
}
