use std::time::Duration;

use crate::assets::intro_scene::CameraKeyframe;
use bevy::asset::io::Reader;
use bevy::asset::{AssetLoader, LoadContext};
use bevy::math::{EulerRot, Quat};
use bevy::prelude::{Asset, TypePath, Vec3};
use bevy_tweening::Sequence;
use serde_derive::{Deserialize, Serialize};
use thiserror::Error;

use crate::util::tweening_ext::keyframe_tween;

#[derive(TypePath, Asset, Serialize, Deserialize, Default, Debug, Clone)]
pub struct CharSelectScene {
    name: String,
    cam_base: Vec3,
    cam_offset: Vec3,
    char_start_offset: Vec3,
    char_end_offset: Vec3,
    initial_camera_transforms: Vec<CameraKeyframe>,
    select_camera_transforms: Vec<CameraKeyframe>,
    create_camera_transforms: Vec<CameraKeyframe>,
}

#[derive(Error, Debug)]
pub enum CharSelectSceneLoaderError {
    #[error("failed to parse yaml: {0}")]
    YAML(serde_yaml::Error),
    #[error("IO Error: {0}")]
    IO(std::io::Error),
}

impl AssetLoader for CharSelectScene {
    type Asset = CharSelectScene;
    type Settings = ();
    type Error = CharSelectSceneLoaderError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut buf = Vec::new();
        let _ = reader
            .read_to_end(&mut buf)
            .await
            .map_err(|e| CharSelectSceneLoaderError::IO(e));
        let scene: Self::Asset =
            serde_yaml::from_slice(&buf).map_err(|e| CharSelectSceneLoaderError::YAML(e))?;
        Ok(scene)
    }

    fn extensions(&self) -> &[&str] {
        &["selection"]
    }
}

impl CameraKeyframe {
    fn rotation_without_y_offset(&self) -> Quat {
        Quat::from_euler(
            EulerRot::XYZ,
            self.rotation.x,
            self.rotation.y,
            self.rotation.z,
        )
    }
}

impl CharSelectScene {
    #[allow(dead_code)]
    pub fn name(&self) -> &String {
        &self.name
    }

    pub fn cam_base(&self) -> Vec3 {
        self.cam_base * Vec3::new(1920.0, 0.0, 1920.0)
    }
    pub fn cam_offset(&self) -> Vec3 {
        self.cam_offset
    }
    pub fn char_start_offset(&self) -> Vec3 {
        self.char_start_offset
    }
    pub fn char_end_offset(&self) -> Vec3 {
        self.char_end_offset
    }

    pub fn get_init_camera_anim(&self, origin: Vec3) -> Sequence {
        let mut tweens = Vec::with_capacity(self.initial_camera_transforms.len() - 1);

        for i in 0..self.initial_camera_transforms.len() - 1 {
            let start = self.initial_camera_transforms[i];
            let end = self.initial_camera_transforms[i + 1];

            let duration = Duration::from_secs_f32(end.frame - start.frame);

            tweens.push(keyframe_tween(
                start.translation(origin),
                end.translation(origin),
                start.rotation_without_y_offset(),
                end.rotation_without_y_offset(),
                duration,
            ));
        }

        Sequence::new(tweens)
    }

    /// Final pose of the initial camera animation: the canonical zoomed-out
    /// view the camera returns to when a selection is cancelled. Using the
    /// last keyframe (instead of the live camera transform) keeps the zoom-out
    /// target deterministic even when a character is clicked mid-animation.
    pub fn init_camera_end_pose(&self, origin: Vec3) -> Option<(Vec3, Quat)> {
        self.initial_camera_transforms
            .last()
            .map(|kf| (kf.translation(origin), kf.rotation_without_y_offset()))
    }

    /// Held camera pose of the character-creation stage: the last (typically
    /// only) keyframe of `create_camera_transforms`. A single keyframe cannot
    /// form a tween, so creation sets this pose directly instead of replaying
    /// the char-select fly-in.
    pub fn create_camera_pose(&self, origin: Vec3) -> Option<(Vec3, Quat)> {
        self.create_camera_transforms
            .last()
            .map(|kf| (kf.translation(origin), kf.rotation_without_y_offset()))
    }

    #[allow(dead_code)]
    pub fn get_select_camera_anim(&self, origin: Vec3) -> Sequence {
        let mut tweens = Vec::with_capacity(self.select_camera_transforms.len() - 1);

        for i in 0..self.select_camera_transforms.len() - 1 {
            let start = self.select_camera_transforms[i];
            let end = self.select_camera_transforms[i + 1];

            let duration = Duration::from_secs_f32(end.frame - start.frame);

            tweens.push(keyframe_tween(
                start.translation(origin),
                end.translation(origin),
                start.rotation_without_y_offset(),
                end.rotation_without_y_offset(),
                duration,
            ));
        }

        Sequence::new(tweens)
    }
}

#[cfg(test)]
mod tests {
    use crate::assets::char_select_scene::CharSelectScene;

    #[test]
    fn rejects_empty_scene() {
        let yaml = r#"
        "#;

        let parsed = serde_yaml::from_str::<CharSelectScene>(&yaml);
        assert!(parsed.is_err());
    }
}
