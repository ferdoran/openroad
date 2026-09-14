//! Live full-body paper-doll for the equipment panel.
//!
//! Idea: instead of pointing a camera at the live player (whose run/attack
//! animations would play in the preview), a display-only CLONE of the
//! character is assembled the same data-driven way as the player and the
//! char-select previews (body `UnloadedResource` + `PendingItemAttachment`
//! children from the equipped items) and parked far below the world, where
//! only the offscreen paper-doll camera (a child of the clone, rendering the
//! PaperDoll layer into a target texture the UI samples) can see it. With no
//! movement input the clone idles in the stand animation — the vanilla look.
//! Its meshes are re-tagged onto the PaperDoll layer as they stream in, so
//! the main camera never renders them. The clone is rebuilt whenever the
//! equipment slots of the player's [`Inventory`] change, and the rotate
//! buttons orbit the camera around it via [`PaperDollYaw`].

use bevy::camera::primitives::Aabb;
use bevy::camera::visibility::RenderLayers;
use bevy::camera::RenderTarget;
use bevy::light::AmbientLight;
use bevy::math::Vec3A;
use bevy::prelude::*;

use crate::plugins::camera::CameraLayers;
use crate::plugins::dynamic_resource_loader::{
    MirroredResource, PendingItemAttachment, PreferredAnimationGroup, UnloadedResource,
};
use crate::plugins::hud::inventory::model::InventoryState;
use crate::plugins::net::character_info::CharacterInfo;
use crate::plugins::net::inventory::{Inventory, EQUIP_SLOT_COUNT};
use crate::plugins::player::Player;
use crate::plugins::textdata::{ClientCharacterData, ClientItemData};
use crate::util::mesh::{mirrored, needs_winding_reversal};

/// Render-target resolution (2x the ~102x300 UI slot, aspect preserved).
pub const RT_W: u32 = 204;
pub const RT_H: u32 = 600;
const FOV: f32 = 0.5;
/// Extra view height around the body box.
const MARGIN: f32 = 1.15;
/// Where the clone is parked, relative to the player at spawn time: far below
/// the world so streaming meshes never flash into the main camera's view.
const CLONE_OFFSET: Vec3 = Vec3::new(0.0, -400.0, 0.0);

/// The paper-doll's render-target image, sampled by the UI view node.
#[derive(Resource)]
pub struct PaperDollTarget(pub Handle<Image>);

/// Camera orbit angle set by the rotate buttons (radians; 0 = front).
#[derive(Resource, Default)]
pub struct PaperDollYaw(pub f32);

impl PaperDollYaw {
    pub const STEP: f32 = std::f32::consts::FRAC_PI_4 / 2.0;
}

/// The UI node showing the render target (in the equipment panel).
#[derive(Component)]
pub struct PaperDollView;

/// The display-only character clone; carries the equipment ref ids it was
/// built with so equipment changes trigger a rebuild.
#[derive(Component)]
pub struct PaperDollClone {
    equipment: Vec<u32>,
}

#[derive(Component)]
pub struct PaperDollCamera;

/// Last framing, kept so yaw changes re-pose without re-measuring.
#[derive(Component, Default)]
pub struct PaperDollAim {
    signature: u64,
    focus: Vec3,
    distance: f32,
}

/// The equipped ref ids (wire slots 0..12) the clone should wear.
fn equipment_refs(inventory: &Inventory) -> Vec<u32> {
    (0..EQUIP_SLOT_COUNT)
        .filter_map(|slot| inventory.get(slot).map(|item| item.ref_id))
        .collect()
}

/// Build (and on equipment changes rebuild) the paper-doll clone: the same
/// assembly recipe as `game_scene::spawn_selected_player`, minus the gameplay
/// markers — body resource from the player's characterdata ref, one pending
/// attachment per equipped item, stand animation by default.
#[allow(clippy::too_many_arguments)]
pub fn maintain_paperdoll_clone(
    target: Option<Res<PaperDollTarget>>,
    players: Query<(&CharacterInfo, &Inventory, &Transform), With<Player>>,
    clones: Query<(Entity, &PaperDollClone)>,
    char_data: Res<ClientCharacterData>,
    item_data: Res<ClientItemData>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
) {
    if target.is_none() {
        return;
    }
    let Ok((info, inventory, player_transform)) = players.single() else {
        return;
    };
    let Some(ref_id) = info.stats.as_ref().map(|stats| stats.ref_id) else {
        return;
    };
    let equipment = equipment_refs(inventory);
    match clones.single() {
        Ok((_, clone)) if clone.equipment == equipment => return,
        Ok((entity, _)) => commands.entity(entity).despawn(),
        Err(_) => {}
    }

    let Some(char_row) = char_data.get(&(ref_id as i32)) else {
        return;
    };
    // Mirrored like the body it echoes (`util::mesh`). Without this the doll
    // renders the opposite handedness to the character standing next to it and
    // to the char-select preview it is modelled on — the equipped weapon shows
    // in the wrong hand.
    let transform = mirrored(Transform::from_translation(
        player_transform.translation + CLONE_OFFSET,
    ));
    let mut clone_cmd = commands.spawn((
        PaperDollClone {
            equipment: equipment.clone(),
        },
        Name::from("Paper Doll Clone"),
        // stand stance fallback; the equipped weapon overrides it below
        PreferredAnimationGroup("sword".to_string()),
        transform,
        Visibility::default(),
    ));
    if needs_winding_reversal(&transform.to_matrix()) {
        clone_cmd.insert(MirroredResource);
    }
    if let Some(path) = char_row.resource_path() {
        clone_cmd.insert(UnloadedResource(asset_server.load(path)));
    }
    let clone = clone_cmd.id();
    for item_ref in &equipment {
        let Some(item_row) = item_data.get(&(*item_ref as i32)) else {
            continue;
        };
        if let Some(group) = item_row.animation_group() {
            commands
                .entity(clone)
                .insert(PreferredAnimationGroup(group.to_string()));
        }
        let Some(item_path) = item_row.resource_path() else {
            continue;
        };
        commands.spawn((
            PendingItemAttachment(asset_server.load(item_path)),
            ChildOf(clone),
            Name::from(format!("doll item {}", item_row.code_name())),
        ));
    }
    // the camera rig hangs off the clone and dies with it on rebuilds
    let camera = commands
        .spawn((
            PaperDollCamera,
            Name::from("Paper Doll Camera"),
            Camera3d::default(),
            Camera {
                clear_color: ClearColorConfig::Custom(Color::NONE),
                order: -2, // before the portrait (-1), main (0) and UI
                is_active: false,
                ..default()
            },
            RenderTarget::from(target.unwrap().0.clone()),
            Projection::Perspective(PerspectiveProjection {
                fov: FOV,
                near: 0.5,
                far: 120.0,
                ..default()
            }),
            RenderLayers::layer(CameraLayers::PaperDoll.into()),
            PaperDollAim::default(),
            Transform::default(),
            AmbientLight {
                color: Color::WHITE,
                brightness: 500.0,
                ..default()
            },
            ChildOf(clone),
        ))
        .id();
    // headlight: identity transform -> shines wherever the camera looks
    commands.spawn((
        DirectionalLight {
            illuminance: 3_000.0,
            ..default()
        },
        RenderLayers::layer(CameraLayers::PaperDoll.into()),
        ChildOf(camera),
    ));
    debug!(
        "inventory: paper-doll clone built ({} equipped items)",
        equipment.len()
    );
}

/// Move every clone mesh onto the PaperDoll layer (exclusively) as it streams
/// in, hiding the clone from the main camera.
pub fn tag_paperdoll_meshes(
    clones: Query<Entity, With<PaperDollClone>>,
    children: Query<&Children>,
    meshes: Query<Option<&RenderLayers>, With<Mesh3d>>,
    mut commands: Commands,
) {
    let doll_layer = RenderLayers::layer(CameraLayers::PaperDoll.into());
    for root in clones.iter() {
        for entity in children.iter_descendants(root) {
            match meshes.get(entity) {
                Ok(Some(layers)) if *layers == doll_layer => {}
                Ok(_) => {
                    commands.entity(entity).insert(doll_layer.clone());
                }
                Err(_) => {}
            }
        }
    }
}

/// Frame the clone's full body whenever its mesh set changes, and re-pose on
/// yaw changes. Mesh-set signature instead of measured height because skinned
/// meshes sway with the stand animation.
pub fn aim_paperdoll_camera(
    state: Res<InventoryState>,
    yaw: Res<PaperDollYaw>,
    clones: Query<&GlobalTransform, With<PaperDollClone>>,
    children: Query<&Children>,
    meshes: Query<(&GlobalTransform, &Aabb, &Visibility), With<Mesh3d>>,
    mut cameras: Query<(&ChildOf, &mut Transform, &mut PaperDollAim), With<PaperDollCamera>>,
) {
    if !state.open {
        return;
    }
    for (child_of, mut transform, mut aim) in cameras.iter_mut() {
        let clone = child_of.parent();
        let Ok(clone_gt) = clones.get(clone) else {
            continue;
        };

        let mut signature = 0u64;
        for entity in children.iter_descendants(clone) {
            if let Ok((_, _, visibility)) = meshes.get(entity) {
                if *visibility != Visibility::Hidden {
                    signature ^= entity.to_bits();
                }
            }
        }
        if signature == 0 || (signature == aim.signature && !yaw.is_changed()) {
            continue;
        }

        if signature != aim.signature {
            // combined bounding box of the visible body meshes, in clone space
            let to_clone = clone_gt.affine().inverse();
            let mut min = Vec3A::splat(f32::MAX);
            let mut max = Vec3A::splat(f32::MIN);
            for entity in children.iter_descendants(clone) {
                let Ok((mesh_gt, aabb, visibility)) = meshes.get(entity) else {
                    continue;
                };
                if *visibility == Visibility::Hidden {
                    continue;
                }
                let to_local = to_clone * mesh_gt.affine();
                for i in 0..8 {
                    let corner = Vec3A::from(aabb.center)
                        + Vec3A::from(aabb.half_extents)
                            * Vec3A::new(
                                if i & 1 == 0 { -1.0 } else { 1.0 },
                                if i & 2 == 0 { -1.0 } else { 1.0 },
                                if i & 4 == 0 { -1.0 } else { 1.0 },
                            );
                    let p = to_local.transform_point3a(corner);
                    min = min.min(p);
                    max = max.max(p);
                }
            }
            if min.y >= max.y {
                // AABBs not ready yet — leave the signature unset so we retry
                continue;
            }
            let height = (max.y - min.y).max(0.2);
            aim.signature = signature;
            aim.focus = Vec3::from((min + max) * 0.5);
            aim.distance = height * MARGIN * 0.5 / (FOV * 0.5).tan();
        }

        // orbit pose: in front of the body (it faces -Z), rotated by yaw
        let offset = Quat::from_rotation_y(yaw.0) * Vec3::new(0.0, 0.0, -aim.distance);
        *transform = Transform::from_translation(aim.focus + offset).looking_at(aim.focus, Vec3::Y);
    }
}

/// Render the paper-doll only while the inventory window is open.
pub fn update_paperdoll_activity(
    state: Res<InventoryState>,
    mut cameras: Query<&mut Camera, With<PaperDollCamera>>,
) {
    if !state.is_changed() {
        return;
    }
    for mut camera in cameras.iter_mut() {
        if camera.is_active != state.open {
            camera.is_active = state.open;
        }
    }
}
