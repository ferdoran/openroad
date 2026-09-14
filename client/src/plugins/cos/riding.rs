//! Mount/dismount transitions, rider transform slaving, mounted movement and
//! ground alignment.
//!
//! Idea: the server moves the COS entity, never the rider — so riding is (a)
//! routing the player's move orders to the COS (0x70C5 online, the COS's own
//! `RemoteMovement` offline) and (b) copying the COS transform onto every
//! rider each frame, after all movement systems ran. The copy is pure
//! render-space entity-to-entity, so no `WorldOrigin` math is involved.
//!
//! The rider sits on the mount's **`saddle` bone**: every rideable COS
//! skeleton in the 1.188 corpus carries one (`c_horse` bind position
//! `(0.00, 17.32, 0.27)`, parented to `Bip01 Spine`, so it bobs with the gait)
//! — see `docs/re/systems/mount.md`. We *sample* that bone rather than
//! parenting the rider under it: the body wrapper carries the SRO X-mirror
//! (`scale.x = -1`), which would mirror the rider, and a parented rider would
//! die with the mount.
//!
//! Mounts and pets also pitch/roll onto the ground. Nothing in the nav stack
//! exposes a surface normal, so [`align_cos_to_slope`] derives one by central
//! differences over `NavMeshRaycast::ground()` at the terrain grid step — the
//! same stencil the rendered terrain bakes its own normals from
//! (`assets/m/block_mesh.rs::region_normal`), so the model agrees with the
//! shading underneath it.

use bevy::camera::primitives::Aabb;
use bevy::prelude::*;

use packets::agent::prelude::{PetActionRequest, PetMountRequest, PetUnsummonRequest};
use packets::Packet;

use crate::net::connection::SilkroadConnection;
use crate::plugins::net::agent::AgentConnection;
use crate::plugins::net::character_info::MovementSpeed;
use crate::plugins::net::entities::{NetworkEntities, RemoteMovement};
use crate::plugins::player::{Player, PlayerCommands, PlayerMoveOrder};
use crate::plugins::world_origin::WorldOrigin;

use super::{ActiveCosList, CosCommand, CosEntity, RiderOf, RiderState};

/// How far to the side the player steps when dismounting (render units).
const DISMOUNT_OFFSET: f32 = 6.0;

/// The bone every rideable COS skeleton carries for its rider.
const SADDLE_BONE: &str = "saddle";

/// Fraction of the mount's Aabb top used as the seat when a model has no
/// [`SADDLE_BONE`] — OUR estimate, and only a stand-in until the skeleton
/// loads (or for the handful of rideables that lack the bone).
const SADDLE_HEIGHT_FACTOR: f32 = 0.85;

/// Where the rider sits, in the mount's own local space.
#[derive(Component, Debug, Clone, Copy)]
pub enum SaddleSeat {
    /// The mount's `saddle` bone; sampled every frame so the rider bobs with
    /// the gait, as the original does.
    Bone(Entity),
    /// Aabb-derived height fallback (no `saddle` bone on this model).
    Height(f32),
}

/// Resolve each COS's [`SaddleSeat`]: the `saddle` bone of the skeleton the
/// body wrapper carries, or an Aabb-derived height when the model has none.
/// Retries every frame until the `.bsr`/`.bsk` have streamed in (both the
/// wrapper and its bones appear asynchronously).
pub fn resolve_saddle_seats(
    cos: Query<(Entity, Option<&SaddleSeat>), With<CosEntity>>,
    children: Query<&Children>,
    skeletons: Query<&crate::commands::SkeletonBinding>,
    aabbs: Query<&Aabb>,
    mut commands: Commands,
) {
    for (entity, seat) in cos.iter() {
        // A bone seat is final; a height seat is only a stand-in and keeps
        // looking, since meshes (and their Aabbs) can load before the
        // skeleton does.
        if matches!(seat, Some(SaddleSeat::Bone(_))) {
            continue;
        }
        // The wrapper is a child of the COS root and owns the skeleton map.
        let bone = children
            .iter_descendants(entity)
            .filter_map(|child| skeletons.get(child).ok())
            .find_map(|binding| binding.bones.get(SADDLE_BONE).copied());
        if let Some(bone) = bone {
            // `try_insert`, not `insert` — the rule for every entity command
            // whose target came from the network. This retries every frame
            // against a COS the server can despawn under us, so the entity can
            // die between this query and the command applying: a GM teleport
            // while mounted did exactly that, taking the wrapper's parent with
            // it and panicking the schedule out of `insert::<SaddleSeat>`.
            commands.entity(entity).try_insert(SaddleSeat::Bone(bone));
            continue;
        }
        if seat.is_some() {
            continue; // already holding the fallback
        }
        // No skeleton yet, or a model without the bone: fall back to the mesh
        // extent, but only once meshes exist (else we'd seat the rider at the
        // mount's feet).
        let mut top: Option<f32> = None;
        for child in children.iter_descendants(entity) {
            if let Ok(aabb) = aabbs.get(child) {
                let child_top = aabb.center.y + aabb.half_extents.y;
                top = Some(top.map_or(child_top, |t: f32| t.max(child_top)));
            }
        }
        if let Some(top) = top {
            // Same reasoning as the bone seat above.
            commands
                .entity(entity)
                .try_insert(SaddleSeat::Height(top * SADDLE_HEIGHT_FACTOR));
        }
    }
}

/// The rider's seat in the mount's local space.
///
/// For a bone seat this is `inverse(mount_global) * bone_global`, i.e. both
/// sides come from the *same* transform-propagation pass (which runs in
/// `PostUpdate`, so both are one frame old). Taking the relative offset makes
/// that lag cancel: it is applied to the mount's current transform below.
fn seat_offset(
    seat: Option<&SaddleSeat>,
    mount_global: Option<&GlobalTransform>,
    bones: &Query<&GlobalTransform, With<crate::commands::Bone>>,
) -> Vec3 {
    match seat {
        Some(SaddleSeat::Bone(bone)) => {
            match (mount_global, bones.get(*bone).ok()) {
                (Some(mount), Some(bone)) => mount
                    .affine()
                    .inverse()
                    .transform_point3(bone.translation()),
                // Not propagated yet — sit at the origin for one frame.
                _ => Vec3::ZERO,
            }
        }
        Some(SaddleSeat::Height(height)) => Vec3::Y * *height,
        None => Vec3::ZERO,
    }
}

/// Bone-name fragment identifying a hand attachment. `commands/attach.rs`
/// branches on exactly this to decide a weapon grip, so it is the same test
/// that defines "is this a held item" everywhere in the tree.
const HAND_BONE: &str = "Hand";

/// Hide a rider's held items (weapon, shield) while mounted, as the original
/// does, and restore them on dismount.
///
/// Held items are the only attachments parented to a *bone* — armor and
/// clothes attach to the body wrapper instead — so "parent is a `Bone` whose
/// name contains `Hand`" selects exactly the weapon and shield. Hiding the
/// attachment ROOT takes its whole subtree with it (its own bones, mesh groups
/// and any bone-anchored aura) in one write, and composes with the GM ghost
/// (which swaps materials instead of visibility).
///
/// Polls rather than reacting to `RiderOf` being added: attachments resolve
/// over several frames, and a player can equip while mounted — an event-driven
/// version would miss both. Writes only on an actual state change, so a
/// settled character costs one comparison per attachment.
///
/// The inventory paper-doll is unaffected: it renders its own
/// `PaperDollClone`, which is neither a `Player` nor a `RemoteEntity`.
pub fn hide_held_items_while_mounted(
    characters: Query<
        (Entity, Has<RiderOf>),
        Or<(
            With<Player>,
            With<crate::plugins::net::entities::RemoteEntity>,
        )>,
    >,
    children: Query<&Children>,
    mut attachments: Query<(&ChildOf, &mut Visibility), With<crate::commands::SpawnedFromResource>>,
    bones: Query<&Name, With<crate::commands::Bone>>,
) {
    for (character, mounted) in characters.iter() {
        let wanted = if mounted {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
        for descendant in children.iter_descendants(character) {
            let Ok((parent, mut visibility)) = attachments.get_mut(descendant) else {
                continue;
            };
            // Held items hang off a hand bone; armor hangs off the wrapper.
            if !bones
                .get(parent.parent())
                .is_ok_and(|name| name.as_str().contains(HAND_BONE))
            {
                continue;
            }
            if *visibility != wanted {
                *visibility = wanted;
            }
        }
    }
}

/// Copy each COS transform onto its riders (position + rotation + seat).
/// Runs after remote movement, the ground alignment and the local player
/// stepper, so the camera (PostUpdate) samples a settled rider position.
///
/// The seat is rotated by the mount's rotation, so a rider stays in the saddle
/// once [`align_cos_to_slope`] pitches the mount — a world-space offset would
/// float them off the back on any slope.
pub fn slave_riders_to_cos(
    mut riders: Query<(&RiderOf, &mut Transform)>,
    mounts: Query<(&Transform, Option<&SaddleSeat>, Option<&GlobalTransform>), Without<RiderOf>>,
    bones: Query<&GlobalTransform, With<crate::commands::Bone>>,
) {
    for (rider_of, mut transform) in riders.iter_mut() {
        let Ok((cos_transform, seat, cos_global)) = mounts.get(rider_of.0) else {
            continue;
        };
        let seat_local = seat_offset(seat, cos_global, &bones);
        // Scale before rotation, because the mount carries the SRO X-mirror
        // (`scale.x = -1`, `util::mesh`) and `seat_offset` hands back a point in
        // the mount's *un-mirrored* local frame — the mirror is part of the
        // global affine it inverts out. Applying only the rotation therefore put
        // any sideways seat offset on the wrong side of the animal. Saddles sit
        // near x = 0 (c_horse's is 0.00), so this was small, but it was wrong.
        transform.translation =
            cos_transform.translation + cos_transform.rotation * (cos_transform.scale * seat_local);
        transform.rotation = cos_transform.rotation;
    }
}

// --- Ground alignment ---------------------------------------------------------

/// Sample distance for the central-difference ground normal. This is the
/// terrain heightfield's own grid step (`HEIGHT_MAP_STEP`), which makes the
/// stencil identical to the one the rendered terrain bakes its vertex normals
/// from — so the mount lies flush with the shading under it.
const SLOPE_SAMPLE_DISTANCE: f32 = 20.0;
/// Slerp weight per second toward a new ground normal. The heightfield is
/// bilinear, so its gradient is piecewise constant and jumps across cell
/// borders; without this easing the mount would visibly snap at every border.
const SLOPE_ALIGN_SPEED: f32 = 8.0;
/// Steepest tilt we apply, so a cliff edge can't lay the model on its side.
const MAX_SLOPE_TILT: f32 = std::f32::consts::FRAC_PI_4 * 0.9; // ~40°

/// The mount's heading and the rotation we last wrote, so repeated
/// extract-recompose cycles can't drift the heading (see
/// [`align_cos_to_slope`]).
#[derive(Component, Debug, Clone, Copy)]
pub struct GroundAlign {
    yaw: f32,
    applied: Quat,
}

/// Heading of a rotation, as the angle of its forward vector in XZ. Exact
/// inverse of `Quat::from_rotation_y` for yaw-only rotations, which is what
/// the movers write.
fn yaw_of(rotation: Quat) -> f32 {
    let forward = rotation * Vec3::Z;
    forward.x.atan2(forward.z)
}

/// Central-difference normal from four ground samples taken `distance` apart
/// in `[-x, +x, -z, +z]` order.
fn ground_normal(heights: [f32; 4], distance: f32) -> Vec3 {
    Vec3::new(
        heights[0] - heights[1],
        2.0 * distance,
        heights[2] - heights[3],
    )
    .normalize_or(Vec3::Y)
}

/// Rotation laying `Vec3::Y` onto `normal`, capped at `max_angle`.
fn slope_tilt(normal: Vec3, max_angle: f32) -> Quat {
    let angle = Vec3::Y.angle_between(normal);
    if !angle.is_finite() || angle < 1e-4 {
        return Quat::IDENTITY;
    }
    let Some(axis) = Vec3::Y.cross(normal).try_normalize() else {
        return Quat::IDENTITY;
    };
    Quat::from_axis_angle(axis, angle.min(max_angle))
}

/// Pitch and roll every COS onto the ground it stands on (mounts, transports
/// and pets alike — characters on foot stay upright, as they are today).
///
/// The nav stack exposes no surface normal, so one is derived by central
/// differences over `ground()`; because that call keeps the entity's tracked
/// `NavLocation`, the samples follow an object's surface (stair treads, a
/// bridge deck) exactly as the mount's own movement does.
///
/// Heading is left entirely to the movers: each frame the yaw is taken from
/// *their* write and the tilt is re-composed on top, so this system is the
/// last writer without ever fighting them for the heading. The yaw we last
/// used is remembered because extracting it back out of an already-tilted
/// rotation is only approximate — re-extracting every frame while parked on a
/// slope would slowly spin the model.
pub fn align_cos_to_slope(
    time: Res<Time>,
    nav: crate::plugins::nav::NavMeshRaycast,
    mut cos: Query<
        (
            Entity,
            &mut Transform,
            &crate::plugins::nav::NavLocation,
            Option<&mut GroundAlign>,
        ),
        With<CosEntity>,
    >,
    mut commands: Commands,
) {
    let distance = SLOPE_SAMPLE_DISTANCE;
    for (entity, mut transform, location, align) in cos.iter_mut() {
        let xz = transform.translation.xz();
        let reference_y = transform.translation.y;
        let sample = |offset: Vec2| {
            nav.ground(xz + offset, reference_y, *location)
                .map(|(height, _)| height)
        };
        // Nav data not streamed in yet: hold the tilt we have rather than
        // snapping upright for a frame.
        let (Some(x0), Some(x1), Some(z0), Some(z1)) = (
            sample(Vec2::new(-distance, 0.0)),
            sample(Vec2::new(distance, 0.0)),
            sample(Vec2::new(0.0, -distance)),
            sample(Vec2::new(0.0, distance)),
        ) else {
            continue;
        };

        let normal = ground_normal([x0, x1, z0, z1], distance);
        // Keep our own yaw while nothing else wrote the rotation; re-read it
        // when a mover did (their writes are yaw-only, so this is exact).
        let yaw = match align.as_deref() {
            Some(align) if align.applied == transform.rotation => align.yaw,
            _ => yaw_of(transform.rotation),
        };
        let yaw_rotation = Quat::from_rotation_y(yaw);
        let current_tilt = transform.rotation * yaw_rotation.inverse();
        let tilt = current_tilt.slerp(
            slope_tilt(normal, MAX_SLOPE_TILT),
            (SLOPE_ALIGN_SPEED * time.delta_secs()).min(1.0),
        );
        let rotation = tilt * yaw_rotation;
        transform.rotation = rotation;

        let state = GroundAlign {
            yaw,
            applied: rotation,
        };
        match align {
            Some(mut align) => *align = state,
            None => {
                // `try_insert` for the same reason as the saddle seats: this
                // iterates COS entities the server can despawn mid-frame.
                commands.entity(entity).try_insert(state);
            }
        }
    }
}

/// Board/dismount/unsummon/follow, from the command bar and the dev window.
/// `local_only` COS (and any COS while no agent connection exists) transition
/// directly; server-side COS go through the wire and wait for the ack.
#[allow(clippy::too_many_arguments)]
pub fn handle_cos_commands(
    mut commands_in: MessageReader<CosCommand>,
    list: Res<ActiveCosList>,
    index: Res<NetworkEntities>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    player: Query<(Entity, &Transform), With<Player>>,
    origin: Res<WorldOrigin>,
    mut rider: ResMut<RiderState>,
    mut player_commands: ResMut<PlayerCommands>,
    mut list_mut: Local<Vec<u32>>, // unsummoned-this-frame uids
    mut movements: Query<&mut RemoteMovement>,
    mut ecs: Commands,
) {
    list_mut.clear();
    for command in commands_in.read() {
        let Ok((player_entity, player_transform)) = player.single() else {
            continue;
        };
        // A COS not in the list (someone else's mount) is not commandable.
        let status_of = |uid: u32| list.get(uid);
        // Direct transitions bypass the wire: dev-spawned COS always, any COS
        // when offline (the server can't answer what it never spawned).
        let offline = conn.single().is_err();
        match *command {
            CosCommand::Board(uid) => {
                if rider.0.is_some() {
                    continue;
                }
                let Some(status) = status_of(uid) else {
                    warn!("cos: Board({uid}) for a COS not in the active list");
                    continue;
                };
                if !status.kind.is_rideable() {
                    continue;
                }
                if status.local_only || offline {
                    let Some(cos_entity) = index.get(uid) else {
                        continue;
                    };
                    info!("cos: boarding {} locally", uid);
                    ecs.entity(player_entity).insert(RiderOf(cos_entity));
                    rider.0 = Some(uid);
                    player_commands.stop();
                } else {
                    info!("cos: sending 0x70CB mount request for {}", uid);
                    send(
                        &conn,
                        PetMountRequest {
                            mount_state: 1,
                            cos_unique_id: uid,
                        },
                    );
                }
            }
            CosCommand::Dismount => {
                let Some(uid) = rider.0 else {
                    continue;
                };
                let local = status_of(uid).is_none_or(|s| s.local_only);
                if local || offline {
                    dismount_locally(
                        &mut ecs,
                        player_entity,
                        player_transform,
                        &mut rider,
                        &mut player_commands,
                    );
                } else {
                    info!("cos: sending 0x70CB dismount request for {}", uid);
                    send(
                        &conn,
                        PetMountRequest {
                            mount_state: 0,
                            cos_unique_id: uid,
                        },
                    );
                }
            }
            CosCommand::Unsummon(uid) => {
                let Some(status) = status_of(uid) else {
                    continue;
                };
                if rider.0 == Some(uid) {
                    dismount_locally(
                        &mut ecs,
                        player_entity,
                        player_transform,
                        &mut rider,
                        &mut player_commands,
                    );
                }
                if status.local_only || offline {
                    info!("cos: unsummoning {} locally", uid);
                    if let Some(cos_entity) = index.get(uid) {
                        ecs.entity(cos_entity).despawn();
                    }
                    list_mut.push(uid);
                } else {
                    send(&conn, PetUnsummonRequest { unique_id: uid });
                }
            }
            CosCommand::Follow(uid) => {
                let Some(status) = status_of(uid) else {
                    continue;
                };
                if status.local_only || offline {
                    if let Some(mut movement) =
                        index.get(uid).and_then(|e| movements.get_mut(e).ok())
                    {
                        movement.target = Some(player_transform.translation);
                    }
                } else {
                    let (region, x, y, z) = crate::scenes::game_scene::render_to_server_position(
                        player_transform.translation,
                        &origin,
                    );
                    send(
                        &conn,
                        PetActionRequest::Movement {
                            pet_unique_id: uid,
                            region,
                            x: x.round() as i32,
                            y: y.round() as i32,
                            z: z.round() as i32,
                        },
                    );
                }
            }
        }
    }
    if !list_mut.is_empty() {
        // Deferred so the loop above can hold `list` immutably.
        let unsummoned = std::mem::take(&mut *list_mut);
        ecs.queue(move |world: &mut World| {
            let mut list = world.resource_mut::<ActiveCosList>();
            for uid in unsummoned {
                list.remove(uid);
            }
        });
    }
}

fn dismount_locally(
    ecs: &mut Commands,
    player_entity: Entity,
    player_transform: &Transform,
    rider: &mut RiderState,
    player_commands: &mut PlayerCommands,
) {
    info!("cos: dismounting locally");
    ecs.entity(player_entity).try_remove::<RiderOf>();
    rider.0 = None;
    player_commands.stop();
    // Step off sideways so the player doesn't stand inside the mount; the
    // player's own systems reconcile the height on the next move.
    let side = player_transform.rotation * Vec3::X;
    let offset = side.xz().normalize_or_zero() * DISMOUNT_OFFSET;
    let target = player_transform.translation + Vec3::new(offset.x, 0.0, offset.y);
    ecs.entity(player_entity)
        .entry::<Transform>()
        .and_modify(move |mut t| {
            t.translation = target;
        });
    // The hop off the saddle is a discontinuous move (and the mount may have
    // carried us onto a bridge/stairs), so whatever surface the player was
    // tracking is meaningless — ADR-0007's rule for teleports.
    ecs.entity(player_entity)
        .insert(crate::plugins::nav::NavLocation::Unresolved);
}

/// While mounted on a `local_only` COS (or offline), the player's click orders
/// drive the COS directly: its `RemoteMovement` walks it (with the COS's own
/// speed) and the slaving system carries the rider. Server-side mounts are
/// driven by `game_scene::send_movement_request`, which reroutes the order to
/// 0x70C5 — this system must not double-drive those.
pub fn route_move_orders_while_mounted(
    mut orders: MessageReader<PlayerMoveOrder>,
    rider: Res<RiderState>,
    list: Res<ActiveCosList>,
    index: Res<NetworkEntities>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    mut movements: Query<(&mut RemoteMovement, Option<&MovementSpeed>)>,
) {
    let Some(uid) = rider.0 else {
        // Not mounted: drain nothing — the normal player path owns the orders.
        return;
    };
    let local = list.get(uid).is_none_or(|s| s.local_only) || conn.single().is_err();
    for order in orders.read() {
        if !local {
            continue; // 0x70C5 sender handles it
        }
        if let Some((mut movement, speed)) = index.get(uid).and_then(|e| movements.get_mut(e).ok())
        {
            if let Some(speed) = speed {
                movement.speed = speed.current();
            }
            movement.target = Some(order.0);
        }
    }
}

fn send<T>(conn: &Query<&SilkroadConnection, With<AgentConnection>>, packet: T)
where
    Packet: From<T>,
{
    let Ok(conn) = conn.single() else {
        return;
    };
    if let Err(e) = conn.get_sender().send(Packet::from(packet).into()) {
        error!("cos: failed to send packet: {}", e.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A COS despawned between the query and the command flush leaves the
    /// world consistent.
    ///
    /// **Honest limit: this does not reproduce the panic.** The live failure
    /// was `insert::<SaddleSeat>` reaching bevy's error handler after a GM
    /// teleport despawned the mount, and driving the system by hand does not
    /// route the dead-entity error there — the test passes against the unfixed
    /// `insert` too, with `FallbackErrorHandler(panic)` installed or not. So
    /// this is a consistency guard, not a regression guard, and the fix rests
    /// on the tree's existing rule (`try_insert` for anything the network
    /// owns) rather than on a red test.
    #[test]
    fn a_cos_despawned_before_the_flush_leaves_the_world_consistent() {
        use bevy::ecs::system::{IntoSystem, System};

        let mut world = World::new();
        // A COS whose child carries an Aabb, so the height-fallback branch
        // has something to queue.
        let cos = world
            .spawn(CosEntity {
                kind: packets::agent::pet::CosKind::Vehicle,
                owner_uid: None,
            })
            .id();
        let child = world
            .spawn(Aabb::from_min_max(Vec3::ZERO, Vec3::splat(2.0)))
            .id();
        world.entity_mut(cos).add_child(child);

        let mut system = IntoSystem::into_system(resolve_saddle_seats);
        system.initialize(&mut world);
        system.run((), &mut world);

        // ...and the mount is gone before the command lands.
        world.entity_mut(cos).despawn();
        system.apply_deferred(&mut world);

        assert!(world.get_entity(cos).is_err(), "the COS stays despawned");
    }

    /// The same system on a live COS still does its job — the `try_insert`
    /// must not have turned the fix into a no-op.
    #[test]
    fn a_live_cos_still_gets_its_height_seat() {
        use bevy::ecs::system::{IntoSystem, System};

        let mut world = World::new();
        let cos = world
            .spawn(CosEntity {
                kind: packets::agent::pet::CosKind::Vehicle,
                owner_uid: None,
            })
            .id();
        let child = world
            .spawn(Aabb::from_min_max(Vec3::ZERO, Vec3::new(2.0, 10.0, 2.0)))
            .id();
        world.entity_mut(cos).add_child(child);

        let mut system = IntoSystem::into_system(resolve_saddle_seats);
        system.initialize(&mut world);
        system.run((), &mut world);
        system.apply_deferred(&mut world);

        let seat = world.get::<SaddleSeat>(cos).expect("a seat was resolved");
        match seat {
            SaddleSeat::Height(h) => assert!(*h > 0.0, "seat height {h} must be above the feet"),
            other => panic!("expected the height fallback, got {other:?}"),
        }
    }

    /// `yaw_of` must invert `Quat::from_rotation_y` exactly, because the
    /// alignment system round-trips the movers' yaw-only writes through it
    /// every frame.
    #[test]
    fn yaw_extraction_inverts_from_rotation_y() {
        for yaw in [0.0, 0.7, 2.5, -1.3, std::f32::consts::PI - 0.01] {
            let extracted = yaw_of(Quat::from_rotation_y(yaw));
            assert!(
                (extracted - yaw).abs() < 1e-5,
                "yaw {yaw} round-tripped to {extracted}"
            );
        }
    }

    /// Flat ground yields the up vector; a slope tilts the normal *against*
    /// the rise (uphill toward -x means the normal leans +x).
    #[test]
    fn ground_normal_from_central_differences() {
        let flat = ground_normal([10.0, 10.0, 10.0, 10.0], 20.0);
        assert!((flat - Vec3::Y).length() < 1e-5);

        // Rising toward +x (h(+x) > h(-x)): the normal leans toward -x.
        let ramp = ground_normal([0.0, 20.0, 10.0, 10.0], 20.0);
        assert!(ramp.x < 0.0, "normal must lean downhill, got {ramp:?}");
        assert!(ramp.z.abs() < 1e-5, "no roll on a pure x ramp");
        assert!(ramp.y > 0.0, "normal always points up");
        assert!((ramp.length() - 1.0).abs() < 1e-5);
    }

    /// A cliff edge must not lay the model over: the tilt saturates at the cap
    /// while keeping the tilt direction.
    #[test]
    fn slope_tilt_is_capped() {
        let gentle = Vec3::new(0.0, 10.0, 1.0).normalize();
        let angle = Vec3::Y.angle_between(gentle);
        let tilt = slope_tilt(gentle, MAX_SLOPE_TILT);
        assert!(
            ((tilt * Vec3::Y).angle_between(Vec3::Y) - angle).abs() < 1e-4,
            "a gentle slope is applied as-is"
        );

        // Near-vertical: capped, but still tilting the same way.
        let cliff = Vec3::new(0.0, 0.1, 1.0).normalize();
        let capped = slope_tilt(cliff, MAX_SLOPE_TILT);
        let applied = (capped * Vec3::Y).angle_between(Vec3::Y);
        assert!(
            (applied - MAX_SLOPE_TILT).abs() < 1e-4,
            "expected the cap, got {applied}"
        );
        assert!((capped * Vec3::Y).z > 0.0, "tilt direction preserved");

        // Flat ground is the identity (no axis to rotate about).
        assert_eq!(slope_tilt(Vec3::Y, MAX_SLOPE_TILT), Quat::IDENTITY);
    }

    /// The composed rotation keeps the heading the movers set, whatever the
    /// tilt — the property that stops the mount from spinning on a slope.
    #[test]
    fn tilt_composition_preserves_heading() {
        let yaw = 1.1;
        let tilt = slope_tilt(Vec3::new(0.3, 1.0, 0.0).normalize(), MAX_SLOPE_TILT);
        let rotation = tilt * Quat::from_rotation_y(yaw);
        // Recovering the tilt back out is exact, which is what lets the system
        // slerp from its own previous tilt.
        let recovered = rotation * Quat::from_rotation_y(yaw).inverse();
        assert!(recovered.abs_diff_eq(tilt, 1e-5));
        // ...and the heading survives the tilt: the forward vector's XZ
        // direction still points where the mover aimed it.
        let forward = rotation * Vec3::Z;
        assert!((forward.x.atan2(forward.z) - yaw).abs() < 0.05);
    }
}
