//! Optional developer fast-login for the intro_v2 scene (config
//! `dev_fast_login`).
//!
//! Idea: drive the exact same packet flow the manual UI does, but from
//! `ClientConfig.dev_fast_login` instead of the widgets — skip the splash, fire
//! the login once the shard list is up, answer the captcha with the configured
//! code, and join the first character. The existing gateway/agent login response
//! handlers and the `SceneState::GameWorld` transition are reused unchanged;
//! these systems only supply the inputs the manual path would have. Every system
//! is gated on [`enabled`].
//!
//! This is **openroad-only** behaviour with zero counterpart in the original —
//! and it is not the original's "Auto Login", which is a login queue
//! (`docs/re/ui/scene-intro-autologin.md`). It stays off by default.
//!
//! The once-per-visit guards live in [`FastLoginProgress`] rather than in
//! `Local`s: a `Local` survives leaving and re-entering the intro scene, so a
//! second visit skipped the splash but never re-sent the login, leaving the
//! scene wedged.

use bevy::prelude::*;

use packets::agent::prelude::{CharacterJoinRequest, CharacterSelectionActionResponse};
use packets::login::{LoginCaptchaChallenge, LoginCaptchaConfirmRequest, LoginRequest};
use packets::Packet;

use crate::net::connection::SilkroadConnection;
use crate::plugins::config::division::DivisionInfo;
use crate::plugins::config::ClientConfig;
use crate::plugins::net::agent::AgentConnection;
use crate::plugins::net::gateway::shard_list::ShardList;
use crate::plugins::net::gateway::GatewayConnection;

use super::captcha::{CaptchaImageV2, CaptchaModal};
use super::character_select::{JoiningCharacter, PendingWorldJoin};
use super::net::LoginCredentials;
use super::server_select::SelectedShardV2;
use super::IntroV2State;

/// Run condition: developer fast-login is turned on in the config.
pub fn enabled(config: Res<ClientConfig>) -> bool {
    config.dev_fast_login.enabled
}

/// Per-visit progress of the fast-login flow.
///
/// Both flags are one-shot *per visit to the intro scene*, and both are cleared
/// by [`reset_progress`] on entering it. Keeping them in a resource rather than
/// in `Local`s is the fix for two defects:
///
/// - **re-entry dead-lock**: a `Local` survives the scene, so on a second visit
///   the splash was still skipped but the login was never re-sent;
/// - **captcha re-fire**: an unguarded handler answered every challenge with the
///   same code, so a rejected code was resent in a loop instead of handing the
///   challenge back to the user.
#[derive(Resource, Default, Debug, PartialEq, Eq)]
pub struct FastLoginProgress {
    pub login_sent: bool,
    pub captcha_answered: bool,
}

/// Clear the one-shot guards when the intro scene is (re-)entered.
pub fn reset_progress(mut progress: ResMut<FastLoginProgress>) {
    *progress = FastLoginProgress::default();
}

/// Skip the splash screen straight to the login form.
pub fn skip_splash(mut next: ResMut<NextState<IntroV2State>>) {
    next.set(IntroV2State::LoginForm);
}

/// Once the shard list and gateway connection are ready, pick the first
/// operating shard and send the login request with the configured credentials.
/// Fires once per visit to the intro scene ([`FastLoginProgress`]).
pub fn send_login(
    mut progress: ResMut<FastLoginProgress>,
    config: Res<ClientConfig>,
    shard_list: Option<Res<ShardList>>,
    mut selected_shard: ResMut<SelectedShardV2>,
    gateway: Query<&SilkroadConnection, With<GatewayConnection>>,
    division: Res<DivisionInfo>,
    mut commands: Commands,
) {
    if progress.login_sent {
        return;
    }
    let Some(shard_list) = shard_list else {
        return;
    };
    let Ok(conn) = gateway.single() else {
        return;
    };
    // Prefer an operating shard; fall back to the first listed.
    let Some(shard) = shard_list
        .0
        .shards
        .iter()
        .find(|s| s.is_operating)
        .or_else(|| shard_list.0.shards.first())
    else {
        warn!("dev_fast_login: shard list is empty");
        return;
    };

    let username = config.dev_fast_login.username.clone();
    let password = config.dev_fast_login.password.clone();
    selected_shard.0 = Some(shard.id);
    // Remembered for the follow-up agent login (see `net::on_gateway_login_response`).
    commands.insert_resource(LoginCredentials {
        username: username.clone(),
        password: password.clone(),
    });

    let frame = Packet::from(LoginRequest {
        content_id: division.content_id,
        username,
        password,
        shard_id: shard.id,
    })
    .into();
    if let Err(e) = conn.get_sender().send(frame) {
        error!("dev_fast_login: failed to send LoginRequest: {}", e.0);
        return;
    }
    info!(
        "dev_fast_login: sent LoginRequest for '{}' on shard {} ({})",
        config.dev_fast_login.username, shard.id, shard.name
    );
    progress.login_sent = true;
}

/// Answer the IBUV challenge with the configured code and tear down the modal
/// if the manual path already spawned it.
///
/// Two things this deliberately does **not** do. It does not invent a code: the
/// challenge is a server-generated image, so with no `captcha_answer` configured
/// the only correct move is to leave the modal up and let the user read it —
/// the previously hardcoded `"1"` was a guess that could not survive a real
/// challenge. And it answers at most once per visit: a second challenge means
/// the code was rejected, so resending it would loop forever.
pub fn answer_captcha(
    mut events: MessageReader<LoginCaptchaChallenge>,
    config: Res<ClientConfig>,
    mut progress: ResMut<FastLoginProgress>,
    gateway: Query<&SilkroadConnection, With<GatewayConnection>>,
    modal: Query<Entity, With<CaptchaModal>>,
    mut commands: Commands,
) {
    if events.read().count() == 0 {
        return;
    }
    if progress.captcha_answered {
        info!("dev_fast_login: captcha challenged again — handing it to the modal");
        return;
    }
    let Some(code) = config.dev_fast_login.captcha_answer.clone() else {
        info!("dev_fast_login: no captcha_answer configured — leaving the modal up");
        return;
    };
    let Ok(conn) = gateway.single() else {
        return;
    };
    let frame = Packet::from(LoginCaptchaConfirmRequest { code }).into();
    if let Err(e) = conn.get_sender().send(frame) {
        error!("dev_fast_login: failed to send captcha confirm: {}", e.0);
        return;
    }
    progress.captcha_answered = true;
    // Suppress the manual modal (harmless if it never spawned).
    commands.remove_resource::<CaptchaImageV2>();
    for entity in modal.iter() {
        commands.entity(entity).despawn();
    }
    info!("dev_fast_login: submitted the configured captcha answer");
}

/// On the character list response, join the first character (mirrors
/// `character_select::on_start_activate`, minus the UI). The existing
/// `on_character_join_response` then transitions to the game world.
pub fn join_first_character(
    pending: Option<Res<PendingWorldJoin>>,
    mut reader: MessageReader<CharacterSelectionActionResponse>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    mut commands: Commands,
) {
    if pending.is_some() {
        return;
    }
    let mut first_character = None;
    for res in reader.read() {
        if let Some(characters) = &res.characters {
            if let Some(first) = characters.characters.first() {
                first_character = Some(first.clone());
            }
        }
    }
    let Some(first) = first_character else {
        return;
    };
    let Ok(conn) = conn.single() else {
        return;
    };

    let frame = Packet::from(CharacterJoinRequest {
        character_name: first.name.clone(),
    })
    .into();
    if let Err(e) = conn.get_sender().send(frame) {
        error!(
            "dev_fast_login: failed to send CharacterJoinRequest: {}",
            e.0
        );
        return;
    }
    commands.insert_resource(PendingWorldJoin {
        character_name: first.name.clone(),
    });
    commands.insert_resource(JoiningCharacter(first.clone()));
    info!("dev_fast_login: joining first character '{}'", first.name);
}
