use bevy::asset::io::AssetReaderError;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Cursor;
use std::path::PathBuf;

use bevy::prelude::info;
use bytes::Bytes;

use crate::pk2::blowfish::Blowfish;
use crate::pk2::constants::{HEADER_SIZE, MAX_CHAIN_BLOCKS};
use crate::pk2::entry::Entry;
use crate::pk2::errors::Error;
use crate::pk2::util::{read_block, CursorExt};

/// Represents a directory entry in the PK2 archive.
#[derive(Clone)]
pub struct Directory {
    entry: Entry,
    pub entries: HashMap<PathBuf, Entry>,
    pub directories: HashMap<PathBuf, Directory>,
}

impl From<Entry> for Directory {
    /// Creates a [Directory] from a given [Entry].
    fn from(entry: Entry) -> Self {
        if !entry.is_dir() {
            panic!("{:?} is not a directory", entry.path_buf().as_path());
        }
        Directory {
            entry,
            entries: HashMap::new(),
            directories: HashMap::new(),
        }
    }
}

impl Directory {
    /// Expands a directory recursively. Used for indexing.
    ///
    /// The directory tree is walked with a visited-set on block positions: a
    /// hostile or corrupt archive can point a subdirectory back at an ancestor,
    /// which used to recurse until the stack died. An already-expanded position
    /// is skipped, and errors propagate instead of `unwrap`-ing.
    pub fn expand(&mut self, cursor: &mut Cursor<Bytes>, blowfish: &Blowfish) -> Result<(), Error> {
        let mut visited = HashSet::new();
        self.expand_guarded(cursor, blowfish, &mut visited)
    }

    fn expand_guarded(
        &mut self,
        cursor: &mut Cursor<Bytes>,
        blowfish: &Blowfish,
        visited: &mut HashSet<u64>,
    ) -> Result<(), Error> {
        if visited.len() >= MAX_CHAIN_BLOCKS {
            return Err(Error::ChainLoop(self.entry.position));
        }
        if !visited.insert(self.entry.position) {
            // already expanded from this block - a cycle in the tree
            return Ok(());
        }
        let entries = cursor.read_block(self.entry.position, blowfish)?;
        let mapped_entries: HashMap<PathBuf, Entry> = entries
            .iter()
            .filter(|e| !e.is_empty())
            .filter(|e| e.name[0] != 0x2E)
            .map(|entry| (entry.path_buf(), *entry))
            .collect();
        self.entries.extend(mapped_entries);

        let mut dirs: HashMap<PathBuf, Directory> = entries
            .iter()
            .filter(|e| e.is_dir())
            .filter(|e| e.name[0] != 0x2E) // 0x2E -> "."
            .map(|e| Directory::from(*e))
            .map(|d| (d.entry.path_buf(), d))
            .collect();

        for d in dirs.values_mut() {
            d.expand_guarded(cursor, blowfish, visited)?;
        }

        self.directories.extend(dirs);

        Ok(())
    }

    /// Indexes all archive entries recursively
    pub fn index(file: &mut File, blowfish: &Blowfish) -> Result<Directory, Error> {
        let entries = read_block(file, HEADER_SIZE as u64, blowfish)?;
        // 0x2E -> "."
        let mut root_dir_entry = *entries
            .iter()
            .find(|e| e.is_dir() && e.name[0] == 0x2e)
            .ok_or(Error::InvalidBlock("archive has no root directory entry"))?;
        root_dir_entry.name[0] = 0;
        let mut root_dir = Directory::from(root_dir_entry);
        root_dir.expand_from_file(file, blowfish)?;
        Ok(root_dir)
    }

    /// File-backed counterpart of [`Self::expand`], with the same cycle guard.
    pub fn expand_from_file(&mut self, file: &mut File, blowfish: &Blowfish) -> Result<(), Error> {
        let mut visited = HashSet::new();
        self.expand_from_file_guarded(file, blowfish, &mut visited)
    }

    fn expand_from_file_guarded(
        &mut self,
        file: &mut File,
        blowfish: &Blowfish,
        visited: &mut HashSet<u64>,
    ) -> Result<(), Error> {
        if visited.len() >= MAX_CHAIN_BLOCKS {
            return Err(Error::ChainLoop(self.entry.position));
        }
        if !visited.insert(self.entry.position) {
            return Ok(());
        }
        let entries = read_block(file, self.entry.position, blowfish)?;
        let path = self.entry.path_buf().clone();
        let mapped_entries: HashMap<PathBuf, Entry> = entries
            .iter()
            .filter(|e| !e.is_empty())
            .filter(|e| e.name[0] != 0x2E)
            .map(|entry| {
                let mut cloned_path = path.clone();
                cloned_path.push(entry.path_buf());
                (entry.path_buf(), *entry)
            })
            .collect();
        self.entries.extend(mapped_entries);

        let mut dirs: HashMap<PathBuf, Directory> = entries
            .iter()
            .filter(|e| e.is_dir())
            .filter(|e| e.name[0] != 0x2E) // 0x2E -> "."
            .map(|e| Directory::from(*e))
            .map(|d| (d.entry.path_buf(), d))
            .collect();

        for d in dirs.values_mut() {
            d.expand_from_file_guarded(file, blowfish, visited)?;
        }

        self.directories.extend(dirs);

        Ok(())
    }

    /// Prints out all entries of a directory recursively.
    pub fn print_entries(&self) {
        self.directories.iter().for_each(|(p, d)| {
            info!("Dir: {:?}", p.as_path());
            d.print_entries();
        });
        self.entries
            .iter()
            .filter(|(_, e)| e.is_file())
            .for_each(|(p, _)| {
                info!("File: {:?}", p.as_path());
            });
    }

    pub fn load_file(&self, path: PathBuf) -> Result<(u64, usize), AssetReaderError> {
        let p = path.clone();
        let mut components = p.iter();
        let entry_path = match components.next() {
            None => return Err(AssetReaderError::NotFound(path)),
            Some(path) => PathBuf::from(path),
        };

        let entry = self
            .entries
            .get(&entry_path)
            .ok_or(AssetReaderError::NotFound(entry_path.clone()))?;

        if entry.is_dir() {
            let dir = self
                .directories
                .get(&entry_path)
                .ok_or(AssetReaderError::NotFound(entry_path.clone()))?;
            let sub_path = PathBuf::from_iter(components);
            return dir.load_file(sub_path);
        } else if entry.is_file() {
            return Ok((entry.position, entry.size as usize));
        }

        Err(AssetReaderError::NotFound(entry_path))
    }

    pub fn get_all_entries(&self) -> HashMap<PathBuf, Entry> {
        let base_path = self.entry.path_buf();
        let mut entries = HashMap::new();

        for (entry_path, entry) in &self.entries {
            if entry.is_dir() {
                // a dir entry whose block was skipped as a cycle has no
                // expanded Directory - list the entry itself and move on
                let Some(dir) = self.directories.get(entry_path) else {
                    let mut pb = base_path.clone();
                    pb.push(entry.path_buf());
                    entries.insert(pb, *entry);
                    continue;
                };
                let sub_entries = dir.get_all_entries();
                sub_entries.iter().for_each(|(k, v)| {
                    let mut pb = base_path.clone();
                    pb.push(k);
                    entries.insert(pb, *v);
                });
                let mut pb = base_path.clone();
                pb.push(entry.path_buf());
                entries.insert(pb, *entry);
            } else if entry.is_file() {
                let mut bp = base_path.clone();
                bp.push(entry.path_buf());
                entries.insert(bp, *entry);
            }
        }

        entries
    }
}
