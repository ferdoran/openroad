use bevy::asset::{io::Reader, Asset, AssetLoader, LoadContext};
use bevy::prelude::{Component, Vec3};
use bevy::reflect::TypePath;
use bytes::Buf;
use std::ops::Deref;
use thiserror::Error;

/// 12 ASCII bytes (`JMXVMAPO1001` / `JMXVMAPO1000`) at the head of every file.
const SIGNATURE_LEN: usize = 12;
/// A region is a 6x6 grid of blocks.
const BLOCKS: usize = 36;
/// Blocks carry four count-prefixed LoD groups, like `.o2`.
const LOD_GROUPS: usize = 4;
/// Bytes per `.o` record — the `.o2` one plus no trailing `RegionID`.
const RECORD_LEN: usize = 28;

#[derive(TypePath, Asset)]
#[allow(dead_code)]
pub struct JMXVMAPO(pub Vec<Vec<MapObject>>);

/// Idea: `.o` has the same block structure as `.o2` — 12-byte signature, then
/// 36 blocks of **four** count-prefixed LoD groups — only with 28-byte records
/// (no trailing `RegionID`). The SilkroadDoc layout this file used to implement
/// (one count per block) reaches EOF on **0 of 4,360** corpus files and recovers
/// 24.4% of the objects; the four-group model is byte-exact on 4,349 of them and
/// recovers 100%, cross-checked by matching 7,429/7,429 `.o` objects against the
/// same region's `.o2` on `(ObjID, x, z)` (#442).
///
/// A handful of files carry an all-zero, three-group-shaped body. The
/// discriminator is the **body length**, not the signature version: 11 of those
/// 18 files are `JMXVMAPO1001`, so a version rule would mis-read them. Their
/// bodies are entirely zero, so nothing is lost either way.
///
/// Reads are bounds-checked: `bytes::Buf`'s getters panic on underflow and this
/// runs inside an asset-loader task.
impl<T: Buf> From<T> for JMXVMAPO {
    fn from(mut buf: T) -> Self {
        let mut blocks = Vec::with_capacity(BLOCKS);
        if buf.remaining() < SIGNATURE_LEN {
            return Self(blocks);
        }
        buf.advance(SIGNATURE_LEN);

        // An all-zero body that is exactly 36x3 counts long is three-group
        // shaped; everything else is the regular four.
        let groups = if buf.remaining() == BLOCKS * 3 * 2 {
            3
        } else {
            LOD_GROUPS
        };

        for _ in 0..BLOCKS {
            let mut objects = Vec::new();
            for _ in 0..groups {
                if buf.remaining() < 2 {
                    blocks.push(objects);
                    return Self(blocks);
                }
                let object_count = buf.get_u16_le() as usize;
                if buf.remaining() < object_count * RECORD_LEN {
                    blocks.push(objects);
                    return Self(blocks);
                }
                for _ in 0..object_count {
                    objects.push(MapObject::from(&mut buf));
                }
            }
            blocks.push(objects);
        }

        Self(blocks)
    }
}

#[derive(Default, bevy::reflect::TypePath)]
pub struct OLoader;

#[derive(Error, Debug)]
pub enum OLoaderError {}

impl AssetLoader for OLoader {
    type Asset = JMXVMAPO;
    type Settings = ();
    type Error = OLoaderError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await.unwrap();
        let bytes = buf.deref();
        let map_object = JMXVMAPO::from(bytes);
        Ok(map_object)
    }

    fn extensions(&self) -> &[&str] {
        &["o"]
    }
}

#[derive(Component, Debug, Clone)]
#[allow(dead_code)]
pub struct MapObject {
    pub id: u32,
    pub position: Vec3,
    pub is_static: bool,
    pub yaw: f32,
    pub uid: u16,
    pub short_0: u16,
    pub is_big: bool,
    pub is_struct: bool,
}

impl<T: Buf> From<&mut T> for MapObject {
    fn from(buf: &mut T) -> Self {
        Self {
            id: buf.get_u32_le(),
            position: Vec3::new(buf.get_f32_le(), buf.get_f32_le(), buf.get_f32_le()),
            is_static: buf.get_u16_le() == 0xFFFF,
            yaw: buf.get_f32_le(),
            uid: buf.get_u16_le(),
            short_0: buf.get_u16_le(),
            is_big: buf.get_u8() == 1,
            is_struct: buf.get_u8() == 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    fn record(id: u32) -> Vec<u8> {
        let mut out = Vec::with_capacity(RECORD_LEN);
        out.extend_from_slice(&id.to_le_bytes());
        for v in [1.0f32, 2.0, 3.0] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&0xFFFFu16.to_le_bytes()); // is_static
        out.extend_from_slice(&0.25f32.to_le_bytes()); // yaw
        out.extend_from_slice(&5u16.to_le_bytes()); // uid
        out.extend_from_slice(&0u16.to_le_bytes()); // short_0
        out.push(0); // is_big
        out.push(1); // is_struct
        assert_eq!(out.len(), RECORD_LEN);
        out
    }

    fn o_file(signature: &[u8; 12], place: Option<(usize, usize)>) -> Bytes {
        let mut out = Vec::new();
        out.extend_from_slice(signature);
        for block in 0..BLOCKS {
            for group in 0..LOD_GROUPS {
                let has = place == Some((block, group));
                out.extend_from_slice(&(has as u16).to_le_bytes());
                if has {
                    out.extend_from_slice(&record(42));
                }
            }
        }
        Bytes::from(out)
    }

    /// The published one-count-per-block layout stops after the first six
    /// counts; the real one is four LoD groups of 28-byte records, so an object
    /// in the last block's last group must still be found and the file must be
    /// consumed exactly.
    #[test]
    fn four_lod_groups_reach_the_last_block_and_eof() {
        let file = o_file(b"JMXVMAPO1001", Some((BLOCKS - 1, LOD_GROUPS - 1)));
        let mut buf = file.clone();
        let parsed = JMXVMAPO::from(&mut buf);

        assert_eq!(parsed.0.len(), BLOCKS);
        let objects: Vec<_> = parsed.0.iter().flatten().collect();
        assert_eq!(objects.len(), 1);
        assert_eq!(objects[0].id, 42);
        assert!(objects[0].is_struct);
        assert_eq!(buf.remaining(), 0, "the whole file must be consumed");
    }

    /// The three-group bodies are recognised by their **length**, not by the
    /// signature version: 11 of the 18 such files on disk are `JMXVMAPO1001`,
    /// so a version rule would mis-read them.
    #[test]
    fn an_all_zero_three_group_body_is_read_by_length_not_version() {
        for signature in [b"JMXVMAPO1001", b"JMXVMAPO1000"] {
            let mut out = Vec::new();
            out.extend_from_slice(signature);
            out.extend_from_slice(&vec![0u8; BLOCKS * 3 * 2]);
            let mut buf = Bytes::from(out);

            let parsed = JMXVMAPO::from(&mut buf);

            assert_eq!(parsed.0.len(), BLOCKS);
            assert!(parsed.0.iter().all(|b| b.is_empty()));
            assert_eq!(buf.remaining(), 0);
        }
    }

    /// Asset loaders must not panic: `bytes::Buf` underflow would take the
    /// client down inside the load task (#442).
    #[test]
    fn a_truncated_file_stops_instead_of_panicking() {
        let full = o_file(b"JMXVMAPO1001", Some((0, 3)));
        for len in 0..full.len() {
            let mut buf = full.slice(0..len);
            let parsed = JMXVMAPO::from(&mut buf);
            assert!(parsed.0.len() <= BLOCKS);
        }
    }
}
