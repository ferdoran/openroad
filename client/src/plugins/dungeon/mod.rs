//! Dungeon runtime (EP-18 / #100, ADR-0008): loads a `JMXVDOF` interior and
//! spawns its room blocks through the existing `.bsr` resource chain, with
//! per-block point lights, per-block fog, and portal culling from
//! `VisibleBlockIndices`. Scene-agnostic: anything (the `dungeons` test
//! scene, offline world-scene gates) enters by sending [`EnterDungeon`];
//! while [`ActiveDungeon`] exists the overworld streamer and ENVI
//! environment stand down.
//!
//! Coordinate convention (the one place it is defined): DOF data is in the
//! original client's dungeon-local left-handed frame. Our render/SRO space
//! mirrors world X (same convention as the overworld: region x → `-x`), so a
//! raw D3D placement matrix `A` renders as `MIRROR_X · A` — translations get
//! their X negated and Y-rotations conjugate automatically; the resulting
//! negative determinant is what `needs_winding_reversal`/`MirroredResource`
//! already handle on the overworld path. Blocks place at
//! `Translate(Position) · RotY(-Yaw)` (JMX-File-Editor/RSBot), props at
//! `Translate · Euler(YXZ) · Scale` within their block, so child entities of
//! a block root simply carry the *raw* local matrices.

pub mod atmosphere;
pub mod gates;
pub mod spawn;

use bevy::prelude::*;

use crate::assets::dof::format::{DofBlock, DofBlockObject};
use crate::assets::dof::JMXVDOF;
use crate::scenes::SceneState;
use crate::GameState;

pub struct DungeonPlugin;

impl Plugin for DungeonPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<EnterDungeon>()
            .add_message::<LeaveDungeon>()
            .add_systems(
                Update,
                (
                    spawn::begin_enter_dungeon,
                    spawn::spawn_dungeon_when_loaded,
                    spawn::snap_arrival_to_floor,
                    spawn::leave_dungeon,
                    crate::plugins::nav::dungeon::maintain_dungeon_nav,
                    spawn::hide_terrain_in_dungeon,
                    atmosphere::resolve_current_block,
                    atmosphere::portal_culling,
                    atmosphere::light_budget,
                    atmosphere::apply_dungeon_atmosphere,
                    atmosphere::log_dungeon_perf,
                )
                    .chain()
                    .run_if(in_state(GameState::Game)),
            )
            .add_systems(
                Update,
                (
                    gates::sync_gate_circles,
                    gates::update_gate_transforms,
                    gates::trigger_gates,
                )
                    .chain()
                    .run_if(in_state(GameState::Game))
                    .run_if(gates::offline_gate_scene),
            )
            .add_systems(OnExit(SceneState::Dungeons), spawn::cleanup_dungeon);
    }
}

/// Request to enter a dungeon: despawns any active dungeon, loads the DOF for
/// `region_id` (via `dungeoninfo.txt`), re-anchors the world origin, spawns
/// the interior and places the local player at `arrival`.
#[derive(Message, Clone, Copy, Debug)]
pub struct EnterDungeon {
    /// Dungeon-flagged region id (`0x8000 | dungeoninfo id`).
    pub region_id: u16,
    /// Arrival point in raw dungeon-local coordinates (as found in
    /// `teleportdata.txt` dungeon rows, before the X mirror). `None` places
    /// the player at the dungeon's entrance block (`IsEntrance`), or the
    /// first block when none is marked.
    pub arrival: Option<Vec3>,
}

/// Request to leave the active dungeon back to an overworld position
/// (SRO-space, mirrored-X convention).
#[derive(Message, Clone, Copy, Debug)]
pub struct LeaveDungeon {
    pub arrival_sro: Vec3,
}

/// Present while a dungeon interior is active. Its existence gates the
/// overworld streamer and the ENVI environment off (ADR-0008).
#[derive(Resource)]
pub struct ActiveDungeon {
    pub region_id: u16,
    /// Root entity of the spawned interior (blocks are its children).
    pub root: Entity,
    pub dof: Handle<JMXVDOF>,
    /// Block index the player currently stands in (bbox resolve until the
    /// nav runtime provides a surface-accurate one).
    pub current_block: Option<usize>,
}

/// One spawned dungeon room block (child of the dungeon root).
#[derive(Component)]
pub struct DungeonBlock {
    pub index: usize,
}

/// The mirror between the raw D3D frame and our render/SRO frame.
pub const MIRROR_X: Vec3 = Vec3::new(-1.0, 1.0, 1.0);

/// A raw dungeon-local point in our SRO/render space (before origin shift).
pub fn dof_local_to_sro(p: Vec3) -> Vec3 {
    Vec3::new(-p.x, p.y, p.z)
}

/// A block's placement matrix in the raw D3D dungeon frame:
/// `Translate(Position) · RotY(-Yaw)`.
pub fn raw_block_matrix(block: &DofBlock) -> Mat4 {
    Mat4::from_translation(block.position) * Mat4::from_rotation_y(-block.yaw)
}

/// A prop's placement matrix within its block, raw frame. The Euler order is
/// the D3D yaw/pitch/roll idiom (rotation.y about Y first) — an [S]
/// assumption; most props only rotate about Y.
pub fn raw_prop_matrix(obj: &DofBlockObject) -> Mat4 {
    Mat4::from_translation(obj.position)
        * Mat4::from_quat(Quat::from_euler(
            EulerRot::YXZ,
            obj.rotation.y,
            obj.rotation.x,
            obj.rotation.z,
        ))
        * Mat4::from_scale(obj.scale)
}

/// Lift a raw D3D matrix into our mirrored render/SRO frame.
pub fn mirror_matrix(raw: Mat4) -> Mat4 {
    Mat4::from_scale(MIRROR_X) * raw
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::assets::dof::format::FogParam;

    fn block_at(position: Vec3, yaw: f32) -> DofBlock {
        DofBlock {
            path: String::new(),
            name: String::new(),
            unk_uint0: 0,
            position,
            yaw,
            is_entrance: 0,
            collision_box: Default::default(),
            unk_uint1: 0,
            fog: FogParam {
                color: 0,
                near_plane: 0.0,
                far_plane: 0.0,
                intensity: 0.0,
                height_fog: None,
            },
            unk_byte1: 0,
            unk_byte1_payload: None,
            unk_string: String::new(),
            room_index: 0,
            floor_index: 0,
            connected_block_indices: vec![],
            visible_block_indices: vec![],
            collision_object_count: 0,
            objects: vec![],
            lights: vec![],
        }
    }

    #[test]
    fn block_matrix_mirrors_translation_and_conjugates_yaw() {
        use std::f32::consts::FRAC_PI_2;
        // A block at raw (100, 5, 200) rotated -90° (raw frame): its local
        // +X axis maps to raw -Z ... mirrored into render space the
        // translation X negates and the rotation conjugates.
        let block = block_at(Vec3::new(100.0, 5.0, 200.0), FRAC_PI_2);
        let world = mirror_matrix(raw_block_matrix(&block));
        // Origin of the block lands at the mirrored position.
        assert!(
            (world.transform_point3(Vec3::ZERO) - Vec3::new(-100.0, 5.0, 200.0)).length() < 1e-4
        );
        // A point 10 units along raw local +X: raw yaw of -π/2 about Y takes
        // +X to +Z (RH math on raw values: rot(-π/2)·(10,0,0) = (0,0,10));
        // the mirror leaves Z alone.
        let p = world.transform_point3(Vec3::new(10.0, 0.0, 0.0));
        assert!((p - Vec3::new(-100.0, 5.0, 210.0)).length() < 1e-4);
        // The mirror flips handedness — winding reversal must trigger.
        assert!(world.determinant() < 0.0);
    }

    #[test]
    fn sro_conversion_matches_matrix_path() {
        let p = Vec3::new(1011.0, 0.0, -862.0); // GATE_DUNGEON_DH_OUT arrival
        assert_eq!(dof_local_to_sro(p), Vec3::new(-1011.0, 0.0, -862.0));
        let via_matrix = mirror_matrix(Mat4::IDENTITY).transform_point3(p);
        assert_eq!(dof_local_to_sro(p), via_matrix);
    }
}
