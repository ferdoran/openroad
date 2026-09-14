use bevy::app::App;
use bevy::asset::Assets;
use bevy::light::DirectionalLight;
use bevy::light::EnvironmentMapLight;
use bevy::light::GlobalAmbientLight;
use bevy::prelude::{
    ButtonInput, IntoScheduleConfigs, KeyCode, Plugin, Query, Res, ResMut, Resource, Transform,
    Update, With,
};
use bevy_inspector_egui::bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use bevy_inspector_egui::egui;

use crate::assets::bmt::sheen::{SheenSettings, SroSheenMaterial};

pub struct LightingPlugin;

impl Plugin for LightingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SheenDebug>()
            .add_systems(
                Update,
                (
                    ambient_adjusting_system,
                    directional_adjusting_system,
                    directional_transform_adjusting_system,
                    env_reflection_adjusting_system,
                    // egui_window
                ),
            )
            .add_systems(
                EguiPrimaryContextPass,
                sheen_debug_window.run_if(super::dev_windows_visible),
            );
    }
}

#[allow(dead_code)]
fn egui_window(
    mut contexts: EguiContexts,
    ambient: Res<GlobalAmbientLight>,
    query: Query<(&DirectionalLight, &Transform)>,
) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    egui::Window::new("Light Debug").show(ctx, |ui| {
        ui.label(format!("Ambient Brightness: {}", ambient.brightness));

        query.iter().enumerate().for_each(|(i, (dl, transform))| {
            ui.label(format!(
                "Directional Light {} illuminance: {}",
                i, dl.illuminance
            ));
            ui.label(format!(
                "Directional Light {} rotation: {}",
                i, transform.rotation
            ));
        });
    });
}

fn ambient_adjusting_system(
    mut ambient: ResMut<GlobalAmbientLight>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    if keys.pressed(KeyCode::KeyE) {
        ambient.brightness += 0.001;
    } else if keys.pressed(KeyCode::KeyR) {
        ambient.brightness -= 0.001;
    }
}

fn directional_adjusting_system(
    mut query: Query<&mut DirectionalLight>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    if keys.pressed(KeyCode::KeyT) {
        query.iter_mut().for_each(|mut dl| {
            dl.illuminance += 100.0;
        });
    } else if keys.pressed(KeyCode::KeyZ) {
        query.iter_mut().for_each(|mut dl| {
            dl.illuminance -= 100.0;
        });
    }
}

/// Live calibration of the sheen material constants (`SheenSettings` in
/// `assets/bmt/sheen.rs`): the sliders overwrite the four base fields on
/// every loaded `SroSheenMaterial` (per-material `alpha_cutout` and the
/// per-instance shine are preserved). Dial in the vanilla look, then bake
/// the values into `SheenSettings::default`.
#[derive(Resource)]
struct SheenDebug {
    env_strength: f32,
    strength: f32,
    shiny_roughness: f32,
    reflectance: f32,
    shine_pow: f32,
    // combined rim alpha (color alpha × strength); writes both material types
    rim_strength: f32,
    rim_power: f32,
    rim_relative: bool,
}

impl Default for SheenDebug {
    fn default() -> Self {
        let defaults = SheenSettings::default();
        // the mobile-parity knobs mirror the config defaults (modern-on),
        // not the struct's faithful sentinels
        let config_defaults = crate::plugins::config::graphics::SheenGraphicsSettings::default();
        let rim_defaults = crate::plugins::config::graphics::RimGraphicsSettings::default();
        Self {
            env_strength: defaults.env_strength,
            strength: defaults.strength,
            shiny_roughness: defaults.shiny_roughness,
            reflectance: defaults.reflectance,
            shine_pow: config_defaults.shine_pow,
            // 0x5A alpha of the default "5AFFFFFF" × strength 1.0
            rim_strength: 0x5A as f32 / 255.0 * rim_defaults.strength,
            rim_power: rim_defaults.power,
            rim_relative: rim_defaults.mode == crate::plugins::config::graphics::RimMode::Relative,
        }
    }
}

fn sheen_debug_window(
    mut contexts: EguiContexts,
    mut debug: ResMut<SheenDebug>,
    mut materials: ResMut<Assets<SroSheenMaterial>>,
    mut rim_materials: ResMut<Assets<crate::assets::bmt::rim::SroRimMaterial>>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return };
    let mut changed = false;
    egui::Window::new("Sheen Debug")
        .default_open(false)
        .show(ctx, |ui| {
            changed |= ui
                .add(
                    egui::Slider::new(&mut debug.env_strength, 0.0..=2.0)
                        .text("env chrome (vanilla 0.5)"),
                )
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut debug.strength, 0.0..=1.0).text("pbr metallic"))
                .changed();
            changed |= ui
                .add(
                    egui::Slider::new(&mut debug.shiny_roughness, 0.0..=1.0).text("mask roughness"),
                )
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut debug.reflectance, 0.0..=1.0).text("mask reflectance"))
                .changed();
            changed |= ui
                .add(
                    egui::Slider::new(&mut debug.shine_pow, 1.0..=16.0)
                        .text("shine streak pow (1 = faithful soft)"),
                )
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut debug.rim_strength, 0.0..=2.0).text("rim strength"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut debug.rim_power, 0.5..=8.0).text("rim power"))
                .changed();
            changed |= ui
                .checkbox(&mut debug.rim_relative, "rim relative (lit x (1 + rim))")
                .changed();
        });
    if changed {
        // iter_mut marks every sheen material changed -> full re-prepare;
        // fine at interaction rate
        for (_, material) in materials.iter_mut() {
            let settings = &mut material.extension.settings;
            settings.env_strength = debug.env_strength;
            settings.strength = debug.strength;
            settings.shiny_roughness = debug.shiny_roughness;
            settings.reflectance = debug.reflectance;
            settings.shine_pow = debug.shine_pow;
            // only touch rims that are on (a > 0), so rim-disabled configs
            // stay rim-free
            if settings.rim_color.w > 0.0 {
                settings.rim_color.w = debug.rim_strength;
                settings.rim_power = debug.rim_power;
                settings.rim_mode = if debug.rim_relative { 1.0 } else { 0.0 };
            }
        }
        // the character-mesh rim materials (also selection clones — those
        // re-clone from their originals on the next hover change)
        for (_, material) in rim_materials.iter_mut() {
            let settings = &mut material.extension.settings;
            if settings.color.w > 0.0 {
                settings.color.w = debug.rim_strength;
                settings.power = debug.rim_power;
                settings.mode = if debug.rim_relative { 1.0 } else { 0.0 };
            }
        }
    }
}

/// Calibration aid for the sky-reflection probe
/// (`environment::reflections::SKY_REFLECTION_INTENSITY`). Targets the baked
/// `EnvironmentMapLight` (intensity is applied at shading time, so it keeps
/// working after `freeze_baked_env_maps` removes the generator); before the
/// bake completes the generator's own `EnvironmentMapLight` is already present.
fn env_reflection_adjusting_system(
    mut query: Query<&mut EnvironmentMapLight>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    if keys.pressed(KeyCode::KeyP) {
        query.iter_mut().for_each(|mut env| {
            env.intensity += 100.0;
        });
    } else if keys.pressed(KeyCode::KeyO) {
        query.iter_mut().for_each(|mut env| {
            env.intensity = (env.intensity - 100.0).max(0.0);
        });
    }
}

fn directional_transform_adjusting_system(
    mut query: Query<&mut Transform, With<DirectionalLight>>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    if keys.pressed(KeyCode::KeyU) {
        query.iter_mut().for_each(|mut dl| {
            dl.rotate_x(0.01);
        });
    } else if keys.pressed(KeyCode::KeyI) {
        query.iter_mut().for_each(|mut dl| {
            dl.rotate_x(-0.01);
        });
    }
}
