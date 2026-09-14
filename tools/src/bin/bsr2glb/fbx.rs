// Builds a binary FBX 7.4 file from a loaded ResourceBundle — the same
// scene the glTF builder emits (bone hierarchy + skinned meshes per
// primitive group + materials with embedded PNG textures + one animation
// stack per .ban), expressed as the FBX object graph:
//
//   Model(Null) root ─ Model(LimbNode) bones ─ NodeAttribute(Skeleton)
//                    └ Model(Null) groups ─ Model(Mesh) ─ Geometry
//   Geometry ─ Deformer(Skin) ─ SubDeformer(Cluster, one per used bone,
//     Transform = inverse bind matrix, TransformLink = global bind)
//   Material(phong) ─ Texture ─ Video (PNG embedded as Content)
//   AnimationStack ─ AnimationLayer ─ AnimationCurveNode(T/R per bone)
//     ─ AnimationCurve (KeyTime in KTime ticks, linear keys)
//
// The low-level binary encoding (header, node records, array attributes,
// footer) is fbxcel's writer; this module only produces the node tree.
// FBX pitfalls handled here: node names are "Name\x00\x01Class", UV V is
// flipped (FBX uses a bottom-left origin), matrices are 16 doubles in
// glam's to_cols_array layout (translation at 12..14), rotations are
// XYZ-order euler *degrees* (M = Rz*Ry*Rx — glam's EulerRot::ZYX, locked
// by a unit test) with per-channel ±360° unwrapping for curve continuity.
// Known limitation: euler curves interpolate linearly, not by slerp, so
// sparse keyframes crossing gimbal regions can wobble slightly.

use std::collections::HashMap;
use std::io::Cursor;

use bevy::math::{EulerRot, Mat4, Quat};
use fbxcel::low::v7400::ArrayAttributeEncoding;
use fbxcel::low::FbxVersion;
use fbxcel::writer::v7400::binary::{AttributesWriter, FbxFooter, Writer};

use client::assets::ban::AnimationType;

use crate::bundle::ResourceBundle;
use crate::gltf::{inverse_bind_matrices, resolve_joint_weights, safe_normalize};

/// FBX time unit: 46186158000 ticks per second.
const KTIME_PER_MS: i64 = 46_186_158;

/// Zlib-compress the big geometry/animation arrays (vertex data compresses
/// to a fraction of its raw f64 size; every FBX importer supports it).
const ZLIB: Option<ArrayAttributeEncoding> = Some(ArrayAttributeEncoding::Zlib);

type Sink = Cursor<Vec<u8>>;
type BinResult<T> = std::result::Result<T, fbxcel::writer::v7400::binary::Error>;

fn werr(e: impl std::fmt::Display) -> String {
    format!("fbx write failed: {e}")
}

/// `"Name\x00\x01Class"` — the FBX-binary encoding of `Class::Name`.
fn name_class(name: &str, class: &str) -> String {
    format!("{name}\u{0}\u{1}{class}")
}

/// FBX rotations are XYZ-order euler angles in degrees, X applied first
/// (M = Rz*Ry*Rx). glam's EulerRot::ZYX composes exactly that and returns
/// (z, y, x); reorder to (x, y, z) for the FBX property.
fn quat_to_fbx_euler_deg(q: Quat) -> [f64; 3] {
    let (z, y, x) = safe_normalize(q).to_euler(EulerRot::ZYX);
    [
        x.to_degrees() as f64,
        y.to_degrees() as f64,
        z.to_degrees() as f64,
    ]
}

/// Shifts `current` by whole turns so it lands within 180° of `previous` —
/// euler curves must stay continuous or interpolation spins the long way.
fn unwrap_deg(previous: f64, current: f64) -> f64 {
    let mut value = current;
    while value - previous > 180.0 {
        value -= 360.0;
    }
    while previous - value > 180.0 {
        value += 360.0;
    }
    value
}

fn mat_to_f64(mat: &Mat4) -> [f64; 16] {
    mat.to_cols_array().map(f64::from)
}

struct Fbx {
    w: Writer<Sink>,
    /// (child, parent, property): OO connection when property is None,
    /// OP otherwise. Written as the trailing Connections section.
    connections: Vec<(i64, i64, Option<&'static str>)>,
    next_id: i64,
}

impl Fbx {
    fn new() -> Result<Self, String> {
        Ok(Self {
            w: Writer::new(Cursor::new(Vec::new()), FbxVersion::V7_4).map_err(werr)?,
            connections: Vec::new(),
            next_id: 100_000_000,
        })
    }

    fn id(&mut self) -> i64 {
        self.next_id += 1;
        self.next_id
    }

    fn open(&mut self, name: &str) -> Result<(), String> {
        self.w.new_node(name).map(|_| ()).map_err(werr)
    }

    /// Opens a node whose sole own attribute is an i32 index
    /// (`LayerElementNormal: 0`, `Layer: 0`).
    fn open_indexed(&mut self, name: &str, index: i32) -> Result<(), String> {
        let mut attrs = self.w.new_node(name).map_err(werr)?;
        attrs.append_i32(index).map_err(werr)?;
        Ok(())
    }

    fn close(&mut self) -> Result<(), String> {
        self.w.close_node().map_err(werr)
    }

    /// Writes a complete node: attributes from the closure, no children.
    fn leaf<F>(&mut self, name: &str, f: F) -> Result<(), String>
    where
        F: FnOnce(&mut AttributesWriter<'_, Sink>) -> BinResult<()>,
    {
        let mut attrs = self.w.new_node(name).map_err(werr)?;
        f(&mut attrs).map_err(werr)?;
        drop(attrs);
        self.w.close_node().map_err(werr)
    }

    /// Opens an object node (`id, "Name\x00\x01Class", "Subclass"`);
    /// the caller writes children and closes it.
    fn open_object(
        &mut self,
        node: &str,
        id: i64,
        name: &str,
        class: &str,
        subclass: &str,
    ) -> Result<(), String> {
        let name_class = name_class(name, class);
        let mut attrs = self.w.new_node(node).map_err(werr)?;
        attrs.append_i64(id).map_err(werr)?;
        attrs.append_string_direct(&name_class).map_err(werr)?;
        attrs.append_string_direct(subclass).map_err(werr)?;
        Ok(())
    }

    /// Writes one Properties70 `P` entry; the closure appends the values.
    fn prop<F>(
        &mut self,
        name: &str,
        typ: &str,
        label: &str,
        flags: &str,
        f: F,
    ) -> Result<(), String>
    where
        F: FnOnce(&mut AttributesWriter<'_, Sink>) -> BinResult<()>,
    {
        self.leaf("P", |a| {
            a.append_string_direct(name)?;
            a.append_string_direct(typ)?;
            a.append_string_direct(label)?;
            a.append_string_direct(flags)?;
            f(a)
        })
    }

    fn prop_i32(&mut self, name: &str, value: i32) -> Result<(), String> {
        self.prop(name, "int", "Integer", "", |a| a.append_i32(value))
    }

    fn prop_f64(&mut self, name: &str, value: f64) -> Result<(), String> {
        self.prop(name, "double", "Number", "", |a| a.append_f64(value))
    }

    fn prop_color(&mut self, name: &str, rgb: [f64; 3]) -> Result<(), String> {
        self.prop(name, "Color", "", "A", |a| {
            a.append_f64(rgb[0])?;
            a.append_f64(rgb[1])?;
            a.append_f64(rgb[2])
        })
    }

    /// `Lcl Translation`/`Lcl Rotation`/`Lcl Scaling` (type name = property name).
    fn prop_lcl(&mut self, name: &str, value: [f64; 3]) -> Result<(), String> {
        self.prop(name, name, "", "A", |a| {
            a.append_f64(value[0])?;
            a.append_f64(value[1])?;
            a.append_f64(value[2])
        })
    }

    fn prop_ktime(&mut self, name: &str, value: i64) -> Result<(), String> {
        self.prop(name, "KTime", "Time", "", |a| a.append_i64(value))
    }

    fn connect_oo(&mut self, child: i64, parent: i64) {
        self.connections.push((child, parent, None));
    }

    fn connect_op(&mut self, child: i64, parent: i64, property: &'static str) {
        self.connections.push((child, parent, Some(property)));
    }

    fn finish(self) -> Result<Vec<u8>, String> {
        let sink = self
            .w
            .finalize_and_flush(&FbxFooter::default())
            .map_err(werr)?;
        Ok(sink.into_inner())
    }
}

pub fn build_fbx(bundle: &ResourceBundle, fallback_name: &str) -> Result<Vec<u8>, String> {
    let mut fbx = Fbx::new()?;

    let root_name = if bundle.parsed.object_info.name.is_empty() {
        fallback_name.to_string()
    } else {
        bundle.parsed.object_info.name.clone()
    };

    // ---- plan the scene (ids first: Definitions needs the counts) -------
    let skeleton = bundle.skeleton.as_ref();
    let mut joint_by_name: HashMap<&str, u16> = HashMap::new();
    let mut bone_globals: Vec<Mat4> = Vec::new();
    if let Some(skeleton) = skeleton {
        for (ordinal, bone) in skeleton.bones.iter().enumerate() {
            joint_by_name
                .entry(bone.name.as_str())
                .or_insert(ordinal as u16);
            bone_globals.push(skeleton.calculate_bind_pose_for_bone(bone));
        }
    }
    let ibms: Vec<Mat4> = skeleton
        .map(|s| {
            inverse_bind_matrices(s)
                .iter()
                .map(|cols| Mat4::from_cols_array(cols))
                .collect()
        })
        .unwrap_or_default();

    let groups: Vec<(String, Vec<u32>)> = if bundle.parsed.primitive_group.is_empty() {
        vec![(
            "default".to_string(),
            (0..bundle.meshes.len() as u32).collect(),
        )]
    } else {
        bundle
            .parsed
            .primitive_group
            .iter()
            .map(|group| (group.name.clone(), group.files_indices.clone()))
            .collect()
    };

    let root_model_id = fbx.id();
    let bone_model_ids: Vec<i64> = (0..bone_globals.len()).map(|_| fbx.id()).collect();
    let bone_attr_ids: Vec<i64> = (0..bone_globals.len()).map(|_| fbx.id()).collect();
    let group_model_ids: Vec<i64> = (0..groups.len()).map(|_| fbx.id()).collect();

    // one geometry per distinct mesh index; one model per (group, mesh)
    let mut geometry_ids: HashMap<u32, i64> = HashMap::new();
    let mut mesh_models: Vec<(i64, usize, u32)> = Vec::new(); // (model id, group ordinal, mesh index)
    for (group_ordinal, (_, indices)) in groups.iter().enumerate() {
        for &mesh_index in indices {
            let Some(Some(mesh)) = bundle.meshes.get(mesh_index as usize) else {
                continue;
            };
            if mesh.vertex_data.vertices.is_empty() || mesh.indices.is_empty() {
                continue;
            }
            if !geometry_ids.contains_key(&mesh_index) {
                let id = fbx.id();
                geometry_ids.insert(mesh_index, id);
            }
            mesh_models.push((fbx.id(), group_ordinal, mesh_index));
        }
    }

    // materials/textures actually used by the exported meshes
    let mut material_ids: HashMap<String, i64> = HashMap::new();
    let mut texture_ids: HashMap<String, (i64, i64)> = HashMap::new(); // key -> (texture, video)
    for &mesh_index in geometry_ids.keys() {
        let mesh = bundle.meshes[mesh_index as usize].as_ref().unwrap();
        let name = mesh.material.to_ascii_lowercase();
        if material_ids.contains_key(&name) {
            continue;
        }
        let Some(entry) = bundle.materials.get(&name) else {
            continue;
        };
        let id = fbx.id();
        material_ids.insert(name, id);
        if let Some(key) = entry
            .texture_key
            .as_deref()
            .filter(|key| bundle.textures.contains_key(*key))
        {
            if !texture_ids.contains_key(key) {
                let ids = (fbx.id(), fbx.id());
                texture_ids.insert(key.to_string(), ids);
            }
        }
    }

    // per skinned geometry: skin id + one cluster per bone with weights
    struct SkinPlan {
        mesh_index: u32,
        skin_id: i64,
        /// (cluster id, joint ordinal, vertex indices, weights)
        clusters: Vec<(i64, u16, Vec<i32>, Vec<f64>)>,
    }
    let mut skins: Vec<SkinPlan> = Vec::new();
    if skeleton.is_some() {
        for (&mesh_index, _) in &geometry_ids {
            let mesh = bundle.meshes[mesh_index as usize].as_ref().unwrap();
            let Some(bone_data) = &mesh.bone_data else {
                continue;
            };
            if bone_data.bone_data.len() != mesh.vertex_data.vertices.len() {
                continue;
            }
            let local_to_joint: Vec<Option<u16>> = bone_data
                .bones
                .iter()
                .map(|name| joint_by_name.get(name.as_str()).copied())
                .collect();
            let mut per_joint: HashMap<u16, (Vec<i32>, Vec<f64>)> = HashMap::new();
            for (vertex_index, data) in bone_data.bone_data.iter().enumerate() {
                let (joints, weights) = resolve_joint_weights(data, &local_to_joint);
                for (joint, weight) in joints.iter().zip(weights.iter()) {
                    if *weight > 0.0 {
                        let entry = per_joint.entry(*joint).or_default();
                        entry.0.push(vertex_index as i32);
                        entry.1.push(*weight as f64);
                    }
                }
            }
            let mut clusters: Vec<(i64, u16, Vec<i32>, Vec<f64>)> = Vec::new();
            let mut joints: Vec<u16> = per_joint.keys().copied().collect();
            joints.sort_unstable();
            for joint in joints {
                let (indexes, weights) = per_joint.remove(&joint).unwrap();
                clusters.push((fbx.id(), joint, indexes, weights));
            }
            skins.push(SkinPlan {
                mesh_index,
                skin_id: fbx.id(),
                clusters,
            });
        }
        skins.sort_by_key(|skin| skin.mesh_index);
    }
    let skinned: std::collections::HashSet<u32> = skins.iter().map(|s| s.mesh_index).collect();
    let pose_id = fbx.id();

    // animation stacks (skeleton only; euler curves target bone models)
    struct StackPlan {
        stack_id: i64,
        layer_id: i64,
        file_index: usize,
    }
    let mut stacks: Vec<StackPlan> = Vec::new();
    if skeleton.is_some() {
        for (file_index, ban) in bundle.animations.iter().enumerate() {
            let Some(ban) = ban else { continue };
            if ban.key_frame_times.len() < 2 {
                continue;
            }
            stacks.push(StackPlan {
                stack_id: fbx.id(),
                layer_id: fbx.id(),
                file_index,
            });
        }
    }
    let mut group_mappings: HashMap<u32, (String, u32)> = HashMap::new();
    for group in &bundle.parsed.primitive_animation_group {
        for animation in &group.animations {
            if animation.file_index != u32::MAX {
                group_mappings
                    .entry(animation.file_index)
                    .or_insert_with(|| (group.group_name.clone(), animation.typ));
            }
        }
    }

    // ---- header sections -------------------------------------------------
    fbx.open("FBXHeaderExtension")?;
    fbx.leaf("FBXHeaderVersion", |a| a.append_i32(1003))?;
    fbx.leaf("FBXVersion", |a| a.append_i32(7400))?;
    fbx.leaf("Creator", |a| a.append_string_direct("openroad bsr2glb"))?;
    fbx.close()?;

    fbx.open("GlobalSettings")?;
    fbx.leaf("Version", |a| a.append_i32(1000))?;
    fbx.open("Properties70")?;
    fbx.prop_i32("UpAxis", 1)?;
    fbx.prop_i32("UpAxisSign", 1)?;
    fbx.prop_i32("FrontAxis", 2)?;
    fbx.prop_i32("FrontAxisSign", 1)?;
    fbx.prop_i32("CoordAxis", 0)?;
    fbx.prop_i32("CoordAxisSign", 1)?;
    fbx.prop_i32("OriginalUpAxis", 1)?;
    fbx.prop_i32("OriginalUpAxisSign", 1)?;
    // SRO units are meters; UnitScaleFactor is cm-per-unit
    fbx.prop_f64("UnitScaleFactor", 100.0)?;
    fbx.prop_f64("OriginalUnitScaleFactor", 100.0)?;
    fbx.close()?;
    fbx.close()?;

    let document_id = fbx.id();
    fbx.open("Documents")?;
    fbx.leaf("Count", |a| a.append_i32(1))?;
    {
        // Document attrs are plain "Scene", "Scene" (no name/class encoding)
        let mut attrs = fbx.w.new_node("Document").map_err(werr)?;
        attrs.append_i64(document_id).map_err(werr)?;
        attrs.append_string_direct("Scene").map_err(werr)?;
        attrs.append_string_direct("Scene").map_err(werr)?;
    }
    fbx.open("Properties70")?;
    fbx.prop("SourceObject", "object", "", "", |_| Ok(()))?;
    fbx.prop("ActiveAnimStackName", "KString", "", "", |a| {
        a.append_string_direct("")
    })?;
    fbx.close()?;
    fbx.leaf("RootNode", |a| a.append_i64(0))?;
    fbx.close()?;
    fbx.close()?;

    fbx.open("References")?;
    fbx.close()?;

    let model_count = 1 + bone_model_ids.len() + group_model_ids.len() + mesh_models.len();
    let deformer_count = skins.iter().map(|s| 1 + s.clusters.len()).sum::<usize>();
    let object_types = [
        ("GlobalSettings", 1),
        ("Model", model_count),
        ("NodeAttribute", bone_attr_ids.len()),
        ("Geometry", geometry_ids.len()),
        ("Material", material_ids.len()),
        ("Texture", texture_ids.len()),
        ("Video", texture_ids.len()),
        ("Deformer", deformer_count),
        ("Pose", usize::from(!skins.is_empty())),
        ("AnimationStack", stacks.len()),
        ("AnimationLayer", stacks.len()),
    ];
    fbx.open("Definitions")?;
    fbx.leaf("Version", |a| a.append_i32(100))?;
    let total: usize = object_types.iter().map(|(_, count)| count).sum();
    fbx.leaf("Count", |a| a.append_i32(total as i32))?;
    for (object_type, count) in object_types {
        if count == 0 && object_type != "GlobalSettings" {
            continue;
        }
        {
            // ObjectType carries the type name as its sole attribute
            let mut attrs = fbx.w.new_node("ObjectType").map_err(werr)?;
            attrs.append_string_direct(object_type).map_err(werr)?;
        }
        fbx.leaf("Count", |a| a.append_i32(count as i32))?;
        fbx.close()?; // ObjectType
    }
    fbx.close()?;

    // ---- objects ---------------------------------------------------------
    fbx.open("Objects")?;

    // resource root
    fbx.open_object("Model", root_model_id, &root_name, "Model", "Null")?;
    fbx.leaf("Version", |a| a.append_i32(232))?;
    fbx.open("Properties70")?;
    fbx.prop_lcl("Lcl Translation", [0.0; 3])?;
    fbx.prop_lcl("Lcl Rotation", [0.0; 3])?;
    fbx.prop_lcl("Lcl Scaling", [1.0, 1.0, 1.0])?;
    fbx.close()?;
    fbx.close()?;
    fbx.connect_oo(root_model_id, 0);

    // bones
    if let Some(skeleton) = skeleton {
        for (ordinal, bone) in skeleton.bones.iter().enumerate() {
            fbx.open_object(
                "Model",
                bone_model_ids[ordinal],
                &bone.name,
                "Model",
                "LimbNode",
            )?;
            fbx.leaf("Version", |a| a.append_i32(232))?;
            fbx.open("Properties70")?;
            let translation = bone.parent_translation;
            fbx.prop_lcl(
                "Lcl Translation",
                [
                    translation.x as f64,
                    translation.y as f64,
                    translation.z as f64,
                ],
            )?;
            fbx.prop_lcl("Lcl Rotation", quat_to_fbx_euler_deg(bone.parent_rotation))?;
            fbx.prop_lcl("Lcl Scaling", [1.0, 1.0, 1.0])?;
            fbx.close()?;
            fbx.close()?;

            fbx.open_object(
                "NodeAttribute",
                bone_attr_ids[ordinal],
                &bone.name,
                "NodeAttribute",
                "LimbNode",
            )?;
            fbx.leaf("TypeFlags", |a| a.append_string_direct("Skeleton"))?;
            fbx.close()?;
            fbx.connect_oo(bone_attr_ids[ordinal], bone_model_ids[ordinal]);

            let parent = joint_by_name
                .get(bone.parent_bone_name.as_str())
                .copied()
                .filter(|&p| p as usize != ordinal);
            match parent {
                Some(p) => fbx.connect_oo(bone_model_ids[ordinal], bone_model_ids[p as usize]),
                None => fbx.connect_oo(bone_model_ids[ordinal], root_model_id),
            }
        }
    }

    // group nulls
    for (ordinal, (group_name, _)) in groups.iter().enumerate() {
        fbx.open_object(
            "Model",
            group_model_ids[ordinal],
            group_name,
            "Model",
            "Null",
        )?;
        fbx.leaf("Version", |a| a.append_i32(232))?;
        fbx.close()?;
        fbx.connect_oo(group_model_ids[ordinal], root_model_id);
    }

    // geometries (sorted for deterministic output)
    let mut geometry_order: Vec<(u32, i64)> = geometry_ids.iter().map(|(&k, &v)| (k, v)).collect();
    geometry_order.sort_unstable();
    for (mesh_index, geometry_id) in geometry_order {
        let mesh = bundle.meshes[mesh_index as usize].as_ref().unwrap();
        let vertices = &mesh.vertex_data.vertices;
        let _ = geometry_id;
        fbx.open_object(
            "Geometry",
            geometry_ids[&mesh_index],
            &mesh.name,
            "Geometry",
            "Mesh",
        )?;
        fbx.leaf("Vertices", |a| {
            a.append_arr_f64_from_iter(
                ZLIB,
                vertices.iter().flat_map(|v| {
                    [
                        v.position.x as f64,
                        v.position.y as f64,
                        v.position.z as f64,
                    ]
                }),
            )
        })?;
        fbx.leaf("PolygonVertexIndex", |a| {
            a.append_arr_i32_from_iter(
                ZLIB,
                mesh.indices
                    .iter()
                    .flat_map(|(x, y, z)| [*x as i32, *y as i32, !(*z as i32)]),
            )
        })?;
        fbx.leaf("GeometryVersion", |a| a.append_i32(124))?;

        fbx.open_indexed("LayerElementNormal", 0)?;
        fbx.leaf("Version", |a| a.append_i32(101))?;
        fbx.leaf("Name", |a| a.append_string_direct(""))?;
        fbx.leaf("MappingInformationType", |a| {
            a.append_string_direct("ByPolygonVertex")
        })?;
        fbx.leaf("ReferenceInformationType", |a| {
            a.append_string_direct("Direct")
        })?;
        fbx.leaf("Normals", |a| {
            a.append_arr_f64_from_iter(
                ZLIB,
                mesh.indices.iter().flat_map(|(x, y, z)| {
                    [*x, *y, *z].into_iter().flat_map(|i| {
                        let n = vertices[i as usize].normal;
                        [n.x as f64, n.y as f64, n.z as f64]
                    })
                }),
            )
        })?;
        fbx.close()?;

        fbx.open_indexed("LayerElementUV", 0)?;
        fbx.leaf("Version", |a| a.append_i32(101))?;
        fbx.leaf("Name", |a| a.append_string_direct("UVMap"))?;
        fbx.leaf("MappingInformationType", |a| {
            a.append_string_direct("ByPolygonVertex")
        })?;
        fbx.leaf("ReferenceInformationType", |a| {
            a.append_string_direct("IndexToDirect")
        })?;
        // FBX UV origin is bottom-left; SRO/DDS is top-left, so flip V
        fbx.leaf("UV", |a| {
            a.append_arr_f64_from_iter(
                ZLIB,
                vertices
                    .iter()
                    .flat_map(|v| [v.uv_0.x as f64, 1.0 - v.uv_0.y as f64]),
            )
        })?;
        fbx.leaf("UVIndex", |a| {
            a.append_arr_i32_from_iter(
                ZLIB,
                mesh.indices
                    .iter()
                    .flat_map(|(x, y, z)| [*x as i32, *y as i32, *z as i32]),
            )
        })?;
        fbx.close()?;

        fbx.open_indexed("LayerElementMaterial", 0)?;
        fbx.leaf("Version", |a| a.append_i32(101))?;
        fbx.leaf("Name", |a| a.append_string_direct(""))?;
        fbx.leaf("MappingInformationType", |a| {
            a.append_string_direct("AllSame")
        })?;
        fbx.leaf("ReferenceInformationType", |a| {
            a.append_string_direct("IndexToDirect")
        })?;
        fbx.leaf("Materials", |a| a.append_arr_i32_from_iter(None, [0]))?;
        fbx.close()?;

        fbx.open_indexed("Layer", 0)?;
        fbx.leaf("Version", |a| a.append_i32(100))?;
        for element_type in [
            "LayerElementNormal",
            "LayerElementUV",
            "LayerElementMaterial",
        ] {
            fbx.open("LayerElement")?;
            fbx.leaf("Type", |a| a.append_string_direct(element_type))?;
            fbx.leaf("TypedIndex", |a| a.append_i32(0))?;
            fbx.close()?;
        }
        fbx.close()?;

        fbx.close()?; // Geometry
    }

    // mesh models
    for &(model_id, group_ordinal, mesh_index) in &mesh_models {
        let mesh = bundle.meshes[mesh_index as usize].as_ref().unwrap();
        fbx.open_object("Model", model_id, &mesh.name, "Model", "Mesh")?;
        fbx.leaf("Version", |a| a.append_i32(232))?;
        fbx.open("Properties70")?;
        fbx.prop_lcl("Lcl Translation", [0.0; 3])?;
        fbx.prop_lcl("Lcl Rotation", [0.0; 3])?;
        fbx.prop_lcl("Lcl Scaling", [1.0, 1.0, 1.0])?;
        fbx.close()?;
        fbx.close()?;
        fbx.connect_oo(model_id, group_model_ids[group_ordinal]);
        fbx.connect_oo(geometry_ids[&mesh_index], model_id);
        if let Some(&material_id) = material_ids.get(&mesh.material.to_ascii_lowercase()) {
            fbx.connect_oo(material_id, model_id);
        }
    }

    // materials + textures
    for (name, &material_id) in &material_ids {
        let entry = &bundle.materials[name];
        let ambient = entry.material.ambient.to_srgba();
        let diffuse = entry.material.diffuse.to_srgba();
        fbx.open_object(
            "Material",
            material_id,
            &entry.material.name,
            "Material",
            "",
        )?;
        fbx.leaf("Version", |a| a.append_i32(102))?;
        fbx.leaf("ShadingModel", |a| a.append_string_direct("phong"))?;
        fbx.leaf("MultiLayer", |a| a.append_i32(0))?;
        fbx.open("Properties70")?;
        fbx.prop_color(
            "DiffuseColor",
            [
                diffuse.red as f64,
                diffuse.green as f64,
                diffuse.blue as f64,
            ],
        )?;
        fbx.prop_color(
            "AmbientColor",
            [
                ambient.red as f64,
                ambient.green as f64,
                ambient.blue as f64,
            ],
        )?;
        fbx.prop_f64("SpecularFactor", 0.0)?;
        fbx.prop_f64(
            "EmissiveFactor",
            if entry.material.is_emissive() {
                1.0
            } else {
                0.0
            },
        )?;
        fbx.close()?;
        fbx.close()?;

        if let Some(key) = entry
            .texture_key
            .as_deref()
            .filter(|key| texture_ids.contains_key(*key))
        {
            let (texture_id, _) = texture_ids[key];
            fbx.connect_op(texture_id, material_id, "DiffuseColor");
        }
    }

    for (key, &(texture_id, video_id)) in &texture_ids {
        let file_name = key
            .rsplit('/')
            .next()
            .unwrap_or(key)
            .replace(".ddj", ".png");
        let png = &bundle.textures[key];

        fbx.open_object("Video", video_id, &file_name, "Video", "Clip")?;
        fbx.leaf("Type", |a| a.append_string_direct("Clip"))?;
        fbx.leaf("Filename", |a| a.append_string_direct(&file_name))?;
        fbx.leaf("RelativeFilename", |a| a.append_string_direct(&file_name))?;
        fbx.leaf("Content", |a| a.append_binary_direct(png))?;
        fbx.close()?;

        fbx.open_object("Texture", texture_id, &file_name, "Texture", "")?;
        fbx.leaf("Type", |a| a.append_string_direct("TextureVideoClip"))?;
        fbx.leaf("Version", |a| a.append_i32(202))?;
        fbx.leaf("TextureName", |a| {
            a.append_string_direct(&name_class(&file_name, "Texture"))
        })?;
        fbx.leaf("Media", |a| {
            a.append_string_direct(&name_class(&file_name, "Video"))
        })?;
        fbx.leaf("FileName", |a| a.append_string_direct(&file_name))?;
        fbx.leaf("RelativeFilename", |a| a.append_string_direct(&file_name))?;
        fbx.close()?;
        fbx.connect_oo(video_id, texture_id);
    }

    // skins + clusters + bind pose
    for skin in &skins {
        let geometry_id = geometry_ids[&skin.mesh_index];
        fbx.open_object("Deformer", skin.skin_id, "", "Deformer", "Skin")?;
        fbx.leaf("Version", |a| a.append_i32(101))?;
        fbx.leaf("Link_DeformAcuracy", |a| a.append_f64(50.0))?;
        fbx.close()?;
        fbx.connect_oo(skin.skin_id, geometry_id);

        for (cluster_id, joint, indexes, weights) in &skin.clusters {
            fbx.open_object("Deformer", *cluster_id, "", "SubDeformer", "Cluster")?;
            fbx.leaf("Version", |a| a.append_i32(100))?;
            fbx.leaf("UserData", |a| {
                a.append_string_direct("")?;
                a.append_string_direct("")
            })?;
            fbx.leaf("Indexes", |a| {
                a.append_arr_i32_from_iter(ZLIB, indexes.iter().copied())
            })?;
            fbx.leaf("Weights", |a| {
                a.append_arr_f64_from_iter(ZLIB, weights.iter().copied())
            })?;
            let joint = *joint as usize;
            fbx.leaf("Transform", |a| {
                a.append_arr_f64_from_iter(None, mat_to_f64(&ibms[joint]))
            })?;
            fbx.leaf("TransformLink", |a| {
                a.append_arr_f64_from_iter(None, mat_to_f64(&bone_globals[joint]))
            })?;
            fbx.close()?;
            fbx.connect_oo(*cluster_id, skin.skin_id);
            fbx.connect_oo(bone_model_ids[joint], *cluster_id);
        }
    }

    if !skins.is_empty() {
        let pose_nodes: Vec<(i64, Mat4)> = bone_model_ids
            .iter()
            .zip(bone_globals.iter())
            .map(|(&id, &global)| (id, global))
            .chain(
                mesh_models
                    .iter()
                    .filter(|(_, _, mesh_index)| skinned.contains(mesh_index))
                    .map(|&(model_id, ..)| (model_id, Mat4::IDENTITY)),
            )
            .collect();
        fbx.open_object("Pose", pose_id, "", "Pose", "BindPose")?;
        fbx.leaf("Type", |a| a.append_string_direct("BindPose"))?;
        fbx.leaf("Version", |a| a.append_i32(100))?;
        fbx.leaf("NbPoseNodes", |a| a.append_i32(pose_nodes.len() as i32))?;
        for (node_id, matrix) in pose_nodes {
            fbx.open("PoseNode")?;
            fbx.leaf("Node", |a| a.append_i64(node_id))?;
            fbx.leaf("Matrix", |a| {
                a.append_arr_f64_from_iter(None, mat_to_f64(&matrix))
            })?;
            fbx.close()?;
        }
        fbx.close()?;
    }

    // animations
    for stack in &stacks {
        let ban = bundle.animations[stack.file_index].as_ref().unwrap();
        let times = &ban.key_frame_times;
        let end_ktime = *times.last().unwrap() as i64 * KTIME_PER_MS;
        let stack_name = match group_mappings.get(&(stack.file_index as u32)) {
            Some((group, typ)) => format!("{group}/{typ}:{}", ban.name),
            None => ban.name.clone(),
        };
        let type_suffix = match ban.animation_type {
            AnimationType::Cyclic => " [cyclic]",
            AnimationType::OneShot => "",
        };
        let stack_name = format!("{stack_name}{type_suffix}");

        fbx.open_object(
            "AnimationStack",
            stack.stack_id,
            &stack_name,
            "AnimStack",
            "",
        )?;
        fbx.open("Properties70")?;
        fbx.prop_ktime("LocalStart", 0)?;
        fbx.prop_ktime("LocalStop", end_ktime)?;
        fbx.prop_ktime("ReferenceStart", 0)?;
        fbx.prop_ktime("ReferenceStop", end_ktime)?;
        fbx.close()?;
        fbx.close()?;

        fbx.open_object(
            "AnimationLayer",
            stack.layer_id,
            "BaseLayer",
            "AnimLayer",
            "",
        )?;
        fbx.close()?;
        fbx.connect_oo(stack.layer_id, stack.stack_id);

        for bone in &ban.animated_bones {
            let Some(&joint) = joint_by_name.get(bone.name.as_str()) else {
                continue;
            };
            let count = bone.keyframes.len().min(times.len());
            if count < 2 {
                continue;
            }
            let key_times: Vec<i64> = times[..count]
                .iter()
                .map(|ms| *ms as i64 * KTIME_PER_MS)
                .collect();

            // translation: three curves off one T curve node
            let translations: Vec<[f64; 3]> = bone.keyframes[..count]
                .iter()
                .map(|(t, _)| [t.x as f64, t.y as f64, t.z as f64])
                .collect();
            // rotation: euler degrees with per-channel unwrapping
            let mut rotations: Vec<[f64; 3]> = Vec::with_capacity(count);
            for (_, q) in &bone.keyframes[..count] {
                let mut euler = quat_to_fbx_euler_deg(*q);
                if let Some(previous) = rotations.last() {
                    for i in 0..3 {
                        euler[i] = unwrap_deg(previous[i], euler[i]);
                    }
                }
                rotations.push(euler);
            }

            for (curve_node_name, property, values) in [
                ("T", "Lcl Translation", &translations),
                ("R", "Lcl Rotation", &rotations),
            ] {
                let curve_node_id = fbx.id();
                fbx.open_object(
                    "AnimationCurveNode",
                    curve_node_id,
                    curve_node_name,
                    "AnimCurveNode",
                    "",
                )?;
                fbx.open("Properties70")?;
                fbx.prop("d|X", "Number", "", "A", |a| a.append_f64(values[0][0]))?;
                fbx.prop("d|Y", "Number", "", "A", |a| a.append_f64(values[0][1]))?;
                fbx.prop("d|Z", "Number", "", "A", |a| a.append_f64(values[0][2]))?;
                fbx.close()?;
                fbx.close()?;
                fbx.connect_oo(curve_node_id, stack.layer_id);
                fbx.connect_op(curve_node_id, bone_model_ids[joint as usize], property);

                for (channel, property_name) in [(0, "d|X"), (1, "d|Y"), (2, "d|Z")] {
                    let curve_id = fbx.id();
                    fbx.open_object("AnimationCurve", curve_id, "", "AnimCurve", "")?;
                    fbx.leaf("Default", |a| a.append_f64(values[0][channel]))?;
                    fbx.leaf("KeyVer", |a| a.append_i32(4008))?;
                    fbx.leaf("KeyTime", |a| {
                        a.append_arr_i64_from_iter(ZLIB, key_times.iter().copied())
                    })?;
                    fbx.leaf("KeyValueFloat", |a| {
                        a.append_arr_f32_from_iter(ZLIB, values.iter().map(|v| v[channel] as f32))
                    })?;
                    // 0x104 = linear interpolation + auto tangent
                    fbx.leaf("KeyAttrFlags", |a| a.append_arr_i32_from_iter(None, [260]))?;
                    fbx.leaf("KeyAttrDataFloat", |a| {
                        a.append_arr_f32_from_iter(None, [0.0, 0.0, 0.0, 0.0])
                    })?;
                    fbx.leaf("KeyAttrRefCount", |a| {
                        a.append_arr_i32_from_iter(None, [count as i32])
                    })?;
                    fbx.close()?;
                    fbx.connect_op(curve_id, curve_node_id, property_name);
                }
            }
        }
    }

    fbx.close()?; // Objects

    // ---- connections -----------------------------------------------------
    fbx.open("Connections")?;
    let connections = std::mem::take(&mut fbx.connections);
    for (child, parent, property) in connections {
        fbx.leaf("C", |a| match property {
            Some(property) => {
                a.append_string_direct("OP")?;
                a.append_i64(child)?;
                a.append_i64(parent)?;
                a.append_string_direct(property)
            }
            None => {
                a.append_string_direct("OO")?;
                a.append_i64(child)?;
                a.append_i64(parent)
            }
        })?;
    }
    fbx.close()?;

    fbx.open("Takes")?;
    fbx.leaf("Current", |a| a.append_string_direct(""))?;
    fbx.close()?;

    fbx.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::Vec3;

    #[test]
    fn fbx_euler_convention_matches_rz_ry_rx() {
        // FBX eEulerXYZ applies X first: M = Rz * Ry * Rx. The conversion
        // must produce angles that rebuild the original rotation this way.
        for q in [
            Quat::from_rotation_x(0.6),
            Quat::from_rotation_y(-1.1),
            Quat::from_rotation_z(2.2),
            Quat::from_euler(EulerRot::XYZ, 0.4, -0.9, 1.7),
            Quat::from_euler(EulerRot::ZXY, -2.8, 0.2, 0.9),
        ] {
            let [x_deg, y_deg, z_deg] = quat_to_fbx_euler_deg(q);
            let rebuilt = Mat4::from_quat(Quat::from_rotation_z(z_deg.to_radians() as f32))
                * Mat4::from_quat(Quat::from_rotation_y(y_deg.to_radians() as f32))
                * Mat4::from_quat(Quat::from_rotation_x(x_deg.to_radians() as f32));
            assert!(
                Mat4::from_quat(q).abs_diff_eq(rebuilt, 1e-4),
                "euler rebuild mismatch for {q:?}"
            );
        }
    }

    #[test]
    fn euler_unwrap_keeps_channels_continuous() {
        assert_eq!(unwrap_deg(170.0, -170.0), 190.0);
        assert_eq!(unwrap_deg(-170.0, 170.0), -190.0);
        assert_eq!(unwrap_deg(10.0, 20.0), 20.0);
        assert_eq!(unwrap_deg(720.0, 5.0), 725.0);
    }

    #[test]
    fn ktime_is_exact_ticks_per_millisecond() {
        assert_eq!(KTIME_PER_MS * 1000, 46_186_158_000);
    }

    #[test]
    fn writer_produces_parseable_fbx() {
        let mut fbx = Fbx::new().unwrap();
        fbx.open("Objects").unwrap();
        fbx.open_object("Model", 42, "thing", "Model", "Null")
            .unwrap();
        fbx.leaf("Version", |a| a.append_i32(232)).unwrap();
        fbx.close().unwrap();
        fbx.close().unwrap();
        fbx.connect_oo(42, 0);
        fbx.open("Connections").unwrap();
        let connections = std::mem::take(&mut fbx.connections);
        for (child, parent, _) in connections {
            fbx.leaf("C", |a| {
                a.append_string_direct("OO")?;
                a.append_i64(child)?;
                a.append_i64(parent)
            })
            .unwrap();
        }
        fbx.close().unwrap();
        let bytes = fbx.finish().unwrap();

        let tree = fbxcel::tree::any::AnyTree::from_seekable_reader(Cursor::new(bytes)).unwrap();
        let fbxcel::tree::any::AnyTree::V7400(version, tree, _) = tree else {
            panic!("unexpected tree version");
        };
        assert_eq!(version, FbxVersion::V7_4);
        let objects = tree
            .root()
            .first_child_by_name("Objects")
            .expect("Objects node present");
        let model = objects.first_child_by_name("Model").expect("Model present");
        assert!(matches!(
            model.attributes()[0],
            fbxcel::low::v7400::AttributeValue::I64(42)
        ));
        assert!(matches!(
            &model.attributes()[1],
            fbxcel::low::v7400::AttributeValue::String(s) if s == "thing\u{0}\u{1}Model"
        ));
        assert!(tree.root().first_child_by_name("Connections").is_some());
    }

    #[test]
    fn mirror_and_matrix_layout_roundtrip() {
        // FBX stores matrices as 16 doubles with translation at 12..14 —
        // the same memory layout as glam's to_cols_array
        let m =
            Mat4::from_rotation_translation(Quat::from_rotation_y(0.5), Vec3::new(1.0, 2.0, 3.0));
        let arr = mat_to_f64(&m);
        assert!((arr[12] - 1.0).abs() < 1e-6);
        assert!((arr[13] - 2.0).abs() < 1e-6);
        assert!((arr[14] - 3.0).abs() < 1e-6);
        assert!((arr[15] - 1.0).abs() < 1e-6);
    }
}
