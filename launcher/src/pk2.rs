use crate::utils::show_error;
use bevy_pk2::prelude::{Archive, Pk2Key};
use byteorder::{LittleEndian, ReadBytesExt};
use ddsfile::{D3DFormat, Dds};
use std::env;
use std::io::{Cursor, Read};
use std::path::PathBuf;

pub fn dat_or_image(data: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    if data.len() >= 12 && &data[..7] == b"JMXVDDJ" {
        return ddj(data);
    }

    if data.len() >= 4 && &data[..4] == b"DDS " {
        return dds(data);
    }

    let img = image::load_from_memory(data).map_err(|e| format!("Image decode failed: {e}"))?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Ok((w, h, rgba.into_raw()))
}

pub fn ddj(data: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let mut cursor = Cursor::new(data);
    let mut signature = [0u8; 12];
    cursor
        .read_exact(&mut signature)
        .map_err(|e| format!("DDJ signature read failed: {e}"))?;
    let signature_str = String::from_utf8_lossy(&signature);
    if !signature_str.starts_with("JMXVDDJ") {
        return Err("Invalid DDJ signature".to_string());
    }

    let _buffer_size = cursor
        .read_i32::<LittleEndian>()
        .map_err(|e| format!("DDJ size read failed: {e}"))?;
    let _texture_type = cursor
        .read_i32::<LittleEndian>()
        .map_err(|e| format!("DDJ type read failed: {e}"))?;

    let mut dds_bytes = Vec::new();
    cursor
        .read_to_end(&mut dds_bytes)
        .map_err(|e| format!("DDJ DDS read failed: {e}"))?;

    dds(&dds_bytes)
}

pub fn dds(data: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    if let Ok(img) = image::load_from_memory_with_format(data, image::ImageFormat::Dds) {
        let rgba = img.to_rgba8();
        let (w, h) = rgba.dimensions();
        return Ok((w, h, rgba.into_raw()));
    }

    let mut cursor = Cursor::new(data);
    let dds = Dds::read(&mut cursor).map_err(|e| format!("DDS read failed: {e}"))?;
    let format = dds
        .get_d3d_format()
        .ok_or_else(|| "DDS format unknown".to_string())?;

    let (w, h) = (dds.header.width, dds.header.height);
    let rgba = match format {
        D3DFormat::A1R5G5B5 => a1r5g5b5_to_rgba8(&dds.data),
        D3DFormat::R5G6B5 => r5g6b5_to_rgba8(&dds.data),
        D3DFormat::X8R8G8B8 => x8r8g8b8_to_rgba8(&dds.data),
        _ => {
            return Err(format!("Unsupported DDS format: {:?}", format));
        }
    };

    Ok((w, h, rgba))
}

pub fn r5g6b5_to_rgba8(bytes: &Vec<u8>) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len() * 2);
    let mut rdr = Cursor::new(bytes);
    let mut i = 0;
    while i < bytes.len() {
        let word = rdr.read_u16::<LittleEndian>().expect("r5g6b5 read failed");
        let r = ((word & 0b11111_000000_00000) >> 8) | 0b00000111;
        let g = ((word & 0b00000_111111_00000) >> 3) | 0b00000111;
        let b = ((word & 0b00000_000000_11111) << 3) | 0b00000111;
        out.push(r as u8);
        out.push(g as u8);
        out.push(b as u8);
        out.push(0xff);
        i += 2;
    }
    out
}

pub fn a1r5g5b5_to_rgba8(bytes: &Vec<u8>) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len() * 2);
    let mut rdr = Cursor::new(bytes);
    let mut i = 0;
    while i < bytes.len() {
        let word = rdr
            .read_u16::<LittleEndian>()
            .expect("a1r5g5b5 read failed");
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

pub fn x8r8g8b8_to_rgba8(bytes: &Vec<u8>) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len() * 2);
    let mut rdr = Cursor::new(bytes);
    let mut i = 0;
    while i < bytes.len() {
        let word = rdr
            .read_u32::<LittleEndian>()
            .expect("x8r8g8b8 read failed");
        let r = (word & 0x00ff0000) >> 16;
        let g = (word & 0x0000ff00) >> 8;
        let b = word & 0x000000ff;
        out.push(r as u8);
        out.push(g as u8);
        out.push(b as u8);
        out.push(0xff);
        i += 4;
    }
    out
}

pub fn read_pk2(file_path: &str) -> Result<Archive, String> {
    let sro_path = match env::var_os("SRO_PATH") {
        Some(val) => PathBuf::from(val),
        None => env::current_dir().map_err(|e| {
            let msg = format!("Failed to read current dir: {}", e);
            show_error(&msg);
            msg
        })?,
    };

    let abs_path = sro_path.join(file_path);
    if !abs_path.is_file() {
        let msg = format!(
            "{file_path} not found at {}\n\nSet SRO_PATH to your SRO folder.",
            abs_path.display()
        );
        show_error(&msg);
        return Err(format!("{file_path} missing"));
    }

    // Resolved before the open so a missing key reports itself, rather than
    // surfacing as a generic "failed to open" from the archive open below.
    let key = Pk2Key::resolve().map_err(|err| {
        let msg = err.to_string();
        show_error(&msg);
        msg
    })?;

    let archive = Archive::open(&abs_path, &key).map_err(|err| {
        let msg = format!("Failed to open {} - {err}", abs_path.display());
        show_error(&msg);
        msg
    })?;

    Ok(archive)
}
