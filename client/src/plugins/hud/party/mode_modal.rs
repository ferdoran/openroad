//! `ifsetpartymode.txt` — the party-mode modal.
//!
//! Idea: this is where the `SRParty.Setup` flags are chosen, *before* the
//! create request goes out. That matters more than a settings dialog usually
//! would, because one of the three bits is not cosmetic: `EXP_SHARED` changes
//! the party's capacity from 4 to 8. `PartyCreateSetup` has existed as the seam
//! this writes to since the roster landed; this fills it.
//!
//! The tree declares **no frame control at all** — zero `CIFSubFrame`, zero
//! `CIFFrame`, no frame `DDJ` — so the chrome is code-supplied, which is what
//! `hud::modal_dialog` provides. The plate size is not authored either; it
//! falls out of the background: `_BG` is `16,40,268,144`, and the msgbox2
//! insets are 16/40/16, so the plate is `300x200` exactly. `the_plate_wraps_the_background`
//! pins that derivation rather than leaving 300x200 as a magic pair.
//!
//! Two data facts shape the controls:
//!
//! - the two `CIFRadioButton`s are **group containers**, not buttons. Each is a
//!   single block 120x44 carrying `Text=""` and `DDJ=""` while having to offer
//!   two mutually exclusive options, so no block carries a per-option label or
//!   art and the options are code-fed. Every mode string sits in one contiguous
//!   run (L502-L509) that appears in **no** resinfo file at all.
//! - the "can invite" control is a `CIFCheckBox` whose art is
//!   `com_radiobutton_off.ddj`. That looks like an authoring slip and it is
//!   followed anyway: the data is the reference, and swapping in a checkbox
//!   sprite would be us redesigning it.
//!
//! `msgbox_blackbox_03.ddj` is RGB565 with **no alpha mask** — bit 15 is not
//! alpha, so it must not be keyed.

use bevy::ecs::relationship::RelatedSpawnerCommands;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui_widgets::{Activate, Button};

use packets::agent::party::PartySetup;

use crate::assets::FontAssets;
use crate::plugins::hud::game_window::abs_node;
use crate::plugins::hud::modal_dialog::{modal_plate_node, modal_scrim_node, spawn_modal_frame};
use crate::plugins::hud::party::ui::PartyWindowRequest;
use crate::plugins::hud::scale::hud_scale;
use crate::plugins::net::party::{PartyCreateSetup, PartyRoster};
use crate::plugins::textdata::ClientUiStrings;
use crate::plugins::ui_v2::style::ImageButtonStyle;

// --- Layout (plate space, verbatim from ifsetpartymode.txt) -----------------

/// `_BG` is `16,40,268,144`; the msgbox2 insets are 16 at the sides, 40 on top
/// and 16 at the bottom, so the plate is exactly this big.
const PLATE_W: f32 = 300.0;
const PLATE_H: f32 = 200.0;

const BG_RECT: (f32, f32, f32, f32) = (16.0, 40.0, 268.0, 144.0);
const MESSAGE_RECT: (f32, f32, f32, f32) = (7.0, 42.0, 285.0, 20.0);
const BOX_EXP_RECT: (f32, f32, f32, f32) = (20.0, 72.0, 128.0, 52.0);
const BOX_ITEM_RECT: (f32, f32, f32, f32) = (151.0, 72.0, 128.0, 52.0);
const GROUP_EXP_RECT: (f32, f32, f32, f32) = (24.0, 76.0, 120.0, 44.0);
const GROUP_ITEM_RECT: (f32, f32, f32, f32) = (155.0, 76.0, 120.0, 44.0);
const INVITE_BOX_RECT: (f32, f32, f32, f32) = (260.0, 132.0, 16.0, 16.0);
const INVITE_LABEL_RECT: (f32, f32, f32, f32) = (12.0, 140.0, 240.0, 17.0);
const OK_RECT: (f32, f32, f32, f32) = (68.0, 158.0, 76.0, 24.0);
const CANCEL_RECT: (f32, f32, f32, f32) = (156.0, 158.0, 76.0, 24.0);

/// One option row inside a 120x44 group: two rows of 22, each a 16x16 mark
/// plus its label. The split is ours — the group is one block with no per-row
/// rects — but it is forced: two options in 44 px is 22 each.
const OPTION_H: f32 = 22.0;
const MARK_SIZE: f32 = 16.0;

const ART_COMMON: &str = "media://interface/ifcommon/";
const ART_MSGBOX: &str = "media://interface/messagebox/";

// --- State ------------------------------------------------------------------

/// The modal's own state. The flags mirror `PartySetup`'s three bits and are
/// committed to [`PartyCreateSetup`] only on OK — Cancel must leave a party the
/// player never confirmed unchanged.
#[derive(Resource, Debug, Default)]
pub struct PartyModeState {
    pub open: bool,
    pub exp_shared: bool,
    pub item_shared: bool,
    pub anyone_can_invite: bool,
}

impl PartyModeState {
    /// The flags as the wire byte.
    pub fn setup(&self) -> PartySetup {
        let mut bits = 0;
        if self.exp_shared {
            bits |= PartySetup::EXP_SHARED;
        }
        if self.item_shared {
            bits |= PartySetup::ITEM_SHARED;
        }
        if self.anyone_can_invite {
            bits |= PartySetup::ANYONE_CAN_INVITE;
        }
        PartySetup(bits)
    }
}

#[derive(Component)]
pub struct PartyModeRoot;
/// `(group, option)` — group 0 is EXP, 1 is ITEM; option 0 is "auto share".
#[derive(Component, Clone, Copy)]
struct PartyModeOption(usize, usize);
#[derive(Component)]
struct PartyModeInviteBox;
#[derive(Component, Clone, Copy)]
enum PartyModeButton {
    Ok,
    Cancel,
}

// --- Open / close -----------------------------------------------------------

/// Open the modal when the roster page's "Set" button asks for it, seeded from
/// the party's own setup when there is one and from the pending choice when
/// there is not.
///
/// Seeding from `PartyCreateSetup` alone showed the player a value the party
/// might not have: the pending byte is only ever *sent*, and once a party
/// exists the server's own `setup` (0x3065) is the truth. Opening "Set" while
/// in a party must show that, not the last thing that was typed into the box.
pub fn open_party_mode_modal(
    mut requests: MessageReader<PartyWindowRequest>,
    pending: Res<PartyCreateSetup>,
    roster: Option<Res<PartyRoster>>,
    mut state: ResMut<PartyModeState>,
) {
    for request in requests.read() {
        if !matches!(request, PartyWindowRequest::Setting) {
            continue;
        }
        let setup = match roster.as_deref().filter(|roster| roster.is_active()) {
            Some(roster) => roster.setup,
            None => pending.0,
        };
        state.exp_shared = setup.is_exp_shared();
        state.item_shared = setup.is_item_shared();
        state.anyone_can_invite = setup.anyone_can_invite();
        state.open = true;
    }
}

/// Rebuild the modal whenever its state changes. It is small enough that a
/// rebuild is simpler than a repaint, and the radio marks are the only thing
/// that ever changes.
pub fn sync_party_mode_modal(
    state: Res<PartyModeState>,
    ui_strings: Res<ClientUiStrings>,
    asset_server: Res<AssetServer>,
    fonts: Res<FontAssets>,
    cameras: Query<Entity, With<Camera2d>>,
    roots: Query<Entity, With<PartyModeRoot>>,
    mut commands: Commands,
) {
    if !state.is_changed() {
        return;
    }
    for root in roots.iter() {
        commands.entity(root).despawn();
    }
    if !state.open {
        return;
    }
    // See the note in `party_matching::dialogs`: `single()` fails on more
    // than one `Camera2d`, and a silent return here is indistinguishable from
    // a dead button.
    let Some(camera) = cameras.iter().next() else {
        warn!("party: no 2d camera, mode modal not spawned");
        return;
    };
    let s = hud_scale();

    let text_font = |size: f32| TextFont {
        font: fonts.two.clone().into(),
        font_size: FontSize::Px(size * s),
        ..default()
    };

    // Captured out of the button loop so the root can point Enter at OK
    // (`hud::focus::HudDialog`).
    let mut confirm_button = None;
    let root = commands
        .spawn((
            PartyModeRoot,
            Name::from("Party Mode Modal"),
            GlobalZIndex(90),
            modal_scrim_node(),
            UiTargetCamera(camera),
        ))
        .with_children(|scrim| {
            scrim
                .spawn(modal_plate_node(PLATE_W, PLATE_H, s))
                .with_children(|plate| {
                    spawn_modal_frame(plate, &asset_server, PLATE_W, PLATE_H, s);

                    plate.spawn((
                        abs_node(BG_RECT, s),
                        ImageNode {
                            image: asset_server
                                .load(format!("{ART_COMMON}bg_tile/com_bg_tile_b.ddj")),
                            image_mode: NodeImageMode::Stretch,
                            ..default()
                        },
                        Pickable::IGNORE,
                    ));

                    plate.spawn((
                        Text::new(
                            ui_strings
                                .get_or("UIIT_STT_SET_PARTY_INFO_MSG", "Set the party properties.")
                                .to_string(),
                        ),
                        text_font(9.0),
                        TextColor(Color::WHITE),
                        TextLayout::justify(Justify::Center),
                        abs_node(MESSAGE_RECT, s),
                        Pickable::IGNORE,
                    ));

                    for rect in [BOX_EXP_RECT, BOX_ITEM_RECT] {
                        plate.spawn((
                            abs_node(rect, s),
                            ImageNode {
                                image: asset_server
                                    .load(format!("{ART_MSGBOX}msgbox_blackbox_03.ddj")),
                                image_mode: NodeImageMode::Stretch,
                                ..default()
                            },
                            Pickable::IGNORE,
                        ));
                    }

                    // Group 0 = EXP, group 1 = ITEM; option 0 = auto share.
                    let groups = [
                        (
                            0usize,
                            GROUP_EXP_RECT,
                            state.exp_shared,
                            ["UIIT_STT_PARTY_EXP_SHARE", "UIIT_STT_PARTY_EXP_SELF"],
                            ["Exp Auto Share", "Exp Free-For-All"],
                        ),
                        (
                            1,
                            GROUP_ITEM_RECT,
                            state.item_shared,
                            ["UIIT_STT_PARTY_ITEM_SHARE", "UIIT_STT_PARTY_ITEM_SELF"],
                            ["Item Auto Share", "Item Free-For-All"],
                        ),
                    ];
                    for (group, rect, shared, keys, fallbacks) in groups {
                        for option in 0..2usize {
                            let selected = (option == 0) == shared;
                            spawn_option(
                                plate,
                                &asset_server,
                                group,
                                option,
                                option_rect(rect, option),
                                ui_strings.get_or(keys[option], fallbacks[option]),
                                selected,
                                text_font(8.5),
                                s,
                            );
                        }
                    }

                    plate.spawn((
                        Text::new(
                            ui_strings
                                .get_or(
                                    "UIIT_STT_PARTY_INVITATION_ANYONE",
                                    "Can invite without master status.",
                                )
                                .to_string(),
                        ),
                        text_font(8.5),
                        TextColor(Color::WHITE),
                        abs_node(INVITE_LABEL_RECT, s),
                        Pickable::IGNORE,
                    ));
                    plate
                        .spawn((
                            PartyModeInviteBox,
                            Button,
                            Hovered::default(),
                            abs_node(INVITE_BOX_RECT, s),
                            ImageNode {
                                image: asset_server.load(mark_art(state.anyone_can_invite)),
                                image_mode: NodeImageMode::Stretch,
                                ..default()
                            },
                        ))
                        .observe(on_invite_box);

                    for (button, rect, key, fallback) in [
                        (PartyModeButton::Ok, OK_RECT, "UIIS_CTL_CONFIRM", "OK"),
                        (
                            PartyModeButton::Cancel,
                            CANCEL_RECT,
                            "UIIS_CTL_CANCEL",
                            "Cancel",
                        ),
                    ] {
                        let is_ok = matches!(button, PartyModeButton::Ok);
                        let mut spawned = plate.spawn((
                            button,
                            Button,
                            Hovered::default(),
                            ImageButtonStyle {
                                normal: asset_server.load(format!("{ART_COMMON}com_button.ddj")),
                                hover: asset_server
                                    .load(format!("{ART_COMMON}com_button_focus.ddj")),
                                press: asset_server
                                    .load(format!("{ART_COMMON}com_button_press.ddj")),
                                ..Default::default()
                            },
                            ImageNode {
                                image: asset_server.load(format!("{ART_COMMON}com_button.ddj")),
                                image_mode: NodeImageMode::Stretch,
                                ..default()
                            },
                            abs_node(rect, s),
                        ));
                        spawned.observe(on_mode_button);
                        if is_ok {
                            confirm_button = Some(spawned.id());
                        }
                        spawned.with_children(|button| {
                            button.spawn((
                                Text::new(ui_strings.get_or(key, fallback).to_string()),
                                text_font(8.5),
                                TextColor(Color::WHITE),
                                TextLayout::justify(Justify::Center),
                                Node {
                                    position_type: PositionType::Absolute,
                                    left: Val::Px(0.0),
                                    top: Val::Px(6.0 * s),
                                    width: Val::Px(rect.2 * s),
                                    ..default()
                                },
                                Pickable::IGNORE,
                            ));
                        });
                    }
                });
        })
        .id();
    if let Some(confirm_button) = confirm_button {
        commands
            .entity(root)
            .insert(crate::plugins::hud::focus::HudDialog { confirm_button });
    }
}

/// Where option `n` sits inside a group container.
fn option_rect(group: (f32, f32, f32, f32), option: usize) -> (f32, f32, f32, f32) {
    (
        group.0,
        group.1 + option as f32 * OPTION_H,
        group.2,
        OPTION_H,
    )
}

fn mark_art(on: bool) -> String {
    // The data names `com_radiobutton_off.ddj` even for the CheckBox, so both
    // the groups and the checkbox use the radio sprite pair.
    format!(
        "{ART_COMMON}com_radiobutton_{}.ddj",
        if on { "on" } else { "off" }
    )
}

#[allow(clippy::too_many_arguments)]
fn spawn_option(
    parent: &mut RelatedSpawnerCommands<ChildOf>,
    asset_server: &AssetServer,
    group: usize,
    option: usize,
    rect: (f32, f32, f32, f32),
    label: &str,
    selected: bool,
    font: TextFont,
    s: f32,
) {
    parent
        .spawn((
            PartyModeOption(group, option),
            Button,
            Hovered::default(),
            abs_node(rect, s),
        ))
        .observe(on_option)
        .with_children(|row| {
            row.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    top: Val::Px((rect.3 - MARK_SIZE) / 2.0 * s),
                    width: Val::Px(MARK_SIZE * s),
                    height: Val::Px(MARK_SIZE * s),
                    ..default()
                },
                ImageNode {
                    image: asset_server.load(mark_art(selected)),
                    image_mode: NodeImageMode::Stretch,
                    ..default()
                },
                Pickable::IGNORE,
            ));
            row.spawn((
                Text::new(label.to_string()),
                font,
                TextColor(Color::WHITE),
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px((MARK_SIZE + 4.0) * s),
                    top: Val::Px(3.0 * s),
                    width: Val::Px((rect.2 - MARK_SIZE - 4.0) * s),
                    ..default()
                },
                Pickable::IGNORE,
            ));
        });
}

// --- Observers --------------------------------------------------------------

fn on_option(
    activate: On<Activate>,
    options: Query<&PartyModeOption>,
    mut state: ResMut<PartyModeState>,
) {
    let Ok(option) = options.get(activate.entity) else {
        return;
    };
    // Option 0 is "auto share" in both groups, so the flag is simply whether
    // the first row was picked.
    let shared = option.1 == 0;
    match option.0 {
        0 => state.exp_shared = shared,
        1 => state.item_shared = shared,
        _ => {}
    }
}

fn on_invite_box(_: On<Activate>, mut state: ResMut<PartyModeState>) {
    state.anyone_can_invite = !state.anyone_can_invite;
}

fn on_mode_button(
    activate: On<Activate>,
    buttons: Query<&PartyModeButton>,
    mut state: ResMut<PartyModeState>,
    mut pending: ResMut<PartyCreateSetup>,
) {
    let Ok(button) = buttons.get(activate.entity) else {
        return;
    };
    if let PartyModeButton::Ok = button {
        // Only OK commits: a cancelled dialog must leave the pending setup
        // exactly as it was, since it is what the next 0x7060 will send.
        pending.0 = state.setup();
    }
    state.open = false;
}

// --- Plugin -----------------------------------------------------------------

pub struct PartyModeModalPlugin;

impl Plugin for PartyModeModalPlugin {
    fn build(&self, app: &mut App) {
        use crate::scenes::SceneState;

        app.init_resource::<PartyModeState>().add_systems(
            Update,
            (open_party_mode_modal, sync_party_mode_modal)
                .chain()
                .run_if(in_state(SceneState::GameWorld).or_else(in_state(SceneState::UiTesting))),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::hud::modal_dialog::{MODAL_SIDE, MODAL_TOP};

    /// The same headless guard the other party surfaces carry — a query
    /// conflict panics the schedule on first spawn, not at compile time.
    #[test]
    fn the_modal_systems_have_disjoint_queries() {
        let mut world = World::new();
        let mut schedule = Schedule::default();
        schedule.add_systems((open_party_mode_modal, sync_party_mode_modal));
        schedule
            .initialize(&mut world)
            .expect("the mode modal must build a valid schedule");
    }

    /// The plate size is derived from the authored background and the msgbox2
    /// insets, not chosen. If either moves, this is what says so.
    #[test]
    fn the_plate_wraps_the_background() {
        assert_eq!(BG_RECT.0, MODAL_SIDE);
        assert_eq!(BG_RECT.1, MODAL_TOP);
        assert_eq!(PLATE_W, BG_RECT.2 + 2.0 * MODAL_SIDE);
        assert_eq!(PLATE_H, BG_RECT.1 + BG_RECT.3 + 16.0);
        assert_eq!((PLATE_W, PLATE_H), (300.0, 200.0));
    }

    /// Two options split a 44-tall group evenly, and each stays inside the
    /// black box its group sits in.
    #[test]
    fn options_split_their_group_and_stay_inside_the_box() {
        let first = option_rect(GROUP_EXP_RECT, 0);
        let second = option_rect(GROUP_EXP_RECT, 1);
        assert_eq!(first.1, GROUP_EXP_RECT.1);
        assert_eq!(second.1, GROUP_EXP_RECT.1 + OPTION_H);
        assert_eq!(second.1 + second.3, GROUP_EXP_RECT.1 + GROUP_EXP_RECT.3);

        // group inside its blackbox, both dimensions
        assert!(GROUP_EXP_RECT.0 >= BOX_EXP_RECT.0);
        assert!(GROUP_EXP_RECT.0 + GROUP_EXP_RECT.2 <= BOX_EXP_RECT.0 + BOX_EXP_RECT.2);
        assert!(GROUP_ITEM_RECT.0 >= BOX_ITEM_RECT.0);
        assert!(GROUP_ITEM_RECT.0 + GROUP_ITEM_RECT.2 <= BOX_ITEM_RECT.0 + BOX_ITEM_RECT.2);
    }

    /// The three flags map onto `PartySetup`'s bits, and EXP sharing is the one
    /// that changes the party's capacity.
    #[test]
    fn the_flags_become_the_wire_byte() {
        let mut state = PartyModeState::default();
        assert_eq!(state.setup().0, 0);
        assert_eq!(state.setup().capacity(), 4);

        state.exp_shared = true;
        state.item_shared = true;
        state.anyone_can_invite = true;
        let setup = state.setup();
        assert!(setup.is_exp_shared() && setup.is_item_shared() && setup.anyone_can_invite());
        assert_eq!(setup.capacity(), 8);
    }

    /// The two buttons do not overlap and both fit on the plate.
    #[test]
    fn the_footer_buttons_fit() {
        assert!(OK_RECT.0 + OK_RECT.2 < CANCEL_RECT.0);
        assert!(CANCEL_RECT.0 + CANCEL_RECT.2 < PLATE_W);
        assert!(CANCEL_RECT.1 + CANCEL_RECT.3 <= BG_RECT.1 + BG_RECT.3);
    }
}
