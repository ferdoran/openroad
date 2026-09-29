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

        // Two independent failure classes, handled at their own level. A
        // missing *archive* is a data condition, not a bug: report which one
        // and carry on -- consumers already tolerate an absent source
        // (`Option<Res<MediaArchive>>`, asset loads that fail by name). A
        // missing *file* inside an archive that did open is handled one layer
        // down by `FallbackAssetReader`, which substitutes a warned-about
        // placeholder so a `Failed` load cannot hang `bevy_asset_loader`'s
        // `AssetCollection` gate (see `fallback_reader` module docs).
        let media_archive = Archive::open_or_report(media_path, &key);
        if let Some(media_archive) = media_archive.clone() {
            app.insert_resource(MediaArchive(media_archive));
        }

        for (name, archive) in [
            ("media", media_archive),
            ("map", Archive::open_or_report(map_path, &key)),
            ("music", Archive::open_or_report(music_path, &key)),
            ("data", Archive::open_or_report(data_path, &key)),
            ("particles", Archive::open_or_report(particles_path, &key)),
        ] {
            let Some(archive) = archive else { continue };
            let reader = Box::new(FallbackAssetReader { inner: archive });
            app.register_asset_source(name, AssetSourceBuilder::new(move || reader.clone()));
        }
    }
}
