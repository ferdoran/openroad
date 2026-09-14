//! Using one inventory item **on another** — the arm → click → confirm flow.
//!
//! Idea: a few item classes do not act on the user or on a world entity but on
//! a second *inventory item*, and their `0x704C` body carries that item's slot
//! (`ItemUseRequest::WithSlot`). The pet-revival item is the one that matters
//! today: a dead pet has no entity to click, so "revive the pet" means "use the
//! Grass of life on the pet's summon scroll".
//!
//! Vanilla's interaction for this is a cursor mode — right-click the item, the
//! cursor changes, the next click picks the target. This repo already has that
//! shape working once, in `hud::store`'s `RepairMode`, whose own comment admits
//! it is "vanilla's hammer-cursor mode, minus the cursor art for now". This
//! module is the same pattern with the cursor half added, and the two live
//! side by side in `on_slot_press`.
//!
//! ⚠️ The wire tail is `[U]` — see [`ItemUseRequest::WithSlot`]. Everything
//! here is arranged so that a mis-click cannot reach the wire: only a dead
//! pet's scroll is an accepted target, and the confirm is a second gate.

use bevy::prelude::*;

use crate::plugins::hud::chat::model::{ChatHistory, ChatLine};
use crate::plugins::hud::underbar::cast::UseItemRequest;
use crate::plugins::net::inventory::Inventory;
use crate::plugins::player::Player;
use crate::plugins::textdata::{ClientItemData, ClientTextNames};

/// An item is armed and waiting for its target slot.
///
/// Deliberately not folded into `InventoryState`: this outlives a single
/// window interaction (the cursor stays armed until the player clicks or
/// cancels) and the cursor plugin reads it, so it is its own resource.
#[derive(Resource, Default)]
pub struct PendingItemUse(pub Option<ArmedItem>);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArmedItem {
    /// Where the item being *used* lives.
    pub source_slot: u8,
    pub ref_id: u32,
}

impl PendingItemUse {
    pub fn is_armed(&self) -> bool {
        self.0.is_some()
    }
}

/// A target has been picked and the player is being asked to confirm.
#[derive(Resource, Default)]
pub struct UseOnItemConfirm {
    pub prompt: Option<UseOnItemPrompt>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UseOnItemPrompt {
    pub armed: ArmedItem,
    /// The slot the item will be used on.
    pub target_slot: u8,
    pub message: String,
}

/// Whether `slot` is a legal target for the armed item.
///
/// Today the only armed class we build is the COS revive, whose target must be
/// a summon scroll **whose pet is dead** — the state the inventory already
/// tints and labels. Refusing anything else is what keeps a stray click off a
/// wire whose tail shape is still unverified.
pub fn is_valid_target(
    armed: &ArmedItem,
    target_slot: u8,
    inventory: &Inventory,
    item_data: &ClientItemData,
) -> bool {
    let Some(target) = inventory.get(target_slot) else {
        return false;
    };
    // Using an item on itself is never meaningful.
    if target_slot == armed.source_slot {
        return false;
    }
    let is_revive = item_data
        .get(&(armed.ref_id as i32))
        .is_some_and(|row| row.is_cos_revive());
    if is_revive {
        return target.data.cos_is_dead();
    }
    // The other `needs_target_slot` classes (rent extension, reinforce, …) are
    // not built yet; until one is, nothing else arms, so nothing else targets.
    false
}

/// Consume a click on `target_slot` while an item is armed.
///
/// Returns `true` when the click was consumed — the caller must then not treat
/// it as the start of a drag. Always disarms: an armed cursor that survives a
/// bad click is worse than one that makes you right-click again.
#[allow(clippy::too_many_arguments)]
pub fn take_armed_click(
    target_slot: u8,
    pending: &mut PendingItemUse,
    confirm: &mut UseOnItemConfirm,
    inventories: &Query<&Inventory, With<Player>>,
    item_data: &ClientItemData,
    names: &ClientTextNames,
    history: &mut ChatHistory,
) -> bool {
    let Some(armed) = pending.0.take() else {
        return false;
    };
    let Ok(inventory) = inventories.single() else {
        return true;
    };
    let name_of = |slot: u8| -> String {
        inventory
            .get(slot)
            .and_then(|item| item_data.get(&(item.ref_id as i32)))
            .and_then(|row| row.name_key())
            .and_then(|key| names.name(key))
            .unwrap_or("that item")
            .to_string()
    };
    if !is_valid_target(&armed, target_slot, inventory, item_data) {
        // Say why rather than swallowing the click: an armed cursor that does
        // nothing is the "I clicked and nothing happened" report again.
        history.push(ChatLine::system(format!(
            "{} cannot be used on {}.",
            name_of(armed.source_slot),
            name_of(target_slot),
        )));
        return true;
    }
    confirm.prompt = Some(UseOnItemPrompt {
        armed,
        target_slot,
        message: format!(
            "Use {} on {}?",
            name_of(armed.source_slot),
            name_of(target_slot)
        ),
    });
    true
}

/// Root of the confirmation modal, so a prompt change can tear it down.
#[derive(Component)]
pub struct UseOnItemConfirmRoot;

#[derive(Component, Clone, Copy)]
enum UseOnItemButton {
    Ok,
    Cancel,
}

/// Rebuild the confirmation on prompt changes — the same scrim + panel chrome
/// the repair confirm uses (`hud::store::ui::sync_repair_confirm`), so the two
/// modals do not drift apart visually.
pub fn sync_use_on_item_confirm(
    confirm: Res<UseOnItemConfirm>,
    existing: Query<Entity, With<UseOnItemConfirmRoot>>,
    ui_strings: Res<crate::plugins::textdata::ClientUiStrings>,
    fonts: Res<crate::assets::FontAssets>,
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
    let s = crate::plugins::hud::scale::hud_scale();
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
    commands
        .spawn((
            UseOnItemConfirmRoot,
            Name::from("Use-on-item Confirm"),
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.6)),
            GlobalZIndex(66),
            bevy::ui::UiTargetCamera(camera),
        ))
        .with_children(|scrim| {
            scrim
                .spawn((
                    Node {
                        width: Val::Px(240.0 * s),
                        height: Val::Px(96.0 * s),
                        ..default()
                    },
                    BackgroundColor(Color::srgb(0.09, 0.08, 0.06)),
                    Outline {
                        width: Val::Px(1.0),
                        color: Color::srgb(0.55, 0.45, 0.25),
                        ..default()
                    },
                ))
                .with_children(|panel| {
                    panel.spawn((
                        Text::new(prompt.message.clone()),
                        text_font(8.0),
                        TextColor(Color::srgb(1.0, 0.85, 0.32)),
                        TextLayout::justify(Justify::Center),
                        crate::plugins::hud::game_window::abs_node((10.0, 12.0, 220.0, 34.0), s),
                        Pickable::IGNORE,
                    ));
                    for (button, key, fallback, x) in [
                        (UseOnItemButton::Ok, "UIIT_CTL_CONFIRM", "Confirm", 35.0),
                        (UseOnItemButton::Cancel, "UIIT_CTL_CANCEL", "Cancel", 125.0),
                    ] {
                        panel
                            .spawn((
                                button,
                                bevy::ui_widgets::Button,
                                bevy::picking::hover::Hovered::default(),
                                crate::plugins::hud::game_window::abs_node(
                                    (x, 54.0, 80.0, 24.0),
                                    s,
                                ),
                                ImageNode {
                                    image: button_style.normal.clone(),
                                    image_mode: NodeImageMode::Stretch,
                                    ..default()
                                },
                                button_style.clone(),
                            ))
                            .observe(on_use_on_item_button)
                            .with_children(|b| {
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
        });
}

/// Confirm sends the 0x704C `WithSlot` body; Cancel just closes.
///
/// This is the only place the `[U]` tail reaches the wire, which is why the
/// target was already validated when the prompt was built — by the time the
/// player sees this box, the only remaining question is whether they meant it.
fn on_use_on_item_button(
    activate: On<bevy::ui_widgets::Activate>,
    buttons: Query<&UseOnItemButton>,
    mut confirm: ResMut<UseOnItemConfirm>,
    mut uses: MessageWriter<UseItemRequest>,
) {
    let Ok(button) = buttons.get(activate.entity) else {
        return;
    };
    let Some(prompt) = confirm.prompt.take() else {
        return;
    };
    if matches!(button, UseOnItemButton::Cancel) {
        return;
    }
    uses.write(UseItemRequest {
        ref_id: prompt.armed.ref_id,
        slot: Some(prompt.armed.source_slot),
        target_slot: Some(prompt.target_slot),
    });
}

/// Cancel an armed item (Escape, or closing the window).
///
/// Claims the press when it actually cancelled something, so the same Escape
/// does not also close a window and open the Esc menu behind it — see
/// [`EscConsumed`](crate::plugins::hud::focus::EscConsumed).
pub fn cancel_armed_item(
    keys: Res<ButtonInput<KeyCode>>,
    mut pending: ResMut<PendingItemUse>,
    mut confirm: ResMut<UseOnItemConfirm>,
    mut consumed: ResMut<crate::plugins::hud::focus::EscConsumed>,
) {
    if !keys.just_pressed(KeyCode::Escape) {
        return;
    }
    // Nothing armed and no prompt open: this press is not ours.
    if pending.0.is_none() && confirm.prompt.is_none() {
        return;
    }
    if !consumed.claim() {
        return;
    }
    pending.0 = None;
    confirm.prompt = None;
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::assets::textdata::itemdata::{ItemData, ItemDataRow};
    use packets::agent::character_data::InventoryItem;
    use packets::agent::character_data::{
        ItemTypeData, RentInfo, COS_STATE_DEAD, COS_STATE_SUMMONED,
    };
    use std::collections::HashMap;

    const GRASS_REF: i32 = 7552;
    const SCROLL_REF: i32 = 100;

    fn row(type_ids: (u32, u32, u32, u32)) -> ItemDataRow {
        let mut fields = vec![String::new(); 60];
        fields[9] = type_ids.0.to_string();
        fields[10] = type_ids.1.to_string();
        fields[11] = type_ids.2.to_string();
        fields[12] = type_ids.3.to_string();
        ItemDataRow(fields)
    }

    fn item_data() -> ClientItemData {
        ClientItemData::from_data(ItemData(HashMap::from([
            // Grass of life — the COS revive class.
            (GRASS_REF, row((3, 3, 1, 6))),
            (SCROLL_REF, row((3, 2, 1, 1))),
        ])))
    }

    fn pet_scroll(slot: u8, state: u8) -> InventoryItem {
        InventoryItem {
            slot,
            rent: RentInfo::default(),
            ref_id: SCROLL_REF as u32,
            data: ItemTypeData::CosPet {
                state,
                cos_ref_id: None,
                name: None,
                rent_seconds: None,
                unk: None,
            },
        }
    }

    fn inventory_with(items: Vec<InventoryItem>) -> Inventory {
        let mut slots = vec![None; 45];
        for item in items {
            let slot = item.slot as usize;
            slots[slot] = Some(item);
        }
        Inventory {
            slots,
            ..Default::default()
        }
    }

    /// The guard that keeps a mis-click off a wire whose tail is unverified:
    /// only a **dead** pet's scroll is a legal revive target.
    #[test]
    fn only_a_dead_pets_scroll_is_a_revive_target() {
        let armed = ArmedItem {
            source_slot: 13,
            ref_id: GRASS_REF as u32,
        };
        let data = item_data();

        let dead = inventory_with(vec![pet_scroll(14, COS_STATE_DEAD)]);
        assert!(is_valid_target(&armed, 14, &dead, &data));

        // A living pet's scroll is not a revive target.
        let alive = inventory_with(vec![pet_scroll(14, COS_STATE_SUMMONED)]);
        assert!(!is_valid_target(&armed, 14, &alive, &data));

        // Nor is an empty slot, nor the grass itself.
        assert!(!is_valid_target(&armed, 20, &dead, &data));
        assert!(!is_valid_target(&armed, 13, &dead, &data));
    }
}
