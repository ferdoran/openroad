use bevy::prelude::*;

/// The original game authors triangles clockwise, while Bevy/wgpu treats
/// counter-clockwise as front-facing (`FrontFace::Ccw`, cull `Back`). wgpu
/// decides front/back from the triangle's *screen-space* signed area after the
/// full model matrix, so a mirroring (negative-determinant) placement transform
/// flips a triangle's effective winding. Every SRO mesh is placed under such a
/// mirror (the `scale.x = -1` LH -> RH conversion applied to characters,
/// world objects and terrain), so its winding must be reversed to keep the
/// front faces from being culled.
///
/// This is the single source of truth for that decision: reverse a mesh's
/// winding iff its effective placement transform has a negative determinant.
/// Non-mirrored placements (e.g. the water/ice plane, whose double `-1` scale
/// cancels to a positive determinant) keep the source winding.
pub fn needs_winding_reversal(matrix: &Mat4) -> bool {
    matrix.determinant() < 0.0
}

/// Apply the SRO -> Bevy handedness mirror to a placement transform.
///
/// This *forces* `scale.x` negative rather than flipping its sign, which makes
/// it **idempotent**: mirroring an already-mirrored transform is a no-op. That
/// matters because a caller can legitimately hand back a transform it read off
/// a live entity — the dev respawn button does exactly that
/// (`plugins::dev::player_config`), and with a sign-flip it silently
/// un-mirrored the character and dropped its winding reversal.
///
/// The magnitude is preserved, so a caller that already scaled the entity (a
/// unique monster's rarity size) keeps its size and still ends up with the
/// negative determinant [`needs_winding_reversal`] looks for. Scaling *after*
/// mirroring is likewise safe, since multiplying by a positive factor cannot
/// change the sign.
pub fn mirrored(mut transform: Transform) -> Transform {
    transform.scale.x = -transform.scale.x.abs();
    transform
}

/// Reverses the winding order of a triangle-list index buffer in place by
/// swapping the 2nd and 3rd vertex of every triangle. Used for the terrain
/// `u32` index buffers; `.bms` meshes reverse their `u16` indices inline while
/// building the mesh (see `JMXVBMS::to_mesh`).
pub fn reverse_winding_u32(indices: &mut [u32]) {
    for tri in indices.chunks_exact_mut(3) {
        tri.swap(1, 2);
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// The mirror is what makes the placement determinant negative, which is
    /// the whole trigger for reversing a mesh's winding.
    #[test]
    fn mirroring_flips_the_determinant() {
        let plain = Transform::from_translation(Vec3::new(1.0, 2.0, 3.0));
        assert!(!needs_winding_reversal(&plain.to_matrix()));
        assert!(needs_winding_reversal(&mirrored(plain).to_matrix()));
    }

    /// A unique monster's rarity scale must survive the mirror — assigning
    /// `-1.0` outright would reset every such body to unit size.
    #[test]
    fn mirroring_preserves_an_existing_scale() {
        let scaled = Transform::from_scale(Vec3::splat(1.4));
        let mirrored = mirrored(scaled);
        assert_eq!(mirrored.scale, Vec3::new(-1.4, 1.4, 1.4));
        assert!(needs_winding_reversal(&mirrored.to_matrix()));
    }

    /// **Idempotence is the safety property**, and the regression guard for a
    /// real bug: the dev respawn button reads the live player's transform and
    /// feeds it straight back in. While this flipped the sign, that second
    /// application returned `scale.x` to `+1`, took the determinant positive
    /// and dropped the winding reversal — silently un-mirroring the character
    /// on every "Apply & Respawn".
    #[test]
    fn mirroring_an_already_mirrored_transform_is_a_no_op() {
        let once = mirrored(Transform::from_translation(Vec3::X).with_scale(Vec3::splat(2.0)));
        let twice = mirrored(once);
        assert_eq!(twice.scale, once.scale);
        assert!(needs_winding_reversal(&twice.to_matrix()));
    }

    /// Scaling after mirroring keeps the mirror: `game_scene` multiplies a
    /// monster's rarity size onto the already-mirrored base, and a positive
    /// factor cannot change the sign.
    #[test]
    fn scaling_after_mirroring_preserves_the_determinant() {
        let mut transform = mirrored(Transform::IDENTITY);
        transform.scale *= 1.4;
        assert_eq!(transform.scale, Vec3::new(-1.4, 1.4, 1.4));
        assert!(needs_winding_reversal(&transform.to_matrix()));
    }
}
