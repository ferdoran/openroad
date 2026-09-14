use crate::plugins::cursor::GameCursorCamera;
use crate::AppMode;
use bevy::app::{App, Plugin};
use bevy::camera::primitives::Aabb;
use bevy::prelude::*;

mod enemies;
pub mod entity_select;
pub mod npcs;
mod objects;
mod player;

#[derive(Component)]
#[allow(dead_code)]
pub struct GameCursorTarget {
    pub intersection_at: Option<f32>,
    mesh_collision_enabled: bool,
}

impl Default for GameCursorTarget {
    fn default() -> Self {
        Self {
            intersection_at: None,
            mesh_collision_enabled: false,
        }
    }
}

impl GameCursorTarget {
    pub fn is_hovered(&self) -> bool {
        return self.intersection_at != None;
    }

    #[allow(dead_code)]
    pub fn with_mesh_collision() -> Self {
        Self {
            mesh_collision_enabled: true,
            ..default()
        }
    }
}

pub struct CursorInteractionsPlugin;

impl Plugin for CursorInteractionsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                check_cursor_aabb_intersection,
                handle_cursor_clicks,
                player::check_player_cursor_intersection,
            )
                .chain()
                .run_if(in_state(AppMode::PlayMode).and_then(cursor_interaction_needed)),
        );
    }
}

/// The AABB sweep below is O(loaded terrain blocks + map objects), so it only
/// runs when the cursor ray actually changed — the ray is recomputed from both
/// the cursor position and the camera transform (see `move_cursor`), so this
/// covers mouse movement, camera movement, and hovering over egui (ray goes
/// `None`). Click frames always run so clicks resolve against a fresh sweep;
/// on skipped frames the hover state simply persists.
fn cursor_interaction_needed(
    mut last_ray: Local<Option<(Vec3, Vec3)>>,
    buttons: Res<ButtonInput<MouseButton>>,
    cursor_camera_query: Query<&GameCursorCamera>,
) -> bool {
    let ray = cursor_camera_query
        .iter()
        .find_map(|camera| camera.cursor_ray)
        .map(|ray| (ray.origin, *ray.direction));
    let ray_moved = ray != *last_ray;
    *last_ray = ray;

    ray_moved || buttons.just_pressed(MouseButton::Left) || buttons.just_released(MouseButton::Left)
}

/// Entities farther than this from the ray origin (the camera) are skipped by
/// the picking sweep before any AABB math. Generous on purpose: it only needs
/// to cull the outer ring of streamed regions (a ~4-region radius, thousands
/// of units), not act as a gameplay interaction range.
const MAX_PICK_DISTANCE: f32 = 1_000.0;

fn check_cursor_aabb_intersection(
    mut aabb_query: Query<(&Aabb, &GlobalTransform, &mut GameCursorTarget), With<GameCursorTarget>>,
    cursor_camera_query: Query<&GameCursorCamera>,
) {
    // Several cameras carry a GameCursorCamera (the play camera and the debug
    // fly camera), but `move_cursor` only sets a cursor ray on the active one,
    // so pick that one instead of assuming a single camera (which would panic).
    let ray = cursor_camera_query
        .iter()
        .find_map(|camera| camera.cursor_ray);

    for (aabb, global_transform, mut game_cursor_target) in aabb_query.iter_mut() {
        // Only write when clearing an actual hover: an unconditional write
        // would flag every GameCursorTarget as changed on every sweep.
        if game_cursor_target.intersection_at.is_some() {
            game_cursor_target.intersection_at = None;
        }

        let Some(ray) = ray else {
            continue;
        };

        // Cheap distance cull before the affine decomposition and AABB
        // transform; half_extents pads the radius so large ground blocks
        // whose origin is far but whose surface is near still get tested.
        let translation = global_transform.translation();
        let reach = MAX_PICK_DISTANCE + aabb.half_extents.length();
        if translation.distance_squared(ray.origin) > reach * reach {
            continue;
        }

        let (_scale, rotation, translation) = global_transform.to_scale_rotation_translation();
        let world_aabb = transform_aabb_to_world(&aabb, translation, rotation);

        if let Some(t) = ray_intersects_aabb(&ray, &world_aabb) {
            // collect intersected AABB's
            game_cursor_target.intersection_at = Some(t);
        }
    }
}

fn handle_cursor_clicks(
    buttons: Res<ButtonInput<MouseButton>>,
    mut cursor_target_query: Query<
        (Entity, &mut GameCursorTarget, Option<&Name>),
        With<GameCursorTarget>,
    >,
    _cursor_camera_query: Query<&mut GameCursorCamera, With<GameCursorCamera>>,
) {
    if buttons.just_pressed(MouseButton::Right) {
        // do nothing because of camera rotation
        return;
    }

    if buttons.just_pressed(MouseButton::Middle) {
        // do nothing because of camera zoom
        return;
    }

    // Hoisted out of the loop: skip iterating every pickable entity on
    // frames without a completed left click.
    if !buttons.just_released(MouseButton::Left) {
        return;
    }

    for (_entity, _cursor_target, _name) in cursor_target_query.iter_mut() {

        /* STILL NEEDED

        cursor_target.is_selected = cursor_target.is_hovered;

        if cursor_target.is_selected {
            let n = match name {
                Some(name) => name.clone(),
                _ => Name::from(String::from("<no name>"))
            };
            info!("Selected AABB of {:?}", n);
        }*/
    }
}

pub fn transform_aabb_to_world(aabb: &Aabb, translation: Vec3, rotation: Quat) -> Aabb {
    // Die AABB-Eckpunkte in Objektraumkoordinaten
    let min = Vec3::from(aabb.center - aabb.half_extents);
    let max = Vec3::from(aabb.center + aabb.half_extents);

    // Transformieren Sie die Eckpunkte in Weltkoordinaten
    let min_world = translation + rotation * min;
    let max_world = translation + rotation * max;

    // Erstellen Sie eine neue AABB in Weltkoordinaten
    Aabb::from_min_max(min_world, max_world)
}

pub fn ray_intersects_aabb(ray: &Ray3d, aabb: &Aabb) -> Option<f32> {
    let origin = ray.origin;
    let direction = ray.direction;

    let t1 = (aabb.center.x - aabb.half_extents.x - origin.x) / direction.x;
    let t2 = (aabb.center.x + aabb.half_extents.x - origin.x) / direction.x;
    let t3 = (aabb.center.y - aabb.half_extents.y - origin.y) / direction.y;
    let t4 = (aabb.center.y + aabb.half_extents.y - origin.y) / direction.y;
    let t5 = (aabb.center.z - aabb.half_extents.z - origin.z) / direction.z;
    let t6 = (aabb.center.z + aabb.half_extents.z - origin.z) / direction.z;

    let tmin = t1.min(t2).max(t3.min(t4).max(t5.min(t6)));
    let tmax = t1.max(t2).min(t3.max(t4).min(t5.max(t6)));

    // Check if there's an intersection
    if tmax < 0.0 || tmin > tmax {
        return None;
    }
    Some(tmin) // Intersection point at tmin
}
#[cfg(test)]
mod tests {
    /// This module's source, up to (not including) the test module — so the
    /// literals the test searches for cannot match themselves.
    fn registry() -> &'static str {
        include_str!("mod.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("split always yields a first part")
    }

    /// #56-A: **exactly one click-to-move path.** Click-to-move and its
    /// on-ground marker belong to `plugins::nav::decal`
    /// (`place_decal_on_click` writes the `PlayerMoveOrder`). Until this test
    /// existed, a second, older path was still registered here: a per-triangle
    /// terrain raycast that ran on every left-click release and wrote a
    /// `Transform` onto a component nothing ever spawned. It was invisible
    /// precisely because its output went nowhere, so it survived the nav-decal
    /// rewrite. Pin the absence rather than trusting it.
    ///
    /// The predicate is the module declaration and the path-qualified system,
    /// not the bare word: this module legitimately *talks about* terrain
    /// blocks in a doc comment about the AABB sweep, and a `contains("terrain")`
    /// guard fails on that prose instead of on a real regression.
    #[test]
    fn the_cursor_registry_runs_no_second_terrain_click_path() {
        for forbidden in ["mod terrain", "terrain::"] {
            assert!(
                !registry().contains(forbidden),
                "`{forbidden}`: click-to-move belongs to plugins::nav::decal, \
                 and a terrain click path registered here is the duplicate \
                 #56-A removed"
            );
        }
    }

    /// The removed path printed with `println!`, not the log system, so its
    /// three lines per click bypassed every log filter and landed in the
    /// user's stdout during live play. `println!` in a per-frame or per-click
    /// system is the defect; keep this module clear of it.
    #[test]
    fn the_cursor_registry_logs_through_bevy_not_stdout() {
        assert!(
            !registry().contains("println!"),
            "use info!/debug!/trace! — println! bypasses the log filters and \
             spams the player's stdout"
        );
    }
}
