use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::File;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bevy::asset::io::{AssetReader, AssetReaderError, PathStream, Reader, VecReader};
use bevy::prelude::{debug, error, info};
use bevy::reflect::TypePath;

use crate::pk2::blowfish::Blowfish;
use crate::pk2::constants::HEADER_SIZE;
use crate::pk2::directory::Directory;
use crate::pk2::entry::Entry;
use crate::pk2::errors::Error;
use crate::pk2::header::Header;
use crate::pk2::key::Pk2Key;
use crate::pk2::read_stats;

/// A structure to access an SRO PK2 archive.
#[derive(TypePath, Clone)]
#[allow(dead_code)]
pub struct Archive {
    /// Shared without a lock: after indexing, all reads are positioned
    /// (`read_exact_at`), so concurrent asset loads from one archive don't
    /// serialize on a mutex around a shared file cursor.
    file: Arc<File>,
    blowfish: Blowfish,
    strip_prefix: OsString,
    pub root: Directory,
    entries: HashMap<PathBuf, Entry>,
}

/// Reads `buf.len()` bytes at `offset` without touching a shared cursor, so
/// concurrent readers of the same [`File`] need no synchronization.
#[cfg(unix)]
fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
    use std::os::unix::fs::FileExt;
    file.read_exact_at(buf, offset)
}

/// Windows has no true positional read; `seek_read` moves the file cursor,
/// which is fine here because nothing reads through the cursor after the
/// archive index is built.
#[cfg(windows)]
fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    let mut pos = 0;
    while pos < buf.len() {
        let read = file.seek_read(&mut buf[pos..], offset + pos as u64)?;
        if read == 0 {
            return Err(std::io::Error::new(
                ErrorKind::UnexpectedEof,
                "failed to fill whole buffer",
            ));
        }
        pos += read;
    }
    Ok(())
}

/// PK2 archive lookups are case-insensitive — the original Windows client
/// relies on it: textdata routinely references files in a different case than
/// the archive stores them (e.g. `char\china\chinaman_adventurer.bsr` for the
/// stored `Res/Char/China/chinaman_Adventurer.bsr`). Lowercasing both the
/// index keys and every query lets a mixed-case archive (the official-casing
/// PK2s) resolve the client's lowercase asset paths, while an already
/// all-lowercase archive (the normalized vSRO assets) is unaffected.
fn normalize(path: &Path) -> PathBuf {
    PathBuf::from(path.to_string_lossy().to_ascii_lowercase())
}

impl Archive {
    /// Open and index an archive, surfacing every failure as an [`Error`]
    /// rather than a panic. The archive is our trust boundary — SRO drops are
    /// treated as potentially hostile — so a malformed header, a short read or
    /// a cyclic block chain must be reportable, not fatal.
    pub fn open<P: AsRef<Path>>(path: P, key: &Pk2Key) -> Result<Archive, Error> {
        let path = path.as_ref();
        let blowfish = Blowfish::from_key(key)
            .map_err(|_| Error::InvalidHeader("PK2 key rejected by the cipher (bad length?)"))?;
        let mut header_buf: [u8; HEADER_SIZE] = [0; HEADER_SIZE];
        let mut file = File::open(path).map_err(Error::IO)?;
        file.read_exact(&mut header_buf)
            .map_err(|_| Error::InvalidHeader("header shorter than 256 bytes"))?;
        let header = Header::from(header_buf);
        header.verify(&blowfish)?;
        let root = Directory::index(&mut file, &blowfish)?;
        let entries = root
            .get_all_entries()
            .into_iter()
            .map(|(k, v)| (normalize(&k), v))
            .collect();
        Ok(Archive {
            file: Arc::new(file),
            blowfish,
            strip_prefix: "".into(),
            root,
            entries,
        })
    }
}

impl Archive {
    /// Open an archive the caller can survive without: the failure is reported
    /// in one readable line and `None` comes back.
    ///
    /// A client that is handed a data
    /// folder with only some of the archives in it should say which archive is
    /// missing and run with what it has: a panic here happens before the window
    /// exists, so the user would only see a backtrace naming an `unwrap`.
    pub fn open_or_report<P: AsRef<Path>>(path: P, key: &Pk2Key) -> Option<Self> {
        let path = path.as_ref();
        match Archive::open(path, key) {
            Ok(archive) => Some(archive),
            Err(err) => {
                // Both channels on purpose: this runs during plugin build, so
                // the tracing subscriber may not be installed yet.
                eprintln!("archive {} unavailable - {err}", path.display());
                error!("archive {} unavailable - {}", path.display(), err);
                None
            }
        }
    }

    /// Open one archive with the key resolved from the environment or
    /// `config.yaml` ([`Pk2Key::resolve`]).
    ///
    /// For the one-off callers — command-line tools and tests — that open a
    /// single named archive and cannot do anything at all without it, so an
    /// absent archive is a usage error. Anything long-running (the client)
    /// uses [`Archive::open_or_report`] or [`Archive::open`].
    pub fn configured<P: AsRef<Path>>(path: P) -> Self {
        let path = path.as_ref();
        let key = Pk2Key::resolve().unwrap_or_else(|err| panic!("{err}"));
        Archive::open(path, &key)
            .unwrap_or_else(|err| panic!("archive {} unavailable - {err}", path.display()))
    }
}

impl AssetReader for Archive {
    // fn read<'a>(&'a self, path: &'a Path) -> BoxedFuture<'a, Result<Box<Reader<'a>>, AssetReaderError>> {
    //     Box::pin(async move {
    //         let stripped_path = path.strip_prefix(&self.strip_prefix).unwrap().to_path_buf();
    //
    //         return match self.entries.get(&stripped_path) {
    //             None => Err(AssetReaderError::NotFound(stripped_path)),
    //             Some(entry) => {
    //                 let file = Arc::clone(&self.file);
    //                 let mut file = file.lock().expect("failed to lock cursor");
    //                 let mut buf = BytesMut::zeroed(entry.size as usize);
    //                 file.seek(SeekFrom::Start(entry.position))?;
    //                 let read_bytes = file.read(&mut buf)?;
    //                 if read_bytes != entry.size as usize {
    //                     warn!("expected {} bytes but read {}", entry.size, read_bytes);
    //                 }
    //
    //                 let buf = buf.freeze();
    //                 Ok(Box::new(buf.reader()))
    //             }
    //         }
    //     })
    // }

    async fn read<'a>(&'a self, path: &'a Path) -> Result<Box<dyn Reader>, AssetReaderError> {
        let stripped_path = normalize(path);
        debug!("reading path: {}", stripped_path.display());
        match self.entries.get(&stripped_path) {
            None => Err(AssetReaderError::NotFound(stripped_path)),
            Some(entry) => {
                // Positioned read straight into the buffer handed to the
                // reader: no lock (a burst of loads from one archive used to
                // serialize on it) and no intermediate copy of the bytes.
                let mut bytes = vec![0u8; entry.size as usize];
                read_exact_at(&self.file, &mut bytes, entry.position)?;
                // #434 step 1: count what a cache would have served. Inert
                // unless PK2_READ_STATS is set.
                read_stats::record(&stripped_path, entry.size as u64);
                let reader: Box<dyn Reader> = Box::new(VecReader::new(bytes));
                Ok(reader)
            }
        }
    }

    // fn get_metadata(&self, path: &Path) -> Result<Metadata, AssetReaderError> {
    //     let stripped_path = path.strip_prefix(&self.strip_prefix).unwrap().to_path_buf();
    //
    //     match self.entries.get(&stripped_path) {
    //         None => Err(AssetReaderError::NotFound(stripped_path)),
    //         Some(entry) => Ok(Metadata::from(entry))
    //     }
    // }
    //
    // fn watch_path_for_changes(&self, to_watch: &Path, to_reload: Option<PathBuf>) -> Result<(), AssetReaderError> {
    //     debug!("watch_path_for_changes not implemented");
    //     Ok(())
    // }
    //
    // fn watch_for_changes(&self, configuration: &ChangeWatcher) -> Result<(), AssetReaderError> {
    //     debug!("watch_for_changes not implemented");
    //     Ok(())
    // }

    async fn read_meta<'a>(&'a self, path: &'a Path) -> Result<Box<dyn Reader>, AssetReaderError> {
        debug!("reading meta path: {}", path.display());
        Err(AssetReaderError::NotFound(path.to_path_buf()))
        // let stripped_path = path.strip_prefix(&self.strip_prefix).unwrap().to_path_buf();
        //
        //     match self.entries.get(&stripped_path) {
        //         None => Err(AssetReaderError::NotFound(stripped_path)),
        //         Some(entry) => Ok(Metadata::from(entry))
        //     }
    }

    async fn read_directory<'a>(
        &'a self,
        path: &'a Path,
    ) -> Result<Box<PathStream>, AssetReaderError> {
        let stripped_path = normalize(path);
        debug!("reading dir path: {}", stripped_path.display());

        let entry = match self.entries.get(&stripped_path) {
            None => return Err(AssetReaderError::NotFound(path.to_path_buf())),
            Some(entry) => entry,
        };

        if !entry.is_dir() {
            return Err(AssetReaderError::Io(Arc::new(std::io::Error::new(
                ErrorKind::NotADirectory,
                format!("{}", path.display()),
            ))));
        }

        // `root.directories` is keyed by the archive's original casing, so a
        // normalized (lowercase) path can miss it on a mixed-case archive;
        // fail gracefully rather than panic (directory listing isn't on the
        // load-by-path hot path).
        let dir = match self.root.directories.get(&stripped_path) {
            Some(dir) => dir,
            None => return Err(AssetReaderError::NotFound(path.to_path_buf())),
        };

        let entries: Vec<PathBuf> = dir
            .entries
            .keys()
            .map(|k| {
                let mut pb = path.to_path_buf();
                pb.push(k);
                pb.to_owned()
            })
            .collect();
        let read_dir: Box<PathStream> = Box::new(futures::stream::iter(entries));
        Ok(read_dir)
    }

    async fn is_directory<'a>(&'a self, path: &'a Path) -> Result<bool, AssetReaderError> {
        let stripped_path = normalize(path);
        debug!("is dir path: {}", stripped_path.display());

        let entry = match self.entries.get(&stripped_path) {
            None => return Err(AssetReaderError::NotFound(path.to_path_buf())),
            Some(entry) => entry,
        };

        Ok(entry.is_dir())
    }
}

impl Archive {
    /// Synchronously reads a contained file's bytes, for callers outside the
    /// asset system (e.g. test-scene tooling reading textdata directly).
    pub fn read_file_bytes(&self, path: &Path) -> Option<Vec<u8>> {
        let entry = self.entries.get(&normalize(path))?;
        if !entry.is_file() {
            return None;
        }
        let mut buf = vec![0u8; entry.size as usize];
        read_exact_at(&self.file, &mut buf, entry.position).ok()?;
        read_stats::record(&normalize(path), entry.size as u64);
        Some(buf)
    }

    pub fn print_all_entries(&self) {
        self.entries.iter().for_each(|(k, _v)| {
            info!("Entry: {}", k.display());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (d) A data folder missing one archive must be reportable, not fatal.
    /// An `unwrap` here dies before the window exists, so a user with only some
    /// of the archives would see a backtrace.
    #[test]
    fn a_missing_archive_is_an_error_not_a_panic() {
        let key = Pk2Key::from_config("testkey", "00112233445566778899").unwrap();
        let missing = std::env::temp_dir().join("openroad-no-such-archive.pk2");
        let Err(err) = Archive::open(&missing, &key) else {
            panic!("a missing file cannot open");
        };
        assert!(matches!(err, Error::IO(_)), "want IO error, got {err:?}");
        // and the message a user gets is a sentence, not a Debug dump
        assert_eq!(err.to_string(), "file not found");
        assert!(
            Archive::open_or_report(&missing, &key).is_none(),
            "reporting open yields None instead of unwinding"
        );
    }
}
