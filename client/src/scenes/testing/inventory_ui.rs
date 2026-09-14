//! Idea: an offline preview of the inventory window with a mock inventory —
//! a +5 weapon and full heavy-armor set on the equipment slots, stacked
//! potions across two bag pages, and enough gold to exercise the thousands
//! grouping. Run with `SCENE=ui_testing`. The inventory's own Update systems
//! already run in `SceneState::UiTesting`, so this only spawns the window, a
//! stand-in player entity carrying the mock `Inventory` component, and opens
//! it. Ref ids are vanilla itemdata rows (ITEM_CH_* / ITEM_ETC_*), so icons,
//! names and tooltip stats are real.

use bevy::prelude::*;

use packets::agent::character_data::{EquipmentData, InventoryItem, ItemTypeData, RentInfo};

use crate::plugins::hud::inventory::model::InventoryState;
use crate::plugins::hud::inventory::ui::spawn_inventory_window;
use crate::plugins::net::inventory::Inventory;
use crate::plugins::player::Player;
use crate::scenes::SceneState;

pub struct InventoryUiPreviewPlugin;

impl Plugin for InventoryUiPreviewPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            OnEnter(SceneState::UiTesting),
            (spawn_inventory_window, seed_mock_inventory).chain(),
        );
    }
}

// vanilla itemdata ref ids (itemdata_5000.txt)
const CH_SPEAR: u32 = 143; // ITEM_CH_SPEAR_01_A
const CH_SHIELD: u32 = 251; // ITEM_CH_SHIELD_01_A
const CH_HELM: u32 = 287; // ITEM_CH_M_HEAVY_01_HA_A
const CH_SHOULDER: u32 = 359; // ITEM_CH_M_HEAVY_01_SA_A
const CH_CHEST: u32 = 395; // ITEM_CH_M_HEAVY_01_BA_A
const CH_PANTS: u32 = 431; // ITEM_CH_M_HEAVY_01_LA_A
const CH_GAUNTLET: u32 = 467; // ITEM_CH_M_HEAVY_01_AA_A
const CH_BOOTS: u32 = 503; // ITEM_CH_M_HEAVY_01_FA_A
const CH_RING: u32 = 1799; // ITEM_CH_RING_01_A
const HP_POTION: u32 = 4; // ITEM_ETC_HP_POTION_01
const MP_POTION: u32 = 11; // ITEM_ETC_MP_POTION_01

fn seed_mock_inventory(mut state: ResMut<InventoryState>, mut commands: Commands) {
    let equipment = |slot: u8, ref_id: u32, opt_level: u8| InventoryItem {
        slot,
        rent: RentInfo::default(),
        ref_id,
        data: ItemTypeData::Equipment(EquipmentData {
            opt_level,
            // a mid-high roll on every white stat slot
            variance: 0x1084_2108_4210_8421 & 0x03FF_FFFF_FFFF,
            durability: 46,
            mag_params: vec![],
            socket_tag: 1,
            sockets: vec![],
            elixir_tag: 2,
            adv_elixirs: vec![],
        }),
    };
    let stack = |slot: u8, ref_id: u32, count: u16| InventoryItem {
        slot,
        rent: RentInfo::default(),
        ref_id,
        data: ItemTypeData::Expendable {
            stack_count: count,
            assimilation_prob: None,
            mag_params: vec![],
        },
    };

    let mut slots: Vec<Option<InventoryItem>> = vec![None; 77]; // 64 bag slots -> two pages
    for item in [
        // equipment slots (wire 0..12)
        equipment(0, CH_HELM, 0),
        equipment(1, CH_CHEST, 3),
        equipment(2, CH_SHOULDER, 0),
        equipment(3, CH_GAUNTLET, 0),
        equipment(4, CH_PANTS, 0),
        equipment(5, CH_BOOTS, 0),
        equipment(6, CH_SPEAR, 5),
        equipment(7, CH_SHIELD, 0),
        equipment(11, CH_RING, 0),
        // bag page 1 (wire 13..44)
        stack(13, HP_POTION, 50),
        stack(14, MP_POTION, 50),
        equipment(16, CH_SPEAR, 7),
        stack(20, HP_POTION, 1),
        // bag page 2 (wire 45..76)
        stack(45, MP_POTION, 250),
        equipment(50, CH_SHIELD, 1),
    ] {
        let slot = item.slot as usize;
        slots[slot] = Some(item);
    }

    // a stand-in player entity: the HUD reads Inventory off the Player, and
    // gameplay systems are not active in UiTesting
    commands.spawn((
        Player,
        Name::from("Preview Player"),
        Inventory {
            slots,
            avatar_slots: vec![None; 5],
            gold: 123_456_789,
        },
    ));
    state.open = true;
}
