use bevy::animation::graph::{AnimationGraphHandle, AnimationNodeIndex};
use bevy::animation::{AnimatedBy, AnimationPlayer, AnimationTargetId};
use std::collections::HashMap;

use crate::assets::ban::{bone_target_id, JMXVBAN};
use crate::plugins::config::graphics::ObjectLodSettings;
use crate::plugins::config::ClientConfig;
use crate::plugins::map::objects::{SroBindPoses, SroMeshes};
use crate::plugins::map::terrain::{FOG_RANGE, REGION_SIZE, VISIBLE_RANGE};
use bevy::asset::{AssetPath, AssetServer, Assets};
use bevy::camera::visibility::VisibilityRange;
use bevy::ecs::hierarchy::{ChildOf, ChildSpawner};
use bevy::ecs::system::Command;
use bevy::log::trace;
use bevy::math::{Mat4, Vec2, Vec3};
use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};

use crate::assets::bmt::material::JMXVBMT;
use bevy::pbr::{Material, StandardMaterial};
use bevy::prelude::{
    warn, AlphaMode, AnimationClip, AnimationGraph, Component, Entity, Handle, Mesh, Mesh3d,
    MeshMaterial3d, Mut, Name, Transform, Visibility, World,
};

use crate::assets::bms::mesh::JMXVBMS;
use crate::assets::bmt::material::{
    material_label, rim_material_label, sheen_cutout_material_label, sheen_material_label,
    BmtMaterialDefaults,
};
use crate::assets::bmt::rim::SroRimMaterial;
use crate::assets::bmt::sheen::SroSheenMaterial;
use crate::assets::bsk::JMXVBSK;
use crate::assets::bsr::bsr::PrimitiveAnimationTypeData;
use crate::assets::bsr::resource::{
    EffectModOwner, ParticleModEntry, SoundModEntry, SroResource, ANIM_TYPE_STAND,
};
use crate::plugins::texani::UvScrollSpeed;
use bevy::light::NotShadowCaster;

pub mod attach;
use crate::plugins::nav::ObjectNavMesh;

#[derive(Component)]
pub struct SilkroadEntity;

#[derive(Component)]
pub struct Bone;

/// Skeleton lookup data of a spawned resource, used to attach further
/// resources (e.g. equipped items) to its bones.
#[derive(Component)]
pub struct SkeletonBinding {
    /// bone name -> spawned bone entity
    pub bones: HashMap<String, Entity>,
    /// bone name -> inverse origin (bind pose) matrix
    pub bind_poses: HashMap<String, Mat4>,
}

/// The resource a wrapper entity was spawned from.
#[derive(Component)]
pub struct SpawnedFromResource(pub Handle<SroResource>);

/// Whether this resource's meshes were built with reversed winding because it
/// was placed under a mirroring (negative-determinant) transform. Recorded on
/// the wrapper so attached items (weapons, clothes) can reverse their meshes
/// consistently with the character they hang on — their own local transforms
/// are pure rotations, so they inherit the character's winding sign.
#[derive(Component, Clone, Copy)]
pub struct ReversedWinding(pub bool);

/// Mesh list index -> spawned mesh entity of a resource, used to hide
/// default body part meshes replaced by attached items (the attachment
/// slots reference meshes by their index in the resource's mesh list).
#[derive(Component)]
pub struct MeshIndexMap(pub HashMap<u32, Entity>);

/// The Transform/Name wrapper entity `spawn_mesh_groups` stamps out per mesh
/// group. Pure marker so entity-count diagnostics can attribute these.
#[derive(Component)]
pub struct MeshGroup;

/// One prepared mesh part: mesh index within its group, the mesh + material, the
/// bone names for skinning (empty if unskinned), the bind poses, and the
/// per-part LOD cull range (see `mesh_visibility_range`).
type MeshPartBundle<M> = (
    u32,
    Mesh3d,
    MeshMaterial3d<M>,
    Vec<String>,
    Option<Handle<SkinnedMeshInverseBindposes>>,
    VisibilityRange,
);

// Object-part distance LOD. Every object mesh part stops drawing past a distance
// scaled to its own size, so small props (rocks, clutter, fences) drop out of
// the draw set well before the fog boundary while large structures stay visible
// right up to it. `cull_fogged_objects` still hides whole objects past the fog
// range; this trims the cheaper-to-lose parts inside it, cutting the mesh-
// instance count the GPU-preprocessing/instance-buffer systems pay for.
//
// Tuning: `OBJECT_LOD_FACTOR` maps an object's largest dimension to its cull
// distance (the "constant screen size" heuristic — a part of size S is only a
// few pixels past ~S·factor). `OBJECT_LOD_MIN_DIST` is a floor so nothing near
// the camera is ever culled; lower it (or the factor) to cull more aggressively.
// The ceiling is the fully-fogged distance, so a part's range never outlives
// what the fog cull already hides.
// The factor, floor and fade band are config-exposed (`graphics.objects`,
// [`ObjectLodSettings`]) because the right values are hardware-dependent — the
// original client has no per-part LOD to match. The ceiling is not: it is the
// fully-fogged distance, so a part's range can never outlive what the fog cull
// already hides.
const OBJECT_LOD_MAX_DIST: f32 = (VISIBLE_RANGE + FOG_RANGE) as f32 * REGION_SIZE;

/// Per-part cull range from its `.bms` bounding box: fully visible up to a
/// size-scaled distance, then dithered out over `OBJECT_LOD_FADE` (materials
/// without the crossfade path just hard-cut at the far edge). `use_aabb` is
/// false so the distance is measured to the part's origin — which equals the
/// object's world anchor, since every level between carries a default transform,
/// matching how `cull_fogged_objects`/`cull_distant_animations` measure.
fn mesh_visibility_range(bounding_box: (Vec3, Vec3), lod: &ObjectLodSettings) -> VisibilityRange {
    let extent = (bounding_box.1 - bounding_box.0).max_element().max(0.0);
    let cull = (extent * lod.factor).clamp(
        lod.min_distance.min(OBJECT_LOD_MAX_DIST),
        OBJECT_LOD_MAX_DIST,
    );
    VisibilityRange {
        start_margin: 0.0..0.0,
        end_margin: (cull - lod.fade).max(0.0)..cull,
        use_aabb: false,
    }
}

/// Build the sub-asset path structurally because SRO material names can contain
/// `#`, which Bevy otherwise treats as another label separator when parsing a string.
pub(crate) fn material_asset_path(
    material_set_path: &AssetPath<'_>,
    material_label: String,
) -> AssetPath<'static> {
    material_set_path.clone_owned().with_label(material_label)
}

/// All animations of a spawned resource across all its groups, one graph
/// node per clip, so UIs can switch the played animation on the wrapper's
/// [`AnimationPlayer`] and `sync_animation_effects` can map playing nodes
/// back to their (group, type) key.
#[derive(Component)]
pub struct AnimationLibrary {
    pub entries: Vec<AnimationLibraryEntry>,
}

pub struct AnimationLibraryEntry {
    /// Animation group ("default", weapon class, "avatar_*", ...).
    pub group: String,
    /// Animation type id within the group (0 = stand, 4 = die, ...).
    pub anim_type: u32,
    /// Display label: "{group}/{type id} {ban stem}".
    pub label: String,
    pub node: AnimationNodeIndex,
    /// **Every** `PrimAnimationEvent` of this animation, sorted by keytime —
    /// not just the hit events (#276). See [`AnimEvent`].
    pub events: Vec<AnimEvent>,
}

/// One `PrimAnimationEvent` on the playback bus: at `key_time` ms into this
/// animation, fire event `typ` with params `p1`/`p2`
/// (`{i32 time, i32 type, i32 p1, i32 p2}`, JMX-File-Editor
/// `PrimAnimationEvent.cs:17-20`; parsed by `assets::bsr`).
///
/// Only [`ANIM_EVENT_HIT`] is *consumed* today. The other types this build's
/// corpus contains are carried here deliberately unread, because their
/// meaning is genuinely UNKNOWN — no public source names the enum
/// (`docs/re/formats/primanimevent.md` §3/§9). Corpus histogram over 7,714
/// `.bsr`: type 1 x23,214, type 2 x741 (locomotion entries, `p1`/`p2`
/// alternating (1,0)/(0,1) — *shaped* like footstep L/R, unverified), type 4
/// x48 (skill entries, stepping `p1` — staged sub-triggers, unverified),
/// type 0 x2 (both in the corrupt `tt.bsr`). Surfacing them costs four `u32`
/// per event and is what lets the audio and skill-staging lanes verify the
/// hypotheses against live playback instead of re-deriving the parse.
// `p1`/`p2` are read by no non-test code yet — that is the whole point of
// surfacing them (#276); their meaning is UNKNOWN until a lane verifies it.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AnimEvent {
    /// Milliseconds into the clip. The `AnimationClip` timeline
    /// `to_animation_clip` builds is the same value divided by 1000.
    pub key_time: u32,
    pub typ: u32,
    pub p1: u32,
    pub p2: u32,
}

/// The one event type openroad consumes: the hit/impact frame of an attack or
/// skill animation. `skilleffect.txt` `StartEvent N` fires at the Nth of them.
pub const ANIM_EVENT_HIT: u32 = 1;

impl AnimationLibraryEntry {
    /// Combat-hit keytimes (ms), sorted ascending — the [`ANIM_EVENT_HIT`]
    /// subset of [`Self::events`], which is what every consumer used before
    /// the other types were surfaced.
    pub fn hit_events(&self) -> Vec<u32> {
        self.events
            .iter()
            .filter(|event| event.typ == ANIM_EVENT_HIT)
            .map(|event| event.key_time)
            .collect()
    }

    /// Every event of one type, in keytime order. Nothing calls this for
    /// types 2 and 4 yet — that is the point of #276: the data is on the bus
    /// so the footstep and skill-staging lanes can read it without touching
    /// the parse or the graph build.
    #[allow(dead_code)]
    pub fn events_of_type(&self, typ: u32) -> impl Iterator<Item = &AnimEvent> {
        self.events.iter().filter(move |event| event.typ == typ)
    }
}

/// Copies an animation entry's whole event list onto the playback bus, sorted
/// by keytime.
///
/// Sorting here (rather than at each consumer) is what keeps `hit_events()`
/// ascending: the `.bsr` stores events in authoring order, and the hit-timing
/// consumers index into the list positionally (`StartEvent N`).
pub fn animation_events(anim: &PrimitiveAnimationTypeData) -> Vec<AnimEvent> {
    let mut events: Vec<AnimEvent> = anim
        .events
        .iter()
        .map(|event| AnimEvent {
            key_time: event.key_time,
            typ: event.typ,
            p1: event.index,
            p2: event.unknown,
        })
        .collect();
    events.sort_by_key(|event| event.key_time);
    events
}

/// Animation-gated resource effects (Particle ModData owned by typ-1 mod
/// sets), keyed by (animation group, animation type). Spawned/despawned by
/// `sync_animation_effects` while the matching animation plays, `delay_ms`
/// after it starts (e.g. isyutaru's death smoke 2379ms into "default"/4).
#[derive(Component)]
pub struct AnimationEffects(pub HashMap<(String, u32), Vec<ParticleModEntry>>);

/// Animation-linked sound tracks (Sound ModData owned by typ-1 mod sets),
/// keyed by (animation group, animation type) exactly like
/// [`AnimationEffects`]. Fired by `play_animation_sounds` at each track's
/// keytime into the playing animation (e.g. the tombstone's death thud
/// 347ms into "default"/4).
#[derive(Component)]
pub struct AnimationSounds(pub HashMap<(String, u32), Vec<SoundModEntry>>);

/// Which of the resource's material sets to apply. Mob `.bsr` files list the
/// base `.bmt` plus optional recolors: `*_champ.bmt` for champions (picked by
/// the spawn rarity byte) and `*_clone.bmt` for summon variants (e.g.
/// `waterghost.bsr` lists `waterghost.bmt`, `waterghost_clone.bmt` and
/// `waterghost_champ.bmt` — the "water ghost slave" uses the `_clone` set on
/// the shared model). Also a component: put it on the entity carrying
/// `UnloadedResource` and the loader threads it through.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MaterialVariant {
    #[default]
    Base,
    Champion,
    /// Summon clone / variant recolor (`*_clone.bmt`).
    Clone,
}

impl MaterialVariant {
    /// The `.bmt` filename suffix this variant selects, or `None` for the
    /// base set (which is neither a champ nor a clone recolor).
    fn suffix(self) -> Option<&'static str> {
        match self {
            MaterialVariant::Base => None,
            MaterialVariant::Champion => Some("_champ.bmt"),
            MaterialVariant::Clone => Some("_clone.bmt"),
        }
    }
}

pub struct SpawnResource {
    pub resource: Handle<SroResource>,
    pub transform: Transform,
    pub parent: Option<Entity>,
    /// Animation group (weapon class, e.g. "sword") to pick the played
    /// animation from, falling back to the resource's "default" group.
    pub animation_group: Option<String>,
    /// Reverse triangle winding for meshes placed under a mirroring
    /// (negative-determinant) transform. Every SRO resource — characters,
    /// world/map objects, equipment — is placed under such a mirror, so this
    /// is derived from the placement transform's determinant via the single
    /// shared rule (`util::mesh::needs_winding_reversal`). See `JMXVBMS::to_mesh`.
    pub reverse_winding: bool,
    /// Material set to apply (champion mobs use the `_champ` recolor).
    pub material_variant: MaterialVariant,
}

impl Command for SpawnResource {
    type Out = ();

    fn apply(self, world: &mut World) {
        // Take the mesh/bind-pose dedup caches out of the world for the
        // duration of the spawn, so the body can keep borrowing assets from
        // the world while inserting into them. The two asset collections the
        // caches manage are scoped out too: cache misses must insert via
        // `Assets::add` (immediately visible to `get_strong_handle`), not the
        // queued `asset_server.add`, or a same-frame second spawn of the same
        // model would miss the cache and break dedup/batching.
        world.init_resource::<SroMeshes>();
        world.init_resource::<SroBindPoses>();
        world.resource_scope(|world, mesh_cache: Mut<SroMeshes>| {
            world.resource_scope(|world, bind_pose_cache: Mut<SroBindPoses>| {
                world.resource_scope(|world, meshes: Mut<Assets<Mesh>>| {
                    world.resource_scope(
                        |world, inverse_bindposes: Mut<Assets<SkinnedMeshInverseBindposes>>| {
                            self.apply_with_caches(
                                world,
                                mesh_cache.into_inner(),
                                bind_pose_cache.into_inner(),
                                meshes.into_inner(),
                                inverse_bindposes.into_inner(),
                            );
                        },
                    );
                });
            });
        });
    }
}

impl SpawnResource {
    /// Body of [`Command::apply`], with the dedup caches already scoped out
    /// of the world. Also called by `AttachResource`, which holds the same
    /// scopes (nesting `resource_scope` on the same resource would panic).
    pub(crate) fn apply_with_caches(
        self,
        world: &mut World,
        mesh_cache: &mut SroMeshes,
        bind_pose_cache: &mut SroBindPoses,
        meshes: &mut Assets<Mesh>,
        inverse_bindposes: &mut Assets<SkinnedMeshInverseBindposes>,
    ) {
        // The owning entity (the item/monster carrying `UnloadedResource`) can
        // despawn while its resource loads asynchronously — common with the
        // larger PK2s' faster spawn/despawn churn. Bail before building the
        // mesh hierarchy rather than parenting it to a dead entity (panic).
        if self
            .parent
            .is_some_and(|parent| world.get_entity(parent).is_err())
        {
            return;
        }

        let entity = world
            .spawn((
                self.transform,
                Visibility::Inherited,
                // SilkroadEntity
            ))
            .id();

        let Some(resource) = world
            .resource::<Assets<SroResource>>()
            .get(self.resource.id())
        else {
            warn!(
                "failed to get sro resource for: {:?}",
                &self.resource.path()
            );
            return;
        };

        let bms_assets = world.resource::<Assets<JMXVBMS>>();
        let bsk_assets = world.resource::<Assets<JMXVBSK>>();
        let ban_assets = world.resource::<Assets<JMXVBAN>>();

        // Pick the material set by variant: champions use `*_champ.bmt` and
        // clones `*_clone.bmt` when the resource ships one; the base variant
        // uses the plain set (neither recolor). Anything unmatched (e.g. a
        // clone-less model asked for the clone set) falls back to the first
        // (base) entry.
        let set_name = |handle: &&Handle<JMXVBMT>| -> Option<String> {
            handle
                .path()
                .map(|p| p.path().to_string_lossy().to_ascii_lowercase())
        };
        let wanted_suffix = self.material_variant.suffix();
        let material_handle = resource
            .materials
            .iter()
            .find(|handle| match (wanted_suffix, set_name(handle)) {
                (Some(suffix), Some(name)) => name.ends_with(suffix),
                (None, Some(name)) => {
                    !name.ends_with("_champ.bmt") && !name.ends_with("_clone.bmt")
                }
                _ => false,
            })
            .or_else(|| resource.materials.first());
        let Some(material_set_path) = material_handle.and_then(|handle| handle.path()) else {
            // A resource with no material set at all is data, not a defect: 69
            // of the corpus' 120 `Res/etc/*.bsr` are 221-byte stubs (seasonal
            // props stripped from this build), and each one is instanced many
            // times, so warning per instance buries the log in hundreds of
            // lines. Only a resource that HAS a material set yet cannot name
            // one is anomalous.
            if resource.materials.is_empty() {
                bevy::log::debug!("no material set in {:?}", &self.resource.path());
            } else {
                warn!(
                    "failed to get material set path for: {:?}",
                    &self.resource.path()
                );
            }
            return;
        };

        // mesh-list index -> (UV scroll speed, blend override) for TexAni
        // resources (waterfalls, canal water); resolved against the applied
        // material set, which is loaded by now (spawn_resources_when_loaded
        // gates on is_loaded_with_dependencies)
        let uv_scroll_by_mesh: HashMap<u32, (Vec2, Option<AlphaMode>)> =
            if resource.texani_mods.is_empty() {
                HashMap::new()
            } else {
                resolve_texani_targets(
                    resource,
                    material_handle
                        .and_then(|handle| world.resource::<Assets<JMXVBMT>>().get(handle)),
                    bms_assets,
                )
            };

        let asset_server = world.resource::<AssetServer>();
        let (bind_poses, bones, animations) =
            match Self::prepare_skeleton(resource, bsk_assets, ban_assets, asset_server) {
                Some(value) => value,
                None => return,
            };

        let name = resource.object_info.name.clone();

        // Meshes carrying a walkable (object-level) nav mesh; exposed on the
        // resource root so NavMeshRaycast can intersect them (see plugins/nav).
        let nav_mesh_handles: Vec<Handle<JMXVBMS>> = resource
            .mesh
            .iter()
            .filter(|handle| {
                bms_assets
                    .get(*handle)
                    .is_some_and(|bms| bms.navmesh.is_some())
            })
            .cloned()
            .collect();

        let has_skeleton = resource.skeleton.is_some();
        let effect_mods = resource.effect_mods.clone();
        let sound_mods = resource.sound_mods.clone();
        // always-on rim: enabled by config AND a character-class resource
        // (map props, ground item drops etc. keep the plain material; worn
        // equipment attaches through AttachResource, which rims explicitly)
        let rim = world
            .get_resource::<BmtMaterialDefaults>()
            .is_some_and(|defaults| defaults.rim.is_some())
            && is_character_class_path(self.resource.path());
        // Per-part LOD tuning (`graphics.objects`). Read here rather than
        // threaded from a system because this is a `Command`: the spawn runs
        // with exclusive world access, the same way `rim` above resolves.
        let lod = world
            .get_resource::<ClientConfig>()
            .map(|config| config.graphics.objects.clone())
            .unwrap_or_default();
        let mesh_groups = PreparedMeshGroups::prepare(
            asset_server,
            resource,
            bms_assets,
            material_set_path,
            &bind_poses,
            has_skeleton,
            self.reverse_winding,
            resource.skeleton.as_ref(),
            mesh_cache,
            bind_pose_cache,
            meshes,
            inverse_bindposes,
            &lod,
            rim,
        );

        // every clip of every animation group goes into the graph (exposed
        // via AnimationLibrary); resources without animation groups (map
        // objects, ...) fall back to the first animation as before
        let ban_stem = |idx: usize| {
            resource
                .animation
                .animations
                .get(idx)
                .and_then(|handle| handle.path())
                .and_then(|path| path.path().file_stem())
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_else(|| format!("animation {idx}"))
        };
        let mut anim_graph = AnimationGraph::new();
        let mut clip_nodes: HashMap<usize, AnimationNodeIndex> = HashMap::new();
        let mut library = Vec::new();
        for group in &resource.primitive_animation_group {
            for anim in &group.animations {
                let idx = anim.file_index as usize;
                let Some(clip) = (anim.file_index != u32::MAX)
                    .then(|| animations.get(idx).cloned().flatten())
                    .flatten()
                else {
                    continue;
                };
                let root = anim_graph.root;
                let node = *clip_nodes
                    .entry(idx)
                    .or_insert_with(|| anim_graph.add_clip(clip, 1.0, root));
                library.push(AnimationLibraryEntry {
                    group: group.group_name.clone(),
                    anim_type: anim.typ,
                    label: format!("{}/{:02} {}", group.group_name, anim.typ, ban_stem(idx)),
                    node,
                    events: animation_events(anim),
                });
            }
        }
        if library.is_empty() {
            if let Some(clip) = animations.iter().flatten().next().cloned() {
                let node = anim_graph.add_clip(clip, 1.0, anim_graph.root);
                library.push(AnimationLibraryEntry {
                    group: "default".to_string(),
                    anim_type: ANIM_TYPE_STAND,
                    label: "default".to_string(),
                    node,
                    events: Vec::new(),
                });
            }
        }
        trace!(
            "animation library for {} (group {:?}): {} entries",
            name,
            self.animation_group,
            library.len()
        );

        let mut bone_entities = HashMap::new();
        let mut mesh_entities = HashMap::new();
        world
            .entity_mut(entity)
            .with_children(|resource_entity| {
                Self::spawn_bone_entities(bones, resource_entity, &name, &mut bone_entities);
                mesh_entities = mesh_groups.spawn(resource_entity, &bone_entities);
            })
            .insert((
                Name::from(name.clone()),
                SpawnedFromResource(self.resource.clone()),
                ReversedWinding(self.reverse_winding),
            ));

        // TexAni meshes get their material swapped for the scrolling
        // variant by apply_uv_scroll_materials once it is loaded; the
        // static masked cutout in the shadow prepass would be wrong, and
        // waterfalls shouldn't cast shadows anyway
        for (mesh_idx, (speed, alpha_mode)) in &uv_scroll_by_mesh {
            if let Some(&mesh_entity) = mesh_entities.get(mesh_idx) {
                world.entity_mut(mesh_entity).insert((
                    UvScrollSpeed {
                        uv_speed: *speed,
                        alpha_mode: *alpha_mode,
                    },
                    NotShadowCaster,
                ));
            }
        }

        world.entity_mut(entity).insert(MeshIndexMap(mesh_entities));

        if !nav_mesh_handles.is_empty() {
            world
                .entity_mut(entity)
                .insert(ObjectNavMesh(nav_mesh_handles));
        }

        if has_skeleton {
            world.entity_mut(entity).insert(SkeletonBinding {
                bones: bone_entities,
                bind_poses,
            });
        }

        if !library.is_empty() {
            // stand of the preferred group, falling back like the original
            // engine to the "default" group, plays by default
            let preferred = self.animation_group.as_deref();
            let stand_of = |group: Option<&str>| {
                library
                    .iter()
                    .find(|e| Some(e.group.as_str()) == group && e.anim_type == ANIM_TYPE_STAND)
            };
            let initial = stand_of(preferred)
                .or_else(|| stand_of(Some("default")))
                .unwrap_or(&library[0])
                .node;
            let mut anim_player = AnimationPlayer::default();
            anim_player.play(initial).repeat();

            // effects gated on animations, spawned while their animation plays
            let mut animation_effects: HashMap<(String, u32), Vec<ParticleModEntry>> =
                HashMap::new();
            for entry in &effect_mods {
                if let EffectModOwner::Animation { group, anim_type } = &entry.owner {
                    animation_effects
                        .entry((group.clone(), *anim_type))
                        .or_default()
                        .push(entry.clone());
                }
            }

            let graph = world
                .resource_mut::<Assets<AnimationGraph>>()
                .add(anim_graph);
            world.entity_mut(entity).insert((
                anim_player,
                AnimationGraphHandle(graph),
                AnimatedBy(entity),
                AnimationTargetId::from_names(vec![Name::from(name.clone())].iter()),
                AnimationLibrary { entries: library },
            ));
            if !animation_effects.is_empty() {
                world.entity_mut(entity).insert((
                    AnimationEffects(animation_effects),
                    crate::plugins::effects::ActiveAnimationEffects::default(),
                ));
            }

            // sounds gated on animations, fired at their keytime
            let mut animation_sounds: HashMap<(String, u32), Vec<SoundModEntry>> = HashMap::new();
            for entry in &sound_mods {
                animation_sounds
                    .entry((entry.group.clone(), entry.anim_type))
                    .or_default()
                    .push(entry.clone());
            }
            if !animation_sounds.is_empty() {
                world.entity_mut(entity).insert((
                    AnimationSounds(animation_sounds),
                    crate::plugins::animation_sounds::AnimationSoundCursor::default(),
                ));
            }
        }

        // Always-on particle effects from the mod palette (system/"ambient"
        // sets, e.g. torch flames). Animation-gated entries are handled by
        // sync_animation_effects via AnimationEffects above; External
        // (skill/state) entries are never auto-played. Loaded here (not in
        // the .bsr loader) so dangling references in game data cannot wedge
        // loading states; instantiate_effects despawns wrappers whose load
        // fails.
        let effect_spawns: Vec<_> = {
            let asset_server = world.resource::<AssetServer>();
            effect_mods
                .into_iter()
                .filter(|entry| entry.owner == EffectModOwner::AlwaysOn)
                .map(|entry| {
                    let handle = asset_server.load(format!("particles://{}", entry.path));
                    (handle, entry)
                })
                .collect()
        };
        for (handle, entry) in effect_spawns {
            // bone-anchored entries (garment talisman ward glows) follow
            // their bone; offset-anchored ones sit fixed on the wrapper
            let anchor = match &entry.bone {
                Some(bone) => world
                    .entity(entity)
                    .get::<SkeletonBinding>()
                    .and_then(|binding| binding.bones.get(bone).copied())
                    .unwrap_or_else(|| {
                        warn!(
                            "effect {}: bone {bone:?} not found in {name}, anchoring to wrapper",
                            entry.path
                        );
                        entity
                    }),
                None => entity,
            };
            bevy::log::debug!(
                "effect mod on {name}: {} bone={:?} offset={:?} scale={}",
                entry.path,
                entry.bone,
                entry.offset,
                entry.scale
            );
            let spawned = world
                .spawn((
                    crate::plugins::effects::EffectInstance { handle },
                    crate::plugins::effects::EffectPendingInit,
                    // The ModData scale applies to the effect geometry only
                    // (exe: wrapper+0xc0 → SetScale); the attach offset is
                    // already in resource-local units and must stay unscaled.
                    // Verified against world data (2026-07-30): cj_lamp01's
                    // flame is authored at y=16.32 on a 19.95-unit lamp and
                    // cj_weap_chimn's smoke at y=57.85 on the chimney top —
                    // multiplying by the ubiquitous Float0=0.5 put the flame
                    // mid-post and the smoke inside the roof.
                    Transform::from_translation(entry.offset)
                        .with_scale(bevy::math::Vec3::splat(entry.scale)),
                    Visibility::Inherited,
                    Name::from(format!("effect: {}", entry.path)),
                    ChildOf(anchor),
                ))
                .id();
            if entry.night_only {
                world
                    .entity_mut(spawned)
                    .insert(crate::plugins::effects::NightOnlyEffect);
            }
        }

        if let Some(parent) = self.parent {
            world.entity_mut(parent).add_child(entity);
        }
    }
}

impl SpawnResource {
    /// The returned animation clips keep the indices of the resource's
    /// animation list (`None` for .ban files that failed to load) so the
    /// animation group file indices stay valid.
    fn prepare_skeleton(
        resource: &SroResource,
        bsk_assets: &Assets<JMXVBSK>,
        ban_assets: &Assets<JMXVBAN>,
        asset_server: &AssetServer,
    ) -> Option<(
        HashMap<String, Mat4>,
        Vec<(Transform, String, String)>,
        Vec<Option<Handle<AnimationClip>>>,
    )> {
        let mut bind_poses = HashMap::new();
        let mut bones = Vec::new();
        let mut animations = Vec::with_capacity(resource.animation.animations.len());
        if let Some(skeleton_handle) = &resource.skeleton {
            if let Some(skeleton) = bsk_assets.get(skeleton_handle) {
                // bone lookup for walking the parent chain
                let by_name = skeleton
                    .bones
                    .iter()
                    .map(|b| (b.name.as_str(), b))
                    .collect::<HashMap<_, _>>();
                let parent_names = skeleton.parent_map();
                for i in 0..skeleton.bones.len() {
                    let bone = &skeleton.bones[i];
                    let parent_mat = bone.get_parent_matrix();

                    let bone_entity = (
                        Transform::from_matrix(parent_mat),
                        bone.name.clone(),
                        bone.parent_bone_name.clone(),
                    );

                    // The bind pose is the bone's rest transform accumulated
                    // through the parent chain (matching how the bone entities
                    // are nested), NOT its object-space `origin`. For most
                    // bones these are equal, but some (e.g. EU `Spine_Base`,
                    // whose origin is left at identity/world-origin) only get a
                    // correct rest transform from the chain - using `origin`
                    // there flings the skinned vertices toward the feet.
                    let mut bind = parent_mat;
                    let mut cursor = bone;
                    // Composed edges (parent fields + child lists) so
                    // multi-root skeletons keep their `[root]` transform;
                    // bounded so a corrupt cycle cannot spin here (#280).
                    for _ in 0..skeleton.bones.len() {
                        let Some(parent) = parent_names
                            .get(cursor.name.as_str())
                            .and_then(|n| by_name.get(*n))
                        else {
                            break;
                        };
                        bind = parent.get_parent_matrix() * bind;
                        cursor = parent;
                    }

                    bind_poses.insert(bone.name.clone(), bind.inverse());
                    bones.push(bone_entity);
                }

                for animation in &resource.animation.animations {
                    animations.push(ban_assets.get(animation).map(|animation| {
                        let clip =
                            animation.to_animation_clip(skeleton, &resource.object_info.name);
                        asset_server.add(clip)
                    }));
                }
            } else {
                warn!("Skeleton not loaded");
                return None;
            }
        }
        Some((bind_poses, bones, animations))
    }

    /// `use_skinning` controls whether meshes get skinned to the bones in
    /// `bind_poses`. Those are usually the resource's own skeleton, but for
    /// attached clothes they belong to the character the item is worn by.
    ///
    /// Generic over the material asset type because sheen resources use the
    /// extended [`SroSheenMaterial`]; `material_label` maps a mesh's material
    /// name to the labeled sub-asset of that type (see [`PreparedMeshGroups`]).
    ///
    /// Meshes and bind poses are shared through `mesh_cache`/`bind_pose_cache`
    /// so repeated spawns of the same model reuse one asset (and Bevy can
    /// batch the draws) instead of registering an identical copy per instance.
    /// `skeleton` keys the bind-pose cache: the skeleton `bind_poses` came
    /// from — the resource's own, or the wearing character's for clothes.
    fn prepare_mesh_groups<M: Material>(
        asset_server: &AssetServer,
        resource: &SroResource,
        bms_assets: &Assets<JMXVBMS>,
        material_set_path: &AssetPath,
        bind_poses: &HashMap<String, Mat4>,
        use_skinning: bool,
        reverse_winding: bool,
        skeleton: Option<&Handle<JMXVBSK>>,
        mesh_cache: &mut SroMeshes,
        bind_pose_cache: &mut SroBindPoses,
        meshes: &mut Assets<Mesh>,
        inverse_bindposes: &mut Assets<SkinnedMeshInverseBindposes>,
        lod: &ObjectLodSettings,
        material_label: impl Fn(&str) -> String,
    ) -> Vec<(Name, Vec<MeshPartBundle<M>>)> {
        // Skill-object resources (the skilleffect.txt arrows —
        // cha_arrow_normal/_critical/_fire/…) list their meshes but carry
        // ZERO primitive groups (corpus-verified 2026-08-03; the equipment
        // arrows cha_arrow/cha_arrow_01 DO have a `default` group). The exe
        // renders the raw mesh list regardless, so synthesize a group over
        // all meshes instead of spawning a bare skeleton.
        let fallback_group;
        let groups: &[crate::assets::bsr::bsr::PrimitiveGroupData] =
            if resource.primitive_group.is_empty() && !resource.mesh.is_empty() {
                fallback_group = [crate::assets::bsr::bsr::PrimitiveGroupData {
                    name: "default".to_string(),
                    files_indices: (0..resource.mesh.len() as u32).collect(),
                }];
                &fallback_group
            } else {
                &resource.primitive_group
            };
        let mut group_bundles = Vec::with_capacity(groups.len());
        for group in groups.iter() {
            let group_name = group.name.clone();
            let group_name = Name::from(group_name);
            let mut bundles = Vec::with_capacity(group.files_indices.len());
            for i in &group.files_indices {
                let bms_handle = &resource.mesh[*i as usize];
                let Some(mesh) = bms_assets.get(bms_handle) else {
                    warn!("could not find mesh: {:?}", bms_handle);
                    continue;
                };

                // One authoritative flag for this part: it decides the bone list
                // (=> `SkinnedMesh` insertion in `spawn_mesh_groups`) AND whether
                // the built mesh carries joint attributes. The two must agree —
                // see `JMXVBMS::to_mesh` — so both derive from it.
                let skinned = use_skinning && mesh.has_skinning_data();
                let bone_names = if skinned {
                    mesh.bone_data
                        .as_ref()
                        .expect("has_skinning_data")
                        .bones
                        .clone()
                } else {
                    Vec::new()
                };

                let material: Handle<M> = asset_server.load(material_asset_path(
                    material_set_path,
                    material_label(&mesh.material),
                ));
                // Weak cache: resolve the cached id to a fresh strong handle;
                // a dead id (last using entity despawned, asset freed) falls
                // through to a rebuild. See `SroMeshes`.
                let mesh_key = (bms_handle.id(), reverse_winding, skinned);
                let cached_mesh = mesh_cache
                    .0
                    .get(&mesh_key)
                    .and_then(|id| meshes.get_strong_handle(*id));
                let mesh_handle = cached_mesh.unwrap_or_else(|| {
                    let handle = meshes.add(mesh.to_mesh(reverse_winding, skinned));
                    mesh_cache.0.insert(mesh_key, handle.id());
                    handle
                });

                // Keep one bind pose per mesh bone, aligned with `bone_names`
                // (and thus with the vertex joint indices). Some meshes list a
                // bone that the target skeleton lacks but never actually weight
                // any vertex to it (e.g. a phantom `Bone03` on EU heavy leg
                // armor); dropping it here would shift every following index
                // and corrupt skinning, so substitute identity and keep going.
                let build_bind_poses = || {
                    let bind_poses = bone_names.iter().map(|b| {
                        bind_poses.get(b).copied().unwrap_or_else(|| {
                            warn!("no bind pose for bone {} (likely unused by this mesh); using identity", b);
                            Mat4::IDENTITY
                        })
                    }).collect::<Vec<_>>();
                    SkinnedMeshInverseBindposes::from(bind_poses)
                };
                let bind_poses_handle = if bone_names.is_empty() {
                    // unskinned meshes never read bind poses; don't register one
                    None
                } else {
                    Some(match skeleton {
                        // weak cache, same resolve-or-rebuild as the mesh above
                        Some(skeleton) => {
                            let key = (bms_handle.id(), skeleton.id());
                            let cached = bind_pose_cache
                                .0
                                .get(&key)
                                .and_then(|id| inverse_bindposes.get_strong_handle(*id));
                            cached.unwrap_or_else(|| {
                                let handle = inverse_bindposes.add(build_bind_poses());
                                bind_pose_cache.0.insert(key, handle.id());
                                handle
                            })
                        }
                        // no skeleton handle to key the cache with; build uncached
                        None => inverse_bindposes.add(build_bind_poses()),
                    })
                };

                bundles.push((
                    *i,
                    Mesh3d(mesh_handle),
                    MeshMaterial3d(material),
                    bone_names,
                    bind_poses_handle,
                    mesh_visibility_range(mesh.bounding_box, lod),
                ));
            }
            group_bundles.push((group_name, bundles));
        }
        group_bundles
    }

    fn spawn_bone_entities(
        bones: Vec<(Transform, String, String)>,
        resource_entity: &mut ChildSpawner,
        root_name: &String,
        bone_entities: &mut HashMap<String, Entity>,
    ) {
        let e = resource_entity.target_entity();
        let parent_bones = bones
            .iter()
            .filter(|(_, _, parent_name)| !parent_name.is_empty())
            .map(|(_, name, parent_name)| (name.clone(), parent_name.clone()))
            .collect::<HashMap<_, _>>();
        for bone in bones {
            trace!("adding root name: {root_name}");
            let anim_target_id = bone_target_id(&bone.1, &parent_bones, root_name);
            let name = bone.1.clone();
            // Bones carry `Visibility` so the visibility chain stays intact for
            // content parented to them (equip effects, gear meshes). Without it,
            // a bone is a parent lacking `InheritedVisibility` and Bevy warns
            // B0004 with inconsistent visibility for the attached effect.
            let mut b = resource_entity.spawn((
                bone.0,
                Name::from(name),
                Bone,
                AnimatedBy(e),
                anim_target_id,
                Visibility::Inherited,
            ));
            bone_entities.insert(bone.1, b.id());
            if let Some(parent) = bone_entities.get(&bone.2) {
                b.insert(ChildOf(*parent));
            }
        }
    }

    fn spawn_mesh_groups<M: Material>(
        mesh_groups: Vec<(Name, Vec<MeshPartBundle<M>>)>,
        resource_entity: &mut ChildSpawner,
        bone_entities: &HashMap<String, Entity>,
    ) -> HashMap<u32, Entity> {
        let mut mesh_entities = HashMap::new();
        for (name, children) in mesh_groups {
            resource_entity.spawn((Transform::default(), Visibility::default(), name, MeshGroup)).with_children(|child| {
                for (mesh_idx, mesh_3d, mesh_material_3d, bones, inverse_bindposes, visibility_range) in children {
                    if bones.is_empty() {
                        let e = child.spawn((mesh_3d, mesh_material_3d, Transform::default(), Visibility::default(), visibility_range)).id();
                        mesh_entities.insert(mesh_idx, e);
                    } else {
                        // The joint list must stay index-aligned with the
                        // vertices, so keep one entry per mesh bone. A bone the
                        // target skeleton lacks but no vertex weights to (e.g.
                        // the phantom `Bone03` on EU heavy leg armor) would
                        // otherwise shorten the list and either drop the whole
                        // mesh (invisible legs) or misalign skinning. Map such
                        // bones to the group entity; since no vertex references
                        // them, they have no visible effect.
                        let fallback = child.target_entity();
                        let joints = bones.iter().map(|bone| {
                            bone_entities.get(bone).copied().unwrap_or_else(|| {
                                warn!("unresolved bone {} (likely unused by this mesh); mapping to fallback", bone);
                                fallback
                            })
                        }).collect::<Vec<_>>();

                        // prepare_mesh_groups always builds bind poses when
                        // the bone list is non-empty
                        let Some(inverse_bindposes) = inverse_bindposes else {
                            warn!("skinned mesh {} has no bind poses; skipping", mesh_idx);
                            continue;
                        };
                        let e = child.spawn((
                            mesh_3d,
                            mesh_material_3d,
                            Transform::default(),
                            Visibility::default(),
                            visibility_range,
                            SkinnedMesh {
                                joints,
                                inverse_bindposes
                            }
                        )).id();
                        mesh_entities.insert(mesh_idx, e);
                    }
                }
            });
        }
        mesh_entities
    }
}

/// Resolves a resource's TexAni entries to mesh-list indices: an entry's
/// material index (`None` = all) selects material names in the applied
/// .bmt set, and every mesh referencing one of those names (through the
/// primitive groups) scrolls at the entry's speed. Entries apply in order,
/// so a -1 catch-all is overridden by later specific indices. The blend
/// overrides of the resource's Material mods resolve by the same rules
/// and ride along per mesh (waterfall sheets are alpha-blended/additive,
/// not alpha-masked — see [`crate::assets::bsr::resource::BlendModEntry`]).
fn resolve_texani_targets(
    resource: &SroResource,
    bmt: Option<&JMXVBMT>,
    bms_assets: &Assets<JMXVBMS>,
) -> HashMap<u32, (Vec2, Option<AlphaMode>)> {
    let Some(bmt) = bmt else {
        warn!(
            "TexAni resource {}: material set not loaded, spawning static",
            resource.object_info.name
        );
        return HashMap::new();
    };

    // applies one (mtrl_idx -> value) entry onto a label map
    fn apply_entry<T: Copy>(
        by_label: &mut HashMap<String, T>,
        bmt: &JMXVBMT,
        resource_name: &str,
        mtrl_idx: Option<u32>,
        value: T,
    ) {
        match mtrl_idx {
            None => {
                for material in &bmt.materials {
                    by_label.insert(material_label(&material.name), value);
                }
            }
            Some(idx) => match bmt.materials.get(idx as usize) {
                Some(material) => {
                    by_label.insert(material_label(&material.name), value);
                }
                None => warn!(
                    "TexAni resource {resource_name}: material index {idx} out of range ({} materials)",
                    bmt.materials.len()
                ),
            },
        }
    }

    let name = &resource.object_info.name;
    let mut speed_by_label: HashMap<String, Vec2> = HashMap::new();
    for entry in &resource.texani_mods {
        apply_entry(
            &mut speed_by_label,
            bmt,
            name,
            entry.mtrl_idx,
            entry.uv_speed,
        );
    }
    let mut blend_by_label: HashMap<String, AlphaMode> = HashMap::new();
    for entry in &resource.blend_mods {
        apply_entry(
            &mut blend_by_label,
            bmt,
            name,
            entry.mtrl_idx,
            entry.alpha_mode,
        );
    }

    let mut by_mesh = HashMap::new();
    for group in &resource.primitive_group {
        for i in &group.files_indices {
            let Some(mesh) = resource
                .mesh
                .get(*i as usize)
                .and_then(|handle| bms_assets.get(handle))
            else {
                continue;
            };
            let label = material_label(&mesh.material);
            if let Some(speed) = speed_by_label.get(&label) {
                by_mesh.insert(*i, (*speed, blend_by_label.get(&label).copied()));
            }
        }
    }
    by_mesh
}

/// Whether a resource path is character-class — bodies, hair, mobs, NPCs,
/// COS/pets — i.e. the set the always-on rim (`graphics.rim`) applies to.
/// Map props (`res/bldg|nature|artifact|dun`) and standalone ground item
/// drops (`res/item` spawned without a wearer) stay plain; worn equipment
/// goes through `AttachResource`, which rims explicitly.
fn is_character_class_path(path: Option<&AssetPath>) -> bool {
    path.is_some_and(|path| {
        let path = path.path().to_string_lossy().to_ascii_lowercase();
        ["res/char", "res/mob", "res/npc", "res/cos", "res/pet"]
            .iter()
            .any(|prefix| path.starts_with(prefix))
    })
}

/// The mesh bundles of a resource, prepared with the material type its
/// alpha semantics demand: sheen resources (weapons, metal armor — texture
/// alpha is an env-map mask, see [`SroResource::alpha_is_sheen`]) use the
/// extended [`SroSheenMaterial`] whose fragment shader turns that alpha
/// into per-texel metallic; character-class resources use the
/// [`SroRimMaterial`] `.rim` variant when the always-on rim is enabled
/// (`graphics.rim`, see [`BmtMaterialDefaults`]); everything else uses the
/// plain alpha-masked [`StandardMaterial`]. The enum bridges the
/// monomorphizations between the prepare and spawn steps, which run at
/// different points of [`SpawnResource::apply`].
enum PreparedMeshGroups {
    Standard(Vec<(Name, Vec<MeshPartBundle<StandardMaterial>>)>),
    Rim(Vec<(Name, Vec<MeshPartBundle<SroRimMaterial>>)>),
    Sheen(Vec<(Name, Vec<MeshPartBundle<SroSheenMaterial>>)>),
}

impl PreparedMeshGroups {
    #[allow(clippy::too_many_arguments)]
    fn prepare(
        asset_server: &AssetServer,
        resource: &SroResource,
        bms_assets: &Assets<JMXVBMS>,
        material_set_path: &AssetPath,
        bind_poses: &HashMap<String, Mat4>,
        use_skinning: bool,
        reverse_winding: bool,
        skeleton: Option<&Handle<JMXVBSK>>,
        mesh_cache: &mut SroMeshes,
        bind_pose_cache: &mut SroBindPoses,
        meshes: &mut Assets<Mesh>,
        inverse_bindposes: &mut Assets<SkinnedMeshInverseBindposes>,
        lod: &ObjectLodSettings,
        // pick the `.rim` sub-asset (always-on character rim) over the plain
        // Masked material; only valid when the loader built it (rim enabled)
        rim: bool,
    ) -> Self {
        if resource.alpha_is_sheen {
            // resources with the EnvMap alpha-test flag additionally cut
            // out exact-zero alpha texels (see SroResource::sheen_alpha_test)
            let label: fn(&str) -> String = if resource.sheen_alpha_test {
                sheen_cutout_material_label
            } else {
                sheen_material_label
            };
            Self::Sheen(SpawnResource::prepare_mesh_groups(
                asset_server,
                resource,
                bms_assets,
                material_set_path,
                bind_poses,
                use_skinning,
                reverse_winding,
                skeleton,
                mesh_cache,
                bind_pose_cache,
                meshes,
                inverse_bindposes,
                lod,
                |material_name| label(material_name),
            ))
        } else if rim {
            Self::Rim(SpawnResource::prepare_mesh_groups(
                asset_server,
                resource,
                bms_assets,
                material_set_path,
                bind_poses,
                use_skinning,
                reverse_winding,
                skeleton,
                mesh_cache,
                bind_pose_cache,
                meshes,
                inverse_bindposes,
                lod,
                rim_material_label,
            ))
        } else {
            Self::Standard(SpawnResource::prepare_mesh_groups(
                asset_server,
                resource,
                bms_assets,
                material_set_path,
                bind_poses,
                use_skinning,
                reverse_winding,
                skeleton,
                mesh_cache,
                bind_pose_cache,
                meshes,
                inverse_bindposes,
                lod,
                material_label,
            ))
        }
    }

    fn spawn(
        self,
        resource_entity: &mut ChildSpawner,
        bone_entities: &HashMap<String, Entity>,
    ) -> HashMap<u32, Entity> {
        match self {
            Self::Standard(groups) => {
                SpawnResource::spawn_mesh_groups(groups, resource_entity, bone_entities)
            }
            Self::Rim(groups) => {
                SpawnResource::spawn_mesh_groups(groups, resource_entity, bone_entities)
            }
            Self::Sheen(groups) => {
                SpawnResource::spawn_mesh_groups(groups, resource_entity, bone_entities)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::assets::bsr::bsr::PrimitiveAnimationEvent;

    #[test]
    fn material_asset_path_preserves_hash_in_label() {
        let material_set_path =
            AssetPath::parse("data://prim/mtrl/bldg/china/jin_imperial/enter/c_jin_enter01.bmt");

        let path = material_asset_path(&material_set_path, "Material #427".to_owned());

        assert_eq!(
            path.path(),
            Path::new("prim/mtrl/bldg/china/jin_imperial/enter/c_jin_enter01.bmt")
        );
        assert_eq!(path.get_extension(), Some("bmt"));
        assert_eq!(path.label(), Some("Material #427"));
    }

    /// #276: the parse always saw all four event types, but only `typ == 1`
    /// ever left the loader — the 741 type-2 and 48 type-4 events of this
    /// build's corpus were dropped at the library boundary. Rows are shaped
    /// after the corpus samples in `docs/re/formats/primanimevent.md` §3:
    /// `char_cpa.bsr` RUN carries typ-2 at 291 (0,1) and 623 (1,0),
    /// `chinaman_spidey.bsr` typ-26 carries typ-4 with stepping p1.
    #[test]
    fn every_animation_event_type_reaches_the_library_not_just_the_hit_frame() {
        let anim = PrimitiveAnimationTypeData {
            typ: 7,
            file_index: 3,
            events: vec![
                // authoring order is NOT keytime order
                PrimitiveAnimationEvent {
                    key_time: 623,
                    typ: 2,
                    index: 1,
                    unknown: 0,
                },
                PrimitiveAnimationEvent {
                    key_time: 291,
                    typ: 2,
                    index: 0,
                    unknown: 1,
                },
                PrimitiveAnimationEvent {
                    key_time: 801,
                    typ: 1,
                    index: 0,
                    unknown: 0,
                },
                PrimitiveAnimationEvent {
                    key_time: 352,
                    typ: 1,
                    index: 0,
                    unknown: 0,
                },
                PrimitiveAnimationEvent {
                    key_time: 900,
                    typ: 4,
                    index: 2,
                    unknown: 1,
                },
            ],
            walk_length: 0.0,
            walk_points: Vec::new(),
        };

        let entry = AnimationLibraryEntry {
            group: "default".to_string(),
            anim_type: anim.typ,
            label: "default/07".to_string(),
            node: AnimationNodeIndex::new(1),
            events: animation_events(&anim),
        };

        // every type survives, sorted by keytime
        assert_eq!(
            entry
                .events
                .iter()
                .map(|e| (e.key_time, e.typ))
                .collect::<Vec<_>>(),
            vec![(291, 2), (352, 1), (623, 2), (801, 1), (900, 4)]
        );

        // the consumed subset is unchanged: typ-1 keytimes, ascending
        assert_eq!(entry.hit_events(), vec![352, 801]);

        // type 2 keeps its (p1, p2) pair — the alternating (1,0)/(0,1) that
        // the footstep-L/R hypothesis rests on. UNVERIFIED and unconsumed:
        // this asserts the bytes arrive, not what they mean.
        assert_eq!(
            entry
                .events_of_type(2)
                .map(|e| (e.key_time, e.p1, e.p2))
                .collect::<Vec<_>>(),
            vec![(291, 0, 1), (623, 1, 0)]
        );
        // type 4 likewise: stepping p1, meaning UNKNOWN
        assert_eq!(
            entry.events_of_type(4).map(|e| e.p1).collect::<Vec<_>>(),
            vec![2]
        );
        // nothing invents a type that is not in the record
        assert_eq!(entry.events_of_type(3).count(), 0);
    }
}
