#[cfg(not(feature = "terrain_hand_rolled_pipeline"))]
use std::collections::HashSet;

use bevy::asset::{Assets, RenderAssetUsages};
use bevy::prelude::{
    error, info, not, resource_exists, warn, App, AssetServer, Commands, Component, DetectChanges,
    Handle, Image, IntoScheduleConfigs, Plugin, PreUpdate, Res, ResMut, Resource, Startup, Update,
};
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::render_resource::{
    Buffer, BufferInitDescriptor, BufferUsages, Extent3d, TextureDimension, TextureFormat,
};
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::render::settings::WgpuFeatures;
use bevy::render::{Extract, ExtractSchedule, Render, RenderApp, RenderStartup, RenderSystems};

use crate::assets::ifo::IFOAsset;
use crate::assets::m::TerrainBlock;
use crate::plugins::map::assets::TileAssets;

// Only the Material-based draw path (the default) needs bevy's Material/AsBindGroup machinery —
// the hand-rolled pipeline (`plugins::map::terrain::render`, feature
// `terrain_hand_rolled_pipeline`) builds its bind groups by hand from `RenderDevice` directly.
#[cfg(not(feature = "terrain_hand_rolled_pipeline"))]
use bevy::asset::Asset;
#[cfg(not(feature = "terrain_hand_rolled_pipeline"))]
use bevy::ecs::system::SystemParam;
#[cfg(not(feature = "terrain_hand_rolled_pipeline"))]
use bevy::log::warn_once;
#[cfg(not(feature = "terrain_hand_rolled_pipeline"))]
use bevy::mesh::MeshVertexBufferLayoutRef;
#[cfg(not(feature = "terrain_hand_rolled_pipeline"))]
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey};
#[cfg(not(feature = "terrain_hand_rolled_pipeline"))]
use bevy::prelude::{default, AlphaMode, Material};
#[cfg(not(feature = "terrain_hand_rolled_pipeline"))]
use bevy::reflect::TypePath;
#[cfg(not(feature = "terrain_hand_rolled_pipeline"))]
use bevy::render::render_asset::RenderAssets;
#[cfg(not(feature = "terrain_hand_rolled_pipeline"))]
use bevy::render::render_resource::{
    AddressMode, AsBindGroup, AsBindGroupError, BindGroupEntry, BindGroupLayout,
    BindGroupLayoutDescriptor, BindGroupLayoutEntry, BindingResource, BindingType,
    BufferBindingType, Face, FilterMode, MipmapFilterMode, PipelineCache, PreparedBindGroup,
    RenderPipelineDescriptor, SamplerBindingType, SamplerDescriptor, ShaderStages,
    SpecializedMeshPipelineError, TextureSampleType, TextureViewDimension, UnpreparedBindGroup,
};
#[cfg(not(feature = "terrain_hand_rolled_pipeline"))]
use bevy::render::texture::{FallbackImage, GpuImage};
#[cfg(not(feature = "terrain_hand_rolled_pipeline"))]
use bevy::shader::ShaderRef;
#[cfg(not(feature = "terrain_hand_rolled_pipeline"))]
use std::num::NonZeroU32;

/// Size of [`TerrainTileAtlas`]'s CPU-side registry, mapping every tile id the map format can
/// express to its texture handle. `JMXVMAPM` packs a vertex's tile id into 10 bits
/// (`assets/m/mod.rs`: `flags & 0b0000_0011_1111_1111`), so 1024 covers the entire id space by
/// construction. The shipped `tile2d.ifo` defines 719 ids (dense 0..718), of which 685 are
/// referenced by some region (Map.pk2 census 2026-08-08).
///
/// This is *not* the size of the GPU-bound `tile_atlas` binding array any more — see
/// [`REGION_TILE_SLOT_COUNT`]. This registry stays global because it's cheap (a `Vec` of
/// handles, no GPU resources) and every region needs to resolve its own tile ids against it.
pub const TILE_SLOT_COUNT: u32 = 1024;

/// Size of the *per-region* `tile_atlas` binding array actually bound to the GPU (see
/// [`TerrainBlockSplatMaterial::used_tiles`]).
///
/// Global, direct 10-bit-id indexing (`TILE_SLOT_COUNT` = 1024 slots bound per region,
/// regardless of how many of those textures that region's own ground actually uses) was the
/// previous design — see git history / `docs/perf-remote.md` for why it was replaced: every
/// region's bind group ended up referencing all ~719 globally-defined textures, ~700 of which
/// it could never sample. This trades that for a hard cap: a region's tile ids are remapped to
/// a compact, region-local index (0..`REGION_TILE_SLOT_COUNT`) baked into `tile_map` by
/// `pack_tile_map`, and the bind group only needs `REGION_TILE_SLOT_COUNT` slots, not 1024.
///
/// 64 is not a guess: a 2026-08-08 census of the shipped map data (see the doc comment on
/// [`TILE_SLOT_COUNT`]) found a real region using 38 distinct tiles — the worst case on record.
/// 64 leaves that comfortable headroom rather than sitting right at the documented max. A region
/// that somehow exceeds it degrades loudly instead of silently: `TerrainBlockSplatMaterial::from`
/// logs a `warn!` and the overflow tiles render as whichever tile lands in the last slot, rather
/// than corrupting the bind group or panicking.
///
/// **Not done, and deliberately out of scope here**: every region still gets its *own* bind
/// group (just a 64-slot one now instead of a 1024-slot one) — this does not eliminate
/// per-region bind-group duplication itself, only shrink it. A true single bind group shared by
/// every region (binding the atlas + samplers + `TerrainAmbientRatioBuffer`/
/// `TerrainRenderParamsBuffer` exactly once, globally) is not reachable through `Material`:
/// Bevy 0.19's "bindless" material support is shaped for a handful of *named* per-material
/// textures with slab-level dedup, not one shared array (and slab capacity is platform-capped as
/// low as 64 resources on macOS/iOS); `Material`'s draw-command chain
/// (view/mesh/material bind groups only) is fixed by a blanket `impl<M: Material> for
/// MeshMaterial3d<M>` inside `bevy_pbr` itself, with no supported extension point for a 4th,
/// globally-bound group. The only route that actually shares one bind group globally is dropping
/// `Material`/`MaterialPlugin` for terrain and hand-rolling a `SpecializedMeshPipeline` + custom
/// `RenderCommand`/`DrawFunctions` registration from scratch — real precedent is
/// `bevy_pbr::wireframe` (its own pipeline, draw command tuple, and a bind group sourced from a
/// per-frame-prepared `Resource` instead of per-material). That's real, buildable, moderate-to-
/// large new code (order of a few hundred lines: extract/specialize/queue/prepare systems
/// modeled on `bevy_pbr::material`'s own, trimmed to this one material), but it also means
/// terrain loses shadow-casting, prepass and deferred-pass support — which `Material` currently
/// provides for free and which `config.yaml` has switched on (`shadows.enabled: true`, and
/// terrain does cast/receive shadows today) — unless those are separately reimplemented too.
#[cfg_attr(feature = "terrain_hand_rolled_pipeline", allow(dead_code))]
pub const REGION_TILE_SLOT_COUNT: u32 = 64;

/// Vertices per block edge, and blocks per region edge — the `tile_map` packing below.
pub(crate) const BLOCK_VERTS: usize = 17;
pub(crate) const REGION_BLOCKS_PER_SIDE: usize = 6;
/// `tile_map` is one 102x102 texture per region: 6 blocks of 17 vertices per axis, with each
/// block keeping its own duplicated edge vertices so a fragment never gathers across a block
/// boundary (see the header comment in `terrain_splat.wgsl`).
pub(crate) const TILE_MAP_SIZE: usize = REGION_BLOCKS_PER_SIDE * BLOCK_VERTS;

/// Every ground tile texture in the game, indexed by the map's 10-bit tile id — the CPU-side
/// registry a region's [`TerrainBlockSplatMaterial::used_tiles`] resolves against to find each
/// tile's real texture handle. Not bound to the GPU directly (see
/// [`REGION_TILE_SLOT_COUNT`]) — each region's bind group only carries the handles for the
/// handful of ids it actually references.
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

#[cfg(not(feature = "terrain_hand_rolled_pipeline"))]
#[derive(TypePath, Asset, Default, Debug, Clone)]
pub struct TerrainBlockSplatMaterial {
    /// Per-vertex tile choice for one region, as a 102x102 `Rg16Uint` image: `r` = this
    /// region's *local* tile index (position in [`used_tiles`](Self::used_tiles), not the raw
    /// map tile id — see [`REGION_TILE_SLOT_COUNT`]), `g` = splat scale code. Texel
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

    /// Distinct tile ids this region's own vertices reference — sorted, deduped, and capped at
    /// [`REGION_TILE_SLOT_COUNT`]. Position in this `Vec` *is* the local index `tile_map` bakes
    /// into its `r` channel (see [`TerrainBlockSplatMaterial::tile_map`]) and the slot
    /// `as_bind_group` fills in the GPU-bound `tile_atlas` array — so this is what makes the
    /// per-region binding array small instead of the full ~719-entry global registry. A region
    /// needing more than `REGION_TILE_SLOT_COUNT` distinct tiles (none on record as of the
    /// 2026-08-08 census — see [`REGION_TILE_SLOT_COUNT`]) has its overflow ids dropped here and
    /// `pack_tile_map` clamps their texels to the last slot instead of producing an out-of-range
    /// index.
    pub used_tiles: Vec<u16>,
}

/// Per-region ground textures for the hand-rolled render pipeline
/// (`client/src/plugins/map/terrain/render/`, feature `terrain_hand_rolled_pipeline`) — the
/// counterpart to [`TerrainBlockSplatMaterial`] for that path.
///
/// A plain `Component`, not an `Asset`: unlike a normal material, these textures are never
/// shared/deduplicated across users (each region's `tile_map`/`lightmap` are already unique), so
/// the `Handle<Self>`/`Assets<T>` indirection `TerrainBlockSplatMaterial` needs would be pure
/// overhead here. Extracted into the render world by
/// `plugins::map::terrain::render::extract_terrain_ground_textures`.
#[cfg(feature = "terrain_hand_rolled_pipeline")]
#[derive(Component, Clone, Default)]
pub struct TerrainGroundTextures {
    /// Per-vertex tile choice for one region, as a 102x102 `Rg16Uint` image — same layout as
    /// [`TerrainBlockSplatMaterial::tile_map`], except `r` is always the map's raw 10-bit tile
    /// id (see `pack_tile_map_raw`): the hand-rolled pipeline binds the tile atlas exactly once,
    /// globally, so there is no per-region size pressure to remap ids down to a small local
    /// index the way the `Material`-based path needs.
    pub tile_map: Handle<Image>,
    pub backface_culling: bool,
    /// Baked terrain lightmap — see [`TerrainBlockSplatMaterial::lightmap`].
    pub lightmap: Handle<Image>,
}

#[cfg(feature = "terrain_hand_rolled_pipeline")]
impl TerrainGroundTextures {
    /// Builds the per-region textures for one region's merged 6x6 block grid. Same role as
    /// `TerrainBlockSplatMaterial::from`, minus the local-index remap: see the type's own doc
    /// comment for why the hand-rolled pipeline doesn't need it.
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
            pack_tile_map_raw(blocks),
            TextureFormat::Rg16Uint,
            RenderAssetUsages::RENDER_WORLD,
        ));

        Self {
            tile_map,
            backface_culling: true,
            lightmap,
        }
    }
}

/// Packs a region's per-vertex tile choices the same way as [`pack_tile_map`], except `r` is the
/// vertex's raw map tile id directly — no local-index remap, since the hand-rolled pipeline's
/// `tile_atlas` is bound once, globally, at the map format's full 1024-id size (see
/// `REGION_TILE_SLOT_COUNT`'s doc comment for why the `Material`-based path needs the remap and
/// this one doesn't).
#[cfg(feature = "terrain_hand_rolled_pipeline")]
fn pack_tile_map_raw(blocks: &[(&TerrainBlock, f32, f32)]) -> Vec<u8> {
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

/// Exercises `pack_tile_map_raw`'s raw-id packing (the hand-rolled pipeline's path — no local
/// remap, see `REGION_TILE_SLOT_COUNT`'s doc comment). Mirrors the shape of `pack_tile_map`'s own
/// tests below, minus the remap-specific ones (used_tiles/overflow-clamp), which don't apply here.
#[cfg(all(test, feature = "terrain_hand_rolled_pipeline"))]
mod raw_pack_tests {
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
    /// (bx * 17 + x, bz * 17 + z) — same addressing as `pack_tile_map`, just with the raw map
    /// tile id instead of a remapped local index.
    #[test]
    fn packs_blocks_at_their_grid_position_with_raw_ids() {
        let b = block(2, 3, |x, z| ((z * BLOCK_VERTS + x) as u16, 8));
        let buf = pack_tile_map_raw(&[(&b, 0.0, 0.0)]);

        assert_eq!(texel(&buf, 2 * 17, 3 * 17), (0, 8), "vertex (0,0)");
        assert_eq!(texel(&buf, 2 * 17 + 5, 3 * 17), (5, 8), "vertex (5,0)");
        assert_eq!(texel(&buf, 2 * 17, 3 * 17 + 5), (5 * 17, 8), "vertex (0,5)");
        assert_eq!(
            texel(&buf, 2 * 17 + 16, 3 * 17 + 16),
            ((16 * 17 + 16) as u16, 8),
            "vertex (16,16)"
        );
        assert_eq!(
            texel(&buf, 0, 0),
            (0, 0),
            "a different block's area is untouched"
        );
    }

    /// Neighbouring blocks keep their own duplicated edge vertices, same as `pack_tile_map`.
    #[test]
    fn adjacent_blocks_keep_separate_edge_vertices() {
        let left = block(0, 0, |_, _| (11, 16));
        let right = block(1, 0, |_, _| (22, 32));
        let buf = pack_tile_map_raw(&[(&left, 0.0, 0.0), (&right, 320.0, 0.0)]);

        assert_eq!(texel(&buf, 16, 0), (11, 16), "left block's last vertex");
        assert_eq!(texel(&buf, 17, 0), (22, 32), "right block's first vertex");
    }
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
///   current full PBR sun + ambient over the baked lightmap; `Baked` is
///   `albedo × lightmap`, full stop — the mobile port's (and probably the
///   original's) fully-baked ground. A third mode `FlatBaked` (GPU value 1.0)
///   was removed in ferdoran/openroad#5: it scaled the ground by the ambient
///   term alone, which in `render_mode: pbr` is 30x smaller than in vanilla
///   and arrives without the lightmap, so the ground rendered black.
///
/// Kept in step with `graphics.terrain` by `apply_terrain_render_params`
/// (see [`crate::plugins::settings::live`]); the render-debug panel and the
/// dungeon atmosphere override cycle it live on top of that.
#[derive(Resource, Clone, PartialEq, ExtractResource)]
pub struct TerrainRenderParams {
    pub lightmap_flip_v: bool,
    pub lighting_mode: TerrainLightingMode,
    /// Baked terrain lightmap on/off — follows `graphics.render_mode`
    /// (`Vanilla` = on) as a baseline (`apply_terrain_render_params`), with
    /// the environment plugin's live hotkey-N override
    /// (`EnvironmentSettings.mode`, `apply_render_mode`) able to flip it for
    /// the current session without touching config. NOT owned by the
    /// render-debug panel; `on_settings_changed` carries the current value
    /// through its rebuild instead of resetting it.
    pub lightmap_enabled: bool,
    /// Extra sun-shadow darkening on the lit ground. Always 0 now — it stood
    /// in for a vanilla-mode player shadow that vanilla no longer casts at
    /// all (vanilla fully disables the Sun's shadow maps). Kept as a no-op
    /// GPU-layout slot rather than reworking `terrain_splat.wgsl`'s uniform
    /// layout for its removal.
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
    /// `albedo × lightmap` — fully baked ground (mobile port / original).
    /// Encoded as 2.0, not 1.0: 1.0 was a removed third mode (see the type
    /// doc) and the panel/`perf-capture.ps1` numbering stays as it was.
    Baked,
}

impl TerrainRenderParams {
    /// GPU layout: three vec4s — `[0]` = lightmap UV scale.xy + offset.zw,
    /// `[1].x` = lighting mode as float (the shader branches on `< 0.5`),
    /// `[1].yzw` + `[2].xy` = the splat repeat factors for codes 0..32,
    /// `[2].z` = lightmap strength (1 = multiply baked lightmap into albedo,
    /// 0 = off, PBR mode), `[2].w` = extra sun-shadow darkening — unused,
    /// always 0 (see [`TerrainRenderParams::shadow_strength`]).
    /// Factors are clamped away from 0 — the shader divides by them.
    fn to_gpu(&self) -> [f32; 12] {
        let (scale_v, offset_v) = if self.lightmap_flip_v {
            (-1.0, 1.0)
        } else {
            (1.0, 0.0)
        };
        let mode = match self.lighting_mode {
            TerrainLightingMode::Dynamic => 0.0,
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
    *params = config
        .graphics
        .terrain
        .to_render_params(config.graphics.render_mode);
}

pub struct TerrainAmbientRatioPlugin;

impl Plugin for TerrainAmbientRatioPlugin {
    fn build(&self, app: &mut App) {
        // Works around a leak specific to the Material-based path's CreateBindGroupDirectly
        // bind group (see TerrainAmbientRatio's doc comment) — the hand-rolled pipeline never
        // goes through that allocator at all, so it has nothing to work around here.
        #[cfg(not(feature = "terrain_hand_rolled_pipeline"))]
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
/// non-uniform indexing of a sampled-texture binding array, and the array needs
/// `REGION_TILE_SLOT_COUNT` elements per stage. Both are device capabilities bevy *detects*
/// rather than guarantees — its own light-probe code falls back when they are missing
/// (`bevy_pbr::light_probe`) — so say so loudly at startup rather than leaving mis-rendered
/// ground to debug.
///
/// On Metal both hold whenever Argument Buffers Tier 2 is available (which reports 1,000,000
/// binding-array elements); the pre-Tier-2 tiers cap out at 96 — comfortably above
/// `REGION_TILE_SLOT_COUNT` (64), unlike the previous 1024-slot global design this replaced.
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
    // The hand-rolled pipeline binds the atlas once, globally, at the map format's full 1024-id
    // size (no per-region remap — see REGION_TILE_SLOT_COUNT's doc comment); the Material-based
    // path binds a region-local remapped copy sized REGION_TILE_SLOT_COUNT instead.
    #[cfg(feature = "terrain_hand_rolled_pipeline")]
    let needed = TILE_SLOT_COUNT;
    #[cfg(not(feature = "terrain_hand_rolled_pipeline"))]
    let needed = REGION_TILE_SLOT_COUNT;

    let limit = render_device
        .limits()
        .max_binding_array_elements_per_shader_stage;
    if limit < needed {
        error!(
            "GPU allows {limit} binding-array elements per shader stage; the ground-tile atlas \
             needs {needed}"
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

#[cfg(not(feature = "terrain_hand_rolled_pipeline"))]
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

#[cfg(not(feature = "terrain_hand_rolled_pipeline"))]
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
    // sized `REGION_TILE_SLOT_COUNT`), which `OwnedBindingResource`/`UnpreparedBindGroup` has no
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

        // `texture_views[local_index]` for `local_index` = position of a tile id in
        // `self.used_tiles` — the same local index `pack_tile_map` baked into `tile_map`'s `r`
        // channel, so a fragment's lookup and this array line up by construction. Only
        // `REGION_TILE_SLOT_COUNT` slots exist at all (not the ~719-entry global registry), and a
        // tile whose image is still loading falls back rather than stalling the region.
        let fallback_view = &*fallback_image.d2.texture_view;
        let mut texture_views = vec![fallback_view; REGION_TILE_SLOT_COUNT as usize];
        for (local_index, &tile_id) in self.used_tiles.iter().enumerate() {
            let Some(Some(handle)) = tile_atlas.slots.get(tile_id as usize) else {
                continue;
            };
            if let Some(image) = image_assets.get(handle) {
                texture_views[local_index] = &*image.texture_view;
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
            // 2: this region's local ground-tile atlas, indexed by the region-local index
            // `pack_tile_map` bakes into `tile_map` (see `TerrainBlockSplatMaterial::used_tiles`)
            BindGroupLayoutEntry {
                binding: 2,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Texture {
                    multisampled: false,
                    sample_type: TextureSampleType::Float { filterable: true },
                    view_dimension: TextureViewDimension::D2,
                },
                count: NonZeroU32::new(REGION_TILE_SLOT_COUNT),
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

#[cfg(not(feature = "terrain_hand_rolled_pipeline"))]
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
        let mut used_tiles: Vec<u16> = blocks
            .iter()
            .flat_map(|(block, _, _)| block.vertices.iter().map(|v| v.texture_id))
            .collect::<HashSet<u16>>()
            .into_iter()
            .collect();
        used_tiles.sort_unstable();
        // See `REGION_TILE_SLOT_COUNT`'s doc comment: no region on record needs this, but a
        // region that does gets a loud warning and clamped (wrong, not corrupt) overflow tiles
        // rather than an out-of-range GPU index.
        if used_tiles.len() > REGION_TILE_SLOT_COUNT as usize {
            warn!(
                "terrain region uses {} distinct ground tiles, above the {}-slot local atlas \
                 cap; {} tile(s) will render as whichever tile lands in the last slot",
                used_tiles.len(),
                REGION_TILE_SLOT_COUNT,
                used_tiles.len() - REGION_TILE_SLOT_COUNT as usize
            );
            used_tiles.truncate(REGION_TILE_SLOT_COUNT as usize);
        }

        let tile_map = image_assets.add(Image::new(
            Extent3d {
                width: TILE_MAP_SIZE as u32,
                height: TILE_MAP_SIZE as u32,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            pack_tile_map(blocks, &used_tiles),
            TextureFormat::Rg16Uint,
            RenderAssetUsages::RENDER_WORLD,
        ));

        Self {
            tile_map,
            lightmap,
            backface_culling: true,
            used_tiles,
        }
    }
}

/// Packs a region's per-vertex tile choices into the `Rg16Uint` 102x102 layout described on
/// [`TerrainBlockSplatMaterial::tile_map`]: `r` = this vertex's tile's *local* index (its
/// position in `used_tiles`, not its raw map id — see [`REGION_TILE_SLOT_COUNT`]), `g` = splat
/// scale code.
///
/// Blocks are placed by their own `(x, z)` grid position, so this does not depend on the order
/// `blocks` arrives in or on the mesh's vertex layout. `used_tiles` must be sorted (its own
/// binary search relies on it) — `TerrainBlockSplatMaterial::from` guarantees this.
#[cfg(not(feature = "terrain_hand_rolled_pipeline"))]
fn pack_tile_map(blocks: &[(&TerrainBlock, f32, f32)], used_tiles: &[u16]) -> Vec<u8> {
    // Every vertex of every block is written, so the zero fill never survives into a texel that
    // the shader can reach — local index 0 is a real (if possibly truncated-into) tile, so a gap
    // here would render as one.
    let mut buf = vec![0u8; TILE_MAP_SIZE * TILE_MAP_SIZE * 4];
    for (block, _, _) in blocks {
        let (bx, bz) = (block.x as usize, block.z as usize);
        debug_assert!(bx < REGION_BLOCKS_PER_SIDE && bz < REGION_BLOCKS_PER_SIDE);
        for z in 0..BLOCK_VERTS {
            for x in 0..BLOCK_VERTS {
                let v = &block.vertices[z * BLOCK_VERTS + x];
                // `Err` means this tile id was truncated out of `used_tiles` (the cap-overflow
                // case `TerrainBlockSplatMaterial::from` already warned about) — clamp to the
                // last real slot rather than baking an index `tile_atlas` was never sized for.
                let local_index = used_tiles
                    .binary_search(&v.texture_id)
                    .unwrap_or(used_tiles.len().saturating_sub(1))
                    as u16;
                let texel = (bz * BLOCK_VERTS + z) * TILE_MAP_SIZE + (bx * BLOCK_VERTS + x);
                let o = texel * 4;
                buf[o..o + 2].copy_from_slice(&local_index.to_le_bytes());
                buf[o + 2..o + 4].copy_from_slice(&(v.splat_scale as u16).to_le_bytes());
            }
        }
    }
    buf
}

// Exercises `pack_tile_map`'s region-local remap, which only exists on the Material-based path
// (see `REGION_TILE_SLOT_COUNT`'s doc comment) — `pack_tile_map_raw`'s raw-id packing is covered
// separately, see below.
#[cfg(all(test, not(feature = "terrain_hand_rolled_pipeline")))]
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

    /// Mirrors `TerrainBlockSplatMaterial::from`'s own derivation (sorted, deduped, no cap —
    /// these fixtures stay well under `REGION_TILE_SLOT_COUNT`), so tests can build the
    /// `used_tiles` a real caller would pass to `pack_tile_map` without duplicating cap logic.
    fn used_tiles_for(blocks: &[(&TerrainBlock, f32, f32)]) -> Vec<u16> {
        let mut used: Vec<u16> = blocks
            .iter()
            .flat_map(|(block, _, _)| block.vertices.iter().map(|v| v.texture_id))
            .collect::<HashSet<u16>>()
            .into_iter()
            .collect();
        used.sort_unstable();
        used
    }

    /// A block lands at its own grid position, and vertex (x, z) at texel
    /// (bx * 17 + x, bz * 17 + z) — the addressing `sample_splat` assumes.
    #[test]
    fn packs_blocks_at_their_grid_position() {
        // tile id encodes the vertex (mod 50, to stay under REGION_TILE_SLOT_COUNT) so a
        // transposed or mis-strided write is visible. 289 vertices mod 50 cycles through every
        // residue 0..49, so `used_tiles` is exactly [0..49] and local index == raw id here —
        // computed via `used_tiles_for` rather than assumed, so this doesn't silently rot if
        // that stops being true.
        let b = block(2, 3, |x, z| (((z * BLOCK_VERTS + x) as u16) % 50, 8));
        let used_tiles = used_tiles_for(&[(&b, 0.0, 0.0)]);
        let buf = pack_tile_map(&[(&b, 0.0, 0.0)], &used_tiles);
        let local = |raw: u16| {
            used_tiles
                .binary_search(&raw)
                .expect("raw id is in used_tiles")
        };

        assert_eq!(
            texel(&buf, 2 * 17, 3 * 17),
            (local(0) as u16, 8),
            "vertex (0,0)"
        );
        assert_eq!(
            texel(&buf, 2 * 17 + 5, 3 * 17),
            (local(5) as u16, 8),
            "vertex (5,0)"
        );
        assert_eq!(
            texel(&buf, 2 * 17, 3 * 17 + 5),
            (local((5 * 17) % 50) as u16, 8),
            "vertex (0,5)"
        );
        assert_eq!(
            texel(&buf, 2 * 17 + 16, 3 * 17 + 16),
            (local((16 * 17 + 16) % 50) as u16, 8),
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
        let blocks = [(&left, 0.0, 0.0), (&right, 320.0, 0.0)];
        let used_tiles = used_tiles_for(&blocks);
        let buf = pack_tile_map(&blocks, &used_tiles);
        let local = |raw: u16| {
            used_tiles
                .binary_search(&raw)
                .expect("raw id is in used_tiles")
        };

        assert_eq!(
            texel(&buf, 16, 0),
            (local(11) as u16, 16),
            "left block's last vertex"
        );
        assert_eq!(
            texel(&buf, 17, 0),
            (local(22) as u16, 32),
            "right block's first vertex"
        );
    }

    /// The full 6x6 grid covers every texel of the 102x102 map, so no texel is left at the
    /// zero fill (which would render as local index 0 rather than the authored tile).
    #[test]
    fn full_region_leaves_no_unwritten_texels() {
        let blocks: Vec<TerrainBlock> = (0..REGION_BLOCKS_PER_SIDE as i32)
            .flat_map(|bz| {
                (0..REGION_BLOCKS_PER_SIDE as i32).map(move |bx| block(bx, bz, |_, _| (7, 16)))
            })
            .collect();
        let refs: Vec<(&TerrainBlock, f32, f32)> = blocks.iter().map(|b| (b, 0.0, 0.0)).collect();
        let used_tiles = used_tiles_for(&refs);
        let buf = pack_tile_map(&refs, &used_tiles);

        assert_eq!(
            used_tiles,
            vec![7],
            "only one distinct tile in this fixture"
        );
        assert_eq!(buf.len(), TILE_MAP_SIZE * TILE_MAP_SIZE * 4);
        for v in 0..TILE_MAP_SIZE {
            for u in 0..TILE_MAP_SIZE {
                assert_eq!(texel(&buf, u, v), (0, 16), "texel ({u},{v}) unwritten");
            }
        }
    }

    /// The cap-overflow case `TerrainBlockSplatMaterial::from` warns about and truncates
    /// `used_tiles` for: a raw tile id `pack_tile_map` can't find clamps to the last slot
    /// instead of baking a `tile_atlas` index the bind group was never sized for.
    #[test]
    fn tile_id_missing_from_used_tiles_clamps_to_the_last_slot() {
        let b = block(0, 0, |x, _| if x == 0 { (999, 5) } else { (7, 5) });
        // 999 deliberately absent (as a truncated-away id would be); 42 present but not used by
        // any vertex here, so a clamp to it (rather than to 7's real slot) is unambiguous.
        let used_tiles = [7u16, 42];
        let buf = pack_tile_map(&[(&b, 0.0, 0.0)], &used_tiles);

        assert_eq!(
            texel(&buf, 0, 0),
            (1, 5),
            "missing id clamps to the last slot"
        );
        assert_eq!(
            texel(&buf, 1, 0),
            (0, 5),
            "present id resolves to its own slot"
        );
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
