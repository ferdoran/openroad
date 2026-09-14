//! Headless net-check client: drives the full login → join → in-game network
//! roundtrip with no window/GPU, so real server packets can be captured and
//! verified fast (e.g. the group-spawn packets 0x3017/18/19).
//!
//! Idea: reuse the exact framing/handshake/dispatch stack the GUI uses
//! ([`NetworkCorePlugin`] + the `packets` crate) under `MinimalPlugins`, and
//! replace the UI-driven login/join handlers with tiny config-driven ones. The
//! gateway shard-list systems are reused verbatim; everything else is a headless
//! port of the `intro_v2`/`game_scene` packet handlers. Every inbound and
//! outbound frame is dumped (the `packets_in`/`packets_out` trace targets wired
//! in `plugins/net/plugin.rs`), and group-spawn payloads are hex-dumped
//! explicitly.
//!
//! Run with `make netcheck` (or `NETCHECK=1 cargo run -p client`). Credentials
//! come from `config.dev_fast_login`; the captcha is answered with
//! `dev_fast_login.captcha_answer` when set, first operating shard
//! and first character are chosen automatically.
//!
//! With `NETCHECK_ACTIONS=1` the client also *drives* actions after join — it
//! walks a short square (0x7021 → 0xB021) and selects a nearby entity
//! (0x7045 → 0xB045) — so those C→S/response pairs are captured live without a
//! GUI. Item-use (0x704C) is intentionally not driven here (needs itemdata from
//! Data.pk2; see `docs/planning/ROADMAP_V2.md` Phase 2).

use std::env;
use std::time::Duration;

use bevy::app::ScheduleRunnerPlugin;
use bevy::log::{Level, LogPlugin};
use bevy::prelude::*;
use bytes::Bytes;
use mac_address::get_mac_address;

use packets::agent::character_data::parse_character_data;
use packets::agent::prelude::{
    CelestialPosition, CharacterDataBody, CharacterDataEnd, CharacterJoinRequest,
    CharacterJoinResponse, CharacterSelectionAction, CharacterSelectionActionRequest,
    CharacterSelectionActionResponse, GameReady, GroupEntitySpawnBegin, GroupEntitySpawnData,
    GroupEntitySpawnEnd, MovementRequest, MovementResponse, SelectEntityRequest,
    SelectEntityResponse, SingleEntitySpawn,
};

use crate::net::entity_spawn::{parse_group_spawn, RefResolver, RefType};
use packets::agent::{AgentLoginRequest, AgentLoginResponse};
use packets::login::{
    LoginCaptchaChallenge, LoginCaptchaConfirmRequest, LoginRequest, LoginResponse,
};
use packets::Packet;

use crate::net::connection::SilkroadConnection;
use crate::net::frame::SilkroadFrame;
use crate::plugins::config::division::DivisionInfo;
use crate::plugins::config::ClientConfig;
use crate::plugins::net::agent::{AgentConnection, AgentConnectionBundle};
use crate::plugins::net::gateway::shard_list::{
    on_shardlist_ping_response, on_shardlist_response, ShardList,
};
use crate::plugins::net::gateway::systems::{init_gateway_service, poll_gateway_connection};
use crate::plugins::net::gateway::GatewayConnection;
use crate::plugins::net::plugin::{NetworkCorePlugin, NetworkState};
use packets::hexdump;

/// Build and run the headless net-check app. Blocks until the process is killed.
pub fn run_headless(config: ClientConfig) {
    App::new()
        .add_plugins(MinimalPlugins.set(ScheduleRunnerPlugin::run_loop(Duration::from_millis(5))))
        .add_plugins(LogPlugin {
            // Everything at info, plus the per-frame packet dumps at trace.
            level: Level::TRACE,
            filter: "info,packets_in=trace,packets_out=trace".to_string(),
            ..default()
        })
        .insert_resource(config)
        .insert_resource(DivisionInfo::load_or_fallback())
        .insert_resource(ActionDriver {
            enabled: env::var("NETCHECK_ACTIONS").is_ok(),
            ..default()
        })
        .insert_resource(OpcodeProbe::from_env())
        .add_plugins(NetworkCorePlugin)
        // Kick off the (asynchronous) gateway connect. `poll_gateway_connection`
        // delivers the established connection and fires the shard-list ping, so
        // `netcheck_login` (which already waits for both the shard list and the
        // connection) proceeds once they arrive.
        .add_systems(Startup, init_gateway_service)
        .add_systems(
            Update,
            (
                // reused gateway reactions (poll connect -> ping -> shard list)
                poll_gateway_connection,
                on_shardlist_ping_response,
                on_shardlist_response,
                // headless login/join driver
                netcheck_login,
                netcheck_captcha,
                netcheck_on_login_response,
                netcheck_request_char_list,
                netcheck_join_first_character,
                netcheck_on_join_response,
                netcheck_game_ready,
                netcheck_dump_group_spawns,
            ),
        )
        // Action driver (only active with NETCHECK_ACTIONS=1); every system
        // early-returns when disabled.
        .add_systems(
            Update,
            (
                netcheck_capture_local_uid,
                netcheck_capture_local_pos,
                netcheck_drive_actions,
                netcheck_select_target,
                netcheck_log_action_responses,
                netcheck_fire_probe,
            ),
        )
        .run();
}

/// Once the shard list and gateway connection are ready, send the login request
/// with the configured credentials on the first operating shard. Fires once.
fn netcheck_login(
    mut fired: Local<bool>,
    config: Res<ClientConfig>,
    shard_list: Option<Res<ShardList>>,
    gateway: Query<&SilkroadConnection, With<GatewayConnection>>,
    division: Res<DivisionInfo>,
) {
    if *fired {
        return;
    }
    let Some(shard_list) = shard_list else {
        return;
    };
    let Ok(conn) = gateway.single() else {
        return;
    };
    let Some(shard) = shard_list
        .0
        .shards
        .iter()
        .find(|s| s.is_operating)
        .or_else(|| shard_list.0.shards.first())
    else {
        warn!("netcheck: shard list is empty");
        return;
    };

    let frame = Packet::from(LoginRequest {
        content_id: division.content_id,
        username: config.dev_fast_login.username.clone(),
        password: config.dev_fast_login.password.clone(),
        shard_id: shard.id,
    })
    .into();
    if let Err(e) = conn.get_sender().send(frame) {
        error!("netcheck: failed to send LoginRequest: {}", e.0);
        return;
    }
    info!(
        "netcheck: sent LoginRequest for '{}' on shard {} ({})",
        config.dev_fast_login.username, shard.id, shard.name
    );
    *fired = true;
}

/// Answer the captcha with the configured `dev_fast_login.captcha_answer`.
///
/// Headless has no modal to fall back on, so with no code configured this can
/// only say so — the old hardcoded `"1"` merely hid that by guessing.
fn netcheck_captcha(
    mut events: MessageReader<LoginCaptchaChallenge>,
    config: Res<ClientConfig>,
    gateway: Query<&SilkroadConnection, With<GatewayConnection>>,
) {
    if events.read().count() == 0 {
        return;
    }
    let Some(code) = config.dev_fast_login.captcha_answer.clone() else {
        error!(
            "netcheck: server sent an IBUV captcha but dev_fast_login.captcha_answer is unset; \
             login cannot continue"
        );
        return;
    };
    let Ok(conn) = gateway.single() else {
        return;
    };
    let frame = Packet::from(LoginCaptchaConfirmRequest { code }).into();
    if let Err(e) = conn.get_sender().send(frame) {
        error!("netcheck: failed to send captcha confirm: {}", e.0);
        return;
    }
    info!("netcheck: submitted the configured captcha answer");
}

/// Port of `intro_v2::net::on_gateway_login_response`: open the agent connection
/// from the gateway's login info and send the agent login.
fn netcheck_on_login_response(
    mut reader: MessageReader<LoginResponse>,
    config: Res<ClientConfig>,
    mut network_state: ResMut<NetworkState>,
    gateway: Query<Entity, With<GatewayConnection>>,
    division: Res<DivisionInfo>,
    mut commands: Commands,
) {
    for res in reader.read() {
        if let Some(err) = &res.login_error {
            error!(
                "netcheck: login failed: {} ({:?})",
                packets::login::describe_login_error(err.error_code),
                err.failure()
            );
            continue;
        }
        let Some(info) = res.login_info.clone() else {
            continue;
        };

        if let Ok(gw) = gateway.single() {
            commands.entity(gw).despawn();
            network_state.gateway = false;
        }

        match SilkroadConnection::new(&format!("{}:{}", info.agent_ip, info.agent_port)) {
            Ok(conn) => {
                network_state.agent = true;
                let mac = get_mac_address()
                    .ok()
                    .flatten()
                    .map(|m| m.bytes())
                    .unwrap_or([0; 6]);
                let sender = conn.get_sender();
                let frame = Packet::from(AgentLoginRequest {
                    token: info.agent_token,
                    username: config.dev_fast_login.username.clone(),
                    password: config.dev_fast_login.password.clone(),
                    content_id: division.content_id,
                    mac_address: mac,
                })
                .into();
                if let Err(e) = sender.send(frame) {
                    error!("netcheck: failed to send AgentLoginRequest: {}", e.0);
                }
                commands.spawn(AgentConnectionBundle::new(conn));
                info!(
                    "netcheck: opened agent connection to {}:{}",
                    info.agent_ip, info.agent_port
                );
            }
            Err(e) => error!("netcheck: failed to connect to agent server: {}", e),
        }
    }
}

/// On a successful agent login, request the character list.
fn netcheck_request_char_list(
    mut reader: MessageReader<AgentLoginResponse>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
) {
    let mut ok = false;
    for res in reader.read() {
        if res.error_code.is_none() {
            ok = true;
        } else {
            if let Some(code) = res.error_code {
                error!(
                    "netcheck: agent login failed: code {} ({})",
                    code,
                    packets::agent::describe_agent_auth_error(code)
                );
            }
        }
    }
    if !ok {
        return;
    }
    let Ok(conn) = conn.single() else {
        return;
    };
    let frame = Packet::from(CharacterSelectionActionRequest {
        action: CharacterSelectionAction::List,
        name: None,
        create: None,
    })
    .into();
    if let Err(e) = conn.get_sender().send(frame) {
        error!("netcheck: failed to request character list: {}", e.0);
        return;
    }
    info!("netcheck: requested character list");
}

/// Join the first character from the list response. Fires once.
fn netcheck_join_first_character(
    mut fired: Local<bool>,
    mut reader: MessageReader<CharacterSelectionActionResponse>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
) {
    if *fired {
        return;
    }
    let mut name = None;
    for res in reader.read() {
        if let Some(characters) = &res.characters {
            if let Some(first) = characters.characters.first() {
                name = Some(first.name.clone());
            }
        }
    }
    let Some(name) = name else {
        return;
    };
    let Ok(conn) = conn.single() else {
        return;
    };
    let frame = Packet::from(CharacterJoinRequest {
        character_name: name.clone(),
    })
    .into();
    if let Err(e) = conn.get_sender().send(frame) {
        error!("netcheck: failed to send CharacterJoinRequest: {}", e.0);
        return;
    }
    info!("netcheck: joining first character '{}'", name);
    *fired = true;
}

/// Log the join outcome.
fn netcheck_on_join_response(mut reader: MessageReader<CharacterJoinResponse>) {
    for res in reader.read() {
        if res.result == 1 {
            info!("netcheck: joined the world — now capturing in-game packets (Ctrl-C to stop)");
        } else {
            error!("netcheck: join failed with error {:?}", res.error);
        }
    }
}

/// After the character-data stream ends, send `GameReady` so the server proceeds
/// to stream the surrounding entities. Fires once.
fn netcheck_game_ready(
    mut fired: Local<bool>,
    mut reader: MessageReader<CharacterDataEnd>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
) {
    if reader.read().count() == 0 || *fired {
        return;
    }
    let Ok(conn) = conn.single() else {
        return;
    };
    let frame = Packet::from(GameReady).into();
    if let Err(e) = conn.get_sender().send(frame) {
        error!("netcheck: failed to send GameReady: {}", e.0);
        return;
    }
    info!("netcheck: sent GameReady (0x3012)");
    *fired = true;
}

/// Explicitly surface the group-spawn packets we are verifying, hex-dumping the
/// data payload (cross-check against `net::entity_spawn`).
fn netcheck_dump_group_spawns(
    mut begins: MessageReader<GroupEntitySpawnBegin>,
    mut datas: MessageReader<GroupEntitySpawnData>,
    mut ends: MessageReader<GroupEntitySpawnEnd>,
) {
    for b in begins.read() {
        info!(
            "netcheck: GroupEntitySpawnBegin (0x3017) kind={} count={}",
            b.kind, b.count
        );
    }
    for d in datas.read() {
        info!(
            "netcheck: GroupEntitySpawnData (0x3019) {} bytes: {}",
            d.raw.len(),
            hexdump(&d.raw, 512)
        );
    }
    for _ in ends.read() {
        info!("netcheck: GroupEntitySpawnEnd (0x3018)");
    }
}

// ---------------------------------------------------------------------------
// Action driver (NETCHECK_ACTIONS=1)
//
// Idea: once joined, "play" a little so the server's C->S responses are
// captured live — walk a short square (0x7021 -> 0xB021) and select a nearby
// entity (0x7045 -> 0xB045). Item-use (0x704C) is deliberately NOT driven
// here: resolving a real consumable slot -> packed type_id needs the client's
// itemdata tables (Data.pk2), which the headless harness does not load, so it
// belongs to a GUI play session (see docs/planning/ROADMAP_V2.md, Phase 2).
// ---------------------------------------------------------------------------

/// Shared state for the post-join action sequence.
#[derive(Resource, Default)]
struct ActionDriver {
    enabled: bool,
    local_uid: Option<u32>,
    pos_known: bool,
    region: u16,
    base_x: i32,
    base_y: i32,
    base_z: i32,
    moves_sent: u8,
    /// Wall-clock (elapsed secs) at which the next move may be sent; `None`
    /// until the settle delay after position is known has been scheduled.
    next_move_at: Option<f64>,
    select_sent: bool,
    /// A CHARACTER_DATA body that arrived before the unique id did, kept so the
    /// scan can be retried once `CelestialPosition` names the id (#728).
    pending_body: Option<Bytes>,
}

/// Minimal [`RefResolver`] for headless spawn parsing: with no itemdata tables
/// we cannot classify ref ids, so treat every record as a monster/NPC — the
/// leading `unique_id` + position are read before any itemdata-dependent
/// branch, which is all we need to pick a select target. Player/item/structure
/// records simply fail to parse and are skipped (`parse_group_spawn` never
/// panics), so a wrong guess costs at most a skipped target.
struct HeadlessResolver;

impl RefResolver for HeadlessResolver {
    fn resolve(&self, _ref_id: u32) -> RefType {
        RefType::Monster
    }
    fn item_is_equipment(&self, _ref_id: u32) -> bool {
        false
    }
}

/// Record the local player's in-world id from the celestial-position packet.
fn netcheck_capture_local_uid(
    mut driver: ResMut<ActionDriver>,
    mut reader: MessageReader<CelestialPosition>,
) {
    if !driver.enabled {
        return;
    }
    for p in reader.read() {
        if driver.local_uid.is_none() {
            driver.local_uid = Some(p.unique_id);
            info!("netcheck: local player unique_id = {}", p.unique_id);
            // The body almost always arrived first (see `netcheck_capture_local_pos`),
            // and its message is gone by now — retry the stashed copy with the id
            // the scan was missing.
            if let Some(raw) = driver.pending_body.take() {
                lock_position(&mut driver, &raw);
            }
        }
    }
}

/// Parse the local player's spawn position out of the CHARACTER_DATA body so we
/// have a valid region + coords to move from. Resolver-free (uses the anchor
/// scanner), so it works without any itemdata.
fn netcheck_capture_local_pos(
    mut driver: ResMut<ActionDriver>,
    mut reader: MessageReader<CharacterDataBody>,
) {
    if !driver.enabled || driver.pos_known {
        return;
    }
    for body in reader.read() {
        if lock_position(&mut driver, &body.raw) {
            continue;
        }
        // No position yet. On the live server the body (0x3013) beats
        // `CelestialPosition` (0x3020) by ~20 ms — `packet_dump/0x3013.log`
        // 2026-08-16T09:21:39.127Z vs `0x3020.log` …:39.149Z — so the scan ran
        // WITHOUT a unique id, and the id-less scan deliberately refuses a body
        // that offers more than one plausible position
        // (`packets/src/agent/character_data.rs:696-703`). A real body offers
        // dozens. Keep it until the id arrives instead of dropping it: the
        // message itself does not survive to the next frame.
        driver.pending_body = Some(body.raw.clone());
    }
}

/// Try to pin the driver's spawn position from a CHARACTER_DATA body. Returns
/// whether the position is now known, so the caller can decide whether to stash
/// the body for a retry.
fn lock_position(driver: &mut ActionDriver, raw: &Bytes) -> bool {
    if driver.pos_known {
        return true;
    }
    let Some(pc) = parse_character_data(raw, driver.local_uid) else {
        return false;
    };
    if driver.local_uid.is_none() {
        driver.local_uid = Some(pc.unique_id);
    }
    driver.region = pc.region;
    driver.base_x = pc.x.round() as i32;
    driver.base_y = pc.y.round() as i32;
    driver.base_z = pc.z.round() as i32;
    driver.pos_known = true;
    info!(
        "netcheck: local position region={} ({}, {}, {}) — driving actions",
        pc.region, driver.base_x, driver.base_y, driver.base_z
    );
    true
}

/// Walk a small square around the spawn point (four legs, ~150 region-local
/// units each, paced ~1.5 s apart after a 2 s settle), sending one
/// [`MovementRequest`] per leg.
fn netcheck_drive_actions(
    mut driver: ResMut<ActionDriver>,
    time: Res<Time>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
) {
    if !driver.enabled || !driver.pos_known || driver.moves_sent >= 4 {
        return;
    }
    let now = time.elapsed_secs_f64();
    let due = *driver.next_move_at.get_or_insert(now + 2.0);
    if now < due {
        return;
    }
    let Ok(conn) = conn.single() else {
        return;
    };
    const STEP: i32 = 150;
    let (dx, dz) = match driver.moves_sent {
        0 => (STEP, 0),
        1 => (0, STEP),
        2 => (-STEP, 0),
        _ => (0, -STEP),
    };
    let req = MovementRequest {
        region: driver.region,
        x: driver.base_x + dx,
        y: driver.base_y,
        z: driver.base_z + dz,
    };
    let (region, x, y, z) = (req.region, req.x, req.y, req.z);
    if let Err(e) = conn.get_sender().send(Packet::from(req).into()) {
        error!("netcheck: failed to send MovementRequest: {}", e.0);
        return;
    }
    info!(
        "netcheck: sent MovementRequest #{} -> region={} ({}, {}, {})",
        driver.moves_sent + 1,
        region,
        x,
        y,
        z
    );
    driver.moves_sent += 1;
    driver.next_move_at = Some(now + 1.5);
}

/// Select the first parseable nearby entity from a single-entity spawn, once.
fn netcheck_select_target(
    mut driver: ResMut<ActionDriver>,
    mut reader: MessageReader<SingleEntitySpawn>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
) {
    if !driver.enabled || driver.select_sent {
        return;
    }
    let Ok(conn) = conn.single() else {
        return;
    };
    for spawn in reader.read() {
        let parsed = parse_group_spawn(&spawn.raw, true, 1, &HeadlessResolver);
        let Some(entity) = parsed.spawns.first() else {
            continue;
        };
        if entity.unique_id == 0 {
            continue;
        }
        let uid = entity.unique_id;
        if let Err(e) = conn
            .get_sender()
            .send(Packet::from(SelectEntityRequest { unique_id: uid }).into())
        {
            error!("netcheck: failed to send SelectEntityRequest: {}", e.0);
            return;
        }
        info!(
            "netcheck: sent SelectEntityRequest for nearby entity uid={} at region={}",
            uid, entity.position.region
        );
        driver.select_sent = true;
        break;
    }
}

/// Surface the action responses explicitly (they are also in the raw frame
/// trace, but this annotates the capture).
fn netcheck_log_action_responses(
    driver: Res<ActionDriver>,
    mut moves: MessageReader<MovementResponse>,
    mut selects: MessageReader<SelectEntityResponse>,
) {
    if !driver.enabled {
        return;
    }
    for m in moves.read() {
        if Some(m.unique_id) == driver.local_uid {
            info!(
                "netcheck: MovementResponse (0xB021) local: has_dest={} region={} ({}, {}, {}) angle={}",
                m.has_destination, m.region, m.x, m.y, m.z, m.angle
            );
        }
    }
    for s in selects.read() {
        info!(
            "netcheck: SelectEntityResponse (0xB045) result={} uid={} tail={}B",
            s.result,
            s.unique_id,
            s.tail.len()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build `[u32 id][u16 region][f32 x][f32 y][f32 z][u16 heading]`.
    fn id_and_position(id: u32, region: u16, x: f32, y: f32, z: f32) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&id.to_le_bytes());
        v.extend_from_slice(&region.to_le_bytes());
        v.extend_from_slice(&x.to_le_bytes());
        v.extend_from_slice(&y.to_le_bytes());
        v.extend_from_slice(&z.to_le_bytes());
        v.extend_from_slice(&0u16.to_le_bytes());
        v
    }

    /// #728's root cause, pinned: a CHARACTER_DATA body that offers more than
    /// one plausible position is REFUSED by the id-less scan
    /// (`packets/src/agent/character_data.rs:696-703` — "a second match means we
    /// can't tell the real spawn from a coincidence"). That is correct
    /// behaviour, and it is why the headless driver never started: on the live
    /// server the body (0x3013) beats `CelestialPosition` (0x3020) by ~20 ms, so
    /// the scan always ran without the id. The real 2026-08-16 body offers 46
    /// candidates; two are enough to reproduce it.
    #[test]
    fn a_body_without_the_unique_id_cannot_be_pinned() {
        let mut raw = vec![0u8; 8];
        raw.extend(id_and_position(119_948, 25_000, 800.0, 20.0, 900.0));
        raw.extend(id_and_position(4_242, 24_000, 700.0, 10.0, 600.0));
        let raw = Bytes::from(raw);

        assert!(parse_character_data(&raw, None).is_none());
        assert!(parse_character_data(&raw, Some(119_948)).is_some());
    }

    /// The fix: the body is kept and re-scanned when the id arrives, so the
    /// out-of-order arrival stops being fatal.
    #[test]
    fn the_stashed_body_is_pinned_once_the_unique_id_arrives() {
        let mut raw = vec![0u8; 8];
        raw.extend(id_and_position(119_948, 25_000, 800.0, 20.0, 900.0));
        raw.extend(id_and_position(4_242, 24_000, 700.0, 10.0, 600.0));
        let raw = Bytes::from(raw);

        let mut driver = ActionDriver {
            enabled: true,
            ..default()
        };
        // Body first, id unknown: refused, so the caller stashes it.
        assert!(!lock_position(&mut driver, &raw));
        assert!(!driver.pos_known);

        // CelestialPosition names the id; the retry succeeds.
        driver.local_uid = Some(119_948);
        assert!(lock_position(&mut driver, &raw));
        assert!(driver.pos_known);
        assert_eq!(driver.region, 25_000);
        assert_eq!(
            (driver.base_x, driver.base_y, driver.base_z),
            (800, 20, 900)
        );
    }
}

// ---------------------------------------------------------------------------
// Raw opcode probe (NETCHECK_PROBE=<opcode-hex>:<body-hex>[,<opcode>:<body>…])
//
// Idea: some questions about this server can only be answered by sending one
// exact frame and watching what comes back — #215's item-use gate is the
// standing example ("0x704C resets the connection, in any body form"), and that
// claim was last measured before the #454 body fix. A typed request cannot
// express the negative control the experiment needs (an opcode the server has
// no handler for at all), so this sends a frame built by hand: opcode + body
// bytes, encryption flag 0, exactly like the original's FUN_00841780(op, 0).
//
// It fires once, `NETCHECK_PROBE_DELAY` seconds after the world join (default
// 5), and logs the wire bytes with a millisecond timestamp so the send can be
// lined up against the dumps and against a connection reset.
// ---------------------------------------------------------------------------

/// One parsed `opcode:body` probe plus the schedule state for firing it.
#[derive(Resource, Default)]
struct OpcodeProbe {
    frames: Vec<(u16, Bytes)>,
    delay: f64,
    due: Option<f64>,
    sent: bool,
}

impl OpcodeProbe {
    /// Parse `NETCHECK_PROBE`; an unparsable entry is a hard error rather than
    /// a silent skip — a probe that quietly did not fire would be read as
    /// "the server accepted it".
    fn from_env() -> Self {
        let Ok(spec) = env::var("NETCHECK_PROBE") else {
            return Self::default();
        };
        let mut frames = Vec::new();
        for entry in spec.split(',').filter(|s| !s.is_empty()) {
            let (op, body) = entry.split_once(':').unwrap_or((entry, ""));
            let opcode = u16::from_str_radix(op.trim_start_matches("0x"), 16)
                .unwrap_or_else(|e| panic!("NETCHECK_PROBE: bad opcode {op:?}: {e}"));
            let body = body.trim();
            let bytes: Vec<u8> = (0..body.len())
                .step_by(2)
                .map(|i| {
                    u8::from_str_radix(&body[i..(i + 2).min(body.len())], 16)
                        .unwrap_or_else(|e| panic!("NETCHECK_PROBE: bad body {body:?}: {e}"))
                })
                .collect();
            frames.push((opcode, Bytes::from(bytes)));
        }
        Self {
            frames,
            delay: env::var("NETCHECK_PROBE_DELAY")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(5.0),
            ..Self::default()
        }
    }
}

/// Fire the configured probe frames once, after the join has settled.
fn netcheck_fire_probe(
    mut probe: ResMut<OpcodeProbe>,
    time: Res<Time>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
) {
    if probe.sent || probe.frames.is_empty() {
        return;
    }
    let Ok(conn) = conn.single() else {
        return;
    };
    let now = time.elapsed_secs_f64();
    let delay = probe.delay;
    let due = *probe.due.get_or_insert(now + delay);
    if now < due {
        return;
    }
    probe.sent = true;
    let frames = std::mem::take(&mut probe.frames);
    for (opcode, data) in frames {
        info!(
            "netcheck: PROBE sending opcode {:#06x} body [{}] ({} bytes)",
            opcode,
            hexdump(&data, 64),
            data.len()
        );
        let frame = SilkroadFrame::Packet {
            count: 0,
            crc: 0,
            opcode,
            encrypted: 0,
            data,
        };
        if let Err(e) = conn.get_sender().send(frame) {
            error!("netcheck: PROBE send failed: {}", e.0);
        }
    }
}
