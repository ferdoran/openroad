use bevy::asset::{io::Reader, Asset, AssetLoader, LoadContext};
use bevy::prelude::{Component, Vec3};
use bevy::reflect::TypePath;
use bytes::{Buf, Bytes};
use thiserror::Error;

use crate::util::buf_ext::BufExt;

#[derive(Default, bevy::reflect::TypePath)]
pub struct O2Loader;

#[derive(Error, Debug)]
pub enum O2LoaderError {}

impl AssetLoader for O2Loader {
    type Asset = JMXVMAPO2;
    type Settings = ();
    type Error = O2LoaderError;
    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await.unwrap();
        let bytes = &buf;
        let mut buf = Bytes::copy_from_slice(bytes);
        let map_object = JMXVMAPO2::from(&mut buf);

        Ok(map_object)
    }

    fn extensions(&self) -> &[&str] {
        &["o2"]
    }
}

/// `JMXVMAPO1001` and friends: 12 ASCII bytes at the head of every file.
const SIGNATURE_LEN: usize = 12;
/// A region is a 6x6 grid of blocks.
const BLOCKS: usize = 36;
/// Each block carries four count-prefixed LoD groups (`docs/formats/mapo-jmxvmapo.md`).
const LOD_GROUPS: usize = 4;
/// Bytes per `.o2` object record (the `.o` one is 28 — it has no `RegionID`).
const RECORD_LEN: usize = 30;

#[derive(TypePath, Asset)]
pub struct JMXVMAPO2 {
    pub blocks: Vec<MapBlockData>,
}

/// Idea: a `.o2` is a 12-byte signature followed by 36 blocks, each of which is
/// **four** count-prefixed LoD groups of 30-byte records. Two traps, both hit
/// before (#287, #442):
///
/// 1. The signature must be skipped explicitly. The old reader had a
///    `if first_u16 != 0 { push empty block; continue }` branch that consumed
///    it *by accident* — six nonzero `u16`s ("JM XV MA PO 10 01") pushed six
///    empty blocks and landed at offset 12. It therefore parsed real blocks
///    0..29 into slots 6..35 and never read blocks 30..35 at all: 37,599
///    placements, 6,803 distinct world objects, silently missing.
/// 2. `bytes::Buf`'s getters panic on underflow, and this runs inside an asset
///    loader task, so a truncated or unexpected file would take the client
///    down. Every read here is bounds-checked and simply stops early instead.
impl From<&mut Bytes> for JMXVMAPO2 {
    fn from(buf: &mut Bytes) -> Self {
        let mut blocks = Vec::with_capacity(BLOCKS);
        if buf.remaining() < SIGNATURE_LEN {
            return Self { blocks };
        }
        buf.advance(SIGNATURE_LEN);

        for _ in 0..BLOCKS {
            let mut lod_groups = Vec::with_capacity(LOD_GROUPS);
            for _ in 0..LOD_GROUPS {
                if buf.remaining() < 2 {
                    blocks.push(MapBlockData { lod_groups });
                    return Self { blocks };
                }
                let object_count = buf.get_u16_le() as usize;
                if buf.remaining() < object_count * RECORD_LEN {
                    blocks.push(MapBlockData { lod_groups });
                    return Self { blocks };
                }
                let mut objects = Vec::with_capacity(object_count);
                for _ in 0..object_count {
                    objects.push(MapObject::from(&mut *buf));
                }
                lod_groups.push(LodGroup { objects })
            }
            blocks.push(MapBlockData { lod_groups });
        }

        Self { blocks }
    }
}

pub struct MapBlockData {
    pub lod_groups: Vec<LodGroup>,
}

pub struct LodGroup {
    pub objects: Vec<MapObject>,
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
    pub region_id: u16,
}

impl<T: Buf> From<&mut T> for MapObject {
    fn from(buf: &mut T) -> Self {
        Self {
            id: buf.get_u32_le(),
            position: buf.get_vec3(),
            is_static: buf.get_u16_le() == 0xFFFF,
            yaw: buf.get_f32_le(),
            uid: buf.get_u16_le(),
            short_0: buf.get_u16_le(),
            is_big: buf.get_u8() == 1,
            is_struct: buf.get_u8() == 1,
            region_id: buf.get_u16_le(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file shaped exactly like the ones on disk: the 12-byte signature, then
    /// 36 blocks x 4 count-prefixed LoD groups. `objects` places one record in
    /// the given (block, group).
    fn o2_file(place: Option<(usize, usize)>) -> Bytes {
        let mut out = Vec::new();
        out.extend_from_slice(b"JMXVMAPO1001");
        for block in 0..BLOCKS {
            for group in 0..LOD_GROUPS {
                let has = place == Some((block, group));
                out.extend_from_slice(&(has as u16).to_le_bytes());
                if has {
                    out.extend_from_slice(&7u32.to_le_bytes()); // id
                    for v in [1.0f32, 2.0, 3.0] {
                        out.extend_from_slice(&v.to_le_bytes());
                    }
                    out.extend_from_slice(&0xFFFFu16.to_le_bytes()); // is_static
                    out.extend_from_slice(&0.5f32.to_le_bytes()); // yaw
                    out.extend_from_slice(&9u16.to_le_bytes()); // uid
                    out.extend_from_slice(&0u16.to_le_bytes()); // short_0
                    out.push(1); // is_big
                    out.push(0); // is_struct
                    out.extend_from_slice(&0x6141u16.to_le_bytes()); // region id
                }
            }
        }
        Bytes::from(out)
    }

    /// #287/#442: the old reader consumed the signature by accident and shifted
    /// every block six slots, so the last six blocks of every region were never
    /// read. An object in the very last block/group must arrive, and the file
    /// must be consumed to the byte.
    #[test]
    fn the_last_block_is_read_and_the_file_is_consumed() {
        let mut buf = o2_file(Some((BLOCKS - 1, LOD_GROUPS - 1)));
        let parsed = JMXVMAPO2::from(&mut buf);

        assert_eq!(parsed.blocks.len(), BLOCKS);
        assert!(parsed
            .blocks
            .iter()
            .all(|b| b.lod_groups.len() == LOD_GROUPS));
        let objects: Vec<_> = parsed
            .blocks
            .iter()
            .flat_map(|b| b.lod_groups.iter())
            .flat_map(|g| g.objects.iter())
            .collect();
        assert_eq!(objects.len(), 1);
        assert_eq!(objects[0].id, 7);
        assert_eq!(objects[0].region_id, 0x6141);
        assert_eq!(buf.remaining(), 0, "the whole file must be consumed");
    }

    /// The signature's own bytes ("JM", "XV", …) are six nonzero `u16`s. Read as
    /// counts they demand ~19,786 records, which is what panicked the first
    /// re-land on all 4,506 corpus files.
    #[test]
    fn a_signature_is_never_read_as_an_object_count() {
        let mut buf = o2_file(None);
        let parsed = JMXVMAPO2::from(&mut buf);

        assert_eq!(parsed.blocks.len(), BLOCKS);
        assert!(parsed
            .blocks
            .iter()
            .flat_map(|b| b.lod_groups.iter())
            .all(|g| g.objects.is_empty()));
    }

    /// `bytes::Buf` panics on underflow and this runs in an asset-loader task,
    /// so every truncation must degrade to a short read.
    #[test]
    fn a_truncated_file_stops_instead_of_panicking() {
        let full = o2_file(Some((0, 2)));
        for len in 0..full.len() {
            let mut buf = full.slice(0..len);
            let parsed = JMXVMAPO2::from(&mut buf);
            assert!(parsed.blocks.len() <= BLOCKS);
        }
    }
}
