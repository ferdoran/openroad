// Builds a self-contained binary glTF (.glb) from a loaded ResourceBundle:
// one JSON chunk (gltf-json Root) + one BIN chunk holding every accessor
// and the embedded PNG textures.
//
// Structure: node 0 is the resource root; skeleton bones become nodes
// 1..=bone_count (local TRS = the .bsk parent transform, hierarchy from
// parent_bone_name) and double as the skin joints, with inverse bind
// matrices from the accumulated parent chain (calculate_bind_pose_for_bone
// — NOT origin.inverse(), which is wrong for e.g. the EU Spine_Base).
// Primitive groups become nodes with one child node per mesh; skinned mesh
// nodes carry identity transforms (glTF ignores transforms of skinned
// nodes). SRO's two-influence vertex weights are remapped from the mesh's
// local bone-name list to skin joint ordinals. Each .ban becomes one glTF
// animation with LINEAR translation+rotation channels per bone; loop
// semantics (OneShot/Cyclic) only exist in `extras` since glTF has no
// loop flag.

use std::collections::{BTreeMap, HashMap};

use bevy::math::Quat;
use gltf_json as json;
use json::validation::Checked::Valid;
use json::validation::USize64;

use client::assets::ban::AnimationType;
use client::assets::bms::skeleton::BoneData;
use client::assets::bsk::JMXVBSK;

use crate::bundle::ResourceBundle;

struct GlbBuilder {
    root: json::Root,
    bin: Vec<u8>,
}

impl GlbBuilder {
    fn new() -> Self {
        let mut root = json::Root::default();
        root.asset.generator = Some("openroad bsr2glb".to_string());
        Self {
            root,
            bin: Vec::new(),
        }
    }

    /// Appends raw data as a 4-byte-aligned buffer view over buffer 0.
    fn push_view(
        &mut self,
        data: &[u8],
        target: Option<json::buffer::Target>,
    ) -> json::Index<json::buffer::View> {
        while self.bin.len() % 4 != 0 {
            self.bin.push(0);
        }
        let offset = self.bin.len();
        self.bin.extend_from_slice(data);
        let index = json::Index::new(self.root.buffer_views.len() as u32);
        self.root.buffer_views.push(json::buffer::View {
            buffer: json::Index::new(0),
            byte_length: USize64::from(data.len()),
            byte_offset: Some(USize64::from(offset)),
            byte_stride: None,
            name: None,
            target: target.map(Valid),
            extensions: None,
            extras: Default::default(),
        });
        index
    }

    #[allow(clippy::too_many_arguments)]
    fn push_accessor(
        &mut self,
        view: json::Index<json::buffer::View>,
        count: usize,
        component_type: json::accessor::ComponentType,
        type_: json::accessor::Type,
        min: Option<json::Value>,
        max: Option<json::Value>,
    ) -> json::Index<json::Accessor> {
        let index = json::Index::new(self.root.accessors.len() as u32);
        self.root.accessors.push(json::Accessor {
            buffer_view: Some(view),
            byte_offset: Some(USize64(0)),
            count: USize64::from(count),
            component_type: Valid(json::accessor::GenericComponentType(component_type)),
            extensions: None,
            extras: Default::default(),
            type_: Valid(type_),
            min,
            max,
            name: None,
            normalized: false,
            sparse: None,
        });
        index
    }

    fn push_f32s(
        &mut self,
        values: &[f32],
        components: usize,
        type_: json::accessor::Type,
        with_min_max: bool,
        target: Option<json::buffer::Target>,
    ) -> json::Index<json::Accessor> {
        let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        let view = self.push_view(&bytes, target);
        let count = values.len() / components;
        let (min, max) = if with_min_max && count > 0 {
            let mut min = vec![f32::MAX; components];
            let mut max = vec![f32::MIN; components];
            for chunk in values.chunks_exact(components) {
                for (i, v) in chunk.iter().enumerate() {
                    min[i] = min[i].min(*v);
                    max[i] = max[i].max(*v);
                }
            }
            (Some(json::Value::from(min)), Some(json::Value::from(max)))
        } else {
            (None, None)
        };
        self.push_accessor(
            view,
            count,
            json::accessor::ComponentType::F32,
            type_,
            min,
            max,
        )
    }

    fn push_vec2s(&mut self, values: &[[f32; 2]]) -> json::Index<json::Accessor> {
        let flat: Vec<f32> = values.iter().flatten().copied().collect();
        self.push_f32s(
            &flat,
            2,
            json::accessor::Type::Vec2,
            false,
            Some(json::buffer::Target::ArrayBuffer),
        )
    }

    fn push_vec3s(
        &mut self,
        values: &[[f32; 3]],
        with_min_max: bool,
    ) -> json::Index<json::Accessor> {
        let flat: Vec<f32> = values.iter().flatten().copied().collect();
        self.push_f32s(
            &flat,
            3,
            json::accessor::Type::Vec3,
            with_min_max,
            Some(json::buffer::Target::ArrayBuffer),
        )
    }

    fn push_vec4s(&mut self, values: &[[f32; 4]]) -> json::Index<json::Accessor> {
        let flat: Vec<f32> = values.iter().flatten().copied().collect();
        self.push_f32s(
            &flat,
            4,
            json::accessor::Type::Vec4,
            false,
            Some(json::buffer::Target::ArrayBuffer),
        )
    }

    fn push_joints_u16(&mut self, values: &[[u16; 4]]) -> json::Index<json::Accessor> {
        let bytes: Vec<u8> = values
            .iter()
            .flatten()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let view = self.push_view(&bytes, Some(json::buffer::Target::ArrayBuffer));
        self.push_accessor(
            view,
            values.len(),
            json::accessor::ComponentType::U16,
            json::accessor::Type::Vec4,
            None,
            None,
        )
    }

    fn push_indices_u16(&mut self, indices: &[u16]) -> json::Index<json::Accessor> {
        let bytes: Vec<u8> = indices.iter().flat_map(|v| v.to_le_bytes()).collect();
        let view = self.push_view(&bytes, Some(json::buffer::Target::ElementArrayBuffer));
        self.push_accessor(
            view,
            indices.len(),
            json::accessor::ComponentType::U16,
            json::accessor::Type::Scalar,
            None,
            None,
        )
    }

    /// Animation sampler input: keyframe times in seconds. min/max are
    /// required by the spec on sampler inputs.
    fn push_times(&mut self, times: &[f32]) -> json::Index<json::Accessor> {
        let bytes: Vec<u8> = times.iter().flat_map(|v| v.to_le_bytes()).collect();
        let view = self.push_view(&bytes, None);
        let min = times.iter().copied().fold(f32::MAX, f32::min);
        let max = times.iter().copied().fold(f32::MIN, f32::max);
        self.push_accessor(
            view,
            times.len(),
            json::accessor::ComponentType::F32,
            json::accessor::Type::Scalar,
            Some(json::Value::from(vec![min])),
            Some(json::Value::from(vec![max])),
        )
    }

    fn push_mat4s(&mut self, mats: &[[f32; 16]]) -> json::Index<json::Accessor> {
        let bytes: Vec<u8> = mats
            .iter()
            .flatten()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let view = self.push_view(&bytes, None);
        self.push_accessor(
            view,
            mats.len(),
            json::accessor::ComponentType::F32,
            json::accessor::Type::Mat4,
            None,
            None,
        )
    }
}

fn blank_node(name: Option<String>) -> json::Node {
    json::Node {
        camera: None,
        children: None,
        extensions: None,
        extras: Default::default(),
        matrix: None,
        mesh: None,
        name,
        rotation: None,
        scale: None,
        translation: None,
        skin: None,
        weights: None,
    }
}

pub fn safe_normalize(q: Quat) -> Quat {
    if q.length_squared() > 1e-12 {
        q.normalize()
    } else {
        Quat::IDENTITY
    }
}

/// Maps one vertex's two-influence SRO bone data onto glTF
/// JOINTS_0/WEIGHTS_0. `local_to_joint` maps the mesh's local bone list to
/// skin joint ordinals (`None` = phantom bone missing from the skeleton).
/// Weights are u16 fractions renormalized to sum 1; a vertex with no valid
/// influence at all binds fully to joint 0 — the same visual outcome as
/// the runtime's static fallback entity for phantom bones.
pub fn resolve_joint_weights(
    data: &BoneData,
    local_to_joint: &[Option<u16>],
) -> ([u16; 4], [f32; 4]) {
    let resolve = |idx: u8| -> Option<u16> {
        if idx == 0xFF {
            None
        } else {
            local_to_joint.get(idx as usize).copied().flatten()
        }
    };
    let j1 = resolve(data.index1);
    let j2 = resolve(data.index2);
    let mut w1 = if j1.is_some() {
        data.weight1 as f32 / u16::MAX as f32
    } else {
        0.0
    };
    let mut w2 = if j2.is_some() {
        data.weight2 as f32 / u16::MAX as f32
    } else {
        0.0
    };
    let sum = w1 + w2;
    if sum <= 0.0 {
        return ([0, 0, 0, 0], [1.0, 0.0, 0.0, 0.0]);
    }
    w1 /= sum;
    w2 /= sum;
    ([j1.unwrap_or(0), j2.unwrap_or(0), 0, 0], [w1, w2, 0.0, 0.0])
}

/// Inverse bind matrix of every skeleton bone, in bone (= joint) order.
pub fn inverse_bind_matrices(skeleton: &JMXVBSK) -> Vec<[f32; 16]> {
    skeleton
        .bones
        .iter()
        .map(|bone| {
            skeleton
                .calculate_bind_pose_for_bone(bone)
                .inverse()
                .to_cols_array()
        })
        .collect()
}

pub fn build_glb(bundle: &ResourceBundle, fallback_name: &str) -> Result<Vec<u8>, String> {
    let mut b = GlbBuilder::new();

    // --- nodes: resource root (0) + one node per skeleton bone (1..) ----
    let mut nodes: Vec<json::Node> = Vec::new();
    let root_name = if bundle.parsed.object_info.name.is_empty() {
        fallback_name.to_string()
    } else {
        bundle.parsed.object_info.name.clone()
    };
    nodes.push(blank_node(Some(root_name)));
    let mut root_children: Vec<json::Index<json::Node>> = Vec::new();

    let skeleton = bundle.skeleton.as_ref();
    let mut joint_by_name: HashMap<&str, u16> = HashMap::new();
    if let Some(skeleton) = skeleton {
        for (ordinal, bone) in skeleton.bones.iter().enumerate() {
            joint_by_name
                .entry(bone.name.as_str())
                .or_insert(ordinal as u16);
            let mut node = blank_node(Some(bone.name.clone()));
            node.translation = Some(bone.parent_translation.to_array());
            node.rotation = Some(json::scene::UnitQuaternion(
                safe_normalize(bone.parent_rotation).to_array(),
            ));
            nodes.push(node);
        }
        // hierarchy from parent_bone_name; parentless bones hang off the root
        for (ordinal, bone) in skeleton.bones.iter().enumerate() {
            let node_index = json::Index::new(1 + ordinal as u32);
            let parent = joint_by_name
                .get(bone.parent_bone_name.as_str())
                .copied()
                .filter(|&p| p as usize != ordinal);
            match parent {
                Some(p) => nodes[1 + p as usize]
                    .children
                    .get_or_insert_with(Vec::new)
                    .push(node_index),
                None => root_children.push(node_index),
            }
        }
    }
    let bone_node_index =
        |joint: u16| -> json::Index<json::Node> { json::Index::new(1 + joint as u32) };

    // --- materials + embedded textures ----------------------------------
    // one default repeat-wrap sampler shared by all textures
    b.root.samplers.push(json::texture::Sampler::default());
    let mut texture_index_by_key: HashMap<&str, json::Index<json::Texture>> = HashMap::new();
    let mut material_index_by_name: HashMap<String, json::Index<json::Material>> = HashMap::new();
    let mesh_material_names: Vec<Option<String>> = bundle
        .meshes
        .iter()
        .map(|mesh| mesh.as_ref().map(|m| m.material.to_ascii_lowercase()))
        .collect();
    for name in mesh_material_names.iter().flatten() {
        if material_index_by_name.contains_key(name) {
            continue;
        }
        let Some(entry) = bundle.materials.get(name) else {
            eprintln!("warning: material '{name}' referenced by a mesh but not found in any .bmt");
            continue;
        };
        let material = &entry.material;

        let base_color_texture = entry
            .texture_key
            .as_deref()
            .filter(|key| bundle.textures.contains_key(*key))
            .map(|key| {
                let texture = *texture_index_by_key.entry(key).or_insert_with(|| {
                    let png = &bundle.textures[key];
                    let view = b.push_view(png, None);
                    let image_index = json::Index::new(b.root.images.len() as u32);
                    b.root.images.push(json::Image {
                        buffer_view: Some(view),
                        mime_type: Some(json::image::MimeType("image/png".to_string())),
                        name: Some(key.to_string()),
                        uri: None,
                        extensions: None,
                        extras: Default::default(),
                    });
                    let texture_index = json::Index::new(b.root.textures.len() as u32);
                    b.root.textures.push(json::Texture {
                        name: None,
                        sampler: Some(json::Index::new(0)),
                        source: image_index,
                        extensions: None,
                        extras: Default::default(),
                    });
                    texture_index
                });
                json::texture::Info {
                    index: texture,
                    tex_coord: 0,
                    extensions: None,
                    extras: Default::default(),
                }
            });

        // matching the runtime's to_standard_material: textured materials
        // keep a white base factor (the texture carries the color),
        // untextured ones fall back to the BMT ambient color
        let base_color_factor = if base_color_texture.is_some() {
            [1.0, 1.0, 1.0, 1.0]
        } else {
            let c = material.ambient.to_srgba();
            [c.red, c.green, c.blue, c.alpha]
        };
        let masked = material.has_alpha_channel() && !bundle.parsed.alpha_is_sheen;
        let emissive = material.is_emissive();
        let mut gltf_material = json::Material {
            name: Some(material.name.clone()),
            alpha_cutoff: masked.then(|| json::material::AlphaCutoff(0.5)),
            alpha_mode: Valid(if masked {
                json::material::AlphaMode::Mask
            } else {
                json::material::AlphaMode::Opaque
            }),
            double_sided: material.is_two_sided(),
            ..Default::default()
        };
        gltf_material.pbr_metallic_roughness.base_color_factor =
            json::material::PbrBaseColorFactor(base_color_factor);
        gltf_material.pbr_metallic_roughness.metallic_factor = json::material::StrengthFactor(0.0);
        gltf_material.pbr_metallic_roughness.roughness_factor = json::material::StrengthFactor(0.8);
        if emissive {
            // self-illuminated panes/lamps glow in their own texture colors
            gltf_material.emissive_factor = json::material::EmissiveFactor([1.0, 1.0, 1.0]);
            gltf_material.emissive_texture = base_color_texture.clone();
        }
        gltf_material.pbr_metallic_roughness.base_color_texture = base_color_texture;
        let index = json::Index::new(b.root.materials.len() as u32);
        b.root.materials.push(gltf_material);
        material_index_by_name.insert(name.clone(), index);
    }

    // --- skin ------------------------------------------------------------
    let any_skinned = skeleton.is_some()
        && bundle.meshes.iter().flatten().any(|mesh| {
            mesh.bone_data
                .as_ref()
                .is_some_and(|bd| bd.bone_data.len() == mesh.vertex_data.vertices.len())
        });
    let skin_index = if let (Some(skeleton), true) = (skeleton, any_skinned) {
        let ibms = inverse_bind_matrices(skeleton);
        let ibm_accessor = b.push_mat4s(&ibms);
        let skeleton_root = skeleton
            .bones
            .iter()
            .position(|bone| !joint_by_name.contains_key(bone.parent_bone_name.as_str()))
            .unwrap_or(0) as u16;
        b.root.skins.push(json::Skin {
            extensions: None,
            extras: Default::default(),
            inverse_bind_matrices: Some(ibm_accessor),
            joints: (0..skeleton.bones.len() as u16)
                .map(bone_node_index)
                .collect(),
            name: None,
            skeleton: Some(bone_node_index(skeleton_root)),
        });
        Some(json::Index::new(0))
    } else {
        None
    };

    // --- meshes, grouped per BSR primitive group -------------------------
    // groups are what the runtime spawns; group-less resources (some
    // simple props) get a synthesized group over all meshes
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
    let referenced: std::collections::HashSet<u32> = groups
        .iter()
        .flat_map(|(_, indices)| indices)
        .copied()
        .collect();
    for index in 0..bundle.meshes.len() as u32 {
        if !referenced.contains(&index) && bundle.meshes[index as usize].is_some() {
            eprintln!("warning: mesh {index} is not referenced by any primitive group, skipping");
        }
    }

    let mut gltf_mesh_by_index: HashMap<u32, (json::Index<json::Mesh>, bool)> = HashMap::new();
    for (group_name, indices) in &groups {
        let group_node = json::Index::new(nodes.len() as u32);
        nodes.push(blank_node(Some(group_name.clone())));
        root_children.push(group_node);
        let mut group_children = Vec::new();
        for &mesh_index in indices {
            let Some(Some(mesh)) = bundle.meshes.get(mesh_index as usize) else {
                continue;
            };
            if mesh.vertex_data.vertices.is_empty() || mesh.indices.is_empty() {
                eprintln!("warning: mesh {} is empty, skipping", mesh.name);
                continue;
            }
            let (gltf_mesh, skinned) = *gltf_mesh_by_index.entry(mesh_index).or_insert_with(|| {
                let vertices = &mesh.vertex_data.vertices;
                let positions: Vec<[f32; 3]> =
                    vertices.iter().map(|v| v.position.to_array()).collect();
                let normals: Vec<[f32; 3]> = vertices.iter().map(|v| v.normal.to_array()).collect();
                let uv0: Vec<[f32; 2]> = vertices.iter().map(|v| v.uv_0.to_array()).collect();
                let mut attributes = BTreeMap::new();
                attributes.insert(
                    Valid(json::mesh::Semantic::Positions),
                    b.push_vec3s(&positions, true),
                );
                attributes.insert(
                    Valid(json::mesh::Semantic::Normals),
                    b.push_vec3s(&normals, false),
                );
                attributes.insert(
                    Valid(json::mesh::Semantic::TexCoords(0)),
                    b.push_vec2s(&uv0),
                );
                if vertices.first().is_some_and(|v| v.uv_1.is_some()) {
                    // lightmap UVs; exported without a bound texture
                    let uv1: Vec<[f32; 2]> = vertices
                        .iter()
                        .map(|v| v.uv_1.unwrap_or_default().to_array())
                        .collect();
                    attributes.insert(
                        Valid(json::mesh::Semantic::TexCoords(1)),
                        b.push_vec2s(&uv1),
                    );
                }

                let mut skinned = false;
                if let (Some(bone_data), Some(_)) = (&mesh.bone_data, skin_index) {
                    if bone_data.bone_data.len() == vertices.len() {
                        let local_to_joint: Vec<Option<u16>> = bone_data
                            .bones
                            .iter()
                            .map(|name| {
                                let joint = joint_by_name.get(name.as_str()).copied();
                                if joint.is_none() {
                                    eprintln!(
                                        "warning: mesh {} references bone '{name}' missing \
                                             from the skeleton (phantom bone)",
                                        mesh.name
                                    );
                                }
                                joint
                            })
                            .collect();
                        let mut joints = Vec::with_capacity(vertices.len());
                        let mut weights = Vec::with_capacity(vertices.len());
                        for data in &bone_data.bone_data {
                            let (j, w) = resolve_joint_weights(data, &local_to_joint);
                            joints.push(j);
                            weights.push(w);
                        }
                        attributes.insert(
                            Valid(json::mesh::Semantic::Joints(0)),
                            b.push_joints_u16(&joints),
                        );
                        attributes.insert(
                            Valid(json::mesh::Semantic::Weights(0)),
                            b.push_vec4s(&weights),
                        );
                        skinned = true;
                    }
                }

                let flat_indices: Vec<u16> = mesh
                    .indices
                    .iter()
                    .flat_map(|(a, b, c)| [*a, *b, *c])
                    .collect();
                let indices_accessor = b.push_indices_u16(&flat_indices);
                let material = material_index_by_name
                    .get(&mesh.material.to_ascii_lowercase())
                    .copied();
                let primitive = json::mesh::Primitive {
                    attributes,
                    extensions: None,
                    extras: Default::default(),
                    indices: Some(indices_accessor),
                    material,
                    mode: Valid(json::mesh::Mode::Triangles),
                    targets: None,
                };
                let index = json::Index::new(b.root.meshes.len() as u32);
                b.root.meshes.push(json::Mesh {
                    extensions: None,
                    extras: Default::default(),
                    name: Some(mesh.name.clone()),
                    primitives: vec![primitive],
                    weights: None,
                });
                (index, skinned)
            });
            let mut node = blank_node(Some(mesh.name.clone()));
            node.mesh = Some(gltf_mesh);
            if skinned {
                node.skin = skin_index;
            }
            let node_index = json::Index::new(nodes.len() as u32);
            nodes.push(node);
            group_children.push(node_index);
        }
        if !group_children.is_empty() {
            nodes[group_node.value() as usize].children = Some(group_children);
        }
    }

    // --- animations ------------------------------------------------------
    // animation names resolve through the BSR's animation groups
    // ("{group}/{type_id}:{ban name}"); loop mode + fps go to extras
    let mut group_mappings: HashMap<u32, Vec<(String, u32)>> = HashMap::new();
    for group in &bundle.parsed.primitive_animation_group {
        for animation in &group.animations {
            if animation.file_index != u32::MAX {
                group_mappings
                    .entry(animation.file_index)
                    .or_default()
                    .push((group.group_name.clone(), animation.typ));
            }
        }
    }
    if skeleton.is_some() {
        for (file_index, ban) in bundle.animations.iter().enumerate() {
            let Some(ban) = ban else { continue };
            let times: Vec<f32> = ban
                .key_frame_times
                .iter()
                .map(|t| *t as f32 / 1000.0)
                .collect();
            if times.len() < 2 {
                continue;
            }
            let full_input = b.push_times(&times);
            let mut channels = Vec::new();
            let mut samplers = Vec::new();
            for bone in &ban.animated_bones {
                let Some(&joint) = joint_by_name.get(bone.name.as_str()) else {
                    eprintln!(
                        "warning: animation {} targets bone '{}' missing from the skeleton, \
                         skipping channel",
                        ban.name, bone.name
                    );
                    continue;
                };
                let count = bone.keyframes.len().min(times.len());
                if count < 2 {
                    continue;
                }
                let input = if count == times.len() {
                    full_input
                } else {
                    eprintln!(
                        "warning: animation {} bone '{}' has {} keyframes for {} key times, \
                         truncating",
                        ban.name,
                        bone.name,
                        bone.keyframes.len(),
                        times.len()
                    );
                    b.push_times(&times[..count])
                };

                let translations: Vec<[f32; 3]> = bone.keyframes[..count]
                    .iter()
                    .map(|(t, _)| t.to_array())
                    .collect();
                let rotations: Vec<[f32; 4]> = bone.keyframes[..count]
                    .iter()
                    .map(|(_, q)| safe_normalize(*q).to_array())
                    .collect();
                let translation_output = b.push_f32s(
                    &translations.into_iter().flatten().collect::<Vec<f32>>(),
                    3,
                    json::accessor::Type::Vec3,
                    false,
                    None,
                );
                let rotation_output = b.push_f32s(
                    &rotations.into_iter().flatten().collect::<Vec<f32>>(),
                    4,
                    json::accessor::Type::Vec4,
                    false,
                    None,
                );
                for (path, output) in [
                    (json::animation::Property::Translation, translation_output),
                    (json::animation::Property::Rotation, rotation_output),
                ] {
                    let sampler = json::Index::new(samplers.len() as u32);
                    samplers.push(json::animation::Sampler {
                        extensions: None,
                        extras: Default::default(),
                        input,
                        interpolation: Valid(json::animation::Interpolation::Linear),
                        output,
                    });
                    channels.push(json::animation::Channel {
                        sampler,
                        target: json::animation::Target {
                            extensions: None,
                            extras: Default::default(),
                            node: bone_node_index(joint),
                            path: Valid(path),
                        },
                        extensions: None,
                        extras: Default::default(),
                    });
                }
            }
            if channels.is_empty() {
                continue;
            }
            let name = match group_mappings
                .get(&(file_index as u32))
                .and_then(|m| m.first())
            {
                Some((group, typ)) => format!("{group}/{typ}:{}", ban.name),
                None => ban.name.clone(),
            };
            let animation_type = match ban.animation_type {
                AnimationType::Cyclic => "Cyclic",
                AnimationType::OneShot => "OneShot",
            };
            let extras = json::extras::RawValue::from_string(format!(
                r#"{{"sro_animation_type":"{animation_type}","sro_fps":{}}}"#,
                ban.frames_per_second
            ))
            .ok();
            b.root.animations.push(json::Animation {
                extensions: None,
                extras,
                channels,
                name: Some(name),
                samplers,
            });
        }
    }

    // --- scene + GLB container -------------------------------------------
    nodes[0].children = Some(root_children);
    b.root.nodes = nodes;
    b.root.scenes.push(json::Scene {
        extensions: None,
        extras: Default::default(),
        name: None,
        nodes: vec![json::Index::new(0)],
    });
    b.root.scene = Some(json::Index::new(0));
    b.root.buffers.push(json::Buffer {
        byte_length: USize64::from(b.bin.len()),
        name: None,
        uri: None,
        extensions: None,
        extras: Default::default(),
    });
    pack_glb(&b.root, &b.bin)
}

/// Wraps the JSON + BIN chunks in the GLB container: 12-byte header, JSON
/// chunk padded to 4 with spaces, BIN chunk padded with zeros.
fn pack_glb(root: &json::Root, bin: &[u8]) -> Result<Vec<u8>, String> {
    let mut json_bytes = json::serialize::to_string(root)
        .map_err(|e| format!("glTF serialization failed: {e}"))?
        .into_bytes();
    while json_bytes.len() % 4 != 0 {
        json_bytes.push(b' ');
    }
    let mut bin_padded = bin.to_vec();
    while bin_padded.len() % 4 != 0 {
        bin_padded.push(0);
    }
    let total = 12 + 8 + json_bytes.len() + 8 + bin_padded.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json_bytes);
    out.extend_from_slice(&(bin_padded.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&bin_padded);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::{Mat4, Vec3};

    #[test]
    fn glb_chunks_are_aligned_and_sized() {
        let mut b = GlbBuilder::new();
        // 5 bytes force padding in both the view and the BIN chunk
        b.push_view(&[1, 2, 3, 4, 5], None);
        b.push_view(&[9, 9, 9], None);
        b.root.buffers.push(json::Buffer {
            byte_length: USize64::from(b.bin.len()),
            name: None,
            uri: None,
            extensions: None,
            extras: Default::default(),
        });
        let glb = pack_glb(&b.root, &b.bin).unwrap();

        assert_eq!(&glb[0..4], b"glTF");
        assert_eq!(u32::from_le_bytes(glb[4..8].try_into().unwrap()), 2);
        assert_eq!(
            u32::from_le_bytes(glb[8..12].try_into().unwrap()) as usize,
            glb.len()
        );
        let json_len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        assert_eq!(json_len % 4, 0);
        assert_eq!(&glb[16..20], b"JSON");
        let bin_start = 20 + json_len;
        let bin_len =
            u32::from_le_bytes(glb[bin_start..bin_start + 4].try_into().unwrap()) as usize;
        assert_eq!(bin_len % 4, 0);
        assert_eq!(&glb[bin_start + 4..bin_start + 8], b"BIN\0");
        assert_eq!(glb.len(), bin_start + 8 + bin_len);
        // the second view starts 4-byte aligned, after the padded first one
        assert_eq!(b.root.buffer_views[1].byte_offset, Some(USize64(8)));
    }

    #[test]
    fn position_accessor_carries_min_max() {
        let mut b = GlbBuilder::new();
        b.push_vec3s(&[[1.0, -2.0, 3.0], [-4.0, 5.0, 0.5]], true);
        let accessor = &b.root.accessors[0];
        assert_eq!(
            accessor.min,
            Some(json::Value::from(vec![-4.0_f32, -2.0, 0.5]))
        );
        assert_eq!(
            accessor.max,
            Some(json::Value::from(vec![1.0_f32, 5.0, 3.0]))
        );
    }

    #[test]
    fn sampler_input_has_min_max() {
        let mut b = GlbBuilder::new();
        b.push_times(&[0.0, 0.5, 1.25]);
        let accessor = &b.root.accessors[0];
        assert_eq!(accessor.min, Some(json::Value::from(vec![0.0_f32])));
        assert_eq!(accessor.max, Some(json::Value::from(vec![1.25_f32])));
    }

    #[test]
    fn joint_weights_remap_and_renormalize() {
        // local bone 0 -> joint 7, local bone 1 -> phantom (missing)
        let map = vec![Some(7), None];
        let data = BoneData {
            index1: 0,
            weight1: u16::MAX / 2,
            index2: 1,
            weight2: u16::MAX / 2,
        };
        let (joints, weights) = resolve_joint_weights(&data, &map);
        // the phantom influence is dropped and the rest renormalized
        assert_eq!(joints, [7, 0, 0, 0]);
        assert!((weights[0] - 1.0).abs() < 1e-6);
        assert_eq!(weights[1], 0.0);

        // 0xFF = no bone
        let data = BoneData {
            index1: 0xFF,
            weight1: 0,
            index2: 0,
            weight2: 12345,
        };
        let (joints, weights) = resolve_joint_weights(&data, &map);
        assert_eq!(joints, [0, 7, 0, 0]);
        assert!((weights[1] - 1.0).abs() < 1e-6);

        // no valid influence at all: fully bound to joint 0
        let data = BoneData {
            index1: 0xFF,
            weight1: 0,
            index2: 0xFF,
            weight2: 0,
        };
        let (joints, weights) = resolve_joint_weights(&data, &map);
        assert_eq!(joints, [0, 0, 0, 0]);
        assert_eq!(weights, [1.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn quat_mirror_matches_matrix_conjugation() {
        // the bundle's X-mirror conjugates rotations as q -> (x,-y,-z,w);
        // verify against the matrix identity M * R * M with M = diag(-1,1,1)
        let m = Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0));
        for q in [
            Quat::from_rotation_x(0.7),
            Quat::from_rotation_y(-1.2),
            Quat::from_rotation_z(2.5),
            Quat::from_euler(bevy::math::EulerRot::XYZ, 0.3, -0.8, 1.9),
        ] {
            let mirrored = Quat::from_xyzw(q.x, -q.y, -q.z, q.w);
            let expected = m * Mat4::from_quat(q) * m;
            let actual = Mat4::from_quat(mirrored);
            assert!(
                expected.abs_diff_eq(actual, 1e-5),
                "mismatch for {q:?}: {expected:?} vs {actual:?}"
            );
        }
    }
}
