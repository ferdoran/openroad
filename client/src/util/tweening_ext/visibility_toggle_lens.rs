use bevy::prelude::{Mut, Visibility};
use bevy_tweening::Lens;

#[derive(Default)]
pub enum VisibilityOutcome {
    #[default]
    Toggle,
    Hidden,
    #[allow(dead_code)]
    Visible,
}

#[derive(Default)]
pub struct VisibilityLens(VisibilityOutcome);

#[allow(dead_code)]
impl VisibilityLens {
    pub fn toggle() -> Self {
        Self(VisibilityOutcome::Toggle)
    }

    pub fn hidden() -> Self {
        Self(VisibilityOutcome::Hidden)
    }

    pub fn visible() -> Self {
        Self(VisibilityOutcome::Visible)
    }
}

impl Lens<Visibility> for VisibilityLens {
    fn lerp(&mut self, mut target: Mut<Visibility>, ratio: f32) {
        if ratio >= 1.0 {
            match self.0 {
                VisibilityOutcome::Toggle => match *target {
                    Visibility::Inherited => {}
                    Visibility::Hidden => *target = Visibility::Visible,
                    Visibility::Visible => *target = Visibility::Hidden,
                },
                VisibilityOutcome::Hidden => *target = Visibility::Hidden,
                VisibilityOutcome::Visible => *target = Visibility::Visible,
            }
        }
    }
}
