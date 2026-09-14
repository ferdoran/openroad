use std::collections::HashMap;

use bevy::asset::{io::Reader, AssetLoader, LoadContext};
use bevy::reflect::TypePath;
use thiserror::Error;

use super::format::{self, EeParameter, EfController};
use super::{normalize_effect_path, JMXVEFF};

#[derive(Default, TypePath)]
pub struct EfpLoader;

#[derive(Error, Debug)]
pub enum EfpLoaderError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse error: {0}")]
    Parse(#[from] format::ParseError),
}

impl AssetLoader for EfpLoader {
    type Asset = JMXVEFF;
    type Settings = ();
    type Error = EfpLoaderError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await?;
        let effect = format::parse_efp(&buf)?;

        // Pre-load every referenced texture/mesh/animation so that
        // is_loaded_with_dependencies() covers the whole effect. All paths in
        // .efp files are relative to the Particles.pk2 root.
        let mut textures = HashMap::new();
        let mut meshes = HashMap::new();
        let mut animations = HashMap::new();
        for node in &effect.nodes {
            let resources = node.controllers.iter().filter_map(|c| match c {
                EfController::Shape { resource, .. } => Some(resource),
                _ => None,
            });
            for resource in resources.chain(std::iter::once(&node.resource)) {
                for path in resource.texture_paths() {
                    let normalized = normalize_effect_path(path);
                    textures
                        .entry(normalized.clone())
                        .or_insert_with(|| load_context.load(format!("particles://{normalized}")));
                }
                for path in resource.mesh_paths() {
                    let normalized = normalize_effect_path(path);
                    meshes
                        .entry(normalized.clone())
                        .or_insert_with(|| load_context.load(format!("particles://{normalized}")));
                }
            }

            let ban_lists = node.controllers.iter().filter_map(|c| match c {
                EfController::Ban(paths) => Some(paths),
                _ => None,
            });
            let global_ban_lists = node.global_params.iter().filter_map(|(_, p)| match p {
                EeParameter::BsAnimation(paths) => Some(paths),
                _ => None,
            });
            for path in ban_lists.chain(global_ban_lists).flatten() {
                if path.is_empty() {
                    continue;
                }
                let normalized = normalize_effect_path(path);
                animations
                    .entry(normalized.clone())
                    .or_insert_with(|| load_context.load(format!("particles://{normalized}")));
            }
        }

        Ok(JMXVEFF {
            effect,
            textures,
            meshes,
            animations,
        })
    }

    fn extensions(&self) -> &[&str] {
        &["efp"]
    }
}
