//! Ribbon meshes for RenderLinkPipe / RenderLinkDPipe nodes (weapon slashes,
//! motion trails). Each trail node samples its local anchor segment every
//! frame and rebuilds a triangle ribbon through the recent history.

use std::collections::VecDeque;

use bevy::asset::{Assets, RenderAssetUsages};
use bevy::camera::primitives::Aabb;
use bevy::math::{Vec2, Vec3};
use bevy::mesh::{Indices, Mesh, PrimitiveTopology, VertexAttributeValues};
use bevy::prelude::{Component, GlobalTransform, Handle, Query, Res, ResMut, Time, Without};

use crate::plugins::effects::components::EffectSimPaused;

/// Fallback ribbon sample age for trail nodes with a 0-length timeline. Nodes
/// with an authored frame window derive their age from it instead (see
/// `spawn::trail_max_age`); LinkMode is unrelated — exe RE showed it is
/// parent-chain link topology, not trail length.
pub const TRAIL_MAX_AGE: f32 = 0.5;

/// Minimum world-space movement before a new sample is recorded.
const MIN_SAMPLE_DISTANCE_SQ: f32 = 0.0001;

pub struct TrailSample {
    pub a: Vec3,
    pub b: Vec3,
    pub age: f32,
}

#[derive(Component)]
pub struct EffectTrail {
    pub mesh: Handle<Mesh>,
    pub history: VecDeque<TrailSample>,
    pub max_age: f32,
    /// Whether the mesh currently holds no ribbon: empty trails skip the
    /// rebuild (and the GPU reupload a `meshes.get_mut` alone would cause).
    empty: bool,
}

impl EffectTrail {
    pub fn new(mesh: Handle<Mesh>, max_age: f32) -> Self {
        Self {
            mesh,
            history: VecDeque::new(),
            max_age,
            empty: true,
        }
    }
}

/// An empty dynamic mesh a trail can be rebuilt into every frame.
pub fn empty_trail_mesh() -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    clear_ribbon(&mut mesh);
    mesh
}

pub fn update_effect_trails(
    time: Res<Time>,
    speed: Res<crate::plugins::effects::EffectPlaybackSpeed>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut trails: Query<
        (
            &mut EffectTrail,
            &GlobalTransform,
            &mut Aabb,
            bevy::prelude::Has<crate::plugins::effects::components::PooledParticle>,
        ),
        Without<EffectSimPaused>,
    >,
) {
    let dt = time.delta_secs() * speed.0;
    for (mut trail, global, mut aabb, pooled) in &mut trails {
        // Parked (pooled) trail particles drop their ribbon so a re-arm
        // starts clean instead of flashing the previous life's ghost.
        if pooled {
            if !trail.empty {
                trail.history.clear();
                trail.empty = true;
                if let Some(mut mesh) = meshes.get_mut(&trail.mesh) {
                    clear_ribbon(&mut mesh);
                }
                *aabb = Aabb::default();
            }
            continue;
        }
        for sample in trail.history.iter_mut() {
            sample.age += dt;
        }
        let max_age = trail.max_age;
        while trail.history.front().is_some_and(|s| s.age > max_age) {
            trail.history.pop_front();
        }

        // The anchor segment: the node's local Y extent (unit segment scaled
        // by the node's animated scale, via the global transform).
        let a = global.transform_point(Vec3::new(0.0, -0.5, 0.0));
        let b = global.transform_point(Vec3::new(0.0, 0.5, 0.0));
        let moved = trail
            .history
            .back()
            .map(|last| {
                (last.a - a).length_squared() + (last.b - b).length_squared()
                    > MIN_SAMPLE_DISTANCE_SQ
            })
            .unwrap_or(true);
        if moved {
            trail.history.push_back(TrailSample { a, b, age: 0.0 });
        } else if let Some(last) = trail.history.back_mut() {
            last.age = 0.0;
        }

        // A ribbon needs 2+ samples. Non-empty trails must rebuild every
        // frame (the fade alphas age even when the shape is static), but
        // an empty one only needs a single clearing rebuild — touching the
        // mesh asset after that would reupload nothing, every frame.
        if trail.history.len() < 2 {
            if !trail.empty {
                trail.empty = true;
                if let Some(mut mesh) = meshes.get_mut(&trail.mesh) {
                    clear_ribbon(&mut mesh);
                }
                *aabb = Aabb::default();
            }
            continue;
        }
        trail.empty = false;
        let Some(mut mesh) = meshes.get_mut(&trail.mesh) else {
            continue;
        };
        *aabb = rebuild_ribbon(&trail, global, &mut mesh);
    }
}

/// Takes an attribute's buffer out of the mesh so a rebuild can reuse its
/// allocation instead of growing a fresh `Vec` every frame.
macro_rules! take_attribute {
    ($mesh:expr, $attr:expr, $variant:ident) => {
        match $mesh.remove_attribute($attr) {
            Some(VertexAttributeValues::$variant(mut v)) => {
                v.clear();
                v
            }
            _ => Vec::new(),
        }
    };
}

/// Rebuilds the ribbon vertices in node-local space (the entity's transform
/// is applied by the renderer, so world samples are pulled back through the
/// current global inverse). Returns the local-space bounds of the rebuilt
/// ribbon, so the entity's `Aabb` stays valid for frustum culling — trails
/// used to opt out of culling entirely because their bounds went stale.
fn rebuild_ribbon(trail: &EffectTrail, global: &GlobalTransform, mesh: &mut Mesh) -> Aabb {
    let n = trail.history.len();
    debug_assert!(n >= 2);

    let inverse = global.to_matrix().inverse();
    let mut positions = take_attribute!(mesh, Mesh::ATTRIBUTE_POSITION, Float32x3);
    let mut uvs = take_attribute!(mesh, Mesh::ATTRIBUTE_UV_0, Float32x2);
    let mut colors = take_attribute!(mesh, Mesh::ATTRIBUTE_COLOR, Float32x4);
    let mut min = Vec3::splat(f32::MAX);
    let mut max = Vec3::splat(f32::MIN);
    for (i, sample) in trail.history.iter().enumerate() {
        let a = inverse.transform_point3(sample.a);
        let b = inverse.transform_point3(sample.b);
        min = min.min(a).min(b);
        max = max.max(a).max(b);
        positions.push([a.x, a.y, a.z]);
        positions.push([b.x, b.y, b.z]);
        // U runs along the trail from oldest (0) to newest (1) so the
        // texture streaks behind the motion; V spans the segment.
        let u = i as f32 / (n - 1) as f32;
        uvs.push(Vec2::new(u, 0.0).to_array());
        uvs.push(Vec2::new(u, 1.0).to_array());
        // Older samples fade out per-vertex.
        let alpha = 1.0 - (sample.age / trail.max_age).clamp(0.0, 1.0);
        colors.push([1.0, 1.0, 1.0, alpha]);
        colors.push([1.0, 1.0, 1.0, alpha]);
    }

    let mut indices = match mesh.remove_indices() {
        Some(Indices::U32(mut v)) => {
            v.clear();
            v
        }
        _ => Vec::new(),
    };
    for i in 0..(n as u32 - 1) {
        let base = i * 2;
        indices.extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 1, base + 3]);
    }

    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32(indices));

    Aabb::from_min_max(min, max)
}

/// The ribbon's "nothing to draw" state: one degenerate triangle, not an
/// empty buffer.
///
/// Bevy 0.19's `MeshAllocator` skips allocation for a mesh whose vertex buffer
/// is zero-sized but still copies its element data unconditionally, so the
/// slab lookup misses and it logs `slab_allocator: Use-after-free` twice for
/// every `Added`/`Modified` event on such a mesh (bevyengine/bevy#24874,
/// guarded upstream by #24960 — merged after 0.19.0, so not in our release).
/// Every trail spawn and every park/short-history clear produced exactly that
/// mesh, which is the per-frame storm in #233. Keeping the vertex count
/// non-zero keeps the allocator's two loops in agreement; the three vertices
/// coincide at the local origin, so the triangle has no area and rasterizes
/// no fragments (its vertex colours are transparent besides).
fn clear_ribbon(mesh: &mut Mesh) {
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0.0f32; 3]; 3]);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0f32; 2]; 3]);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![[0.0f32; 4]; 3]);
    mesh.insert_indices(Indices::U32(vec![0, 1, 2]));
}

#[cfg(test)]
mod test {
    use super::*;

    /// `MeshAllocator::allocate_and_free_meshes` skips allocation when this is
    /// 0 but copies element data unconditionally, so a zero-vertex mesh logs
    /// `slab_allocator: Use-after-free` twice per `Added`/`Modified` event.
    /// Trails spawn empty and re-clear on every park, which is the #233 storm.
    #[test]
    fn a_spawned_trail_is_not_a_zero_vertex_mesh() {
        let mesh = empty_trail_mesh();

        assert_ne!(mesh.get_vertex_buffer_size(), 0);
        assert_ne!(mesh.count_vertices(), 0);
    }

    /// The clear path (pooled particle parked, or history below 2 samples)
    /// mutates the live asset, so it must not empty it either.
    #[test]
    fn clearing_a_built_ribbon_is_not_a_zero_vertex_mesh() {
        let mut mesh = empty_trail_mesh();
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, vec![[1.0f32, 2.0, 3.0]; 8]);

        clear_ribbon(&mut mesh);

        assert_ne!(mesh.get_vertex_buffer_size(), 0);
        assert_ne!(mesh.count_vertices(), 0);
    }

    /// ...while still drawing nothing: every vertex sits on the same point, so
    /// the triangle has no area.
    #[test]
    fn the_cleared_ribbon_is_degenerate() {
        let mesh = empty_trail_mesh();

        let Some(VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("the ribbon must keep float32x3 positions");
        };
        assert_eq!(positions.len(), 3);
        assert!(positions.windows(2).all(|pair| pair[0] == pair[1]));
    }
}
