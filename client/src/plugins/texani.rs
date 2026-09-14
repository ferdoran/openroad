// TexAni runtime: swaps the material of marked meshes for the UV-scrolling
// variant. Spawn (`SpawnResource::apply_with_caches`) tags meshes targeted
// by a resource's TexAni mods with `UvScrollSpeed`; this plugin's system
// then clones the mesh's loaded StandardMaterial (the labeled .bmt
// sub-asset shared by every instance) into an `SroUvScrollMaterial` whose
// shader scrolls the UVs by `speed * globals.time`. Deduped per (base
// material, speed) so repeated waterfall instances share one material and
// keep batching; animation itself is shader-clock driven, so nothing here
// runs per frame once the swap happened.
use std::collections::HashMap;
use std::time::Duration;

use bevy::asset::AssetId;
use bevy::pbr::{ExtendedMaterial, MaterialPlugin, MeshMaterial3d};
use bevy::prelude::*;
use bevy::time::common_conditions::on_timer;

use crate::assets::bmt::uv_scroll::{SroUvScrollMaterial, UvScrollExtension, UvScrollSettings};

/// UV scroll (uv/sec) a mesh's material should animate at, from its
/// resource's TexAni mod. Removed once the material swap happened.
#[derive(Component)]
pub struct UvScrollSpeed {
    pub uv_speed: Vec2,
    /// Blend override from the resource's Material mod (waterfall sheets
    /// are alpha-blended or additive, not masked); `None` keeps the base
    /// material's alpha mode.
    pub alpha_mode: Option<AlphaMode>,
}

/// Cache key discriminant for the blend override (AlphaMode is not Hash).
fn alpha_mode_key(mode: Option<AlphaMode>) -> u8 {
    match mode {
        None => 0,
        Some(AlphaMode::Blend) => 1,
        Some(AlphaMode::Add) => 2,
        Some(_) => 3, // loader only produces Blend/Add today
    }
}

/// Caches the scrolling material built per (base StandardMaterial, speed
/// bits, blend override), so every instance of the same waterfall shares
/// one material.
///
/// Entries are weak `AssetId`s, same lifetime scheme as
/// [`crate::plugins::map::objects::SroMeshes`]: the swapped entities are
/// the only strong owners, lookups resolve via `get_strong_handle` and
/// rebuild on a dead id, and a timer prune sweeps dead entries.
#[derive(Resource, Default)]
pub struct UvScrollMaterials(
    pub HashMap<(AssetId<StandardMaterial>, u32, u32, u8), AssetId<SroUvScrollMaterial>>,
);

pub struct TexAniPlugin;

impl Plugin for TexAniPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<SroUvScrollMaterial>::default())
            .init_resource::<UvScrollMaterials>()
            .add_systems(
                Update,
                (
                    apply_uv_scroll_materials,
                    prune_uv_scroll_cache.run_if(on_timer(Duration::from_secs(10))),
                ),
            );
    }
}

/// Swaps `MeshMaterial3d<StandardMaterial>` for the scrolling variant on
/// tagged meshes. Runs until the base material asset is loaded (spawn
/// usually beat the async .bmt sub-asset by zero frames — the resource
/// loader gates on dependencies — but retrying keeps any path safe).
fn apply_uv_scroll_materials(
    mut commands: Commands,
    pending: Query<(Entity, &UvScrollSpeed, &MeshMaterial3d<StandardMaterial>)>,
    standard: Res<Assets<StandardMaterial>>,
    mut scroll_materials: ResMut<Assets<SroUvScrollMaterial>>,
    mut cache: ResMut<UvScrollMaterials>,
) {
    for (entity, scroll, material) in &pending {
        let key = (
            material.0.id(),
            scroll.uv_speed.x.to_bits(),
            scroll.uv_speed.y.to_bits(),
            alpha_mode_key(scroll.alpha_mode),
        );
        let handle = cache
            .0
            .get(&key)
            .and_then(|id| scroll_materials.get_strong_handle(*id))
            .or_else(|| {
                // base not loaded yet -> retry next frame
                let mut base = standard.get(&material.0)?.clone();
                if let Some(alpha_mode) = scroll.alpha_mode {
                    base.alpha_mode = alpha_mode;
                }
                let handle = scroll_materials.add(ExtendedMaterial {
                    base,
                    extension: UvScrollExtension {
                        settings: UvScrollSettings {
                            uv_speed: scroll.uv_speed,
                            ..default()
                        },
                    },
                });
                cache.0.insert(key, handle.id());
                Some(handle)
            });
        let Some(handle) = handle else {
            continue;
        };
        commands
            .entity(entity)
            .remove::<MeshMaterial3d<StandardMaterial>>()
            .remove::<UvScrollSpeed>()
            .insert(MeshMaterial3d(handle));
    }
}

fn prune_uv_scroll_cache(
    scroll_materials: Res<Assets<SroUvScrollMaterial>>,
    mut cache: ResMut<UvScrollMaterials>,
) {
    cache.0.retain(|_, id| scroll_materials.contains(*id));
}
