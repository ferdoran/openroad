//! Resolve foliage colours from loader-derived terrain tints. The table survives
//! mode changes and world re-entry; reload revisions rebuild baked vertex colours.
use super::{FoliageBlock, FoliageDone, FoliageLibrary, FoliagePending};
use crate::assets::m::block_splat_material::TerrainTileAtlas;
use crate::assets::tile_tint::TerrainTileTints;
use bevy::prelude::*;

pub(super) fn resolve_tile_tints(
    mut library: ResMut<FoliageLibrary>,
    atlas: Option<Res<TerrainTileAtlas>>,
    tints: Res<TerrainTileTints>,
    server: Res<AssetServer>,
    mut last_revision: Local<u64>,
    mut commands: Commands,
    merged: Query<Entity, With<FoliageBlock>>,
    built: Query<Entity, Or<(With<FoliageDone>, With<FoliagePending>)>>,
) {
    let Some(atlas) = atlas else {
        return;
    };
    let table = tints.0.read().unwrap();
    if library.tints_resolved && *last_revision == table.revision {
        return;
    }
    if *last_revision != table.revision {
        debug!(
            "terrain tints: {} textures, {} source bytes transferred through the render queue",
            table.entries.len(),
            table
                .entries
                .values()
                .map(|entry| entry.source_bytes)
                .sum::<usize>()
        );
    }
    *last_revision = table.revision;
    let mut all_resolved = true;
    let mut changed = false;
    let ids: Vec<_> = library.tile_recipes.keys().copied().collect();
    for id in ids {
        let color = match atlas.slots.get(id as usize).and_then(Option::as_ref) {
            None => Some(Vec3::ONE),
            Some(handle) => handle
                .path()
                .and_then(|path| table.entries.get(path))
                .map(|entry| entry.color)
                .or_else(|| {
                    matches!(
                        server.get_load_state(handle),
                        Some(bevy::asset::LoadState::Failed(_))
                    )
                    .then_some(Vec3::ONE)
                }),
        };
        let Some(color) = color else {
            all_resolved = false;
            continue;
        };
        let previous = library.tile_tints.insert(id, color);
        changed |= previous.is_some_and(|previous| previous != color);
    }
    library.tints_resolved = all_resolved;
    if changed {
        for entity in &merged {
            commands.entity(entity).despawn();
        }
        for entity in &built {
            commands
                .entity(entity)
                .remove::<FoliageDone>()
                .remove::<FoliagePending>();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::asset::{AssetPath, RenderAssetUsages};
    use bevy::ecs::system::RunSystemOnce;
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

    #[test]
    fn tints_survive_library_resets_and_reload_rebuilds_baked_grass() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<Image>()
            .init_resource::<FoliageLibrary>()
            .init_resource::<TerrainTileTints>();
        let path = AssetPath::from("tile2d/synthetic.ddj");
        let handle = app
            .world()
            .resource::<AssetServer>()
            .load::<Image>(path.clone());
        app.insert_resource(TerrainTileAtlas {
            slots: vec![Some(handle), None],
        });
        let mut image = Image::new_fill(
            Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            &[0, 255, 0, 255],
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::RENDER_WORLD,
        );
        app.world()
            .resource::<TerrainTileTints>()
            .derive(path.clone(), &image);
        for _ in 0..3 {
            // Mode changes and world re-entry may replace the entire library.
            let mut library = FoliageLibrary::default();
            for id in 0..2 {
                library.tile_recipes.insert(
                    id,
                    super::super::TileRecipe {
                        pairs: vec![],
                        pack_eligible: true,
                    },
                );
            }
            app.insert_resource(library);
            app.world_mut().run_system_once(resolve_tile_tints).unwrap();
            let library = app.world().resource::<FoliageLibrary>();
            assert!(library.tints_resolved);
            assert_eq!(library.tile_tints[&0], Vec3::Y);
            assert_eq!(library.tile_tints[&1], Vec3::ONE);
        }
        let merged = app.world_mut().spawn(FoliageBlock).id();
        let block = app.world_mut().spawn(FoliageDone).id();
        image
            .data
            .as_mut()
            .unwrap()
            .copy_from_slice(&[255, 0, 0, 255]);
        app.world()
            .resource::<TerrainTileTints>()
            .derive(path, &image);
        app.world_mut().run_system_once(resolve_tile_tints).unwrap();
        assert_eq!(
            app.world().resource::<FoliageLibrary>().tile_tints[&0],
            Vec3::X
        );
        assert!(app.world().get_entity(merged).is_err());
        assert!(app.world().get::<FoliageDone>(block).is_none());
    }
}
