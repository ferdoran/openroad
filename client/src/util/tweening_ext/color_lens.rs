use bevy::color::{ColorToComponents, Srgba};
use bevy::prelude::{Color, Vec4};

pub struct ColorLens {
    pub(crate) start: Vec4,
    pub(crate) end: Vec4,
}

impl Default for ColorLens {
    fn default() -> Self {
        Self {
            start: Color::WHITE.to_srgba().to_vec4(),
            end: Srgba::new(1.0, 1.0, 1.0, 0.0).to_vec4(),
        }
    }
}

impl ColorLens {
    pub fn fade_transparent() -> Self {
        Self::default()
    }

    pub fn fade_opaque() -> Self {
        Self {
            start: Srgba::new(1.0, 1.0, 1.0, 0.0).to_vec4(),
            end: Color::WHITE.to_srgba().to_vec4(),
        }
    }
}
