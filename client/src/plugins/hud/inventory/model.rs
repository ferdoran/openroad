//! Inventory window state and its data feeds.
//!
//! Idea: the inventory itself lives as an [`Inventory`] component on the
//! player entity (one `Option<InventoryItem>` per wire slot), inserted by the
//! game scene when CHARACTER_DATA (0x3013) resolves. The systems here keep it
//! live — gold points updates (0x304E) and server-confirmed inventory moves
//! (0xB034) mutate the component (never optimistically, so the UI cannot
//! drift from the authoritative inventory) — and hold the pure window state
//! (open, page, hover, drag). Equip/unequip pushes (0x3038/0x3039) drive the
//! 3d model attachment only; slot bookkeeping always comes from the 0xB034
//! move that caused them.

use bevy::prelude::*;

use packets::agent::prelude::{
    CharacterPointsUpdate, EntityEquip, EntityUnequip, InventoryCapacityUpdate,
    InventoryItemDurabilityUpdate, InventoryItemUpdate, InventoryOperationResponse,
    InventoryOperationResult, ItemUseResponse,
};

use crate::assets::bsr::resource::SroResource;
use crate::commands::SkeletonBinding;
use crate::plugins::dynamic_resource_loader::{
    AttachmentRareAura, AttachmentShine, PendingItemAttachment, PendingRareAura,
    PendingWeaponShine, PreferredAnimationGroup,
};
use crate::plugins::hud::chat::model::ChatState;
use crate::plugins::net::entities::{NetworkId, RemoteEntity};
use crate::plugins::net::inventory::{Inventory, AMMO_SLOT};
use crate::plugins::player::Player;
use crate::plugins::settings::keymap::KEY_INVENTORY;
use crate::plugins::settings::options::GameOptions;
use crate::plugins::textdata::{ClientItemData, ClientRareEffects};
use crate::util::commands_ext::CommandsExt;

#[derive(Resource, Default)]
pub struct InventoryState {
    pub open: bool,
    /// 0-based bag page shown in the grid.
    pub active_page: u8,
    /// Wire slot currently hovered (tooltip anchor), equipment or bag.
    pub hovered_slot: Option<u8>,
    /// Wire slot an in-progress drag started from (ghost icon in ui.rs).
    pub drag: Option<u8>,
    /// Equipment panel shows the avatar slots instead of the gear slots
    /// (vanilla's GDR_EQ_BTN_CHANGE_AVATAR/EQUIP_SLOT toggle).
    pub avatar_view: bool,
}

/// The `KeyInventory` shortcut toggles the window — unless the chat input is
/// capturing keystrokes. Rebindable via the Key Map tab; `I` by default.
pub fn toggle_inventory(
    keys: Res<ButtonInput<KeyCode>>,
    chat: Res<ChatState>,
    options: Res<GameOptions>,
    mut state: ResMut<InventoryState>,
) {
    let Some(key) = options.key_for(KEY_INVENTORY) else {
        return;
    };
    if keys.just_pressed(key) && !chat.input_open {
        state.open = !state.open;
    }
}

/// Apply the server's inventory delta pushes (0x3052 durability, 0x3040
/// slot update, 0x3092 bag capacity).
///
/// These are the server-initiated half of inventory bookkeeping: until now the
/// [`Inventory`] only ever moved on a 0xB034 ack, so wear, consumption, pet
/// state changes and bag purchases were dropped on the floor. Change detection
/// on the component drives the window/tooltip refresh, so applying them here is
/// all the UI needs.
///
/// Unknown `0x3040` update types are logged and skipped, matching the original
/// parser's `default`-less switch — the wire decode already tolerates them.
pub fn on_inventory_deltas(
    mut durability: MessageReader<InventoryItemDurabilityUpdate>,
    mut items: MessageReader<InventoryItemUpdate>,
    mut capacity: MessageReader<InventoryCapacityUpdate>,
    mut inventories: Query<&mut Inventory, With<Player>>,
) {
    let Ok(mut inventory) = inventories.single_mut() else {
        return;
    };
    for msg in durability.read() {
        debug!(
            "inventory: slot {} durability -> {}",
            msg.slot, msg.durability
        );
        inventory.set_durability(msg.slot, msg.durability);
    }
    for msg in items.read() {
        match (msg.update_type, msg.quantity, msg.cos_state) {
            (_, Some(quantity), _) => {
                debug!("inventory: slot {} quantity -> {}", msg.slot, quantity);
                inventory.set_stack_count(msg.slot, quantity);
            }
            (_, _, Some(state)) => {
                debug!("inventory: slot {} cos state -> {}", msg.slot, state);
                inventory.set_cos_state(msg.slot, state);
            }
            // Byte 1 is a BITMASK of eight blocks rather than an enum value
            // (docs/protocol/opcodes.md), and the wire struct models two of
            // them — so a push combining quantity with any other block decodes
            // to no fields at all and lands here.
            //
            // `warn!`, not `debug!`: this line is the tripwire for the six
            // blocks we have never seen a sample of. Ammunition consumption is
            // suspected to ride one of them (no 0x3040 appears in any capture
            // to date), and at debug level that evidence reaches nobody.
            (update_type, None, None) => warn!(
                "inventory: unhandled 0x3040 update mask {:#04x} on slot {} — \
                 capture for decode",
                update_type, msg.slot
            ),
        }
    }
    for msg in capacity.read() {
        if let Some(new_capacity) = msg.new_capacity {
            debug!("inventory: bag capacity -> {}", new_capacity);
            inventory.set_capacity(new_capacity);
        }
    }
}

/// Predict ammunition spend: one arrow per basic bow/crossbow swing.
///
/// **`[S]`, and predicted rather than authoritative** — this server never
/// reports ammo consumption at all. Capture-verified across one session: 500
/// arrows at the 20:49 login and 402 at the 21:04 login, with no `0x3040`
/// whatsoever, no quantity in any `0xB034` move ack (the unequip ack carries
/// `0`, and the equip ack merely echoes what the client asked for), and no
/// `0x3052` naming the ammo slot. The true total arrives only in the next
/// `0x3013`, so with no prediction the count sits visibly stale for a whole
/// session — the reported defect.
///
/// A fresh `0x3013` snapshot overwrites this, which is the intended correction
/// path, and the error is bounded by one session's shooting. It can only ever
/// leave the count too HIGH: a swing we fail to observe under-counts, where
/// inventing spend would show arrows the character still has.
///
/// One arrow per damage instance, so a plain shot spends one while a 2- or
/// 3-arrow combo spends two or three — [`LocalAttackLanded`] carries the count
/// straight off the server's 0xB070.
pub fn predict_ammo_consumption(
    mut landed: MessageReader<crate::plugins::combat::LocalAttackLanded>,
    weapon: Res<crate::plugins::skills::EquippedWeapon>,
    mut inventories: Query<&mut Inventory, With<Player>>,
) {
    let Ok(mut inventory) = inventories.single_mut() else {
        landed.clear();
        return;
    };
    // Only a drawn bow or crossbow spends ammo (itemdata TID4 6 / 12); a blade
    // swings for free, which is why the count correctly never moved for one.
    if !matches!(weapon.class, Some(6) | Some(12)) {
        landed.clear();
        return;
    }
    let spent: u16 = landed.read().map(|hit| hit.0).sum();
    if spent == 0 {
        return;
    }
    let Some(item) = inventory.get(AMMO_SLOT) else {
        return;
    };
    // Only a stack is spendable. The shield shares this hole and decodes as
    // Equipment, so it must be left alone.
    let packets::agent::character_data::ItemTypeData::Expendable { stack_count, .. } = &item.data
    else {
        return;
    };
    let remaining = stack_count.saturating_sub(spent);
    debug!("inventory: predicted {spent} arrow(s) spent, {remaining} left in slot {AMMO_SLOT}");
    inventory.set_stack_count(AMMO_SLOT, remaining);
}

/// Apply 0x304E gold updates (pickups, spending, drops).
pub fn on_gold_update(
    mut reader: MessageReader<CharacterPointsUpdate>,
    mut inventories: Query<&mut Inventory, With<Player>>,
) {
    for msg in reader.read() {
        if let CharacterPointsUpdate::Gold { amount, .. } = msg {
            for mut inventory in inventories.iter_mut() {
                if inventory.gold != *amount {
                    inventory.gold = *amount;
                }
            }
        }
    }
}

/// Apply the 0xB04C item-use ack's `remaining` to the consumed slot.
///
/// **The stack count is only on this packet.** The client used to assume the
/// decrement "rides the ack's own inventory update", i.e. a follow-up 0x3040 —
/// it does not. All 17 `0x3040` lines in the capture are COS-state pushes
/// (mask `0x40`); not one carries a quantity, and none follows an item use. So
/// a potion was consumed server-side while the bag kept showing the old count
/// until the next full 0x3013.
///
/// One HP-potion stack in slot `0x42`, straight from `packet_dump/0xb04c.log`:
///
/// ```text
/// 01 42 0600 ec08   remaining 6
/// 01 42 0500 ec08   remaining 5
/// 01 42 0400 ec08   remaining 4
/// ```
///
/// `remaining` is the server's own absolute count, so this is authoritative
/// rather than an optimistic guess. A separate reader from the two systems in
/// `hud::underbar::cast` that also watch this message — Bevy fans a message out
/// to every reader, so no ordering between them is needed, and inventory
/// mutation belongs here beside the other appliers.
pub fn on_item_consumed(
    mut reader: MessageReader<ItemUseResponse>,
    mut inventories: Query<&mut Inventory, With<Player>>,
) {
    for msg in reader.read() {
        let ItemUseResponse::Success {
            slot, remaining, ..
        } = msg
        else {
            continue;
        };
        for mut inventory in inventories.iter_mut() {
            if *remaining == 0 {
                debug!("inventory: slot {slot} emptied by use");
                inventory.take_slot(*slot);
            } else {
                debug!("inventory: slot {slot} -> {remaining} after use");
                inventory.set_stack_count(*slot, *remaining);
            }
        }
    }
}

/// Apply 0xB034: mirror a confirmed move into the player's [`Inventory`] and
/// end the drag either way (the ghost despawn keys off `drag` clearing).
pub fn on_inventory_operation_response(
    mut reader: MessageReader<InventoryOperationResponse>,
    mut inventories: Query<&mut Inventory, With<Player>>,
    item_data: Res<ClientItemData>,
    mut state: ResMut<InventoryState>,
) {
    for msg in reader.read() {
        match (&msg.operation, msg.error) {
            (
                Some(InventoryOperationResult::Move {
                    source,
                    target,
                    amount,
                    chained,
                }),
                _,
            ) => {
                debug!("inventory: move {} -> {} (x{})", source, target, amount);
                for mut inventory in inventories.iter_mut() {
                    inventory.apply_move(*source, *target, *amount, &item_data);
                    // The server's own follow-up moves ride in the same body:
                    // equipping a two-handed weapon takes the shield out of
                    // slot 7, and a one-handed weapon puts it back. Ignoring
                    // these left the shield drawn in a slot the server had
                    // already emptied, and every later move of it answered
                    // `0x1809` "item gone".
                    for record in chained {
                        if record.op != 0 {
                            warn!(
                                "inventory: 0xB034 chained record has op {:#04x}, not a move — \
                                 capture for decode",
                                record.op
                            );
                            continue;
                        }
                        info!(
                            "inventory: server also moved {} -> {} (chained)",
                            record.source, record.target
                        );
                        inventory.apply_move(
                            record.source,
                            record.target,
                            record.amount,
                            &item_data,
                        );
                    }
                }
            }
            (Some(InventoryOperationResult::Drop { slot }), _) => {
                debug!("inventory: dropped the item in slot {slot}");
                for mut inventory in inventories.iter_mut() {
                    inventory.take_slot(*slot);
                }
            }
            (Some(op @ InventoryOperationResult::Pickup { .. }), _) => {
                if let Some(amount) = op.pickup_gold() {
                    // Gold is NOT applied here: the server sends the
                    // authoritative running total in a 0x304E right before
                    // this packet (verified 1:1 in captures), handled by
                    // `on_gold_update`. Adding `amount` on top double-counts,
                    // which reads as the pickup granting less than it should
                    // (each 0x304E then corrects the inflated value back
                    // down). This packet is log-only for gold.
                    debug!("inventory: picked up {} gold (total via 0x304E)", amount);
                } else if let Some(item) = op.pickup_item(&*item_data) {
                    debug!(
                        "inventory: picked up item ref {} into slot {}",
                        item.ref_id, item.slot
                    );
                    for mut inventory in inventories.iter_mut() {
                        inventory.gain_item(item.clone());
                    }
                } else {
                    warn!("inventory: pickup payload not understood — capture 0xB034 for decode");
                }
            }
            // store buys/sells/buybacks are applied by
            // `hud::store::model::on_store_response`
            (
                Some(
                    InventoryOperationResult::Buy { .. }
                    | InventoryOperationResult::Sell { .. }
                    | InventoryOperationResult::BuyBack { .. },
                ),
                _,
            ) => {}
            // storage ops are applied by `hud::storage::model::on_storage_response`
            (
                Some(
                    InventoryOperationResult::StorageToStorage { .. }
                    | InventoryOperationResult::InventoryToStorage { .. }
                    | InventoryOperationResult::StorageToInventory { .. }
                    | InventoryOperationResult::StorageGoldToInventory { .. }
                    | InventoryOperationResult::InventoryGoldToStorage { .. },
                ),
                _,
            ) => {}
            // the pick-pet bag ops are applied by
            // `hud::cos::state::apply_cos_bag_ops` — ops 26/27 carry only two
            // slot numbers, so the moved item record is known only to the
            // container it leaves, and that system holds both sides at once
            (
                Some(
                    InventoryOperationResult::GroundToPet { .. }
                    | InventoryOperationResult::PetToPet { .. }
                    | InventoryOperationResult::PetToInventory { .. }
                    | InventoryOperationResult::InventoryToPet { .. }
                    | InventoryOperationResult::GroundToPetToInventory { .. },
                ),
                _,
            ) => {}
            (Some(InventoryOperationResult::Unknown { op, .. }), _) => {
                warn!("inventory: unknown 0xB034 op {op:#04x} — capture for decode");
            }
            (None, error) => {
                warn!("inventory: operation rejected (error {:?})", error);
            }
        }
        if state.drag.is_some() {
            state.drag = None;
        }
    }
}

/// Remove the item attached in one equipment slot: despawn a still-pending
/// attachment for it, then detach whatever it already spawned under the
/// character's skeleton wrapper.
///
/// Keyed on the resource **handle**, which is the identity both halves already
/// speak: `PendingItemAttachment` holds one, and `AttachResource` records it as
/// `SpawnedFromResource` for `DetachResource` to match on. Matching the pending
/// half by its `item <code name>` `Name` (as the unequip path used to) could
/// only ever find that half, and only while the spelling agreed.
fn remove_attached_item(
    commands: &mut Commands,
    pending: &Query<(Entity, &PendingItemAttachment, &ChildOf)>,
    character: Entity,
    wrapper: Entity,
    handle: Handle<SroResource>,
) {
    for (entity, item, child_of) in pending.iter() {
        if child_of.parent() == character && item.0.id() == handle.id() {
            // try_despawn: the character may go with it in the same tick
            commands.entity(entity).try_despawn();
        }
    }
    commands.detach_resource(handle, wrapper);
}

/// The character's body wrapper — the child holding the [`SkeletonBinding`]
/// that attachments hang under.
fn body_wrapper(
    children: &Query<&Children>,
    wrappers: &Query<(), With<SkeletonBinding>>,
    character: Entity,
) -> Option<Entity> {
    children
        .get(character)
        .ok()?
        .iter()
        .find(|child| wrappers.get(*child).is_ok())
}

/// Apply 0x3038 for the local player: attach the newly equipped item's 3d
/// resource to the body (same recipe as the spawn-time attachment in
/// `game_scene::attach_character_equipment`) and let weapons switch the
/// animation stance.
///
/// **A swap removes the outgoing piece here**, because the wire never mentions
/// it: moving a bag item onto an occupied equipment slot produces a second
/// 0x3038 for that slot and no 0x3039 at all (capture-verified — two equips
/// into slot 1 at 22:15:10 and 22:15:25 in `packet_dump/0x3038.log` with
/// nothing between them in `0x3039.log`, while an explicit unequip at 22:36
/// does send one). Attaching without removing left the old armor on the body
/// underneath the new one.
#[allow(clippy::too_many_arguments)]
pub fn on_entity_equip(
    mut reader: MessageReader<EntityEquip>,
    player: Query<(Entity, &NetworkId), With<Player>>,
    remote_players: Query<(Entity, &NetworkId, &RemoteEntity), Without<Player>>,
    inventories: Query<&Inventory, With<Player>>,
    children: Query<&Children>,
    wrappers: Query<(), With<SkeletonBinding>>,
    pending: Query<(Entity, &PendingItemAttachment, &ChildOf)>,
    item_data: Res<ClientItemData>,
    rare_effects: Res<ClientRareEffects>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
) {
    for msg in reader.read() {
        // 0x3038 is broadcast for everyone in range; resolve the local
        // player first, then nearby remote players by NetworkId. The packet
        // carries no opt level (capture-verified: 10 bytes,
        // uid|slot|ref|one_handed) — the local player's +N tier resolves
        // later from the settled inventory slot; remote players' tier is
        // UNKNOWN on a live equip, so they get no +N shine/flare until
        // respawn (the spawn record does carry opt levels).
        let local = player
            .single()
            .ok()
            .filter(|(_, NetworkId(uid))| msg.unique_id == *uid)
            .map(|(entity, _)| (entity, true));
        let remote = || {
            remote_players
                .iter()
                .find(|(_, NetworkId(uid), kind)| {
                    msg.unique_id == *uid && matches!(kind, RemoteEntity::Player)
                })
                .map(|(entity, ..)| (entity, false))
        };
        let Some((entity, is_local)) = local.or_else(remote) else {
            continue;
        };
        let Some(item_row) = item_data.get(&(msg.ref_id as i32)) else {
            warn!("inventory: no itemdata for equipped item {}", msg.ref_id);
            continue;
        };
        if let Some(group) = item_row.animation_group() {
            commands
                .entity(entity)
                .insert(PreferredAnimationGroup(group.to_string()));
        }
        // Detach whatever this equip replaces, BEFORE attaching the new piece:
        // `DetachResource` re-shows the default body meshes the outgoing item
        // had hidden, so running it afterwards would undo the incoming item's
        // own hiding and leave a bare torso under fresh armor.
        //
        // The outgoing item is read from the local player's `Inventory`, which
        // still holds it at this moment — the 0x3038 equip precedes the 0xB034
        // move that fills the slot, the same ordering `AttachmentShine::EquipSlot`
        // and `AttachmentRareAura::gate` already depend on. That is why this
        // needs no separate slot->attachment bookkeeping.
        //
        // Remote players have no `Inventory`, so their swaps keep the old piece
        // until they respawn into view. The wire gives us nothing to identify it
        // with, so that gap is UNKNOWN by construction rather than unhandled.
        if is_local {
            let outgoing = inventories
                .single()
                .ok()
                .and_then(|inventory| inventory.get(msg.slot))
                .filter(|item| item.ref_id != msg.ref_id)
                .and_then(|item| item_data.get(&(item.ref_id as i32)))
                .and_then(|row| row.resource_path());
            if let (Some(path), Some(wrapper)) =
                (outgoing, body_wrapper(&children, &wrappers, entity))
            {
                debug!(
                    "inventory: slot {} swap — detaching the outgoing piece",
                    msg.slot
                );
                remove_attached_item(
                    &mut commands,
                    &pending,
                    entity,
                    wrapper,
                    asset_server.load(path),
                );
            }
        }
        if let Some(item_path) = item_row.resource_path() {
            let mut item = commands.spawn((
                PendingItemAttachment(asset_server.load(item_path)),
                ChildOf(entity),
                Name::from(format!("item {}", item_row.code_name())),
            ));
            // +N enhancement glow: EntityEquip carries no opt level, and the
            // 0x3038 equip precedes the 0xB034 move that fills the slot, so
            // the local player's tier is resolved later from the settled
            // equipment slot (weapons only — attach filters skeleton-less
            // items). Remote players have no Inventory to resolve against.
            if is_local {
                item.insert(AttachmentShine::EquipSlot {
                    slot: msg.slot,
                    ref_id: msg.ref_id,
                });
            }
            // Rare ("Seal of …") aura + +8 enchant flare, same gate as the
            // spawn-time attach (game_scene::attach_character_equipment):
            // ItemRare.txt rows (min_opt None) need only rarity and attach
            // immediately; ItemOptionEfp.txt rows (min_opt) can't read the
            // opt level yet — local equips carry the slot gate and resolve
            // once the 0xB034 move settles (AttachmentRareAura::gate);
            // remote equips drop them (opt UNKNOWN, see above).
            let auras: Vec<_> = rare_effects
                .get(item_row.code_name())
                .iter()
                .filter(|a| match a.min_opt {
                    Some(_) => is_local,
                    None => item_row.is_rare(),
                })
                .cloned()
                .collect();
            if !auras.is_empty() {
                item.insert(AttachmentRareAura {
                    auras,
                    gate: is_local.then_some((msg.slot, msg.ref_id)),
                });
            }
        }
    }
}

/// Apply 0x3039: detach the unequipped item's meshes from the skeleton wrapper
/// (`DetachResource`, the inverse of the attach recipe — it also re-shows the
/// default body parts the item had replaced), and despawn a still-pending
/// attachment if the resource had not finished loading yet.
///
/// Every way this can decline to act now says so. The path is a chain of
/// lookups — the character, its itemdata row, its resource, its wrapper — and
/// each one used to `continue` in silence, which is indistinguishable from
/// "the item was detached" when the armor is still visibly on the body.
#[allow(clippy::too_many_arguments)]
pub fn on_entity_unequip(
    mut reader: MessageReader<EntityUnequip>,
    player: Query<(Entity, &NetworkId), With<Player>>,
    remote_players: Query<(Entity, &NetworkId, &RemoteEntity), Without<Player>>,
    children: Query<&Children>,
    wrappers: Query<(), With<SkeletonBinding>>,
    pending: Query<(Entity, &PendingItemAttachment, &ChildOf)>,
    item_data: Res<ClientItemData>,
    asset_server: Res<AssetServer>,
    pending_shines: Query<(Entity, &PendingWeaponShine)>,
    pending_auras: Query<(Entity, &PendingRareAura)>,
    mut commands: Commands,
) {
    for msg in reader.read() {
        // broadcast like 0x3038: local player or a nearby remote player
        let local = player
            .single()
            .ok()
            .filter(|(_, NetworkId(uid))| msg.unique_id == *uid)
            .map(|(entity, _)| entity);
        let remote = || {
            remote_players
                .iter()
                .find(|(_, NetworkId(uid), kind)| {
                    msg.unique_id == *uid && matches!(kind, RemoteEntity::Player)
                })
                .map(|(entity, ..)| entity)
        };
        let Some(entity) = local.or_else(remote) else {
            continue;
        };
        let Some(item_row) = item_data.get(&(msg.ref_id as i32)) else {
            warn!(
                "inventory: no itemdata for unequipped item {} (slot {}) — \
                 it stays rendered on the character",
                msg.ref_id, msg.slot
            );
            continue;
        };
        // Taking a weapon off returns the character to the unarmed stance.
        //
        // `"default"` is not a guess: it is the group `SroResource::
        // find_animation_entry` already falls back to for every type a named
        // weapon group does not carry, so naming it here asks for exactly the
        // clips an unarmed character was always going to resolve.
        //
        // A *swap* never reaches this code — the wire sends a second 0x3038 for
        // the slot and no 0x3039 at all (see `on_entity_equip`) — so a 0x3039
        // for a weapon really does mean the hands are now empty. This runs
        // before the resource-path check below, because an item with no 3d
        // resource still changes the stance.
        if item_row.animation_group().is_some() {
            commands
                .entity(entity)
                .insert(PreferredAnimationGroup("default".to_string()));
        }
        // Nothing was ever attached for an item with no 3d resource, so there
        // is nothing to take off either.
        let Some(item_path) = item_row.resource_path() else {
            continue;
        };
        let Some(wrapper) = body_wrapper(&children, &wrappers, entity) else {
            warn!(
                "inventory: unequip of {} found no skeleton wrapper on the character — \
                 it stays rendered",
                item_row.code_name()
            );
            continue;
        };
        let handle: Handle<SroResource> = asset_server.load(item_path);
        // Stale-pending guard: a queued shine/aura for this item would
        // otherwise ride its 5s timer and could land on a same-model item
        // re-equipped during a fast swap.
        for (marker, shine) in pending_shines.iter() {
            if shine.wrapper == wrapper && shine.item.id() == handle.id() {
                commands.entity(marker).despawn();
            }
        }
        for (marker, aura) in pending_auras.iter() {
            if aura.wrapper == wrapper && aura.item.id() == handle.id() {
                commands.entity(marker).despawn();
            }
        }
        remove_attached_item(&mut commands, &pending, entity, wrapper, handle);
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use packets::agent::character_data::{ItemTypeData, RentInfo};

    /// A player holding one stack of `count` potions in slot 13.
    fn app_with_stack(count: u16) -> App {
        let mut app = App::new();
        let mut slots = vec![None; 45];
        slots[13] = Some(packets::agent::character_data::InventoryItem {
            slot: 13,
            rent: RentInfo::default(),
            ref_id: 4,
            data: ItemTypeData::Expendable {
                stack_count: count,
                assimilation_prob: None,
                mag_params: vec![],
            },
        });
        app.add_message::<ItemUseResponse>()
            .add_systems(Update, on_item_consumed);
        app.world_mut().spawn((
            Player,
            Inventory {
                slots,
                avatar_slots: vec![None; 5],
                gold: 0,
            },
        ));
        app
    }

    /// A player wielding weapon class `class` with `arrows` in the ammo slot.
    fn bow_app(class: Option<u8>, arrows: Option<u16>) -> App {
        use crate::plugins::combat::LocalAttackLanded;
        use crate::plugins::skills::EquippedWeapon;

        let mut app = App::new();
        let mut slots = vec![None; 45];
        if let Some(stack_count) = arrows {
            slots[AMMO_SLOT as usize] = Some(packets::agent::character_data::InventoryItem {
                slot: AMMO_SLOT,
                rent: RentInfo::default(),
                ref_id: 62, // ITEM_ETC_AMMO_ARROW_01, from the live capture
                data: ItemTypeData::Expendable {
                    stack_count,
                    assimilation_prob: None,
                    mag_params: vec![],
                },
            });
        }
        app.add_message::<LocalAttackLanded>()
            .insert_resource(EquippedWeapon { class, reach: None })
            .add_systems(Update, predict_ammo_consumption);
        app.world_mut().spawn((
            Player,
            Inventory {
                slots,
                avatar_slots: vec![None; 5],
                gold: 0,
            },
        ));
        app
    }

    /// One of our own 0xB070 attacks, delivering `instances` damage instances.
    fn fire(app: &mut App, instances: u16) {
        app.world_mut()
            .write_message(crate::plugins::combat::LocalAttackLanded(instances));
        app.update();
    }

    /// One arrow per shot — every shot, not once per engagement.
    ///
    /// The regression this pins: the count was first taken off `AttackSwing`,
    /// which travels through the `PendingSwings` presentation queue and so is
    /// delayed, coalesced and dropped. That spent a single arrow per monster
    /// attacked no matter how many shots were loosed at it.
    #[test]
    fn every_bow_shot_spends_an_arrow() {
        let mut app = bow_app(Some(6), Some(500));

        fire(&mut app, 1);
        assert_eq!(stack_at(&app, AMMO_SLOT), Some(499));

        fire(&mut app, 1);
        fire(&mut app, 1);
        assert_eq!(
            stack_at(&app, AMMO_SLOT),
            Some(497),
            "only the first shot of the engagement was counted"
        );
    }

    /// A 2- or 3-arrow combo looses that many arrows, which the packet reports
    /// as the attack's per-target damage-instance count.
    #[test]
    fn a_multi_arrow_combo_spends_one_arrow_per_arrow() {
        let mut app = bow_app(Some(6), Some(500));

        fire(&mut app, 2);
        assert_eq!(stack_at(&app, AMMO_SLOT), Some(498));

        fire(&mut app, 3);
        assert_eq!(stack_at(&app, AMMO_SLOT), Some(495));
    }

    /// A blade swings for free — which is why the count correctly never moved
    /// while a melee weapon was equipped.
    #[test]
    fn a_melee_swing_spends_no_ammo() {
        let mut app = bow_app(Some(2), Some(500));

        fire(&mut app, 1);

        assert_eq!(stack_at(&app, AMMO_SLOT), Some(500));
    }

    /// Firing the last arrow empties the slot rather than leaving a zero stack.
    #[test]
    fn the_last_arrow_empties_the_slot() {
        let mut app = bow_app(Some(6), Some(1));

        fire(&mut app, 1);

        assert_eq!(stack_at(&app, AMMO_SLOT), None);
    }

    fn stack_at(app: &App, slot: u8) -> Option<u16> {
        let world = app.world();
        let inventory = world
            .iter_entities()
            .find_map(|entity| entity.get::<Inventory>())?;
        match inventory.get(slot)?.data {
            ItemTypeData::Expendable { stack_count, .. } => Some(stack_count),
            _ => None,
        }
    }

    /// The consumed count only ever arrives on the 0xB04C ack — no 0x3040
    /// quantity push follows an item use on this server — so drinking a potion
    /// left the bag showing the old number until the next full 0x3013.
    #[test]
    fn the_use_ack_decrements_the_stack() {
        let mut app = app_with_stack(6);
        app.world_mut().write_message(ItemUseResponse::Success {
            slot: 13,
            remaining: 5,
            type_id: 0x08ec,
        });
        app.update();
        assert_eq!(stack_at(&app, 13), Some(5));
    }

    /// `remaining` is absolute, not a delta: three uses of the same stack must
    /// land on the server's number each time, not subtract repeatedly.
    #[test]
    fn remaining_is_applied_as_an_absolute_count() {
        let mut app = app_with_stack(6);
        for remaining in [5, 4, 3] {
            app.world_mut().write_message(ItemUseResponse::Success {
                slot: 13,
                remaining,
                type_id: 0x08ec,
            });
            app.update();
            assert_eq!(stack_at(&app, 13), Some(remaining));
        }
    }

    /// The last one consumed empties the slot rather than leaving a ghost
    /// stack of zero, which would still draw an icon.
    #[test]
    fn using_the_last_one_clears_the_slot() {
        let mut app = app_with_stack(1);
        app.world_mut().write_message(ItemUseResponse::Success {
            slot: 13,
            remaining: 0,
            type_id: 0x08ec,
        });
        app.update();
        assert_eq!(stack_at(&app, 13), None);
    }

    /// A refusal must not touch the bag at all — the item was not consumed.
    #[test]
    fn a_rejected_use_leaves_the_stack_alone() {
        let mut app = app_with_stack(6);
        app.world_mut()
            .write_message(ItemUseResponse::Error { code: 0x18A5 });
        app.update();
        assert_eq!(stack_at(&app, 13), Some(6));
    }
}
