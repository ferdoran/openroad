use bevy::asset::Handle;
use bevy::prelude::*;
use bevy_asset_loader::prelude::AssetCollection;

use crate::assets::ifo::IFOAsset;
use crate::assets::mfo::JMXVMFO;

#[derive(AssetCollection, Resource)]
pub struct TileAssets {
    #[asset(path = "map://tile2d", collection(typed))]
    pub tiles: Vec<Handle<Image>>,
    #[asset(path = "map://tile2d.ifo")]
    pub tile_index: Handle<IFOAsset>,
}
#[derive(AssetCollection, Resource)]
#[allow(dead_code)]
pub struct MapsAssets {
    // #[asset(path="Map", collection)]
    // pub folder: Vec<HandleUntyped>,
    #[asset(path = "map://mapinfo.mfo")]
    pub map_info: Handle<JMXVMFO>,
    #[asset(path = "map://object.ifo")]
    pub object_index: Handle<IFOAsset>,
    #[asset(path = "map://environment.ifo")]
    pub environment_info: Handle<IFOAsset>,
    #[asset(path = "map://skybox/cloud1.ddj")]
    pub cloud_layer_1: Handle<Image>,
    #[asset(path = "map://skybox/cloud99.ddj")]
    pub cloud_layer_2: Handle<Image>,
}
