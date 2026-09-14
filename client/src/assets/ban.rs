use std::collections::HashMap;
use std::io::Cursor;
use std::time::Duration;

use bevy::animation::prelude::AnimatedField;
use bevy::animation::{animated_field, AnimationClip, AnimationTargetId};
use bevy::asset::{io::Reader, Asset, AssetLoader, LoadContext};
use bevy::math::curve::UnevenSampleAutoCurve;
use bevy::prelude::{warn, AnimatableCurve, Name, Quat, Transform, Vec3};
use bevy::reflect::TypePath;
use bytes::Buf;
use thiserror::Error;

use crate::assets::bsk::JMXVBSK;
use crate::util::buf_ext::BufExt;

#[derive(TypePath, Debug, Asset)]
pub struct JMXVBAN {
    pub int0: i32,
    pub int1: i32,
    pub name: String,
    pub duration: Duration,
    pub frames_per_second: i32,
    pub animation_type: AnimationType,
    pub key_frame_times: Vec<u32>,
    pub animated_bones: Vec<AnimatedBone>,
}

#[derive(Debug)]
pub struct AnimatedBone {
    pub name: String,
    pub keyframes: Vec<(Vec3, Quat)>,
}

#[derive(Debug)]
pub enum AnimationType {
    OneShot,
    Cyclic,
}

impl<T: Buf> From<&mut T> for JMXVBAN {
    fn from(buf: &mut T) -> Self {
        let int0 = buf.get_i32_le();
        let int1 = buf.get_i32_le();
        let name = buf.get_double_len_string();
        let duration = Duration::from_millis(buf.get_i32_le() as u64);
        let frames_per_second = buf.get_i32_le();
        let animation_type = match buf.get_i32_le() {
            1 => AnimationType::Cyclic,
            0 | _ => AnimationType::OneShot,
        };

        let num_keyframe_times = buf.get_u32_le();
        let key_frame_times: Vec<u32> = (0..num_keyframe_times).map(|_| buf.get_u32_le()).collect();

        let num_animated_bones = buf.get_u32_le();
        let animated_bones: Vec<AnimatedBone> = (0..num_animated_bones)
            .map(|_| {
                let name = buf.get_double_len_string();
                let count = buf.get_u32_le();
                let keyframes: Vec<(Vec3, Quat)> = (0..count)
                    .map(|_| {
                        let quat = buf.get_vec4();
                        let quat = Quat::from_vec4(quat);
                        let trans = buf.get_vec3();
                        (trans, quat)
                    })
                    .collect();
                AnimatedBone { name, keyframes }
            })
            .collect();

        Self {
            int0,
            int1,
            name,
            duration,
            frames_per_second,
            animation_type,
            key_frame_times,
            animated_bones,
        }
    }
}

#[derive(Default, bevy::reflect::TypePath)]
pub struct BanLoader;

#[derive(Error, Debug)]
pub enum BanLoaderError {
    #[error("invalid signature: {0}")]
    Signature(String),
}

impl AssetLoader for BanLoader {
    type Asset = JMXVBAN;
    type Settings = ();
    type Error = BanLoaderError;
    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await.unwrap();
        let bytes = &buf;
        let mut cursor = Cursor::new(bytes);
        let sig = cursor.get_fixed_size_string(12);
        if sig != "JMXVBAN 0102" {
            return Err(BanLoaderError::Signature(sig));
        }
        let animation = JMXVBAN::from(&mut cursor);
        // let anim_clip = AnimationClip::from(&animation);
        // info!("loaded animation");
        Ok(animation)
    }

    fn extensions(&self) -> &[&str] {
        &["ban"]
    }
}

impl From<&JMXVBAN> for AnimationClip {
    fn from(ban: &JMXVBAN) -> Self {
        // Note: This impl requires skeleton context for proper entity paths.
        // Use JMXVBAN::to_animation_clip(skeleton, wrapper_name) instead for full animation.
        let _ = ban;
        AnimationClip::default()
    }
}

impl JMXVBAN {
    pub fn to_animation_clip(
        &self,
        skeleton: &JMXVBSK,
        wrapper_entity_name: &String,
    ) -> AnimationClip {
        let mut anim_clip = AnimationClip::default();
        let mut keyframe_times = Vec::with_capacity(self.key_frame_times.len());
        for time in &self.key_frame_times {
            keyframe_times.push(*time as f32 / 1000.0);
        }

        let mut parent_bones = HashMap::new();
        let mut bones = HashMap::new();
        for bone in &skeleton.bones {
            bones.insert(
                bone.name.clone(),
                (bone.origin_translation, bone.origin_rotation),
            );
            if !parent_bones.contains_key(&bone.name) && !bone.parent_bone_name.is_empty() {
                parent_bones.insert(bone.name.clone(), bone.parent_bone_name.clone());
            }
        }

        for animated_bone in &self.animated_bones {
            let target_id = bone_target_id(&animated_bone.name, &parent_bones, wrapper_entity_name);

            let mut translations = Vec::with_capacity(animated_bone.keyframes.len());
            let mut rotations = Vec::with_capacity(animated_bone.keyframes.len());
            for (translation, rotation) in &animated_bone.keyframes {
                translations.push(*translation);
                rotations.push(*rotation);
            }

            if translations.len() != keyframe_times.len() || rotations.len() != keyframe_times.len()
            {
                warn!(
                    "different translation length… {}/{}",
                    translations.len(),
                    keyframe_times.len()
                );
                warn!(
                    "different rotations length… {}/{}",
                    rotations.len(),
                    rotations.len()
                );
            }

            if keyframe_times.len() < 2 || translations.len() < 2 || rotations.len() < 2 {
                continue;
            }

            let translation_curve = UnevenSampleAutoCurve::new(
                keyframe_times
                    .iter()
                    .zip(translations.iter())
                    .map(|(t, v)| (*t, *v)),
            );
            if let Ok(curve) = translation_curve {
                anim_clip.add_curve_to_target(
                    target_id,
                    AnimatableCurve::new(animated_field!(Transform::translation), curve),
                );
            }

            let rotation_curve = UnevenSampleAutoCurve::new(
                keyframe_times
                    .iter()
                    .zip(rotations.iter())
                    .map(|(t, v)| (*t, *v)),
            );
            if let Ok(curve) = rotation_curve {
                anim_clip.add_curve_to_target(
                    target_id,
                    AnimatableCurve::new(animated_field!(Transform::rotation), curve),
                );
            }
        }

        anim_clip
    }
}

/// Computes the [`AnimationTargetId`] for a bone from the name path
/// `[wrapper, ..., parent, bone]`. Clip curves ([`JMXVBAN::to_animation_clip`])
/// and the spawned bone entities must use this same function, otherwise the
/// hashes differ and the curves silently animate nothing.
pub fn bone_target_id(
    bone_name: &str,
    parent_bones: &HashMap<String, String>,
    wrapper_entity_name: &str,
) -> AnimationTargetId {
    let mut entity_path_parts = vec![bone_name.to_string()];
    add_parent_bones(parent_bones, &mut entity_path_parts, &bone_name.to_string());
    entity_path_parts.push(wrapper_entity_name.to_string());
    entity_path_parts.reverse();
    let parts = entity_path_parts
        .iter()
        .map(|bone| Name::from(bone.clone()))
        .collect::<Vec<Name>>();
    AnimationTargetId::from_names(parts.iter())
}

fn add_parent_bones(
    parent_bones: &HashMap<String, String>,
    entity_path: &mut Vec<String>,
    bone: &String,
) {
    if let Some(parent) = parent_bones.get(bone) {
        entity_path.push(parent.clone());
        add_parent_bones(parent_bones, entity_path, parent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bone_target_id_hashes_full_path_from_wrapper_to_bone() {
        let mut parent_bones = HashMap::new();
        parent_bones.insert("bone".to_string(), "parent".to_string());
        parent_bones.insert("parent".to_string(), "grandparent".to_string());

        let expected = AnimationTargetId::from_names(
            [
                Name::from("wrapper"),
                Name::from("grandparent"),
                Name::from("parent"),
                Name::from("bone"),
            ]
            .iter(),
        );
        assert_eq!(bone_target_id("bone", &parent_bones, "wrapper"), expected);

        let root_expected = AnimationTargetId::from_names(
            [Name::from("wrapper"), Name::from("grandparent")].iter(),
        );
        assert_eq!(
            bone_target_id("grandparent", &parent_bones, "wrapper"),
            root_expected
        );
    }
}
