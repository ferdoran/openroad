// Abstraction over where SRO game files come from: a PK2 archive or an
// extracted directory tree. SRO data references sub-assets with
// backslash-separated, case-mixed pk2-root-relative paths, so this layer
// owns normalization: every lookup goes through a lowercase forward-slash
// key into a prebuilt index, making reads separator- and case-insensitive.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bevy_pk2::prelude::Archive;

pub trait Source {
    /// Reads a file by its pk2-root-relative path (either separator, any case).
    fn read(&self, sro_path: &str) -> Option<Vec<u8>>;
    /// Lists all `.bsr` files under the given path prefix (normalized keys).
    fn list_bsr(&self, prefix: &str) -> Vec<String>;
}

/// Lowercase forward-slash key used for index lookups.
pub fn normalize(sro_path: &str) -> String {
    sro_path.replace('\\', "/").to_ascii_lowercase()
}

pub struct Pk2Source {
    archive: Archive,
    /// normalized path -> actual index path inside the archive
    index: HashMap<String, PathBuf>,
}

impl Pk2Source {
    pub fn open(pk2_path: &Path) -> Self {
        let archive = Archive::configured(pk2_path);
        let index = archive
            .root
            .get_all_entries()
            .into_iter()
            .filter(|(_, entry)| entry.is_file())
            .map(|(path, _)| (normalize(&path.to_string_lossy()), path))
            .collect();
        Self { archive, index }
    }
}

impl Source for Pk2Source {
    fn read(&self, sro_path: &str) -> Option<Vec<u8>> {
        let actual = self.index.get(&normalize(sro_path))?;
        self.archive.read_file_bytes(actual)
    }

    fn list_bsr(&self, prefix: &str) -> Vec<String> {
        let prefix = normalize(prefix);
        let mut paths: Vec<String> = self
            .index
            .keys()
            .filter(|key| key.starts_with(&prefix) && key.ends_with(".bsr"))
            .cloned()
            .collect();
        paths.sort();
        paths
    }
}

pub struct DirSource {
    /// normalized relative path -> absolute file path
    index: HashMap<String, PathBuf>,
}

impl DirSource {
    pub fn open(root: &Path) -> Self {
        let mut index = HashMap::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if let Ok(rel) = path.strip_prefix(root) {
                    index.insert(normalize(&rel.to_string_lossy()), path);
                }
            }
        }
        Self { index }
    }
}

impl Source for DirSource {
    fn read(&self, sro_path: &str) -> Option<Vec<u8>> {
        let actual = self.index.get(&normalize(sro_path))?;
        std::fs::read(actual).ok()
    }

    fn list_bsr(&self, prefix: &str) -> Vec<String> {
        let prefix = normalize(prefix);
        let mut paths: Vec<String> = self
            .index
            .keys()
            .filter(|key| key.starts_with(&prefix) && key.ends_with(".bsr"))
            .cloned()
            .collect();
        paths.sort();
        paths
    }
}
