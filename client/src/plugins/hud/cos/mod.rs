pub mod info;
pub mod inventory;
pub mod model;
pub mod setup;
pub mod state;
pub mod ui;

use bevy::prelude::*;

/// Self-registration for the COS (pet) window (#558). The HUD registry holds one line per
/// window, so two windows landing in the same lap no longer collide on it.
pub struct CosPlugin;

impl Plugin for CosPlugin {
    fn build(&self, app: &mut App) {
        use crate::scenes::SceneState;

        app.init_resource::<model::CosWindowState>()
            .init_resource::<state::CosState>()
            .init_resource::<setup::CosSetupState>()
            .init_resource::<inventory::CosBagPage>()
            .add_systems(OnEnter(SceneState::GameWorld), ui::spawn_cos_window)
            .add_systems(OnExit(SceneState::GameWorld), ui::cleanup_cos_window)
            .add_systems(
                Update,
                (
                    model::toggle_cos_window
                        .run_if(not(crate::plugins::settings::keymap::text_field_focused)),
                    ui::apply_cos_visibility,
                    // 0x30C8/0x30C9 -> CosState -> the info page's statics
                    state::on_pet_data,
                    state::on_pet_update,
                    // 0x30C9 arm 2 (whole-bag refresh) needs the itemdata
                    // resolver, so it is its own system
                    state::apply_cos_bag_refresh,
                    // 0xB034 pick-pet bag ops -> the COS bag + the inventory
                    state::apply_cos_bag_ops,
                    info::apply_cos_info,
                    // ifcosinventory.txt page: the pick pet's bag grid
                    inventory::refresh_cos_inventory,
                    // ifcossetup.txt page: 0x30C8/0xB420 -> the flag controls
                    setup::apply_cos_settings,
                    setup::refresh_cos_setup,
                )
                    .run_if(
                        in_state(SceneState::GameWorld).or_else(in_state(SceneState::UiTesting)),
                    ),
            );
    }
}
