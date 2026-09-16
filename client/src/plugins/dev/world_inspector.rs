use bevy::prelude::*;
use bevy_inspector_egui::bevy_egui::{EguiContext, EguiPrimaryContextPass, PrimaryEguiContext};
use bevy_inspector_egui::{bevy_inspector, egui, DefaultInspectorConfigPlugin};

/// Replacement for `bevy_inspector_egui::quick::WorldInspectorPlugin`.
///
/// The stock plugin walks the reflection of every entity/component in the
/// world on *every* frame it is shown, which is what costs the double-digit
/// FPS noted where this is wired up (`main.rs`). Reflecting the world is only
/// ever useful right after something changed, so this variant does that walk
/// on demand — when the panel's "Refresh" button is pressed — instead of
/// unconditionally each frame.
pub struct ManualWorldInspectorPlugin;

impl Plugin for ManualWorldInspectorPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<DefaultInspectorConfigPlugin>() {
            app.add_plugins(DefaultInspectorConfigPlugin);
        }

        app.add_systems(
            EguiPrimaryContextPass,
            world_inspector_ui.run_if(super::dev_windows_visible),
        );
    }
}

/// The (expensive) `ui_for_world` reflection walk only starts once the
/// "Refresh" button has been clicked — before that, the panel just shows a
/// placeholder and costs nothing. This trades the old "always live" view for
/// an explicit opt-in per session; toggle the dev windows off (the corner
/// "dev" button) to stop paying for it again once you're done inspecting.
fn world_inspector_ui(world: &mut World, mut refresh_clicked: Local<bool>) {
    let egui_context = world
        .query_filtered::<&mut EguiContext, With<PrimaryEguiContext>>()
        .single(world);

    let Ok(egui_context) = egui_context else {
        return;
    };
    let mut egui_context = egui_context.clone();

    egui::Window::new("World Inspector")
        .default_size((320., 160.))
        .show(egui_context.get_mut(), |ui| {
            if ui.button("Refresh").clicked() {
                *refresh_clicked = true;
            }
            ui.separator();

            if *refresh_clicked {
                egui::ScrollArea::both().show(ui, |ui| {
                    bevy_inspector::ui_for_world(world, ui);
                    ui.allocate_space(ui.available_size());
                });
            } else {
                ui.label("Click Refresh to inspect the current world state.");
            }
        });
}
