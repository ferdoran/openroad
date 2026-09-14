//! Index from a server-assigned unique id to the Bevy `Entity` representing it.
//!
//! Idea: SRO's "unique IDs" (u32) key every in-world entity — the local player,
//! other players, NPCs, monsters, dropped items — and they thread through almost
//! every in-game packet (spawn, movement, stat/hp updates, ...). Packet handlers
//! need to resolve "which entity is unique id N" cheaply, so we keep a
//! `HashMap<u32, Entity>` and mark the owning entity with a [`NetworkId`]
//! component.
//!
//! The map is kept in sync automatically by [`NetworkId`]'s component lifecycle
//! hooks: adding the component inserts the mapping, removing it (including on
//! despawn) removes it. That makes the index self-maintaining and free of stale
//! entries, without a polling/prune system.

use std::collections::HashMap;

use bevy::ecs::lifecycle::HookContext;
use bevy::ecs::world::DeferredWorld;
use bevy::prelude::*;

use packets::agent::prelude::{BadStatus, EntityBarsUpdate, SelectEntityResponse};

use crate::assets::nvm::JMXVNVM;
use crate::plugins::nav::{NavLocation, NavMeshRaycast};

/// How fast a remote entity turns (radians/sec of slerp weight) to face its
/// direction of travel — mirrors the local player's `PLAYER_TURN_SPEED`.
const REMOTE_TURN_SPEED: f32 = 12.0;

/// Which kind of networked entity a [`RemoteEntity`] is — the local per-frame
/// systems (input, follow camera) key off [`crate::plugins::player::Player`]
/// instead, so this is deliberately a *different* marker.
///
/// Requires [`NavLocation`]: remote entities are ground-snapped against the
/// surface they are on, so a bridge deck holds them up instead of the water
/// below it.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
#[require(NavLocation)]
pub enum RemoteEntity {
    Player,
    Npc,
    Monster,
    Item,
}

/// Marks a [`RemoteEntity::Monster`] whose characterdata rarity class is
/// "unique" (3) — the world bosses the minimap highlights with the big sign.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct UniqueMonster;

/// The characterdata ref id the entity spawned from, so HUD code (target
/// window, tooltips) can re-look the row up without re-threading it at spawn.
#[derive(Component, Clone, Copy, Debug)]
pub struct CharacterRef(pub u32);

/// Interaction option ids a talkable NPC advertises in its spawn record.
/// NOTE: playtests showed these are NOT a reliable dialog-option list (city
/// guards advertise trade-ish bits) — the dialog derives its options from
/// the shop/teleport/speech tables and only logs these.
#[derive(Component, Clone, Debug)]
pub struct NpcTalkOptions(pub Vec<u8>);

/// A meshless interactable (teleport gate) still waiting for its invisible
/// clickable volume (`cursor::interactions::npcs::equip_gate_volumes`).
#[derive(Component)]
pub struct GateVolumeNeeded;

/// The buffs a remote entity carried in its spawn record, as `ref_skill_id`s in
/// wire order (`net/reader.rs`'s character-state block: a `u8` count followed by
/// that many `ActiveBuff` entries). Only the skill ids are kept: they are what
/// resolves an icon through `ClientSkillData`, whereas the entry's `duration`
/// would need a per-entity timer nothing ticks today.
///
/// Distinct from [`crate::plugins::skills::status::ActiveBuffs`], which owns
/// spawned aura effects and expiry timers for the local player. This is the
/// remote, display-only view.
#[derive(Component, Clone, Debug, Default, PartialEq, Eq)]
pub struct RemoteBuffs(pub Vec<u32>);

/// The abnormal-state mask 0x3057 last reported for this entity.
///
/// Present on any entity the server has sent a bad-status block for — the
/// local player and monsters alike, since 0x3057 is per-uid. The mask is
/// **absolute**, not a delta: a body with the block clears whatever is not in
/// it, and a body *without* the block says nothing at all (so it must not be
/// read as "no ailments" — see [`EntityBarsUpdate::bad_status`]).
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EntityAilments(pub BadStatus);

/// A remote **character's** body state (0x30BF kind 4, or the spawn record's
/// state block): GM invisibility, stealth, untouchable and so on.
///
/// Deliberately kept only on `RemoteEntity::Player`. `packets`'
/// [`packets::agent::prelude::hidden_render`] carries the reason in full: a
/// census of `packet_dump/0x30bf.log` finds body-state 4 on 217 distinct unique
/// ids, so what that value means for a **non-character** entity is UNKNOWN, and
/// acting on it world-wide could hide most of the world.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RemoteBodyState(pub u8);

/// The per-instance rarity class of a [`RemoteEntity::Monster`], from the
/// spawn packet's rarity byte (characterdata's column as fallback): 0 normal,
/// 1 champion, 3 unique, 4 giant, 5 titan, 6 elite, 7 strong, 8 unique2;
/// bit 0x10 marks a party mob (per skrillax's `EntityRarity`). Drives the
/// target window's `tw_icon_*` badge + label. [`UniqueMonster`] stays the
/// coarse "world boss" marker (class 3).
#[derive(Component, Clone, Copy, Debug)]
pub struct MonsterRarity(pub u8);

impl MonsterRarity {
    /// The class with the party bit masked off.
    pub fn kind(&self) -> u8 {
        self.0 & !0x10
    }

    /// Whether the party-mob bit is set.
    pub fn party(&self) -> bool {
        self.0 & 0x10 != 0
    }

    /// Visual model scale of the class (playtest-calibrated against the
    /// original client). Champions also swap to their `_champ.bmt` recolor;
    /// uniques are dedicated characterdata rows and need no scaling.
    pub fn scale(&self) -> f32 {
        match self.kind() {
            1 => 1.5,  // champion
            4 => 5.0,  // giant
            5 => 1.6,  // titan
            6 => 1.4,  // elite
            7 => 1.25, // strong
            _ => 1.0,
        }
    }
}

/// The rarity class of a [`RemoteEntity::Item`] ground drop, from the 0x3015
/// item record's rarity byte (`net/entity_spawn.rs::parse_item`).
///
/// It exists because a ground drop carries **no item body**: the spawn record
/// has no opt level and no `mag_params`, so nothing else on the wire can say
/// whether the pile is plain or a "blue" magic-option drop. Seal-grade is a
/// separate, itemdata-derived fact (`ItemDataRow::is_rare`, the `_RARE`
/// code-name suffix), which is also what drives the drop's sparkle effect.
///
/// **Only `0` is confirmed** (= plain; every gold line in
/// `packet_dump/0x3015.log` carries it). Any non-zero class is treated as
/// "carries options" and reads blue; the individual classes above 1 are
/// `UNKNOWN` and deliberately not mapped — verify against a capture of a known
/// blue vs. set vs. rare drop before splitting them.
#[derive(Component, Clone, Copy, Debug)]
pub struct DropRarity(pub u8);

impl DropRarity {
    /// Whether the drop carries magic ("blue") options.
    pub fn has_options(&self) -> bool {
        self.0 != 0
    }
}

/// A Seal-grade ("_RARE") ground drop — the coarse marker for the drops that
/// also carry the sparkle pillar, set from itemdata rather than the wire
/// (`ItemDataRow::is_rare`). Same relationship to [`DropRarity`] that
/// [`UniqueMonster`] has to [`MonsterRarity`]: one derived flag the render
/// side can query without reaching back into textdata.
#[derive(Component, Clone, Copy, Debug)]
pub struct SealDrop;

/// Live vitals of a remote entity. `max_hp` comes from characterdata
/// (RefObjChar `MaxHP`) at spawn with `hp` seeded to full; `hp` then follows
/// `EntityBarsUpdate` (0x3057). Only monsters get one today — max HP for
/// remote players is not on the wire.
#[derive(Component, Clone, Copy, Debug)]
pub struct EntityVitals {
    pub hp: u32,
    pub max_hp: u32,
}

impl EntityVitals {
    pub fn full(max_hp: u32) -> Self {
        Self { hp: max_hp, max_hp }
    }

    /// Current HP as a 0..=1 bar fill.
    pub fn fill(&self) -> f32 {
        if self.max_hp == 0 {
            return 0.0;
        }
        (self.hp as f32 / self.max_hp as f32).clamp(0.0, 1.0)
    }

    /// Take the server's absolute HP, correcting `max_hp` upward if the wire
    /// exceeds it.
    ///
    /// `max_hp` is only ever a *guess*: it comes from the characterdata row at
    /// spawn, and no packet confirms it — 0x3057 carries no maximum at all, and
    /// 0x303D (the one that does) is local-player-only. So the wire is the
    /// authority on current HP, and a current above our guess proves the guess
    /// wrong rather than the packet.
    ///
    /// This used to clamp the other way (`hp.min(max_hp)`), which silently
    /// truncated every such entity: the bar pinned at 100 % for the whole first
    /// half of the fight and then fell in one step.
    pub fn apply_server_hp(&mut self, hp: u32) {
        if hp > self.max_hp {
            self.max_hp = hp;
        }
        self.hp = hp;
    }
}

/// The name drawn on the entity's floating nameplate. Set at spawn: the packet
/// name for players (local and remote), the localized characterdata name for
/// NPCs/monsters. Kept separate from Bevy's debug `Name` (which carries the
/// unique-id suffix) so the nameplate reads a clean display string uniformly.
#[derive(Component, Clone, Debug)]
pub struct DisplayName(pub String);

/// Server-driven movement target for a [`RemoteEntity`]: the render-space point
/// the entity is walking toward (from a `MovementResponse`).
/// [`move_remote_entities`] eases the transform toward it; `None` means standing
/// still.
#[derive(Component, Default, Clone, Copy, Debug)]
pub struct RemoteMovement {
    pub target: Option<Vec3>,
    /// Units per second; seeded from the entity's run speed (a sane default here).
    pub speed: f32,
    /// The gait the server put this entity in: `true` = walking
    /// (`MOTION_STATE_WALK`), `false` = running. Seeded from the spawn
    /// record's state block and updated by 0x30BF motion updates; selects the
    /// WALK animation over RUN (#275).
    pub walking: bool,
}

impl RemoteMovement {
    /// Default ground run speed (SRO units/sec) until a per-entity speed is known.
    pub const DEFAULT_SPEED: f32 = 50.0;
}

/// Ease each [`RemoteEntity`] toward its server-given [`RemoteMovement`] target,
/// clearing it on arrival. A stand-in for authoritative interpolation until the
/// per-entity motion model lands; keeps remote entities from teleporting.
///
/// The walk is done in XZ only, with the Y snapped to the walkable surface each
/// frame — the server's Y is unreliable, so following it verbatim floats/sinks
/// the entity. The entity also turns to face its direction of travel so the walk
/// reads correctly instead of sliding sideways.
///
/// Deliberately `ground()` and not `step()`: remote entities are
/// server-authoritative and must never be stopped by a client-side wall test,
/// or they visibly freeze or desync when the client and server disagree. Only
/// the height is corrected; the tracked surface follows along.
fn move_remote_entities(
    time: Res<Time>,
    nav: NavMeshRaycast,
    mut query: Query<(&mut Transform, &mut RemoteMovement, &mut NavLocation), With<RemoteEntity>>,
) {
    for (mut transform, mut movement, mut nav_location) in query.iter_mut() {
        let Some(target) = movement.target else {
            continue;
        };
        let speed = if movement.speed > 0.0 {
            movement.speed
        } else {
            RemoteMovement::DEFAULT_SPEED
        };

        let current_xz = transform.translation.xz();
        let target_xz = target.xz();
        let delta = target_xz - current_xz;
        let distance = delta.length();
        let step = speed * time.delta_secs();

        let next_xz = if distance <= step || distance < f32::EPSILON {
            movement.target = None;
            target_xz
        } else {
            let direction = delta / distance;
            // Face the walking direction (yaw only). The SRO body faces -Z, so add
            // PI to the +Z yaw; slerp gives a smooth turn instead of a snap.
            let target_rotation =
                Quat::from_rotation_y(direction.x.atan2(direction.y) + std::f32::consts::PI);
            transform.rotation = transform.rotation.slerp(
                target_rotation,
                (REMOTE_TURN_SPEED * time.delta_secs()).min(1.0),
            );
            current_xz + direction * step
        };

        let next_y = match nav.ground(next_xz, transform.translation.y, *nav_location) {
            Some((height, location)) => {
                *nav_location = location;
                height
            }
            // Nav data not streamed in: keep the current height rather than
            // dropping the entity to zero.
            None => transform.translation.y,
        };
        transform.translation = Vec3::new(next_xz.x, next_y, next_xz.y);
    }
}

/// Marks a [`RemoteEntity`] whose Y still needs reconciling with the walkable
/// surface — set at spawn and after a teleport. The region's nav mesh may not be
/// streamed in yet at that moment (so the height query returns `None`), so
/// [`snap_remote_entities_to_ground`] retries each frame until it succeeds.
#[derive(Component, Default, Clone, Copy, Debug)]
pub struct NeedsGroundSnap;

/// Snap [`NeedsGroundSnap`] entities down onto the walkable surface once the
/// covering nav mesh has loaded, then drop the marker. Stationary spawns and
/// teleports go through here (walking entities are snapped by
/// [`move_remote_entities`] every frame).
fn snap_remote_entities_to_ground(
    mut commands: Commands,
    nav: NavMeshRaycast,
    mut query: Query<(Entity, &mut Transform, &mut NavLocation), With<NeedsGroundSnap>>,
) {
    for (entity, mut transform, mut nav_location) in query.iter_mut() {
        let xz = transform.translation.xz();
        // The marker means the position moved discontinuously (spawn,
        // teleport), so any tracked surface is meaningless — resolve afresh.
        if let Some((y, location)) =
            nav.ground(xz, transform.translation.y, NavLocation::Unresolved)
        {
            transform.translation.y = y;
            *nav_location = location;
            commands.entity(entity).remove::<NeedsGroundSnap>();
        }
    }
}

/// Follow per-entity HP updates (0x3057) into [`EntityVitals`]. The local
/// player has no `EntityVitals` (its HUD reads `PlayerVitals`), so resolving
/// the unique id and fetching the component naturally scopes this to remote
/// monsters. Values are absolute, and authoritative over the characterdata max
/// (see [`EntityVitals::apply_server_hp`]).
///
/// Ordered after the combat prediction by [`VitalsSet`] — the whole point is
/// that the server's number lands last.
fn update_remote_vitals(
    mut updates: MessageReader<EntityBarsUpdate>,
    index: Res<NetworkEntities>,
    mut vitals: Query<&mut EntityVitals>,
    mut commands: Commands,
) {
    for update in updates.read() {
        let Some(entity) = index.get(update.unique_id) else {
            continue;
        };
        if let Some(hp) = update.hp {
            if let Ok(mut vitals) = vitals.get_mut(entity) {
                vitals.apply_server_hp(hp);
            }
        }
        // The ailment mask is absolute, so a body carrying the block replaces
        // the whole set — including clearing it. A body WITHOUT the block says
        // nothing about ailments and must leave the component alone.
        if let Some(status) = update.bad_status() {
            commands.entity(entity).try_insert(EntityAilments(status));
        }
    }
}

/// Seed a monster's current HP from the select-entity answer (0xB045) when the
/// server actually fills it in — go-sro stubs it to 0, which `monster_hp()`
/// reads as "unknown", so this is a no-op there but correct against vSRO.
fn apply_select_response_vitals(
    mut responses: MessageReader<SelectEntityResponse>,
    index: Res<NetworkEntities>,
    mut vitals: Query<&mut EntityVitals>,
) {
    for response in responses.read() {
        let Some(hp) = response.monster_hp() else {
            continue;
        };
        let Some(entity) = index.get(response.unique_id) else {
            continue;
        };
        if let Ok(mut vitals) = vitals.get_mut(entity) {
            vitals.apply_server_hp(hp);
        }
    }
}

/// The server-assigned unique id of an in-world entity. Adding, REPLACING or
/// removing this component keeps [`NetworkEntities`] in sync via the hooks
/// below. `on_insert`/`on_discard` (not `on_add`/`on_remove`): the server
/// reassigns the local player's uid on every teleport and the client swaps
/// the id by re-inserting the component — `on_add` fires only for genuinely
/// new components, so the replace left the index on the STALE uid and every
/// per-uid packet for the post-teleport player (the buffed 0x30D0 speeds,
/// vitals, movement corrections) silently missed. `on_discard` sees the old
/// value before it's overwritten AND still fires on plain removal/despawn.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
#[component(on_insert = network_id_added, on_discard = network_id_removed)]
pub struct NetworkId(pub u32);

/// Resolves a unique id to its entity in O(1). Maintained by [`NetworkId`]'s
/// hooks — never mutate it directly.
#[derive(Resource, Default)]
pub struct NetworkEntities(HashMap<u32, Entity>);

impl NetworkEntities {
    /// The entity currently registered for `id`, if any.
    pub fn get(&self, id: u32) -> Option<Entity> {
        self.0.get(&id).copied()
    }

    /// Number of registered entities (for diagnostics).
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// `on_insert`: registers the NEW value — runs on first add and after every
/// replace (the replace sequence is `on_discard(old)` then `on_insert(new)`,
/// so a swapped uid deregisters before re-registering).
fn network_id_added(mut world: DeferredWorld, ctx: HookContext) {
    let Some(&NetworkId(id)) = world.entity(ctx.entity).get::<NetworkId>() else {
        return;
    };
    if let Some(mut index) = world.get_resource_mut::<NetworkEntities>() {
        index.0.insert(id, ctx.entity);
    }
}

/// `on_discard`: the OLD component value is still readable here, on replace,
/// removal and despawn alike.
fn network_id_removed(mut world: DeferredWorld, ctx: HookContext) {
    let Some(&NetworkId(id)) = world.entity(ctx.entity).get::<NetworkId>() else {
        return;
    };
    if let Some(mut index) = world.get_resource_mut::<NetworkEntities>() {
        // Only drop the entry if it still points at this entity — guards against
        // clobbering an id that was just re-registered onto another entity.
        if index.0.get(&id) == Some(&ctx.entity) {
            index.0.remove(&id);
        }
    }
}

/// Label for the remote ground-snap + movement chain, so dependents (the COS
/// rider-slaving system) can order after the frame's entity movement.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RemoteMovementSet;

/// Ordering for everything that writes [`EntityVitals`].
///
/// Idea: two kinds of writer share this component and they are not
/// commutative. Combat predicts a **relative** decrement the moment a damage
/// packet arrives (`combat::queue_damage_popups`), while 0x3057/0xB045 carry
/// the server's **absolute** value (`update_remote_vitals`,
/// `apply_select_response_vitals`). A whole TCP read drains into one
/// `PreUpdate`, so a hit and its authoritative follow-up routinely land in the
/// same frame — and until this existed all three ran in plain unordered
/// tuples.
///
/// That made the target bar flicker: with the absolute write first, the
/// prediction then subtracted the same damage a second time from a value that
/// already contained it, and the next 0x3057 snapped the bar back up. Pinning
/// prediction before authority makes the double-subtract impossible, because
/// the server's number is always the last word within a frame.
///
/// The prediction is kept rather than deleted: it is what makes a hit feel
/// immediate, and it costs nothing once it can no longer stack on top of the
/// correction.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VitalsSet {
    /// Client-side prediction from combat results. Relative.
    Predicted,
    /// The server's own values. Absolute, and always last.
    Authoritative,
}

/// Registers the [`NetworkEntities`] index. The `NetworkId` hooks are wired at
/// the type level, so this only needs to make the resource exist.
pub struct NetworkEntitiesPlugin;

impl Plugin for NetworkEntitiesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NetworkEntities>()
            // Also registered by `add_network_events`; repeated here so the
            // plugin stands alone (unit tests build it without the net stack).
            .add_message::<EntityBarsUpdate>()
            .add_message::<SelectEntityResponse>()
            // Both borrow `NavMeshRaycast` (which needs `Assets<JMXVNVM>`); the
            // headless net-check client has no asset plugins, so gate them on the
            // nav-mesh asset collection existing — otherwise SystemParam
            // validation panics every frame before login even completes.
            .add_systems(
                Update,
                (snap_remote_entities_to_ground, move_remote_entities)
                    .chain()
                    .in_set(RemoteMovementSet)
                    .run_if(resource_exists::<Assets<JMXVNVM>>),
            )
            .configure_sets(
                Update,
                VitalsSet::Predicted.before(VitalsSet::Authoritative),
            )
            .add_systems(
                Update,
                (update_remote_vitals, apply_select_response_vitals)
                    .in_set(VitalsSet::Authoritative),
            );
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// The ordinary case: the server's number is taken verbatim.
    #[test]
    fn server_hp_is_taken_as_authoritative() {
        let mut vitals = EntityVitals::full(100);
        vitals.apply_server_hp(70);
        assert_eq!(vitals.hp, 70);
        assert_eq!(vitals.max_hp, 100, "an in-range value leaves the max alone");
    }

    /// `max_hp` is a characterdata guess that no packet confirms, so a current
    /// HP above it proves the guess wrong. This used to clamp the other way,
    /// which pinned the bar at 100 % until the server dropped below the guess
    /// and then fell in one step.
    #[test]
    fn server_hp_above_the_guessed_max_raises_the_max() {
        let mut vitals = EntityVitals::full(100);
        vitals.apply_server_hp(249);
        assert_eq!(vitals.hp, 249, "the wire is authoritative, not truncated");
        assert_eq!(vitals.max_hp, 249);
        assert_eq!(vitals.fill(), 1.0, "and the bar reads full, not overfull");
    }

    /// The flicker this pass fixes, in the order the frame actually runs it:
    /// prediction first, then the server's absolute value. The prediction must
    /// not survive into the corrected number.
    #[test]
    fn a_prediction_followed_by_the_server_value_does_not_double_subtract() {
        let mut vitals = EntityVitals::full(100);
        // combat predicts the 19 damage the hit packet reported
        vitals.hp = vitals.hp.saturating_sub(19);
        assert_eq!(vitals.hp, 81);
        // ...and 0x3057 confirms the same 81 later in the same frame
        vitals.apply_server_hp(81);
        assert_eq!(vitals.hp, 81, "not 81 - 19 = 62");
    }

    /// `VitalsSet` is what guarantees the order the test above assumes. If the
    /// prediction ran after the absolute write, the same two numbers would
    /// leave 62 and the next update would snap the bar back up.
    #[test]
    fn the_reversed_order_is_what_produced_the_flicker() {
        let mut vitals = EntityVitals::full(100);
        vitals.apply_server_hp(81);
        vitals.hp = vitals.hp.saturating_sub(19);
        assert_eq!(vitals.hp, 62, "the bug, kept here to document the ordering");
    }

    #[test]
    fn index_tracks_add_and_despawn() {
        let mut app = App::new();
        // The plugin's systems read `Res<Time>` (movement) and the nav-mesh asset
        // collections via `NavMeshRaycast`, so `app.update()` below needs both the
        // time plugin and the registered `JMXVNVM`/`JMXVBMS` asset resources.
        app.add_plugins((
            bevy::time::TimePlugin,
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
            NetworkEntitiesPlugin,
        ))
        .init_asset::<crate::assets::nvm::JMXVNVM>()
        .init_asset::<crate::assets::bms::mesh::JMXVBMS>();

        let e1 = app.world_mut().spawn(NetworkId(100)).id();
        let e2 = app.world_mut().spawn(NetworkId(200)).id();
        app.update();

        let index = app.world().resource::<NetworkEntities>();
        assert_eq!(index.get(100), Some(e1));
        assert_eq!(index.get(200), Some(e2));
        assert_eq!(index.len(), 2);

        // Despawn drops the entry via the on_remove hook.
        app.world_mut().entity_mut(e1).despawn();
        let index = app.world().resource::<NetworkEntities>();
        assert_eq!(index.get(100), None);
        assert_eq!(index.get(200), Some(e2));
    }

    #[test]
    fn removing_component_drops_entry() {
        let mut app = App::new();
        app.add_plugins(NetworkEntitiesPlugin);
        let e = app.world_mut().spawn(NetworkId(7)).id();
        assert_eq!(app.world().resource::<NetworkEntities>().get(7), Some(e));
        app.world_mut().entity_mut(e).remove::<NetworkId>();
        assert_eq!(app.world().resource::<NetworkEntities>().get(7), None);
    }

    #[test]
    fn replacing_component_moves_the_entry() {
        // The teleport case: the server reassigns the local player's uid and
        // the client re-inserts NetworkId with the new value — the index
        // must drop the old uid and resolve the new one (with on_add/
        // on_remove hooks the replace updated neither).
        let mut app = App::new();
        app.add_plugins(NetworkEntitiesPlugin);
        let e = app.world_mut().spawn(NetworkId(0x018B50)).id();
        app.world_mut().entity_mut(e).insert(NetworkId(0x018B84));
        let index = app.world().resource::<NetworkEntities>();
        assert_eq!(index.get(0x018B50), None, "stale uid must be dropped");
        assert_eq!(index.get(0x018B84), Some(e), "new uid must resolve");
        assert_eq!(index.len(), 1);
    }
}
