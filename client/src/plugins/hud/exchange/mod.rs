pub mod model;
pub mod ui;

use bevy::prelude::*;

/// Self-registration for the player-exchange (trade) window (#558). The HUD registry holds one line per
/// window, so two windows landing in the same lap no longer collide on it.
pub struct ExchangePlugin;

impl Plugin for ExchangePlugin {
    fn build(&self, app: &mut App) {
        use crate::scenes::SceneState;

        app.init_resource::<model::ExchangeState>()
            .add_systems(OnExit(SceneState::GameWorld), ui::cleanup_exchange)
            .add_systems(PostUpdate, ui::despawn_closing_exchange)
            // server-driven throughout: it opens on 0x3085 and closes only on
            // an ack/cancel, never on a click (docs/re/systems/exchange.md §3)
            .add_systems(
                Update,
                (
                    model::on_invite_response,
                    model::on_exchange_started,
                    model::on_partner_items,
                    model::on_partner_gold,
                    model::on_partner_confirmed,
                    model::on_confirm_response,
                    model::on_approve_response,
                    model::on_exchange_completed,
                    model::on_exchange_canceled,
                    model::on_exit_response,
                    ui::sync_exchange_window,
                )
                    .run_if(super::hud_scenes),
            );
    }
}
