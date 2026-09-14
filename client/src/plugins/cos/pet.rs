//! Pet commands and their acks — the half of the COS family that is *not*
//! riding.
//!
//! Idea: `riding.rs` owns everything a transport does (board, slave, tilt); a
//! pet does none of that and instead takes orders — attack, pick, follow,
//! rename, terminate, and the offensive/defensive AI switch. Both halves talk
//! to the same 0x70C5 command envelope, so the split is by *subject*, not by
//! opcode: this module holds the pet-shaped commands plus every COS ack the
//! server answers them with.
//!
//! What the acks are for: 0xB0C5 echoes the action byte we sent, so it is the
//! only way to learn whether an action code we inferred rather than recovered
//! (`PET_ACTION_FOLLOW`, `[S]`) is real. Logging its verdict is the point —
//! `docs/re/CAPTURE_LIST.md` group F closes on these lines.

use bevy::prelude::*;

use packets::agent::pet::{
    AttackPetSettings, CosKind, PetActionRequest, PetActionResponse, PetRenameRequest,
    PetRenameResponse, PetSettingsChangeRequest, PetSettingsChangeResponse, PetTerminateRequest,
    PetTerminateResponse, PetUnsummonResponse, StuckDistanceWarning, PET_ACTION_ATTACK,
    PET_ACTION_ITEM_PICKUP, PET_SETTINGS_TYPE_GOLD, STUCK_REASON_QUEST_MONSTER,
    STUCK_REASON_TRADE_CART,
};
use packets::Packet;

use crate::net::connection::SilkroadConnection;
use crate::plugins::hud::cos::state::CosState;
use crate::plugins::net::agent::AgentConnection;

/// UI → network pet orders. Kept separate from
/// [`CosCommand`](super::CosCommand), which is the riding channel.
#[derive(Message, Debug, Clone)]
pub enum PetCommand {
    /// Flip the attack pet between offensive and defensive AI (0x7420). The
    /// original's tooltip calls this "COS AI type", and the two share one key
    /// — it is a mode switch, not an on/off.
    SetAiMode { unique_id: u32, offensive: bool },
    /// Send a pet at a target (0x70C5 action 2).
    Attack {
        unique_id: u32,
        target_unique_id: u32,
    },
    /// Send a pick pet at a ground item (0x70C5 action 8).
    Pick { unique_id: u32, item_unique_id: u32 },
    /// Rename (0x7117). The new name comes back on 0x30C9 arm 5, never from
    /// the ack, so nothing is applied optimistically.
    Rename { unique_id: u32, name: String },
    /// End the summon outright (0x70C6), as distinct from putting it away.
    Terminate { unique_id: u32 },
}

/// Turn [`PetCommand`]s into wire packets.
pub fn handle_pet_commands(
    mut reader: MessageReader<PetCommand>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    mut state: ResMut<CosState>,
) {
    for command in reader.read() {
        match command {
            PetCommand::SetAiMode {
                unique_id,
                offensive,
            } => {
                // Mirror the requested flag locally so the bar lights up
                // immediately; 0xB420 is authoritative and overwrites it.
                let settings = if *offensive {
                    AttackPetSettings::OFFENSIVE
                } else {
                    0
                };
                if let Some(cos) = state.get_mut(*unique_id) {
                    cos.body.unk_f = Some(settings);
                }
                send(
                    &conn,
                    PetSettingsChangeRequest {
                        unique_id: *unique_id,
                        settings_type: PET_SETTINGS_TYPE_GOLD,
                        settings,
                    },
                );
            }
            PetCommand::Attack {
                unique_id,
                target_unique_id,
            } => send(
                &conn,
                PetActionRequest::Attack {
                    pet_unique_id: *unique_id,
                    target_unique_id: *target_unique_id,
                },
            ),
            PetCommand::Pick {
                unique_id,
                item_unique_id,
            } => send(
                &conn,
                PetActionRequest::ItemPickUp {
                    pet_unique_id: *unique_id,
                    item_unique_id: *item_unique_id,
                },
            ),
            PetCommand::Rename { unique_id, name } => send(
                &conn,
                PetRenameRequest {
                    unique_id: *unique_id,
                    name: name.clone(),
                },
            ),
            PetCommand::Terminate { unique_id } => send(
                &conn,
                PetTerminateRequest {
                    unique_id: *unique_id,
                },
            ),
        }
    }
}

/// Our choice, not the original's: when the player attacks something and an
/// attack pet is out **in offensive mode**, the pet is sent at the same target.
///
/// The original drives this from a COS command mode in its target UI — the
/// same builders emit 0x7074 or 0x70C5 depending on which subject is selected
/// (`docs/re/net/outbound/pet-cos.md` §0x70C5). We have no such mode, and
/// inventing a modal targeting UI would be a larger deviation than this one;
/// "offensive pet joins your fight" is what the mode's own name promises. A
/// defensive pet is deliberately left alone, which is the switch's whole
/// purpose.
pub fn send_pet_attack_with_player(
    mut orders: MessageReader<crate::plugins::combat::AttackOrder>,
    ids: Query<&crate::plugins::net::entities::NetworkId>,
    state: Res<CosState>,
    mut commands_out: MessageWriter<PetCommand>,
) {
    for order in orders.read() {
        let Ok(target) = ids.get(order.0) else {
            continue;
        };
        let Some(pet) = state.first_of_kind(CosKind::GrowthPet) else {
            continue;
        };
        if !AttackPetSettings(pet.settings()).is_offensive() {
            continue;
        }
        commands_out.write(PetCommand::Attack {
            unique_id: pet.unique_id,
            target_unique_id: target.0,
        });
    }
}

/// 0xB0C5 — the verdict on a 0x70C5 command.
///
/// The action byte is echoed, which is what makes this the probe that settles
/// the `[S]` action codes. On the pick-up action the ack also names the
/// grabbed item; the bag change itself arrives on 0xB034, so nothing is
/// applied here.
pub fn on_pet_action_response(mut reader: MessageReader<PetActionResponse>) {
    for msg in reader.read() {
        if msg.success {
            debug!(
                "cos: 0x70C5 action {} accepted for uid {} (item {:?})",
                msg.action, msg.unique_id, msg.item_gid
            );
        } else {
            warn!(
                "cos: 0x70C5 action {} rejected for uid {} — error {:#06x}",
                msg.action,
                msg.unique_id,
                msg.error_code.unwrap_or(0)
            );
        }
    }
}

/// The three COS acks whose success body is empty (0xB0C6 / 0xB116 / 0xB117).
///
/// Nothing to apply on success by construction — the state change arrives as
/// its own push (0x30C9 arm 1 for the teardown, arm 5 for the new name). Only
/// the failures carry information, and they carry it as a code in the COS
/// error category `0x0C`, whose table is [U].
pub fn on_pet_acks(
    mut terminate: MessageReader<PetTerminateResponse>,
    mut unsummon: MessageReader<PetUnsummonResponse>,
    mut rename: MessageReader<PetRenameResponse>,
) {
    for msg in terminate.read() {
        if !msg.success {
            warn!("cos: terminate refused — error {:?}", msg.error_code);
        }
    }
    for msg in unsummon.read() {
        if !msg.success {
            warn!("cos: unsummon refused — error {:?}", msg.error_code);
        }
    }
    for msg in rename.read() {
        if !msg.success {
            warn!("cos: rename refused — error {:?}", msg.error_code);
        }
    }
}

/// 0xB420 — the authoritative settings word, overwriting the optimistic one
/// [`handle_pet_commands`] wrote.
pub fn on_pet_settings_response(
    mut reader: MessageReader<PetSettingsChangeResponse>,
    mut state: ResMut<CosState>,
) {
    for msg in reader.read() {
        if !msg.success {
            warn!("cos: settings change refused — error {:?}", msg.error_code);
            continue;
        }
        let (Some(unique_id), Some(settings)) = (msg.unique_id, msg.settings) else {
            continue;
        };
        if let Some(cos) = state.get_mut(unique_id) {
            cos.body.unk_f = Some(settings);
        }
    }
}

/// 0x30E7 — "you have wandered too far". The distances the original prints
/// (100 / 30) are its own literals, not wire fields, so they are not echoed
/// here; reasons the original ignores are ignored.
pub fn on_stuck_distance_warning(mut reader: MessageReader<StuckDistanceWarning>) {
    for msg in reader.read() {
        match msg.reason {
            STUCK_REASON_TRADE_CART => warn!("cos: too far from the trade cart"),
            STUCK_REASON_QUEST_MONSTER => warn!("quest: too far from the quest monster"),
            other => debug!("0x30E7 reason {other} (ignored, as the original does)"),
        }
    }
}

fn send<T>(conn: &Query<&SilkroadConnection, With<AgentConnection>>, packet: T)
where
    Packet: From<T>,
{
    let Ok(conn) = conn.single() else {
        return;
    };
    if let Err(e) = conn.get_sender().send(Packet::from(packet).into()) {
        error!("cos: failed to send pet packet: {}", e.0);
    }
}

/// The two action codes this module builds that the *binaries* confirm — both
/// the original's own builders and the vSRO server's `0xB0C5` writer branch on
/// them, so neither is a bot-sourced guess.
pub const VERIFIED_ACTIONS: [u8; 2] = [PET_ACTION_ATTACK, PET_ACTION_ITEM_PICKUP];

#[cfg(test)]
mod test {
    use super::*;
    use packets::agent::character_data::InventoryItem;
    use packets::agent::pet::CosBody;

    use crate::plugins::hud::cos::state::Cos;

    fn pet(kind: CosKind, settings: u32) -> Cos {
        Cos {
            unique_id: 11,
            ref_obj_id: 4242,
            kind,
            body: CosBody {
                hp: 0,
                unk_b: 0,
                growth: None,
                unk_f: Some(settings),
                name: None,
                inventory_size: 0,
                items: Vec::<InventoryItem>::new(),
                unk_g: None,
                unk_h: None,
            },
            hgp: None,
            exp: 0,
            level: None,
        }
    }

    /// The whole point of the offensive/defensive switch: a defensive pet does
    /// not join the player's fight, an offensive one does.
    #[test]
    fn only_an_offensive_attack_pet_joins_the_players_target() {
        let case = |kind, settings| {
            let mut app = App::new();
            app.add_message::<crate::plugins::combat::AttackOrder>()
                .add_message::<PetCommand>()
                .init_resource::<CosState>()
                .add_systems(Update, send_pet_attack_with_player);
            app.world_mut().resource_mut::<CosState>().cos = vec![pet(kind, settings)];
            let target = app
                .world_mut()
                .spawn(crate::plugins::net::entities::NetworkId(909))
                .id();
            app.world_mut()
                .write_message(crate::plugins::combat::AttackOrder(target));
            app.update();
            app.world_mut()
                .resource_mut::<Messages<PetCommand>>()
                .drain()
                .collect::<Vec<_>>()
        };

        // Offensive attack pet -> one Attack command at the player's target.
        let sent = case(CosKind::GrowthPet, AttackPetSettings::OFFENSIVE);
        assert!(matches!(
            sent.as_slice(),
            [PetCommand::Attack {
                unique_id: 11,
                target_unique_id: 909
            }]
        ));

        // Defensive attack pet, and a pick pet, both stay out of it.
        assert!(case(CosKind::GrowthPet, 0).is_empty());
        assert!(case(CosKind::GrabPet, AttackPetSettings::OFFENSIVE).is_empty());
    }

    /// 0xB420 is authoritative: a refused change must not leave the optimistic
    /// flag the command wrote, and an accepted one overwrites it.
    #[test]
    fn the_settings_ack_overrides_the_optimistic_flag() {
        let mut app = App::new();
        app.add_message::<PetSettingsChangeResponse>()
            .init_resource::<CosState>()
            .add_systems(Update, on_pet_settings_response);
        app.world_mut().resource_mut::<CosState>().cos =
            vec![pet(CosKind::GrowthPet, AttackPetSettings::OFFENSIVE)];

        app.world_mut().write_message(PetSettingsChangeResponse {
            success: true,
            unique_id: Some(11),
            settings_type: Some(PET_SETTINGS_TYPE_GOLD),
            settings: Some(0),
            error_code: None,
        });
        app.update();
        let state = app.world().resource::<CosState>();
        assert_eq!(state.get(11).map(|c| c.settings()), Some(0));

        // An ack for a COS we do not hold changes nothing.
        app.world_mut().write_message(PetSettingsChangeResponse {
            success: true,
            unique_id: Some(999),
            settings_type: Some(PET_SETTINGS_TYPE_GOLD),
            settings: Some(AttackPetSettings::OFFENSIVE),
            error_code: None,
        });
        app.update();
        assert_eq!(
            app.world()
                .resource::<CosState>()
                .get(11)
                .map(|c| c.settings()),
            Some(0)
        );
    }
}
