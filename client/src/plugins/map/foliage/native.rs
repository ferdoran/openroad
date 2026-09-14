//! Native 3D-grass geometry: tile2d.ifo `{model,count}` pairs resolve through
//! object.ifo to ordinary grass `.bsr` resources in the user's Data.pk2, whose
//! `.bms` meshes and `.bmt` materials are already parsed by the regular asset
//! pipeline. This module only *copies the loaded data out* into
//! [`FoliagePartGeometry`] so the scatter stage can stamp thousands of tufts
//! into merged meshes without touching assets again — and swaps each mesh's
//! Masked `StandardMaterial` for a shared cull-off clone, because grass
//! cross-quads are meant to be seen from both sides while regular map objects
//! cull backfaces.

use bevy::asset::{AssetPath, Handle};
use bevy::pbr::StandardMaterial;
use bevy::prelude::*;

use crate::assets::bms::mesh::JMXVBMS;
use crate::assets::bmt::material::material_label;
use crate::assets::bsr::resource::SroResource;
use crate::commands::material_asset_path;

use super::{FoliageModel, FoliagePartGeometry};

/// Copy a loaded grass resource's meshes into stamp-ready geometry. Returns
/// `None` while any dependency (mesh data or labeled material) is not yet
/// resident — the caller retries next frame.
pub(super) fn extract_model(
    resource: &SroResource,
    bms_assets: &Assets<JMXVBMS>,
    asset_server: &AssetServer,
    materials: &mut Assets<StandardMaterial>,
    material_cache: &mut std::collections::HashMap<
        AssetPath<'static>,
        (Handle<StandardMaterial>, Vec3),
    >,
    material_pending: &mut std::collections::HashMap<AssetPath<'static>, Handle<StandardMaterial>>,
) -> Option<FoliageModel> {
    let material_set_path = resource
        .materials
        .first()
        .and_then(|handle| handle.path())?
        .clone_owned();

    let mut parts = Vec::with_capacity(resource.mesh.len());
    for bms_handle in &resource.mesh {
        let bms = bms_assets.get(bms_handle)?;
        let label_path = material_asset_path(&material_set_path, material_label(&bms.material));
        let material = foliage_material(
            &label_path,
            asset_server,
            materials,
            material_cache,
            material_pending,
        )?;

        let mut indices = Vec::with_capacity(bms.indices.len() * 3);
        for (a, b, c) in &bms.indices {
            indices.extend_from_slice(&[*a as u32, *b as u32, *c as u32]);
        }
        let color_factor = material.1;
        let material = material.0;
        parts.push(FoliagePartGeometry {
            positions: bms
                .vertex_data
                .vertices
                .iter()
                .map(|v| v.position)
                .collect(),
            normals: bms.vertex_data.vertices.iter().map(|v| v.normal).collect(),
            uvs: bms
                .vertex_data
                .vertices
                .iter()
                .map(|v| v.uv_0.to_array())
                .collect(),
            indices,
            material,
            color_factor,
        });
    }
    Some(FoliageModel { parts })
}

/// The shared double-sided clone of a `.bmt` labeled Masked material, plus
/// its authored base color as a linear factor. The clone's `base_color` goes
/// white because the merged foliage meshes carry vertex colors, which Bevy
/// substitutes *for* the base color — the authored color survives by being
/// folded into every vertex instead. Cached per label path so every grass
/// model referencing the same material (most share `group_grs01.bmt`) reuses
/// one handle — one bind group, batched draws.
fn foliage_material(
    label_path: &AssetPath<'static>,
    asset_server: &AssetServer,
    materials: &mut Assets<StandardMaterial>,
    material_cache: &mut std::collections::HashMap<
        AssetPath<'static>,
        (Handle<StandardMaterial>, Vec3),
    >,
    material_pending: &mut std::collections::HashMap<AssetPath<'static>, Handle<StandardMaterial>>,
) -> Option<(Handle<StandardMaterial>, Vec3)> {
    if let Some(cached) = material_cache.get(label_path) {
        return Some(cached.clone());
    }
    // The in-flight handle must outlive this call. `AssetServer::load` keys the
    // load on the handle it returns, so dropping the last strong handle cancels
    // it — and returning `None` below with the handle held only in a local did
    // exactly that, restarting the PK2 read + decrypt every frame for as long as
    // foliage stayed on (#741, ~450 MiB/min while standing still). Parking it in
    // `material_pending` keeps the load running until it resolves.
    let source = material_pending
        .entry(label_path.clone())
        .or_insert_with(|| asset_server.load(label_path.clone()))
        .clone();
    let base = materials.get(&source)?.clone();
    let authored = base.base_color.to_linear();
    let color_factor = Vec3::new(authored.red, authored.green, authored.blue);
    // cull_mode None: both quad faces rasterize. double_sided stays FALSE:
    // that flag makes the PBR shader negate the normal on back faces, and
    // grass normals are authored straight up (+Y, verified in the corpus) so
    // the flip turned every back-facing quad dark — random yaws then read as
    // an alternating dark/bright tuft patchwork whenever sun N·L dominates
    // the ambient. Without the flip both faces shade like the ground.
    let foliage = StandardMaterial {
        double_sided: false,
        cull_mode: None,
        base_color: Color::WHITE,
        ..base
    };
    let handle = materials.add(foliage);
    material_cache.insert(label_path.clone(), (handle.clone(), color_factor));
    // Resolved: the clone in `material_cache` is the one the meshes reference,
    // so the source load no longer needs pinning.
    material_pending.remove(label_path);
    Some((handle, color_factor))
}
