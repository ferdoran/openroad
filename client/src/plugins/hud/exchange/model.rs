//! Player-to-player exchange (trade) session state + the two-stage lock.
//!
//! Idea: the server owns the whole trade. 0x3085 opens the window against a
//! partner's spawn id; from then on the *partner's* half is render-only mirror
//! state, replaced wholesale out of 0x308C (items) and 0x3089 (gold), and our
//! own half is never applied optimistically. The lock is deliberately two
//! stage — it is the original's anti-scam core: 0x7082 confirms (locks) our
//! offer, and approve (0x7083) is only offered once the *partner* has
//! confirmed too, so neither side can swap an item out after the other has
//! agreed. 0x3087 completes the trade, 0x3088 aborts it, and closing the
//! window is a protocol act (0x7084) rather than a UI hide.
//!
//! Staging our own items and gold is not an exchange opcode at all: it rides
//! 0x7034/0xB034 sub-ops 4/5/13, which `InventoryOperationRequest` does not
//! model yet (#36). Until it does, [`ExchangeSession::own_items`] stays empty
//! and the own pane renders as empty vanilla slots — the partner mirror, the
//! lock and the exit path are unaffected.
//!
//! Wire layouts: `docs/net-exchange-0x3085.md`. Lifecycle + xBot citations:
//! `docs/re/systems/exchange.md`. Window layout: `docs/re/ui/exchange-window.md`.

use bevy::prelude::*;

use packets::agent::character_data::InventoryItem;
use packets::agent::ingame::{ExchangeInviteRequest, ExchangeInviteResponse};

use packets::agent::exchange::{
    ExchangeApproveRequest, ExchangeApproveResponse, ExchangeCanceled, ExchangeCompleted,
    ExchangeConfirmRequest, ExchangeConfirmResponse, ExchangeExitRequest, ExchangeExitResponse,
    ExchangeGoldUpdate, ExchangeItemsUpdate, ExchangePlayerConfirmed, ExchangeStarted,
};
use packets::Packet;

use crate::net::connection::SilkroadConnection;
use crate::plugins::hud::chat::model::{ChatHistory, ChatLine};
use crate::plugins::net::agent::AgentConnection;
use crate::plugins::net::inventory::{Inventory, BAG_FIRST_SLOT};
use crate::plugins::player::Player;
use crate::plugins::textdata::ClientItemData;

/// Slots per side. `ifexchange.txt` declares 12 per pane (ids 100-111 and
/// 200-211); the 0x308C capacity-12 assumption in `docs/re/systems/exchange.md`
/// §9.9 is the same number seen from the wire.
pub const EXCHANGE_SLOTS: usize = 12;

/// Shown when the server refuses our exchange invite. **Ours, stated deviation
/// (ADR-0009):** `FUN_00778190(1, code, …)` proves the original renders these
/// codes through its error-message box in category 1 (exchange,
/// `docs/re/net/inbound/exchange-trade.md` §0xB081), but nothing in this
/// repository — and no `UIIT_*` key in the user's own `textuisystem.txt` —
/// enumerates a single value of that category, so there is no string id to
/// bind. The raw code is printed with the line; an invented sentence per code
/// would look sourced and would not be.
pub const INVITE_REFUSED_NOTICE: &str = "The exchange request was refused.";

/// `UIIT_MSG_DEAL_ASKING`, textuisystem L1714 — the inviter's own "waiting for
/// an answer" line, with `%s` for the target. The invitee's side of the same
/// exchange is `UIIT_MSG_DEAL_ASK` (L1713), which `hud::petition` uses.
pub const DEAL_ASKING: (&str, &str) = ("UIIT_MSG_DEAL_ASKING", "Applying for a trade to [%s].");

/// The open trade, or `None` when no window is up.
#[derive(Resource, Default)]
pub struct ExchangeState {
    pub session: Option<ExchangeSession>,
}

/// One trade. Everything here is server-pushed; nothing is predicted.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExchangeSession {
    /// The partner's spawn unique id (0x3085), used to reject a 0x308C that
    /// describes somebody else.
    pub partner_unique_id: u32,
    /// Replaced wholesale on every 0x308C — the original mirrors the peer's
    /// pane rather than diffing it.
    pub partner_items: Vec<InventoryItem>,
    pub partner_gold: u64,
    /// Our staged offer. Stays empty until 0x7034 sub-ops 4/5/13 exist (#36).
    pub own_items: Vec<InventoryItem>,
    pub own_gold: u64,
    /// We sent 0x7082 and the server acked it: our offer is locked.
    pub own_confirmed: bool,
    /// The peer sent theirs (0x3086).
    pub partner_confirmed: bool,
    pub own_approved: bool,
}

impl ExchangeSession {
    fn new(partner_unique_id: u32) -> Self {
        ExchangeSession {
            partner_unique_id,
            ..default()
        }
    }

    /// Confirm is offered until our own side locks. There is no unconfirm
    /// opcode in the family — backing out after the lock means exiting
    /// (`docs/re/systems/exchange.md` §3, "No unconfirm opcode").
    pub fn can_confirm(&self) -> bool {
        !self.own_confirmed
    }

    /// The anti-scam gate: approve is enabled only once **both** sides are
    /// locked (`InfoManager.cs:1073-1074`). Gating on our own confirm alone
    /// would let the partner restage after we approved, which is precisely the
    /// fraud the two-stage flow exists to prevent.
    pub fn can_approve(&self) -> bool {
        self.own_confirmed && self.partner_confirmed && !self.own_approved
    }

    /// Vanilla reuses one button pair: the action button reads "Confirm" until
    /// our side is locked and "Approve" afterwards (`InfoManager.cs:1068-1080`).
    pub fn awaiting_approve(&self) -> bool {
        self.own_confirmed
    }

    /// Gold is editable only before our own lock — same source line as the
    /// button relabel.
    pub fn gold_editable(&self) -> bool {
        !self.own_confirmed
    }
}

/// 0x3085 — the server opened a trade window against `partner_unique_id`.
pub fn on_exchange_started(
    mut reader: MessageReader<ExchangeStarted>,
    mut state: ResMut<ExchangeState>,
) {
    for msg in reader.read() {
        info!(
            "exchange: started with partner {} (0x3085)",
            msg.partner_unique_id
        );
        state.session = Some(ExchangeSession::new(msg.partner_unique_id));
    }
}

/// 0x308C — the partner's staged list, replaced wholesale.
pub fn on_partner_items(
    mut reader: MessageReader<ExchangeItemsUpdate>,
    item_data: Res<ClientItemData>,
    mut state: ResMut<ExchangeState>,
) {
    for msg in reader.read() {
        let Some(session) = state.session.as_mut() else {
            warn!("exchange: 0x308C with no open trade — ignored");
            continue;
        };
        // The original's parser early-returns for our own uid, so a 0x308C on
        // the wire always describes the peer. Whether a self-targeted one
        // exists at all is [U] (docs/net-exchange-0x3085.md, 0x308C): rather
        // than guess its shape, refuse anything that is not the partner.
        if msg.player_unique_id != session.partner_unique_id {
            warn!(
                "exchange: 0x308C for {} but the trade partner is {} — ignored; \
                 capture packet_dump/0x308C.log if this is a self-targeted update",
                msg.player_unique_id, session.partner_unique_id
            );
            continue;
        }
        let Some(items) = msg.items(&*item_data) else {
            // A partial list would misrepresent what the peer is offering, so
            // the accessor returns None rather than a prefix.
            warn!("exchange: could not decode the partner's staged items — pane left unchanged");
            continue;
        };
        if items.len() > EXCHANGE_SLOTS {
            // The 12-slot capacity is [S], inferred from the window grid
            // rather than observed (docs/re/systems/exchange.md §9.9). If the
            // server ever exceeds it, that assumption is wrong and the extra
            // records would render nowhere — so say so instead of dropping
            // them silently.
            warn!(
                "exchange: partner staged {} items but the vanilla pane holds {} — \
                 capacity assumption refuted, capture packet_dump/0x308C.log",
                items.len(),
                EXCHANGE_SLOTS
            );
        }
        debug!("exchange: partner staged {} item(s)", items.len());
        session.partner_items = items;
    }
}

/// 0x3089 — the partner's staged gold.
pub fn on_partner_gold(
    mut reader: MessageReader<ExchangeGoldUpdate>,
    mut state: ResMut<ExchangeState>,
) {
    for msg in reader.read() {
        let Some(session) = state.session.as_mut() else {
            warn!("exchange: 0x3089 with no open trade — ignored");
            continue;
        };
        debug!(
            "exchange: partner staged {} gold (unk byte {})",
            msg.gold, msg.unk_byte01
        );
        session.partner_gold = msg.gold;
    }
}

/// 0x3086 — the partner locked their offer.
pub fn on_partner_confirmed(
    mut reader: MessageReader<ExchangePlayerConfirmed>,
    mut state: ResMut<ExchangeState>,
) {
    for _ in reader.read() {
        let Some(session) = state.session.as_mut() else {
            continue;
        };
        info!("exchange: partner confirmed (0x3086)");
        session.partner_confirmed = true;
    }
}

/// 0xB082 — ack for our confirm. Only a successful ack locks our side; a
/// refusal must leave the window editable, or the player is stuck.
pub fn on_confirm_response(
    mut reader: MessageReader<ExchangeConfirmResponse>,
    mut state: ResMut<ExchangeState>,
) {
    for msg in reader.read() {
        let Some(session) = state.session.as_mut() else {
            continue;
        };
        if msg.success {
            info!("exchange: our offer is locked (0xB082)");
            session.own_confirmed = true;
        } else {
            // The failure tail is unmodelled ([U], docs/net-exchange-0x3085.md).
            warn!("exchange: confirm refused (0xB082 success=false)");
        }
    }
}

/// 0xB083 — ack for our approve.
pub fn on_approve_response(
    mut reader: MessageReader<ExchangeApproveResponse>,
    mut state: ResMut<ExchangeState>,
) {
    for msg in reader.read() {
        let Some(session) = state.session.as_mut() else {
            continue;
        };
        if msg.success {
            info!("exchange: approved, waiting for the partner (0xB083)");
            session.own_approved = true;
        } else {
            warn!("exchange: approve refused (0xB083 success=false)");
        }
    }
}

/// 0x3087 — the trade went through. The client applies the transfer locally:
/// the partner's staged records land in our first free bag slots and our own
/// staged records leave the bag (`InfoManager.cs:967-1001`).
///
/// Gold is deliberately **not** touched here. The server sends the
/// authoritative new total separately (0x304E), and applying the staged
/// amounts on top of it double-counts — the same trap that made a storage
/// deposit read as twice the gold until a relog (see
/// `hud/storage/model.rs::on_storage_response`).
pub fn on_exchange_completed(
    mut reader: MessageReader<ExchangeCompleted>,
    mut state: ResMut<ExchangeState>,
    mut inventories: Query<&mut Inventory, With<Player>>,
) {
    for _ in reader.read() {
        let Some(session) = state.session.take() else {
            continue;
        };
        info!(
            "exchange: completed (0x3087) — {} item(s) in, {} out",
            session.partner_items.len(),
            session.own_items.len()
        );
        for mut inventory in inventories.iter_mut() {
            for slot in session.own_items.iter().map(|item| item.slot) {
                if let Some(entry) = inventory.slots.get_mut(slot as usize) {
                    *entry = None;
                }
            }
            for item in session.partner_items.iter() {
                let Some(free) = first_free_bag_slot(&inventory) else {
                    warn!("exchange: no free bag slot for a traded item — relog to resync");
                    break;
                };
                let mut item = item.clone();
                item.slot = free;
                inventory.gain_item(item);
            }
        }
    }
}

/// 0x3088 — the trade was called off by the peer or the server.
pub fn on_exchange_canceled(
    mut reader: MessageReader<ExchangeCanceled>,
    mut state: ResMut<ExchangeState>,
) {
    for _ in reader.read() {
        if state.session.take().is_some() {
            info!("exchange: cancelled (0x3088)");
        }
    }
}

/// 0xB084 — ack for our exit. The window closes on the ack, not on the click,
/// so a refused exit keeps the trade visible instead of desyncing us from a
/// trade the server still considers open.
pub fn on_exit_response(
    mut reader: MessageReader<ExchangeExitResponse>,
    mut state: ResMut<ExchangeState>,
) {
    for msg in reader.read() {
        if !msg.success {
            warn!("exchange: exit refused (0xB084 success=false) — window stays open");
            continue;
        }
        if state.session.take().is_some() {
            info!("exchange: exited (0xB084)");
        }
    }
}

/// First free bag slot (equipment slots are below [`BAG_FIRST_SLOT`]).
fn first_free_bag_slot(inventory: &Inventory) -> Option<u8> {
    (BAG_FIRST_SLOT..inventory.size()).find(|slot| inventory.get(*slot).is_none())
}

/// Send one of the empty-bodied exchange requests.
fn send(conn: &Query<&SilkroadConnection, With<AgentConnection>>, packet: Packet, what: &str) {
    let Ok(conn) = conn.single() else {
        return;
    };
    if let Err(e) = conn.get_sender().send(packet.into()) {
        error!("network: failed to send exchange {what}: {}", e.0);
    }
}

/// 0x7081 — ask `unique_id` to trade. The server raises the 0x3080 petition on
/// them and answers us with 0xB081; the window itself opens on the following
/// 0x3085, never here, so nothing is predicted (`docs/net-invite-0x3080.md`
/// §4).
pub fn send_invite(conn: &Query<&SilkroadConnection, With<AgentConnection>>, unique_id: u32) {
    send(
        conn,
        Packet::from(ExchangeInviteRequest { unique_id }),
        "invite",
    );
}

/// 0xB081 — the inviter-side ack for our own 0x7081.
///
/// A raised petition is only an ack: the trade starts with 0x3085. A refusal
/// carries a `u16` code whose value space no source names, so it is reported
/// verbatim rather than translated into an invented sentence — the same rule
/// the stall buyer flow follows for its error codes.
pub fn on_invite_response(
    mut reader: MessageReader<ExchangeInviteResponse>,
    mut history: ResMut<ChatHistory>,
) {
    for msg in reader.read() {
        match msg {
            ExchangeInviteResponse::Accepted { unique_id } => {
                info!("exchange: invite raised on {unique_id} (0xB081)");
            }
            ExchangeInviteResponse::Refused { error } => {
                warn!("exchange: invite refused (0xB081 code {error:#06x})");
                history.push(ChatLine::system(format!(
                    "{INVITE_REFUSED_NOTICE} (code {error:#06x})"
                )));
            }
        }
    }
}

/// 0x7082 — lock our offer.
pub fn send_confirm(conn: &Query<&SilkroadConnection, With<AgentConnection>>) {
    send(conn, Packet::from(ExchangeConfirmRequest), "confirm");
}

/// 0x7083 — approve the trade.
pub fn send_approve(conn: &Query<&SilkroadConnection, With<AgentConnection>>) {
    send(conn, Packet::from(ExchangeApproveRequest), "approve");
}

/// 0x7084 — back out. Closing the window is a protocol act: the session is
/// cleared by the 0xB084 ack, never here.
pub fn send_exit(conn: &Query<&SilkroadConnection, With<AgentConnection>>) {
    send(conn, Packet::from(ExchangeExitRequest), "exit");
}

#[cfg(test)]
mod test {
    use super::*;
    use packets::agent::character_data::{ItemTypeData, RentInfo};

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

    /// The anti-scam gate is the whole point of the two-stage lock: approve
    /// must stay closed until BOTH sides are locked, so the partner cannot
    /// restage after we agreed.
    #[test]
    fn approve_opens_only_after_both_sides_confirmed() {
        let mut session = ExchangeSession::new(7);
        assert!(session.can_confirm());
        assert!(!session.can_approve());

        session.partner_confirmed = true;
        assert!(
            !session.can_approve(),
            "the partner's confirm alone must not open approve"
        );

        session.partner_confirmed = false;
        session.own_confirmed = true;
        assert!(
            !session.can_approve(),
            "our own confirm alone must not open approve"
        );
        assert!(!session.can_confirm(), "confirm is spent once locked");
        assert!(session.awaiting_approve());
        assert!(!session.gold_editable(), "gold locks with the offer");

        session.partner_confirmed = true;
        assert!(session.can_approve());

        session.own_approved = true;
        assert!(!session.can_approve(), "approve is sent once");
    }

    /// 0x3087 moves the partner's records into our first free BAG slots —
    /// never over the equipment slots below `BAG_FIRST_SLOT` — and clears the
    /// slots our own staged records came from.
    #[test]
    fn completion_lands_partner_items_in_free_bag_slots() {
        let mut inventory = Inventory {
            slots: vec![None; BAG_FIRST_SLOT as usize + 3],
            avatar_slots: Vec::new(),
            gold: 500,
        };
        inventory.slots[BAG_FIRST_SLOT as usize] = Some(item(BAG_FIRST_SLOT, 111));

        let session = ExchangeSession {
            partner_unique_id: 7,
            partner_items: vec![item(4, 222), item(9, 333)],
            own_items: vec![item(BAG_FIRST_SLOT, 111)],
            ..default()
        };

        // mirrors on_exchange_completed's body
        for slot in session.own_items.iter().map(|i| i.slot) {
            inventory.slots[slot as usize] = None;
        }
        for item in session.partner_items.iter() {
            let free = first_free_bag_slot(&inventory).expect("a free slot");
            let mut item = item.clone();
            item.slot = free;
            inventory.gain_item(item);
        }

        assert_eq!(
            inventory.get(BAG_FIRST_SLOT).map(|i| i.ref_id),
            Some(222),
            "the freed slot is refilled before later ones"
        );
        assert_eq!(
            inventory.get(BAG_FIRST_SLOT + 1).map(|i| i.ref_id),
            Some(333)
        );
        assert!(
            (0..BAG_FIRST_SLOT).all(|slot| inventory.get(slot).is_none()),
            "equipment slots must never receive a traded item"
        );
        assert_eq!(inventory.gold, 500, "gold is corrected by 0x304E, not here");
    }
}
