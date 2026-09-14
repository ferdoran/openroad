//! Auto-attack combat: click-again-to-attack → 0x7074 → server-driven swings.
//!
//! Idea: vSRO combat is server-driven. One 0x7074 Attack request makes the
//! server path the character into range (through the normal 0xB021 movement
//! reconciliation) and repeat basic-attack swings until a Cancel or the target
//! dies — the client never schedules swings itself. Its job is presentation:
//! each 0xB070 [`ObjectActionUpdate`] names the acting entity (one-shot attack
//! animation via [`AttackSwing`]) and the damaged targets (floating hitcount
//! numbers via [`DamagePopup`] plus an optimistic HP decrement); 0x3057
//! remains the authoritative HP source and later overwrites the same values.

use bevy::prelude::*;

pub mod action_slot;

use packets::agent::prelude::{
    ActionKind, CharacterDied, DamageContent, EntityLevelUp, EntityStateUpdate, HitEffect,
    HitPosition, ObjectActionRequest, ObjectActionResponse, ObjectActionUpdate, SkillEnd,
};
use packets::Packet;

use crate::commands::SpawnedFromResource;
use crate::net::connection::SilkroadConnection;
use crate::plugins::cursor::interactions::entity_select::SelectedEntity;
use crate::plugins::effects::EffectCommandsExt;
use crate::plugins::hud::death::PlayerDeath;
use crate::plugins::hud::player_mini_info::PlayerVitals;
use crate::plugins::net::agent::AgentConnection;
use crate::plugins::net::entities::{
    EntityVitals, NetworkEntities, NetworkId, RemoteMovement, VitalsSet,
};
use crate::plugins::player::{Player, PlayerCommands, PlayerMoveOrder};
use crate::plugins::skills::cast::{
    resolve_online_swing, CastInstanceInfo, CastInstances, LocalCastEnded, PendingSwing,
    RunningSwings, SkillCooldowns, SkillSwing, SkillSwingTimings, SwingTiming,
};
use crate::plugins::skills::EquippedWeapon;
use crate::scenes::SceneState;

/// Order to start auto-attacking a world entity (double-click / re-click on
/// the selected monster).
#[derive(Message)]
pub struct AttackOrder(pub Entity);

/// Order to pick up a ground item (click on it) — server-driven like attacks:
/// the server paths the character there, answers 0xB034 (inventory gain) and
/// broadcasts the 0x3036 bow-down.
#[derive(Message)]
pub struct PickupOrder(pub Entity);

/// One attack swing performed by `owner` — consumed by the animation layer,
/// which plays a one-shot attack clip AND releases the carried damage popups
/// timed to the clip's combat-hit keytimes (so multi-hit swings show their
/// numbers when each hit visually lands, not all at once).
#[derive(Message)]
pub struct AttackSwing {
    pub owner: Entity,
    pub popups: Vec<QueuedPopup>,
    /// Whether to actually (re)start the attack clip.
    ///
    /// `false` on the server's echo of a swing the client already predicted at
    /// the click: the clip is mid-flight and restarting it would snap it back
    /// to frame 0 and re-fire every animation-keyed sound. The popups still
    /// need timing, so the swing is delivered either way — the animation layer
    /// reads the keytimes off the clip already playing instead of a fresh one.
    pub replay_clip: bool,
}

/// The local player's own attack just landed, carrying how many damage
/// instances it delivered per target — one arrow each for a drawn bow, so a
/// 2-arrow combo reports 2 and a 3-arrow combo 3.
///
/// Raised from the server's 0xB070 rather than from [`AttackSwing`], because
/// that one is a *presentation* message: it travels through
/// `PendingSwings`, which delays it behind a busy body, releases at most one
/// per caster per frame and drops the tail of a long burst. Fine for playing a
/// clip, useless as a count of shots fired — reading it spent one arrow per
/// engagement instead of one per shot.
#[derive(Message)]
pub struct LocalAttackLanded(pub u16);

/// A damage popup waiting for its swing's Nth combat-hit moment.
#[derive(Clone)]
pub struct QueuedPopup {
    /// Index into the swing's hit-event list (the Nth damage instance).
    pub hit_index: usize,
    pub popup: DamagePopup,
}

/// Which hitcount art color-set a damage popup uses (pixel-verified:
/// neutral = white, `_enemy` = red, `_player` = gray).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitcountSet {
    /// Damage dealt by the local player (neutral white digits).
    Dealt,
    /// Damage the local player received (red `_enemy` digits — "from the
    /// enemy", matching vanilla's red numbers when you get hit).
    Received,
    /// Damage between third parties (gray `_player` digits).
    Other,
}

/// One damage number to float above `anchor`, `delay` seconds from now.
#[derive(Message, Clone)]
pub struct DamagePopup {
    pub anchor: Entity,
    /// Anchor position snapshot — the fallback when the anchor has despawned
    /// by the time the (delayed) popup fires.
    pub world: Vec3,
    pub delay: f32,
    pub amount: u32,
    pub critical: bool,
    /// The defender took no damage — wire hit arm 2
    /// ([`packets::agent::ingame::HIT_ARM_AVOIDED`]).
    ///
    /// Such a record carries no damage word at all, so this popup shows the
    /// `blocking` word glyph **alone**, with no number: `amount` is 0 and
    /// meaningless. It also draws no HP and, deliberately, no hit reaction —
    /// see `player::reaction_delay`.
    ///
    /// The wire does not distinguish miss from parry from block (it carries no
    /// discriminator); "BLOCK" is the label we present for the whole avoided
    /// class. Mutually exclusive with `critical`, which needs a number.
    pub blocked: bool,
    /// Not styled yet (vanilla shows no special art); kept for death-effect
    /// consumers.
    #[allow(dead_code)]
    pub killing_blow: bool,
    pub set: HitcountSet,
    /// Set when this hit also knocked its target down.
    ///
    /// The knockdown rides the popup rather than travelling as its own message
    /// because the two are strictly 1:1 — a displacement arm always carries a
    /// `DamageValue`, so it always produces a popup at the same `hit_index`,
    /// and the only thing that skips one (a missing anchor) skips both.
    ///
    /// Riding along is what makes the *timing* right for free: a knockdown that
    /// fired at packet arrival dropped the body a few hundred ms before the
    /// number and the flinch it belongs to, because only the popup knew the
    /// swing clip's hit keytime. Now it inherits [`Self::delay`] on every
    /// path — every popup-producing path is hit-keytime-timed today (basic
    /// attacks via [`AttackSwing`], skills via [`SkillSwing::popups`] or a
    /// `residual_delay` lookup); only the source-entity-not-found fallback in
    /// [`on_object_action_update`] still releases at `delay: 0.0`, because
    /// there is nothing to animate.
    pub knockdown: Option<KnockdownHit>,
}

/// The displacement payload of a hit that knocked its target down.
///
/// Both fields are carried uninterpreted — see [`queue_damage_popups`] for why
/// `arm` is not resolved into knockback-vs-pull and why `position` is not
/// applied to the victim's transform.
#[derive(Clone, Copy, Debug)]
pub struct KnockdownHit {
    pub arm: u8,
    #[allow(dead_code, reason = "awaits a capture confirming the tail's scaling")]
    pub position: HitPosition,
}

/// The in-flight engagement (auto-attack loop or a targeted skill cast), if
/// any. Cleared on cancel, rejection, or target despawn — the server owns
/// the actual attack loop; the client uses this to steer the approach
/// corrections (0xB021 projection + [`close_attack_gap`]).
#[derive(Resource, Default)]
pub struct ActiveAttack {
    pub target: Option<Entity>,
    /// An *authored* reach override for this engagement — a skill's col-21
    /// Action_Range. `None`, the usual case for a plain weapon attack, means
    /// "use the equipped weapon's reach"; both are resolved by
    /// [`EquippedWeapon::engagement_reach`], never read raw.
    pub stop_range: Option<f32>,
}

/// Keeps an attacker's body turned toward its current victim between swings
/// (set from each 0xB070; the server never sends facing).
#[derive(Component)]
pub struct FaceTarget(pub Entity);

/// Yaw slerp fraction per second while facing a victim (matches the remote
/// walking turn speed).
const FACE_TURN_SPEED: f32 = 12.0;

/// Fallback reach (world units) for an engagement whose weapon is unknown:
/// empty hands, or itemdata not loaded yet. A known weapon uses its own
/// itemdata reach instead — see [`EquippedWeapon::engagement_reach`], which
/// is the only thing that reads this.
pub const ATTACK_GAP_STOP: f32 = 16.0;
/// Standoff slack past the stop distance before the gap-close corrects —
/// the server sometimes stops the approach short (its destination is
/// range-checked against where the monster *was*, or the monster drifted).
const ATTACK_GAP_SLACK: f32 = 4.0;

/// Marks an entity a 0xB070 killing blow has landed on: its upcoming 0x3016
/// despawn plays the death animation instead of vanishing instantly.
#[derive(Component)]
pub struct Slain;

/// A slain entity's despawn arrived — start the death presentation (emitted
/// by the despawn handlers in `game_scene`; consumed here for state and by
/// the animation layer for the die clip).
#[derive(Message)]
pub struct EntityDied(pub Entity);

/// A corpse playing its death animation; despawned when the timer runs out.
#[derive(Component)]
pub struct Dying {
    timer: Timer,
}

impl Dying {
    /// A fresh corpse, lingering for [`CORPSE_LINGER_SECS`].
    pub fn lingering() -> Self {
        Dying {
            timer: Timer::from_seconds(CORPSE_LINGER_SECS, TimerMode::Once),
        }
    }
}

/// How long a corpse lingers before despawning — long enough for the die
/// clip plus the death smoke (fires ~2.4s in on classic monsters).
const CORPSE_LINGER_SECS: f32 = 4.0;

/// "Is this entity dead, as far as this client knows?"
///
/// The one place that question is answered, because the codebase had been
/// answering it two incompatible ways: `dying.contains(e)` (six systems, e.g.
/// [`close_attack_gap`]) misses the frames between the killing blow and
/// `Dying` landing, and `vitals.get(e).is_ok_and(|v| v.hp > 0)`
/// (`skills::cast`) is fail-*closed* — it calls anything without
/// [`EntityVitals`] dead, which is every ground item, NPC and remote player,
/// since only monsters carry vitals today.
///
/// The shape here is the authoritative one `game_scene::despawn_or_die`
/// already uses to decide whether a despawn plays a death or just vanishes.
#[derive(bevy::ecs::system::SystemParam)]
pub struct TargetLiveness<'w, 's> {
    dead: Query<'w, 's, (Has<Slain>, Option<&'static EntityVitals>, Has<Dying>)>,
}

impl TargetLiveness<'_, '_> {
    /// Known dead: a killing blow has landed, the corpse is lingering, or its
    /// HP has reached zero.
    ///
    /// Fail-**open** on the unknown. An entity we cannot see at all (already
    /// despawned) is not "dead" here either — the caller's own id lookup is
    /// what rejects that, and answering `true` would conflate "gone" with
    /// "a corpse", which want different messages.
    pub fn is_known_dead(&self, entity: Entity) -> bool {
        match self.dead.get(entity) {
            Ok((slain, vitals, dying)) => slain || dying || vitals.is_some_and(|v| v.hp == 0),
            Err(_) => false,
        }
    }
}

/// How many frames a 0x30BF whose entity is not spawned yet is kept before
/// being given up on.
///
/// A state update can genuinely arrive **before** its entity exists on our
/// side. A spawn packet turns into `commands.spawn`, which is deferred, and the
/// uid only enters `NetworkEntities` when the `NetworkId` component hook runs
/// at the next sync point — so there is a window of a frame or two after the
/// spawn record in which the uid is unknown. A GM `/zoe2` monster is the case
/// that exposed it: the capture has its life→dead land **38 ms** after its
/// spawn record, inside that window, and dropping it left a monster that was
/// never marked `Dying` — so it kept its hover, its hit proxy and its pickable
/// meshes, and `finish_dying` never despawned it either. A monster killed in a
/// normal fight dies minutes after spawning and never hit this.
///
/// Frames rather than seconds because the thing being waited on is a command
/// flush, which is per-frame. Eight is generous for a one-to-two-frame gap and
/// still bounded, so a state update for an entity we never spawn (one that
/// left range between the two packets) is discarded rather than retried
/// forever.
pub(crate) const STATE_UPDATE_RETRY_FRAMES: u8 = 8;

pub struct CombatPlugin;

impl Plugin for CombatPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ActiveAttack>()
            .init_resource::<action_slot::ActionSlot>()
            .add_message::<action_slot::ActionRefused>()
            .add_message::<AttackOrder>()
            .add_message::<PickupOrder>()
            .add_message::<AttackSwing>()
            .add_message::<LocalAttackLanded>()
            .add_message::<DamagePopup>()
            .add_message::<EntityDied>()
            .add_systems(
                Update,
                (
                    send_attack_request,
                    send_pickup_request,
                    cancel_attack_on_move,
                    clear_attack_on_target_gone,
                    on_object_action_response,
                    action_slot::apply_action_acks,
                    // After every producer, so a press onto a free slot still
                    // reaches the wire in the frame it was made.
                    action_slot::drive_action_slot
                        .after(send_attack_request)
                        .after(send_pickup_request)
                        .after(cancel_attack_on_move)
                        .after(crate::plugins::skills::cast::dispatch_casts)
                        .after(action_slot::apply_action_acks),
                    // The two predicted-HP writers. They must land before the
                    // server's absolute values or the prediction stacks on a
                    // number that already contains it — see `VitalsSet`.
                    on_object_action_update.in_set(VitalsSet::Predicted),
                    on_entity_state_update,
                    on_character_died,
                    on_entity_level_up,
                    handle_entity_death,
                    close_attack_gap,
                    finish_dying,
                    on_skill_end.in_set(VitalsSet::Predicted),
                )
                    .run_if(in_state(SceneState::GameWorld)),
            )
            // timed effect wrappers and target-facing are also used by the
            // offline skill simulation (cast emissions + the caster turning
            // to face its skill target), so they run in the Skills scene too
            .add_systems(
                Update,
                (expire_timed_effects, face_attack_targets)
                    .run_if(in_state(SceneState::GameWorld).or_else(in_state(SceneState::Skills))),
            );
    }
}

/// Turn an [`AttackOrder`] into the 0x7074 Attack request and remember the
/// engagement. Fire-and-forget like all action packets.
fn send_attack_request(
    time: Res<Time>,
    mut orders: MessageReader<AttackOrder>,
    ids: Query<&NetworkId>,
    players: Query<Entity, With<Player>>,
    transforms: Query<&GlobalTransform>,
    children: Query<&Children>,
    busy_wrappers: crate::plugins::player::BusyWrappers,
    weapon: Res<EquippedWeapon>,
    mut active: ResMut<ActiveAttack>,
    mut slot: ResMut<action_slot::ActionSlot>,
    mut predicted: ResMut<crate::plugins::skills::cast::PredictedCasts>,
    mut swings: MessageWriter<AttackSwing>,
) {
    for AttackOrder(target) in orders.read() {
        // The uid is resolved again at send time; this only rejects an order
        // for something that was never a network entity.
        if ids.get(*target).is_err() {
            continue;
        }
        // Re-clicking a monster orders an attack per click
        // (`entity_select::update_selection`), so these go through the action
        // slot like every other 0x7074 rather than straight onto the wire.
        slot.request(
            action_slot::Action::Attack(*target),
            time.elapsed_secs_f64(),
        );
        active.target = Some(*target);
        // No *authored* override: a plain weapon attack's reach is the
        // weapon's own, which `EquippedWeapon::engagement_reach` resolves at
        // each point of use. Resolving there rather than latching a number
        // here is what makes a mid-fight weapon swap take effect.
        active.stop_range = None;

        // Swing now rather than a round trip from now, for the same reason and
        // with the same guard as the skill prediction in `skills::cast`: only
        // when already in reach, because the server walks us to the target
        // before it executes, and a swing played mid-approach hits thin air.
        //
        // Only the OPENING swing is predicted. The rest of the loop stays
        // server-timed, so the attack cadence still comes from the server and
        // cannot drift; this just removes the latency from the first blow.
        //
        // No popups: the numbers ride the server's own 0xB070, exactly as
        // before. `play_attack_swings` handles an empty list by playing the
        // clip and releasing nothing.
        //
        // And never over a running one-shot. `resume_auto_attack` issues this
        // order the moment our cast's 0xB071 lands, which the captures put
        // 0.43-0.98 s after the cast started — i.e. while its shot clip is
        // still playing. Predicting there restarted the body mid-swing; the
        // server's own 0xB070 presents the swing a moment later anyway.
        let Ok(player) = players.single() else {
            continue;
        };
        if crate::plugins::player::is_mid_one_shot(player, &children, &busy_wrappers) {
            debug!("combat: not predicting the opening swing — a one-shot is still playing");
            continue;
        }
        if crate::plugins::skills::cast::within_cast_range(
            player,
            Some(*target),
            weapon.engagement_reach(active.stop_range),
            &transforms,
        ) {
            swings.write(AttackSwing {
                owner: player,
                popups: Vec::new(),
                replay_clip: true,
            });
            predicted.record_auto_attack(time.elapsed_secs_f64());
        }
    }
}

/// Turn a [`PickupOrder`] into the 0x7074 Pickup request.
fn send_pickup_request(
    time: Res<Time>,
    mut orders: MessageReader<PickupOrder>,
    ids: Query<&NetworkId>,
    mut slot: ResMut<action_slot::ActionSlot>,
) {
    for PickupOrder(target) in orders.read() {
        if ids.get(*target).is_err() {
            continue;
        }
        slot.request(
            action_slot::Action::Pickup(*target),
            time.elapsed_secs_f64(),
        );
    }
}

/// A ground click while engaged cancels the server's attack loop — matching
/// the original client, where moving away stops auto-attacking.
fn cancel_attack_on_move(
    mut moves: MessageReader<PlayerMoveOrder>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    mut active: ResMut<ActiveAttack>,
    mut slot: ResMut<action_slot::ActionSlot>,
) {
    if moves.is_empty() {
        return;
    }
    moves.clear();
    // A deliberate stop must not be chased by whatever was waiting for the
    // slot — walking away cancels the intent, not just the running action.
    slot.clear_pending();
    if active.target.take().is_none() {
        return;
    }
    let Ok(conn) = conn.single() else {
        return;
    };
    info!("combat: move order cancels attack");
    if let Err(e) = conn
        .get_sender()
        .send(Packet::from(ObjectActionRequest::Cancel).into())
    {
        error!("network: failed to send attack cancel: {}", e.0);
    }
}

/// Drop the engagement when the target despawns (killed/desynced). No packet:
/// the server ends its own loop on death.
fn clear_attack_on_target_gone(mut active: ResMut<ActiveAttack>, entities: Query<&NetworkId>) {
    if let Some(target) = active.target {
        if entities.get(target).is_err() {
            active.target = None;
        }
    }
}

/// Log the 0xB074 ack (capture-verified 2-byte `phase code` shape). A
/// rejection refuses the NEWEST 0x7074 while the currently running action
/// keeps going (the dumps show the auto-attack stream continuing right
/// through them), so the engagement state stays untouched here —
/// [`action_slot::apply_action_acks`] is what re-parks a refused request and
/// re-fires it once the running action ends.
fn on_object_action_response(mut reader: MessageReader<ObjectActionResponse>) {
    for msg in reader.read() {
        match msg {
            ObjectActionResponse::Started { code } if msg.is_rejected() => {
                info!("combat: action rejected — server action slot busy (code {code})");
            }
            ObjectActionResponse::Started { code } => {
                debug!("combat: action started (code {code})")
            }
            ObjectActionResponse::Ended { code } => debug!("combat: action ended (code {code})"),
            ObjectActionResponse::Failed { code, error } => {
                warn!("combat: action refused (code {code}, error {error:#06x})")
            }
            ObjectActionResponse::Unknown { result, tail } => warn!(
                "combat: unknown 0xB074 shape (result {result} tail {}) — capture for decode",
                hex(tail)
            ),
        }
    }
}

/// Build the popups for one damage block and apply its side effects:
/// optimistic HP decrement per target and a [`Slain`] mark on killing blows
/// (turning the upcoming despawn into a death animation). HP writes are
/// presentation-latency cover only — 0x3057 overwrites authoritatively.
/// Shared by the 0xB070 inline-damage path and the 0xB071 deferred-damage
/// path this server uses for targeted skills.
#[allow(clippy::too_many_arguments)]
fn queue_damage_popups(
    damage: &DamageContent,
    source_uid: Option<u32>,
    local_uid: Option<u32>,
    net: &NetworkEntities,
    transforms: &Query<&GlobalTransform>,
    vitals: &mut Query<&mut EntityVitals>,
    player_vitals: &mut PlayerVitals,
    commands: &mut Commands,
) -> Vec<QueuedPopup> {
    let mut queued: Vec<QueuedPopup> = Vec::new();
    let source_is_local = source_uid.is_some() && local_uid == source_uid;
    for per_entity in &damage.entities {
        let target_is_local = local_uid == Some(per_entity.target);
        let set = if source_is_local {
            HitcountSet::Dealt
        } else if target_is_local {
            HitcountSet::Received
        } else {
            HitcountSet::Other
        };
        let anchor = net.get(per_entity.target);
        let world = anchor
            .and_then(|e| transforms.get(e).ok())
            .map(|gt| gt.translation())
            .unwrap_or_default();
        let mut total: u32 = 0;
        for (hit_index, hit) in per_entity.hits.iter().enumerate() {
            // A displacement arm (4/5) is a knockdown — the only knockdown
            // signal on the wire, since 0x30BF carries no such kind anywhere in
            // the captured corpus. It rides the popup this hit also produces
            // (always: `SkillPartDamage::value()` returns `Some` for
            // `Displaced`), which is what gives it the hit's own timing.
            let knockdown = match &hit.effect {
                HitEffect::Displaced { arm, pos, .. } => Some(KnockdownHit {
                    arm: *arm & 0x7F,
                    position: *pos,
                }),
                _ => None,
            };
            // Arm 2 is the one payload-less arm that still means something to
            // the player: the defender avoided the hit entirely. It has no
            // damage word, so it shows the BLOCK word on its own.
            if hit.is_avoided() {
                if let Some(anchor) = anchor {
                    queued.push(QueuedPopup {
                        hit_index,
                        popup: DamagePopup {
                            anchor,
                            world,
                            delay: 0.0,
                            amount: 0,
                            critical: false,
                            blocked: true,
                            killing_blow: false,
                            set,
                            knockdown: None,
                        },
                    });
                }
                continue;
            }
            // every other payload-less arm carries no popup (#547)
            let Some(value) = hit.value() else { continue };
            let killing_blow = hit.killing_blow;
            total += value.amount;
            let Some(anchor) = anchor else {
                // The target is not in `NetworkEntities` yet: `receive_packets`
                // drains a whole TCP read into one PreUpdate, and an entity
                // spawned in the same tick is created with deferred `Commands`,
                // so the index cannot see it. Same shape as the mount-ack race
                // (#35). Dropping the popup is the safe move — there is nothing
                // to anchor it to — but it was silent, so say it.
                warn_once!(
                    "combat: damage popup dropped, target {} not in the network index yet \
                     (same-tick spawn)",
                    per_entity.target
                );
                continue;
            };
            if killing_blow {
                // `try_insert`, not `insert`: `anchor` came from the network
                // index when the packet arrived, and the entity can be gone by
                // the time this command applies — a COS unsummon, a 0x3016
                // despawn or a corpse expiry can all land in the same tick as
                // the killing blow. A plain insert panics there.
                commands.entity(anchor).try_insert(Slain);
            }
            queued.push(QueuedPopup {
                hit_index,
                popup: DamagePopup {
                    anchor,
                    world,
                    delay: 0.0,
                    amount: value.amount,
                    critical: value.is_critical(),
                    blocked: false,
                    killing_blow,
                    set,
                    knockdown,
                },
            });
        }
        if total == 0 {
            continue;
        }
        if target_is_local {
            player_vitals.hp = player_vitals.hp.saturating_sub(total);
        } else if let Some(mut v) = anchor.and_then(|e| vitals.get_mut(e).ok()) {
            v.hp = v.hp.saturating_sub(total);
        }
    }
    queued
}

/// How long a popup should still wait, given the swing it belongs to already
/// has a resolved timing (`SkillSwingTimings`) but arrived on a packet that
/// carries no animation info of its own — the echo of a click-predicted cast,
/// or a 0xB071's deferred damage. `hit_index` picks the matching combat-hit
/// keytime the same way `play_attack_swings`/`play_skill_swings` do for a
/// fresh swing; clamped to 0 once the hit moment has already passed (a slow
/// packet, or a timing entry re-based to a claimed prediction's echo — see
/// [`SkillSwingTimings::claim`]) rather than going negative.
fn residual_delay(timing: &SwingTiming, hit_index: usize, now: f64) -> f32 {
    let hit_t = timing
        .hit_times
        .get(hit_index)
        .copied()
        .or_else(|| timing.hit_times.last().copied())
        .unwrap_or(0.0);
    let hit_at = timing.started_at + timing.charge_secs as f64 + hit_t as f64;
    (hit_at - now).max(0.0) as f32
}

/// Apply one 0xB070 action update. Basic attacks (kind Attack, no
/// resolvable skill aniset) keep the original path: damage popups riding an
/// [`AttackSwing`] so the animation layer times them to the clip's hit
/// keytimes. Casts whose `skill_id` resolves a real skill aniset
/// ([`resolve_online_swing`]) instead emit the offline pipeline's
/// [`SkillSwing`] — charge + shot animation, cast effects and self-buffs all
/// light up from that one message — carrying inline damage (when present) as
/// [`SkillSwing::popups`], released once the clip resolves its own hit
/// keytimes (or, on the echo of a swing already predicted at the keypress,
/// via [`residual_delay`] against that prediction's claimed timing); this
/// server announces targeted skills as kind-None cast starts and delivers the
/// damage later in 0xB071 (see [`on_skill_end`]), correlated through
/// [`CastInstances`] and [`SkillSwingTimings`].
#[allow(clippy::too_many_arguments)]
fn on_object_action_update(
    mut reader: MessageReader<ObjectActionUpdate>,
    time: Res<Time>,
    net: Res<NetworkEntities>,
    player: Query<&NetworkId, With<Player>>,
    transforms: Query<&GlobalTransform>,
    skill_data: Res<crate::plugins::textdata::ClientSkillData>,
    skill_effects: Res<crate::plugins::textdata::ClientSkillEffects>,
    mut vitals: Query<&mut EntityVitals>,
    mut player_vitals: ResMut<PlayerVitals>,
    mut active: ResMut<ActiveAttack>,
    mut instances: ResMut<CastInstances>,
    mut cooldowns: ResMut<SkillCooldowns>,
    mut predicted: ResMut<crate::plugins::skills::cast::PredictedCasts>,
    // Grouped: Bevy tops out at 16 system params and this system sits at the
    // limit, so the outgoing messages (plus the swing-timing lookup the echo
    // arm needs) travel as one tuple param.
    (
        mut swings,
        mut skill_swings,
        mut popups,
        mut timings,
        mut pending_swings,
        mut running,
        mut landed,
    ): (
        MessageWriter<AttackSwing>,
        MessageWriter<SkillSwing>,
        MessageWriter<DamagePopup>,
        ResMut<SkillSwingTimings>,
        ResMut<crate::plugins::skills::cast::PendingSwings>,
        ResMut<RunningSwings>,
        MessageWriter<LocalAttackLanded>,
    ),
    mut commands: Commands,
) {
    let local_uid = player.single().ok().map(|id| id.0);
    let now = time.elapsed_secs_f64();
    for msg in reader.read() {
        let ObjectActionUpdate::Success {
            skill_id,
            source,
            instance,
            target,
            kind,
            ..
        } = msg
        else {
            match msg {
                ObjectActionUpdate::Failure { error } => {
                    warn!("combat: action failed (error {error:#06x})");
                    active.target = None;
                }
                ObjectActionUpdate::Unknown { result, tail } => warn!(
                    "combat: unknown 0xB070 shape (result {result} tail {}) — capture for decode",
                    hex(tail)
                ),
                ObjectActionUpdate::Success { .. } => unreachable!(),
            }
            continue;
        };
        debug!(
            "combat: action update skill {skill_id} source {source} target {target} kind {kind:?}"
        );
        // remember the cast for 0xB071 damage attribution; prune stragglers
        instances.0.retain(|_, info| {
            now - info.recorded_at < crate::plugins::skills::cast::CAST_INSTANCE_TTL_SECS
        });
        instances.0.insert(
            *instance,
            CastInstanceInfo {
                source: *source,
                skill_id: *skill_id,
                recorded_at: now,
            },
        );
        // Pin the attacker's facing on its victim (both directions: the
        // player attacking a strafing monster AND monsters circling us).
        if let (Some(owner), Some(victim)) = (net.get(*source), net.get(*target)) {
            if owner != victim {
                commands.entity(owner).insert(FaceTarget(victim));
            }
        }
        // our cast just EXECUTED — start the cooldown mirror now (vanilla
        // timing: at the skill firing, not the keypress; the server sends no
        // cooldown packet and enforces silently via the 0x7074 reject)
        if local_uid == Some(*source) {
            if let Some(row) = skill_data.get(&(*skill_id as i32)) {
                if row.reuse_delay_ms() > 0 {
                    cooldowns
                        .0
                        .insert(*skill_id, now + row.reuse_delay_ms() as f64 / 1000.0);
                }
            }
        }
        let spec = resolve_online_swing(*skill_id, &skill_data, &skill_effects);
        let damage = match kind {
            ActionKind::Attack { damage } => damage.as_ref(),
            // kind-None cast starts (and self-buffs): skill presentation
            // only, the damage follows in 0xB071
            ActionKind::None => None,
            ActionKind::Teleport => continue,
        };
        // Our own attack: say how many instances it delivered, so the ammo
        // prediction can spend one arrow per arrow actually loosed. Taken here,
        // off the packet itself, rather than off the swing presentation — every
        // swing produces exactly one of these, including the echo of a swing we
        // predicted at the click (the clip was already playing, but the arrow
        // still flew).
        if local_uid == Some(*source) && matches!(kind, ActionKind::Attack { .. }) {
            let instances = damage.map_or(1, |damage| damage.instance_count.max(1));
            landed.write(LocalAttackLanded(instances as u16));
        }
        let queued = damage
            .map(|damage| {
                queue_damage_popups(
                    damage,
                    Some(*source),
                    local_uid,
                    &net,
                    &transforms,
                    &mut vitals,
                    &mut player_vitals,
                    &mut commands,
                )
            })
            .unwrap_or_default();
        let owner = net.get(*source);
        match (spec, owner) {
            (Some(spec), Some(owner)) => {
                // If we already played this cast at the keypress, this packet
                // is its echo: presenting it again would restart the clip
                // mid-swing and spawn a second projectile, impact effect and
                // set of sounds. The popups below are NOT suppressed — the
                // prediction deliberately shows no numbers, so these are the
                // only ones there are.
                let echo = local_uid == Some(*source)
                    && predicted.claim_echo(*skill_id, time.elapsed_secs_f64());
                // A CONTINUATION of the clip already playing on this body.
                //
                // Every segment of a chain shares one skilldata `Basic_Group`,
                // so `spec.codename` is identical across the whole combo and
                // they all resolve the SAME animation — there is exactly one
                // clip for the chain (1873/1873 chains corpus-wide). Its
                // duration equals the sum of its segments' authored durations
                // and its combat-hit keytimes sit on the segment boundaries,
                // so a continuation is DAMAGE, not animation: it lands on the
                // next hit event of the running clip. Presenting each segment
                // as its own swing restarted a 2433 ms clip four times and
                // showed only its first ~600 ms, four times over.
                //
                // The count is the segment's HIT count, not its popup count:
                // `queued` holds one popup per (entity × instance), so an AoE
                // segment landing on three monsters would otherwise burn three
                // of the clip's hit events and exhaust the record after one
                // segment — after which every later segment fell through to a
                // fresh swing and replayed the whole clip.
                let hits = damage.map_or(1, |d| d.instance_count.max(1) as usize);
                let continuation = running.claim_continuation(owner, &spec.codename, hits, now);
                if let Some((timing, hit_at)) = continuation {
                    for q in queued {
                        let mut popup = q.popup;
                        popup.delay = residual_delay(&timing, hit_at + q.hit_index, now);
                        popups.write(popup);
                    }
                } else if !echo {
                    // Real damage rides the swing itself: play_skill_swings
                    // releases each popup once it knows the clip's own hit
                    // keytimes, instead of at packet arrival.
                    let swing = SkillSwing {
                        owner,
                        skill_id: Some(*skill_id),
                        codename: spec.codename,
                        anim_group: spec.anim_group,
                        ready_anim_type: spec.ready_anim_type,
                        wait_anim_type: spec.wait_anim_type,
                        anim_type: spec.anim_type,
                        // No wind-up: this packet IS the execution, the server
                        // already spent preparing+casting before sending it.
                        // (A chain's clip additionally bakes the wind-up in —
                        // its length is the sum of its segments' full
                        // preparing+casting+action.) The click prediction and
                        // the offline sim keep their charge; only this path
                        // would be replaying one the server already performed.
                        charge_secs: 0.0,
                        preparing_secs: 0.0,
                        flying_speed: spec.flying_speed,
                        damage: None,
                        // The victim the server named, so this cast's
                        // projectile/impact effects have somewhere to fly.
                        target: net.get(*target),
                        popups: queued,
                        instance: Some(*instance),
                    };
                    // Queued rather than presented outright, so it cannot cut
                    // a clip still playing on this body (the server restarts
                    // its auto-attack loop while a chain clip is still
                    // running). `advance_pending_swings` releases it as soon
                    // as the body is free; a standalone cast on an idle body
                    // goes out on the next frame.
                    if let Some(swing) =
                        pending_swings.offer(PendingSwing::Skill(Box::new(swing)), now)
                    {
                        if let PendingSwing::Skill(swing) = swing {
                            skill_swings.write(*swing);
                        }
                    }
                } else {
                    // The clip is already playing from the keypress
                    // prediction. Time the popups against that prediction's
                    // own resolved timing (claimed here, now that the server
                    // has named this instance).
                    timings.claim(*skill_id, *instance);
                    let timing = timings.get(*instance);
                    for q in queued {
                        let mut popup = q.popup;
                        popup.delay = timing
                            .map(|t| residual_delay(t, q.hit_index, now))
                            .unwrap_or(0.0);
                        popups.write(popup);
                    }
                }
            }
            (None, Some(owner)) if matches!(kind, ActionKind::Attack { .. }) => {
                // The animation layer replays these popups with hit-timed
                // delays — the pre-skill auto-attack path, byte-identical.
                //
                // If we already played the opening swing at the click, drop the
                // *clip* but keep the popups: the prediction showed no numbers,
                // so releasing them here is what makes them appear at all. They
                // still land on the swing already in flight, because
                // `play_attack_swings` timed it from the same clip.
                // If we already swung at the click, this packet is that swing's
                // echo: deliver it so its numbers still get timed to the clip's
                // hit keytimes, but do not restart the clip itself.
                let echo = local_uid == Some(*source)
                    && predicted.claim_auto_attack_echo(time.elapsed_secs_f64());
                let attack = AttackSwing {
                    owner,
                    popups: queued,
                    replay_clip: !echo,
                };
                // Through the same per-caster gate as skill segments. The
                // server restarts its auto-attack loop the instant a chain
                // ends — measured 1.99 / 2.68 / 3.74 s after segment 1 of a
                // 3/4/5-segment chain — and `play_attack_swings` has no
                // busy guard of its own, so an arriving swing used to
                // `stop_all()` a skill segment still playing. Queueing keeps
                // its damage popups with it rather than splitting them off.
                if let Some(swing) = pending_swings.offer(PendingSwing::Attack(attack), now) {
                    if let PendingSwing::Attack(attack) = swing {
                        swings.write(attack);
                    }
                }
            }
            // No source entity to animate — show the numbers immediately.
            _ => {
                for q in queued {
                    popups.write(q.popup);
                }
            }
        }
    }
}

/// A slain entity's despawn arrived: keep the corpse for a few seconds (the
/// animation layer plays the die clip), stop its movement, and drop every
/// reference the UI holds on it — vanilla closes the target window on death.
fn handle_entity_death(
    mut deaths: MessageReader<EntityDied>,
    players: Query<(), With<Player>>,
    mut movement: Query<&mut RemoteMovement>,
    mut selected: ResMut<SelectedEntity>,
    mut active: ResMut<ActiveAttack>,
    mut slot: ResMut<action_slot::ActionSlot>,
    mut commands: Commands,
) {
    for EntityDied(entity) in deaths.read() {
        let mut cmds = commands.entity(*entity);
        // Fallible throughout: this runs off a message, so the entity it names
        // may already have been despawned by whatever else the same tick
        // delivered (see the `try_insert` note in `queue_damage_popups`).
        // a corpse stops tracking whoever it was fighting
        cmds.try_remove::<FaceTarget>();
        // The local player must NOT despawn (#141): it enters the death window /
        // resurrect flow instead of the monster-corpse pipeline. Everything else
        // lingers as a corpse and the `Dying` timer despawns it.
        if !players.contains(*entity) {
            cmds.try_insert(Dying::lingering());
        }
        if let Ok(mut movement) = movement.get_mut(*entity) {
            movement.target = None;
        }
        if selected.0 == Some(*entity) {
            selected.0 = None;
        }
        if active.target == Some(*entity) {
            active.target = None;
        }
        // ...and a press still waiting for the action slot must not go out at
        // the thing that just died. The slot re-checks liveness at send time
        // too; dropping it here frees the slot for the player's next press
        // instead of holding a doomed intent for the whole buffer window.
        slot.forget_target(*entity);
    }
}

/// While engaged, close a visible gap to the target: the server stops the
/// approach where *it* considers the target in range, which can be short
/// when the monster moved — walk the render-side remainder locally (no
/// packet; the server re-syncs us with its next 0xB021 anyway). Standing:
/// correct any standoff past the engagement's reach. Walking: re-aim only
/// when the current destination has gone stale (the monster drifted past
/// the slack), so a drifting monster gets chased instead of parked beside.
fn close_attack_gap(
    active: Res<ActiveAttack>,
    weapon: Res<EquippedWeapon>,
    dying: Query<(), With<Dying>>,
    players: Query<&Transform, With<Player>>,
    targets: Query<&Transform, Without<Player>>,
    mut player_commands: ResMut<PlayerCommands>,
) {
    let Some(target) = active.target else {
        return;
    };
    if dying.contains(target) {
        return;
    }
    let (Ok(player), Ok(target_tf)) = (players.single(), targets.get(target)) else {
        return;
    };
    let stop_distance = weapon.engagement_reach(active.stop_range);
    let trigger = stop_distance + ATTACK_GAP_SLACK;
    let target_xz = target_tf.translation.xz();
    if let Some(destination) = player_commands.move_destination() {
        // mid-walk: the queued destination still lands within reach of the
        // monster's CURRENT position — leave the walk alone
        if target_xz.distance(destination.xz()) <= trigger {
            return;
        }
    } else if target_xz.distance(player.translation.xz()) <= trigger {
        return;
    }
    let delta = target_xz - player.translation.xz();
    let distance = delta.length().max(f32::EPSILON);
    let direction = delta / distance;
    let stop = target_xz - direction * stop_distance;
    player_commands.move_to(Vec3::new(stop.x, player.translation.y, stop.y));
}

/// 0x30BF life-state → dead is the real death signal (it arrives before the
/// 0x3016 despawn): start the death presentation immediately. The despawn
/// handlers keep their killing-blow fallback for deaths this misses.
///
/// The **local player** is special-cased (#141): its death sets the
/// [`PlayerDeath`] flag (driving the death window + input lock) but still plays
/// the die clip via [`EntityDied`] — `handle_entity_death` skips the corpse
/// `Dying`/despawn for it. A later life→Alive (the SPEC-derived revive signal,
/// see docs/re/notes/death-resurrect.md) clears the flag.
///
/// An update whose entity has not spawned yet is **retried**, not dropped:
/// see [`STATE_UPDATE_RETRY_FRAMES`] for the race that makes that necessary.
fn on_entity_state_update(
    mut reader: MessageReader<EntityStateUpdate>,
    net: Res<NetworkEntities>,
    dying: Query<(), With<Dying>>,
    players: Query<(), With<Player>>,
    mut pending: Local<Vec<(EntityStateUpdate, u8)>>,
    mut movement: Query<&mut RemoteMovement>,
    mut death: ResMut<PlayerDeath>,
    mut died: MessageWriter<EntityDied>,
) {
    pending.extend(reader.read().map(|msg| (msg.clone(), 0)));
    pending.retain_mut(|(msg, attempts)| {
        let Some(entity) = net.get(msg.unique_id) else {
            // Not spawned *yet* — keep it and try again next frame. See
            // STATE_UPDATE_RETRY_FRAMES for why this cannot just be dropped.
            *attempts += 1;
            return *attempts < STATE_UPDATE_RETRY_FRAMES;
        };
        let is_local = players.contains(entity);
        // A gait change (kind 1) is the server telling us which locomotion
        // clip to play — monsters toggle it constantly while wandering (#275).
        if let Some(walking) = msg.motion_walking() {
            if let Ok(mut movement) = movement.get_mut(entity) {
                movement.walking = walking;
            }
        }
        if msg.is_death() {
            if is_local {
                death.set_dead();
            }
            if !dying.contains(entity) {
                died.write(EntityDied(entity));
            }
        } else if is_local && msg.is_revive() {
            death.set_alive();
        }
        false
    });
}

/// 0x3011 is the server's dedicated "your character died" push, sent only to
/// the dying client, so it raises the death window directly. The 0x30BF
/// life→Dead path above still does the same, but only once the local player's
/// uid resolves through [`NetworkEntities`]; this one carries no uid and needs
/// no lookup. Both are idempotent, and the revive (life→Alive) clears the flag.
///
/// `death_cause` is deliberately not branched on — only `0x04` has ever been
/// observed and its value space is UNKNOWN (docs/net-entity-events-0x3011.md).
fn on_character_died(
    mut reader: MessageReader<CharacterDied>,
    mut death: ResMut<PlayerDeath>,
    mut attack: ResMut<ActiveAttack>,
) {
    for msg in reader.read() {
        debug!("character died, cause byte {:#04x}", msg.death_cause);
        death.set_dead();
        // Drop the engagement: the gap-close chase re-queues a move order
        // every frame while a target is held, and the movement gate alone
        // would leave that queue churning behind a frozen corpse.
        attack.target = None;
        attack.stop_range = None;
    }
}

/// Slerp each [`FaceTarget`] holder's yaw toward its victim whenever it isn't
/// actively path-following (walking rotation wins while moving). Ends when
/// the victim despawns or becomes a corpse.
#[allow(clippy::too_many_arguments)]
fn face_attack_targets(
    time: Res<Time>,
    player_commands: Res<PlayerCommands>,
    players: Query<(), With<Player>>,
    movement: Query<&RemoteMovement>,
    targets: Query<&GlobalTransform>,
    dying: Query<(), With<Dying>>,
    mut facing: Query<(Entity, &FaceTarget, &mut Transform)>,
    mut commands: Commands,
) {
    for (entity, FaceTarget(target), mut transform) in facing.iter_mut() {
        let Ok(target_gt) = targets.get(*target) else {
            commands.entity(entity).remove::<FaceTarget>();
            continue;
        };
        if dying.contains(*target) {
            commands.entity(entity).remove::<FaceTarget>();
            continue;
        }
        let moving = (players.contains(entity) && player_commands.is_moving())
            || movement.get(entity).is_ok_and(|m| m.target.is_some());
        if moving {
            continue;
        }
        let delta = (target_gt.translation() - transform.translation).xz();
        if delta.length_squared() < f32::EPSILON {
            continue;
        }
        let direction = delta.normalize();
        // same convention as remote walking: SRO bodies face -Z, so add PI
        let target_rotation =
            Quat::from_rotation_y(direction.x.atan2(direction.y) + std::f32::consts::PI);
        transform.rotation = transform.rotation.slerp(
            target_rotation,
            (FACE_TURN_SPEED * time.delta_secs()).min(1.0),
        );
    }
}

/// The level-up flash, attached to whoever 0x3054 names.
const LEVELUP_EFFECT: &str = "particles://system/system_levelup.efp";
/// How long the level-up effect wrapper lives (the .efp finishes well within).
const LEVELUP_EFFECT_SECS: f32 = 6.0;

/// A fire-and-forget effect wrapper: despawned when the timer runs out
/// (effect wrappers are not cleaned up by the effects plugin itself).
#[derive(Component)]
pub struct TimedEffect {
    timer: Timer,
}

impl TimedEffect {
    pub fn new(secs: f32) -> Self {
        Self {
            timer: Timer::from_seconds(secs, TimerMode::Once),
        }
    }

    /// Reset the timer to a new total lifetime (used to refine a fallback
    /// lifetime once an effect's true duration is known).
    pub fn set_remaining(&mut self, secs: f32) {
        self.timer = Timer::from_seconds(secs.max(0.05), TimerMode::Once);
    }
}

/// 0x3054: play the level-up flash on the leveling entity (attached to its
/// body wrapper so it follows and anchors at the feet like vanilla).
fn on_entity_level_up(
    mut reader: MessageReader<EntityLevelUp>,
    net: Res<NetworkEntities>,
    children: Query<&Children>,
    wrappers: Query<(), With<SpawnedFromResource>>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
) {
    for msg in reader.read() {
        let Some(owner) = net.get(msg.unique_id) else {
            continue;
        };
        info!("combat: level up for uid {}", msg.unique_id);
        let anchor = children
            .get(owner)
            .into_iter()
            .flat_map(|children| children.iter())
            .find(|child| wrappers.contains(*child))
            .unwrap_or(owner);
        let effect = commands.attach_effect(
            asset_server.load(LEVELUP_EFFECT),
            anchor,
            Transform::IDENTITY,
        );
        commands.entity(effect).insert(TimedEffect {
            timer: Timer::from_seconds(LEVELUP_EFFECT_SECS, TimerMode::Once),
        });
    }
}

/// Despawn expired fire-and-forget effect wrappers.
fn expire_timed_effects(
    time: Res<Time>,
    mut effects: Query<(Entity, &mut TimedEffect)>,
    mut commands: Commands,
) {
    for (entity, mut effect) in effects.iter_mut() {
        if effect.timer.tick(time.delta()).just_finished() {
            commands.entity(entity).despawn();
        }
    }
}

/// Despawn corpses once their linger runs out.
fn finish_dying(time: Res<Time>, mut dying: Query<(Entity, &mut Dying)>, mut commands: Commands) {
    for (entity, mut dying) in dying.iter_mut() {
        if dying.timer.tick(time.delta()).just_finished() {
            commands.entity(entity).despawn();
        }
    }
}

/// Apply 0xB071 cast ends: this server delivers a targeted skill's damage
/// here (~the visual hit moment) rather than inline in 0xB070, so a
/// damage-carrying end pops the numbers through the shared helper, with the
/// caster attributed via the [`CastInstances`] entry its 0xB070 recorded.
/// Popups are timed against that same instance's [`SkillSwingTimings`] entry
/// (its swing's own hit keytimes, via [`residual_delay`]) rather than
/// released at packet arrival — the swing already started when its 0xB070
/// landed, so "the server timed the packet" does not by itself mean this
/// packet's arrival is the visual hit moment.
#[allow(clippy::too_many_arguments)]
fn on_skill_end(
    mut reader: MessageReader<SkillEnd>,
    time: Res<Time>,
    net: Res<NetworkEntities>,
    player: Query<&NetworkId, With<Player>>,
    transforms: Query<&GlobalTransform>,
    mut vitals: Query<&mut EntityVitals>,
    mut player_vitals: ResMut<PlayerVitals>,
    mut instances: ResMut<CastInstances>,
    timings: Res<SkillSwingTimings>,
    mut popups: MessageWriter<DamagePopup>,
    mut local_ends: MessageWriter<LocalCastEnded>,
    mut commands: Commands,
) {
    let local_uid = player.single().ok().map(|id| id.0);
    let now = time.elapsed_secs_f64();
    for msg in reader.read() {
        match msg {
            SkillEnd::Success {
                instance,
                target,
                kind,
            } => {
                let info = instances.0.remove(instance);
                debug!("combat: cast {instance} ended (target {target} kind {kind:?})");
                // our own cast ended → the auto-attack resume trigger
                if local_uid.is_some() && info.as_ref().map(|i| i.source) == local_uid {
                    local_ends.write(LocalCastEnded);
                }
                let ActionKind::Attack {
                    damage: Some(damage),
                } = kind
                else {
                    continue;
                };
                let queued = queue_damage_popups(
                    damage,
                    info.map(|i| i.source),
                    local_uid,
                    &net,
                    &transforms,
                    &mut vitals,
                    &mut player_vitals,
                    &mut commands,
                );
                let timing = timings.get(*instance);
                for q in queued {
                    let mut popup = q.popup;
                    popup.delay = timing
                        .map(|t| residual_delay(t, q.hit_index, now))
                        .unwrap_or(0.0);
                    popups.write(popup);
                }
            }
            SkillEnd::Unknown { result, tail } => debug!(
                "combat: unknown 0xB071 shape (result {result} tail {})",
                hex(tail)
            ),
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use packets::agent::prelude::{LIFE_STATE_DEAD, STATE_KIND_LIFE};

    /// A player at `player_x` engaged with a target at [`TARGET_X`] on +X.
    fn gap_app(weapon: EquippedWeapon, stop_range: Option<f32>, player_x: f32) -> App {
        let mut app = App::new();
        app.init_resource::<PlayerCommands>()
            .insert_resource(weapon)
            .add_systems(Update, close_attack_gap);
        let target = app
            .world_mut()
            .spawn(Transform::from_xyz(TARGET_X, 0.0, 0.0))
            .id();
        app.world_mut()
            .spawn((Player, Transform::from_xyz(player_x, 0.0, 0.0)));
        app.insert_resource(ActiveAttack {
            target: Some(target),
            stop_range,
        });
        app
    }

    const TARGET_X: f32 = 300.0;

    /// Where the queued walk ends up, as a distance from the target.
    fn standoff(app: &App) -> Option<f32> {
        let destination = app
            .world()
            .resource::<PlayerCommands>()
            .move_destination()?;
        Some(
            Vec3::new(TARGET_X, 0.0, 0.0)
                .xz()
                .distance(destination.xz()),
        )
    }

    /// The reported defect: with a bow the gap-close walked the *rendered*
    /// player to the melee fallback (16) while the server kept the real
    /// character at bow reach, so the body ended up beside the monster and the
    /// monster chased a position ~145 units behind it.
    #[test]
    fn a_bow_engagement_stops_at_the_weapons_reach() {
        let mut app = gap_app(
            EquippedWeapon {
                class: Some(6),
                reach: Some(180.0),
            },
            None,
            0.0,
        );
        app.update();

        let standoff = standoff(&app).expect("the gap-close queues a walk");
        assert!(
            (standoff - 180.0).abs() < 0.01,
            "walked to {standoff} from the target, expected the bow's 180"
        );
    }

    /// Empty hands (or itemdata still loading) keep the old behaviour exactly.
    #[test]
    fn an_unknown_weapon_still_closes_to_the_melee_fallback() {
        let mut app = gap_app(EquippedWeapon::default(), None, 0.0);
        app.update();

        let standoff = standoff(&app).expect("the gap-close queues a walk");
        assert!(
            (standoff - ATTACK_GAP_STOP).abs() < 0.01,
            "walked to {standoff}"
        );
    }

    /// A skill's authored col-21 range outranks the weapon it is cast with.
    #[test]
    fn an_authored_skill_range_overrides_the_weapon() {
        let mut app = gap_app(
            EquippedWeapon {
                class: Some(6),
                reach: Some(180.0),
            },
            Some(200.0),
            0.0,
        );
        app.update();

        let standoff = standoff(&app).expect("the gap-close queues a walk");
        assert!((standoff - 200.0).abs() < 0.01, "walked to {standoff}");
    }

    /// The other half of the symptom: already inside bow reach, the character
    /// must stand and shoot rather than close any further.
    #[test]
    fn a_bow_engagement_already_in_reach_queues_no_walk() {
        // 100 units out — comfortably inside the bow's reach
        let mut app = gap_app(
            EquippedWeapon {
                class: Some(6),
                reach: Some(180.0),
            },
            None,
            TARGET_X - 100.0,
        );
        app.update();

        assert_eq!(
            standoff(&app),
            None,
            "a character already in range must not walk closer"
        );
    }

    fn death_of(uid: u32) -> EntityStateUpdate {
        EntityStateUpdate {
            unique_id: uid,
            kind: STATE_KIND_LIFE,
            value: LIFE_STATE_DEAD,
        }
    }

    fn race_app() -> App {
        let mut app = App::new();
        app.init_resource::<NetworkEntities>()
            .init_resource::<PlayerDeath>()
            .add_message::<EntityStateUpdate>()
            .add_message::<EntityDied>()
            .add_systems(Update, on_entity_state_update);
        app
    }

    fn deaths_seen(app: &mut App) -> usize {
        app.world_mut()
            .resource_mut::<Messages<EntityDied>>()
            .drain()
            .count()
    }

    /// The defect: a `/zoe2` monster's life→dead lands 38 ms after its spawn
    /// record — inside the window where the uid is not in `NetworkEntities`
    /// yet, because the spawn is a deferred command. Dropping it left a
    /// monster that was never marked `Dying`, so it kept its hover and its
    /// pickable meshes and was still selectable as a corpse.
    #[test]
    fn a_death_that_beats_its_spawn_is_applied_once_the_entity_arrives() {
        let mut app = race_app();

        // the death arrives first, for a uid nothing has spawned
        app.world_mut().write_message(death_of(99));
        app.update();
        assert_eq!(deaths_seen(&mut app), 0, "nothing to kill yet");

        // ...then the spawn command lands and registers the uid
        app.world_mut().spawn(NetworkId(99));
        app.update();
        assert_eq!(
            deaths_seen(&mut app),
            1,
            "the held death was never delivered"
        );

        // and it is delivered exactly once
        app.update();
        assert_eq!(deaths_seen(&mut app), 0, "the death fired twice");
    }

    /// The other half: a uid that never appears must not be retried forever.
    /// An entity can leave range between the two packets.
    #[test]
    fn a_death_for_an_entity_that_never_spawns_is_given_up_on() {
        let mut app = race_app();
        app.world_mut().write_message(death_of(1234));

        for _ in 0..(STATE_UPDATE_RETRY_FRAMES as usize + 2) {
            app.update();
        }
        // it expired rather than lingering: a late spawn finds nothing waiting
        app.world_mut().spawn(NetworkId(1234));
        app.update();
        assert_eq!(deaths_seen(&mut app), 0, "an expired death still fired");
    }

    /// A death for an entity that IS registered still works the direct way —
    /// the retry must not have turned the common path into a one-frame delay.
    #[test]
    fn a_death_for_a_spawned_entity_lands_the_same_frame() {
        let mut app = race_app();
        app.world_mut().spawn(NetworkId(7));
        app.update();
        deaths_seen(&mut app);

        app.world_mut().write_message(death_of(7));
        app.update();
        assert_eq!(deaths_seen(&mut app), 1);
    }

    /// `residual_delay` should hand back the wait still remaining until the
    /// swing's own hit keytime, not the raw keytime itself — a popup arriving
    /// after the swing already started must not be re-delayed from zero.
    #[test]
    fn residual_delay_counts_down_from_the_swing_start_not_from_now() {
        let timing = SwingTiming {
            started_at: 10.0,
            charge_secs: 0.5,
            hit_times: vec![0.2, 0.6],
        };
        // hit #0 lands at 10.0 + 0.5 + 0.2 = 10.7; asked 0.1s into the swing
        assert_eq!(residual_delay(&timing, 0, 10.1), 0.6);
        // hit #1 lands at 11.1; asked right at that instant
        assert!((residual_delay(&timing, 1, 11.1) - 0.0).abs() < f32::EPSILON);
    }

    /// A popup for a hit whose moment has already passed (a late packet, or a
    /// timing re-based to a claimed prediction's echo) clamps to an immediate
    /// pop rather than going negative.
    #[test]
    fn residual_delay_clamps_to_zero_once_the_hit_already_landed() {
        let timing = SwingTiming {
            started_at: 0.0,
            charge_secs: 0.0,
            hit_times: vec![0.1],
        };
        assert_eq!(residual_delay(&timing, 0, 5.0), 0.0);
    }

    /// A hit index past the end of the swing's known keytimes (a multi-hit
    /// packet against a clip with fewer authored hit events) staggers past
    /// the last one instead of panicking or defaulting to an immediate pop.
    #[test]
    fn residual_delay_falls_back_to_the_last_known_keytime() {
        let timing = SwingTiming {
            started_at: 0.0,
            charge_secs: 0.0,
            hit_times: vec![0.3],
        };
        // index 5 has no matching keytime — falls back to the last one (0.3)
        assert_eq!(residual_delay(&timing, 5, 0.0), 0.3);
    }

    /// `SkillSwingTimings::claim` moves a click-prediction's timing (filed
    /// under its skill id, before any server instance existed) over to the
    /// instance id the server names for it, so the echo path can look it up.
    #[test]
    fn claiming_a_prediction_makes_it_findable_by_instance() {
        let mut timings = SkillSwingTimings::default();
        let timing = SwingTiming {
            started_at: 1.0,
            charge_secs: 0.2,
            hit_times: vec![0.1],
        };
        timings.record_predicted(42, timing);
        assert!(timings.get(99).is_none(), "not claimed yet");

        timings.claim(42, 99);
        assert!(
            timings.get(99).is_some(),
            "claim should file it by instance"
        );

        // claiming again (e.g. a stray duplicate) is a no-op, not a panic
        timings.claim(42, 100);
        assert!(timings.get(100).is_none());
    }
}
