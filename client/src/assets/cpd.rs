use std::io::{Cursor, Seek, SeekFrom};
use std::path::{PathBuf, MAIN_SEPARATOR_STR};

use bevy::asset::{io::Reader, Asset, AssetLoader, LoadContext};
use bevy::reflect::TypePath;
use bytes::Buf;
use thiserror::Error;

use crate::util::buf_ext::BufExt;

/// The 12-byte signature every `.cpd` starts with.
const SIGNATURE: &str = "JMXVCPD 0101";
/// Bytes the fixed part of the header occupies after the signature: seven
/// `u32`s plus the `Type`/`Category` `i16` pair, before the name.
const HEADER_FIXED_LEN: usize = 8 * 4;
/// Bytes of the two trailing header `u32`s after the name.
const HEADER_TAIL_LEN: usize = 2 * 4;
/// Smallest possible resource entry: a `u32` length prefix and nothing else.
const MIN_RESOURCE_LEN: usize = 4;

#[derive(TypePath, Asset, Clone)]
#[allow(dead_code)]
pub struct JMXVCPD {
    pub header: Header,
    /// Per-compound collision geometry, present on `Type == 2` compounds only
    /// (69 of the 377 corpus files, all `CompoundObject`). Parsed but not yet
    /// consumed — object collision does not exist in the client yet (#290).
    pub collision_mesh: PathBuf,
    pub resources: Vec<PathBuf>,
}

#[derive(Clone)]
#[allow(dead_code)]
pub struct Header {
    pub collision_offset: u32,
    pub resource_offset: u32,
    pub unknown0: u32,
    pub unknown1: u32,
    pub unknown2: u32,
    pub unknown3: u32,
    pub unknown4: u32,
    /// `ObjectGeneralInfo.Type` — the low half of the old `object_type` u32
    /// (`Common/ObjectGeneralInfo.cs:18`). `0` = CompoundCharacter (275 corpus
    /// files), `2` = CompoundObject (102), and only `2` carries a collision
    /// mesh.
    pub object_type: i16,
    /// `ObjectGeneralInfo.Category` — the high half (`…cs:19`); `3` (compound)
    /// in every corpus file.
    pub object_category: i16,
    pub object_name: String,
    pub unknown5: u32,
    pub unknown6: u32,
}

#[derive(Default, bevy::reflect::TypePath)]
pub struct CpdLoader;

#[derive(Error, Debug)]
pub enum CpdLoaderError {
    #[error("could not read the .cpd file: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a JMXVCPD file: signature was {0:?}")]
    BadSignature(String),
    #[error("truncated .cpd: {needed} byte(s) needed at offset {at}, file is {len} byte(s)")]
    Truncated { at: u64, needed: usize, len: usize },
}

/// Parse a `.cpd` (compound object descriptor).
///
/// Idea: a 12-byte signature, a header whose two `u32` offsets point at the
/// collision-mesh path and at the resource list, and those two blocks. The
/// offsets and the list count come from the file, so every one of them is
/// validated before it is followed: `bytes::Buf`'s getters panic on underflow
/// and a bogus count would otherwise spin a multi-billion iteration loop, both
/// inside an asset-loader task (#290 — the loader used to `unwrap` the read,
/// discard the signature and seek unguarded, so it could only panic, never
/// degrade).
fn parse(bytes: &[u8]) -> Result<JMXVCPD, CpdLoaderError> {
    let len = bytes.len();
    let mut cursor = Cursor::new(bytes);

    if len < SIGNATURE.len() {
        return Err(CpdLoaderError::Truncated {
            at: 0,
            needed: SIGNATURE.len(),
            len,
        });
    }
    let signature = cursor.get_fixed_size_string(SIGNATURE.len());
    if signature != SIGNATURE {
        return Err(CpdLoaderError::BadSignature(signature));
    }

    need(&cursor, HEADER_FIXED_LEN, len)?;
    let collision_offset = cursor.get_u32_le();
    let resource_offset = cursor.get_u32_le();
    let unknown0 = cursor.get_u32_le();
    let unknown1 = cursor.get_u32_le();
    let unknown2 = cursor.get_u32_le();
    let unknown3 = cursor.get_u32_le();
    let unknown4 = cursor.get_u32_le();
    let object_type = cursor.get_i16_le();
    let object_category = cursor.get_i16_le();
    let object_name = read_string(&mut cursor, len)?;
    need(&cursor, HEADER_TAIL_LEN, len)?;
    let header = Header {
        collision_offset,
        resource_offset,
        unknown0,
        unknown1,
        unknown2,
        unknown3,
        unknown4,
        object_type,
        object_category,
        object_name,
        unknown5: cursor.get_u32_le(),
        unknown6: cursor.get_u32_le(),
    };

    seek_to(&mut cursor, header.collision_offset, len)?;
    let collision_mesh = read_path(&mut cursor, len)?;

    seek_to(&mut cursor, header.resource_offset, len)?;
    need(&cursor, 4, len)?;
    let count = cursor.get_u32_le() as usize;
    // A resource entry cannot be shorter than its own length prefix, so a count
    // the remaining bytes cannot possibly hold is a corrupt file, not a long one.
    if count.saturating_mul(MIN_RESOURCE_LEN) > cursor.remaining() {
        return Err(CpdLoaderError::Truncated {
            at: cursor.position(),
            needed: count.saturating_mul(MIN_RESOURCE_LEN),
            len,
        });
    }
    let mut resources = Vec::with_capacity(count);
    for _ in 0..count {
        resources.push(read_path(&mut cursor, len)?);
    }

    Ok(JMXVCPD {
        header,
        collision_mesh,
        resources,
    })
}

/// A `u32`-length-prefixed string, with the declared length validated before it
/// is read: `BufExt`'s helpers clamp a too-long string instead of failing, which
/// would turn a truncated file into a silently short value.
fn read_string(cursor: &mut Cursor<&[u8]>, len: usize) -> Result<String, CpdLoaderError> {
    need(cursor, 4, len)?;
    let declared = cursor.get_u32_le() as usize;
    need(cursor, declared, len)?;
    Ok(cursor.get_fixed_size_string(declared))
}

/// The same, normalised into a path the way `BufExt::get_path_buf_double_len`
/// does (SRO writes Windows separators).
fn read_path(cursor: &mut Cursor<&[u8]>, len: usize) -> Result<PathBuf, CpdLoaderError> {
    let raw = read_string(cursor, len)?;
    Ok(PathBuf::from(
        raw.replace(r"\\", r"\").replace(r"\", MAIN_SEPARATOR_STR),
    ))
}

fn need(cursor: &Cursor<&[u8]>, bytes: usize, len: usize) -> Result<(), CpdLoaderError> {
    if cursor.remaining() < bytes {
        return Err(CpdLoaderError::Truncated {
            at: cursor.position(),
            needed: bytes,
            len,
        });
    }
    Ok(())
}

/// Follow a file-supplied offset, refusing one that points past the end.
fn seek_to(cursor: &mut Cursor<&[u8]>, offset: u32, len: usize) -> Result<(), CpdLoaderError> {
    if offset as usize > len {
        return Err(CpdLoaderError::Truncated {
            at: offset as u64,
            needed: 1,
            len,
        });
    }
    cursor.seek(SeekFrom::Start(offset as u64))?;
    Ok(())
}

impl AssetLoader for CpdLoader {
    type Asset = JMXVCPD;
    type Settings = ();
    type Error = CpdLoaderError;
    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await?;
        parse(&buf)
    }

    fn extensions(&self) -> &[&str] {
        &["cpd"]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal but real-shaped `.cpd`: signature, header, then the collision
    /// path and the resource list at the offsets the header names.
    fn cpd_file(object_type: i16, collision: &str, resources: &[&str]) -> Vec<u8> {
        fn push_string(out: &mut Vec<u8>, s: &str) {
            out.extend_from_slice(&(s.len() as u32).to_le_bytes());
            out.extend_from_slice(s.as_bytes());
        }

        let name = "compound";
        let header_len = SIGNATURE.len() + HEADER_FIXED_LEN + 4 + name.len() + HEADER_TAIL_LEN;
        let collision_offset = header_len as u32;
        let resource_offset = collision_offset + 4 + collision.len() as u32;

        let mut out = Vec::new();
        out.extend_from_slice(SIGNATURE.as_bytes());
        out.extend_from_slice(&collision_offset.to_le_bytes());
        out.extend_from_slice(&resource_offset.to_le_bytes());
        for v in 0..5u32 {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&object_type.to_le_bytes());
        out.extend_from_slice(&3i16.to_le_bytes()); // category: compound
        push_string(&mut out, name);
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(out.len(), header_len);
        push_string(&mut out, collision);
        out.extend_from_slice(&(resources.len() as u32).to_le_bytes());
        for r in resources {
            push_string(&mut out, r);
        }
        out
    }

    /// `Type` and `Category` are two `i16`s (`ObjectGeneralInfo.cs:18-19`), not
    /// one `u32`: the corpus' `0x00030002` is Type 2 (CompoundObject) with
    /// Category 3, and only Type 2 carries a collision mesh.
    #[test]
    fn type_and_category_are_two_fields() {
        let bytes = cpd_file(2, "collision.bms", &["a.bsr"]);
        let parsed = parse(&bytes).expect("parses");

        assert_eq!(parsed.header.object_type, 2);
        assert_eq!(parsed.header.object_category, 3);
        assert_eq!(parsed.header.object_name, "compound");
        assert_eq!(parsed.collision_mesh, PathBuf::from("collision.bms"));
        assert_eq!(parsed.resources, vec![PathBuf::from("a.bsr")]);
    }

    /// The loader used to discard the signature entirely, so any file at all
    /// was decoded as a compound.
    #[test]
    fn a_wrong_signature_is_an_error() {
        let mut bytes = cpd_file(0, "", &[]);
        bytes[0..12].copy_from_slice(b"JMXVBSR 0110");

        assert!(matches!(
            parse(&bytes),
            Err(CpdLoaderError::BadSignature(_))
        ));
    }

    /// Offsets and the resource count come from the file, and the getters panic
    /// on underflow — inside an asset-loader task. Every truncation must become
    /// an error instead.
    #[test]
    fn a_truncated_file_is_an_error_not_a_panic() {
        let full = cpd_file(2, "collision.bms", &["a.bsr", "b.bsr"]);
        for len in 0..full.len() {
            assert!(parse(&full[..len]).is_err(), "len {} must not parse", len);
        }
    }

    /// A file-supplied offset past the end, and a resource count larger than the
    /// file can hold, are corrupt input — not a seek into nowhere and not a
    /// multi-billion iteration loop.
    #[test]
    fn out_of_range_offsets_and_counts_are_errors() {
        let mut bytes = cpd_file(2, "collision.bms", &["a.bsr"]);
        let resource_offset_at = SIGNATURE.len() + 4;
        bytes[resource_offset_at..resource_offset_at + 4]
            .copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        assert!(matches!(
            parse(&bytes),
            Err(CpdLoaderError::Truncated { .. })
        ));

        let mut bytes = cpd_file(2, "collision.bms", &["a.bsr"]);
        let count_at = bytes.len() - (4 + 4 + "a.bsr".len());
        bytes[count_at..count_at + 4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        assert!(matches!(
            parse(&bytes),
            Err(CpdLoaderError::Truncated { .. })
        ));
    }
}
