//! Hover & click-to-select for in-world entities (players, NPCs, monsters).
//!
//! Idea: reuse Bevy's `bevy_picking` mesh raycast (already added app-wide with
//! `require_markers`) the same way the character-select scene does — tag the
//! streamed leaf meshes `Pickable`, put `MeshPickingCamera` on the game camera,
//! and give each `RemoteEntity` root a `Hovered` component that picking bubbles
//! hover state up to. From there:
//!  - [`update_hover_target`] resolves which root is hovered (players only while
//!    Shift is held, so the ground stays clickable), writing [`HoveredEntity`];
//!  - [`update_selection`] promotes a left-click into the persistent
//!    [`SelectedEntity`];
//!  - [`apply_highlights`] drives a subtle fresnel-rim + emissive lighten on the
//!    union of the two, cloning materials (never mutating the shared handles,
//!    which are loaded by asset path). The nameplate underline lives in the HUD
//!    and reads the same two resources.

use std::collections::HashMap;

use bevy::camera::primitives::Aabb;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::picking::hover::Hovered;
use bevy::prelude::*;

use packets::agent::prelude::SelectEntityRequest;
use packets::Packet;

use crate::assets::bmt::rim::{rim_settings, RimExtension, SroRimMaterial};
use crate::assets::bmt::sheen::SroSheenMaterial;
use crate::net::connection::SilkroadConnection;
use crate::plugins::camera::PlayerCamera;
use crate::plugins::combat::{AttackOrder, Dying, PickupOrder};
use crate::plugins::config::selection::{HighlightColors, SelectionDecalColors};
use crate::plugins::config::ClientConfig;
use crate::plugins::cos::{interacts_as_character, CosEntity};
use crate::plugins::hud::chat::model::ChatState;
use crate::plugins::nav::decal::{decal_mesh, NavMeshDecal};
use crate::plugins::net::agent::AgentConnection;
use crate::plugins::net::entities::{NetworkId, RemoteEntity};
use crate::scenes::SceneState;

// --- Selection state --------------------------------------------------------

/// The entity the cursor is currently over (players only while Shift is held);
/// transient, recomputed every frame. Cleared when nothing valid is hovered.
#[derive(Resource, Default)]
pub struct HoveredEntity(pub Option<Entity>);

/// The persistent click-selected target — the entity later interactions
/// (attack, talk, trade) act on. Set on left-click of a hovered entity, cleared
/// by clicking empty ground.
#[derive(Resource, Default)]
pub struct SelectedEntity(pub Option<Entity>);

#[derive(Clone, Copy, PartialEq, Eq)]
enum HighlightKind {
    Hover,
    Selected,
}

/// The `StandardMaterial` a highlighted mesh had before it was swapped for a
/// rim clone; restored when the highlight ends.
#[derive(Component)]
struct OriginalStandardMaterial(Handle<StandardMaterial>);

/// The `SroSheenMaterial` a highlighted mesh had before its emissive-lighten
/// clone; restored when the highlight ends.
#[derive(Component)]
struct OriginalSheenMaterial(Handle<SroSheenMaterial>);

/// The `SroRimMaterial` a highlighted mesh had before its boosted clone —
/// character meshes carry the subtle always-on rim by default
/// (`graphics.rim`), so highlighting them is a rim-parameter override, not a
/// material-type swap. Restored when the highlight ends.
#[derive(Component)]
struct OriginalRimMaterial(Handle<SroRimMaterial>);

// --- Plugin -----------------------------------------------------------------

pub struct EntitySelectionPlugin;

impl Plugin for EntitySelectionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<crate::plugins::config::selection::HighlightColors>()
            .init_resource::<crate::plugins::config::selection::SelectionDecalColors>()
            .add_systems(
                PreUpdate,
                apply_selection_colors.run_if(crate::plugins::settings::live::config_changed),
            )
            .init_resource::<HoveredEntity>()
            .init_resource::<SelectedEntity>()
            // MaterialPlugin::<SroRimMaterial> is registered in main.rs: the
            // rim material doubles as the always-on character rim, needed
            // before any selection happens
            // also active in the offline Skills scene (selecting the training
            // dummy; send_select_request no-ops without a connection)
            .add_systems(OnEnter(SceneState::GameWorld), spawn_selection_decal)
            .add_systems(OnExit(SceneState::GameWorld), cleanup_selection_decal)
            .add_systems(OnEnter(SceneState::Skills), spawn_selection_decal)
            .add_systems(OnExit(SceneState::Skills), cleanup_selection_decal)
            .add_systems(
                Update,
                (
                    enable_entity_picking_camera,
                    tag_hoverable_entities,
                    maintain_hit_proxies,
                    untarget_dying_entities,
                    update_hover_target,
                    // No world select/attack/pickup while the local player is
                    // dead (#142 input lock).
                    update_selection.run_if(not(crate::plugins::hud::death::player_is_dead)),
                    clear_dangling_selection,
                    apply_highlights,
                    update_selection_decal,
                    send_select_request,
                )
                    .chain()
                    .run_if(in_state(SceneState::GameWorld).or_else(in_state(SceneState::Skills))),
            );
    }
}

// --- Systems ----------------------------------------------------------------

/// The mesh raycast backend runs with `require_markers`, so the game camera
/// must opt in to mesh picking explicitly (mirrors char-select).
fn enable_entity_picking_camera(
    cam_query: Query<Entity, (With<PlayerCamera>, Without<MeshPickingCamera>)>,
    mut commands: Commands,
) {
    for entity in cam_query.iter() {
        commands.entity(entity).insert(MeshPickingCamera);
    }
}

/// Give every hoverable root a `Hovered` component and make its descendant
/// meshes pickable (`RayCastBackfaces` because SRO models render mirrored, so
/// face winding is unreliable for backface culling). Ground items are hoverable
/// too (hover shows their name, click picks them up).
///
/// Remote entity meshes stream in asynchronously — body first, then equipment —
/// so this has to keep catching up after the root exists. It used to do that by
/// re-walking every remote entity's whole subtree every frame, which is where
/// the cost was: a character rig is a few hundred entities (bones, mesh groups,
/// attachments), so a normal town population meant tens of thousands of
/// hierarchy hops per frame to discover, almost always, nothing.
///
/// The two events that can actually create work are enumerated instead:
///
/// * a **root appears** — walk its subtree once, on the frame it first lacks
///   `Hovered`. Inserting `Hovered` is what takes it out of that query, so the
///   walk happens exactly once per root.
/// * a **mesh appears** — walk *up* from the meshes added this frame to see
///   whether they sit under a hoverable root. That is a handful of entities per
///   frame times a shallow ancestor chain, whichever part of the world they
///   belong to.
///
/// The invariant that makes the second half sufficient: a root never loses and
/// regains its meshes. `untarget_dying_entities` strips `Pickable` when an
/// entity starts dying, and `Dying` is never removed — `finish_dying` despawns
/// the entity outright — so a stripped subtree is gone rather than revived, and
/// its replacement arrives as a fresh root through the first case.
fn tag_hoverable_entities(
    new_roots: Query<Entity, (With<RemoteEntity>, Without<Dying>, Without<Hovered>)>,
    new_meshes: Query<Entity, (Added<Mesh3d>, Without<Pickable>)>,
    hoverable: Query<(), (With<RemoteEntity>, Without<Dying>)>,
    children: Query<&Children>,
    parents: Query<&ChildOf>,
    untagged: Query<(), (With<Mesh3d>, Without<Pickable>)>,
    mut commands: Commands,
) {
    for root in new_roots.iter() {
        commands.entity(root).insert(Hovered::default());
        for entity in children.iter_descendants(root) {
            if untagged.contains(entity) {
                commands
                    .entity(entity)
                    .insert((Pickable::default(), RayCastBackfaces));
            }
        }
    }
    for mesh in new_meshes.iter() {
        if parents
            .iter_ancestors(mesh)
            .any(|ancestor| hoverable.contains(ancestor))
        {
            commands
                .entity(mesh)
                .insert((Pickable::default(), RayCastBackfaces));
        }
    }
}

/// On the root: how many descendant AABBs the current click proxy was built
/// from — rebuilt when more meshes stream in (body, then equipment).
#[derive(Component)]
struct HitProxy {
    sources: usize,
}

/// Marker on the invisible click-volume child itself. `pub(crate)` so
/// meshless interactables (teleport gates) can attach their own volume —
/// `maintain_hit_proxies` skips those roots (no mesh AABB sources) and the
/// marker keeps the manual volume out of the source count.
#[derive(Component)]
pub(crate) struct HitProxyVolume;

/// Padding factor on the merged model AABB — vanilla's click cylinders are
/// noticeably generous.
const HIT_PROXY_PADDING: f32 = 1.15;
/// Minimum proxy extent per axis for characters (players/NPCs/monsters), so a
/// bind-pose silhouette or a thin model stays comfortably clickable.
const HIT_PROXY_MIN_EXTENT: f32 = 8.0;
/// Minimum proxy extent for ground items — a small drop (coin pile) has a real,
/// small mesh AABB, so the box should hug it rather than balloon to the
/// character floor. Just a small floor so a flat pile keeps some height.
const HIT_PROXY_MIN_EXTENT_ITEM: f32 = 2.5;

/// Give every remote entity an invisible, generously-sized click volume.
///
/// Mesh picking raycasts actual triangles, and for skinned characters those
/// are the *bind pose* — a narrow silhouette that drifts from the animated
/// model on screen. Like the original client (which clicks against collision
/// cylinders), we spawn a hidden cuboid child sized from the union of the
/// spawned meshes' AABBs (`MeshPickingSettings.ray_cast_visibility` is `Any`
/// so hidden volumes are raycastable). Hover events bubble up the hierarchy
/// to the root's `Hovered` exactly like mesh hits do.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn maintain_hit_proxies(
    roots: Query<(&GlobalTransform, &RemoteEntity, Option<&HitProxy>), Without<Dying>>,
    // A root with no proxy yet, and every root above an AABB that appeared
    // since this system last ran. Together these are the only ways the merged
    // bounds can become stale — see the note on the walk below.
    unproxied: Query<Entity, (With<RemoteEntity>, Without<Dying>, Without<HitProxy>)>,
    new_aabbs: Query<Entity, (Added<Aabb>, Without<HitProxyVolume>)>,
    children: Query<&Children>,
    parents: Query<&ChildOf>,
    volumes: Query<(), With<HitProxyVolume>>,
    aabbs: Query<(&Aabb, &GlobalTransform)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut unit_cube: Local<Option<Handle<Mesh>>>,
    mut commands: Commands,
) {
    let unit_cube = unit_cube
        .get_or_insert_with(|| meshes.add(Cuboid::new(1.0, 1.0, 1.0)))
        .clone();

    // This used to walk every remote entity's subtree every frame purely to
    // `.count()` the contributing AABBs and compare that against the last
    // build — the same tens of thousands of hierarchy hops per frame that
    // `tag_hoverable_entities` above was paying, for a number that changes only
    // while a rig is still streaming. Enumerate the roots that can have gone
    // stale instead; the count then falls out of the merge walk that a rebuild
    // has to do anyway, so a rebuilding root walks its subtree once instead of
    // twice.
    let mut dirty: bevy::platform::collections::HashSet<Entity> = unproxied.iter().collect();
    for entity in new_aabbs.iter() {
        // `Aabb` is inserted by bevy's `calculate_bounds` in PostUpdate, so a
        // mesh that arrived last frame is `Added` here this frame.
        if let Some(root) = parents
            .iter_ancestors(entity)
            .find(|ancestor| roots.contains(*ancestor))
        {
            dirty.insert(root);
        }
    }

    for root in dirty {
        let Ok((root_gt, kind, proxy)) = roots.get(root) else {
            continue;
        };
        // Merge every streamed-in mesh AABB into root-local space, counting the
        // contributors as we go. The proxy's own AABB is excluded so it can't
        // feed back into its bounds.
        let to_local = root_gt.affine().inverse();
        let mut sources = 0usize;
        let mut min = Vec3::MAX;
        let mut max = Vec3::MIN;
        for child in children.iter_descendants(root) {
            if volumes.contains(child) {
                continue;
            }
            let Ok((aabb, gt)) = aabbs.get(child) else {
                continue;
            };
            sources += 1;
            let center = Vec3::from(aabb.center);
            let half = Vec3::from(aabb.half_extents);
            for corner in [
                Vec3::new(-1.0, -1.0, -1.0),
                Vec3::new(-1.0, -1.0, 1.0),
                Vec3::new(-1.0, 1.0, -1.0),
                Vec3::new(-1.0, 1.0, 1.0),
                Vec3::new(1.0, -1.0, -1.0),
                Vec3::new(1.0, -1.0, 1.0),
                Vec3::new(1.0, 1.0, -1.0),
                Vec3::new(1.0, 1.0, 1.0),
            ] {
                let local = to_local.transform_point3(gt.transform_point(center + corner * half));
                min = min.min(local);
                max = max.max(local);
            }
        }
        // Nothing has streamed in yet (meshless interactables keep their own
        // manually attached volume), or the bounds are already current.
        if sources == 0 || proxy.is_some_and(|p| p.sources == sources) {
            continue;
        }
        // Rebuild: drop the previous volume, spawn the padded replacement.
        for child in children.iter_descendants(root) {
            if volumes.contains(child) {
                commands.entity(child).despawn();
            }
        }
        let min_extent = if matches!(kind, RemoteEntity::Item) {
            HIT_PROXY_MIN_EXTENT_ITEM
        } else {
            HIT_PROXY_MIN_EXTENT
        };
        let size = ((max - min) * HIT_PROXY_PADDING).max(Vec3::splat(min_extent));
        let center = (min + max) / 2.0;
        commands.entity(root).insert(HitProxy { sources });
        commands.entity(root).with_children(|parent| {
            parent.spawn((
                HitProxyVolume,
                Mesh3d(unit_cube.clone()),
                Transform::from_translation(center).with_scale(size),
                Pickable::default(),
                RayCastBackfaces,
                Visibility::Hidden,
            ));
        });
    }
}

/// A corpse is scenery: the moment an entity starts dying, strip everything
/// that made it a click target — the hover tag on the root, the hidden click
/// volume, and the `Pickable` markers on its meshes — so rays pass through
/// to whatever is behind it. Runs once per death (`Added<Dying>`); the
/// hover/proxy maintenance systems skip dying roots so nothing re-tags.
#[allow(clippy::type_complexity)]
fn untarget_dying_entities(
    corpses: Query<(Entity, Has<Hovered>), (With<RemoteEntity>, Added<Dying>)>,
    children: Query<&Children>,
    volumes: Query<(), With<HitProxyVolume>>,
    pickables: Query<(), With<Pickable>>,
    mut commands: Commands,
) {
    for (root, has_hovered) in corpses.iter() {
        if has_hovered {
            commands.entity(root).remove::<Hovered>();
        }
        commands.entity(root).remove::<HitProxy>();
        for child in children.iter_descendants(root) {
            if volumes.contains(child) {
                commands.entity(child).despawn();
            } else if pickables.contains(child) {
                commands.entity(child).remove::<Pickable>();
            }
        }
    }
}

/// Resolve which hovered root is the effective target: NPCs/monsters always
/// count, characters only while Shift is held; nothing while the chat input is
/// focused.
///
/// "Character" is [`interacts_as_character`], which counts a COS as one — a pet
/// grabbing the hover as you walk past it is the same nuisance a player would
/// be, and is why Shift gates them both.
fn update_hover_target(
    keys: Res<ButtonInput<KeyCode>>,
    chat: Res<ChatState>,
    entities: Query<(Entity, &Hovered, &RemoteEntity, Has<CosEntity>)>,
    mut hovered: ResMut<HoveredEntity>,
) {
    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    let target = if chat.input_open {
        None
    } else {
        entities.iter().find_map(|(entity, hov, kind, is_cos)| {
            if !hov.0 {
                return None;
            }
            if interacts_as_character(kind, is_cos) {
                return shift.then_some(entity);
            }
            Some(entity)
        })
    };
    if hovered.0 != target {
        hovered.0 = target;
    }
}

/// Left-click on a hovered entity promotes it to the persistent selection.
/// Ground clicks (movement) deliberately do NOT clear it — the selection only
/// ends when the target despawns ([`clear_dangling_selection`]).
///
/// A click on the *already selected* monster orders an attack: because the
/// selection persists, this single rule covers both "double-click" (first
/// press selects, second attacks) and "click again later" without any timer.
fn update_selection(
    buttons: Res<ButtonInput<MouseButton>>,
    chat: Res<ChatState>,
    hovered: Res<HoveredEntity>,
    mut selected: ResMut<SelectedEntity>,
    kinds: Query<&RemoteEntity>,
    cos: Query<(), With<CosEntity>>,
    mut attacks: MessageWriter<AttackOrder>,
    mut pickups: MessageWriter<PickupOrder>,
    mut talks: MessageWriter<super::npcs::TalkOrder>,
) {
    if chat.input_open || !buttons.just_pressed(MouseButton::Left) {
        return;
    }
    if let Some(target) = hovered.0 {
        // Ground items are pickup targets, not selection targets: a single
        // click orders the (server-driven) pickup run.
        if matches!(kinds.get(target), Ok(RemoteEntity::Item)) {
            pickups.write(PickupOrder(target));
            return;
        }
        // Per vanilla, ANY click on an NPC (first or repeat) also starts the
        // walk-up-and-talk interaction; player re-clicks need PVP state.
        //
        // A COS spawns as an NPC and must never talk. This used to exempt only
        // the COS you were *riding* — enough to stop the rider walking up to
        // its own horse, but it left every other pet opening a shopkeeper
        // dialog. `interacts_as_character` is the general rule, and the ridden
        // mount falls out of it.
        let is_cos = cos.contains(target);
        if let Ok(kind) = kinds.get(target) {
            if matches!(kind, RemoteEntity::Npc) && !interacts_as_character(kind, is_cos) {
                talks.write(super::npcs::TalkOrder(target));
            }
        }
        if selected.0 != Some(target) {
            selected.0 = Some(target);
        } else if matches!(kinds.get(target), Ok(RemoteEntity::Monster)) {
            attacks.write(AttackOrder(target));
        }
    }
}

/// Drop a selection whose entity has despawned (e.g. a killed monster).
fn clear_dangling_selection(
    mut selected: ResMut<SelectedEntity>,
    entities: Query<(), With<RemoteEntity>>,
) {
    if let Some(entity) = selected.0 {
        if entities.get(entity).is_err() {
            selected.0 = None;
        }
    }
}

/// Apply/remove the rim + emissive highlight on the union of the hovered and
/// selected entities, diffing against what is currently highlighted. Runs every
/// frame so meshes that stream in mid-highlight still get tinted; already-tinted
/// meshes are skipped, so the per-frame cost is one descendant walk over the
/// (tiny) highlighted set.
#[allow(clippy::too_many_arguments)]
fn apply_highlights(
    hovered: Res<HoveredEntity>,
    selected: Res<SelectedEntity>,
    colors: Res<HighlightColors>,
    mut active: Local<HashMap<Entity, HighlightKind>>,
    children: Query<&Children>,
    std_meshes: Query<&MeshMaterial3d<StandardMaterial>>,
    sheen_meshes: Query<&MeshMaterial3d<SroSheenMaterial>>,
    rim_meshes: Query<&MeshMaterial3d<SroRimMaterial>>,
    std_originals: Query<&OriginalStandardMaterial>,
    sheen_originals: Query<&OriginalSheenMaterial>,
    rim_originals: Query<&OriginalRimMaterial>,
    std_assets: Res<Assets<StandardMaterial>>,
    mut sheen_assets: ResMut<Assets<SroSheenMaterial>>,
    mut rim_assets: ResMut<Assets<SroRimMaterial>>,
    mut commands: Commands,
) {
    // Desired highlight per root; a selected entity outranks a hovered one.
    let mut desired: HashMap<Entity, HighlightKind> = HashMap::new();
    if let Some(entity) = selected.0 {
        desired.insert(entity, HighlightKind::Selected);
    }
    if let Some(entity) = hovered.0 {
        desired.entry(entity).or_insert(HighlightKind::Hover);
    }

    // Restore roots that left the set or changed kind.
    let to_restore: Vec<Entity> = active
        .iter()
        .filter(|(entity, kind)| desired.get(entity) != Some(kind))
        .map(|(entity, _)| *entity)
        .collect();
    for root in to_restore {
        for entity in children.iter_descendants(root) {
            if let Ok(original) = std_originals.get(entity) {
                commands
                    .entity(entity)
                    .insert(MeshMaterial3d(original.0.clone()))
                    .remove::<MeshMaterial3d<SroRimMaterial>>()
                    .remove::<OriginalStandardMaterial>();
            }
            if let Ok(original) = sheen_originals.get(entity) {
                commands
                    .entity(entity)
                    .insert(MeshMaterial3d(original.0.clone()))
                    .remove::<OriginalSheenMaterial>();
            }
            if let Ok(original) = rim_originals.get(entity) {
                commands
                    .entity(entity)
                    .insert(MeshMaterial3d(original.0.clone()))
                    .remove::<OriginalRimMaterial>();
            }
        }
        active.remove(&root);
    }

    // Apply the desired highlight (also picks up meshes streamed in since the
    // last change).
    for (root, kind) in desired.iter() {
        let rim_color = match kind {
            HighlightKind::Hover => colors.hover_rim,
            HighlightKind::Selected => colors.selected_rim,
        };
        for entity in children.iter_descendants(*root) {
            // Character meshes carrying the always-on rim (`graphics.rim`):
            // the highlight is a rim-parameter override on a clone — the
            // subtle ambient rim becomes the stronger selection color, plus
            // the usual emissive lighten. (A std mesh already swapped by the
            // branch below also matches this query; its OriginalStandard
            // stash marks it as handled.)
            if let Ok(material) = rim_meshes.get(entity) {
                if rim_originals.contains(entity) || std_originals.contains(entity) {
                    continue;
                }
                let Some(mut cloned) = rim_assets.get(&material.0).cloned() else {
                    continue;
                };
                cloned.base.emissive = lighten(cloned.base.emissive, colors.emissive_strength);
                cloned.extension.settings = rim_settings(
                    rim_color,
                    colors.rim_strength,
                    colors.rim_power,
                    colors.rim_relative,
                );
                let handle = rim_assets.add(cloned);
                commands.entity(entity).insert((
                    MeshMaterial3d(handle),
                    OriginalRimMaterial(material.0.clone()),
                ));
                continue;
            }
            // Body/cloth (StandardMaterial): swap to a rim clone with a small
            // emissive lighten.
            if let Ok(material) = std_meshes.get(entity) {
                if std_originals.contains(entity) {
                    continue;
                }
                let Some(mut base) = std_assets.get(&material.0).cloned() else {
                    continue;
                };
                base.emissive = lighten(base.emissive, colors.emissive_strength);
                let handle = rim_assets.add(SroRimMaterial {
                    base,
                    extension: RimExtension {
                        settings: rim_settings(
                            rim_color,
                            colors.rim_strength,
                            colors.rim_power,
                            colors.rim_relative,
                        ),
                    },
                });
                commands
                    .entity(entity)
                    .insert((
                        MeshMaterial3d(handle),
                        OriginalStandardMaterial(material.0.clone()),
                    ))
                    .remove::<MeshMaterial3d<StandardMaterial>>();
                continue;
            }
            // Weapons/metal (sheen): emissive lighten (keep its type), plus —
            // behind `selection.highlight.rim_boost` — the selection color on
            // the sheen material's own rim term, so the outline matches the
            // body's instead of only brightening.
            if let Ok(material) = sheen_meshes.get(entity) {
                if sheen_originals.contains(entity) {
                    continue;
                }
                let Some(mut cloned) = sheen_assets.get(&material.0).cloned() else {
                    continue;
                };
                cloned.base.emissive = lighten(cloned.base.emissive, colors.emissive_strength);
                if colors.rim_boost {
                    let boost = rim_settings(
                        rim_color,
                        colors.rim_strength,
                        colors.rim_power,
                        colors.rim_relative,
                    );
                    cloned.extension.settings.rim_color = boost.color;
                    cloned.extension.settings.rim_power = boost.power;
                }
                let handle = sheen_assets.add(cloned);
                commands.entity(entity).insert((
                    MeshMaterial3d(handle),
                    OriginalSheenMaterial(material.0.clone()),
                ));
            }
        }
        active.insert(*root, *kind);
    }
}

// --- Selection ground decal ---------------------------------------------------

/// The colored ring draped on the ground under the click-selected target
/// (reuses the nav-mesh decal draping the click-to-move circle uses).
#[derive(Component)]
struct SelectionDecal;

/// How far the target must move before the decal follows — draping re-projects
/// the whole vertex grid on every transform write, so don't chase sub-unit
/// jitter of a standing target.
const DECAL_FOLLOW_EPSILON: f32 = 0.25;

fn spawn_selection_decal(
    colors: Res<SelectionDecalColors>,
    asset_server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
) {
    commands.spawn((
        Name::from("Selection Decal"),
        SelectionDecal,
        NavMeshDecal { size: colors.size },
        Mesh3d(meshes.add(decal_mesh())),
        // Own exclusive material instance (tinted per target kind) — never
        // shared with the click-to-move decal.
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color_texture: Some(asset_server.load("media://effect/select_01.ddj")),
            unlit: true,
            alpha_mode: AlphaMode::Add,
            ..default()
        })),
        Transform::default(),
        Visibility::Hidden,
        NoFrustumCulling,
        NotShadowCaster,
        NotShadowReceiver,
    ));
}

fn cleanup_selection_decal(decals: Query<Entity, With<SelectionDecal>>, mut commands: Commands) {
    for entity in decals.iter() {
        commands.entity(entity).despawn();
    }
}

/// Keep the decal under the selected target, tinted by its kind (monster red,
/// NPC/player blue — configurable); hidden while nothing is selected.
fn update_selection_decal(
    selected: Res<SelectedEntity>,
    colors: Res<SelectionDecalColors>,
    targets: Query<(&Transform, &RemoteEntity), Without<SelectionDecal>>,
    mut decals: Query<
        (
            &mut Transform,
            &mut Visibility,
            &MeshMaterial3d<StandardMaterial>,
        ),
        With<SelectionDecal>,
    >,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let Ok((mut transform, mut visibility, material)) = decals.single_mut() else {
        return;
    };
    let target = selected.0.and_then(|entity| targets.get(entity).ok());
    let Some((target_transform, kind)) = target else {
        if *visibility != Visibility::Hidden {
            *visibility = Visibility::Hidden;
        }
        return;
    };

    let tint = match kind {
        RemoteEntity::Monster => colors.monster,
        RemoteEntity::Npc => colors.npc,
        _ => colors.player,
    };
    if let Some(mut material) = materials.get_mut(&material.0) {
        if material.base_color != tint {
            material.base_color = tint;
        }
    }

    // Writing the transform triggers a full re-drape, so only follow real moves
    // (visibility flips also force one placement so a fresh selection drapes).
    let target_pos = target_transform.translation;
    if *visibility == Visibility::Hidden
        || transform.translation.distance_squared(target_pos) > DECAL_FOLLOW_EPSILON.powi(2)
    {
        transform.translation = target_pos;
    }
    if *visibility != Visibility::Visible {
        *visibility = Visibility::Visible;
    }
}

// --- Select-entity packet (0x7045) --------------------------------------------

/// Tell the server about a new selection so it answers with 0xB045 (target
/// info). Fire-and-forget: go-sro never responds for NPCs, so nothing here may
/// wait on the reply.
fn send_select_request(
    selected: Res<SelectedEntity>,
    ids: Query<&NetworkId>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
) {
    if !selected.is_changed() || selected.is_added() {
        return;
    }
    let Some(id) = selected.0.and_then(|entity| ids.get(entity).ok()) else {
        return;
    };
    // Offline sandbox has no agent connection — selection still works locally.
    let Ok(conn) = conn.single() else {
        return;
    };
    let request = SelectEntityRequest { unique_id: id.0 };
    if let Err(e) = conn.get_sender().send(Packet::from(request).into()) {
        error!("network: failed to send SelectEntityRequest: {}", e.0);
    }
}

/// Add a neutral value to each emissive channel (the whole-model lighten).
fn lighten(e: LinearRgba, strength: f32) -> LinearRgba {
    LinearRgba::new(
        e.red + strength,
        e.green + strength,
        e.blue + strength,
        // alpha 0: bevy scales emissive by `mix(1.0, view.exposure, a)` —
        // at the default alpha 1.0 the +0.06 lighten was multiplied by
        // exposure (~0.001) and contributed nothing visible
        0.0,
    )
}

/// Re-resolves the hover/selection rim + decal palettes from `config.yaml`
/// ([`crate::plugins::settings::live`]).
///
/// `rim_relative` is read from `graphics.rim.mode`, not from the selection
/// block: there is one global rim application mode, shared with the always-on
/// character rim, and this system is the single place that copies it.
pub fn apply_selection_colors(
    config: Res<ClientConfig>,
    mut colors: ResMut<crate::plugins::config::selection::HighlightColors>,
    mut decal_colors: ResMut<crate::plugins::config::selection::SelectionDecalColors>,
) {
    *colors = config.selection.highlight.resolved();
    colors.rim_relative =
        config.graphics.rim.mode == crate::plugins::config::graphics::RimMode::Relative;
    *decal_colors = config.selection.decal.resolved();
}
