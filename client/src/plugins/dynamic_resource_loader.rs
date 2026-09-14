use std::time::Duration;

use bevy::app::App;
use bevy::ecs::hierarchy::ChildOf;
use bevy::prelude::{
    trace, warn, AssetServer, Assets, Children, Commands, Component, Entity, Handle, Has, Image,
    MeshMaterial3d, Plugin, Query, Res, ResMut, Time, Timer, TimerMode, Transform, Update, With,
};

use packets::agent::character_data::ItemTypeData;

use crate::assets::bmt::sheen::{set_shine, ShineColor, SroSheenMaterial};
use crate::assets::bsk::JMXVBSK;
use crate::assets::bsr::resource::SroResource;
use crate::assets::resinfo::item_rare::RareEffect;
use crate::commands::{MaterialVariant, SkeletonBinding, SpawnedFromResource};
use crate::plugins::effects::EffectCommandsExt;
use crate::plugins::net::inventory::Inventory;
use crate::plugins::player::Player;
use crate::util::commands_ext::CommandsExt;

#[derive(Default)]
pub struct DynamicResourceLoaderPlugin;

#[derive(Component)]
pub struct UnloadedResource(pub Handle<SroResource>);

/// Marks a resource spawned under a mirroring (negative-determinant) transform —
/// every SRO resource (characters, world/map objects, ...) is, for the SRO -> Bevy
/// coordinate conversion (`scale.x = -1`). Its meshes need their triangle winding
/// reversed so backface culling keeps the front faces. Set from the placement
/// transform's determinant via `util::mesh::needs_winding_reversal`. See
/// `JMXVBMS::to_mesh`.
#[derive(Component)]
pub struct MirroredResource;

/// Animation group (weapon class, e.g. "sword") the spawned resource should
/// play its animations from; without it only the "default" group applies.
#[derive(Component)]
pub struct PreferredAnimationGroup(pub String);

/// An equipped item that still has to be attached to its owning character's
/// skeleton (waiting for both the character's and the item's resources to be
/// ready). Spawned as a `ChildOf` the character entity.
#[derive(Component)]
pub struct PendingItemAttachment(pub Handle<SroResource>);

/// Optional companion to [`PendingItemAttachment`]: how to pick the
/// alchemy-enhancement glow tier for a (skeleton'd) item once attached. Kept
/// separate so the many plain-attachment call sites are unaffected — only
/// enhanced-equipment spawners add it.
#[derive(Component, Clone, Copy, Debug)]
pub enum AttachmentShine {
    /// Tier known up front (spawn from `LobbyItem.plus`, char-select).
    Tier(ShineColor),
    /// Live equip: resolve from the local player's equipment slot once it has
    /// settled. The 0x3038 equip broadcast precedes the 0xB034 move that
    /// fills the slot, so the opt level isn't readable when the attachment is
    /// requested; `ref_id` guards against reading a stale/previous item.
    EquipSlot { slot: u8, ref_id: u32 },
}

/// Optional companion to [`PendingItemAttachment`] for rare ("Seal of …")
/// weapons/shields: the looping aura from `ItemRare.txt` — effect path, scale,
/// and the weapon bone (`ai_end` = blade tip, …) it hangs off. Kept separate
/// like [`AttachmentShine`] so plain attachments are unaffected.
#[derive(Component, Clone)]
pub struct AttachmentRareAura {
    pub auras: Vec<RareEffect>,
    /// For rows whose `min_opt` gate couldn't be evaluated at request time
    /// (a live 0x3038 equip precedes the 0xB034 move that fills the slot,
    /// same ordering problem [`AttachmentShine::EquipSlot`] solves): the
    /// equipment `(slot, ref_id)` to resolve the opt level from once the
    /// inventory settles. `None` = rows were pre-validated (spawn-time
    /// attach, test scenes).
    pub gate: Option<(u8, u32)>,
}

/// A queued rare aura waiting for its item's root to spawn under the character,
/// at which point [`apply_rare_aura`] attaches the looping effect to its bone.
#[derive(Component)]
pub struct PendingRareAura {
    /// The character body wrapper the item attaches under.
    pub wrapper: Entity,
    /// The item's resource handle — identifies its spawned root by
    /// [`SpawnedFromResource`].
    pub item: Handle<SroResource>,
    /// The `particles://…efp` aura path.
    effect: String,
    /// Attach scale from `ItemRare.txt`.
    scale: f32,
    /// The weapon bone to hang the effect on (`ai_end`, `Bone01`, …).
    bone: String,
    /// The row's minimum enhancement, with the [`AttachmentRareAura::gate`]
    /// to resolve it against; `(Some(min), Some(gate))` defers the attach
    /// until the slot settles at `opt_level >= min`.
    min_opt: Option<u8>,
    gate: Option<(u8, u32)>,
    /// Give-up bound if the item root never materializes.
    timeout: Timer,
}

/// A queued enhancement glow waiting for its weapon's meshes (and their
/// sheen materials) to finish spawning under the character, at which point
/// [`apply_weapon_shine`] runs `set_shine` scoped to that weapon.
#[derive(Component)]
pub struct PendingWeaponShine {
    /// The character body wrapper the weapon attaches under.
    pub wrapper: Entity,
    /// The weapon's resource handle — identifies its spawned root by
    /// [`SpawnedFromResource`].
    pub item: Handle<SroResource>,
    shine: AttachmentShine,
    /// Give-up bound for resolving the **tier**, i.e. for the equipment slot to
    /// settle. Only ticks while the tier is still unresolved.
    resolve_timeout: Timer,
    /// Give-up bound for the **meshes**: weapon sheen materials normally arrive
    /// within a few frames of the item root spawning; a resource that never
    /// yields any (e.g. a non-sheen item) stops here. Only ticks once the tier
    /// is resolved, so a slow inventory settle cannot spend this budget.
    ///
    /// Splitting the two matters: a single wall-clock budget covering both
    /// waits let a slow 0xB034 eat the whole allowance and drop the glow while
    /// the weapon was still streaming in — a live `+N` equip silently produced
    /// no shine at all, which is #14 as reported.
    mesh_timeout: Timer,
}

/// How long each of [`PendingWeaponShine`]'s two waits may take before the
/// request is abandoned (with a `warn!` naming which one expired).
const SHINE_STAGE_TIMEOUT: Duration = Duration::from_secs(5);

impl Plugin for DynamicResourceLoaderPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                spawn_resources_when_loaded,
                attach_pending_items,
                apply_weapon_shine,
                apply_rare_aura,
            ),
        );
    }
}

fn spawn_resources_when_loaded(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    sro_resource_assets: Res<Assets<SroResource>>,
    sro_skeleton_assets: Res<Assets<JMXVBSK>>,
    query: Query<(
        Entity,
        &UnloadedResource,
        Option<&PreferredAnimationGroup>,
        Option<&MaterialVariant>,
        Has<MirroredResource>,
    )>,
) {
    for (entity, res, anim_group, material_variant, mirrored) in query.iter() {
        if asset_server.is_loaded_with_dependencies(&res.0) {
            let resource = sro_resource_assets.get(&res.0).unwrap();
            if let Some(skeleton) = &resource.skeleton {
                if !asset_server.is_loaded_with_dependencies(skeleton)
                    || !sro_skeleton_assets.contains(skeleton)
                {
                    trace!("skeleton not loaded. skipping to spawn resource");
                    continue;
                }
            }
            commands.spawn_resource(
                res.0.clone(),
                Transform::default(),
                Some(entity),
                anim_group.map(|g| g.0.clone()),
                mirrored,
                material_variant.copied().unwrap_or_default(),
            );
            // try_remove: the entity may despawn before this applies (the
            // resource-load race handled in SpawnResource), and a plain
            // remove on a dead entity panics.
            commands.entity(entity).try_remove::<UnloadedResource>();
        }
    }
}

/// Attaches equipped items to their owning character once both the
/// character's skeleton entities and the item resource are ready.
#[allow(clippy::type_complexity)]
fn attach_pending_items(
    asset_server: Res<AssetServer>,
    sro_resource_assets: Res<Assets<SroResource>>,
    bsk_assets: Res<Assets<JMXVBSK>>,
    pending: Query<(
        Entity,
        &PendingItemAttachment,
        &ChildOf,
        Option<&AttachmentShine>,
        Option<&AttachmentRareAura>,
    )>,
    children_query: Query<&Children>,
    wrappers: Query<(), With<SkeletonBinding>>,
    mut commands: Commands,
) {
    for (pending_entity, pending_item, child_of, shine, rare_aura) in pending.iter() {
        // the wrapper (and its bones) only exists once the character's
        // resource has been spawned
        let Ok(siblings) = children_query.get(child_of.parent()) else {
            continue;
        };
        let Some(wrapper) = siblings.iter().find(|e| wrappers.get(**e).is_ok()) else {
            continue;
        };

        if !asset_server.is_loaded_with_dependencies(&pending_item.0) {
            continue;
        }
        let Some(resource) = sro_resource_assets.get(&pending_item.0) else {
            continue;
        };
        if let Some(skeleton) = &resource.skeleton {
            if bsk_assets.get(skeleton).is_none() {
                continue;
            }
        }

        commands.attach_resource(pending_item.0.clone(), *wrapper);
        // Enhanced weapons (skeleton'd items) get their glow scheduled: the
        // meshes spawn a frame or two later, so a deferred resolver applies
        // the shine once they exist. Non-sheen / skeleton-less items are
        // skipped (nothing to sweep).
        if let Some(shine) = shine {
            if resource.skeleton.is_some() {
                commands.spawn(PendingWeaponShine {
                    wrapper: *wrapper,
                    item: pending_item.0.clone(),
                    shine: *shine,
                    resolve_timeout: Timer::new(SHINE_STAGE_TIMEOUT, TimerMode::Once),
                    mesh_timeout: Timer::new(SHINE_STAGE_TIMEOUT, TimerMode::Once),
                });
            }
        }
        // Rare weapons/shields: schedule the looping aura the same deferred way
        // (the item's root spawns a frame or two after attach_resource).
        if let Some(aura) = rare_aura {
            for spec in &aura.auras {
                commands.spawn(PendingRareAura {
                    wrapper: *wrapper,
                    item: pending_item.0.clone(),
                    effect: spec.effect.clone(),
                    scale: spec.scale,
                    bone: spec.bone.clone(),
                    min_opt: spec.min_opt,
                    gate: aura.gate,
                    timeout: Timer::new(Duration::from_secs(5), TimerMode::Once),
                });
            }
        }
        // try_despawn: the pending item is a child of the character, which
        // may despawn (taking it) before this applies.
        commands.entity(pending_entity).try_despawn();
    }
}

/// Apply a queued [`PendingWeaponShine`] once its tier is resolvable and the
/// weapon's meshes/sheen materials have loaded: locate the weapon root under
/// the character by its [`SpawnedFromResource`] handle and run `set_shine` on
/// that subtree only (so the sweep is on the weapon, not the whole body).
#[allow(clippy::too_many_arguments)]
fn apply_weapon_shine(
    time: Res<Time>,
    asset_server: Res<AssetServer>,
    children: Query<&Children>,
    from_resource: Query<&SpawnedFromResource>,
    sheen_meshes: Query<&MeshMaterial3d<SroSheenMaterial>>,
    inventories: Query<&Inventory, With<Player>>,
    // Optional: the offline test scenes build this plugin without a config.
    config: Option<Res<crate::plugins::config::ClientConfig>>,
    mut materials: ResMut<Assets<SroSheenMaterial>>,
    mut pending: Query<(Entity, &mut PendingWeaponShine)>,
    mut commands: Commands,
) {
    let intensity = config
        .as_deref()
        .map_or(1.0, |c| c.graphics.sheen.intensity);
    for (marker, mut req) in pending.iter_mut() {
        // Resolve the tier. `None` = not yet resolvable (equip slot still
        // catching up), `Some(None)` = resolved to no glow (opt < 3),
        // `Some(Some(tier))` = apply this tier.
        let resolved: Option<Option<ShineColor>> = match req.shine {
            AttachmentShine::Tier(tier) => Some(Some(tier)),
            AttachmentShine::EquipSlot { slot, ref_id } => inventories
                .single()
                .ok()
                .and_then(|inv| inv.get(slot))
                .filter(|item| item.ref_id == ref_id)
                .map(|item| match &item.data {
                    ItemTypeData::Equipment(eq) => ShineColor::for_opt_level(eq.opt_level),
                    _ => None,
                }),
        };
        let tier = match resolved {
            Some(Some(tier)) => tier,
            // resolved to no glow — nothing to do
            Some(None) => {
                commands.entity(marker).despawn();
                continue;
            }
            // Not resolvable yet: the 0xB034 move that fills the equipment slot
            // has not landed. Only this budget ticks here, so a slow ack cannot
            // spend the mesh budget below.
            None => {
                if req.resolve_timeout.tick(time.delta()).is_finished() {
                    warn!(
                        "shine: gave up resolving the +N tier for {:?} after {:.0}s — the \
                         equipment slot never settled, so the weapon renders unenhanced",
                        req.shine,
                        SHINE_STAGE_TIMEOUT.as_secs_f32(),
                    );
                    commands.entity(marker).despawn();
                }
                continue;
            }
        };
        let timed_out = req.mesh_timeout.tick(time.delta()).is_finished();

        let root = children.iter_descendants(req.wrapper).find(|e| {
            from_resource
                .get(*e)
                .is_ok_and(|r| r.0.id() == req.item.id())
        });
        // (rare-aura resolver mirrors this root lookup; see `apply_rare_aura`)
        let updates = root.map(|root| {
            // non-color: the streak texture is an intensity map (see DdjSettings)
            let texture: Handle<Image> = asset_server
                .load_builder()
                .with_settings(|settings: &mut crate::assets::ddj::DdjSettings| {
                    settings.non_color = true;
                })
                .load(tier.texture_path());
            set_shine(
                root,
                tier.params().scaled(intensity),
                &texture,
                &children,
                &sheen_meshes,
                &mut materials,
            )
        });
        match updates {
            // materials loaded and cloned — apply and finish
            Some(updates) if !updates.is_empty() => {
                for (entity, material) in updates {
                    // try_insert: the weapon mesh can despawn with its owner
                    // before this applies.
                    commands.entity(entity).try_insert(MeshMaterial3d(material));
                }
                commands.entity(marker).despawn();
            }
            // Weapon not attached yet, or its sheen materials not loaded:
            // `set_shine` returns an empty list for both, and both are normal
            // for a frame or two. Past the bound, say which one it was — a
            // missing root means the item never spawned under the character,
            // an existing root with no updates means the resource yielded no
            // sheen materials at all (a non-sheen item, where dropping the
            // request is the correct outcome).
            _ if timed_out => {
                warn!(
                    "shine: gave up applying the {:?} glow after {:.0}s — {}",
                    tier,
                    SHINE_STAGE_TIMEOUT.as_secs_f32(),
                    if root.is_some() {
                        "the weapon has no sheen materials"
                    } else {
                        "the weapon never spawned under the character"
                    },
                );
                commands.entity(marker).despawn();
            }
            _ => {}
        }
    }
}

/// Attach a queued rare aura once its item's root has spawned under the
/// character: locate the item root by its [`SpawnedFromResource`] handle (same
/// lookup as [`apply_weapon_shine`]), then hang the looping effect off the
/// weapon bone named in `ItemRare.txt` (`ai_end` = blade tip, `Bone01` = bow
/// mount, …) at the table's scale, so the aura sits on the blade and follows
/// it. Falls back to the item root if the bone is missing.
fn apply_rare_aura(
    time: Res<Time>,
    asset_server: Res<AssetServer>,
    children: Query<&Children>,
    from_resource: Query<&SpawnedFromResource>,
    bindings: Query<&SkeletonBinding>,
    inventories: Query<&Inventory, With<Player>>,
    mut pending: Query<(Entity, &mut PendingRareAura)>,
    mut commands: Commands,
) {
    for (marker, mut req) in pending.iter_mut() {
        let timed_out = req.timeout.tick(time.delta()).is_finished();
        // A min_opt row queued from a live equip needs the settled slot's
        // opt level first (mirrors AttachmentShine::EquipSlot in
        // apply_weapon_shine). None = slot not settled yet, Some(false) =
        // settled below the threshold (no aura), Some(true) = go.
        let gate_ok: Option<bool> = match (req.min_opt, req.gate) {
            (Some(min), Some((slot, ref_id))) => inventories
                .single()
                .ok()
                .and_then(|inv| inv.get(slot))
                .filter(|item| item.ref_id == ref_id)
                .map(|item| match &item.data {
                    ItemTypeData::Equipment(eq) => eq.opt_level >= min,
                    _ => false,
                }),
            // pre-validated (spawn-time attach) or no threshold at all
            _ => Some(true),
        };
        match gate_ok {
            Some(true) => {}
            Some(false) => {
                commands.entity(marker).despawn();
                continue;
            }
            None => {
                if timed_out {
                    commands.entity(marker).despawn();
                }
                continue;
            }
        }
        let root = children.iter_descendants(req.wrapper).find(|e| {
            from_resource
                .get(*e)
                .is_ok_and(|r| r.0.id() == req.item.id())
        });
        match root {
            Some(root) => {
                // The aura hangs off the named bone, in the BONE's frame:
                // the authored content compensates for the bone chain's
                // rotations itself (e.g. the blade-wrap's `compo` node
                // carries a +90° Y SetRotation that lands its sweep on the
                // bone-frame blade axis), so attaching in any other frame
                // turns the whole effect off-axis (BRP-measured 2026-07-28).
                // Scale is the table's, as authored: weapons are modeled big
                // (an 8-unit blade on a 10-unit character) and the sweep
                // spans that blade at exactly this scale.
                let anchor = bindings
                    .get(root)
                    .ok()
                    .and_then(|binding| binding.bones.get(&req.bone).copied())
                    .unwrap_or(root);
                // No hand-added markers or dampeners beyond this: the former
                // time-scale / intensity / size dampeners compensated the old
                // 8x-boosted additive pipeline, and the former per-aura leaf
                // self-emission opt-in is now the engine-wide default
                // (LeafEmitPolicy) — with the exe-faithful brightness math the
                // authored efp + ItemRare.txt values are the ground truth.
                bevy::log::debug!(
                    "rare aura: {} bone={:?} scale={}",
                    req.effect,
                    req.bone,
                    req.scale
                );
                let placement = Transform::from_scale(bevy::math::Vec3::splat(req.scale));
                commands.attach_effect(asset_server.load(&req.effect), anchor, placement);
                commands.entity(marker).despawn();
            }
            None if timed_out => {
                commands.entity(marker).despawn();
            }
            None => {}
        }
    }
}
