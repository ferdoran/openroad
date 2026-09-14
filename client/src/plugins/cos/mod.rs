//! COS (callable object summons: mounts, transports, pets, guild guards).
//!
//! Idea: a COS is an ordinary remote entity — it spawns through the shared
//! `RemoteEntity::Npc` pipeline (movement lerp, ground snap, nameplates,
//! selection all come for free) plus a [`CosEntity`] marker carrying what the
//! generic pipeline doesn't know: the COS subtype (characterdata tid4) and the
//! owner linkage. Riding is client-side slaving — the server moves the COS
//! entity and every rider copies its transform each frame — so mount state is
//! just [`RiderOf`] on the rider (see `docs/re/systems/mount.md`,
//! xBot `InfoManager.cs:743-771`). The wire family lives in
//! `packets/src/agent/pet.rs`; this plugin is its consumer: 0x30C8 seeds the
//! local player's summon list, 0x30C9 retires it, 0xB0CB flips mount state.
//! Everything works offline too (dev-spawned `local_only` COS skip the wire
//! and apply their transitions directly), which is how riding is testable
//! without a server.

pub mod pet;
pub mod riding;
pub mod spawn;

use bevy::prelude::*;

use packets::agent::prelude::{PetData, PetPlayerMounted, PetUpdate};

use crate::plugins::combat::{Dying, EntityDied, Slain};
use crate::plugins::net::character_info::MovementSpeed;
use crate::plugins::net::entities::{
    DisplayName, EntityVitals, NetworkEntities, RemoteEntity, RemoteMovement,
};
use crate::plugins::player::Player;
use crate::plugins::textdata::{
    ClientCharacterData, ClientItemData, ClientTextNames, ClientUiStrings,
};
use crate::scenes::{in_playable_world, SceneState};
use packets::agent::pet::{CosKind, PetUpdatePayload};

use spawn::spawn_cos_entity;

/// Marker for a spawned COS entity, alongside its `RemoteEntity::Npc` marker.
#[derive(Component, Debug, Clone, Copy)]
pub struct CosEntity {
    pub kind: CosKind,
    /// The summoner's unique id (`None` for ride-only horses, whose spawn
    /// record carries no owner tail, and for locally spawned dev COS).
    pub owner_uid: Option<u32>,
}

/// Whether an entity takes the **character** interaction path — shift-gated
/// hover, no dialog — rather than the NPC one.
///
/// A COS is an NPC to the pipelines that move and label it, and a character to
/// the player pointing at it. Reading `RemoteEntity::Npc` alone therefore gave
/// a pet the talk cursor, made it hoverable without Shift, and opened a
/// shopkeeper dialog on click. The rule lives here, in one place, because it is
/// asked at four unrelated call sites (cursor, hover, click, target window) and
/// four copies of `Has<CosEntity>` would drift apart.
///
/// This subsumes the narrower carve-out the click path used to carry, which
/// exempted only the COS you were *riding* — a pet standing next to you still
/// opened a dialog.
pub fn interacts_as_character(kind: &RemoteEntity, is_cos: bool) -> bool {
    is_cos || matches!(kind, RemoteEntity::Player)
}

/// The summoner's character name, drawn under the COS's own name on its
/// nameplate. A component rather than a field on [`CosEntity`], matching how
/// `GuildTag` and `StallOwner` — the other two nameplate sub-lines — are
/// carried.
#[derive(Component, Debug, Clone)]
pub struct CosOwner(pub String);

/// A player spawned (or logged in) already mounted on the COS with this unique
/// id. Resolved to [`RiderOf`] once the COS entity exists — within a spawn
/// batch the rider can arrive before its mount.
#[derive(Component, Debug, Clone, Copy)]
pub struct PendingMount(pub u32);

/// The entity this rider is transform-slaved to (the COS it sits on). Present
/// on remote riders and on the local player while mounted.
#[derive(Component, Debug, Clone, Copy)]
pub struct RiderOf(pub Entity);

/// One of the local player's active summons — what the HUD status stack and
/// the command bar list. Vehicles cap at one in practice; kept as a list
/// because pets and a mount can be out together.
#[derive(Debug, Clone)]
pub struct CosStatus {
    pub unique_id: u32,
    pub ref_id: u32,
    pub kind: CosKind,
    /// The name its owner gave it, and nothing else — `None` for an unnamed
    /// pet and for the kinds that cannot be named at all. Deliberately not a
    /// display string: its consumer feeds it back in as a spawn parameter, and
    /// a resolved label ("No name", or a species) round-tripping through there
    /// would become the pet's identity. Views resolve it with
    /// [`spawn::pet_display_name`].
    pub name: Option<String>,
    /// HP seed from 0x30C8 (the live bar reads the entity's [`EntityVitals`],
    /// these are the fallback while the entity hasn't spawned yet).
    pub hp: u32,
    pub hp_max: u32,
    /// Spawned by the dev window without a server — every transition
    /// (mount/dismount/unsummon/move) applies directly instead of via packets.
    pub local_only: bool,
}

/// The local player's active summons, newest last.
#[derive(Resource, Default)]
pub struct ActiveCosList(pub Vec<CosStatus>);

impl ActiveCosList {
    pub fn get(&self, unique_id: u32) -> Option<&CosStatus> {
        self.0.iter().find(|c| c.unique_id == unique_id)
    }

    fn remove(&mut self, unique_id: u32) {
        self.0.retain(|c| c.unique_id != unique_id);
    }
}

/// The COS unique id the local player currently rides (`None` = on foot).
#[derive(Resource, Default)]
pub struct RiderState(pub Option<u32>);

/// UI → riding-system commands (from the COS command bar and the dev window).
#[derive(Message, Debug, Clone, Copy)]
pub enum CosCommand {
    /// Mount the COS with this unique id.
    Board(u32),
    /// Get off the currently ridden COS.
    Dismount,
    /// Dismiss the summon entirely.
    Unsummon(u32),
    /// Order the COS to walk to the player ("follow me").
    Follow(u32),
}

pub struct CosPlugin;

impl Plugin for CosPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ActiveCosList>()
            .init_resource::<RiderState>()
            .add_message::<CosCommand>()
            .add_message::<pet::PetCommand>()
            // Also registered by `add_network_events`; repeated so the plugin
            // stands alone in unit tests.
            .add_message::<PetData>()
            .add_message::<PetUpdate>()
            .add_message::<PetPlayerMounted>()
            .add_systems(
                Update,
                (
                    // `on_pet_data` borrows itemdata as well as characterdata,
                    // and the two land from *separate* asset-load events
                    // (`plugins::textdata`) — guarding on only one leaves a
                    // window where SystemParam validation panics.
                    (
                        on_pet_data,
                        on_pet_update,
                        respawn_grown_cos,
                        apply_cos_rename,
                    )
                        .run_if(
                            resource_exists::<ClientCharacterData>
                                .and_then(resource_exists::<ClientItemData>),
                        ),
                    on_pet_mounted,
                    resolve_pending_mounts,
                    pet::handle_pet_commands,
                    pet::send_pet_attack_with_player,
                    pet::on_pet_action_response,
                    pet::on_pet_acks,
                    pet::on_pet_settings_response,
                    pet::on_stuck_distance_warning,
                    riding::handle_cos_commands,
                    riding::route_move_orders_while_mounted,
                    riding::resolve_saddle_seats,
                    riding::hide_held_items_while_mounted,
                )
                    .run_if(in_playable_world),
            )
            // Ground alignment tilts the mount, then the riders are slaved onto
            // the settled transform — same frame, in that order, and both after
            // everything that moves entities.
            .add_systems(
                Update,
                (riding::align_cos_to_slope, riding::slave_riders_to_cos)
                    .chain()
                    .after(crate::plugins::net::entities::RemoteMovementSet)
                    .run_if(in_playable_world)
                    // `align_cos_to_slope` borrows `NavMeshRaycast`, which
                    // needs `Assets<JMXVNVM>`; the headless net-check client
                    // has no asset plugins, so gate it the same way
                    // `RemoteMovementSet` already gates its own raycasters
                    // (`net::entities`) rather than panicking every frame.
                    .run_if(resource_exists::<Assets<crate::assets::nvm::JMXVNVM>>),
            )
            .add_systems(OnExit(SceneState::GameWorld), reset_cos_state)
            .add_systems(OnExit(SceneState::WorldSandbox), reset_cos_state);
    }
}

fn reset_cos_state(
    mut list: ResMut<ActiveCosList>,
    mut rider: ResMut<RiderState>,
    riders: Query<Entity, With<RiderOf>>,
    mut commands: Commands,
) {
    list.0.clear();
    rider.0 = None;
    for entity in riders.iter() {
        commands.entity(entity).try_remove::<RiderOf>();
    }
}

/// 0x30C8 — the per-summon data blob after a summon.
///
/// The tail layout is picked by the model's tid4 (not on the wire), but every
/// kind ends up in [`ActiveCosList`] the same way: what differs is which
/// optional parts of the body were present, and [`PetData::body`] has already
/// resolved that. Pets used to be decoded here and thrown away.
fn on_pet_data(
    mut reader: MessageReader<PetData>,
    char_data: Res<ClientCharacterData>,
    item_data: Res<ClientItemData>,
    index: Res<NetworkEntities>,
    mut list: ResMut<ActiveCosList>,
    vitals: Query<&EntityVitals>,
) {
    for data in reader.read() {
        let Some(row) = char_data.get(&(data.ref_obj_id as i32)) else {
            warn!(
                "cos: 0x30C8 for unknown model {} (uid {})",
                data.ref_obj_id, data.unique_id
            );
            continue;
        };
        let Some(kind) = row.cos_kind() else {
            warn!(
                "cos: 0x30C8 model {} ({}) is not a COS row",
                data.ref_obj_id,
                row.code_name()
            );
            continue;
        };
        let Some(body) = data.body(kind, &*item_data) else {
            warn!(
                "cos: 0x30C8 tail failed to decode (uid {}, kind {kind:?}): {}",
                data.unique_id,
                packets::hexdump(&data.tail, 96)
            );
            continue;
        };
        info!(
            "cos: {} uid={} kind={:?} name={:?} slots={}",
            row.code_name(),
            data.unique_id,
            kind,
            body.name,
            body.inventory_size,
        );
        // Only the given name. An unnamed pet sends a zero-length string, not
        // an absent field, so the empty case has to be filtered explicitly —
        // `Option` alone does not answer "is it named?".
        let name = body.name.clone().filter(|name| !name.trim().is_empty());
        // 0x30C8 carries no *current* HP field — the two leading u32s are
        // `[U]` (`CosBody::unk_a/unk_b`), so seeding the bar from them would
        // be an invented reading. Live HP comes from the entity's own vitals;
        // the maximum is a plain characterdata lookup (`MaxHP`), which is a
        // fact rather than a guess, and is what makes a pet's bar drawable
        // before any vitals packet arrives.
        let (hp, hp_max) = index
            .get(data.unique_id)
            .and_then(|entity| vitals.get(entity).ok())
            .map(|v| (v.hp, v.max_hp))
            .or_else(|| row.max_hp().map(|max| (max, max)))
            .unwrap_or((1, 1));
        list.remove(data.unique_id);
        list.0.push(CosStatus {
            unique_id: data.unique_id,
            ref_id: data.ref_obj_id,
            kind,
            name,
            hp,
            hp_max,
            local_only: false,
        });
    }
}

/// 0x30C9 — per-summon delta.
///
/// `Unsummoned` tears the COS down (entity despawns, status entry drops, a
/// ridden mount force-dismounts). `ModelChanged` is the growth-stage swap: the
/// server hands over a new *ref object*, so the status entry re-keys onto it
/// and the entity is respawned from the new model rather than mutated in
/// place — the mesh, skeleton and animation set all change together, which is
/// exactly what [`spawn::spawn_cos_entity`] already builds.
///
/// The bag, exp, hunger and rename arms are [`crate::plugins::hud::cos`]'s to
/// apply — they change what the windows show, not what exists in the world.
fn on_pet_update(
    mut reader: MessageReader<PetUpdate>,
    index: Res<NetworkEntities>,
    char_data: Res<ClientCharacterData>,
    mut list: ResMut<ActiveCosList>,
    mut rider: ResMut<RiderState>,
    riders: Query<(Entity, &RiderOf)>,
    slain: Query<(), With<Slain>>,
    dying: Query<(), With<Dying>>,
    mut died: MessageWriter<EntityDied>,
    mut commands: Commands,
) {
    for update in reader.read() {
        match &update.payload {
            PetUpdatePayload::Unsummoned => {
                info!("cos: uid {} unsummoned", update.unique_id);
                list.remove(update.unique_id);
                if rider.0 == Some(update.unique_id) {
                    rider.0 = None;
                }
                if let Some(cos_entity) = index.get(update.unique_id) {
                    // Anyone sitting on it gets off before the mount vanishes.
                    for (rider_entity, rider_of) in riders.iter() {
                        if rider_of.0 == cos_entity {
                            commands.entity(rider_entity).try_remove::<RiderOf>();
                        }
                    }
                    // Arm 1 covers two different endings, and they must not
                    // look the same. A COS that was **killed** dies like any
                    // other creature: `EntityDied` starts the die clip and the
                    // `Dying` corpse timer despawns it ~4s later. Despawning
                    // it here instead both cut the animation off and raced the
                    // combat commands still in flight for the killing blow —
                    // the server sends this arm in the same tick as the fatal
                    // 0xB071, so a queued `insert(Slain)` landed on a dead
                    // entity and panicked.
                    //
                    // A plain `/unsummon` (the pet was put away) has no death
                    // to show and still goes immediately.
                    if slain.contains(cos_entity) && !dying.contains(cos_entity) {
                        info!("cos: uid {} was slain — dying", update.unique_id);
                        died.write(EntityDied(cos_entity));
                    } else if !dying.contains(cos_entity) {
                        commands.entity(cos_entity).despawn();
                    }
                }
            }
            PetUpdatePayload::ModelChanged { new_ref_obj_id } => {
                let Some(status) = list.0.iter_mut().find(|c| c.unique_id == update.unique_id)
                else {
                    continue;
                };
                info!(
                    "cos: uid {} grew into ref {}",
                    update.unique_id, new_ref_obj_id
                );
                status.ref_id = *new_ref_obj_id;
                // A growth stage is its own characterdata row, so the new
                // maximum HP comes with it (360/441/524 at stages 1/2/3).
                if let Some(max_hp) = char_data
                    .get(&(*new_ref_obj_id as i32))
                    .and_then(|row| row.max_hp())
                {
                    status.hp_max = max_hp;
                    status.hp = status.hp.min(max_hp);
                }
                // The world entity is rebuilt by `respawn_grown_cos`, which
                // needs the asset server this system does not borrow.
            }
            PetUpdatePayload::Exp { delta, .. } => {
                debug!("cos: uid {} exp {:+}", update.unique_id, delta);
            }
            _ => {}
        }
    }
}

/// Apply 0x30C9 arm 5 — the pet was renamed.
///
/// The name reaches three places and until now reached only one: `CosState`
/// (`hud::cos::state::on_pet_update`) held it, while the summon list and the
/// entity's own nameplate kept whatever the *spawn* said. So naming a pet left
/// its plate stale until the next summon.
///
/// The rename is the only path that changes a name without respawning the
/// entity, which is why it needs its own system rather than riding along with
/// [`respawn_grown_cos`].
fn apply_cos_rename(
    mut reader: MessageReader<PetUpdate>,
    index: Res<NetworkEntities>,
    ui_strings: Option<Res<ClientUiStrings>>,
    mut list: ResMut<ActiveCosList>,
    mut names: Query<&mut DisplayName>,
) {
    for update in reader.read() {
        let PetUpdatePayload::Renamed { name } = &update.payload else {
            continue;
        };
        let given = Some(name.as_str()).filter(|name| !name.trim().is_empty());
        if let Some(status) = list
            .0
            .iter_mut()
            .find(|status| status.unique_id == update.unique_id)
        {
            status.name = given.map(str::to_string);
        }
        // Only the kinds that can be named get the "No name" treatment, so a
        // rename we cannot resolve a string table for leaves the plate alone
        // rather than blanking it.
        let Some(ui_strings) = ui_strings.as_deref() else {
            continue;
        };
        if let Some(mut display) = index
            .get(update.unique_id)
            .and_then(|entity| names.get_mut(entity).ok())
        {
            display.0 = spawn::pet_display_name(given, ui_strings);
        }
    }
}

/// Rebuild a COS's world entity after a growth stage (0x30C9 arm 7).
///
/// Idea: a growth stage is a *different characterdata row* — new mesh, new
/// skeleton, new animation set, new max HP — so mutating `CharacterRef` in
/// place would leave the loaded resource, the skinned mesh and the animation
/// library all describing the previous stage. Despawning and rebuilding
/// through the shared recipe is what guarantees the grown pet is assembled the
/// same way any other COS is. Position and movement carry over so the swap is
/// not a teleport.
///
/// Split from [`on_pet_update`] purely because it needs the asset server and
/// the name table, which that system does not borrow.
fn respawn_grown_cos(
    mut reader: MessageReader<PetUpdate>,
    asset_server: Res<AssetServer>,
    char_data: Res<ClientCharacterData>,
    names: Option<Res<ClientTextNames>>,
    index: Res<NetworkEntities>,
    list: Res<ActiveCosList>,
    ui_strings: Option<Res<ClientUiStrings>>,
    existing: Query<(
        &CosEntity,
        &Transform,
        &RemoteMovement,
        &MovementSpeed,
        Option<&CosOwner>,
    )>,
    riders: Query<(Entity, &RiderOf)>,
    mut commands: Commands,
) {
    for update in reader.read() {
        let PetUpdatePayload::ModelChanged { new_ref_obj_id } = &update.payload else {
            continue;
        };
        let Some(old_entity) = index.get(update.unique_id) else {
            continue;
        };
        let Ok((cos, transform, movement, speed, owner)) = existing.get(old_entity) else {
            continue;
        };
        let params = spawn::CosSpawnParams {
            ref_id: *new_ref_obj_id,
            unique_id: update.unique_id,
            kind: cos.kind,
            // The given name survives a growth stage — only the model changes.
            // Read from the status list and nowhere else: the old entity's
            // `DisplayName` used to be the fallback, but that is the *rendered*
            // label, so a species name or a localized "No name" would come back
            // in as the pet's identity.
            pet_name: list.get(update.unique_id).and_then(|s| s.name.clone()),
            owner_name: owner.map(|o| o.0.clone()),
            owner_uid: cos.owner_uid,
            transform: *transform,
            movement: movement.clone(),
            speed: *speed,
        };
        // Despawn first: `NetworkEntities` is keyed by unique id, and the new
        // entity claims the same one.
        commands.entity(old_entity).despawn();
        let Some(new_entity) = spawn_cos_entity(
            &mut commands,
            &asset_server,
            &char_data,
            names.as_deref(),
            ui_strings.as_deref(),
            params,
        ) else {
            warn!("cos: grown ref {new_ref_obj_id} has no characterdata row");
            continue;
        };
        // Whoever was riding the old body is riding the new one.
        for (rider_entity, rider_of) in riders.iter() {
            if rider_of.0 == old_entity {
                commands.entity(rider_entity).insert(RiderOf(new_entity));
            }
        }
    }
}

/// How many extra frames an ack whose *rider* has not spawned yet is kept
/// before it is discarded. The spawn is always in the same network read or the
/// one before it, so a couple of frames is generous; the cap is what stops a
/// uid that never spawns from being retried forever.
const MOUNT_ACK_RETRY_FRAMES: u8 = 4;

/// 0xB0CB — the server's mount/dismount verdict, for the local player and
/// (assumed broadcast, mount.md §9.2) remote riders alike.
///
/// Idea: neither uid in this packet is guaranteed to be resolvable *yet*, so
/// nothing here may treat "not in the index" as "does not exist". The entities
/// are created with `Commands` by the spawn consumers
/// (`game_scene::on_single_spawn`/`on_group_spawn`), and [`NetworkEntities`]
/// is fed by `NetworkId`'s insert hook — which runs when those commands are
/// *applied*. Since `receive_packets` drains a whole TCP read into one tick,
/// the summon burst (spawn … then this ack) routinely lands in the same tick,
/// and the spawn consumers are unordered `Update` systems in another plugin,
/// so no sync point separates them: within that tick the COS is invisible here
/// whichever system ran first. Dropping the ack therefore lost the mount
/// outright, and only *sometimes* — whenever the two packets happened to split
/// across reads it worked, which is why this survived a playtest.
///
/// So both misses defer instead of dropping: an unknown COS becomes a
/// [`PendingMount`] (the retry [`resolve_pending_mounts`] already runs for
/// spawn-record riders), and an unknown rider is held for a few frames in
/// `deferred`. Note this is deliberately *not* fixed with a system ordering:
/// `.after(on_single_spawn)` would force a sync point, but COS entities also
/// arrive via `on_group_spawn` and the character-data path, and
/// `on_single_spawn` is `GameWorld`-gated so it does not run in the sandbox
/// scenes at all. The retry covers every producer; an ordering edge would
/// cover one and need keeping in sync with the rest.
fn on_pet_mounted(
    mut reader: MessageReader<PetPlayerMounted>,
    index: Res<NetworkEntities>,
    players: Query<Has<Player>>,
    mut rider: ResMut<RiderState>,
    mut deferred: Local<Vec<(PetPlayerMounted, u8)>>,
    mut commands: Commands,
) {
    // Acks held from earlier frames go first, so a rider that has since
    // spawned is served in arrival order.
    let mut pending: Vec<(PetPlayerMounted, u8)> = std::mem::take(&mut deferred);
    pending.extend(reader.read().cloned().map(|msg| (msg, 0)));

    for (msg, age) in pending {
        if !msg.success {
            info!("cos: mount request rejected by server");
            continue;
        }
        let (Some(player_uid), Some(is_mounting)) = (msg.player_unique_id, msg.is_mounting) else {
            continue;
        };
        let Some(player_entity) = index.get(player_uid) else {
            if age < MOUNT_ACK_RETRY_FRAMES {
                debug!("cos: 0xB0CB rider uid {player_uid} not spawned yet, retrying");
                deferred.push((msg, age + 1));
            } else {
                warn!("cos: 0xB0CB for unknown player uid {player_uid}, giving up");
            }
            continue;
        };
        let is_local = players.get(player_entity).unwrap_or(false);
        if is_mounting {
            let Some(cos_uid) = msg.riding_unique_id else {
                continue;
            };
            let Some(cos_entity) = index.get(cos_uid) else {
                // The rider exists, the mount does not — hand it to the
                // component-driven retry rather than holding it here, so it
                // survives however long the COS takes to spawn. `RiderState`
                // is set by the resolver, not here: until the mount is really
                // applied the player is still on foot.
                debug!("cos: 0xB0CB mount onto cos uid {cos_uid}, awaiting its spawn");
                commands.entity(player_entity).insert(PendingMount(cos_uid));
                continue;
            };
            info!(
                "cos: player {} mounted cos {}{}",
                player_uid,
                cos_uid,
                if is_local { " (local)" } else { "" }
            );
            commands.entity(player_entity).insert(RiderOf(cos_entity));
            if is_local {
                rider.0 = Some(cos_uid);
            }
        } else {
            info!("cos: player {} dismounted", player_uid);
            commands
                .entity(player_entity)
                .try_remove::<RiderOf>()
                // A dismount that overtakes an unresolved mount must cancel
                // it, or the resolver would seat the player again afterwards.
                .try_remove::<PendingMount>();
            if is_local {
                rider.0 = None;
            }
        }
    }
}

/// Resolve [`PendingMount`] (spawn-record riders, relog-while-mounted) to
/// [`RiderOf`] once the COS uid exists in the index; retries each frame until
/// the mount has spawned.
fn resolve_pending_mounts(
    pending: Query<(Entity, &PendingMount)>,
    players: Query<Has<Player>>,
    index: Res<NetworkEntities>,
    mut rider: ResMut<RiderState>,
    mut commands: Commands,
) {
    for (entity, mount) in pending.iter() {
        let Some(cos_entity) = index.get(mount.0) else {
            continue;
        };
        commands
            .entity(entity)
            .remove::<PendingMount>()
            .insert(RiderOf(cos_entity));
        if players.get(entity).unwrap_or(false) {
            rider.0 = Some(mount.0);
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::plugins::net::character_info::MovementSpeed;
    use crate::plugins::net::entities::{NetworkId, RemoteMovement};
    use crate::plugins::player::{PlayerCommands, PlayerMoveOrder};
    use crate::plugins::world_origin::WorldOrigin;
    use packets::agent::pet::{CosKind, PET_UPDATE_UNSUMMONED};

    const LOCAL_UID: u32 = 0x8000_0001;
    const PLAYER_UID: u32 = 42;

    /// The defect: a COS spawns as `RemoteEntity::Npc`, so every site that read
    /// that alone gave a pet the talk cursor, hover without Shift, and a
    /// shopkeeper dialog on click. One rule, asked at four call sites.
    #[test]
    fn a_cos_takes_the_character_path_and_a_real_npc_does_not() {
        // the whole point: same kind, opposite answers
        assert!(interacts_as_character(&RemoteEntity::Npc, true));
        assert!(!interacts_as_character(&RemoteEntity::Npc, false));

        // players are characters with or without the flag
        assert!(interacts_as_character(&RemoteEntity::Player, false));
        assert!(interacts_as_character(&RemoteEntity::Player, true));

        // monsters and ground items keep the NPC-side path
        assert!(!interacts_as_character(&RemoteEntity::Monster, false));
        assert!(!interacts_as_character(&RemoteEntity::Item, false));

        // A COS of ANY kind counts — the old carve-out covered only the mount
        // you were riding, which is what left every other pet talking.
        assert!(interacts_as_character(&RemoteEntity::Monster, true));
    }

    /// Just the two ack-consuming systems and an empty index — the state the
    /// client is really in when a summon burst arrives, with nothing spawned
    /// yet. Deliberately **not** `.chain()`ed: the bug being pinned is what
    /// happens with no ordering edge (and therefore no sync point) between the
    /// spawn and the ack, which is exactly how the real schedule is built.
    fn ack_app() -> App {
        let mut app = App::new();
        app.init_resource::<NetworkEntities>()
            .init_resource::<RiderState>()
            .add_message::<PetPlayerMounted>()
            .add_systems(Update, (on_pet_mounted, resolve_pending_mounts));
        app
    }

    fn mount_ack(player_uid: u32, cos_uid: u32) -> PetPlayerMounted {
        PetPlayerMounted {
            success: true,
            player_unique_id: Some(player_uid),
            is_mounting: Some(true),
            riding_unique_id: Some(cos_uid),
            error_code: None,
        }
    }

    /// The regression from the live capture (`packet_dump/0xb0cb.log`,
    /// 2026-08-16T22:02:47): the server acked the mount in the same tick the
    /// horse spawned, the COS was not in the index yet, and the ack was
    /// dropped — leaving the horse standing beside an unmounted player.
    #[test]
    fn a_mount_ack_for_a_not_yet_spawned_cos_still_mounts() {
        let mut app = ack_app();
        let player = app
            .world_mut()
            .spawn((Player::new(), NetworkId(PLAYER_UID)))
            .id();

        app.world_mut()
            .write_message(mount_ack(PLAYER_UID, LOCAL_UID));
        app.update();

        assert_eq!(
            app.world().get::<PendingMount>(player).map(|p| p.0),
            Some(LOCAL_UID),
            "an unresolvable mount must be deferred, not discarded"
        );
        assert!(app.world().get::<RiderOf>(player).is_none());
        assert_eq!(
            app.world().resource::<RiderState>().0,
            None,
            "the player is still on foot until the mount really resolves"
        );

        let cos = app.world_mut().spawn(NetworkId(LOCAL_UID)).id();
        app.update();

        assert_eq!(app.world().get::<RiderOf>(player).map(|r| r.0), Some(cos));
        assert_eq!(app.world().resource::<RiderState>().0, Some(LOCAL_UID));
        assert!(app.world().get::<PendingMount>(player).is_none());
    }

    /// The true shape of the bug: the COS is spawned through `Commands` in the
    /// *same* tick the ack is read, so its `NetworkId` hook has not run and the
    /// index cannot know it — whichever of the two systems Bevy runs first.
    #[test]
    fn a_mount_ack_in_the_same_tick_as_the_spawn_mounts() {
        #[derive(Resource, Default)]
        struct SpawnCosNow(bool);

        fn spawn_cos_when_asked(mut flag: ResMut<SpawnCosNow>, mut commands: Commands) {
            if !flag.0 {
                return;
            }
            flag.0 = false;
            commands.spawn(NetworkId(LOCAL_UID));
        }

        let mut app = ack_app();
        app.init_resource::<SpawnCosNow>()
            .add_systems(Update, spawn_cos_when_asked);
        let player = app
            .world_mut()
            .spawn((Player::new(), NetworkId(PLAYER_UID)))
            .id();

        app.world_mut().resource_mut::<SpawnCosNow>().0 = true;
        app.world_mut()
            .write_message(mount_ack(PLAYER_UID, LOCAL_UID));
        app.update();
        app.update();

        assert!(
            app.world().get::<RiderOf>(player).is_some(),
            "a mount acked in the spawn's own tick must still seat the player"
        );
        assert_eq!(app.world().resource::<RiderState>().0, Some(LOCAL_UID));
    }

    /// The mirror race: a remote player who walks into view already mounted can
    /// have their own spawn share a tick with the broadcast ack, so an
    /// unresolvable *rider* has to be retried too.
    #[test]
    fn a_mount_ack_whose_rider_spawns_late_is_retried() {
        let mut app = ack_app();
        app.world_mut()
            .write_message(mount_ack(PLAYER_UID, LOCAL_UID));
        app.update();

        let cos = app.world_mut().spawn(NetworkId(LOCAL_UID)).id();
        let rider = app.world_mut().spawn(NetworkId(PLAYER_UID)).id();
        app.update();

        assert_eq!(
            app.world().get::<RiderOf>(rider).map(|r| r.0),
            Some(cos),
            "the ack must survive until its rider exists"
        );
        assert_eq!(
            app.world().resource::<RiderState>().0,
            None,
            "a remote rider must not touch the local RiderState"
        );
    }

    /// ...but not forever: the retry is capped, so an ack naming a uid that
    /// never spawns is eventually dropped instead of being carried for the
    /// lifetime of the session.
    #[test]
    fn a_mount_ack_for_a_rider_that_never_spawns_is_dropped() {
        let mut app = ack_app();
        app.world_mut()
            .write_message(mount_ack(PLAYER_UID, LOCAL_UID));
        for _ in 0..MOUNT_ACK_RETRY_FRAMES + 2 {
            app.update();
        }

        app.world_mut().spawn(NetworkId(LOCAL_UID));
        let late = app.world_mut().spawn(NetworkId(PLAYER_UID)).id();
        app.update();

        assert!(
            app.world().get::<RiderOf>(late).is_none(),
            "a long-abandoned ack must not seat a player that shows up later"
        );
    }

    /// A dismount that overtakes a still-unresolved mount has to cancel it,
    /// or the pending retry would seat the player again afterwards.
    #[test]
    fn a_dismount_cancels_an_unresolved_mount() {
        let mut app = ack_app();
        let player = app
            .world_mut()
            .spawn((Player::new(), NetworkId(PLAYER_UID)))
            .id();

        app.world_mut()
            .write_message(mount_ack(PLAYER_UID, LOCAL_UID));
        app.update();
        assert!(app.world().get::<PendingMount>(player).is_some());

        app.world_mut().write_message(PetPlayerMounted {
            success: true,
            player_unique_id: Some(PLAYER_UID),
            is_mounting: Some(false),
            riding_unique_id: Some(LOCAL_UID),
            error_code: None,
        });
        app.update();

        // The mount finally spawns — too late, the player already got off.
        app.world_mut().spawn(NetworkId(LOCAL_UID));
        app.update();

        assert!(app.world().get::<PendingMount>(player).is_none());
        assert!(app.world().get::<RiderOf>(player).is_none());
        assert_eq!(app.world().resource::<RiderState>().0, None);
    }

    /// A minimal world with the riding systems and one local-only mount +
    /// player; no scene states, no connection (the offline path).
    fn riding_app() -> (App, Entity, Entity) {
        let mut app = App::new();
        app.init_resource::<NetworkEntities>()
            .init_resource::<RiderState>()
            .init_resource::<PlayerCommands>()
            .init_resource::<WorldOrigin>()
            .init_resource::<ActiveCosList>()
            .add_message::<CosCommand>()
            .add_message::<PlayerMoveOrder>()
            .add_message::<PetPlayerMounted>()
            .add_systems(
                Update,
                (
                    on_pet_mounted,
                    resolve_pending_mounts,
                    riding::handle_cos_commands,
                    riding::route_move_orders_while_mounted,
                    riding::slave_riders_to_cos,
                )
                    .chain(),
            );
        app.world_mut()
            .resource_mut::<ActiveCosList>()
            .0
            .push(CosStatus {
                unique_id: LOCAL_UID,
                ref_id: 2137,
                kind: CosKind::Vehicle,
                name: Some("Horse".into()),
                hp: 100,
                hp_max: 100,
                local_only: true,
            });
        let cos = app
            .world_mut()
            .spawn((
                CosEntity {
                    kind: CosKind::Vehicle,
                    owner_uid: None,
                },
                NetworkId(LOCAL_UID),
                Transform::from_xyz(10.0, 0.0, 5.0),
                RemoteMovement {
                    target: None,
                    speed: 90.0,
                    walking: false,
                },
                MovementSpeed {
                    walk: 45.0,
                    run: 90.0,
                    hwan: 90.0,
                },
                riding::SaddleSeat::Height(12.0),
            ))
            .id();
        let player = app
            .world_mut()
            .spawn((Player::new(), Transform::from_xyz(0.0, 0.0, 0.0)))
            .id();
        (app, cos, player)
    }

    #[test]
    fn offline_board_slaves_and_dismount_releases() {
        let (mut app, cos, player) = riding_app();

        app.world_mut().write_message(CosCommand::Board(LOCAL_UID));
        app.update();
        assert_eq!(
            app.world().get::<RiderOf>(player).map(|r| r.0),
            Some(cos),
            "boarding a local mount applies directly"
        );
        assert_eq!(app.world().resource::<RiderState>().0, Some(LOCAL_UID));

        // The rider is slaved onto the mount + saddle height.
        app.update();
        let rider_pos = app.world().get::<Transform>(player).unwrap().translation;
        assert_eq!(rider_pos, Vec3::new(10.0, 12.0, 5.0));

        app.world_mut().write_message(CosCommand::Dismount);
        app.update();
        assert!(app.world().get::<RiderOf>(player).is_none());
        assert_eq!(app.world().resource::<RiderState>().0, None);
        // Stepped off sideways, not standing inside the mount.
        let after = app.world().get::<Transform>(player).unwrap().translation;
        assert_ne!(after.xz(), Vec3::new(10.0, 12.0, 5.0).xz());
        // The hop off the saddle invalidates the tracked surface (ADR-0007).
        assert_eq!(
            app.world()
                .get::<crate::plugins::nav::NavLocation>(player)
                .copied(),
            Some(crate::plugins::nav::NavLocation::Unresolved),
        );
    }

    /// The seat is in the mount's LOCAL space: turn the mount and the rider
    /// swings around with it. A world-space offset (what we shipped first)
    /// would leave the rider hanging off the saddle as soon as the mount
    /// tilts on a slope.
    #[test]
    fn the_seat_follows_the_mounts_rotation() {
        let (mut app, cos, player) = riding_app();
        app.world_mut().write_message(CosCommand::Board(LOCAL_UID));
        app.update();

        // Pitch the mount nose-down 90°: its local +Y now points along -Z, so
        // a 12-unit seat lands 12 units behind the mount rather than above it.
        app.world_mut().entity_mut(cos).insert(Transform {
            translation: Vec3::new(10.0, 0.0, 5.0),
            rotation: Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2),
            scale: Vec3::ONE,
        });
        app.update();

        let rider = app.world().get::<Transform>(player).unwrap().translation;
        assert!(
            (rider - Vec3::new(10.0, 0.0, -7.0)).length() < 1e-3,
            "seat must be rotated by the mount (got {rider:?})"
        );
    }

    #[test]
    fn move_orders_drive_the_local_mount() {
        let (mut app, cos, _player) = riding_app();
        app.world_mut().write_message(CosCommand::Board(LOCAL_UID));
        app.update();

        app.world_mut()
            .write_message(PlayerMoveOrder(Vec3::new(100.0, 0.0, 50.0)));
        app.update();
        let movement = app.world().get::<RemoteMovement>(cos).unwrap();
        assert_eq!(
            movement.target,
            Some(Vec3::new(100.0, 0.0, 50.0)),
            "the click walks the mount, not the rider"
        );
        assert_eq!(movement.speed, 90.0, "the mount uses its own run speed");
    }

    #[test]
    fn pet_mounted_ack_applies_mount_state() {
        let (mut app, cos, player) = riding_app();
        // Give the player a uid so 0xB0CB can address it.
        app.world_mut().entity_mut(player).insert(NetworkId(42));

        app.world_mut().write_message(PetPlayerMounted {
            success: true,
            player_unique_id: Some(42),
            is_mounting: Some(true),
            riding_unique_id: Some(LOCAL_UID),
            error_code: None,
        });
        app.update();
        assert_eq!(app.world().get::<RiderOf>(player).map(|r| r.0), Some(cos));
        assert_eq!(app.world().resource::<RiderState>().0, Some(LOCAL_UID));

        // A dismount carries the transport uid too — it is not `None` here,
        // which is what the old 6-of-10-bytes read made it look like.
        app.world_mut().write_message(PetPlayerMounted {
            success: true,
            player_unique_id: Some(42),
            is_mounting: Some(false),
            riding_unique_id: Some(LOCAL_UID),
            error_code: None,
        });
        app.update();
        assert!(app.world().get::<RiderOf>(player).is_none());
        assert_eq!(app.world().resource::<RiderState>().0, None);
    }

    /// A rider's held items vanish while mounted and come back on dismount.
    /// Armor must NOT be touched — it hangs off the body wrapper, not a bone.
    #[test]
    fn mounting_hides_held_items_only() {
        let (mut app, cos, player) = riding_app();
        app.add_systems(Update, riding::hide_held_items_while_mounted);

        // wrapper -> hand bone -> weapon, and wrapper -> armor
        let wrapper = app.world_mut().spawn(Visibility::default()).id();
        app.world_mut().entity_mut(player).add_child(wrapper);
        let hand = app
            .world_mut()
            .spawn((
                crate::commands::Bone,
                Name::from("Bip01 R HandMid"),
                Visibility::default(),
            ))
            .id();
        app.world_mut().entity_mut(wrapper).add_child(hand);
        let weapon = app
            .world_mut()
            .spawn((
                crate::commands::SpawnedFromResource(Handle::default()),
                Visibility::Inherited,
            ))
            .id();
        app.world_mut().entity_mut(hand).add_child(weapon);
        let armor = app
            .world_mut()
            .spawn((
                crate::commands::SpawnedFromResource(Handle::default()),
                Visibility::Inherited,
            ))
            .id();
        app.world_mut().entity_mut(wrapper).add_child(armor);

        app.world_mut().write_message(CosCommand::Board(LOCAL_UID));
        app.update(); // boards (RiderOf is a deferred insert)
        app.update(); // the polling system sees it
        assert_eq!(
            app.world().get::<Visibility>(weapon).copied(),
            Some(Visibility::Hidden),
            "the weapon must be hidden in the saddle"
        );
        assert_eq!(
            app.world().get::<Visibility>(armor).copied(),
            Some(Visibility::Inherited),
            "armor is not a held item"
        );

        app.world_mut().write_message(CosCommand::Dismount);
        app.update();
        app.update();
        assert_eq!(
            app.world().get::<Visibility>(weapon).copied(),
            Some(Visibility::Inherited),
            "dismounting restores the weapon"
        );
        let _ = cos;
    }

    #[test]
    fn unsummon_despawns_and_force_dismounts() {
        let (mut app, cos, player) = riding_app();
        app.world_mut().write_message(CosCommand::Board(LOCAL_UID));
        app.update();

        app.world_mut()
            .write_message(CosCommand::Unsummon(LOCAL_UID));
        app.update();
        assert!(app.world().get::<RiderOf>(player).is_none());
        assert_eq!(app.world().resource::<RiderState>().0, None);
        assert!(app.world().get_entity(cos).is_err(), "mount despawned");
        assert!(app.world().resource::<ActiveCosList>().0.is_empty());
    }

    /// A harness for the 0x30C9 arm-1 path — the packet, not the local
    /// `CosCommand`. `on_pet_update` needs characterdata for the growth arm,
    /// which an unsummon never reaches, so an empty table is enough.
    fn unsummon_app() -> App {
        let mut app = App::new();
        app.init_resource::<NetworkEntities>()
            .init_resource::<RiderState>()
            .init_resource::<ActiveCosList>()
            .insert_resource(ClientCharacterData::default())
            .add_message::<PetUpdate>()
            .add_message::<EntityDied>()
            .add_systems(Update, on_pet_update);
        app.world_mut()
            .resource_mut::<ActiveCosList>()
            .0
            .push(CosStatus {
                unique_id: LOCAL_UID,
                ref_id: 4242,
                kind: CosKind::GrowthPet,
                name: Some("Nuri".into()),
                hp: 0,
                hp_max: 360,
                local_only: false,
            });
        app
    }

    /// Naming a pet has to reach its **plate**, not just `CosState`.
    ///
    /// Arm 5 used to be handled in exactly one place — `hud::cos::state` set
    /// `body.name` and nothing else — so the summon list and the entity's
    /// `DisplayName` kept whatever the spawn record said until the next summon.
    #[test]
    fn a_rename_reaches_the_summon_list_and_the_nameplate() {
        let mut app = App::new();
        app.init_resource::<NetworkEntities>()
            .init_resource::<ActiveCosList>()
            .init_resource::<ClientUiStrings>()
            .add_message::<PetUpdate>()
            .add_systems(Update, apply_cos_rename);
        app.world_mut()
            .resource_mut::<ActiveCosList>()
            .0
            .push(CosStatus {
                unique_id: LOCAL_UID,
                ref_id: 6106,
                kind: CosKind::GrowthPet,
                name: None,
                hp: 360,
                hp_max: 360,
                local_only: false,
            });
        // `NetworkId`'s component hooks index it — no manual registration.
        let pet = app
            .world_mut()
            .spawn((NetworkId(LOCAL_UID), DisplayName("No name".into())))
            .id();

        let rename = |app: &mut App, name: &str| {
            app.world_mut().write_message(PetUpdate {
                unique_id: LOCAL_UID,
                update_type: 5,
                payload: PetUpdatePayload::Renamed {
                    name: name.to_string(),
                },
            });
            app.update();
        };

        rename(&mut app, "Rex");
        assert_eq!(
            app.world().resource::<ActiveCosList>().0[0].name.as_deref(),
            Some("Rex")
        );
        assert_eq!(app.world().get::<DisplayName>(pet).unwrap().0, "Rex");

        // ...and clearing it puts the placeholder back rather than a blank.
        rename(&mut app, "");
        assert_eq!(app.world().resource::<ActiveCosList>().0[0].name, None);
        assert_eq!(app.world().get::<DisplayName>(pet).unwrap().0, "No name");
    }

    fn unsummon(app: &mut App) {
        app.world_mut().write_message(PetUpdate {
            unique_id: LOCAL_UID,
            update_type: PET_UPDATE_UNSUMMONED,
            payload: PetUpdatePayload::Unsummoned,
        });
        app.update();
    }

    /// The crash regression (2026-08-18): a pet that **died** must not be
    /// despawned by the unsummon arm. The server sends it in the same tick as
    /// the fatal 0xB071, so despawning here landed the combat layer's queued
    /// `insert(Slain)` on a dead entity and panicked — besides cutting the
    /// death animation off. A slain COS goes down the ordinary corpse path.
    #[test]
    fn a_slain_cos_dies_instead_of_being_despawned() {
        let mut app = unsummon_app();
        let pet = app.world_mut().spawn((NetworkId(LOCAL_UID), Slain)).id();
        unsummon(&mut app);

        assert!(
            app.world().get_entity(pet).is_ok(),
            "a slain pet must survive the unsummon for its death animation"
        );
        let died: Vec<Entity> = app
            .world_mut()
            .resource_mut::<Messages<EntityDied>>()
            .drain()
            .map(|EntityDied(entity)| entity)
            .collect();
        assert_eq!(died, vec![pet], "it goes down the normal death path");
        // The status entry drops immediately either way: a corpse offers no
        // commands, even while it is still on screen.
        assert!(app.world().resource::<ActiveCosList>().0.is_empty());
    }

    /// The other half of arm 1: a pet that was merely *put away* has no death
    /// to show, so it still goes at once.
    #[test]
    fn an_unslain_cos_is_despawned_at_once() {
        let mut app = unsummon_app();
        let pet = app.world_mut().spawn(NetworkId(LOCAL_UID)).id();
        unsummon(&mut app);

        assert!(app.world().get_entity(pet).is_err(), "put away, so gone");
        assert!(app
            .world_mut()
            .resource_mut::<Messages<EntityDied>>()
            .drain()
            .next()
            .is_none());
        assert!(app.world().resource::<ActiveCosList>().0.is_empty());
    }

    /// A COS already dying (its corpse timer running) must not be despawned or
    /// re-killed by a late arm 1 — that would be the same race in reverse.
    #[test]
    fn an_already_dying_cos_is_left_alone() {
        let mut app = unsummon_app();
        let pet = app
            .world_mut()
            .spawn((NetworkId(LOCAL_UID), Slain, Dying::lingering()))
            .id();
        unsummon(&mut app);

        assert!(app.world().get_entity(pet).is_ok(), "the corpse survives");
        assert!(
            app.world_mut()
                .resource_mut::<Messages<EntityDied>>()
                .drain()
                .next()
                .is_none(),
            "and is not killed twice"
        );
    }
}
