// bsr2glb: converts Silkroad .bsr resources — including their .bms meshes,
// .bmt materials, .ddj textures, .bsk skeleton and .ban animations — into
// self-contained binary glTF (.glb) and/or binary FBX 7.4 (.fbx) files,
// viewable in Blender or any glTF/FBX-capable tool. Reuses the client
// crate's format parsers (via its parser-only lib target); by default
// bakes the client's X-mirror + winding flip so models look like in-game
// (--raw exports as authored). Reads either straight out of Data.pk2 or
// from an extracted directory tree.

use std::path::{Path, PathBuf};
use std::process::exit;

use clap::{App, Arg};

mod bundle;
mod fbx;
mod gltf;
mod source;
mod texture;

use source::{normalize, DirSource, Pk2Source, Source};

#[derive(Clone, Copy, PartialEq)]
enum Format {
    Glb,
    Fbx,
}

impl Format {
    fn extension(self) -> &'static str {
        match self {
            Format::Glb => "glb",
            Format::Fbx => "fbx",
        }
    }
}

fn main() {
    let matches = App::new("bsr2glb")
        .version("0.1.0")
        .about("Convert Silkroad .bsr resources (meshes, materials, textures, skeleton, animations) to binary glTF (.glb) and/or FBX 7.4 (.fbx)")
        .arg(
            Arg::with_name("bsr")
                .long("bsr")
                .short("b")
                .takes_value(true)
                .help("pk2-internal path of the .bsr to convert (backslash or slash separators)"),
        )
        .arg(
            Arg::with_name("pk2")
                .long("pk2")
                .short("p")
                .takes_value(true)
                .help("Path to Data.pk2 (default: $SRO_PK2_PATH or $SRO_PATH + /Data.pk2)"),
        )
        .arg(
            Arg::with_name("dir")
                .long("dir")
                .short("d")
                .takes_value(true)
                .conflicts_with("pk2")
                .help("Read from an extracted data directory tree instead of a PK2"),
        )
        .arg(
            Arg::with_name("out")
                .long("out")
                .short("o")
                .takes_value(true)
                .help("Output file (single mode; .fbx extension selects FBX) or directory (batch mode)"),
        )
        .arg(
            Arg::with_name("prefix")
                .long("prefix")
                .takes_value(true)
                .conflicts_with("bsr")
                .help("Batch mode: convert every .bsr under this pk2 path prefix"),
        )
        .arg(
            Arg::with_name("raw")
                .long("raw")
                .help("Export as authored (skip the in-game X-mirror and winding flip)"),
        )
        .arg(
            Arg::with_name("format")
                .long("format")
                .short("f")
                .takes_value(true)
                .possible_values(&["glb", "fbx", "both"])
                .help("Output format (default: inferred from --out extension, else glb)"),
        )
        .get_matches();

    let mirror = !matches.is_present("raw");
    let formats = parse_formats(&matches);

    let source: Box<dyn Source> = if let Some(dir) = matches.value_of("dir") {
        Box::new(DirSource::open(Path::new(dir)))
    } else {
        let pk2_path = matches
            .value_of("pk2")
            .map(PathBuf::from)
            .unwrap_or_else(default_data_pk2);
        if !pk2_path.exists() {
            eprintln!(
                "error: {} does not exist (give --pk2/--dir or set SRO_PK2_PATH/SRO_PATH)",
                pk2_path.display()
            );
            exit(1);
        }
        Box::new(Pk2Source::open(&pk2_path))
    };

    if let Some(prefix) = matches.value_of("prefix") {
        let targets = source.list_bsr(prefix);
        if targets.is_empty() {
            eprintln!("error: no .bsr files found under '{prefix}'");
            exit(1);
        }
        let out_dir = PathBuf::from(matches.value_of("out").unwrap_or("out_models"));
        let mut converted = 0usize;
        let mut failed = 0usize;
        for target in &targets {
            match convert(source.as_ref(), target, mirror, &formats) {
                Ok(outputs) => {
                    let mut ok = true;
                    for (format, bytes) in outputs {
                        let out_path =
                            out_dir.join(Path::new(target).with_extension(format.extension()));
                        if let Err(e) = write_output(&out_path, &bytes) {
                            eprintln!("error: {}: {e}", out_path.display());
                            ok = false;
                            continue;
                        }
                        println!("{target} -> {} ({} bytes)", out_path.display(), bytes.len());
                    }
                    if ok {
                        converted += 1;
                    } else {
                        failed += 1;
                    }
                }
                Err(e) => {
                    eprintln!("error: {target}: {e}");
                    failed += 1;
                }
            }
        }
        println!(
            "converted {converted}/{} .bsr files ({failed} failed)",
            targets.len()
        );
        if converted == 0 {
            exit(1);
        }
    } else if let Some(bsr) = matches.value_of("bsr") {
        let normalized = normalize(bsr);
        let stem = Path::new(&normalized)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "out".to_string());
        match convert(source.as_ref(), &normalized, mirror, &formats) {
            Ok(outputs) => {
                for (format, bytes) in outputs {
                    let out_path = match matches.value_of("out") {
                        // with multiple formats the extension always follows
                        // the format, only the stem comes from --out
                        Some(out) if formats.len() > 1 => {
                            Path::new(out).with_extension(format.extension())
                        }
                        Some(out) => PathBuf::from(out),
                        None => PathBuf::from(format!("{stem}.{}", format.extension())),
                    };
                    if let Err(e) = write_output(&out_path, &bytes) {
                        eprintln!("error: {}: {e}", out_path.display());
                        exit(1);
                    }
                    println!("{bsr} -> {} ({} bytes)", out_path.display(), bytes.len());
                }
            }
            Err(e) => {
                eprintln!("error: {bsr}: {e}");
                exit(1);
            }
        }
    } else {
        eprintln!("error: either --bsr <path> or --prefix <path> is required");
        exit(1);
    }
}

/// `--format` wins; otherwise the `--out` extension decides, defaulting to glb.
fn parse_formats(matches: &clap::ArgMatches) -> Vec<Format> {
    match matches.value_of("format") {
        Some("glb") => vec![Format::Glb],
        Some("fbx") => vec![Format::Fbx],
        Some("both") => vec![Format::Glb, Format::Fbx],
        Some(_) => unreachable!("clap validates possible_values"),
        None => {
            let from_out = matches
                .value_of("out")
                .and_then(|out| Path::new(out).extension())
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.eq_ignore_ascii_case("fbx"));
            match from_out {
                Some(true) => vec![Format::Fbx],
                _ => vec![Format::Glb],
            }
        }
    }
}

/// `$SRO_PK2_PATH` -> `$SRO_PATH` -> `<cwd>/assets`, joined with Data.pk2
/// (the same resolution order the client uses).
fn default_data_pk2() -> PathBuf {
    std::env::var_os("SRO_PK2_PATH")
        .or_else(|| std::env::var_os("SRO_PATH"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("assets"))
        .join("Data.pk2")
}

/// Converts one .bsr into the requested formats, loading and parsing the
/// resource only once. The SRO parsers trust offsets/counts and panic on
/// malformed input, so the whole conversion runs behind catch_unwind — in
/// batch mode one broken file must not abort the run.
fn convert(
    source: &dyn Source,
    bsr_path: &str,
    mirror: bool,
    formats: &[Format],
) -> Result<Vec<(Format, Vec<u8>)>, String> {
    let fallback_name = Path::new(bsr_path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "resource".to_string());
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let bundle = bundle::load(source, bsr_path, mirror)?;
        formats
            .iter()
            .map(|&format| {
                let bytes = match format {
                    Format::Glb => gltf::build_glb(&bundle, &fallback_name)?,
                    Format::Fbx => fbx::build_fbx(&bundle, &fallback_name)?,
                };
                Ok((format, bytes))
            })
            .collect()
    }))
    .unwrap_or_else(|_| Err("parser panicked (malformed file?)".to_string()))
}

fn write_output(path: &Path, glb: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(path, glb)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// End-to-end conversion against the real game data: converts one
    /// skinned character and one static building .bsr from Data.pk2 to
    /// both formats, re-imports the GLB with the `gltf` crate and
    /// re-parses the FBX with fbxcel. Needs `SRO_PK2_PATH` (same variable
    /// the client uses); skips silently otherwise so CI without game data
    /// stays green. No game data is shipped — the .bsr paths are
    /// discovered by scanning the archive.
    #[test]
    fn converts_real_bsr_files_to_valid_glb() {
        let Some(dir) = std::env::var_os("SRO_PK2_PATH") else {
            eprintln!("SRO_PK2_PATH not set; skipping bsr2glb integration test");
            return;
        };
        let data_pk2 = PathBuf::from(dir).join("Data.pk2");
        if !data_pk2.exists() {
            eprintln!("{} not found; skipping", data_pk2.display());
            return;
        }
        let source = Pk2Source::open(&data_pk2);

        let pick = |prefix: &str| source.list_bsr(prefix).into_iter().next();
        let character = pick("res/char/");
        let building = pick("res/bldg/");
        assert!(
            character.is_some() || building.is_some(),
            "no .bsr found under res/char/ or res/bldg/"
        );

        for target in [character, building].into_iter().flatten() {
            let outputs = convert(&source, &target, true, &[Format::Glb, Format::Fbx])
                .unwrap_or_else(|e| panic!("{target}: conversion failed: {e}"));
            let glb_bytes = &outputs[0].1;
            check_fbx(&target, &outputs[1].1);
            let (document, buffers, _images) = ::gltf::import_slice(glb_bytes)
                .unwrap_or_else(|e| panic!("{target}: glTF re-import failed: {e}"));

            assert!(document.meshes().len() > 0, "{target}: no meshes exported");
            let bin = &buffers[0];
            for accessor in document.accessors() {
                let view = accessor.view().expect("no sparse accessors are emitted");
                let end = view.offset() + view.length();
                assert!(
                    end <= bin.len(),
                    "{target}: accessor view [{}..{end}] outside BIN chunk ({} bytes)",
                    view.offset(),
                    bin.len()
                );
            }
            for skin in document.skins() {
                let ibm = skin
                    .inverse_bind_matrices()
                    .expect("skins are emitted with IBMs");
                assert_eq!(
                    ibm.count(),
                    skin.joints().count(),
                    "{target}: IBM count != joint count"
                );
            }
            for animation in document.animations() {
                for channel in animation.channels() {
                    let input = channel.sampler().input();
                    assert!(
                        input.min().is_some() && input.max().is_some(),
                        "{target}: animation sampler input lacks min/max"
                    );
                }
            }
            println!(
                "{target}: {} meshes, {} skins, {} animations, {} bytes",
                document.meshes().len(),
                document.skins().len(),
                document.animations().len(),
                glb_bytes.len()
            );
        }
    }

    /// Re-parses a produced FBX with fbxcel and checks the object graph:
    /// Objects/Connections exist, every connection endpoint is a known
    /// object id (or the document root 0).
    fn check_fbx(target: &str, bytes: &[u8]) {
        use fbxcel::low::v7400::AttributeValue;

        let tree =
            fbxcel::tree::any::AnyTree::from_seekable_reader(std::io::Cursor::new(bytes.to_vec()))
                .unwrap_or_else(|e| panic!("{target}: FBX re-parse failed: {e}"));
        let fbxcel::tree::any::AnyTree::V7400(version, tree, _) = tree else {
            panic!("{target}: unexpected FBX tree version");
        };
        assert_eq!(
            version,
            fbxcel::low::FbxVersion::V7_4,
            "{target}: wrong FBX version"
        );

        let objects = tree
            .root()
            .first_child_by_name("Objects")
            .unwrap_or_else(|| panic!("{target}: no Objects node"));
        let mut ids = std::collections::HashSet::new();
        ids.insert(0i64);
        let mut geometry_count = 0usize;
        for object in objects.children() {
            if let Some(AttributeValue::I64(id)) = object.attributes().first() {
                ids.insert(*id);
            }
            if object.name() == "Geometry" {
                geometry_count += 1;
            }
        }
        assert!(geometry_count > 0, "{target}: no FBX geometry exported");

        let connections = tree
            .root()
            .first_child_by_name("Connections")
            .unwrap_or_else(|| panic!("{target}: no Connections node"));
        for connection in connections.children_by_name("C") {
            let attrs = connection.attributes();
            let (Some(AttributeValue::I64(child)), Some(AttributeValue::I64(parent))) =
                (attrs.get(1), attrs.get(2))
            else {
                panic!("{target}: malformed connection {attrs:?}");
            };
            assert!(
                ids.contains(child) && ids.contains(parent),
                "{target}: connection references unknown id {child}->{parent}"
            );
        }
        println!(
            "{target}: FBX ok ({geometry_count} geometries, {} bytes)",
            bytes.len()
        );
    }
}
