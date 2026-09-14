//! The in-game system window (Esc menu) and the logout flow behind its
//! Exit/Restart buttons.
//!
//! Idea: pressing Esc in `GameWorld` toggles a small centered window (layout
//! transcribed from `Media.pk2/resinfo/ifsystemwnd.txt`) with Option / Help /
//! Restart / Exit. Restart and Exit don't act immediately — they drive the SRO
//! logout handshake: the button sends a `LogoutRequest` (mode Exit/Restart), the
//! server replies with a countdown, and the authoritative `LogoutSuccess`
//! (0x300A) triggers the actual quit (`AppExit`) or restart (disconnect + back
//! to the login scene). Esc during the countdown cancels the logout. Both
//! countdown texts are the vanilla `UIIT_MSG_LOGOUT_REMAIN_TIME*` strings, which
//! are mode-agnostic — one message for Exit and Restart alike.
//!
//! The window uses the vanilla art: a 9-slice `mframe_wnd_*` frame, `sys_button`
//! for the buttons (with focus/press states) and `com_windowclose` for the X.

use bevy::app::AppExit;
use bevy::ecs::relationship::RelatedSpawnerCommands;
use bevy::ecs::system::IntoObserverSystem;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui::{InteractionDisabled, Pressed, UiTargetCamera};
use bevy::ui_widgets::{Activate, Button};

use packets::agent::prelude::{
    LogoutCancelRequest, LogoutCancelResponse, LogoutRequest, LogoutResponse, LogoutSuccess,
    LOGOUT_MODE_EXIT, LOGOUT_MODE_RESTART,
};
use packets::Packet;

use crate::net::connection::SilkroadConnection;
use crate::plugins::hud::chat::model::{ChatHistory, ChatLine};
use crate::plugins::net::agent::AgentConnection;
use crate::plugins::player::PlayerMoveOrder;
use crate::plugins::small_popup::spawn_small_popup;
use crate::plugins::textdata::ClientUiStrings;
use crate::plugins::ui_v2::style::ImageButtonStyle;
use crate::scenes::SceneState;

// Button rects from `resinfo/ifsystemwnd.txt` `Section = Create`: all five share
// x=31 and carry `0,0` sizes, so the extent is `sys_button.ddj`'s own 152x24.
// The layout confirms the width independently — 214 - (31 + 152) = 31 leaves a
// symmetric right margin. `GDR_SYSTEM_BTN_CAS_HELP` (:44, ID 12, y=125, "Help
// Request") is deliberately not built: the customer-service flow behind it is
// unreachable for us today — whether it is protocol-blocked or merely unbuilt is
// unresolved (docs/re/ui/system-window.md §9-U5) — so its slot stays empty
// rather than being stubbed.
const BTN_X: f32 = 31.0;
const BTN_W: f32 = 152.0;
const BTN_H: f32 = 24.0;
const BTN_Y_OPTION: f32 = 58.0;
const BTN_Y_HELP: f32 = 92.0;
const BTN_Y_RESTART: f32 = 159.0;
const BTN_Y_QUIT: f32 = 192.0;

/// Button label colour, uniform across all five buttons
/// (`ifsystemwnd.txt:11,30,49,68,87` `FontColor=COLOR,"255,254,251,216"`).
/// Read as ARGB that is an opaque pale cream, not a translucent white.
const BTN_LABEL_COLOR: Color = Color::srgb_u8(254, 251, 216);

const SYS_BUTTON_DDJ: &str = "media://interface/system/sys_button.ddj";
const SYS_BUTTON_FOCUS_DDJ: &str = "media://interface/system/sys_button_focus.ddj";
const SYS_BUTTON_PRESS_DDJ: &str = "media://interface/system/sys_button_press.ddj";
const BUTTON_FONT: &str = crate::assets::BUNDLED_FALLBACK_FACE;

// The vanilla logout messages, `server_dep/silkroad/textdata/textuisystem.txt`
// :1753 and :1754. Both are mode-agnostic — the original shows one message for
// Exit and Restart alike — and only the countdown carries a placeholder, a
// single `%d`.
const LOGOUT_REMAIN_KEY: &str = "UIIT_MSG_LOGOUT_REMAIN_TIME";
const LOGOUT_REMAIN_FALLBACK: &str = "It will take %d seconds to close the game.";
const LOGOUT_CANCEL_KEY: &str = "UIIT_MSG_LOGOUT_REMAIN_TIME_CANCLE";
const LOGOUT_CANCEL_FALLBACK: &str = "Logout countdown has been canceled.";
/// Shown when the server *rejects* a logout or a logout-cancel.
///
/// The original has three specific strings for this — `textuisystem.txt` 1747
/// (in battle), 1750 (during the countdown) and 1752 (while teleporting) — but
/// **the wire error code -> string mapping is not established anywhere**
/// (#608). Picking one of the three per code would be an invented table, so we
/// show one generic notice for every rejection and keep the raw code in the
/// `warn!` for diagnosis.
///
/// The wording is the data's own generic sentence (`textuisystem.txt` 1701,
/// "The requested order cannot be carried out."), so the notice reads in the
/// client's voice and stays localized for every locale that ships the table.
/// Using *this* key here is **our choice**, not a recovered mapping: its name
/// records a quest cause, and we borrow only its generic phrasing. Exhaustive
/// key/value scan of the table found no generic logout-failure string.
const LOGOUT_REJECTED_KEY: &str = "UIIT_MSG_STRGERR_REQUESTED_JOB_BLOCKED_BY_QUEST";
const LOGOUT_REJECTED_FALLBACK: &str = "The requested order cannot be carried out.";

/// Root marker of the system window.
#[derive(Component)]
pub(crate) struct SystemWindow;

/// Marks a window scheduled to be despawned in `PostUpdate` (see
/// `despawn_closing_windows`), so it outlives the click that closed it.
/// `pub(crate)`: the options window shares this teardown path.
#[derive(Component)]
pub(crate) struct Closing;

/// Root marker of the logout countdown overlay.
#[derive(Component)]
struct LogoutCountdown;

/// The countdown text, updated each frame while a logout is pending.
#[derive(Component)]
struct CountdownText;

/// A logout accepted by the server and counting down. Present between the
/// `LogoutResponse` and the `LogoutSuccess`/cancel. `pub(crate)` only because
/// [`toggle_system_window`]'s signature (referenced for cross-plugin ordering)
/// must not leak a private type.
#[derive(Resource)]
pub(crate) struct LogoutPending {
    mode: u8,
    seconds_left: f32,
}

pub struct SystemWindowPlugin;

impl Plugin for SystemWindowPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                toggle_system_window,
                update_button_visuals,
                cancel_logout_on_move,
                on_logout_response,
                tick_logout_countdown,
                on_logout_success,
                on_logout_cancel_response,
            )
                .run_if(in_state(SceneState::GameWorld)),
        )
        // Deferred so a button that closes the window still exists during this
        // frame's Update (the click-to-move gate reads it); despawning it in the
        // click observer would let the click fall through to a movement order.
        .add_systems(PostUpdate, despawn_closing_windows);
    }
}

/// Send a packet on the agent connection; returns whether it went out.
fn send_agent(conn: &Query<&SilkroadConnection, With<AgentConnection>>, packet: Packet) -> bool {
    let Ok(conn) = conn.single() else {
        return false;
    };
    if let Err(e) = conn.get_sender().send(packet.into()) {
        error!("system window: failed to send packet: {}", e.0);
        return false;
    }
    true
}

/// Esc toggles the window; Esc while a logout is counting down cancels it.
/// `pub(crate)` so the chat input's Esc handling can order itself after it.
pub(crate) fn toggle_system_window(
    keys: Res<ButtonInput<KeyCode>>,
    pending: Option<Res<LogoutPending>>,
    consumed: Res<crate::plugins::hud::focus::EscConsumed>,
    chat: Res<crate::plugins::hud::chat::model::ChatState>,
    mut selected: ResMut<crate::plugins::cursor::interactions::entity_select::SelectedEntity>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    window: Query<Entity, With<SystemWindow>>,
    options: Query<(), With<crate::plugins::options_window::OptionsWindow>>,
    cameras: Query<Entity, With<Camera2d>>,
    asset_server: Res<AssetServer>,
    ui_strings: Res<ClientUiStrings>,
    mut commands: Commands,
) {
    if !keys.just_pressed(KeyCode::Escape) {
        return;
    }
    // An earlier stage of the Escape chain already used this press — a
    // cancelled transient, or a HUD window that closed. Without this the same
    // press did two things at once: dismissing an armed item *and* opening the
    // Esc menu behind it. See `hud::focus`.
    if consumed.0 {
        return;
    }
    // Esc while typing in chat closes the chat input instead (that handler
    // runs after this one).
    if chat.input_open {
        return;
    }
    // Esc while the options window is open closes it instead (its own
    // `close_on_esc`); without this guard the same press would also reopen
    // the Esc menu.
    if !options.is_empty() {
        return;
    }
    if pending.is_some() {
        send_agent(&conn, Packet::from(LogoutCancelRequest));
        return;
    }
    if let Ok(open) = window.single() {
        commands.entity(open).despawn();
    } else if selected.0.is_some() {
        // Esc drops the current target (closing the target window) first;
        // the next Esc opens the menu.
        selected.0 = None;
    } else if let Some(camera) = cameras.iter().next() {
        spawn_system_window(&mut commands, &asset_server, &ui_strings, camera);
    } else {
        warn!("system window: no 2d camera to target");
    }
}

/// `pub(crate)` so the underbar's Option button can open the window too.
pub(crate) fn spawn_system_window(
    commands: &mut Commands,
    asset_server: &AssetServer,
    ui_strings: &ClientUiStrings,
    camera: Entity,
) {
    let font = asset_server.load::<Font>(BUTTON_FONT);
    let button_style = ImageButtonStyle {
        normal: asset_server.load(SYS_BUTTON_DDJ),
        hover: asset_server.load(SYS_BUTTON_FOCUS_DDJ),
        press: asset_server.load(SYS_BUTTON_PRESS_DDJ),
        ..Default::default()
    };

    // The shell is not this window's: it is the copied small-popup template
    // (see `plugins::small_popup`), and the System window is its first user.
    let popup = spawn_small_popup(
        commands,
        asset_server,
        camera,
        ui_strings.get_or("UIIT_PAG_SYSTEM", "System"),
        &font,
    );
    commands.entity(popup.close_button).observe(on_close);
    commands
        .entity(popup.root)
        .insert(SystemWindow)
        .with_children(|w| {
            spawn_button(
                w,
                ui_strings.get_or("UIIT_CTL_OPTION", "Option"),
                BTN_Y_OPTION,
                &button_style,
                &font,
                on_settings,
            );
            spawn_button(
                w,
                ui_strings.get_or("UIIT_STT_HELP", "Help"),
                BTN_Y_HELP,
                &button_style,
                &font,
                on_help,
            );
            spawn_button(
                w,
                ui_strings.get_or("UIIT_CTL_RESTART", "Restart"),
                BTN_Y_RESTART,
                &button_style,
                &font,
                on_restart,
            );
            spawn_button(
                w,
                ui_strings.get_or("UIIT_CTL_GAMEEXIT", "Exit"),
                BTN_Y_QUIT,
                &button_style,
                &font,
                on_quit,
            );
        });
}

/// Spawn one labelled button at the given y offset, wired to `observer`. The
/// button swaps its texture on hover/press via `update_button_visuals`.
fn spawn_button<M: Send + Sync + 'static>(
    parent: &mut RelatedSpawnerCommands<ChildOf>,
    text: &str,
    y: f32,
    style: &ImageButtonStyle,
    font: &Handle<Font>,
    observer: impl IntoObserverSystem<Activate, (), M>,
) {
    let label = text.to_string();
    let font = font.clone();
    parent
        .spawn((
            Button,
            Hovered::default(),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(BTN_X),
                top: Val::Px(y),
                width: Val::Px(BTN_W),
                height: Val::Px(BTN_H),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            ImageNode::new(style.normal.clone()),
            style.clone(),
        ))
        .observe(observer)
        .with_children(|b| {
            b.spawn((
                Text::new(label),
                TextFont {
                    font: font.into(),
                    font_size: FontSize::Px(12.0),
                    ..default()
                },
                TextColor(BTN_LABEL_COLOR),
                Pickable::IGNORE,
            ));
        });
}

/// Mark the window for deferred despawn (see `despawn_closing_windows`).
fn despawn_window(commands: &mut Commands, window: &Query<Entity, With<SystemWindow>>) {
    for w in window.iter() {
        commands.entity(w).insert(Closing);
    }
}

fn despawn_closing_windows(closing: Query<Entity, With<Closing>>, mut commands: Commands) {
    for entity in closing.iter() {
        commands.entity(entity).despawn();
    }
}

// --- Button observers -------------------------------------------------------

fn on_quit(
    _activate: On<Activate>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    window: Query<Entity, With<SystemWindow>>,
    mut exit: MessageWriter<AppExit>,
    mut commands: Commands,
) {
    // Fall back to an immediate exit if we're somehow not connected.
    if !send_agent(
        &conn,
        Packet::from(LogoutRequest {
            mode: LOGOUT_MODE_EXIT,
        }),
    ) {
        exit.write(AppExit::Success);
    }
    despawn_window(&mut commands, &window);
}

fn on_restart(
    _activate: On<Activate>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    window: Query<Entity, With<SystemWindow>>,
    mut commands: Commands,
) {
    send_agent(
        &conn,
        Packet::from(LogoutRequest {
            mode: LOGOUT_MODE_RESTART,
        }),
    );
    despawn_window(&mut commands, &window);
}

/// The System window's Help button opens the in-game help book (#575) — the
/// same `UIIT_STT_HELP` string titles both, and `GDR_GAMEGUIDE` is the only
/// window the original's help entry points at.
fn on_help(
    _activate: On<Activate>,
    window: Query<Entity, With<SystemWindow>>,
    mut guide: ResMut<crate::plugins::hud::game_guide::GameGuideWindowState>,
    mut commands: Commands,
) {
    guide.open = true;
    despawn_window(&mut commands, &window);
}

fn on_settings(
    _activate: On<Activate>,
    window: Query<Entity, With<SystemWindow>>,
    cameras: Query<Entity, With<Camera2d>>,
    asset_server: Res<AssetServer>,
    ui_strings: Res<ClientUiStrings>,
    options: Res<crate::plugins::settings::options::GameOptions>,
    mut commands: Commands,
) {
    let Some(camera) = cameras.iter().next() else {
        warn!("options window: no 2d camera to target");
        return;
    };
    crate::plugins::options_window::spawn_options_window(
        &mut commands,
        &asset_server,
        &ui_strings,
        &options,
        camera,
    );
    despawn_window(&mut commands, &window);
}

fn on_close(
    _activate: On<Activate>,
    window: Query<Entity, With<SystemWindow>>,
    mut commands: Commands,
) {
    despawn_window(&mut commands, &window);
}

/// Swap each button's texture to match its interaction state. Mirrors ui_v2's
/// `update_image_button_visuals`, which only runs in the intro scenes; the
/// system window lives in GameWorld and needs its own.
fn update_button_visuals(
    mut buttons: Query<(
        &Hovered,
        Has<Pressed>,
        Has<InteractionDisabled>,
        &ImageButtonStyle,
        &mut ImageNode,
    )>,
) {
    for (hovered, pressed, disabled, style, mut image) in buttons.iter_mut() {
        let target = match (disabled, pressed, hovered.get()) {
            (true, _, _) => &style.normal,
            (_, true, _) => &style.press,
            (_, _, true) => &style.hover,
            _ => &style.normal,
        };
        if image.image != *target {
            image.image = target.clone();
        }
    }
}

// --- Logout flow ------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn on_logout_response(
    mut reader: MessageReader<LogoutResponse>,
    asset_server: Res<AssetServer>,
    ui_strings: Res<ClientUiStrings>,
    cameras: Query<Entity, With<Camera2d>>,
    window: Query<Entity, With<SystemWindow>>,
    overlay: Query<Entity, With<LogoutCountdown>>,
    mut history: ResMut<ChatHistory>,
    mut commands: Commands,
) {
    for res in reader.read() {
        if res.result != 1 {
            // The player used to see nothing at all here (#608): tell them the
            // request failed on the system chat line — the surface `chat::net`
            // already uses for locally generated notices, since our
            // message-box family is still dead code (#308) — and keep the raw
            // code in the log for diagnosis.
            warn!(
                "system window: logout rejected (error {:#06X})",
                res.error.unwrap_or_default()
            );
            history.push(ChatLine::system(
                &ui_strings.get_plain_or(LOGOUT_REJECTED_KEY, LOGOUT_REJECTED_FALLBACK),
            ));
            continue;
        }
        let mode = res.mode.unwrap_or(LOGOUT_MODE_EXIT);
        let seconds = res.countdown.unwrap_or(5);
        commands.insert_resource(LogoutPending {
            mode,
            seconds_left: seconds as f32,
        });
        despawn_window(&mut commands, &window);
        if overlay.is_empty() {
            if let Some(camera) = cameras.iter().next() {
                let text = logout_countdown_text(
                    &ui_strings.get_plain_or(LOGOUT_REMAIN_KEY, LOGOUT_REMAIN_FALLBACK),
                    seconds as u32,
                );
                spawn_countdown_overlay(&mut commands, &asset_server, camera, &text);
            }
        }
    }
}

/// Fill the vanilla countdown message's single `%d` with the seconds left. A
/// table that supplies no placeholder is shown verbatim.
fn logout_countdown_text(template: &str, seconds: u32) -> String {
    template.replacen("%d", &seconds.to_string(), 1)
}

/// The countdown's *text* is data-driven, but its presentation is not: no
/// resinfo block describes a countdown element, so the position and size below
/// are ours. The original routes these `UIIT_MSG_*` strings through the
/// message-box family, which is still dead code (#308) — until that lands this
/// overlay is a placeholder.
fn spawn_countdown_overlay(
    commands: &mut Commands,
    asset_server: &AssetServer,
    camera: Entity,
    text: &str,
) {
    let font = asset_server.load::<Font>(BUTTON_FONT);
    commands
        .spawn((
            LogoutCountdown,
            Name::from("Logout Countdown"),
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                top: Val::Percent(35.0),
                justify_content: JustifyContent::Center,
                ..default()
            },
            GlobalZIndex(160),
            Pickable::IGNORE,
            UiTargetCamera(camera),
        ))
        .with_children(|c| {
            c.spawn((
                CountdownText,
                Text::new(text.to_string()),
                TextFont {
                    font: font.into(),
                    font_size: FontSize::Px(22.0),
                    ..default()
                },
                TextColor(Color::WHITE),
                Pickable::IGNORE,
            ));
        });
}

fn tick_logout_countdown(
    time: Res<Time>,
    ui_strings: Res<ClientUiStrings>,
    pending: Option<ResMut<LogoutPending>>,
    mut text: Query<&mut Text, With<CountdownText>>,
) {
    let Some(mut pending) = pending else {
        return;
    };
    pending.seconds_left = (pending.seconds_left - time.delta_secs()).max(0.0);
    let secs = pending.seconds_left.ceil() as u32;
    let template = ui_strings.get_plain_or(LOGOUT_REMAIN_KEY, LOGOUT_REMAIN_FALLBACK);
    for mut t in text.iter_mut() {
        let updated = logout_countdown_text(&template, secs);
        if t.0 != updated {
            t.0 = updated;
        }
    }
}

/// The authoritative logout trigger: perform the exit or restart.
fn on_logout_success(
    mut reader: MessageReader<LogoutSuccess>,
    pending: Option<Res<LogoutPending>>,
    agent: Query<Entity, With<AgentConnection>>,
    overlay: Query<Entity, With<LogoutCountdown>>,
    mut exit: MessageWriter<AppExit>,
    mut next_scene: ResMut<NextState<SceneState>>,
    mut commands: Commands,
) {
    let mut triggered = false;
    for _ in reader.read() {
        triggered = true;
    }
    if !triggered {
        return;
    }
    let mode = pending.as_ref().map(|p| p.mode).unwrap_or(LOGOUT_MODE_EXIT);
    for o in overlay.iter() {
        commands.entity(o).despawn();
    }
    commands.remove_resource::<LogoutPending>();

    if mode == LOGOUT_MODE_RESTART {
        // Drop the agent connection and return to the login screen. Leaving
        // GameWorld runs `cleanup_game_scene`, which clears the join resources.
        for a in agent.iter() {
            commands.entity(a).despawn();
        }
        next_scene.set(SceneState::IntroV2);
    } else {
        exit.write(AppExit::Success);
    }
}

/// Issuing a move order during the logout countdown cancels the logout (matches
/// the vanilla client: any action aborts the timer).
fn cancel_logout_on_move(
    mut orders: MessageReader<PlayerMoveOrder>,
    pending: Option<Res<LogoutPending>>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
) {
    if pending.is_none() {
        orders.clear();
        return;
    }
    if orders.read().next().is_some() {
        send_agent(&conn, Packet::from(LogoutCancelRequest));
    }
}

fn on_logout_cancel_response(
    mut reader: MessageReader<LogoutCancelResponse>,
    ui_strings: Res<ClientUiStrings>,
    overlay: Query<Entity, With<LogoutCountdown>>,
    mut history: ResMut<ChatHistory>,
    mut commands: Commands,
) {
    for res in reader.read() {
        if res.result == 1 {
            for o in overlay.iter() {
                commands.entity(o).despawn();
            }
            commands.remove_resource::<LogoutPending>();
            // The original confirms the cancel with its own message. Our
            // message-box family is still dead code (#308), so it goes to the
            // system chat line — the surface `chat::net` already uses for
            // locally generated notices.
            history.push(ChatLine::system(
                &ui_strings.get_plain_or(LOGOUT_CANCEL_KEY, LOGOUT_CANCEL_FALLBACK),
            ));
        } else {
            warn!(
                "system window: logout cancel rejected (error {:#06X})",
                res.error.unwrap_or_default()
            );
            history.push(ChatLine::system(
                &ui_strings.get_plain_or(LOGOUT_REJECTED_KEY, LOGOUT_REJECTED_FALLBACK),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // The shell's own extent constants are only read here now: the window
    // takes its size from the shell, and the test is what pins that the two
    // still agree.
    use crate::plugins::small_popup::{WINDOW_H, WINDOW_RECT, WINDOW_W};

    /// The extent is a transcription of the v1.188 resinfo, so pin it to the
    /// cited bytes: it had drifted to a hand-picked 200x260 (issue #311). The
    /// shell itself now lives in `plugins::small_popup`; what this test still
    /// owns is the button column, which is the System window's *own* second
    /// witness for the same 214.
    #[test]
    fn window_extent_matches_the_ginterface_rect() {
        // resinfo/ginterface.txt:306 — GDR_SYSTEM:CIFSystemWnd, ID 98 (:305).
        assert_eq!(WINDOW_RECT, (300.0, 200.0, 214.0, 245.0));
        assert_eq!((WINDOW_W, WINDOW_H), (214.0, 245.0));
        assert_eq!(BTN_X + BTN_W + BTN_X, WINDOW_W);
    }

    /// Every button rect is byte-exact against `resinfo/ifsystemwnd.txt`; the
    /// cited line is the element's `Rect`.
    #[test]
    fn button_rects_match_ifsystemwnd() {
        assert_eq!(BTN_X, 31.0); // shared by all five buttons
        assert_eq!((BTN_W, BTN_H), (152.0, 24.0)); // sys_button.ddj's own size
        assert_eq!(BTN_Y_OPTION, 58.0); // :91  GDR_SYSTEM_BTN_OPTION
        assert_eq!(BTN_Y_HELP, 92.0); // :72  GDR_SYSTEM_BTN_HELP
        assert_eq!(BTN_Y_RESTART, 159.0); // :34  GDR_SYSTEM_BTN_RESTART
        assert_eq!(BTN_Y_QUIT, 192.0); // :15  GDR_SYSTEM_BTN_QUIT
                                       // GDR_SYSTEM_BTN_CAS_HELP (:53, y=125) is deliberately not built, so
                                       // Help->Restart spans 67px where every built neighbour pair spans 33-34.
        assert_eq!(BTN_Y_RESTART - BTN_Y_HELP, 67.0);
    }

    /// resinfo colours are ARGB, so `255,254,251,216` is an opaque cream rather
    /// than a 216-alpha white, and the title is plain white — it used to be a
    /// hardcoded warm gold.
    #[test]
    fn label_colors_are_the_argb_rgb_components() {
        assert_eq!(BTN_LABEL_COLOR, Color::srgb_u8(254, 251, 216));
    }

    /// The countdown shows the vanilla mode-agnostic message with its single
    /// `%d` filled in; it used to format an invented "Exiting/Restarting in N".
    #[test]
    fn countdown_text_fills_the_vanilla_placeholder() {
        // textuisystem.txt:1753 — UIIT_MSG_LOGOUT_REMAIN_TIME.
        assert_eq!(
            logout_countdown_text(LOGOUT_REMAIN_FALLBACK, 5),
            "It will take 5 seconds to close the game."
        );
        assert_eq!(
            logout_countdown_text(LOGOUT_REMAIN_FALLBACK, 0),
            "It will take 0 seconds to close the game."
        );
        // :1754 carries no placeholder, so it passes through untouched.
        assert_eq!(
            logout_countdown_text(LOGOUT_CANCEL_FALLBACK, 5),
            "Logout countdown has been canceled."
        );
    }

    /// The cancel confirmation was pure dead wire — the string shipped and the
    /// resource was already in this file, but nothing surfaced it. Driving the
    /// real system also validates its parameters, which nothing else does:
    /// these systems only ever run in `SceneState::GameWorld`.
    #[test]
    fn cancelling_a_logout_surfaces_the_vanilla_message() {
        let mut app = App::new();
        app.add_message::<LogoutCancelResponse>()
            .init_resource::<ClientUiStrings>()
            .init_resource::<ChatHistory>()
            .insert_resource(LogoutPending {
                mode: LOGOUT_MODE_EXIT,
                seconds_left: 5.0,
            })
            .add_systems(Update, on_logout_cancel_response);
        app.world_mut().write_message(LogoutCancelResponse {
            result: 1,
            error: None,
        });
        app.update();

        assert!(
            app.world().get_resource::<LogoutPending>().is_none(),
            "an accepted cancel clears the pending logout"
        );
        let lines: Vec<String> = app
            .world()
            .resource::<ChatHistory>()
            .iter()
            .map(|line| line.display())
            .collect();
        assert_eq!(lines, vec![LOGOUT_CANCEL_FALLBACK.to_string()]);
    }

    /// #608: a rejected cancel only ever produced a `warn!` with a raw hex
    /// code, so the player saw nothing. It now surfaces one generic notice —
    /// generic on purpose: the wire code -> string mapping (1747/1750/1752) is
    /// not established anywhere, and a three-way table picked by guess would be
    /// exactly the unsourced constant ADR-0009 forbids.
    #[test]
    fn a_rejected_cancel_tells_the_player() {
        let mut app = App::new();
        app.add_message::<LogoutCancelResponse>()
            .init_resource::<ClientUiStrings>()
            .init_resource::<ChatHistory>()
            .insert_resource(LogoutPending {
                mode: LOGOUT_MODE_EXIT,
                seconds_left: 5.0,
            })
            .add_systems(Update, on_logout_cancel_response);
        app.world_mut().write_message(LogoutCancelResponse {
            result: 2,
            error: Some(0x1750),
        });
        app.update();

        assert!(
            app.world().get_resource::<LogoutPending>().is_some(),
            "a rejected cancel leaves the pending logout alone"
        );
        let lines: Vec<String> = app
            .world()
            .resource::<ChatHistory>()
            .iter()
            .map(|line| line.display())
            .collect();
        assert_eq!(lines, vec![LOGOUT_REJECTED_FALLBACK.to_string()]);
    }

    /// The notice must stay code-agnostic: every rejection code produces the
    /// same line, because we assert no mapping. Option (B) of #608 — one string
    /// per code — needs the mapping recovered from `packet_dump/` or the exe
    /// string table first.
    #[test]
    fn every_rejection_code_shows_the_same_generic_notice() {
        let mut lines = Vec::new();
        for error in [0x1747u16, 0x1750, 0x1752, 0x0000] {
            let mut app = App::new();
            app.add_message::<LogoutCancelResponse>()
                .init_resource::<ClientUiStrings>()
                .init_resource::<ChatHistory>()
                .add_systems(Update, on_logout_cancel_response);
            app.world_mut().write_message(LogoutCancelResponse {
                result: 2,
                error: Some(error),
            });
            app.update();
            lines.push(
                app.world()
                    .resource::<ChatHistory>()
                    .iter()
                    .map(|line| line.display())
                    .collect::<Vec<_>>(),
            );
        }
        assert!(
            lines
                .iter()
                .all(|l| *l == vec![LOGOUT_REJECTED_FALLBACK.to_string()]),
            "all rejection codes share one notice: {lines:?}"
        );
    }
}
