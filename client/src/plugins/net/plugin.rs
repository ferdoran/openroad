use bevy::app::{App, Plugin, PostUpdate, PreUpdate};
use bevy::log::{debug, error, info, trace};
use bevy::prelude::{
    warn, Commands, Entity, Local, Message, MessageReader, MessageWriter, Query, Res, ResMut,
    Resource, Time, Timer, TimerMode, Update, With,
};
use bytes::{Bytes, BytesMut};
use std::io::Write;
use std::io::{ErrorKind, Read};
use std::sync::{Arc, RwLock};

use packets::global::{GlobalStateRequest, GlobalStateUpdate, KeepAlive};
use packets::{NetworkExt, Packet, PacketError};

use crate::net::connection::SilkroadConnection;
use crate::net::frame::SilkroadFrame;
use crate::net::security::SilkroadSecurityState;
use crate::plugins::config::ClientConfig;
use crate::plugins::net::agent::AgentConnection;
use crate::plugins::net::gateway::plugin::GatewayPlugin;
use crate::plugins::net::gateway::GatewayConnection;
use crate::plugins::net::packet_dump::PacketDump;

#[derive(Resource)]
pub struct KeepAliveTimer {
    timer: Timer,
}

#[derive(Resource, Default)]
pub struct NetworkState {
    pub gateway: bool,
    pub agent: bool,
}

impl Default for KeepAliveTimer {
    fn default() -> Self {
        Self {
            timer: Timer::from_seconds(5.0, TimerMode::Repeating),
        }
    }
}

/// The rendering-independent networking core: the packet event wiring plus the
/// read/write/keep-alive/disconnect systems. Reused by both the GUI
/// [`NetworkPlugin`] and the headless net-check client (`crate::netcheck`),
/// which cannot pull in the `SceneState`-keyed [`GatewayPlugin`].
pub struct NetworkCorePlugin;
impl Plugin for NetworkCorePlugin {
    fn build(&self, app: &mut App) {
        // Config is optional here: the headless net-check client reuses this
        // plugin without a ClientConfig resource — dump by default there too.
        let dump_enabled = app
            .world()
            .get_resource::<ClientConfig>()
            .map(|c| c.network_settings.packet_dump)
            .unwrap_or(true);
        if dump_enabled {
            app.init_resource::<PacketDump>();
        }
        // Opt-in, and absent by default: sending an encrypted login the
        // server does not expect would break the only working flow we have.
        let encrypt_outbound = app
            .world()
            .get_resource::<ClientConfig>()
            .map(|c| c.network_settings.outbound_encryption)
            .unwrap_or(false);
        if encrypt_outbound {
            app.init_resource::<OutboundEncryption>();
        }
        app.add_network_events()
            .add_plugins(crate::plugins::net::entities::NetworkEntitiesPlugin)
            .add_plugins(crate::plugins::net::party::PartyPlugin)
            .add_plugins(crate::plugins::net::guild::GuildPlugin)
            .add_plugins(crate::plugins::net::stall::StallEntitiesPlugin)
            .init_resource::<NetworkState>()
            .init_resource::<crate::plugins::net::gateway::GatewayConnectionStatus>()
            .add_message::<GatewayServiceDisconnected>()
            .add_message::<AgentConnectionDisconnected>()
            .add_systems(PreUpdate, (receive_packets, fetch_disconnects))
            .add_systems(
                Update,
                (
                    keep_alive,
                    on_global_state_request,
                    on_global_state_update,
                    on_agent_disconnected,
                    on_gateway_disconnected,
                ),
            )
            .add_systems(PostUpdate, send_packets);
    }
}

pub struct NetworkPlugin;
impl Plugin for NetworkPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(NetworkCorePlugin);
        let client_config = app.world().resource::<ClientConfig>();
        if client_config.network_settings.enabled {
            app.add_plugins(GatewayPlugin);
        } else {
            warn!("networking disabled");
        }
    }
}

/// Run condition for anything that starts a gateway connect outside
/// [`GatewayPlugin`]. `poll_gateway_connection` lives *only* in that plugin, so
/// a connect kicked off while networking is disabled is never delivered to the
/// ECS: the status stays `Connecting` forever and the login form waits on a
/// connection that cannot arrive. Callers outside the plugin must gate on this.
pub fn networking_enabled(config: Option<Res<ClientConfig>>) -> bool {
    config.is_some_and(|c| c.network_settings.enabled)
}

#[derive(Message)]
pub struct AgentConnectionDisconnected;
#[derive(Message)]
pub struct GatewayServiceDisconnected;

/// Opcodes the original client receives and deliberately discards, so ours must
/// not report them as unhandled. `0x2110`'s branch in the original's receive loop
/// skips the whole body without reading a byte (`sro_client.exe 00842ff0:90-92`)
/// and it appears in none of its registration tables; there is no layout to wire.
///
/// `0x3C81` (SERVER_ACADEMY_DATA) is the same case one table over: it *is*
/// registered (table B, handler `FUN_008986c0`), but the handler contains no
/// read call at all — the original consumes zero bytes of the message
/// (`docs/re/net/inbound/academy.md:118-127`, verdict do-not-wire `[V]`). A
/// struct for it would model nothing, so it is ignored by name instead of
/// showing up as an unhandled opcode forever (#261).
const KNOWN_IGNORED_OPCODES: &[u16] = &[0x2110, 0x3C81];

/// Emit a decoded packet, or log an undecodable one. Opcodes we don't handle
/// yet (buffs, inventory, chat, ...) are expected — surface them as network
/// debug logs, not errors, so genuine decode failures stand out.
fn dispatch_packet(result: Result<Packet, PacketError>, packets: &mut MessageWriter<Packet>) {
    match result {
        Ok(packet) => {
            packets.write(packet);
        }
        Err(PacketError::UnknownOpcode(opcode)) if KNOWN_IGNORED_OPCODES.contains(&opcode) => {
            trace!("network: ignoring opcode {opcode:#06X} (known-and-ignored)");
        }
        Err(PacketError::UnknownOpcode(opcode)) => {
            debug!("network: unhandled opcode {opcode:#06X}");
        }
        Err(e) => {
            error!("network: failed to deserialize packet: {}", e);
        }
    }
}

/// One step of draining the inbound byte accumulator. The receive loop pulls
/// logical packets off the front of the buffer with [`next_packet`]; a frame
/// whose bytes have not all arrived yet reports [`FrameStep::Incomplete`] and
/// stays buffered for the next read, rather than being dropped (which used to
/// corrupt every packet after the split).
enum FrameStep {
    /// A complete `Packet`, or a massive packet reassembled from its payloads.
    Ready {
        opcode: u16,
        data: Bytes,
        consumed: usize,
        /// The frame's wire `0x8000` bit, read from the plaintext size prefix
        /// before decryption. Carried out so `packet_dump` can record it
        /// (#459); for a massive packet it is the *header* frame's bit.
        encrypted: bool,
    },
    /// A frame parsed but carrying nothing to dispatch (a stray massive payload
    /// or a future frame kind) — advance past it.
    Skip { consumed: usize },
    /// Not enough bytes buffered yet for the next frame; wait for more.
    Incomplete,
    /// The length prefix is impossible or a frame failed to parse: the stream is
    /// desynchronised and the buffered bytes can no longer be trusted.
    Corrupt,
}

/// Pull the next logical packet off the front of `buf`. `buf` is decrypted in
/// place by [`SilkroadFrame::parse`]; the returned `data` is copied out, so the
/// caller can drop the consumed prefix afterwards.
fn next_packet(buf: &mut [u8], security: &Arc<RwLock<SilkroadSecurityState>>) -> FrameStep {
    let Some(frame_len) = SilkroadFrame::wire_len(buf) else {
        return FrameStep::Incomplete; // fewer than 2 bytes: length prefix not here yet
    };
    // Read before `parse`, which decrypts `buf` in place and drops the flag for
    // every frame kind but `Packet`.
    let encrypted = SilkroadFrame::wire_is_encrypted(buf).unwrap_or(false);
    // Smallest real frame is size(2) + opcode(2) + count(1) + crc(1); a shorter
    // length field means the stream is out of sync.
    if frame_len < 6 {
        return FrameStep::Corrupt;
    }
    if frame_len > buf.len() {
        return FrameStep::Incomplete; // partial tail — keep it for the next read
    }

    let frame = match SilkroadFrame::parse(&mut buf[..frame_len], security.clone()) {
        Ok((_, frame)) => frame,
        Err(e) => {
            debug!("network: failed to parse frame: {}", e);
            return FrameStep::Corrupt;
        }
    };
    // Full inbound-frame dump for the headless net-check client (filtered out at
    // info level, so free in the GUI build).
    trace!(target: "packets_in", "recv {}", frame);

    match frame {
        SilkroadFrame::Packet { opcode, data, .. } => FrameStep::Ready {
            opcode,
            data,
            consumed: frame_len,
            encrypted,
        },
        SilkroadFrame::MassiveHeader { opcode, amount, .. } => {
            // Reassemble the logical packet from the `amount` MassivePayload
            // frames that trail the header. If they have not all arrived, report
            // Incomplete so the header stays buffered and is retried next read.
            let mut body = BytesMut::new();
            let mut offset = frame_len;
            for _ in 0..amount {
                let Some(payload_len) = SilkroadFrame::wire_len(&buf[offset..]) else {
                    return FrameStep::Incomplete;
                };
                if offset + payload_len > buf.len() {
                    return FrameStep::Incomplete;
                }
                match SilkroadFrame::parse(&mut buf[offset..offset + payload_len], security.clone())
                {
                    Ok((_, payload @ SilkroadFrame::MassivePayload { .. })) => {
                        trace!(target: "packets_in", "recv {}", payload);
                        if let SilkroadFrame::MassivePayload { inner, .. } = payload {
                            body.extend_from_slice(&inner);
                        }
                        offset += payload_len;
                    }
                    _ => return FrameStep::Corrupt,
                }
            }
            FrameStep::Ready {
                opcode,
                data: body.freeze(),
                consumed: offset,
                encrypted,
            }
        }
        // A stray payload without a header, or any future frame kind.
        _ => FrameStep::Skip {
            consumed: frame_len,
        },
    }
}

fn receive_packets(
    mut query: Query<(Entity, &mut SilkroadConnection)>,
    mut commands: Commands,
    mut packets: MessageWriter<Packet>,
    mut dump: Option<ResMut<PacketDump>>,
) {
    for (entity, mut conn) in query.iter_mut() {
        let stream = conn.stream.clone();
        let Ok(mut stream) = stream.lock() else {
            continue;
        };
        let mut scratch = [0u8; 4096];
        match stream.read(&mut scratch) {
            Ok(read_bytes) => {
                // Append this read to the accumulator, then drain every complete
                // frame off the front. The server packs the post-join burst
                // (character-data begin/body/end, celestial, ...) back-to-back
                // and a single read can end mid-frame; carrying the unparsed
                // tail across reads is what keeps a split frame — and every
                // frame after it — from being dropped.
                conn.read_buf.extend_from_slice(&scratch[..read_bytes]);
                let security = conn.get_security();
                let mut consumed = 0;
                loop {
                    match next_packet(&mut conn.read_buf[consumed..], &security) {
                        FrameStep::Ready {
                            opcode,
                            data,
                            consumed: n,
                            encrypted,
                        } => {
                            if let Some(dump) = dump.as_mut() {
                                dump.dump(opcode, &data, encrypted);
                            }
                            dispatch_packet(Packet::deserialize(opcode, data), &mut packets);
                            consumed += n;
                        }
                        FrameStep::Skip { consumed: n } => consumed += n,
                        FrameStep::Incomplete => break,
                        FrameStep::Corrupt => {
                            warn!(
                                "network: desynchronised inbound stream, dropping {} buffered bytes",
                                conn.read_buf.len() - consumed
                            );
                            consumed = conn.read_buf.len();
                            break;
                        }
                    }
                }
                if consumed > 0 {
                    conn.read_buf.drain(..consumed);
                }
            }
            Err(err) => {
                if err.kind() != ErrorKind::WouldBlock {
                    error!("stop handling incoming packets because of: {}", err);
                    conn.dc_sender
                        .send(())
                        .expect("failed to send disconnected msg");
                    // Should we rather despawn? Currently the connection exists in its own entity
                    commands.entity(entity).remove::<SilkroadConnection>();
                }
            }
        }
    }
}

/// Present only when `network_settings.outbound_encryption` is on; its absence
/// is what keeps the plaintext-login behaviour of #243 as the default.
#[derive(Resource, Default)]
pub struct OutboundEncryption;

/// Opcodes whose outbound body contains the account password in clear text:
/// the gateway login `0x6102` and the agent login `0x6103` (`packets/src/lib.rs`,
/// `docs/net-login-gateway.md`).
fn carries_credentials(opcode: u16) -> bool {
    matches!(opcode, 0x6102 | 0x6103)
}

/// The `packets_out` trace line for a frame.
///
/// `make netcheck` runs with `packets_out=trace`, which hex-dumps every frame —
/// including the two that carry the password, so the account password ended up
/// in terminal scrollback and in anything pasted from it
/// (`docs/re/ui/scene-intro-autologin.md` §Security). Those two frames are
/// summarised instead of dumped; the on-disk `packet_dump/` is unaffected,
/// since offline payload analysis is its whole point.
fn outbound_trace_line(frame: &SilkroadFrame) -> String {
    let opcode = match frame {
        SilkroadFrame::Packet { opcode, .. } | SilkroadFrame::MassiveHeader { opcode, .. } => {
            Some(*opcode)
        }
        SilkroadFrame::MassivePayload { .. } => None,
    };
    match opcode {
        Some(opcode) if carries_credentials(opcode) => {
            format!("{opcode:#06X} [redacted: carries credentials]")
        }
        _ => format!("{frame}"),
    }
}

fn send_packets(
    mut query: Query<(Entity, &mut SilkroadConnection)>,
    mut commands: Commands,
    mut dump: Option<ResMut<PacketDump>>,
    encrypt_outbound: Option<Res<OutboundEncryption>>,
) {
    for (entity, conn) in query.iter_mut() {
        if conn.o_receiver.is_empty() {
            continue;
        }

        let stream = conn.stream.clone();
        let Ok(mut stream) = stream.lock() else {
            continue;
        };
        let security = conn.get_security();
        for mut frame in conn.o_receiver.try_iter() {
            // Full outbound-frame dump for the headless net-check client.
            trace!(target: "packets_out", "send {}", outbound_trace_line(&frame));
            // Mirror of the inbound dump, taken before `serialize` so the
            // recorded body stays plaintext once outbound encryption is on.
            if let Some(dump) = dump.as_mut() {
                match &frame {
                    SilkroadFrame::Packet { opcode, data, .. }
                    | SilkroadFrame::MassiveHeader { opcode, data, .. } => {
                        dump.dump_sent(*opcode, data)
                    }
                    // continuation chunks carry no opcode of their own
                    SilkroadFrame::MassivePayload { .. } => {}
                }
            }
            // Always applied: the gameplay opcodes the original encrypts per
            // packet (item use) are not part of the login-encryption opt-in.
            frame.apply_send_encryption(&security, encrypt_outbound.is_some());
            let output = frame
                .serialize(security.clone())
                .expect("failed to serialize frame");
            if let Err(err) = stream.write(&output) {
                if err.kind() != ErrorKind::WouldBlock {
                    error!("stop handling outgoing packets because of: {}", err);
                    conn.dc_sender
                        .send(())
                        .expect("failed to send disconnected msg");
                    // Should we rather despawn? Currently the connection exists in its own entity
                    commands.entity(entity).remove::<SilkroadConnection>();
                }
            }
        }
    }
}

fn fetch_disconnects(
    mut query: Query<(
        &mut SilkroadConnection,
        Option<&GatewayConnection>,
        Option<&AgentConnection>,
    )>,
    mut agent_dc_event_writer: MessageWriter<AgentConnectionDisconnected>,
    mut gateway_dc_event_writer: MessageWriter<GatewayServiceDisconnected>,
) {
    for (conn, gateway, agent) in query.iter_mut() {
        let disconnected = conn.get_dc_receiver().try_iter().next();
        if disconnected.is_some() {
            if gateway.is_some() {
                gateway_dc_event_writer.write(GatewayServiceDisconnected);
            } else if agent.is_some() {
                agent_dc_event_writer.write(AgentConnectionDisconnected);
            }
            continue;
        }
    }
}

fn keep_alive(
    mut query: Query<&mut SilkroadConnection>,
    time: Res<Time>,
    mut timer: Local<KeepAliveTimer>,
) {
    let timer = timer.timer.tick(time.delta());
    if !timer.just_finished() {
        return;
    }
    query.iter_mut().for_each(|conn| {
        let sender = conn.get_sender();
        let (opcode, data) = Packet::from(KeepAlive).into_serialize();
        let frame = SilkroadFrame::Packet {
            count: 0,
            crc: 0,
            opcode,
            encrypted: 0,
            data,
        };
        if let Err(e) = sender.send(frame) {
            error!("failed to send frame: {}", e.0);
        }
    });
}

pub(crate) fn on_global_state_update(mut events: MessageReader<GlobalStateUpdate>) {
    if let Some(res) = events.read().next() {
        info!("[GlobalStateUpdate]: {:?}", res);
    }
}

pub(crate) fn on_global_state_request(mut events: MessageReader<GlobalStateRequest>) {
    if let Some(res) = events.read().next() {
        info!("[GlobalStateRequest]: {:?}", res);
    }
}

fn on_agent_disconnected(
    mut reader: MessageReader<AgentConnectionDisconnected>,
    mut network_state: ResMut<NetworkState>,
    mut commands: Commands,
    query: Query<Entity, With<AgentConnection>>,
) {
    if let Some(_) = reader.read().next() {
        warn!("Agent Disconnected");
        network_state.agent = false;
        let Ok(entity) = query.single() else {
            return;
        };
        commands.entity(entity).despawn();
    }
}

fn on_gateway_disconnected(
    mut reader: MessageReader<GatewayServiceDisconnected>,
    mut network_state: ResMut<NetworkState>,
    mut commands: Commands,
    query: Query<Entity, With<GatewayConnection>>,
) {
    if let Some(_) = reader.read().next() {
        warn!("Gateway Disconnected");
        network_state.gateway = false;
        let Ok(entity) = query.single() else {
            return;
        };
        commands.entity(entity).despawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::blowfish::Blowfish;
    use crate::net::security::{SilkroadSecurity, SilkroadSecurityState};

    fn security() -> Arc<RwLock<SilkroadSecurityState>> {
        // `new()` leaves the state uninitialized, so `parse` takes the
        // unencrypted path and never needs a blowfish instance.
        Arc::new(RwLock::new(SilkroadSecurityState::new()))
    }

    /// `0x2110` is received and dropped by the original without reading its body
    /// (`00842ff0:90-92`), so it must be listed as known-and-ignored rather than
    /// wired — and it must stay out of the `packets!` macro.
    #[test]
    fn known_ignored_opcodes_cover_0x2110_and_it_stays_unwired() {
        assert!(KNOWN_IGNORED_OPCODES.contains(&0x2110));
        assert!(matches!(
            Packet::deserialize(0x2110, Bytes::new()),
            Err(PacketError::UnknownOpcode(0x2110))
        ));
    }

    /// `0x3C81`'s handler in the original reads no bytes at all, so it is
    /// ignored rather than typed (#261). The second assertion is the one that
    /// matters: if someone later adds a body for it to the `packets!` macro,
    /// the ignore entry becomes a lie and this fails.
    #[test]
    fn academy_data_0x3c81_is_ignored_and_stays_unwired() {
        assert!(KNOWN_IGNORED_OPCODES.contains(&0x3C81));
        assert!(matches!(
            Packet::deserialize(0x3C81, Bytes::new()),
            Err(PacketError::UnknownOpcode(0x3C81))
        ));
    }

    /// A `SilkroadFrame::Packet` carrying `data`, as `send_packets` sees it.
    fn outbound_packet(opcode: u16, data: &[u8]) -> SilkroadFrame {
        SilkroadFrame::Packet {
            count: 0,
            crc: 0,
            opcode,
            encrypted: 0,
            data: Bytes::copy_from_slice(data),
        }
    }

    /// The password must not reach the `packets_out` trace: `make netcheck`
    /// runs that target at trace level, so anything logged there lands in
    /// terminal scrollback (`docs/re/ui/scene-intro-autologin.md` §Security).
    #[test]
    fn outbound_trace_redacts_the_login_frames() {
        // 0x6102 LoginRequest body shape: content_id | username | password | shard
        let mut body = vec![22u8];
        body.extend_from_slice(&4u16.to_le_bytes());
        body.extend_from_slice(b"user");
        body.extend_from_slice(&7u16.to_le_bytes());
        body.extend_from_slice(b"hunter2");
        body.extend_from_slice(&1u16.to_le_bytes());

        let gateway = outbound_trace_line(&outbound_packet(0x6102, &body));
        assert!(!gateway.contains("hunter2"), "{gateway}");
        assert!(
            !gateway.to_ascii_uppercase().contains("68756E74657232"),
            "{gateway}"
        );
        assert!(
            gateway.contains("0x6102") && gateway.contains("redacted"),
            "{gateway}"
        );

        // the agent login (0x6103) repeats the same password
        let agent = outbound_trace_line(&outbound_packet(0x6103, &body));
        assert!(!agent.contains("hunter2"), "{agent}");
        assert!(agent.contains("0x6103"), "{agent}");
    }

    /// Every other opcode keeps its full dump — the trace's whole purpose.
    #[test]
    fn outbound_trace_keeps_non_credential_frames_intact() {
        let line = outbound_trace_line(&outbound_packet(0x7021, &[0x01, 0x02, 0x03]));
        assert!(line.contains("Packet"), "{line}");
        assert!(!line.contains("redacted"), "{line}");
    }

    /// Build an unencrypted `Packet` frame: size | opcode | count | crc | data.
    fn packet_frame(opcode: u16, data: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&(data.len() as u16).to_le_bytes());
        v.extend_from_slice(&opcode.to_le_bytes());
        v.push(0); // count
        v.push(0); // crc
        v.extend_from_slice(data);
        v
    }

    fn massive_header_frame(inner_opcode: u16, amount: u16) -> Vec<u8> {
        let mut content = vec![1u8]; // header flag
        content.extend_from_slice(&amount.to_le_bytes());
        content.extend_from_slice(&inner_opcode.to_le_bytes());
        packet_frame(0x600D, &content)
    }

    fn massive_payload_frame(inner: &[u8]) -> Vec<u8> {
        let mut content = vec![0u8]; // payload flag
        content.extend_from_slice(inner);
        packet_frame(0x600D, &content)
    }

    /// Mirror of the `receive_packets` drain loop over an in-memory buffer.
    fn drain(
        buf: &mut Vec<u8>,
        security: &Arc<RwLock<SilkroadSecurityState>>,
    ) -> Vec<(u16, Vec<u8>)> {
        let mut out = Vec::new();
        let mut consumed = 0;
        loop {
            match next_packet(&mut buf[consumed..], security) {
                FrameStep::Ready {
                    opcode,
                    data,
                    consumed: n,
                    ..
                } => {
                    out.push((opcode, data.to_vec()));
                    consumed += n;
                }
                FrameStep::Skip { consumed: n } => consumed += n,
                FrameStep::Incomplete => break,
                FrameStep::Corrupt => {
                    consumed = buf.len();
                    break;
                }
            }
        }
        buf.drain(..consumed);
        out
    }

    /// #459: `packet_dump` recorded `<ts> <hex>` and nothing else, so a padded
    /// encrypted body and a plaintext one of the same length were
    /// indistinguishable when re-reading an old log. `next_packet` now carries
    /// the wire `0x8000` bit out to the dumper. Payload fixture is the first
    /// captured line of `packet_dump/0x3020.log`.
    #[test]
    fn next_packet_reports_the_wire_encryption_bit() {
        let payload = [0xb5u8, 0xa8, 0x01, 0x00, 0xd7, 0x01, 0x07, 0x11];

        let plain_security = security();
        let mut plain = packet_frame(0x3020, &payload);
        match next_packet(&mut plain, &plain_security) {
            FrameStep::Ready {
                opcode, encrypted, ..
            } => {
                assert_eq!(opcode, 0x3020);
                assert!(!encrypted, "plaintext frame reported as encrypted");
            }
            _ => panic!("expected a ready frame"),
        }

        // Same payload, but on the wire with the 0x8000 bit: round-trip it
        // through `serialize` so the bytes are a real encrypted frame.
        let established = {
            let mut state = SilkroadSecurityState::new();
            state.state = SilkroadSecurity::Established;
            state.context.blowfish = Some(Blowfish::new(&[0u8; 8]).expect("test key"));
            Arc::new(RwLock::new(state))
        };
        let mut wire = SilkroadFrame::Packet {
            count: 0,
            crc: 0,
            opcode: 0x3020,
            encrypted: 1,
            data: Bytes::copy_from_slice(&payload),
        }
        .serialize(established.clone())
        .expect("serialize")
        .to_vec();
        match next_packet(&mut wire, &established) {
            FrameStep::Ready {
                opcode,
                data,
                encrypted,
                ..
            } => {
                assert_eq!(opcode, 0x3020);
                assert!(encrypted, "encrypted frame reported as plaintext");
                // The body is block-padded, which is exactly why the flag has
                // to be recorded: its length alone does not identify it.
                assert_eq!(&data[..payload.len()], &payload);
            }
            _ => panic!("expected a ready frame"),
        }
    }

    #[test]
    fn drains_multiple_frames_in_one_pass() {
        let sec = security();
        let mut buf = packet_frame(0x3013, &[1, 2, 3]);
        buf.extend(packet_frame(0xB001, &[9]));

        let out = drain(&mut buf, &sec);

        assert_eq!(out, vec![(0x3013, vec![1, 2, 3]), (0xB001, vec![9])]);
        assert!(buf.is_empty(), "fully drained buffer must be empty");
    }

    #[test]
    fn frame_split_across_reads_is_not_dropped() {
        let sec = security();
        let mut full = packet_frame(0x3013, &[1, 2, 3, 4, 5]);
        full.extend(packet_frame(0xB070, &[7, 7]));

        // First read ends inside the first frame's body.
        let split = 4;
        let mut buf: Vec<u8> = full[..split].to_vec();
        let first = drain(&mut buf, &sec);
        assert!(first.is_empty(), "no whole frame yet");
        assert_eq!(buf.len(), split, "partial tail must be retained");

        // Rest of the stream arrives.
        buf.extend_from_slice(&full[split..]);
        let rest = drain(&mut buf, &sec);

        assert_eq!(
            rest,
            vec![(0x3013, vec![1, 2, 3, 4, 5]), (0xB070, vec![7, 7])]
        );
        assert!(buf.is_empty());
    }

    #[test]
    fn split_between_frames_keeps_the_second() {
        let sec = security();
        let first = packet_frame(0x3013, &[1, 2, 3]);
        let second = packet_frame(0xB070, &[8, 9]);
        let mut full = first.clone();
        full.extend(second.clone());

        // First read: whole first frame + half of the second.
        let split = first.len() + 3;
        let mut buf: Vec<u8> = full[..split].to_vec();
        let out = drain(&mut buf, &sec);
        assert_eq!(out, vec![(0x3013, vec![1, 2, 3])]);
        assert_eq!(
            buf.len(),
            split - first.len(),
            "only the second frame's head remains"
        );

        buf.extend_from_slice(&full[split..]);
        let out = drain(&mut buf, &sec);
        assert_eq!(out, vec![(0xB070, vec![8, 9])]);
        assert!(buf.is_empty());
    }

    #[test]
    fn massive_packet_reassembled_across_reads() {
        let sec = security();
        let mut full = massive_header_frame(0x34A5, 2);
        full.extend(massive_payload_frame(&[1, 2, 3]));
        full.extend(massive_payload_frame(&[4, 5]));

        // First read: header + only the first of two payloads.
        let after_first_payload =
            massive_header_frame(0x34A5, 2).len() + massive_payload_frame(&[1, 2, 3]).len();
        let mut buf: Vec<u8> = full[..after_first_payload].to_vec();
        let out = drain(&mut buf, &sec);
        assert!(
            out.is_empty(),
            "massive packet incomplete — nothing dispatched yet"
        );
        assert_eq!(
            buf.len(),
            after_first_payload,
            "header + partial payloads retained"
        );

        // Second payload arrives; the logical packet reassembles.
        buf.extend_from_slice(&full[after_first_payload..]);
        let out = drain(&mut buf, &sec);
        assert_eq!(out, vec![(0x34A5, vec![1, 2, 3, 4, 5])]);
        assert!(buf.is_empty());
    }

    /// #130: `next_packet` catches parse errors so a desynchronised stream can
    /// resynchronise, but the 0x600D arm read its flag and header fields off
    /// the body with no length check — and `Buf::get_*` panics on underflow.
    /// Random bytes that happen to read as 0x600D with a short body therefore
    /// aborted the process instead of dropping the buffer. All three shapes
    /// clear the only structural pre-filter (`frame_len >= 6`).
    #[test]
    fn truncated_massive_frames_are_corrupt_not_a_panic() {
        for (name, body) in [
            ("empty body - no flag byte", vec![]),
            ("header flag, no amount/opcode", vec![1u8]),
            ("header flag, amount only", vec![1u8, 2, 0]),
        ] {
            let sec = security();
            let mut buf = packet_frame(0x600D, &body);
            let out = drain(&mut buf, &sec);
            assert!(out.is_empty(), "{name}: nothing may be dispatched");
            assert!(buf.is_empty(), "{name}: corrupt frame drops the buffer");
        }
    }

    /// The guard must not reject a legitimately empty massive chunk: a payload
    /// flag with no inner bytes carries the flag byte and nothing else.
    #[test]
    fn massive_payload_with_empty_inner_still_parses() {
        let sec = security();
        let mut buf = massive_header_frame(0x34A5, 1);
        buf.extend(massive_payload_frame(&[]));
        let out = drain(&mut buf, &sec);
        assert_eq!(out, vec![(0x34A5, vec![])]);
        assert!(buf.is_empty());
    }
}
