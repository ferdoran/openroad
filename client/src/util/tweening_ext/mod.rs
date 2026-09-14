use bevy::color::{ColorToComponents, Srgba};
use bevy::math::{Quat, Vec3, Vec4};
use bevy::prelude::{Color, EaseFunction, Mut, Transform};
use bevy::text::TextColor;
use bevy::ui::widget::ImageNode;
use bevy_tweening::lens::{TransformPositionLens, TransformRotationLens};
use bevy_tweening::{EaseMethod, Lens, Tween};
use std::time::Duration;

use crate::util::tweening_ext::color_lens::ColorLens;
use crate::util::tweening_ext::text_fade_lens::TextFadeLens;

pub mod color_lens;
pub mod text_fade_lens;
pub mod visibility_toggle_lens;

impl Lens<ImageNode> for ColorLens {
    fn lerp(&mut self, mut target: Mut<ImageNode>, ratio: f32) {
        let color = self.start + (self.end - self.start) * ratio;
        target.color = Srgba::from_vec4(color).into();
    }
}

impl Lens<TextColor> for ColorLens {
    fn lerp(&mut self, mut target: Mut<TextColor>, ratio: f32) {
        let color = self.start + (self.end - self.start) * ratio;
        target.0 = Srgba::from_vec4(color).into();
    }
}

impl Lens<TextColor> for TextFadeLens {
    fn lerp(&mut self, mut target: Mut<TextColor>, ratio: f32) {
        if self.target_color.is_none() {
            self.target_color = Some(target.0);
        }
        let none: Vec4 = Color::NONE.to_srgba().to_vec4();
        let end: Vec4 = self.target_color.unwrap().to_srgba().to_vec4();
        target.0 = Srgba::from_vec4(none + (end - none) * ratio).into();
    }
}

/// A lens that interpolates both position and rotation of a Transform simultaneously.
/// Used to replace `Tracks<Transform>` which was removed in bevy_tweening 0.14.
pub struct TransformKeyframeLens {
    pub start_pos: Vec3,
    pub end_pos: Vec3,
    pub start_rot: Quat,
    pub end_rot: Quat,
}

impl Lens<Transform> for TransformKeyframeLens {
    fn lerp(&mut self, mut target: Mut<Transform>, ratio: f32) {
        target.translation = self.start_pos + (self.end_pos - self.start_pos) * ratio;
        target.rotation = self.start_rot.slerp(self.end_rot, ratio);
    }
}

#[allow(dead_code)]
pub fn translation_tween(start: Vec3, end: Vec3, duration: Duration) -> Tween {
    Tween::new(
        EaseMethod::EaseFunction(EaseFunction::Linear),
        duration,
        TransformPositionLens { start, end },
    )
}
#[allow(dead_code)]
pub fn rotation_tween(start: Quat, end: Quat, duration: Duration) -> Tween {
    Tween::new(
        EaseMethod::EaseFunction(EaseFunction::Linear),
        duration,
        TransformRotationLens { start, end },
    )
}

pub fn keyframe_tween(
    start_pos: Vec3,
    end_pos: Vec3,
    start_rot: Quat,
    end_rot: Quat,
    duration: Duration,
) -> Tween {
    Tween::new(
        EaseMethod::EaseFunction(EaseFunction::Linear),
        duration,
        TransformKeyframeLens {
            start_pos,
            end_pos,
            start_rot,
            end_rot,
        },
    )
}
