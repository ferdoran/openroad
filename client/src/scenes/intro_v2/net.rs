use bevy::prelude::*;
use bevy::text::EditableText;
use bevy::ui::{InteractionDisabled, Pressed};
use bevy::ui_widgets::Activate;
use crossbeam::channel::{Receiver, TryRecvError};
use mac_address::get_mac_address;

use packets::agent::{describe_agent_auth_error, AgentLoginRequest, AgentLoginResponse};
use packets::login::{describe_login_error, LoginFailure, LoginRequest, LoginResponse};
use packets::Packet;

use crate::net::connection::{PendingConnection, SilkroadConnection};
use crate::plugins::config::division::DivisionInfo;
use crate::plugins::net::agent::AgentConnectionBundle;
use crate::plugins::net::gateway::{GatewayConnection, GatewayConnectionStatus};
use crate::plugins::net::plugin::NetworkState;
use crate::plugins::settings::options::GameOptions;

use super::assets::IntroV2Assets;
use super::chrome::InfoTextV2Update;
use super::fade::{FadeToBlack, FadeToBlackTimer};
use super::login_form::{ConnectButton, IdInput, PwInput};
use super::server_select::SelectedShardV2;
use super::IntroV2State;

/// The credentials used to log in, set by [`on_connect_activate`] from the form
/// inputs (or by dev_fast_login from config). [`on_gateway_login_response`] reads
/// them for the follow-up agent login instead of re-reading the widgets, which
/// lets dev_fast_login drive the flow without touching the `EditableText` inputs.
#[derive(Resource, Clone)]
pub struct LoginCredentials {
    pub username: String,
    pub password: String,
}

/// Clears the disabled state left on the Connect button by
/// [`on_connect_activate`] when the login form is (re-)entered, e.g. after
/// cancelling out of the character selection. Also strips a stale `Pressed`:
/// the button's release observer skips its removal on disabled buttons, which
/// would leave the press art stuck.
pub fn reenable_connect_button(
    query: Query<
        Entity,
        (
            With<ConnectButton>,
            Or<(With<InteractionDisabled>, With<Pressed>)>,
        ),
    >,
    mut commands: Commands,
) {
    for entity in query.iter() {
        commands
            .entity(entity)
            .remove::<(InteractionDisabled, Pressed)>();
    }
}

/// `Activate` observer of the Connect button: sends the gateway login
/// request. Port of the old `on_connect_button_clicked_system`.
pub fn on_connect_activate(
    activate: On<Activate>,
    id_query: Query<&EditableText, With<IdInput>>,
    pw_query: Query<&EditableText, With<PwInput>>,
    mut info_text_writer: MessageWriter<InfoTextV2Update>,
    selected_shard: Res<SelectedShardV2>,
    gateway_query: Query<&SilkroadConnection, With<GatewayConnection>>,
    assets: Res<IntroV2Assets>,
    division: Res<DivisionInfo>,
    options: Res<GameOptions>,
    mut commands: Commands,
) {
    let Ok(id_input) = id_query.single() else {
        return;
    };
    let Ok(pw_input) = pw_query.single() else {
        return;
    };

    let username = id_input.value().to_string().trim().to_string();
    let password = pw_input.value().to_string().trim().to_string();

    let Some(shard_id) = selected_shard.0 else {
        return;
    };

    // Remembered for the follow-up agent login (see `on_gateway_login_response`).
    commands.insert_resource(LoginCredentials {
        username: username.clone(),
        password: password.clone(),
    });

    let Ok(connection) = gateway_query.single() else {
        return;
    };

    info!("[Login] user = {}, shard_id = {}", username, shard_id);
    // Also clear Pressed: the button's release observer won't remove it once
    // the button is disabled, leaving the press art stuck.
    commands
        .entity(activate.entity)
        .insert(InteractionDisabled)
        .remove::<Pressed>();
    if let Some(playback) = options.audio.fx_playback() {
        commands.spawn((AudioPlayer::new(assets.sound_error.clone()), playback));
    }
    info_text_writer.write(InfoTextV2Update(String::from(
        "...Requesting user confirmation...",
    )));

    let frame = Packet::from(LoginRequest {
        content_id: division.content_id,
        username,
        password,
        shard_id,
    })
    .into();

    if let Err(e) = connection.get_sender().send(frame) {
        error!("failed to send frame: {}", e.0);
    }
}

/// Surfaces an asynchronous gateway connect failure as intro info text. Because
/// the connect runs off-thread, the failure can arrive after the login form is
/// already visible; this runs both on login-form entry (to show a failure that
/// was already pending) and whenever [`GatewayConnectionStatus`] changes.
pub fn surface_gateway_error(
    status: Res<GatewayConnectionStatus>,
    mut info_text_writer: MessageWriter<InfoTextV2Update>,
) {
    if let GatewayConnectionStatus::Failed(msg) = &*status {
        info_text_writer.write(InfoTextV2Update(msg.clone()));
    }
}

/// Handles the gateway's login response: surfaces errors on the info text or
/// opens the agent connection. Port of the old `on_gateway_login_response`.
pub fn on_gateway_login_response(
    mut event_reader: MessageReader<LoginResponse>,
    mut info_text_writer: MessageWriter<InfoTextV2Update>,
    mut commands: Commands,
    mut network_state: ResMut<NetworkState>,
    credentials: Option<Res<LoginCredentials>>,
    gateway_query: Query<Entity, With<GatewayConnection>>,
    connect_button_query: Query<Entity, With<ConnectButton>>,
    division: Res<DivisionInfo>,
) {
    let Some(credentials) = credentials else {
        return;
    };

    let Some(res) = event_reader.read().next() else {
        return;
    };

    let Ok(gateway_entity) = gateway_query.single() else {
        return;
    };

    let Ok(connect_button) = connect_button_query.single() else {
        return;
    };

    let username = credentials.username.clone();
    let password = credentials.password.clone();

    if let Some(err) = &res.login_error {
        commands
            .entity(connect_button)
            .remove::<InteractionDisabled>();
        error!("[Login Error]: {:?}", err);
        if let Some(blocked_err) = &err.account_blocked_err {
            if let Some(ban_info) = &blocked_err.ban_info {
                info_text_writer.write(InfoTextV2Update(ban_info.reason.clone()));
            }
        }

        if let Some(wrong_attempt) = &err.wrong_attempt {
            let msg = format!(
                "Password entry has failed {} out of {} times.",
                wrong_attempt.cur_attempts, wrong_attempt.max_attempts
            );
            info_text_writer.write(InfoTextV2Update(msg));
        }

        // Codes with a payload rendered their own message above; everything
        // else used to render nothing at all (#465). The original shows one
        // textuisystem string per code — until those are wired, name the code.
        match err.failure() {
            LoginFailure::AlreadyConnected => {
                info_text_writer.write(InfoTextV2Update(String::from(
                    "This user is already connected. The user may still be connected because of an error that forced the game to close. Please try again in 5 minutes.",
                )));
            }
            LoginFailure::WrongPassword(_) | LoginFailure::Blocked(_) => {}
            _ => {
                info_text_writer.write(InfoTextV2Update(String::from(describe_login_error(
                    err.error_code,
                ))));
            }
        }
    } else {
        commands.entity(gateway_entity).despawn();
        network_state.gateway = false;
        let info = res.login_info.clone().unwrap();

        // Both the agent connect (TCP + handshake) and the MAC lookup (an adapter
        // enumeration on Windows) block; run inline here they froze the frame for
        // ~0.6 s at login. `poll_agent_login` finishes the handoff once both land.
        commands.spawn((
            PendingAgentLogin {
                connection: SilkroadConnection::connect_async(&format!(
                    "{}:{}",
                    info.agent_ip, info.agent_port
                )),
                mac: spawn_mac_lookup(),
                mac_address: None,
                token: info.agent_token,
                username,
                password,
                content_id: division.content_id,
            },
            Name::from("AgentLogin (connecting)"),
        ));
    }
}

/// An agent-server handoff in flight: the connect + handshake and the MAC lookup
/// both run on worker threads, and [`poll_agent_login`] sends the
/// [`AgentLoginRequest`] once both have delivered. Kept on its own entity rather
/// than on an [`AgentConnection`](crate::plugins::net::agent::AgentConnection)
/// so nothing that queries agent connections ever sees one without its socket.
#[derive(Component)]
pub struct PendingAgentLogin {
    connection: PendingConnection,
    mac: Receiver<[u8; 6]>,
    mac_address: Option<[u8; 6]>,
    token: u32,
    username: String,
    password: String,
    content_id: u8,
}

/// Looks up the local MAC address on a worker thread. A machine without one
/// (or a failed lookup) sends zeros, as the headless net-check client does.
fn spawn_mac_lookup() -> Receiver<[u8; 6]> {
    let (tx, rx) = crossbeam::channel::bounded(1);
    let spawned = std::thread::Builder::new()
        .name("sro-mac-lookup".to_string())
        .spawn(move || {
            let mac = get_mac_address().ok().flatten().map(|m| m.bytes());
            if mac.is_none() {
                warn!("no MAC address found; sending zeros in the agent login");
            }
            let _ = tx.send(mac.unwrap_or([0; 6]));
        });
    if spawned.is_err() {
        // The closure (and `tx`) was dropped; `poll_agent_login` sees the
        // disconnect and falls back to zeros instead of waiting forever.
        warn!("failed to spawn the MAC lookup thread");
    }
    rx
}

/// Completes a [`PendingAgentLogin`]: once the MAC is known and the agent
/// connection is established, sends the agent login and replaces the pending
/// entity with the [`AgentConnectionBundle`].
pub fn poll_agent_login(
    mut pending: Query<(Entity, &mut PendingAgentLogin)>,
    mut network_state: ResMut<NetworkState>,
    mut info_text_writer: MessageWriter<InfoTextV2Update>,
    mut commands: Commands,
) {
    for (entity, mut login) in pending.iter_mut() {
        if login.mac_address.is_none() {
            login.mac_address = match login.mac.try_recv() {
                Ok(mac) => Some(mac),
                Err(TryRecvError::Empty) => continue,
                Err(TryRecvError::Disconnected) => Some([0; 6]),
            };
        }
        let Some(result) = login.connection.poll() else {
            continue;
        };
        commands.entity(entity).despawn();
        let conn = match result {
            Ok(conn) => conn,
            Err(err) => {
                error!("failed to connect to agent server: {}", err);
                info_text_writer.write(InfoTextV2Update(format!(
                    "Failed to connect to server: {err}"
                )));
                continue;
            }
        };
        network_state.agent = true;
        let frame = Packet::from(AgentLoginRequest {
            token: login.token,
            username: std::mem::take(&mut login.username),
            password: std::mem::take(&mut login.password),
            content_id: login.content_id,
            mac_address: login.mac_address.unwrap_or([0; 6]),
        })
        .into();
        if let Err(e) = conn.get_sender().send(frame) {
            error!("failed to send frame: {}", e.0);
        }
        commands.spawn(AgentConnectionBundle::new(conn));
    }
}

/// On successful agent login, fade to black and enter character selection.
/// Port of the old `on_agent_login_response`.
pub fn on_agent_login_response(
    mut reader: MessageReader<AgentLoginResponse>,
    mut info_text_writer: MessageWriter<InfoTextV2Update>,
    mut fade_writer: MessageWriter<FadeToBlack>,
    connect_button_query: Query<Entity, With<ConnectButton>>,
    mut commands: Commands,
) {
    for res in reader.read() {
        if let Some(e) = &res.error_code {
            error!("[Agent]: Login failed because of {:?}", e);
            // A failed agent login used to end here. The screen then kept the
            // "requesting user confirmation" line and a Connect button that
            // stays disabled — the re-enable lives in the *gateway* branch,
            // and the gateway login had succeeded. Nothing but Alt+F4 got the
            // player out, while the headless netcheck named the code all
            // along.
            info_text_writer.write(InfoTextV2Update(format!(
                "Login to the game server failed: {}.",
                describe_agent_auth_error(*e)
            )));
            if let Ok(connect_button) = connect_button_query.single() {
                commands
                    .entity(connect_button)
                    .remove::<InteractionDisabled>();
            }
        } else {
            info_text_writer.write(InfoTextV2Update(String::new()));
            commands.insert_resource(FadeToBlackTimer::to(IntroV2State::CharacterList));
            fade_writer.write(FadeToBlack);
        }
    }
}

/// Enter presses Connect on the login form.
///
/// IDEA. The same trick the HUD uses for its dialogs (`hud::focus`): the key
/// does not re-implement what the button does, it triggers `Activate` on the
/// button itself. Connect already carries the whole login path, so the key and
/// the click cannot drift apart. Without this the pre-game had exactly one
/// key handler in fourteen files, and the tab ring could not reach Connect at
/// all — you could not log in without a mouse.
pub fn enter_presses_connect(
    keys: Res<ButtonInput<KeyCode>>,
    connect_button: Query<Entity, (With<ConnectButton>, Without<InteractionDisabled>)>,
    mut commands: Commands,
) {
    if !keys.just_pressed(KeyCode::Enter) && !keys.just_pressed(KeyCode::NumpadEnter) {
        return;
    }
    let Ok(entity) = connect_button.single() else {
        return;
    };
    commands.trigger(Activate { entity });
}

#[cfg(test)]
mod login_deadend_test {
    use super::*;

    /// The dead end this slice closes: a failed agent login used to leave the
    /// Connect button disabled, because the only re-enable sits in the gateway
    /// branch — and the gateway login had succeeded. Asserting the message
    /// path here would need a full app; what is worth pinning is that every
    /// code the client can receive produces a line with content, so the player
    /// never gets an empty info bar in place of a frozen one.
    #[test]
    fn every_agent_auth_code_names_something() {
        for code in 0u8..=16 {
            let text = describe_agent_auth_error(code);
            assert!(!text.is_empty(), "code {code} has no description");
        }
        // The three the packet crate classifies, so a renamed arm shows up here
        // rather than silently degrading to "unknown error" on screen.
        assert_eq!(describe_agent_auth_error(3), "already connected");
        assert_eq!(describe_agent_auth_error(4), "server full");
        assert_eq!(describe_agent_auth_error(5), "IP limit reached");
        assert_eq!(describe_agent_auth_error(2), "unknown error");
    }
}
