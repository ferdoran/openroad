//! `pk2_probe` — corpus histogram harness for PK2 enum/flag fields.
//!
//! Idea: the repo rule is corpus-verify-before-you-code, but every probe so far
//! has been a throwaway script. This walks a user's PK2s through the client's
//! own parsers and prints a `value -> count` histogram for one named field, so
//! a claim like "flag is only ever 0 or 1" becomes a citable number instead of
//! an impression.
//!
//! Read-only by construction: it opens archives through `bevy_pk2`, never
//! writes, never executes anything it reads, and never touches the network.
//!
//! Field coverage is a small registry rather than reflection — each entry names
//! where its semantics come from, so adding a field is a two-line change next
//! to its citation.
//!
//! ```text
//! pk2_probe --pk2 /path/Media.pk2 --field bms.vertex_flag
//! pk2_probe --pk2 /path/Data.pk2  --field itemdata.TypeId1 --format json
//! pk2_probe --corpus /path/pk2dir --field ddj.D3dResourceType --prefix data/
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bevy_pk2::prelude::Archive;
use bytes::Bytes;
use client::assets::ban::JMXVBAN;
use client::assets::ddj::D3dResourceType;
use client::assets::textdata::decode::decode_textdata;

/// A histogram bucket key. Values are rendered as decimal, plus hex for the
/// bit-flag fields where that is the readable form.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Bucket {
    Int(i64),
    Text(String),
}

impl std::fmt::Display for Bucket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Bucket::Int(v) => write!(f, "{v}"),
            Bucket::Text(s) => write!(f, "{s}"),
        }
    }
}

/// How a field is extracted from one archive entry.
enum Extract {
    /// Binary format: parse the file's bytes, yield zero or more values.
    Binary {
        /// Only files whose normalized path ends with this are read.
        extension: &'static str,
        extract: fn(&[u8]) -> Vec<Bucket>,
    },
    /// Textdata table: decode the file and read one tab-separated column.
    ///
    /// The column index is the semantic knowledge, and it is cited per field.
    /// The row parsers themselves live behind an async `AssetLoader` and a
    /// `pub(crate)` row type, so they are not constructible from a binary —
    /// see the module docs on `client/src/assets/textdata/mod.rs`.
    TextColumn {
        /// Matched against the entry's file name.
        file_stem: &'static str,
        column: usize,
        /// Rows shorter than this are skipped (the loader's own guard).
        min_columns: usize,
    },
}

struct Field {
    name: &'static str,
    /// Where the field's meaning is defined — printed with the histogram so a
    /// pasted result carries its own citation.
    cite: &'static str,
    extract: Extract,
}

/// The seed registry. Every entry cites the code or column that defines it.
const FIELDS: &[Field] = &[
    Field {
        name: "bms.vertex_flag",
        cite: "client/src/assets/bms/header.rs:20 (consumed by the parse; not retained on JMXVBMS)",
        extract: Extract::Binary {
            extension: ".bms",
            extract: bms_vertex_flag,
        },
    },
    Field {
        name: "ddj.D3dResourceType",
        cite: "client/src/assets/ddj.rs:850-858 (D3DRESOURCETYPE)",
        extract: Extract::Binary {
            extension: ".ddj",
            extract: ddj_resource_type,
        },
    },
    Field {
        name: "ban.AnimationType",
        cite: "client/src/assets/ban.rs:36-39 (0 = OneShot, 1 = Cyclic)",
        extract: Extract::Binary {
            extension: ".ban",
            extract: ban_animation_type,
        },
    },
    Field {
        name: "itemdata.TypeId1",
        cite: "client/src/assets/textdata/itemdata.rs:16 (col 9)",
        extract: Extract::TextColumn {
            file_stem: "itemdata",
            column: 9,
            min_columns: 11,
        },
    },
    Field {
        name: "itemdata.TypeId2",
        cite: "client/src/assets/textdata/itemdata.rs:17 (col 10)",
        extract: Extract::TextColumn {
            file_stem: "itemdata",
            column: 10,
            min_columns: 11,
        },
    },
    Field {
        name: "itemdata.TypeId3",
        cite: "client/src/assets/textdata/itemdata.rs:18 (col 11)",
        extract: Extract::TextColumn {
            file_stem: "itemdata",
            column: 11,
            min_columns: 12,
        },
    },
    Field {
        name: "itemdata.TypeId4",
        cite: "client/src/assets/textdata/itemdata.rs:19 (col 12)",
        extract: Extract::TextColumn {
            file_stem: "itemdata",
            column: 12,
            min_columns: 13,
        },
    },
    Field {
        name: "itemdata.Gender",
        cite: "itemdata col 58",
        extract: Extract::TextColumn {
            file_stem: "itemdata",
            column: 58,
            min_columns: 59,
        },
    },
    Field {
        name: "itemdata.ItemClass",
        cite: "itemdata col 61",
        extract: Extract::TextColumn {
            file_stem: "itemdata",
            column: 61,
            min_columns: 62,
        },
    },
    Field {
        name: "skilldata.Activity",
        cite: "client/src/assets/textdata/skilldata.rs:30 (col 8)",
        extract: Extract::TextColumn {
            file_stem: "skilldata",
            column: 8,
            min_columns: 9,
        },
    },
];

/// SRO containers all open with a 12-byte `JMXVxxx nnnn` magic. Checking it
/// keeps mislabelled files out of the histogram — `res_ui/nifenchantwnd.ddj`
/// is a 2DT window definition wearing a `.ddj` extension (`ddj.rs:40-52`).
fn has_signature(bytes: &[u8], signature: &str) -> bool {
    bytes.len() > signature.len() && &bytes[..signature.len()] == signature.as_bytes()
}

/// `vertex_flag` lives on the BMS header, which the mesh parse consumes without
/// retaining, so it is read from the raw header rather than from `JMXVBMS`.
fn bms_vertex_flag(bytes: &[u8]) -> Vec<Bucket> {
    // Family prefix, not a pinned version: `parse_bms` seeks past the 12-byte
    // signature without reading it (`bms/mod.rs:52-58`), and the corpus ships
    // both `JMXVBMS 0110` and `JMXVBMS 0109`, which share this header layout.
    if !has_signature(bytes, "JMXVBMS ") {
        return Vec::new();
    }
    // "JMXVBMS 1000" signature (12), then the header's u32 run. `vertex_flag`
    // is the 14th word (header.rs:7-20: vertex_offset .. sub_prime_count are
    // the 13 before it), so index 13.
    const SIGNATURE_LEN: usize = 12;
    const VERTEX_FLAG_WORD_INDEX: usize = 13;
    const FLAG_OFFSET: usize = SIGNATURE_LEN + VERTEX_FLAG_WORD_INDEX * 4;
    let Some(word) = bytes.get(FLAG_OFFSET..FLAG_OFFSET + 4) else {
        return Vec::new();
    };
    let flag = u32::from_le_bytes(word.try_into().expect("4 bytes"));
    vec![Bucket::Text(format!("0x{flag:08X}"))]
}

/// `texture_type` is a private field of `JMXVDDJ`, so the raw i32 is read from
/// the container header and mapped through the client's own enum — the
/// semantics stay in one place (`ddj.rs:850-858`) rather than being restated.
fn ddj_resource_type(bytes: &[u8]) -> Vec<Bucket> {
    if !has_signature(bytes, "JMXVDDJ 1000") {
        return Vec::new();
    }
    // signature (12) + texture_buffer_size (4), then the resource type.
    let Some(word) = bytes.get(16..20) else {
        return Vec::new();
    };
    let raw = i32::from_le_bytes(word.try_into().expect("4 bytes"));
    let bucket = match D3dResourceType::try_from(raw) {
        Ok(t) => Bucket::Text(format!("{t:?}")),
        Err(_) => Bucket::Text(format!("unknown({raw})")),
    };
    vec![bucket]
}

/// Runs the real `JMXVBAN` parse. The field sits behind a variable-length name
/// string, so there is no fixed offset to read it from.
fn ban_animation_type(bytes: &[u8]) -> Vec<Bucket> {
    // The loader accepts exactly this version (`ban.rs:110`).
    if !has_signature(bytes, "JMXVBAN 0102") {
        return Vec::new();
    }
    let mut buf = Bytes::copy_from_slice(&bytes[12..]);
    let animation = JMXVBAN::from(&mut buf);
    vec![Bucket::Text(format!("{:?}", animation.animation_type))]
}

/// Split a decoded textdata file into its tab-separated rows.
fn text_rows(content: &str) -> impl Iterator<Item = Vec<&str>> {
    content
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .map(|l| l.split('\t').collect::<Vec<_>>())
}

fn histogram_text_column(
    content: &str,
    column: usize,
    min_columns: usize,
    counts: &mut BTreeMap<Bucket, u64>,
    rows: &mut u64,
) {
    for cols in text_rows(content) {
        if cols.len() < min_columns {
            continue;
        }
        let Some(raw) = cols.get(column) else {
            continue;
        };
        let raw = raw.trim();
        *rows += 1;
        let bucket = match raw.parse::<i64>() {
            Ok(v) => Bucket::Int(v),
            Err(_) => Bucket::Text(raw.to_string()),
        };
        *counts.entry(bucket).or_insert(0) += 1;
    }
}

/// Resolve a master list into the archive keys of the shards it names.
///
/// The client reaches these tables through the same indirection
/// (`client/src/assets/textdata/mod.rs:138-160`: a master `skilldata.txt`
/// names the `skilldata_5000.txt` shards). Globbing the file stem instead
/// would also match the encrypted `*enc.txt` siblings, which the master list
/// omits and the client never reads.
fn shard_keys(master_key: &str, listing: &str) -> Vec<String> {
    let dir = master_key.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    listing
        .lines()
        .map(|l| l.trim().trim_start_matches('\u{feff}'))
        .filter(|l| !l.is_empty())
        .map(|name| format!("{dir}/{}", normalize(name)))
        .collect()
}

struct Args {
    pk2: Vec<PathBuf>,
    field: String,
    prefix: Option<String>,
    json: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut pk2 = Vec::new();
    let mut field = None;
    let mut prefix = None;
    let mut json = false;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--pk2" => pk2.push(PathBuf::from(it.next().ok_or("--pk2 needs a path")?)),
            "--corpus" => {
                let dir = PathBuf::from(it.next().ok_or("--corpus needs a directory")?);
                let entries =
                    std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path
                        .extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("pk2"))
                    {
                        pk2.push(path);
                    }
                }
            }
            "--field" => field = Some(it.next().ok_or("--field needs a name")?),
            "--prefix" => prefix = Some(it.next().ok_or("--prefix needs a value")?),
            "--format" => {
                json = it.next().ok_or("--format needs text|json")? == "json";
            }
            "-h" | "--help" => return Err(usage()),
            other => return Err(format!("unknown argument {other}\n\n{}", usage())),
        }
    }
    if pk2.is_empty() {
        return Err(format!("no archives given\n\n{}", usage()));
    }
    Ok(Args {
        pk2,
        field: field.ok_or_else(|| format!("--field is required\n\n{}", usage()))?,
        prefix,
        json,
    })
}

fn usage() -> String {
    let mut s = String::from(
        "pk2_probe --pk2 <file>... | --corpus <dir>  --field <name>  [--prefix <p>] \
         [--format text|json]\n\nfields:\n",
    );
    for f in FIELDS {
        s.push_str(&format!("  {:<24} {}\n", f.name, f.cite));
    }
    s
}

fn normalize(path: &str) -> String {
    path.replace('\\', "/").to_ascii_lowercase()
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    let Some(field) = FIELDS.iter().find(|f| f.name == args.field) else {
        eprintln!("unknown field {}\n\n{}", args.field, usage());
        std::process::exit(2);
    };

    let mut counts: BTreeMap<Bucket, u64> = BTreeMap::new();
    let mut rows = 0u64;
    let mut files = 0u64;
    let mut entries_seen = 0u64;
    let mut skipped = 0u64;
    let mut sample: Vec<String> = Vec::new();

    // The format parsers panic on malformed input by design, and a corpus
    // holds mislabelled files (`ddj.rs:40-52`). Without this, one bad file
    // aborts a 22k-file walk and prints nothing at all; the count is reported
    // so a skip can never be mistaken for a clean run.
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));

    for archive_path in &args.pk2 {
        let archive = Archive::configured(archive_path.as_path());
        // Normalized key -> real entry path, so shard names read out of a
        // master list resolve without relying on the reader's own path
        // normalization.
        let mut index: BTreeMap<String, PathBuf> = BTreeMap::new();
        for (path, entry) in archive.root.get_all_entries() {
            if !entry.is_file() {
                continue;
            }
            entries_seen += 1;
            let key = normalize(&path.to_string_lossy());
            if sample.len() < 5 {
                sample.push(key.clone());
            }
            if let Some(prefix) = &args.prefix {
                if !key.starts_with(&normalize(prefix)) {
                    continue;
                }
            }
            index.insert(key, path);
        }

        match &field.extract {
            Extract::Binary { extension, extract } => {
                for (key, path) in &index {
                    if !key.ends_with(extension) {
                        continue;
                    }
                    let Some(bytes) = archive.read_file_bytes(path) else {
                        continue;
                    };
                    let parsed =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| extract(&bytes)));
                    let Ok(buckets) = parsed else {
                        skipped += 1;
                        continue;
                    };
                    files += 1;
                    for bucket in buckets {
                        rows += 1;
                        *counts.entry(bucket).or_insert(0) += 1;
                    }
                }
            }
            Extract::TextColumn {
                file_stem,
                column,
                min_columns,
            } => {
                let master = index.iter().find(|(key, _)| {
                    Path::new(key.as_str()).file_stem().and_then(|s| s.to_str()) == Some(*file_stem)
                });
                let Some((master_key, master_path)) = master else {
                    continue;
                };
                let Some(bytes) = archive.read_file_bytes(master_path) else {
                    continue;
                };
                let listing = decode_textdata(&bytes);
                for shard_key in shard_keys(master_key, &listing) {
                    let Some(shard) = index.get(&shard_key) else {
                        eprintln!("warning: {master_key} lists {shard_key}, not in the archive");
                        continue;
                    };
                    let Some(bytes) = archive.read_file_bytes(shard) else {
                        continue;
                    };
                    files += 1;
                    let content = decode_textdata(&bytes);
                    histogram_text_column(&content, *column, *min_columns, &mut counts, &mut rows);
                }
            }
        }
    }

    std::panic::set_hook(previous_hook);
    if skipped > 0 {
        eprintln!("warning: {skipped} file(s) panicked during parse and were skipped");
    }

    if args.json {
        let body = counts
            .iter()
            .map(|(k, v)| format!("    {{ \"value\": \"{k}\", \"count\": {v} }}"))
            .collect::<Vec<_>>()
            .join(",\n");
        println!("{{\n  \"field\": \"{}\",", field.name);
        println!("  \"cite\": \"{}\",", field.cite);
        println!("  \"files\": {files},\n  \"rows\": {rows},\n  \"skipped\": {skipped},");
        println!("  \"histogram\": [\n{body}\n  ]\n}}");
    } else {
        println!("field: {}  ({})", field.name, field.cite);
        println!("files: {files}   rows: {rows}");
        for (value, count) in &counts {
            let pct = if rows > 0 {
                *count as f64 * 100.0 / rows as f64
            } else {
                0.0
            };
            println!("  {value:>12} -> {count:>9}  ({pct:5.2}%)");
        }
        if counts.is_empty() {
            // A probe that silently prints nothing is worse than useless, so
            // say what was walked and show a few real keys to compare against.
            println!("  (no rows matched)");
            println!("  scanned {entries_seen} entries; sample paths:");
            for key in &sample {
                println!("    {key}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In-memory fixture, matching the crate convention of byte-array tests
    /// that need no real PK2.
    /// Both shipped versions parse; a mislabelled file contributes nothing.
    #[test]
    fn bms_accepts_every_version_but_not_a_foreign_file() {
        let body = [0u8; 14 * 4];
        for version in [&b"JMXVBMS 0110"[..], &b"JMXVBMS 0109"[..]] {
            let mut bytes = version.to_vec();
            bytes.extend_from_slice(&body);
            assert_eq!(bms_vertex_flag(&bytes).len(), 1, "version {version:?}");
        }
        let mut foreign = b"JMXVBSK 0101".to_vec();
        foreign.extend_from_slice(&body);
        assert!(bms_vertex_flag(&foreign).is_empty());
    }

    /// A `.ddj` that is really a 2DT window definition must not reach the
    /// histogram — `res_ui/nifenchantwnd.ddj` is exactly that file.
    #[test]
    fn a_mislabelled_ddj_yields_nothing() {
        let mut fake = vec![0x48, 0x00, 0x00, 0x00];
        fake.extend_from_slice(b"CNIFEnchantWnd\0\0");
        assert!(ddj_resource_type(&fake).is_empty());

        let mut real = b"JMXVDDJ 1000".to_vec();
        real.extend_from_slice(&0i32.to_le_bytes());
        real.extend_from_slice(&3i32.to_le_bytes());
        assert_eq!(
            ddj_resource_type(&real),
            vec![Bucket::Text("Texture".into())]
        );
    }

    #[test]
    fn bms_vertex_flag_reads_the_header_word() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"JMXVBMS 0110");
        bytes.extend_from_slice(&[0u8; 13 * 4]); // the 13 words before vertex_flag
        bytes.extend_from_slice(&0x0000_0410u32.to_le_bytes());
        assert_eq!(
            bms_vertex_flag(&bytes),
            vec![Bucket::Text("0x00000410".into())]
        );
    }

    /// A truncated file must be skipped, not panic — corpus tooling meets
    /// malformed input by definition.
    #[test]
    /// The encrypted `*enc.txt` siblings sit right next to the real shards and
    /// a file-stem glob swallows them, which produced mojibake buckets.
    #[test]
    fn the_master_list_excludes_the_encrypted_siblings() {
        let listing = "\u{feff}SkillData_5000.txt\r\nSkillData_10000.txt\n\n";
        let keys = shard_keys("server_dep/silkroad/textdata/skilldata.txt", listing);
        assert_eq!(
            keys,
            vec![
                "server_dep/silkroad/textdata/skilldata_5000.txt",
                "server_dep/silkroad/textdata/skilldata_10000.txt",
            ]
        );
        assert!(!keys.iter().any(|k| k.contains("enc")));
    }

    #[test]
    fn a_short_bms_yields_nothing() {
        assert!(bms_vertex_flag(b"JMXVBMS 0110").is_empty());
        assert!(bms_vertex_flag(&[]).is_empty());
    }

    /// The column histogram counts values, skips short rows, and keeps
    /// non-numeric cells as text rather than dropping them.
    #[test]
    fn text_column_histogram_counts_and_skips() {
        let content = "//header\tcols\there\n\
                       1\t2\t3\tA\n\
                       1\t2\t3\tA\n\
                       1\t2\t3\tB\n\
                       1\t2\n";
        let mut counts = BTreeMap::new();
        let mut rows = 0;
        histogram_text_column(content, 3, 4, &mut counts, &mut rows);
        assert_eq!(rows, 3, "the 2-column row is skipped");
        assert_eq!(counts.get(&Bucket::Text("A".into())), Some(&2));
        assert_eq!(counts.get(&Bucket::Text("B".into())), Some(&1));
    }

    /// Numeric cells bucket as integers so the output sorts numerically
    /// (1, 2, 10) rather than lexically (1, 10, 2).
    #[test]
    fn numeric_cells_sort_numerically() {
        let content = "x\t10\ny\t2\nz\t1\n";
        let mut counts = BTreeMap::new();
        let mut rows = 0;
        histogram_text_column(content, 1, 2, &mut counts, &mut rows);
        let order: Vec<_> = counts.keys().cloned().collect();
        assert_eq!(order, vec![Bucket::Int(1), Bucket::Int(2), Bucket::Int(10)]);
    }

    /// The `//` comment row that ships at the top of these tables is not data.
    #[test]
    fn the_comment_row_is_not_counted() {
        let content = "//Item\tTypeId1\n1\t7\n";
        let mut counts = BTreeMap::new();
        let mut rows = 0;
        histogram_text_column(content, 1, 2, &mut counts, &mut rows);
        assert_eq!(rows, 1);
        assert_eq!(counts.get(&Bucket::Int(7)), Some(&1));
    }

    /// Every registry entry must carry a citation — a histogram pasted into an
    /// issue is only evidence if it says where the field is defined.
    #[test]
    fn every_field_is_cited() {
        for f in FIELDS {
            assert!(!f.cite.is_empty(), "{} has no citation", f.name);
            assert!(f.name.contains('.'), "{} should be parser.Field", f.name);
        }
    }
}
