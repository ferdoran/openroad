//! NPC store session state + buy/sell application.
//!
//! Idea: the dialog's "Trade in the shop." option emits `OpenStore`; the
//! session snapshot is built here from `ClientShops` (the denormalized
//! ref-shop chain keyed by the NPC's characterdata codename) and closed
//! whenever the underlying talk session ends. Buys and sells go out as the
//! EXPERIMENTAL 0x7034 op-8/op-9 requests and are **never** applied
//! optimistically: [`PendingStoreOp`] remembers what was requested, and the
//! 0xB034 ack applies it — inserting the bought item at the server-named
//! slots (best-effort tail decode, see `packets::agent::inventory`) or
//! shrinking the sold stack. Gold flows through the already-consumed 0x304E.
//! Each sell ack also names the buyback-tray index the item landed in, which
//! is mirrored into the session's 5-deep tray (server-owned eviction — we
//! only follow the index it names).

use bevy::prelude::*;

use packets::agent::character_data::{EquipmentData, InventoryItem, ItemTypeData, RentInfo};
use packets::agent::inventory::{
    InventoryOperationResponse, InventoryOperationResult, ItemRepairResponse, BUYBACK_SLOT_NONE,
};
use packets::hexdump;

use crate::assets::textdata::shops::ShopLayout;
use crate::plugins::hud::npc_dialog::model::{NpcDialogState, OpenStore};
use crate::plugins::net::entities::{CharacterRef, DisplayName, NetworkId};
use crate::plugins::net::inventory::Inventory;
use crate::plugins::player::Player;
use crate::plugins::textdata::{ClientCharacterData, ClientItemData, ClientShops};

#[derive(Resource, Default)]
pub struct StoreState {
    pub session: Option<StoreSession>,
}

/// The itemdata ref id of the shop good under the cursor, for the shared item
/// tooltip (`inventory::tooltip`).
///
/// A ref id and not an [`InventoryItem`] because a shop good is a CATALOG row,
/// not an instance: it has no opt level, no variance roll, no durability and
/// no magic options yet — those are rolled by the server at purchase. The
/// tooltip renders the type-level block for it (see `catalog_tooltip_lines`).
///
/// Deliberately its own resource rather than a field of [`StoreState`], for
/// the same reason [`super::super::storage::model::StorageHoveredItem`] is:
/// `sync_store_window` rebuilds the whole window when `StoreState` changes,
/// so hover state living there would respawn the grid on every mouse move.
#[derive(Resource, Default)]
pub struct StoreHoveredGood(pub Option<u32>);

#[derive(Clone, Debug)]
pub struct StoreSession {
    /// The NPC entity — kept for consumers that need more than the network
    /// id (e.g. a future face/portrait header).
    #[allow(dead_code)]
    pub npc: Entity,
    pub npc_id: u32,
    pub title: String,
    pub layout: ShopLayout,
    /// Tab within the current tab group.
    pub tab: usize,
    /// Spinner page — an index into `ui::page_entries` (tab group × slot
    /// chunk), NOT a raw group index.
    pub page: usize,
    /// The buyback tray: what this session sold, indexed by the server's
    /// `slot_buyback` byte. Session-local and 5 deep, oldest evicted — the
    /// eviction is the server's, we only mirror the index it names
    /// (`docs/re/systems/npc-shop.md` §3). No UI yet: where the strip sits
    /// depends on an unresolved `#ifdef` question (doc §9.5).
    pub buyback: [Option<BuybackEntry>; BUYBACK_TRAY_DEPTH],
}

/// The 5 buyback slots the original tray holds (xBot
/// `PacketParser.cs:1872-1874`).
pub const BUYBACK_TRAY_DEPTH: usize = 5;

/// One item sitting in the buyback tray, as named by a sell ack.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BuybackEntry {
    pub ref_id: u32,
    pub quantity: u16,
}

/// The last store request sent, so the ack can be applied server-confirmed.
#[derive(Resource, Default)]
pub struct PendingStoreOp(pub Option<StoreOp>);

#[derive(Clone, Copy, Debug)]
pub enum StoreOp {
    Buy {
        ref_id: i32,
        opt_level: u8,
        /// The requested quantity; the ack echoes its own (authoritative)
        /// count, so this is Debug-log context only.
        #[allow(dead_code)]
        quantity: u16,
    },
    Sell {
        slot: u8,
        quantity: u16,
    },
}

/// Single-item repair armed: the "Repair" button was clicked and the next
/// click on an inventory slot sends the 0x703E One request (vanilla's
/// hammer-cursor mode, minus the cursor art for now). Cleared on use, on
/// store close, or by a non-slot click.
#[derive(Resource, Default)]
pub struct RepairMode(pub bool);

/// The last repair request sent, applied on the 0xB03E ack (never
/// optimistically — like every store op).
#[derive(Resource, Default)]
pub struct PendingRepair(pub Option<RepairOp>);

#[derive(Clone, Copy, Debug)]
pub enum RepairOp {
    One(u8),
    All,
}

/// The vanilla pre-repair confirmation msgbox (repairing costs gold):
/// Confirm sends the 0x703E request, Cancel just closes.
#[derive(Resource, Default)]
pub struct RepairConfirm {
    pub prompt: Option<RepairPrompt>,
}

#[derive(Clone, Debug)]
pub struct RepairPrompt {
    pub op: RepairOp,
    /// The msgbox body (UIIT_MSG_MSGBOX_REPAIR_ITEM for All, the item name
    /// for One).
    pub message: String,
    /// Durability-prorated cost estimate — the server's actual deduction
    /// (via 0x304E) is authoritative.
    pub cost: u64,
}

/// Client-side repair cost estimate: itemdata's authored full-repair cost
/// (col 27) prorated by lost durability, summed over the op's slots.
pub fn repair_cost_estimate(
    inventory: &Inventory,
    item_data: &ClientItemData,
    magic_options: &crate::plugins::textdata::ClientMagicOptions,
    op: RepairOp,
) -> u64 {
    let slot_cost = |slot: u8| -> u64 {
        let Some(item) = inventory.get(slot) else {
            return 0;
        };
        let ItemTypeData::Equipment(eq) = &item.data else {
            return 0;
        };
        let Some(row) = item_data.get(&(item.ref_id as i32)) else {
            return 0;
        };
        let (Some(cost), Some(max)) = (
            row.cost_repair(),
            crate::plugins::net::inventory::max_durability(row, eq),
        ) else {
            return 0;
        };
        if max == 0 || eq.durability >= max {
            return 0;
        }
        let lost = (max - eq.durability) as u64;
        (cost * lost).div_ceil(max as u64)
    };
    match op {
        RepairOp::One(slot) => slot_cost(slot),
        RepairOp::All => inventory
            .damaged_slots(item_data, magic_options)
            .into_iter()
            .map(slot_cost)
            .sum(),
    }
}

/// Apply the EXPERIMENTAL 0xB03E repair ack: restore durability on the
/// pending slot (or all damaged equipment). The raw tail is hexdumped on
/// every ack — the first live capture pins the real response layout.
pub fn on_repair_response(
    mut reader: MessageReader<ItemRepairResponse>,
    mut pending: ResMut<PendingRepair>,
    mut inventories: Query<&mut Inventory, With<Player>>,
    item_data: Res<ClientItemData>,
    magic_options: Res<crate::plugins::textdata::ClientMagicOptions>,
) {
    for msg in reader.read() {
        info!(
            "store: repair ack (0xB03E) result {} tail {} — capture for decode",
            msg.result,
            hexdump(&msg.tail, 48)
        );
        let Some(op) = pending.0.take() else {
            warn!("store: unsolicited repair ack");
            continue;
        };
        if !msg.is_success() {
            warn!("store: repair {op:?} rejected");
            continue;
        }
        let Ok(mut inventory) = inventories.single_mut() else {
            continue;
        };
        match op {
            RepairOp::One(slot) => inventory.repair_slot(slot, &item_data),
            RepairOp::All => {
                for slot in inventory.damaged_slots(&item_data, &magic_options) {
                    inventory.repair_slot(slot, &item_data);
                }
            }
        }
    }
}

/// The repair mode + confirm dialog are bound to the store session.
pub fn clear_repair_mode_with_store(
    state: Res<StoreState>,
    mut repair: ResMut<RepairMode>,
    mut confirm: ResMut<RepairConfirm>,
) {
    if state.session.is_none() {
        if repair.0 {
            repair.0 = false;
        }
        if confirm.prompt.is_some() {
            confirm.prompt = None;
        }
    }
}

/// Open the store window for an NPC (dialog "Trade in the shop." option).
/// Vanilla flow: the shop swaps in for the dialog (hidden by the dialog ui
/// while a session is open — NOT state-closed, that would end the store via
/// [`close_store_with_dialog`]) and the inventory opens alongside, ready
/// for the drag-to-buy/sell carries.
pub fn open_store(
    mut requests: MessageReader<OpenStore>,
    npcs: Query<(&CharacterRef, &NetworkId, Option<&DisplayName>)>,
    char_data: Res<ClientCharacterData>,
    shops: Res<ClientShops>,
    mut state: ResMut<StoreState>,
    mut inventory: ResMut<crate::plugins::hud::inventory::model::InventoryState>,
) {
    for OpenStore { npc } in requests.read() {
        let Ok((char_ref, network_id, display_name)) = npcs.get(*npc) else {
            warn!("store: OpenStore for an entity without NPC identity");
            continue;
        };
        let Some(codename) = char_data
            .get(&(char_ref.0 as i32))
            .map(|row| row.code_name())
        else {
            warn!("store: no characterdata row for ref {}", char_ref.0);
            continue;
        };
        let Some(layout) = shops.shop_for_npc(codename) else {
            warn!("store: no shop data for {codename}");
            continue;
        };
        state.session = Some(StoreSession {
            npc: *npc,
            npc_id: network_id.0,
            title: display_name.map(|n| n.0.clone()).unwrap_or_default(),
            layout: layout.clone(),
            tab: 0,
            page: 0,
            buyback: [None; BUYBACK_TRAY_DEPTH],
        });
        inventory.open = true;
    }
}

/// The store lives inside the talk session: when the dialog closes (walked
/// away, deselected, ended), the store closes with it.
pub fn close_store_with_dialog(dialog: Res<NpcDialogState>, mut state: ResMut<StoreState>) {
    // Clear the session once the dialog no longer belongs to this NPC — not only
    // on Closed. A lingering session (shop used, then a different NPC's dialog
    // opens) otherwise deadlocks: `hide_dialog_while_store_open` hides the fresh
    // dialog and this never fired because the dialog was Open, not Closed (#217).
    if let Some(session) = state.session.as_ref() {
        if dialog.npc() != Some(session.npc) {
            state.session = None;
        }
    }
}

/// Apply the 0xB034 buy/sell acks against the pending request.
pub fn on_store_response(
    mut reader: MessageReader<InventoryOperationResponse>,
    mut pending: ResMut<PendingStoreOp>,
    mut store: ResMut<StoreState>,
    mut inventories: Query<&mut Inventory, With<Player>>,
    item_data: Res<ClientItemData>,
) {
    for msg in reader.read() {
        let Some(operation) = &msg.operation else {
            // rejections drop the pending op (the error itself is logged by
            // the inventory handler)
            if pending.0.take().is_some() {
                warn!("store: operation rejected (error {:?})", msg.error);
            }
            continue;
        };
        match operation {
            op @ InventoryOperationResult::Buy { tail } => {
                let Some(StoreOp::Buy {
                    ref_id, opt_level, ..
                }) = pending.0.take()
                else {
                    warn!("store: unsolicited 0xB034 buy ack");
                    continue;
                };
                let Some((slots, echoed_quantity)) = op.bought() else {
                    warn!(
                        "store: buy ack tail {} not understood — capture 0xB034 for decode \
                         (inventory now stale until next login)",
                        packets::hexdump(tail, 24)
                    );
                    continue;
                };
                let stackable = item_data
                    .get(&ref_id)
                    .and_then(|row| row.type_ids())
                    .map(|(_, tid2, _, _)| tid2 == 3)
                    .unwrap_or(false);
                for slot in slots {
                    let data = if stackable {
                        ItemTypeData::Expendable {
                            stack_count: echoed_quantity,
                            assimilation_prob: None,
                            mag_params: Vec::new(),
                        }
                    } else {
                        ItemTypeData::Equipment(EquipmentData {
                            opt_level,
                            variance: 0,
                            durability: 0,
                            mag_params: Vec::new(),
                            socket_tag: 1,
                            sockets: Vec::new(),
                            elixir_tag: 2,
                            adv_elixirs: Vec::new(),
                        })
                    };
                    debug!("store: bought ref {ref_id} x{echoed_quantity} into slot {slot}");
                    for mut inventory in inventories.iter_mut() {
                        inventory.gain_item(InventoryItem {
                            slot,
                            rent: RentInfo::default(),
                            ref_id: ref_id as u32,
                            data: data.clone(),
                        });
                    }
                }
            }
            op @ InventoryOperationResult::Sell { tail } => {
                let Some(StoreOp::Sell { slot, quantity }) = pending.0.take() else {
                    warn!("store: unsolicited 0xB034 sell ack");
                    continue;
                };
                let (slot, quantity) = op.sold().unwrap_or_else(|| {
                    debug!(
                        "store: sell ack tail {} not understood, applying the request",
                        packets::hexdump(tail, 24)
                    );
                    (slot, quantity)
                });
                debug!("store: sold x{quantity} from slot {slot}");
                let sold_ref_id = inventories
                    .iter()
                    .find_map(|inventory| inventory.get(slot).map(|item| item.ref_id));
                for mut inventory in inventories.iter_mut() {
                    inventory.remove_amount(slot, quantity);
                }
                if let (Some(session), Some(ref_id), Some((_, tray_slot))) =
                    (store.session.as_mut(), sold_ref_id, op.sold_buyback())
                {
                    record_buyback(&mut session.buyback, tray_slot, ref_id, quantity);
                }
            }
            _ => {}
        }
    }
}

/// Mirror a sell ack's buyback index into the session tray. The server owns
/// the tray's eviction (it names the index the item landed in, reusing the
/// oldest one after 5 sells); [`BUYBACK_SLOT_NONE`] and any index past the
/// tray mean "not buyback-able" and are dropped.
fn record_buyback(
    tray: &mut [Option<BuybackEntry>; BUYBACK_TRAY_DEPTH],
    tray_slot: u8,
    ref_id: u32,
    quantity: u16,
) {
    if tray_slot == BUYBACK_SLOT_NONE {
        return;
    }
    let Some(entry) = tray.get_mut(tray_slot as usize) else {
        warn!("store: sell ack named buyback slot {tray_slot}, past the {BUYBACK_TRAY_DEPTH}-deep tray");
        return;
    };
    *entry = Some(BuybackEntry { ref_id, quantity });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buyback_tray_mirrors_the_servers_index_and_evicts() {
        let mut tray = [None; BUYBACK_TRAY_DEPTH];
        for slot in 0..BUYBACK_TRAY_DEPTH as u8 {
            record_buyback(&mut tray, slot, 100 + slot as u32, 1);
        }
        assert!(tray.iter().all(|e| e.is_some()));
        // the 6th sell reuses index 0 (the server's oldest-evicted tray)
        record_buyback(&mut tray, 0, 999, 7);
        assert_eq!(
            tray[0],
            Some(BuybackEntry {
                ref_id: 999,
                quantity: 7,
            })
        );
    }

    #[test]
    fn buyback_tray_ignores_unbuyable_and_out_of_range_slots() {
        let mut tray = [None; BUYBACK_TRAY_DEPTH];
        record_buyback(&mut tray, BUYBACK_SLOT_NONE, 42, 1);
        record_buyback(&mut tray, BUYBACK_TRAY_DEPTH as u8, 42, 1);
        assert!(tray.iter().all(|e| e.is_none()));
    }
}
