//! Per-block atmosphere + portal culling for the active dungeon.
//!
//! The original client renders `currentBlock ∪ VisibleBlockIndices` (portal
//! occlusion) and drives fog and lighting from the current block's own
//! parameters instead of the overworld's ENVI region profiles. Here: the
//! player's block is resolved from the DOF block collision boxes (a
//! provisional resolve until the nav runtime provides the surface-accurate
//! one), block roots toggle `Visibility`, and the camera's `DistanceFog` +
//! the clear color + the global ambient follow the current block. `field_18`
//! (`block.unk_uint1`) is applied as the block's ambient color — a corpus
//! observation (0xFF-alpha packed dark tones), still [S] semantics
//! (`docs/formats/dof-jmxvdof.md`).

use bevy::pbr::DistanceFog;
use bevy::prelude::*;

use super::spawn::DungeonLight;
use super::{ActiveDungeon, DungeonBlock};
use crate::assets::dof::JMXVDOF;
use crate::plugins::config::ClientConfig;
use crate::plugins::nav::dungeon::ActiveDungeonNav;
use crate::plugins::nav::NavLocation;
use crate::plugins::player::Player;
use crate::plugins::world_origin::WorldOrigin;

/// Ambient brightness inside dungeons — calibration constant; the color is
/// PK2-sourced (`field_18`).
const DUNGEON_AMBIENT_BRIGHTNESS: f32 = 250.0;

/// Resolve which block the player currently stands in. Best source: the nav
/// location — the block whose `BmsNavMesh` the player is standing on (exact,
/// incl. stacked floors). Fallbacks: the voxel grid's candidate list ranked
/// by collision-box center-Y distance, then plain nearest box center (for an
/// unresolved player, e.g. right after arrival).
pub fn resolve_current_block(
    active: Option<ResMut<ActiveDungeon>>,
    dungeon_nav: Option<Res<ActiveDungeonNav>>,
    dofs: Res<Assets<JMXVDOF>>,
    player: Query<(&Transform, &NavLocation), With<Player>>,
    parents: Query<&ChildOf>,
    blocks: Query<&super::DungeonBlock>,
    origin: Res<WorldOrigin>,
) {
    let Some(mut active) = active else { return };
    let Some(dof) = dofs.get(&active.dof) else {
        return;
    };
    let Ok((player, nav_location)) = player.single() else {
        return;
    };

    // The surface the player stands on names the block directly. The
    // ObjectNavMesh entity is the block root or a descendant of it.
    if let Some(nav) = &dungeon_nav {
        if let Some(standing_on) = nav_location.object() {
            let block = std::iter::once(standing_on)
                .chain(parents.iter_ancestors(standing_on))
                .find_map(|entity| {
                    nav.block_of_entity
                        .get(&entity)
                        .copied()
                        .or_else(|| blocks.get(entity).ok().map(|b| b.index))
                });
            if let Some(index) = block {
                if active.current_block != Some(index) {
                    active.current_block = Some(index);
                }
                return;
            }
        }
    }

    let sro = origin.to_sro(player.translation);
    // Back into the raw D3D frame the boxes and the voxel grid live in.
    let raw = Vec3::new(-sro.x, sro.y, sro.z);

    // Bbox fallback. `Block.CollisionBox0` is BLOCK-LOCAL in the data —
    // only the world-lifted boxes in `DungeonNavData` may be compared
    // against the player position. Until they exist, nearest block
    // *position* (dungeon-frame by definition) is the coarse stand-in.
    let resolved = match &dungeon_nav {
        Some(nav) => {
            // Voxel candidates first (the DOF's own narrow phase).
            let voxel = nav.data.voxel_candidates(player.translation);
            let candidates: Vec<usize> = if voxel.is_empty() {
                (0..dof.blocks.len()).collect()
            } else {
                voxel.iter().map(|&i| i as usize).collect()
            };
            let mut containing: Option<(usize, f32)> = None;
            let mut nearest: Option<(usize, f32)> = None;
            for index in candidates {
                let Some((min, max)) = nav.data.world_box(index) else {
                    continue;
                };
                let center = (min + max) * 0.5;
                let dist = center.distance_squared(raw);
                if nearest.is_none_or(|(_, best)| dist < best) {
                    nearest = Some((index, dist));
                }
                if raw.cmpge(min).all() && raw.cmple(max).all() {
                    let dy = (center.y - raw.y).abs();
                    if containing.is_none_or(|(_, best)| dy < best) {
                        containing = Some((index, dy));
                    }
                }
            }
            containing.or(nearest).map(|(index, _)| index)
        }
        None => dof
            .blocks
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                a.position
                    .distance_squared(raw)
                    .total_cmp(&b.position.distance_squared(raw))
            })
            .map(|(index, _)| index),
    };
    if active.current_block != resolved {
        active.current_block = resolved;
    }
}

/// Extra reach past the fog far plane before a block is culled outright —
/// covers the fade band so geometry never pops inside visible fog.
const FOG_CULL_MARGIN: f32 = 200.0;
/// Draw distance when the current block declares no usable fog planes (same
/// fallback the fog itself uses in [`apply_dungeon_atmosphere`]).
const FOG_FALLBACK_FAR: f32 = 5000.0;

/// The block's effective fog far plane.
fn fog_far(block: &crate::assets::dof::format::DofBlock) -> f32 {
    if block.fog.far_plane > block.fog.near_plane {
        block.fog.far_plane
    } else {
        FOG_FALLBACK_FAR
    }
}

/// The active dungeon's current fog reach — the shared culling radius for
/// blocks and effect simulation (nothing past it is visible through the fog).
pub fn current_fog_far(active: &ActiveDungeon, dofs: &Assets<JMXVDOF>) -> f32 {
    active
        .current_block
        .and_then(|index| dofs.get(&active.dof).and_then(|dof| dof.blocks.get(index)))
        .map(fog_far)
        .unwrap_or(FOG_FALLBACK_FAR)
}

/// Portal + fog culling: a block renders iff it is in
/// `current ∪ VisibleBlockIndices` AND its collision box lies within the
/// current fog far plane of the player. The fog test is what keeps dense
/// open dungeons (Donwhang cave: 151 blocks in a ~3.8k-unit grid whose
/// visibility sets cover most of the cave) from drawing everything at once —
/// past the fog the original's own fog would have erased them anyway (the
/// dungeon analogue of `region_visibility`). With no resolved block, only
/// the fog test applies.
pub fn portal_culling(
    active: Option<Res<ActiveDungeon>>,
    dungeon_nav: Option<Res<ActiveDungeonNav>>,
    dofs: Res<Assets<JMXVDOF>>,
    player: Query<&Transform, With<Player>>,
    origin: Res<WorldOrigin>,
    mut blocks: Query<(&DungeonBlock, &mut Visibility)>,
) {
    let Some(active) = active else { return };
    let Some(dof) = dofs.get(&active.dof) else {
        return;
    };
    let visible_set: Option<Vec<usize>> = active.current_block.map(|current| {
        let mut set = vec![current];
        if let Some(block) = dof.blocks.get(current) {
            set.extend(block.visible_block_indices.iter().map(|&i| i as usize));
        }
        set
    });
    let hide_dist = current_fog_far(&active, &dofs) + FOG_CULL_MARGIN;
    // Raw-frame player position — the space the world-lifted boxes live in.
    let raw = player.single().ok().map(|t| {
        let sro = origin.to_sro(t.translation);
        Vec3::new(-sro.x, sro.y, sro.z)
    });

    for (block, mut visibility) in blocks.iter_mut() {
        let in_portal_set = visible_set
            .as_ref()
            .is_none_or(|set| set.contains(&block.index));
        // The stored `CollisionBox0` is block-local; only the world-lifted
        // boxes may be distance-tested. Until the nav data has them, the
        // fog test stays open (portal-only culling).
        let within_fog = match (
            raw,
            dungeon_nav
                .as_ref()
                .and_then(|n| n.data.world_box(block.index)),
        ) {
            (Some(raw), Some((min, max))) => {
                let dx = raw.x - raw.x.clamp(min.x, max.x);
                let dz = raw.z - raw.z.clamp(min.z, max.z);
                dx * dx + dz * dz <= hide_dist * hide_dist
            }
            _ => true,
        };
        let target = if in_portal_set && within_fog {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *visibility != target {
            *visibility = target;
        }
    }
}

/// Cap the number of active dungeon point lights to the
/// `graphics.dungeon.max_lights` nearest the player (0 = unlimited). Dense
/// dungeons carry hundreds of authored lights (median 7 per block, max 117);
/// clustered forward rendering pays for every one in view. Only lights whose
/// block is currently shown compete for the budget; the rest are hidden by
/// their block root anyway and get their own flag reset so they light up
/// correctly when the block reappears.
pub fn light_budget(
    active: Option<Res<ActiveDungeon>>,
    config: Res<ClientConfig>,
    player: Query<&Transform, With<Player>>,
    blocks: Query<&Visibility, (With<DungeonBlock>, Without<DungeonLight>)>,
    mut lights: Query<(&GlobalTransform, &ChildOf, &mut Visibility), With<DungeonLight>>,
) {
    if active.is_none() {
        return;
    }
    let budget = config.graphics.dungeon.max_lights;
    if budget == 0 {
        for (_, _, mut visibility) in lights.iter_mut() {
            if *visibility != Visibility::Inherited {
                *visibility = Visibility::Inherited;
            }
        }
        return;
    }
    let Ok(player) = player.single() else { return };

    let mut ranked: Vec<(f32, usize)> = Vec::new();
    let mut entries: Vec<(bool, Mut<Visibility>)> = Vec::new();
    for (transform, parent, visibility) in lights.iter_mut() {
        let block_shown = blocks
            .get(parent.parent())
            .is_ok_and(|v| *v != Visibility::Hidden);
        let index = entries.len();
        if block_shown {
            let dist = transform.translation().distance_squared(player.translation);
            ranked.push((dist, index));
        }
        entries.push((block_shown, visibility));
    }
    ranked.sort_by(|a, b| a.0.total_cmp(&b.0));
    let enabled: std::collections::HashSet<usize> =
        ranked.iter().take(budget).map(|&(_, i)| i).collect();

    for (index, (block_shown, visibility)) in entries.iter_mut().enumerate() {
        // Lights of hidden blocks reset to Inherited so the block's own
        // visibility stays the single switch for them.
        let target = if !*block_shown || enabled.contains(&index) {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if **visibility != target {
            **visibility = target;
        }
    }
}

/// Drive fog, clear color and ambient from the current block. The ENVI
/// environment chain is gated off while a dungeon is active, so this is the
/// only fog writer.
pub fn apply_dungeon_atmosphere(
    active: Option<Res<ActiveDungeon>>,
    dofs: Res<Assets<JMXVDOF>>,
    mut cameras: Query<(Entity, Option<&mut DistanceFog>), With<Camera3d>>,
    mut clear_color: ResMut<ClearColor>,
    mut ambient: ResMut<GlobalAmbientLight>,
    mut commands: Commands,
) {
    let Some(active) = active else { return };
    let Some(dof) = dofs.get(&active.dof) else {
        return;
    };
    let Some(block) = active.current_block.and_then(|i| dof.blocks.get(i)) else {
        return;
    };

    let fog_color = argb(block.fog.color);
    // Fog params are authored per block; 0/0 planes mean "no fog here" —
    // push the planes out instead of fogging everything at distance 0.
    let (near, far) = if block.fog.far_plane > block.fog.near_plane {
        (block.fog.near_plane, block.fog.far_plane)
    } else {
        (500.0, 5000.0)
    };
    let fog = DistanceFog {
        color: fog_color,
        falloff: bevy::pbr::FogFalloff::Linear {
            start: near,
            end: far,
        },
        ..default()
    };
    for (entity, existing) in cameras.iter_mut() {
        match existing {
            Some(mut distance_fog) => *distance_fog = fog.clone(),
            None => {
                commands.entity(entity).insert(fog.clone());
            }
        }
    }
    // Interiors have no sky: the clear color is the fog color.
    clear_color.0 = fog_color;
    ambient.color = argb(block.unk_uint1);
    ambient.brightness = DUNGEON_AMBIENT_BRIGHTNESS;
}

/// Periodic one-line perf digest while a dungeon is active, so playtest
/// reports carry numbers: FPS, how many blocks/lights actually render, and
/// the current block's fog reach (the culling radius).
pub fn log_dungeon_perf(
    time: Res<Time>,
    mut last: Local<f32>,
    active: Option<Res<ActiveDungeon>>,
    dofs: Res<Assets<JMXVDOF>>,
    diagnostics: Option<Res<bevy::diagnostic::DiagnosticsStore>>,
    blocks: Query<&Visibility, With<DungeonBlock>>,
    lights: Query<&ViewVisibility, With<DungeonLight>>,
    effects: Query<
        Has<crate::plugins::effects::components::EffectSimPaused>,
        With<crate::plugins::effects::components::EffectInstance>,
    >,
) {
    let Some(active) = active else { return };
    if time.elapsed_secs() - *last < 2.0 {
        return;
    }
    *last = time.elapsed_secs();

    let fps = diagnostics
        .as_ref()
        .and_then(|store| store.get(&bevy::diagnostic::FrameTimeDiagnosticsPlugin::FPS))
        .and_then(|diag| diag.smoothed())
        .unwrap_or(0.0);
    let shown_blocks = blocks
        .iter()
        .filter(|visibility| **visibility != Visibility::Hidden)
        .count();
    let lit = lights.iter().filter(|view| view.get()).count();
    let fog = active
        .current_block
        .and_then(|index| dofs.get(&active.dof).and_then(|dof| dof.blocks.get(index)))
        .map(fog_far);
    let effects_active = effects.iter().filter(|paused| !paused).count();
    info!(
        "dungeon perf: fps {fps:.0} · blocks shown {shown_blocks}/{} · lights lit {lit}/{} · effects simulating {effects_active}/{} · current block {:?} fog_far {:?}",
        blocks.iter().count(),
        lights.iter().count(),
        effects.iter().count(),
        active.current_block,
        fog,
    );
}

/// A DOF-packed 0xAARRGGBB color.
fn argb(packed: u32) -> Color {
    Color::srgb_u8(
        ((packed >> 16) & 0xFF) as u8,
        ((packed >> 8) & 0xFF) as u8,
        (packed & 0xFF) as u8,
    )
}
