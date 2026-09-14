use std::collections::HashMap;
use std::time::Duration;

use bevy::animation::graph::AnimationNodeIndex;
use bevy::animation::AnimationPlayer;
use bevy::asset::{AssetId, AssetServer, Assets, Handle};
use bevy::ecs::hierarchy::ChildOf;
use bevy::ecs::system::SystemParam;
use bevy::image::Image;
use bevy::light::NotShadowCaster;
use bevy::mesh::{Mesh, Mesh3d, MeshTag};
use bevy::pbr::MeshMaterial3d;
use bevy::prelude::{
    Commands, Component, Entity, FromWorld, Has, Name, Query, Rectangle, Res, ResMut, Resource,
    Time, Timer, TimerMode, Transform, Visibility, With, World,
};

use crate::commands::{AnimationEffects, AnimationLibrary, SkeletonBinding};

use crate::assets::bms::mesh::JMXVBMS;
use crate::assets::efp::format::{
    EeResource, EfController, EfStaticEmit, EfStoredObject, EffectCommand, RenderShape, ViewMode,
};
use crate::assets::efp::{normalize_effect_path, JMXVEFF};
use crate::plugins::effects::components::*;
use crate::plugins::effects::material::SroEffectMaterial;
use crate::plugins::effects::trail::{empty_trail_mesh, EffectTrail, TRAIL_MAX_AGE};

/// Shared 1x1 quad in the XY plane used by every RenderPlate node.
#[derive(Resource)]
pub struct EffectQuad(pub Handle<Mesh>);

impl FromWorld for EffectQuad {
    fn from_world(world: &mut World) -> Self {
        let mut meshes = world.resource_mut::<Assets<Mesh>>();
        Self(meshes.add(Rectangle::new(1.0, 1.0)))
    }
}

/// Converted RenderMesh meshes, cached per source .bms id (effect meshes
/// are never skinned or winding-reversed, so the id alone is the key).
///
/// Entries are weak `AssetId`s: the effect entities are the only strong
/// owners, so a mesh frees once the last instance despawns instead of being
/// pinned for the session. Lookups resolve via `Assets::get_strong_handle`
/// and rebuild on a dead id; `prune_effect_caches` sweeps dead entries.
#[derive(Resource, Default)]
pub struct EffectMeshes(pub HashMap<AssetId<JMXVBMS>, AssetId<Mesh>>);

/// Shared immutable materials, cached per (texture, src_blend, dst_blend).
/// Nodes animate their tint via `MeshTag`, so every node with the same
/// texture and blend state can render with one material (and batch);
/// TextureSlide nodes bypass the cache with a private instance.
/// Weak `AssetId` entries, same lifetime scheme as [`EffectMeshes`] — the
/// materials transitively pin their textures, so releasing them matters.
/// Key: (texture, src_blend, dst_blend, color_op, alpha_op) — the color/alpha
/// texture-stage ops select the material's brightness multiplier (MODULATE2X
/// etc.), so nodes sharing a texture+blend but a different op must not share a
/// material.
#[derive(Resource, Default)]
pub struct EffectMaterials(
    pub HashMap<(Option<AssetId<Image>>, u32, u32, u32, u32), AssetId<SroEffectMaterial>>,
);

/// Periodic sweep of the effect caches' dead entries (see [`EffectMeshes`]);
/// without it the maps themselves would grow for the whole session.
pub fn prune_effect_caches(
    meshes: Res<Assets<Mesh>>,
    materials: Res<Assets<SroEffectMaterial>>,
    mut mesh_cache: ResMut<EffectMeshes>,
    mut material_cache: ResMut<EffectMaterials>,
) {
    mesh_cache.0.retain(|_, id| meshes.contains(*id));
    material_cache.0.retain(|_, id| materials.contains(*id));
}

/// Everything the node-tree instantiation needs, bundled so the spawn path
/// can be called from multiple systems. `effects` is separate from the
/// mutable `assets` so a borrowed `&JMXVEFF` can outlive `&mut assets`.
#[derive(SystemParam)]
pub struct EffectSpawnParams<'w> {
    pub effects: Res<'w, Assets<JMXVEFF>>,
    pub assets: EffectSpawnAssets<'w>,
}

#[derive(SystemParam)]
pub struct EffectSpawnAssets<'w> {
    pub materials: ResMut<'w, Assets<SroEffectMaterial>>,
    pub meshes: ResMut<'w, Assets<Mesh>>,
    pub bms_assets: Res<'w, Assets<JMXVBMS>>,
    pub mesh_cache: ResMut<'w, EffectMeshes>,
    pub material_cache: ResMut<'w, EffectMaterials>,
    pub quad: Res<'w, EffectQuad>,
    pub additive_intensity: Res<'w, crate::plugins::effects::EffectAdditiveIntensity>,
    pub ldr_additive: Res<'w, crate::plugins::effects::EffectLdrAdditive>,
}

pub trait EffectCommandsExt {
    /// Spawn an effect wrapper at `transform`, optionally under `parent`.
    /// The node tree is instantiated once the asset finished loading.
    fn spawn_effect(
        &mut self,
        handle: Handle<JMXVEFF>,
        transform: Transform,
        parent: Option<Entity>,
    ) -> Entity;

    /// Spawn an effect as a child of `target` (bone, weapon wrapper, ...).
    fn attach_effect(
        &mut self,
        handle: Handle<JMXVEFF>,
        target: Entity,
        transform: Transform,
    ) -> Entity;
}

/// Tracks which animation's gated effects are currently live on a wrapper
/// with [`AnimationEffects`](crate::commands::AnimationEffects).
#[derive(Component, Default)]
pub struct ActiveAnimationEffects {
    node: Option<AnimationNodeIndex>,
    spawned: Vec<Entity>,
}

/// An animation-gated effect waiting out its keytime before the node tree
/// is instantiated (e.g. death smoke 2379ms into the die animation).
#[derive(Component)]
pub struct DelayedEffect {
    pub handle: Handle<JMXVEFF>,
    pub timer: Timer,
}

/// Spawns/despawns animation-gated resource effects when the played
/// animation changes: the wrapper's playing node is mapped back to its
/// (group, type) via the AnimationLibrary and the matching AnimationEffects
/// entries are spawned as delayed children; effects of the previously
/// playing animation are removed.
pub fn sync_animation_effects(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut wrappers: Query<(
        Entity,
        &AnimationPlayer,
        &AnimationLibrary,
        &AnimationEffects,
        &mut ActiveAnimationEffects,
        Option<&SkeletonBinding>,
    )>,
) {
    for (entity, player, library, effects, mut active, skeleton) in &mut wrappers {
        let playing = library
            .entries
            .iter()
            .find(|entry| player.is_playing_animation(entry.node));
        if playing.map(|entry| entry.node) == active.node {
            continue;
        }
        for spawned in active.spawned.drain(..) {
            if let Ok(mut spawned) = commands.get_entity(spawned) {
                spawned.despawn();
            }
        }
        active.node = playing.map(|entry| entry.node);
        let Some(entry) = playing else { continue };
        let Some(mods) = effects.0.get(&(entry.group.clone(), entry.anim_type)) else {
            continue;
        };
        for entry_mod in mods {
            let handle = asset_server.load(format!("particles://{}", entry_mod.path));
            // bone-anchored entries follow their bone (e.g. transformed
            // characters' hand/spine auras); the rest sit on the wrapper
            let anchor = entry_mod
                .bone
                .as_ref()
                .and_then(|bone| skeleton.and_then(|s| s.bones.get(bone).copied()))
                .unwrap_or(entity);
            let spawned = commands
                .spawn((
                    DelayedEffect {
                        handle,
                        timer: Timer::new(
                            Duration::from_millis(entry_mod.delay_ms as u64),
                            TimerMode::Once,
                        ),
                    },
                    // ModData scale covers the effect geometry only — the
                    // offset is resource-local and unscaled, like the
                    // always-on path in `commands::mod`.
                    Transform::from_translation(entry_mod.offset)
                        .with_scale(bevy::math::Vec3::splat(entry_mod.scale)),
                    Visibility::Inherited,
                    Name::new(format!("anim effect: {}", entry_mod.path)),
                    ChildOf(anchor),
                ))
                .id();
            active.spawned.push(spawned);
        }
    }
}

/// Turns [`DelayedEffect`] wrappers into live effect instances once their
/// keytime has elapsed.
pub fn start_delayed_effects(
    mut commands: Commands,
    time: Res<Time>,
    mut pending: Query<(Entity, &mut DelayedEffect)>,
) {
    for (entity, mut delayed) in &mut pending {
        if delayed.timer.tick(time.delta()).just_finished() {
            let handle = delayed.handle.clone();
            commands
                .entity(entity)
                .remove::<DelayedEffect>()
                .insert((EffectInstance { handle }, EffectPendingInit));
        }
    }
}

impl EffectCommandsExt for Commands<'_, '_> {
    fn spawn_effect(
        &mut self,
        handle: Handle<JMXVEFF>,
        transform: Transform,
        parent: Option<Entity>,
    ) -> Entity {
        let mut entity = self.spawn((
            EffectInstance { handle },
            EffectPendingInit,
            transform,
            Visibility::default(),
            Name::new("effect"),
        ));
        if let Some(parent) = parent {
            entity.insert(ChildOf(parent));
        }
        entity.id()
    }

    fn attach_effect(
        &mut self,
        handle: Handle<JMXVEFF>,
        target: Entity,
        transform: Transform,
    ) -> Entity {
        self.spawn_effect(handle, transform, Some(target))
    }
}

/// Resolved emission parameters, exe-RE confirmed (worker 0xc84e70):
/// `ints = [start_frame, window_frames, period_frames, max_alive]` and
/// `spawn_rate` = particles added per emission tick (fractional part
/// accumulates). Emission runs on frames `start..start+window` whenever
/// `(frame - start) % period == 0`. See docs/formats/efp-jmxveff.md
/// § EFStaticEmit.
#[derive(Clone, Copy)]
pub struct EmitParams {
    pub start_frame: u32,
    pub window: u32,
    pub period: u32,
    pub max_alive: u32,
    /// Particles per emission tick; 0 is authored (a disabled emitter).
    pub rate: f32,
}

impl From<&EfStaticEmit> for EmitParams {
    fn from(emit: &EfStaticEmit) -> Self {
        Self {
            start_frame: emit.ints[0],
            window: emit.ints[1].max(1),
            period: emit.ints[2].max(1),
            max_alive: emit.ints[3].max(1),
            rate: emit.spawn_rate.max(0.0),
        }
    }
}

pub fn emit_params(node: &EfStoredObject) -> Option<EmitParams> {
    if let Some(emit) = node.static_emit() {
        return Some(emit.into());
    }
    node.emitters
        .iter()
        .find_map(|source| match &source.command {
            EffectCommand::StaticEmit(emit) => Some(emit.into()),
            _ => None,
        })
}

/// Render shape/resource of a node, falling back to its Shape controller.
pub fn effective_shape(node: &EfStoredObject) -> (RenderShape, &EeResource) {
    if node.render_shape != RenderShape::None {
        return (node.render_shape, &node.resource);
    }
    node.controllers
        .iter()
        .find_map(|c| match c {
            EfController::Shape { shape, resource } => Some((*shape, resource)),
            _ => None,
        })
        .unwrap_or((RenderShape::None, &node.resource))
}

/// The node's program length in effect frames as authored by its baked
/// per-frame rows: the Frame* graph arrays (scale/diffuse/UV cells/BAN
/// keys) carry one entry per effect frame (exe RE: the interpreter selects
/// `programs[frame]`), so the longest array IS the node's cycle length.
/// The emission window counts too — an emitter lives at least as long as
/// it emits. 0 when the node has no baked rows at all.
pub fn program_frames(node: &EfStoredObject) -> u32 {
    use EffectCommand as C;
    let mut frames = emit_params(node)
        .map(|e| e.start_frame + e.window)
        .unwrap_or(0);
    let sources = node.programs.iter().chain(node.decorations.iter()).chain(
        node.controllers.iter().flat_map(|c| match c {
            EfController::Program(list) => list.as_slice(),
            _ => &[],
        }),
    );
    for source in sources {
        let rows = match &source.command {
            C::SetGraphScale(keys) => keys.len(),
            C::SetGraphDiffuse(keys) => keys.len(),
            C::TextureSlide(slide) => slide.frames.len(),
            C::SetBanPos(keys) => keys.len(),
            C::SetBanRot(keys) => keys.len(),
            _ => 0,
        };
        frames = frames.max(rows as u32);
    }
    frames
}

/// The node's program length in effect frames — the authored `program_len`
/// (exe: node+0x7c; instance lifetime, loop modulus, and percent-schedule
/// base). The fallback for a 0 length (rare/defensive) is the longest baked
/// row array, floored at the default particle life.
pub fn node_program_len(node: &EfStoredObject) -> u32 {
    if node.program_len > 0 {
        return node.program_len;
    }
    program_frames(node).max((DEFAULT_PARTICLE_LIFE * EFFECT_FPS) as u32)
}

/// One authored playthrough of a whole effect, in seconds: the longest node
/// program cycle across the tree divided by the effect frame rate. Callers
/// that want an effect to play *once* (one-shot skill casts) despawn it
/// after this — our runtime otherwise loops every node forever.
pub fn effect_oneshot_secs(effect: &crate::assets::efp::JMXVEFF) -> f32 {
    let frames = effect
        .effect
        .nodes
        .iter()
        .map(node_program_len)
        .max()
        .unwrap_or((EFFECT_FPS) as u32);
    (frames as f32 / EFFECT_FPS).clamp(0.3, 4.0)
}

/// Lifespan from the node's authored program length + lifetime command
/// (exe: NormalTimeLoop wraps age modulo the length, NormalTimeExtinct
/// kills at it, NeverExtinct — also the default — lives on):
/// - authored loops keep their state across wraps;
/// - emitted leaf particles die at the length (pooled, refunding emission
///   credit — the emitter's steady stream re-fills);
/// - emitted sub-emitters and static extinct nodes loop-respawn at the
///   length instead: the original replays the whole effect composition
///   when it ends, and the per-node replay is our approximation of that —
///   a Once here would leave ambient cascades dark, and WhileRootLives let
///   force-driven nodes integrate velocity forever. EXCEPT under a
///   [`OneShotEffect`] wrapper (`one_shot`): a skill burst must play once,
///   so those nodes get `Once` and despawn at their program end instead of
///   replaying inside the wrapper's (tree-max) lifetime.
fn lifespan_of(
    node: &EfStoredObject,
    is_particle: bool,
    is_emitter: bool,
    one_shot: bool,
) -> Lifespan {
    let window = node_program_len(node) as f32 / EFFECT_FPS;

    let loops = node
        .controllers
        .iter()
        .any(|c| matches!(c, EfController::NormalTimeLoopLife))
        || node
            .lifetime
            .as_ref()
            .is_some_and(|l| matches!(l.command, EffectCommand::NormalTimeLoop));
    if loops {
        return Lifespan::Loop {
            period: window,
            respawn: false,
        };
    }
    let extinct = node
        .lifetime
        .as_ref()
        .is_some_and(|l| matches!(l.command, EffectCommand::NormalTimeExtinct));
    if !extinct {
        // NeverExtinct, explicit or the serialized default.
        return Lifespan::WhileRootLives;
    }
    if (is_particle && !is_emitter) || one_shot {
        Lifespan::Once(window)
    } else {
        Lifespan::Loop {
            period: window,
            respawn: true,
        }
    }
}

/// How long a trail node's ribbon samples live: the node's program length
/// (the trail exists exactly as long as the node animates), clamped to a
/// sane range.
fn trail_max_age(node: &EfStoredObject) -> f32 {
    if node.program_len == 0 {
        return TRAIL_MAX_AGE;
    }
    (node.program_len as f32 / EFFECT_FPS).clamp(0.05, 5.0)
}

fn resolve_render_mesh(
    assets: &mut EffectSpawnAssets,
    effect: &JMXVEFF,
    resource: &EeResource,
) -> Option<Handle<Mesh>> {
    let path = resource.mesh_paths().next()?;
    let bms_handle = effect.meshes.get(&normalize_effect_path(path))?;
    // Weak cache: a dead id (all effect instances gone, asset freed) falls
    // through to a rebuild. See `EffectMeshes`.
    let cached = assets.mesh_cache.0.get(&bms_handle.id()).copied();
    if let Some(mesh) = cached.and_then(|id| assets.meshes.get_strong_handle(id)) {
        return Some(mesh);
    }
    let bms = assets.bms_assets.get(bms_handle)?;
    // Never skinned: effect nodes spawn plain `Mesh3d` entities, so the mesh
    // must not carry joint attributes (see `JMXVBMS::to_mesh`).
    let mesh = assets.meshes.add(bms.to_mesh(false, false));
    assets.mesh_cache.0.insert(bms_handle.id(), mesh.id());
    Some(mesh)
}

/// Instantiate the node `node_idx` of `effect` (and, for non-emitters, its
/// static children) under `parent`. Returns the spawned entity.
#[allow(clippy::too_many_arguments)]
pub fn spawn_node_tree(
    commands: &mut Commands,
    assets: &mut EffectSpawnAssets,
    handle: &Handle<JMXVEFF>,
    effect: &JMXVEFF,
    node_idx: usize,
    parent: Entity,
    root: Entity,
    transform: Transform,
    emitted_by: Option<EmittedBy>,
    // When true this is a self-emitted leaf particle (a copy of a leaf emitter);
    // it must render but never re-emit, or it would recurse forever.
    particle_only: bool,
    // Whether leaf self-emission is enabled (see `LeafEmitPolicy`).
    allow_leaf_emit: bool,
    // Age the node starts at. Emitted particles are pre-aged to their
    // emitter's 50 ms effect-frame grid (the spawn command runs at render
    // rate, up to one effect frame after the emission tick it belongs to);
    // without the alignment a particle's death drifts off the tick its
    // replacement spawns on, and the slipped phase becomes a hole +
    // duplicate pair in the phase comb — a strong once-per-loop pulse in
    // effects that should twinkle steadily.
    initial_age: f32,
    // The wrapper carries `OneShotEffect` — nodes play their program once
    // instead of loop-respawning (see `lifespan_of`).
    one_shot: bool,
) -> Entity {
    let node = &effect.effect.nodes[node_idx];
    let is_particle = emitted_by.is_some();
    let mut cache = EffectProgramCache::build(node, effect);
    let uv_animated = cache.uv.is_some();

    // A node with a StaticEmit is an emitter. Group emitters (with children)
    // emit their children; a leaf emitter emits copies of *itself*, but only
    // when its effect opted into leaf self-emission. A self-emitted particle
    // (`particle_only`) never re-emits — it is a plain particle, NOT an
    // emitter, and dies at its program length like any other.
    let emit = (!particle_only)
        .then(|| {
            if node.children.is_empty() && !allow_leaf_emit {
                None // leaf self-emission disabled: render as a single plate
            } else {
                emit_params(node)
            }
        })
        .flatten();
    let is_emitter = emit.is_some();
    // A leaf emitter is only an emission ANCHOR: it must not render (its
    // emitted copies carry the visual) and, like the original where only
    // emitted instances run the node's program, it must not run the motion
    // commands either — the attached copies would inherit its drift on top
    // of their own.
    let self_emitter = is_emitter && node.children.is_empty();
    if self_emitter {
        cache.motion.clear();
    }

    let mut entity = commands.spawn((
        EffectNode {
            handle: handle.clone(),
            node: node_idx,
            age: initial_age,
            lifespan: lifespan_of(node, is_particle, is_emitter, one_shot),
            root,
        },
        EffectMotion {
            origin: transform.translation,
            origin_rotation: transform.rotation,
            ..Default::default()
        },
        cache,
        transform,
        Visibility::default(),
        Name::new(if node.name.is_empty() {
            "effect node".to_string()
        } else {
            node.name.clone()
        }),
        ChildOf(parent),
    ));
    if let Some(emitted_by) = emitted_by {
        entity.insert(emitted_by);
    }

    // Emission is evaluated per effect frame starting at frame 0, so
    // cascades fire their first particles immediately (hit effects are 3
    // levels deep and must not lag their animation keytime).
    if let Some(emit) = emit {
        let slots = node.children.len().max(1);
        entity.insert(EffectEmitter {
            frame: 0,
            params: emit,
            alive: vec![0; slots],
            acc: vec![0.0; slots],
            pool: Vec::new(),
        });
    }
    let entity = entity.id();

    let (shape, resource) = effective_shape(node);
    let texture = resource
        .texture_paths()
        .next()
        .and_then(|path| effect.textures.get(&normalize_effect_path(path)))
        .cloned();

    let render_mesh = match shape {
        RenderShape::Plate => Some(assets.quad.0.clone()),
        RenderShape::Mesh => resolve_render_mesh(assets, effect, resource),
        RenderShape::LinkPipe | RenderShape::LinkDPipe => {
            let mesh = assets.meshes.add(empty_trail_mesh());
            // The Aabb is rewritten on every ribbon rebuild, so trails
            // frustum-cull normally (they used to carry NoFrustumCulling
            // because the bounds went stale).
            commands.entity(entity).insert((
                EffectTrail::new(mesh.clone(), trail_max_age(node)),
                bevy::camera::primitives::Aabb::default(),
            ));
            Some(mesh)
        }
        // LinkObj positions its (already instantiated) children; None is a
        // pure emitter/group node.
        RenderShape::LinkObj | RenderShape::None => None,
    };

    if let Some(mesh) = render_mesh.filter(|_| !self_emitter) {
        // TextureSlide animates the uv uniform -> private material; every
        // other node shares one immutable material per (texture, blend).
        let build = |assets: &EffectSpawnAssets| {
            let mut material = SroEffectMaterial::from_resource(resource, texture.clone());
            if material.dst_blend == 2 {
                material.params.x = material.base_color_scale * assets.additive_intensity.0;
            }
            material.set_ldr_additive(assets.ldr_additive.0);
            material
        };
        let material = if uv_animated {
            let material = build(assets);
            assets.materials.add(material)
        } else {
            // weak cache, resolve-or-rebuild (see EffectMaterials)
            let key = (
                texture.as_ref().map(Handle::id),
                resource.src_blend,
                resource.dst_blend,
                resource.texture_stage[2], // color op
                resource.texture_stage[5], // alpha op
            );
            let cached = assets
                .material_cache
                .0
                .get(&key)
                .copied()
                .and_then(|id| assets.materials.get_strong_handle(id));
            cached.unwrap_or_else(|| {
                let material = build(assets);
                let handle = assets.materials.add(material);
                assets.material_cache.0.insert(key, handle.id());
                handle
            })
        };
        commands.entity(entity).insert((
            Mesh3d(mesh),
            MeshMaterial3d(material.clone()),
            MeshTag(0xFFFFFFFF),
            EffectVisual {
                uv_material: uv_animated.then_some(material),
                last_argb: 0xFFFFFFFF,
                last_uv: bevy::math::Vec4::new(0.0, 0.0, 1.0, 1.0),
            },
            NotShadowCaster,
        ));
        if shape == RenderShape::Plate && node.view_mode != ViewMode::None {
            commands
                .entity(entity)
                .insert(EffectBillboard(node.view_mode));
        }
    }

    if !is_emitter {
        for &child in &node.children {
            spawn_node_tree(
                commands,
                assets,
                handle,
                effect,
                child,
                entity,
                root,
                Transform::IDENTITY,
                None,
                false,
                allow_leaf_emit,
                initial_age,
                one_shot,
            );
        }
    }

    entity
}

/// Re-arms a pooled leaf particle in place of a fresh `spawn_node_tree`:
/// the same components are re-initialized (grid-aligned age, fresh
/// motion/parenting) while the entity, mesh, material, and `EffectVisual`
/// are reused as-is —
/// `sample_graphs` re-syncs color/UV from the new age on the next frame.
/// Returns false if the entity is gone, so the caller falls back to a spawn.
#[allow(clippy::too_many_arguments)]
pub fn reset_pooled_particle(
    commands: &mut Commands,
    handle: &Handle<JMXVEFF>,
    effect: &JMXVEFF,
    node_idx: usize,
    entity: Entity,
    parent: Entity,
    root: Entity,
    transform: Transform,
    initial_age: f32,
) -> bool {
    let Ok(mut ec) = commands.get_entity(entity) else {
        return false;
    };
    let node = &effect.effect.nodes[node_idx];
    ec.insert((
        EffectNode {
            handle: handle.clone(),
            node: node_idx,
            age: initial_age,
            // pooled re-arms are always emitted leaf particles — `Once`
            // either way, so the one-shot flag is moot here
            lifespan: lifespan_of(node, true, false, false),
            root,
        },
        EffectMotion {
            origin: transform.translation,
            origin_rotation: transform.rotation,
            ..Default::default()
        },
        // Strictly still valid (same handle + node index), but rebuilding
        // on re-arm is cheap and closes the hot-reload edge where the
        // template changed while the particle sat parked.
        EffectProgramCache::build(node, effect),
        transform,
        Visibility::default(),
        ChildOf(parent),
    ))
    .remove::<PooledParticle>();
    true
}

/// Instantiates pending effect wrappers once their asset (including
/// dependencies) finished loading.
pub fn instantiate_effects(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut params: EffectSpawnParams,
    pending: Query<(Entity, &EffectInstance, Has<OneShotEffect>), With<EffectPendingInit>>,
    leaf_policy: Res<crate::plugins::effects::LeafEmitPolicy>,
) {
    for (entity, instance, one_shot) in &pending {
        let allow_leaf_emit = leaf_policy.global;
        // Missing files (dangling references in game data) or parse failures
        // (the few odd-version .efp) must not leave pending wrappers around.
        if matches!(
            asset_server.get_load_state(&instance.handle),
            Some(bevy::asset::LoadState::Failed(_))
        ) || matches!(
            asset_server.get_recursive_dependency_load_state(&instance.handle),
            Some(bevy::asset::RecursiveDependencyLoadState::Failed(_))
        ) {
            bevy::log::warn!(
                "effect failed to load, dropping instance: {:?}",
                instance.handle.path()
            );
            commands.entity(entity).despawn();
            continue;
        }
        if !asset_server.is_loaded_with_dependencies(&instance.handle) {
            continue;
        }
        let Some(effect) = params.effects.get(&instance.handle) else {
            continue;
        };
        let root_transform = Transform::from_scale(bevy::math::Vec3::splat(
            effect.effect.root_scale.max(f32::EPSILON),
        ));
        spawn_node_tree(
            &mut commands,
            &mut params.assets,
            &instance.handle,
            effect,
            effect.effect.root,
            entity,
            entity,
            root_transform,
            None,
            false,
            allow_leaf_emit,
            0.0,
            one_shot,
        );
        commands.entity(entity).remove::<EffectPendingInit>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trail_age_follows_the_authored_program_length() {
        let mut node = EfStoredObject::default();
        node.program_len = 20; // 1s at EFFECT_FPS
        assert_eq!(trail_max_age(&node), 1.0);

        node.program_len = 0; // defensive fallback
        assert_eq!(trail_max_age(&node), TRAIL_MAX_AGE);

        node.program_len = 100_000; // absurd length clamps
        assert_eq!(trail_max_age(&node), 5.0);
    }
}
