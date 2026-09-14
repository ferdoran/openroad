use std::borrow::BorrowMut;
use std::io::Cursor;
use std::ops::Deref;

use bevy::asset::{io::Reader, Asset, AssetLoader, LoadContext};
use bevy::reflect::TypePath;
use bytes::Buf;
use thiserror::Error;

use crate::assets::read_str_and_jump;

/// Signature (12) + width/height + four shorts = 24 bytes before the bitmap.
const MFO_HEADER_LEN: usize = 24;

// https://github.com/DummkopfOfHachtenduden/SilkroadDoc/wiki/JMXVMFO
#[derive(TypePath, Asset, Debug)]
#[allow(dead_code)]
pub struct JMXVMFO {
    pub signature: String, // [char, 12]
    pub map_width: usize,
    pub map_height: usize,
    pub short0: i16,
    pub short1: i16,
    pub short2: i16,
    pub short3: i16,
    pub raw_region_data: Vec<u8>, // [u8; 8192], byte array
    pub region_data: Vec<bool>,   // [bool; 8192*8] bit array
}

impl From<&[u8]> for JMXVMFO {
    fn from(bytes: &[u8]) -> Self {
        let mut cursor = Cursor::new(bytes);
        let signature = read_str_and_jump(cursor.borrow_mut(), 12);
        // Little-endian, like every other SRO on-disk word. `get_i16_ne` read
        // these in the host's byte order, which only happened to be right
        // because every supported target is little-endian.
        let map_width = cursor.get_i16_le();
        let map_height = cursor.get_i16_le();
        let short0 = cursor.get_i16_le();
        let short1 = cursor.get_i16_le();
        let short2 = cursor.get_i16_le();
        let short3 = cursor.get_i16_le();
        // Callers check the length first (see the loader); this stays graceful
        // for any other construction site.
        let raw_region_data = bytes.get(MFO_HEADER_LEN..).unwrap_or(&[]);
        let mut region_data = vec![false; raw_region_data.len() * 8];

        for i in 0..raw_region_data.len() {
            let byte = raw_region_data[i];
            let index = i * 8;
            region_data[index + 0] = (byte & 0b10000000) > 0;
            region_data[index + 1] = (byte & 0b01000000) > 0;
            region_data[index + 2] = (byte & 0b00100000) > 0;
            region_data[index + 3] = (byte & 0b00010000) > 0;
            region_data[index + 4] = (byte & 0b00001000) > 0;
            region_data[index + 5] = (byte & 0b00000100) > 0;
            region_data[index + 6] = (byte & 0b00000010) > 0;
            region_data[index + 7] = (byte & 0b00000001) > 0;
        }

        JMXVMFO {
            signature,
            map_width: map_width as usize,
            map_height: map_height as usize,
            short0,
            short1,
            short2,
            short3,
            raw_region_data: raw_region_data.to_vec(),
            region_data,
        }
    }
}

#[derive(Default, bevy::reflect::TypePath)]
pub struct MFOLoader;

#[derive(Error, Debug)]
pub enum MFOLoaderError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Shorter than the 24-byte header, so there is no region bitmap to read.
    #[error("truncated mfo: {0} bytes")]
    Truncated(usize),
}

impl AssetLoader for MFOLoader {
    type Asset = JMXVMFO;
    type Settings = ();
    type Error = MFOLoaderError;
    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await?;
        let bytes = buf.deref();
        if bytes.len() < MFO_HEADER_LEN {
            return Err(MFOLoaderError::Truncated(bytes.len()));
        }
        Ok(JMXVMFO::from(bytes))
    }

    fn extensions(&self) -> &[&str] {
        &["mfo"]
    }
}
