//! The target right-click menu (`res_ui/targetmenu.2dt`, `CNIFTargetMenu` id
//! 174) — five verbs on the selected player.
//!
//! Idea: this is not its own window class. `targetmenu.2dt` is the *same*
//! three-part construction as the under-bar's Menu flyout — a `CNIFrame` on
//! `ub_new_wnd_`, a `com_bg_tile_u` fill at inset 20, rows of
//! `ub_new_menu_button.ddj` — at a different size, and the recovered client
//! module map homes that construction to `NIFUnderMenuBar.cpp` with no
//! `NIFTargetMenu.cpp` anywhere in its 161 sources. So the widget lives in
//! [`crate::plugins::hud::context_menu`] and this module is only the *item list*
//! (`docs/re/ui/hud-target-menu.md` §3.3).
//!
//! Two traps from the descriptor, both avoided by construction here:
//!
//! 1. **The rows are neither in id order nor in record order.** Id 17 ("Add
//!    friend") renders *above* id 16 ("Invite party"), and record `[5]` precedes
//!    record `[4]` on screen. [`TargetAction::ORDER`] is the visual order and the
//!    ids are carried only as documentation.
//! 2. **The root's `Text` is `UIIT_CTL_AUTOTRACE_TT`** — byte-identical to its
//!    own last row's key. A `CNIFrame` root has nothing to draw a caption on and
//!    "Trace" is not a title for a five-item menu, so it is authoring
//!    copy-paste and is deliberately not rendered.
//!
//! Anchor: the authored root `638,321` sits 95px into the target window's
//! `543,289,196,36` band and 4px above its bottom edge, so the menu is read as
//! anchored to the target window rather than to the cursor (`[S]`, §9-U1) — the
//! offsets below are that reading, expressed against our own panel.
//!
//! The five verbs are emitted as [`TargetMenuAction`] messages and each needs
//! its own wire path. "Invite party" has one now
//! ([`send_target_menu_party_invite`] → `net::party::PartyAction`) and so does
//! "Exchange" ([`send_target_menu_exchange_invite`] → 0x7081); whisper and
//! friend still do not, and inventing one here would be a guess.
//! The menu stays honest about what exists — it opens, it is localized, it
//! names the action — and the wiring is a separate issue per verb.

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::assets::FontAssets;
use crate::net::connection::SilkroadConnection;
use crate::plugins::cursor::interactions::entity_select::SelectedEntity;
use crate::plugins::hud::chat::model::{ChatHistory, ChatLine};
use crate::plugins::hud::context_menu::{
    close_context_menus, spawn_context_menu, ContextMenuItem, ContextMenuOwner, ContextMenuRoot,
    ContextMenuRow,
};
use crate::plugins::hud::exchange;
use crate::plugins::hud::scale::hud_scale;
use crate::plugins::hud::system_message::model::format_template;
use crate::plugins::net::agent::AgentConnection;
use crate::plugins::net::entities::{DisplayName, NetworkId, RemoteEntity};
use crate::plugins::net::party::PartyAction;
use crate::plugins::textdata::ClientUiStrings;

/// Target-window geometry the anchor is expressed against
/// (`iftargetwindow.txt`: the player plate is `196x36` at `PANEL_TOP`).
const TARGET_FRAME_W: f32 = 196.0;
const TARGET_PANEL_TOP: f32 = 6.0;
/// The authored offsets of the menu root inside the target window's band:
/// `638 - 543 = 95` horizontally, `321 - 289 = 32` vertically.
const MENU_DX: f32 = 95.0;
const MENU_DY: f32 = 32.0;

/// A right-click that moved more than this many logical pixels was a camera
/// drag, not a click. **openroad choice** — the original has no such threshold
/// because its right button is not the camera's.
const DRAG_SLOP: f32 = 4.0;

/// The five verbs, in the order they render.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetAction {
    Whisper,
    Exchange,
    AddFriend,
    InviteParty,
    Trace,
}

impl TargetAction {
    /// Visual (y) order — *not* the descriptor's id or record order.
    pub const ORDER: [TargetAction; 5] = [
        TargetAction::Whisper,
        TargetAction::Exchange,
        TargetAction::AddFriend,
        TargetAction::InviteParty,
        TargetAction::Trace,
    ];

    /// textuisystem key, with the English column as the offline fallback.
    pub fn label(self) -> (&'static str, &'static str) {
        match self {
            TargetAction::Whisper => ("UIIT_STT_GET_WHISPER", "Whisper"),
            TargetAction::Exchange => ("UIIT_CTL_EXCHANGE_TT", "Exchange"),
            TargetAction::AddFriend => ("UIIT_CTL_FRIENDADD", "Add friend"),
            TargetAction::InviteParty => ("UIIT_STT_INVITE_PARTY", "Invite party"),
            TargetAction::Trace => ("UIIT_CTL_AUTOTRACE_TT", "Trace"),
        }
    }

    /// The descriptor's control id — documentation only, never an ordering key.
    pub fn original_id(self) -> u32 {
        match self {
            TargetAction::Whisper => 14,
            TargetAction::Exchange => 15,
            TargetAction::AddFriend => 17,
            TargetAction::InviteParty => 16,
            TargetAction::Trace => 18,
        }
    }
}

/// A picked menu row. Each verb's wire path is a separate issue; this is the
/// seam they plug into.
#[derive(Message, Clone, Copy, Debug)]
pub struct TargetMenuAction {
    pub target: Entity,
    pub action: TargetAction,
}

/// The entity the open menu acts on.
#[derive(Resource, Default)]
pub struct TargetMenuTarget(pub Option<Entity>);

/// Open the menu on a right-click that did not drag the camera, when the
/// current target is another player. Right-drag rotates the camera in openroad,
/// so the press position is remembered and a moved release is ignored.
#[allow(clippy::too_many_arguments)]
pub fn open_target_menu(
    buttons: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    selected: Res<SelectedEntity>,
    remotes: Query<&RemoteEntity>,
    ui_strings: Res<ClientUiStrings>,
    asset_server: Res<AssetServer>,
    fonts: Res<FontAssets>,
    cameras: Query<Entity, With<Camera2d>>,
    open: Query<Entity, With<ContextMenuRoot>>,
    mut target: ResMut<TargetMenuTarget>,
    mut owner: ResMut<ContextMenuOwner>,
    mut press_at: Local<Option<Vec2>>,
    mut commands: Commands,
) {
    let cursor = windows.iter().next().and_then(|w| w.cursor_position());
    if buttons.just_pressed(MouseButton::Right) {
        *press_at = cursor;
        return;
    }
    if !buttons.just_released(MouseButton::Right) {
        return;
    }
    let moved = match (press_at.take(), cursor) {
        (Some(down), Some(up)) => down.distance(up) > DRAG_SLOP,
        _ => true,
    };
    if moved {
        return;
    }

    close_context_menus(&mut commands, &open, &mut owner);
    target.0 = None;

    let Some(entity) = selected.0 else { return };
    if !matches!(remotes.get(entity), Ok(RemoteEntity::Player)) {
        return;
    }
    let Some(camera) = cameras.iter().next() else {
        return;
    };

    let items: Vec<ContextMenuItem> = TargetAction::ORDER
        .iter()
        .map(|action| {
            let (key, fallback) = action.label();
            ContextMenuItem {
                label: ui_strings.get_or(key, fallback).to_string(),
                enabled: true,
            }
        })
        .collect();

    spawn_context_menu(
        &mut commands,
        &asset_server,
        &fonts,
        camera,
        Val::Percent(50.0),
        Val::Px((TARGET_PANEL_TOP + MENU_DY) * hud_scale()),
        // the target window is centred, so its left edge is 50% - half its
        // width; the authored menu sits MENU_DX into that band.
        Val::Px((MENU_DX - TARGET_FRAME_W / 2.0) * hud_scale()),
        &items,
        hud_scale(),
    );
    target.0 = Some(entity);
    *owner = ContextMenuOwner::Target;
}

/// Fire the picked row's action and close the menu.
pub fn activate_target_menu_row(
    buttons: Res<ButtonInput<MouseButton>>,
    rows: Query<(&ContextMenuRow, &Hovered)>,
    open: Query<Entity, With<ContextMenuRoot>>,
    mut target: ResMut<TargetMenuTarget>,
    mut owner: ResMut<ContextMenuOwner>,
    mut actions: MessageWriter<TargetMenuAction>,
    mut commands: Commands,
) {
    if !buttons.just_released(MouseButton::Left) {
        return;
    }
    if *owner != ContextMenuOwner::Target {
        return;
    }
    let Some(entity) = target.0 else { return };
    let picked = rows
        .iter()
        .find(|(_, hovered)| hovered.get())
        .and_then(|(row, _)| TargetAction::ORDER.get(row.0).copied());
    if let Some(action) = picked {
        actions.write(TargetMenuAction {
            target: entity,
            action,
        });
    }
    // any left click dismisses the menu, picked or not
    close_context_menus(&mut commands, &open, &mut owner);
    target.0 = None;
}

/// Close the menu when its target goes away (deselected, out of range, dead).
pub fn close_target_menu_on_deselect(
    selected: Res<SelectedEntity>,
    open: Query<Entity, With<ContextMenuRoot>>,
    mut target: ResMut<TargetMenuTarget>,
    mut owner: ResMut<ContextMenuOwner>,
    mut commands: Commands,
) {
    let Some(entity) = target.0 else { return };
    if selected.0 != Some(entity) {
        close_context_menus(&mut commands, &open, &mut owner);
        target.0 = None;
    }
}

/// "Invite party" — the first row with a wire path.
///
/// The row names a *screen* entity; the wire wants the target's spawn id, which
/// is [`NetworkId`]. Whether that becomes 0x7060 or 0x7062 is not decided here:
/// the menu has no idea whether we are in a party, so it states the intent and
/// [`PartyAction`]'s sender resolves it against the roster.
pub fn send_target_menu_party_invite(
    mut actions: MessageReader<TargetMenuAction>,
    ids: Query<&NetworkId>,
    mut party: MessageWriter<PartyAction>,
) {
    for action in actions.read() {
        if action.action != TargetAction::InviteParty {
            continue;
        }
        match ids.get(action.target) {
            Ok(id) => {
                party.write(PartyAction::Invite(id.0));
            }
            // Selected entities always carry a NetworkId; if one does not, the
            // menu should say so rather than send a zero uid.
            Err(_) => warn!(
                "target menu: invite party on {:?}, which has no NetworkId",
                action.target
            ),
        }
    }
}

/// "Exchange" — the second row with a wire path.
///
/// Same shape as the party funnel: the row names a screen entity, the wire
/// wants its spawn id, and the trade window is *not* opened here — 0x7081 only
/// asks the server to raise the petition on the target
/// (`docs/net-invite-0x3080.md` §4). The window follows on 0x3085 if they
/// accept.
pub fn send_target_menu_exchange_invite(
    mut actions: MessageReader<TargetMenuAction>,
    targets: Query<(&NetworkId, Option<&DisplayName>)>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    ui_strings: Res<ClientUiStrings>,
    mut history: ResMut<ChatHistory>,
) {
    for action in actions.read() {
        if action.action != TargetAction::Exchange {
            continue;
        }
        match targets.get(action.target) {
            Ok((id, name)) => {
                exchange::model::send_invite(&conn, id.0);
                // The original tells the inviter it is waiting, with the
                // target's name — `UIIT_MSG_DEAL_ASKING` (L1714). Without it
                // an unanswered request looks like a dead click.
                let (key, fallback) = exchange::model::DEAL_ASKING;
                let who = name.map(|n| n.0.as_str()).unwrap_or_default();
                history.push(ChatLine::system(format_template(
                    ui_strings.get_or(key, fallback),
                    &[who],
                )));
            }
            Err(_) => warn!(
                "target menu: exchange on {:?}, which has no NetworkId",
                action.target
            ),
        }
    }
}

/// Until each verb has a wire path, name the picked action so the menu is
/// observable and the backlog is visible (the under-bar menu does the same for
/// its unbuilt rows). "Invite party" and "Exchange" are wired (see
/// [`send_target_menu_party_invite`] and [`send_target_menu_exchange_invite`])
/// and are therefore not reported here.
pub fn log_target_menu_actions(mut actions: MessageReader<TargetMenuAction>) {
    for action in actions.read() {
        if matches!(
            action.action,
            TargetAction::InviteParty | TargetAction::Exchange
        ) {
            continue;
        }
        info!(
            "target menu: {:?} on {:?} has no wire path yet",
            action.action, action.target
        );
    }
}

pub fn cleanup_target_menu(
    mut commands: Commands,
    open: Query<Entity, With<ContextMenuRoot>>,
    mut target: ResMut<TargetMenuTarget>,
    mut owner: ResMut<ContextMenuOwner>,
) {
    close_context_menus(&mut commands, &open, &mut owner);
    target.0 = None;
}

#[cfg(test)]
mod test {
    use super::*;

    /// The rows render in y order, which is neither id order nor record order —
    /// this is the file's own trap, so pin it.
    #[test]
    fn rows_render_in_visual_order_not_id_order() {
        let ids: Vec<u32> = TargetAction::ORDER
            .iter()
            .map(|a| a.original_id())
            .collect();
        // authored y: 353, 373, 392, 412, 431 → ids 14, 15, 17, 16, 18
        assert_eq!(ids, vec![14, 15, 17, 16, 18]);
        // ...which is NOT sorted: id 17 renders above id 16
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        assert_ne!(ids, sorted);
    }

    /// The five keys, verbatim from `targetmenu.2dt`'s `Text` fields.
    #[test]
    fn the_five_keys_match_the_descriptor() {
        let keys: Vec<&str> = TargetAction::ORDER.iter().map(|a| a.label().0).collect();
        assert_eq!(
            keys,
            vec![
                "UIIT_STT_GET_WHISPER",
                "UIIT_CTL_EXCHANGE_TT",
                "UIIT_CTL_FRIENDADD",
                "UIIT_STT_INVITE_PARTY",
                "UIIT_CTL_AUTOTRACE_TT",
            ]
        );
        // the root's own Text is the LAST row's key — a copy-paste, not a
        // caption; nothing here may render it as a title.
        assert_eq!(TargetAction::Trace.label().0, "UIIT_CTL_AUTOTRACE_TT");
    }

    /// The anchor is expressed as the authored offset inside the target
    /// window's band, not as the absolute `638,321` (which would pin the menu
    /// to one screen position at one resolution).
    #[test]
    fn the_anchor_is_relative_to_the_target_window() {
        // 638 - 543 = 95, 321 - 289 = 32
        assert_eq!(MENU_DX, 638.0 - 543.0);
        assert_eq!(MENU_DY, 321.0 - 289.0);
        // and it lands inside the 196-wide band, above its bottom edge
        assert!(MENU_DX > 0.0 && MENU_DX < TARGET_FRAME_W);
        assert!(MENU_DY < 36.0);
    }
}

/// Self-registration for the target right-click menu (#448) (#558). The HUD registry holds one line per
/// window, so two windows landing in the same lap no longer collide on it.
pub struct TargetMenuPlugin;

impl Plugin for TargetMenuPlugin {
    fn build(&self, app: &mut App) {
        use crate::scenes::SceneState;

        app.init_resource::<TargetMenuTarget>()
            .init_resource::<ContextMenuOwner>()
            .add_message::<TargetMenuAction>()
            .add_systems(OnExit(SceneState::GameWorld), cleanup_target_menu)
            // the ub_new_wnd_ popup, live game world only (it acts on a
            // selected remote player) (#448)
            .add_systems(
                Update,
                (
                    open_target_menu,
                    activate_target_menu_row,
                    close_target_menu_on_deselect,
                    send_target_menu_party_invite,
                    send_target_menu_exchange_invite,
                    log_target_menu_actions,
                )
                    .chain()
                    .run_if(in_state(SceneState::GameWorld)),
            );
    }
}
