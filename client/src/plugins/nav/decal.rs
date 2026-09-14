use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::picking::hover::HoverMap;
use bevy::prelude::*;

use crate::plugins::cursor::interactions::entity_select::HoveredEntity;
use crate::plugins::cursor::GameCursorCamera;
use crate::plugins::hud::death::player_is_dead;
use crate::plugins::hud::inventory::model::InventoryState;
use crate::plugins::nav::NavMeshRaycast;
use crate::plugins::player::PlayerMoveOrder;

/// World-space edge length of the cursor decal.
const CURSOR_DECAL_SIZE: f32 = 16.0;
/// Quads per side of a decal grid. The nav height map samples every 20 units,
/// so even a large decal stays smooth at this resolution.
const DECAL_RESOLUTION: usize = 16;
/// Lift above the nav mesh surface to avoid z-fighting with the terrain.
const DECAL_LIFT: f32 = 0.3;

pub struct NavMeshDecalPlugin;

impl Plugin for NavMeshDecalPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<super::NavObjectGrid>()
            // PostUpdate: needs this frame's propagated GlobalTransforms.
            .add_systems(
                PostUpdate,
                super::index::rebuild_nav_object_grid
                    .after(bevy::transform::TransformSystems::Propagate),
            )
            .add_systems(Startup, spawn_cursor_decal)
            // Chained so draping sees the transform written this frame, not last
            // frame's — otherwise the heights would lag one click behind.
            .add_systems(
                Update,
                (
                    // Don't move when the click lands on a UI element (e.g. the
                    // system window) or while the player is dead (#142 input lock).
                    place_decal_on_click
                        .run_if(not(pointer_over_ui))
                        .run_if(not(player_is_dead)),
                    drape_decals,
                )
                    .chain(),
            );
    }
}

/// True when the pointer is over any pickable UI node, so a map click shouldn't
/// fall through to a movement order.
fn pointer_over_ui(hover_map: Res<HoverMap>, ui_nodes: Query<(), With<Node>>) -> bool {
    hover_map
        .values()
        .flat_map(|hits| hits.keys())
        .any(|entity| ui_nodes.contains(*entity))
}

/// A textured `size` x `size` quad grid whose vertices are re-draped onto the
/// walkable nav mesh surface (terrain height field or object nav meshes like
/// bridge decks) around the entity's translation every frame, so the texture
/// deforms with the surface like a projected decal.
///
/// Spawn one with [`decal_mesh`] as its `Mesh3d`; `drape_decals` does the rest.
/// The entity must be top-level (draping reads its `Transform` directly).
#[derive(Component)]
pub struct NavMeshDecal {
    pub size: f32,
}

/// Marks the decal shown at the last clicked nav mesh point (the move-target
/// circle). Hidden until the first click.
#[derive(Component)]
struct CursorDecal;

fn spawn_cursor_decal(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        Name::from("Nav Mesh Cursor Decal"),
        NavMeshDecal {
            size: CURSOR_DECAL_SIZE,
        },
        CursorDecal,
        Mesh3d(meshes.add(decal_mesh())),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color_texture: Some(asset_server.load("media://effect/select_01.ddj")),
            unlit: true,
            // Silkroad effect textures glow on a black background; switch to
            // AlphaMode::Blend if the texture turns out to carry real alpha.
            alpha_mode: AlphaMode::Add,
            ..default()
        })),
        Transform::default(),
        Visibility::Hidden,
        // The mesh is rewritten every frame, so its baked Aabb is meaningless.
        NoFrustumCulling,
        NotShadowCaster,
        NotShadowReceiver,
    ));
}

/// On left click, moves the decal to the cursor's nav mesh contact point and
/// shows it. Clicks that miss the nav mesh leave the decal where it was.
fn place_decal_on_click(
    buttons: Res<ButtonInput<MouseButton>>,
    cursor_cameras: Query<&GameCursorCamera>,
    hovered: Res<HoveredEntity>,
    nav_raycast: NavMeshRaycast,
    // Optional: the offline scenes that build the nav plugin without the HUD
    // have no inventory, and "no inventory" means "nothing can be carried".
    carry: Option<Res<InventoryState>>,
    mut move_orders: MessageWriter<PlayerMoveOrder>,
    mut decals: Query<(&mut Transform, &mut Visibility), With<CursorDecal>>,
    mut pressed_on_entity: Local<bool>,
) {
    // Selection/attack reacts on *press* while movement reacts on *release*
    // (same edge as cursor/interactions/terrain.rs) — so a press that began
    // on an entity must never turn into a move order just because the cursor
    // slipped off the hitbox before release (that used to cancel a
    // freshly-ordered attack with an unwanted walk).
    if buttons.just_pressed(MouseButton::Left) {
        *pressed_on_entity = hovered.0.is_some();
    }
    if !buttons.just_released(MouseButton::Left) {
        return;
    }
    // A click on a hoverable entity is a select/interact, not a move order.
    if hovered.0.is_some() || *pressed_on_entity {
        *pressed_on_entity = false;
        return;
    }

    let Some(hit) = cursor_cameras
        .iter()
        .filter_map(|camera| camera.cursor_ray)
        .find_map(|ray| nav_raycast.cast_walkable(&ray))
    else {
        return;
    };

    // A click that releases a carried item must not also walk the character.
    //
    // Six systems read `PlayerMoveOrder`, so suppressing it at the one emitter
    // is far cheaper than guarding all of them — the same shape as the mounted
    // guard in `player::apply_local_move_order`. What *is* a drop is decided by
    // `hud::inventory::drop_item::detect_drop_on_nothing`, not here: this path
    // only ever sees walkable ground, which turned out to be far narrower than
    // "the player released on nothing".
    if carry.is_some_and(|state| state.drag.is_some()) {
        return;
    }

    // Issue a move order; whether it's applied locally (sandbox) or sent to the
    // server (in-game) is decided downstream.
    move_orders.write(PlayerMoveOrder(hit.point));

    for (mut transform, mut visibility) in decals.iter_mut() {
        transform.translation = hit.point;
        *visibility = Visibility::Visible;
    }
}

/// Re-drapes a decal's grid onto the walkable surface around it whenever the
/// decal moves. The decal center's height picks between stacked surfaces, so a
/// decal placed on a bridge deck follows the deck while one placed below follows
/// the ground.
///
/// Gated on `Changed<Transform>`: the walkable surface is static, so a decal
/// that hasn't moved re-projects to the exact same 289 vertices — draping it
/// every frame was ~8 ms of wasted nav raycasts (one per grid vertex) while the
/// move-target circle sat still. `place_decal_on_click` is chained before this
/// and writes the transform on the click frame, so the drape still lands the
/// same frame the decal is placed.
fn drape_decals(
    decals: Query<(&NavMeshDecal, &Transform, &Visibility, &Mesh3d), Changed<Transform>>,
    mut meshes: ResMut<Assets<Mesh>>,
    nav_raycast: NavMeshRaycast,
) {
    for (decal, transform, visibility, mesh3d) in decals.iter() {
        if *visibility == Visibility::Hidden {
            continue;
        }
        let Some(mut mesh) = meshes.get_mut(&mesh3d.0) else {
            continue;
        };

        let center = transform.translation;
        let step = decal.size / DECAL_RESOLUTION as f32;
        let half = decal.size / 2.0;

        // Resolve the surface once, at the centre, and drape every vertex
        // against it. Per-vertex resolution would let vertices either side of a
        // deck edge pick different surfaces and tear the decal in half; this
        // way the decal belongs to whatever the centre sits on, and vertices
        // that overhang fall back to the terrain.
        let center_location = nav_raycast.resolve_location(center);

        let mut positions = Vec::with_capacity((DECAL_RESOLUTION + 1) * (DECAL_RESOLUTION + 1));
        for gz in 0..=DECAL_RESOLUTION {
            for gx in 0..=DECAL_RESOLUTION {
                let local_x = gx as f32 * step - half;
                let local_z = gz as f32 * step - half;
                let world_y = nav_raycast
                    .ground(
                        Vec2::new(center.x + local_x, center.z + local_z),
                        center.y,
                        center_location,
                    )
                    .map_or(center.y, |(height, _)| height);
                positions.push([local_x, world_y + DECAL_LIFT - center.y, local_z]);
            }
        }
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    }
}

/// Flat `DECAL_RESOLUTION`² quad grid with UVs spanning the full texture;
/// positions are overwritten each frame by `drape_decals`.
pub fn decal_mesh() -> Mesh {
    let verts_per_side = DECAL_RESOLUTION + 1;
    let mut positions = Vec::with_capacity(verts_per_side * verts_per_side);
    let mut uvs = Vec::with_capacity(verts_per_side * verts_per_side);
    for gz in 0..verts_per_side {
        for gx in 0..verts_per_side {
            positions.push([0.0_f32, 0.0, 0.0]);
            uvs.push([
                gx as f32 / DECAL_RESOLUTION as f32,
                gz as f32 / DECAL_RESOLUTION as f32,
            ]);
        }
    }

    let mut indices = Vec::with_capacity(DECAL_RESOLUTION * DECAL_RESOLUTION * 6);
    for gz in 0..DECAL_RESOLUTION as u32 {
        for gx in 0..DECAL_RESOLUTION as u32 {
            let i = gz * verts_per_side as u32 + gx;
            let below = i + verts_per_side as u32;
            indices.extend_from_slice(&[i, below, i + 1, i + 1, below, below + 1]);
        }
    }

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        // MAIN_WORLD keeps the CPU copy so positions can be rewritten each frame.
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_NORMAL,
        vec![[0.0_f32, 1.0, 0.0]; verts_per_side * verts_per_side],
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}
