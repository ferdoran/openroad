use std::env;
use std::path::PathBuf;

use bevy::app::{App, Plugin};
use bevy::asset::io::AssetSourceBuilder;
use bevy::asset::AssetApp;
use bevy::ecs::resource::Resource;

use bevy_pk2::prelude::{Archive, Pk2Key};

use crate::plugins::assets::fallback_reader::FallbackAssetReader;

/// A shared handle on the opened Media.pk2 for consumers that need direct
/// archive access (directory listings) outside the asset-server path — e.g.
/// the minimap's `minimap_d` group discovery. Cloning shares the file handle
/// and index.
#[derive(Resource, Clone)]
pub struct MediaArchive(pub Archive);

pub struct SroAssetPlugin;

impl Plugin for SroAssetPlugin {
    fn build(&self, app: &mut App) {
        let sro_path = env::var_os("SRO_PK2_PATH")
            .or_else(|| env::var_os("SRO_PATH"))
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let mut working_dir = env::current_dir().unwrap();
                working_dir.push("assets");
                working_dir
            });
        println!("PK2 directory: {}", sro_path.display());

        let media_path = sro_path.join("Media.pk2");
        let map_path = sro_path.join("Map.pk2");
        let data_path = sro_path.join("Data.pk2");
        let music_path = sro_path.join("Music.pk2");
        let particles_path = sro_path.join("Particles.pk2");

        // The archive key is the user's, not ours: it is not compiled in, so a
        // missing one is a configuration error rather than a fallback. This
        // runs before the window exists, so the message is all the user gets.
        let key = Pk2Key::resolve().unwrap_or_else(|err| panic!("{err}"));

        let media_archive = Archive::open_or_panic(media_path, &key);
        app.insert_resource(MediaArchive(media_archive.clone()));
        // Wrapped in `FallbackAssetReader` so one missing `.ddj`/`.wav` file
        // substitutes a warned-about placeholder instead of leaving its
        // `LoadState` `Failed`, which would otherwise hang
        // `bevy_asset_loader`'s `AssetCollection` gate — and therefore the
        // loading screen — forever (see `fallback_reader` module docs).
        let media_reader = Box::new(FallbackAssetReader {
            inner: media_archive,
        });
        let map_reader = Box::new(FallbackAssetReader {
            inner: Archive::open_or_panic(map_path, &key),
        });
        let music_reader = Box::new(FallbackAssetReader {
            inner: Archive::open_or_panic(music_path, &key),
        });
        let data_reader = Box::new(FallbackAssetReader {
            inner: Archive::open_or_panic(data_path, &key),
        });
        let particles_reader = Box::new(FallbackAssetReader {
            inner: Archive::open_or_panic(particles_path, &key),
        });

        app.register_asset_source(
            "media",
            AssetSourceBuilder::new(move || media_reader.clone()),
        );
        app.register_asset_source("map", AssetSourceBuilder::new(move || map_reader.clone()));
        app.register_asset_source("data", AssetSourceBuilder::new(move || data_reader.clone()));
        app.register_asset_source(
            "music",
            AssetSourceBuilder::new(move || music_reader.clone()),
        );
        app.register_asset_source(
            "particles",
            AssetSourceBuilder::new(move || particles_reader.clone()),
        );
    }
}
