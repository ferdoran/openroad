use std::path::Path;

use bevy::asset::io::{AssetReader, AssetReaderError, PathStream, Reader, VecReader};
use bevy::log::warn;

use bevy_pk2::prelude::Archive;

use crate::assets::ddj::placeholder_ddj_bytes;

/// Wraps a PK2 [`Archive`] and substitutes a warned-about placeholder for a
/// `.ddj`/`.wav` path the archive doesn't have, instead of the plain
/// [`AssetReaderError::NotFound`] `Archive` itself returns for every path.
///
/// Why this exists: `bevy_asset_loader`'s `AssetCollection` only creates its
/// resource and lets `SceneState::Loading` advance once *every* handle in
/// the collection reaches `LoadState::Loaded` — a single `Failed` handle
/// (e.g. one missing UI button `.ddj`, and `IntroV2Assets` alone names
/// nearly a hundred of them) leaves that collection, and therefore the whole
/// loading screen, stuck forever with nothing logged anywhere
/// (`bevy_asset_loader-0.27.0/src/loading_state/systems.rs`:
/// `count_loaded_handles` never removes a failed collection from the pending
/// set, and no `.on_failure_continue_to_state` is configured for
/// `SceneState::Loading`). Substituting a real, decodable placeholder for
/// the file kinds where that's safe keeps the load succeeding instead.
///
/// Deliberately scoped to `ddj`/`wav`: substituting bytes for a `.txt`
/// (`Textdata`) path would silently feed garbage into a structured parser
/// rather than producing an obviously-fake result, so those keep surfacing
/// `NotFound` unchanged. `assets::textdata::TextdataLoader`'s own
/// master-list loaders (characterdata/skilldata/itemdata/textdataname)
/// separately warn-and-skip a missing *shard* a master file references; a
/// genuinely absent top-level table is left as a hard stop on purpose.
#[derive(Clone)]
pub struct FallbackAssetReader {
    pub inner: Archive,
}

impl AssetReader for FallbackAssetReader {
    async fn read<'a>(&'a self, path: &'a Path) -> Result<Box<dyn Reader>, AssetReaderError> {
        match self.inner.read(path).await {
            Err(AssetReaderError::NotFound(missing)) => {
                match missing.extension().and_then(|ext| ext.to_str()) {
                    Some("ddj") => {
                        warn!(
                            "asset missing, using placeholder texture: {}",
                            missing.display()
                        );
                        Ok(Box::new(VecReader::new(placeholder_ddj_bytes())))
                    }
                    Some("wav") => {
                        warn!(
                            "asset missing, using silent placeholder audio: {}",
                            missing.display()
                        );
                        Ok(Box::new(VecReader::new(placeholder_wav_bytes())))
                    }
                    _ => Err(AssetReaderError::NotFound(missing)),
                }
            }
            other => other,
        }
    }

    async fn read_meta<'a>(&'a self, path: &'a Path) -> Result<Box<dyn Reader>, AssetReaderError> {
        self.inner.read_meta(path).await
    }

    async fn read_directory<'a>(
        &'a self,
        path: &'a Path,
    ) -> Result<Box<PathStream>, AssetReaderError> {
        self.inner.read_directory(path).await
    }

    async fn is_directory<'a>(&'a self, path: &'a Path) -> Result<bool, AssetReaderError> {
        self.inner.is_directory(path).await
    }
}

/// A minimal valid silent WAV file (44-byte header, zero PCM frames) — the
/// audio counterpart of `placeholder_ddj_bytes`, for the handful of `.wav`
/// sounds an asset collection can name.
fn placeholder_wav_bytes() -> Vec<u8> {
    const SAMPLE_RATE: u32 = 44100;
    const BITS_PER_SAMPLE: u16 = 16;
    const CHANNELS: u16 = 1;
    let byte_rate = SAMPLE_RATE * CHANNELS as u32 * (BITS_PER_SAMPLE as u32 / 8);
    let block_align = CHANNELS * (BITS_PER_SAMPLE / 8);

    let mut bytes = Vec::with_capacity(44);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&36u32.to_le_bytes()); // file size - 8, no data frames
    bytes.extend_from_slice(b"WAVE");
    bytes.extend_from_slice(b"fmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
    bytes.extend_from_slice(&CHANNELS.to_le_bytes());
    bytes.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    bytes.extend_from_slice(&byte_rate.to_le_bytes());
    bytes.extend_from_slice(&block_align.to_le_bytes());
    bytes.extend_from_slice(&BITS_PER_SAMPLE.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&0u32.to_le_bytes()); // zero PCM bytes
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_wav_is_a_well_formed_44_byte_header() {
        let bytes = placeholder_wav_bytes();
        assert_eq!(bytes.len(), 44);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(&bytes[12..16], b"fmt ");
        assert_eq!(&bytes[36..40], b"data");
        assert_eq!(u32::from_le_bytes(bytes[40..44].try_into().unwrap()), 0);
    }
}
