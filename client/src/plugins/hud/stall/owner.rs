//! Owner half of the player stall (#781): open a titled stall, switch it
//! between selling and modifying, and close it again.
//!
//! Idea, and it is the same one the buyer half runs on (`net.rs`): the server
//! owns the stall. Nothing here mutates [`StallState`] optimistically —
//! `0x70B1` create, `0x70BA` update and `0x70B2` destroy each leave the window
//! exactly as it was until the matching ack (`0xB0B1` / `0xB0BA` / `0xB0B2`)
//! says otherwise. The one thing we hold locally is the title we *asked* for,
//! parked in [`RequestedStallTitle`] until the create ack accepts it, because
//! `0xB0B1` does not echo it back.
//!
//! **Entry point: the `/Stall` chat command.** `docs/re/systems/stall.md` §3
//! "Open (seller)" gives create as `0x70B1{title}` and nothing else, and the
//! original's own vocabulary already has the command
//! (`UIIT_STT_CHAT_COMMAND_STREETSTORE` = `/Stall`, textuisystem L669), which
//! `chat/input.rs` listed as unsupported purely because no request path
//! existed. It exists now, so the command is wired rather than answered with
//! `UIIT_CHATERR_INVALID_COMMAND` — that is the rule that file states for
//! itself.
//!
//! **Not here, and named rather than guessed:** stocking (add / re-price /
//! remove, `0x70BA` types 2/1/3) and the note/title *edit* buttons. All four
//! need a text-or-price entry box, and the only candidate in the data is
//! `MsgBoxStoreMoney` (`docs/re/ui/hud-stall-window.md` §3h), whose binding to
//! this window is explicitly `[S]` and refuted as a uniqueness argument in
//! §9-U6/U7. Wiring an invented dialog into the *money* path is the one place
//! in this window where a guess would cost the player gold. The acks for all
//! of them are implemented, so a stall stocked from anywhere else still
//! renders correctly here.

use bevy::prelude::*;
use bevy::ui_widgets::Activate;

use packets::agent::stall::{
    EntityStallDestroy, EntityStallTitleUpdate, StallCreateRequest, StallCreateResponse,
    StallDestroyRequest, StallDestroyResponse, StallUpdateAck, StallUpdateRequest,
    StallUpdateResponse, STALL_UPDATE_ITEM_ADDED, STALL_UPDATE_ITEM_REMOVED,
};
use packets::Packet;

use crate::net::connection::SilkroadConnection;
use crate::plugins::hud::player_mini_info::PlayerVitals;
use crate::plugins::hud::stall::model::{StallRow, StallState, StallTradingState};
use crate::plugins::hud::stall::net::{item_name, rows_to_slots, send, stall_error_text};
use crate::plugins::hud::stall::ui::STALL_SLOTS;
use crate::plugins::hud::toast::{ShowToast, ToastKind};
use crate::plugins::net::agent::AgentConnection;
use crate::plugins::net::entities::NetworkId;
use crate::plugins::player::Player;
use crate::plugins::textdata::{ClientItemData, ClientTextNames, ClientUiStrings};

/// `/Stall [title]` was typed. Carried as a message so the chat module keeps
/// knowing nothing about the stall wire.
#[derive(Message, Debug, Clone, PartialEq)]
pub struct OpenStallCommand {
    /// `None` when the command carried no title — the caller supplies the
    /// vanilla default.
    pub title: Option<String>,
}

/// The title we asked for, held until `0xB0B1` accepts it. `0xB0B1` is
/// `{result}` (+ an error tail) and echoes no title, so without this the
/// window would open nameless — and applying it before the ack would show a
/// title the server may have refused.
#[derive(Resource, Debug, Default, Clone, PartialEq)]
pub struct RequestedStallTitle(pub Option<String>);

/// `UIIT_STT_STALL_DEFAULT_TITLE` is `[%s]'s stall.`
/// (`docs/re/ui/hud-stall-window.md` §3c) — the `%s` is the owner's name.
const DEFAULT_TITLE_KEY: &str = "UIIT_STT_STALL_DEFAULT_TITLE";
const DEFAULT_TITLE_FALLBACK: &str = "[%s]'s stall.";

/// Fill the vanilla default title's single `%s` with the owner's name.
pub fn default_stall_title(template: &str, owner_name: &str) -> String {
    template.replacen("%s", owner_name, 1)
}

/// `0x70B1` — open a stall under this title.
pub fn on_open_stall_command(
    mut reader: MessageReader<OpenStallCommand>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    vitals: Res<PlayerVitals>,
    ui_strings: Res<ClientUiStrings>,
    state: Res<StallState>,
    mut requested: ResMut<RequestedStallTitle>,
    mut toasts: MessageWriter<ShowToast>,
) {
    for command in reader.read() {
        if state.owner {
            warn!("stall: /Stall while a stall is already open — ignored");
            toasts.write(ShowToast::new(ToastKind::Warning, "Stall: already open"));
            continue;
        }
        let title = command.title.clone().unwrap_or_else(|| {
            default_stall_title(
                ui_strings.get_or(DEFAULT_TITLE_KEY, DEFAULT_TITLE_FALLBACK),
                &vitals.name,
            )
        });
        info!("stall: opening a stall titled {title:?} (0x70B1)");
        requested.0 = Some(title.clone());
        send(&conn, Packet::from(StallCreateRequest { title }), "create");
    }
}

/// `0xB0B1` — the create ack. The window becomes ours only here.
pub fn on_stall_create_response(
    mut reader: MessageReader<StallCreateResponse>,
    mut state: ResMut<StallState>,
    mut requested: ResMut<RequestedStallTitle>,
    mut toasts: MessageWriter<ShowToast>,
) {
    for response in reader.read() {
        if response.result != 1 {
            let text = match response.error_code {
                Some(code) => stall_error_text(code),
                None => "Stall: could not open the stall".to_string(),
            };
            warn!("stall: create refused — {text}");
            toasts.write(ShowToast::new(ToastKind::Warning, text));
            requested.0 = None;
            continue;
        }
        let title = requested.0.take().unwrap_or_default();
        info!("stall: our stall is open, titled {title:?} (0xB0B1)");
        state.open = true;
        state.owner = true;
        // A fresh stall starts in the owner's edit state: the original follows
        // create with the note packet and the type-5 "go on sale" only later
        // (`docs/re/systems/stall.md` §3 "Open (seller)").
        state.trading = StallTradingState::Modifying;
        state.title = title;
        state.slots = vec![None; STALL_SLOTS];
    }
}

/// `0xB0B2` — the destroy ack. The window closes here, never on the click.
pub fn on_stall_destroy_response(
    mut reader: MessageReader<StallDestroyResponse>,
    mut state: ResMut<StallState>,
    mut toasts: MessageWriter<ShowToast>,
) {
    for response in reader.read() {
        if response.result == 1 {
            info!("stall: our stall is closed (0xB0B2)");
            *state = StallState::default();
            continue;
        }
        let text = match response.error_code {
            Some(code) => stall_error_text(code),
            None => "Stall: could not close the stall".to_string(),
        };
        warn!("stall: destroy refused — {text}");
        toasts.write(ShowToast::new(ToastKind::Warning, text));
    }
}

/// What a `0xB0BA` does to the window, decoded once so the branching is
/// testable headless. `rows` is the caller's already-decoded row list (the
/// decode needs an item resolver, which a pure function must not carry).
pub fn apply_update_ack(
    state: &mut StallState,
    response: &StallUpdateResponse,
    rows: Option<Vec<Option<StallRow>>>,
) -> Option<u16> {
    match &response.body {
        StallUpdateAck::ItemUpdate {
            stall_slot,
            quantity,
            price,
            error_code,
        } => {
            if response.result != 1 {
                return Some(*error_code);
            }
            // The ack re-states the row's own numbers; a slot the window does
            // not know about is a capacity fact refuted, not a row to invent.
            if let Some(Some(row)) = state.slots.get_mut(*stall_slot as usize) {
                row.quantity = *quantity;
                row.price = *price;
            } else {
                warn!("stall: 0xB0BA ItemUpdate for slot {stall_slot}, which holds no row");
            }
            None
        }
        StallUpdateAck::ItemList { error_code, .. } => {
            if response.result != 1 {
                return Some(*error_code);
            }
            // Types 2 and 3 re-send the WHOLE list, so it replaces the grid
            // (`docs/re/systems/stall.md` §3).
            if let Some(rows) = rows {
                state.slots = rows;
            } else {
                warn!("stall: 0xB0BA add/remove rows did not decode — grid left unchanged");
            }
            None
        }
        StallUpdateAck::State {
            is_open,
            stall_network_result,
        } => {
            if response.result != 1 {
                return Some(*stall_network_result);
            }
            state.trading = if *is_open == 1 {
                StallTradingState::Open
            } else {
                StallTradingState::Modifying
            };
            None
        }
        StallUpdateAck::Note { note } => {
            if response.result == 1 {
                state.greeting = note.clone();
            }
            None
        }
        // Type 7 carries no payload: the new title arrives on 0x30BB instead
        // (`docs/net-stall-0x30B7.md`, 0xB0BA type 7).
        StallUpdateAck::Title => None,
        StallUpdateAck::FleaMarketMode { mode } => {
            debug!("stall: flea-market mode {mode} (0xB0BA type 4)");
            None
        }
        StallUpdateAck::Unknown { .. } => {
            warn!(
                "stall: 0xB0BA update type {} is described by neither source — ignored",
                response.update_type
            );
            None
        }
    }
}

/// `0xB0BA` — an owner edit was applied (or refused).
pub fn on_stall_update_response(
    mut reader: MessageReader<StallUpdateResponse>,
    item_data: Res<ClientItemData>,
    names: Res<ClientTextNames>,
    mut state: ResMut<StallState>,
    mut toasts: MessageWriter<ShowToast>,
) {
    for response in reader.read() {
        let rows = match response.update_type {
            STALL_UPDATE_ITEM_ADDED | STALL_UPDATE_ITEM_REMOVED => response
                .rows(&*item_data)
                .map(|rows| rows_to_slots(&rows, |ref_id| item_name(&item_data, &names, ref_id))),
            _ => None,
        };
        if let Some(code) = apply_update_ack(&mut state, response, rows) {
            let text = stall_error_text(code);
            warn!("stall: edit refused — {text}");
            toasts.write(ShowToast::new(ToastKind::Warning, text));
        }
    }
}

/// `0x30BB` — a stall was renamed. Ours is the one on our own spawn id; every
/// other one belongs to a stall in the world (#782's nameplate half).
pub fn on_entity_stall_title_update(
    mut reader: MessageReader<EntityStallTitleUpdate>,
    own: Query<&NetworkId, With<Player>>,
    mut state: ResMut<StallState>,
) {
    let Some(own_unique_id) = own.iter().next().map(|id| id.0) else {
        return;
    };
    for update in reader.read() {
        if update.unique_id != own_unique_id {
            continue;
        }
        info!(
            "stall: our stall was renamed to {:?} (0x30BB)",
            update.title
        );
        state.title = update.title.clone();
    }
}

/// `0x30B9` — a stall vanished. If it is ours, the window goes with it: the
/// server can close a stall without our `0x70B2` (dismount, teleport, a GM).
pub fn on_entity_stall_destroy(
    mut reader: MessageReader<EntityStallDestroy>,
    own: Query<&NetworkId, With<Player>>,
    mut state: ResMut<StallState>,
) {
    let Some(own_unique_id) = own.iter().next().map(|id| id.0) else {
        return;
    };
    for destroy in reader.read() {
        if destroy.unique_id != own_unique_id || !state.owner {
            continue;
        }
        info!("stall: our stall was removed by the server (0x30B9)");
        *state = StallState::default();
    }
}

/// `0x70B2` — close our own stall.
pub fn send_destroy(conn: &Query<&SilkroadConnection, With<AgentConnection>>) {
    send(conn, Packet::from(StallDestroyRequest), "destroy");
}

/// `0x70BA` type 5 — put the stall on sale (`is_open = 1`) or take it back to
/// the edit state (`0`).
pub fn send_state(conn: &Query<&SilkroadConnection, With<AgentConnection>>, is_open: bool) {
    send(
        conn,
        Packet::from(StallUpdateRequest::State {
            is_open: u8::from(is_open),
            // The original writes a trailing u16 on this arm and go-sro reads
            // it back as `stall_network_result`; it is the client's own value,
            // and 0 is what a stall that is not using the ware network sends
            // (`docs/net-stall-0x30B7.md`, 0x70BA type 5).
            unknown0: 0,
        }),
        "state",
    );
}

/// The `GDR_BTN_TRADINGSTATE` button: it is the owner's on-sale switch, and it
/// only exists for the owner — a visitor's click must not send a type-5 for
/// somebody else's stall.
pub fn on_trading_state_button(
    _: On<Activate>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    state: Res<StallState>,
) {
    if !state.owner {
        return;
    }
    // The button asks for the state we are NOT in; the window flips only on
    // the 0xB0BA type-5 ack.
    let wanted_open = state.trading == StallTradingState::Modifying;
    info!("stall: asking for trading state open={wanted_open} (0x70BA type 5)");
    send_state(&conn, wanted_open);
}

#[cfg(test)]
mod test {
    use super::*;
    use packets::agent::stall::{STALL_UPDATE_NOTE, STALL_UPDATE_STATE};

    fn owned_stall() -> StallState {
        StallState {
            open: true,
            owner: true,
            title: "[Foo]'s stall.".into(),
            ..default()
        }
    }

    fn ack(update_type: u8, body: StallUpdateAck, result: u8) -> StallUpdateResponse {
        StallUpdateResponse {
            result,
            update_type,
            body,
        }
    }

    /// The vanilla default title is a template with one `%s`, not a sentence
    /// we compose: `[%s]'s stall.` with the owner's name in it.
    #[test]
    fn the_default_stall_title_fills_the_templates_single_placeholder() {
        assert_eq!(
            default_stall_title(DEFAULT_TITLE_FALLBACK, "Foo"),
            "[Foo]'s stall."
        );
        // a template without the placeholder is used as-is, never appended to
        assert_eq!(default_stall_title("My stall", "Foo"), "My stall");
    }

    /// Type 5 is the on-sale switch, and the window follows the SERVER's
    /// value — flipping locally on the click is what would desync a stall the
    /// server refused to open.
    #[test]
    fn the_type_five_ack_is_what_flips_the_trading_state() {
        let mut state = owned_stall();
        assert_eq!(state.trading, StallTradingState::Open);

        let error = apply_update_ack(
            &mut state,
            &ack(
                STALL_UPDATE_STATE,
                StallUpdateAck::State {
                    is_open: 0,
                    stall_network_result: 0,
                },
                1,
            ),
            None,
        );
        assert_eq!(error, None);
        assert_eq!(state.trading, StallTradingState::Modifying);

        let error = apply_update_ack(
            &mut state,
            &ack(
                STALL_UPDATE_STATE,
                StallUpdateAck::State {
                    is_open: 1,
                    stall_network_result: 0,
                },
                1,
            ),
            None,
        );
        assert_eq!(error, None);
        assert_eq!(state.trading, StallTradingState::Open);
    }

    /// A refused edit reports its code and changes nothing.
    #[test]
    fn a_refused_edit_leaves_the_window_alone_and_yields_its_code() {
        let mut state = owned_stall();
        state.greeting = "hello".into();
        let error = apply_update_ack(
            &mut state,
            &ack(
                STALL_UPDATE_NOTE,
                StallUpdateAck::Note {
                    note: "spam".into(),
                },
                2,
            ),
            None,
        );
        assert_eq!(error, None, "the note arm carries no error code of its own");
        assert_eq!(
            state.greeting, "hello",
            "a refused note must not be applied"
        );

        let error = apply_update_ack(
            &mut state,
            &ack(
                STALL_UPDATE_STATE,
                StallUpdateAck::State {
                    is_open: 1,
                    stall_network_result: 0x3C0E,
                },
                2,
            ),
            None,
        );
        assert_eq!(error, Some(0x3C0E));
        assert_eq!(
            state.trading,
            StallTradingState::Open,
            "a refused state change must not move the badge"
        );
    }

    /// Add/remove re-send the whole listing, so the ack REPLACES the grid
    /// rather than merging into it — a merge would keep the row the owner
    /// just removed.
    #[test]
    fn an_item_list_ack_replaces_the_whole_grid() {
        let mut state = owned_stall();
        state.slots[0] = Some(StallRow {
            name: "old".into(),
            quantity: 1,
            price: 10,
        });
        let mut replacement = vec![None; STALL_SLOTS];
        replacement[4] = Some(StallRow {
            name: "new".into(),
            quantity: 3,
            price: 99,
        });
        let error = apply_update_ack(
            &mut state,
            &ack(
                STALL_UPDATE_ITEM_ADDED,
                StallUpdateAck::ItemList {
                    error_code: 0,
                    raw_rows: Default::default(),
                },
                1,
            ),
            Some(replacement),
        );
        assert_eq!(error, None);
        assert!(state.slots[0].is_none(), "the old row is gone, not merged");
        assert_eq!(state.slots[4].as_ref().map(|r| r.price), Some(99));
    }

    /// Type 1 re-prices a row in place; a type 1 for an empty slot must not
    /// conjure a row out of quantity and price alone (it carries no item).
    #[test]
    fn an_item_update_ack_reprices_a_row_and_never_invents_one() {
        let mut state = owned_stall();
        state.slots[2] = Some(StallRow {
            name: "sword".into(),
            quantity: 1,
            price: 10,
        });
        apply_update_ack(
            &mut state,
            &ack(
                1,
                StallUpdateAck::ItemUpdate {
                    stall_slot: 2,
                    quantity: 5,
                    price: 4200,
                    error_code: 0,
                },
                1,
            ),
            None,
        );
        assert_eq!(state.slots[2].as_ref().map(|r| r.price), Some(4200));
        assert_eq!(state.slots[2].as_ref().map(|r| r.quantity), Some(5));
        assert_eq!(
            state.slots[2].as_ref().map(|r| r.name.clone()),
            Some("sword".into()),
            "the ack carries no item, so the name must survive it"
        );

        apply_update_ack(
            &mut state,
            &ack(
                1,
                StallUpdateAck::ItemUpdate {
                    stall_slot: 7,
                    quantity: 1,
                    price: 1,
                    error_code: 0,
                },
                1,
            ),
            None,
        );
        assert!(state.slots[7].is_none());
    }
}
