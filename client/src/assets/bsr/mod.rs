use std::io::{Cursor, Seek, SeekFrom};

use bevy::asset::{io::Reader, AssetLoader, LoadContext};
use bevy::prelude::default;
use bytes::Buf;
use thiserror::Error;

use crate::assets::bsr::bsr::{MaterialData, MeshData, ObjectInfo, ResourceHeader, JMXVRES};
use crate::assets::bsr::collision_mesh::CollisionMesh;
use crate::util::buf_ext::BufExt;

pub mod bsr;
pub mod collision_mesh;
pub mod loader;
pub mod resource;

#[derive(Default, bevy::reflect::TypePath)]
#[allow(dead_code)]
pub struct BsrLoader;

#[derive(Error, Debug)]
pub enum BsrLoaderError {
    #[error("IO error: {0}")]
    IO(std::io::Error),
    #[error("failed to seek to {0} offset: {1}")]
    Seek(&'static str, std::io::Error),
}

impl AssetLoader for BsrLoader {
    type Asset = JMXVRES;
    type Settings = ();
    type Error = BsrLoaderError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await.unwrap();
        let bytes = &buf;
        let mut cursor = Cursor::new(bytes);

        let _sig = cursor.get_fixed_size_string(12);
        let header = ResourceHeader::from(&mut cursor);
        let object_info = ObjectInfo::from(&mut cursor);
        let _ = cursor
            .seek(SeekFrom::Start(header.collision_offset as u64))
            .map_err(|e| BsrLoaderError::Seek("collision", e))?;
        let collision_mesh = CollisionMesh::from(&mut cursor);

        let _ = cursor
            .seek(SeekFrom::Start(header.mesh_offset as u64))
            .map_err(|e| BsrLoaderError::Seek("mesh", e))?;
        let mesh_count = cursor.get_u32_le();
        // debug!("{}: {} meshes", load_context.path().path().display(), mesh_count);
        let mesh = (0..mesh_count)
            .map(|_| {
                let path = cursor.get_path_buf_double_len();
                let unknown = if header.prim_mesh_flag & 1 != 0 {
                    Some(cursor.get_u32_le())
                } else {
                    None
                };
                MeshData { path, unknown }
            })
            .collect();

        let _ = cursor
            .seek(SeekFrom::Start(header.material_offset as u64))
            .map_err(|e| BsrLoaderError::Seek("material", e))?;
        let material_count = cursor.get_u32_le();
        let mut materials = Vec::with_capacity(material_count as usize);
        for _ in 0..material_count {
            materials.push(MaterialData {
                id: cursor.get_u32_le(),
                path: cursor.get_path_buf_double_len(),
            });
        }

        let _ = cursor
            .seek(SeekFrom::Start(header.skeleton_offset as u64))
            .map_err(|e| BsrLoaderError::Seek("skeleton", e))?;
        let has_skeleton = cursor.get_u32_le() == 1;
        let skeleton = if has_skeleton {
            Some((
                cursor.get_path_buf_double_len(),
                cursor.get_double_len_string(),
            ))
        } else {
            None
        };

        // debug!("materials: {:?}", materials);
        let resource = JMXVRES {
            header,
            object_info,
            collision_mesh,
            mesh,
            materials,
            skeleton,
            ..default()
        };
        // load_context.set_default_asset(LoadedAsset::new(resource));
        Ok(resource)
    }

    fn extensions(&self) -> &[&str] {
        &["bsr"]
    }
}
