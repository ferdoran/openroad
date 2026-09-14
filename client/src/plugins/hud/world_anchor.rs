//! Shared projection for the pooled world-anchored HUD overlays.
//!
//! Idea: a pooled absolute-node overlay follows a world point — each frame it
//! culls anything past a distance cutoff and projects the survivor to viewport
//! pixels. Extracted in #661 (an instance of #635) from the two modules that
//! described themselves as copies of each other and each carried their own
//! `MAX_DISTANCE = 600.0`, so the cutoff is declared once and the projection
//! exists once.
//!
//! **`chat_bubble` is the only caller today.** `hitcount` was the other one and
//! was deliberately reverted to its own projection and its own cutoff, so this
//! module no longer holds a seam between two consumers — it is a helper with
//! one user until a second adopts it. `nameplates` runs the same shape and
//! keeps its own constants: it was never part of the reviewed evidence, and
//! unifying it here would be a change nobody checked. (`quickstate`, a third
//! copy, is gone with its overhead monster HP bar.)

use bevy::prelude::*;

/// Overlays farther than this from the camera are not drawn. **openroad
/// choice**, not a value from the data.
pub const OVERLAY_MAX_DISTANCE: f32 = 600.0;

/// Whether a world anchor is close enough to the camera to be drawn.
///
/// Split out from [`project_world_anchor`] so the cutoff — the half both
/// copies had to agree on — is testable without a rendering camera.
pub fn within_overlay_range(camera_pos: Vec3, world: Vec3) -> bool {
    world.distance(camera_pos) <= OVERLAY_MAX_DISTANCE
}

/// Viewport pixels for a world anchor, or `None` when it is culled by
/// [`within_overlay_range`] or lies outside the camera's view.
pub fn project_world_anchor(
    camera: &Camera,
    camera_transform: &GlobalTransform,
    world: Vec3,
) -> Option<Vec2> {
    if !within_overlay_range(camera_transform.translation(), world) {
        return None;
    }
    camera.world_to_viewport(camera_transform, world).ok()
}

#[cfg(test)]
mod test {
    use super::*;

    /// The cutoff both modules duplicated: 600 units, and the boundary itself
    /// still draws. `hitcount` and `chat_bubble` each declared this separately,
    /// so a change to one silently desynced the overlay pair (#661).
    #[test]
    fn the_cutoff_is_declared_once_and_includes_its_boundary() {
        assert_eq!(OVERLAY_MAX_DISTANCE, 600.0);
        let eye = Vec3::new(10.0, 20.0, 30.0);
        assert!(within_overlay_range(eye, eye));
        assert!(within_overlay_range(
            eye,
            eye + Vec3::X * OVERLAY_MAX_DISTANCE
        ));
        assert!(!within_overlay_range(
            eye,
            eye + Vec3::X * (OVERLAY_MAX_DISTANCE + 0.1)
        ));
        // measured in 3d, not on the ground plane
        assert!(!within_overlay_range(eye, eye + Vec3::Y * 601.0));
    }

    /// Culling happens before projection, so a far anchor is `None` whatever
    /// the camera would have done with it — that ordering is what let both
    /// copies skip the `world_to_viewport` call entirely.
    #[test]
    fn a_culled_anchor_never_reaches_the_camera() {
        let camera = Camera::default();
        let transform = GlobalTransform::from_translation(Vec3::ZERO);
        assert_eq!(
            project_world_anchor(&camera, &transform, Vec3::X * 601.0),
            None
        );
    }
}
