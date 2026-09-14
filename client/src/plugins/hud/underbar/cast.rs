//! Quickslot activation → a fire-and-forget request for the owning plugin.
//!
//! Idea: the underbar's UI handlers never talk to the network themselves —
//! they translate a slot activation into an intent message. A skill slot emits
//! a `CastRequest` (target = the currently selected entity), turned into the
//! 0x7074 packet (or the local simulation offline) by
//! `skills::cast::dispatch_casts`. An item slot emits a [`UseItemRequest`];
//! [`dispatch_item_use`] resolves it against the live inventory + itemdata and
//! sends the 0x704C ItemUseRequest. Fire-and-forget either way: the server's
//! ack (0xB074.. for skills, 0xB04C for items) is logged/handled elsewhere,
//! never used to gate UI state.

use bevy::prelude::*;

use packets::agent::inventory::item_use_error_message;
use packets::agent::prelude::{ItemUseRequest, ItemUseResponse};
use packets::Packet;

use crate::net::connection::SilkroadConnection;
use crate::plugins::config::ClientConfig;
use crate::plugins::cursor::interactions::entity_select::SelectedEntity;
use crate::plugins::effects::EffectCommandsExt;
use crate::plugins::hud::chat::model::{ChatHistory, ChatLine};
use crate::plugins::hud::toast::{ShowToast, ToastKind};
use crate::plugins::hud::underbar::model::SlotAction;
use crate::plugins::net::agent::AgentConnection;
use crate::plugins::net::inventory::Inventory;
use crate::plugins::player::Player;
use crate::plugins::skills::cast::CastRequest;
use crate::plugins::textdata::{ClientItemData, ClientUiStrings};

/// What the player is told when the #215 gate swallows an item-use.
///
/// **Invented copy, stated deviation (ADR 0009):** the original has no such
/// message because the original's server implements `0x704C`. No `UIIT_MSG_*`
/// id covers "this server does not implement item use", and inventing a string
/// *id* would be the defect — so this is plain text with its rationale. It
/// disappears with the gate, once a server that answers `0x704C` exists.
pub const ITEM_USE_DISABLED_NOTICE: &str =
    "Item use is disabled for this server (no 0x704C support — see issue #215).";

/// Use an item, identified by its ref id. Emitted by the underbar and by
/// right-clicking an inventory slot; resolved to a 0x704C send by
/// [`dispatch_item_use`].
#[derive(Message)]
pub struct UseItemRequest {
    pub ref_id: u32,
    /// The exact slot to use, when the caller knows it (an inventory click
    /// does; a quickslot only knows the ref). Without it the first slot
    /// holding `ref_id` is used, which picks the wrong one when two slots
    /// hold the same item.
    pub slot: Option<u8>,
    /// The slot of the item this one is used **on** — set only by the
    /// arm-click-confirm flow in `hud::inventory::use_on_item` (pet revival
    /// and its siblings). Its presence is what selects the `WithSlot` body
    /// over `Simple`.
    pub target_slot: Option<u8>,
}

/// Fire a quickslot action: skills emit a [`CastRequest`], items a
/// [`UseItemRequest`] — both fire-and-forget intents resolved by their
/// dispatch systems.
pub fn activate_slot(
    action: SlotAction,
    selected: &SelectedEntity,
    casts: &mut MessageWriter<CastRequest>,
    item_uses: &mut MessageWriter<UseItemRequest>,
) {
    match action {
        SlotAction::Skill { ref_id } => {
            casts.write(CastRequest {
                skill_id: ref_id,
                target: selected.0,
            });
        }
        SlotAction::Item { ref_id } => {
            item_uses.write(UseItemRequest {
                ref_id,
                slot: None,
                target_slot: None,
            });
        }
    }
}

/// What we last sent, so the server's answer can be attributed to it.
///
/// The 0xB04C failure branch carries **only a code** — no slot, no ref id — so
/// without this an error cannot be named in the log or to the player.
///
/// **This used to also block retries for 30 s** after any refusal, because a
/// live vSRO session answered a COS summon with an error and then *reset the
/// connection* when the same request was repeated. The block is gone by
/// request: it made experimenting with a refusing item impossible, and the one
/// refusal it was built for turned out to be server state that never clears
/// (see `docs/net-item-use-0x704c.md`, the 0x18A5 timeline). The exposure it
/// covered is real but is the server's to fix; the client should not silently
/// swallow the player's clicks for half a minute.
#[derive(Resource, Default)]
pub struct ItemUseGate {
    /// Ref id of the request awaiting an ack.
    in_flight: Option<u32>,
}

/// Pack an item's four TypeInfo ids into the 0x704C `type_id` word.
///
/// Layout per `docs/net-item-use-0x704c.md` (SPEC-correct, Hyperbot/SilkroadDoc):
/// `| TID4(5b@11) | TID3(4b@7) | TID2(2b@5) | TID1(3b@2) | 00 |`, the low 2 bits
/// (item BindItem/CashItem) zero.
fn pack_type_id(tid1: u32, tid2: u32, tid3: u32, tid4: u32) -> u16 {
    ((tid1 << 2) | (tid2 << 5) | (tid3 << 7) | (tid4 << 11)) as u16
}

/// The inverse of [`pack_type_id`] — recover the four ids from the word.
///
/// Used by the item-use effect, which needs to know *what* was consumed. It
/// reads the ack's own `type_id` rather than remembering what was sent: the
/// server's answer is the authority on which item it actually consumed, and it
/// needs no ordering against `ItemUseGate`'s in-flight bookkeeping (which the
/// success path clears).
fn unpack_type_id(word: u16) -> (u32, u32, u32, u32) {
    let word = word as u32;
    (
        (word >> 2) & 0b111,
        (word >> 5) & 0b11,
        (word >> 7) & 0b1111,
        (word >> 11) & 0b1_1111,
    )
}

/// Resolve a [`UseItemRequest`] to the 0x704C ItemUseRequest and send it: find
/// the item's current slot in the live [`Inventory`] (which tracks
/// server-confirmed moves/consumption, so the slot stays correct after the
/// login snapshot) and pack its TypeInfo word from itemdata. Fire-and-forget —
/// the authoritative stack decrement rides the 0xB04C ack's inventory update.
///
/// GATED (#215): 0x704C is unsupported on the maintainer's go-sro build — it
/// resets the connection on receipt (in any body form). The send is behind the
/// `network_settings.item_use_enabled` config flag (default off); enable it only
/// against a server that implements 0x704C.
#[allow(clippy::too_many_arguments)]
pub fn dispatch_item_use(
    mut requests: MessageReader<UseItemRequest>,
    inventories: Query<&Inventory, With<Player>>,
    characters: Query<&crate::plugins::net::character_info::CharacterInfo, With<Player>>,
    item_data: Res<ClientItemData>,
    config: Res<ClientConfig>,
    mut gate: ResMut<ItemUseGate>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    mut history: ResMut<ChatHistory>,
) {
    for request in requests.read() {
        let Ok(inventory) = inventories.single() else {
            warn!(
                "underbar: no player inventory to use item {}",
                request.ref_id
            );
            continue;
        };
        // The caller's slot when it knows one (an inventory right-click),
        // else the first slot holding this item — fine for the consumables
        // EP-07 targets (a stack of potions occupies a single slot).
        let Some(item) = request
            .slot
            .and_then(|slot| inventory.get(slot))
            .filter(|item| item.ref_id == request.ref_id)
            .or_else(|| {
                inventory
                    .slots
                    .iter()
                    .flatten()
                    .find(|item| item.ref_id == request.ref_id)
            })
        else {
            info!("underbar: item ref {} not in inventory", request.ref_id);
            continue;
        };
        let Some((t1, t2, t3, t4)) = item_data
            .get(&(request.ref_id as i32))
            .and_then(|row| row.type_ids())
        else {
            warn!(
                "underbar: no itemdata type ids for item ref {}",
                request.ref_id
            );
            continue;
        };
        // (A refused item used to be blocked here for 30s; see `ItemUseGate`
        // for why that is gone. Every click now reaches the server, which is
        // the only thing that actually knows whether the use is legal.)
        // Level requirement (itemdata ReqLevel1): the server refuses these
        // anyway, so catching it here turns a wire refusal into a clear reason.
        let required = item_data
            .get(&(request.ref_id as i32))
            .and_then(|row| row.required_level());
        let level = characters
            .single()
            .ok()
            .and_then(|info| info.stats.as_ref())
            .map(|stats| stats.level as u32);
        if let (Some(required), Some(level)) = (required, level) {
            if level < required {
                // Say it where the player can see it, for the same reason the
                // #215 gate below does: a refusal that only reaches the log is
                // indistinguishable from a broken client. This gate used to be
                // `info!`-only, three lines above a comment stating exactly
                // that principle.
                let notice = format!("You need level {required} to use this item.");
                info!("underbar: not using item {} — {notice}", request.ref_id);
                history.push(ChatLine::system(notice));
                continue;
            }
        }
        let Ok(conn) = conn.single() else {
            warn!("underbar: not using item, no agent connection");
            continue;
        };
        // `item.slot` is the server's own slot number, where 0..12 are the
        // equipment slots and the bag starts at 13 — i.e. already the wire slot
        // the original produces by adding `+0x0D` to its bag-relative index.
        // Adding the bias here would land 13 slots past the item, and the same
        // holds for `target_slot`.
        //
        // `WithSlot` when the caller named a target item (the revival flow),
        // else `Simple` — the no-tail class the underbar can drive (potions,
        // pills, plain scrolls). `WithTarget` / `WithName` still need a target
        // entity or a destination name no caller carries yet (#454).
        let type_id = pack_type_id(t1, t2, t3, t4);
        let req = match request.target_slot {
            Some(target_slot) => ItemUseRequest::WithSlot {
                slot: item.slot,
                type_id,
                target_slot,
            },
            None => ItemUseRequest::Simple {
                slot: item.slot,
                type_id,
            },
        };
        if config.network_settings.item_use_enabled {
            info!(
                "underbar: using item ref {} (slot {}, type_id {:#06x}) (0x704C)",
                request.ref_id,
                req.slot(),
                req.type_id()
            );
            // Recorded only once the send actually succeeded. Setting it first
            // left `in_flight` poisoned by a failed send, and the next
            // unrelated 0xB04C error was then attributed to an item that never
            // reached the wire.
            match conn.get_sender().send(Packet::from(req).into()) {
                Ok(()) => gate.in_flight = Some(request.ref_id),
                Err(e) => error!("network: failed to send ItemUseRequest: {}", e.0),
            }
        } else {
            info!(
                "underbar: item-use not sent — 0x704C unsupported on this server (#215); ref {} slot {}",
                request.ref_id, req.slot()
            );
            // A gate the player cannot see is indistinguishable from a broken
            // client: the same "I pressed it and nothing happened" report the
            // death window earned (#236/#143). Say it in chat, once per press.
            history.push(ChatLine::system(ITEM_USE_DISABLED_NOTICE));
        }
    }
}

/// Log the server's 0xB04C item-use result for the capture-verification loop.
/// Cooldown UI is out of scope (#135/EP-07); the stack decrement rides the
/// ack's own inventory update.
pub fn log_item_use_response(
    mut reader: MessageReader<ItemUseResponse>,
    ui_strings: Option<Res<ClientUiStrings>>,
    mut gate: ResMut<ItemUseGate>,
    mut history: ResMut<ChatHistory>,
    mut toasts: MessageWriter<ShowToast>,
) {
    for response in reader.read() {
        match response {
            ItemUseResponse::Success {
                slot,
                remaining,
                type_id,
            } => {
                info!(
                    "item use ok: slot {slot}, {remaining} left, type_id {type_id:#06x} (0xB04C)"
                );
                gate.in_flight = None;
            }
            ItemUseResponse::Error { code } => {
                // The code table is non-exhaustive and one documented label is
                // already refuted (0x1889 — see `item_use_error_message`), so
                // the raw code always goes to the log even when we can name it.
                warn!(
                    "item use rejected: code {code:#06x} for item {:?} (0xB04C)",
                    gate.in_flight.take(),
                );
                // A rejection the player cannot see is indistinguishable from
                // a broken client — the same lesson the #215 gate above and
                // the death window (#236/#143) already learned.
                //
                // Where the archive gives us the vanilla wording, use it. Where
                // it does not, say so with the raw code rather than staying
                // silent: half the codes seen live are unmapped (0x18A5 alone
                // accounts for 7 of 11 refusals in the capture), and silence
                // there is exactly the "I clicked and nothing happened" report.
                // The code is what the player can quote back to us.
                let named = item_use_error_message(*code).and_then(|key| {
                    ui_strings
                        .as_deref()
                        .and_then(|strings| strings.get(key))
                        .map(str::to_string)
                });
                let text = named
                    .unwrap_or_else(|| format!("The server refused this item (code {code:#06x})."));
                toasts.write(ShowToast::new(ToastKind::Notice, text.clone()));
                history.push(ChatLine::system(text));
            }
        }
    }
}

/// Play the configured particle effect on the player when the server confirms
/// an item use (0xB04C success).
///
/// Idea: the effect is attached to the player's **body wrapper**, so it follows
/// the character and anchors where the model is — the same anchor and the same
/// `TimedEffect` lifetime the level-up flash uses
/// (`combat::on_entity_level_up`), because these are the same kind of thing: a
/// fire-and-forget burst on a character, with no cleanup of its own.
///
/// **What plays is entirely `effects.item_use`'s business** (see
/// [`EffectSettings`](crate::plugins::config::effects::EffectSettings)): no
/// archive file maps a *consumable* to an effect, so the table is derived by
/// pairing effect filenames with captured type ids, and an unlisted item plays
/// nothing. The `info!` on that path is the discovery aid — it names the exact
/// key to add, once per type id so a spammed hotkey cannot flood the log.
///
/// A separate reader from [`log_item_use_response`] rather than a branch inside
/// it: that system is already at a comfortable parameter count, and the two
/// have nothing to say to each other.
pub fn play_item_use_effect(
    mut reader: MessageReader<ItemUseResponse>,
    config: Option<Res<ClientConfig>>,
    players: Query<(Entity, &Children), With<Player>>,
    wrappers: Query<(), With<crate::commands::SpawnedFromResource>>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
    mut announced: Local<std::collections::HashSet<(u32, u32, u32, u32)>>,
) {
    let Some(config) = config else {
        return;
    };
    for response in reader.read() {
        let ItemUseResponse::Success { type_id, .. } = response else {
            continue;
        };
        let type_ids = unpack_type_id(*type_id);
        // Owned: `AssetServer::load` needs a `'static` path, and the config is
        // only borrowed for this system's run.
        let paths: Option<Vec<String>> = config
            .effects
            .item_use_effects(type_ids)
            .map(<[String]>::to_vec);
        let Some(paths) = paths else {
            // Loud enough to be seen without `RUST_LOG=debug` — an item that
            // silently plays nothing is indistinguishable from a broken effect,
            // which is how the universal pill's missing row went unnoticed. Once
            // per type id, so holding a hotkey does not flood the log.
            if announced.insert(type_ids) {
                let (a, b, c, d) = type_ids;
                info!(
                    "item use: no effect configured for type id {a}.{b}.{c}.{d} — \
                     add `effects.item_use: {{ \"{a}.{b}.{c}.{d}\": \"particles://...\" }}` \
                     to config.yaml to play one"
                );
            }
            continue;
        };
        let Ok((player, children)) = players.single() else {
            continue;
        };
        // The wrapper is where the model lives; falling back to the root keeps
        // the effect on the character rather than dropping it, for the frames
        // before the body has spawned.
        let anchor = children
            .iter()
            .find(|child| wrappers.contains(*child))
            .unwrap_or(player);
        // A row may name several effects that play together — a cure is a burst
        // *and* a ring, and one without the other does not read as a cure. Each
        // gets its own wrapper, the way the skill path spawns one per emission.
        for path in paths {
            debug!("item use: playing {path}");
            let effect =
                commands.attach_effect(asset_server.load(path), anchor, Transform::IDENTITY);
            // `OneShotEffect` is what stops the replay. Without it `lifespan_of`
            // gives the tree's static and sub-emitter nodes `Lifespan::Loop`, and
            // `tick_effect_nodes` wraps their age at the node's own period — so a
            // ~1s `item_hpotion.efp` inside a 2s wrapper visibly plays twice. Same
            // defect the skills path already carries the marker for
            // (`skills::status`), and the reason a potion looked doubled.
            commands.entity(effect).insert((
                crate::plugins::combat::TimedEffect::new(config.effects.item_use_seconds),
                crate::plugins::effects::OneShotEffect,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The item-use effect picks its table entry from the ack's own `type_id`,
    /// so the unpack has to be the exact inverse of the pack — a shifted field
    /// would look up the wrong effect for every item rather than failing.
    #[test]
    fn the_type_id_word_round_trips() {
        // one from each family, at the top of each field's range
        for ids in [
            (3u32, 3u32, 1u32, 1u32), // HP potion
            (3, 3, 3, 2),             // COS summon scroll
            (3, 1, 6, 2),             // sword
            (3, 3, 15, 31),           // maxima of TID3 (4b) and TID4 (5b)
            (7, 3, 0, 0),             // maxima of TID1 (3b) and TID2 (2b)
        ] {
            let (a, b, c, d) = ids;
            assert_eq!(unpack_type_id(pack_type_id(a, b, c, d)), ids, "{ids:?}");
        }
    }

    /// The #215 gate must never be silent. Pin the notice's substance rather
    /// than its wording: it has to name the gate (so a player knows the client
    /// is not broken) and carry the issue, because it is invented copy that has
    /// to be deletable the day a server answers 0x704C.
    #[test]
    fn the_gated_item_use_notice_explains_itself() {
        assert!(ITEM_USE_DISABLED_NOTICE.contains("0x704C"));
        assert!(ITEM_USE_DISABLED_NOTICE.contains("#215"));
        // It goes out as a system line, which is what the All tab renders
        // unprefixed — no "sender:" glued in front of it.
        let line = ChatLine::system(ITEM_USE_DISABLED_NOTICE);
        assert_eq!(line.display(), ITEM_USE_DISABLED_NOTICE);
    }
}
