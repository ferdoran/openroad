//! Click-to-talk: walk into range of a clicked NPC, then open its dialog.
//!
//! Idea: per vanilla, a single click on an NPC both selects it and starts the
//! talk approach. The walk is purely client-side, modeled on the combat
//! gap-close (`combat::close_attack_gap`) but with its own [`PendingTalk`]
//! state so the combat cancel rules never fight it: the player is steered via
//! `PlayerCommands::move_to` until within [`TALK_GAP_STOP`] of the NPC, then
//! the close→select→talk packet sequence goes out (0x704B/0x7045/0x7046,
//! capture-informed) and a [`TalkStarted`] message hands over to the dialog
//! HUD (which opens immediately; the 0xB046 ack is logging-only). A manual
//! ground click, selecting something else, or Esc cancels the approach.

use bevy::picking::mesh_picking::ray_cast::RayCastBackfaces;
use bevy::prelude::*;

use packets::agent::prelude::{CloseTalkRequest, SelectEntityRequest, TalkRequest};
use packets::Packet;

use crate::net::connection::SilkroadConnection;
use crate::plugins::cursor::interactions::entity_select::{HitProxyVolume, SelectedEntity};
use crate::plugins::net::agent::AgentConnection;
use crate::plugins::net::entities::{GateVolumeNeeded, NetworkId};
use crate::plugins::player::{Player, PlayerCommands, PlayerMoveOrder};
use crate::scenes::SceneState;

/// Stop the approach this far (render units) from the NPC. Melee attacks stop
/// at 16; NPCs allow a little more standoff.
const TALK_GAP_STOP: f32 = 24.0;
/// Hysteresis so a marginally-out-of-range NPC doesn't retrigger walks.
const TALK_GAP_SLACK: f32 = 4.0;

/// A dialog interaction kind. NOTE: playtests showed the spawn-record
/// option-bit list is NOT a reliable source for these (city guards advertise
/// trade-ish bits) — the dialog derives its options from the shop/teleport/
/// speech tables instead and only logs the raw bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TalkOption {
    /// Plain talk/gossip page.
    Talk,
    /// Open the NPC's store (buy/sell).
    Store,
    /// Open the storage (bank) — not implemented yet.
    Storage,
    /// Open the GUILD storage (guild warehouse) — same NPC, different family
    /// (0x7250, `hud::guild_storage`).
    GuildStorage,
    /// Open the teleport destination list.
    Teleport,
}

/// "Walk to this NPC and talk" — written by the selection click handler.
#[derive(Message)]
pub struct TalkOrder(pub Entity);

/// The talk request for this NPC went out (0x7046) — the dialog HUD takes
/// over. The dialog opens on send via this message (client-driven); this server
/// does not answer 0x7046 with 0xB046, and the dialog does not depend on it.
#[derive(Message)]
pub struct TalkStarted {
    pub npc: Entity,
}

/// The NPC currently being approached for a talk (at most one).
#[derive(Resource, Default)]
pub struct PendingTalk(pub Option<Entity>);

pub struct NpcInteractionPlugin;

impl Plugin for NpcInteractionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PendingTalk>()
            .add_message::<TalkOrder>()
            .add_message::<TalkStarted>()
            .add_systems(
                Update,
                (
                    equip_gate_volumes,
                    on_talk_order,
                    cancel_talk_on_move,
                    cancel_talk_on_deselect,
                    approach_talk_target,
                )
                    .chain()
                    .run_if(in_state(SceneState::GameWorld)),
            );
    }
}

/// Give meshless interactables (teleport gates — their stones are static map
/// objects, the entity is just an anchor) an invisible clickable volume,
/// following the hit-proxy recipe. `maintain_hit_proxies` never touches
/// these roots (they have no mesh AABB sources).
pub fn equip_gate_volumes(
    gates: Query<Entity, With<GateVolumeNeeded>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut unit_cube: Local<Option<Handle<Mesh>>>,
    mut commands: Commands,
) {
    if gates.is_empty() {
        return;
    }
    let unit_cube = unit_cube
        .get_or_insert_with(|| meshes.add(Cuboid::new(1.0, 1.0, 1.0)))
        .clone();
    for gate in gates.iter() {
        commands
            .entity(gate)
            .remove::<GateVolumeNeeded>()
            .with_children(|parent| {
                parent.spawn((
                    HitProxyVolume,
                    Mesh3d(unit_cube.clone()),
                    // a generous stone-sized volume, centered above the anchor
                    Transform::from_translation(Vec3::new(0.0, 15.0, 0.0))
                        .with_scale(Vec3::new(16.0, 30.0, 16.0)),
                    Pickable::default(),
                    RayCastBackfaces,
                    Visibility::Hidden,
                ));
            });
    }
}

fn on_talk_order(mut orders: MessageReader<TalkOrder>, mut pending: ResMut<PendingTalk>) {
    for TalkOrder(npc) in orders.read() {
        debug!("npc talk: approaching {:?}", npc);
        pending.0 = Some(*npc);
    }
}

/// A manual ground click cancels the approach (matching the combat rule; the
/// approach's own `move_to` steering does not emit [`PlayerMoveOrder`]).
fn cancel_talk_on_move(
    mut moves: MessageReader<PlayerMoveOrder>,
    mut pending: ResMut<PendingTalk>,
) {
    if moves.is_empty() {
        return;
    }
    moves.clear();
    if pending.0.take().is_some() {
        debug!("npc talk: move order cancels the approach");
    }
}

/// Deselecting the NPC (Esc, selecting something else) cancels the approach.
fn cancel_talk_on_deselect(selected: Res<SelectedEntity>, mut pending: ResMut<PendingTalk>) {
    let Some(npc) = pending.0 else {
        return;
    };
    if selected.0 != Some(npc) {
        pending.0 = None;
    }
}

/// Steer toward the pending NPC; within range, send the talk request and hand
/// over to the dialog HUD.
fn approach_talk_target(
    mut pending: ResMut<PendingTalk>,
    players: Query<&Transform, With<Player>>,
    targets: Query<(&Transform, &NetworkId), Without<Player>>,
    mut player_commands: ResMut<PlayerCommands>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    mut started: MessageWriter<TalkStarted>,
) {
    let Some(npc) = pending.0 else {
        return;
    };
    let Ok(player) = players.single() else {
        return;
    };
    let Ok((target_tf, network_id)) = targets.get(npc) else {
        // the NPC despawned/streamed out mid-approach
        pending.0 = None;
        return;
    };
    let target_xz = target_tf.translation.xz();
    let trigger = TALK_GAP_STOP + TALK_GAP_SLACK;

    if target_xz.distance(player.translation.xz()) > trigger {
        // mid-walk: leave a still-valid destination alone (NPCs don't move,
        // so no re-aiming logic is needed beyond issuing the walk once)
        if let Some(destination) = player_commands.move_destination() {
            if target_xz.distance(destination.xz()) <= trigger {
                return;
            }
        }
        let delta = target_xz - player.translation.xz();
        let distance = delta.length().max(f32::EPSILON);
        let stop = target_xz - (delta / distance) * TALK_GAP_STOP;
        player_commands.move_to(Vec3::new(stop.x, player.translation.y, stop.y));
        return;
    }

    pending.0 = None;
    let Ok(conn) = conn.single() else {
        warn!("npc talk: no agent connection, dropping talk request");
        return;
    };
    // Three packets back-to-back on the ordered stream (capture-informed):
    // a close first — the server answers `02 0b1c` ("session already open")
    // when a previous talk was never properly closed — then a re-select so
    // the talk never races an unprocessed selection (`02 0500`), then the
    // talk itself.
    let sequence: [Packet; 3] = [
        CloseTalkRequest {
            unique_id: network_id.0,
        }
        .into(),
        SelectEntityRequest {
            unique_id: network_id.0,
        }
        .into(),
        TalkRequest {
            unique_id: network_id.0,
            talk_flag: 1,
        }
        .into(),
    ];
    info!(
        "npc talk: requesting talk with uid {} (0x7046)",
        network_id.0
    );
    for packet in sequence {
        if let Err(e) = conn.get_sender().send(packet.into()) {
            error!("network: failed to send talk sequence packet: {}", e.0);
            return;
        }
    }
    started.write(TalkStarted { npc });
}
