use bevy::asset::{io::Reader, AssetLoader, LoadContext};
use std::ops::Deref;
use thiserror::Error;

use crate::assets::m::JMXVMAPM;

#[derive(Default, bevy::reflect::TypePath)]
pub struct MLoader;

#[derive(Error, Debug)]
pub enum MLoaderError {
    #[error("failed to parse x coord")]
    XCoord,
    #[error("failed to parse z coord")]
    ZCoord,
    #[error("io: {0}")]
    Io(std::io::Error),
}

impl AssetLoader for MLoader {
    type Asset = JMXVMAPM;
    type Settings = ();
    type Error = MLoaderError;
    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut buf = Vec::new();
        reader
            .read_to_end(&mut buf)
            .await
            .map_err(MLoaderError::Io)?;
        let bytes = buf.deref();
        let m_file = JMXVMAPM::from(bytes);
        let mut path = load_context.path().path().to_path_buf();
        let _x_coord = path
            .file_stem()
            .and_then(|c| c.to_str())
            .ok_or(MLoaderError::XCoord)?
            .to_string();
        path.pop();
        let _z_coord = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or(MLoaderError::ZCoord)?;

        Ok(m_file)
    }

    fn extensions(&self) -> &[&str] {
        &["m"]
    }
}
