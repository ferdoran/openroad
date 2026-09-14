//! Status effects: incapacitating debuffs (stun/freeze) and timed
//! self-buffs with their looping aura effects.
//!
//! Two sources, and they are not equally good. **Online the wire wins**: the
//! 0x3057 bad-status block is an absolute per-entity ailment bitmask, decoded
//! into `net::entities::EntityAilments` and presented by
//! [`sync_ailment_effects`] at the bottom of this file. That is the
//! authoritative path for monsters and the local player alike.
//!
//! The skilldata path below is the OFFLINE simulation's, and it rests on
//! weaker evidence: only `'st' <duration_ms> <prob%> <level>` (stun) is
//! corpus-verified. `'fz'/'fb'/'es'/'bu'` — freeze / frostbite / electric
//! shock / burn — have **inferred names and UNKNOWN argument counts**
//! (`docs/re/gamedata/textdata-effect-option-tables.md`), `'fb'` does not even
//! appear in the tag census, and they are read with `param_after`, a linear
//! scan that can match an argument value rather than a tag. Treat what it
//! produces as a demo, not as fact. The local simulation applies them
//! deterministically (no
//! probability roll — documented simplification, so a stun skill always
//! demonstrably stuns). Incapacitated entities stop moving, stop facing,
//! and get their in-flight one-shot cast cancelled by the player plugin;
//! the cast pipeline refuses new casts from an incapacitated caster.
//! Self-buffs hold an [`AppliedBuff`] with the skill's `'dura'` duration
//! and own their ACT_S burst / ACT_L loop emissions, despawned on expiry.

use bevy::prelude::*;

use crate::assets::textdata::skilldata::SkillDataRow;
use crate::assets::textdata::skilleffect::{EffectPhase, SkillEntry};
use crate::commands::SkeletonBinding;
use crate::plugins::combat::{FaceTarget, TimedEffect};
use crate::plugins::effects::spawn::DelayedEffect;
use crate::plugins::effects::OneShotEffect;
use crate::plugins::net::entities::RemoteMovement;
use crate::plugins::player::{Player, PlayerCommands};
use crate::plugins::skills::cast::SkillSwingStarted;
use crate::plugins::textdata::{ClientSkillData, ClientSkillEffects};

/// Stunned: no movement, no casting, current cast cancelled.
#[derive(Component)]
pub struct Stunned(pub Timer);

/// Frozen: same incapacitation as stun (visual distinction comes later).
#[derive(Component)]
pub struct Frozen(pub Timer);

/// Query filter matching any incapacitating status.
pub type IncapacitatedFilter = Or<(With<Stunned>, With<Frozen>)>;

/// Fallback duration for status params that carry no duration argument
/// (freeze & co.), and for buffs without a `'dura'` tag.
const STATUS_FALLBACK_SECS: f32 = 5.0;
const BUFF_FALLBACK_SECS: f32 = 10.0;

/// One active self-buff on an entity, owning its aura effect wrappers.
pub struct BuffInstance {
    pub codename: String,
    pub timer: Timer,
    /// Spawned ACT_L loop wrappers, despawned when the buff expires
    /// (ACT_S bursts clean themselves up via [`TimedEffect`]).
    pub loop_effects: Vec<Entity>,
    /// The skill's icon (`media://icon/...`), shown on the magic state
    /// board next to the player mini info.
    pub icon: Option<String>,
    /// The server's buff-instance id (0xB0BD), what the assumed 0xB072
    /// removal references; `None` for purely local applications.
    pub instance_id: Option<u32>,
}

/// The entity's active self-buffs (the local player today).
#[derive(Component, Default)]
pub struct ActiveBuffs(pub Vec<BuffInstance>);

/// Read the row's status params and insert the matching debuff components
/// on `target`. Deterministic: any listed status applies.
pub fn apply_status_effects(commands: &mut Commands, row: &SkillDataRow, target: Entity) {
    if let Some(duration) = row.param_after("st") {
        let secs = (duration as f32 / 1000.0).max(0.1);
        info!("skills: stunning target for {secs:.1}s");
        commands
            .entity(target)
            .insert(Stunned(Timer::from_seconds(secs, TimerMode::Once)));
    }
    // freeze/frostbite share the frozen presentation; shock stuns briefly
    if row.param_after("fz").is_some() || row.param_after("fb").is_some() {
        info!("skills: freezing target for {STATUS_FALLBACK_SECS}s");
        commands.entity(target).insert(Frozen(Timer::from_seconds(
            STATUS_FALLBACK_SECS,
            TimerMode::Once,
        )));
    }
    if row.param_after("es").is_some() {
        info!("skills: shocking target");
        commands
            .entity(target)
            .insert(Stunned(Timer::from_seconds(2.0, TimerMode::Once)));
    }
}

/// While stunned/frozen: stop server-style movement and target facing; stop
/// the local player's click-to-move. (One-shot cast cancellation lives in
/// the player plugin, which owns the wrapper animation state.)
pub fn enforce_incapacitation(
    mut afflicted: Query<(Entity, Option<&mut RemoteMovement>, Has<Player>), IncapacitatedFilter>,
    mut player_commands: ResMut<PlayerCommands>,
    mut commands: Commands,
) {
    for (entity, movement, is_player) in afflicted.iter_mut() {
        if let Some(mut movement) = movement {
            movement.target = None;
        }
        if is_player {
            player_commands.stop();
        }
        commands.entity(entity).remove::<FaceTarget>();
    }
}

/// Tick status timers and drop expired components.
pub fn expire_status_effects(
    time: Res<Time>,
    mut stunned: Query<(Entity, &mut Stunned)>,
    mut frozen: Query<(Entity, &mut Frozen)>,
    mut commands: Commands,
) {
    for (entity, mut status) in stunned.iter_mut() {
        if status.0.tick(time.delta()).just_finished() {
            commands.entity(entity).remove::<Stunned>();
        }
    }
    for (entity, mut status) in frozen.iter_mut() {
        if status.0.tick(time.delta()).just_finished() {
            commands.entity(entity).remove::<Frozen>();
        }
    }
}

/// Spawn a buff's ACT_S burst + ACT_L loop emissions on `wrapper`, anchored
/// to their authored bones (pelvis/waist/hand) with the raw offset,
/// inheriting the single character mirror — the same recipe as
/// `cast::spawn_cast_effects` and the always-on effect path; falls back to
/// the wrapper when the bone is unnamed. Returns the loop wrappers (the
/// caller despawns them when the buff ends); ACT_S bursts clean themselves
/// up via [`TimedEffect`] and play once ([`OneShotEffect`]).
fn spawn_buff_auras(
    entry: &SkillEntry,
    wrapper: Entity,
    skeletons: &Query<&SkeletonBinding>,
    asset_server: &AssetServer,
    commands: &mut Commands,
) -> Vec<Entity> {
    let mut loop_effects = Vec::new();
    for emission in &entry.emissions {
        let looping = match emission.phase {
            EffectPhase::ActL => true,
            EffectPhase::ActS => false,
            _ => continue,
        };
        let anchor = skeletons
            .get(wrapper)
            .ok()
            .and_then(|binding| {
                emission
                    .start_bone
                    .as_ref()
                    .and_then(|bone| binding.bones.get(bone).copied())
            })
            .unwrap_or(wrapper);
        let mut spawned = commands.spawn((
            DelayedEffect {
                handle: asset_server.load(format!("particles://{}", emission.efp_path)),
                timer: Timer::from_seconds(0.0, TimerMode::Once),
            },
            Transform::from_translation(emission.start_offset),
            Visibility::Inherited,
            Name::new(format!("buff effect: {}", emission.efp_path)),
            ChildOf(anchor),
        ));
        if looping {
            loop_effects.push(spawned.id());
        } else {
            spawned.insert((TimedEffect::new(6.0), OneShotEffect));
        }
    }
    loop_effects
}

/// Self-buff application, piggybacking on the swing-start report so the
/// aura effects anchor on the caster's loaded body wrapper: skills without
/// an enemy target hold an [`AppliedBuff`]-style entry for their `'dura'`
/// and spawn their ACT_S burst + ACT_L loop emissions. Re-casting an
/// active buff refreshes its timer.
pub fn apply_self_buffs(
    mut started: MessageReader<SkillSwingStarted>,
    skill_data: Res<ClientSkillData>,
    skill_effects: Res<ClientSkillEffects>,
    mut buffs: Query<&mut ActiveBuffs>,
    skeletons: Query<&SkeletonBinding>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
) {
    for swing in started.read() {
        let Some(entry) = skill_effects.get(&swing.codename) else {
            continue;
        };
        // enemy-targeted swings were damage casts; buffs are the self casts
        // anything non-offensive that got this far is a self-buff — imbues
        // (activity 1) carry all-zero target columns, so a positive
        // `targets_self` requirement would miss them. The EXACT row (via the
        // swing's skill id) wins: the codename scan lands on an arbitrary
        // level of the group, giving the wrong 'dura'/icon for leveled buffs.
        let row = swing
            .skill_id
            .and_then(|id| skill_data.get(&(id as i32)))
            .or_else(|| {
                skill_data.data().and_then(|data| {
                    data.values()
                        .find(|r| r.basic_group() == Some(swing.codename.as_str()))
                })
            });
        let is_buff = row.is_some_and(|r| !r.targets_enemy());
        if !is_buff {
            continue;
        }
        let duration = row
            .and_then(|r| r.param_after("dura"))
            .map(|ms| ms as f32 / 1000.0)
            .unwrap_or(BUFF_FALLBACK_SECS);

        // re-cast of an active buff just refreshes its timer (only the
        // timer: an adopted 0xB0BD instance_id must survive the refresh)
        if let Ok(mut active) = buffs.get_mut(swing.owner) {
            if let Some(existing) = active
                .0
                .iter_mut()
                .find(|buff| buff.codename == swing.codename)
            {
                existing.timer = Timer::from_seconds(duration, TimerMode::Once);
                continue;
            }
        }

        let loop_effects = spawn_buff_auras(
            entry,
            swing.wrapper,
            &skeletons,
            &asset_server,
            &mut commands,
        );
        info!(
            "skills: buff {} active for {duration:.1}s ({} loop effects)",
            swing.codename,
            loop_effects.len()
        );
        let instance = BuffInstance {
            codename: swing.codename.clone(),
            timer: Timer::from_seconds(duration, TimerMode::Once),
            loop_effects,
            icon: row.and_then(|r| r.icon_path()),
            instance_id: None,
        };
        push_buff_instance(&mut buffs, &mut commands, swing.owner, instance);
    }
}

/// Push or refresh (by codename) a buff on `owner`. A re-application only
/// rewrites the timer and adopts a wire instance id — existing loop effects
/// and icon stay, so a 0xB0BD following our own cast's [`apply_self_buffs`]
/// entry MERGES instead of duplicating (the incoming duplicate's loop
/// effects are despawned).
pub(crate) fn push_buff_instance(
    buffs: &mut Query<&mut ActiveBuffs>,
    commands: &mut Commands,
    owner: Entity,
    instance: BuffInstance,
) {
    if let Ok(mut active) = buffs.get_mut(owner) {
        if let Some(existing) = active
            .0
            .iter_mut()
            .find(|buff| buff.codename == instance.codename)
        {
            existing.timer = instance.timer;
            if instance.instance_id.is_some() {
                existing.instance_id = instance.instance_id;
            }
            for effect in instance.loop_effects {
                if let Ok(mut e) = commands.get_entity(effect) {
                    e.despawn();
                }
            }
            return;
        }
        active.0.push(instance);
    } else {
        commands.entity(owner).insert(ActiveBuffs(vec![instance]));
    }
}

/// Buffs with no wire duration and no skilldata `'dura'` are treated as
/// permanent-until-removed: a day-long Timer keeps the gauge/expiry
/// machinery untouched (infinite `Duration`s panic, and an `Option<Timer>`
/// would ripple through expire_buffs for no visible gain — the board gauge
/// simply reads full).
const PERMANENT_BUFF_SECS: f32 = 24.0 * 3600.0;

/// 0xB0BD/0xB072 — server-driven buff add/remove (the join-time auto-buffs
/// and every networked cast's authoritative buff state). Network adds also
/// spawn the ACT_S/ACT_L aura effects UNLESS the entity already shows the
/// buff (our own cast's [`apply_self_buffs`] entry — the following 0xB0BD
/// then merges by codename without re-spawning): without this, item/scroll
/// buffs got their board icon but no visual (the fourth-playtest report).
/// The aura anchors on the owner's body wrapper (the child carrying the
/// [`SkeletonBinding`]); if the body hasn't streamed in yet, the effects
/// fall back to the entity root.
pub fn apply_network_buffs(
    mut adds: MessageReader<packets::agent::prelude::BuffAdd>,
    mut removes: MessageReader<packets::agent::prelude::BuffRemove>,
    net: Res<crate::plugins::net::entities::NetworkEntities>,
    skill_data: Res<ClientSkillData>,
    skill_effects: Res<ClientSkillEffects>,
    mut buffs: Query<&mut ActiveBuffs>,
    children: Query<&Children>,
    skeletons: Query<&SkeletonBinding>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
) {
    for add in adds.read() {
        let Some(owner) = net.get(add.unique_id) else {
            // join-order race: the buff may beat the entity registration —
            // logged so playtests can tell whether a retry buffer is needed
            debug!("skills: buff add for unknown uid {}", add.unique_id);
            continue;
        };
        let Some(row) = skill_data.get(&(add.ref_skill_id as i32)) else {
            debug!("skills: buff add for unknown skill {}", add.ref_skill_id);
            continue;
        };
        // no duration on the wire: skilldata 'dura' when authored, else
        // permanent-until-0xB072. NOT the 10s BUFF_FALLBACK — that would
        // silently expire real permanent buffs.
        let duration = row
            .param_after("dura")
            .map(|ms| ms as f32 / 1000.0)
            .unwrap_or(PERMANENT_BUFF_SECS);
        let codename = row.basic_group().unwrap_or(row.code_name()).to_string();
        info!(
            "skills: server buff {codename} on uid {} ({duration:.0}s, instance {})",
            add.unique_id, add.buff_instance_id
        );
        let already_shown = buffs
            .get(owner)
            .is_ok_and(|active| active.0.iter().any(|buff| buff.codename == codename));
        let loop_effects = if already_shown {
            Vec::new()
        } else {
            skill_effects
                .get(&codename)
                .map(|entry| {
                    let wrapper = children
                        .get(owner)
                        .into_iter()
                        .flat_map(|c| c.iter())
                        .find(|&child| skeletons.contains(child))
                        .unwrap_or(owner);
                    spawn_buff_auras(entry, wrapper, &skeletons, &asset_server, &mut commands)
                })
                .unwrap_or_default()
        };
        push_buff_instance(
            &mut buffs,
            &mut commands,
            owner,
            BuffInstance {
                codename,
                timer: Timer::from_seconds(duration, TimerMode::Once),
                loop_effects,
                icon: row.icon_path(),
                instance_id: Some(add.buff_instance_id),
            },
        );
    }
    // 0xB072 is a LIST (count-prefixed) — see BuffRemove. Captures only ever
    // showed one id, but the original loops, so drop every id it names.
    for remove in removes.read() {
        for instance_id in &remove.buff_instance_ids {
            info!("skills: buff remove instance {instance_id} (0xB072)");
            for mut active in buffs.iter_mut() {
                let Some(pos) = active
                    .0
                    .iter()
                    .position(|buff| buff.instance_id == Some(*instance_id))
                else {
                    continue;
                };
                let buff = active.0.remove(pos);
                for effect in buff.loop_effects {
                    if let Ok(mut e) = commands.get_entity(effect) {
                        e.despawn();
                    }
                }
                info!("skills: buff {} removed by server", buff.codename);
            }
        }
    }
}

/// Cancel an active buff by codename: despawn its aura wrappers and drop
/// the instance (the magic state board's right-click path). Returns whether
/// a buff was removed. Nothing goes to the server — SilkroadDoc documents
/// buff add/remove only as server→client (0xB0BD/0xB072); the client's
/// cancel-request opcode needs a vanilla capture before it can be wired.
pub fn cancel_buff(active: &mut ActiveBuffs, codename: &str, commands: &mut Commands) -> bool {
    let Some(pos) = active.0.iter().position(|buff| buff.codename == codename) else {
        return false;
    };
    let buff = active.0.remove(pos);
    for effect in buff.loop_effects {
        if let Ok(mut e) = commands.get_entity(effect) {
            e.despawn();
        }
    }
    info!("skills: buff {} cancelled", buff.codename);
    true
}

/// Tick buffs; despawn loop effects when one expires.
pub fn expire_buffs(time: Res<Time>, mut buffs: Query<&mut ActiveBuffs>, mut commands: Commands) {
    for mut active in buffs.iter_mut() {
        active.0.retain_mut(|buff| {
            if !buff.timer.tick(time.delta()).just_finished() {
                return true;
            }
            info!("skills: buff {} expired", buff.codename);
            for effect in buff.loop_effects.drain(..) {
                if let Ok(mut e) = commands.get_entity(effect) {
                    e.despawn();
                }
            }
            false
        });
    }
}

// --- Wire-driven ailments (0x3057 bad-status mask) --------------------------
//
// Idea: online the server owns ailments outright — `EntityBarsUpdate`'s
// `flag & 0x04` block carries an absolute per-entity `BadStatus` bitmask
// (`net::entities::EntityAilments`), for monsters and the local player alike.
// That is a far better source than `apply_status_effects` above, which infers
// stun/freeze from skilldata fourcc tags whose names and argument counts are
// still `[U]` — see this module's header. The offline sim keeps using the
// tags because it has no wire; online, this is what actually fires.
//
// Presentation is the archive's own art: each ailment names an authored
// `battle/status_bad_<name>.efp`, attached to the body wrapper for as long as
// the bit stays set. **No tint is applied** — no per-ailment colour exists in
// any SRO data we have (textdata, resinfo, or the RE corpus), so choosing one
// would be exactly the unsourced magic number ADR-0009 forbids. An ailment
// whose `.efp` is missing from the archive simply shows nothing rather than
// borrowing another's art.

use packets::agent::prelude::Ailment;

use crate::plugins::net::entities::EntityAilments;

/// The authored looping debuff effect for one ailment, when the archive has
/// one paired to it ([`Ailment::archive_name`] documents the pairing and which
/// ailments deliberately have none).
fn ailment_effect(ailment: Ailment) -> Option<String> {
    ailment
        .archive_name()
        .map(|name| format!("particles://battle/status_bad_{name}.efp"))
}

/// The ailment effect wrappers currently attached to an entity, keyed by the
/// mask bit that spawned them, so a bit going out despawns exactly its own.
#[derive(Component, Default)]
pub struct AilmentEffects {
    /// The mask these effects were spawned for — the diff base.
    shown: u32,
    effects: Vec<(u32, Entity)>,
}

/// Reconcile each entity's authored ailment effects with the mask 0x3057 last
/// reported for it: spawn a looping `.efp` for every bit that came on, despawn
/// the wrappers of every bit that went off.
///
/// Diffed against what is actually spawned rather than driven by change
/// detection, because the mask is re-sent unchanged on every vitals tick — a
/// burning monster's 0x3057 repeats `08000000` for the whole burn — and
/// re-spawning the aura each tick would stack dozens of copies.
pub fn sync_ailment_effects(
    changed: Query<(Entity, &EntityAilments, Option<&Children>)>,
    wrappers: Query<(), With<crate::commands::SpawnedFromResource>>,
    skeletons: Query<&SkeletonBinding>,
    mut shown: Query<&mut AilmentEffects>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
) {
    for (entity, ailments, children) in changed.iter() {
        let mask = ailments.0 .0;
        let current = shown.get(entity).map(|s| s.shown).unwrap_or(0);
        if mask == current {
            continue;
        }
        // The body wrapper is where the model (and its skeleton) lives; the
        // root is the fallback for the frames before it has streamed in.
        let anchor = children
            .into_iter()
            .flat_map(|c| c.iter())
            .find(|child| wrappers.contains(*child))
            .unwrap_or(entity);
        // Nothing to anchor to yet — leave `shown` untouched so this retries
        // next frame once the body arrives, rather than recording a mask whose
        // effects never spawned.
        if anchor == entity && skeletons.get(entity).is_err() && children.is_none() {
            continue;
        }
        let mut entry = match shown.get_mut(entity) {
            Ok(entry) => entry,
            Err(_) => {
                commands
                    .entity(entity)
                    .try_insert(AilmentEffects::default());
                continue; // pick it up next frame, once the component exists
            }
        };
        // bits that went out
        entry.effects.retain(|(bit, effect)| {
            if mask & bit != 0 {
                return true;
            }
            if let Ok(mut e) = commands.get_entity(*effect) {
                e.despawn();
            }
            false
        });
        // bits that came on
        for ailment in ailments.0.ailments() {
            let bit = ailment.bit();
            if current & bit != 0 {
                continue;
            }
            let Some(path) = ailment_effect(ailment) else {
                // No authored file pairs to this bit — show nothing rather
                // than borrow another ailment's art. Still recorded in
                // `shown`, so it is not retried every frame.
                debug!("status: {ailment:?} on {entity} has no authored effect — nothing played");
                continue;
            };
            debug!("status: {ailment:?} on {entity} — playing {path}");
            let effect = commands
                .spawn((
                    DelayedEffect {
                        handle: asset_server.load(path.clone()),
                        timer: Timer::from_seconds(0.0, TimerMode::Once),
                    },
                    Transform::IDENTITY,
                    Visibility::Inherited,
                    Name::new(format!("ailment effect: {path}")),
                    ChildOf(anchor),
                ))
                .id();
            entry.effects.push((bit, effect));
        }
        entry.shown = mask;
    }
}
