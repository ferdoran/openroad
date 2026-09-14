use std::fs;
use std::path::{Path, PathBuf};

use bevy::asset::io::AssetReader;
use bevy_pk2::prelude::Archive;
use clap::{App, Arg};
use futures_lite::future::block_on;
use futures_lite::io::AsyncReadExt;

fn main() {
    let matches = App::new("pk2_unpack")
        .version("0.1.0")
        .about("List or extract files from a Silkroad PK2 archive")
        .arg(
            Arg::with_name("pk2")
                .long("pk2")
                .short("p")
                .takes_value(true)
                .required(true)
                .help("Path to a .pk2 file"),
        )
        .arg(
            Arg::with_name("out")
                .long("out")
                .short("o")
                .takes_value(true)
                .default_value("out")
                .help("Output directory (for extraction)"),
        )
        .arg(
            Arg::with_name("list")
                .long("list")
                .help("Only list files, do not extract"),
        )
        .arg(
            Arg::with_name("extract")
                .long("extract")
                .help("Extract files (default if no mode is specified)"),
        )
        .arg(
            Arg::with_name("prefix")
                .long("prefix")
                .takes_value(true)
                .help("Only process files with this path prefix inside the archive"),
        )
        .get_matches();

    let pk2_path = PathBuf::from(matches.value_of("pk2").unwrap());
    let out_dir = PathBuf::from(matches.value_of("out").unwrap());
    let list_only = matches.is_present("list");
    let extract = matches.is_present("extract") || !list_only;
    let prefix = matches.value_of("prefix").map(PathBuf::from);

    let archive = Archive::configured(&pk2_path);
    let mut entries: Vec<_> = archive
        .root
        .get_all_entries()
        .into_iter()
        .filter(|(_, entry)| entry.is_file())
        .collect();

    entries.sort_by_key(|(path, _)| path.to_string_lossy().to_string());

    for (path, entry) in entries {
        if let Some(ref pfx) = prefix {
            // Case-insensitive, matching the archive's case-insensitive reads
            // (PK2 paths are stored in the client's original mixed casing).
            let path_lc = path.to_string_lossy().to_ascii_lowercase();
            let pfx_lc = pfx.to_string_lossy().to_ascii_lowercase();
            if !path_lc.starts_with(&pfx_lc) {
                continue;
            }
        }

        if list_only {
            println!("{} ({})", path.display(), entry.size);
            continue;
        }

        if extract {
            extract_file(&archive, &path, &out_dir);
        }
    }
}

fn extract_file(archive: &Archive, path: &Path, out_dir: &Path) {
    let mut reader = match block_on(archive.read(path)) {
        Ok(reader) => reader,
        Err(err) => {
            eprintln!("failed to read {}: {}", path.display(), err);
            return;
        }
    };

    let mut data = Vec::new();
    let read_result = block_on(async { reader.read_to_end(&mut data).await });
    if let Err(err) = read_result {
        eprintln!("failed to read bytes for {}: {}", path.display(), err);
        return;
    }

    let out_path = out_dir.join(path);
    if let Some(parent) = out_path.parent() {
        if let Err(err) = fs::create_dir_all(parent) {
            eprintln!("failed to create {}: {}", parent.display(), err);
            return;
        }
    }

    if let Err(err) = fs::write(&out_path, data) {
        eprintln!("failed to write {}: {}", out_path.display(), err);
    }
}
