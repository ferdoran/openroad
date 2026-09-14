//! JMXVEFF (.efp) particle/visual effect assets.
//!
//! `format` holds the pure byte-level parser (shared with the efp_scan tool),
//! `loader` the Bevy `AssetLoader` that additionally pre-loads the textures,
//! meshes, and animations an effect references from Particles.pk2.

pub mod format;
pub mod loader;

use std::collections::HashMap;

use bevy::asset::{Asset, Handle};
use bevy::image::Image;
use bevy::reflect::TypePath;

use crate::assets::ban::JMXVBAN;
use crate::assets::bms::mesh::JMXVBMS;
use format::EfStoredEffect;

/// A parsed effect file plus handles to every asset it references.
/// Keys are the normalized (`\` -> `/`) paths as they appear in the file.
#[derive(Asset, TypePath)]
pub struct JMXVEFF {
    pub effect: EfStoredEffect,
    pub textures: HashMap<String, Handle<Image>>,
    pub meshes: HashMap<String, Handle<JMXVBMS>>,
    pub animations: HashMap<String, Handle<JMXVBAN>>,
}

/// Normalize a resource path stored inside an .efp to asset-path form.
pub fn normalize_effect_path(path: &str) -> String {
    path.replace('\\', "/")
}
