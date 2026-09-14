use bevy::asset::Handle;
use bevy::math::{Quat, Vec3};
use bevy::prelude::{Component, Entity};

use crate::assets::ban::JMXVBAN;
use crate::assets::efp::format::{
    EeParameter, EeSourceData, EfController, EfStoredObject, EffectCommand, ViewMode,
};
use crate::assets::efp::{normalize_effect_path, JMXVEFF};
use crate::plugins::effects::material::SroEffectMaterial;

/// Frame rate of effect timelines (frames in `NodeTimeline`, emitter frame
/// counts, per-frame velocities). Exe RE confirmed: the EasyFX sim steps on
/// a fixed 50 ms accumulator (producer 0xbc1490, const 50.0) — 20 fps.
pub const EFFECT_FPS: f32 = 20.0;

/// Fallback lifetime for particles whose node carries no usable frame window.
pub const DEFAULT_PARTICLE_LIFE: f32 = 1.0;

/// Marker on the effect wrapper entity spawned by `spawn_effect`.
#[derive(Component)]
pub struct EffectInstance {
    pub handle: Handle<JMXVEFF>,
}

/// Present until the effect asset (with dependencies) finished loading and
/// the node tree has been instantiated.
#[derive(Component)]
pub struct EffectPendingInit;

/// The effect's ModData entry is flagged "night time only" (street and
/// building lamp glows). Rendered around the clock until a day/night cycle
/// exists; that driver should toggle `Visibility` on entities carrying this.
#[derive(Component)]
pub struct NightOnlyEffect;

/// Simulation paused because the effect root is beyond the fully-fogged
/// distance (see `cull_effect_simulation`). Stamped on the root *and* every
/// descendant, so the per-frame effect systems exclude the whole subtree
/// archetypally instead of paying a per-node root lookup.
#[derive(Component)]
pub struct EffectSimPaused;

/// The effect wrapper is a one-shot (skill hit burst, cast flash): every
/// node plays its program ONCE instead of loop-respawning on its own period.
/// Without it, a 1 s burst node inside a wrapper trimmed to the tree's
/// longest program (3 s of trailing smoke, say) visibly replays three times
/// — the fourth-playtest "skill effect plays 3×" bug. Ambient/aura effects
/// (no marker) keep the loop-respawn that holds them visible continuously.
#[derive(Component)]
pub struct OneShotEffect;

/// How long a node instance lives.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Lifespan {
    /// Lives until the effect root is despawned.
    WhileRootLives,
    /// Despawns after this many seconds.
    Once(f32),
    /// Age wraps at `period` seconds. `respawn` marks synthetic loops
    /// (0-length-timeline nodes emulating the original's whole-effect
    /// replay): their wrap snaps the node back to its spawn state so
    /// integrated velocity can't drift it away across cycles. Authored
    /// NormalTimeLoopLife loops keep their state.
    Loop { period: f32, respawn: bool },
}

/// One live instance of an `EfStoredObject` node.
#[derive(Component)]
pub struct EffectNode {
    pub handle: Handle<JMXVEFF>,
    pub node: usize,
    pub age: f32,
    pub lifespan: Lifespan,
    pub root: Entity,
}

impl EffectNode {
    /// Normalized life fraction in [0, 1] used to sample keyframe graphs.
    pub fn life_frac(&self) -> f32 {
        match self.lifespan {
            Lifespan::Once(duration)
            | Lifespan::Loop {
                period: duration, ..
            } if duration > f32::EPSILON => (self.age / duration).clamp(0.0, 1.0),
            _ => self.age.clamp(0.0, 1.0),
        }
    }
}

/// Emission state for nodes with a StaticEmit controller/source. The node's
/// children act as particle templates instead of being instantiated directly.
/// Alive counts, emission credit, and the reuse pool are kept per child
/// *slot*: every emission tick feeds each template up to the shared
/// `max_alive` cap, so multi-part effects (a flame's 7 part-templates)
/// render all parts simultaneously instead of cycling one global slot
/// through them.
///
/// The pacing model is the exe-traced original (worker 0xc84e70): emission
/// is evaluated once per *effect frame*; on frames where
/// `(frame - start) % period == 0` each slot's accumulator gains
/// `spawn_rate`, clamped at `max_alive`, and the integer part it crosses is
/// the number of particles spawned. A dying particle refunds 1.0 credit.
#[derive(Component)]
pub struct EffectEmitter {
    /// Next effect frame to evaluate (frames since spawn / loop restart).
    pub frame: u32,
    /// Emission parameters, resolved once at spawn (they're static template
    /// data — recomputing them per frame walked the controller/emitter
    /// lists for every emitter).
    pub params: crate::plugins::effects::spawn::EmitParams,
    /// Live emitted particles per child slot.
    pub alive: Vec<u32>,
    /// Emission accumulator per child slot: alive count + fractional credit.
    pub acc: Vec<f32>,
    /// Expired leaf particles parked for reuse, keyed by child slot.
    /// Recycling instead of despawn/respawn eliminates the constant entity
    /// and material-asset churn of steady-state ambient emitters.
    pub pool: Vec<(usize, Entity)>,
}

/// Marks an emitted particle with the emitter that counts it as alive and
/// the emitter child slot it occupies (index into the node's children list).
#[derive(Component, Clone, Copy)]
pub struct EmittedBy {
    pub emitter: Entity,
    pub slot: usize,
}

/// An expired emitted leaf particle parked in its emitter's pool: hidden and
/// excluded from the per-frame effect systems until the emitter re-arms it.
#[derive(Component)]
pub struct PooledParticle;

/// Per-effect playback-rate multiplier on an effect root (`< 1.0` slows its
/// animation). Used to calm the continuously-looping rare-item aura without
/// touching the global effect timing.
#[derive(Component, Clone, Copy)]
pub struct EffectTimeScale(pub f32);

/// Per-effect brightness multiplier on an effect root (`< 1.0` dims it),
/// applied to the per-instance tint so it also dims graph-less-color shine
/// plates, not just the particle count. Scoped so the rare aura can be toned
/// down without affecting the global effect brightness.
#[derive(Component, Clone, Copy)]
pub struct EffectIntensity(pub f32);

/// Index-based reference back into an [`EfStoredObject`]'s command lists,
/// mirroring the `program_sources` chain order (programs, then every
/// Program controller's list, then decorations). Cached at spawn so the
/// per-frame systems reach a source in O(1) instead of re-walking and
/// re-matching the whole chain. Resolution is bounds-checked: after an
/// asset hot-reload the indices may be stale, in which case the command is
/// skipped until the cache is rebuilt (see `rebuild_program_caches`).
#[derive(Clone, Copy)]
pub enum SourceRef {
    /// `node.programs[i]`
    Program(u16),
    /// `node.decorations[i]`
    Decoration(u16),
}

impl SourceRef {
    pub fn resolve<'a>(&self, node: &'a EfStoredObject) -> Option<&'a EeSourceData> {
        match *self {
            SourceRef::Program(i) => node.programs.get(i as usize),
            SourceRef::Decoration(i) => node.decorations.get(i as usize),
        }
    }
}

/// The effect frames a program command executes on: `start`, `start+step`,
/// … up to `end` inclusive, once per occupied frame (exe RE: row compiler
/// 0xca9070 appends one runtime command per occupied row).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Schedule {
    pub start: u32,
    pub end: u32,
    pub step: f32,
}

impl Schedule {
    pub fn contains(&self, frame: u32) -> bool {
        if frame < self.start || frame > self.end {
            return false;
        }
        let k = ((frame - self.start) as f32 / self.step).round();
        (self.start as f32 + k * self.step) as u32 == frame
    }
}

/// Decodes a command's `(mode, start, step, end)` fields into its execution
/// [`Schedule`], given the node's program length in frames — the exe's
/// decoder 0xca0f40: per-field mode bits select absolute frames vs percent
/// of the program length, and the end additionally supports relative /
/// count encodings. Returns `None` when the command never schedules
/// (invalid range or step), exactly like the original skips the source.
pub fn schedule(source: &EeSourceData, len: u32) -> Option<Schedule> {
    let len_f = len as f32;
    let finite = source.start.is_finite() && source.step.is_finite() && source.end.is_finite();
    if !finite {
        return None;
    }
    let mut bits = source.mode;
    let start_mode = bits & 1;
    bits >>= 1;
    let step_mode = bits & 1;
    let end_mode = (bits >> 1) % 5;

    let start = if start_mode == 0 {
        source.start as i64
    } else {
        (source.start / 100.0) as i64 * len as i64
    };
    let step = if step_mode == 0 {
        source.step
    } else {
        len_f * source.step / 100.0
    };
    let end = match end_mode {
        0 => source.end as i64,
        1 => (source.end / 100.0) as i64 * len as i64,
        2 => start + source.end as i64,
        3 => start - (source.end / -100.0) as i64 * len as i64,
        4 if source.end < 1.0 => return None,
        4 => start + (source.end * step) as i64,
        _ => unreachable!(),
    };
    let end = end.min(len as i64 - 1);
    if start > end || end > len as i64 || step.abs() <= 1e-6 {
        return None;
    }
    Some(Schedule {
        start: start.max(0) as u32,
        end: end.max(0) as u32,
        step,
    })
}

/// One motion-relevant program command with its precomputed [`Schedule`].
pub struct ProgramSlot {
    pub source: SourceRef,
    pub schedule: Schedule,
}

/// Which tier a scale/diffuse graph resolves to, with the runtime's
/// program > controller > global-param priority already applied at spawn —
/// replaces a 3-tier miss chain of chain-walk + controller scan +
/// global-param scan per node per frame.
#[derive(Clone, Copy)]
pub enum GraphBinding {
    None,
    /// A `SetGraphScale`/`SetGraphDiffuse` command with non-empty keys.
    Program(SourceRef),
    /// `node.controllers[i]` is the ScaleGraph/DiffuseGraph controller.
    Controller(u16),
    /// `node.global_params[i]` is the BlendScale/BlendDiffuseGraph param.
    GlobalParam(u16),
}

impl GraphBinding {
    pub fn is_none(&self) -> bool {
        matches!(self, GraphBinding::None)
    }
}

/// Spawn-time resolution of everything the per-frame effect systems would
/// otherwise re-derive from the node template each frame. All fields are
/// pure functions of the immutable `(Handle<JMXVEFF>, node index)` pair, so
/// they hold for the node's whole life (including pooled re-arm, which
/// rebuilds the cache anyway).
#[derive(Component)]
pub struct EffectProgramCache {
    /// Sources whose commands `run_programs` executes, in chain order.
    pub motion: Vec<ProgramSlot>,
    pub scale: GraphBinding,
    pub diffuse: GraphBinding,
    /// The node's TextureSlide source, if any (implies a private uv
    /// material). Nodes carry the command twice — a parameter-only copy
    /// (grid cols/rows/rate) and one with the pre-baked per-frame cell
    /// array; the baked copy is preferred.
    pub uv: Option<SourceRef>,
    /// SetGraphRandomScale (exe RE, interpreter case 78): while its window
    /// is active the node's scale is the effect's shared BlendScaleGraph
    /// evaluated at a fresh uniform-random parameter every effect frame,
    /// replacing the regular scale graph. The serialized u32 payload is a
    /// dead editor pointer; without a non-empty BlendScaleGraph global the
    /// command is a no-op, so it only binds when one exists.
    pub random_scale: Option<RandomScaleBinding>,
    /// Resolved BAN controller animation — replaces a per-frame controller
    /// scan + path-normalize String allocation + HashMap lookup.
    pub ban: Option<Handle<JMXVBAN>>,
}

/// See [`EffectProgramCache::random_scale`].
#[derive(Clone, Copy)]
pub struct RandomScaleBinding {
    /// `node.global_params[param]` is the shared BlendScaleGraph.
    pub param: u16,
    /// Frames the re-roll executes on.
    pub schedule: Schedule,
}

impl EffectProgramCache {
    pub fn build(node: &EfStoredObject, effect: &JMXVEFF) -> Self {
        use EffectCommand as C;

        // The node's program length in frames — the base the percent-mode
        // schedules resolve against.
        let len = crate::plugins::effects::spawn::node_program_len(node);

        // Only the decorations list (the exe's CEEProgram at +0x234, whose
        // row vector the executor runs) schedules commands; the `programs`
        // list (+0x130) carries the ProgramUpdate marker. The `Program`
        // CONTROLLER holds a byte-identical static copy of the decoration
        // commands (the command registry) that the original never executes —
        // chaining it too doubled every Force impulse and spawn command.
        let mut refs: Vec<SourceRef> = Vec::new();
        for i in 0..node.programs.len() {
            refs.push(SourceRef::Program(i as u16));
        }
        for i in 0..node.decorations.len() {
            refs.push(SourceRef::Decoration(i as u16));
        }

        let mut motion = Vec::new();
        let mut scale = GraphBinding::None;
        let mut diffuse = GraphBinding::None;
        let mut uv = None;
        let mut uv_baked = false;
        let mut random_scale_schedule = None;
        for sref in refs.iter() {
            let Some(source) = sref.resolve(node) else {
                continue;
            };
            match &source.command {
                C::SetPosition(_)
                | C::SetSpherePos(_)
                | C::SetConePos(_)
                | C::SetVelocity(_)
                | C::SetConeVel(_)
                | C::SetRotation(_)
                | C::SetRotationAxis(_)
                | C::SetRotationMat(_)
                | C::SetRVelocity(_)
                | C::SetRVelocityAxis(_)
                | C::SetRVelocityMat(_)
                | C::SetShapeRot(_)
                | C::SetShapeRotVel(_)
                | C::Force(_)
                | C::ConeForce(_)
                | C::Attraction(_)
                | C::SetBanPos(_)
                | C::SetBanRot(_) => {
                    // Commands whose schedule never fires are skipped, like
                    // the original's compiler skipping the source.
                    if let Some(schedule) = schedule(source, len) {
                        motion.push(ProgramSlot {
                            source: *sref,
                            schedule,
                        });
                    }
                }
                // Empty-key graph commands fall through to the next tier at
                // runtime (sample_keyframes returns None on them), so only
                // non-empty ones bind here.
                C::SetGraphScale(keys) if !keys.is_empty() && scale.is_none() => {
                    scale = GraphBinding::Program(*sref);
                }
                C::SetGraphDiffuse(keys) if !keys.is_empty() && diffuse.is_none() => {
                    diffuse = GraphBinding::Program(*sref);
                }
                C::TextureSlide(slide)
                    if uv.is_none() || (!uv_baked && !slide.frames.is_empty()) =>
                {
                    uv = Some(*sref);
                    uv_baked = !slide.frames.is_empty();
                }
                C::SetGraphRandomScale(_) if random_scale_schedule.is_none() => {
                    random_scale_schedule = schedule(source, len);
                }
                _ => {}
            }
        }

        // Controller tier: wins over global params whenever present, even
        // with empty graphs (matching the runtime's early return). Global
        // tier: the runtime's find_map skips params whose sample is None,
        // which is static (empty blend); probe with t=0 to replicate.
        if scale.is_none() {
            if let Some(i) = node
                .controllers
                .iter()
                .position(|c| matches!(c, EfController::ScaleGraph { .. }))
            {
                scale = GraphBinding::Controller(i as u16);
            } else if let Some(i) = node.global_params.iter().position(|(_, p)| {
                matches!(p, EeParameter::BlendScaleGraph(blend) if blend.sample(0.0, |a, _, _| a).is_some())
            }) {
                scale = GraphBinding::GlobalParam(i as u16);
            }
        }
        if diffuse.is_none() {
            if let Some(i) = node
                .controllers
                .iter()
                .position(|c| matches!(c, EfController::DiffuseGraph { .. }))
            {
                diffuse = GraphBinding::Controller(i as u16);
            } else if let Some(i) = node.global_params.iter().position(|(_, p)| {
                matches!(p, EeParameter::BlendDiffuseGraph(blend) if blend.sample(0.0, |a, _, _| a).is_some())
            }) {
                diffuse = GraphBinding::GlobalParam(i as u16);
            }
        }

        let random_scale = random_scale_schedule.and_then(|schedule| {
            node.global_params
                .iter()
                .position(|(_, p)| {
                    matches!(p, EeParameter::BlendScaleGraph(blend)
                        if blend.sample(0.0, |a, _, _| a).is_some())
                })
                .map(|i| RandomScaleBinding {
                    param: i as u16,
                    schedule,
                })
        });

        let ban = node
            .controllers
            .iter()
            .find_map(|c| match c {
                EfController::Ban(paths) => Some(paths),
                _ => None,
            })
            .and_then(|paths| paths.iter().find(|p| !p.is_empty()))
            .and_then(|path| effect.animations.get(&normalize_effect_path(path)).cloned());

        Self {
            motion,
            scale,
            diffuse,
            uv,
            random_scale,
            ban,
        }
    }
}

/// Per-instance motion state driven by the node's program commands.
#[derive(Component)]
pub struct EffectMotion {
    /// Local translation/rotation at spawn (emitter pose for detached
    /// particles); position commands are relative to this.
    pub origin: Vec3,
    pub origin_rotation: Quat,
    pub velocity: Vec3,
    /// Euler rotation velocity in degrees/second (SetRVelocity).
    pub rvel_deg: Vec3,
    /// Base orientation from SetRotation-style commands.
    pub base_rotation: Quat,
    /// Shape spin (SetShapeRot/SetShapeRotVel), composed after billboarding.
    pub spin_axis: Vec3,
    pub spin_deg: f32,
    pub spin_deg_vel: f32,
    /// Next effect frame the program simulation evaluates (frames since
    /// spawn / loop restart) — commands execute on their scheduled frames
    /// exactly once, like the original's per-frame row lists.
    pub sim_frame: u32,
}

impl Default for EffectMotion {
    fn default() -> Self {
        Self {
            origin: Vec3::ZERO,
            origin_rotation: Quat::IDENTITY,
            velocity: Vec3::ZERO,
            rvel_deg: Vec3::ZERO,
            base_rotation: Quat::IDENTITY,
            spin_axis: Vec3::Y,
            spin_deg: 0.0,
            spin_deg_vel: 0.0,
            sim_frame: 0,
        }
    }
}

impl EffectMotion {
    pub fn spin(&self) -> Quat {
        if self.spin_deg.abs() < f32::EPSILON {
            Quat::IDENTITY
        } else {
            Quat::from_axis_angle(
                self.spin_axis.try_normalize().unwrap_or(Vec3::Y),
                self.spin_deg.to_radians(),
            )
        }
    }
}

/// A rendered node. The animated diffuse-graph tint travels as a packed
/// ARGB `MeshTag` (unpacked in sro_effect.wgsl), so materials stay immutable
/// and are shared per (texture, blend) — per-frame material writes would
/// re-prepare every material's bind group and defeat batching. Only
/// TextureSlide nodes keep a private material whose `uv_offset_scale`
/// uniform is animated.
#[derive(Component)]
pub struct EffectVisual {
    /// Private material of a TextureSlide node; None = shared cached material.
    pub uv_material: Option<Handle<SroEffectMaterial>>,
    /// Last tint written to the `MeshTag`, packed 0xAARRGGBB (sRGB bytes).
    pub last_argb: u32,
    pub last_uv: bevy::math::Vec4,
}

/// Billboard orientation mode for rendered plates.
#[derive(Component)]
pub struct EffectBillboard(pub ViewMode);
