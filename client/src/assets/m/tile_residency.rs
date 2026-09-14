//! Watch uploaded atlas views, then replace terrain material handles when those
//! views change. New handles retire the old bind groups normally, avoiding Bevy
//! 0.19's leaking direct-bind-group modification path. CPU pixels are never edited.
use super::block_splat_material::{TerrainBlockSplatMaterial, TerrainTileAtlas};
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::TextureViewId;
use bevy::render::texture::GpuImage;
use bevy::render::{Render, RenderApp, RenderSystems};
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

#[derive(Resource, Clone, Default)]
struct AtlasRevision(Arc<AtomicU64>);

pub(super) fn register(app: &mut App) {
    let revision = AtlasRevision::default();
    app.insert_resource(revision.clone())
        .add_systems(Update, refresh_materials);
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render.insert_resource(revision).add_systems(
            Render,
            observe_uploaded_views.after(RenderSystems::PrepareAssets),
        );
    }
}

fn observe_uploaded_views(
    atlas: Option<Res<TerrainTileAtlas>>,
    images: Res<RenderAssets<GpuImage>>,
    revision: Res<AtlasRevision>,
    mut previous: Local<Vec<Option<TextureViewId>>>,
) {
    let Some(atlas) = atlas else {
        return;
    };
    if !images.is_changed() && !atlas.is_changed() {
        return;
    }
    let mut changed = previous.len() != atlas.slots.len();
    previous.resize(atlas.slots.len(), None);
    for (last, handle) in previous.iter_mut().zip(&atlas.slots) {
        let view = handle
            .as_ref()
            .and_then(|h| images.get(h))
            .map(|image| image.texture_view.id());
        changed |= *last != view;
        *last = view;
    }
    if changed {
        revision.0.fetch_add(1, Ordering::Release);
    }
}

fn refresh_materials(
    revision: Res<AtlasRevision>,
    mut last: Local<u64>,
    mut materials: ResMut<Assets<TerrainBlockSplatMaterial>>,
    mut meshes: Query<&mut MeshMaterial3d<TerrainBlockSplatMaterial>>,
) {
    let current = revision.0.load(Ordering::Acquire);
    if current == *last {
        return;
    }
    *last = current;
    let mut replacements = HashMap::new();
    for mut mesh in &mut meshes {
        let old = mesh.0.id();
        if let Some(handle) = replacements.get(&old) {
            mesh.0 = Handle::clone(handle);
        } else if let Some(material) = materials.get(old).cloned() {
            let handle = materials.add(material);
            mesh.0 = handle.clone();
            replacements.insert(old, handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    #[test]
    fn a_new_gpu_revision_replaces_shared_handles_without_modifying_old_assets() {
        let mut world = World::new();
        world.init_resource::<Assets<TerrainBlockSplatMaterial>>();
        world.insert_resource(AtlasRevision(Arc::new(AtomicU64::new(1))));
        let old = world
            .resource_mut::<Assets<TerrainBlockSplatMaterial>>()
            .add(TerrainBlockSplatMaterial::default());
        let a = world.spawn(MeshMaterial3d(old.clone())).id();
        let b = world.spawn(MeshMaterial3d(old.clone())).id();
        world.run_system_once(refresh_materials).unwrap();
        let new_a = &world
            .get::<MeshMaterial3d<TerrainBlockSplatMaterial>>(a)
            .unwrap()
            .0;
        let new_b = &world
            .get::<MeshMaterial3d<TerrainBlockSplatMaterial>>(b)
            .unwrap()
            .0;
        assert_ne!(new_a.id(), old.id());
        assert_eq!(new_a.id(), new_b.id());
        assert!(world
            .resource::<Assets<TerrainBlockSplatMaterial>>()
            .contains(old.id()));
    }
}
