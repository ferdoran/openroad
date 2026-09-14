//! The underbar: SRO's bottom-of-screen bar with the EXP/SP gauges, the
//! quickslot skill bar (4 pages x 10 slots + the special "M" slot), page
//! up/down arrows and the menu/community/option/item-mall button cluster.
//!
//! Split like the inventory window: `model.rs` holds the state resources and
//! packet/seeding systems, `ui.rs` the layout + interactions + refresh, and
//! `cast.rs` the experimental skill-cast send path.

pub mod cast;
pub mod menu_popup;
pub mod model;
pub mod ui;

use bevy::prelude::*;

/// Self-registration for the underbar (#558). The HUD registry holds one line per
/// window, so two windows landing in the same lap no longer collide on it.
pub struct UnderbarPlugin;

impl Plugin for UnderbarPlugin {
    fn build(&self, app: &mut App) {
        use crate::scenes::SceneState;

        app.init_resource::<model::QuickSlots>()
            .init_resource::<model::PlayerProgress>()
            // Both item-use systems take it as `ResMut`, and in Bevy 0.19 a
            // missing resource is a system-param validation failure whose
            // default handler panics — it killed the client on world entry
            // until it was registered here (#712 follow-up).
            .init_resource::<cast::ItemUseGate>()
            .add_message::<cast::UseItemRequest>()
            .add_systems(OnEnter(SceneState::GameWorld), ui::spawn_underbar)
            .add_systems(OnExit(SceneState::GameWorld), ui::cleanup_underbar)
            .add_systems(
                Update,
                (
                    model::seed_progress_from_character_info,
                    model::seed_quickslots_from_character_info,
                    model::on_experience_gain,
                    model::on_sp_update,
                    ui::handle_slot_keys,
                    cast::dispatch_item_use,
                    cast::log_item_use_response,
                    cast::play_item_use_effect,
                    // Before the refresh so the frame it disarms is the frame
                    // the ring is hidden, rather than one frame later.
                    ui::fade_armed_slot.before(ui::refresh_underbar),
                    ui::refresh_underbar.run_if(ui::underbar_needs_refresh),
                )
                    .run_if(super::hud_scenes),
            );
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// Every resource this plugin's systems take as `ResMut` must be
    /// registered by the same plugin. `ItemUseGate` was not, and in Bevy 0.19
    /// that is not a silent no-op: system-param validation fails and the
    /// default error handler panics the moment the systems first run — i.e.
    /// on entering the game world, after a real login, which no scene smoke
    /// reaches (#712 follow-up, reported from a live join).
    #[test]
    fn the_plugin_registers_the_resources_its_systems_demand() {
        let mut app = App::new();
        app.add_plugins(UnderbarPlugin);

        assert!(app.world().contains_resource::<model::QuickSlots>());
        assert!(app.world().contains_resource::<model::PlayerProgress>());
        assert!(app.world().contains_resource::<cast::ItemUseGate>());
    }
}
