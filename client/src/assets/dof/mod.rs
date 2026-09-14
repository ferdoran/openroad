//! JMXVDOF (`*.dof`) dungeon asset: loader shell around the pure parser in
//! [`format`]. Dungeon files live in Data.pk2 (`data://dungeon/**.dof`) and
//! are looked up through the `dungeoninfo.txt` id→path table.

pub mod format;

use bevy::asset::{io::Reader, AssetLoader, LoadContext};
use thiserror::Error;

pub use format::JMXVDOF;

#[derive(Default, bevy::reflect::TypePath)]
pub struct DofLoader;

#[derive(Error, Debug)]
pub enum DofLoaderError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("DOF parse error: {0}")]
    Parse(#[from] format::DofError),
}

impl AssetLoader for DofLoader {
    type Asset = JMXVDOF;
    type Settings = ();
    type Error = DofLoaderError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await?;
        Ok(format::parse(&buf)?)
    }

    fn extensions(&self) -> &[&str] {
        &["dof"]
    }
}
