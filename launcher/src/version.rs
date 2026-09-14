use std::path::Path;

use bevy::asset::io::AssetReader;
use bevy_pk2::{Blowfish, Pk2Key, prelude::Archive};
use futures_lite::io::AsyncReadExt;

pub(crate) fn read_sv_t_version(archive: &Archive) -> Result<u32, String> {
    let mut reader = futures_lite::future::block_on(archive.read(Path::new("SV.T")))
        .map_err(|e| format!("Failed to read SV.T: {e}"))?;

    let mut data = Vec::new();
    futures_lite::future::block_on(async { reader.read_to_end(&mut data).await })
        .map_err(|e| format!("Failed to read SV.T bytes: {e}"))?;

    if data.len() < 4 {
        return Err("SV.T too short".to_string());
    }

    let len = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
    if data.len() < 4 + len {
        return Err(format!("SV.T length {len} exceeds file size"));
    }

    if len % 8 != 0 {
        return Err(format!(
            "SV.T encrypted buffer length {len} is not 8-byte aligned"
        ));
    }

    let mut enc = data[4..4 + len].to_vec();
    // User-supplied like the archive key, and a different one — see
    // bevy_pk2::Pk2Key. Blowfish here takes only the first 8 bytes of it;
    // that truncation is part of how SV.T is encrypted, not a property of the
    // configured value, so it stays here rather than in the config.
    let configured = Pk2Key::resolve_version().map_err(|e| e.to_string())?;
    let key = configured.key();
    let take = key.len().min(8);
    let blowfish = Blowfish::new(&key[..take], configured.salt())
        .map_err(|e| format!("SV.T blowfish key error: {e}"))?;
    blowfish.decrypt(&mut enc);

    let version_str = enc
        .iter()
        .take_while(|b| **b != 0)
        .map(|b| *b as char)
        .collect::<String>()
        .trim()
        .to_string();

    let version = version_str
        .parse::<u32>()
        .map_err(|_| format!("SV.T version not numeric: {version_str:?}"))?;

    Ok(version)
}
