use std::path::PathBuf;

use bevy::asset::{Asset, Handle};
use bevy::prelude::{AssetServer, Res};

pub trait PathBufExt {
    fn load<T: Asset>(&self, asset_server: &Res<AssetServer>) -> Handle<T>;
    fn load2<T: Asset>(&self, asset_server: &AssetServer) -> Handle<T>;
}

impl PathBufExt for PathBuf {
    fn load<T: Asset>(&self, asset_server: &Res<AssetServer>) -> Handle<T> {
        asset_server.load(self.clone())
    }

    fn load2<T: Asset>(&self, asset_server: &AssetServer) -> Handle<T> {
        asset_server.load(self.clone())
    }
}
