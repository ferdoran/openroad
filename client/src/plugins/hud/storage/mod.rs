pub mod model;
pub mod ui;

use bevy::prelude::*;

/// Self-registration for the storage (warehouse) window (#558). The HUD registry holds one line per
/// window, so two windows landing in the same lap no longer collide on it.
pub struct StoragePlugin;

impl Plugin for StoragePlugin {
    fn build(&self, app: &mut App) {
        use crate::scenes::SceneState;

        app.init_resource::<model::StorageState>()
            .init_resource::<model::StorageDataBuffer>()
            .init_resource::<model::StorageHoveredItem>()
            .init_resource::<model::PendingStorageOp>()
            .init_resource::<ui::StorageCarry>()
            .init_resource::<ui::GoldModal>()
            .init_resource::<ui::GoldAmount>()
            .add_systems(OnExit(SceneState::GameWorld), ui::cleanup_storage)
            .add_systems(PostUpdate, ui::despawn_closing_storage)
            .add_systems(
                Update,
                (
                    model::open_storage,
                    model::close_storage_with_dialog,
                    model::on_storage_data_ack,
                    // chained: begin/chunk/end usually land in ONE frame, and
                    // an unordered tuple let `end` parse before `chunk` had
                    // filled the buffer (playtest: a real 0x3049 decoded as
                    // "0 bytes")
                    (
                        model::on_storage_begin,
                        model::on_storage_chunk,
                        model::on_storage_end,
                    )
                        .chain(),
                    model::on_storage_response,
                    ui::sync_storage_window,
                    ui::deposit_drop_on_storage,
                    ui::finish_storage_carry,
                    ui::update_storage_ghost,
                    ui::sync_gold_modal,
                    ui::sync_gold_amount,
                    ui::clear_modal_with_storage,
                    ui::track_storage_hover,
                )
                    .run_if(super::hud_scenes),
            );
    }
}
