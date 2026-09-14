// https://github.com/DummkopfOfHachtenduden/SilkroadDoc/wiki/JMXVRES

use std::io::{Cursor, Seek, SeekFrom};
use std::path::PathBuf;

use crate::assets::ban::JMXVBAN;
use bevy::prelude::{Asset, Handle, Vec2};
use bevy::reflect::TypePath;
use bytes::Buf;

use crate::assets::bsr::collision_mesh::CollisionMesh;
use crate::assets::bsr::BsrLoaderError;
use crate::util::buf_ext::BufExt;

#[derive(TypePath, Asset, Default)]
#[allow(dead_code)]
pub struct JMXVRES {
    pub header: ResourceHeader,
    pub object_info: ObjectInfo,
    pub collision_mesh: CollisionMesh,
    pub materials: Vec<MaterialData>,
    pub mesh: Vec<MeshData>,
    pub animation: AnimationData,
    pub skeleton: Option<(PathBuf, String)>,
    pub primitive_group: Vec<PrimitiveGroupData>,
    pub primitive_animation_group: Vec<PrimitiveAnimationGroupData>,
    pub system_mod_set: Vec<ModSet>,
    pub animation_mod_set: Vec<ModSet>,
    // pub unknown_buf: [u8; 40]
}

#[derive(Default)]
#[allow(dead_code)]
pub struct ResourceHeader {
    pub material_offset: u32,
    pub mesh_offset: u32,
    pub skeleton_offset: u32,
    pub animation_offset: u32,
    pub prim_mesh_group_offset: u32,
    pub prim_animation_group_offset: u32,
    pub mod_palette_offset: u32,
    pub collision_offset: u32,
    pub prim_mesh_flag: u32,
    pub mod_data_flag: u32,
    pub int1: u32,
    pub int2: u32,
    pub int3: u32,
}

impl<T: Buf> From<&mut T> for ResourceHeader {
    fn from(value: &mut T) -> Self {
        Self {
            material_offset: value.get_u32_le(),
            mesh_offset: value.get_u32_le(),
            skeleton_offset: value.get_u32_le(),
            animation_offset: value.get_u32_le(),
            prim_mesh_group_offset: value.get_u32_le(),
            prim_animation_group_offset: value.get_u32_le(),
            mod_palette_offset: value.get_u32_le(),
            collision_offset: value.get_u32_le(),
            prim_mesh_flag: value.get_u32_le(),
            mod_data_flag: value.get_u32_le(),
            int1: value.get_u32_le(),
            int2: value.get_u32_le(),
            int3: value.get_u32_le(),
        }
    }
}

#[derive(Default, Debug)]
#[allow(dead_code)]
pub struct ObjectInfo {
    pub typ: u32,
    pub name: String,
    pub unknown_0: u32,
    pub unknown_1: u32,
}

impl<T: Buf + BufExt> From<&mut T> for ObjectInfo {
    fn from(value: &mut T) -> Self {
        Self {
            typ: value.get_u32_le(),
            name: value.get_double_len_string(),
            unknown_0: value.get_u32_le(),
            unknown_1: value.get_u32_le(),
        }
    }
}

#[derive(Default, Debug)]
#[allow(dead_code)]
pub struct MaterialData {
    pub id: u32,
    pub path: PathBuf,
}

#[derive(Default, Clone)]
#[allow(dead_code)]
pub struct MeshData {
    pub path: PathBuf,
    pub unknown: Option<u32>,
}

#[derive(Default)]
#[allow(dead_code)]
pub struct AnimationData {
    pub(crate) type_version: u32,
    pub(crate) type_user_define: u32,
    pub(crate) animations: Vec<Handle<JMXVBAN>>,
}

#[derive(Default)]
#[allow(dead_code)]
pub struct SkeletonData {
    path: PathBuf,
    bone_path: PathBuf,
}

#[derive(Default)]
pub struct PrimitiveGroupData {
    pub name: String,
    pub files_indices: Vec<u32>,
}

/// Animation set of a resource, named after the weapon class it applies
/// to ("default", "sword", "spear", "bow", "onehand_sword", ...). Items
/// and simple objects only ever carry a "default" group.
#[derive(Default)]
pub struct PrimitiveAnimationGroupData {
    pub group_name: String,
    pub animations: Vec<PrimitiveAnimationTypeData>,
}

#[derive(Default)]
#[allow(dead_code)]
pub struct PrimitiveAnimationTypeData {
    /// animation type id (0 = stand, 1 = walk, 6 = battle stance, 7 = run, ...)
    pub typ: u32,
    /// index into the resource's animation list, `u32::MAX` = none
    pub file_index: u32,
    pub events: Vec<PrimitiveAnimationEvent>,
    pub walk_length: f32,
    pub walk_points: Vec<Vec2>,
}

#[derive(Default)]
#[allow(dead_code)]
pub struct PrimitiveAnimationEvent {
    pub key_time: u32,
    pub typ: u32,
    pub index: u32,
    pub unknown: u32,
}

/// A fully parsed .bsr with every sub-asset kept as its pk2-root-relative
/// path (backslash separators, as stored). This is the Bevy-free view of a
/// resource: `BsrLoaderV2` turns the paths into asset handles, standalone
/// tools (bsr2glb) resolve and parse them directly.
#[derive(Default)]
#[allow(dead_code)]
pub struct ParsedBsr {
    pub header: ResourceHeader,
    pub object_info: ObjectInfo,
    pub collision_mesh: CollisionMesh,
    /// .bms paths, index-aligned with the indices in [`Self::primitive_group`].
    pub mesh_paths: Vec<PathBuf>,
    /// .bmt material sets `{id, path}`.
    pub material_sets: Vec<MaterialData>,
    /// .bsk path + attachment bone name. The bone names the *target*
    /// skeleton bone for attachable items (weapons), empty for characters.
    pub skeleton: Option<(PathBuf, String)>,
    pub animation_type_version: u32,
    pub animation_type_user_define: u32,
    /// .ban paths, index-aligned with the file indices in
    /// [`Self::primitive_animation_group`].
    pub animation_paths: Vec<PathBuf>,
    pub primitive_group: Vec<PrimitiveGroupData>,
    pub primitive_animation_group: Vec<PrimitiveAnimationGroupData>,
    pub attach_info: Option<ResAttachInfo>,
    /// See [`crate::assets::bsr::resource::SroResource::alpha_is_sheen`].
    pub alpha_is_sheen: bool,
    pub sheen_alpha_test: bool,
    /// TexAni UV scroll entries (waterfalls, canal water).
    pub texani_mods: Vec<RawTexAniMod>,
    /// Advanced-material entries (D3D blend states).
    pub material_mods: Vec<RawMaterialMod>,
}

/// One AniGroup entry's walk graph: `count u32, length f32, points`.
///
/// The length comes **before** the points (JMX-File-Editor `PrimAniTypeData`
/// round-trip, the wiki, and our own `docs/formats/bsr-jmxvres.md`). Reading
/// the points first spans the same bytes, so it never fails to parse — it just
/// shifts every value by four: the real length lands in `points[0].x` and the
/// last point's y (≈1.0 almost everywhere) is mistaken for the length.
fn read_walk_graph<T: Buf>(cursor: &mut T) -> (f32, Vec<Vec2>) {
    let walk_point_count = cursor.get_u32_le();
    let walk_length = cursor.get_f32_le();
    let mut walk_points = Vec::with_capacity(walk_point_count as usize);
    for _ in 0..walk_point_count {
        walk_points.push(cursor.get_vec2());
    }
    (walk_length, walk_points)
}

/// Parses a .bsr file. Section order in the file is irrelevant — every
/// section is reached through its header offset.
pub fn parse_bsr(data: &[u8]) -> Result<ParsedBsr, BsrLoaderError> {
    let mut cursor = Cursor::new(data);

    let _sig = cursor.get_fixed_size_string(12);
    let header = ResourceHeader::from(&mut cursor);
    let object_info = ObjectInfo::from(&mut cursor);

    cursor
        .seek(SeekFrom::Start(header.collision_offset as u64))
        .map_err(|e| BsrLoaderError::Seek("collision", e))?;
    let collision_mesh = CollisionMesh::from(&mut cursor);

    cursor
        .seek(SeekFrom::Start(header.mesh_offset as u64))
        .map_err(|e| BsrLoaderError::Seek("mesh", e))?;
    let mesh_count = cursor.get_u32_le();
    let mut mesh_paths = Vec::with_capacity(mesh_count as usize);
    for _ in 0..mesh_count {
        mesh_paths.push(cursor.get_path_buf_double_len());
        if header.prim_mesh_flag & 1 != 0 {
            let _unknown = cursor.get_u32_le();
        }
    }

    cursor
        .seek(SeekFrom::Start(header.material_offset as u64))
        .map_err(|e| BsrLoaderError::Seek("material", e))?;
    let material_count = cursor.get_u32_le();
    let mut material_sets = Vec::with_capacity(material_count as usize);
    for _ in 0..material_count {
        material_sets.push(MaterialData {
            id: cursor.get_u32_le(),
            path: cursor.get_path_buf_double_len(),
        });
    }

    cursor
        .seek(SeekFrom::Start(header.skeleton_offset as u64))
        .map_err(|e| BsrLoaderError::Seek("skeleton", e))?;
    let skeleton = if cursor.get_u32_le() == 1 {
        Some((
            cursor.get_path_buf_double_len(),
            cursor.get_double_len_string(),
        ))
    } else {
        None
    };

    cursor
        .seek(SeekFrom::Start(header.prim_mesh_group_offset as u64))
        .map_err(|e| BsrLoaderError::Seek("prim_mesh_group", e))?;
    let group_count = cursor.get_u32_le();
    let mut primitive_group = Vec::with_capacity(group_count as usize);
    for _ in 0..group_count {
        let name = cursor.get_double_len_string();
        let index_count = cursor.get_u32_le();
        let mut files_indices = Vec::with_capacity(index_count as usize);
        for _ in 0..index_count {
            files_indices.push(cursor.get_u32_le());
        }
        primitive_group.push(PrimitiveGroupData {
            name,
            files_indices,
        });
    }

    cursor
        .seek(SeekFrom::Start(header.animation_offset as u64))
        .map_err(|e| BsrLoaderError::Seek("animation", e))?;
    let animation_type_version = cursor.get_u32_le();
    let animation_type_user_define = cursor.get_u32_le();
    let anim_count = cursor.get_u32_le() as usize;
    let mut animation_paths = Vec::with_capacity(anim_count);
    for _ in 0..anim_count {
        animation_paths.push(cursor.get_path_buf_double_len());
    }

    cursor
        .seek(SeekFrom::Start(header.prim_animation_group_offset as u64))
        .map_err(|e| BsrLoaderError::Seek("prim_animation_group", e))?;
    let anim_group_count = cursor.get_u32_le();
    let mut primitive_animation_group = Vec::with_capacity(anim_group_count as usize);
    for _ in 0..anim_group_count {
        let group_name = cursor.get_double_len_string();
        let entry_count = cursor.get_u32_le();
        let mut group_animations = Vec::with_capacity(entry_count as usize);
        for _ in 0..entry_count {
            let typ = cursor.get_u32_le();
            let file_index = cursor.get_u32_le();
            let event_count = cursor.get_u32_le();
            let mut events = Vec::with_capacity(event_count as usize);
            for _ in 0..event_count {
                events.push(PrimitiveAnimationEvent {
                    key_time: cursor.get_u32_le(),
                    typ: cursor.get_u32_le(),
                    index: cursor.get_u32_le(),
                    unknown: cursor.get_u32_le(),
                });
            }
            let (walk_length, walk_points) = read_walk_graph(&mut cursor);
            group_animations.push(PrimitiveAnimationTypeData {
                typ,
                file_index,
                events,
                walk_length,
                walk_points,
            });
        }
        primitive_animation_group.push(PrimitiveAnimationGroupData {
            group_name,
            animations: group_animations,
        });
    }

    let attach_info =
        parse_attach_info(data, mesh_paths.len(), attach_form_is_item(object_info.typ));
    let envmap = envmap_mod_alpha_test(data, header.mod_palette_offset as usize);
    let texani_mods = parse_texani_mods(data, header.mod_palette_offset as usize);
    let material_mods = parse_material_mods(data, header.mod_palette_offset as usize);

    Ok(ParsedBsr {
        header,
        object_info,
        collision_mesh,
        mesh_paths,
        material_sets,
        skeleton,
        animation_type_version,
        animation_type_user_define,
        animation_paths,
        primitive_group,
        primitive_animation_group,
        attach_info,
        alpha_is_sheen: envmap.is_some(),
        sheen_alpha_test: envmap.unwrap_or(false),
        texani_mods,
        material_mods,
    })
}

#[derive(Default)]
#[allow(dead_code)]
pub struct ModSet {
    typ: u32,
    animation_type: u32,
    name: String,
    data: Vec<ModData>,
}

// TODO: first u32 is data type and second is a variable type depending on the data type.
//  see: https://github.com/DummkopfOfHachtenduden/SilkroadDoc/wiki/ModData
#[derive(Default)]
#[allow(dead_code)]
pub enum ModData {
    #[default]
    Unknown,
    Material,       // 0x0000 0000
    TexAni,         // 0x0001 0000
    MultiTex,       // 0x0001 0001
    MultiTexRev,    // 0x0001 0002
    Particle,       // 0x0003 0000
    EnvMap,         // 0x0004 0000
    BumpEnv,        // 0x0004 0001
    Sound,          // 0x0005 0000
    DynamicVertex,  // 0x0006 0000
    DynamicJoint,   // 0x0006 0001
    DynamicLattice, // 0x0006 0002
    ProgEquipPow,   // 0x0007 0000
}

// CResAttachable : CResObject, IResObject;
// if(objInfo.Type == ObjectType.Character || objInfo.Type == ObjectType.Attachable)
// {
// 4   uint    unkUInt0 //0 = CHAR, 1 = ITEM
// 4   uint    unkUInt1 //see below
// 4   uint    attachMethod //0 = BASE, 1 = REPLACE, 2 = ADD
// 4   uint    slotCount
// for (int i = 0; i < slotCount; i++)
// {
// 4   uint    slotId //see below
// 4   uint    slotMeshIdx //PrimMeshIdx
// }
//
// //CResChar: CResAttachable, CResObject, IResObject;
// if(objInfo.Type == ObjectType.Character)
// 4   uint    nComboNum               //0, "ASSERT(nComboNum == 0)"
// }

/// The CResAttachable block of a character or attachable item resource.
///
/// For characters the slots map a body part slot to the mesh holding the
/// default look of that part (naked torso, hair, ...). For items the
/// slots name the body part slots the item occupies.
#[derive(Debug, Clone, PartialEq)]
pub struct ResAttachInfo {
    /// unkUInt0: false = CHAR, true = ITEM
    pub is_item: bool,
    /// unkUInt1, see table below
    pub attach_kind: u32,
    /// 0 = BASE, 1 = REPLACE, 2 = ADD
    pub attach_method: u32,
    /// (slot id, mesh index into the resource's mesh list), see table below
    pub slots: Vec<(u32, u32)>,
}

/// ModData type tag of an environment map entry (`0x0004 0000`, see the
/// [`ModData`] table).
const MOD_DATA_ENVMAP: u32 = 0x0004_0000;

/// Checks whether the resource's "ambient" system mod set has an EnvMap
/// ModData entry among its mods, returning `Some(alpha_test)` when it
/// does.
///
/// The original engine repurposes the alpha channel of such resources as
/// an environment/sheen mask (weapons, metal armor), so their meshes have
/// to be rendered opaque instead of alpha-masked. Resources without the
/// marker (characters, hair, cloth) keep the engine-default alpha test.
///
/// The returned `alpha_test` is bit 2 of the flags dword at payload
/// offset 24: when set, `CRTModEnvMap::Apply` (sro_client.exe @0xc836b0)
/// enables `ALPHATESTENABLE` with `GREATEREQUAL` ref **1**, so texels with
/// alpha exactly 0 are cut out while everything else stays opaque sheen
/// (e.g. the degree-4/8 glaive blade cutouts; degree-1 shares the same
/// .bmt but has the bit clear).
///
/// The EnvMap entry is USUALLY the ambient set's first mod, but not
/// always: avatar/event items put Particle or Material mods first (china
/// shield_12..14), so the set's whole mod span — up to the next set
/// header — is scanned. Mod payloads are variable-length (Particle
/// entries embed path strings), so entries are found by a validated
/// byte scan rather than by walking slots: the set header around the
/// name is checked (like [`scan_mod_set_headers`]) and each candidate's
/// generic IModData header fields must hold, which rules out the
/// `00 00 04 00` patterns inside animation data and mod strings that a
/// bare tag scan false-positives on (character bodies got sheen-routed
/// whole). Census of Data.pk2: every real EnvMap lives in a
/// `(typ 2, "ambient")` system set, nearly all of them items. The rest
/// of the payload carries no usable per-resource strength: only the
/// flags dword and one blend-alpha-like byte vary (uniform sheen params
/// suffice).
pub fn envmap_mod_alpha_test(data: &[u8], mod_palette_offset: usize) -> Option<bool> {
    let region = data.get(mod_palette_offset..)?;
    let read_u32 = |pos: usize| -> Option<u32> {
        region
            .get(pos..pos + 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    };

    let set_headers = scan_mod_set_headers(region);

    let mut search = 0usize;
    while let Some(rel) = region
        .get(search..)?
        .windows(7)
        .position(|w| w == b"ambient")
    {
        let name_pos = search + rel;
        search = name_pos + 1;

        // set header: u32 typ | u32 animType | u32 nameLen | name | u32 modCount
        let Some(header) = name_pos.checked_sub(12) else {
            continue;
        };
        if read_u32(header) != Some(2) || read_u32(header + 8) != Some(7) {
            continue;
        }
        if !matches!(read_u32(name_pos + 7), Some(1..=16)) {
            continue;
        }
        // the set's mods span from its first-mod slot to the next set header
        let first_mod = name_pos + 7 + 4;
        let end = set_headers
            .iter()
            .map(|(pos, ..)| *pos)
            .find(|pos| *pos > name_pos)
            .unwrap_or(region.len());

        let mut pos = first_mod;
        // a candidate needs at least the tag and the Float0 field; the
        // later header fields are validated when present (real files may
        // truncate at the tail, see envmap_truncated_flags_default_off)
        while pos + 8 <= end {
            let flags = (|| {
                if read_u32(pos)? != MOD_DATA_ENVMAP {
                    return None;
                }
                // the generic IModData header: Float0 (0.5 on every mod),
                // two small ints, mtrlIdx (-1 = all materials), a zero pad
                let strength =
                    f32::from_le_bytes(region.get(pos + 4..pos + 8)?.try_into().unwrap());
                if !(0.0..=1.0).contains(&strength) {
                    return None;
                }
                if read_u32(pos + 8).is_some_and(|v| v > 8)
                    || read_u32(pos + 12).is_some_and(|v| v >= 0x10000)
                    || read_u32(pos + 16).is_some_and(|v| !(-1..=63).contains(&(v as i32)))
                    || read_u32(pos + 20).is_some_and(|v| v != 0)
                {
                    return None;
                }
                Some(read_u32(pos + 24).unwrap_or(0))
            })();
            if let Some(flags) = flags {
                return Some(flags & 0x2 != 0);
            }
            pos += 1;
        }
    }
    None
}

/// A particle effect reference scanned from the mod palette (`0x0003 0000`
/// Particle ModData entries).
#[derive(Debug, Clone, PartialEq)]
pub struct RawParticleMod {
    /// Path relative to Particles.pk2, as stored (backslashes).
    pub path: String,
    /// Bone of the resource's *own* skeleton the effect is anchored to
    /// (e.g. the garment talisman's glow on its ward-tip bones), `None`
    /// for offset-anchored entries (lamp flames, death smoke).
    pub bone: Option<String>,
    /// Spawn position in resource-local space.
    pub offset: bevy::math::Vec3,
    /// Keytime into the owning set's animation, in milliseconds.
    pub delay_ms: u32,
    /// Uniform scale of the spawned effect *geometry*. The f32 right after
    /// the mod block's `0x0003 0000` tag: the original client stores it on
    /// the effect wrapper (+0xc0, creator 0xc816c0) and applies it via the
    /// effect's SetScale vfunc (0xcaa0b0) to every plate/instance — the
    /// factor between effect-local and world units (talisman ward glows
    /// author 0.5; without it they render 2x). It does NOT scale the attach
    /// `offset`: an earlier reading of the pre-step (0xc81190) claimed it
    /// did, but world data contradicts it (2026-07-30 — cj_lamp01's flame
    /// offset y=16.32 sits in the lantern cage of its 19.95-unit mesh only
    /// unscaled; halved by the ubiquitous Float0=0.5 it hangs mid-post).
    /// Non-finite/non-positive serialized values fall back to 1.0.
    pub scale: f32,
    /// The effect only shows at night (street/building lamps); byte 1 of
    /// the entry's trailing flag bytes per the ModData wiki, matching all
    /// observed lamp entries.
    pub night_only: bool,
    /// Owning mod set `(typ, animation_type, name)`, if a valid set header
    /// precedes the entry. See [`scan_mod_set_headers`] for the semantics.
    pub set: Option<(u32, u32, String)>,
}

/// ModData type tag of a texture animation entry (`0x0001 0000`, see the
/// [`ModData`] table).
const MOD_DATA_TEXANI: u32 = 0x0001_0000;

/// A texture UV animation scanned from the mod palette (`0x0001 0000`
/// TexAni ModData entries): waterfalls, dungeon canal water, ...
///
/// The entry stores a row-major 4x4 D3D texture-transform matrix that the
/// original client applies per frame scaled by time; in all observed data
/// only the classic D3D UV-translation slots `_31`/`_32` (row-major
/// indices 8/9) are nonzero, so the practical payload is a UV scroll
/// speed in uv/sec.
#[derive(Debug, Clone, PartialEq)]
pub struct RawTexAniMod {
    /// `JMXVBMT_MtrlIdx`: -1 = applies to every material of the active
    /// material set, otherwise an index into the .bmt's material list.
    pub mtrl_idx: i32,
    /// UV translation speed in uv/sec (matrix `_31`/`_32`). Waterfalls
    /// scroll V negative (down the sheet, -0.3..-2.78), dungeon canals
    /// scroll U (±0.04..0.07).
    pub uv_speed: bevy::math::Vec2,
    /// The matrix carried terms outside the translation slots
    /// (rotation/scale) — never observed in game data; callers should
    /// warn and still apply the translation.
    pub non_translation: bool,
    /// `ModDataTexAni.UnkUInt06` (+32) is set: the transform drives the
    /// **MultiTex second stage**, not the base diffuse. In 117 of the 118
    /// corpus entries that carry it the file also has a MultiTex
    /// (`0x0001 0001`) mod on the same `MtrlIdx`. openroad has no MultiTex
    /// consumer, so applying such a transform would scroll the wrong
    /// texture; the loader drops these entries.
    pub multi_tex_stage: bool,
    /// Owning mod set `(typ, animation_type, name)`, if a valid set header
    /// precedes the entry. See [`scan_mod_set_headers`] for the semantics.
    pub set: Option<(u32, u32, String)>,
}

/// The known ModData type tags (two little-endian u16s read as one u32),
/// used to validate mod set headers: a real header's mod count is followed
/// by its first mod's tag.
const MOD_DATA_TAGS: [u32; 12] = [
    0x0000_0000, // Material
    0x0001_0000, // TexAni
    0x0001_0001, // MultiTex
    0x0001_0002, // MultiTexRev
    0x0003_0000, // Particle
    0x0004_0000, // EnvMap
    0x0004_0001, // BumpEnv
    0x0005_0000, // Sound
    0x0006_0000, // DynamicVertex
    0x0006_0001, // DynamicJoint
    0x0006_0002, // DynamicLattice
    0x0007_0000, // ProgEquipPow
];

/// Scans the mod palette for mod set headers, returning
/// `(region offset, typ, animation_type, name)` per set.
///
/// Verified layout (chinaman_adventurer.bsr, isyutaru.bsr):
/// `u32 typ | u32 animationType | u32 nameLen | name | u32 modCount | mods`.
/// Set semantics by `typ`:
/// - 2: system set ("ambient"): its mods are always active,
/// - 1: animation-linked: `name` is an animation *group* name and
///   `animationType` the animation type id — the mods apply while that
///   animation plays (e.g. isyutaru's death smoke: group "default", type 4),
/// - 0: referenced externally by name (`animationType` = -1): transformed
///   movement-state variants ("chinaman_bogy_runforward") and per-hit skill
///   sets ("spear_chain_a_02"), presumably driven by skilldata/state logic.
///
/// Like the other palette scanners this validates candidates instead of
/// walking the palette (non-particle ModData payload sizes are unknown):
/// typ/animationType/name-charset/modCount are range-checked and the first
/// mod's type tag must be a known [`MOD_DATA_TAGS`] value.
pub fn scan_mod_set_headers(region: &[u8]) -> Vec<(usize, u32, u32, String)> {
    let read_u32 = |pos: usize| -> Option<u32> {
        region
            .get(pos..pos + 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    };

    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos + 16 <= region.len() {
        let candidate = (|| {
            let typ = read_u32(pos)?;
            if typ > 2 {
                return None;
            }
            let anim_type = read_u32(pos + 4)?;
            if anim_type != u32::MAX && anim_type >= 512 {
                return None;
            }
            let name_len = read_u32(pos + 8)? as usize;
            if !(1..=48).contains(&name_len) {
                return None;
            }
            let name = region.get(pos + 12..pos + 12 + name_len)?;
            if !name.iter().all(|&b| b.is_ascii_alphanumeric() || b == b'_') {
                return None;
            }
            let mod_count = read_u32(pos + 12 + name_len)?;
            if mod_count > 16 {
                return None;
            }
            if mod_count > 0 && !MOD_DATA_TAGS.contains(&read_u32(pos + 16 + name_len)?) {
                return None;
            }
            Some((
                typ,
                anim_type,
                String::from_utf8_lossy(name).to_string(),
                name_len,
            ))
        })();
        match candidate {
            Some((typ, anim_type, name, name_len)) => {
                out.push((pos, typ, anim_type, name));
                pos += 12 + name_len;
            }
            None => pos += 1,
        }
    }
    out
}

/// Scans the mod palette for particle effect entries.
///
/// Like [`envmap_mod_alpha_test`]/[`parse_attach_info`] this does not walk the
/// palette (entry sizes are unreliable); it finds `.efp` path strings and
/// validates the entry layout around them, verified against ALL 5853
/// Particle entries in Data.pk2 res/ (2026-07-12 census):
/// `u32 anchorMode | u32 pathLen | path | u32 boneLen | bone | Vec3 offset | u32 delayMs | u32`
/// `anchorMode` discriminates perfectly: 1 = anchored at `offset` (boneLen
/// always 0), 0 = anchored to the named bone of the resource's own skeleton
/// (boneLen always > 0, e.g. the garment talisman's ward-tip glows).
pub fn parse_particle_mods(data: &[u8], mod_palette_offset: usize) -> Vec<RawParticleMod> {
    let Some(region) = data.get(mod_palette_offset..) else {
        return Vec::new();
    };
    let read_u32 = |pos: usize| -> Option<u32> {
        region
            .get(pos..pos + 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    };
    let read_f32 = |pos: usize| read_u32(pos).map(f32::from_bits);

    let set_headers = scan_mod_set_headers(region);
    // the owning set of an entry is the nearest set header preceding it
    let owning_set = |entry_pos: usize| -> Option<(u32, u32, String)> {
        set_headers
            .iter()
            .take_while(|(pos, ..)| *pos < entry_pos)
            .last()
            .map(|(_, typ, anim_type, name)| (*typ, *anim_type, name.clone()))
    };

    // Per-mod uniform effect scale: the f32 right after each `0x0003 0000`
    // Particle tag (see `RawParticleMod::scale`). Like the set headers,
    // an entry's owning mod block is the nearest tag preceding it. The tag
    // bytes also occur as data inside other palette structures, so a bare
    // window match is not trusted: a tag counts when it sits at a validated
    // set header's first-mod position (`header + 16 + name_len`, the
    // position `scan_mod_set_headers` already range-checked — covers both
    // observed layouts, with and without the 0x30 block header), or when
    // the constant 0x30 block-size marker follows the float (later mods of
    // a multi-mod ambient set). Out-of-range floats degrade to 1.0.
    let structural_tags: std::collections::HashSet<usize> = set_headers
        .iter()
        .map(|(pos, _, _, name)| pos + 16 + name.len())
        .collect();
    let tag = 0x0003_0000u32.to_le_bytes();
    let mod_scales: Vec<(usize, f32)> = region
        .windows(4)
        .enumerate()
        .filter(|(_, w)| *w == tag)
        .filter(|(pos, _)| {
            structural_tags.contains(pos)
                || read_u32(pos + 8) == Some(0x30)
                || read_u32(pos + 12) == Some(0x30)
        })
        .filter_map(|(pos, _)| read_f32(pos + 4).map(|f| (pos, f)))
        .collect();
    let owning_scale = |entry_pos: usize| -> f32 {
        mod_scales
            .iter()
            .take_while(|(pos, _)| *pos < entry_pos)
            .last()
            .map(|&(_, f)| f)
            .filter(|f| f.is_finite() && *f > 0.01 && *f < 100.0)
            .unwrap_or(1.0)
    };

    let mut out = Vec::new();
    for path_end in region
        .windows(4)
        .enumerate()
        .filter(|(_, w)| *w == b".efp")
        .map(|(i, _)| i + 4)
        .collect::<Vec<_>>()
    {
        // Find the length prefix that ends exactly at the ".efp".
        let entry = (5..=256usize).find_map(|len| {
            let path_start = path_end.checked_sub(len)?;
            let len_pos = path_start.checked_sub(4)?;
            let anchor_pos = len_pos.checked_sub(4)?;
            if read_u32(len_pos)? as usize != len {
                return None;
            }
            let bone_anchored = match read_u32(anchor_pos)? {
                0 => true,
                1 => false,
                _ => return None,
            };
            let path_bytes = &region[path_start..path_end];
            if !path_bytes.iter().all(|&b| (0x20..0x7f).contains(&b)) {
                return None;
            }

            let bone_len = read_u32(path_end)? as usize;
            // the anchor mode discriminates the bone-name length without
            // exception in the census, so a mismatch is not a real entry
            if bone_anchored != (bone_len > 0) || bone_len > 32 {
                return None;
            }
            let bone_bytes = region.get(path_end + 4..path_end + 4 + bone_len)?;
            if !bone_bytes
                .iter()
                .all(|&b| b.is_ascii_alphanumeric() || b == b' ' || b == b'_')
            {
                return None;
            }
            let bone = bone_anchored.then(|| String::from_utf8_lossy(bone_bytes).to_string());

            let offset_pos = path_end + 4 + bone_len;
            let offset = bevy::math::Vec3::new(
                read_f32(offset_pos)?,
                read_f32(offset_pos + 4)?,
                read_f32(offset_pos + 8)?,
            );
            if !(offset.is_finite() && offset.abs().max_element() < 100_000.0) {
                return None;
            }
            let delay_ms = read_u32(offset_pos + 12)?;
            if delay_ms > 3_600_000 {
                return None;
            }
            // trailing flag bytes; byte 1 = "night time only" (lamps)
            let night_only = region.get(offset_pos + 17) == Some(&1);

            Some(RawParticleMod {
                path: String::from_utf8_lossy(path_bytes).to_string(),
                bone,
                offset,
                delay_ms,
                night_only,
                scale: owning_scale(anchor_pos),
                set: owning_set(anchor_pos),
            })
        });
        if let Some(entry) = entry {
            out.push(entry);
        }
    }
    out
}

/// A sound track referenced from the mod palette (`0x0005 0000` Sound
/// ModData entries) — the original's animation→SFX binding.
#[derive(Debug, Clone, PartialEq)]
pub struct RawSoundMod {
    /// Path relative to Data.pk2, as stored (backslashes, always `.wav`).
    pub path: String,
    /// Keytime into the owning set's animation, in milliseconds (0 = at the
    /// animation's first frame; corpus max 5,856 ms).
    pub key_time_ms: u32,
    /// The track's event name (`snd_swing_s1`, `voc_shout2`, `snd_run`, ...),
    /// empty on 602 of the 45,086 corpus tracks. Not a trigger — the keytime
    /// is — but it names what the track is and is kept for diagnostics.
    pub event: String,
    /// Owning mod set `(typ, animation_type, name)`, if a valid set header
    /// precedes the entry. See [`scan_mod_set_headers`] for the semantics.
    pub set: Option<(u32, u32, String)>,
}

/// Upper bound on a track keytime (ms) used to validate a candidate. The
/// corpus maximum is 5,856 ms; 60 s is a loose sanity bound, not a datum.
const MAX_SOUND_KEY_TIME_MS: u32 = 60_000;

/// Scans the mod palette for the sound tracks of `0x0005 0000` Sound entries.
///
/// Idea: a Sound ModData entry is a nested container
/// (`i32 nSndSetNum | 11 config dwords | per set {name, i32 nTrackNum, tracks}`,
/// `docs/re/formats/moddata-unhandled.md` §3), and the only part the client
/// needs to make noise is the innermost track:
/// `u32 hasValue | u32 pathLen | path(.wav) | i32 keyTimeMs | u32 eventLen | event`.
/// So — like [`parse_particle_mods`] with `.efp` — this anchors on the `.wav`
/// suffix and validates the record around it instead of walking the whole
/// nested payload, which keeps the scanner independent of the parts of the
/// container that are still `[U]` (the 11 config dwords).
///
/// Census over the user's Data.pk2 (7,688 `.bsr` reachable here): **615 files
/// / 45,086 tracks**, against the EOF-exact palette walk's 615 files / 45,098
/// tracks (99.97 %). The 12 missed tracks have CP949 (non-ASCII) paths that
/// our lossy path handling could not open anyway. Owning set types:
/// 39,783 animation-linked (typ 1), 5,300 external (typ 0), 3 ambient (typ 2).
pub fn parse_sound_mods(data: &[u8], mod_palette_offset: usize) -> Vec<RawSoundMod> {
    let Some(region) = data.get(mod_palette_offset..) else {
        return Vec::new();
    };
    let read_u32 = |pos: usize| -> Option<u32> {
        region
            .get(pos..pos + 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    };

    let set_headers = scan_mod_set_headers(region);
    // the owning set of a track is the nearest set header preceding it
    let owning_set = |entry_pos: usize| -> Option<(u32, u32, String)> {
        set_headers
            .iter()
            .take_while(|(pos, ..)| *pos < entry_pos)
            .last()
            .map(|(_, typ, anim_type, name)| (*typ, *anim_type, name.clone()))
    };

    let mut out = Vec::new();
    for path_end in region
        .windows(4)
        .enumerate()
        .filter(|(_, w)| *w == b".wav")
        .map(|(i, _)| i + 4)
        .collect::<Vec<_>>()
    {
        // Find the length prefix that ends exactly at the ".wav", with the
        // track's `hasValue` dword in front of it.
        let entry = (5..=256usize).find_map(|len| {
            let path_start = path_end.checked_sub(len)?;
            let len_pos = path_start.checked_sub(4)?;
            let has_value_pos = len_pos.checked_sub(4)?;
            if read_u32(len_pos)? as usize != len {
                return None;
            }
            // 0 = the track carries no path (nothing to play)
            if read_u32(has_value_pos)? != 1 {
                return None;
            }
            let path_bytes = &region[path_start..path_end];
            if !path_bytes.iter().all(|&b| (0x20..0x7f).contains(&b)) {
                return None;
            }

            let key_time_ms = read_u32(path_end)?;
            if key_time_ms > MAX_SOUND_KEY_TIME_MS {
                return None;
            }
            let event_len = read_u32(path_end + 4)? as usize;
            if event_len > 64 {
                return None;
            }
            let event_bytes = region.get(path_end + 8..path_end + 8 + event_len)?;
            if !event_bytes
                .iter()
                .all(|&b| b.is_ascii_alphanumeric() || b == b'_')
            {
                return None;
            }

            Some(RawSoundMod {
                path: String::from_utf8_lossy(path_bytes).to_string(),
                key_time_ms,
                event: String::from_utf8_lossy(event_bytes).to_string(),
                set: owning_set(has_value_pos),
            })
        });
        if let Some(entry) = entry {
            out.push(entry);
        }
    }
    out
}

/// The DyVertex tag (`0x0006 0000`) — the soft-body / wind-driven vertex
/// simulation flag (`docs/re/formats/moddata-unhandled.md` §3).
const MOD_DATA_DYVERTEX: u32 = 0x0006_0000;

/// A soft-body (cloth/wind) flag scanned from the mod palette
/// (`0x0006 0000` DyVertex ModData entries): capes, robes, wings, guild
/// banners, tent cloth, ship sails, rope bridges — 1,015 of the corpus's
/// 7,715 `.bsr`.
///
/// The entry carries **no payload at all**: it is exactly 32 bytes, the tag
/// plus the generic 28-byte `IModData` base, so everything it says is said by
/// `MtrlIdx` — *which* material of the resource is simulated — and by its
/// owning mod set.
#[derive(Debug, Clone, PartialEq)]
pub struct RawDyVertexMod {
    /// `JMXVBMT_MtrlIdx`: -1 = every material of the active material set,
    /// otherwise an index into the `.bmt`'s material list.
    pub mtrl_idx: i32,
    /// Owning mod set `(typ, animation_type, name)`, if a valid set header
    /// precedes the entry. See [`scan_mod_set_headers`] for the semantics.
    pub set: Option<(u32, u32, String)>,
}

/// Scans the mod palette for DyVertex (soft-body) entries.
///
/// Idea: this is the *smallest* ModData type — "no additional data"
/// (`ModDataDyVertex.cs:3-8`), corpus-proved as **every one of the 1,015
/// entries is exactly 32 B and the file still ends EOF-exact**
/// (`docs/re/formats/moddata-unhandled.md` §3). There is nothing to anchor on
/// the way [`parse_particle_mods`] anchors on `.efp` and [`parse_sound_mods`]
/// on `.wav`, so the candidate test *is* the base header, and it is tight
/// enough to carry the scan: the exact tag, `Float0 == 0.5` (100 % of all
/// target entries in the census), the four base fields that are zero in every
/// observed entry, and `Int1 == 0`, which the census reports as **unique to
/// this type** (TexAni uses `0x110`/`0x310`, MultiTex `0x100`/`0x300`).
///
/// `MtrlIdx` is range-checked rather than trusted: a `.bmt` material list is
/// small, so a plausible index or the -1 "all materials" sentinel is a far
/// stronger signal than "any i32".
pub fn parse_dyvertex_mods(data: &[u8], mod_palette_offset: usize) -> Vec<RawDyVertexMod> {
    let Some(region) = data.get(mod_palette_offset..) else {
        return Vec::new();
    };
    let read_u32 = |pos: usize| -> Option<u32> {
        region
            .get(pos..pos + 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    };
    let read_i32 = |pos: usize| read_u32(pos).map(|v| v as i32);

    let set_headers = scan_mod_set_headers(region);
    let owning_set = |entry_pos: usize| -> Option<(u32, u32, String)> {
        set_headers
            .iter()
            .take_while(|(pos, ..)| *pos < entry_pos)
            .last()
            .map(|(_, typ, anim_type, name)| (*typ, *anim_type, name.clone()))
    };

    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos + 32 <= region.len() {
        let entry = (|| {
            if read_u32(pos)? != MOD_DATA_DYVERTEX {
                return None;
            }
            // Float0 — 0.5 in 100% of the census's target entries
            if read_u32(pos + 4)? != 0.5f32.to_bits() {
                return None;
            }
            // Int0 is 1 or 2 across every ModData type
            if !matches!(read_i32(pos + 8)?, 1 | 2) {
                return None;
            }
            // Int1 == 0 is what makes this type identifiable at all
            if read_i32(pos + 12)? != 0 {
                return None;
            }
            let mtrl_idx = read_i32(pos + 16)?;
            if !(-1..=255).contains(&mtrl_idx) {
                return None;
            }
            // Int3, Int4 and the four flag bytes are zero in every observed
            // entry of every type except EnvMap's Int4
            if read_u32(pos + 20)? != 0 || read_u32(pos + 24)? != 0 || read_u32(pos + 28)? != 0 {
                return None;
            }
            Some(RawDyVertexMod {
                mtrl_idx,
                set: owning_set(pos),
            })
        })();
        match entry {
            Some(entry) => {
                out.push(entry);
                pos += 32;
            }
            None => pos += 1,
        }
    }
    out
}

/// Scans the mod palette for texture animation entries.
///
/// Like the other palette scanners this validates candidates instead of
/// walking the palette. Verified entry layout (waterfall/canal census of
/// res/nature/particle, cross-checked with the ModData wiki, 2026-07-22),
/// 116 bytes from the tag:
/// `u32 tag | f32 Float0 (0.5) | i32 Int0 (1|2) | i32 Int1 (0x110|0x310) |`
/// `i32 mtrlIdx (-1 = all) | i32 0 | i32 0 | u32 0 (4 flag bytes) |`
/// `u32 0 | u32[4] (1,1|10,1,1) | f32[16] row-major D3D texture transform`.
/// The ~30 constrained header bytes plus 16 bounded floats make loose-scan
/// false positives implausible (same rationale as [`envmap_mod_alpha_test`]);
/// MultiTex (`0x0001 0001`) never matches the exact tag compare.
pub fn parse_texani_mods(data: &[u8], mod_palette_offset: usize) -> Vec<RawTexAniMod> {
    let Some(region) = data.get(mod_palette_offset..) else {
        return Vec::new();
    };
    let read_u32 = |pos: usize| -> Option<u32> {
        region
            .get(pos..pos + 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    };
    let read_f32 = |pos: usize| read_u32(pos).map(f32::from_bits);

    let set_headers = scan_mod_set_headers(region);
    // the owning set of an entry is the nearest set header preceding it
    let owning_set = |entry_pos: usize| -> Option<(u32, u32, String)> {
        set_headers
            .iter()
            .take_while(|(pos, ..)| *pos < entry_pos)
            .last()
            .map(|(_, typ, anim_type, name)| (*typ, *anim_type, name.clone()))
    };

    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos + 116 <= region.len() {
        let entry = (|| {
            if read_u32(pos)? != MOD_DATA_TEXANI {
                return None;
            }
            // the generic IModData header fields (Float0 0.5 on every mod)
            let float0 = read_f32(pos + 4)?;
            if !(0.0..=1.0).contains(&float0) {
                return None;
            }
            if read_u32(pos + 8)? > 8 || read_u32(pos + 12)? >= 0x10000 {
                return None;
            }
            let mtrl_idx = read_u32(pos + 16)? as i32;
            if !(-1..=63).contains(&mtrl_idx) {
                return None;
            }
            // Int3 | Int4 | flag bytes: the tail of the 28-byte generic
            // IModData header, zero in all observed data
            if (20..=28).step_by(4).any(|o| read_u32(pos + o) != Some(0)) {
                return None;
            }
            // +32 is the first ModDataTexAni field past that header,
            // UnkUInt06, not padding — corpus histogram 0 x164 / 1 x118.
            // Requiring it to be zero rejected 43% of the real carriers.
            let multi_tex_stage = match read_u32(pos + 32)? {
                0 => false,
                1 => true,
                _ => return None,
            };
            if (36..=48)
                .step_by(4)
                .any(|o| read_u32(pos + o).is_none_or(|v| v > 4096))
            {
                return None;
            }
            let mut matrix = [0.0f32; 16];
            for (i, m) in matrix.iter_mut().enumerate() {
                *m = read_f32(pos + 52 + 4 * i)?;
                if !m.is_finite() || m.abs() >= 100.0 {
                    return None;
                }
            }
            let non_translation = matrix
                .iter()
                .enumerate()
                .any(|(i, m)| i != 8 && i != 9 && *m != 0.0);

            Some(RawTexAniMod {
                mtrl_idx,
                uv_speed: bevy::math::Vec2::new(matrix[8], matrix[9]),
                non_translation,
                multi_tex_stage,
                set: owning_set(pos),
            })
        })();
        match entry {
            Some(entry) => {
                out.push(entry);
                pos += 116;
            }
            None => pos += 1,
        }
    }
    out
}

/// An advanced-material entry scanned from the mod palette (`0x0000 0000`
/// Material ModData entries): raw D3D render states plus an animated
/// material-color gradient. Only the blend states are extracted — they
/// decide how TexAni surfaces composite (waterfalls: SRCALPHA/INVSRCALPHA
/// alpha blend or SRCALPHA/ONE additive glow).
#[derive(Debug, Clone, PartialEq)]
pub struct RawMaterialMod {
    /// `JMXVBMT_MtrlIdx`: -1 = every material of the set, else an index
    /// into the .bmt's material list.
    pub mtrl_idx: i32,
    /// `D3DRS_SRCBLEND` (D3DBLEND: 2 = ONE, 5 = SRCALPHA, 6 = INVSRCALPHA).
    pub src_blend: u8,
    /// `D3DRS_DESTBLEND`.
    pub dst_blend: u8,
    /// Owning mod set `(typ, animation_type, name)`, see
    /// [`scan_mod_set_headers`].
    pub set: Option<(u32, u32, String)>,
}

/// Scans the mod palette for advanced-material entries (tag `0x0000 0000`).
///
/// Variable-length entry, so unlike the fixed-size TexAni scan this walks
/// the verified layout and validates every field (census of all 5,536
/// res/ .bsr, 2026-07-23: 478 entries parse cleanly; srcblend/dstblend
/// dominated by (5,6) alpha blend and (5,2) additive):
///
/// ```text
/// u32 tag 0 | f32 Float0 (0.5) | i32 Int0 (1..8) | i32 Int1 | i32 mtrlIdx |
/// 12 zero bytes | u32 durationMs | u32 flag | u32 | u32 gradientKeyCount |
/// keys { u32 timeMs, f32[4] color } | if flag & 4 { u32 curveKeyCount,
/// keys { u32 timeMs, f32 value } } | u32[4] | u8[12] render states
/// (byte 0 = D3DRS_SRCBLEND, byte 1 = D3DRS_DESTBLEND) | f32 | u32
/// ```
///
/// The all-zero tag would false-positive everywhere, but requiring
/// Float0 == 0.5-ish (nonzero), Int0 in 1..=8 and the full bounded walk
/// to succeed rules noise out.
pub fn parse_material_mods(data: &[u8], mod_palette_offset: usize) -> Vec<RawMaterialMod> {
    let Some(region) = data.get(mod_palette_offset..) else {
        return Vec::new();
    };
    let read_u32 = |pos: usize| -> Option<u32> {
        region
            .get(pos..pos + 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    };
    let read_f32 = |pos: usize| read_u32(pos).map(f32::from_bits);

    let set_headers = scan_mod_set_headers(region);
    let owning_set = |entry_pos: usize| -> Option<(u32, u32, String)> {
        set_headers
            .iter()
            .take_while(|(pos, ..)| *pos < entry_pos)
            .last()
            .map(|(_, typ, anim_type, name)| (*typ, *anim_type, name.clone()))
    };

    // walks one candidate entry, returning (entry fields, end position)
    let try_parse = |pos: usize| -> Option<(i32, u8, u8, usize)> {
        if read_u32(pos)? != 0 {
            return None;
        }
        let float0 = read_f32(pos + 4)?;
        if !(float0 > 0.0 && float0 <= 1.0) {
            return None;
        }
        if !(1..=8).contains(&read_u32(pos + 8)?) || read_u32(pos + 12)? >= 0x10000 {
            return None;
        }
        let mtrl_idx = read_u32(pos + 16)? as i32;
        if !(-1..=63).contains(&mtrl_idx) {
            return None;
        }
        if region.get(pos + 20..pos + 32)?.iter().any(|&b| b != 0) {
            return None;
        }
        let duration = read_u32(pos + 32)?;
        let flag = read_u32(pos + 36)?;
        let gradient_keys = read_u32(pos + 44)?;
        if duration > 3_600_000 || flag > 0xFF || read_u32(pos + 40)? > 4096 || gradient_keys > 64 {
            return None;
        }
        let mut p = pos + 48;
        for _ in 0..gradient_keys {
            if read_u32(p)? > 3_600_000 {
                return None;
            }
            for i in 0..4 {
                let c = read_f32(p + 4 + 4 * i)?;
                if !c.is_finite() || !(0.0..=16.0).contains(&c) {
                    return None;
                }
            }
            p += 20;
        }
        if flag & 4 != 0 {
            let curve_keys = read_u32(p)?;
            if curve_keys > 64 {
                return None;
            }
            p += 4;
            for _ in 0..curve_keys {
                let v = read_f32(p + 4)?;
                if !v.is_finite() || v.abs() > 1e6 {
                    return None;
                }
                p += 8;
            }
        }
        if (0..4).any(|i| read_u32(p + 4 * i).is_none_or(|v| v > 4096)) {
            return None;
        }
        p += 16;
        let states = region.get(p..p + 12)?;
        let (src_blend, dst_blend) = (states[0], states[1]);
        // D3DBLEND values are 1..=17; (0,0) occurs once in the corpus
        if src_blend > 17 || dst_blend > 17 {
            return None;
        }
        p += 12 + 8; // states + trailing f32 + u32
        if p > region.len() {
            return None;
        }
        Some((mtrl_idx, src_blend, dst_blend, p))
    };

    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos + 100 <= region.len() {
        match try_parse(pos) {
            Some((mtrl_idx, src_blend, dst_blend, end)) => {
                out.push(RawMaterialMod {
                    mtrl_idx,
                    src_blend,
                    dst_blend,
                    set: owning_set(pos),
                });
                pos = end;
            }
            None => pos += 1,
        }
    }
    out
}

/// Which form of the trailing `CResAttachable` block an object type carries.
///
/// The block exists in two shapes (see [`parse_attach_info`]): the *item* form
/// (`unkUInt0 == 1`, no trailing `nComboNum`) and the *character* form
/// (`unkUInt0 == 0` plus `nComboNum`). Which one a file uses is decided by its
/// object type, and the source of truth is the EOF-exact mod-palette walk
/// censused in `docs/re/formats/moddata-unhandled.md` §6 over the user's 7,715-file
/// 1.188 corpus:
///
/// | object type | census | form |
/// |---|---|---|
/// | 0 (character) | attach + combo ×155 | character |
/// | 1 (NPC) | attach + combo ×113 | character |
/// | 5 (item) | attach ×2,746 | item |
/// | 2/3/4/6 | no attach block at all | (never reached) |
///
/// This used to read `typ & 0xFFFF != 0`, i.e. "everything but a character is an
/// item", which matched all 830 NPC resources against the item form (#602). Since
/// the item form demands `unkUInt0 == 1` where an NPC file has `0`, that could only
/// ever match the *tail* of a real character block by coincidence — measured on the
/// corpus, 29 of 830 NPC files did, all 29 producing an empty slot list. Types 2, 3,
/// 4 and 6 carry no block, so their classification is unobservable and left as it
/// was rather than guessed.
fn attach_form_is_item(object_type: u32) -> bool {
    !matches!(object_type & 0xFFFF, 0 | 1)
}

/// Parses the trailing CResAttachable block of a .bsr file.
///
/// The block sits at the very end of the file behind the mod palette,
/// whose entries have no reliably documented sizes. Instead of walking
/// the palette this matches the fixed-size block against the file tail
/// and validates every field, returning `None` when nothing matches
/// (e.g. for drop models and other resources without attachment data).
///
/// `is_item` selects the block form (items: `unkUInt0 == 1`, characters:
/// `unkUInt0 == 0` plus a trailing `nComboNum`) and comes from
/// [`ObjectInfo::typ`], where characters are type 0.
pub fn parse_attach_info(data: &[u8], mesh_count: usize, is_item: bool) -> Option<ResAttachInfo> {
    let read_u32 = |pos: usize| u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap());

    let validate = |slot_count: usize| -> Option<ResAttachInfo> {
        // characters carry an additional trailing nComboNum
        let size = 16 + slot_count * 8 + if is_item { 0 } else { 4 };
        let pos = data.len().checked_sub(size)?;

        if read_u32(pos) != if is_item { 1 } else { 0 } {
            return None;
        }
        let attach_kind = read_u32(pos + 4);
        // -1 is documented as valid (arrows)
        if attach_kind != u32::MAX && attach_kind >= 32 {
            return None;
        }
        let attach_method = read_u32(pos + 8);
        if attach_method > 2 {
            return None;
        }
        if read_u32(pos + 12) as usize != slot_count {
            return None;
        }
        if !is_item && read_u32(data.len() - 4) != 0 {
            return None;
        }

        let mut slots = Vec::with_capacity(slot_count);
        for i in 0..slot_count {
            let slot_id = read_u32(pos + 16 + i * 8);
            let mesh_idx = read_u32(pos + 20 + i * 8);
            if slot_id >= 64 || mesh_idx as usize >= mesh_count {
                return None;
            }
            slots.push((slot_id, mesh_idx));
        }

        Some(ResAttachInfo {
            is_item,
            attach_kind,
            attach_method,
            slots,
        })
    };

    // largest block first: a big block's slot data can look like a small
    // valid block, while a false positive spanning *more* data than the
    // real block would have to pass all field checks against unrelated
    // mod palette bytes
    (0..=32).rev().find_map(validate)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn parses_item_attach_info() {
        // tail of res/item/china/weapon/blade_01.bsr (preceded by mod
        // palette bytes that must not confuse the parser)
        let data: Vec<u8> = [
            0x7c, 0, 0, 0, 0, // trailing mod palette bytes
            1, 0, 0, 0, // unkUInt0 = ITEM
            7, 0, 0, 0, // unkUInt1 = right hand
            2, 0, 0, 0, // attachMethod = ADD
            1, 0, 0, 0, // slotCount
            9, 0, 0, 0, // slotId = right hand
            0, 0, 0, 0, // slotMeshIdx
        ]
        .into();

        assert_eq!(
            parse_attach_info(&data, 1, true),
            Some(ResAttachInfo {
                is_item: true,
                attach_kind: 7,
                attach_method: 2,
                slots: vec![(9, 0)],
            })
        );
    }

    /// #602: object type 1 is **NPC**, and NPCs carry the character form. The old
    /// predicate (`typ & 0xFFFF != 0`) said "anything that is not a character is an
    /// item" and so matched all 830 NPC resources in the corpus against the item
    /// form. Types are checked with the high bits the files actually carry
    /// (`0x0002_0000 | n`), because that is what `ObjectInfo::typ` holds.
    #[test]
    fn npc_and_character_types_use_the_character_attach_form() {
        assert!(!attach_form_is_item(0x0002_0000)); // 0 = character
        assert!(!attach_form_is_item(0x0002_0001)); // 1 = NPC — the fix
        assert!(attach_form_is_item(0x0002_0005)); // 5 = item, 2,746 attach blocks
                                                   // types with no attach block at all: left as they were, not guessed
        assert!(attach_form_is_item(0x0002_0006));
    }

    /// The same NPC tail parsed both ways, from the real bytes of
    /// `res/mob/china/bandit_champ.bsr` (object type `0x0002_0001`, 3 meshes).
    ///
    /// As a character it is a 4-slot block; as an item the matcher can only latch
    /// onto the block's own last slot pair plus `nComboNum` and reports an *empty*
    /// attach block — which is what every NPC resource got before #602. The second
    /// half of this test is therefore the regression: it pins the wrong answer to
    /// the wrong form.
    #[test]
    fn npc_attach_block_parses_as_character_not_item() {
        let mut data: Vec<u8> = b"voc_moan".to_vec(); // preceding mod-palette bytes
        data.extend(
            [
                0u32, 13, 0, 4, // unk0 = CHAR, kind, method = BASE, slotCount
                3, 0, 0, 0, 1, 1, 2, 2, // slots
                0, // nComboNum
            ]
            .iter()
            .flat_map(|v| v.to_le_bytes()),
        );

        let is_item = attach_form_is_item(0x0002_0001);
        assert!(!is_item, "object type 1 is an NPC, not an item");

        let info = parse_attach_info(&data, 3, is_item).expect("NPC block should parse");
        assert!(!info.is_item);
        assert_eq!(info.attach_kind, 13);
        assert_eq!(info.attach_method, 0);
        assert_eq!(info.slots, vec![(3, 0), (0, 0), (1, 1), (2, 2)]);

        // the pre-#602 behaviour, for contrast: the item form finds a zero-slot
        // block in the same bytes and the four real slots are lost
        let as_item = parse_attach_info(&data, 3, true).expect("the old false positive");
        assert!(as_item.slots.is_empty());
    }

    #[test]
    fn parses_character_attach_info() {
        // tail of res/char/china/chinaman_fighter.bsr
        let mut data: Vec<u8> = vec![0x84, 3, 0, 0, 0]; // mod palette bytes
        data.extend(
            [
                0u32, 13, 0, 9, // unk0 = CHAR, kind, method = BASE, count
                2, 6, 3, 5, 11, 3, 12, 4, 13, 2, 5, 1, 6, 0, 0, 8, 1, 7, // slots
                0, // nComboNum
            ]
            .iter()
            .flat_map(|v| v.to_le_bytes()),
        );

        let info = parse_attach_info(&data, 9, false).expect("should parse");
        assert!(!info.is_item);
        assert_eq!(info.attach_method, 0);
        assert_eq!(info.slots.len(), 9);
        assert_eq!(info.slots[0], (2, 6));
        assert_eq!(info.slots[8], (1, 7));
    }

    #[test]
    fn rejects_missing_attach_info() {
        let data: Vec<u8> = vec![0xab; 64];
        assert_eq!(parse_attach_info(&data, 4, true), None);
        assert_eq!(parse_attach_info(&data, 4, false), None);
    }

    #[test]
    fn parses_particle_mods() {
        // one Particle entry as observed in res/mob/karakoram/isyutaru.bsr:
        // anchorMode 1 | pathLen | path | boneLen 0 | Vec3 offset | delayMs | u32
        let path = br"monster\dead\dead_bluesmog.efp";
        let mut data: Vec<u8> = vec![0xff; 8]; // leading palette noise
        data.extend(1u32.to_le_bytes());
        data.extend((path.len() as u32).to_le_bytes());
        data.extend_from_slice(path);
        data.extend(0u32.to_le_bytes());
        data.extend(11.877f32.to_le_bytes());
        data.extend(44.588f32.to_le_bytes());
        data.extend(13.956f32.to_le_bytes());
        data.extend(2379u32.to_le_bytes());
        data.extend(0u32.to_le_bytes());

        let mods = parse_particle_mods(&data, 0);
        assert_eq!(mods.len(), 1);
        assert_eq!(mods[0].path, r"monster\dead\dead_bluesmog.efp");
        assert_eq!(mods[0].bone, None);
        assert_eq!(mods[0].delay_ms, 2379);
        assert!((mods[0].offset.y - 44.588).abs() < 1e-3);
        assert_eq!(mods[0].set, None); // no set header in front
        assert_eq!(mods[0].scale, 1.0); // no mod tag in front either
    }

    #[test]
    fn parses_lamp_particle_mod() {
        // ambient set of res/bldg/china/jangan02/cj_pub01_light01.bsr: the
        // "object light" — an always-on flame/glow at a local offset
        let path = br"map\cj_pal_lamp_red.efp";
        let mut data: Vec<u8> = Vec::new();
        data.extend(2u32.to_le_bytes()); // set typ 2 = ambient/system
        data.extend(u32::MAX.to_le_bytes()); // animation type -1
        data.extend(7u32.to_le_bytes());
        data.extend_from_slice(b"ambient");
        data.extend(2u32.to_le_bytes()); // mod count
        data.extend(0x0003_0000u32.to_le_bytes()); // Particle tag
        data.extend(0.5f32.to_le_bytes());
        data.extend(
            [0x30u32, u32::MAX, 0, 0, 0]
                .iter()
                .flat_map(|v| v.to_le_bytes()),
        );
        data.extend(1u32.to_le_bytes()); // entry count
        data.extend(1u32.to_le_bytes()); // anchor mode: at offset
        data.extend((path.len() as u32).to_le_bytes());
        data.extend_from_slice(path);
        data.extend(0u32.to_le_bytes()); // empty bone name
        data.extend(0.0f32.to_le_bytes());
        data.extend(84.412f32.to_le_bytes());
        data.extend((-2.4e-7f32).to_le_bytes());
        data.extend(0u32.to_le_bytes()); // delay
        data.extend(0x10fu32.to_le_bytes()); // trailing u32

        let mods = parse_particle_mods(&data, 0);
        assert_eq!(mods.len(), 1);
        assert_eq!(mods[0].path, r"map\cj_pal_lamp_red.efp");
        assert_eq!(mods[0].bone, None);
        assert!((mods[0].offset.y - 84.412).abs() < 1e-3);
        assert_eq!(mods[0].delay_ms, 0);
        assert!(mods[0].night_only); // trailing bytes 0f 01 00 00
        assert_eq!(mods[0].set, Some((2, u32::MAX, "ambient".to_string())));
        assert_eq!(mods[0].scale, 0.5); // the f32 after the Particle tag
    }

    #[test]
    fn parses_bone_anchored_particle_mods() {
        // ambient set of res/item/china/man_item/clothes_01_sa.bsr (garment
        // talisman): two glows anchored to the ward-tip bones (anchor mode 0)
        let path = br"system\item_equip_shoulder_white.efp";
        let mut data: Vec<u8> = Vec::new();
        data.extend(2u32.to_le_bytes());
        data.extend(u32::MAX.to_le_bytes());
        data.extend(7u32.to_le_bytes());
        data.extend_from_slice(b"ambient");
        data.extend(1u32.to_le_bytes()); // mod count
        data.extend(0x0003_0000u32.to_le_bytes()); // Particle tag
        data.extend(0.5f32.to_le_bytes());
        data.extend(
            [0x30u32, u32::MAX, 0, 0, 0]
                .iter()
                .flat_map(|v| v.to_le_bytes()),
        );
        data.extend(2u32.to_le_bytes()); // entry count
        for bone in [b"Bone06", b"Bone04"] {
            data.extend(0u32.to_le_bytes()); // anchor mode: at bone
            data.extend((path.len() as u32).to_le_bytes());
            data.extend_from_slice(path);
            data.extend(6u32.to_le_bytes());
            data.extend_from_slice(bone);
            data.extend([0.0f32; 3].iter().flat_map(|v| v.to_le_bytes()));
            data.extend(0u32.to_le_bytes()); // delay
            data.extend(0u32.to_le_bytes()); // trailing u32
        }

        let mods = parse_particle_mods(&data, 0);
        assert_eq!(mods.len(), 2);
        assert_eq!(mods[0].bone.as_deref(), Some("Bone06"));
        assert_eq!(mods[1].bone.as_deref(), Some("Bone04"));
        assert!(mods
            .iter()
            .all(|m| m.path == r"system\item_equip_shoulder_white.efp"));
        assert!(mods.iter().all(|m| m.offset == bevy::math::Vec3::ZERO));
        assert!(mods.iter().all(|m| !m.night_only));
        assert!(mods.iter().all(|m| m.scale == 0.5));
        assert!(mods
            .iter()
            .all(|m| m.set == Some((2, u32::MAX, "ambient".to_string()))));
    }

    #[test]
    fn associates_particle_mods_with_their_set() {
        // animation mod set as observed in res/mob/karakoram/isyutaru.bsr:
        // typ 1 (animation-linked), animation type 4 (die), group "default",
        // one Particle mod whose payload holds the entry
        let mut data: Vec<u8> = Vec::new();
        data.extend(1u32.to_le_bytes()); // typ
        data.extend(4u32.to_le_bytes()); // animation type
        data.extend(7u32.to_le_bytes());
        data.extend_from_slice(b"default");
        data.extend(1u32.to_le_bytes()); // mod count
        data.extend(0x0003_0000u32.to_le_bytes()); // Particle tag
        data.extend(0.5f32.to_le_bytes()); // payload prefix
        let path = br"monster\dead\dead_bluesmog.efp";
        data.extend(1u32.to_le_bytes());
        data.extend((path.len() as u32).to_le_bytes());
        data.extend_from_slice(path);
        data.extend(0u32.to_le_bytes());
        data.extend(
            [11.877f32, 44.588, 13.956]
                .iter()
                .flat_map(|v| v.to_le_bytes()),
        );
        data.extend(2379u32.to_le_bytes());
        data.extend(0u32.to_le_bytes());

        let mods = parse_particle_mods(&data, 0);
        assert_eq!(mods.len(), 1);
        assert_eq!(mods[0].set, Some((1, 4, "default".to_string())));
        // the animation-set layout has NO 0x30 block header between the tag
        // float and the entry — the tag is validated structurally (it sits
        // at the set header's first-mod position), so the scale still parses
        assert_eq!(mods[0].scale, 0.5);
    }

    #[test]
    fn ignores_false_particle_tags_in_palette_noise() {
        // the 0x0003_0000 tag bytes appearing as data inside another
        // palette structure, followed by an in-range float: neither at a
        // set header's first-mod position nor 0x30-marked, so it must NOT
        // be latched as the entry's scale (a false 7.7 would blow up the
        // spawned effect's size)
        let path = br"monster\dead\dead_bluesmog.efp";
        let mut data: Vec<u8> = vec![0xab; 8];
        data.extend(0x0003_0000u32.to_le_bytes()); // false tag in noise
        data.extend(7.7f32.to_le_bytes());
        data.extend([0xcdu8; 12]); // no 0x30 marker after the float
        data.extend(1u32.to_le_bytes()); // anchor mode: at offset
        data.extend((path.len() as u32).to_le_bytes());
        data.extend_from_slice(path);
        data.extend(0u32.to_le_bytes()); // empty bone name
        data.extend([0.0f32; 3].iter().flat_map(|v| v.to_le_bytes()));
        data.extend(0u32.to_le_bytes()); // delay
        data.extend(0u32.to_le_bytes()); // trailing u32

        let mods = parse_particle_mods(&data, 0);
        assert_eq!(mods.len(), 1);
        assert_eq!(mods[0].scale, 1.0);
    }

    /// Diagnostic (needs real assets/Data.pk2, hence ignored): dumps one
    /// .bsr's mod palette — every set header with its first-mod position,
    /// every Particle-tag candidate with a hex window and the verdict of
    /// both validation rules (structural / 0x30 marker), and the parsed
    /// entries with their associated scale. Decides whether a wrong
    /// runtime effect scale comes from a rejected genuine tag (fallback
    /// 1.0) or a latched false tag. Run:
    /// BSR_PROBE='res/item/china/man_item/clothes_01_sa.bsr' \
    ///   cargo test -p client probe_bsr_mod_palette -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic; needs real assets/Data.pk2"]
    fn probe_bsr_mod_palette() {
        use bevy::asset::io::AssetReader;
        use futures_lite::AsyncReadExt;

        let archive = bevy_pk2::prelude::Archive::configured(&PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../assets/Data.pk2"
        )));
        let path = PathBuf::from(
            std::env::var("BSR_PROBE")
                .unwrap_or_else(|_| "res/item/china/man_item/clothes_01_sa.bsr".into()),
        );
        let mut reader = bevy::tasks::block_on(archive.read(&path)).expect("bsr in Data.pk2");
        let mut data = Vec::new();
        bevy::tasks::block_on(reader.read_to_end(&mut data)).expect("read");

        let mut cursor = Cursor::new(data.as_slice());
        let _sig = cursor.get_fixed_size_string(12);
        let header = ResourceHeader::from(&mut cursor);
        let palette = header.mod_palette_offset as usize;
        println!(
            "{}: {} bytes, mod palette at 0x{palette:x}",
            path.display(),
            data.len()
        );

        let region = &data[palette..];
        let read_u32 = |pos: usize| -> Option<u32> {
            region
                .get(pos..pos + 4)
                .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        };
        let headers = scan_mod_set_headers(region);
        let structural: std::collections::HashSet<usize> = headers
            .iter()
            .map(|(pos, _, _, name)| pos + 16 + name.len())
            .collect();
        for (pos, typ, anim, name) in &headers {
            println!(
                "set header @+0x{pos:04x}: typ={typ} anim={anim:#x} name={name:?} \
                 first mod tag @+0x{:04x}",
                pos + 16 + name.len()
            );
        }

        let tag = 0x0003_0000u32.to_le_bytes();
        for (pos, _) in region.windows(4).enumerate().filter(|(_, w)| *w == tag) {
            let float = read_u32(pos + 4).map(f32::from_bits);
            let marker = read_u32(pos + 8) == Some(0x30) || read_u32(pos + 12) == Some(0x30);
            let start = pos.saturating_sub(0x10);
            let end = (pos + 0x30).min(region.len());
            let hex: Vec<String> = region[start..end]
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            println!(
                "particle tag @+0x{pos:04x}: float={float:?} structural={} marker0x30={marker}\n  \
                 bytes[+0x{start:04x}..+0x{end:04x}]: {}",
                structural.contains(&pos),
                hex.join(" ")
            );
        }

        for m in parse_particle_mods(&data, palette) {
            println!(
                "entry: path={:?} bone={:?} offset={:?} delay={}ms scale={} night={} set={:?}",
                m.path, m.bone, m.offset, m.delay_ms, m.scale, m.night_only, m.set
            );
        }
    }

    /// Diagnostic: census of every EnvMap ModData payload across all `.bsr`
    /// in Data.pk2 — does any field vary per resource enough to drive a
    /// per-item sheen tint/metallicity? (Gap #3 in
    /// docs/rendering-mobile-shader-comparison.md: the mobile port carries a
    /// per-material `_MetalColor`; our TFACTOR is a constant because the
    /// sampled ModData floats never deviated from 0.5 — this measures the
    /// whole corpus instead of a sample.) `envmap_mod_alpha_test` validates
    /// Float0 and then discards it; this dumps the distribution of Float0
    /// and the payload dwords at +8..+36, grouped by resource path prefix.
    /// Run:
    /// cargo test -p client probe_envmap_moddata_census -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic; needs real assets/Data.pk2"]
    fn probe_envmap_moddata_census() {
        use std::collections::BTreeMap;

        let archive = bevy_pk2::prelude::Archive::configured(&PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../assets/Data.pk2"
        )));
        let mut paths: Vec<PathBuf> = archive
            .root
            .get_all_entries()
            .into_iter()
            .filter(|(path, entry)| {
                entry.is_file()
                    && path
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("bsr"))
            })
            .map(|(path, _)| path)
            .collect();
        paths.sort();

        let mut resources = 0u64;
        let mut with_envmap = 0u64;
        let mut header_failures = 0u64;
        // f32 bit patterns → count, so exact values (not lossy prints) are
        // compared; offsets are relative to the entry's tag dword
        let mut float0: BTreeMap<u32, u64> = BTreeMap::new();
        const DWORD_OFFSETS: [usize; 8] = [8, 12, 16, 20, 24, 28, 32, 36];
        let mut dwords: Vec<BTreeMap<u32, u64>> = vec![BTreeMap::new(); DWORD_OFFSETS.len()];
        // path prefix (first three components, lowercased) → Float0 histogram
        let mut per_prefix: BTreeMap<String, BTreeMap<u32, u64>> = BTreeMap::new();
        let mut nondefault_samples: Vec<String> = Vec::new();

        for path in &paths {
            let Some(data) = archive.read_file_bytes(path) else {
                continue;
            };
            let Ok(header) = std::panic::catch_unwind(|| {
                let mut cursor = Cursor::new(data.as_slice());
                let _sig = cursor.get_fixed_size_string(12);
                ResourceHeader::from(&mut cursor)
            }) else {
                header_failures += 1;
                continue;
            };
            let palette = header.mod_palette_offset as usize;
            if palette >= data.len() {
                continue;
            }
            resources += 1;
            let region = &data[palette..];
            let read_u32 = |pos: usize| -> Option<u32> {
                region
                    .get(pos..pos + 4)
                    .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            };

            // Same validated scan as `envmap_mod_alpha_test`, but collecting
            // every hit's payload instead of returning the alpha-test bit of
            // the first one.
            let set_headers = scan_mod_set_headers(region);
            let mut hits: Vec<usize> = Vec::new();
            let mut search = 0usize;
            while let Some(rel) = region
                .get(search..)
                .and_then(|r| r.windows(7).position(|w| w == b"ambient"))
            {
                let name_pos = search + rel;
                search = name_pos + 1;
                let Some(hdr) = name_pos.checked_sub(12) else {
                    continue;
                };
                if read_u32(hdr) != Some(2) || read_u32(hdr + 8) != Some(7) {
                    continue;
                }
                if !matches!(read_u32(name_pos + 7), Some(1..=16)) {
                    continue;
                }
                let first_mod = name_pos + 7 + 4;
                let end = set_headers
                    .iter()
                    .map(|(pos, ..)| *pos)
                    .find(|pos| *pos > name_pos)
                    .unwrap_or(region.len());
                let mut pos = first_mod;
                while pos + 8 <= end {
                    let valid = (|| {
                        if read_u32(pos)? != MOD_DATA_ENVMAP {
                            return None;
                        }
                        let strength =
                            f32::from_le_bytes(region.get(pos + 4..pos + 8)?.try_into().unwrap());
                        if !(0.0..=1.0).contains(&strength) {
                            return None;
                        }
                        if read_u32(pos + 8).is_some_and(|v| v > 8)
                            || read_u32(pos + 12).is_some_and(|v| v >= 0x10000)
                            || read_u32(pos + 16).is_some_and(|v| !(-1..=63).contains(&(v as i32)))
                            || read_u32(pos + 20).is_some_and(|v| v != 0)
                        {
                            return None;
                        }
                        Some(())
                    })();
                    if valid.is_some() {
                        hits.push(pos);
                    }
                    pos += 1;
                }
            }

            if hits.is_empty() {
                continue;
            }
            with_envmap += 1;
            let prefix: String = path
                .iter()
                .take(3)
                .map(|c| c.to_string_lossy().to_ascii_lowercase())
                .collect::<Vec<_>>()
                .join("/");
            for pos in hits {
                let f0_bits = read_u32(pos + 4).unwrap_or(0);
                *float0.entry(f0_bits).or_default() += 1;
                *per_prefix
                    .entry(prefix.clone())
                    .or_default()
                    .entry(f0_bits)
                    .or_default() += 1;
                if f32::from_bits(f0_bits) != 0.5 && nondefault_samples.len() < 20 {
                    nondefault_samples.push(format!(
                        "{}: Float0 = {}",
                        path.display(),
                        f32::from_bits(f0_bits)
                    ));
                }
                for (slot, offset) in DWORD_OFFSETS.iter().enumerate() {
                    if let Some(v) = read_u32(pos + offset) {
                        *dwords[slot].entry(v).or_default() += 1;
                    }
                }
            }
        }

        println!(
            "{} .bsr files, {resources} with a mod palette, {with_envmap} with EnvMap, \
             {header_failures} header parse failures",
            paths.len()
        );
        println!("Float0 distribution:");
        for (bits, count) in &float0 {
            println!("  {} (0x{bits:08x}): {count}", f32::from_bits(*bits));
        }
        for (slot, offset) in DWORD_OFFSETS.iter().enumerate() {
            let histogram = &dwords[slot];
            print!("payload dword +{offset}: {} distinct —", histogram.len());
            for (v, count) in histogram.iter().take(8) {
                print!(" 0x{v:x} (f32 {}) x{count};", f32::from_bits(*v));
            }
            println!("{}", if histogram.len() > 8 { " …" } else { "" });
        }
        println!("Float0 by path prefix:");
        for (prefix, histogram) in &per_prefix {
            let total: u64 = histogram.values().sum();
            let values: Vec<String> = histogram
                .iter()
                .map(|(bits, count)| format!("{} x{count}", f32::from_bits(*bits)))
                .collect();
            println!("  {prefix}: {total} entries — {}", values.join(", "));
        }
        for s in &nondefault_samples {
            println!("non-0.5 sample: {s}");
        }
    }

    #[test]
    fn rejects_particle_noise() {
        // ".efp" bytes without a valid entry layout around them
        let mut data: Vec<u8> = vec![0xab; 16];
        data.extend_from_slice(b".efp");
        data.extend(vec![0xcd; 16]);
        assert!(parse_particle_mods(&data, 0).is_empty());
    }

    /// A full 116-byte TexAni entry as observed in the waterfall bsrs
    /// (e.g. res/nature/particle/dun_waterfall_s01.bsr).
    fn texani_entry(tag: u32, float0: f32, mtrl_idx: i32, u: f32, v: f32) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend(tag.to_le_bytes());
        data.extend(float0.to_le_bytes());
        data.extend(1u32.to_le_bytes()); // Int0
        data.extend(0x110u32.to_le_bytes()); // Int1
        data.extend(mtrl_idx.to_le_bytes());
        data.extend([0u8; 16]); // Int3 | Int4 | flag bytes | unkUInt5
        data.extend([1u32; 4].iter().flat_map(|x| x.to_le_bytes())); // unkUInt6..9
        let mut matrix = [0.0f32; 16];
        matrix[8] = u;
        matrix[9] = v;
        data.extend(matrix.iter().flat_map(|m| m.to_le_bytes()));
        data
    }

    #[test]
    fn parses_texani_mod_in_ambient_set() {
        // ambient set header like the waterfall bsrs, entry applying to
        // all materials (mtrl_idx -1) scrolling V down at 1.5 uv/sec
        let mut data: Vec<u8> = Vec::new();
        data.extend(2u32.to_le_bytes()); // set typ 2 = ambient/system
        data.extend(u32::MAX.to_le_bytes()); // animation type -1
        data.extend(7u32.to_le_bytes());
        data.extend_from_slice(b"ambient");
        data.extend(1u32.to_le_bytes()); // mod count
        data.extend(texani_entry(0x0001_0000, 0.5, -1, 0.0, -1.5));

        let mods = parse_texani_mods(&data, 0);
        assert_eq!(mods.len(), 1);
        assert_eq!(mods[0].mtrl_idx, -1);
        assert_eq!(mods[0].uv_speed, bevy::math::Vec2::new(0.0, -1.5));
        assert!(!mods[0].non_translation);
        assert_eq!(mods[0].set, Some((2, u32::MAX, "ambient".to_string())));
    }

    #[test]
    fn parses_texani_mod_with_material_index() {
        // specific-material entry (cj_waterfall02_01 targets material 0)
        let data = texani_entry(0x0001_0000, 0.5, 0, 0.07, 0.0);
        let mods = parse_texani_mods(&data, 0);
        assert_eq!(mods.len(), 1);
        assert_eq!(mods[0].mtrl_idx, 0);
        assert_eq!(mods[0].uv_speed, bevy::math::Vec2::new(0.07, 0.0));
        assert_eq!(mods[0].set, None); // no set header in front
    }

    #[test]
    fn flags_texani_non_translation_matrix() {
        let mut data = texani_entry(0x0001_0000, 0.5, -1, 0.0, -0.5);
        // put a rotation-ish term into matrix element 0 (offset 52)
        data[52..56].copy_from_slice(&1.0f32.to_le_bytes());
        let mods = parse_texani_mods(&data, 0);
        assert_eq!(mods.len(), 1);
        assert!(mods[0].non_translation);
        assert_eq!(mods[0].uv_speed, bevy::math::Vec2::new(0.0, -0.5));
    }

    /// A Material (advanced material) entry as observed in the waterfall
    /// bsrs: two-key color gradient, no curve, 12 render-state bytes.
    fn material_entry(float0: f32, src_blend: u8, dst_blend: u8) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend(0u32.to_le_bytes()); // Material tag
        data.extend(float0.to_le_bytes());
        data.extend(1u32.to_le_bytes()); // Int0
        data.extend(0x110u32.to_le_bytes()); // Int1
        data.extend((-1i32).to_le_bytes()); // mtrl_idx = all
        data.extend([0u8; 12]); // Int3 | Int4 | flag bytes
        data.extend(1000u32.to_le_bytes()); // duration ms
        data.extend(3u32.to_le_bytes()); // flag (no curve)
        data.extend(0u32.to_le_bytes());
        data.extend(2u32.to_le_bytes()); // gradient key count
        for time in [0u32, 1000] {
            data.extend(time.to_le_bytes());
            data.extend(
                [0.58f32, 0.58, 0.58, 1.0]
                    .iter()
                    .flat_map(|c| c.to_le_bytes()),
            );
        }
        data.extend([0u32, 1, 1, 0].iter().flat_map(|v| v.to_le_bytes()));
        data.extend([src_blend, dst_blend, 5, 0, 2, 3, 2, 2, 128, 7, 100, 200]);
        data.extend(1.0f32.to_le_bytes());
        data.extend(1u32.to_le_bytes());
        data
    }

    #[test]
    fn parses_material_mod_blend_states() {
        // ambient set with [Material, TexAni] like sd_waterfall_01.bsr
        let mut data: Vec<u8> = Vec::new();
        data.extend(2u32.to_le_bytes());
        data.extend(u32::MAX.to_le_bytes());
        data.extend(7u32.to_le_bytes());
        data.extend_from_slice(b"ambient");
        data.extend(2u32.to_le_bytes()); // mod count
        data.extend(material_entry(0.5, 5, 2)); // SRCALPHA / ONE = additive
        data.extend(texani_entry(0x0001_0000, 0.5, -1, 0.0, -1.0));

        let mods = parse_material_mods(&data, 0);
        assert_eq!(mods.len(), 1);
        assert_eq!(mods[0].mtrl_idx, -1);
        assert_eq!((mods[0].src_blend, mods[0].dst_blend), (5, 2));
        assert_eq!(mods[0].set, Some((2, u32::MAX, "ambient".to_string())));
        // the TexAni entry still parses behind the variable-length entry
        assert_eq!(parse_texani_mods(&data, 0).len(), 1);
    }

    #[test]
    fn rejects_material_mod_noise() {
        // all-zero region: tag matches everywhere but Float0 == 0 fails
        assert!(parse_material_mods(&vec![0u8; 256], 0).is_empty());
        // bogus Float0
        assert!(parse_material_mods(&material_entry(7.5, 5, 6), 0).is_empty());
        // invalid blend value
        assert!(parse_material_mods(&material_entry(0.5, 99, 6), 0).is_empty());
    }

    #[test]
    fn rejects_texani_noise() {
        // MultiTex tag must not match
        assert!(parse_texani_mods(&texani_entry(0x0001_0001, 0.5, -1, 0.0, -1.0), 0).is_empty());
        // Float0 out of range
        assert!(parse_texani_mods(&texani_entry(0x0001_0000, 7.5, -1, 0.0, -1.0), 0).is_empty());
        // mtrl_idx out of range
        assert!(parse_texani_mods(&texani_entry(0x0001_0000, 0.5, 900, 0.0, -1.0), 0).is_empty());
        // non-finite matrix element
        let mut data = texani_entry(0x0001_0000, 0.5, -1, 0.0, -1.0);
        data[60..64].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(parse_texani_mods(&data, 0).is_empty());
        // tag bytes in random noise
        let mut noise: Vec<u8> = vec![0xab; 8];
        noise.extend(0x0001_0000u32.to_le_bytes());
        noise.extend(vec![0xcd; 120]);
        assert!(parse_texani_mods(&noise, 0).is_empty());
    }

    // PROVENANCE OF THE TWO BYTE FIXTURES BELOW.
    //
    // `AMALRUN_MOD_PALETTE` and `TOMBSTONE_MOD_PALETTE` are short excerpts of
    // real records from the user's own Data.pk2 — 287 and 265 bytes, out of a
    // ~3.2 GB archive. They are kept deliberately, and they are the only literal
    // game bytes anywhere in this tree.
    //
    // Why not synthesize them: both are REGRESSION tests whose whole value is
    // that they are real. `parses_texani_mod_with_multi_tex_stage_flag` exists
    // because a synthetic assumption — "+20..=+32 must be zero" — rejected 118
    // genuine corpus entries. A fixture built from our own reading of the format
    // would test that reading, not the format, and would re-open exactly the
    // hole these tests were written to close.
    //
    // What they contain is structural: field offsets, numeric parameters and two
    // asset path strings. They are used here solely as interoperability test
    // vectors. If they ever need to go, the replacement is a corpus probe run
    // against the user's own archive at test time (gated on the PK2s being
    // present), not an invented byte array.

    /// `res/item/avatar/avata_m_amalrun_2.bsr`, mod palette (file offset
    /// 378) through the first bytes of the MultiTex entry that follows —
    /// real bytes from the 1.188 Data.pk2. The ambient set holds
    /// [Material, TexAni @+151, MultiTex @+267]; the TexAni entry carries
    /// `UnkUInt06 == 1` at +32, which the old `+20..=+32 must be zero`
    /// check rejected together with 117 other carriers.
    const AMALRUN_MOD_PALETTE: [u8; 287] = [
        0x01, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0x07, 0x00, 0x00,
        0x00, 0x61, 0x6d, 0x62, 0x69, 0x65, 0x6e, 0x74, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x3f, 0x02, 0x00, 0x00, 0x00, 0x10, 0x01, 0x00, 0x00, 0x05, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xe8,
        0x03, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0xe1, 0x7a, 0x14, 0x3f, 0xe1, 0x7a, 0x14, 0x3f, 0xe1, 0x7a, 0x14,
        0x3f, 0x00, 0x00, 0x80, 0x3f, 0xe8, 0x03, 0x00, 0x00, 0xe1, 0x7a, 0x14, 0x3f, 0xe1, 0x7a,
        0x14, 0x3f, 0xe1, 0x7a, 0x14, 0x3f, 0x00, 0x00, 0x80, 0x3f, 0x00, 0x00, 0x00, 0x00, 0x01,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x02, 0x05, 0x00,
        0x02, 0x02, 0x02, 0x02, 0x80, 0x07, 0x64, 0xc8, 0x00, 0x00, 0x80, 0x3f, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x3f, 0x02, 0x00, 0x00, 0x00, 0x10, 0x01,
        0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
        0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x9a, 0x99, 0x19, 0xbe, 0x33,
        0x33, 0xb3, 0xbe, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01,
        0x00, 0x00, 0x00, 0x00, 0x3f, 0x02, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00,
        0x00, 0x00,
    ];

    #[test]
    fn parses_texani_mod_with_multi_tex_stage_flag() {
        let mods = parse_texani_mods(&AMALRUN_MOD_PALETTE, 0);
        assert_eq!(mods.len(), 1);
        assert_eq!(mods[0].mtrl_idx, 5);
        assert_eq!(mods[0].uv_speed, bevy::math::Vec2::new(-0.15, -0.35));
        assert!(!mods[0].non_translation);
        // UnkUInt06 == 1: the transform drives the MultiTex second stage
        assert!(mods[0].multi_tex_stage);
        assert_eq!(mods[0].set, Some((2, u32::MAX, "ambient".to_string())));
        // the entry sits right in front of the MultiTex tag it belongs to
        assert_eq!(AMALRUN_MOD_PALETTE[267..271], [0x01, 0x00, 0x01, 0x00]);
    }

    #[test]
    fn rejects_texani_unk6_out_of_range() {
        // UnkUInt06 only ever holds 0 or 1 across the 282 corpus entries
        let mut data = texani_entry(0x0001_0000, 0.5, -1, 0.0, -1.0);
        data[32..36].copy_from_slice(&2u32.to_le_bytes());
        assert!(parse_texani_mods(&data, 0).is_empty());
        // +20/+24/+28 stay generic-header padding and must remain zero
        for off in [20usize, 24, 28] {
            let mut data = texani_entry(0x0001_0000, 0.5, -1, 0.0, -1.0);
            data[off..off + 4].copy_from_slice(&1u32.to_le_bytes());
            assert!(parse_texani_mods(&data, 0).is_empty());
        }
    }

    #[test]
    fn detects_envmap_mod() {
        // mod palette of res/item/china/weapon/sword_01.bsr: one "ambient"
        // set with a single EnvMap (0x00040000) entry of strength 0.5,
        // flags dword 1 (bit 2 clear = no alpha test)
        let data: Vec<u8> = [
            0x01, 0, 0, 0, 0x02, 0, 0, 0, 0xff, 0xff, 0xff, 0xff, // set count, typ, anim type
            0x07, 0, 0, 0, b'a', b'm', b'b', b'i', b'e', b'n', b't', // set name
            0x01, 0, 0, 0, // entry count
            0x00, 0x00, 0x04, 0x00, // EnvMap tag
            0x00, 0x00, 0x00, 0x3f, // strength 0.5
            0x01, 0, 0, 0, 0x00, 0x03, 0, 0, // payload
            0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, // payload
            0x01, 0x00, 0x00, 0x00, // flags dword (+24): no alpha test
        ]
        .into();

        assert_eq!(envmap_mod_alpha_test(&data, 0), Some(false));
        // offset beyond the tag must not match
        assert_eq!(envmap_mod_alpha_test(&data, 32), None);
    }

    #[test]
    fn detects_envmap_alpha_test_flag() {
        // res/item/china/weapon/tblade_04.bsr: flags dword 3 — bit 2 set
        // means alpha test GREATEREQUAL 1 (exact-zero texels are cutouts)
        let data: Vec<u8> = [
            0x01, 0, 0, 0, 0x02, 0, 0, 0, 0xff, 0xff, 0xff, 0xff, 0x07, 0, 0, 0, b'a', b'm', b'b',
            b'i', b'e', b'n', b't', 0x01, 0, 0, 0, // entry count
            0x00, 0x00, 0x04, 0x00, // EnvMap tag
            0x00, 0x00, 0x00, 0x3f, // strength 0.5
            0x01, 0, 0, 0, 0x00, 0x03, 0, 0, // payload
            0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, // payload
            0x03, 0x00, 0x00, 0x00, // flags dword (+24): alpha test on
        ]
        .into();

        assert_eq!(envmap_mod_alpha_test(&data, 0), Some(true));
    }

    #[test]
    fn detects_envmap_behind_other_mods() {
        // res/item/china/shield/shield_14.bsr: the ambient set puts a
        // Particle mod (attached .efp) first and the EnvMap second — the
        // whole set span must be scanned, not just the first mod slot
        let mut data: Vec<u8> = [
            0x01, 0, 0, 0, 0x02, 0, 0, 0, 0xff, 0xff, 0xff, 0xff, // set count, typ, anim type
            0x07, 0, 0, 0, b'a', b'm', b'b', b'i', b'e', b'n', b't', // set name
            0x02, 0, 0, 0, // entry count
            0x00, 0x00, 0x03, 0x00, // Particle tag
            0x00, 0x00, 0x00, 0x3f, // strength 0.5
            0x01, 0, 0, 0, 0x30, 0, 0, 0, // header marker 0x30
            0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, // payload
        ]
        .to_vec();
        // an embedded path string, as real Particle entries carry
        data.extend_from_slice(&[0x09, 0, 0, 0]);
        data.extend_from_slice(b"a\\b.efp\x00\x00");
        data.extend_from_slice(&[
            0x00, 0x00, 0x04, 0x00, // EnvMap tag
            0x00, 0x00, 0x00, 0x3f, // strength 0.5
            0x01, 0, 0, 0, 0x00, 0x01, 0, 0, // payload
            0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, // payload
            0x03, 0x00, 0x00, 0x00, // flags dword (+24): alpha test on
        ]);

        assert_eq!(envmap_mod_alpha_test(&data, 0), Some(true));
    }

    #[test]
    fn envmap_truncated_flags_default_off() {
        // an entry whose payload ends before the flags dword still counts
        // as EnvMap, with the alpha test defaulting off
        let data: Vec<u8> = [
            0x01, 0, 0, 0, 0x02, 0, 0, 0, 0xff, 0xff, 0xff, 0xff, 0x07, 0, 0, 0, b'a', b'm', b'b',
            b'i', b'e', b'n', b't', 0x01, 0, 0, 0, // entry count
            0x00, 0x00, 0x04, 0x00, // EnvMap tag
            0x00, 0x00, 0x00, 0x3f, // strength 0.5
            0x01, 0, 0, 0, 0x00, 0x03, 0, 0, // truncated payload
        ]
        .into();

        assert_eq!(envmap_mod_alpha_test(&data, 0), Some(false));
    }

    #[test]
    fn ignores_material_mod() {
        // mod palette of res/nature/common/tree/tre_maple03.bsr: the leaf's
        // "ambient" set carries a Material (0x00000000) entry with D3D
        // blend states, not an EnvMap
        let data: Vec<u8> = [
            0x01, 0, 0, 0, 0x02, 0, 0, 0, 0xff, 0xff, 0xff, 0xff, 0x07, 0, 0, 0, b'a', b'm', b'b',
            b'i', b'e', b'n', b't', 0x01, 0, 0, 0, // entry count
            0x00, 0x00, 0x00, 0x00, // Material tag
            0x00, 0x00, 0x00, 0x3f, // strength 0.5
            0x10, 0x01, 0, 0, 0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xe8,
            0x03, 0, 0, 0x02, 0, 0, 0,
        ]
        .into();

        assert_eq!(envmap_mod_alpha_test(&data, 0), None);
    }

    #[test]
    fn no_envmap_without_ambient_set() {
        // character resources have no "ambient" system set at all; a stray
        // EnvMap-looking tag outside one must not count
        let data: Vec<u8> = [
            0x07, 0, 0, 0, b'b', b'o', b'w', b'_', b'r', b'u', b'n', 0x00, 0x00, 0x04, 0x00, 0x00,
            0x00, 0x00, 0x3f,
        ]
        .into();

        assert_eq!(envmap_mod_alpha_test(&data, 0), None);
        assert_eq!(envmap_mod_alpha_test(&data, 999), None);
    }

    /// #274: the walk graph is `count, length, points`. Reading the points
    /// first spans the same bytes, so it parses "fine" while shifting every
    /// value by four — the real length lands in `points[0].x` and the last
    /// point's y is mistaken for the length. These bytes are shaped so the old
    /// order produces exactly that symptom: length 1.0, and 12.5 in points[0].x.
    #[test]
    fn walk_graph_reads_the_length_before_the_points() {
        let mut data: Vec<u8> = 2u32.to_le_bytes().to_vec(); // walkPointCnt
        data.extend(12.5f32.to_le_bytes()); // walkLength
        data.extend(0.0f32.to_le_bytes()); // point 0 x
        data.extend(0.0f32.to_le_bytes()); // point 0 y
        data.extend(1.0f32.to_le_bytes()); // point 1 x
        data.extend(1.0f32.to_le_bytes()); // point 1 y

        let (walk_length, walk_points) = read_walk_graph(&mut data.as_slice());

        assert_eq!(walk_length, 12.5);
        assert_eq!(walk_points, vec![Vec2::ZERO, Vec2::ONE]);
    }

    /// An empty walk graph still consumes its length field, so a following
    /// read is not knocked out of alignment.
    #[test]
    fn walk_graph_with_no_points_still_consumes_the_length() {
        let mut data: Vec<u8> = 0u32.to_le_bytes().to_vec();
        data.extend(0.0f32.to_le_bytes());
        data.extend(0xAABBCCDDu32.to_le_bytes()); // the next field

        let mut cursor = data.as_slice();
        let (walk_length, walk_points) = read_walk_graph(&mut cursor);

        assert_eq!(walk_length, 0.0);
        assert!(walk_points.is_empty());
        assert_eq!(cursor.get_u32_le(), 0xAABBCCDD);
    }

    /// `res/mob/china/tombstone_die.bsr`, mod palette (file offset 795,
    /// 265 bytes to EOF): one animation-linked set `("default", type 4 =
    /// die)` holding a single Sound ModData entry with **two** tracks —
    /// `cm_gstone_thud_a.wav` at 347 ms and `cm_tomb_die.wav` at 0 ms.
    /// Both the nested `{SoundSet -> nTrackNum -> track}` container and the
    /// owning set header are real bytes from the user's Data.pk2.
    const TOMBSTONE_MOD_PALETTE: [u8; 265] = [
        0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00,
        0x00, 0x07, 0x00, 0x00, 0x00, 0x64, 0x65, 0x66, 0x61, 0x75, 0x6c, 0x74, 0x01, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0x00, 0x3f, 0x01, 0x00, 0x00, 0x00, 0x10, 0x00,
        0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0xc0, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x20, 0x41, 0x00, 0x00, 0xc8, 0x42, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x07, 0x00, 0x00, 0x00, 0x64, 0x65, 0x66, 0x61, 0x75,
        0x6c, 0x74, 0x02, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x25, 0x00, 0x00, 0x00, 0x70,
        0x72, 0x69, 0x6d, 0x5c, 0x73, 0x6e, 0x64, 0x5c, 0x6d, 0x6f, 0x6e, 0x73, 0x74, 0x65, 0x72,
        0x5c, 0x63, 0x6d, 0x5f, 0x67, 0x73, 0x74, 0x6f, 0x6e, 0x65, 0x5f, 0x74, 0x68, 0x75, 0x64,
        0x5f, 0x61, 0x2e, 0x77, 0x61, 0x76, 0x5b, 0x01, 0x00, 0x00, 0x09, 0x00, 0x00, 0x00, 0x73,
        0x6e, 0x64, 0x5f, 0x64, 0x65, 0x61, 0x74, 0x68, 0x01, 0x00, 0x00, 0x00, 0x20, 0x00, 0x00,
        0x00, 0x70, 0x72, 0x69, 0x6d, 0x5c, 0x73, 0x6e, 0x64, 0x5c, 0x6d, 0x6f, 0x6e, 0x73, 0x74,
        0x65, 0x72, 0x5c, 0x63, 0x6d, 0x5f, 0x74, 0x6f, 0x6d, 0x62, 0x5f, 0x64, 0x69, 0x65, 0x2e,
        0x77, 0x61, 0x76, 0x00, 0x00, 0x00, 0x00, 0x09, 0x00, 0x00, 0x00, 0x76, 0x6f, 0x63, 0x5f,
        0x64, 0x65, 0x61, 0x74, 0x68, 0xff, 0xff, 0xff, 0xff, 0x0d, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];

    /// A DyVertex ("cloth") mod palette. The **28-byte base and the set
    /// header are real bytes** from `res/mob/china/tombstone_die.bsr`'s
    /// palette (the fixture above); the tag and `Int1` carry the DyVertex
    /// values the census gives (`0x0006 0000`, `Int1 == 0`), and the second
    /// entry's `MtrlIdx` is 2 instead of -1. No DyVertex carrier is checked
    /// into this repo — the 1,015 of them live in the user's Data.pk2 — so
    /// this is a constructed fixture over a real base, and it is written down
    /// as such rather than presented as a capture.
    const CLOTH_MOD_PALETTE: [u8; 87] = [
        0x01, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x07, 0x00, 0x00, 0x00, 0x64, 0x65, 0x66,
        0x61, 0x75, 0x6c, 0x74, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x00,
        0x3f, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x06, 0x00, 0x00,
        0x00, 0x00, 0x3f, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];

    /// Both 32-byte entries are found, both keep their own `MtrlIdx`, and
    /// both are attributed to the set header in front of them — the whole
    /// payload of a type that has no payload.
    #[test]
    fn parses_dyvertex_flags_and_their_material_index() {
        let mods = parse_dyvertex_mods(&CLOTH_MOD_PALETTE, 0);

        assert_eq!(mods.len(), 2, "two 32-byte entries, back to back");
        assert_eq!(mods[0].mtrl_idx, -1, "-1 = every material of the set");
        assert_eq!(mods[1].mtrl_idx, 2);
        for entry in &mods {
            assert_eq!(entry.set, Some((1, 4, "default".to_string())));
        }
    }

    /// The scan has no string to anchor on, so its whole defence is the base
    /// header. Each of the three fields the census calls diagnostic must
    /// reject on its own — in particular `Int1 != 0`, which is what separates
    /// DyVertex from TexAni (`0x110`) and MultiTex (`0x100`).
    #[test]
    fn dyvertex_scan_rejects_a_foreign_base() {
        let flip = |offset: usize, value: u32| {
            let mut data = CLOTH_MOD_PALETTE.to_vec();
            // 23 = the first entry's tag; the base follows it
            data[23 + offset..23 + offset + 4].copy_from_slice(&value.to_le_bytes());
            parse_dyvertex_mods(&data, 0)
        };

        // Int1 = 0x110: a TexAni base, not a DyVertex one
        assert_eq!(flip(12, 0x110).len(), 1, "only the untouched entry remains");
        // Float0 = 1.0: the census says 0.5 in 100% of entries
        assert_eq!(flip(4, 1.0f32.to_bits()).len(), 1);
        // MtrlIdx = 70,000: no .bmt has that many materials
        assert_eq!(flip(16, 70_000).len(), 1);
        // and pure noise yields nothing at all
        assert!(parse_dyvertex_mods(&[0xcd; 128], 0).is_empty());
    }

    #[test]
    fn parses_sound_mod_tracks_from_real_palette() {
        let mods = parse_sound_mods(&TOMBSTONE_MOD_PALETTE, 0);

        assert_eq!(mods.len(), 2);
        assert_eq!(mods[0].path, "prim\\snd\\monster\\cm_gstone_thud_a.wav");
        assert_eq!(mods[0].key_time_ms, 347);
        assert_eq!(mods[0].event, "snd_death");
        assert_eq!(mods[1].path, "prim\\snd\\monster\\cm_tomb_die.wav");
        assert_eq!(mods[1].key_time_ms, 0);
        assert_eq!(mods[1].event, "voc_death");
        // both tracks belong to the die animation of the "default" group
        for entry in &mods {
            assert_eq!(entry.set, Some((1, 4, "default".to_string())));
        }
    }

    /// The scan anchors on `.wav`, so a path-looking byte run that is not a
    /// real track record (no `hasValue`/length prefix pair in front of it)
    /// must not produce an entry — the same false-positive discipline the
    /// other palette scanners use.
    #[test]
    fn sound_mod_scan_rejects_bare_wav_bytes() {
        let mut noise: Vec<u8> = b"prim\\snd\\monster\\cm_tomb_die.wav".to_vec();
        noise.extend(vec![0xcd; 32]);
        assert!(parse_sound_mods(&noise, 0).is_empty());

        // a valid record whose keytime is absurd is not a track either
        let mut bad = TOMBSTONE_MOD_PALETTE.to_vec();
        // keytime of the first track (347 ms) -> 10 minutes
        let key = 347u32.to_le_bytes();
        let pos = bad
            .windows(4)
            .position(|w| w == &key)
            .expect("first track keytime");
        bad[pos..pos + 4].copy_from_slice(&600_000u32.to_le_bytes());
        assert_eq!(parse_sound_mods(&bad, 0).len(), 1);
    }
}

//CResAttachable.unkUInt1:
//-1 = NONE/Invalid? (for an arrow?!)
//00 = _ha
//01 = _ba (also for avatars?)
//02 = _la
//03 = _fa
//04 = _sa
//05 = _aa
//06 = Left Hand (shield, bow)
//07 = Right Hand (spear, tblade, blade, sword)
//08 =
//09 =
//10 =
//11 =
//12 =
//13 = char
//14 =
//15 =
//16 = attach

//CResAttachable.SlotId:
//00 = Hair
//01 = Face
//02 = torso_upper
//03 = torso_lower
//04 = ??? (override in avatars but never set?)
//05 = arm_upper
//06 = arm_lower
//07 = Left hand (Shield, Bow, Dagger, ...)
//08 =
//09 = Right hand (Blade, TBlade, Crossbow, Axe, ...)
//10 = Spear
//11 = pelvis
//12 = thigh
//13 = calf
//14 = attach/cape (on the back)?
