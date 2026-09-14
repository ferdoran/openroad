// Loads a .bsr and all its sub-dependencies (.bms/.bmt/.ddj/.bsk/.ban)
// from a Source into plain parsed structs, reusing the client crate's
// parsers. Missing or broken sub-files become `None` slots with a warning
// so the animation-group file indices stay valid (same contract as the
// runtime's prepare_skeleton).
//
// The optional X-mirror is applied here, at ingest: the client renders SRO
// data with a `scale.x = -1` placement plus reversed triangle winding
// (util/mesh.rs), so a matching export negates X on positions/normals/
// translations, conjugates rotations across the YZ plane
// (q -> (x, -y, -z, w)) and swaps two indices per triangle. Conjugation
// distributes over parent-chain products, so bind poses computed from the
// mirrored bone transforms stay consistent with the mirrored vertices.

use std::collections::HashMap;
use std::io::Cursor;

use bevy::math::Quat;

use client::assets::ban::JMXVBAN;
use client::assets::bms::mesh::JMXVBMS;
use client::assets::bms::parse_bms;
use client::assets::bmt::material::{SroMaterial, JMXVBMT};
use client::assets::bsk::JMXVBSK;
use client::assets::bsr::bsr::{parse_bsr, ParsedBsr};
use client::util::buf_ext::BufExt;

use crate::source::{normalize, Source};
use crate::texture::ddj_to_png;

pub struct MaterialEntry {
    pub material: SroMaterial,
    /// Normalized pk2 path of the diffuse .ddj, when the material has one.
    pub texture_key: Option<String>,
}

pub struct ResourceBundle {
    pub parsed: ParsedBsr,
    /// Index-aligned with `parsed.mesh_paths`.
    pub meshes: Vec<Option<JMXVBMS>>,
    /// Keyed by lowercased material name (SRO names are case-insensitive,
    /// same rule as the client's material_label).
    pub materials: HashMap<String, MaterialEntry>,
    pub skeleton: Option<JMXVBSK>,
    /// Index-aligned with `parsed.animation_paths`.
    pub animations: Vec<Option<JMXVBAN>>,
    /// PNG bytes keyed by normalized texture path.
    pub textures: HashMap<String, Vec<u8>>,
}

pub fn load(source: &dyn Source, bsr_path: &str, mirror: bool) -> Result<ResourceBundle, String> {
    let bytes = source
        .read(bsr_path)
        .ok_or_else(|| format!("{bsr_path}: not found in source"))?;
    let parsed = parse_bsr(&bytes).map_err(|e| format!("{bsr_path}: {e}"))?;

    let meshes: Vec<Option<JMXVBMS>> = parsed
        .mesh_paths
        .iter()
        .map(|path| {
            let path = path.to_string_lossy();
            let Some(bytes) = source.read(&path) else {
                eprintln!("warning: mesh {path} not found, skipping");
                return None;
            };
            match parse_bms(&bytes) {
                Ok(bms) => Some(bms),
                Err(e) => {
                    eprintln!("warning: mesh {path} failed to parse: {e}");
                    None
                }
            }
        })
        .collect();

    let mut materials = HashMap::new();
    for material_data in &parsed.material_sets {
        let path = material_data.path.to_string_lossy().to_string();
        let Some(bytes) = source.read(&path) else {
            eprintln!("warning: material set {path} not found, skipping");
            continue;
        };
        let mut cursor = Cursor::new(bytes.as_slice());
        let sig = cursor.get_fixed_size_string(12);
        if sig != "JMXVBMT 0102" {
            eprintln!("warning: material set {path} has unexpected signature '{sig}', skipping");
            continue;
        }
        let bmt_dir = material_data
            .path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_default();
        let set = JMXVBMT::from(&mut cursor, bmt_dir.clone());
        for material in set.materials {
            // replicate BmtLoader's texture path semantics: is_relative
            // means pk2-root-relative, otherwise relative to the .bmt's dir
            let map_path = material.diffuse_map_path.to_string_lossy().to_string();
            let texture_key = if map_path.is_empty() {
                None
            } else if material.diffuse_map_is_relative {
                Some(normalize(&map_path))
            } else {
                Some(normalize(&format!("{}/{map_path}", bmt_dir.display())))
            };
            let name = material.name.to_ascii_lowercase();
            if materials.contains_key(&name) {
                eprintln!("warning: duplicate material name '{name}', keeping the first");
                continue;
            }
            materials.insert(
                name,
                MaterialEntry {
                    material,
                    texture_key,
                },
            );
        }
    }

    let mut textures = HashMap::new();
    for entry in materials.values() {
        let Some(key) = &entry.texture_key else {
            continue;
        };
        if textures.contains_key(key) {
            continue;
        }
        let Some(ddj) = source.read(key) else {
            eprintln!("warning: texture {key} not found, material stays untextured");
            continue;
        };
        match ddj_to_png(&ddj) {
            Some(png) => {
                textures.insert(key.clone(), png);
            }
            None => eprintln!(
                "warning: texture {key} has an unsupported DDS format, material stays untextured"
            ),
        }
    }

    let skeleton = parsed.skeleton.as_ref().and_then(|(path, _)| {
        let path = path.to_string_lossy();
        let Some(bytes) = source.read(&path) else {
            eprintln!("warning: skeleton {path} not found, exporting unskinned");
            return None;
        };
        let mut cursor = Cursor::new(bytes.as_slice());
        let sig = cursor.get_fixed_size_string(12);
        if sig != "JMXVBSK 0101" {
            eprintln!("warning: skeleton {path} has unexpected signature '{sig}', skipping");
            return None;
        }
        Some(JMXVBSK::from(&mut cursor))
    });

    let animations: Vec<Option<JMXVBAN>> = parsed
        .animation_paths
        .iter()
        .map(|path| {
            let path = path.to_string_lossy();
            let Some(bytes) = source.read(&path) else {
                eprintln!("warning: animation {path} not found, skipping");
                return None;
            };
            let mut cursor = Cursor::new(bytes.as_slice());
            let sig = cursor.get_fixed_size_string(12);
            if sig != "JMXVBAN 0102" {
                eprintln!("warning: animation {path} has unexpected signature '{sig}', skipping");
                return None;
            }
            Some(JMXVBAN::from(&mut cursor))
        })
        .collect();

    let mut bundle = ResourceBundle {
        parsed,
        meshes,
        materials,
        skeleton,
        animations,
        textures,
    };
    if mirror {
        mirror_x(&mut bundle);
    }
    Ok(bundle)
}

/// Conjugates a rotation by the reflection `diag(-1,1,1)`.
fn mirror_quat(q: Quat) -> Quat {
    Quat::from_xyzw(q.x, -q.y, -q.z, q.w)
}

fn mirror_x(bundle: &mut ResourceBundle) {
    for mesh in bundle.meshes.iter_mut().flatten() {
        for vertex in &mut mesh.vertex_data.vertices {
            vertex.position.x = -vertex.position.x;
            vertex.normal.x = -vertex.normal.x;
        }
        for (_, b, c) in &mut mesh.indices {
            std::mem::swap(b, c);
        }
        mesh.bounding_box.0.x = -mesh.bounding_box.0.x;
        mesh.bounding_box.1.x = -mesh.bounding_box.1.x;
        std::mem::swap(&mut mesh.bounding_box.0.x, &mut mesh.bounding_box.1.x);
    }

    if let Some(skeleton) = &mut bundle.skeleton {
        for bone in &mut skeleton.bones {
            bone.parent_rotation = mirror_quat(bone.parent_rotation);
            bone.parent_translation.x = -bone.parent_translation.x;
            bone.origin_rotation = mirror_quat(bone.origin_rotation);
            bone.origin_translation.x = -bone.origin_translation.x;
            bone.local_rotation = mirror_quat(bone.local_rotation);
            bone.local_translation.x = -bone.local_translation.x;
        }
    }

    for animation in bundle.animations.iter_mut().flatten() {
        for bone in &mut animation.animated_bones {
            for (translation, rotation) in &mut bone.keyframes {
                translation.x = -translation.x;
                *rotation = mirror_quat(*rotation);
            }
        }
    }
}
