use bevy::prelude::*;
use bevy_inspector_egui::bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use bevy_inspector_egui::egui;

use crate::plugins::player::{spawn_player_character, Player, PlayerConfig, RACES};
use crate::scenes::SceneState;

/// A debug window for configuring the playable character (race, gender, armor
/// set and weapon) and respawning it live in the world. Mirrors the other dev
/// egui windows (see `lighting.rs` / `render_debug.rs`).
pub struct PlayerConfigPlugin;

impl Plugin for PlayerConfigPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            // egui UI must build inside the multipass context pass —
            // in Update it would render but never receive input
            EguiPrimaryContextPass,
            player_config_window
                .run_if(in_state(SceneState::WorldSandbox))
                .run_if(super::dev_windows_visible),
        );
    }
}

fn player_config_window(
    mut contexts: EguiContexts,
    mut config: ResMut<PlayerConfig>,
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    player_query: Query<(Entity, &Transform), With<Player>>,
) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };

    egui::Window::new("Player Config")
        .default_open(false)
        .show(ctx, |ui| {
            let before = *config;

            egui::ComboBox::from_label("Race")
                .selected_text(config.race().label)
                .show_ui(ui, |ui| {
                    for (i, race) in RACES.iter().enumerate() {
                        ui.selectable_value(&mut config.race, i, race.label);
                    }
                });

            // A race switch invalidates the armor/weapon indices (each race has
            // its own lists), so re-clamp before the dependent combo boxes read
            // them.
            config.clamp();
            let race = config.race();

            egui::ComboBox::from_label("Gender")
                .selected_text(if config.female { "Woman" } else { "Man" })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut config.female, false, "Man");
                    ui.selectable_value(&mut config.female, true, "Woman");
                });

            egui::ComboBox::from_label("Armor")
                .selected_text(config.armor().label)
                .show_ui(ui, |ui| {
                    for (i, armor) in race.armor.iter().enumerate() {
                        ui.selectable_value(&mut config.armor, i, armor.label);
                    }
                });

            egui::ComboBox::from_label("Weapon")
                .selected_text(config.weapon().label)
                .show_ui(ui, |ui| {
                    for (i, weapon) in race.weapons.iter().enumerate() {
                        ui.selectable_value(&mut config.weapon, i, weapon.label);
                    }
                });

            let dirty = before.race != config.race
                || before.female != config.female
                || before.armor != config.armor
                || before.weapon != config.weapon;

            ui.separator();

            let respawn = ui.button("Apply & Respawn").clicked();
            if dirty {
                ui.label(egui::RichText::new("unapplied changes").italics().weak());
            }

            if respawn {
                // Preserve the character's current position; despawning the
                // Player parent recursively removes its body, armor and weapon
                // children (Bevy's default despawn), then we rebuild from the
                // edited config in place.
                let transform = player_query.single().map(|(_, t)| *t).unwrap_or_default();
                for (entity, _) in player_query.iter() {
                    commands.entity(entity).despawn();
                }
                spawn_player_character(&mut commands, &asset_server, &config, transform);
            }
        });
}
