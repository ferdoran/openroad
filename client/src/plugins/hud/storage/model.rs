//! Storage (warehouse) session state + deposit/withdraw application.
//!
//! Idea: the dialog's "Deposit into storage." option emits `OpenStorage`; the
//! window opens immediately (like the store — its chrome is client data) and
//! 0x703C asks the server for the storage, which answers with the
//! begin/data/end push (0x3047 gold → 0x3049 chunks → 0x3048). The chunks
//! only mean something concatenated, so they accumulate in
//! [`StorageDataBuffer`] until the end marker parses them into
//! [`StorageState::items`] — a second [`Inventory`] instance mirroring the
//! storage slots. The push comes only ONCE per character session (a repeat
//! 0x703C is refused with 0xB03C `02 0E1C`), so reopens reuse the cached
//! model until the world scene is left.
//!
//! Item moves ride the 0x7034 storage ops (1 within-storage, 2 deposit,
//! 3 withdraw, 11/12 gold) and are **never** applied optimistically: the
//! 0xB034 ack carries the authoritative slots and applies them, with
//! [`PendingStorageOp`] only guarding against unsolicited acks. The deposit
//! fee is server-side with no confirm dialog (vanilla parity — no fee string
//! exists in textdata); the player's gold total arrives via the
//! already-consumed 0x304E. Layouts: `docs/net-storage-0x3047-0x3049.md`.

use bevy::prelude::*;

use packets::agent::character_data::InventoryItem;
use packets::agent::inventory::{InventoryOperationResponse, InventoryOperationResult};
use packets::agent::storage::{
    parse_storage_items, StorageDataBegin, StorageDataChunk, StorageDataEnd, StorageDataRequest,
    StorageDataResponse,
};
use packets::{hexdump, Packet};

use crate::net::connection::SilkroadConnection;
use crate::plugins::hud::npc_dialog::model::{NpcDialogState, OpenStorage};
use crate::plugins::net::agent::AgentConnection;
use crate::plugins::net::entities::{DisplayName, NetworkId};
use crate::plugins::net::inventory::Inventory;
use crate::plugins::player::Player;
use crate::plugins::textdata::ClientItemData;

/// Storage pages are the vanilla 6x5 grid.
pub const STORAGE_SLOTS_PER_PAGE: u8 = 30;

/// The storage item currently under the cursor, as a clone so the tooltip can
/// render it without reaching into the window.
///
/// Deliberately NOT a field of [`StorageState`]: `sync_storage_window`
/// rebuilds the whole window whenever that resource changes, so hover state
/// living there would despawn and respawn the grid on every mouse move.
#[derive(Resource, Default)]
pub struct StorageHoveredItem(pub Option<InventoryItem>);

/// The 0x3049 chunks between a 0x3047 begin and its 0x3048 end. The server
/// may split the item section across several packets, so nothing can be
/// parsed until the end marker arrives (xBot accumulates the same way).
#[derive(Resource, Default)]
pub struct StorageDataBuffer(pub Vec<u8>);

#[derive(Resource, Default)]
pub struct StorageState {
    pub session: Option<StorageSession>,
    /// The storage slots + storage gold, mirrored ONLY from server packets
    /// (0x3049 list, 0x3047 gold, 0xB034 acks) — reuses [`Inventory`] with
    /// slot 0 as the first storage slot (no equipment offset).
    pub items: Inventory,
    /// The 0x3049 item list arrived for this session; the grid renders
    /// dimmed until then.
    pub synced: bool,
    pub active_page: u8,
}

impl StorageState {
    /// Spinner page count over the current storage size.
    pub fn page_count(&self) -> u8 {
        self.items.size().div_ceil(STORAGE_SLOTS_PER_PAGE).max(1)
    }

    /// First empty storage slot (window-body drops).
    pub fn first_free_slot(&self) -> Option<u8> {
        (0..self.items.size()).find(|&slot| self.items.get(slot).is_none())
    }
}

#[derive(Clone, Debug)]
pub struct StorageSession {
    /// The NPC entity backing the talk session.
    #[allow(dead_code)]
    pub npc: Entity,
    pub npc_id: u32,
    #[allow(dead_code)]
    pub title: String,
}

/// The last storage request sent, so the ack can be applied server-confirmed.
#[derive(Resource, Default)]
pub struct PendingStorageOp(pub Option<StorageOp>);

/// What is in flight. The ack carries its own authoritative slots/amount, so
/// this only guards against applying an unsolicited one; the request's own
/// values are logged at send time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageOp {
    /// Inventory → storage.
    Deposit,
    /// Storage → inventory.
    Withdraw,
    /// Storage-internal move.
    MoveInStorage,
    GoldDeposit,
    GoldWithdraw,
}

/// Open the storage window (dialog "Deposit into storage." option). Vanilla
/// flow: storage swaps in for the dialog (hidden, not state-closed) and the
/// inventory opens alongside; 0x703C asks the server for the storage push.
pub fn open_storage(
    mut requests: MessageReader<OpenStorage>,
    npcs: Query<(&NetworkId, Option<&DisplayName>)>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    mut state: ResMut<StorageState>,
    mut buffer: ResMut<StorageDataBuffer>,
    mut inventory: ResMut<crate::plugins::hud::inventory::model::InventoryState>,
) {
    for OpenStorage { npc } in requests.read() {
        let Ok((network_id, display_name)) = npcs.get(*npc) else {
            warn!("storage: OpenStorage for an entity without a network id");
            continue;
        };
        state.session = Some(StorageSession {
            npc: *npc,
            npc_id: network_id.0,
            title: display_name.map(|n| n.0.clone()).unwrap_or_default(),
        });
        state.active_page = 0;
        inventory.open = true;
        // The server sends the storage push only ONCE per character session;
        // a repeat 0x703C is refused with 0xB03C `02 0E1C`. So once synced,
        // reopen from the cached model (kept current by the 0xB034 acks) and
        // don't ask again. `synced` resets on leaving the world scene.
        if state.synced {
            info!(
                "storage: reopening with cached data ({} items)",
                state.items.slots.iter().flatten().count()
            );
            continue;
        }
        state.items = Inventory::default();
        buffer.0.clear();
        let Ok(conn) = conn.single() else {
            continue;
        };
        let request = StorageDataRequest::new(network_id.0);
        info!(
            "storage: requesting storage data (0x703C) from npc {}",
            network_id.0
        );
        if let Err(e) = conn.get_sender().send(Packet::from(request).into()) {
            error!("network: failed to send StorageDataRequest: {}", e.0);
        }
    }
}

/// The storage lives inside the talk session: when the dialog closes (walked
/// away, deselected, ended), the storage closes with it.
pub fn close_storage_with_dialog(dialog: Res<NpcDialogState>, mut state: ResMut<StorageState>) {
    // Clear the session once the dialog no longer belongs to this NPC — not only
    // on Closed — so a lingering storage session can't hide a different NPC's
    // freshly opened dialog (#217; see close_store_with_dialog).
    if let Some(session) = state.session.as_ref() {
        if dialog.npc() != Some(session.npc) {
            state.session = None;
        }
    }
}

/// 0xB03C — ack for the 0x703C request. Success is silent (the push itself
/// follows); a rejection would otherwise leave a dead, dimmed window, so it
/// must be visible in the log.
pub fn on_storage_data_ack(mut reader: MessageReader<StorageDataResponse>) {
    for msg in reader.read() {
        if msg.result != 1 {
            warn!(
                "storage: data request rejected (0xB03C result {}, error {:#06X})",
                msg.result,
                msg.error.unwrap_or_default()
            );
        }
    }
}

/// 0x3047 — the push begins and carries the storage gold; start a fresh
/// chunk buffer.
pub fn on_storage_begin(
    mut reader: MessageReader<StorageDataBegin>,
    mut state: ResMut<StorageState>,
    mut buffer: ResMut<StorageDataBuffer>,
) {
    for msg in reader.read() {
        info!("storage: data begin (0x3047), gold {}", msg.gold);
        if !msg.tail.is_empty() {
            debug!(
                "storage: 0x3047 has an unexpected tail {} — capture for decode",
                hexdump(&msg.tail, 24)
            );
        }
        state.items.gold = msg.gold;
        buffer.0.clear();
    }
}

/// 0x3049 — accumulate; the section can span several packets.
pub fn on_storage_chunk(
    mut reader: MessageReader<StorageDataChunk>,
    mut buffer: ResMut<StorageDataBuffer>,
) {
    for msg in reader.read() {
        buffer.0.extend_from_slice(&msg.raw);
    }
}

/// 0x3048 — the accumulated chunks are complete: parse the item section.
pub fn on_storage_end(
    mut reader: MessageReader<StorageDataEnd>,
    item_data: Res<ClientItemData>,
    mut state: ResMut<StorageState>,
    mut buffer: ResMut<StorageDataBuffer>,
) {
    for _ in reader.read() {
        let raw = std::mem::take(&mut buffer.0);
        if raw.is_empty() {
            // Distinct from a decode failure: either no 0x3049 arrived at all
            // or something ran this before `on_storage_chunk` (the systems
            // are chained precisely to prevent that).
            warn!("storage: 0x3048 end with no 0x3049 chunks — storage stays unsynced");
            continue;
        }
        match parse_storage_items(&raw, &*item_data) {
            Ok((size, items)) => {
                info!(
                    "storage: data end (0x3048) — size {size}, {} items",
                    items.len()
                );
                let gold = state.items.gold;
                let mut slots = vec![None; size as usize];
                for item in items {
                    let index = item.slot as usize;
                    if index >= slots.len() {
                        slots.resize(index + 1, None);
                    }
                    slots[index] = Some(item);
                }
                state.items = Inventory {
                    slots,
                    avatar_slots: Vec::new(),
                    gold,
                };
                state.synced = true;
            }
            Err(e) => {
                warn!(
                    "storage: item section parse failed ({e:?}) — {} bytes: {} — capture for decode",
                    raw.len(),
                    hexdump(&raw, 64)
                );
            }
        }
    }
}

/// Apply the 0xB034 storage acks. The ack carries the authoritative slots
/// (xBot's `InventoryItemMovement_*` handlers read exactly these fields), so
/// the pending op is only a sanity check — item records move between the
/// player [`Inventory`] and [`StorageState::items`], never optimistically.
pub fn on_storage_response(
    mut reader: MessageReader<InventoryOperationResponse>,
    mut pending: ResMut<PendingStorageOp>,
    mut state: ResMut<StorageState>,
    mut inventories: Query<&mut Inventory, With<Player>>,
    // Needed by `apply_move`, which merges same-item stacks and has to read
    // `max_stack` to know where a stack tops out.
    item_data: Res<crate::plugins::textdata::ClientItemData>,
) {
    for msg in reader.read() {
        let Some(operation) = &msg.operation else {
            // rejections drop the pending op (the raw error code is logged by
            // the inventory handler); the code table is uncaptured — record
            // observed codes in docs/net-storage-0x3047-0x3049.md
            if pending.0.take().is_some() {
                warn!("storage: operation rejected (error {:?})", msg.error);
            }
            continue;
        };
        match *operation {
            InventoryOperationResult::StorageToStorage {
                source,
                target,
                amount,
            } => {
                pending.0.take();
                debug!("storage: moved {source} -> {target} (x{amount})");
                state.items.apply_move(source, target, amount, &item_data);
            }
            InventoryOperationResult::InventoryToStorage { source, target } => {
                pending.0.take();
                let Ok(mut inventory) = inventories.single_mut() else {
                    continue;
                };
                let Some(mut item) = inventory
                    .slots
                    .get_mut(source as usize)
                    .and_then(|slot| slot.take())
                else {
                    warn!("storage: deposit ack for an empty inventory slot {source}");
                    continue;
                };
                debug!("storage: deposited slot {source} -> storage {target}");
                item.slot = target;
                state.items.gain_item(item);
            }
            InventoryOperationResult::StorageToInventory { source, target } => {
                pending.0.take();
                let Some(mut item) = state
                    .items
                    .slots
                    .get_mut(source as usize)
                    .and_then(|slot| slot.take())
                else {
                    warn!("storage: withdraw ack for an empty storage slot {source}");
                    continue;
                };
                debug!("storage: withdrew storage {source} -> slot {target}");
                item.slot = target;
                for mut inventory in inventories.iter_mut() {
                    inventory.gain_item(item.clone());
                }
            }
            // gold: the ack's amount is authoritative for the STORAGE total
            // (xBot adds/subtracts exactly this); the player's own total is
            // corrected by the 0x304E that accompanies it.
            // Gold: only the STORAGE side is applied here. The player's own
            // total is NOT touched — the server sends the authoritative new
            // total in a 0x304E right before this ack (capture 2026-08-08:
            // 0x304E `01 ff2c0000` 38ms ahead of `01 0c d2040000…`), handled
            // by `inventory::model::on_gold_update`. Subtracting the amount
            // on top double-counted it, so a 1234 deposit read as 2468 gone
            // until a relog. Same trap as pickup gold (op 6).
            InventoryOperationResult::InventoryGoldToStorage { amount } => {
                pending.0.take();
                debug!("storage: deposited {amount} gold (player total via 0x304E)");
                state.items.gold = state.items.gold.saturating_add(amount);
            }
            InventoryOperationResult::StorageGoldToInventory { amount } => {
                pending.0.take();
                debug!("storage: withdrew {amount} gold (player total via 0x304E)");
                state.items.gold = state.items.gold.saturating_sub(amount);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use packets::agent::character_data::{InventoryItem, ItemTypeData, RentInfo};

    fn item(slot: u8, ref_id: u32) -> InventoryItem {
        InventoryItem {
            slot,
            rent: RentInfo::default(),
            ref_id,
            data: ItemTypeData::Expendable {
                stack_count: 1,
                assimilation_prob: None,
                mag_params: vec![],
            },
        }
    }

    #[test]
    fn page_count_and_first_free_slot() {
        let mut state = StorageState::default();
        assert_eq!(state.page_count(), 1);
        state.items.slots = vec![None; 60];
        assert_eq!(state.page_count(), 2);
        state.items.slots[0] = Some(item(0, 5));
        assert_eq!(state.first_free_slot(), Some(1));
        state.items.slots = vec![Some(item(0, 5))];
        assert_eq!(state.first_free_slot(), None);
    }
}
