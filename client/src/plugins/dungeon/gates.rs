//! Offline dungeon gate circles: the walk-in teleport areas from
//! `teleportdata.txt`, wired for the offline `World` and `Dungeons` scenes.
//!
//! Idea: the 114 owner-ref-0 teleportdata rows are standalone circle areas
//! (no NPC, no gate building) — dungeon entrances in the overworld
//! (`GATE_DUNGEON_DH_IN` west of Donwhang, `GATE_JINSI_OUT` east of Jangan),
//! the matching exit gates inside, and jinsi's floor-to-floor gates. The
//! gate set is data-driven: in the overworld every ref-0 gate whose links
//! lead into a dungeon spawns; inside a dungeon every ref-0 gate placed in
//! that region spawns. Walking into the circle resolves the teleportlink and
//! fires [`EnterDungeon`]/[`LeaveDungeon`]. A gate only arms once the player
//! has been *outside* its circle, so arriving on a return gate doesn't
//! ping-pong straight back.
//!
//! The flat circle visual is a non-original placeholder (the original shows
//! a portal glow effect) behind `graphics.dungeon.gate_circles`. Networked
//! gate use (0x705A in `GameWorld`) is out of scope here — ADR-0008.

use bevy::prelude::*;

use super::{ActiveDungeon, EnterDungeon, LeaveDungeon};
use crate::assets::textdata::teleport::TeleportTable;
use crate::plugins::config::ClientConfig;
use crate::plugins::player::Player;
use crate::plugins::textdata::ClientTeleport;
use crate::plugins::world_origin::WorldOrigin;
use crate::scenes::game_scene::server_position_to_sro;
use crate::scenes::SceneState;
use crate::util::region::RegionIdExt;

/// A spawned gate trigger circle.
#[derive(Component)]
pub struct GateCircle {
    pub teleporter_id: u32,
    /// Center in SRO space (mirrored-X convention); the render transform is
    /// re-derived from this every frame so origin re-anchors can't strand it.
    pub sro: Vec3,
    pub radius: f32,
    /// Armed once the player has been outside the circle; a trigger fires on
    /// the outside→inside transition only.
    pub armed: bool,
}

/// Where the current gate set was spawned for: `None` = overworld,
/// `Some(region)` = inside that dungeon.
#[derive(Default)]
pub struct GateContext(Option<Option<u16>>);

/// Offline scenes where walking into a gate teleports client-side. The
/// networked `GameWorld` flow (0x705A) is deliberately excluded.
pub fn offline_gate_scene(state: Res<State<SceneState>>) -> bool {
    matches!(**state, SceneState::WorldSandbox | SceneState::Dungeons)
}

/// (Re)spawn the gate set whenever the context (overworld ↔ specific
/// dungeon) changes.
#[allow(clippy::too_many_arguments)]
pub fn sync_gate_circles(
    mut context: Local<GateContext>,
    active: Option<Res<ActiveDungeon>>,
    teleport: Res<ClientTeleport>,
    config: Res<ClientConfig>,
    existing: Query<Entity, With<GateCircle>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
) {
    let Some(table) = teleport.table() else {
        return;
    };
    let current = active.as_ref().map(|a| a.region_id);
    if context.0 == Some(current) {
        return;
    }
    context.0 = Some(current);

    for entity in existing.iter() {
        commands.entity(entity).despawn();
    }

    let show_circles = config.graphics.dungeon.gate_circles;
    let visual = show_circles.then(|| {
        (
            // A ring at 85-100% of the trigger radius, like a painted circle.
            meshes.add(Mesh::from(Annulus::new(0.85, 1.0))),
            materials.add(StandardMaterial {
                base_color: Color::srgba(0.35, 0.75, 1.0, 0.35),
                alpha_mode: AlphaMode::Blend,
                unlit: true,
                double_sided: true,
                cull_mode: None,
                ..default()
            }),
        )
    });

    let mut spawned = 0;
    for (&id, info) in &table.info {
        if !gate_belongs(table, info.owner_ref, info.region, id, current) {
            continue;
        }
        let sro = server_position_to_sro(
            info.region,
            info.position.x,
            info.position.y,
            info.position.z,
        );
        let mut gate = commands.spawn((
            GateCircle {
                teleporter_id: id,
                sro,
                radius: info.radius.max(10.0),
                armed: false,
            },
            // Transform is set from `sro` every frame by
            // `update_gate_transforms`.
            Transform::default(),
            Visibility::default(),
            Name::new(format!("gate {} {}", id, info.codename)),
        ));
        if let Some((mesh, material)) = &visual {
            let radius = info.radius.max(10.0);
            gate.with_children(|parent| {
                parent.spawn((
                    Mesh3d(mesh.clone()),
                    MeshMaterial3d(material.clone()),
                    // The annulus lies in XY; lay it flat, slightly lifted
                    // against z-fighting, scaled to the trigger radius.
                    Transform::from_xyz(0.0, 1.0, 0.0)
                        .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2))
                        .with_scale(Vec3::splat(radius)),
                ));
            });
        }
        spawned += 1;
    }
    info!(
        "dungeon gates: spawned {spawned} circle(s) for {}",
        match current {
            None => String::from("overworld"),
            Some(region) => format!("dungeon {region:#06x}"),
        }
    );
}

/// Whether a teleportdata row spawns as a circle gate in the given context:
/// owner-ref-0 rows only; in a dungeon, the rows placed in that region; in
/// the overworld, the rows whose links lead into any dungeon.
fn gate_belongs(
    table: &TeleportTable,
    owner_ref: i32,
    region: u16,
    id: u32,
    context: Option<u16>,
) -> bool {
    if owner_ref != 0 {
        return false;
    }
    match context {
        Some(dungeon_region) => region == dungeon_region,
        None => {
            !region.is_dungeon()
                && region != 0
                && table.links.get(&id).is_some_and(|links| {
                    links.iter().any(|link| {
                        table
                            .info
                            .get(&link.destination)
                            .is_some_and(|dest| dest.region.is_dungeon())
                    })
                })
        }
    }
}

/// Keep gate render transforms derived from their SRO centers (the world
/// origin moves on every dungeon enter/leave).
pub fn update_gate_transforms(
    origin: Res<WorldOrigin>,
    mut gates: Query<(&GateCircle, &mut Transform)>,
) {
    for (gate, mut transform) in gates.iter_mut() {
        let target = origin.to_render(gate.sro);
        if transform.translation != target {
            transform.translation = target;
        }
    }
}

/// Fire a gate when the player walks into its circle (outside→inside
/// transition), resolving the teleportlink destination client-side.
pub fn trigger_gates(
    mut gates: Query<(&mut GateCircle, &Transform)>,
    player: Query<&Transform, With<Player>>,
    teleport: Res<ClientTeleport>,
    mut enter: MessageWriter<EnterDungeon>,
    mut leave: MessageWriter<LeaveDungeon>,
) {
    let Ok(player) = player.single() else { return };
    let Some(table) = teleport.table() else {
        return;
    };
    for (mut gate, transform) in gates.iter_mut() {
        let inside = (player.translation - transform.translation).xz().length() <= gate.radius;
        if !inside {
            gate.armed = true;
            continue;
        }
        if !gate.armed {
            continue;
        }
        gate.armed = false;
        let Some(destination) = table
            .links
            .get(&gate.teleporter_id)
            .and_then(|links| links.first())
            .and_then(|link| table.info.get(&link.destination))
        else {
            warn!(
                "dungeon gate {}: no linked destination — ignoring",
                gate.teleporter_id
            );
            continue;
        };
        info!(
            "dungeon gate {}: teleporting to {} (region {:#06x})",
            gate.teleporter_id, destination.codename, destination.region
        );
        if destination.region.is_dungeon() {
            enter.write(EnterDungeon {
                region_id: destination.region,
                // Dungeon rows carry raw dungeon-local coordinates.
                arrival: Some(destination.position),
            });
        } else {
            leave.write(LeaveDungeon {
                arrival_sro: server_position_to_sro(
                    destination.region,
                    destination.position.x,
                    destination.position.y,
                    destination.position.z,
                ),
            });
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::assets::textdata::teleport::{TeleportInfo, TeleportLink};

    fn info(owner_ref: i32, region: u16) -> TeleportInfo {
        TeleportInfo {
            codename: String::new(),
            name_key: String::new(),
            owner_ref,
            region,
            position: Vec3::ZERO,
            radius: 50.0,
        }
    }

    /// The AC pairs: overworld entry gates spawn in the overworld context,
    /// exit gates in their dungeon's context, NPC-owned teleporters never.
    #[test]
    fn gate_context_selection() {
        let mut table = TeleportTable::default();
        // 11 = GATE_DUNGEON_DH_IN (overworld) ⇄ 10 = GATE_DUNGEON_DH_OUT
        // (inside 0x8001); 55/56 = the jinsi pair; 1 = an NPC teleporter.
        table.info.insert(11, info(0, 27027));
        table.info.insert(10, info(0, 0x8001));
        table.info.insert(55, info(0, 26284));
        table.info.insert(56, info(0, (-32761i32) as u16));
        table.info.insert(1, info(2094, 25000));
        let link = |src: u32, dst: u32| {
            (
                src,
                vec![TeleportLink {
                    destination: dst,
                    min_level: None,
                    // Dungeon gates in this fixture are free; the fee column
                    // only matters to the teleport board's price line.
                    fee: 0,
                }],
            )
        };
        table.links.extend([
            link(11, 10),
            link(10, 11),
            link(55, 56),
            link(56, 55),
            link(1, 55),
        ]);

        // Overworld: both entry gates, no dungeon-placed rows, no NPC rows.
        assert!(gate_belongs(&table, 0, 27027, 11, None));
        assert!(gate_belongs(&table, 0, 26284, 55, None));
        assert!(!gate_belongs(&table, 0, 0x8001, 10, None));
        assert!(!gate_belongs(&table, 2094, 25000, 1, None));

        // Donwhang cave: only its own exit gate.
        assert!(gate_belongs(&table, 0, 0x8001, 10, Some(0x8001)));
        assert!(!gate_belongs(&table, 0, 27027, 11, Some(0x8001)));
        assert!(!gate_belongs(
            &table,
            0,
            (-32761i32) as u16,
            56,
            Some(0x8001)
        ));
    }
}
