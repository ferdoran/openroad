use bevy::asset::Asset;
use bevy::asset::RenderAssetUsages;
use bevy::math::Vec3;
use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues};
use bevy::prelude::Mesh;
use bevy::reflect::TypePath;

use crate::assets::bms::navmesh::BmsNavMesh;
use crate::assets::bms::skeleton::MeshBones;
use crate::assets::bms::vertex::VertexData;

#[derive(TypePath, Asset)]
pub struct JMXVBMS {
    pub name: String,
    pub vertex_data: VertexData,
    pub indices: Vec<(u16, u16, u16)>,
    pub bounding_box: (Vec3, Vec3),
    pub navmesh: Option<BmsNavMesh>,
    pub material: String,
    pub bone_data: Option<MeshBones>,
}

impl JMXVBMS {
    /// True when this mesh carries usable per-vertex skinning data (bone weights
    /// for every vertex). The single authoritative predicate: spawn sites must
    /// insert `SkinnedMesh` iff they also built the mesh with `with_skinning`.
    pub fn has_skinning_data(&self) -> bool {
        self.bone_data
            .as_ref()
            .is_some_and(|bd| bd.bone_data.len() == self.vertex_data.vertices.len())
    }

    /// Builds a Bevy mesh from this `.bms`.
    ///
    /// `reverse_winding` flips the triangle winding order. Every SRO resource —
    /// characters, world/map objects, equipment — is placed with a mirroring
    /// (negative-determinant) transform for the SRO -> Bevy coordinate conversion
    /// (`scale.x = -1`), which reverses the effective winding at render time and
    /// makes backface culling discard the front faces (walls disappear, character
    /// surfaces turn inside-out). Reversing the winding here compensates. The flag
    /// is derived from the placement transform's determinant via the single shared
    /// rule (`util::mesh::needs_winding_reversal`); only positive-determinant
    /// placements pass `false` and keep the source order.
    ///
    /// `with_skinning` controls whether the JOINT_INDEX/JOINT_WEIGHT attributes go
    /// in, and MUST match whether the spawning entity gets a `SkinnedMesh`: Bevy
    /// specializes the render pipeline from the mesh *attributes* but picks the
    /// mesh bind group from the *entity's* skin data, so a joint-attributed mesh
    /// on an unskinned entity draws a skinned pipeline with the model-only bind
    /// group — a wgpu validation error that quits the app. Skeleton-less spawns of
    /// skinned `.bms` files (ground drops of armor, skill-object models) are the
    /// normal case, not an anomaly.
    pub fn to_mesh(&self, reverse_winding: bool, with_skinning: bool) -> Mesh {
        let bms = self;
        // MAIN_WORLD retention is required on top of RENDER_WORLD: mesh
        // raycasting (character picking in the selection scene, later
        // click-to-target in the world) reads the vertex data CPU-side, and a
        // RENDER_WORLD-only mesh is unloaded from the main world after the
        // GPU upload — raycasts against it silently never hit.
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        );

        let mut positions: Vec<[f32; 3]> = Vec::with_capacity(bms.vertex_data.vertices.len());
        let mut normals: Vec<[f32; 3]> = Vec::with_capacity(bms.vertex_data.vertices.len());
        let mut uv0: Vec<[f32; 2]> = Vec::with_capacity(bms.vertex_data.vertices.len());
        let mut indices = Vec::with_capacity(bms.indices.len() * 3);
        for (a, b, c) in &bms.indices {
            if reverse_winding {
                indices.extend_from_slice(&[*a, *c, *b]);
            } else {
                indices.extend_from_slice(&[*a, *b, *c]);
            }
        }

        for vertex in &bms.vertex_data.vertices {
            positions.push(vertex.position.to_array());
            normals.push(vertex.normal.to_array());
            uv0.push(vertex.uv_0.to_array());
        }

        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv0);

        if let Some(bone_data) = bms.bone_data.as_ref().filter(|_| with_skinning) {
            let mut joint_indices: Vec<[u16; 4]> = Vec::with_capacity(bone_data.bone_data.len());
            let mut joint_weights: Vec<[f32; 4]> = Vec::with_capacity(bone_data.bone_data.len());

            for b in &bone_data.bone_data {
                let (b1, mut w1) = if b.index1 == 0xFF {
                    (0, 0.0)
                } else {
                    (b.index1 as u16, (b.weight1 as f32 / u16::MAX as f32))
                };

                let (b2, mut w2) = if b.index2 == 0xFF {
                    (0, 0.0)
                } else {
                    (b.index2 as u16, (b.weight2 as f32 / u16::MAX as f32))
                };

                // there are some outliers which are not normalized
                let w_sum = w1 + w2;
                if w_sum != 1.0 {
                    let diff = 1.0 - (w_sum);
                    let f1 = w1 / w_sum;
                    let f2 = w2 / w_sum;
                    w1 += f1 * diff;
                    w2 += f2 * diff;
                }

                joint_indices.push([b1, b2, 0, 0]);
                joint_weights.push([w1, w2, 0.0, 0.0]);
            }
            if bone_data.bone_data.len() == bms.vertex_data.vertices.len() {
                mesh.insert_attribute(
                    Mesh::ATTRIBUTE_JOINT_WEIGHT,
                    VertexAttributeValues::Float32x4(joint_weights),
                );
                mesh.insert_attribute(
                    Mesh::ATTRIBUTE_JOINT_INDEX,
                    VertexAttributeValues::Uint16x4(joint_indices),
                );
            }
        }

        mesh.insert_indices(Indices::U16(indices));

        mesh
    }
}

#[cfg(test)]
mod tests {
    use bevy::math::Vec2;

    use super::*;
    use crate::assets::bms::skeleton::BoneData;
    use crate::assets::bms::vertex::Vertex;

    fn fixture(with_bones: bool) -> JMXVBMS {
        let vertices = (0..3)
            .map(|i| Vertex {
                position: Vec3::new(i as f32, 0.0, 0.0),
                normal: Vec3::Y,
                uv_0: Vec2::ZERO,
                uv_1: None,
                morphing_data: None,
                float: 0.0,
                int0: 0,
                int1: 0,
            })
            .collect::<Vec<_>>();
        let bone_data = with_bones.then(|| MeshBones {
            bones: vec!["Bone01".into()],
            bone_data: vec![
                BoneData {
                    index1: 0,
                    weight1: u16::MAX,
                    index2: 0xFF,
                    weight2: 0,
                };
                vertices.len()
            ],
        });
        JMXVBMS {
            name: "test".into(),
            vertex_data: VertexData {
                vertices,
                lightmap_path: None,
                new_vertex_data: None,
            },
            indices: vec![(0, 1, 2)],
            bounding_box: (Vec3::ZERO, Vec3::ONE),
            navmesh: None,
            material: "mat".into(),
            bone_data,
        }
    }

    #[test]
    fn joint_attributes_follow_with_skinning_flag() {
        let bms = fixture(true);
        let skinned = bms.to_mesh(false, true);
        assert!(skinned.contains_attribute(Mesh::ATTRIBUTE_JOINT_INDEX));
        assert!(skinned.contains_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT));
        // An unskinned spawn of the same skinned .bms must not carry joint
        // attributes: the pipeline (specialized from attributes) and the bind
        // group (per-entity skin data) would disagree — wgpu validation error.
        let unskinned = bms.to_mesh(false, false);
        assert!(!unskinned.contains_attribute(Mesh::ATTRIBUTE_JOINT_INDEX));
        assert!(!unskinned.contains_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT));
    }

    #[test]
    fn has_skinning_data_requires_per_vertex_bone_data() {
        assert!(fixture(true).has_skinning_data());
        assert!(!fixture(false).has_skinning_data());
        let mut truncated = fixture(true);
        truncated.bone_data.as_mut().unwrap().bone_data.pop();
        assert!(!truncated.has_skinning_data());
    }
}
