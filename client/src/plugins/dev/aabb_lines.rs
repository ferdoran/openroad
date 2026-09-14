use bevy::color::Srgba;
// use bevy_prototype_debug_lines::{DebugShapes};
use crate::plugins::cursor::interactions::GameCursorTarget;
use crate::plugins::dev::DevConfig;
use bevy::camera::primitives::Aabb;
use bevy::prelude::{
    ButtonInput, Color, Component, GlobalTransform, KeyCode, Query, Res, ResMut, ViewVisibility,
    With,
};

#[derive(Component)]
pub struct DebugAabb {
    pub color: Srgba,
}

impl DebugAabb {
    pub fn default() -> Self {
        DebugAabb {
            color: bevy::color::palettes::css::RED,
        }
    }
    pub fn with_color(color: Srgba) -> Self {
        DebugAabb { color }
    }
}

pub fn draw_debug_lines_for_aabb(
    mut dev_config: ResMut<DevConfig>,
    keys: Res<ButtonInput<KeyCode>>,
    // mut shapes: ResMut<DebugShapes>,
    aabb_query: Query<
        (
            &Aabb,
            &ViewVisibility,
            Option<&DebugAabb>,
            Option<&GameCursorTarget>,
            &GlobalTransform,
        ),
        With<Aabb>,
    >,
) {
    if keys.just_pressed(KeyCode::KeyE) {
        dev_config.show_aabb = !dev_config.show_aabb;
    }

    if !dev_config.show_aabb {
        return;
    }

    for (_aabb, visibility, debug, target, _transform) in aabb_query.iter() {
        if !visibility.get() {
            continue;
        }

        let is_hovered = match target {
            Some(t) => t.is_hovered(),
            _ => false,
        };
        let _color = if is_hovered {
            Color::WHITE
        } else {
            Color::Srgba(debug.unwrap_or(&DebugAabb::default()).color)
        };
        // TODO: bevy_prototype_debug_lines is not on 0.12 yet
        // shapes
        //     .cuboid()
        //     .position(Vec3::from(aabb.center) + transform.translation())
        //     .size(Vec3::from(aabb.half_extents).mul(2.0))
        //     .color(color);
    }
}
