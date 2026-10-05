//! Shadow-casting fast-follow for the hand-rolled terrain pipeline (see the module
//! doc on `super` and the plan doc this shipped with). Directional-light-only:
//! this game spawns exactly one directional light (the Sun,
//! `client::plugins::map::mod::setup_lighting`) and has no point/spot shadow
//! casters anywhere.
//!
//! Reuses Bevy's own `Shadow` binned phase and its `SetPrepassViewBindGroup`/
//! `SetPrepassViewEmptyBindGroup` render commands, backed by a `PrepassViewBindGroup`
//! resource `PrepassPipelinePlugin` already populates every frame, unconditionally —
//! it's core PBR setup (`PbrPlugin` -> `MaterialsPlugin`), not gated on any
//! `Material` existing, so it's already running today regardless of terrain's
//! hand-rolled path. Groups 0/1 of this pipeline's layout are cloned straight from
//! `Res<PrepassPipeline>`'s own public fields rather than reinvented, since
//! `SetPrepassViewBindGroup<0>`/`SetPrepassViewEmptyBindGroup<1>` bind whatever
//! bind group that shared resource was built from — this pipeline's layout has to
//! match it exactly. `PrepassPipeline::specialize` itself (the method that builds
//! this shape for every `Material`) is private and can't be called; this module
//! hand-writes an equivalent depth-only descriptor instead, mirrored from reading
//! that method's source directly (`bevy_pbr::prepass::PrepassPipeline::specialize`).
//!
//! No group 3 at all: a depth-only, non-discarding caster needs no material bind
//! group, and terrain's shadow draw carries no per-region data of any kind — unlike
//! the opaque pass's `Opaque3dBatchSetKey`, there is no batch-set-key collision
//! hazard to defend against here. `material_bind_group_index: None` for every
//! terrain shadow entry is exactly what Bevy's own stock depth-only shadow draws
//! already do (`light.rs::queue_shadows`), and it's safe for the same reason: no
//! two regions can ever have their (nonexistent) per-region shadow bind group
//! confused, since there isn't one.
//!
//! Deliberately dirty-tracking-free, same reasoning as `specialize_terrain`/
//! `queue_terrain_opaque` in `super`.
//!
//! Known limitation, deferred: on GPUs without the `DEPTH_CLIP_CONTROL` wgpu
//! feature, directional cascades need a depth-clamp-in-fragment-shader fallback
//! (`UNCLIPPED_DEPTH_ORTHO_EMULATION`, mirrored from Bevy's own embedded
//! `prepass.wgsl`) that isn't implemented yet — `depth_clip_control_supported` is
//! logged at startup so this is diagnosable; expected `true` on most desktop GPUs,
//! including this dev machine's.

use std::collections::HashMap;

use bevy::core_pipeline::core_3d::CORE_3D_DEPTH_FORMAT;
use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::{
    DrawMesh, LightEntity, MeshPipelineKey, PrepassPipeline, RenderMeshInstanceFlags,
    RenderMeshInstances, SetMeshBindGroup, SetPrepassViewBindGroup, SetPrepassViewEmptyBindGroup,
    Shadow, ShadowBatchSetKey, ShadowBinKey,
};
use bevy::prelude::*;
use bevy::render::batching::gpu_preprocessing::GpuPreprocessingSupport;
use bevy::render::mesh::allocator::MeshAllocator;
use bevy::render::mesh::RenderMesh;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_phase::*;
use bevy::render::render_resource::*;
use bevy::render::sync_world::{MainEntityHashMap, MainEntityHashSet};
use bevy::render::view::{ExtractedView, RenderShadowMapVisibleEntities, RetainedViewEntity};

use super::{RenderTerrainGroundTextures, TerrainPipeline};

/// Mirrors `PrepassPipeline`'s own bind-group-layout shape for a plain
/// (non-skinned/morphed/lightmapped, non-motion-vector) depth-only caster —
/// cloned from `Res<PrepassPipeline>` rather than reinvented, per the module doc.
#[derive(Resource, Clone)]
pub struct TerrainShadowPipeline {
    shader: Handle<Shader>,
    view_layout: BindGroupLayoutDescriptor,
    empty_layout: BindGroupLayoutDescriptor,
    mesh_bind_group_layout: BindGroupLayoutDescriptor,
    depth_clip_control_supported: bool,
}

pub(super) fn init_terrain_shadow_pipeline(
    mut commands: Commands,
    terrain_pipeline: Res<TerrainPipeline>,
    prepass_pipeline: Res<PrepassPipeline>,
) {
    info!(
        "terrain shadow pipeline: depth_clip_control_supported = {} (false means the \
         UNCLIPPED_DEPTH_ORTHO_EMULATION fallback would be needed for correct cascade \
         edges — not implemented yet, see this module's doc comment)",
        prepass_pipeline.depth_clip_control_supported
    );
    commands.insert_resource(TerrainShadowPipeline {
        shader: terrain_pipeline.shader.clone(),
        view_layout: prepass_pipeline.view_layout_no_motion_vectors.clone(),
        empty_layout: prepass_pipeline.empty_layout.clone(),
        mesh_bind_group_layout: prepass_pipeline.mesh_layouts.model_only.clone(),
        depth_clip_control_supported: prepass_pipeline.depth_clip_control_supported,
    });
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct TerrainShadowPipelineKey {
    pub mesh_key: MeshPipelineKey,
}

impl SpecializedMeshPipeline for TerrainShadowPipeline {
    type Key = TerrainShadowPipelineKey;

    fn specialize(
        &self,
        key: Self::Key,
        layout: &MeshVertexBufferLayoutRef,
    ) -> Result<RenderPipelineDescriptor, SpecializedMeshPipelineError> {
        let mesh_key = key.mesh_key;
        let mut shader_defs = vec![
            "TERRAIN_HAND_ROLLED_PIPELINE".into(),
            "VERTEX_OUTPUT_INSTANCE_INDEX".into(),
        ];
        // Position only — mirrors PrepassPipeline::specialize's own depth-only
        // (non-normal-prepass) attribute set exactly; the shadow pass never
        // samples normals/UVs.
        let mut vertex_attributes = Vec::new();
        if layout.0.contains(Mesh::ATTRIBUTE_POSITION) {
            shader_defs.push("VERTEX_POSITIONS".into());
            vertex_attributes.push(Mesh::ATTRIBUTE_POSITION.at_shader_location(0));
        }
        let view_projection = mesh_key.intersection(MeshPipelineKey::VIEW_PROJECTION_RESERVED_BITS);
        if view_projection == MeshPipelineKey::VIEW_PROJECTION_ORTHOGRAPHIC {
            shader_defs.push("VIEW_PROJECTION_ORTHOGRAPHIC".into());
        } else if view_projection == MeshPipelineKey::VIEW_PROJECTION_PERSPECTIVE {
            shader_defs.push("VIEW_PROJECTION_PERSPECTIVE".into());
        }
        // See the module doc: the emulation fallback for GPUs without
        // DEPTH_CLIP_CONTROL isn't implemented yet, so this is always false on
        // hardware lacking the feature (an accepted, logged, deferred gap) rather
        // than silently wrong — `unclipped_depth` itself is still correctly gated
        // on `depth_clip_control_supported` so supported hardware works today.
        let unclipped_depth = mesh_key.contains(MeshPipelineKey::UNCLIPPED_DEPTH_ORTHO)
            && self.depth_clip_control_supported;

        let vertex_buffer_layout = layout.0.get_layout(&vertex_attributes)?;

        Ok(RenderPipelineDescriptor {
            label: Some("terrain_shadow_pipeline".into()),
            layout: vec![
                self.view_layout.clone(),
                self.empty_layout.clone(),
                self.mesh_bind_group_layout.clone(),
            ],
            vertex: VertexState {
                shader: self.shader.clone(),
                shader_defs,
                buffers: vec![vertex_buffer_layout],
                ..default()
            },
            // Depth-only, non-discarding caster: no fragment shader at all, same
            // as every stock opaque `Material`'s own shadow pipeline.
            fragment: None,
            primitive: PrimitiveState {
                topology: mesh_key.primitive_topology(),
                strip_index_format: mesh_key.strip_index_format(),
                unclipped_depth,
                // PrepassPipeline::specialize never sets cull_mode for any
                // material's shadow pass — matched here rather than reusing
                // TerrainGroundTextures.backface_culling, which stays opaque-only.
                cull_mode: None,
                ..default()
            },
            depth_stencil: Some(DepthStencilState {
                format: CORE_3D_DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(CompareFunction::GreaterEqual),
                stencil: StencilState {
                    front: StencilFaceState::IGNORE,
                    back: StencilFaceState::IGNORE,
                    read_mask: 0,
                    write_mask: 0,
                },
                bias: DepthBiasState {
                    constant: 0,
                    slope_scale: 0.0,
                    clamp: 0.0,
                },
            }),
            multisample: MultisampleState {
                count: mesh_key.msaa_samples(),
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            ..default()
        })
    }
}

// ---------------------------------------------------------------------------
// Specialize / queue — deliberately dirty-tracking-free, see the module doc.
// Directional-only: resolves each shadow cascade's own visible-entity list via
// `RenderShadowMapVisibleEntities` (attached to the light entity, not the
// per-cascade view entity) rather than the private `get_shadow_map_visible_entities`
// helper, which does the same lookup but isn't callable from outside bevy_pbr.
// ---------------------------------------------------------------------------

#[derive(Resource, Default)]
pub(super) struct SpecializedTerrainShadowPipelineCache(
    HashMap<RetainedViewEntity, MainEntityHashMap<CachedRenderPipelineId>>,
);

pub(super) fn specialize_terrain_shadows(
    mut cache: ResMut<SpecializedTerrainShadowPipelineCache>,
    mut pipelines: ResMut<SpecializedMeshPipelines<TerrainShadowPipeline>>,
    pipeline: Option<Res<TerrainShadowPipeline>>,
    pipeline_cache: Res<PipelineCache>,
    render_meshes: Res<RenderAssets<RenderMesh>>,
    render_mesh_instances: Res<RenderMeshInstances>,
    ground_textures: Res<RenderTerrainGroundTextures>,
    view_lights: Query<(&LightEntity, &ExtractedView)>,
    shadow_map_visible_entities: Query<&RenderShadowMapVisibleEntities>,
) {
    let Some(pipeline) = pipeline else { return };
    let base_key = MeshPipelineKey::DEPTH_PREPASS
        | MeshPipelineKey::VIEW_PROJECTION_ORTHOGRAPHIC
        | MeshPipelineKey::UNCLIPPED_DEPTH_ORTHO;
    for (light_entity, extracted_view) in &view_lights {
        let LightEntity::Directional { light_entity, .. } = light_entity else {
            continue;
        };
        let Ok(shadow_map_visible_entities) = shadow_map_visible_entities.get(*light_entity) else {
            continue;
        };
        let Some(visible_entities) = shadow_map_visible_entities
            .subviews
            .get(&extracted_view.retained_view_entity)
        else {
            continue;
        };
        let Some(mesh_entities) = visible_entities.get::<Mesh3d>() else {
            continue;
        };
        let view_cache = cache
            .0
            .entry(extracted_view.retained_view_entity)
            .or_default();
        for (_, main_entity) in mesh_entities.iter_visible() {
            if !ground_textures.0.contains_key(main_entity) {
                continue;
            }
            let Some(mesh_instance) = render_mesh_instances.render_mesh_queue_data(*main_entity)
            else {
                continue;
            };
            let Some(mesh) = render_meshes.get(mesh_instance.mesh_asset_id()) else {
                continue;
            };
            let key = TerrainShadowPipelineKey {
                mesh_key: base_key
                    | MeshPipelineKey::from_primitive_topology_and_strip_index(
                        mesh.primitive_topology(),
                        mesh.index_format(),
                    ),
            };
            match pipelines.specialize(&pipeline_cache, &pipeline, key, &mesh.layout) {
                Ok(id) => {
                    view_cache.insert(*main_entity, id);
                }
                Err(err) => {
                    error!("terrain shadow pipeline specialize: {err}");
                }
            }
        }
    }
}

pub(super) fn queue_terrain_shadows(
    draw_functions: Res<DrawFunctions<Shadow>>,
    mut shadow_phases: ResMut<ViewBinnedRenderPhases<Shadow>>,
    mut queued: ResMut<super::QueuedTerrain<Shadow>>,
    mut added: Local<MainEntityHashSet>,
    cache: Res<SpecializedTerrainShadowPipelineCache>,
    render_mesh_instances: Res<RenderMeshInstances>,
    mesh_allocator: Res<MeshAllocator>,
    gpu_preprocessing_support: Res<GpuPreprocessingSupport>,
    ground_textures: Res<RenderTerrainGroundTextures>,
    view_lights: Query<(&LightEntity, &ExtractedView)>,
    shadow_map_visible_entities: Query<&RenderShadowMapVisibleEntities>,
) {
    let draw_function = draw_functions.read().id::<DrawTerrainShadow>();
    for (light_entity, extracted_view) in &view_lights {
        let LightEntity::Directional { light_entity, .. } = light_entity else {
            continue;
        };
        let Some(phase) = shadow_phases.get_mut(&extracted_view.retained_view_entity) else {
            continue;
        };
        added.clear();
        'collect: {
            let Some(view_cache) = cache.0.get(&extracted_view.retained_view_entity) else {
                break 'collect;
            };
            let Ok(shadow_map_visible_entities) = shadow_map_visible_entities.get(*light_entity)
            else {
                break 'collect;
            };
            let Some(visible_entities) = shadow_map_visible_entities
                .subviews
                .get(&extracted_view.retained_view_entity)
            else {
                break 'collect;
            };
            let Some(mesh_entities) = visible_entities.get::<Mesh3d>() else {
                break 'collect;
            };
            for (render_entity, main_entity) in mesh_entities.iter_visible() {
                if !ground_textures.0.contains_key(main_entity) {
                    continue;
                }
                let Some(pipeline_id) = view_cache.get(main_entity).copied() else {
                    continue;
                };
                let Some(mesh_instance) =
                    render_mesh_instances.render_mesh_queue_data(*main_entity)
                else {
                    continue;
                };
                if !mesh_instance
                    .flags()
                    .contains(RenderMeshInstanceFlags::SHADOW_CASTER)
                {
                    continue;
                }
                let Some(slabs) = mesh_allocator.mesh_slabs(&mesh_instance.mesh_asset_id()) else {
                    continue;
                };
                added.insert(*main_entity);
                queued.add(
                    extracted_view.retained_view_entity,
                    phase,
                    ShadowBatchSetKey {
                        pipeline: pipeline_id,
                        draw_function,
                        // No per-region data in this draw at all (unlike the opaque
                        // pass) — safe to leave `None`, exactly like Bevy's own stock
                        // depth-only shadow draws. See the module doc.
                        material_bind_group_index: None,
                        slabs,
                    },
                    ShadowBinKey {
                        asset_id: mesh_instance.mesh_asset_id().into(),
                    },
                    (*render_entity, *main_entity),
                    mesh_instance.current_uniform_index,
                    BinnedRenderPhaseType::mesh(
                        mesh_instance.should_batch(),
                        &gpu_preprocessing_support,
                    ),
                );
            }
        }
        queued.sweep(extracted_view.retained_view_entity, phase, &added);
    }
}

pub(super) type DrawTerrainShadow = (
    SetItemPipeline,
    SetPrepassViewBindGroup<0>,
    SetPrepassViewEmptyBindGroup<1>,
    SetMeshBindGroup<2>,
    DrawMesh,
);
