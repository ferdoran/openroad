use std::num::NonZeroU32;

use bevy::asset::RenderAssetUsages;
use bevy::asset::{Asset, Assets};
use bevy::ecs::system::SystemParam;
use bevy::log::warn_once;
use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey};
use bevy::prelude::default;
use bevy::prelude::{
    error, info, not, resource_exists, warn, AlphaMode, App, AssetServer, Commands, DetectChanges,
    Handle, Image, IntoScheduleConfigs, Material, Plugin, PreUpdate, Res, ResMut, Resource,
    Startup, Update,
};
use bevy::reflect::TypePath;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::{
    AddressMode, AsBindGroup, AsBindGroupError, BindGroupEntry, BindGroupLayout,
    BindGroupLayoutDescriptor, BindGroupLayoutEntry, BindingResource, BindingType, Buffer,
    BufferBindingType, BufferInitDescriptor, BufferUsages, Extent3d, Face, FilterMode,
    MipmapFilterMode, PipelineCache, PreparedBindGroup, RenderPipelineDescriptor,
    SamplerBindingType, SamplerDescriptor, ShaderStages, SpecializedMeshPipelineError,
    TextureDimension, TextureFormat, TextureSampleType, TextureViewDimension, UnpreparedBindGroup,
};
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::render::settings::WgpuFeatures;
use bevy::render::texture::{FallbackImage, GpuImage};
use bevy::render::{Extract, ExtractSchedule, Render, RenderApp, RenderStartup, RenderSystems};
use bevy::shader::ShaderRef;

use crate::assets::ifo::IFOAsset;
use crate::assets::m::TerrainBlock;
use crate::plugins::map::assets::TileAssets;

/// Size of the ground-tile binding array. `JMXVMAPM` packs a vertex's tile id into 10 bits
/// (`assets/m/mod.rs`: `flags & 0b0000_0011_1111_1111`), so 1024 covers the entire id space by
/// construction and a tile id can index the array *directly* — no per-group remapping, and
/// therefore nothing to overflow. The shipped `tile2d.ifo` defines 719 ids (dense 0..718), of
/// which 685 are referenced by some region (Map.pk2 census 2026-08-08).
pub const TILE_SLOT_COUNT: u32 = 1024;

/// Vertices per block edge, and blocks per region edge — the `tile_map` packing below.
const BLOCK_VERTS: usize = 17;
const REGION_BLOCKS_PER_SIDE: usize = 6;
/// `tile_map` is one 102x102 texture per region: 6 blocks of 17 vertices per axis, with each
/// block keeping its own duplicated edge vertices so a fragment never gathers across a block
/// boundary (see the header comment in `terrain_splat.wgsl`).
const TILE_MAP_SIZE: usize = REGION_BLOCKS_PER_SIDE * BLOCK_VERTS;

/// Every ground tile texture in the game, indexed by the map's 10-bit tile id.
///
/// This is what lets terrain share one texture binding across every draw: the old design gave
/// each merge group its own array of just the tiles that group used, so the tile id had to be
/// remapped to a group-local index, which in turn forced the one-hot weight encoding and a hard
/// cap on tiles per group. Indexing globally removes all three.
///
/// The handles are the same assets `TileAssets` already loads (the whole `map://tile2d` folder),
/// so populating this costs no extra memory or load time — it only gives them an id-ordered home.
#[derive(Resource, Clone, Default)]
pub struct TerrainTileAtlas {
    /// `TILE_SLOT_COUNT` entries; `None` where the id is undefined or its texture failed to load
    /// (those slots bind the fallback image, so an unknown id renders as the fallback rather
    /// than breaking the bind group).
    pub slots: Vec<Option<Handle<Image>>>,
}

impl TerrainTileAtlas {
    pub fn new() -> Self {
        Self {
            slots: vec![None; TILE_SLOT_COUNT as usize],
        }
    }
}

#[derive(TypePath, Asset, Default, Debug, Clone)]
pub struct TerrainBlockSplatMaterial {
    /// Per-vertex tile choice for one region, as a 102x102 `Rg16Uint` image: `r` = tile id
    /// (indexes [`TerrainTileAtlas`] directly), `g` = splat scale code. Texel
    /// `(block_col * 17 + i, block_row * 17 + j)` is that block's vertex `(i, j)`.
    ///
    /// The shader gathers the 4 texels around a fragment and blends them by their bilinear
    /// weights, which is algebraically identical to the one-hot weight texture this replaces
    /// but costs 4 texel reads instead of one sample per tile in the group.
    pub tile_map: Handle<Image>,

    pub backface_culling: bool,

    /// Baked terrain lightmap for this group's region, decoded from the region's `.t`
    /// (JMXVMAPT, see `assets/t.rs`). Sampled at region-local UV and multiplied into the ground
    /// albedo in `terrain_splat.wgsl`, adding SRO's static baked sun/shadow on top of the dynamic
    /// lighting. Regions without a `.t` get a shared 1×1 white handle, making the multiply a no-op.
    pub lightmap: Handle<Image>,
}

/// Per-channel ratio of SRO's terrain ambient to the global (object) ambient light,
/// `1.0` = no difference. The shader re-weights the ambient term Bevy already applied
/// by this factor, giving terrain its own ambient color (see `plugins/environment`)
/// without needing brightness/exposure bookkeeping. `w` unused.
///
/// Deliberately NOT a material field: every `TerrainBlockSplatMaterial` binds one
/// shared GPU buffer (binding 4) that [`TerrainAmbientRatioPlugin`] updates in place
/// via `queue.write_buffer`. Mutating the materials instead would flag them all
/// `Modified` on every day/night tick — and bevy_pbr 0.19's `CreateBindGroupDirectly`
/// re-prepare path (which this material takes for its texture binding array) never
/// frees the previous prepared bind group, permanently leaking its buffers, samplers,
/// and pinned texture views each time (bevy_pbr material.rs `prepare_asset`, the
/// direct-path arm misses the `bind_group_allocator.free` the unprepared arm has).
#[derive(Resource, Clone, ExtractResource)]
pub struct TerrainAmbientRatio(pub [f32; 4]);

impl Default for TerrainAmbientRatio {
    fn default() -> Self {
        Self([1.0; 4])
    }
}

/// The shared render-world buffer behind [`TerrainAmbientRatio`], bound by every
/// splat material's bind group.
#[derive(Resource)]
pub struct TerrainAmbientRatioBuffer(pub Buffer);

/// Global terrain render parameters, shared by every splat material through
/// one GPU buffer (binding 6) — the same zero-leak `write_buffer` route as
/// [`TerrainAmbientRatio`], so all values are runtime-togglable without
/// dirtying a single material:
///
/// - `lightmap_flip_v`: mirrors the baked-lightmap V axis. The `.t` DDS row
///   order vs. world Z was never GPU-calibrated (`docs/formats/mapt-jmxvmapt.md`);
///   this makes the pending flip a data change instead of a shader edit.
/// - `lighting_mode`: the terrain dynamic-lighting A/B
///   (`docs/rendering-mobile-shader-comparison.md` gap #6). `Dynamic` is the
///   current full PBR sun + ambient over the baked lightmap; `FlatBaked`
///   drops N·L/specular but keeps the time-of-day ambient tint;
///   `Baked` is `albedo × lightmap`, full stop — the mobile port's (and
///   probably the original's) fully-baked ground.
///
/// Kept in step with `graphics.terrain` by `apply_terrain_render_params`
/// (see [`crate::plugins::settings::live`]); the render-debug panel and the
/// dungeon atmosphere override cycle it live on top of that.
#[derive(Resource, Clone, PartialEq, ExtractResource)]
pub struct TerrainRenderParams {
    pub lightmap_flip_v: bool,
    pub lighting_mode: TerrainLightingMode,
    /// Baked terrain lightmap on/off — owned by the environment plugin's
    /// vanilla/PBR switch (`EnvironmentSettings.enabled`, hotkey N), NOT by
    /// the render-debug panel; `on_settings_changed` carries the current
    /// value through its rebuild instead of resetting it.
    pub lightmap_enabled: bool,
    /// Extra sun-shadow darkening on the lit ground (0 = none): the vanilla
    /// player-shadow boost, env-owned like `lightmap_enabled` — set from
    /// `EnvironmentSettings::vanilla_shadow_strength` in vanilla mode, 0 in
    /// PBR mode (where apply_pbr_lighting's physically-based shadowing is
    /// the whole story).
    pub shadow_strength: f32,
    /// Tiling repeat factors for the five splat-scale codes `8·i`
    /// (`docs/formats/mapm-jmxvmapm.md`), index `i` = code/8, all
    /// live-tunable from the render-debug panel. Playtest verdict
    /// 2026-08-10: **the vertex "Scale" field does not drive tiling at
    /// all** — every code matches vanilla at a constant 0.25 (one repeat
    /// per 80 world units). The constant also removes the hard tiling
    /// seams a varying per-texel factor produced. The field's real
    /// meaning is UNKNOWN; the per-code plumbing stays for future
    /// re-calibration.
    pub splat_factors: [f32; 5],
}

impl Default for TerrainRenderParams {
    fn default() -> Self {
        Self {
            lightmap_flip_v: false,
            lighting_mode: TerrainLightingMode::Dynamic,
            lightmap_enabled: true,
            shadow_strength: 0.0,
            splat_factors: [0.25; 5],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerrainLightingMode {
    /// Full PBR sun + ambient over the baked lightmap (current behavior).
    Dynamic,
    /// `albedo × lightmap × ambient tint × exposure` — keeps time of day,
    /// drops N·L and specular.
    FlatBaked,
    /// `albedo × lightmap` — fully baked ground (mobile port / original).
    Baked,
}

impl TerrainRenderParams {
    /// GPU layout: three vec4s — `[0]` = lightmap UV scale.xy + offset.zw,
    /// `[1].x` = lighting mode as float (shader branches on `< 0.5`/`< 1.5`),
    /// `[1].yzw` + `[2].xy` = the splat repeat factors for codes 0..32,
    /// `[2].z` = lightmap strength (1 = multiply baked lightmap into albedo,
    /// 0 = off, PBR mode), `[2].w` = extra sun-shadow darkening (the vanilla
    /// player-shadow boost, 0 = off).
    /// Factors are clamped away from 0 — the shader divides by them.
    fn to_gpu(&self) -> [f32; 12] {
        let (scale_v, offset_v) = if self.lightmap_flip_v {
            (-1.0, 1.0)
        } else {
            (1.0, 0.0)
        };
        let mode = match self.lighting_mode {
            TerrainLightingMode::Dynamic => 0.0,
            TerrainLightingMode::FlatBaked => 1.0,
            TerrainLightingMode::Baked => 2.0,
        };
        let f = self.splat_factors.map(|factor| factor.max(0.01));
        let lightmap = if self.lightmap_enabled { 1.0 } else { 0.0 };
        let shadow = self.shadow_strength.clamp(0.0, 1.0);
        [
            1.0, scale_v, 0.0, offset_v, mode, f[0], f[1], f[2], f[3], f[4], lightmap, shadow,
        ]
    }
}

/// The shared render-world buffer behind [`TerrainRenderParams`], bound by
/// every splat material's bind group (binding 6).
#[derive(Resource)]
pub struct TerrainRenderParamsBuffer(pub Buffer);

/// Shared 1×1 white texture used as the lightmap for regions that ship no `.t` file (or whose
/// lightmap failed to decode): white multiplies to a no-op in `terrain_splat.wgsl`, so such terrain
/// renders unmodulated. Created once at startup; every lightmap-less material clones this handle.
#[derive(Resource)]
pub struct TerrainLightmapFallback(pub Handle<Image>);

/// Re-derives the terrain render params from `graphics.terrain`
/// ([`crate::plugins::settings::live`]).
///
/// Previously computed in `Plugin::build`, which meant the lighting mode and
/// the lightmap V-flip were fixed for the whole process; `ExtractResourcePlugin`
/// copies this resource into the render world every frame, so writing it here
/// is all a live change needs. The resource keeps its `Default` for tests and
/// tools that build the plugin without a `ClientConfig`.
fn apply_terrain_render_params(
    config: Res<crate::plugins::config::ClientConfig>,
    mut params: ResMut<TerrainRenderParams>,
) {
    *params = config.graphics.terrain.to_render_params();
}

pub struct TerrainAmbientRatioPlugin;

impl Plugin for TerrainAmbientRatioPlugin {
    fn build(&self, app: &mut App) {
        super::tile_residency::register(app);
        app.init_resource::<TerrainAmbientRatio>()
            .init_resource::<TerrainRenderParams>()
            .add_plugins((
                ExtractResourcePlugin::<TerrainAmbientRatio>::default(),
                ExtractResourcePlugin::<TerrainRenderParams>::default(),
            ))
            .add_systems(
                PreUpdate,
                apply_terrain_render_params.run_if(crate::plugins::settings::live::config_changed),
            )
            .add_systems(Startup, init_terrain_lightmap_fallback)
            .add_systems(
                Update,
                build_tile_atlas.run_if(not(resource_exists::<TerrainTileAtlas>)),
            );
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .add_systems(
                RenderStartup,
                (
                    init_terrain_ambient_ratio_buffer,
                    init_terrain_params_buffer,
                    check_tile_atlas_support,
                ),
            )
            .add_systems(
                ExtractSchedule,
                extract_tile_atlas.run_if(not(resource_exists::<TerrainTileAtlas>)),
            )
            .add_systems(
                Render,
                (write_terrain_ambient_ratio, write_terrain_params)
                    .in_set(RenderSystems::PrepareResources),
            );
    }
}

/// Populates [`TerrainTileAtlas`] from `tile2d.ifo`, mapping each tile id to its texture handle.
///
/// Runs until the index asset is available, then inserts the resource (which drops it out via its
/// run condition). The handles resolve to assets `TileAssets` has already loaded — bevy_asset_loader
/// gates `GameState::Loading` on that whole collection — so by the time terrain builds, every
/// texture behind these handles is resident.
fn build_tile_atlas(
    mut commands: Commands,
    tile_assets: Option<Res<TileAssets>>,
    ifo_assets: Res<Assets<IFOAsset>>,
    asset_server: Res<AssetServer>,
) {
    let Some(tile_assets) = tile_assets else {
        return;
    };
    let Some(index) = ifo_assets
        .get(&tile_assets.tile_index)
        .and_then(|ifo| ifo.tile_info_index.as_ref())
    else {
        return;
    };

    let mut atlas = TerrainTileAtlas::new();
    let mut out_of_range = 0usize;
    for (id, info) in &index.tiles {
        let slot = *id as usize;
        // The map format can only express ids < TILE_SLOT_COUNT, so an id beyond it could never
        // be referenced by a vertex anyway — skip rather than grow the array.
        if slot >= TILE_SLOT_COUNT as usize {
            out_of_range += 1;
            continue;
        }
        atlas.slots[slot] =
            Some(asset_server.load(format!("map://tile2d/{}", info.texture.display())));
    }
    let defined = atlas.slots.iter().filter(|s| s.is_some()).count();
    if out_of_range > 0 {
        warn!("{out_of_range} tile ids in tile2d.ifo exceed the {TILE_SLOT_COUNT}-slot atlas");
    }
    info!("ground tile atlas: {defined} tile ids of {TILE_SLOT_COUNT} slots");
    commands.insert_resource(atlas);
}

/// Copies the atlas into the render world once; it is immutable after [`build_tile_atlas`], so
/// this runs exactly once rather than cloning 1024 handles every frame.
fn extract_tile_atlas(mut commands: Commands, atlas: Extract<Option<Res<TerrainTileAtlas>>>) {
    if let Some(atlas) = atlas.as_ref() {
        commands.insert_resource((*atlas).clone());
    }
}

fn init_terrain_lightmap_fallback(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let white = Image::new_fill(
        Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[255, 255, 255, 255],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    commands.insert_resource(TerrainLightmapFallback(images.add(white)));
}

/// The gather in `terrain_splat.wgsl` indexes `tile_atlas` with per-fragment data, so it needs
/// non-uniform indexing of a sampled-texture binding array, and the array needs `TILE_SLOT_COUNT`
/// elements per stage. Both are device capabilities bevy *detects* rather than guarantees — its
/// own light-probe code falls back when they are missing (`bevy_pbr::light_probe`) — so say so
/// loudly at startup rather than leaving mis-rendered ground to debug.
///
/// On Metal both hold whenever Argument Buffers Tier 2 is available (which reports 1,000,000
/// binding-array elements); the pre-Tier-2 tiers cap out at 96 and cannot host this atlas.
fn check_tile_atlas_support(render_device: Res<RenderDevice>) {
    let missing = WgpuFeatures::TEXTURE_BINDING_ARRAY
        | WgpuFeatures::SAMPLED_TEXTURE_AND_STORAGE_BUFFER_ARRAY_NON_UNIFORM_INDEXING;
    let missing = missing.difference(render_device.features());
    if !missing.is_empty() {
        error!(
            "GPU is missing {missing:?}; the ground-tile atlas needs a non-uniformly indexed \
             texture binding array and terrain will not render correctly without it"
        );
    }
    let limit = render_device
        .limits()
        .max_binding_array_elements_per_shader_stage;
    if limit < TILE_SLOT_COUNT {
        error!(
            "GPU allows {limit} binding-array elements per shader stage; the ground-tile atlas \
             needs {TILE_SLOT_COUNT}"
        );
    }
}

fn init_terrain_ambient_ratio_buffer(mut commands: Commands, render_device: Res<RenderDevice>) {
    let buffer = render_device.create_buffer_with_data(&BufferInitDescriptor {
        label: "terrain_ambient_ratio_shared_buffer".into(),
        contents: bytemuck::cast_slice(&[1.0f32; 4]),
        usage: BufferUsages::COPY_DST | BufferUsages::STORAGE,
    });
    commands.insert_resource(TerrainAmbientRatioBuffer(buffer));
}

fn init_terrain_params_buffer(mut commands: Commands, render_device: Res<RenderDevice>) {
    let buffer = render_device.create_buffer_with_data(&BufferInitDescriptor {
        label: "terrain_render_params_shared_buffer".into(),
        contents: bytemuck::cast_slice(&TerrainRenderParams::default().to_gpu()),
        usage: BufferUsages::COPY_DST | BufferUsages::STORAGE,
    });
    commands.insert_resource(TerrainRenderParamsBuffer(buffer));
}

fn write_terrain_params(
    params: Option<Res<TerrainRenderParams>>,
    buffer: Option<Res<TerrainRenderParamsBuffer>>,
    queue: Res<RenderQueue>,
) {
    let (Some(params), Some(buffer)) = (params, buffer) else {
        return;
    };
    if params.is_changed() {
        queue.write_buffer(&buffer.0, 0, bytemuck::cast_slice(&params.to_gpu()));
    }
}

fn write_terrain_ambient_ratio(
    ratio: Option<Res<TerrainAmbientRatio>>,
    buffer: Option<Res<TerrainAmbientRatioBuffer>>,
    queue: Res<RenderQueue>,
) {
    // ratio is absent until the first extraction, the buffer until RenderStartup ran
    let (Some(ratio), Some(buffer)) = (ratio, buffer) else {
        return;
    };
    if ratio.is_changed() {
        queue.write_buffer(&buffer.0, 0, bytemuck::cast_slice(&ratio.0));
    }
}

impl Material for TerrainBlockSplatMaterial {
    fn vertex_shader() -> ShaderRef {
        "shaders/terrain_splat.wgsl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "shaders/terrain_splat.wgsl".into()
    }

    #[inline]
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Opaque
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = if key.bind_group_data {
            Some(Face::Back)
        } else {
            None
        };
        Ok(())
    }
}

impl AsBindGroup for TerrainBlockSplatMaterial {
    type Data = bool;
    type Param = (
        Res<'static, RenderAssets<GpuImage>>,
        Res<'static, FallbackImage>,
        Option<Res<'static, TerrainAmbientRatioBuffer>>,
        Option<Res<'static, TerrainRenderParamsBuffer>>,
        Option<Res<'static, TerrainTileAtlas>>,
    );

    fn label() -> &'static str {
        "terrain_block_splat_material"
    }

    fn bind_group_data(&self) -> Self::Data {
        self.backface_culling
    }

    // `tile_atlas` needs a genuine WGPU texture binding array (`BindingResource::TextureViewArray`,
    // sized `TILE_SLOT_COUNT`), which `OwnedBindingResource`/`UnpreparedBindGroup` has no
    // variant for. Returning `CreateBindGroupDirectly` here routes the framework to
    // `as_bind_group()` below instead, which is allowed to build the raw wgpu bind group itself.
    fn unprepared_bind_group(
        &self,
        _layout: &BindGroupLayout,
        _render_device: &RenderDevice,
        _param: &mut <Self::Param as SystemParam>::Item<'_, '_>,
        _force_no_bindless: bool,
    ) -> Result<UnpreparedBindGroup, AsBindGroupError> {
        Err(AsBindGroupError::CreateBindGroupDirectly)
    }

    fn as_bind_group(
        &self,
        layout_descriptor: &BindGroupLayoutDescriptor,
        render_device: &RenderDevice,
        pipeline_cache: &PipelineCache,
        param: &mut <Self::Param as SystemParam>::Item<'_, '_>,
    ) -> Result<PreparedBindGroup, AsBindGroupError> {
        let layout = &pipeline_cache.get_bind_group_layout(layout_descriptor);
        let (image_assets, fallback_image, ambient_ratio_buffer, params_buffer, tile_atlas) = param;

        // Every guard below returns RetryNextUpdate, which bevy retries WITHOUT logging —
        // a dependency that never materializes therefore renders as "terrain silently
        // missing". The warn_once calls turn a stuck retry loop into a diagnosable log
        // line while staying quiet on the expected first-frames retries (each fires at
        // most once per run, and a healthy startup passes through these within a frame
        // or two of the resources appearing).

        // created in RenderStartup; not there yet on the very first prepares
        let Some(ambient_ratio_buffer) = ambient_ratio_buffer else {
            warn_once!("terrain splat: waiting for TerrainAmbientRatioBuffer (RenderStartup)");
            return Err(AsBindGroupError::RetryNextUpdate);
        };
        let Some(params_buffer) = params_buffer else {
            warn_once!("terrain splat: waiting for TerrainRenderParamsBuffer (RenderStartup)");
            return Err(AsBindGroupError::RetryNextUpdate);
        };
        // extracted once the main world has built it from `tile2d.ifo`
        let Some(tile_atlas) = tile_atlas else {
            warn_once!("terrain splat: waiting for the ground-tile atlas (tile2d.ifo)");
            return Err(AsBindGroupError::RetryNextUpdate);
        };

        let Some(tile_map_tex) = image_assets.get(&self.tile_map) else {
            warn_once!("terrain splat: waiting for a region tile map upload");
            return Err(AsBindGroupError::RetryNextUpdate);
        };
        let Some(lightmap_tex) = image_assets.get(&self.lightmap) else {
            warn_once!("terrain splat: waiting for a region lightmap upload");
            return Err(AsBindGroupError::RetryNextUpdate);
        };

        // Slots the atlas has no texture for (undefined tile ids) bind the fallback image, so the
        // array is always fully populated even though `tile2d.ifo` only defines 719 of the 1024
        // ids the map format can express. A tile whose image is still loading also falls back
        // rather than stalling the whole region — every terrain draw shares this one array, so
        // retrying until all 719 have landed would hold up the first region indefinitely.
        let fallback_view = &*fallback_image.d2.texture_view;
        let mut texture_views = vec![fallback_view; TILE_SLOT_COUNT as usize];
        for (slot, handle) in tile_atlas.slots.iter().enumerate() {
            if let Some(image) = handle.as_ref().and_then(|h| image_assets.get(h)) {
                texture_views[slot] = &*image.texture_view;
            }
        }

        let clamp_sampler = render_device.create_sampler(&SamplerDescriptor {
            min_filter: FilterMode::Linear,
            mag_filter: FilterMode::Linear,
            mipmap_filter: MipmapFilterMode::Linear,
            ..default()
        });
        let tile_sampler = render_device.create_sampler(&SamplerDescriptor {
            min_filter: FilterMode::Linear,
            mag_filter: FilterMode::Linear,
            mipmap_filter: MipmapFilterMode::Linear,
            address_mode_u: AddressMode::Repeat,
            address_mode_v: AddressMode::Repeat,
            address_mode_w: AddressMode::Repeat,
            // ground tiles are viewed at grazing angles almost everywhere;
            // 4x aniso keeps them sharp where trilinear over-blurs
            anisotropy_clamp: 4,
            ..default()
        });

        let bind_group = render_device.create_bind_group(
            "terrain_block_splat_material_bind_group",
            layout,
            &[
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureView(&*tile_map_tex.texture_view),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Sampler(&clamp_sampler),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: BindingResource::TextureViewArray(&texture_views[..]),
                },
                BindGroupEntry {
                    binding: 3,
                    resource: BindingResource::Sampler(&tile_sampler),
                },
                // the shared terrain-ambient buffer (see TerrainAmbientRatio):
                // updated in place, never re-created, so it is not owned below.
                // Storage, not uniform: wgpu forbids mixing a binding array (the
                // ground-texture array at binding 2) with uniform buffers in one
                // bind group.
                BindGroupEntry {
                    binding: 4,
                    resource: BindingResource::Buffer(
                        ambient_ratio_buffer.0.as_entire_buffer_binding(),
                    ),
                },
                // Baked region lightmap; sampled in the shader with `clamp_sampler`
                // (binding 1) at region-local UV. White 1×1 for regions without a `.t`.
                BindGroupEntry {
                    binding: 5,
                    resource: BindingResource::TextureView(&*lightmap_tex.texture_view),
                },
                // Global terrain render params — the second shared in-place buffer
                // (see TerrainRenderParams).
                BindGroupEntry {
                    binding: 6,
                    resource: BindingResource::Buffer(params_buffer.0.as_entire_buffer_binding()),
                },
            ],
        );

        Ok(PreparedBindGroup {
            // Nothing here is owned by the bind group any more: the tile atlas, the ambient
            // buffer and both samplers outlive it, and the per-region textures are plain assets.
            bindings: bevy::render::render_resource::BindingResources(vec![]),
            bind_group,
        })
    }

    fn bind_group_layout_entries(
        _render_device: &RenderDevice,
        _force_no_bindless: bool,
    ) -> Vec<BindGroupLayoutEntry>
    where
        Self: Sized,
    {
        vec![
            // 0: per-region tile map (Rg16Uint, textureLoad only)
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
            // 1: clamp sampler (lightmap)
            BindGroupLayoutEntry {
                binding: 1,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Sampler(SamplerBindingType::Filtering),
                count: None,
            },
            // 2: the global ground-tile atlas, indexed by tile id
            BindGroupLayoutEntry {
                binding: 2,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Texture {
                    multisampled: false,
                    sample_type: TextureSampleType::Float { filterable: true },
                    view_dimension: TextureViewDimension::D2,
                },
                count: NonZeroU32::new(TILE_SLOT_COUNT),
            },
            // 3: repeat+aniso sampler (ground tiles)
            BindGroupLayoutEntry {
                binding: 3,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Sampler(SamplerBindingType::Filtering),
                count: None,
            },
            // 4: shared terrain ambient ratio
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
            // 5: baked region lightmap
            BindGroupLayoutEntry {
                binding: 5,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Texture {
                    multisampled: false,
                    sample_type: TextureSampleType::Float { filterable: true },
                    view_dimension: TextureViewDimension::D2,
                },
                count: None,
            },
            // 6: shared global terrain render params
            BindGroupLayoutEntry {
                binding: 6,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ]
    }
}

impl TerrainBlockSplatMaterial {
    /// Builds the material for one region's merged 6x6 block grid. `blocks` is `(block, dx, dz)`
    /// in the same order and offset convention as `block_mesh::merge_block_meshes`; the tile map
    /// is addressed by each block's own `(x, z)` grid position rather than by `dx`/`dz`, so the
    /// packing is independent of the mesh's vertex ordering.
    pub(crate) fn from(
        blocks: &[(&TerrainBlock, f32, f32)],
        lightmap: Handle<Image>,
        image_assets: &mut ResMut<Assets<Image>>,
    ) -> Self {
        let tile_map = image_assets.add(Image::new(
            Extent3d {
                width: TILE_MAP_SIZE as u32,
                height: TILE_MAP_SIZE as u32,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            pack_tile_map(blocks),
            TextureFormat::Rg16Uint,
            RenderAssetUsages::RENDER_WORLD,
        ));

        Self {
            tile_map,
            lightmap,
            backface_culling: true,
        }
    }
}

/// Packs a region's per-vertex tile choices into the `Rg16Uint` 102x102 layout described on
/// [`TerrainBlockSplatMaterial::tile_map`]: `r` = tile id, `g` = splat scale code.
///
/// Blocks are placed by their own `(x, z)` grid position, so this does not depend on the order
/// `blocks` arrives in or on the mesh's vertex layout.
fn pack_tile_map(blocks: &[(&TerrainBlock, f32, f32)]) -> Vec<u8> {
    // Every vertex of every block is written, so the zero fill never survives into a texel that
    // the shader can reach — tile id 0 is a real tile, so a gap here would render as one.
    let mut buf = vec![0u8; TILE_MAP_SIZE * TILE_MAP_SIZE * 4];
    for (block, _, _) in blocks {
        let (bx, bz) = (block.x as usize, block.z as usize);
        debug_assert!(bx < REGION_BLOCKS_PER_SIDE && bz < REGION_BLOCKS_PER_SIDE);
        for z in 0..BLOCK_VERTS {
            for x in 0..BLOCK_VERTS {
                let v = &block.vertices[z * BLOCK_VERTS + x];
                let texel = (bz * BLOCK_VERTS + z) * TILE_MAP_SIZE + (bx * BLOCK_VERTS + x);
                let o = texel * 4;
                buf[o..o + 2].copy_from_slice(&v.texture_id.to_le_bytes());
                buf[o + 2..o + 4].copy_from_slice(&(v.splat_scale as u16).to_le_bytes());
            }
        }
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::m::{MapVertex, WaterType};
    use bevy::camera::primitives::Aabb;
    use bevy::math::Vec3;

    fn block(bx: i32, bz: i32, tile_at: impl Fn(usize, usize) -> (u16, u8)) -> TerrainBlock {
        let mut vertices = Vec::with_capacity(BLOCK_VERTS * BLOCK_VERTS);
        for z in 0..BLOCK_VERTS {
            for x in 0..BLOCK_VERTS {
                let (texture_id, splat_scale) = tile_at(x, z);
                vertices.push(MapVertex {
                    x: x as i32,
                    z: z as i32,
                    height: 0.0,
                    texture_id,
                    brightness: 0,
                    splat_scale,
                    splat_offset: 0,
                });
            }
        }
        TerrainBlock {
            x: bx,
            z: bz,
            flag: 0,
            environment_id: 0,
            water_type: WaterType::None,
            vertices,
            tiles: Vec::new(),
            aabb: Aabb::from_min_max(Vec3::ZERO, Vec3::ONE),
        }
    }

    fn texel(buf: &[u8], u: usize, v: usize) -> (u16, u16) {
        let o = (v * TILE_MAP_SIZE + u) * 4;
        (
            u16::from_le_bytes([buf[o], buf[o + 1]]),
            u16::from_le_bytes([buf[o + 2], buf[o + 3]]),
        )
    }

    /// A block lands at its own grid position, and vertex (x, z) at texel
    /// (bx * 17 + x, bz * 17 + z) — the addressing `sample_splat` assumes.
    #[test]
    fn packs_blocks_at_their_grid_position() {
        // tile id encodes the vertex so a transposed or mis-strided write is visible
        let b = block(2, 3, |x, z| ((z * BLOCK_VERTS + x) as u16, 8));
        let buf = pack_tile_map(&[(&b, 0.0, 0.0)]);

        assert_eq!(texel(&buf, 2 * 17, 3 * 17), (0, 8), "vertex (0,0)");
        assert_eq!(texel(&buf, 2 * 17 + 5, 3 * 17), (5, 8), "vertex (5,0)");
        assert_eq!(texel(&buf, 2 * 17, 3 * 17 + 5), (5 * 17, 8), "vertex (0,5)");
        assert_eq!(
            texel(&buf, 2 * 17 + 16, 3 * 17 + 16),
            ((16 * 17 + 16) as u16, 8),
            "vertex (16,16)"
        );
        // a different block's area is untouched by this one
        assert_eq!(texel(&buf, 0, 0), (0, 0));
    }

    /// Neighbouring blocks keep their own duplicated edge vertices: the shared world position at
    /// block b's vertex 16 and block b+1's vertex 0 occupies two distinct texels, which is what
    /// stops a gather from ever crossing a block boundary.
    #[test]
    fn adjacent_blocks_keep_separate_edge_vertices() {
        let left = block(0, 0, |_, _| (11, 16));
        let right = block(1, 0, |_, _| (22, 32));
        let buf = pack_tile_map(&[(&left, 0.0, 0.0), (&right, 320.0, 0.0)]);

        assert_eq!(texel(&buf, 16, 0), (11, 16), "left block's last vertex");
        assert_eq!(texel(&buf, 17, 0), (22, 32), "right block's first vertex");
    }

    /// The full 6x6 grid covers every texel of the 102x102 map, so no texel is left at the
    /// zero fill (which would render as tile id 0 rather than the authored tile).
    #[test]
    fn full_region_leaves_no_unwritten_texels() {
        let blocks: Vec<TerrainBlock> = (0..REGION_BLOCKS_PER_SIDE as i32)
            .flat_map(|bz| {
                (0..REGION_BLOCKS_PER_SIDE as i32).map(move |bx| block(bx, bz, |_, _| (7, 16)))
            })
            .collect();
        let refs: Vec<(&TerrainBlock, f32, f32)> = blocks.iter().map(|b| (b, 0.0, 0.0)).collect();
        let buf = pack_tile_map(&refs);

        assert_eq!(buf.len(), TILE_MAP_SIZE * TILE_MAP_SIZE * 4);
        for v in 0..TILE_MAP_SIZE {
            for u in 0..TILE_MAP_SIZE {
                assert_eq!(texel(&buf, u, v), (7, 16), "texel ({u},{v}) unwritten");
            }
        }
    }

    /// Tile ids are 10-bit by construction, so every id the map can express indexes the atlas.
    #[test]
    fn every_expressible_tile_id_fits_the_atlas() {
        assert!(u16::from(u16::MAX & 0b0000_0011_1111_1111) < TILE_SLOT_COUNT as u16);
        assert_eq!(
            TerrainTileAtlas::new().slots.len(),
            TILE_SLOT_COUNT as usize
        );
    }
}
