//! The skill-cast pipeline and its offline/online split.
//!
//! Idea: the underbar (and later the world click paths) only emit
//! [`CastRequest`]s. With a live agent connection the request becomes the
//! same experimental 0x7074 CastSkill packet as before — the server then
//! drives presentation through 0xB070. In the offline Skills test scene the
//! [`LocalCombat`] resource is present instead and [`dispatch_casts`] runs
//! the whole loop locally: validate against the [`SkillBook`] + skilldata
//! target rules, resolve the skill's shot animation from skilleffect.txt
//! ([`SkillSwing`] → played one-shot by the player plugin, which reports the
//! clip's combat-hit keytimes back as [`SkillSwingStarted`]), fire the
//! Shot-phase `.efp` emissions timed to those keytimes, schedule flat
//! per-hit damage ([`PendingHits`]), and apply self-buffs/status effects.

use bevy::prelude::*;

use packets::agent::prelude::CharacterFinished;

use crate::assets::textdata::skilldata::SkillDataRow;
use crate::assets::textdata::skilleffect::{slot_to_anim_type, EffectPhase};
use crate::commands::SkeletonBinding;
use crate::plugins::combat::action_slot::{Action, ActionRefused, ActionSlot};
use crate::plugins::combat::{
    ActiveAttack, AttackOrder, AttackSwing, FaceTarget, HitcountSet, QueuedPopup, TimedEffect,
};
use crate::plugins::effects::spawn::DelayedEffect;
use crate::plugins::effects::OneShotEffect;
use crate::plugins::hud::player_mini_info::PlayerVitals;
use crate::plugins::net::entities::EntityVitals;
use crate::plugins::player::Player;
use crate::plugins::skills::book::SkillBook;
use crate::plugins::skills::status::{apply_status_effects, IncapacitatedFilter};
use crate::plugins::skills::{EquippedWeapon, LocalCombat};
use crate::plugins::textdata::{ClientSkillData, ClientSkillEffects};

/// Ask to cast a skill (by skilldata id) at an optional target entity.
/// Emitted by the underbar; consumed by [`dispatch_casts`].
#[derive(Message)]
pub struct CastRequest {
    pub skill_id: u32,
    pub target: Option<Entity>,
}

/// A locally simulated cast ready for presentation: the player plugin plays
/// the charge (ready) then shot clip on `owner`'s body wrapper and reports
/// back via [`SkillSwingStarted`].
#[derive(Message)]
pub struct SkillSwing {
    pub owner: Entity,
    /// Ref skill id of the cast, when the writer knows it — lets consumers
    /// resolve the EXACT skilldata row (a codename scan lands on an
    /// arbitrary level of the group).
    pub skill_id: Option<u32>,
    /// skilleffect codename (skilldata `basic_group`) for the effect lookup.
    pub codename: String,
    /// Animation group of the aniset (lowercased to match .bsr group names);
    /// `None` falls back to the wrapper's own group.
    pub anim_group: Option<String>,
    /// Charge (ready) animation type id, held during `preparing_secs`; `None`
    /// skips straight to the wait pose (or the shot, if that's absent too).
    pub ready_anim_type: Option<u32>,
    /// Wait/casting animation type id (skilleffect `AniWait`, col 8), held
    /// during the charge's second sub-phase (`charge_secs - preparing_secs`);
    /// `None` leaves whatever's already showing (the ready pose, or nothing)
    /// alone for that span rather than clearing it.
    pub wait_anim_type: Option<u32>,
    /// Shot animation type id; `None` falls back to basic-attack cycling.
    pub anim_type: Option<u32>,
    /// Wind-up before the shot fires, in seconds (skilldata preparing +
    /// casting time) — the charge phase plays the ready clip then the wait
    /// clip (each held for its own authored sub-duration) + READY
    /// emissions throughout, then the shot lands.
    pub charge_secs: f32,
    /// The `preparing_secs` (skilldata col 11) portion of `charge_secs` —
    /// the ready pose's own duration; the remainder (`charge_secs -
    /// preparing_secs`) is the wait pose's.
    pub preparing_secs: f32,
    /// Authored projectile speed in world units/sec (skilldata col 16,
    /// Action_FlyingSpeed); `None` = the fixed presentation fallback.
    pub flying_speed: Option<f32>,
    /// Damage template: every combat-hit keytime of the resolved clip lands
    /// one hit of `per_hit` on `target` (popup + HP decrement). Offline local
    /// simulation only — the flat per-hit amount comes from [`LocalCombat`],
    /// not a server packet.
    pub damage: Option<SwingDamage>,
    /// The cast's victim, on EVERY path — deliberately independent of
    /// [`Self::damage`], which only the offline sim populates.
    ///
    /// This is what [`spawn_cast_effects`] needs: every target-directed
    /// emission (`AT_MOV_*` projectiles, `AT_TARGET`/`AT_TARGET_F`/
    /// `AT_DMG_POS` impacts, the aniset `DamageEfp`) is gated on knowing where
    /// the victim is. Deriving it from `damage` meant online casts — where
    /// `damage` is always `None` because the numbers ride
    /// [`Self::popups`] — silently dropped all of them, so no skill projectile
    /// or impact effect had ever played online (bows included).
    pub target: Option<Entity>,
    /// Real server-computed damage popups (`combat::queue_damage_popups`),
    /// released once this swing's clip resolves its combat-hit keytimes —
    /// each `hit_index` picks the matching keytime, same recipe as
    /// [`crate::plugins::combat::AttackSwing::popups`]. Online path only;
    /// empty for offline sim and for the click-predicted swing (whose real
    /// damage hasn't arrived yet — see [`SkillSwingTimings`]).
    pub popups: Vec<QueuedPopup>,
    /// The server's cast-instance id (0xB070 `instance`), when this swing is
    /// driven by a real packet — `None` for the offline sim and for the
    /// click-predicted swing, which fires before the server names one.
    /// Threaded into [`SkillSwingStarted`] so [`SkillSwingTimings`] can
    /// correlate a later 0xB071's deferred damage back to this swing's
    /// resolved timing.
    pub instance: Option<u32>,
}

#[derive(Clone, Copy)]
pub struct SwingDamage {
    pub target: Entity,
    pub per_hit: u32,
    pub set: HitcountSet,
}

/// The cast started on `wrapper`: the charge duration before the shot, and
/// the shot clip's combat-hit keytimes (seconds into the shot clip). READY
/// emissions fire at charge start, SHOT emissions at `charge_secs` + keytime.
#[derive(Message)]
pub struct SkillSwingStarted {
    pub owner: Entity,
    pub wrapper: Entity,
    /// Ref skill id carried over from the [`SkillSwing`], when known.
    pub skill_id: Option<u32>,
    pub codename: String,
    pub charge_secs: f32,
    pub hit_times: Vec<f32>,
    /// The played clip's own length, so [`RunningSwings`] knows when the body
    /// is free again. `0.0` when no clip played (an imbue, or a group missing
    /// the shot animation).
    pub duration_secs: f32,
    /// The offensive cast's victim (target-anchored and projectile
    /// emissions need it).
    pub target: Option<Entity>,
    /// Authored projectile speed (skilldata col 16), carried over from the
    /// [`SkillSwing`].
    pub flying_speed: Option<f32>,
    /// Carried over from [`SkillSwing::instance`] — lets
    /// [`track_skill_swing_timing`] file this swing's resolved timing under
    /// the right key ([`SkillSwingTimings`]).
    pub instance: Option<u32>,
}

/// A skill emission flying from the caster toward its target (`AT_MOV_*`
/// act types); despawned on arrival. `.efp` projectiles start flying once
/// their [`DelayedEffect`] keytime elapses; `.bsr` model projectiles (bow
/// arrows) use [`ProjectileLaunch`] instead.
#[derive(Component)]
pub struct SkillProjectile {
    pub to: Vec3,
    pub speed: f32,
    /// `MOV_UPR` (effectset MovTypeSpeed col): fly a parabolic arc peaking
    /// at this height (Param col, SRO units; 0 = derive from the distance).
    /// `None` = straight line.
    pub arc: Option<f32>,
    /// Whether the flyer is a `.bsr` model (−Z forward) or a directional
    /// `.efp` (+Z forward) — picks the rotation recipe when the arc bends
    /// the flight tangent.
    pub model_frame: bool,
    /// Flight baseline, captured on the first moving tick (after any
    /// [`LaunchFrom`] re-seat), and the fraction of the path covered.
    pub from: Option<Vec3>,
    pub progress: f32,
}

/// A model projectile ([`SkillProjectile`] on a `.bsr` arrow) held hidden at
/// its start until the launch delay elapses, then revealed and flown.
#[derive(Component)]
pub struct ProjectileLaunch(pub Timer);

/// A bone-parented effect whose WORLD orientation is held fixed at `rotation`
/// while its position rides the animated bone: [`orient_aimed_bone_effects`]
/// rewrites the local rotation each frame as inverse(parent affine linear) ×
/// desired, re-orthonormalized — the mirror-safe recipe of
/// `billboard_effect_nodes`.
#[derive(Component)]
pub struct AimedBoneEffect {
    pub rotation: Quat,
}

/// Seed this projectile's flight start from an anchor entity's world
/// position, sampled when `sample` fires (ticked by
/// [`move_skill_projectiles`], so a `DelayedEffect`-gated projectile starts
/// counting at its keytime). The bow arrow samples at CHARGE END — the held
/// full-draw pose — because by the launch keytime the shot clip has already
/// swung the draw hand (and the nock riding it) down to the waist; `.efp`
/// projectiles sample immediately (the draw pose develops during the charge,
/// so the cast-time body-center point is an arm's reach off).
#[derive(Component)]
pub struct LaunchFrom {
    pub anchor: Entity,
    pub sample: Timer,
}

/// The draw-phase force aura this arrow adopts on its launch frame: the
/// wrapper is reparented from the draw hand into the arrow's frame with its
/// running effect state intact. Vanilla launches the nocked arrow OBJECT
/// itself, so its aura's age (scale/alpha graphs) carries into flight —
/// spawning a fresh instance instead restarts those graphs and made the
/// aura visibly shrink at release.
#[derive(Component)]
pub struct AdoptEffectAtLaunch(pub Entity);

/// One scheduled damage application, released at its clip-synced moment.
pub struct PendingHit {
    pub target: Entity,
    pub amount: u32,
    /// `Time::elapsed_secs_f64` timestamp to apply at.
    pub at: f64,
}

/// Damage waiting for its visual hit moment (local simulation only).
#[derive(Resource, Default)]
pub struct PendingHits(pub Vec<PendingHit>);

/// Per-skill cooldowns (skilldata `ReuseDelay`), keyed by skill id →
/// ready-at timestamp. Local simulation only; the server owns this online.
#[derive(Resource, Default)]
pub struct SkillCooldowns(pub std::collections::HashMap<u32, f64>);

impl SkillCooldowns {
    pub fn ready(&self, skill_id: u32, now: f64) -> bool {
        self.0.get(&skill_id).is_none_or(|&at| now >= at)
    }
}

/// How long an emission wrapper outlives its start moment before cleanup —
/// our effect runtime loops programs, so this is what makes one-shot cast
/// visuals actually end.
const EMISSION_LINGER_SECS: f32 = 2.5;
/// Rough mid-body height of a character, in world units (SRO bodies are
/// ~18 tall) — where projectiles/target flashes anchor.
const EFFECT_MID_HEIGHT: f32 = 9.0;
/// Last-resort flight duration, for an emission that authors **no** speed at
/// all — neither an effectset `MovTypeSpeed` nor a skilldata
/// `Action_FlyingSpeed`. Kept short so the projectile arrives about when the
/// damage lands (charge + keytime, with no flight added).
const PROJECTILE_FLIGHT_SECS: f32 = 0.35;

/// The flight speed of one emission, in world units/sec.
///
/// **The emission's own `MovTypeSpeed` (skilleffect col 14) wins**, because it
/// is the only per-object speed the data has. Skilldata's
/// `Action_FlyingSpeed` (col 16) is not one: censused over the user's
/// `Media.pk2`, every skill carrying an `AT_MOV_*` emission has col 16 equal
/// to either **0 (71 skills) or 400 (58 skills)** — a has-a-projectile flag,
/// not a speed. The effectset meanwhile authors the real spread: 100, 150,
/// 200, 250, 300, 350, 400, 420, 450, 500, 600, plus a few ramps.
///
/// They also disagree almost always — of the 606 `AT_MOV_*` rows, 183 join a
/// player skilldata row and only **9 agree**. Reading col 16 therefore flew
/// everything at a uniform 400: Soul Cut Blade's blade force (authored 300
/// over its 120-unit range) arrived in 0.30 s instead of 0.40, and a bow's
/// arrow (authored 500) in 0.375 s instead of 0.30.
///
/// The `AT_TARGET` motion path already preferred the authored value; this is
/// what makes the caster→target path agree with it.
///
/// Ramped rows (`250,300`, `230,0`) keep only the start speed — constant-speed
/// flight is the existing simplification, not something changed here.
fn projectile_speed(mov_speed: f32, flying_speed: Option<f32>, distance: f32) -> f32 {
    if mov_speed > 0.0 {
        return mov_speed;
    }
    flying_speed
        .filter(|speed| *speed > 0.0)
        .unwrap_or((distance / PROJECTILE_FLIGHT_SECS).max(1.0))
}
/// Flying-projectile model scale. The old 2.5 "visibility bump" was tuned
/// when the arrow mesh never actually rendered (the group-less `.bsr` bug);
/// with the real 14-unit shaft visible, authored size is right — anything
/// larger makes the arrow balloon at launch next to the 1:1 nocked one.
const ARROW_SCALE: f32 = 1.0;
/// Arc apex of a `MOV_UPR` projectile: playtest-calibrated reading is that
/// Param is an initial vertical SPEED (units/s), whose ballistic apex over a
/// flight of duration `T` is `v·T/4`.
///
/// `T` is now the projectile's REAL flight time (distance / its authored
/// speed). It used to be the fixed [`PROJECTILE_FLIGHT_SECS`] folded into a
/// constant, which was fair while every projectile flew for 0.35 s — but once
/// the authored `MovTypeSpeed` is honoured ([`projectile_speed`]) flight time
/// varies per emission and per range, and a constant apex would make a slow
/// long lob arc exactly as low as a fast short one. Same numbers as before
/// wherever the flight really does last 0.35 s.
fn arc_apex(param: f32, flight_secs: f32) -> f32 {
    param * flight_secs / 4.0
}
/// Arc apex as a distance fraction for the few `MOV_UPR` rows authored with
/// Param 0.
const ARC_DEFAULT_FRACTION: f32 = 0.05;

/// 0xB070 cast instances awaiting their 0xB071 end, for damage attribution
/// (this server delivers targeted skill damage in the 0xB071 tail). Entries
/// are removed by `on_skill_end` and pruned after [`CAST_INSTANCE_TTL_SECS`]
/// so unended casts can't leak.
#[derive(Resource, Default)]
pub struct CastInstances(pub std::collections::HashMap<u32, CastInstanceInfo>);

pub struct CastInstanceInfo {
    /// Unique id of the casting entity.
    pub source: u32,
    /// Parsed-but-unconsumed until per-skill damage timing lands.
    #[allow(dead_code)]
    pub skill_id: u32,
    /// `Time::elapsed_secs_f64` when the 0xB070 arrived, for pruning.
    pub recorded_at: f64,
}

pub const CAST_INSTANCE_TTL_SECS: f64 = 30.0;

/// Whether the caster is already close enough to act on `target` right now.
///
/// This is the gate that keeps prediction honest. The server walks the
/// character into range before executing a cast (`combat::close_attack_gap`
/// mirrors the remainder locally), and vanilla plays the swing at *execution*,
/// not at the keypress. Predicting unconditionally would therefore animate a
/// full skill while the body is still sprinting at its victim.
///
/// An untargeted cast (a self-buff) has nothing to approach, so it is always in
/// range. A targeted one is measured against the same reach the gap-close uses,
/// so "close enough" means one thing in both systems.
pub fn within_cast_range(
    caster: Entity,
    target: Option<Entity>,
    reach: f32,
    transforms: &Query<&GlobalTransform>,
) -> bool {
    let Some(target) = target else {
        return true;
    };
    let (Ok(from), Ok(to)) = (transforms.get(caster), transforms.get(target)) else {
        // Not placed yet — let the server drive rather than guess.
        return false;
    };
    from.translation().distance(to.translation()) <= reach
}

/// How long a local prediction suppresses the server's own copy of the cast.
///
/// The window only has to cover the round trip: we predict *only* when already
/// inside the stop range, so the server has no approach walk to do first. Two
/// seconds is generous for that and still short enough that a genuine re-cast
/// of the same skill a moment later is presented rather than swallowed.
const PREDICTION_WINDOW_SECS: f64 = 2.0;

/// Casts already presented locally, so the server's announcement of the *same*
/// cast does not present them a second time.
///
/// Idea: with prediction on, one keypress produces two presentation triggers —
/// ours at the keypress and the server's 0xB070 a round trip later. Nothing
/// deduplicates `SkillSwing`, so the second one restarts the clip mid-swing,
/// re-arms the charge, spawns a *second* projectile and impact effect, and
/// re-fires every animation-keyed sound. This records what we already played so
/// `on_object_action_update` can drop its copy.
///
/// Keyed by skill id rather than by an instance id because the client has no
/// cast instance until the server names one — the id plus a short window is
/// the most specific key available at keypress time.
#[derive(Resource, Default)]
pub struct PredictedCasts {
    skills: std::collections::HashMap<u32, f64>,
    /// The opening auto-attack swing, which has no skill id to key on: the
    /// client issues `Attack`, not a numbered skill, and only learns the id the
    /// server chose when 0xB070 comes back.
    auto_attack: Option<f64>,
}

impl PredictedCasts {
    /// Record that we have presented `skill_id` locally.
    pub fn record(&mut self, skill_id: u32, now: f64) {
        self.skills.insert(skill_id, now);
    }

    /// Whether the server's announcement of `skill_id` is the echo of a cast we
    /// already played. Consumes the record, so only the *first* announcement is
    /// suppressed and a genuine second cast still presents.
    pub fn claim_echo(&mut self, skill_id: u32, now: f64) -> bool {
        Self::claim(&mut self.skills.remove(&skill_id), now)
    }

    /// Record that we have presented the opening auto-attack swing locally.
    pub fn record_auto_attack(&mut self, now: f64) {
        self.auto_attack = Some(now);
    }

    /// Whether an incoming basic-attack swing is the echo of the opening one we
    /// already played. Only the first is ever suppressed — the rest of the loop
    /// stays server-timed, so its cadence cannot drift from the server's.
    pub fn claim_auto_attack_echo(&mut self, now: f64) -> bool {
        Self::claim(&mut self.auto_attack.take(), now)
    }

    /// Forget a prediction whose cast the server refused, so the retry that
    /// follows presents normally instead of being mistaken for an echo.
    ///
    /// Returns whether there WAS a prediction — which is what tells the caller
    /// it may retract a swing. A refusal retracts the body's current clip, so
    /// doing that for a cast we never predicted cuts whatever else is playing:
    /// pressing a second skill mid-cast made the refusal land on the FIRST
    /// skill's animation.
    pub fn forget(&mut self, skill_id: u32) -> bool {
        self.skills.remove(&skill_id).is_some()
    }

    /// A taken record counts as an echo only inside the window. Outside it the
    /// prediction never got its answer (a refused cast, or a very late packet),
    /// so the packet is a genuine presentation and must not be swallowed.
    fn claim(taken: &mut Option<f64>, now: f64) -> bool {
        taken
            .take()
            .is_some_and(|at| now - at <= PREDICTION_WINDOW_SECS)
    }
}

/// One swing's resolved presentation timing: when it started and the
/// combat-hit keytimes (seconds into the shot clip) its damage lands on.
/// Recorded from [`SkillSwingStarted`] once the clip actually resolves — the
/// same data [`spawn_cast_effects`] already uses to time impact VFX, kept
/// here so damage popups can be timed to it too, even when the popup arrives
/// on a *later* packet than the one that started the swing.
#[derive(Clone)]
pub struct SwingTiming {
    /// `Time::elapsed_secs_f64` when this swing's [`SkillSwingStarted`] was
    /// recorded — an approximation of the true animation start, exact for a
    /// fresh server-driven swing and off by up to one round trip for a
    /// claimed prediction (see [`SkillSwingTimings::claim`]).
    pub started_at: f64,
    pub charge_secs: f32,
    pub hit_times: Vec<f32>,
}

/// Correlates a swing's resolved timing to a server cast-instance id, so a
/// popup arriving on a packet that carries no animation info of its own — the
/// echo of a click-predicted cast, or a 0xB071's deferred damage — can still
/// be delayed to the swing's real hit keytime instead of popping at packet
/// arrival (`combat::residual_delay`).
///
/// Two-stage like [`PredictedCasts`]: a click prediction has no instance id
/// yet, so its timing is filed under `predicted` (keyed by skill id) until
/// the server's 0xB070 names an instance and [`Self::claim`] moves it over.
/// A fresh server-driven swing (no local prediction) is recorded straight
/// into `by_instance`.
#[derive(Resource, Default)]
pub struct SkillSwingTimings {
    predicted: std::collections::HashMap<u32, SwingTiming>,
    by_instance: std::collections::HashMap<u32, SwingTiming>,
}

impl SkillSwingTimings {
    /// File a click-predicted swing's timing under its skill id, awaiting a
    /// server instance to claim it.
    pub fn record_predicted(&mut self, skill_id: u32, timing: SwingTiming) {
        self.predicted.insert(skill_id, timing);
    }

    /// File a fresh server-driven swing's timing directly under its instance.
    pub fn record_instance(&mut self, instance: u32, timing: SwingTiming) {
        self.by_instance.insert(instance, timing);
    }

    /// Move a predicted swing's timing (if any) under the instance id the
    /// server just named for it. A no-op when nothing was predicted for
    /// `skill_id` (the common non-predicted case).
    pub fn claim(&mut self, skill_id: u32, instance: u32) {
        if let Some(timing) = self.predicted.remove(&skill_id) {
            self.by_instance.insert(instance, timing);
        }
    }

    /// Look up a cast instance's resolved swing timing.
    pub fn get(&self, instance: u32) -> Option<&SwingTiming> {
        self.by_instance.get(&instance)
    }

    /// Drop entries older than [`CAST_INSTANCE_TTL_SECS`] — the same bound
    /// [`CastInstances`] prunes on, so an unclaimed prediction or an instance
    /// nothing ever ended for cannot leak.
    pub fn prune(&mut self, now: f64) {
        self.predicted
            .retain(|_, t| now - t.started_at < CAST_INSTANCE_TTL_SECS);
        self.by_instance
            .retain(|_, t| now - t.started_at < CAST_INSTANCE_TTL_SECS);
    }
}

/// Retract a presentation that turned out not to have happened: the body drops
/// back to stand and the cast's not-yet-started emissions are cancelled.
///
/// Written when the server REFUSES a cast we had already predicted at the
/// keypress. The prediction is a bet that the cast will execute; when the
/// refusal comes back, leaving the swing playing shows the player an attack
/// that never happened — which is exactly what a precondition-gated skill
/// (one needing a knocked-down target, say) looked like: a phantom swing, then
/// nothing. Consumed by `player::cancel_swings`.
#[derive(Message)]
pub struct CancelSwing(pub Entity);

/// Server-driven swings waiting for the caster's current one to finish.
///
/// **Chain/combo skills are server-driven online.** One `CastSkill` request
/// produces several 0xB070s with *consecutive* skill ids — the authored
/// `Basic_ChainCode` segments (`docs/formats/textdata-skilldata.md`) — and the
/// server sends them far faster than they play: in `packet_dump/0xb070.log`
/// segments 1 and 2 of a `CastSkill{6}` land in the SAME millisecond and
/// segment 3 half a second later, while each segment's own animation window
/// (charge + `Action_ActionDuration`) is several hundred ms.
///
/// Presenting each on arrival therefore cut the previous swing off at frame 0 —
/// the "the animation restarts half a second in" report. Each segment is a
/// real swing that should play, so they are queued here and released one at a
/// time instead of being dropped or deduplicated.
///
/// The damage popups ride their own segment's [`SkillSwing`] (see
/// [`SkillSwing::popups`]), so holding a segment holds its numbers with it —
/// they still land on the hit keytime of the swing that produced them.
#[derive(Resource, Default)]
pub struct PendingSwings {
    /// Per caster: the queued swings, and when the one in flight frees the
    /// body (`Time::elapsed_secs_f64`).
    casters: std::collections::HashMap<Entity, CasterQueue>,
}

/// One presentation waiting its turn on a caster's body. Basic attacks queue
/// alongside skill segments because they contend for the same wrapper: the
/// server restarts its auto-attack loop the moment a chain ends, and an
/// `AttackSwing` presented on arrival `stop_all()`s whatever skill clip is
/// still playing (`player::play_attack_swings`) — the "auto attacks between
/// the skill animation" report.
pub enum PendingSwing {
    Skill(Box<SkillSwing>),
    Attack(AttackSwing),
}

impl PendingSwing {
    fn owner(&self) -> Entity {
        match self {
            PendingSwing::Skill(s) => s.owner,
            PendingSwing::Attack(a) => a.owner,
        }
    }
}

/// Anti-stall bound: how long a queued swing may wait for the body to free up
/// before it is released anyway.
///
/// Purely a safety valve — the real release condition is the body actually
/// being free (see [`advance_pending_swings`]). It only has to be longer than
/// any legitimate clip, and the longest authored chain animation in the corpus
/// is `skill_ch_sword_chain_h.ban` at 5.1 s. Past this the wrapper is stuck
/// (a lost `all_finished`, a body that never streamed in), and showing the
/// swing late beats never showing it or its damage numbers.
const MAX_SWING_QUEUE_LAG_SECS: f64 = 6.0;

#[derive(Default)]
pub struct CasterQueue {
    /// `(swing, the time it must be released by)`.
    queued: std::collections::VecDeque<(PendingSwing, f64)>,
}

impl PendingSwings {
    /// Offer a server-driven swing. Returns it back when nothing is queued
    /// ahead of it — the caller presents it and the body-busy check in
    /// [`advance_pending_swings`] takes over from there; otherwise it is held
    /// behind what is already waiting.
    pub fn offer(&mut self, swing: PendingSwing, now: f64) -> Option<PendingSwing> {
        let queue = self.casters.entry(swing.owner()).or_default();
        // Bounded: a burst longer than any real combo is a desync, and
        // dropping its tail is better than parking the body for seconds.
        if queue.queued.len() >= MAX_CHAIN_STEPS as usize {
            warn_once!("skills: swing queue full for one caster — dropping a swing");
            return None;
        }
        queue
            .queued
            .push_back((swing, now + MAX_SWING_QUEUE_LAG_SECS));
        None
    }

    /// Take the swings whose caster's body is free (`is_free`), plus any that
    /// have waited out [`MAX_SWING_QUEUE_LAG_SECS`].
    ///
    /// One per caster per call: releasing a second in the same frame would
    /// stack it on the clip the first just started.
    fn drain_due(&mut self, now: f64, is_free: impl Fn(Entity) -> bool) -> Vec<PendingSwing> {
        let mut released = Vec::new();
        for (owner, queue) in self.casters.iter_mut() {
            let due = queue
                .queued
                .front()
                .is_some_and(|(_, deadline)| now >= *deadline || is_free(*owner));
            if due {
                let (swing, _) = queue.queued.pop_front().expect("checked by front()");
                released.push(swing);
            }
        }
        self.casters.retain(|_, q| !q.queued.is_empty());
        released
    }
}

/// Release a queued swing as soon as its caster's body is actually free.
///
/// The release condition is the wrapper's own animation state, not a duration
/// budget: the authored `Action_ActionDuration` is not the clip's length, and
/// a chain's clip runs 2.4-5.1 s where the per-segment numbers are ~0.6 s. So
/// "is the body still playing a one-shot" is the only honest test, and it is
/// the same one `combat::send_attack_request` already uses to decide whether
/// to predict.
pub fn advance_pending_swings(
    time: Res<Time>,
    mut pending: ResMut<PendingSwings>,
    children: Query<&Children>,
    busy: crate::plugins::player::BusyWrappers,
    mut swings: MessageWriter<SkillSwing>,
    mut attacks: MessageWriter<AttackSwing>,
) {
    let released = pending.drain_due(time.elapsed_secs_f64(), |owner| {
        !crate::plugins::player::is_mid_one_shot(owner, &children, &busy)
    });
    for swing in released {
        match swing {
            PendingSwing::Skill(skill) => {
                swings.write(*skill);
            }
            PendingSwing::Attack(attack) => {
                attacks.write(attack);
            }
        }
    }
}

/// Record every resolved swing's timing ([`SkillSwingStarted`]) into
/// [`SkillSwingTimings`], keyed by instance when the server named one or by
/// skill id otherwise (a click prediction, claimed later by
/// `combat::on_object_action_update`; harmless for offline-sim swings, which
/// are never looked up by either key).
pub fn track_skill_swing_timing(
    time: Res<Time>,
    mut started: MessageReader<SkillSwingStarted>,
    mut timings: ResMut<SkillSwingTimings>,
    mut running: ResMut<RunningSwings>,
) {
    let now = time.elapsed_secs_f64();
    for swing in started.read() {
        let timing = SwingTiming {
            started_at: now,
            charge_secs: swing.charge_secs,
            hit_times: swing.hit_times.clone(),
        };
        running.begin(
            swing.owner,
            swing.codename.clone(),
            timing.clone(),
            now + (swing.charge_secs + swing.duration_secs) as f64,
        );
        match swing.instance {
            Some(instance) => timings.record_instance(instance, timing),
            None => {
                if let Some(skill_id) = swing.skill_id {
                    timings.record_predicted(skill_id, timing);
                }
            }
        }
    }
    timings.prune(now);
    running.prune(now);
}

/// The skill clip each caster is currently playing, so a chain's continuation
/// packets can land on it instead of restarting it.
///
/// **A combo is ONE animation.** Every segment of a chain carries the same
/// skilldata `Basic_Group`, and the skilleffect aniset is keyed on exactly
/// that — so all segments resolve one `AniShot`, one clip. Verified corpus
/// wide over the user's Media.pk2: of 1873 chains, **1873 share one
/// `Basic_Group` and none has a per-segment animation**. The `.ban` agrees to
/// the millisecond: `skill_ch_sword_chain_b.ban` (Blood Chain) is 2433 ms =
/// 596+602+605+630, the sum of its four segments' authored durations, and its
/// four combat-hit keytimes (266, 596, 1198, 1803 ms) sit on the cumulative
/// segment boundaries.
///
/// So the server's continuation 0xB070s are **damage, not animation**.
/// Presenting each as its own swing restarted a 2433 ms clip four times and
/// showed only its first ~600 ms, four times over — the "starts, restarts,
/// restarts" report. Segment *i*'s numbers instead ride hit event *i* of the
/// clip already playing.
#[derive(Resource, Default)]
pub struct RunningSwings(std::collections::HashMap<Entity, RunningSwing>);

pub struct RunningSwing {
    /// The shared `Basic_Group` — what identifies "still the same chain".
    codename: String,
    timing: SwingTiming,
    /// Which combat-hit keytime the next continuation's damage lands on.
    next_hit: usize,
    /// When the clip stops (`Time::elapsed_secs_f64`) — charge + the played
    /// clip's own length. This, not a TTL, is what separates "the next segment
    /// of the chain still playing" from "the player pressed the skill again":
    /// a re-press after the clip ended is a NEW cast and must animate.
    ends_at: f64,
}

impl RunningSwings {
    /// A fresh clip started on `owner` — later segments of the same chain
    /// attach to it.
    fn begin(&mut self, owner: Entity, codename: String, timing: SwingTiming, ends_at: f64) {
        self.0.insert(
            owner,
            RunningSwing {
                codename,
                timing,
                // Segment 1's own damage rides hit event 0; the next
                // continuation takes event 1.
                next_hit: 1,
                ends_at,
            },
        );
    }

    /// Claim `hits` hit events for a continuation of `codename` on `owner`,
    /// returning the swing's timing and the hit index its FIRST hit lands on.
    ///
    /// Running past the clip's last keytime **clamps** rather than failing: a
    /// continuation is damage, never animation, so a chain with more segments
    /// than the clip has hit events lands its tail on the last event instead of
    /// restarting the clip. (It failed before, and the caller's fallback was to
    /// queue a fresh swing — which replayed the whole 2.4 s combo.)
    ///
    /// `None` means this genuinely is not a continuation: a different skill, a
    /// clip that has already finished, or one with no hit events at all.
    pub fn claim_continuation(
        &mut self,
        owner: Entity,
        codename: &str,
        hits: usize,
        now: f64,
    ) -> Option<(SwingTiming, usize)> {
        let running = self.0.get_mut(&owner)?;
        if running.codename != codename
            || running.timing.hit_times.is_empty()
            || now >= running.ends_at
        {
            return None;
        }
        let at = running
            .next_hit
            .min(running.timing.hit_times.len().saturating_sub(1));
        running.next_hit += hits.max(1);
        Some((running.timing.clone(), at))
    }

    fn prune(&mut self, now: f64) {
        self.0
            .retain(|_, r| now - r.timing.started_at < CAST_INSTANCE_TTL_SECS);
    }
}

/// The presentation data a server-announced cast resolves to (a
/// [`SkillSwing`] minus owner/damage).
pub struct OnlineSwingSpec {
    pub codename: String,
    pub anim_group: Option<String>,
    pub ready_anim_type: Option<u32>,
    pub wait_anim_type: Option<u32>,
    pub anim_type: Option<u32>,
    pub charge_secs: f32,
    pub preparing_secs: f32,
    pub flying_speed: Option<f32>,
}

/// Resolve a 0xB070 `skill_id` into skill-cast presentation, or `None` when
/// the plain basic-attack path should keep the event. A "real skill" is a
/// skilldata row whose skilleffect aniset resolves a shot clip — precisely
/// the condition under which `play_skill_swings` plays a distinct skill
/// animation. Base auto-attacks (skill ids 1/2 on the reference server) and
/// any unresolvable row fall through, so auto-attack presentation can never
/// regress into the skill path.
pub fn resolve_online_swing(
    skill_id: u32,
    skill_data: &ClientSkillData,
    skill_effects: &ClientSkillEffects,
) -> Option<OnlineSwingSpec> {
    if skill_id <= 2 {
        return None;
    }
    let row = skill_data.get(&(skill_id as i32))?;
    let codename = row.basic_group().unwrap_or(row.code_name()).to_string();
    let aniset = &skill_effects.get(&codename)?.aniset;
    let anim_type = slot_to_anim_type(&aniset.ani_shot)?;
    Some(OnlineSwingSpec {
        anim_group: Some(aniset.ani_group.to_lowercase()).filter(|g| !g.is_empty()),
        ready_anim_type: slot_to_anim_type(&aniset.ani_ready),
        wait_anim_type: slot_to_anim_type(&aniset.ani_wait),
        anim_type: Some(anim_type),
        // The full authored wind-up. Only the click PREDICTION should actually
        // play it (it fires at the keypress, before the server charges); the
        // server-announced path zeroes it — see the `SkillSwing` built in
        // `combat::on_object_action_update`.
        charge_secs: (row.preparing_time_ms() + row.casting_time_ms()) as f32 / 1000.0,
        preparing_secs: row.preparing_time_ms() as f32 / 1000.0,
        flying_speed: row.flying_speed(),
        codename,
    })
}

/// The player's last cast that should resume auto-attacking once it ends —
/// skilldata col 19 (Action_AutoAttackType) == 1, vanilla's behaviour after
/// offensive skills. `at` is the completion time for locally simulated casts
/// (charge + ActionDuration); `None` waits for [`LocalCastEnded`] instead.
#[derive(Resource, Default)]
pub struct AutoAttackResume(pub Option<PendingResume>);

pub struct PendingResume {
    pub target: Entity,
    pub at: Option<f64>,
    /// Wall-clock time this arm gives up at.
    ///
    /// The `at: None` arm waits for a [`LocalCastEnded`], which comes from our
    /// cast's 0xB071 — but a **chain skill never produces one**: its segments
    /// are announced as `kind = Attack` 0xB070s with inline damage, and the
    /// server only sends `SkillEnd` for `kind = None` cast starts (verified
    /// across `packet_dump/0xb071.log` — not one chain instance appears in it).
    /// So a combo armed a resume whose trigger never arrived, and it sat there
    /// until some unrelated later cast ended and fired it at a stale target.
    pub expires_at: f64,
}

/// How long a resume arm waits for its trigger before being given up on.
///
/// The trigger is our cast's own 0xB071, which the captures put 0.43-0.98 s
/// after the 0xB070 that started it; the action slot's own buffer window
/// already treats 3 s as "this intent has gone stale"
/// ([`CombatSettings::action_buffer_seconds`](crate::plugins::config::combat::CombatSettings)).
/// Five is generous against both and still
/// short enough that an arm which lost its trigger cannot survive into a
/// different engagement and re-attack a target the player has moved on from.
pub const RESUME_ARM_TTL_SECS: f64 = 5.0;

/// One of OUR cast instances ended (its 0xB071 arrived) — written by
/// combat's `on_skill_end` after attributing the instance through
/// [`CastInstances`]. Raw [`SkillEnd`]s won't do as a resume trigger: they
/// fire for every entity's casts, so a bystander's cast end would launch an
/// unrequested attack.
#[derive(Message)]
pub struct LocalCastEnded;

/// Fire the recorded [`AutoAttackResume`] by writing the same [`AttackOrder`]
/// a double-click does. Online that restarts the server's auto-attack loop;
/// offline the order is a no-op until a local basic-attack loop exists, but
/// both paths inherit the rule from here.
pub fn resume_auto_attack(
    time: Res<Time>,
    mut resume: ResMut<AutoAttackResume>,
    mut cast_ends: MessageReader<LocalCastEnded>,
    vitals: Query<&EntityVitals>,
    mut attacks: MessageWriter<AttackOrder>,
) {
    let cast_ended = cast_ends.read().count() > 0;
    let now = time.elapsed_secs_f64();
    let Some(pending) = &resume.0 else { return };
    // An arm whose trigger never came (a chain — see `PendingResume`) is
    // dropped rather than left primed to fire on somebody else's cast end.
    if now >= pending.expires_at {
        debug!("skills: auto-attack resume expired unfired");
        resume.0 = None;
        return;
    }
    let due = match pending.at {
        Some(at) => now >= at,
        None => cast_ended,
    };
    if !due {
        return;
    }
    let target = pending.target;
    resume.0 = None;
    // only chase a target that is still alive
    if vitals.get(target).is_ok_and(|v| v.hp > 0) {
        info!("skills: resuming auto-attack after cast");
        attacks.write(AttackOrder(target));
    }
}

/// Remaining cooldown to replay for one 0x3077 entry, in milliseconds.
///
/// The packet's per-entry record is **UNVERIFIED** — the only capture of
/// 0x3077 has both lists empty, so neither the field order nor the unit of
/// `cooldown` is confirmed (`packets/src/agent/ingame.rs:551-556`). What *is*
/// data-grounded is the ceiling: a skill's remaining reuse time can never
/// exceed its own `Action_ReuseDelay` (skilldata col 14). Clamping to that
/// keeps a wrong unit from parking a skill for hours — the worst case becomes
/// "one full reuse delay", which the very next successful cast overwrites.
fn replay_remaining_ms(raw: u32, reuse_delay_ms: u32) -> u32 {
    raw.min(reuse_delay_ms)
}

/// Replay the join-time cooldowns the server sends in 0x3077.
///
/// Without this the packet is decoded and dropped, so every cooldown is lost
/// across a relog (docs/re/ui/hud-cooldown.md §6). Item cooldowns are read and
/// logged only — we have no item-cooldown store yet (#135).
pub fn apply_cooldown_replay(
    time: Res<Time>,
    skill_data: Res<ClientSkillData>,
    mut finished: MessageReader<CharacterFinished>,
    mut cooldowns: ResMut<SkillCooldowns>,
) {
    let now = time.elapsed_secs_f64();
    for msg in finished.read() {
        if !msg.item_cooldowns.is_empty() {
            info!(
                "skills: 0x3077 carries {} item cooldowns — no item-cooldown store yet (#135)",
                msg.item_cooldowns.len()
            );
        }
        let Some(data) = skill_data.data() else {
            if !msg.skill_cooldowns.is_empty() {
                warn!("skills: 0x3077 cooldown replay dropped — skilldata not loaded yet");
            }
            continue;
        };
        for entry in &msg.skill_cooldowns {
            let Some(row) = data.0.get(&(entry.ref_id as i32)) else {
                warn!(
                    "skills: 0x3077 replay for unknown skill {} — skipped",
                    entry.ref_id
                );
                continue;
            };
            let remaining = replay_remaining_ms(entry.cooldown, row.reuse_delay_ms());
            if remaining == 0 {
                continue;
            }
            cooldowns
                .0
                .insert(entry.ref_id, now + remaining as f64 / 1000.0);
        }
    }
}

/// Retract the presentation of a cast the server refused.
///
/// The client plays a cast optimistically at the keypress
/// ([`dispatch_casts`]), so a refusal has to take that swing back off the body
/// — otherwise it finishes a full phantom animation with no damage behind it,
/// which is what pressing an on-cooldown skill looked like (the
/// [`SkillCooldowns`] mirror is armed at execution, so it is blind to a
/// cooldown it never saw start).
///
/// Only ever retracts a swing **we actually predicted**: `cancel_swings` stops
/// whatever clip the body is currently playing, so retracting an unpredicted
/// cast cuts the clip that IS running — the skill pressed before this one.
/// That is what made a second press cut the first skill's animation while its
/// damage numbers and hit effects, timed off that clip, still played out.
///
/// Which cast was refused comes from [`ActionSlot`], which keeps at most one
/// request in flight and therefore simply knows. It cannot be read off the
/// ack: correlating the capture's sends against its acks shows the `code` byte
/// is not the action type (see [`ActionRefused`]).
pub fn retract_refused_casts(
    mut refusals: MessageReader<ActionRefused>,
    players: Query<Entity, With<Player>>,
    mut predicted: ResMut<PredictedCasts>,
    mut cancels: MessageWriter<CancelSwing>,
) {
    for ActionRefused(action) in refusals.read() {
        let Action::Cast { skill_id, .. } = action else {
            continue; // a refused attack or pickup animates nothing to retract
        };
        // Drop the prediction either way, so the server's eventual 0xB070 for
        // a *later* attempt is not mistaken for this one's echo — which would
        // swallow the presentation of the cast that actually succeeded.
        if predicted.forget(*skill_id) {
            info!("skills: cast {skill_id} refused — retracting its predicted swing");
            if let Ok(caster) = players.single() {
                cancels.write(CancelSwing(caster));
            }
        }
    }
}

/// The entity lookups [`dispatch_casts`] validates against, bundled to stay
/// under Bevy's 16-system-param limit.
#[derive(bevy::ecs::system::SystemParam)]
pub struct CastLookups<'w, 's> {
    players: Query<'w, 's, Entity, With<Player>>,
    incapacitated: Query<'w, 's, (), IncapacitatedFilter>,
    vitals: Query<'w, 's, &'static EntityVitals>,
    transforms: Query<'w, 's, &'static GlobalTransform>,
    /// The caster's body wrapper lives under it as a child, so the
    /// one-shot check has to hop through `Children` — see
    /// [`crate::plugins::player::is_mid_one_shot`].
    children: Query<'w, 's, &'static Children>,
    busy_wrappers: crate::plugins::player::BusyWrappers<'w, 's>,
    liveness: crate::plugins::combat::TargetLiveness<'w, 's>,
}

/// Turn [`CastRequest`]s into either the 0x7074 packet (online) or the local
/// simulation pipeline (offline, [`LocalCombat`] present).
#[allow(clippy::too_many_arguments)]
pub fn dispatch_casts(
    mut requests: MessageReader<CastRequest>,
    local: Option<Res<LocalCombat>>,
    time: Res<Time>,
    skill_data: Res<ClientSkillData>,
    skill_effects: Res<ClientSkillEffects>,
    book: Res<SkillBook>,
    weapon: Res<EquippedWeapon>,
    lookups: CastLookups,
    mut player_vitals: ResMut<PlayerVitals>,
    // Grouped: Bevy tops out at 16 system params and this one sits at the
    // limit, so the cast's bookkeeping resources travel as one tuple.
    (mut cooldowns, mut resume, mut active, mut slot, mut chain, mut predicted): (
        ResMut<SkillCooldowns>,
        ResMut<AutoAttackResume>,
        ResMut<ActiveAttack>,
        ResMut<ActionSlot>,
        ResMut<ChainCast>,
        ResMut<PredictedCasts>,
    ),
    mut swings: MessageWriter<SkillSwing>,
    mut commands: Commands,
) {
    let CastLookups {
        players,
        incapacitated,
        vitals,
        transforms,
        children,
        busy_wrappers,
        liveness,
    } = lookups;
    for request in requests.read() {
        let Some(local) = local.as_deref() else {
            // self-buffs/imbues must go out untargeted: aiming them at the
            // selected monster is a plausible server-side rejection cause
            // (imbues carry all-zero target columns, `targets_enemy` false)
            let wire_target = request.target.filter(|_| {
                skill_data
                    .get(&(request.skill_id as i32))
                    .is_none_or(|row| row.targets_enemy())
            });
            if let Some(row) = skill_data.get(&(request.skill_id as i32)) {
                // the cooldown gate reads the mirror that on_object_action_
                // update records at EXECUTION time (our cast's 0xB070) — the
                // server sends no cooldown packet and enforces silently
                // (0x7074 reject 0x05), and vanilla starts the cooldown when
                // the skill fires, not at the keypress (the server may first
                // walk us into range). Pre-execution re-presses just re-send
                // 0x7074; the server arbitrates.
                let now = time.elapsed_secs_f64();
                if !cooldowns.ready(request.skill_id, now) {
                    info!("skills: {} still on cooldown", row.code_name());
                    continue;
                }
                // The gate the offline branch has always had (`hp > 0` at the
                // target rules below), which this branch was missing entirely:
                // an offensive cast at something already dead never reaches
                // the wire, and never occupies the action slot's one buffer
                // waiting to.
                if let Some(target) = wire_target {
                    if row.targets_enemy() && liveness.is_known_dead(target) {
                        info!("skills: {} — target is already dead", row.code_name());
                        continue;
                    }
                }
                if let Some(target) = request.target {
                    if row.targets_enemy() {
                        // engage the approach corrections (0xB021 projection
                        // + close_attack_gap) like an auto-attack; ranged
                        // skills (bow/crossbow/harp cast weapons, or an
                        // authored col-21 range) stop at bow reach instead
                        // of melee
                        active.target = Some(target);
                        // The authored col-21 range only. 0 there means "use
                        // weapon range", which `engagement_reach` supplies at
                        // each point of use — so this no longer sniffs the
                        // required-weapon columns, which say what *may* cast
                        // the skill, not how far it reaches.
                        active.stop_range = row.action_range();
                        // Resume auto-attacking after offensive skills (col 19,
                        // Action_AutoAttackType 1) — armed here, fired on
                        // SkillEnd.
                        //
                        // NOT for a chain head. A chain's segments arrive as
                        // kind-Attack 0xB070s with inline damage and get **no
                        // 0xB071 at all**, so this arm's trigger would never
                        // come — it would sit primed and fire on some later,
                        // unrelated cast's end, sending an `AttackOrder` at a
                        // stale target. And it is not needed: the captures show
                        // this server restarting its own auto-attack loop
                        // unprompted after the last segment (1.99 / 2.68 /
                        // 3.74 s after segment 1 for a 3/4/5-segment chain,
                        // with no client 0x7074 in between).
                        if row.auto_attack_type() == 1 && row.chain_code().is_none() {
                            resume.0 = Some(PendingResume {
                                target,
                                at: None,
                                expires_at: time.elapsed_secs_f64() + RESUME_ARM_TTL_SECS,
                            });
                        }
                    }
                }
            }
            // Handed to the action slot rather than sent here: the server
            // takes one action at a time, and a fight is full of presses that
            // arrive faster than that. `ActionSlot` sends this now if the slot
            // is free, or holds it as THE pending intent — a newer press
            // replaces it — and fires it when the slot frees.
            slot.request(
                Action::Cast {
                    skill_id: request.skill_id,
                    target: wire_target,
                },
                time.elapsed_secs_f64(),
            );

            // Present the cast NOW rather than waiting a round trip for the
            // server's 0xB070 to announce it. Animation, the effects that ride
            // it, and the sounds keyed to its clip all start on the keypress;
            // the damage numbers deliberately do not — `damage: None` is what
            // makes `play_skill_swings` skip the popup/`PendingHits` block, so
            // the numbers still come from the server's own packets.
            //
            // Gated on already being in range: the server executes only after
            // walking us to the target, so predicting out of range would swing
            // at thin air mid-approach. Out of range we fall through to the old
            // server-driven presentation, which is correct, just later.
            //
            // Also gated on the action slot not looking busy: if another
            // action is still running, `ActionSlot` holds this cast until it
            // ends rather than sending it into a refusal — predicting here
            // would play a throwaway swing now and a second, real one when the
            // held request actually goes out. Skipping the prediction falls
            // through to the same "old server-driven presentation" path below,
            // which fires correctly once the cast really executes.
            //
            // ...and on the BODY being free, which the action slot does not
            // cover. `ActionSlot` tracks the server's action window (the
            // 0xB074 start/end acks), which closes well before our clip does: this
            // server answers a targeted skill with a kind-None 0xB070 and
            // sends the damage half a second later in 0xB071, while a chain's
            // clip runs 2.4-5.1 s. So `busy.0` is routinely false mid-cast,
            // and predicting there let a second keypress `stop_all()` the
            // first skill's clip (`play_skill_swings`; `may_take_body` allows
            // it, Cast replacing Cast) — the "first skill's animation stops
            // but its damage numbers and hit effects still appear" report,
            // those being timed off the clip that was killed underneath them.
            // The same guard `combat::send_attack_request` already uses.
            let predict_target = request.target.filter(|_| {
                skill_data
                    .get(&(request.skill_id as i32))
                    .is_some_and(|row| row.targets_enemy())
            });
            if let (Ok(caster), Some(spec)) = (
                players.single(),
                resolve_online_swing(request.skill_id, &skill_data, &skill_effects),
            ) {
                if crate::plugins::player::is_mid_one_shot(caster, &children, &busy_wrappers) {
                    debug!(
                        "skills: not predicting cast {} — a one-shot is still playing",
                        request.skill_id
                    );
                } else if !slot.occupied()
                    && within_cast_range(
                        caster,
                        predict_target,
                        weapon.engagement_reach(active.stop_range),
                        &transforms,
                    )
                {
                    swings.write(SkillSwing {
                        owner: caster,
                        skill_id: Some(request.skill_id),
                        codename: spec.codename,
                        anim_group: spec.anim_group,
                        ready_anim_type: spec.ready_anim_type,
                        wait_anim_type: spec.wait_anim_type,
                        anim_type: spec.anim_type,
                        charge_secs: spec.charge_secs,
                        preparing_secs: spec.preparing_secs,
                        flying_speed: spec.flying_speed,
                        // Server-owned; see above.
                        damage: None,
                        // The victim still travels, so the cast's projectile
                        // and impact effects have somewhere to fly.
                        target: predict_target,
                        popups: Vec::new(),
                        // No packet yet — SkillSwingStarted files this under
                        // its skill id, claimed once the server's 0xB070
                        // names an instance (see SkillSwingTimings).
                        instance: None,
                    });
                    predicted.record(request.skill_id, time.elapsed_secs_f64());
                }
            }
            continue;
        };
        let Ok(caster) = players.single() else {
            warn!("skills: no player to cast with");
            continue;
        };
        if incapacitated.contains(caster) {
            info!("skills: cast blocked — caster is stunned/frozen");
            continue;
        }
        let Some(row) = skill_data.get(&(request.skill_id as i32)) else {
            warn!("skills: unknown skill id {}", request.skill_id);
            continue;
        };
        if !row.is_castable() {
            info!("skills: {} is passive, not castable", row.code_name());
            continue;
        }
        let group_id = row.group_id();
        if group_id != 0 && !book.knows(group_id, request.skill_id as i32) {
            info!("skills: {} is not learned", row.code_name());
            continue;
        }
        // cast-weapon requirement (cols 50/51): a sword skill can't fire
        // with a spear in hand
        if let (Some((w1, w2)), Some(equipped)) = (row.required_weapons(), weapon.class) {
            if equipped != w1 && equipped != w2 {
                info!(
                    "skills: {} requires weapon class {w1}/{w2}, wielding {equipped}",
                    row.code_name()
                );
                continue;
            }
        }
        let now = time.elapsed_secs_f64();
        if !cooldowns.ready(request.skill_id, now) {
            info!("skills: {} still on cooldown", row.code_name());
            continue;
        }
        // resource gate (cols 52/53): vanilla refuses the cast client-side
        // too; only the local loop checks — online the server is the judge
        if player_vitals.mp < row.mp_cost() {
            info!(
                "skills: {} needs {} MP (have {})",
                row.code_name(),
                row.mp_cost(),
                player_vitals.mp
            );
            continue;
        }
        if row.hp_cost() > 0 && player_vitals.hp <= row.hp_cost() {
            info!("skills: {} would drain all HP", row.code_name());
            continue;
        }

        // target rules (cols 26-33): attacks need a LIVING target,
        // resurrection (SelectDeadBody) a dead one; ally-target skills fall
        // back to a self-cast until friendly entities exist offline
        let offensive = row.targets_enemy();
        let target = if row.targets_dead() {
            match request
                .target
                .filter(|t| vitals.get(*t).is_ok_and(|v| v.hp == 0))
            {
                Some(target) => Some(target),
                None => {
                    info!("skills: {} needs a dead target", row.code_name());
                    continue;
                }
            }
        } else if offensive {
            match request
                .target
                .filter(|t| vitals.get(*t).is_ok_and(|v| v.hp > 0))
            {
                Some(target) => Some(target),
                None => {
                    info!("skills: {} needs a target", row.code_name());
                    continue;
                }
            }
        } else {
            if row.targets_ally() && request.target.is_some() {
                // heals/res/buffs castable on others (cols 27/28) — no
                // friendly entities exist offline yet, so cast on ourselves
                info!("skills: {} ally-cast falls back to self", row.code_name());
            }
            None
        };

        // range gate (col 21): authored on monster skills and some buffs;
        // 0 = "use weapon range", not modeled yet, so those pass. Unit scale
        // assumed 1:1 world units (monster rows author 150-200 against the
        // ~160-unit bow reach) — playtest-calibrated.
        if let (Some(range), Some(target)) = (row.action_range(), target) {
            if let (Ok(caster_at), Ok(target_at)) = (transforms.get(caster), transforms.get(target))
            {
                if caster_at.translation().distance(target_at.translation()) > range {
                    info!("skills: {} target is too far", row.code_name());
                    continue;
                }
            }
        }

        cooldowns
            .0
            .insert(request.skill_id, now + row.reuse_delay_ms() as f64 / 1000.0);
        player_vitals.mp -= row.mp_cost();
        player_vitals.hp = player_vitals.hp.saturating_sub(row.hp_cost());

        if let Some(target) = target {
            commands.entity(caster).insert(FaceTarget(target));
            apply_status_effects(&mut commands, row, target);
        }

        let charge_secs = (row.preparing_time_ms() + row.casting_time_ms()) as f32 / 1000.0;
        if row.auto_attack_type() == 1 {
            if let Some(target) = target {
                // locally the cast "ends" after charge + the shot's action
                // window (col 13 ActionDuration)
                let action_secs = charge_secs + row.action_duration_ms() as f32 / 1000.0;
                resume.0 = Some(PendingResume {
                    target,
                    at: Some(now + action_secs as f64),
                    expires_at: now + RESUME_ARM_TTL_SECS,
                });
            }
        }
        info!(
            "skills: casting {} (charge {charge_secs:.2}s)",
            row.code_name()
        );
        swings.write(build_swing(
            row,
            request.skill_id,
            caster,
            target,
            &skill_effects,
            local.damage_per_hit,
        ));
        // combo/chain: a chain head arms its continuation here; a cast with
        // no chain code clears any combo still in flight (a new cast
        // interrupts the old one, as vanilla does)
        chain.0 = chain_follow_up(row, MAX_CHAIN_STEPS).map(|next| ChainStep {
            skill_id: next,
            caster,
            target,
            at: now + segment_secs(row) as f64,
            remaining: MAX_CHAIN_STEPS - 1,
        });
    }
}

/// Build the presentation/damage message for one locally simulated swing —
/// shared by the first cast and by every chain continuation after it.
fn build_swing(
    row: &SkillDataRow,
    skill_id: u32,
    caster: Entity,
    target: Option<Entity>,
    skill_effects: &ClientSkillEffects,
    damage_per_hit: u32,
) -> SkillSwing {
    let codename = row.basic_group().unwrap_or(row.code_name()).to_string();
    let aniset = skill_effects.get(&codename).map(|entry| &entry.aniset);
    SkillSwing {
        owner: caster,
        skill_id: Some(skill_id),
        codename,
        anim_group: aniset
            .map(|a| a.ani_group.to_lowercase())
            .filter(|g| !g.is_empty()),
        ready_anim_type: aniset.and_then(|a| slot_to_anim_type(&a.ani_ready)),
        wait_anim_type: aniset.and_then(|a| slot_to_anim_type(&a.ani_wait)),
        anim_type: aniset.and_then(|a| slot_to_anim_type(&a.ani_shot)),
        charge_secs: (row.preparing_time_ms() + row.casting_time_ms()) as f32 / 1000.0,
        preparing_secs: row.preparing_time_ms() as f32 / 1000.0,
        flying_speed: row.flying_speed(),
        damage: target.map(|target| SwingDamage {
            target,
            per_hit: damage_per_hit,
            set: HitcountSet::Dealt,
        }),
        target,
        // Offline sim: no server packet, no real damage popups to carry.
        popups: Vec::new(),
        instance: None,
    }
}

/// Combo / chain sequencing (skilldata col 9 `Basic_ChainCode`, #205).
///
/// Idea: a chain skill is authored as one row per *segment* — the head row
/// carries the MP price and the skill-board grid, and its `Basic_ChainCode`
/// names the id of the next segment, which names the next, up to seven deep
/// (`SKILL_CH_SWORD_CHAIN_F_1S_01` 7185 → `_2S_01` 7207 → `_3S_01` 7229 …,
/// verified over the user's `skilldata_*.txt`; continuation rows are priced
/// at 0 MP and hidden from the board with grid 255). So the continuations
/// are not casts the player makes: they are the later swings of the one cast
/// he already paid for. They are therefore expanded here into timed
/// follow-up swings rather than re-entered through [`dispatch_casts`], which
/// would re-charge MP, re-arm the cooldown and reject the continuation id as
/// unlearned.
///
/// Local (offline) simulation only: how the *server* drives a chain is
/// UNKNOWN — no chain skill appears in any `packet_dump/0xb070.log` capture
/// (only base attacks and monster skills do), so the online path keeps
/// sending exactly what the player pressed rather than inventing follow-up
/// 0x7074s.
#[derive(Resource, Default)]
pub struct ChainCast(pub Option<ChainStep>);

/// One armed continuation of a running combo.
#[derive(Clone, Copy, Debug)]
pub struct ChainStep {
    pub skill_id: u32,
    pub caster: Entity,
    pub target: Option<Entity>,
    /// When the previous segment's action window closes.
    pub at: f64,
    /// Remaining budget, so an authored cycle can never loop forever.
    pub remaining: u8,
}

/// The longest chain authored in the user's skilldata is 7 segments; the
/// budget is that plus slack, and it is what stops a cyclic `ChainCode`.
const MAX_CHAIN_STEPS: u8 = 8;

/// One segment's own window: charge (cols 11+12) plus the shot's action
/// duration (col 13). The next segment starts when it closes.
pub fn segment_secs(row: &SkillDataRow) -> f32 {
    (row.preparing_time_ms() + row.casting_time_ms() + row.action_duration_ms()) as f32 / 1000.0
}

/// The segment that follows `row`, if the chain continues and the step
/// budget is not spent.
fn chain_follow_up(row: &SkillDataRow, remaining: u8) -> Option<u32> {
    (remaining > 0)
        .then(|| row.chain_code())
        .flatten()
        .map(|code| code as u32)
}

/// Fire the armed chain continuation once the previous segment's window
/// closes, and arm the one after it. Aborts the combo when the caster is
/// incapacitated or the target died mid-chain.
pub fn advance_chain_casts(
    time: Res<Time>,
    local: Option<Res<LocalCombat>>,
    skill_data: Res<ClientSkillData>,
    skill_effects: Res<ClientSkillEffects>,
    incapacitated: Query<(), IncapacitatedFilter>,
    vitals: Query<&EntityVitals>,
    mut chain: ResMut<ChainCast>,
    mut swings: MessageWriter<SkillSwing>,
    mut commands: Commands,
) {
    let Some(local) = local.as_deref() else {
        // online the server drives execution (see the type doc)
        chain.0 = None;
        return;
    };
    let Some(step) = chain.0 else {
        return;
    };
    let now = time.elapsed_secs_f64();
    if now < step.at {
        return;
    }
    chain.0 = None;
    if incapacitated.contains(step.caster) {
        info!("skills: chain broken — caster is stunned/frozen");
        return;
    }
    if let Some(target) = step.target {
        if !vitals.get(target).is_ok_and(|v| v.hp > 0) {
            return;
        }
    }
    let Some(row) = skill_data.get(&(step.skill_id as i32)) else {
        warn!("skills: chain step {} is not in skilldata", step.skill_id);
        return;
    };
    if let Some(target) = step.target {
        commands.entity(step.caster).insert(FaceTarget(target));
        apply_status_effects(&mut commands, row, target);
    }
    info!("skills: chain step {} ({})", step.skill_id, row.code_name());
    swings.write(build_swing(
        row,
        step.skill_id,
        step.caster,
        step.target,
        &skill_effects,
        local.damage_per_hit,
    ));
    chain.0 = chain_follow_up(row, step.remaining).map(|next| ChainStep {
        skill_id: next,
        caster: step.caster,
        target: step.target,
        at: now + segment_secs(row) as f64,
        remaining: step.remaining - 1,
    });
}

/// Marks a one-shot cast effect whose despawn timer is refined to exactly
/// one authored program cycle once its `.efp` loads, so it plays once
/// instead of looping inside the fixed [`EMISSION_LINGER_SECS`] fallback
/// window (our effect runtime loops programs; without this a short cast
/// effect visibly repeats).
#[derive(Component)]
pub struct OneShotCastEffect {
    pub handle: Handle<crate::assets::efp::JMXVEFF>,
    /// Seconds already spent (the emission's start delay).
    pub start_delay: f32,
}

/// The SRO mirror every character/effect is authored under (`scale.x = -1`),
/// so authored translations flip exactly once. Effects anchored under the
/// caster wrapper inherit it; unparented ones (projectiles, target flashes)
/// carry it themselves so their `.efp` content mirrors the same single time.
const SRO_MIRROR: Vec3 = Vec3::new(-1.0, 1.0, 1.0);

/// A yaw that maps a directional effect's authored local **+Z** onto the
/// horizontal caster→target shot direction. These `.efp`s emit their particles
/// along +Z (verified: `SetVelocity (0,0,+z)`), so a draw aura or impact burst
/// must point +Z at the target to stream toward it — combined with
/// [`SRO_MIRROR`] the X-flip leaves +Z untouched. Pure yaw (no roll/pitch);
/// identity when `from == to`.
fn shot_rotation(from: Vec3, to: Vec3) -> Quat {
    let (dx, dz) = (to.x - from.x, to.z - from.z);
    if dx.abs() < f32::EPSILON && dz.abs() < f32::EPSILON {
        return Quat::IDENTITY;
    }
    Quat::from_rotation_y(dx.atan2(dz))
}

/// Model-frame variant of [`shot_rotation`]: `.bsr` models are authored
/// facing local **−Z** like character bodies ("SRO bodies face −Z, so add
/// PI" — see `combat::face_targets`), so their nose points at the target
/// when −Z maps onto the shot direction — a half turn from the effect frame.
fn model_shot_rotation(from: Vec3, to: Vec3) -> Quat {
    shot_rotation(from, to) * Quat::from_rotation_y(std::f32::consts::PI)
}

/// [`shot_rotation`] with pitch: maps local +Z onto the full 3D `dir` (yaw
/// about world Y, then pitch about local X, no roll) — holds an arcing
/// projectile's nose on its flight tangent, where the yaw-only helpers
/// would fly it level through the parabola.
fn pitched_shot_rotation(dir: Vec3) -> Quat {
    let horizontal = (dir.x * dir.x + dir.z * dir.z).sqrt();
    if horizontal < f32::EPSILON && dir.y.abs() < f32::EPSILON {
        return Quat::IDENTITY;
    }
    Quat::from_rotation_y(dir.x.atan2(dir.z)) * Quat::from_rotation_x(-dir.y.atan2(horizontal))
}

/// How a bow's aniset trail + force effects (`.efp`) are placed. Those two
/// effects live in the skillaniset2 row (not the effectset), so the arrow
/// would otherwise fly bare. The `.efp`s are authored along their own Z axis
/// (particles travel +Z), so they must land in the shot's frame; the draw
/// aura additionally has to sit AT the drawing hand — a fixed world spawn
/// point put its ~15-unit authored tail through the caster's hip.
enum ArrowEffectAnchor {
    /// Ride the flying arrow entity as a child; `scale` counters the arrow's
    /// own scale so the `.efp` renders at authored size (single inherited
    /// mirror). No local rotation: arrow-attached effects live in the
    /// ARROW's model frame — the same convention as the draw-phase force
    /// (playtest-calibrated 2026-08-03: with a half-turn mapping +Z onto the
    /// flight direction they read 180° flipped, pointing at the shooter).
    /// Cleaned up when the arrow despawns (recursive despawn).
    Riding { arrow: Entity, scale: Vec3 },
    /// Parented to the draw-hand bone (position tracks the animated hand for
    /// free); world orientation held on the shot axis each frame by
    /// [`AimedBoneEffect`]. No local mirror: the wrapper's `scale.x = -1` is
    /// inherited once through the bone chain (the inverse-affine rewrite nets
    /// a single residual reflection — equivalent to the unparented
    /// [`SRO_MIRROR`] for the Z-axial force content). Lasts `lifetime`.
    Drawn {
        bone: Entity,
        rotation: Quat,
        lifetime: f32,
    },
}

/// Spawn the given aniset `.efp` effects placed by `anchor`, appearing at
/// `spawn_delay`. Callers pass the trail + force for the flying arrow, but only
/// the force for the draw — the trail is a flight motion-streak that reads as a
/// sideways artifact on the stationary nocked arrow.
fn spawn_arrow_effects(
    commands: &mut Commands,
    asset_server: &AssetServer,
    effects: &[&Option<String>],
    spawn_delay: f32,
    anchor: ArrowEffectAnchor,
) -> Vec<Entity> {
    let mut spawned = Vec::new();
    for efp in effects.iter().copied().flatten() {
        let delayed = DelayedEffect {
            handle: asset_server.load(format!("particles://{efp}")),
            timer: Timer::from_seconds(spawn_delay, TimerMode::Once),
        };
        let name = Name::new(format!("arrow effect: {efp}"));
        let entity = match anchor {
            ArrowEffectAnchor::Riding { arrow, scale } => commands
                .spawn((
                    delayed,
                    Transform::from_scale(scale),
                    Visibility::Inherited,
                    name,
                    ChildOf(arrow),
                ))
                .id(),
            ArrowEffectAnchor::Drawn {
                bone,
                rotation,
                lifetime,
            } => commands
                .spawn((
                    delayed,
                    TimedEffect::new(spawn_delay + lifetime),
                    Transform::IDENTITY,
                    Visibility::Inherited,
                    AimedBoneEffect { rotation },
                    name,
                    ChildOf(bone),
                ))
                .id(),
        };
        spawned.push(entity);
    }
    spawned
}

/// Spawn a skill's impact `.efp` (the effectset `ObjName2`) at the target,
/// unparented with the mirror applied and trimmed to one program cycle — the
/// authored explosion of a projectile on arrival, or an `AT_TARGET` row's
/// secondary burst. Oriented by [`shot_rotation`] from `from` (the caster) so
/// the burst faces along the incoming shot rather than a fixed world axis.
fn spawn_impact_effect(
    commands: &mut Commands,
    asset_server: &AssetServer,
    efp: &str,
    from: Vec3,
    at_world: Vec3,
    delay: f32,
) {
    let handle: Handle<crate::assets::efp::JMXVEFF> =
        asset_server.load(format!("particles://{efp}"));
    commands.spawn((
        DelayedEffect {
            handle: handle.clone(),
            timer: Timer::from_seconds(delay, TimerMode::Once),
        },
        TimedEffect::new(delay + EMISSION_LINGER_SECS),
        Transform::from_translation(at_world)
            .with_rotation(shot_rotation(from, at_world))
            .with_scale(SRO_MIRROR),
        Visibility::Visible,
        Name::new(format!("skill impact: {efp}")),
        OneShotEffect,
        OneShotCastEffect {
            handle,
            start_delay: delay,
        },
    ));
}

/// Fire a started swing's emissions across both phases: READY at the charge
/// start (looping through the wind-up, cleaned up when the shot lands), SHOT
/// at `charge_secs` + the combat-hit keytime, routed by act-type —
/// projectiles (`AT_MOV_*`, incl. bow-arrow `.bsr` models) fly caster →
/// target; `AT_TARGET`/`AT_DMG_POS` anchor unparented at the target's ground
/// (carrying [`SRO_MIRROR`] themselves); the rest anchor to their authored
/// **caster bone** (hand/finger/pelvis) with the raw offset, inheriting the
/// single character mirror — the same recipe as the always-on and
/// animation-effect paths. Reuses the animation-effects [`DelayedEffect`]
/// path + combat's [`TimedEffect`].
pub fn spawn_cast_effects(
    mut started: MessageReader<SkillSwingStarted>,
    skill_effects: Res<ClientSkillEffects>,
    skeletons: Query<&SkeletonBinding>,
    transforms: Query<&GlobalTransform>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
) {
    for swing in started.read() {
        let Some(entry) = skill_effects.get(&swing.codename) else {
            warn_once!(
                "skills: no skilleffect row for codename {} — this cast plays no effects",
                swing.codename
            );
            continue;
        };
        let caster_pos = transforms
            .get(swing.owner)
            .map(|gt| gt.translation())
            .unwrap_or_default();
        let target_pos = swing
            .target
            .and_then(|target| transforms.get(target).ok())
            .map(|gt| gt.translation());
        let caster_bone = |bone: &Option<String>| {
            skeletons
                .get(swing.wrapper)
                .ok()
                .and_then(|binding| {
                    bone.as_ref()
                        .and_then(|bone| binding.bones.get(bone).copied())
                })
                .unwrap_or(swing.wrapper)
        };

        // The aniset DamageEfp is played at the target only when the effectset
        // carries no target impact (bow skills); track that and when the hit
        // lands (arrow arrival for projectiles, else the last shot keytime).
        let mut has_target_impact = false;
        let mut projectile_arrival: Option<f32> = None;
        // The READY nocked arrow, so the SHOT projectile can launch exactly
        // where the nock is at release (READY rows precede SHOT rows in the
        // effectset; the authored bone is the fallback).
        let mut nocked_arrow: Option<Entity> = None;
        // The draw-phase force aura, handed to the flying arrow at launch
        // (see [`AdoptEffectAtLaunch`]).
        let mut drawn_force: Option<Entity> = None;

        for emission in &entry.emissions {
            // when the emission fires, relative to the cast start
            let delay = match emission.phase {
                EffectPhase::Ready => 0.0,
                EffectPhase::Shot => {
                    let event = match emission.start_event {
                        0 => 0.0,
                        n => swing
                            .hit_times
                            .get(n as usize - 1)
                            .or(swing.hit_times.last())
                            .copied()
                            .unwrap_or(0.0),
                    };
                    swing.charge_secs + event
                }
                // ActS/ActL (self-buff auras) are handled by apply_self_buffs
                _ => continue,
            };
            let name = Name::new(format!("skill effect: {}", emission.efp_path));

            // READY loops through the whole wind-up, then vanishes as the
            // shot lands; SHOT one-shots are trimmed to one program cycle.
            let looping = emission.phase == EffectPhase::Ready;
            let lifetime = if looping {
                (swing.charge_secs - delay).max(0.2)
            } else {
                EMISSION_LINGER_SECS
            };
            let is_mov = emission.act_type.starts_with("AT_MOV");
            // Every act type that needs to know where the victim is. The
            // corpus has exactly these families with a target (censused over
            // `#section skilleffectset`): AT_MOV_{1TAR,OPTION,SPLASH},
            // AT_TARGET, AT_TARGET_F, AT_DMG_POS. The rest anchor on the
            // caster and are unaffected by a missing target.
            let is_target_directed = is_mov
                || emission.act_type.starts_with("AT_TARGET")
                || emission.act_type == "AT_DMG_POS";

            // The authored bone a projectile launches from, resolved STRICTLY
            // (no wrapper fallback — the feet are worse than the body-center
            // default). The flight start position is sampled from it on the
            // launch frame, not now: the draw pose develops during the charge.
            let launch_bone = skeletons.get(swing.wrapper).ok().and_then(|binding| {
                emission
                    .start_bone
                    .as_ref()
                    .and_then(|bone| binding.bones.get(bone).copied())
            });

            // A `.bsr` MODEL, not a particle effect — the bow arrow. When it
            // is an AT_MOV emission it is the flying projectile; otherwise
            // (a READY nocked arrow) it is held on the draw hand.
            if emission.efp_path.ends_with(".bsr") {
                let model = crate::plugins::dynamic_resource_loader::UnloadedResource(
                    asset_server.load(format!("data://{}", emission.efp_path)),
                );
                if let (true, Some(to)) = (is_mov, target_pos) {
                    let from = caster_pos + Vec3::Y * EFFECT_MID_HEIGHT;
                    let to = to + Vec3::Y * EFFECT_MID_HEIGHT;
                    let speed =
                        projectile_speed(emission.mov_speed, swing.flying_speed, from.distance(to));
                    let flight_secs = from.distance(to) / speed;
                    let mut arrow_cmd = commands.spawn((
                        model,
                        crate::plugins::dynamic_resource_loader::MirroredResource,
                        SkillProjectile {
                            to,
                            speed,
                            arc: emission.arc,
                            model_frame: true,
                            from: None,
                            progress: 0.0,
                        },
                        ProjectileLaunch(Timer::from_seconds(delay, TimerMode::Once)),
                        TimedEffect::new(delay + flight_secs + 1.0),
                        // model frame: cha_arrow_01.bms is authored shaft
                        // along −Z (measured AABB z ∈ [−13.55, 0.48], nock at
                        // the origin), so the body-model convention applies.
                        Transform::from_translation(from)
                            .with_rotation(model_shot_rotation(from, to))
                            .with_scale(SRO_MIRROR * ARROW_SCALE),
                        Visibility::Hidden,
                        name,
                    ));
                    // Launch from the nocked arrow's position, sampled at
                    // CHARGE END while the draw is still held — the shot
                    // clip swings the hand (and the nock riding it) down to
                    // the waist before the launch keytime, so a launch-frame
                    // sample starts the flight low. Fall back to the
                    // authored bone.
                    if let Some(anchor) = nocked_arrow.or(launch_bone) {
                        arrow_cmd.insert(LaunchFrom {
                            anchor,
                            sample: Timer::from_seconds(swing.charge_secs, TimerMode::Once),
                        });
                    }
                    let arrow = arrow_cmd.id();
                    if let Some(nock) = nocked_arrow {
                        // Keep the nock alive until the launch frame (plus a
                        // sampling margin — despawn and snap race in the same
                        // Update otherwise): a nock that vanishes at charge
                        // end leaves the hand empty through the shot swing.
                        commands.entity(nock).insert(TimedEffect::new(delay + 0.05));
                    }
                    // The flying arrow carries the aniset trail/force, scaled
                    // down to net the single mirror at authored size, appearing
                    // as it launches and despawned with it (recursive despawn).
                    // The force continues from the draw when one glowed there —
                    // a fresh instance would restart its grow-in graphs and
                    // visibly shrink at release — so only the trail (a flight
                    // streak, correctly starting at launch) spawns new here.
                    if let Some(force) = drawn_force.take() {
                        // outlive the draw window it was scheduled for; the
                        // arrow's recursive despawn cleans it up after adoption
                        commands
                            .entity(force)
                            .insert(TimedEffect::new(delay + flight_secs + 1.0));
                        commands.entity(arrow).insert(AdoptEffectAtLaunch(force));
                    } else {
                        spawn_arrow_effects(
                            &mut commands,
                            &asset_server,
                            &[&entry.aniset.arrow_force],
                            delay,
                            ArrowEffectAnchor::Riding {
                                arrow,
                                scale: Vec3::splat(1.0 / ARROW_SCALE),
                            },
                        );
                    }
                    spawn_arrow_effects(
                        &mut commands,
                        &asset_server,
                        &[&entry.aniset.arrow_tail],
                        delay,
                        ArrowEffectAnchor::Riding {
                            arrow,
                            scale: Vec3::splat(1.0 / ARROW_SCALE),
                        },
                    );
                    let arrival = delay + flight_secs;
                    projectile_arrival = Some(arrival);
                    // ObjName2: the arrow's impact explosion at the target.
                    if let Some(impact) = &emission.impact_efp {
                        spawn_impact_effect(
                            &mut commands,
                            &asset_server,
                            impact,
                            caster_pos,
                            to + emission.target_offset - Vec3::Y * EFFECT_MID_HEIGHT,
                            arrival,
                        );
                        has_target_impact = true;
                    }
                } else if is_target_directed {
                    // A target-directed MODEL. The `.bsr` test above runs
                    // BEFORE act-type routing, so these used to fall into the
                    // held-arrow arm below and appear as a prop stuck in the
                    // caster's hand — which is exactly what a flying sword
                    // (`AT_MOV_SPLASH res\etc\sword_skill_a.bsr`) looked like
                    // whenever it had no target to fly at.
                    let Some(to) = target_pos else {
                        warn_once!(
                            "skills: {} is target-directed ({}) but the cast has no target — \
                             not spawning it",
                            emission.efp_path,
                            emission.act_type
                        );
                        continue;
                    };
                    commands.spawn((
                        model,
                        crate::plugins::dynamic_resource_loader::MirroredResource,
                        TimedEffect::new(delay + lifetime),
                        Transform::from_translation(to + emission.target_offset)
                            .with_rotation(model_shot_rotation(caster_pos, to)),
                        Visibility::Inherited,
                        name,
                    ));
                    has_target_impact = true;
                } else {
                    // A held arrow model (the nocked READY arrow during the
                    // draw), on the draw hand. Position rides the animated
                    // hand bone; orientation is held in WORLD space by
                    // AimedBoneEffect (model frame: the shaft is authored
                    // along −Z, so −Z tracks the target through the draw) —
                    // static bone-frame corrections (HAND_GRIP, the unparsed
                    // effectset col 24 Rotate=90) proved uncalibratable
                    // against the twisted draw-hand frame.
                    // MirroredResource: the bone chain above carries the
                    // character's `scale.x = -1` (negative determinant), so
                    // the arrow's winding must be reversed or backface
                    // culling renders it inside-out/invisible (BRP-verified
                    // 2026-08-03: the model spawned, resolved, and rode the
                    // hand — but never showed).
                    let mut nocked_cmd = commands.spawn((
                        model,
                        crate::plugins::dynamic_resource_loader::MirroredResource,
                        TimedEffect::new(delay + lifetime),
                        Transform::from_translation(emission.start_offset)
                            .with_rotation(crate::commands::attach::HAND_GRIP),
                        Visibility::Inherited,
                        name,
                        ChildOf(caster_bone(&emission.start_bone)),
                    ));
                    if let Some(target) = target_pos {
                        nocked_cmd.insert(AimedBoneEffect {
                            rotation: model_shot_rotation(caster_pos, target),
                        });
                    }
                    nocked_arrow = Some(nocked_cmd.id());
                    // Only the force aura glows through the draw (the trail is
                    // a flight streak — sideways on a stationary arrow). It
                    // rides the draw-hand bone for position (a fixed world
                    // point put its authored backward tail through the hip)
                    // while AimedBoneEffect holds its world orientation.
                    // Playtest-calibrated 2026-08-03: the DRAW-phase force
                    // faces AWAY from the target (its authored −Z tail runs
                    // down the drawn shaft toward the string), a half turn
                    // from the flight convention — hence model_shot_rotation,
                    // not shot_rotation.
                    if let Some(target) = target_pos {
                        drawn_force = spawn_arrow_effects(
                            &mut commands,
                            &asset_server,
                            &[&entry.aniset.arrow_force],
                            delay,
                            ArrowEffectAnchor::Drawn {
                                bone: caster_bone(&emission.start_bone),
                                rotation: model_shot_rotation(caster_pos, target),
                                lifetime,
                            },
                        )
                        .first()
                        .copied();
                    }
                }
                continue;
            }
            let handle: Handle<crate::assets::efp::JMXVEFF> =
                asset_server.load(format!("particles://{}", emission.efp_path));

            // AT_MOV_* — an effect flying from the caster to the target.
            if is_mov {
                let Some(target) = target_pos else {
                    warn_once!(
                        "skills: projectile {} dropped — the cast has no target to fly at",
                        emission.efp_path
                    );
                    continue;
                };
                let from = caster_pos + Vec3::Y * EFFECT_MID_HEIGHT;
                let to = target + Vec3::Y * EFFECT_MID_HEIGHT;
                let speed =
                    projectile_speed(emission.mov_speed, swing.flying_speed, from.distance(to));
                let flight_secs = from.distance(to) / speed;
                let mut projectile_cmd = commands.spawn((
                    DelayedEffect {
                        handle,
                        timer: Timer::from_seconds(delay, TimerMode::Once),
                    },
                    SkillProjectile {
                        to,
                        speed,
                        arc: emission.arc,
                        model_frame: false,
                        from: None,
                        progress: 0.0,
                    },
                    TimedEffect::new(delay + flight_secs + 1.0),
                    Transform::from_translation(from)
                        .with_rotation(shot_rotation(from, to))
                        .with_scale(SRO_MIRROR),
                    Visibility::Visible,
                    name,
                ));
                if let Some(bone) = launch_bone {
                    projectile_cmd.insert(LaunchFrom {
                        anchor: bone,
                        sample: Timer::from_seconds(0.0, TimerMode::Once),
                    });
                }
                // ObjName2: impact explosion where the projectile lands.
                if let Some(impact) = &emission.impact_efp {
                    spawn_impact_effect(
                        &mut commands,
                        &asset_server,
                        impact,
                        caster_pos,
                        target + emission.target_offset,
                        delay + flight_secs,
                    );
                    has_target_impact = true;
                }
                continue;
            }

            // AT_TARGET / AT_DMG_POS — at the target's ground (Earth Shock's
            // rising rocks, hit flashes). Spawned UNPARENTED at the target's
            // world position with the mirror applied, rather than parented to
            // the un-mirrored, yaw-rotated monster root (which sent the
            // authored rise/spread off-target).
            if matches!(
                emission.act_type.as_str(),
                "AT_TARGET" | "AT_TARGET_F" | "AT_DMG_POS"
            ) {
                let Some(to) = target_pos else {
                    warn_once!(
                        "skills: target effect {} dropped — the cast has no target",
                        emission.efp_path
                    );
                    continue;
                };
                has_target_impact = true;
                // A MOV type on an AT_TARGET row flies the object from
                // StartOffset to TargetOffset around the target itself —
                // Snow Storm's icicle rain authors (x,100,z) → (x,0,z) with
                // MOV_STRAIGHT,<stagger_ms>,200,300: 20 mini-projectiles
                // falling straight down, spread over ~a second. The travel
                // frame comes entirely from those offsets (the icicle .efp
                // has no rotation/gravity content; +Z = travel like every
                // directional .efp).
                let travel = emission.target_offset - emission.start_offset;
                if emission.mov && travel.length_squared() > f32::EPSILON {
                    let delay = delay + emission.mov_delay_ms as f32 / 1000.0;
                    let from = to + emission.start_offset;
                    let speed = if emission.mov_speed > 0.0 {
                        emission.mov_speed
                    } else {
                        (travel.length() / PROJECTILE_FLIGHT_SECS).max(1.0)
                    };
                    let dir = travel.normalize();
                    // Half-turn like model_shot_rotation: the flyer's body is
                    // a .bms MESH node inside the .efp (ice-atteck-piece), and
                    // meshes follow the model convention (nose at −Z) rather
                    // than the particle +Z frame — without it the icicles
                    // fell base-first, pointing up (playtest-calibrated on
                    // Ice Rain, the only corpus users of AT_TARGET+MOV).
                    let rotation =
                        pitched_shot_rotation(dir) * Quat::from_rotation_y(std::f32::consts::PI);
                    commands.spawn((
                        DelayedEffect {
                            handle: handle.clone(),
                            timer: Timer::from_seconds(delay, TimerMode::Once),
                        },
                        SkillProjectile {
                            to: to + emission.target_offset,
                            speed,
                            arc: emission.arc,
                            model_frame: true,
                            from: None,
                            progress: 0.0,
                        },
                        TimedEffect::new(delay + travel.length() / speed + 1.0),
                        Transform::from_translation(from)
                            .with_rotation(rotation)
                            .with_scale(SRO_MIRROR),
                        Visibility::Visible,
                        name,
                    ));
                    // ObjName2: the landing burst, when the flyer arrives.
                    if let Some(impact) = &emission.impact_efp {
                        spawn_impact_effect(
                            &mut commands,
                            &asset_server,
                            impact,
                            caster_pos,
                            to + emission.target_offset,
                            delay + travel.length() / speed,
                        );
                    }
                    continue;
                }
                commands.spawn((
                    DelayedEffect {
                        handle: handle.clone(),
                        timer: Timer::from_seconds(delay, TimerMode::Once),
                    },
                    TimedEffect::new(delay + lifetime),
                    // TODO: authored start/target offsets are applied in world
                    // axes; the corpus is predominantly (0, y, 0) where the
                    // shot yaw + mirror are a no-op.
                    Transform::from_translation(to + emission.start_offset)
                        .with_rotation(shot_rotation(caster_pos, to))
                        .with_scale(SRO_MIRROR),
                    Visibility::Visible,
                    name,
                    OneShotEffect,
                    OneShotCastEffect {
                        handle,
                        start_delay: delay,
                    },
                ));
                // ObjName2: an AT_TARGET row's secondary burst at the target.
                if let Some(impact) = &emission.impact_efp {
                    spawn_impact_effect(
                        &mut commands,
                        &asset_server,
                        impact,
                        caster_pos,
                        to + emission.target_offset,
                        delay,
                    );
                }
                continue;
            }

            // Everything else (hand glows, imbue auras, cast circles,
            // hit-on-self bursts) anchors to its authored bone — matching the
            // proven always-on / animation-effect recipe (`sync_animation_effects`,
            // `commands/mod.rs`): `ChildOf(bone)` with the raw authored offset,
            // no per-effect mirror (inherited once through the character
            // hierarchy). The bone falls back to the wrapper when unnamed.
            //
            // A SHOT-phase AT_ONE_FOLLOW row IS the skill's authored hit
            // presentation — without counting it, the aniset damage_efp
            // fallback fired a SECOND burst at the target on top of it.
            if matches!(emission.phase, EffectPhase::Shot) && emission.act_type == "AT_ONE_FOLLOW" {
                has_target_impact = true;
            }
            let mut cmd = commands.spawn((
                DelayedEffect {
                    handle: handle.clone(),
                    timer: Timer::from_seconds(delay, TimerMode::Once),
                },
                TimedEffect::new(delay + lifetime),
                Transform::from_translation(emission.start_offset),
                Visibility::Inherited,
                name,
                ChildOf(caster_bone(&emission.start_bone)),
            ));
            // one-shot phases get a precise lifetime once the asset loads
            if !looping {
                cmd.insert((
                    OneShotEffect,
                    OneShotCastEffect {
                        handle,
                        start_delay: delay,
                    },
                ));
            }
        }

        // Impact burst: when the effectset has no target-anchored effect (bow
        // skills — the arrow is their only emission), play the aniset DamageEfp
        // at the target when the hit lands (arrow arrival, else the last shot
        // keytime). Melee skills carry an `AT_DMG_POS` impact and are skipped.
        if let (false, Some(efp), Some(to)) =
            (has_target_impact, &entry.aniset.damage_efp, target_pos)
        {
            let impact_delay = projectile_arrival.unwrap_or_else(|| {
                swing.charge_secs + swing.hit_times.last().copied().unwrap_or(0.0)
            });
            let handle: Handle<crate::assets::efp::JMXVEFF> =
                asset_server.load(format!("particles://{efp}"));
            commands.spawn((
                DelayedEffect {
                    handle: handle.clone(),
                    timer: Timer::from_seconds(impact_delay, TimerMode::Once),
                },
                TimedEffect::new(impact_delay + EMISSION_LINGER_SECS),
                Transform::from_translation(to + Vec3::Y * EFFECT_MID_HEIGHT)
                    .with_rotation(shot_rotation(caster_pos, to))
                    .with_scale(SRO_MIRROR),
                Visibility::Visible,
                Name::new(format!("skill impact: {efp}")),
                OneShotEffect,
                OneShotCastEffect {
                    handle,
                    start_delay: impact_delay,
                },
            ));
        }
    }
}

/// Trim a [`OneShotCastEffect`] to exactly one authored program cycle once
/// its `.efp` is available — the fixed [`EMISSION_LINGER_SECS`] fallback
/// otherwise loops a short cast effect two or three times.
pub fn resolve_oneshot_cast_lifetime(
    effects: Res<Assets<crate::assets::efp::JMXVEFF>>,
    mut pending: Query<(Entity, &OneShotCastEffect, &mut TimedEffect)>,
    mut commands: Commands,
) {
    for (entity, oneshot, mut timed) in pending.iter_mut() {
        let Some(effect) = effects.get(&oneshot.handle) else {
            continue;
        };
        let cycle = crate::plugins::effects::spawn::effect_oneshot_secs(effect);
        // start delay + one cycle + a short tail for trailing particles
        timed.set_remaining(oneshot.start_delay + cycle + 0.4);
        commands.entity(entity).remove::<OneShotCastEffect>();
    }
}

/// Fly started [`SkillProjectile`] emissions toward their target point —
/// straight, or on a parabolic arc for `MOV_UPR` rows (Berserker Arrow, the
/// base bow shot), nose held on the flight tangent; despawn on arrival (the
/// hit flash is its own emission). `.efp` projectiles wait out their
/// [`DelayedEffect`] keytime; `.bsr` arrows their [`ProjectileLaunch`] (held
/// hidden until it fires).
pub fn move_skill_projectiles(
    time: Res<Time>,
    bones: Query<&GlobalTransform>,
    mut projectiles: Query<
        (
            Entity,
            &mut Transform,
            &mut SkillProjectile,
            Option<&mut ProjectileLaunch>,
            Option<&mut LaunchFrom>,
            Option<&AdoptEffectAtLaunch>,
            &mut Visibility,
        ),
        Without<DelayedEffect>,
    >,
    mut commands: Commands,
) {
    for (entity, mut transform, mut projectile, launch, launch_from, adopt, mut visibility) in
        projectiles.iter_mut()
    {
        // Re-seat the (still hidden) flight start once the sample moment
        // arrives — for the bow arrow that is charge end, before the shot
        // swing drops the anchor; retried until the anchor's global resolves.
        if let Some(mut launch_from) = launch_from {
            launch_from.sample.tick(time.delta());
            if launch_from.sample.is_finished() {
                if let Ok(anchor_gt) = bones.get(launch_from.anchor) {
                    transform.translation = anchor_gt.translation();
                    commands.entity(entity).remove::<LaunchFrom>();
                }
            }
        }
        // hold the arrow hidden at its start until the launch delay elapses
        if let Some(mut launch) = launch {
            if !launch.0.tick(time.delta()).just_finished() {
                continue;
            }
            *visibility = Visibility::Visible;
            commands.entity(entity).remove::<ProjectileLaunch>();
        }
        // Adopt the draw-phase force aura into the arrow's frame, running
        // state intact (Riding convention: no local rotation, the inverse
        // arrow scale nets the single mirror). Its world orientation barely
        // moves — the draw held it on the same shot axis the arrow flies.
        if let Some(AdoptEffectAtLaunch(force)) = adopt {
            if let Ok(mut force_cmd) = commands.get_entity(*force) {
                force_cmd
                    .insert((
                        ChildOf(entity),
                        Transform::from_scale(Vec3::splat(1.0 / ARROW_SCALE)),
                    ))
                    .remove::<(AimedBoneEffect, TimedEffect)>();
            }
            commands.entity(entity).remove::<AdoptEffectAtLaunch>();
        }
        let from = *projectile.from.get_or_insert(transform.translation);
        let to = projectile.to;
        let total = from.distance(to).max(f32::EPSILON);
        projectile.progress += projectile.speed * time.delta_secs() / total;
        let t = projectile.progress;
        if t >= 1.0 {
            commands.entity(entity).despawn();
            continue;
        }
        let mut position = from.lerp(to, t);
        if let Some(height) = projectile.arc {
            let peak = if height > 0.0 {
                arc_apex(height, total / projectile.speed.max(f32::EPSILON))
            } else {
                total * ARC_DEFAULT_FRACTION
            };
            // parabola over the straight baseline, peaking mid-flight, with
            // the nose following the tangent (d/dt of the lift is 4p(1-2t))
            position.y += 4.0 * peak * t * (1.0 - t);
            let tangent = (to - from) + Vec3::Y * (4.0 * peak * (1.0 - 2.0 * t));
            if let Some(dir) = tangent.try_normalize() {
                transform.rotation = if projectile.model_frame {
                    // .bsr models fly nose (−Z) first — the same half turn
                    // as model_shot_rotation
                    pitched_shot_rotation(dir) * Quat::from_rotation_y(std::f32::consts::PI)
                } else {
                    pitched_shot_rotation(dir)
                };
            }
        }
        transform.translation = position;
    }
}

/// Hold [`AimedBoneEffect`] children world-aimed on their shot axis while the
/// animated (mirrored) bone chain moves under them: rewrite the local
/// rotation each frame as inverse(parent affine linear) × desired,
/// re-orthonormalized — the same mirror-safe recipe as
/// `billboard_effect_nodes` (whose doc explains why the full affine, not
/// `GlobalTransform::rotation()`, is required under the `scale.x = -1`
/// character mirror). Runs in PostUpdate before transform propagation; the
/// parent globals are one frame old, which is imperceptible.
pub fn orient_aimed_bone_effects(
    parents: Query<&GlobalTransform>,
    mut aimed: Query<(&mut Transform, &AimedBoneEffect, &ChildOf)>,
) {
    for (mut transform, effect, child_of) in &mut aimed {
        let parent_linear = parents
            .get(child_of.parent())
            .map(|p| bevy::math::Mat3::from(p.affine().matrix3))
            .unwrap_or(bevy::math::Mat3::IDENTITY);
        if let Some(rotation) =
            crate::plugins::effects::systems::aimed_local_rotation(parent_linear, effect.rotation)
        {
            transform.rotation = rotation;
        }
    }
}

/// Apply scheduled hits whose clip-synced moment has come: decrement the
/// target's vitals (the popup was already written with the same delay).
/// Death handling is deliberately absent — the Skills scene resets its
/// training dummy instead.
pub fn apply_pending_hits(
    time: Res<Time>,
    mut pending: ResMut<PendingHits>,
    mut vitals: Query<&mut EntityVitals>,
    mut player_vitals: ResMut<PlayerVitals>,
    players: Query<(), With<Player>>,
) {
    let now = time.elapsed_secs_f64();
    pending.0.retain(|hit| {
        if now < hit.at {
            return true;
        }
        if players.contains(hit.target) {
            player_vitals.hp = player_vitals.hp.saturating_sub(hit.amount);
        } else if let Ok(mut v) = vitals.get_mut(hit.target) {
            v.hp = v.hp.saturating_sub(hit.amount);
        }
        false
    });
}

// Pin the two shot-frame conventions so the 0°/180° flip-flop can't recur:
// `.efp` content is authored traveling local +Z, `.bsr` models face local −Z
// like bodies, and the riding trail/force child must net the same frame as an
// unparented effect spawn.
#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::Mat3;

    /// The core of the dedup: one keypress presents once, and the server's
    /// echo of that same cast presents nothing. Without this the clip restarts
    /// mid-swing and a second projectile, impact effect and set of sounds fire.
    #[test]
    fn the_servers_echo_of_a_predicted_cast_is_suppressed_once() {
        let mut predicted = PredictedCasts::default();
        predicted.record(7185, 100.0);
        assert!(predicted.claim_echo(7185, 100.1), "the echo is suppressed");
        assert!(
            !predicted.claim_echo(7185, 100.2),
            "only the first echo — a genuine re-cast must still present"
        );
    }

    /// A cast we never predicted is somebody else's, or one we skipped for
    /// being out of range. It must present normally.
    #[test]
    fn an_unpredicted_cast_is_never_suppressed() {
        let mut predicted = PredictedCasts::default();
        assert!(!predicted.claim_echo(7185, 100.0));
        // ...and a different skill's echo does not consume this one's record
        predicted.record(7185, 100.0);
        assert!(!predicted.claim_echo(7207, 100.1));
        assert!(predicted.claim_echo(7185, 100.1));
    }

    /// A prediction that never got its answer (a refused cast, or a very late
    /// packet) must not swallow a later, genuine presentation.
    #[test]
    fn a_stale_prediction_does_not_suppress() {
        let mut predicted = PredictedCasts::default();
        predicted.record(7185, 100.0);
        assert!(!predicted.claim_echo(7185, 100.0 + PREDICTION_WINDOW_SECS + 0.1));
    }

    /// A rejected cast is retried by re-issuing the same request; the stale
    /// record has to go or the retry's own echo would be swallowed.
    #[test]
    fn forgetting_a_rejected_cast_lets_the_retry_present() {
        let mut predicted = PredictedCasts::default();
        predicted.record(7185, 100.0);
        assert!(predicted.forget(7185), "there was a swing to retract");
        assert!(!predicted.claim_echo(7185, 100.1));
        // ...and forgetting a cast that was never predicted says so, which is
        // what stops its refusal from retracting somebody else's clip.
        assert!(!predicted.forget(7185), "nothing left to retract");
    }

    /// The emission's own authored speed wins over skilldata's, which is a
    /// has-a-projectile flag (0 or 400 corpus-wide), not a speed. Real rows:
    /// Soul Cut Blade authors 300 against skilldata's 400, a bow arrow 500.
    #[test]
    fn an_emissions_own_authored_speed_wins() {
        assert_eq!(projectile_speed(300.0, Some(400.0), 120.0), 300.0);
        assert_eq!(projectile_speed(500.0, Some(400.0), 150.0), 500.0);
        // no per-emission speed: skilldata's is better than nothing
        assert_eq!(projectile_speed(0.0, Some(400.0), 120.0), 400.0);
        // neither authored → the fixed presentation window
        assert!((projectile_speed(0.0, None, 120.0) - 120.0 / PROJECTILE_FLIGHT_SECS).abs() < 1e-3);
        // a zero skilldata speed is "no projectile speed", not "instant"
        assert!((projectile_speed(0.0, Some(0.0), 35.0) - 100.0).abs() < 1e-3);
    }

    /// Soul Cut Blade's blade force: authored 300 u/s over its authored
    /// 120-unit range is a 0.4 s flight. Flying it at skilldata's 400 made it
    /// 0.3 s — a third too fast — which is what the speed report was about.
    #[test]
    fn the_blade_force_flight_matches_its_authored_speed() {
        let range = 120.0;
        let flight = range / projectile_speed(300.0, Some(400.0), range);
        assert!((flight - 0.4).abs() < 1e-3, "{flight}");
    }

    /// The arc apex follows the projectile's REAL flight time, so a slow long
    /// lob arcs higher than a fast short one instead of both getting the apex
    /// the old fixed-window constant baked in.
    #[test]
    fn arc_apex_scales_with_actual_flight_time() {
        // the old constant was PROJECTILE_FLIGHT_SECS / 4, so a 0.35 s flight
        // must still land on exactly the previously calibrated value
        assert!((arc_apex(60.0, PROJECTILE_FLIGHT_SECS) - 60.0 * 0.35 / 4.0).abs() < 1e-4);
        // twice the flight time, twice the apex
        assert!((arc_apex(60.0, 0.7) - 2.0 * arc_apex(60.0, 0.35)).abs() < 1e-4);
    }

    /// A bare queued skill swing for the queue tests.
    fn skill(owner: Entity, skill_id: u32) -> PendingSwing {
        PendingSwing::Skill(Box::new(queued_swing(owner, skill_id)))
    }

    /// The skill id of a released queue entry, for the ordering assertions.
    fn skill_id_of(swing: &PendingSwing) -> Option<u32> {
        match swing {
            PendingSwing::Skill(s) => s.skill_id,
            PendingSwing::Attack(_) => None,
        }
    }

    /// A bare swing for the queue tests — only `owner` is read there.
    fn queued_swing(owner: Entity, skill_id: u32) -> SkillSwing {
        SkillSwing {
            owner,
            skill_id: Some(skill_id),
            codename: String::new(),
            anim_group: None,
            ready_anim_type: None,
            wait_anim_type: None,
            anim_type: None,
            charge_secs: 0.0,
            preparing_secs: 0.0,
            flying_speed: None,
            damage: None,
            target: None,
            popups: Vec::new(),
            instance: None,
        }
    }
    fn timing_with_hits(started_at: f64, hits: &[f32]) -> SwingTiming {
        SwingTiming {
            started_at,
            charge_secs: 0.0,
            hit_times: hits.to_vec(),
        }
    }

    /// **A combo is ONE animation.** Blood Chain's four segments all share the
    /// `SKILL_CH_SWORD_CHAIN_B` group, so they resolve one clip
    /// (`skill_ch_sword_chain_b.ban`, 2433 ms, hit events at 266/596/1198/1803
    /// ms). The continuations must attach to the clip already playing, taking
    /// the next hit event each — presenting them as their own swings restarted
    /// that clip four times, which is what the player saw.
    #[test]
    fn chain_continuations_ride_the_running_clip() {
        let caster = Entity::from_raw_u32(1).unwrap();
        let mut running = RunningSwings::default();
        running.begin(
            caster,
            "SKILL_CH_SWORD_CHAIN_B".into(),
            timing_with_hits(0.0, &[0.266, 0.596, 1.198, 1.803]),
            2.433,
        );

        // the real wire arrivals: segments 2, 3 and 4 each take the next event
        for (arrival, expected) in [(0.18, 1), (0.81, 2), (1.47, 3)] {
            let (_, at) = running
                .claim_continuation(caster, "SKILL_CH_SWORD_CHAIN_B", 1, arrival)
                .expect("a continuation of the running clip");
            assert_eq!(at, expected);
        }

        // A segment past the clip's last event CLAMPS to it. It stays damage,
        // never a fresh clip: failing here made the caller queue a new swing
        // and replay the whole 2433 ms combo.
        let (_, at) = running
            .claim_continuation(caster, "SKILL_CH_SWORD_CHAIN_B", 1, 2.0)
            .expect("a surplus segment still rides the running clip");
        assert_eq!(at, 3, "clamped to the clip's last hit event");

        // Once the clip has actually stopped, the same skill is a NEW cast and
        // must animate again.
        assert!(running
            .claim_continuation(caster, "SKILL_CH_SWORD_CHAIN_B", 1, 2.5)
            .is_none());
    }

    /// A different skill is never a continuation, however soon it arrives.
    #[test]
    fn a_different_skill_is_not_a_continuation() {
        let caster = Entity::from_raw_u32(1).unwrap();
        let mut running = RunningSwings::default();
        running.begin(
            caster,
            "SKILL_CH_SWORD_CHAIN_B".into(),
            timing_with_hits(0.0, &[0.266, 0.596]),
            1.0,
        );
        assert!(running
            .claim_continuation(caster, "SKILL_CH_SWORD_GEOMGI_A", 1, 0.1)
            .is_none());
    }

    /// A finished clip must not swallow a later press of the same skill: the
    /// second press is a new cast and has to animate.
    #[test]
    fn a_finished_clip_is_not_continued() {
        let caster = Entity::from_raw_u32(1).unwrap();
        let mut running = RunningSwings::default();
        running.begin(
            caster,
            "SKILL_CH_SWORD_CHAIN_B".into(),
            timing_with_hits(0.0, &[0.266, 0.596]),
            0.9,
        );
        // still playing
        assert!(running
            .claim_continuation(caster, "SKILL_CH_SWORD_CHAIN_B", 1, 0.5)
            .is_some());
        // stopped
        assert!(running
            .claim_continuation(caster, "SKILL_CH_SWORD_CHAIN_B", 1, 0.9)
            .is_none());
    }

    /// A clip with no combat-hit keytimes has nothing for a continuation to
    /// ride, so it is never one.
    #[test]
    fn a_clip_without_hit_events_is_not_continued() {
        let caster = Entity::from_raw_u32(1).unwrap();
        let mut running = RunningSwings::default();
        running.begin(caster, "X".into(), timing_with_hits(0.0, &[]), 1.0);
        assert!(running.claim_continuation(caster, "X", 1, 0.1).is_none());
    }

    /// A multi-hit segment consumes as many events as it carried, so the next
    /// segment does not land on one already used.
    #[test]
    fn a_multi_hit_segment_consumes_its_events() {
        let caster = Entity::from_raw_u32(1).unwrap();
        let mut running = RunningSwings::default();
        running.begin(
            caster,
            "X".into(),
            timing_with_hits(0.0, &[0.1, 0.2, 0.3, 0.4, 0.5]),
            1.0,
        );
        let (_, at) = running.claim_continuation(caster, "X", 2, 0.05).unwrap();
        assert_eq!(at, 1);
        let (_, at) = running.claim_continuation(caster, "X", 1, 0.15).unwrap();
        assert_eq!(at, 3, "the two-hit segment took events 1 and 2");
    }

    /// The queue holds a swing while the body is busy and releases it the
    /// moment it frees — the authored durations are not the clip's length, so
    /// the body's own state is the only honest gate.
    #[test]
    fn the_queue_releases_when_the_body_is_free() {
        let caster = Entity::from_raw_u32(1).unwrap();
        let mut pending = PendingSwings::default();
        assert!(pending.offer(skill(caster, 6), 0.0).is_none());

        assert!(
            pending.drain_due(0.5, |_| false).is_empty(),
            "held while the body is playing something"
        );
        let released = pending.drain_due(1.0, |_| true);
        assert_eq!(skill_id_of(released.first().expect("released")), Some(6));
    }

    /// Only one swing per caster per frame — releasing two would stack the
    /// second on the clip the first just started.
    #[test]
    fn the_queue_releases_one_swing_per_caster_per_call() {
        let caster = Entity::from_raw_u32(1).unwrap();
        let mut pending = PendingSwings::default();
        pending.offer(skill(caster, 6), 0.0);
        pending.offer(skill(caster, 7), 0.0);
        assert_eq!(pending.drain_due(1.0, |_| true).len(), 1);
        assert_eq!(pending.drain_due(1.1, |_| true).len(), 1);
        assert!(pending.drain_due(1.2, |_| true).is_empty());
    }

    /// The anti-stall valve: a body that never reports free still gets its
    /// swing (and its damage numbers) eventually.
    #[test]
    fn a_stuck_body_still_releases_at_the_deadline() {
        let caster = Entity::from_raw_u32(1).unwrap();
        let mut pending = PendingSwings::default();
        pending.offer(skill(caster, 6), 0.0);
        assert!(pending
            .drain_due(MAX_SWING_QUEUE_LAG_SECS - 0.01, |_| false)
            .is_empty());
        assert_eq!(
            pending.drain_due(MAX_SWING_QUEUE_LAG_SECS, |_| false).len(),
            1
        );
    }

    /// Two casters never block each other — the queue is per body.
    #[test]
    fn casters_queue_independently() {
        let a = Entity::from_raw_u32(1).unwrap();
        let b = Entity::from_raw_u32(2).unwrap();
        let mut pending = PendingSwings::default();
        pending.offer(skill(a, 6), 0.0);
        pending.offer(skill(b, 6), 0.0);
        // only `b` reports free; `a` stays held
        let released = pending.drain_due(0.1, |owner| owner == b);
        assert_eq!(released.len(), 1);
    }

    /// A burst longer than any authored chain is a desync; its tail is dropped
    /// rather than parking the body for seconds.
    #[test]
    fn the_queue_is_bounded() {
        let caster = Entity::from_raw_u32(1).unwrap();
        let mut pending = PendingSwings::default();
        for id in 0..100 {
            pending.offer(skill(caster, id), 0.0);
        }
        let mut released = 0;
        for step in 1..200 {
            released += pending.drain_due(step as f64, |_| true).len();
        }
        assert_eq!(
            released, MAX_CHAIN_STEPS as usize,
            "only the bounded queue's worth is ever replayed"
        );
    }

    /// A refusal retracts the swing it refused — and ONLY that one.
    ///
    /// `cancel_swings` stops whatever clip the body is currently playing, so a
    /// `CancelSwing` written for a cast we never predicted lands on the skill
    /// that IS running. That is what made a second skill press (or a press of
    /// one still on cooldown) cut the first skill's animation while its damage
    /// numbers and hit effects, timed off that clip, still played out.
    #[test]
    fn a_refusal_retracts_only_a_swing_we_predicted() {
        /// Returns how many `CancelSwing` messages a refusal of skill 7185
        /// produces, given whether that keypress played an optimistic swing.
        fn cancels_for(refused: Action, predict: bool) -> usize {
            let mut app = App::new();
            app.init_resource::<PredictedCasts>()
                .add_message::<ActionRefused>()
                .add_message::<CancelSwing>()
                .add_systems(Update, retract_refused_casts);
            app.world_mut().spawn(Player);
            if predict {
                app.world_mut()
                    .resource_mut::<PredictedCasts>()
                    .record(7185, 0.0);
            }
            app.world_mut().write_message(ActionRefused(refused));
            app.update();
            app.world_mut()
                .resource_mut::<bevy::ecs::message::Messages<CancelSwing>>()
                .drain()
                .count()
        }

        let cast = Action::Cast {
            skill_id: 7185,
            target: None,
        };
        assert_eq!(
            cancels_for(cast, true),
            1,
            "a refused cast we DID predict must retract its swing"
        );
        assert_eq!(
            cancels_for(cast, false),
            0,
            "a refused cast we never predicted must not touch the body"
        );
        // A refused attack or pickup animates nothing through this path, so it
        // must never reach for the body either.
        assert_eq!(
            cancels_for(Action::Attack(Entity::from_raw_u32(1).unwrap()), true),
            0,
            "only casts are retracted here"
        );
    }

    /// The auto-attack slot is independent of the skill map, and only the
    /// opening swing is ever suppressed — the rest of the loop stays
    /// server-timed so its cadence cannot drift.
    #[test]
    fn only_the_opening_auto_attack_swing_is_suppressed() {
        let mut predicted = PredictedCasts::default();
        predicted.record_auto_attack(100.0);
        assert!(predicted.claim_auto_attack_echo(100.1));
        assert!(!predicted.claim_auto_attack_echo(100.2));
        // and it does not collide with a skill prediction
        predicted.record(7185, 100.0);
        assert!(!predicted.claim_auto_attack_echo(100.1));
        assert!(predicted.claim_echo(7185, 100.1));
    }

    /// Real chain rows from the user's `skilldata_5000.txt`:
    /// `SKILL_CH_SWORD_CHAIN_F_1S_01` (7185, chain → 7207, cast 256ms,
    /// duration 340ms) → `_2S_01` (7207, chain → 7229, duration 1188ms) →
    /// `_3S_01` (7229, chain → 7251, duration 464ms).
    fn chain_row(chain_code: &str, preparing: &str, casting: &str, duration: &str) -> SkillDataRow {
        let mut fields = vec![String::new(); 119];
        fields[9] = chain_code.to_string();
        fields[11] = preparing.to_string();
        fields[12] = casting.to_string();
        fields[13] = duration.to_string();
        SkillDataRow(fields)
    }

    #[test]
    fn chain_follow_up_walks_the_chaincode_and_stops_at_the_last_segment() {
        let head = chain_row("7207", "0", "256", "340");
        assert_eq!(chain_follow_up(&head, MAX_CHAIN_STEPS), Some(7207));
        // last segment of a chain: col 9 is 0
        let last = chain_row("0", "0", "0", "464");
        assert_eq!(chain_follow_up(&last, MAX_CHAIN_STEPS), None);
        // spent budget stops an authored cycle
        assert_eq!(chain_follow_up(&head, 0), None);
    }

    #[test]
    fn chain_segment_window_is_charge_plus_action_duration() {
        // 7185: preparing 0 + casting 256 + duration 340
        assert!((segment_secs(&chain_row("7207", "0", "256", "340")) - 0.596).abs() < 1e-6);
        // 7207: a pure 1188ms continuation swing
        assert!((segment_secs(&chain_row("7229", "0", "0", "1188")) - 1.188).abs() < 1e-6);
    }

    /// The Skills test-scene geometry (character → dummy).
    const FROM: Vec3 = Vec3::new(40.0, 0.0, 0.0);
    const TO: Vec3 = Vec3::new(85.0, 0.0, 15.0);

    fn shot_dir() -> Vec3 {
        (TO - FROM).normalize()
    }

    #[test]
    fn shot_rotation_maps_plus_z_onto_shot_dir() {
        let rotated = shot_rotation(FROM, TO) * Vec3::Z;
        assert!(rotated.abs_diff_eq(shot_dir(), 1e-6), "{rotated:?}");
        // azimuth of the scene shot: atan2(45, 15) ≈ 71.6°
        assert!((45.0f32.atan2(15.0).to_degrees() - 71.565).abs() < 0.01);
    }

    #[test]
    fn model_shot_rotation_maps_minus_z_onto_shot_dir() {
        let rotated = model_shot_rotation(FROM, TO) * Vec3::NEG_Z;
        assert!(rotated.abs_diff_eq(shot_dir(), 1e-6), "{rotated:?}");
    }

    #[test]
    fn pitched_shot_rotation_maps_plus_z_onto_full_dir() {
        // an arc tangent: the horizontal shot direction plus climb
        let dir = (shot_dir() + Vec3::Y * 0.6).normalize();
        let rotated = pitched_shot_rotation(dir) * Vec3::Z;
        assert!(rotated.abs_diff_eq(dir, 1e-6), "{rotated:?}");
        // level input degenerates to the yaw-only shot_rotation
        let level = pitched_shot_rotation(shot_dir()) * Vec3::Z;
        assert!(level.abs_diff_eq(shot_dir(), 1e-6), "{level:?}");
    }

    #[test]
    fn riding_child_keeps_the_arrow_model_frame_at_authored_scale() {
        // arrow: model frame + mirror·scale; child: counter-scale only —
        // arrow-attached effects live in the ARROW's frame (playtest: a
        // half-turn onto the flight direction read 180° flipped).
        let arrow = Mat3::from_quat(model_shot_rotation(FROM, TO))
            * Mat3::from_diagonal(SRO_MIRROR * ARROW_SCALE);
        let child = Mat3::from_diagonal(Vec3::splat(1.0 / ARROW_SCALE));
        let expected =
            Mat3::from_quat(model_shot_rotation(FROM, TO)) * Mat3::from_diagonal(SRO_MIRROR);
        assert!(
            (arrow * child).abs_diff_eq(expected, 1e-5),
            "{:?} vs {expected:?}",
            arrow * child
        );
    }

    /// 0x3077's per-entry record is UNVERIFIED (the only capture has both
    /// lists empty), so a replayed remaining time is clamped to the skill's
    /// own `Action_ReuseDelay` — a wrong unit can then cost at most one full
    /// reuse delay instead of parking the skill for hours.
    #[test]
    fn cooldown_replay_is_clamped_to_the_skills_reuse_delay() {
        // in range: taken as-is
        assert_eq!(replay_remaining_ms(1_200, 3_000), 1_200);
        // over the ceiling (e.g. seconds misread as ms, or a stale entry)
        assert_eq!(replay_remaining_ms(3_000_000, 3_000), 3_000);
        // a skill with no reuse delay can never be on cooldown
        assert_eq!(replay_remaining_ms(5_000, 0), 0);
        assert_eq!(replay_remaining_ms(0, 3_000), 0);
    }
}
