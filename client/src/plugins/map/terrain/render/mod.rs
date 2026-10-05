//! Hand-rolled render pipeline for terrain ground, replacing `Material`/`MaterialPlugin` so the
//! tile atlas can be bound exactly once, globally, instead of once per region (see
//! `client::assets::m::block_splat_material`'s `REGION_TILE_SLOT_COUNT` doc comment for why
//! `Material` can't do this — its draw-command chain is fixed by a blanket impl inside `bevy_pbr`
//! with no supported extension point for a fourth, globally-bound bind group). Opt-in via
//! `graphics.terrain.pipeline: hand_rolled` (restart; registered by `MapPlugin`); the default
//! keeps using `TerrainBlockSplatMaterial`/`MaterialPlugin`.
//!
//! Idea: reuse everything mesh-shaped from bevy_pbr/bevy_render as-is — mesh extraction, the
//! `Opaque3d` binned phase and its render-pass node, `DrawMesh`, the view/mesh bind-group
//! commands, `SpecializedMeshPipelines` caching — and only hand-write what's genuinely new: one
//! `SpecializedMeshPipeline` that pushes two extra bind-group layouts (region: `tile_map` +
//! `lightmap`; global: the atlas + samplers + the two shared buffers), two small `RenderCommand`s
//! to bind them, and the specialize/queue systems that feed `Opaque3d`. Modeled on
//! `bevy_pbr::wireframe`, the closest fully-custom-pipeline precedent living entirely on public
//! APIs (confirmed by reading Bevy 0.19's own source, not guessed).
//!
//! Deliberately skips Bevy's own incremental dirty-tracking (`DirtySpecializations`/
//! `PendingQueues`, which the stock material path uses): terrain has only a handful of resident/
//! visible regions at once, so re-specializing and re-queuing every visible terrain entity every
//! frame is a cheap cache-hit lookup, not a real cost. Revisit only if profiling says otherwise.
//!
//! Dropping `Material` also drops its free shadow-phase participation, so shadow *casting* is a
//! separate, parallel depth-only pipeline in the `shadow` submodule (directional/Sun only).
//! Shadow *receiving* never depended on it: `terrain_splat.wgsl` samples the shared shadow map
//! through `apply_pbr_lighting` either way.
//!
//! Correctness note: `Opaque3d` is a *binned* phase, and Bevy's GPU-driven renderer can
//! multi-draw-indirect-merge bins that share the same `Opaque3dBatchSetKey` — the bound bind
//! groups then come from only one *representative* entity for the whole merged batch. Every
//! region's per-region bind group is genuinely unique (never shareable), so `queue_terrain_opaque`
//! sets `material_bind_group_index` to the region's own entity index — repurposed as a pure
//! batch-splitting discriminator, not a real material-bind-group index, since terrain has none —
//! to guarantee two regions never collide into the same batch set.

use std::collections::HashMap;
use std::num::NonZeroU32;

use bevy::asset::AssetServer;
use bevy::core_pipeline::core_3d::{Opaque3d, Opaque3dBatchSetKey, Opaque3dBinKey};
use bevy::ecs::system::lifetimeless::SRes;
use bevy::ecs::system::SystemParamItem;
use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::{
    DrawMesh, MeshPipeline, MeshPipelineKey, RenderMeshInstances, SetMeshBindGroup,
    SetMeshViewBindGroup, SetMeshViewBindingArrayBindGroup, Shadow, ViewKeyCache,
};
use bevy::prelude::*;
use bevy::render::batching::gpu_preprocessing::GpuPreprocessingSupport;
use bevy::render::mesh::allocator::MeshAllocator;
use bevy::render::mesh::RenderMesh;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_phase::*;
use bevy::render::render_resource::*;
use bevy::render::renderer::RenderDevice;
use bevy::render::sync_world::{MainEntity, MainEntityHashMap, MainEntityHashSet};
use bevy::render::texture::{FallbackImage, GpuImage};
use bevy::render::view::{ExtractedView, RenderVisibleEntities, RetainedViewEntity};
use bevy::render::{Extract, ExtractSchedule, Render, RenderApp, RenderStartup, RenderSystems};

use crate::assets::m::block_splat_material::{
    TerrainAmbientRatioBuffer, TerrainGroundTextures, TerrainRenderParamsBuffer, TerrainTileAtlas,
    TILE_SLOT_COUNT,
};

mod shadow;
use shadow::{
    init_terrain_shadow_pipeline, queue_terrain_shadows, specialize_terrain_shadows,
    DrawTerrainShadow, SpecializedTerrainShadowPipelineCache, TerrainShadowPipeline,
};

pub struct TerrainRenderPipelinePlugin;

impl Plugin for TerrainRenderPipelinePlugin {
    fn build(&self, app: &mut App) {
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .init_resource::<RenderTerrainGroundTextures>()
            .init_resource::<SpecializedMeshPipelines<TerrainPipeline>>()
            .init_resource::<SpecializedTerrainPipelineCache>()
            .init_resource::<QueuedTerrain<Opaque3d>>()
            .init_resource::<QueuedTerrain<Shadow>>()
            .init_resource::<TerrainRegionBindGroups>()
            .init_resource::<TerrainGlobalBindGroup>()
            .init_resource::<SpecializedMeshPipelines<TerrainShadowPipeline>>()
            .init_resource::<SpecializedTerrainShadowPipelineCache>()
            .add_render_command::<Opaque3d, DrawTerrainOpaque>()
            .add_render_command::<Shadow, DrawTerrainShadow>()
            .add_systems(
                RenderStartup,
                (init_terrain_samplers, init_terrain_pipeline)
                    .chain()
                    .after(bevy::pbr::MeshPipelineSystems),
            )
            .add_systems(
                RenderStartup,
                init_terrain_shadow_pipeline
                    .after(init_terrain_pipeline)
                    .after(bevy::pbr::init_prepass_pipeline),
            )
            .add_systems(ExtractSchedule, extract_terrain_ground_textures)
            .add_systems(
                Render,
                (
                    specialize_terrain
                        .in_set(RenderSystems::Specialize)
                        .after(bevy::render::render_asset::prepare_assets::<RenderMesh>),
                    specialize_terrain_shadows
                        .in_set(RenderSystems::Specialize)
                        .after(bevy::render::render_asset::prepare_assets::<RenderMesh>),
                    prepare_terrain_region_bind_groups
                        .in_set(RenderSystems::PrepareBindGroups)
                        .after(bevy::render::render_asset::prepare_assets::<GpuImage>),
                    prepare_terrain_global_bind_group
                        .in_set(RenderSystems::PrepareBindGroups)
                        .after(bevy::render::render_asset::prepare_assets::<GpuImage>),
                    queue_terrain_opaque.in_set(RenderSystems::QueueMeshes),
                    queue_terrain_shadows.in_set(RenderSystems::QueueMeshes),
                ),
            );
    }
}

// ---------------------------------------------------------------------------
// Extraction: TerrainGroundTextures, keyed by MainEntity (not by render-world
// entity identity, which is not guaranteed stable/queryable the way MainEntity
// is — same reasoning as bevy_pbr::wireframe's RenderWireframeInstances).
// ---------------------------------------------------------------------------

#[derive(Resource, Default)]
struct RenderTerrainGroundTextures(MainEntityHashMap<TerrainGroundTextures>);

fn extract_terrain_ground_textures(
    mut instances: ResMut<RenderTerrainGroundTextures>,
    changed_query: Extract<Query<(Entity, &TerrainGroundTextures), Changed<TerrainGroundTextures>>>,
    mut removed: Extract<RemovedComponents<TerrainGroundTextures>>,
) {
    for (entity, textures) in &changed_query {
        instances.0.insert(entity.into(), textures.clone());
    }
    for entity in removed.read() {
        if !changed_query.contains(entity) {
            instances.0.remove(&MainEntity::from(entity));
        }
    }
}

// ---------------------------------------------------------------------------
// Pipeline
// ---------------------------------------------------------------------------

/// Samplers used by the global bind group — created once, not recreated on every prepare (unlike
/// the `Material`-based path's `as_bind_group`, which the file's own doc comment on
/// `TerrainAmbientRatio` documents as leaking a fresh pair every re-prepare).
#[derive(Resource)]
struct TerrainSamplers {
    clamp: Sampler,
    tile: Sampler,
}

fn init_terrain_samplers(mut commands: Commands, render_device: Res<RenderDevice>) {
    let clamp = render_device.create_sampler(&SamplerDescriptor {
        min_filter: FilterMode::Linear,
        mag_filter: FilterMode::Linear,
        mipmap_filter: MipmapFilterMode::Linear,
        ..default()
    });
    let tile = render_device.create_sampler(&SamplerDescriptor {
        min_filter: FilterMode::Linear,
        mag_filter: FilterMode::Linear,
        mipmap_filter: MipmapFilterMode::Linear,
        address_mode_u: AddressMode::Repeat,
        address_mode_v: AddressMode::Repeat,
        address_mode_w: AddressMode::Repeat,
        anisotropy_clamp: 4,
        ..default()
    });
    commands.insert_resource(TerrainSamplers { clamp, tile });
}

/// Mirrors `Wireframe3dPipeline` (`bevy_pbr::wireframe`): a cloned `MeshPipeline` plus the
/// terrain-specific shader handle and the two new `BindGroupLayout`s/descriptors.
#[derive(Resource, Clone)]
pub struct TerrainPipeline {
    mesh_pipeline: MeshPipeline,
    shader: Handle<Shader>,
    region_bind_group_layout: BindGroupLayout,
    region_bind_group_layout_descriptor: BindGroupLayoutDescriptor,
    global_bind_group_layout: BindGroupLayout,
    global_bind_group_layout_descriptor: BindGroupLayoutDescriptor,
}

fn init_terrain_pipeline(
    mut commands: Commands,
    mesh_pipeline: Res<MeshPipeline>,
    asset_server: Res<AssetServer>,
    render_device: Res<RenderDevice>,
) {
    // Group 3 (per-region): the two textures genuinely unique per region.
    let region_entries = vec![
        BindGroupLayoutEntry {
            binding: 0,
            visibility: ShaderStages::FRAGMENT,
            ty: BindingType::Texture {
                multisampled: false,
                sample_type: TextureSampleType::Uint,
                view_dimension: TextureViewDimension::D2,
            },
            count: None,
        },
        BindGroupLayoutEntry {
            binding: 1,
            visibility: ShaderStages::FRAGMENT,
            ty: BindingType::Texture {
                multisampled: false,
                sample_type: TextureSampleType::Float { filterable: true },
                view_dimension: TextureViewDimension::D2,
            },
            count: None,
        },
    ];
    let region_bind_group_layout =
        render_device.create_bind_group_layout("terrain_region_bind_group_layout", &region_entries);
    let region_bind_group_layout_descriptor =
        BindGroupLayoutDescriptor::new("terrain_region_bind_group_layout", &region_entries);

    // Group 4 (global): bound exactly once, shared by every terrain region's draw.
    let global_entries = vec![
        BindGroupLayoutEntry {
            binding: 0,
            visibility: ShaderStages::FRAGMENT,
            ty: BindingType::Sampler(SamplerBindingType::Filtering),
            count: None,
        },
        BindGroupLayoutEntry {
            binding: 1,
            visibility: ShaderStages::FRAGMENT,
            ty: BindingType::Texture {
                multisampled: false,
                sample_type: TextureSampleType::Float { filterable: true },
                view_dimension: TextureViewDimension::D2,
            },
            count: NonZeroU32::new(TILE_SLOT_COUNT),
        },
        BindGroupLayoutEntry {
            binding: 2,
            visibility: ShaderStages::FRAGMENT,
            ty: BindingType::Sampler(SamplerBindingType::Filtering),
            count: None,
        },
        BindGroupLayoutEntry {
            binding: 3,
            visibility: ShaderStages::FRAGMENT,
            ty: BindingType::Buffer {
                ty: BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        BindGroupLayoutEntry {
            binding: 4,
            visibility: ShaderStages::FRAGMENT,
            ty: BindingType::Buffer {
                ty: BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
    ];
    let global_bind_group_layout =
        render_device.create_bind_group_layout("terrain_global_bind_group_layout", &global_entries);
    let global_bind_group_layout_descriptor =
        BindGroupLayoutDescriptor::new("terrain_global_bind_group_layout", &global_entries);

    commands.insert_resource(TerrainPipeline {
        mesh_pipeline: mesh_pipeline.clone(),
        shader: asset_server.load("shaders/terrain_splat.wgsl"),
        region_bind_group_layout,
        region_bind_group_layout_descriptor,
        global_bind_group_layout,
        global_bind_group_layout_descriptor,
    });
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct TerrainPipelineKey {
    pub mesh_key: MeshPipelineKey,
    pub backface_culling: bool,
}

impl SpecializedMeshPipeline for TerrainPipeline {
    type Key = TerrainPipelineKey;

    fn specialize(
        &self,
        key: Self::Key,
        layout: &MeshVertexBufferLayoutRef,
    ) -> Result<RenderPipelineDescriptor, SpecializedMeshPipelineError> {
        let mut descriptor = self.mesh_pipeline.specialize(key.mesh_key, layout)?;
        descriptor.label = Some("terrain_opaque_pipeline".into());
        descriptor.vertex.shader = self.shader.clone();
        descriptor
            .vertex
            .shader_defs
            .push("TERRAIN_HAND_ROLLED_PIPELINE".into());
        if let Some(fragment) = descriptor.fragment.as_mut() {
            fragment.shader = self.shader.clone();
            fragment
                .shader_defs
                .push("TERRAIN_HAND_ROLLED_PIPELINE".into());
        }
        descriptor.primitive.cull_mode = key.backface_culling.then_some(Face::Back);
        descriptor
            .layout
            .push(self.region_bind_group_layout_descriptor.clone());
        descriptor
            .layout
            .push(self.global_bind_group_layout_descriptor.clone());
        Ok(descriptor)
    }
}

// ---------------------------------------------------------------------------
// Bind groups
// ---------------------------------------------------------------------------

/// Built once (well: rebuilt only while the atlas's images are still uploading — see
/// `prepare_terrain_global_bind_group`), then reused for the rest of the session. This is the
/// whole point of the hand-rolled pipeline: one bind group instead of one per region.
#[derive(Resource, Default)]
pub(crate) struct TerrainGlobalBindGroup(Option<BindGroup>);

fn prepare_terrain_global_bind_group(
    mut global: ResMut<TerrainGlobalBindGroup>,
    pipeline: Option<Res<TerrainPipeline>>,
    samplers: Option<Res<TerrainSamplers>>,
    tile_atlas: Option<Res<TerrainTileAtlas>>,
    ambient_buffer: Option<Res<TerrainAmbientRatioBuffer>>,
    params_buffer: Option<Res<TerrainRenderParamsBuffer>>,
    image_assets: Res<RenderAssets<GpuImage>>,
    fallback_image: Res<FallbackImage>,
    render_device: Res<RenderDevice>,
) {
    let (
        Some(pipeline),
        Some(samplers),
        Some(tile_atlas),
        Some(ambient_buffer),
        Some(params_buffer),
    ) = (
        pipeline,
        samplers,
        tile_atlas,
        ambient_buffer,
        params_buffer,
    )
    else {
        return;
    };
    // Only worth doing the work again while something we depend on might have just finished
    // loading — once nothing changes for a frame, this is a no-op resource read and a bail.
    if global.0.is_some() && !image_assets.is_changed() && !tile_atlas.is_changed() {
        return;
    }

    let fallback_view = &*fallback_image.d2.texture_view;
    let mut texture_views = vec![fallback_view; TILE_SLOT_COUNT as usize];
    for (slot, handle) in tile_atlas.slots.iter().enumerate() {
        if let Some(image) = handle.as_ref().and_then(|h| image_assets.get(h)) {
            texture_views[slot] = &*image.texture_view;
        }
    }

    let bind_group = render_device.create_bind_group(
        "terrain_global_bind_group",
        &pipeline.global_bind_group_layout,
        &[
            BindGroupEntry {
                binding: 0,
                resource: BindingResource::Sampler(&samplers.clamp),
            },
            BindGroupEntry {
                binding: 1,
                resource: BindingResource::TextureViewArray(&texture_views[..]),
            },
            BindGroupEntry {
                binding: 2,
                resource: BindingResource::Sampler(&samplers.tile),
            },
            BindGroupEntry {
                binding: 3,
                resource: BindingResource::Buffer(ambient_buffer.0.as_entire_buffer_binding()),
            },
            BindGroupEntry {
                binding: 4,
                resource: BindingResource::Buffer(params_buffer.0.as_entire_buffer_binding()),
            },
        ],
    );
    global.0 = Some(bind_group);
}

pub(crate) struct SetTerrainGlobalBindGroup;
impl<P: PhaseItem> RenderCommand<P> for SetTerrainGlobalBindGroup {
    type Param = SRes<TerrainGlobalBindGroup>;
    type ViewQuery = ();
    type ItemQuery = ();

    fn render<'w>(
        _item: &P,
        _view: (),
        _entity: Option<()>,
        global: SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some(bind_group) = &global.into_inner().0 else {
            return RenderCommandResult::Skip;
        };
        pass.set_bind_group(4, bind_group, &[]);
        RenderCommandResult::Success
    }
}

/// Per-region bind groups (`tile_map` + `lightmap`), keyed by `MainEntity` — same reasoning as
/// `RenderTerrainGroundTextures` above.
#[derive(Resource, Default)]
pub(crate) struct TerrainRegionBindGroups(MainEntityHashMap<BindGroup>);

fn prepare_terrain_region_bind_groups(
    mut bind_groups: ResMut<TerrainRegionBindGroups>,
    pipeline: Option<Res<TerrainPipeline>>,
    instances: Res<RenderTerrainGroundTextures>,
    image_assets: Res<RenderAssets<GpuImage>>,
    render_device: Res<RenderDevice>,
) {
    let Some(pipeline) = pipeline else { return };
    // Prune entries for regions that no longer exist (despawned/unloaded) so this doesn't grow
    // unbounded as regions stream in/out over a long session.
    bind_groups
        .0
        .retain(|entity, _| instances.0.contains_key(entity));

    for (&entity, textures) in &instances.0 {
        if bind_groups.0.contains_key(&entity) {
            continue;
        }
        let (Some(tile_map), Some(lightmap)) = (
            image_assets.get(&textures.tile_map),
            image_assets.get(&textures.lightmap),
        ) else {
            continue; // still uploading; retried next frame since it's still missing above
        };
        let bind_group = render_device.create_bind_group(
            "terrain_region_bind_group",
            &pipeline.region_bind_group_layout,
            &[
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureView(&tile_map.texture_view),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::TextureView(&lightmap.texture_view),
                },
            ],
        );
        bind_groups.0.insert(entity, bind_group);
    }
}

pub(crate) struct SetTerrainRegionBindGroup;
impl<P: PhaseItem> RenderCommand<P> for SetTerrainRegionBindGroup {
    type Param = SRes<TerrainRegionBindGroups>;
    type ViewQuery = ();
    type ItemQuery = ();

    fn render<'w>(
        item: &P,
        _view: (),
        _entity: Option<()>,
        bind_groups: SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some(bind_group) = bind_groups.into_inner().0.get(&item.main_entity()) else {
            return RenderCommandResult::Skip;
        };
        pass.set_bind_group(3, bind_group, &[]);
        RenderCommandResult::Success
    }
}

pub type DrawTerrainOpaque = (
    SetItemPipeline,
    SetMeshViewBindGroup<0>,
    SetMeshViewBindingArrayBindGroup<1>,
    SetMeshBindGroup<2>,
    SetTerrainRegionBindGroup,
    SetTerrainGlobalBindGroup,
    DrawMesh,
);

// ---------------------------------------------------------------------------
// Specialize / queue — deliberately dirty-tracking-free, see the module doc.
// ---------------------------------------------------------------------------

#[derive(Resource, Default)]
struct SpecializedTerrainPipelineCache(
    HashMap<RetainedViewEntity, MainEntityHashMap<CachedRenderPipelineId>>,
);

fn specialize_terrain(
    mut cache: ResMut<SpecializedTerrainPipelineCache>,
    mut pipelines: ResMut<SpecializedMeshPipelines<TerrainPipeline>>,
    pipeline: Option<Res<TerrainPipeline>>,
    pipeline_cache: Res<PipelineCache>,
    view_key_cache: Res<ViewKeyCache>,
    render_meshes: Res<RenderAssets<RenderMesh>>,
    render_mesh_instances: Res<RenderMeshInstances>,
    ground_textures: Res<RenderTerrainGroundTextures>,
    views: Query<(&ExtractedView, &RenderVisibleEntities)>,
) {
    let Some(pipeline) = pipeline else { return };
    for (view, visible_entities) in &views {
        let Some(view_key) = view_key_cache.get(&view.retained_view_entity) else {
            continue;
        };
        let Some(mesh_entities) = visible_entities.get::<Mesh3d>() else {
            continue;
        };
        let view_cache = cache.0.entry(view.retained_view_entity).or_default();
        for (_, main_entity) in mesh_entities.iter_visible() {
            let Some(textures) = ground_textures.0.get(main_entity) else {
                continue;
            };
            let Some(mesh_instance) = render_mesh_instances.render_mesh_queue_data(*main_entity)
            else {
                continue;
            };
            let Some(mesh) = render_meshes.get(mesh_instance.mesh_asset_id()) else {
                continue;
            };
            let mesh_key = *view_key
                | MeshPipelineKey::from_primitive_topology_and_strip_index(
                    mesh.primitive_topology(),
                    mesh.index_format(),
                );
            let key = TerrainPipelineKey {
                mesh_key,
                backface_culling: textures.backface_culling,
            };
            match pipelines.specialize(&pipeline_cache, &pipeline, key, &mesh.layout) {
                Ok(id) => {
                    view_cache.insert(*main_entity, id);
                }
                Err(err) => {
                    error!("terrain pipeline specialize: {err}");
                }
            }
        }
    }
}

/// What each view's binned phase currently holds for terrain, per entity.
///
/// Bevy 0.19's binned phases are *retained* across frames, and `add` files an
/// entity under its new (batch set, bin) without taking it out of the old one.
/// Re-queuing every visible entity each frame therefore left an entity whose
/// key changed — a new pipeline after any view-key change (a depth prepass,
/// MSAA, an environment map), new mesh slabs — drawn *twice*, once with the
/// stale pipeline, whose view bind-group layout no longer matches: a wgpu
/// validation error that quits the app. Entities that left the view stayed
/// binned too. Bevy's own material queue avoids both with `remove`; so does
/// this, keyed on what was actually added.
#[derive(Resource)]
pub(crate) struct QueuedTerrain<P: BinnedPhaseItem>(
    HashMap<RetainedViewEntity, MainEntityHashMap<(P::BatchSetKey, P::BinKey)>>,
);

impl<P: BinnedPhaseItem> Default for QueuedTerrain<P> {
    fn default() -> Self {
        Self(HashMap::new())
    }
}

impl<P: BinnedPhaseItem> QueuedTerrain<P> {
    /// Adds `main_entity` to `phase`, first taking it out of the bin it was
    /// filed under if its keys changed.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn add(
        &mut self,
        view: RetainedViewEntity,
        phase: &mut BinnedRenderPhase<P>,
        batch_set_key: P::BatchSetKey,
        bin_key: P::BinKey,
        entities: (Entity, MainEntity),
        input_uniform_index: InputUniformIndex,
        phase_type: BinnedRenderPhaseType,
    ) {
        let keys = (batch_set_key.clone(), bin_key.clone());
        let queued = self.0.entry(view).or_default();
        if queued
            .insert(entities.1, keys.clone())
            .is_some_and(|old| old != keys)
        {
            phase.remove(entities.1);
        }
        phase.add(
            batch_set_key,
            bin_key,
            entities,
            input_uniform_index,
            phase_type,
        );
    }

    /// Removes every entity of `view` that was not added this frame.
    pub(crate) fn sweep(
        &mut self,
        view: RetainedViewEntity,
        phase: &mut BinnedRenderPhase<P>,
        added: &MainEntityHashSet,
    ) {
        if let Some(queued) = self.0.get_mut(&view) {
            queued.retain(|entity, _| {
                let keep = added.contains(entity);
                if !keep {
                    phase.remove(*entity);
                }
                keep
            });
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn queue_terrain_opaque(
    draw_functions: Res<DrawFunctions<Opaque3d>>,
    mut opaque_phases: ResMut<ViewBinnedRenderPhases<Opaque3d>>,
    mut queued: ResMut<QueuedTerrain<Opaque3d>>,
    mut added: Local<MainEntityHashSet>,
    cache: Res<SpecializedTerrainPipelineCache>,
    render_mesh_instances: Res<RenderMeshInstances>,
    mesh_allocator: Res<MeshAllocator>,
    gpu_preprocessing_support: Res<GpuPreprocessingSupport>,
    ground_textures: Res<RenderTerrainGroundTextures>,
    views: Query<(&ExtractedView, &RenderVisibleEntities)>,
) {
    let draw_function = draw_functions.read().id::<DrawTerrainOpaque>();
    for (view, visible_entities) in &views {
        let Some(phase) = opaque_phases.get_mut(&view.retained_view_entity) else {
            continue;
        };
        added.clear();
        let view_cache = cache.0.get(&view.retained_view_entity);
        let mesh_entities = visible_entities.get::<Mesh3d>();
        for (render_entity, main_entity) in mesh_entities
            .filter(|_| view_cache.is_some())
            .into_iter()
            .flat_map(|entities| entities.iter_visible())
        {
            if !ground_textures.0.contains_key(main_entity) {
                continue;
            }
            let Some(pipeline_id) = view_cache.and_then(|c| c.get(main_entity)).copied() else {
                continue;
            };
            let Some(mesh_instance) = render_mesh_instances.render_mesh_queue_data(*main_entity)
            else {
                continue;
            };
            let Some(slabs) = mesh_allocator.mesh_slabs(&mesh_instance.mesh_asset_id()) else {
                continue;
            };
            added.insert(*main_entity);
            queued.add(
                view.retained_view_entity,
                phase,
                Opaque3dBatchSetKey {
                    pipeline: pipeline_id,
                    draw_function,
                    // Pure batch-splitting discriminator, NOT a real material bind-group index —
                    // terrain has none. See the module doc's correctness note: this guarantees
                    // two regions (whose bind groups are never shareable) never collide into the
                    // same GPU multi-draw batch set.
                    material_bind_group_index: Some(main_entity.id().index_u32()),
                    slabs,
                    lightmap_slab: None,
                },
                Opaque3dBinKey {
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
        queued.sweep(view.retained_view_entity, phase, &added);
    }
}
