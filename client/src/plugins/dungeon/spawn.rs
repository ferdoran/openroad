//! Dungeon enter/leave flow and interior spawning.
//!
//! Entering is a two-step async load: [`begin_enter_dungeon`] resolves the
//! region id through `dungeoninfo.txt` and starts the DOF load;
//! [`spawn_dungeon_when_loaded`] then re-anchors the world origin on the
//! arrival point (unsnapped — ADR-0006 amendment), spawns one root entity
//! with a child per block (each block's `.bsr` room resource through the
//! existing `UnloadedResource` chain, which also attaches `ObjectNavMesh`
//! from the room's nav sections), fans out props and point lights as raw
//! local children (see the module doc in `mod.rs` for the mirror math), and
//! finally places the local player.

use bevy::prelude::*;

use super::{
    dof_local_to_sro, mirror_matrix, raw_block_matrix, raw_prop_matrix, ActiveDungeon,
    DungeonBlock, EnterDungeon, LeaveDungeon,
};
use crate::assets::dof::format::DofBlockLight;
use crate::assets::dof::JMXVDOF;
use crate::plugins::dynamic_resource_loader::{MirroredResource, UnloadedResource};
use crate::plugins::map::terrain::Terrain;
use crate::plugins::nav::NavLocation;
use crate::plugins::player::{Player, PlayerCommands};
use crate::plugins::textdata::ClientDungeonInfo;
use crate::plugins::world_origin::{set_dungeon_origin, set_world_origin, WorldOrigin};
use crate::util::mesh::needs_winding_reversal;

/// A dungeon enter in flight: the DOF is still loading.
#[derive(Resource)]
pub struct PendingDungeon {
    pub region_id: u16,
    pub handle: Handle<JMXVDOF>,
    /// Raw dungeon-local arrival point; `None` = entrance block.
    pub arrival: Option<Vec3>,
}

/// Root marker of the spawned interior.
#[derive(Component)]
pub struct DungeonRoot;

/// Marks a spawned DOF block light (direct child of its block root) — the
/// light-budget system ranks and toggles these.
#[derive(Component)]
pub struct DungeonLight;

/// An arrival waiting to be snapped onto the room floor: the block `.bms`
/// nav meshes load async, so the downward probe retries every frame,
/// widening a spiral around the arrival when the point itself has no floor
/// under it (teleportdata rows that sit beside the geometry).
#[derive(Resource)]
pub struct PendingArrivalSnap {
    /// Arrival point in render space.
    pub center: Vec3,
    pub attempts: u32,
}

pub fn begin_enter_dungeon(
    mut requests: MessageReader<EnterDungeon>,
    dungeon_info: Res<ClientDungeonInfo>,
    asset_server: Res<AssetServer>,
    active: Option<Res<ActiveDungeon>>,
    mut commands: Commands,
) {
    let Some(request) = requests.read().last().copied() else {
        return;
    };
    let Some(entry) = dungeon_info.by_region(request.region_id) else {
        warn!(
            "dungeon: region {:#06x} has no dungeoninfo.txt entry — ignoring enter request",
            request.region_id
        );
        return;
    };
    // Dungeon→dungeon switch: tear the current interior down first. A
    // still-pending arrival snap belongs to the old origin — drop it.
    if let Some(active) = active {
        if let Ok(mut root) = commands.get_entity(active.root) {
            root.despawn();
        }
        commands.remove_resource::<ActiveDungeon>();
    }
    commands.remove_resource::<PendingArrivalSnap>();
    info!(
        "dungeon: entering {} (region {:#06x}, {})",
        entry.name(),
        request.region_id,
        entry.asset_path()
    );
    commands.insert_resource(PendingDungeon {
        region_id: request.region_id,
        handle: asset_server.load(entry.asset_path()),
        arrival: request.arrival,
    });
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_dungeon_when_loaded(
    pending: Option<Res<PendingDungeon>>,
    dofs: Res<Assets<JMXVDOF>>,
    asset_server: Res<AssetServer>,
    mut origin: ResMut<WorldOrigin>,
    mut terrain: Query<&mut Transform, (With<Terrain>, Without<Player>)>,
    mut player: Query<(&mut Transform, &mut NavLocation), With<Player>>,
    mut player_commands: ResMut<PlayerCommands>,
    mut commands: Commands,
) {
    let Some(pending) = pending else { return };
    if asset_server.load_state(&pending.handle).is_failed() {
        warn!("dungeon: DOF failed to load — abandoning enter");
        commands.remove_resource::<PendingDungeon>();
        return;
    }
    let Some(dof) = dofs.get(&pending.handle) else {
        return;
    };

    // Anchor the origin on the arrival point (unsnapped: dungeon frame has
    // no 1920 grid and no splat terrain) BEFORE placing anything. The
    // fallback is the entrance (or first) block's collision-box center —
    // inside the room by construction, unlike the block's origin point.
    // `CollisionBox0` is BLOCK-LOCAL, so the center must be lifted through
    // the block placement into the dungeon frame.
    let arrival_raw = pending.arrival.unwrap_or_else(|| {
        dof.blocks
            .iter()
            .find(|block| block.is_entrance != 0)
            .or_else(|| dof.blocks.first())
            .map(|block| {
                let bb = &block.collision_box;
                let local_center = Vec3::new(
                    (bb.min.x + bb.max.x) * 0.5,
                    bb.min.y,
                    (bb.min.z + bb.max.z) * 0.5,
                );
                super::raw_block_matrix(block).transform_point3(local_center)
            })
            .unwrap_or(Vec3::ZERO)
    });
    let arrival_sro = dof_local_to_sro(arrival_raw);
    set_dungeon_origin(arrival_sro, &mut origin, &mut terrain);

    let root = commands
        .spawn((
            Transform::IDENTITY,
            Visibility::default(),
            DungeonRoot,
            Name::new(format!("dungeon {:#06x}", pending.region_id)),
        ))
        .id();

    for (index, block) in dof.blocks.iter().enumerate() {
        let world_mat = mirror_matrix(raw_block_matrix(block));
        let mut transform = Transform::from_matrix(world_mat);
        transform.translation = origin.to_render(transform.translation);

        let mut block_commands = commands.spawn((
            transform,
            Visibility::default(),
            DungeonBlock { index },
            Name::new(format!("block {index} {}", block.name)),
            UnloadedResource(asset_server.load(data_path(&block.path))),
            ChildOf(root),
        ));
        if needs_winding_reversal(&world_mat) {
            block_commands.insert(MirroredResource);
        }
        let block_entity = block_commands.id();

        for obj in &block.objects {
            // Collision-only props are invisible walls: their circles feed
            // the nav runtime straight from the DOF data, nothing to draw.
            if obj.is_collision_only() {
                continue;
            }
            // Children of the block root carry the *raw* local matrix — the
            // parent's mirrored matrix lifts them into render space.
            let raw = raw_prop_matrix(obj);
            let mut prop = commands.spawn((
                Transform::from_matrix(raw),
                Visibility::default(),
                Name::new(format!("prop {}", obj.name)),
                UnloadedResource(asset_server.load(data_path(&obj.path))),
                ChildOf(block_entity),
            ));
            // Winding is decided by the full world-space matrix.
            if needs_winding_reversal(&(world_mat * raw)) {
                prop.insert(MirroredResource);
            }
            // Water props (flag 4) currently render through their authored
            // .bsr materials; WaterColor tinting via the HQ water material
            // is a fidelity follow-up (ADR-0008).
        }

        for light in &block.lights {
            commands.spawn((
                Transform::from_translation(light.position),
                Visibility::default(),
                point_light(light),
                DungeonLight,
                Name::new(format!("light {}", light.name)),
                ChildOf(block_entity),
            ));
        }
    }

    // Place the player on the arrival point; the surface resolve happens on
    // the first movement query (NavLocation::Unresolved recovery path). The
    // authored arrival Y is unreliable (teleportdata rows and box centers
    // both float above or below the actual floor, and nav resolution only
    // finds surfaces within its stand tolerance), so a snap-to-floor probe
    // keeps running until the room's nav meshes have streamed in.
    let arrival_render = origin.to_render(arrival_sro);
    if let Ok((mut transform, mut nav_location)) = player.single_mut() {
        transform.translation = arrival_render;
        *nav_location = NavLocation::Unresolved;
    }
    commands.insert_resource(PendingArrivalSnap {
        center: arrival_render,
        attempts: 0,
    });
    player_commands.stop();

    info!(
        "dungeon: spawned {} blocks, grid {}x{}x{} ({} voxels)",
        dof.blocks.len(),
        dof.grid.width,
        dof.grid.height,
        dof.grid.length,
        dof.grid.voxels.len()
    );
    commands.insert_resource(ActiveDungeon {
        region_id: pending.region_id,
        root,
        dof: pending.handle.clone(),
        current_block: None,
    });
    commands.remove_resource::<PendingDungeon>();
}

pub fn leave_dungeon(
    mut requests: MessageReader<LeaveDungeon>,
    active: Option<Res<ActiveDungeon>>,
    mut origin: ResMut<WorldOrigin>,
    mut terrain: Query<&mut Transform, (With<Terrain>, Without<Player>)>,
    mut player: Query<(&mut Transform, &mut NavLocation), With<Player>>,
    mut player_commands: ResMut<PlayerCommands>,
    mut commands: Commands,
) {
    let Some(request) = requests.read().last().copied() else {
        return;
    };
    let Some(active) = active else { return };
    if let Ok(mut root) = commands.get_entity(active.root) {
        root.despawn();
    }
    commands.remove_resource::<ActiveDungeon>();
    commands.remove_resource::<PendingArrivalSnap>();
    // Back to the overworld: grid-snapped anchor, normal streaming resumes.
    set_world_origin(request.arrival_sro, &mut origin, &mut terrain);
    if let Ok((mut transform, mut nav_location)) = player.single_mut() {
        transform.translation = origin.to_render(request.arrival_sro);
        *nav_location = NavLocation::Unresolved;
    }
    player_commands.stop();
    info!("dungeon: left to overworld at {:?}", request.arrival_sro);
}

/// Scene teardown: drop the interior and the gate state with the scene.
pub fn cleanup_dungeon(
    active: Option<Res<ActiveDungeon>>,
    pending: Option<Res<PendingDungeon>>,
    mut commands: Commands,
) {
    if let Some(active) = active {
        if let Ok(mut root) = commands.get_entity(active.root) {
            root.despawn();
        }
        commands.remove_resource::<ActiveDungeon>();
    }
    if pending.is_some() {
        commands.remove_resource::<PendingDungeon>();
    }
    commands.remove_resource::<PendingArrivalSnap>();
}

/// How high above the arrival the floor probe starts. Rooms are placed at
/// arbitrary Y; the probe must clear multi-storey blocks above the arrival.
const SNAP_PROBE_HEIGHT: f32 = 2000.0;
/// Attempts (frames) per spiral ring before widening the search.
const SNAP_ATTEMPTS_PER_RING: u32 = 30;
/// Give up after this many attempts (~5 s at 60 fps, ~20 at 15 fps floors).
const SNAP_MAX_ATTEMPTS: u32 = 300;
/// Ring spacing of the widening spiral, world units.
const SNAP_RING_STEP: f32 = 100.0;
const SNAP_MAX_RING: u32 = 3;

/// Probe points for the current attempt: the arrival itself, then — once
/// the point has repeatedly found no floor (meshes are loaded but nothing is
/// under it) — 8 compass offsets on a widening ring around it.
fn snap_probe_offsets(attempts: u32) -> Vec<Vec2> {
    let ring = (attempts / SNAP_ATTEMPTS_PER_RING).min(SNAP_MAX_RING);
    let mut offsets = vec![Vec2::ZERO];
    for r in 1..=ring {
        let radius = r as f32 * SNAP_RING_STEP;
        for step in 0..8 {
            let angle = step as f32 * std::f32::consts::FRAC_PI_4;
            offsets.push(Vec2::from_angle(angle) * radius);
        }
    }
    offsets
}

/// Land the player on the nearest walkable floor around the arrival point.
/// Runs while [`PendingArrivalSnap`] exists; the room nav meshes stream in
/// async, so early frames legitimately find nothing.
pub fn snap_arrival_to_floor(
    snap: Option<ResMut<PendingArrivalSnap>>,
    nav: crate::plugins::nav::NavMeshRaycast,
    mut player: Query<(&mut Transform, &mut NavLocation), With<Player>>,
    mut commands: Commands,
) {
    let Some(mut snap) = snap else { return };
    let Ok((mut transform, mut nav_location)) = player.single_mut() else {
        return;
    };
    snap.attempts += 1;
    for offset in snap_probe_offsets(snap.attempts) {
        let probe = snap.center + Vec3::new(offset.x, SNAP_PROBE_HEIGHT, offset.y);
        let ray = Ray3d::new(probe, Dir3::NEG_Y);
        if let Some(hit) = nav.cast_walkable(&ray) {
            transform.translation = hit.point + Vec3::Y * 0.5;
            *nav_location = NavLocation::Unresolved;
            info!(
                "dungeon: arrival snapped to floor at {:?} (offset {:?}, attempt {})",
                hit.point, offset, snap.attempts
            );
            commands.remove_resource::<PendingArrivalSnap>();
            return;
        }
    }
    if snap.attempts >= SNAP_MAX_ATTEMPTS {
        warn!(
            "dungeon: no walkable floor found around the arrival after {} attempts — leaving the player as placed",
            snap.attempts
        );
        commands.remove_resource::<PendingArrivalSnap>();
    }
}

/// Hide overworld terrain roots while a dungeon is active (their render
/// positions are hundreds of kilo-units off after the dungeon origin
/// re-anchor; fog mostly swallows them but hiding is exact) and restore them
/// on leave — the resumed streamer re-evaluates their visibility anyway.
pub fn hide_terrain_in_dungeon(
    active: Option<Res<ActiveDungeon>>,
    mut terrain: Query<&mut Visibility, With<Terrain>>,
    mut was_active: Local<bool>,
) {
    let is_active = active.is_some();
    if is_active == *was_active {
        return;
    }
    *was_active = is_active;
    let target = if is_active {
        Visibility::Hidden
    } else {
        Visibility::Inherited
    };
    for mut visibility in terrain.iter_mut() {
        *visibility = target;
    }
}

/// A DOF-stored path (`Dungeon\china\room.bsr`) as a `data://` asset path.
fn data_path(dof_path: &str) -> String {
    format!("data://{}", dof_path.replace('\\', "/").to_lowercase())
}

/// Map a DOF block light onto a Bevy point light.
///
/// Approximation (documented in ADR-0008): D3D lights attenuate as
/// `1 / (a0 + a1·d + a2·d²)` with no hard range; Bevy point lights need one.
/// The range is where attenuation falls to [`ATTENUATION_CUTOFF`], solved
/// from the stored coefficients; intensity scales with the diffuse color's
/// magnitude. Constants are calibration points, not PK2 data.
fn point_light(light: &DofBlockLight) -> PointLight {
    let color = Color::srgb(
        light.diffuse.x.clamp(0.0, 1.0),
        light.diffuse.y.clamp(0.0, 1.0),
        light.diffuse.z.clamp(0.0, 1.0),
    );
    PointLight {
        color,
        intensity: LIGHT_INTENSITY * light.diffuse.length().min(2.0),
        range: attenuation_range(light.attenuation),
        shadow_maps_enabled: false,
        ..default()
    }
}

/// Attenuation value below which a light no longer contributes visibly.
const ATTENUATION_CUTOFF: f32 = 0.02;
/// Base lumen output per unit of diffuse magnitude — visual calibration.
const LIGHT_INTENSITY: f32 = 400_000.0;
const LIGHT_RANGE_FALLBACK: f32 = 600.0;

fn attenuation_range([a0, a1, a2]: [f32; 3]) -> f32 {
    // Solve a2·r² + a1·r + a0 = 1/cutoff for r.
    let target = 1.0 / ATTENUATION_CUTOFF - a0;
    let range = if a2 > f32::EPSILON {
        let disc = a1 * a1 + 4.0 * a2 * target;
        if disc > 0.0 {
            (-a1 + disc.sqrt()) / (2.0 * a2)
        } else {
            LIGHT_RANGE_FALLBACK
        }
    } else if a1 > f32::EPSILON {
        target / a1
    } else {
        LIGHT_RANGE_FALLBACK
    };
    range.clamp(100.0, 3000.0)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn data_path_normalizes() {
        assert_eq!(
            data_path(r"Dungeon\wchina\Dunhwang_Cv.bsr"),
            "data://dungeon/wchina/dunhwang_cv.bsr"
        );
    }

    #[test]
    fn snap_spiral_widens_with_attempts() {
        // First ring window: only the arrival point itself.
        assert_eq!(snap_probe_offsets(1), vec![Vec2::ZERO]);
        assert_eq!(snap_probe_offsets(SNAP_ATTEMPTS_PER_RING - 1).len(), 1);
        // One ring: center + 8 compass points at 100 units.
        let one_ring = snap_probe_offsets(SNAP_ATTEMPTS_PER_RING);
        assert_eq!(one_ring.len(), 9);
        assert!((one_ring[1].length() - SNAP_RING_STEP).abs() < 1e-3);
        // Ring growth is capped.
        let capped = snap_probe_offsets(SNAP_MAX_ATTEMPTS * 2);
        assert_eq!(capped.len(), 1 + 8 * SNAP_MAX_RING as usize);
    }

    #[test]
    fn attenuation_range_shapes() {
        // Pure linear falloff: r = (1/cutoff - a0) / a1.
        assert!((attenuation_range([1.0, 0.049, 0.0]) - 1000.0).abs() < 1.0);
        // No falloff coefficients → fallback.
        assert_eq!(attenuation_range([1.0, 0.0, 0.0]), LIGHT_RANGE_FALLBACK);
        // Quadratic dominates and stays within the clamp.
        let r = attenuation_range([0.0, 0.0, 0.001]);
        assert!((100.0..=3000.0).contains(&r));
    }
}
