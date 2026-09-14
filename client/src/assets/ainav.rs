//! AINavData (`data://navmesh/ainavdata_<region>.dat`) — the precomputed
//! all-pairs dungeon routing tables (`AI_NAVIGATION`): per block a
//! cell→cell→edge lookup, block→block sub-goal cells, and per-edge centroids.
//!
//! This is the *server/mob-AI* acceleration structure; the original client
//! renders and navigates dungeons entirely from the `.dof` (voxel grid +
//! per-block BmsNavMesh), so nothing at runtime consumes this asset yet —
//! it exists as a verified loader for later autopath/AI work. Cell indices
//! may be computed on a retriangulated simplified mesh and need not match
//! the original `.bms` cells (`docs/formats/ainavdata.md`).
//!
//! Parsing is EOF-exact by construction: trailing bytes are an error, which
//! is how the corpus probe proves the layout.

use bevy::asset::{io::Reader, Asset, AssetLoader, LoadContext};
use bevy::math::Vec3;
use bevy::reflect::TypePath;
use thiserror::Error;

use crate::util::binread::{BinReadError, Cur};

#[derive(Error, Debug)]
pub enum AinavError {
    #[error("unsupported AINavData version {0} (expected 1)")]
    BadVersion(u8),
    #[error(transparent)]
    Read(#[from] BinReadError),
    #[error("{remaining} trailing bytes after SimpleDungeonData (offset {at})")]
    TrailingBytes { at: usize, remaining: usize },
}

/// One `CellLookupTable` entry: the ref-edge to use pathing forwards
/// (start→goal) and backwards (goal→start).
#[derive(Debug, Clone, Copy)]
pub struct RefCell {
    pub edge_forward: i16,
    pub edge_backward: i16,
}

/// Off-block link: leave through `cell_id` towards `linked_obj_id`'s global
/// edge `linked_obj_ref_edge_index`. The wire `u32 ID` equals `cell_id` in
/// the whole corpus and is not stored.
#[derive(Debug, Clone, Copy)]
pub struct RefBlockLink {
    pub cell_id: u16,
    pub linked_obj_id: u16,
    pub linked_obj_ref_edge_index: u16,
}

#[derive(Debug, Clone)]
pub struct RefBlock {
    pub index: u32,
    pub cell_count: u32,
    pub edge_count: u32,
    /// `cell_count × cell_count` entries, goal-major (row = goal cell,
    /// column = start cell, matching the file order).
    pub cell_lookup: Vec<RefCell>,
    pub links: Vec<RefBlockLink>,
}

#[derive(Debug, Clone)]
pub struct SimpleDungeonBlock {
    /// 3D centroid of every RefBlock edge, index-aligned (corpus-proven:
    /// counts match RefBlock.edge_count in all blocks).
    pub edge_centers: Vec<Vec3>,
}

#[derive(Debug, Clone, TypePath, Asset)]
pub struct AINavData {
    pub region_id: u16,
    pub blocks: Vec<RefBlock>,
    /// `blocks.len() × blocks.len()` entries, goal-major: the cell to use as
    /// sub-goal when pathing start-block → goal-block. The diagonal
    /// (start == goal) is unzeroed garbage in the data.
    pub block_lookup: Vec<i16>,
    /// From the SimpleDungeonData section (same region id, same block count).
    pub simple_blocks: Vec<SimpleDungeonBlock>,
    /// Whether SimpleDungeonData started exactly where RefDungeon ended
    /// (true across the whole corpus; recorded for the probe).
    pub sections_contiguous: bool,
}

pub fn parse(data: &[u8]) -> Result<AINavData, AinavError> {
    let mut cur = Cur::new(data);
    let version = cur.u8("version")?;
    if version != 1 {
        return Err(AinavError::BadVersion(version));
    }
    let simple_offset = cur.u32("simpleDungeonDataOffset")?;

    let region_id = cur.u16("refDungeon.regionID")?;
    let block_count = cur.u32("refDungeon.blockCount")?;
    cur.plausible("ref blocks", block_count, 16)?;
    let mut blocks = Vec::with_capacity(block_count as usize);
    for _ in 0..block_count {
        let index = cur.u32("block.Index")?;
        let cell_count = cur.u32("block.CellCount")?;
        let edge_count = cur.u32("block.EdgeCount")?;
        let table_len = (cell_count as usize).saturating_mul(cell_count as usize);
        cur.plausible(
            "cell lookup entries",
            table_len.min(u32::MAX as usize) as u32,
            4,
        )?;
        let mut cell_lookup = Vec::with_capacity(table_len);
        for _ in 0..table_len {
            cell_lookup.push(RefCell {
                edge_forward: cur.i16("refEdgeIndex0")?,
                edge_backward: cur.i16("refEdgeIndex1")?,
            });
        }
        let link_count = cur.u32("linkCount")?;
        cur.plausible("block links", link_count, 10)?;
        let mut links = Vec::with_capacity(link_count as usize);
        for _ in 0..link_count {
            let _id = cur.u32("link.ID")?; // == CellID in the whole corpus
            links.push(RefBlockLink {
                cell_id: cur.u16("link.CellID")?,
                linked_obj_id: cur.u16("link.LinkedObjID")?,
                linked_obj_ref_edge_index: cur.u16("link.LinkedObjRefEdgeIndex")?,
            });
        }
        blocks.push(RefBlock {
            index,
            cell_count,
            edge_count,
            cell_lookup,
            links,
        });
    }

    let block_table_len = (block_count as usize).saturating_mul(block_count as usize);
    cur.plausible(
        "block lookup entries",
        block_table_len.min(u32::MAX as usize) as u32,
        2,
    )?;
    let mut block_lookup = Vec::with_capacity(block_table_len);
    for _ in 0..block_table_len {
        block_lookup.push(cur.i16("blockLookup.refCellID")?);
    }
    let _int0 = cur.u32("int0")?;

    let sections_contiguous = cur.pos == simple_offset as usize;
    cur.seek("SimpleDungeonData", simple_offset)?;
    let _region_id2 = cur.u16("simple.regionID")?;
    let simple_block_count = cur.u32("simple.blockCount")?;
    cur.plausible("simple blocks", simple_block_count, 4)?;
    let mut simple_blocks = Vec::with_capacity(simple_block_count as usize);
    for _ in 0..simple_block_count {
        let edge_count = cur.u32("simple.edgeCount")?;
        cur.plausible("edge centers", edge_count, 12)?;
        let mut edge_centers = Vec::with_capacity(edge_count as usize);
        for _ in 0..edge_count {
            edge_centers.push(cur.vec3("edgeCenter")?);
        }
        simple_blocks.push(SimpleDungeonBlock { edge_centers });
    }
    let _int1 = cur.u32("int1")?;

    if cur.remaining() != 0 {
        return Err(AinavError::TrailingBytes {
            at: cur.pos,
            remaining: cur.remaining(),
        });
    }

    Ok(AINavData {
        region_id,
        blocks,
        block_lookup,
        simple_blocks,
        sections_contiguous,
    })
}

#[derive(Default, bevy::reflect::TypePath)]
pub struct AinavLoader;

#[derive(Error, Debug)]
pub enum AinavLoaderError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("AINavData parse error: {0}")]
    Parse(#[from] AinavError),
}

impl AssetLoader for AinavLoader {
    type Asset = AINavData;
    type Settings = ();
    type Error = AinavLoaderError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await?;
        Ok(parse(&buf)?)
    }

    fn extensions(&self) -> &[&str] {
        // Two corpus files ship as uppercase `.DAT`; Bevy extension matching
        // is case-sensitive, so register both spellings.
        &["dat", "DAT"]
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// Synthetic 2-block file: block 0 has 2 cells (4 lookup entries) and one
    /// link, block 1 has 1 cell; contiguous SimpleDungeonData with matching
    /// edge counts.
    fn build_fixture(trailing: bool) -> Vec<u8> {
        let mut w: Vec<u8> = Vec::new();
        w.push(1); // version
        let offset_at = w.len();
        w.extend_from_slice(&0u32.to_le_bytes()); // patched below

        w.extend_from_slice(&0x8001u16.to_le_bytes()); // regionID
        w.extend_from_slice(&2u32.to_le_bytes()); // blockCount

        // block 0
        w.extend_from_slice(&0u32.to_le_bytes()); // Index
        w.extend_from_slice(&2u32.to_le_bytes()); // CellCount
        w.extend_from_slice(&3u32.to_le_bytes()); // EdgeCount
        for (fwd, back) in [(0i16, 1i16), (2, -1), (-1, 2), (1, 0)] {
            w.extend_from_slice(&fwd.to_le_bytes());
            w.extend_from_slice(&back.to_le_bytes());
        }
        w.extend_from_slice(&1u32.to_le_bytes()); // linkCount
        w.extend_from_slice(&1u32.to_le_bytes()); // link.ID (== CellID)
        w.extend_from_slice(&1u16.to_le_bytes()); // CellID
        w.extend_from_slice(&1u16.to_le_bytes()); // LinkedObjID
        w.extend_from_slice(&0u16.to_le_bytes()); // LinkedObjRefEdgeIndex

        // block 1
        w.extend_from_slice(&1u32.to_le_bytes());
        w.extend_from_slice(&1u32.to_le_bytes());
        w.extend_from_slice(&1u32.to_le_bytes());
        w.extend_from_slice(&0i16.to_le_bytes());
        w.extend_from_slice(&0i16.to_le_bytes());
        w.extend_from_slice(&0u32.to_le_bytes()); // no links

        // BlockLookupTable 2x2
        for v in [0i16, 1, 1, 0] {
            w.extend_from_slice(&v.to_le_bytes());
        }
        w.extend_from_slice(&0u32.to_le_bytes()); // int0

        let simple_offset = w.len() as u32;
        w[offset_at..offset_at + 4].copy_from_slice(&simple_offset.to_le_bytes());
        w.extend_from_slice(&0x8001u16.to_le_bytes());
        w.extend_from_slice(&2u32.to_le_bytes());
        // block 0: 3 edge centers
        w.extend_from_slice(&3u32.to_le_bytes());
        for i in 0..3 {
            for c in [i as f32, 0.0, -(i as f32)] {
                w.extend_from_slice(&c.to_le_bytes());
            }
        }
        // block 1: 1 edge center
        w.extend_from_slice(&1u32.to_le_bytes());
        for c in [7.0f32, 8.0, 9.0] {
            w.extend_from_slice(&c.to_le_bytes());
        }
        w.extend_from_slice(&0u32.to_le_bytes()); // int1

        if trailing {
            w.push(0xAA);
        }
        w
    }

    #[test]
    fn parses_fixture_eof_exact() {
        let data = parse(&build_fixture(false)).unwrap();
        assert_eq!(data.region_id, 0x8001);
        assert!(data.sections_contiguous);
        assert_eq!(data.blocks.len(), 2);
        assert_eq!(data.blocks[0].cell_count, 2);
        assert_eq!(data.blocks[0].cell_lookup.len(), 4);
        assert_eq!(data.blocks[0].cell_lookup[1].edge_forward, 2);
        assert_eq!(data.blocks[0].cell_lookup[1].edge_backward, -1);
        assert_eq!(data.blocks[0].links.len(), 1);
        assert_eq!(data.blocks[0].links[0].linked_obj_id, 1);
        assert_eq!(data.blocks[1].cell_lookup.len(), 1);
        assert_eq!(data.block_lookup, vec![0, 1, 1, 0]);
        assert_eq!(data.simple_blocks.len(), 2);
        assert_eq!(data.simple_blocks[0].edge_centers.len(), 3);
        assert_eq!(
            data.simple_blocks[1].edge_centers[0],
            Vec3::new(7.0, 8.0, 9.0)
        );
        // Edge-count alignment invariant from the corpus.
        assert_eq!(
            data.simple_blocks[0].edge_centers.len() as u32,
            data.blocks[0].edge_count
        );
    }

    #[test]
    fn rejects_trailing_bytes() {
        let err = parse(&build_fixture(true)).unwrap_err();
        assert!(matches!(
            err,
            AinavError::TrailingBytes { remaining: 1, .. }
        ));
    }

    #[test]
    fn rejects_bad_version() {
        let err = parse(&[2, 0, 0, 0, 0]).unwrap_err();
        assert!(matches!(err, AinavError::BadVersion(2)));
    }
}
