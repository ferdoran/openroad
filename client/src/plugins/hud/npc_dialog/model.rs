//! NPC dialog session state.
//!
//! Idea: the dialog opens the moment the talk request goes out
//! (`TalkStarted`, `cursor::interactions::npcs`) — everything the window
//! shows is client-side data, and gating on the 0xB046 ack proved fragile in
//! captures (the reference server answers `02 0500` when too far and
//! `02 0b1c` when it thinks a session is already open, while a stale session
//! it never closed would otherwise lock the dialog shut). The ack is
//! logging-only. Deselecting the NPC (Esc, clicking another target) or
//! walking away ends the session with a 0x704B close. The Store/Teleport
//! dialog options only emit [`OpenStore`] / [`OpenTeleport`] messages — the
//! store and teleport windows consume them, keeping the dialog decoupled
//! from what the options open.

use bevy::prelude::*;

use packets::agent::prelude::{CloseTalkRequest, TalkResponse};
use packets::Packet;

use crate::net::connection::SilkroadConnection;
use crate::plugins::cursor::interactions::entity_select::SelectedEntity;
use crate::plugins::cursor::interactions::npcs::TalkStarted;
use crate::plugins::net::agent::AgentConnection;
use crate::plugins::net::entities::NetworkId;
use crate::plugins::player::Player;

/// Walking this far (render units) from the NPC ends the session, matching
/// vanilla's auto-close.
const WALK_AWAY_DISTANCE: f32 = 80.0;

/// Which page of the dialog is showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogPage {
    /// Greeting + option list.
    Options,
    /// The "Talk to this person." chat page.
    Talk,
}

#[derive(Resource, Default, Clone, Debug, PartialEq)]
pub enum NpcDialogState {
    #[default]
    Closed,
    Open {
        npc: Entity,
        page: DialogPage,
    },
}

impl NpcDialogState {
    pub fn npc(&self) -> Option<Entity> {
        match self {
            NpcDialogState::Closed => None,
            NpcDialogState::Open { npc, .. } => Some(*npc),
        }
    }
}

/// "Open this NPC's store window" — consumed by `hud::store`.
#[derive(Message)]
pub struct OpenStore {
    pub npc: Entity,
}

/// "Open this NPC's teleport window" — consumed by the teleport dialog.
#[derive(Message)]
pub struct OpenTeleport {
    pub npc: Entity,
}

/// "Open this NPC's storage window" — consumed by `hud::storage`.
#[derive(Message)]
pub struct OpenStorage {
    pub npc: Entity,
}

/// "Open this NPC's GUILD storage" — consumed by `hud::guild_storage`.
/// A separate message rather than a flag on [`OpenStorage`]: the two are
/// different opcode families (0x703C vs 0x7250) with different gates, and the
/// warehouse NPC offers both options side by side.
#[derive(Message)]
pub struct OpenGuildStorage {
    pub npc: Entity,
}

/// The talk request went out — open the dialog immediately (its content is
/// all client-side data; the server ack is informational).
pub fn on_talk_started(mut started: MessageReader<TalkStarted>, mut state: ResMut<NpcDialogState>) {
    for msg in started.read() {
        *state = NpcDialogState::Open {
            npc: msg.npc,
            page: DialogPage::Options,
        };
    }
}

/// Log the 0xB046 ack. Known codes on the reference server (capture-verified
/// 2026-08-06): success `01 01` (tail = echoed talk flag), `02 0500` = too
/// far away, `02 0b1c` = a talk session is already open server-side.
pub fn on_talk_response(mut acks: MessageReader<TalkResponse>) {
    for ack in acks.read() {
        match ack {
            TalkResponse::Success { tail } => {
                debug!(
                    "npc talk: 0xB046 success (tail {})",
                    packets::hexdump(tail, 16)
                );
            }
            TalkResponse::Failure(code) => {
                debug!("npc talk: 0xB046 rejection (code {code:#06x})");
            }
            TalkResponse::Unknown { result, tail } => {
                warn!(
                    "npc talk: unknown 0xB046 shape result={result:#04x} tail={} — capture for decode",
                    packets::hexdump(tail, 16)
                );
            }
        }
    }
}

/// End the session when the NPC is deselected (Esc, another target) or
/// despawns.
pub fn close_on_deselect(
    selected: Res<SelectedEntity>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    ids: Query<&NetworkId>,
    mut state: ResMut<NpcDialogState>,
) {
    let Some(npc) = state.npc() else {
        return;
    };
    if selected.0 != Some(npc) || !ids.contains(npc) {
        send_close(&conn, ids.get(npc).ok());
        *state = NpcDialogState::Closed;
    }
}

/// End the session when the player walks out of range.
pub fn close_on_walk_away(
    players: Query<&Transform, With<Player>>,
    npcs: Query<(&Transform, &NetworkId), Without<Player>>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    mut state: ResMut<NpcDialogState>,
) {
    let NpcDialogState::Open { npc, .. } = *state else {
        return;
    };
    let (Ok(player), Ok((npc_tf, id))) = (players.single(), npcs.get(npc)) else {
        return;
    };
    if player.translation.xz().distance(npc_tf.translation.xz()) > WALK_AWAY_DISTANCE {
        debug!("npc talk: walked away, closing dialog");
        send_close(&conn, Some(id));
        *state = NpcDialogState::Closed;
    }
}

/// Send the 0x704B close (best-effort; offline previews have no connection).
pub fn send_close(
    conn: &Query<&SilkroadConnection, With<AgentConnection>>,
    id: Option<&NetworkId>,
) {
    if let Some(id) = id {
        send_close_to(conn, id.0);
    }
}

/// Same, for callers that already know the NPC's network id (the store and
/// storage sessions keep it, so they need no entity lookup).
pub fn send_close_to(conn: &Query<&SilkroadConnection, With<AgentConnection>>, unique_id: u32) {
    let Ok(conn) = conn.single() else {
        return;
    };
    let request = CloseTalkRequest { unique_id };
    if let Err(e) = conn.get_sender().send(Packet::from(request).into()) {
        error!("network: failed to send CloseTalkRequest: {}", e.0);
    }
}
