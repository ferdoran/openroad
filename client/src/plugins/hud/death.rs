//! Local-player death window + resurrect (issues #141–#143).
//!
//! Idea: SRO has no dedicated "you died" packet — death is a 0x30BF life-state
//! flip to Dead on the local player's own uid, which
//! `combat::on_entity_state_update` turns into the [`PlayerDeath`] flag instead
//! of routing the local player into the monster-corpse despawn pipeline (#141).
//! This module owns the flag's UI side (#142/#143): a modal death window on the
//! shared `game_window` chrome whose option buttons send the 0x3053 GetUp
//! resurrect request, a fullscreen dimming scrim, and an input gate
//! ([`player_is_dead`]) that keeps the world from being clicked while dead.
//! All three of the original's options are offered: "Resurrect at the specified
//! point." (option byte 1 — the town return, the one a normal character always
//! has), "Resurrect at the present point." (byte 2, which vSRO gates on
//! level/scroll and silently drops otherwise), and "Waiting for other player's
//! help.", which sends nothing, dismisses the box locally and comes back when
//! the own corpse is clicked. Revive (life-state → Alive) clears the flag; the
//! window and scrim tear down and control returns.
//!
//! **Deviation from the original (ADR 0009):** the original's rebirth box shows
//! only *two* buttons — the decompiled builder (`sro_client.exe@00644c90`,
//! msgbox case 3) picks *either* `REBIRTH_STANDING` (present point) *or*
//! `REBIRTH_POINT` (specified point) for its first slot depending on a mode
//! value the server hands it, then adds `REBIRTH_HELP`. We do not yet decode
//! that mode, so we show all three and let the player choose rather than
//! guessing which one the server would have allowed. Once the mode is decoded,
//! collapse the first two back into one slot.
//!
//! **Chrome, and what is still drifting (#142).** The box now wears its own
//! art — `msgbox_rebirth.ddj` as the header and `msgbox_rebirth_button*.ddj`
//! for the options, both at their PK2-native extents, both named by the
//! original's own builder. What it still borrows is the *shell*: the mframe
//! `game_window` chrome with a title band, where the original uses the
//! title-band-less `msgbox2_window_*` 9-slice family. That shell does not exist
//! in this tree yet; it belongs to the shared message-box unit
//! (`docs/re/ui/hud-death-window.md` §6 D1), so it is deliberately left for
//! that work rather than forked here.
//!
//! The fullscreen scrim is **ours** (D5): the original dims the world by other
//! means, if at all. It is kept because it is the only thing that makes the box
//! read as modal on a desktop-sized window, and it doubles as the click sink
//! the input gate would otherwise need.
//!
//! Needs-a-capture: the respawn packet set and death EXP-loss are SPEC-derived
//! (docs/re/notes/death-resurrect.md). The revive keys off the documented
//! 0x30BF life→Alive signal, and the window layout is not yet grounded in the
//! PK2 death-window resinfo (flagged for a fidelity pass).

use bevy::camera::primitives::Aabb;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui::UiTargetCamera;
use bevy::ui_widgets::{Activate, Button};
use bevy::window::PrimaryWindow;

use packets::agent::prelude::GetUpRequest;
use packets::Packet;

use crate::assets::FontAssets;
use crate::net::connection::SilkroadConnection;
use crate::plugins::cursor::interactions::{ray_intersects_aabb, transform_aabb_to_world};
use crate::plugins::cursor::GameCursorCamera;
use crate::plugins::hud::game_window;
use crate::plugins::hud::scale::hud_scale;
use crate::plugins::net::agent::AgentConnection;
use crate::plugins::player::Player;
use crate::plugins::textdata::ClientUiStrings;
use crate::plugins::ui_v2::style::ImageButtonStyle;

/// Content area inside the mframe chrome (window units).
const CONTENT_W: f32 = 240.0;
/// Tall enough for the header art, the three option buttons, the status line
/// and even spacing (52 + 3 × 24 + 12 + 4 × 7 window units). Not an original
/// metric — the original's rebirth box is code-built and shows only two buttons
/// (see the module doc's deviation note); the pieces inside it *are* at their
/// native sizes ([`REBIRTH_BUTTON_SIZE`], [`REBIRTH_HEADER_SIZE`]).
const CONTENT_H: f32 = 164.0;

/// The rebirth box's own art. The original attaches these three in code — the
/// decompiled builder (`sro_client.exe@00644c90`, msgbox case 3) names
/// `interface\\messagebox\\msgbox_rebirth_button.ddj` for *both* of its option
/// buttons, and no resinfo tree references them, which is why a resinfo grep
/// finds no death window at all (`docs/re/ui/hud-death-window.md` §3).
const REBIRTH_BUTTON_DDJ: &str = "media://interface/messagebox/msgbox_rebirth_button.ddj";
const REBIRTH_BUTTON_FOCUS_DDJ: &str =
    "media://interface/messagebox/msgbox_rebirth_button_focus.ddj";
const REBIRTH_BUTTON_PRESS_DDJ: &str =
    "media://interface/messagebox/msgbox_rebirth_button_press.ddj";
/// The box's header art, same provenance (string `00d9d674` in that builder).
const REBIRTH_HEADER_DDJ: &str = "media://interface/messagebox/msgbox_rebirth.ddj";

/// Native extents from the DDS headers of the user's own PK2: the option art is
/// 176×24 and the header art 148×52. Drawing them at anything else is a rescale,
/// which is precisely the drift this pass removes (the buttons used to be
/// `system/sys_button.ddj`, native 152×24, stretched to 190×26).
const REBIRTH_BUTTON_SIZE: (f32, f32) = (176.0, 24.0);
const REBIRTH_HEADER_SIZE: (f32, f32) = (148.0, 52.0);

/// The option captions, as `(textuisystem.txt id, English fallback)`. Both ids
/// were read out of the user's own v1.188 PK2 (`docs/re/ui/hud-death-window.md`
/// §3); the pair they replaced (`UIIT_MSG_DIE_TITLE`,
/// `UIIT_MSG_RETURN_TO_TOWN`) exists in no PK2 file and was invented, so pin
/// these against a repeat (see the test at the bottom of this file).
const PRESENT_POINT_LABEL: (&str, &str) = (
    "UIIT_MSG_MSGBOX_REBIRTH_STANDING_BUTTON",
    "Resurrect at the present point.",
);
const WAIT_FOR_HELP_LABEL: (&str, &str) = (
    "UIIT_MSG_MSGBOX_REBIRTH_HELP_BUTTON",
    "Waiting for other player's help.",
);
/// The town return — the original's `UIIT_MSG_MSGBOX_REBIRTH_POINT_BUTTON`.
/// Its 0x3053 option byte (`1`) is sourced from the vSRO clientless bot, see
/// [`GetUpRequest::RETURN_TO_TOWN`]; it used to be UNKNOWN, which is why this
/// button did not exist and a dead player had no working way back (#143/#236).
/// Shown when a sent request goes unanswered. **Invented copy, stated
/// deviation:** the original never needs it (it offers only the option the
/// server already allows), and no `UIIT_MSG_*` id in the v1.188
/// `textuisystem.txt` covers it — inventing a *string id* would be the defect,
/// so this is plain text with its rationale, not a fake id.
const UNANSWERED_NOTICE: &str = "No answer — try the other option.";

const SPECIFIED_POINT_LABEL: (&str, &str) = (
    "UIIT_MSG_MSGBOX_REBIRTH_POINT_BUTTON",
    "Resurrect at the specified point.",
);

/// Local-player death flag. Set by `combat::on_entity_state_update` on a 0x30BF
/// life→Dead for the player's own uid; cleared on life→Alive (revive).
#[derive(Resource, Default)]
pub struct PlayerDeath {
    pub dead: bool,
    /// The player picked "Waiting for other player's help.": still dead and
    /// still input-locked, but the box is out of the way until the corpse is
    /// clicked again (#236). Reset by [`Self::set_dead`] on a fresh death.
    pub window_hidden: bool,
}

impl PlayerDeath {
    /// Enter the death state. Only an alive→dead transition un-hides the
    /// window, so the repeated death pushes (0x3011 *and* 0x30BF both raise
    /// the flag for one death) cannot pop a box the player dismissed.
    pub fn set_dead(&mut self) {
        if !self.dead {
            self.window_hidden = false;
        }
        self.dead = true;
    }

    /// Revive: leave the death state and re-arm the window for the next death.
    pub fn set_alive(&mut self) {
        self.dead = false;
        self.window_hidden = false;
    }

    /// The window is on screen: dead, and not dismissed to wait for help.
    fn window_visible(&self) -> bool {
        self.dead && !self.window_hidden
    }
}

/// How long a sent 0x3053 may go unanswered before the window admits it.
///
/// The refusal is invisible on the wire: `packet_dump/c2s/0x3053.log` shows
/// five present-point sends the live server answered with no packet of any
/// opcode. 3 s is comfortably above the ~150 ms the accepted path took in the
/// captures (`docs/net-death-resurrect.md` §2: revive lands 1.45 s after death
/// including the player's reaction time) — chosen so a slow-but-working server
/// is never called a failure.
const RESURRECT_TIMEOUT_SECS: f32 = 3.0;

/// The in-flight 0x3053, so the window can say "no answer" instead of looking
/// broken. **Deviation (ADR 0009):** the original has no such message — it only
/// ever offers the option the server already chose (see the module doc), so it
/// never has a refused one to report. We do offer both, so we owe the player
/// the feedback. Deleting this is correct once the server's mode is decoded.
#[derive(Resource, Default)]
pub struct ResurrectRequest {
    /// Seconds since the request went out, while still dead and unanswered.
    pub waiting: Option<f32>,
}

/// Run condition: the local player is dead, so world input (move/select) is
/// locked. Gates the click-to-move and click-to-select systems.
pub fn player_is_dead(death: Res<PlayerDeath>) -> bool {
    death.dead
}

/// The whole death window (mframe chrome + resurrect button).
#[derive(Component)]
pub struct DeathWindowRoot;

/// The fullscreen dimming scrim behind the window.
#[derive(Component)]
pub struct DeathScrim;

/// The line under the option buttons that reports an unanswered request.
#[derive(Component)]
pub struct ResurrectStatusText;

/// Spawn the death window + scrim when [`PlayerDeath`] flips to dead, tear both
/// down when it clears. Rebuilds only on change (mirrors `sync_dialog_window`).
#[allow(clippy::too_many_arguments)]
pub fn sync_death_window(
    death: Res<PlayerDeath>,
    existing: Query<Entity, Or<(With<DeathWindowRoot>, With<DeathScrim>)>>,
    fonts: Res<FontAssets>,
    asset_server: Res<AssetServer>,
    ui_strings: Res<ClientUiStrings>,
    cam_query: Query<Entity, With<Camera2d>>,
    primary_window: Query<&Window, With<PrimaryWindow>>,
    mut commands: Commands,
) {
    if !death.is_changed() {
        return;
    }
    let showing = !existing.is_empty();
    if !death.window_visible() {
        for entity in existing.iter() {
            commands.entity(entity).despawn();
        }
        return;
    }
    if showing {
        return;
    }
    let Ok(camera) = cam_query.single() else {
        warn!("death window: no 2d camera to attach to");
        return;
    };

    // Fullscreen dimming scrim: the modal cue (and a bevy_ui click sink); the
    // input gate on the world systems is the real lock.
    commands.spawn((
        DeathScrim,
        Name::from("Death Scrim"),
        Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        },
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.45)),
        GlobalZIndex(100),
        UiTargetCamera(camera),
    ));

    let s = hud_scale();
    let (outer_w, outer_h) = game_window::outer_size((CONTENT_W, CONTENT_H));
    let (screen_w, screen_h) = primary_window
        .single()
        .map(|w| (w.width(), w.height()))
        .unwrap_or((1280.0, 720.0));
    // Centered — the game_window root is right/top-anchored.
    let anchor = (
        ((screen_w - outer_w * s) / 2.0).max(0.0),
        ((screen_h - outer_h * s) / 2.0).max(0.0),
    );

    let title = ui_strings
        .get_or(
            "UIIT_MSG_MSGBOX_ASK_SELF_REBIRTH_2",
            "Choose a method to resurrect yourself.",
        )
        .to_string();
    // No close (X): resurrect is the only way out of the death state. The
    // shell is asked not to spawn one rather than spawning and despawning it,
    // so `close_button` is honestly `None` for this window.
    let window = game_window::spawn_game_window_styled(
        &mut commands,
        &asset_server,
        &fonts,
        camera,
        &title,
        (CONTENT_W, CONTENT_H),
        anchor,
        s,
        game_window::GameWindowStyle {
            close_button: false,
            ..default()
        },
    );
    commands
        .entity(window.root)
        .insert((DeathWindowRoot, GlobalZIndex(101)));

    let button_style = ImageButtonStyle {
        normal: asset_server.load(REBIRTH_BUTTON_DDJ),
        hover: asset_server.load(REBIRTH_BUTTON_FOCUS_DDJ),
        press: asset_server.load(REBIRTH_BUTTON_PRESS_DDJ),
        ..Default::default()
    };
    // Markup-resolving read (#507): textuisystem rows may carry <sml2>/<br>.
    let specified_point = ui_strings.get_plain_or(SPECIFIED_POINT_LABEL.0, SPECIFIED_POINT_LABEL.1);
    let present_point = ui_strings.get_plain_or(PRESENT_POINT_LABEL.0, PRESENT_POINT_LABEL.1);
    let wait_for_help = ui_strings.get_plain_or(WAIT_FOR_HELP_LABEL.0, WAIT_FOR_HELP_LABEL.1);
    let font = fonts.two.clone();
    commands.entity(window.content).with_children(|content| {
        // The header art the original attaches to this box, at its native
        // 148×52 (see [`REBIRTH_HEADER_SIZE`]).
        content.spawn((
            ImageNode::new(asset_server.load(REBIRTH_HEADER_DDJ)),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px((CONTENT_W * s - REBIRTH_HEADER_SIZE.0 * s) / 2.0),
                top: Val::Px(2.0 * s),
                width: Val::Px(REBIRTH_HEADER_SIZE.0 * s),
                height: Val::Px(REBIRTH_HEADER_SIZE.1 * s),
                ..default()
            },
            Pickable::IGNORE,
        ));
        // The option stack, centered in the content area. SpaceEvenly rather
        // than a padding constant so the row count decides the spacing.
        content
            .spawn((
                Node {
                    width: Val::Px(CONTENT_W * s),
                    height: Val::Px(CONTENT_H * s),
                    // Below the header art, which sits absolutely at the top.
                    padding: UiRect::top(Val::Px((REBIRTH_HEADER_SIZE.1 + 4.0) * s)),
                    flex_direction: FlexDirection::Column,
                    justify_content: JustifyContent::SpaceEvenly,
                    align_items: AlignItems::Center,
                    ..default()
                },
                Pickable::IGNORE,
            ))
            .with_children(|col| {
                let mut option = |label: String, style: ImageButtonStyle| {
                    col.spawn((
                        Button,
                        Hovered::default(),
                        Node {
                            width: Val::Px(REBIRTH_BUTTON_SIZE.0 * s),
                            height: Val::Px(REBIRTH_BUTTON_SIZE.1 * s),
                            justify_content: JustifyContent::Center,
                            align_items: AlignItems::Center,
                            ..default()
                        },
                        ImageNode::new(style.normal.clone()),
                        style,
                    ))
                    .with_children(|b| {
                        b.spawn((
                            Text::new(label),
                            TextFont {
                                font: font.clone().into(),
                                font_size: FontSize::Px(9.5 * s),
                                ..default()
                            },
                            TextColor(Color::WHITE),
                            Pickable::IGNORE,
                        ));
                    })
                    .id()
                };
                let town = option(specified_point, button_style.clone());
                let resurrect = option(present_point, button_style.clone());
                let wait = option(wait_for_help, button_style);
                col.commands()
                    .entity(town)
                    .observe(on_return_to_town_button);
                col.commands()
                    .entity(resurrect)
                    .observe(on_resurrect_button);
                col.commands().entity(wait).observe(on_wait_for_help_button);
                // Empty until a request goes unanswered (see
                // [`notice_unanswered_resurrect`]).
                col.spawn((
                    ResurrectStatusText,
                    Text::new(String::new()),
                    TextFont {
                        font: font.clone().into(),
                        font_size: FontSize::Px(9.0 * s),
                        ..default()
                    },
                    TextColor(Color::srgb(1.0, 0.75, 0.4)),
                    Pickable::IGNORE,
                ));
            });
    });
}

/// "Resurrect at the specified point.": the town return, option byte `1`.
///
/// This is the one that has to work. `packet_dump/c2s/0x3053.log` records five
/// present-point (`02`) sends on 2026-08-15 at 10:50:00–10:50:11 that the live
/// server answered with nothing at all — vSRO gates present-point resurrect
/// (level/scroll), so for a normal character the only accepted option is the
/// designated resurrection point. The byte is sourced, not guessed: see
/// [`GetUpRequest::RETURN_TO_TOWN`].
fn on_return_to_town_button(
    _: On<Activate>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    pending: ResMut<ResurrectRequest>,
) {
    send_get_up(
        GetUpRequest::return_to_town(),
        "return to town",
        conn,
        pending,
    );
}

/// Send one 0x3053 and start the unanswered-request clock.
fn send_get_up(
    request: GetUpRequest,
    what: &str,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    mut pending: ResMut<ResurrectRequest>,
) {
    let Ok(conn) = conn.single() else {
        return;
    };
    info!(
        "death: sending resurrect request ({what}, option {})",
        request.option
    );
    match conn.get_sender().send(Packet::from(request).into()) {
        Ok(()) => pending.waiting = Some(0.0),
        Err(e) => error!("network: failed to send GetUp ({what}): {}", e.0),
    }
}

/// Report a request the server never answered. Only a *sent* request starts the
/// clock, and a revive (life→Alive clears [`PlayerDeath::dead`]) stops it, so
/// the line can only appear when the player really is still lying there.
pub fn notice_unanswered_resurrect(
    time: Res<Time>,
    death: Res<PlayerDeath>,
    mut pending: ResMut<ResurrectRequest>,
    mut text: Query<&mut Text, With<ResurrectStatusText>>,
) {
    if !death.dead {
        pending.waiting = None;
        return;
    }
    let Some(waiting) = pending.waiting.as_mut() else {
        return;
    };
    let before = *waiting;
    *waiting += time.delta_secs();
    if crossed_resurrect_timeout(before, *waiting) {
        warn!("death: no answer to the resurrect request — try the other option");
        for mut text in text.iter_mut() {
            **text = UNANSWERED_NOTICE.to_string();
        }
    }
}

/// "Resurrect at the present point.": option byte `2`. The server answers with
/// its respawn set (a life→Alive there clears the window); a server that
/// refuses the option — vSRO gates it on level/scroll — simply leaves the
/// window up, which is why the town return above exists next to it.
fn on_resurrect_button(
    _: On<Activate>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    pending: ResMut<ResurrectRequest>,
) {
    send_get_up(
        GetUpRequest::present_point(),
        "present point",
        conn,
        pending,
    );
}

/// "Waiting for other player's help.": the original's third-way option — stay
/// dead and let a party member's resurrect skill do it. It sends **nothing**.
///
/// That is a deliberate choice, not a guess dressed up as one: no outbound
/// opcode for "I am waiting" is evidenced anywhere (0x3053's non-present-point
/// option bytes are UNKNOWN, and 0x3053 does not appear in the recovered
/// original-client builder set at all), while the observable behaviour — the
/// player stays dead until someone revives them — is the server's default with
/// no request at all. So the button is purely local: hide the box, keep the
/// death state and the input lock, and let the corpse click bring it back.
fn on_wait_for_help_button(_: On<Activate>, mut death: ResMut<PlayerDeath>) {
    info!("death: waiting for another player's resurrect (window dismissed)");
    death.window_hidden = true;
}

/// The notice fires on the *frame the clock crosses* the timeout, never again:
/// a per-frame `>=` would re-write the text (and re-log) every frame for as
/// long as the player stays dead.
fn crossed_resurrect_timeout(before: f32, after: f32) -> bool {
    before < RESURRECT_TIMEOUT_SECS && after >= RESURRECT_TIMEOUT_SECS
}

/// Does the cursor ray hit any of the corpse's parts?
///
/// The player root carries no `Aabb` of its own — only the streamed-in body and
/// equipment meshes do — so the shared `check_cursor_aabb_intersection` sweep
/// never sees the local player (its `GameCursorTarget` stays `None` forever).
/// Testing the descendants' world AABBs directly is the same maths the sweep
/// does, without needing a picking proxy on a player that is not a
/// `RemoteEntity`.
fn corpse_is_under_ray<'a>(
    ray: &Ray3d,
    parts: impl Iterator<Item = (&'a Aabb, &'a GlobalTransform)>,
) -> bool {
    parts.into_iter().any(|(aabb, gt)| {
        let (_scale, rotation, translation) = gt.to_scale_rotation_translation();
        let world = transform_aabb_to_world(aabb, translation, rotation);
        ray_intersects_aabb(ray, &world).is_some()
    })
}

/// Clicking your own corpse re-opens the death box after "waiting for help"
/// dismissed it (#236, user-reported original behaviour). Only runs while the
/// box is hidden, so it costs one AABB walk per click in that state and
/// nothing otherwise.
pub fn reopen_death_window_on_corpse_click(
    buttons: Res<ButtonInput<MouseButton>>,
    mut death: ResMut<PlayerDeath>,
    cameras: Query<&GameCursorCamera>,
    players: Query<Entity, With<Player>>,
    children: Query<&Children>,
    aabbs: Query<(&Aabb, &GlobalTransform)>,
) {
    if !death.dead || !death.window_hidden || !buttons.just_pressed(MouseButton::Left) {
        return;
    }
    let Some(ray) = cameras.iter().find_map(|camera| camera.cursor_ray) else {
        return;
    };
    let Ok(root) = players.single() else {
        return;
    };
    let parts = children
        .iter_descendants(root)
        .filter_map(|part| aabbs.get(part).ok());
    if corpse_is_under_ray(&ray, parts) {
        death.window_hidden = false;
    }
}

/// OnExit(GameWorld): drop the window/scrim and reset the flag so a re-entry
/// starts alive.
pub fn cleanup_death_window(
    existing: Query<Entity, Or<(With<DeathWindowRoot>, With<DeathScrim>)>>,
    mut death: ResMut<PlayerDeath>,
    mut commands: Commands,
) {
    for entity in existing.iter() {
        commands.entity(entity).despawn();
    }
    death.set_alive();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ids the buttons render must stay the ones that actually exist in the
    /// v1.188 `textuisystem.txt`. The bug this replaces shipped two invented
    /// ids (`UIIT_MSG_DIE_TITLE`, `UIIT_MSG_RETURN_TO_TOWN`) whose fallbacks
    /// therefore always rendered — and described the wrong action (#236).
    #[test]
    fn option_labels_use_the_real_textuisystem_ids() {
        assert_eq!(
            PRESENT_POINT_LABEL.0,
            "UIIT_MSG_MSGBOX_REBIRTH_STANDING_BUTTON"
        );
        assert_eq!(WAIT_FOR_HELP_LABEL.0, "UIIT_MSG_MSGBOX_REBIRTH_HELP_BUTTON");
        assert_eq!(
            SPECIFIED_POINT_LABEL.0,
            "UIIT_MSG_MSGBOX_REBIRTH_POINT_BUTTON"
        );
        // No table loaded -> the fallbacks are what a user without the string
        // file sees, so they must read as the original's captions do.
        let strings = ClientUiStrings::default();
        assert_eq!(
            strings.get_or(PRESENT_POINT_LABEL.0, PRESENT_POINT_LABEL.1),
            "Resurrect at the present point."
        );
        assert_eq!(
            strings.get_or(WAIT_FOR_HELP_LABEL.0, WAIT_FOR_HELP_LABEL.1),
            "Waiting for other player's help."
        );
        assert_eq!(
            strings.get_or(SPECIFIED_POINT_LABEL.0, SPECIFIED_POINT_LABEL.1),
            "Resurrect at the specified point."
        );
    }

    /// The rebirth art is drawn at the native extents of the user's own PK2
    /// DDJs (176×24 buttons, 148×52 header) and the box is big enough to hold
    /// all of it — the drift this replaced stretched `system/sys_button.ddj`
    /// (native 152×24) to 190×26 (`docs/re/ui/hud-death-window.md` §6 D7).
    #[test]
    fn rebirth_art_is_drawn_at_its_native_size() {
        assert_eq!(REBIRTH_BUTTON_SIZE, (176.0, 24.0));
        assert_eq!(REBIRTH_HEADER_SIZE, (148.0, 52.0));
        assert!(
            REBIRTH_BUTTON_SIZE.0 <= CONTENT_W,
            "buttons must fit across"
        );
        let stacked = REBIRTH_HEADER_SIZE.1 + 3.0 * REBIRTH_BUTTON_SIZE.1 + 12.0;
        assert!(
            CONTENT_H >= stacked,
            "content {CONTENT_H} must hold header + 3 options + status line ({stacked})"
        );
    }

    /// The "no answer" line must appear exactly once per request, on the frame
    /// the clock crosses the timeout — not every frame afterwards.
    #[test]
    fn unanswered_notice_fires_once_on_the_crossing_frame() {
        assert!(!crossed_resurrect_timeout(0.0, 0.016));
        assert!(crossed_resurrect_timeout(
            RESURRECT_TIMEOUT_SECS - 0.01,
            RESURRECT_TIMEOUT_SECS
        ));
        assert!(!crossed_resurrect_timeout(
            RESURRECT_TIMEOUT_SECS,
            RESURRECT_TIMEOUT_SECS + 0.016
        ));
    }

    /// A label may never disagree with the byte its button sends — the D3 bug
    /// this window already shipped once ("Return to the nearest town" wired to
    /// the present-point byte, #236). The town button is the *specified* point
    /// (byte 1), the present-point button is byte 2, and they are distinct.
    #[test]
    fn each_option_label_matches_the_byte_its_button_sends() {
        assert_eq!(GetUpRequest::return_to_town().option, 1);
        assert!(SPECIFIED_POINT_LABEL.1.contains("specified point"));
        assert_eq!(GetUpRequest::present_point().option, 2);
        assert!(PRESENT_POINT_LABEL.1.contains("present point"));
    }

    /// "Waiting for help" hides the box but must not end the death state: the
    /// input lock keys off `dead`, so a dismissed window may never unlock the
    /// world. And a repeat death push (0x3011 and 0x30BF both fire for one
    /// death) must not pop the dismissed box back up.
    #[test]
    fn waiting_for_help_hides_the_window_without_leaving_the_death_state() {
        let mut death = PlayerDeath::default();
        death.set_dead();
        assert!(death.window_visible());

        death.window_hidden = true;
        assert!(!death.window_visible());
        assert!(death.dead, "still dead -> world input stays locked");
        death.set_dead(); // the second push for the same death
        assert!(!death.window_visible(), "a repeat push must not re-open it");

        death.set_alive();
        assert!(!death.dead && !death.window_hidden);
        death.set_dead(); // the next death re-arms the window
        assert!(death.window_visible());
    }

    /// Clicking the corpse re-opens the box, clicking past it does not. The
    /// player root has no `Aabb`, so the hit test walks the streamed-in mesh
    /// children — a ray that misses every one of them is a miss.
    #[test]
    fn only_a_ray_through_a_corpse_part_counts_as_a_corpse_click() {
        let part = Aabb::from_min_max(Vec3::splat(-1.0), Vec3::splat(1.0));
        let at_origin = GlobalTransform::from_translation(Vec3::ZERO);
        let far_away = GlobalTransform::from_translation(Vec3::new(50.0, 0.0, 0.0));

        let hit = Ray3d::new(Vec3::new(0.0, 0.0, 10.0), Dir3::NEG_Z);
        let miss = Ray3d::new(Vec3::new(0.0, 20.0, 10.0), Dir3::NEG_Z);

        assert!(corpse_is_under_ray(&hit, [(&part, &at_origin)].into_iter()));
        assert!(!corpse_is_under_ray(
            &miss,
            [(&part, &at_origin)].into_iter()
        ));
        // Only the part the ray passes through matters, not the first one.
        assert!(corpse_is_under_ray(
            &hit,
            [(&part, &far_away), (&part, &at_origin)].into_iter()
        ));
        assert!(!corpse_is_under_ray(&hit, [(&part, &far_away)].into_iter()));
        // No parts streamed in yet -> nothing to click.
        assert!(!corpse_is_under_ray(&hit, [].into_iter()));
    }

    /// The reopen system reads three overlapping component queries; Bevy only
    /// proves those disjoint at system-init time, so a missing filter is a
    /// B0001 panic on the first frame in `GameWorld` and no helper test would
    /// catch it.
    #[test]
    fn the_reopen_system_has_no_conflicting_queries() {
        let mut app = App::new();
        app.init_resource::<PlayerDeath>()
            .init_resource::<ButtonInput<MouseButton>>()
            .add_systems(Update, reopen_death_window_on_corpse_click);
        app.update();
    }
}

/// Self-registration for the death window (#141-#143) (#558). The HUD registry holds one line per
/// window, so two windows landing in the same lap no longer collide on it.
pub struct DeathPlugin;

impl Plugin for DeathPlugin {
    fn build(&self, app: &mut App) {
        use crate::scenes::SceneState;

        app.init_resource::<PlayerDeath>()
            .init_resource::<ResurrectRequest>()
            .add_systems(OnExit(SceneState::GameWorld), cleanup_death_window)
            .add_systems(
                Update,
                (
                    sync_death_window,
                    reopen_death_window_on_corpse_click,
                    notice_unanswered_resurrect,
                )
                    .run_if(in_state(SceneState::GameWorld)),
            );
    }
}
