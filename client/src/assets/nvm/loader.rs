use crate::assets::nvm::{NvmParseError, JMXVNVM};
use bevy::asset::{io::Reader, AssetLoader, LoadContext};
use std::ops::Deref;
use thiserror::Error;

#[derive(Default, bevy::reflect::TypePath)]
pub struct NvmLoader;

#[derive(Error, Debug)]
pub enum NvmLoaderError {
    #[error("could not read the navmesh: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Parse(#[from] NvmParseError),
}

impl AssetLoader for NvmLoader {
    type Asset = JMXVNVM;
    type Settings = ();
    type Error = NvmLoaderError;
    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await?;
        let nvm_file = JMXVNVM::try_from(buf.deref())?;
        Ok(nvm_file)
    }

    fn extensions(&self) -> &[&str] {
        &["nvm"]
    }
}
