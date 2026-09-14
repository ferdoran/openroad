//! Dragging an item out of the bag and onto the ground.
//!
//! Idea: the inventory carries an item with press/release rather than
//! `bevy_picking`'s `DragDrop` (see `ui.rs`'s header), and every existing
//! catcher for that carry is a UI node — a slot cell, the equipment panel, the
//! bag panel. A release that hits none of them simply left the carry live, so
//! "drag it out of the window" had no meaning at all.
//!
//! So the missing catcher is "nobody at all", and that is what
//! [`detect_drop_on_nothing`] adds: the same mouse edge every other catcher
//! polls, plus the observation that a carry is still live and the pointer is
//! outside every window that would take one.
//!
//! An earlier cut asked the *terrain* instead — `nav::decal`'s cursor ray had
//! already resolved a walkable hit, so it emitted the drop from there. That was
//! too narrow: releasing over a wall, over the sky, or over a HUD panel that
//! accepts nothing produced no drop and no feedback, because none of those is
//! walkable ground. `nav::decal` still suppresses the *move order* while a
//! carry is live — a click that drops an item must not also walk the character
//! — but it no longer decides what a drop is.
//!
//! **The confirmation is not optional.** Dropping is irreversible and public —
//! `UIIT_MSG_DROP_WARNING_1` is the original's own wording for why ("If you
//! drop this item anyone will be able to grab it.") — so the release raises a
//! modal and only its Confirm reaches the wire.
//!
//! ⚠️ **The opcode is unverified.** See [`InventoryOperationRequest::Drop`]:
//! op 7 is named by exactly one secondary source and appears in none of our
//! 191 captured 0x7034 bodies. The send is logged at `info!` on purpose, so the
//! first live drop doubles as the capture that settles it.

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui_widgets::{Activate, Button};

use packets::agent::prelude::InventoryOperationRequest;
use packets::Packet;

use crate::assets::FontAssets;
use crate::net::connection::SilkroadConnection;
use crate::plugins::hud::game_window::abs_node;
use crate::plugins::hud::modal_dialog::{
    modal_interior, modal_plate_node, modal_scrim_node, spawn_modal_frame,
};
use crate::plugins::hud::scale::hud_scale;
use crate::plugins::net::agent::AgentConnection;
use crate::plugins::net::inventory::Inventory;
use crate::plugins::player::Player;
use crate::plugins::textdata::{ClientItemData, ClientTextNames, ClientUiStrings};

use super::model::InventoryState;

/// The player released a carried item on nothing that would take it.
///
/// Carries no payload: the *what* is the live carry in
/// [`InventoryState::drag`], and the *where* is the server's business — a drop
/// lands at the character's feet, and the 0x7034 body has no room for a
/// position.
#[derive(Message, Debug, Clone, Copy)]
pub struct DroppedOnNothing;

/// Every window that will accept a carried inventory item.
///
/// A release inside any of these belongs to that window; a release anywhere
/// else is a drop. Kept as one alias so the list is stated once — a window that
/// grows a drop handler and is not added here would silently start asking to
/// destroy the item it just accepted.
///
/// Alchemy, the grant dialog and the quickslot bar hover their *cells* rather
/// than a root, so their roots carry `Hovered` purely for this query.
type DropTargetRoots = Or<(
    With<super::ui::InventoryRoot>,
    With<crate::plugins::hud::storage::ui::StorageWindowRoot>,
    With<crate::plugins::hud::store::ui::StoreWindowRoot>,
    With<crate::plugins::hud::alchemy::ui::AlchemyWindowRoot>,
    With<crate::plugins::hud::alchemy::grant::GrantWindowRoot>,
    With<crate::plugins::hud::underbar::ui::UnderbarRoot>,
)>;

/// Raise a drop when a carried item is released on nothing.
///
/// Idea: the six windows above each take a carried item by polling the same
/// mouse edge and clearing `InventoryState.drag` themselves. So "nobody took
/// it" is simply: the edge happened, a carry is still live, and the pointer is
/// not inside any of them.
///
/// **Ordering is load-bearing.** This must run after every catcher, because
/// clearing `drag` is the only signal any of them gives that the drop was
/// consumed. Registered with explicit `.after(...)` in
/// `hud::inventory::mod`; without it, depositing into storage would also ask
/// to destroy the item.
///
/// This replaces a terrain raycast in `nav::decal`, which only fired over
/// *walkable ground* — so releasing over a wall, over the sky, or over the chat
/// log did nothing at all.
pub fn detect_drop_on_nothing(
    buttons: Res<ButtonInput<MouseButton>>,
    state: Res<InventoryState>,
    targets: Query<&Hovered, DropTargetRoots>,
    confirm: Res<DropConfirm>,
    mut drops: MessageWriter<DroppedOnNothing>,
) {
    // Both edges, like every catcher: a hold-drag ends on release, and the
    // vanilla click-carry ends on the next press.
    if !buttons.just_released(MouseButton::Left) && !buttons.just_pressed(MouseButton::Left) {
        return;
    }
    if state.drag.is_none() || confirm.prompt.is_some() {
        return;
    }
    if targets.iter().any(|hovered| hovered.get()) {
        return;
    }
    drops.write(DroppedOnNothing);
}

/// The open drop confirmation, if any.
///
/// A resource rather than a message for the same reason as the other confirms
/// in this tree: the dialog is modal, so a second question raised while one is
/// up would have nowhere to go.
#[derive(Resource, Default)]
pub struct DropConfirm {
    pub prompt: Option<DropPrompt>,
}

/// The pending question: which slot, and the name to show for it.
#[derive(Debug, Clone)]
pub struct DropPrompt {
    /// Wire slot of the carried item.
    pub slot: u8,
    /// The item's display name, resolved when the prompt was raised.
    pub name: String,
}

#[derive(Component)]
pub struct DropConfirmRoot;

#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum DropConfirmButton {
    Confirm,
    Cancel,
}

/// The plate's own size. `msgbox2_window_` insets 16 at the sides, 40 at the
/// top and 16 at the bottom, so a 300x150 plate leaves a 268x94 interior — room
/// for the warning line, the item name and a button row.
const PLATE: (f32, f32) = (300.0, 150.0);
/// `com_button.ddj` measures 76x24 (`options-screen.md`).
const BUTTON: (f32, f32) = (76.0, 24.0);

/// Raise the confirmation when a carried item is released on nothing.
///
/// Resolving the name here rather than in the dialog keeps the prompt a plain
/// value: by the time the player answers, the inventory may already have moved
/// on (a server push can rewrite the slot), and the question they were asked
/// must not silently change under them.
///
/// **The carry ends here, as the dialog opens.** It used to survive until
/// Confirm, which left the ghost glued to the cursor and following it across
/// the modal — the player was still visibly holding an item while being asked
/// whether to throw it away. Since the prompt already captured the slot and the
/// name, nothing downstream needs the live carry: Confirm sends from
/// `prompt.slot`, and Cancel simply closes, leaving the item where it is.
pub fn on_ground_drop_release(
    mut releases: MessageReader<DroppedOnNothing>,
    mut state: ResMut<InventoryState>,
    inventories: Query<&Inventory, With<Player>>,
    item_data: Res<ClientItemData>,
    names: Res<ClientTextNames>,
    ghosts: Query<Entity, With<super::ui::DragGhost>>,
    mut confirm: ResMut<DropConfirm>,
    mut commands: Commands,
) {
    // One question at a time; the scrim swallows clicks anyway, but the
    // message could still be queued from the frame the dialog went up.
    let raised = !releases.is_empty();
    releases.clear();
    if !raised || confirm.prompt.is_some() {
        return;
    }
    let Some(slot) = state.drag else {
        return;
    };
    let Ok(inventory) = inventories.single() else {
        return;
    };
    let Some(item) = inventory.get(slot) else {
        // The carry outlived the item it started from (a server push emptied
        // the slot). Nothing to ask about — but the cursor must not keep
        // holding a ghost of an item that is gone.
        super::ui::end_carry(&mut state, &ghosts, &mut commands);
        return;
    };
    let name = item_data
        .get(&(item.ref_id as i32))
        .and_then(|row| row.name_key())
        .and_then(|key| names.name(key))
        .map(String::from)
        .unwrap_or_else(|| format!("Item #{}", item.ref_id));
    confirm.prompt = Some(DropPrompt { slot, name });
    super::ui::end_carry(&mut state, &ghosts, &mut commands);
}

/// Rebuild the dialog when the prompt changes — the same despawn-and-respawn
/// shape `use_on_item::sync_use_on_item_confirm` uses, on the shared
/// `msgbox2_window_` shell every other dialog in this tree draws on.
pub fn sync_drop_confirm(
    confirm: Res<DropConfirm>,
    existing: Query<Entity, With<DropConfirmRoot>>,
    ui_strings: Res<ClientUiStrings>,
    fonts: Res<FontAssets>,
    asset_server: Res<AssetServer>,
    cam_query: Query<Entity, With<Camera2d>>,
    mut commands: Commands,
) {
    if !confirm.is_changed() {
        return;
    }
    for entity in existing.iter() {
        commands.entity(entity).despawn();
    }
    let Some(prompt) = &confirm.prompt else {
        return;
    };
    let Ok(camera) = cam_query.single() else {
        return;
    };
    let s = hud_scale();
    let text_font = |size: f32| TextFont {
        font: fonts.two.clone().into(),
        font_size: bevy::text::FontSize::Px(size * s),
        ..default()
    };
    let button_style = crate::plugins::ui_v2::style::ImageButtonStyle {
        normal: asset_server.load("media://interface/ifcommon/com_button.ddj"),
        hover: asset_server.load("media://interface/ifcommon/com_button_focus.ddj"),
        press: asset_server.load("media://interface/ifcommon/com_button_press.ddj"),
        ..Default::default()
    };
    // The original's own warning for this action.
    let warning = ui_strings
        .get_or(
            "UIIT_MSG_DROP_WARNING_1",
            "If you drop this item anyone will be able to grab it.",
        )
        .to_string();

    // Captured out of the button loop so the root can point Enter at Confirm
    // (`hud::focus::HudDialog`).
    let mut confirm_button = None;
    let root = commands
        .spawn((
            DropConfirmRoot,
            Name::from("Drop Confirm"),
            modal_scrim_node(),
            GlobalZIndex(80),
            bevy::ui::UiTargetCamera(camera),
        ))
        .with_children(|scrim| {
            scrim
                .spawn((
                    modal_plate_node(PLATE.0, PLATE.1, s),
                    Name::from("Drop Confirm Plate"),
                ))
                .with_children(|plate| {
                    spawn_modal_frame(plate, &asset_server, PLATE.0, PLATE.1, s);
                    // The interior fill. `spawn_modal_frame` is the 8-piece
                    // `msgbox2_window_` *ring* and nothing else — the family has
                    // no centre piece, and its doc says it "leaves the interior
                    // to the caller". Without this the middle of the dialog is
                    // the scrim showing through. `com_bg_tile_b.ddj` is what
                    // `docs/re/ui/message-box.md` records for this family
                    // (`GDR_MSGBOX_BG:CIFNormalTile`), and both sibling callers
                    // — `party::mode_modal` and `party_matching::dialogs` —
                    // spawn it here too.
                    plate.spawn((
                        abs_node(modal_interior(PLATE.0, PLATE.1), s),
                        ImageNode {
                            image: asset_server
                                .load("media://interface/ifcommon/bg_tile/com_bg_tile_b.ddj"),
                            image_mode: NodeImageMode::Stretch,
                            ..default()
                        },
                        Pickable::IGNORE,
                    ));
                    plate.spawn((
                        Text::new(warning),
                        text_font(8.0),
                        TextColor(Color::srgb(0.95, 0.35, 0.35)),
                        TextLayout::justify(Justify::Center),
                        abs_node((20.0, 48.0, 260.0, 34.0), s),
                        Pickable::IGNORE,
                    ));
                    plate.spawn((
                        Text::new(prompt.name.clone()),
                        text_font(9.0),
                        TextColor(Color::srgb(1.0, 0.85, 0.32)),
                        TextLayout::justify(Justify::Center),
                        abs_node((20.0, 86.0, 260.0, 16.0), s),
                        Pickable::IGNORE,
                    ));
                    for (button, key, fallback, x) in [
                        (
                            DropConfirmButton::Confirm,
                            "UIIT_CTL_CONFIRM",
                            "Confirm",
                            42.0,
                        ),
                        (
                            DropConfirmButton::Cancel,
                            "UIIT_CTL_CANCEL",
                            "Cancel",
                            182.0,
                        ),
                    ] {
                        let is_confirm = button == DropConfirmButton::Confirm;
                        let mut spawned = plate.spawn((
                            button,
                            Button,
                            Hovered::default(),
                            abs_node((x, 110.0, BUTTON.0, BUTTON.1), s),
                            ImageNode {
                                image: button_style.normal.clone(),
                                image_mode: NodeImageMode::Stretch,
                                ..default()
                            },
                            button_style.clone(),
                        ));
                        spawned.observe(on_drop_confirm_button);
                        if is_confirm {
                            confirm_button = Some(spawned.id());
                        }
                        spawned.with_children(|b| {
                            b.spawn((
                                Text::new(ui_strings.get_or(key, fallback).to_string()),
                                text_font(8.0),
                                TextColor(Color::WHITE),
                                TextLayout::justify(Justify::Center),
                                Node {
                                    position_type: PositionType::Absolute,
                                    top: Val::Px(6.0 * s),
                                    width: Val::Percent(100.0),
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

/// Confirm sends the drop; Cancel just closes.
///
/// Neither touches the carry: it already ended when the dialog opened (see
/// [`on_ground_drop_release`]), so by this point the prompt is the only thing
/// holding the slot. Cancel therefore means "leave the item where it is",
/// which is what it looks like once the cursor is empty.
fn on_drop_confirm_button(
    activate: On<Activate>,
    buttons: Query<&DropConfirmButton>,
    mut confirm: ResMut<DropConfirm>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
) {
    let Ok(button) = buttons.get(activate.entity) else {
        return;
    };
    let Some(prompt) = confirm.prompt.take() else {
        return;
    };
    if *button == DropConfirmButton::Cancel {
        return;
    }
    send_drop(&conn, prompt.slot, &prompt.name);
}

/// Put the drop on the wire, loudly.
///
/// `info!` rather than `debug!` because this is the only unverified opcode the
/// client sends by default: the line, plus `packet_dump/c2s/0x7034.log`, is the
/// evidence that settles whether op 7 is right (see
/// [`InventoryOperationRequest::Drop`]).
fn send_drop(conn: &Query<&SilkroadConnection, With<AgentConnection>>, slot: u8, name: &str) {
    let Ok(connection) = conn.single() else {
        info!("drop: offline — would drop {name} from slot {slot} (0x7034 op 7)");
        return;
    };
    info!("drop: sending {name} from slot {slot} (0x7034 op 7, UNVERIFIED opcode)");
    let request = InventoryOperationRequest::Drop { slot };
    if let Err(e) = connection.get_sender().send(Packet::from(request).into()) {
        error!("drop: failed to send: {}", e.0);
    }
}

/// Cancel an open prompt on Escape, like every other transient the inventory
/// holds (`use_on_item::cancel_armed_item`) — and claim the press, so the same
/// Escape does not also close a window behind the dialog it just dismissed.
pub fn cancel_drop_confirm(
    keys: Res<ButtonInput<KeyCode>>,
    mut confirm: ResMut<DropConfirm>,
    mut consumed: ResMut<crate::plugins::hud::focus::EscConsumed>,
) {
    if !keys.just_pressed(KeyCode::Escape) || confirm.prompt.is_none() {
        return;
    }
    if !consumed.claim() {
        return;
    }
    confirm.prompt = None;
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::assets::textdata::itemdata::ItemData;
    use packets::agent::character_data::{InventoryItem, ItemTypeData, RentInfo};
    use std::collections::HashMap;

    const SWORD_REF: u32 = 42;

    fn app_with_carry(carry: Option<u8>) -> App {
        let mut app = App::new();
        let mut slots = vec![None; 45];
        slots[13] = Some(InventoryItem {
            slot: 13,
            rent: RentInfo::default(),
            ref_id: SWORD_REF,
            data: ItemTypeData::Expendable {
                stack_count: 1,
                assimilation_prob: None,
                mag_params: vec![],
            },
        });
        app.add_message::<DroppedOnNothing>()
            .insert_resource(InventoryState {
                open: true,
                drag: carry,
                ..default()
            })
            .insert_resource(crate::plugins::textdata::ClientItemData::from_data(
                ItemData(HashMap::new()),
            ))
            .init_resource::<ClientTextNames>()
            .init_resource::<DropConfirm>()
            .add_systems(Update, on_ground_drop_release);
        app.world_mut().spawn((
            Player,
            Inventory {
                slots,
                avatar_slots: vec![None; 5],
                gold: 0,
            },
        ));
        app
    }

    fn release(app: &mut App) {
        app.world_mut().write_message(DroppedOnNothing);
        app.update();
    }

    /// Raising the question must also take the item off the cursor. The ghost
    /// used to follow the pointer across the modal, so the player was still
    /// visibly holding the item while being asked whether to throw it away.
    #[test]
    fn raising_the_question_ends_the_carry() {
        let mut app = app_with_carry(Some(13));
        // a ghost, as `begin_carry` would have spawned
        let ghost = app.world_mut().spawn(super::super::ui::DragGhost).id();
        release(&mut app);

        assert!(
            app.world().resource::<DropConfirm>().prompt.is_some(),
            "the question should have been raised"
        );
        assert_eq!(
            app.world().resource::<InventoryState>().drag,
            None,
            "the carry must end when the dialog opens"
        );
        assert!(
            app.world().get_entity(ghost).is_err(),
            "the drag ghost must be despawned with the carry"
        );
    }

    /// A carry whose item vanished still has to release the cursor, even though
    /// there is no question to ask — otherwise the ghost outlives everything.
    #[test]
    fn a_vanished_item_still_releases_the_cursor() {
        let mut app = app_with_carry(Some(20));
        let ghost = app.world_mut().spawn(super::super::ui::DragGhost).id();
        release(&mut app);
        assert!(app.world().resource::<DropConfirm>().prompt.is_none());
        assert_eq!(app.world().resource::<InventoryState>().drag, None);
        assert!(app.world().get_entity(ghost).is_err());
    }

    /// The whole point: a carried item released on nothing asks first.
    /// Nothing reaches the wire from this system.
    #[test]
    fn a_release_on_nothing_while_carrying_raises_the_question() {
        let mut app = app_with_carry(Some(13));
        release(&mut app);
        let confirm = app.world().resource::<DropConfirm>();
        let prompt = confirm.prompt.as_ref().expect("no prompt was raised");
        assert_eq!(prompt.slot, 13);
        // no itemdata row in this fixture, so the name falls back to the ref id
        // rather than rendering an empty line
        assert!(prompt.name.contains("42"), "{}", prompt.name);
    }

    /// A ground release with nothing in hand is an ordinary click. It must not
    /// raise a dialog — that would put a modal in front of every step the
    /// player takes.
    #[test]
    fn a_release_with_no_carry_asks_nothing() {
        let mut app = app_with_carry(None);
        release(&mut app);
        assert!(app.world().resource::<DropConfirm>().prompt.is_none());
    }

    /// A carry whose slot has since been emptied (a server push) has nothing to
    /// ask about, and must not raise a prompt naming an item that is gone.
    #[test]
    fn a_carry_of_a_vanished_item_asks_nothing() {
        let mut app = app_with_carry(Some(20));
        release(&mut app);
        assert!(app.world().resource::<DropConfirm>().prompt.is_none());
    }

    /// Only one question at a time: a second release while the dialog is up
    /// must not replace the prompt the player is currently reading.
    #[test]
    fn a_second_release_does_not_rewrite_the_open_question() {
        let mut app = app_with_carry(Some(13));
        release(&mut app);
        let first = app
            .world()
            .resource::<DropConfirm>()
            .prompt
            .as_ref()
            .map(|p| p.slot);
        app.world_mut().resource_mut::<InventoryState>().drag = Some(14);
        release(&mut app);
        assert_eq!(
            app.world()
                .resource::<DropConfirm>()
                .prompt
                .as_ref()
                .map(|p| p.slot),
            first
        );
    }

    /// The interior fill is derived from the plate, never typed in.
    ///
    /// `spawn_modal_frame` draws the 8-piece ring only — the `msgbox2_window_`
    /// family has no centre piece — so the dialog's middle is whatever the
    /// caller puts there, and for a while it was nothing at all: the scrim
    /// showed straight through. Deriving the rect means a change to `PLATE`
    /// moves the fill with it instead of leaving a gap at one edge.
    #[test]
    fn the_interior_fill_covers_the_plate_inside_its_frame() {
        let (x, y, w, h) = modal_interior(PLATE.0, PLATE.1);
        assert_eq!((x, y, w, h), (16.0, 40.0, 268.0, 94.0));
        // it reaches the frame on every side
        assert_eq!(x + w, PLATE.0 - 16.0);
        assert_eq!(y + h, PLATE.1 - 16.0);
        // and the content sits inside it: the warning, the name and the buttons
        for (cy, ch) in [(48.0, 34.0), (86.0, 16.0), (110.0, BUTTON.1)] {
            assert!(cy >= y && cy + ch <= y + h, "{cy}+{ch} escapes the fill");
        }
    }
}
