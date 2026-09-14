//! JMXVDOF 0101 (`*.dof`) byte-level parser — a dungeon as a list of placed
//! room-block instances plus a 200×200×200-unit voxel index over them.
//!
//! Sections are located by the eight header offsets and parsed independently
//! (seek-by-offset, robust to physical reordering) rather than sequentially.
//! Strings are u32-length CP949 (decoded via the shared textdata decoder).
//! One corpus file (`dunhwang_cv1.dof`, unreferenced) uses a legacy block
//! layout with a single flag byte where the modern one has `has_height_fog` +
//! `unk_byte1`; the variant is detected by whether the block section ends
//! exactly at the next section's offset. Layout and corpus invariants:
//! `docs/formats/dof-jmxvdof.md`.
//!
//! Everything here is pure `&[u8] -> Result` with bounds-checked reads so the
//! corpus probe (`tools/src/bin/dungeon_scan`) can report violations instead
//! of panicking.

use bevy::asset::Asset;
use bevy::math::Vec3;
use bevy::reflect::TypePath;
use thiserror::Error;

use crate::util::binread::{BinReadError, Cur};

pub const DOF_SIGNATURE: &[u8; 12] = b"JMXVDOF 0101";

#[derive(Error, Debug)]
pub enum DofError {
    #[error("bad signature {0:?} (expected \"JMXVDOF 0101\")")]
    BadSignature(String),
    #[error(transparent)]
    Read(#[from] BinReadError),
    #[error(
        "block section ends at {modern_end:#x} (modern) / {legacy_end:#x} (legacy), \
         expected {expected:#x} — neither block layout matches"
    )]
    BlockLayoutMismatch {
        modern_end: usize,
        legacy_end: usize,
        expected: usize,
    },
}

/// Packed voxel coordinate: X in bits 0-9, Z in bits 10-19, Y in bits 20-29
/// (X, Z, Y order — Y highest — per RSBot `DungeonVoxelID` and the wiki).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VoxelId(pub u32);

impl VoxelId {
    pub fn pack(x: u32, y: u32, z: u32) -> Self {
        VoxelId((x & 0x3FF) | ((z & 0x3FF) << 10) | ((y & 0x3FF) << 20))
    }

    pub fn x(self) -> u32 {
        self.0 & 0x3FF
    }

    pub fn z(self) -> u32 {
        (self.0 >> 10) & 0x3FF
    }

    pub fn y(self) -> u32 {
        (self.0 >> 20) & 0x3FF
    }
}

/// Voxel edge length in world units (RSBot `DungeonVoxel.Width/Height/Length`).
pub const VOXEL_SIZE: f32 = 200.0;

#[derive(Debug, Clone)]
pub struct ObjGeneralInfo {
    pub type_id: i16,
    pub category: i16,
    pub name: String,
    pub unk0: u32,
    pub unk1: u32,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DofBoundingBox {
    pub min: Vec3,
    pub max: Vec3,
}

#[derive(Debug, Clone)]
pub struct FogParam {
    /// BGRA/ARGB packed color as stored; consumer decides channel order.
    pub color: u32,
    pub near_plane: f32,
    pub far_plane: f32,
    pub intensity: f32,
    /// Present when `has_height_fog == 1` (modern layout only). Semantics of
    /// the four floats are UNKNOWN (`docs/formats/dof-jmxvdof.md`).
    pub height_fog: Option<[f32; 4]>,
}

/// Payload behind `unk_byte1 == 2`; semantics UNKNOWN.
#[derive(Debug, Clone)]
pub struct UnkBlockPayload {
    pub vec0: Vec3,
    pub vec1: Vec3,
    pub unk: u32,
}

#[derive(Debug, Clone)]
pub struct DofBlockObject {
    pub name: String,
    /// Room prop resource, `*.bsr`. Corpus-verified order: Name first, Path
    /// second (the string ending in `.bsr`).
    pub path: String,
    pub position: Vec3,
    pub rotation: Vec3,
    pub scale: Vec3,
    /// 0 = none, 2 = collision object, 4 = water object (never combined in
    /// the corpus).
    pub flag: u32,
    pub unk0: u32,
    /// Collision circle radius is `sqrt(radius_sqrt)`.
    pub radius_sqrt: f32,
    /// Present when `flag & 4` (water object).
    pub water_color: Option<u32>,
}

impl DofBlockObject {
    pub const FLAG_COLLISION: u32 = 2;
    pub const FLAG_WATER: u32 = 4;

    pub fn is_collision_only(&self) -> bool {
        self.flag & Self::FLAG_COLLISION != 0
    }

    pub fn is_water(&self) -> bool {
        self.flag & Self::FLAG_WATER != 0
    }
}

#[derive(Debug, Clone)]
pub struct DofBlockLight {
    pub name: String,
    pub position: Vec3,
    pub diffuse: Vec3,
    pub ambient: Vec3,
    pub specular: Vec3,
    /// D3D-style attenuation coefficients (constant, linear, quadratic).
    pub attenuation: [f32; 3],
}

#[derive(Debug, Clone)]
pub struct DofBlock {
    /// Room resource (`*.bsr` compound) carrying the block's visual meshes and
    /// its walkable `BmsNavMesh`.
    pub path: String,
    pub name: String,
    pub unk_uint0: u32,
    pub position: Vec3,
    /// Block placed by `RotationY(-yaw)` then `Translate(position)`.
    pub yaw: f32,
    pub is_entrance: u32,
    /// **Block-local** bounds (measured: 0/151 Donwhang boxes sit near their
    /// block's position) — lift through `Translate(Position)·RotY(-Yaw)`
    /// before comparing against dungeon-frame positions.
    pub collision_box: DofBoundingBox,
    pub unk_uint1: u32,
    pub fog: FogParam,
    pub unk_byte1: u8,
    pub unk_byte1_payload: Option<UnkBlockPayload>,
    pub unk_string: String,
    /// Index into [`JMXVDOF::room_names`].
    pub room_index: u32,
    /// Index into [`JMXVDOF::floor_names`] — drives the minimap floor readout.
    pub floor_index: u32,
    /// Walkable neighbour blocks (navigation hand-off).
    pub connected_block_indices: Vec<u32>,
    /// Blocks visible from this one (portal-based occlusion culling).
    pub visible_block_indices: Vec<u32>,
    /// `dwColObjCount` — how many of `objects` are collision-relevant.
    pub collision_object_count: u32,
    pub objects: Vec<DofBlockObject>,
    pub lights: Vec<DofBlockLight>,
}

#[derive(Debug, Clone)]
pub struct DofVoxel {
    pub id: VoxelId,
    pub block_indices: Vec<u32>,
}

#[derive(Debug, Clone, Default)]
pub struct DofGrid {
    /// Grid extents in voxels (X, Y, Z respectively); world origin is
    /// `collision_box0.min`.
    pub width: u32,
    pub height: u32,
    pub length: u32,
    /// Only non-empty voxels are stored.
    pub voxels: Vec<DofVoxel>,
}

#[derive(Debug, Clone)]
pub struct DofGroup {
    pub name: String,
    pub flag: u32,
    pub block_indices: Vec<u32>,
}

#[derive(Debug, Clone, TypePath, Asset)]
pub struct JMXVDOF {
    pub info: ObjGeneralInfo,
    /// `0x8000 | dungeonInfoId` for live dungeons, 0 for unwired files. IDs
    /// collide in the corpus — `dungeoninfo.txt` paths are authoritative.
    pub region_id: u16,
    /// `min` is the voxel-grid origin.
    pub collision_box0: DofBoundingBox,
    pub collision_box1: DofBoundingBox,
    pub blocks: Vec<DofBlock>,
    /// One entry per block (cached neighbour lists; `links.len() == blocks.len()`
    /// throughout the corpus).
    pub links: Vec<Vec<u32>>,
    pub grid: DofGrid,
    pub room_names: Vec<String>,
    pub floor_names: Vec<String>,
    pub groups: Vec<DofGroup>,
    /// True for the legacy single-flag-byte block layout (`dunhwang_cv1.dof`).
    pub legacy_block_layout: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum BlockLayout {
    Modern,
    Legacy,
}

fn read_bbox(cur: &mut Cur, what: &'static str) -> Result<DofBoundingBox, BinReadError> {
    Ok(DofBoundingBox {
        min: cur.vec3(what)?,
        max: cur.vec3(what)?,
    })
}

pub fn parse(data: &[u8]) -> Result<JMXVDOF, DofError> {
    let mut cur = Cur::new(data);
    let signature = cur.take("signature", 12)?;
    if signature != DOF_SIGNATURE {
        return Err(DofError::BadSignature(
            String::from_utf8_lossy(signature).into_owned(),
        ));
    }

    let block_offset = cur.u32("header.BlockOffset")?;
    let link_offset = cur.u32("header.LinkOffset")?;
    let grid_offset = cur.u32("header.GridOffset")?;
    let group_offset = cur.u32("header.GroupOffset")?;
    let label_offset = cur.u32("header.LabelOffset")?;
    let _offset5 = cur.u32("header.Offset5")?;
    let _offset6 = cur.u32("header.Offset6")?;
    let bounding_box_offset = cur.u32("header.BoundingBoxOffset")?;

    // ObjGeneralInfo + RegionID follow the header sequentially, not via offset.
    let info = ObjGeneralInfo {
        type_id: cur.i16("objInfo.Type")?,
        category: cur.i16("objInfo.Category")?,
        name: cur.string_cp949("objInfo.Name")?,
        unk0: cur.u32("objInfo.unk0")?,
        unk1: cur.u32("objInfo.unk1")?,
    };
    let region_id = cur.u16("dungeon.RegionID")?;

    cur.seek("bounding boxes", bounding_box_offset)?;
    let collision_box0 = read_bbox(&mut cur, "CollisionBox0")?;
    let collision_box1 = read_bbox(&mut cur, "CollisionBox1")?;

    // Block section, with layout-variant detection: the section must end
    // exactly at the next section's offset (the oracle from the corpus work).
    let expected_end = [
        link_offset,
        grid_offset,
        group_offset,
        label_offset,
        bounding_box_offset,
    ]
    .into_iter()
    .filter(|&o| o != 0 && o > block_offset)
    .min()
    .map(|o| o as usize)
    .unwrap_or(data.len());

    cur.seek("blocks", block_offset)?;
    let (blocks, legacy) = match parse_blocks(&mut cur, BlockLayout::Modern) {
        Ok(blocks) if cur.pos == expected_end => (blocks, false),
        modern => {
            let modern_end = cur.pos;
            cur.seek("blocks (legacy retry)", block_offset)?;
            match parse_blocks(&mut cur, BlockLayout::Legacy) {
                Ok(blocks) if cur.pos == expected_end => (blocks, true),
                Ok(_) => {
                    return Err(DofError::BlockLayoutMismatch {
                        modern_end,
                        legacy_end: cur.pos,
                        expected: expected_end,
                    })
                }
                // Legacy also failed to parse: surface the modern error if
                // there was one (the common corpus shape), else the legacy one.
                Err(legacy_err) => return Err(modern.err().unwrap_or(legacy_err)),
            }
        }
    };

    cur.seek("grid", grid_offset)?;
    let grid = parse_grid(&mut cur)?;

    cur.seek("links", link_offset)?;
    let link_count = cur.u32("linkCount")?;
    cur.plausible("links", link_count, 4)?;
    let mut links = Vec::with_capacity(link_count as usize);
    for _ in 0..link_count {
        links.push(cur.u32_indices("link.blockIndices")?);
    }

    let (room_names, floor_names) = if label_offset != 0 {
        cur.seek("labels", label_offset)?;
        let room_count = cur.u32("roomCounter")?;
        cur.plausible("room names", room_count, 4)?;
        let mut room_names = Vec::with_capacity(room_count as usize);
        for _ in 0..room_count {
            room_names.push(cur.string_cp949("roomName")?);
        }
        // Floor-name length is u32 like room names (the wiki's u16 is a doc
        // error — see docs/formats/dof-jmxvdof.md).
        let floor_count = cur.u32("floorCounter")?;
        cur.plausible("floor names", floor_count, 4)?;
        let mut floor_names = Vec::with_capacity(floor_count as usize);
        for _ in 0..floor_count {
            floor_names.push(cur.string_cp949("floorName")?);
        }
        (room_names, floor_names)
    } else {
        (Vec::new(), Vec::new())
    };

    cur.seek("groups", group_offset)?;
    let group_count = cur.u32("blockGroupCount")?;
    cur.plausible("groups", group_count, 12)?;
    let mut groups = Vec::with_capacity(group_count as usize);
    for _ in 0..group_count {
        groups.push(DofGroup {
            name: cur.string_cp949("group.Name")?,
            flag: cur.u32("group.Flag")?,
            block_indices: cur.u32_indices("group.BlockIndices")?,
        });
    }

    Ok(JMXVDOF {
        info,
        region_id,
        collision_box0,
        collision_box1,
        blocks,
        links,
        grid,
        room_names,
        floor_names,
        groups,
        legacy_block_layout: legacy,
    })
}

fn parse_blocks(cur: &mut Cur, layout: BlockLayout) -> Result<Vec<DofBlock>, DofError> {
    let block_count = cur.u32("dunBlockCnt")?;
    cur.plausible("blocks", block_count, 64)?;
    let mut blocks = Vec::with_capacity(block_count as usize);
    for _ in 0..block_count {
        blocks.push(parse_block(cur, layout)?);
    }
    Ok(blocks)
}

fn parse_block(cur: &mut Cur, layout: BlockLayout) -> Result<DofBlock, DofError> {
    let path = cur.string_cp949("block.Path")?;
    let name = cur.string_cp949("block.Name")?;
    let unk_uint0 = cur.u32("block.unkUInt0")?;
    let position = cur.vec3("block.Position")?;
    let yaw = cur.f32("block.Yaw")?;
    let is_entrance = cur.u32("block.IsEntrance")?;
    let collision_box = read_bbox(cur, "block.CollisionBox0")?;
    let unk_uint1 = cur.u32("block.unkUInt1")?;

    let color = cur.u32("fog.Color")?;
    let near_plane = cur.f32("fog.NearPlane")?;
    let far_plane = cur.f32("fog.FarPlane")?;
    let intensity = cur.f32("fog.Intensity")?;

    // Modern layout: u8 has_height_fog (+4 floats), u8 unk_byte1 (+payload on
    // ==2). Legacy layout (dunhwang_cv1.dof): the height-fog byte is absent
    // and the single flag byte plays the unk_byte1 role.
    let (height_fog, unk_byte1) = match layout {
        BlockLayout::Modern => {
            let has_height_fog = cur.u8("fog.hasHeightFog")?;
            let height_fog = if has_height_fog == 1 {
                Some([
                    cur.f32("fog.unkFloat3")?,
                    cur.f32("fog.unkFloat4")?,
                    cur.f32("fog.unkFloat5")?,
                    cur.f32("fog.unkFloat6")?,
                ])
            } else {
                None
            };
            (height_fog, cur.u8("block.unkByte1")?)
        }
        BlockLayout::Legacy => (None, cur.u8("block.flag (legacy)")?),
    };
    let unk_byte1_payload = if unk_byte1 == 2 {
        Some(UnkBlockPayload {
            vec0: cur.vec3("block.unkVector0")?,
            vec1: cur.vec3("block.unkVector1")?,
            unk: cur.u32("block.unkUInt2")?,
        })
    } else {
        None
    };

    let unk_string = cur.string_cp949("block.unkString")?;
    let room_index = cur.u32("block.RoomIndex")?;
    let floor_index = cur.u32("block.FloorIndex")?;
    let connected_block_indices = cur.u32_indices("block.ConnectedBlockIndices")?;
    let visible_block_indices = cur.u32_indices("block.VisibleBlockIndices")?;

    let obj_count = cur.u32("dwObjCount")?;
    let collision_object_count = cur.u32("dwColObjCount")?;
    cur.plausible("block objects", obj_count, 64)?;
    let mut objects = Vec::with_capacity(obj_count as usize);
    for _ in 0..obj_count {
        let name = cur.string_cp949("obj.Name")?;
        let path = cur.string_cp949("obj.Path")?;
        let position = cur.vec3("obj.Position")?;
        let rotation = cur.vec3("obj.Rotation")?;
        let scale = cur.vec3("obj.Scale")?;
        let flag = cur.u32("obj.Flag")?;
        let unk0 = cur.u32("obj.Int0")?;
        let radius_sqrt = cur.f32("obj.RadiusSqrt")?;
        let water_color = if flag & DofBlockObject::FLAG_WATER != 0 {
            Some(cur.u32("obj.WaterColor")?)
        } else {
            None
        };
        objects.push(DofBlockObject {
            name,
            path,
            position,
            rotation,
            scale,
            flag,
            unk0,
            radius_sqrt,
            water_color,
        });
    }

    let light_count = cur.u32("lightCount")?;
    cur.plausible("block lights", light_count, 64)?;
    let mut lights = Vec::with_capacity(light_count as usize);
    for _ in 0..light_count {
        lights.push(DofBlockLight {
            name: cur.string_cp949("light.Name")?,
            position: cur.vec3("light.Position")?,
            diffuse: cur.vec3("light.Diffuse")?,
            ambient: cur.vec3("light.Ambient")?,
            specular: cur.vec3("light.Specular")?,
            attenuation: [
                cur.f32("light.Atten0")?,
                cur.f32("light.Atten1")?,
                cur.f32("light.Atten2")?,
            ],
        });
    }

    Ok(DofBlock {
        path,
        name,
        unk_uint0,
        position,
        yaw,
        is_entrance,
        collision_box,
        unk_uint1,
        fog: FogParam {
            color,
            near_plane,
            far_plane,
            intensity,
            height_fog,
        },
        unk_byte1,
        unk_byte1_payload,
        unk_string,
        room_index,
        floor_index,
        connected_block_indices,
        visible_block_indices,
        collision_object_count,
        objects,
        lights,
    })
}

fn parse_grid(cur: &mut Cur) -> Result<DofGrid, DofError> {
    let width = cur.u32("gridWidth")?;
    let height = cur.u32("gridHeight")?;
    let length = cur.u32("gridLength")?;
    let voxel_count = cur.u32("gridVoxelCount")?;
    cur.plausible("grid voxels", voxel_count, 8)?;
    let mut voxels = Vec::with_capacity(voxel_count as usize);
    for _ in 0..voxel_count {
        let id = VoxelId(cur.u32("voxel.ID")?);
        let block_indices = cur.u32_indices("voxel.blockIndices")?;
        voxels.push(DofVoxel { id, block_indices });
    }
    Ok(DofGrid {
        width,
        height,
        length,
        voxels,
    })
}

#[cfg(test)]
mod test {
    use super::*;

    /// Byte-buffer builder for synthetic DOF fixtures.
    struct W(Vec<u8>);

    impl W {
        fn new() -> Self {
            W(Vec::new())
        }
        fn u8(&mut self, v: u8) -> &mut Self {
            self.0.push(v);
            self
        }
        fn u16(&mut self, v: u16) -> &mut Self {
            self.0.extend_from_slice(&v.to_le_bytes());
            self
        }
        fn i16(&mut self, v: i16) -> &mut Self {
            self.0.extend_from_slice(&v.to_le_bytes());
            self
        }
        fn u32(&mut self, v: u32) -> &mut Self {
            self.0.extend_from_slice(&v.to_le_bytes());
            self
        }
        fn f32(&mut self, v: f32) -> &mut Self {
            self.0.extend_from_slice(&v.to_le_bytes());
            self
        }
        fn vec3(&mut self, x: f32, y: f32, z: f32) -> &mut Self {
            self.f32(x).f32(y).f32(z)
        }
        fn string(&mut self, s: &[u8]) -> &mut Self {
            self.u32(s.len() as u32);
            self.0.extend_from_slice(s);
            self
        }
        fn pos(&self) -> u32 {
            self.0.len() as u32
        }
        fn patch_u32(&mut self, at: usize, v: u32) {
            self.0[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }
    }

    /// One block in the modern layout: no height fog, no unk payload, one
    /// water object, one light, connected to block 1, sees blocks 0 and 1.
    fn write_modern_block(w: &mut W) {
        w.string(b"dungeon\\china\\room.bsr");
        w.string(b"room-a");
        w.u32(0); // unkUInt0
        w.vec3(10.0, 0.0, 20.0);
        w.f32(1.5); // yaw
        w.u32(0); // IsEntrance
        w.vec3(-5.0, -1.0, -5.0).vec3(5.0, 3.0, 5.0); // collision box
        w.u32(7); // unkUInt1
        w.u32(0xFF334455).f32(10.0).f32(500.0).f32(0.5); // fog
        w.u8(0); // hasHeightFog
        w.u8(0); // unkByte1
        w.string(b""); // unkString
        w.u32(0).u32(1); // room / floor index
        w.u32(1).u32(1); // connected: [1]
        w.u32(2).u32(0).u32(1); // visible: [0, 1]
        w.u32(1).u32(1); // objCount, colObjCount
        w.string(b"water01");
        w.string(b"dungeon\\china\\water.bsr");
        w.vec3(1.0, 2.0, 3.0)
            .vec3(0.0, 0.0, 0.0)
            .vec3(1.0, 1.0, 1.0);
        w.u32(4); // Flag: water
        w.u32(0);
        w.f32(400.0); // RadiusSqrt
        w.u32(0xAABBCCDD); // WaterColor (flag & 4)
        w.u32(1); // lightCount
        w.string(b"light01");
        w.vec3(0.0, 2.0, 0.0);
        w.vec3(1.0, 0.9, 0.8)
            .vec3(0.1, 0.1, 0.1)
            .vec3(0.0, 0.0, 0.0);
        w.f32(1.0).f32(0.01).f32(0.0);
    }

    /// A complete synthetic modern-layout file with 1 block, 1 voxel, labels
    /// and one group. Returns the bytes.
    fn build_modern_fixture() -> Vec<u8> {
        let mut w = W::new();
        w.0.extend_from_slice(DOF_SIGNATURE);
        let header_at = w.pos() as usize;
        for _ in 0..8 {
            w.u32(0); // offsets patched below
        }
        // ObjGeneralInfo + RegionID (sequential)
        w.i16(-1).i16(4);
        w.string(b"Noname");
        w.u32(u32::MAX).u32(u32::MAX);
        w.u16(0x8001);

        let bbox_offset = w.pos();
        w.vec3(-100.0, -10.0, -100.0).vec3(300.0, 50.0, 300.0);
        w.vec3(0.0, 0.0, 0.0).vec3(0.0, 0.0, 0.0);

        let block_offset = w.pos();
        w.u32(1);
        write_modern_block(&mut w);

        let grid_offset = w.pos();
        w.u32(2).u32(1).u32(2); // width, height, length
        w.u32(1); // one stored voxel
        w.u32(VoxelId::pack(1, 0, 1).0);
        w.u32(1).u32(0); // block indices: [0]

        let link_offset = w.pos();
        w.u32(1); // linkCount == blockCount
        w.u32(1).u32(1); // link: [1]

        let label_offset = w.pos();
        w.u32(1);
        w.string(b"Chamber");
        w.u32(2);
        w.string(b"B1");
        w.string(b"B2");

        let group_offset = w.pos();
        w.u32(1);
        w.string(b"main");
        w.u32(1);
        w.u32(1).u32(0);

        // header order: Block, Link, Grid, Group, Label, 0, 0, BBox
        for (i, v) in [
            block_offset,
            link_offset,
            grid_offset,
            group_offset,
            label_offset,
            0,
            0,
            bbox_offset,
        ]
        .into_iter()
        .enumerate()
        {
            w.patch_u32(header_at + i * 4, v);
        }
        w.0
    }

    #[test]
    fn parses_modern_fixture() {
        let dof = parse(&build_modern_fixture()).unwrap();
        assert!(!dof.legacy_block_layout);
        assert_eq!(dof.region_id, 0x8001);
        assert_eq!(dof.info.type_id, -1);
        assert_eq!(dof.info.category, 4);
        assert_eq!(dof.info.name, "Noname");
        assert_eq!(dof.collision_box0.min, Vec3::new(-100.0, -10.0, -100.0));

        assert_eq!(dof.blocks.len(), 1);
        let block = &dof.blocks[0];
        assert_eq!(block.path, "dungeon\\china\\room.bsr");
        assert_eq!(block.position, Vec3::new(10.0, 0.0, 20.0));
        assert_eq!(block.yaw, 1.5);
        assert_eq!(block.fog.color, 0xFF334455);
        assert!(block.fog.height_fog.is_none());
        assert_eq!(block.floor_index, 1);
        assert_eq!(block.connected_block_indices, vec![1]);
        assert_eq!(block.visible_block_indices, vec![0, 1]);

        assert_eq!(block.objects.len(), 1);
        let obj = &block.objects[0];
        assert!(obj.is_water());
        assert!(!obj.is_collision_only());
        assert_eq!(obj.path, "dungeon\\china\\water.bsr");
        assert_eq!(obj.water_color, Some(0xAABBCCDD));
        assert_eq!(obj.radius_sqrt, 400.0);

        assert_eq!(block.lights.len(), 1);
        assert_eq!(block.lights[0].position, Vec3::new(0.0, 2.0, 0.0));
        assert_eq!(block.lights[0].attenuation, [1.0, 0.01, 0.0]);

        assert_eq!(dof.links, vec![vec![1]]);
        assert_eq!(dof.grid.width, 2);
        assert_eq!(dof.grid.voxels.len(), 1);
        assert_eq!(dof.grid.voxels[0].id, VoxelId::pack(1, 0, 1));
        assert_eq!(dof.grid.voxels[0].block_indices, vec![0]);

        assert_eq!(dof.room_names, vec!["Chamber"]);
        assert_eq!(dof.floor_names, vec!["B1", "B2"]);
        assert_eq!(dof.groups.len(), 1);
        assert_eq!(dof.groups[0].name, "main");
        assert_eq!(dof.groups[0].block_indices, vec![0]);
    }

    #[test]
    fn parses_height_fog_and_unk_payload() {
        // Same fixture but with hasHeightFog == 1 and unkByte1 == 2, rebuilt
        // from scratch since the payloads change every later offset.
        let mut w = W::new();
        w.0.extend_from_slice(DOF_SIGNATURE);
        let header_at = w.pos() as usize;
        for _ in 0..8 {
            w.u32(0);
        }
        w.i16(-1).i16(4);
        w.string(b"Noname");
        w.u32(0).u32(0);
        w.u16(0x8003);

        let bbox_offset = w.pos();
        for _ in 0..12 {
            w.f32(0.0);
        }

        let block_offset = w.pos();
        w.u32(1);
        w.string(b"a.bsr").string(b"a");
        w.u32(0);
        w.vec3(0.0, 0.0, 0.0);
        w.f32(0.0);
        w.u32(0);
        w.vec3(0.0, 0.0, 0.0).vec3(0.0, 0.0, 0.0);
        w.u32(0);
        w.u32(0).f32(0.0).f32(0.0).f32(0.0); // fog
        w.u8(1); // hasHeightFog == 1
        w.f32(1.0).f32(2.0).f32(3.0).f32(4.0);
        w.u8(2); // unkByte1 == 2
        w.vec3(5.0, 6.0, 7.0).vec3(8.0, 9.0, 10.0).u32(42);
        w.string(b"");
        w.u32(0).u32(0);
        w.u32(0); // connected
        w.u32(0); // visible
        w.u32(0).u32(0); // objects
        w.u32(0); // lights

        let grid_offset = w.pos();
        w.u32(1).u32(1).u32(1).u32(0);
        let link_offset = w.pos();
        w.u32(1).u32(0);
        let group_offset = w.pos();
        w.u32(0);

        for (i, v) in [
            block_offset,
            link_offset,
            grid_offset,
            group_offset,
            0,
            0,
            0,
            bbox_offset,
        ]
        .into_iter()
        .enumerate()
        {
            w.patch_u32(header_at + i * 4, v);
        }

        let dof = parse(&w.0).unwrap();
        let block = &dof.blocks[0];
        assert_eq!(block.fog.height_fog, Some([1.0, 2.0, 3.0, 4.0]));
        assert_eq!(block.unk_byte1, 2);
        let payload = block.unk_byte1_payload.as_ref().unwrap();
        assert_eq!(payload.vec0, Vec3::new(5.0, 6.0, 7.0));
        assert_eq!(payload.vec1, Vec3::new(8.0, 9.0, 10.0));
        assert_eq!(payload.unk, 42);
        // No labels section (offset 0).
        assert!(dof.room_names.is_empty() && dof.floor_names.is_empty());
    }

    #[test]
    fn detects_legacy_block_layout() {
        // Legacy layout: single flag byte replaces hasHeightFog + unkByte1.
        // The modern parse of this data mis-consumes a byte and misses the
        // section-end oracle, triggering the legacy retry.
        let mut w = W::new();
        w.0.extend_from_slice(DOF_SIGNATURE);
        let header_at = w.pos() as usize;
        for _ in 0..8 {
            w.u32(0);
        }
        w.i16(-1).i16(4);
        w.string(b"Noname");
        w.u32(0).u32(0);
        w.u16(0);

        let bbox_offset = w.pos();
        for _ in 0..12 {
            w.f32(0.0);
        }

        let block_offset = w.pos();
        w.u32(1);
        w.string(b"a.bsr").string(b"a");
        w.u32(0);
        w.vec3(0.0, 0.0, 0.0);
        w.f32(0.0);
        w.u32(0);
        w.vec3(0.0, 0.0, 0.0).vec3(0.0, 0.0, 0.0);
        w.u32(0);
        w.u32(0).f32(0.0).f32(0.0).f32(0.0); // fog
        w.u8(2); // legacy flag byte == 2 → payload
        w.vec3(1.0, 1.0, 1.0).vec3(2.0, 2.0, 2.0).u32(9);
        w.string(b"");
        w.u32(0).u32(0);
        w.u32(0);
        w.u32(0);
        w.u32(0).u32(0);
        w.u32(0);

        let grid_offset = w.pos();
        w.u32(1).u32(1).u32(1).u32(0);
        let link_offset = w.pos();
        w.u32(1).u32(0);
        let group_offset = w.pos();
        w.u32(0);

        for (i, v) in [
            block_offset,
            link_offset,
            grid_offset,
            group_offset,
            0,
            0,
            0,
            bbox_offset,
        ]
        .into_iter()
        .enumerate()
        {
            w.patch_u32(header_at + i * 4, v);
        }

        let dof = parse(&w.0).unwrap();
        assert!(dof.legacy_block_layout);
        let block = &dof.blocks[0];
        assert_eq!(block.unk_byte1, 2);
        assert!(block.fog.height_fog.is_none());
        assert_eq!(block.unk_byte1_payload.as_ref().unwrap().unk, 9);
    }

    #[test]
    fn voxel_id_round_trip() {
        // X low, Z middle, Y high — the packing order is easy to get wrong.
        let id = VoxelId::pack(3, 7, 5);
        assert_eq!((id.x(), id.y(), id.z()), (3, 7, 5));
        assert_eq!(id.0, 3 | (5 << 10) | (7 << 20));
        // 10-bit fields mask cleanly at the boundary.
        let max = VoxelId::pack(1023, 1023, 1023);
        assert_eq!((max.x(), max.y(), max.z()), (1023, 1023, 1023));
        assert_eq!(max.0 & 0xC000_0000, 0); // reserved bits untouched
    }

    #[test]
    fn rejects_bad_signature() {
        let err = parse(b"JMXVNVM 1000....................").unwrap_err();
        assert!(matches!(err, DofError::BadSignature(_)));
    }

    #[test]
    fn rejects_truncated_file() {
        let mut fixture = build_modern_fixture();
        fixture.truncate(fixture.len() - 3);
        assert!(parse(&fixture).is_err());
    }

    #[test]
    fn decodes_cp949_strings() {
        // "한글" in CP949 inside a name string (NUL-padded).
        let mut w = W::new();
        w.u32(6);
        w.0.extend_from_slice(&[0xC7, 0xD1, 0xB1, 0xDB, 0x00, 0x00]);
        let mut cur = Cur::new(&w.0);
        assert_eq!(cur.string_cp949("name").unwrap(), "\u{d55c}\u{ae00}");
        assert_eq!(cur.pos, 10); // consumed padding too
    }
}
