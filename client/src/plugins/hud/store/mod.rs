pub mod model;
pub mod ui;

use bevy::prelude::*;

/// Self-registration for the NPC store + repair window (#558). The HUD registry holds one line per
/// window, so two windows landing in the same lap no longer collide on it.
pub struct StorePlugin;

impl Plugin for StorePlugin {
    fn build(&self, app: &mut App) {
        use crate::scenes::SceneState;

        app.init_resource::<model::StoreState>()
            .init_resource::<model::PendingStoreOp>()
            .init_resource::<model::RepairMode>()
            .init_resource::<model::PendingRepair>()
            .init_resource::<model::RepairConfirm>()
            .init_resource::<model::StoreHoveredGood>()
            .init_resource::<ui::QuantityModal>()
            .init_resource::<ui::ModalAmount>()
            .init_resource::<ui::StoreCarry>()
            .add_systems(OnExit(SceneState::GameWorld), ui::cleanup_store)
            .add_systems(PostUpdate, ui::despawn_closing_store)
            .add_systems(
                Update,
                (
                    model::open_store,
                    model::close_store_with_dialog,
                    model::on_store_response,
                    model::on_repair_response,
                    model::clear_repair_mode_with_store,
                    ui::sync_store_window,
                    ui::update_store_detail,
                    ui::track_store_hover,
                    ui::refresh_store_gold,
                    ui::sell_drop_on_store,
                    ui::update_store_ghost,
                    ui::finish_store_carry,
                    ui::sync_quantity_modal,
                    ui::sync_modal_amount,
                    ui::clear_modal_with_store,
                    ui::sync_repair_confirm,
                )
                    .run_if(super::hud_scenes),
            );
    }
}
