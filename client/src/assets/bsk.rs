use std::collections::HashMap;
use std::io::Cursor;

use bevy::asset::{io::Reader, Asset, AssetLoader, LoadContext};
use bevy::math::Mat4;
use bevy::prelude::{Quat, Vec3};
use bevy::reflect::TypePath;
use bytes::Buf;
use thiserror::Error;

use crate::assets::bsk::BoneType::{PrimBone, PrimDummy};
use crate::util::buf_ext::BufExt;

/// `JMXVBSK 0101`.
const SIGNATURE_LEN: usize = 12;

#[derive(TypePath, Debug, Asset, Clone)]
pub struct JMXVBSK {
    pub int0: u32,
    pub int1: u32,
    pub bones: Vec<SkeletonBone>,
}

#[derive(Debug, Clone)]
pub struct SkeletonBone {
    pub bone_type: BoneType,
    pub name: String,
    pub parent_bone_name: String,

    pub local_rotation: Quat,
    pub local_translation: Vec3,

    pub origin_rotation: Quat,
    pub origin_translation: Vec3,

    pub parent_rotation: Quat,
    pub parent_translation: Vec3,

    pub child_bones: Vec<String>,
}

#[derive(Debug, PartialEq, Copy, Clone)]
pub enum BoneType {
    PrimBone,
    PrimDummy,
}

impl<T: Buf> From<&mut T> for JMXVBSK {
    fn from(buf: &mut T) -> Self {
        let count = buf.get_u32_le();
        let bones: Vec<SkeletonBone> = (0..count)
            .map(|_| {
                let bone_type = match buf.get_u8() {
                    1 => PrimDummy,
                    0 | _ => PrimBone,
                };

                let name = buf.get_double_len_string();
                let parent_bone_name = buf.get_double_len_string();

                let parent_rotation = Quat::from_vec4(buf.get_vec4());
                let parent_translation = buf.get_vec3();

                let origin_rotation = Quat::from_vec4(buf.get_vec4());
                let origin_translation = buf.get_vec3();

                let local_rotation = Quat::from_vec4(buf.get_vec4());
                let local_translation = buf.get_vec3();

                let child_count = buf.get_u32_le();
                let child_bones: Vec<String> = (0..child_count)
                    .map(|_| buf.get_double_len_string())
                    .collect();
                SkeletonBone {
                    bone_type,
                    name,
                    parent_bone_name,
                    local_rotation,
                    local_translation,
                    origin_rotation,
                    origin_translation,
                    parent_rotation,
                    parent_translation,
                    child_bones,
                }
            })
            .collect();

        // The trailing pair is 0/0 on every corpus file, but a malformed one
        // must not abort the process - the values are kept for inspection.
        let int0 = if buf.remaining() >= 4 {
            buf.get_u32_le()
        } else {
            0
        };
        let int1 = if buf.remaining() >= 4 {
            buf.get_u32_le()
        } else {
            0
        };
        debug_assert_eq!(int0, 0);
        debug_assert_eq!(int1, 0);
        Self { bones, int0, int1 }
    }
}

#[derive(Default, bevy::reflect::TypePath)]
pub struct BskLoader;

#[derive(Error, Debug)]
pub enum BskLoaderError {
    #[error("invalid signature: {0}")]
    Signature(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Shorter than the 12-byte signature - 4 zero-byte `.bsk` files ship in
    /// the corpus (e.g. `Data/Prim/ani/dun/demon/demon_air rock01.bsk`).
    #[error("truncated bsk: {0} bytes")]
    Truncated(usize),
}

impl AssetLoader for BskLoader {
    type Asset = JMXVBSK;
    type Settings = ();
    type Error = BskLoaderError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await?;
        if buf.len() < SIGNATURE_LEN {
            return Err(BskLoaderError::Truncated(buf.len()));
        }
        let bytes = &buf;
        let mut cursor = Cursor::new(bytes);
        let sig = cursor.get_fixed_size_string(SIGNATURE_LEN);
        if sig != "JMXVBSK 0101" {
            return Err(BskLoaderError::Signature(sig));
        }
        let skeleton = JMXVBSK::from(&mut cursor);

        Ok(skeleton)
    }

    fn extensions(&self) -> &[&str] {
        &["bsk"]
    }
}

impl SkeletonBone {
    pub fn get_parent_matrix(&self) -> Mat4 {
        Mat4::from_rotation_translation(self.parent_rotation, self.parent_translation)
    }
    pub fn get_origin_matrix(&self) -> Mat4 {
        Mat4::from_rotation_translation(self.origin_rotation, self.origin_translation)
    }
}

impl JMXVBSK {
    /// Effective parent of every bone, by name.
    ///
    /// A bone's `parent_bone_name` is the usual edge, but 82 corpus skeletons
    /// join several roots under a synthetic `[root]` super-root that links its
    /// sub-roots **only through its child-name list** (375 such edges) — those
    /// sub-roots carry an empty parent field. Walking parent fields alone
    /// therefore drops `[root]`'s transform and leaves several apparent roots.
    /// Composing both directions is what makes artifact/building/boss
    /// skeletons (`fort_stone_ht.bsk`, `flame_crazy_stand01.bsk`) resolve;
    /// Bip01 character rigs are single-rooted and unaffected.
    pub fn parent_map(&self) -> HashMap<&str, &str> {
        let mut parents: HashMap<&str, &str> = HashMap::new();
        for bone in &self.bones {
            if !bone.parent_bone_name.is_empty() {
                parents.insert(bone.name.as_str(), bone.parent_bone_name.as_str());
            }
        }
        // child lists fill in the edges the parent fields omit
        for bone in &self.bones {
            for child in &bone.child_bones {
                parents.entry(child.as_str()).or_insert(bone.name.as_str());
            }
        }
        parents
    }

    /// The skeleton's true root: the bone no other bone parents. Falls back to
    /// the first bone so a malformed file still yields something.
    pub fn root_bone(&self) -> Option<&SkeletonBone> {
        let parents = self.parent_map();
        self.bones
            .iter()
            .find(|b| !parents.contains_key(b.name.as_str()))
            .or_else(|| self.bones.first())
    }

    pub fn calculate_bind_pose_for_bone(&self, bone: &SkeletonBone) -> Mat4 {
        let map = self
            .bones
            .iter()
            .map(|b| (b.name.as_str(), b))
            .collect::<HashMap<_, _>>();
        let parents = self.parent_map();

        let mut b = bone;
        let mut matrices = vec![b.get_parent_matrix()];
        // Bounded by the bone count: a corrupt file can name a cycle, which
        // used to be an infinite loop (and `.unwrap()` on a missing parent).
        for _ in 0..self.bones.len() {
            let Some(parent_name) = parents.get(b.name.as_str()) else {
                break;
            };
            let Some(parent) = map.get(parent_name) else {
                break;
            };
            matrices.push(parent.get_parent_matrix());
            b = parent;
        }

        matrices
            .iter()
            .rev()
            .fold(Mat4::IDENTITY, |acc, m| acc * *m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bone(name: &str, parent: &str, children: &[&str], ty: f32) -> SkeletonBone {
        SkeletonBone {
            bone_type: PrimBone,
            name: name.to_string(),
            parent_bone_name: parent.to_string(),
            local_rotation: Quat::IDENTITY,
            local_translation: Vec3::ZERO,
            origin_rotation: Quat::IDENTITY,
            origin_translation: Vec3::ZERO,
            parent_rotation: Quat::IDENTITY,
            parent_translation: Vec3::new(0.0, ty, 0.0),
            child_bones: children.iter().map(|c| c.to_string()).collect(),
        }
    }

    /// The 82 multi-root skeletons: `[root]` links its sub-roots only through
    /// its child-name list, and their parent fields are empty. Parent-field
    /// walking alone dropped `[root]`'s transform (#280).
    fn multi_root() -> JMXVBSK {
        JMXVBSK {
            int0: 0,
            int1: 0,
            bones: vec![
                bone("[root]", "", &["subA", "subB"], 10.0),
                bone("subA", "", &["leafA"], 1.0),
                bone("leafA", "subA", &[], 0.5),
                bone("subB", "", &[], 2.0),
            ],
        }
    }

    #[test]
    fn child_lists_supply_the_edges_parent_fields_omit() {
        let skeleton = multi_root();
        let parents = skeleton.parent_map();
        assert_eq!(parents.get("subA"), Some(&"[root]"));
        assert_eq!(parents.get("subB"), Some(&"[root]"));
        // an explicit parent field still wins
        assert_eq!(parents.get("leafA"), Some(&"subA"));
        // the super-root itself has no parent
        assert!(!parents.contains_key("[root]"));
    }

    /// `find(|b| b.parent_bone_name.is_empty())` matched three of these four
    /// bones, so the root was whichever came first.
    #[test]
    fn the_root_is_the_bone_nobody_parents() {
        assert_eq!(
            multi_root().root_bone().map(|b| b.name.as_str()),
            Some("[root]")
        );
        // the old predicate is genuinely ambiguous here
        let ambiguous = multi_root()
            .bones
            .iter()
            .filter(|b| b.parent_bone_name.is_empty())
            .count();
        assert_eq!(ambiguous, 3, "three empty parent fields, one real root");
    }

    /// The bind pose must include `[root]`'s transform for a sub-root chain.
    #[test]
    fn the_bind_pose_includes_the_super_root_transform() {
        let skeleton = multi_root();
        let leaf = skeleton.bones.iter().find(|b| b.name == "leafA").unwrap();
        let bind = skeleton.calculate_bind_pose_for_bone(leaf);
        // [root] 10 + subA 1 + leafA 0.5
        assert_eq!(bind.w_axis.y, 11.5);
    }

    /// A cycle in a corrupt file must terminate, not spin forever.
    #[test]
    fn a_parent_cycle_terminates() {
        let skeleton = JMXVBSK {
            int0: 0,
            int1: 0,
            bones: vec![bone("a", "b", &[], 1.0), bone("b", "a", &[], 1.0)],
        };
        let a = &skeleton.bones[0];
        let _ = skeleton.calculate_bind_pose_for_bone(a);
    }
}
