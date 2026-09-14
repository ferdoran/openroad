use bevy::asset::{AssetEvent, Assets};
use bevy::camera::{Camera, Camera3d, RenderTarget};
use bevy::ecs::hierarchy::ChildOf;
use bevy::math::{EulerRot, Mat3, Quat, Vec3};
use bevy::mesh::MeshTag;
use bevy::prelude::{
    Children, Commands, DetectChangesMut, Entity, GlobalTransform, Has, MessageReader, Query, Res,
    ResMut, Time, Transform, Visibility, With, Without,
};
use rand::Rng;

use crate::assets::efp::format::{
    argb_channels, Argb, EeParameter, EfController, EfStoredObject, EffectCommand, ViewMode,
};
use crate::assets::efp::JMXVEFF;
use crate::plugins::effects::components::*;
use crate::plugins::effects::material::SroEffectMaterial;
use crate::plugins::effects::spawn::{
    emit_params, reset_pooled_particle, spawn_node_tree, EffectSpawnParams,
};

/// Rebuilds the spawn-time program caches of live nodes when their effect
/// asset is hot-reloaded — the cached indices would otherwise go stale
/// (they degrade to skips, never panics, but dev edits should show up).
/// Event-driven: zero cost while no asset changes.
pub fn rebuild_program_caches(
    mut events: MessageReader<AssetEvent<JMXVEFF>>,
    effects: Res<Assets<JMXVEFF>>,
    mut nodes: Query<(
        &EffectNode,
        &mut EffectProgramCache,
        Option<&mut EffectEmitter>,
    )>,
) {
    for event in events.read() {
        let AssetEvent::Modified { id } = event else {
            continue;
        };
        for (node, mut cache, emitter) in &mut nodes {
            if node.handle.id() != *id {
                continue;
            }
            let Some(effect) = effects.get(*id) else {
                continue;
            };
            let Some(node_data) = effect.effect.nodes.get(node.node) else {
                continue;
            };
            *cache = EffectProgramCache::build(node_data, effect);
            if let Some(mut emitter) = emitter {
                if let Some(params) = emit_params(node_data) {
                    emitter.params = params;
                }
            }
        }
    }
}

/// Distance-culls effect *simulation*. Beyond the fully-fogged distance an
/// effect cannot be seen, but ambient emitters (torch flames, lamp glows)
/// exist on every effect-mod object out to the terrain *unload* boundary —
/// well past the fog — and their particles would keep emitting, integrating
/// motion and billboarding every frame. Roots past the fog end get
/// [`EffectSimPaused`] stamped on their whole subtree, so the sim systems
/// skip the nodes archetypally at zero per-frame cost, and are hidden; both
/// are undone with hysteresis when the camera comes back into range (the
/// frozen state simply resumes). Still-initializing roots are skipped —
/// their GlobalTransform is not propagated yet, and pausing before the node
/// tree exists would mark an incomplete subtree.
///
/// Inside a dungeon the fog reach is the *current block's* fog far plane
/// (850-1600 in Donwhang vs the overworld's 5760) and effects in
/// portal-hidden blocks pause outright — a dense cave carries a four-digit
/// torch-flame count within the overworld radius, which is exactly the
/// "15 FPS with effects on" playtest finding.
#[allow(clippy::too_many_arguments)]
pub fn cull_effect_simulation(
    mut commands: Commands,
    cameras: Query<(&Camera, &RenderTarget, &GlobalTransform), With<Camera3d>>,
    active_dungeon: Option<Res<crate::plugins::dungeon::ActiveDungeon>>,
    dofs: Res<Assets<crate::assets::dof::JMXVDOF>>,
    mut roots: Query<
        (
            Entity,
            &GlobalTransform,
            Has<EffectSimPaused>,
            &mut Visibility,
        ),
        (With<EffectInstance>, Without<EffectPendingInit>),
    >,
    children: Query<&Children>,
    parents: Query<&ChildOf>,
    block_visibility: Query<
        &Visibility,
        (
            With<crate::plugins::dungeon::DungeonBlock>,
            Without<EffectInstance>,
        ),
    >,
) {
    use crate::plugins::map::terrain::{FOG_RANGE, REGION_SIZE, VISIBLE_RANGE};
    let (pause_dist, resume_dist) = match &active_dungeon {
        Some(active) => {
            let fog = crate::plugins::dungeon::atmosphere::current_fog_far(active, &dofs);
            // Small hysteresis band; the dungeon fog reach is short enough
            // that a fraction of it keeps the boundary from thrashing.
            (fog + 200.0, fog + 50.0)
        }
        None => {
            // Fully fogged from here on out (see `terrain/rendering.rs::fog`).
            let pause = (VISIBLE_RANGE + FOG_RANGE) as f32 * REGION_SIZE;
            // Resume a bit closer (~75% fogged, still virtually invisible) so
            // a camera hovering at the boundary doesn't thrash the markers.
            (pause, pause - REGION_SIZE * 0.25)
        }
    };

    let Some(camera) = main_world_camera(&cameras) else {
        return;
    };
    let camera_pos = camera.translation();

    for (root, global, paused, mut visibility) in &mut roots {
        // An effect inside a portal-hidden block is invisible whatever its
        // distance; its block root's own Visibility is the switch (never the
        // effect's inherited visibility — pausing hides the effect root
        // itself, which must not keep it paused).
        let block_hidden = active_dungeon.is_some()
            && std::iter::once(root)
                .chain(parents.iter_ancestors(root))
                .any(|entity| {
                    block_visibility
                        .get(entity)
                        .is_ok_and(|block| *block == Visibility::Hidden)
                });
        let dist_sq = global.translation().distance_squared(camera_pos);
        if !paused && (block_hidden || dist_sq > pause_dist * pause_dist) {
            *visibility = Visibility::Hidden;
            commands.entity(root).insert(EffectSimPaused);
            for descendant in children.iter_descendants(root) {
                commands.entity(descendant).insert(EffectSimPaused);
            }
        } else if paused && !block_hidden && dist_sq < resume_dist * resume_dist {
            visibility.set_if_neq(Visibility::Inherited);
            commands.entity(root).remove::<EffectSimPaused>();
            for descendant in children.iter_descendants(root) {
                commands.entity(descendant).remove::<EffectSimPaused>();
            }
        } else if paused {
            // The render-debug `render_effects` re-enable sets every wrapper
            // back to Inherited wholesale; keep paused effects hidden.
            visibility.set_if_neq(Visibility::Hidden);
        }
    }
}

/// Advances node age and despawns/wraps finished nodes. Expired emitted
/// *leaf* particles with a finite timeline (no children, no sub-emitter)
/// are not despawned but hidden and parked in their emitter's
/// pool for reuse — steady-state emitters otherwise churn entities and
/// material assets every particle lifetime. Emitted 0-length-timeline nodes
/// and sub-emitters never reach the `Once` arm at all: `lifespan_of` gives
/// them a looping lifespan whose wrap emulates a respawn in place, so
/// ambient effects stay visible continuously.
pub fn tick_effect_nodes(
    time: Res<Time>,
    mut commands: Commands,
    mut nodes: Query<
        (
            Entity,
            &mut EffectNode,
            &mut Transform,
            Option<&EmittedBy>,
            Option<&mut EffectMotion>,
            Has<Children>,
            Has<EffectEmitter>,
        ),
        (Without<PooledParticle>, Without<EffectSimPaused>),
    >,
    mut emitters: Query<&mut EffectEmitter>,
    parked: Query<(Entity, &EmittedBy), With<PooledParticle>>,
    time_scales: Query<&EffectTimeScale>,
    speed: Res<crate::plugins::effects::EffectPlaybackSpeed>,
) {
    let base_dt = time.delta_secs() * speed.0;
    for (entity, mut node, mut transform, emitted_by, motion, has_children, is_emitter) in
        &mut nodes
    {
        // Per-effect playback rate (rare-item auras slow their loop/pulse).
        let dt = base_dt * time_scales.get(node.root).map_or(1.0, |s| s.0);
        node.age += dt;
        match node.lifespan {
            Lifespan::Once(duration) if node.age >= duration => {
                let mut pooled = false;
                if let Some(&EmittedBy { emitter, slot }) = emitted_by {
                    if let Ok(mut state) = emitters.get_mut(emitter) {
                        if let Some(alive) = state.alive.get_mut(slot) {
                            *alive = alive.saturating_sub(1);
                        }
                        if let Some(acc) = state.acc.get_mut(slot) {
                            // Death refunds one emission credit (exe: the
                            // death handler 0xc87a20 does `acc -= 1.0`), so
                            // steady-state streams keep flowing at the cap.
                            *acc = (*acc - 1.0).max(0.0);
                        }
                        // Trail nodes pool too — despawning them freed a
                        // dedicated ribbon mesh every particle lifetime,
                        // grinding the render mesh allocator (slab-allocator
                        // use-after-free error spam); the pooled entity
                        // keeps its mesh and update_effect_trails clears
                        // the parked ribbon.
                        //
                        // try_insert, NOT insert: under a one-shot wrapper
                        // the parent emitter is Once too, and its recursive
                        // despawn can apply (this same sync point!) before
                        // this particle's park command — the plain insert
                        // then panics on the dead entity. A stale pool entry
                        // is harmless: reset_pooled_particle falls back to a
                        // fresh spawn when the entity is gone.
                        if !has_children && !is_emitter {
                            commands
                                .entity(entity)
                                .try_insert((PooledParticle, Visibility::Hidden));
                            state.pool.push((slot, entity));
                            pooled = true;
                        }
                    }
                }
                if !pooled {
                    // tolerant for the same reason: an ancestor's Once
                    // despawn may already have taken this node down
                    commands.entity(entity).try_despawn();
                }
            }
            Lifespan::Loop { period, respawn } if node.age >= period => {
                node.age %= period.max(f32::EPSILON);
                if let Some(mut motion) = motion {
                    motion.sim_frame = 0;
                    // A synthetic loop wrap emulates a respawn (the original
                    // replays the whole effect): zero accumulated velocity
                    // and snap back to the spawn origin, or SetVelocity /
                    // Force / Attraction commands drift the node unboundedly
                    // across cycles. Authored NormalTimeLoopLife nodes keep
                    // their state.
                    if respawn {
                        motion.velocity = Vec3::ZERO;
                        transform.translation = motion.origin;
                    }
                }
                // A looping emitter restarts its emission window (exe: the
                // loop-restart reset 0xc86e99 zeroes the accumulator). Live
                // particles keep their credit so the cap holds across wraps.
                if is_emitter {
                    if let Ok(mut state) = emitters.get_mut(entity) {
                        let state = &mut *state;
                        state.frame = 0;
                        for (acc, &alive) in state.acc.iter_mut().zip(state.alive.iter()) {
                            *acc = alive as f32;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // Pooled particles whose emitter died (e.g. a finite Once ancestor took
    // the emitter down) are unreachable — no pool references them anymore —
    // and would sit hidden under the effect root forever.
    for (entity, emitted_by) in &parked {
        if !emitters.contains(emitted_by.emitter) {
            commands.entity(entity).despawn();
        }
    }
}

/// Stamps out particle instances of emitter nodes' template children.
pub fn emit_particles(
    time: Res<Time>,
    mut commands: Commands,
    mut params: EffectSpawnParams,
    mut emitter_nodes: Query<
        (Entity, &EffectNode, &mut EffectEmitter, &GlobalTransform),
        Without<EffectSimPaused>,
    >,
    globals: Query<&GlobalTransform>,
    locals: Query<(&Transform, &ChildOf)>,
    leaf_policy: Res<crate::plugins::effects::LeafEmitPolicy>,
    one_shots: Query<Has<OneShotEffect>>,
) {
    let _ = &time;
    for (entity, node, mut emitter, emitter_global) in &mut emitter_nodes {
        let Some(effect) = params.effects.get(&node.handle) else {
            continue;
        };
        let node_data = &effect.effect.nodes[node.node];
        // Emission parameters were resolved once at spawn.
        let emit = emitter.params;
        // A group emitter emits its child templates; a leaf emitter (no
        // children) emits copies of *itself* — exe-faithful and on by
        // default (LeafEmitPolicy).
        let self_emit = node_data.children.is_empty() && leaf_policy.global;
        let templates: &[usize] = if node_data.children.is_empty() {
            if self_emit {
                std::slice::from_ref(&node.node)
            } else {
                &[]
            }
        } else {
            &node_data.children
        };

        if emitter.alive.len() != templates.len() {
            // Asset hot-reload can change the children list under us.
            emitter.alive.resize(templates.len(), 0);
            emitter.acc.resize(templates.len(), 0.0);
        }
        // Leaf emission uses the policy's density slider for A/B
        // calibration; authored counts at the default density 1.0.
        let max_alive = if self_emit {
            let density = leaf_policy.density.clamp(0.0, 1.0);
            ((emit.max_alive as f32 * density).ceil() as u32).max(1)
        } else {
            emit.max_alive
        };

        // Exe-faithful pacing (worker 0xc84e70): evaluate every effect frame
        // crossed since the last run; on frames inside the emission window
        // where (frame - start) % period == 0, each slot's accumulator gains
        // `rate` (clamped at the cap) and spawns the integer credit crossed.
        // Deaths refund 1.0 credit (see tick_effect_nodes). Catch-up after a
        // hitch is bounded — beyond that the remaining frames are dropped.
        let target = (node.age * EFFECT_FPS) as u32 + 1;
        let frames = target.saturating_sub(emitter.frame).min(16);
        let first = target - frames;
        for frame in first..target {
            // Pre-age new particles to their emission tick's effect-frame
            // boundary: the exe advances all counters on the shared 50 ms
            // grid, so a particle's death lands exactly on the tick that
            // spawns its replacement. Spawning at render-frame age 0 lets
            // the phase slip a period per generation, and the resulting
            // hole + duplicate in the phase comb reads as a strong
            // once-per-loop pulse (probe: aggregate alpha swung 2.2..3.8).
            let initial_age = (node.age - frame as f32 / EFFECT_FPS).max(0.0);
            for (slot, &template) in templates.iter().enumerate() {
                let spawns = emission_spawns(&emit, frame, max_alive, &mut emitter.acc[slot]);
                for _ in 0..spawns {
                    let attach = effect.effect.nodes[template].timeline.attach_to_parent();
                    let (parent, transform) = if attach {
                        (entity, Transform::IDENTITY)
                    } else {
                        // Emitter-relative-to-root composed from LOCAL
                        // transforms, walking ChildOf up to the root: the
                        // GlobalTransform of a node spawned this frame is
                        // still identity (propagation runs in PostUpdate),
                        // and the old root⁻¹·emitter global math landed a
                        // first-sim-frame emission's detached particles at
                        // the WORLD ORIGIN (the bow draw force's pierce
                        // spike, BRP-measured 2026-08-03). Locals are always
                        // current; the wrapper's mirror still never enters
                        // (the walk stops below the root).
                        let mut acc = bevy::math::Mat4::IDENTITY;
                        let mut cur = entity;
                        let mut reached = cur == node.root;
                        for _ in 0..64 {
                            if reached {
                                break;
                            }
                            let Ok((tf, child_of)) = locals.get(cur) else {
                                break;
                            };
                            acc = tf.to_matrix() * acc;
                            cur = child_of.parent();
                            reached = cur == node.root;
                        }
                        let relative = if reached {
                            Transform::from_matrix(acc)
                        } else {
                            // fallback: the pre-2026-08-03 globals math
                            globals
                                .get(node.root)
                                .map(|root_global| {
                                    Transform::from_matrix(
                                        root_global.to_matrix().inverse()
                                            * emitter_global.to_matrix(),
                                    )
                                })
                                .unwrap_or_else(|_| emitter_global.compute_transform())
                        };
                        (node.root, relative)
                    };

                    // Prefer re-arming a parked particle of this slot over
                    // stamping out a fresh node tree.
                    let pooled = emitter
                        .pool
                        .iter()
                        .position(|(s, _)| *s == slot)
                        .map(|pos| emitter.pool.swap_remove(pos).1);
                    let reused = pooled.is_some_and(|entity| {
                        reset_pooled_particle(
                            &mut commands,
                            &node.handle,
                            effect,
                            template,
                            entity,
                            parent,
                            node.root,
                            transform,
                            initial_age,
                        )
                    });
                    if !reused {
                        spawn_node_tree(
                            &mut commands,
                            &mut params.assets,
                            &node.handle,
                            effect,
                            template,
                            parent,
                            node.root,
                            transform,
                            Some(EmittedBy {
                                emitter: entity,
                                slot,
                            }),
                            self_emit,
                            leaf_policy.global,
                            initial_age,
                            one_shots.get(node.root).unwrap_or(false),
                        );
                    }
                    emitter.alive[slot] += 1;
                }
            }
        }
        emitter.frame = target;
    }
}

/// SRO→Bevy handedness fix for authored rotations: every SRO resource
/// renders under the X-mirror placement (`scale.x = -1`, see
/// `MirroredResource`), and a reflection reverses rotation handedness —
/// an authored +90° yaw must visually turn the other way in the mirrored
/// hierarchy to land where the original client puts it (e.g. the Seal
/// aura's `compo` node yaws its BAN sweep onto the blade axis). Conjugating
/// by the mirror negates a quaternion's y/z components.
fn mirror_quat(q: Quat) -> Quat {
    Quat::from_xyzw(q.x, -q.y, -q.z, q.w)
}

/// Axis-angle form of [`mirror_quat`]: same angle, axis y/z negated.
/// (Rotation axes are pseudovectors — they mirror differently from plain
/// displacement vectors, see below.)
///
/// Plain displacement vectors (SetPosition/SetVelocity/Force) are NOT
/// conjugated: node transforms are descendants of the mirrored wrapper, so
/// transform propagation applies the SRO→Bevy X-negation to translations
/// exactly once — pre-negating them here double-mirrored authored
/// asymmetric motion back to the un-mirrored side (pinned by
/// `authored_vectors_mirror_exactly_once_through_the_hierarchy`).
/// Rotations still need the conjugation because their visible frame is
/// re-derived against the camera by `billboard_effect_nodes`, which undoes
/// the parent affine — mirror included (BRP-verified on the Seal aura's
/// `compo` +90° yaw and its BAN blade sweep).
fn mirror_axis(axis: Vec3) -> Vec3 {
    Vec3::new(axis.x, -axis.y, -axis.z)
}

/// The per-slot emission decision for one effect frame, exe-faithful
/// (worker 0xc84e70): inside the window `start..start+window`, on frames
/// where `(frame - start) % period == 0`, the accumulator gains `rate`
/// clamped at the alive cap, and the integer credit crossed is the spawn
/// count. Returns 0 outside the window / off-period.
fn emission_spawns(
    params: &crate::plugins::effects::spawn::EmitParams,
    frame: u32,
    max_alive: u32,
    acc: &mut f32,
) -> u32 {
    let Some(elapsed) = frame.checked_sub(params.start_frame) else {
        return 0;
    };
    if elapsed >= params.window || elapsed % params.period != 0 {
        return 0;
    }
    let acc_old = *acc;
    let acc_new = (acc_old + params.rate).min(max_alive as f32);
    *acc = acc_new;
    (acc_new.trunc() - acc_old.trunc()).max(0.0) as u32
}

fn random_in_ellipsoid(semi_axes: Vec3) -> Vec3 {
    let mut rng = rand::rng();
    loop {
        let p = Vec3::new(
            rng.random_range(-1.0..=1.0f32),
            rng.random_range(-1.0..=1.0f32),
            rng.random_range(-1.0..=1.0f32),
        );
        if p.length_squared() <= 1.0 {
            return p * semi_axes.abs();
        }
    }
}

/// Random direction within a cone of `half_angle_deg` around `axis`,
/// matching the original engine (exe RE, case 62/70 of the EasyFX command
/// interpreter): tilt = rand·half_angle about Z, then a uniform-random
/// azimuth spin about Y — uniform in *tilt angle* (biased toward the axis),
/// not area-uniform, and deliberately unclamped (authored 360° wraps).
fn random_in_cone(axis: Vec3, half_angle_deg: f32) -> Vec3 {
    let mut rng = rand::rng();
    let tilt = rng.random_range(0.0..=half_angle_deg.abs().to_radians());
    let phi = rng.random_range(0.0..std::f32::consts::TAU);
    let local = Quat::from_rotation_y(phi) * Quat::from_rotation_z(tilt) * Vec3::Y;
    Quat::from_rotation_arc(Vec3::Y, axis.try_normalize().unwrap_or(Vec3::Y)) * local
}

/// AngleVector1 (exe RE confirmed): x/y = random speed min/max
/// (`x + rand·(y−x)`), z = cone half-angle in degrees.
fn cone_speed(degrees: Vec3) -> f32 {
    degrees.x + rand::rng().random_range(0.0..=1.0f32) * (degrees.y - degrees.x)
}

/// Evaluates program commands (one-shot at window entry, continuous while
/// active) and integrates velocity/rotation. The command sources were
/// resolved once at spawn into [`EffectProgramCache`] — only the
/// motion-relevant slots are visited, with their windows precomputed,
/// instead of re-walking and re-matching the whole source chain per frame.
pub fn run_programs(
    time: Res<Time>,
    speed: Res<crate::plugins::effects::EffectPlaybackSpeed>,
    effects: Res<Assets<JMXVEFF>>,
    bans: Res<Assets<crate::assets::ban::JMXVBAN>>,
    mut nodes: Query<
        (
            &EffectNode,
            &EffectProgramCache,
            &mut EffectMotion,
            &mut Transform,
            Option<&EffectBillboard>,
            Has<EffectVisual>,
        ),
        (Without<PooledParticle>, Without<EffectSimPaused>),
    >,
) {
    let dt = time.delta_secs() * speed.0;
    for (node, cache, mut motion, mut transform, billboard, has_visual) in &mut nodes {
        // Inert nodes (no motion commands, no BAN drive, nothing in flight)
        // have nothing to integrate; skipping them also keeps their
        // Transform un-Changed so transform propagation ignores them.
        if cache.motion.is_empty()
            && cache.ban.is_none()
            && motion.velocity == Vec3::ZERO
            && motion.rvel_deg == Vec3::ZERO
            && motion.spin_deg_vel == 0.0
        {
            continue;
        }

        let Some(effect) = effects.get(&node.handle) else {
            continue;
        };
        let node_data = &effect.effect.nodes[node.node];
        let frac = node.life_frac();

        if let Some(ban) = &cache.ban {
            apply_ban_controller(ban, node.age, &bans, &mut motion, &mut transform);
        }

        // Execute program commands on their scheduled effect frames — the
        // original compiles one runtime command per occupied 50 ms row and
        // runs each row's list once (compiler 0xca9070, executor 0xc84f90).
        // Frames crossed since the last run are replayed (bounded after a
        // hitch); forces add their FULL authored magnitude per scheduled
        // frame with no dt — that per-frame impulse IS the authored unit.
        let target = (node.age * EFFECT_FPS) as u32 + 1;
        let crossed = target.saturating_sub(motion.sim_frame).min(16);
        for frame in (target - crossed)..target {
            for slot in &cache.motion {
                if !slot.schedule.contains(frame) {
                    continue;
                }
                let Some(source) = slot.source.resolve(node_data) else {
                    continue;
                };
                use EffectCommand as C;
                match &source.command {
                    C::SetPosition(v) => {
                        let origin = motion.origin;
                        transform.translation = origin + motion.origin_rotation * *v;
                    }
                    C::SetSpherePos(v) => {
                        let offset = motion.origin_rotation * random_in_ellipsoid(*v);
                        transform.translation = motion.origin + offset;
                    }
                    C::SetConePos(cone) => {
                        // Spawn at a random point in a cone around local +Y —
                        // the position sibling of SetConeVel (distance from
                        // x/y, spread from the z half-angle).
                        let dir = random_in_cone(motion.origin_rotation * Vec3::Y, cone.degrees.z);
                        transform.translation = motion.origin + dir * cone_speed(cone.degrees);
                    }
                    C::SetVelocity(v) => {
                        motion.velocity = motion.origin_rotation * *v;
                    }
                    C::SetConeVel(cone) => {
                        let dir = random_in_cone(motion.origin_rotation * Vec3::Y, cone.degrees.z);
                        motion.velocity = dir * cone_speed(cone.degrees);
                    }
                    C::SetRotation(rot) => {
                        motion.base_rotation = mirror_quat(Quat::from_euler(
                            EulerRot::XYZ,
                            rot.euler_degrees.x.to_radians(),
                            rot.euler_degrees.y.to_radians(),
                            rot.euler_degrees.z.to_radians(),
                        ));
                    }
                    C::SetRotationAxis(axis) => {
                        let a = axis.axis_angle;
                        motion.base_rotation = mirror_quat(Quat::from_axis_angle(
                            a.truncate().try_normalize().unwrap_or(Vec3::Y),
                            a.w.to_radians(),
                        ));
                    }
                    C::SetRotationMat(mat) => {
                        motion.base_rotation = mirror_quat(Quat::from_mat3(&Mat3::from_mat4(*mat)));
                    }
                    C::SetRVelocity(rot) => {
                        // Mirror-conjugated like the quats: rotations about
                        // the mirrored X axis keep their sense, Y/Z reverse.
                        let e = rot.euler_degrees;
                        motion.rvel_deg = Vec3::new(e.x, -e.y, -e.z);
                    }
                    C::SetRVelocityAxis(axis) => {
                        let a = axis.axis_angle;
                        motion.spin_axis = mirror_axis(a.truncate());
                        motion.spin_deg_vel = a.w;
                    }
                    C::SetRVelocityMat(mat) => {
                        // Matrix form of rotational velocity: decompose to an
                        // axis-angle spin (the sibling of SetRVelocityAxis).
                        let (axis, angle) = Quat::from_mat3(&Mat3::from_mat4(*mat)).to_axis_angle();
                        motion.spin_axis = mirror_axis(axis);
                        motion.spin_deg_vel = angle.to_degrees();
                    }
                    C::SetShapeRot(axis) => {
                        motion.spin_axis = mirror_axis(axis.axis_angle.truncate());
                        motion.spin_deg = axis.axis_angle.w;
                    }
                    C::SetShapeRotVel(axis) => {
                        motion.spin_axis = mirror_axis(axis.axis_angle.truncate());
                        motion.spin_deg_vel = axis.axis_angle.w;
                    }
                    C::Force(v) => {
                        let accel = motion.origin_rotation * *v;
                        motion.velocity += accel;
                    }
                    C::ConeForce(cone) => {
                        // Exe RE (case 70): identical cone construction to
                        // SetConeVel but *added* to velocity, re-randomized
                        // per scheduled frame.
                        let dir = random_in_cone(motion.origin_rotation * Vec3::Y, cone.degrees.z);
                        motion.velocity += dir * cone_speed(cone.degrees);
                    }
                    C::Attraction(strength) => {
                        let to_origin = motion.origin - transform.translation;
                        let dir = to_origin.try_normalize().unwrap_or(Vec3::ZERO);
                        motion.velocity += dir * *strength;
                    }
                    // BAN paths sample continuously below; graphs in
                    // `sample_graphs`; emission in `emit_particles`.
                    _ => {}
                }
            }
        }
        motion.sim_frame = target;

        // Keyframed BAN position/rotation paths: the keys are per-frame rows,
        // sampled continuously (lerped) for smooth motion at render rate.
        for slot in &cache.motion {
            let Some(source) = slot.source.resolve(node_data) else {
                continue;
            };
            use EffectCommand as C;
            match &source.command {
                C::SetBanPos(keys) => {
                    if let Some(pos) = sample_keyframes(keys, frac, Vec3::lerp) {
                        let origin_rotation = motion.origin_rotation;
                        transform.translation = motion.origin + origin_rotation * pos;
                    }
                }
                C::SetBanRot(keys) => {
                    if let Some(rotation) = sample_mat_rotation(keys, frac) {
                        motion.base_rotation = mirror_quat(rotation);
                    }
                }
                _ => {}
            }
        }

        // Integrate. Authored velocities are units-per-effect-frame (exe RE:
        // `pos += vel` once per 50 ms frame, no dt) — ×EFFECT_FPS converts
        // to per-second so the render-rate integration matches the original
        // trajectory. Writes are guarded so motionless nodes don't dirty
        // their Transform (and drag the subtree through propagation).
        if motion.velocity != Vec3::ZERO {
            transform.translation += motion.velocity * (EFFECT_FPS * dt);
        }
        if motion.rvel_deg.length_squared() > f32::EPSILON {
            let rvel = motion.rvel_deg * (EFFECT_FPS * dt);
            motion.base_rotation *= Quat::from_euler(
                EulerRot::XYZ,
                rvel.x.to_radians(),
                rvel.y.to_radians(),
                rvel.z.to_radians(),
            );
        }
        if motion.spin_deg_vel != 0.0 {
            motion.spin_deg += motion.spin_deg_vel * (EFFECT_FPS * dt);
        }

        // Billboarded plates get their rotation from the billboard system.
        // Shape spin (SetShapeRot/Vel) is RENDER-ONLY in the original — a
        // separate shape matrix multiplied at draw time (exe ctx+0x130),
        // never part of the frame children/emitted particles inherit — so
        // it only composes for nodes that render geometry themselves.
        // Spinning an invisible emitter template would swing its attached
        // particle chain around like a clock hand.
        if billboard.is_none() {
            let spin = if has_visual {
                motion.spin()
            } else {
                Quat::IDENTITY
            };
            let rotation = motion.origin_rotation * motion.base_rotation * spin;
            if transform.rotation != rotation {
                transform.rotation = rotation;
            }
        }
    }
}

/// Samples a SetBANRot matrix track, slerping between adjacent keys.
fn sample_mat_rotation(keys: &[bevy::math::Mat4], frac: f32) -> Option<Quat> {
    match keys.len() {
        0 => None,
        1 => Some(Quat::from_mat3(&Mat3::from_mat4(keys[0]))),
        n => {
            let pos = frac.clamp(0.0, 1.0) * (n - 1) as f32;
            let idx = (pos.floor() as usize).min(n - 2);
            let a = Quat::from_mat3(&Mat3::from_mat4(keys[idx]));
            let b = Quat::from_mat3(&Mat3::from_mat4(keys[idx + 1]));
            Some(a.slerp(b, pos - idx as f32))
        }
    }
}

/// Plays the node's BAN controller animation (first referenced .ban, first
/// animated bone) as emitter/node motion. The handle was resolved once at
/// spawn ([`EffectProgramCache::ban`]) — the controller scan and the
/// path-normalize String allocation no longer run per frame.
fn apply_ban_controller(
    ban_handle: &bevy::asset::Handle<crate::assets::ban::JMXVBAN>,
    age: f32,
    bans: &Assets<crate::assets::ban::JMXVBAN>,
    motion: &mut EffectMotion,
    transform: &mut Transform,
) {
    let Some(ban) = bans.get(ban_handle) else {
        return;
    };
    let Some(bone) = ban.animated_bones.first() else {
        return;
    };
    let n = bone.keyframes.len();
    if n == 0 {
        return;
    }

    let duration = ban.duration.as_secs_f32().max(f32::EPSILON);
    let cyclic = matches!(
        ban.animation_type,
        crate::assets::ban::AnimationType::Cyclic
    );
    let t = if cyclic {
        (age / duration).fract()
    } else {
        (age / duration).clamp(0.0, 1.0)
    };
    let pos = t * (n - 1) as f32;
    let idx = (pos.floor() as usize).min(n.saturating_sub(2));
    let s = pos - idx as f32;
    let (translation, rotation) = if n == 1 {
        bone.keyframes[0]
    } else {
        let (ta, ra) = bone.keyframes[idx];
        let (tb, rb) = bone.keyframes[idx + 1];
        (ta.lerp(tb, s), ra.slerp(rb, s))
    };
    transform.translation = motion.origin + motion.origin_rotation * translation;
    motion.base_rotation = mirror_quat(rotation);
}

/// Scale the RGB channels of a packed `0xAARRGGBB` tint by `factor` (alpha and
/// the falloff shape kept); the per-effect brightness dim.
fn dim_argb_rgb(argb: u32, factor: f32) -> u32 {
    if factor >= 1.0 {
        return argb;
    }
    let chan = |shift: u32| {
        let v = ((argb >> shift) & 0xff) as f32 * factor;
        (v.round().clamp(0.0, 255.0) as u32) << shift
    };
    (argb & 0xff00_0000) | chan(16) | chan(8) | chan(0)
}

fn lerp_u8(a: u8, b: u8, s: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * s)
        .round()
        .clamp(0.0, 255.0) as u8
}

fn lerp_argb(a: Argb, b: Argb, s: f32) -> Argb {
    let (ca, cb) = (argb_channels(a), argb_channels(b));
    let mut out = 0u32;
    for i in 0..4 {
        out = (out << 8) | lerp_u8(ca[i], cb[i], s) as u32;
    }
    out
}

/// (a * b) / 255, rounded — combines two u8 alpha channels.
fn mul_u8(a: u8, b: u8) -> u8 {
    ((a as u32 * b as u32 + 127) / 255) as u8
}

/// Samples an evenly-spaced keyframe array at life fraction `frac`.
fn sample_keyframes<T: Copy>(keys: &[T], frac: f32, lerp: impl Fn(T, T, f32) -> T) -> Option<T> {
    match keys.len() {
        0 => None,
        1 => Some(keys[0]),
        n => {
            let pos = frac.clamp(0.0, 1.0) * (n - 1) as f32;
            let idx = (pos.floor() as usize).min(n - 2);
            Some(lerp(keys[idx], keys[idx + 1], pos - idx as f32))
        }
    }
}

/// Samples the scale graph the node's [`GraphBinding`] resolved to at spawn
/// (priority SetGraphScale program > ScaleGraph controller > global
/// BlendScaleGraph was applied then — they typically carry the same
/// authored graph, so exactly one is sampled).
fn sample_scale(binding: &GraphBinding, node_data: &EfStoredObject, frac: f32) -> Option<Vec3> {
    match binding {
        GraphBinding::None => None,
        GraphBinding::Program(sref) => match sref.resolve(node_data).map(|s| &s.command) {
            Some(EffectCommand::SetGraphScale(keys)) => sample_keyframes(keys, frac, Vec3::lerp),
            _ => None,
        },
        GraphBinding::Controller(i) => match node_data.controllers.get(*i as usize) {
            Some(EfController::ScaleGraph { x, y, z, .. }) => {
                let lerp = |a: f32, b: f32, s: f32| a + (b - a) * s;
                Some(Vec3::new(
                    x.sample(frac, lerp).unwrap_or(1.0),
                    y.sample(frac, lerp).unwrap_or(1.0),
                    z.sample(frac, lerp).unwrap_or(1.0),
                ))
            }
            _ => None,
        },
        GraphBinding::GlobalParam(i) => match node_data.global_params.get(*i as usize) {
            Some((_, EeParameter::BlendScaleGraph(blend))) => blend.sample(frac, Vec3::lerp),
            _ => None,
        },
    }
}

/// Samples the diffuse tint as packed 0xAARRGGBB (sRGB bytes; the shader
/// converts to linear) from the node's spawn-resolved [`GraphBinding`].
fn sample_argb(binding: &GraphBinding, node_data: &EfStoredObject, frac: f32) -> Option<Argb> {
    match binding {
        GraphBinding::None => None,
        GraphBinding::Program(sref) => match sref.resolve(node_data).map(|s| &s.command) {
            Some(EffectCommand::SetGraphDiffuse(keys)) => sample_keyframes(keys, frac, lerp_argb),
            _ => None,
        },
        GraphBinding::Controller(i) => match node_data.controllers.get(*i as usize) {
            Some(EfController::DiffuseGraph { alpha, color }) => {
                let argb = color.sample(frac, lerp_argb);
                let a = alpha.sample(frac, lerp_u8);
                match (argb, a) {
                    (Some(argb), Some(a)) => {
                        let combined = mul_u8((argb >> 24) as u8, a) as u32;
                        Some((argb & 0x00ff_ffff) | (combined << 24))
                    }
                    (Some(argb), None) => Some(argb),
                    (None, Some(a)) => Some(((a as u32) << 24) | 0x00ff_ffff),
                    (None, None) => None,
                }
            }
            _ => None,
        },
        GraphBinding::GlobalParam(i) => match node_data.global_params.get(*i as usize) {
            Some((_, EeParameter::BlendDiffuseGraph(blend))) => blend.sample(frac, lerp_argb),
            _ => None,
        },
    }
}

/// Applies scale/diffuse keyframe graphs to transforms and per-instance
/// `MeshTag` tints. Materials are immutable and shared; only TextureSlide
/// nodes write their private material's uv uniform.
pub fn sample_graphs(
    effects: Res<Assets<JMXVEFF>>,
    mut materials: ResMut<Assets<SroEffectMaterial>>,
    mut nodes: Query<
        (
            Entity,
            &EffectNode,
            &EffectProgramCache,
            &mut Transform,
            Option<(&mut EffectVisual, &mut MeshTag)>,
        ),
        (Without<PooledParticle>, Without<EffectSimPaused>),
    >,
    intensities: Query<&EffectIntensity>,
) {
    for (entity, node, cache, mut transform, visual) in &mut nodes {
        // Nodes without any graph binding (resolved at spawn) sample nothing.
        if cache.scale.is_none()
            && cache.diffuse.is_none()
            && cache.uv.is_none()
            && cache.random_scale.is_none()
        {
            continue;
        }
        let Some(effect) = effects.get(&node.handle) else {
            continue;
        };
        let node_data = &effect.effect.nodes[node.node];
        let frac = node.life_frac();

        // Scale graphs are RENDER-ONLY, like the shape spin: the exe's
        // ScaleGraph/SetGraphRandomScale write the per-instance scale
        // register absolutely at draw time, never a hierarchy transform.
        // Writing the sampled value onto an invisible node's Transform (a
        // leaf emitter anchor template samples the graph meant for its
        // copies) multiplied every emitted copy's offset and inherited
        // size by it via propagation — the talisman's anchors sampled
        // ~5.7 and wrapped the wards in an oversized second haze layer
        // (BRP-measured 2026-07-29).
        if visual.is_some() {
            let scale = sample_random_scale(cache, node_data, entity, node.age)
                .or_else(|| sample_scale(&cache.scale, node_data, frac));
            if let Some(scale) = scale {
                if transform.scale != scale {
                    transform.scale = scale;
                }
            }
        }

        // Per-effect brightness dim (rare auras), applied to the tint RGB.
        let intensity = intensities.get(node.root).map_or(1.0, |i| i.0);
        if let Some((mut visual, mut tag)) = visual {
            if let Some(argb) = sample_argb(&cache.diffuse, node_data, frac) {
                let argb = dim_argb_rgb(argb, intensity);
                if argb != visual.last_argb {
                    tag.0 = argb;
                    visual.last_argb = argb;
                }
            }
            if let (Some(uv_source), Some(uv_material)) = (&cache.uv, visual.uv_material.clone()) {
                if let Some(uv) = sample_texture_slide(uv_source, node_data, node.age) {
                    if uv.distance_squared(visual.last_uv) > 1e-8 {
                        if let Some(mut material) = materials.get_mut(&uv_material) {
                            material.uv_offset_scale = uv;
                        }
                        visual.last_uv = uv;
                    }
                }
            }
        }
    }
}

/// SetGraphRandomScale (exe RE, interpreter case 78): while its window is
/// active, the scale is the effect's shared BlendScaleGraph evaluated at a
/// uniform-random parameter — replacing the regular scale graph and
/// re-rolled per effect frame (the command row executes every frame of its
/// window). The roll is a deterministic hash of (entity, effect frame) so
/// the authored 30fps shimmer is render-rate independent.
fn sample_random_scale(
    cache: &EffectProgramCache,
    node_data: &EfStoredObject,
    entity: Entity,
    age: f32,
) -> Option<Vec3> {
    let rs = cache.random_scale.as_ref()?;
    // Clamp to the schedule end so the LAST roll holds after a short
    // window — and hash the SAME clamped frame. One-shot windows (e.g.
    // the talisman wisps' frames 0..1 initial-size roll) then yield one
    // random size per particle; hashing the unclamped frame re-rolled
    // their scale every effect frame for life — a 20 Hz size flicker
    // the authored data never asked for. Whole-life windows (the Seal
    // lights' shimmer) still re-roll per frame inside the window.
    let frame = ((age * EFFECT_FPS) as u32).min(rs.schedule.end);
    if !rs.schedule.contains(frame) {
        return None;
    }
    let Some((_, EeParameter::BlendScaleGraph(blend))) =
        node_data.global_params.get(rs.param as usize)
    else {
        return None;
    };
    let t = hash01(entity.to_bits() as u32, frame);
    blend.sample(t, Vec3::lerp)
}

/// Deterministic uniform [0,1) from two u32s (splitmix-style avalanche).
fn hash01(a: u32, b: u32) -> f32 {
    let mut h = a.wrapping_mul(0x9E37_79B9) ^ b.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7FEB_352D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846C_A68B);
    h ^= h >> 16;
    (h >> 8) as f32 / 16_777_216.0
}

/// TextureSlide is a flipbook (exe RE + corpus): `left` = (grid cols, grid
/// rows, cells advanced per effect frame), `frames` = a pre-baked array with
/// one (offset_u, offset_v, scale_u, scale_v) grid cell per effect frame.
/// The original indexes the baked array by the instance's age in frames with
/// no interpolation; past the end the last cell holds. The parameter-only
/// copy (empty `frames`) falls back to the original bake formula:
/// `cell = trunc(age_frames · rate)`, wrapping modulo the grid.
fn sample_texture_slide(
    source: &SourceRef,
    node_data: &EfStoredObject,
    age: f32,
) -> Option<bevy::math::Vec4> {
    use bevy::math::Vec4;
    let EffectCommand::TextureSlide(slide) = &source.resolve(node_data)?.command else {
        return None;
    };
    let frame = (age * EFFECT_FPS) as usize;
    if let Some(cell) = slide
        .frames
        .get(frame.min(slide.frames.len().wrapping_sub(1)))
    {
        let valid_scale = cell.z.abs() > f32::EPSILON && cell.w.abs() > f32::EPSILON;
        let (su, sv) = if valid_scale {
            (cell.z, cell.w)
        } else {
            (1.0, 1.0)
        };
        return Some(Vec4::new(cell.x, cell.y, su, sv));
    }
    let cols = slide.left.x.max(1.0);
    let rows = slide.left.y.max(1.0);
    let cell = (frame as f32 * slide.left.z).trunc().max(0.0) as u32;
    let col = cell % cols as u32;
    let row = (cell / cols as u32) % rows as u32;
    Some(Vec4::new(
        col as f32 / cols,
        row as f32 / rows,
        1.0 / cols,
        1.0 / rows,
    ))
}

/// The active world camera: renders to a window, not into an offscreen
/// image. The HUD portrait and inventory paper-doll cameras are `Camera3d`s
/// too — an unfiltered `iter().next()` could return one of those, which
/// made every billboard face the *portrait* camera (flat/edge-on glows
/// whenever the view direction diverged from it).
pub(crate) fn main_world_camera<'a>(
    cameras: &'a Query<(&Camera, &RenderTarget, &GlobalTransform), With<Camera3d>>,
) -> Option<&'a GlobalTransform> {
    cameras
        .iter()
        .find(|(camera, target, _)| camera.is_active && matches!(target, RenderTarget::Window(_)))
        .map(|(_, _, transform)| transform)
}

/// Orients billboarded plates toward the camera (in PostUpdate, before
/// transform propagation; the camera/parent globals are one frame old, which
/// is imperceptible).
///
/// The parent compensation works on the parent's full affine, not its
/// decomposed rotation: mirrored map-object placements (negative-determinant
/// transforms, see `MirroredResource`) and scaled ancestors make
/// `GlobalTransform::rotation()` decomposition lie, and a reflection
/// surviving into the plate's world orientation tilts it out of the screen
/// plane (mirrored lamps' glow halos went edge-on at pitch/yaw-dependent
/// camera angles). Mapping the camera basis through the inverse affine and
/// re-orthonormalizing keeps the plate screen-parallel under any ancestor
/// transform; an in-plane texture mirror can remain, which is invisible
/// for the radial/streak plate textures (and culling is off).
pub fn billboard_effect_nodes(
    cameras: Query<(&Camera, &RenderTarget, &GlobalTransform), With<Camera3d>>,
    parents: Query<&GlobalTransform>,
    mut plates: Query<
        (
            &mut Transform,
            &GlobalTransform,
            &EffectBillboard,
            &ChildOf,
            Option<&EffectMotion>,
        ),
        (Without<PooledParticle>, Without<EffectSimPaused>),
    >,
) {
    let Some(camera) = main_world_camera(&cameras) else {
        return;
    };

    for (mut transform, global, billboard, child_of, motion) in &mut plates {
        let desired = match billboard.0 {
            // Screen-aligned: quad +Z parallel to the camera view axis.
            ViewMode::Billboard => camera.rotation(),
            // SilkroadDoc JMXVEFF: ViewVBillboard is a "vertical billboard
            // (rotates only around the Y-axis)" — i.e. a cylindrical billboard
            // upright in world space, the same constraint as YBillboard, not a
            // free screen-facing quad.
            ViewMode::VBillboard | ViewMode::YBillboard => {
                let mut dir = camera.translation() - global.translation();
                dir.y = 0.0;
                match dir.try_normalize() {
                    Some(dir) => Quat::from_rotation_arc(Vec3::Z, dir),
                    None => continue,
                }
            }
            ViewMode::None => continue,
        };
        let spin = motion.map(EffectMotion::spin).unwrap_or(Quat::IDENTITY);

        let parent_linear = parents
            .get(child_of.parent())
            .map(|p| bevy::math::Mat3::from(p.affine().matrix3))
            .unwrap_or(Mat3::IDENTITY);
        let Some(rotation) = aimed_local_rotation(parent_linear, desired) else {
            continue;
        };
        // Exact equality suppresses idle change ticks without an angular tolerance
        // that could discard slow spin or camera motion. All inputs still run.
        let rotation = rotation * spin;
        if transform.rotation != rotation {
            transform.rotation = rotation;
        }
    }
}

/// The local rotation that makes an entity's world basis equal `desired`
/// under a parent whose full affine linear part is `parent_linear`: map the
/// desired basis into parent-local space through the inverse affine and
/// rebuild an orthonormal frame from it (propagation applies the parent
/// affine back on top). Working on the full affine — not the parent's
/// decomposed rotation — is what keeps this correct under the SRO
/// `scale.x = -1` mirror and scaled ancestors (see
/// [`billboard_effect_nodes`]). Returns `None` when a basis axis
/// degenerates; falls back to `desired` for a non-invertible (zero-scale)
/// ancestor.
pub fn aimed_local_rotation(parent_linear: Mat3, desired: Quat) -> Option<Quat> {
    let inverse = parent_linear.inverse();
    if !inverse.is_finite() {
        return Some(desired);
    }
    let z = (inverse * (desired * Vec3::Z)).try_normalize()?;
    let x = (inverse * (desired * Vec3::X)).try_normalize()?;
    let x = (x - z * x.dot(z)).try_normalize()?;
    Some(Quat::from_mat3(&Mat3::from_cols(x, z.cross(x), z)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::camera::ImageRenderTarget;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::prelude::{Image, World};

    /// Headless pulse probe against REAL authored data (needs the user's
    /// Particles.pk2 under assets/, hence ignored): simulates the Seal-of-Star
    /// aura through the actual tick/emit/program systems at 60 fps and prints
    /// the emitted particles' life-phase distribution plus their aggregate
    /// authored-alpha over time. A steady effect shows uniform phases and a
    /// flat aggregate; a synchronized-respawn bug shows clustered phases and
    /// a sawtooth. Run: cargo test -p client probe_star -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic; needs real assets/Particles.pk2"]
    fn probe_star_aura_pulse() {
        use crate::assets::bms::mesh::JMXVBMS;
        use crate::plugins::effects::spawn::{
            spawn_node_tree, EffectMaterials, EffectMeshes, EffectQuad, EffectSpawnParams,
        };
        use crate::plugins::effects::{
            EffectAdditiveIntensity, EffectLdrAdditive, EffectPlaybackSpeed, LeafEmitPolicy,
        };
        use bevy::asset::io::AssetReader;
        use bevy::mesh::Mesh;
        use bevy::prelude::{Commands, Time, Transform};
        use futures_lite::AsyncReadExt;
        use std::path::PathBuf;
        use std::time::Duration;

        let archive = bevy_pk2::prelude::Archive::configured(&PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../assets/Particles.pk2"
        )));
        let path = PathBuf::from(
            std::env::var("EFP_PROBE")
                .unwrap_or_else(|_| "system/system_raretype_a_step1.efp".into()),
        );
        let mut reader =
            bevy::tasks::block_on(archive.read(&path)).expect("step1.efp in Particles.pk2");
        let mut data = Vec::new();
        bevy::tasks::block_on(reader.read_to_end(&mut data)).expect("read");
        let parsed = crate::assets::efp::format::parse_efp(&data).expect("parse");

        // Authored ground truth the runtime numbers are compared against:
        // per node the program length, emission params, and every motion
        // command with its schedule window.
        println!(
            "{}: root={} root_scale={} nodes={}",
            path.display(),
            parsed.root,
            parsed.root_scale,
            parsed.nodes.len()
        );
        for (i, n) in parsed.nodes.iter().enumerate() {
            println!(
                "node {i} {:?}: program_len={} children={:?} view={:?} emit={:?}",
                n.name,
                n.program_len,
                n.children,
                n.view_mode,
                n.static_emit()
            );
            for src in n.programs.iter().chain(n.decorations.iter()) {
                use EffectCommand as C;
                if matches!(
                    &src.command,
                    C::SetPosition(_)
                        | C::SetSpherePos(_)
                        | C::SetConePos(_)
                        | C::SetVelocity(_)
                        | C::SetConeVel(_)
                        | C::SetRotation(_)
                        | C::SetRotationAxis(_)
                        | C::SetRVelocity(_)
                        | C::SetRVelocityAxis(_)
                        | C::SetShapeRot(_)
                        | C::SetShapeRotVel(_)
                        | C::Force(_)
                        | C::ConeForce(_)
                        | C::Attraction(_)
                ) {
                    println!(
                        "  {:?} window=[start {} step {} end {}]",
                        src.command, src.start, src.step, src.end
                    );
                }
            }
        }

        let asset = JMXVEFF {
            effect: parsed,
            textures: Default::default(),
            meshes: Default::default(),
            animations: Default::default(),
        };

        let mut world = World::new();
        world.init_resource::<Assets<JMXVEFF>>();
        world.init_resource::<Assets<Mesh>>();
        world.init_resource::<Assets<SroEffectMaterial>>();
        world.init_resource::<Assets<JMXVBMS>>();
        world.init_resource::<Assets<crate::assets::ban::JMXVBAN>>();
        world.init_resource::<EffectQuad>();
        world.init_resource::<EffectMeshes>();
        world.init_resource::<EffectMaterials>();
        world.init_resource::<EffectAdditiveIntensity>();
        world.init_resource::<EffectLdrAdditive>();
        world.init_resource::<LeafEmitPolicy>();
        world.init_resource::<EffectPlaybackSpeed>();
        world.insert_resource(Time::<()>::default());

        let handle = world.resource_mut::<Assets<JMXVEFF>>().add(asset);
        // EFP_LEAF=0 turns leaf self-emission off (plate-per-leaf model)
        // for an A/B against the exe-faithful default.
        let leaf_emit = std::env::var("EFP_LEAF").map_or(true, |v| v != "0");
        world.resource_mut::<LeafEmitPolicy>().global = leaf_emit;
        // EFP_ROOT_SCALE emulates the BSR ModData wrapper scale (talisman
        // authors 0.5) for a numeric A/B of how it propagates into spread.
        let wrapper_scale: f32 = std::env::var("EFP_ROOT_SCALE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1.0);
        let root = world
            .spawn((
                Transform::from_scale(Vec3::splat(wrapper_scale)),
                GlobalTransform::from_scale(Vec3::splat(wrapper_scale)),
            ))
            .id();
        let spawn_handle = handle.clone();
        world
            .run_system_once(
                move |mut commands: Commands, mut params: EffectSpawnParams| {
                    let effect = params.effects.get(&spawn_handle).expect("asset");
                    let root_node = effect.effect.root;
                    spawn_node_tree(
                        &mut commands,
                        &mut params.assets,
                        &spawn_handle,
                        effect,
                        root_node,
                        root,
                        root,
                        Transform::IDENTITY,
                        None,
                        false,
                        leaf_emit,
                        0.0,
                        false,
                    );
                },
            )
            .expect("spawn");

        // Per-node aggregate authored alpha: sample each particle's actual
        // DiffuseGraph alpha curve at its life fraction and sum per node —
        // the steadiness metric (a flat sum = a steady collective glow).
        let alpha_handle = handle.clone();
        let node_alpha = move |effects: &Assets<JMXVEFF>, node_idx: usize, frac: f32| -> f32 {
            let effect = effects.get(&alpha_handle).expect("asset");
            effect.effect.nodes[node_idx]
                .controllers
                .iter()
                .find_map(|c| match c {
                    EfController::DiffuseGraph { alpha, .. } => {
                        alpha.sample(frac, lerp_u8).map(|a| a as f32 / 255.0)
                    }
                    _ => None,
                })
                .unwrap_or(0.0)
        };

        let dt = Duration::from_secs_f64(1.0 / 60.0);
        let mut step = |world: &mut World| {
            world.resource_mut::<Time>().advance_by(dt);
            world.run_system_once(tick_effect_nodes).unwrap();
            world.run_system_once(emit_particles).unwrap();
            world.run_system_once(run_programs).unwrap();
        };
        // Warm up past several loop generations (EFP_WARMUP=0 to watch
        // the effect from its very first frame).
        let warmup: usize = std::env::var("EFP_WARMUP")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(360);
        for _ in 0..warmup {
            step(&mut world);
        }
        // Probe two authored loop periods.
        // Visible particles only: self-emitter anchor templates also carry
        // EffectNode + EmittedBy but never render (no EffectVisual).
        let mut query = world
            .query_filtered::<(Entity, &EffectNode, &EmittedBy), (Without<PooledParticle>, With<EffectVisual>)>();
        // World positions are composed by walking ChildOf up to the probe
        // root (this world runs no transform propagation). Any ancestor on
        // the way carrying a non-unit scale (other than the wrapper and the
        // particle's own billboard scale) multiplies authored offsets into
        // the cloud spread — recorded and reported at the end.
        let mut scaled_ancestors: std::collections::BTreeMap<Entity, (String, Vec3)> =
            Default::default();
        for i in 0..180 {
            step(&mut world);
            // (node, life fraction, world position) per visible particle.
            let mut samples: Vec<(usize, f32, Vec3)> = Vec::new();
            for (entity, node, _) in query.iter(&world) {
                let mut chain = vec![entity];
                let mut e = entity;
                while let Some(p) = world.get::<ChildOf>(e) {
                    e = p.0;
                    chain.push(e);
                }
                let mut tf = Transform::IDENTITY;
                for &ent in chain.iter().rev() {
                    if let Some(t) = world.get::<Transform>(ent) {
                        tf = tf.mul_transform(*t);
                        if ent != root && ent != entity && (t.scale - Vec3::ONE).length() > 1e-3 {
                            let name = world
                                .get::<bevy::prelude::Name>(ent)
                                .map(|n| n.to_string())
                                .unwrap_or_default();
                            scaled_ancestors.insert(ent, (name, t.scale));
                        }
                    }
                }
                samples.push((node.node, node.life_frac(), tf.translation));
            }
            samples.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
            // (node, count, aggregate authored alpha, radial distances).
            let mut agg: Vec<(usize, u32, f32, Vec<f32>)> = Vec::new();
            {
                let effects = world.resource::<Assets<JMXVEFF>>();
                for &(n, f, pos) in &samples {
                    let a = node_alpha(effects, n, f);
                    let r = pos.length();
                    match agg.last_mut() {
                        Some((idx, count, sum, radii)) if *idx == n => {
                            *count += 1;
                            *sum += a;
                            radii.push(r);
                        }
                        _ => agg.push((n, 1, a, vec![r])),
                    }
                }
            }
            let summary: Vec<String> = agg
                .iter()
                .map(|(n, c, s, radii)| {
                    let mut r = radii.clone();
                    r.sort_by(f32::total_cmp);
                    format!(
                        "n{n}:{c}x a={s:.3} r={:.2}/{:.2}/{:.2}",
                        r[0],
                        r[r.len() / 2],
                        r[r.len() - 1]
                    )
                })
                .collect();
            println!(
                "t={:5.2}s {}",
                (warmup + i) as f32 / 60.0,
                summary.join("  ")
            );
        }
        if scaled_ancestors.is_empty() {
            println!("no non-unit ancestor scales (beyond the wrapper) on any particle path");
        } else {
            println!("non-unit ancestor scales on particle paths (spread multipliers):");
            for (ent, (name, scale)) in &scaled_ancestors {
                println!("  {ent:?} {name:?} scale={scale:?}");
            }
        }
    }

    #[test]
    fn billboards_skip_idle_writes_but_follow_changed_inputs() {
        use bevy::prelude::DetectChanges;
        let mut world = World::new();
        let camera = world
            .spawn((
                Camera3d::default(),
                Camera::default(),
                RenderTarget::default(),
                GlobalTransform::from(
                    Transform::from_xyz(5.0, 4.0, 8.0).with_rotation(Quat::from_rotation_x(0.4)),
                ),
            ))
            .id();
        let parent = world.spawn(GlobalTransform::IDENTITY).id();
        let plate = world
            .spawn((
                Transform::IDENTITY,
                GlobalTransform::IDENTITY,
                EffectBillboard(ViewMode::Billboard),
                EffectMotion::default(),
                ChildOf(parent),
            ))
            .id();
        world.run_system_once(billboard_effect_nodes).unwrap();
        for input in 0..7 {
            world.clear_trackers();
            world.run_system_once(billboard_effect_nodes).unwrap();
            assert!(
                !world
                    .entity(plate)
                    .get_ref::<Transform>()
                    .unwrap()
                    .is_changed(),
                "stationary pass after input {input} wrote a transform"
            );
            match input {
                0 => {
                    world.entity_mut(camera).insert(GlobalTransform::from(
                        Transform::from_xyz(5.0, 4.0, 8.0)
                            .with_rotation(Quat::from_rotation_y(0.7)),
                    ));
                }
                1 => {
                    world.entity_mut(parent).insert(GlobalTransform::from(
                        Transform::from_scale(Vec3::new(-2.0, 3.0, 0.5))
                            .with_rotation(Quat::from_rotation_x(0.3)),
                    ));
                }
                2 => {
                    world.get_mut::<EffectMotion>(plate).unwrap().spin_deg = 20.0;
                }
                3 => {
                    world.get_mut::<EffectBillboard>(plate).unwrap().0 = ViewMode::YBillboard;
                }
                4 => {
                    world
                        .entity_mut(plate)
                        .insert(GlobalTransform::from_translation(Vec3::X * 3.0));
                }
                5 => {
                    let other = world
                        .spawn(GlobalTransform::from(Transform::from_rotation(
                            Quat::from_rotation_z(0.5),
                        )))
                        .id();
                    world.entity_mut(plate).insert(ChildOf(other));
                }
                _ => {
                    world.get_mut::<Camera>(camera).unwrap().is_active = false;
                    world.spawn((
                        Camera3d::default(),
                        Camera::default(),
                        RenderTarget::default(),
                        GlobalTransform::from_translation(Vec3::new(-10.0, 2.0, 3.0)),
                    ));
                }
            }
            world.run_system_once(billboard_effect_nodes).unwrap();
            assert!(
                world
                    .entity(plate)
                    .get_ref::<Transform>()
                    .unwrap()
                    .is_changed(),
                "input {input} did not update the billboard"
            );
        }
        world.entity_mut(plate).insert(EffectSimPaused);
        world.get_mut::<EffectMotion>(plate).unwrap().spin_deg = 45.0;
        world.clear_trackers();
        world.run_system_once(billboard_effect_nodes).unwrap();
        assert!(!world
            .entity(plate)
            .get_ref::<Transform>()
            .unwrap()
            .is_changed());
        world.entity_mut(plate).remove::<EffectSimPaused>();
        world.run_system_once(billboard_effect_nodes).unwrap();
        assert!(world
            .entity(plate)
            .get_ref::<Transform>()
            .unwrap()
            .is_changed());
    }

    /// Headless check of the billboard math: a plate under a rotated parent
    /// must end up screen-aligned with the ACTIVE WINDOW camera, ignoring
    /// offscreen RTT cameras (HUD portrait/paper-doll) and inactive ones.
    #[test]
    fn billboards_align_with_the_active_window_camera() {
        let mut world = World::new();

        // decoy 1: active offscreen RTT camera (the HUD portrait), facing +X
        let rtt_rotation = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        world.spawn((
            Camera3d::default(),
            Camera::default(),
            RenderTarget::Image(ImageRenderTarget::from(
                bevy::asset::Handle::<Image>::default(),
            )),
            GlobalTransform::from(Transform::from_rotation(rtt_rotation)),
        ));
        // decoy 2: inactive window camera (the parked flycam)
        world.spawn((
            Camera3d::default(),
            Camera {
                is_active: false,
                ..Default::default()
            },
            RenderTarget::default(),
            GlobalTransform::from(Transform::from_rotation(Quat::from_rotation_x(1.0))),
        ));
        // the real main camera, at an arbitrary orbit rotation
        let camera_rotation = Quat::from_euler(EulerRot::YXZ, 2.1, -0.4, 0.0);
        world.spawn((
            Camera3d::default(),
            Camera::default(),
            RenderTarget::default(),
            GlobalTransform::from(Transform::from_rotation(camera_rotation)),
        ));

        // lamp wrapper rotated like a placed map object
        let parent_rotation = Quat::from_rotation_y(0.83);
        let parent = world
            .spawn(GlobalTransform::from(Transform::from_rotation(
                parent_rotation,
            )))
            .id();
        let plate = world
            .spawn((
                Transform::IDENTITY,
                GlobalTransform::IDENTITY,
                EffectBillboard(ViewMode::Billboard),
                ChildOf(parent),
            ))
            .id();

        world
            .run_system_once(billboard_effect_nodes)
            .expect("system runs");

        // what transform propagation will produce as the plate's global
        // rotation: parent global * local
        let local = world.get::<Transform>(plate).unwrap().rotation;
        let global = parent_rotation * local;
        assert!(
            global.angle_between(camera_rotation) < 1e-4,
            "plate should be screen-aligned with the main camera, got {global:?} vs {camera_rotation:?}"
        );
        assert!(
            global.angle_between(rtt_rotation) > 0.5,
            "plate must not track the RTT camera"
        );
    }

    /// A plate under a MIRRORED map-object placement (negative-determinant
    /// parent, `MirroredResource`) must still end up screen-parallel:
    /// rotation-only compensation let the reflection tilt the plate out of
    /// the screen plane (edge-on lamp glow halos, observed live via BRP on
    /// mirrored cj_pal_lamp placements).
    #[test]
    fn billboards_stay_screen_parallel_under_mirrored_parents() {
        let mut world = World::new();

        let camera_rotation = Quat::from_euler(EulerRot::YXZ, 2.1, -0.5, 0.0);
        world.spawn((
            Camera3d::default(),
            Camera::default(),
            RenderTarget::default(),
            GlobalTransform::from(Transform::from_rotation(camera_rotation)),
        ));

        // mirrored placement like the live lamp: x=(0,0,1) y=(0,1,0) z=(1,0,0)
        let parent_matrix = Mat3::from_cols(Vec3::Z, Vec3::Y, Vec3::X);
        assert!(parent_matrix.determinant() < 0.0);
        let parent = world
            .spawn(GlobalTransform::from(bevy::math::Affine3A::from_mat3(
                parent_matrix,
            )))
            .id();
        let plate = world
            .spawn((
                Transform::IDENTITY,
                GlobalTransform::IDENTITY,
                EffectBillboard(ViewMode::Billboard),
                ChildOf(parent),
            ))
            .id();

        world
            .run_system_once(billboard_effect_nodes)
            .expect("system runs");

        // what propagation produces: parent affine * local rotation
        let local = world.get::<Transform>(plate).unwrap().rotation;
        let global = parent_matrix * Mat3::from_quat(local);
        let camera_z = camera_rotation * Vec3::Z;
        let plate_z = global.z_axis.normalize();
        assert!(
            plate_z.dot(camera_z) > 0.999,
            "plate normal must match the camera view axis, got {plate_z:?} vs {camera_z:?}"
        );
        assert!(
            global.x_axis.normalize().dot(camera_z).abs() < 1e-4,
            "plate plane must be screen-parallel"
        );
    }

    /// `aimed_local_rotation` must recompose the desired world basis through
    /// any parent affine — including the SRO character mirror, whose negative
    /// determinant makes decomposed-rotation approaches lie.
    #[test]
    fn aimed_local_rotation_recomposes_desired_basis() {
        let desired = Quat::from_rotation_y(1.2);
        for parent in [
            Mat3::IDENTITY,
            Mat3::from_quat(Quat::from_euler(EulerRot::XYZ, 0.4, 1.0, -0.3)),
            // a mirrored (scale.x = -1) rotated ancestor like the character wrapper
            Mat3::from_quat(Quat::from_rotation_y(0.7))
                * Mat3::from_diagonal(Vec3::new(-1.0, 1.0, 1.0)),
        ] {
            let local = aimed_local_rotation(parent, desired).unwrap();
            let world = parent * Mat3::from_quat(local);
            // +Z and +X land exactly on the desired axes (a residual in-plane
            // reflection may flip Y under the mirrored parent)
            assert!(
                world
                    .z_axis
                    .normalize()
                    .abs_diff_eq(desired * Vec3::Z, 1e-5),
                "{:?}",
                world.z_axis
            );
            assert!(
                world
                    .x_axis
                    .normalize()
                    .abs_diff_eq(desired * Vec3::X, 1e-5),
                "{:?}",
                world.x_axis
            );
        }
    }

    /// Every SRO resource renders under a single X-mirror wrapper
    /// (`scale.x = -1`, `MirroredResource`) and effect nodes are its
    /// descendants, so authored displacement vectors receive the SRO→Bevy
    /// reflection ONCE through transform propagation. `run_programs` must
    /// apply them raw in local space: the Seal smoke's authored
    /// `Force(-0.14,0,0)` must come out at world +X (out along the blade),
    /// not double-mirrored back behind the wielder.
    #[test]
    fn authored_vectors_mirror_exactly_once_through_the_hierarchy() {
        use crate::assets::efp::format::{EeSourceData, EfStoredEffect, EfStoredObject};
        use bevy::prelude::Time;

        let authored_pos = Vec3::new(1.5, 0.25, -0.5);
        let authored_force = Vec3::new(-0.14, 0.0, 0.0);

        let mut node = EfStoredObject::default();
        node.program_len = 20;
        for command in [
            EffectCommand::SetPosition(authored_pos),
            EffectCommand::Force(authored_force),
        ] {
            node.decorations.push(EeSourceData {
                command,
                variant: 0,
                mode: 0,
                start: 0.0,
                step: 1.0,
                end: 1000.0,
            });
        }
        let cache = EffectProgramCache::build(
            &node,
            &JMXVEFF {
                effect: EfStoredEffect::default(),
                textures: Default::default(),
                meshes: Default::default(),
                animations: Default::default(),
            },
        );
        let asset = JMXVEFF {
            effect: EfStoredEffect {
                nodes: vec![node],
                root: 0,
                root_scale: 1.0,
                ..Default::default()
            },
            textures: Default::default(),
            meshes: Default::default(),
            animations: Default::default(),
        };

        let mut world = World::new();
        world.init_resource::<Assets<JMXVEFF>>();
        world.init_resource::<Assets<crate::assets::ban::JMXVBAN>>();
        world.init_resource::<crate::plugins::effects::EffectPlaybackSpeed>();
        world.insert_resource(Time::<()>::default());
        let handle = world.resource_mut::<Assets<JMXVEFF>>().add(asset);

        // the mirrored resource wrapper every SRO object renders under
        let wrapper = world
            .spawn(GlobalTransform::from(Transform::from_scale(Vec3::new(
                -1.0, 1.0, 1.0,
            ))))
            .id();
        let instance = world
            .spawn((
                EffectNode {
                    handle,
                    node: 0,
                    age: 0.0, // frame 0: each command executes exactly once
                    lifespan: Lifespan::Once(1.0),
                    root: wrapper,
                },
                cache,
                EffectMotion::default(),
                Transform::IDENTITY,
                GlobalTransform::IDENTITY,
                ChildOf(wrapper),
            ))
            .id();

        world.run_system_once(run_programs).expect("system runs");

        let wrapper_global = *world.get::<GlobalTransform>(wrapper).unwrap();
        let local_pos = world.get::<Transform>(instance).unwrap().translation;
        let world_pos = wrapper_global.transform_point(local_pos);
        let expected_pos = Vec3::new(-authored_pos.x, authored_pos.y, authored_pos.z);
        assert!(
            (world_pos - expected_pos).length() < 1e-5,
            "SetPosition must be mirrored exactly once (by the hierarchy): \
             world {world_pos:?}, expected {expected_pos:?}"
        );

        let velocity = world.get::<EffectMotion>(instance).unwrap().velocity;
        let world_vel = wrapper_global.affine().transform_vector3(velocity);
        let expected_vel = Vec3::new(-authored_force.x, authored_force.y, authored_force.z);
        assert!(
            (world_vel - expected_vel).length() < 1e-5,
            "Force must be mirrored exactly once (by the hierarchy): \
             world {world_vel:?}, expected {expected_vel:?}"
        );
    }

    fn slide_node(left: Vec3, frames: Vec<bevy::math::Vec4>) -> EfStoredObject {
        use crate::assets::efp::format::EeSourceData;
        let mut node = EfStoredObject::default();
        node.decorations.push(EeSourceData {
            command: EffectCommand::TextureSlide(crate::assets::efp::format::FrameTextureSlide {
                left,
                frames,
            }),
            variant: 0,
            mode: 0,
            start: 0.0,
            step: 1.0,
            end: 1000.0,
        });
        node
    }

    /// The baked flipbook array is indexed by age in effect frames — exact
    /// cells, no interpolation, last cell holds past the end.
    #[test]
    fn texture_slide_steps_through_baked_cells() {
        use bevy::math::Vec4;
        let cells = vec![
            Vec4::new(0.0, 0.0, 0.25, 0.25),
            Vec4::new(0.25, 0.0, 0.25, 0.25),
            Vec4::new(0.5, 0.0, 0.25, 0.25),
        ];
        let node = slide_node(Vec3::new(4.0, 4.0, 1.0), cells);
        let source = SourceRef::Decoration(0);

        // frame 0 and mid-frame 0 (no half-cell interpolation)
        let at = |age: f32| sample_texture_slide(&source, &node, age).unwrap();
        assert_eq!(at(0.0), bevy::math::Vec4::new(0.0, 0.0, 0.25, 0.25));
        assert_eq!(
            at(0.5 / EFFECT_FPS),
            bevy::math::Vec4::new(0.0, 0.0, 0.25, 0.25)
        );
        // frame 1 starts exactly one effect frame in
        assert_eq!(
            at(1.01 / EFFECT_FPS),
            bevy::math::Vec4::new(0.25, 0.0, 0.25, 0.25)
        );
        // past the end the last cell holds
        assert_eq!(at(100.0), bevy::math::Vec4::new(0.5, 0.0, 0.25, 0.25));
    }

    /// The parameter-only copy derives cells with the original bake formula:
    /// cell = trunc(age_frames * rate), wrapping modulo the grid.
    #[test]
    fn texture_slide_derives_cells_from_grid_params() {
        let node = slide_node(Vec3::new(3.0, 2.0, 0.5), Vec::new());
        let source = SourceRef::Decoration(0);
        let at = |frames: f32| sample_texture_slide(&source, &node, frames / EFFECT_FPS).unwrap();

        let third = 1.0 / 3.0;
        // rate 0.5: two frames per cell
        assert_eq!(at(0.0), bevy::math::Vec4::new(0.0, 0.0, third, 0.5));
        assert_eq!(at(2.01), bevy::math::Vec4::new(third, 0.0, third, 0.5));
        // cell 3 wraps to the second row of the 3x2 grid
        assert_eq!(at(6.01), bevy::math::Vec4::new(0.0, 0.5, third, 0.5));
        // cell 6 wraps around the whole grid
        assert_eq!(at(12.01), bevy::math::Vec4::new(0.0, 0.0, third, 0.5));
    }

    /// Cone sampling (exe semantics): every direction stays within the
    /// half-angle of the axis, speed is x + rand*(y-x).
    #[test]
    fn cone_directions_stay_within_the_half_angle() {
        for _ in 0..200 {
            let dir = random_in_cone(Vec3::Y, 30.0);
            assert!((dir.length() - 1.0).abs() < 1e-4);
            assert!(dir.angle_between(Vec3::Y).to_degrees() <= 30.0 + 1e-3);
        }
        for _ in 0..200 {
            let speed = cone_speed(Vec3::new(0.2, 0.6, 45.0));
            assert!((0.2..=0.6).contains(&speed), "speed {speed} out of range");
        }
        // degenerate (0,0,z) cones are authored: speed must be 0, not 1
        assert_eq!(cone_speed(Vec3::new(0.0, 0.0, 360.0)), 0.0);
    }

    /// The exe-traced emission model: a `[0, 20, 1, 20] rate=1` emitter is a
    /// steady one-per-frame stream up to the cap; deaths refund credit; a
    /// `[0, 1, 1, 1]` cascade one-shot fires exactly once; `[0, 30, 2, 15]`
    /// (the rare-aura ribbons) emits every other frame.
    #[test]
    fn emission_follows_the_frame_accumulator_model() {
        use crate::plugins::effects::spawn::EmitParams;
        let stream = EmitParams {
            start_frame: 0,
            window: 20,
            period: 1,
            max_alive: 20,
            rate: 1.0,
        };
        let mut acc = 0.0;
        let spawned: u32 = (0..10)
            .map(|f| emission_spawns(&stream, f, 20, &mut acc))
            .sum();
        assert_eq!(spawned, 10, "one particle per frame");
        // at the cap nothing more spawns …
        acc = 20.0;
        assert_eq!(emission_spawns(&stream, 10, 20, &mut acc), 0);
        // … until a death refunds credit
        acc = (acc - 1.0).max(0.0);
        assert_eq!(emission_spawns(&stream, 11, 20, &mut acc), 1);
        // outside the window: nothing
        assert_eq!(emission_spawns(&stream, 25, 20, &mut acc), 0);

        let one_shot = EmitParams {
            start_frame: 0,
            window: 1,
            period: 1,
            max_alive: 1,
            rate: 1.0,
        };
        let mut acc = 0.0;
        assert_eq!(emission_spawns(&one_shot, 0, 1, &mut acc), 1);
        assert_eq!(emission_spawns(&one_shot, 1, 1, &mut acc), 0);

        let ribbons = EmitParams {
            start_frame: 0,
            window: 30,
            period: 2,
            max_alive: 15,
            rate: 1.0,
        };
        let mut acc = 0.0;
        let spawned: u32 = (0..30)
            .map(|f| emission_spawns(&ribbons, f, 15, &mut acc))
            .sum();
        assert_eq!(spawned, 15, "every other frame across the window");
    }

    /// hash01 must be deterministic and uniform-ish in [0,1).
    #[test]
    fn hash01_is_deterministic_and_bounded() {
        for a in 0..50u32 {
            for b in 0..50u32 {
                let v = hash01(a, b);
                assert_eq!(v, hash01(a, b));
                assert!((0.0..1.0).contains(&v));
            }
        }
        assert_ne!(hash01(1, 2), hash01(1, 3));
    }
}
