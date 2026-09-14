use std::io::Cursor;

use bevy::asset::{io::Reader, AssetLoader, LoadContext};
use bytes::Buf;
use thiserror::Error;

use crate::assets::bms::header::Header;
use crate::assets::bms::mesh::JMXVBMS;
use crate::assets::bms::navmesh::BmsNavMesh;
use crate::assets::bms::skeleton::MeshBones;
use crate::assets::bms::vertex::VertexData;
use crate::util::buf_ext::BufExt;

// `pub` so corpus tooling (tools/src/bin/pk2_probe) can histogram
// `vertex_flag`, which the parse consumes and JMXVBMS does not retain.
pub mod header;
pub mod mesh;
pub mod navmesh;
pub mod skeleton;
pub mod vertex;

#[derive(Default, bevy::reflect::TypePath)]
pub struct BmsLoader;

#[derive(Error, Debug)]
pub enum BmsLoaderError {
    #[error("failed to read the asset: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a JMXVBMS file")]
    BadSignature,
    #[error("file ends inside the {0} section")]
    Truncated(&'static str),
    #[error("{0} offset {1} lies past the end of the file")]
    SectionOutOfRange(&'static str, u32),
    #[error("{0} count {1} exceeds the {2} bytes left in the file")]
    ImplausibleCount(&'static str, u32, usize),
}

/// Signature version of a `.bms`. The only two values in the corpus are
/// `JMXVBMS 0109` (20 files) and `JMXVBMS 0110` (22,852).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BmsVersion {
    V0109,
    V0110,
}

impl BmsVersion {
    /// Bone influences stored per vertex in the skin section.
    ///
    /// `0109` stores four, `0110` two — the signature separates them
    /// perfectly: all 15 skinned 0109 files close the section chain only at
    /// 12 B/vertex, all 5,502 skinned 0110 files only at 6 B, with no file
    /// ambiguous under both. Reading 0109 at the 0110 stride left every second
    /// vertex with `index1 == index2 == 0xFF`, whose zero weight sum then
    /// produced NaN joint weights in `mesh.rs`.
    fn influences(self) -> usize {
        match self {
            Self::V0109 => 4,
            Self::V0110 => 2,
        }
    }
}

/// Largest `skin_end - face_offset` gap accepted as a stale offset table (see
/// [`parse_bms`]); the corpus maximum is 21.
const MAX_REPACK_DELTA: u64 = 64;

/// Fails unless `cursor` still holds `needed` bytes.
fn need(cursor: &Cursor<&[u8]>, needed: usize, what: &'static str) -> Result<(), BmsLoaderError> {
    if cursor.remaining() < needed {
        return Err(BmsLoaderError::Truncated(what));
    }
    Ok(())
}

/// Seeks to a header offset, rejecting one that points past the file. The
/// previous `cursor.seek(..).map_err(..)` could not fail: `Cursor`'s `Seek`
/// returns `Ok` for any `SeekFrom::Start`, so parsing simply continued past
/// the end and panicked in the section reader.
fn seek_section(
    cursor: &mut Cursor<&[u8]>,
    offset: u64,
    what: &'static str,
) -> Result<(), BmsLoaderError> {
    let len = cursor.get_ref().len() as u64;
    if offset > len {
        return Err(BmsLoaderError::SectionOutOfRange(what, offset as u32));
    }
    cursor.set_position(offset);
    Ok(())
}

/// Rejects a count whose records cannot fit in what is left of the file, so a
/// garbage length never reaches `Vec::with_capacity`. This is the guard that
/// matters most: a bogus face count of 4,294,967,045 asks for a ~24 GiB
/// allocation, and `handle_alloc_error` aborts the process — which, unlike a
/// panic, Bevy's per-loader `catch_unwind` cannot contain.
fn checked_count(
    cursor: &Cursor<&[u8]>,
    count: u32,
    record_len: usize,
    what: &'static str,
) -> Result<usize, BmsLoaderError> {
    let needed = (count as usize).saturating_mul(record_len);
    if needed > cursor.remaining() {
        return Err(BmsLoaderError::ImplausibleCount(
            what,
            count,
            cursor.remaining(),
        ));
    }
    Ok(count as usize)
}

impl AssetLoader for BmsLoader {
    type Asset = JMXVBMS;
    // type Asset = Mesh;
    type Settings = ();
    type Error = BmsLoaderError;
    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await?;
        parse_bms(&buf)
    }

    fn extensions(&self) -> &[&str] {
        &["bms"]
    }
}

/// Parses a `.bms` mesh.
///
/// Sections are located through the header's offset table, with one deviation:
/// 224 of the user's 22,872 `.bms` (avatar/pet content added by a repack tool,
/// reachable through `charactervisualchange.txt`) have every offset from
/// `FaceOffset` on understated by a small constant — the tool appended a bone
/// name without rewriting the table. Since the sections are laid out
/// contiguously, the skin section's true end reveals that shift, so we read
/// skin first and re-base the later offsets on it. The correction is a no-op
/// on all 22,648 well-formed files by construction (`delta == 0` there), and
/// is capped so a genuinely corrupt table cannot steer the parse.
pub fn parse_bms(bytes: &[u8]) -> Result<JMXVBMS, BmsLoaderError> {
    let signature = bytes
        .get(..12)
        .ok_or(BmsLoaderError::Truncated("signature"))?;
    if &signature[..8] != b"JMXVBMS " {
        return Err(BmsLoaderError::BadSignature);
    }
    let version = match &signature[8..12] {
        b"0109" => BmsVersion::V0109,
        _ => BmsVersion::V0110,
    };

    let mut cursor = Cursor::new(bytes);
    cursor.set_position(12);
    let header = Header::read(&mut cursor)?;

    seek_section(&mut cursor, header.vertex_offset as u64, "vertex")?;
    let vertex_data = VertexData::from(&mut cursor, header.vertex_flag)?;

    // Skin precedes faces on disk, and its end anchors the offset correction
    // above, so it has to be read before the sections that depend on it.
    seek_section(&mut cursor, header.skin_offset as u64, "skin")?;
    let bone_data = MeshBones::from(&mut cursor, vertex_data.vertices.len(), version);
    let skin_end = cursor.position();

    let delta = match skin_end.checked_sub(header.face_offset as u64) {
        Some(d) if d <= MAX_REPACK_DELTA && bone_data.is_some() => d,
        _ => 0,
    };

    seek_section(&mut cursor, header.face_offset as u64 + delta, "faces")?;
    need(&cursor, 4, "faces")?;
    let face_count = cursor.get_u32_le();
    let face_count = checked_count(&cursor, face_count, 6, "face")?;
    let mut indices = Vec::with_capacity(face_count);
    for _ in 0..face_count {
        indices.push((
            cursor.get_u16_le(),
            cursor.get_u16_le(),
            cursor.get_u16_le(),
        ));
    }

    seek_section(
        &mut cursor,
        header.bounding_box_offset as u64 + delta,
        "bounding box",
    )?;
    need(&cursor, 24, "bounding box")?;
    let bounding_box = (cursor.get_vec3(), cursor.get_vec3());

    let navmesh = if header.navmesh_offset > 0 {
        seek_section(&mut cursor, header.navmesh_offset as u64 + delta, "navmesh")?;
        Some(BmsNavMesh::parse(&mut cursor, header.nav_flag))
    } else {
        None
    };

    let bms = JMXVBMS {
        name: header.name,
        vertex_data,
        indices,
        bounding_box,
        bone_data,
        navmesh,
        material: header.material,
    };

    Ok(bms)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use bevy::asset::io::AssetReader;
    use bevy_pk2::prelude::Archive;
    use futures::AsyncReadExt;

    use bevy::prelude::Vec3;

    use super::navmesh::BmsNavMesh;
    use super::parse_bms;

    /// Parses every `.bms` inside the real game's Data.pk2 and sanity-checks
    /// all nav mesh indices. Needs `SRO_PK2_PATH` pointing at a directory with
    /// Data.pk2 (same variable the client uses); skips silently otherwise, so
    /// CI without game data stays green.
    #[test]
    fn parse_all_bms_navmeshes_from_data_pk2() {
        let Some(dir) = std::env::var_os("SRO_PK2_PATH") else {
            eprintln!("SRO_PK2_PATH not set; skipping .bms nav mesh verification");
            return;
        };
        let data_pk2 = PathBuf::from(dir).join("Data.pk2");
        if !data_pk2.exists() {
            eprintln!("{} not found; skipping", data_pk2.display());
            return;
        }

        let archive = Archive::configured(&data_pk2);
        let mut paths: Vec<PathBuf> = archive
            .root
            .get_all_entries()
            .into_iter()
            .filter(|(path, entry)| {
                entry.is_file()
                    && path
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("bms"))
            })
            .map(|(path, _)| path)
            .collect();
        paths.sort();

        let mut parsed = 0_usize;
        let mut with_navmesh = 0_usize;
        let mut failures: Vec<(PathBuf, String)> = Vec::new();
        let mut panicked: Vec<PathBuf> = Vec::new();

        for path in &paths {
            let bytes = futures::executor::block_on(async {
                let mut reader = AssetReader::read(&archive, path)
                    .await
                    .map_err(|e| e.to_string())?;
                let mut buf = Vec::new();
                reader
                    .read_to_end(&mut buf)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok::<_, String>(buf)
            });
            let bytes = match bytes {
                Ok(bytes) => bytes,
                Err(e) => {
                    failures.push((path.clone(), format!("read failed: {e}")));
                    continue;
                }
            };

            // The loader must never panic on file input — the `catch_unwind`
            // stays as the assertion for that, not as a workaround.
            match std::panic::catch_unwind(|| parse_bms(&bytes)) {
                Err(_) => panicked.push(path.clone()),
                Ok(Err(e)) => failures.push((path.clone(), e.to_string())),
                Ok(Ok(bms)) => {
                    parsed += 1;
                    if let Some(nav) = &bms.navmesh {
                        with_navmesh += 1;
                        if let Err(e) = check_navmesh(nav) {
                            failures.push((path.clone(), e));
                        }
                    }
                }
            }
        }

        println!(
            "parsed {parsed}/{} .bms files, {with_navmesh} with nav mesh section",
            paths.len()
        );
        assert!(
            !paths.is_empty(),
            "no .bms entries found in {}",
            data_pk2.display()
        );
        assert!(
            with_navmesh > 0,
            "no nav meshes parsed at all — navmesh_offset/nav_flag handling is likely wrong"
        );
        assert!(
            panicked.is_empty(),
            "the loader must return Err on malformed input, never panic — {} panicked: {:#?}",
            panicked.len(),
            &panicked[..panicked.len().min(5)]
        );
        // Three files in the user's Data.pk2 have every offset shifted,
        // VertexOffset included, so the vertex section itself is unreachable
        // from the header and the skin-anchored correction has nothing to
        // measure against. They are rejected cleanly rather than recovered.
        const UNRECOVERABLE: [&str; 3] = [
            "avatar_m_ghost_captain_part2.bms",
            "avatar_w_2012_new_devil_wing_part1.bms",
            "avatar_w_2012_new_devil_wing_part2.bms",
        ];
        let unexpected: Vec<_> = failures
            .iter()
            .filter(|(path, _)| {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                !UNRECOVERABLE.contains(&name.as_ref())
            })
            .collect();
        assert!(
            unexpected.is_empty(),
            "{} of {} .bms files failed unexpectedly, first few: {:#?}",
            unexpected.len(),
            paths.len(),
            &unexpected[..unexpected.len().min(5)]
        );
    }

    /// Diagnostic: where does the `0x400` second-UV vertex flag actually
    /// occur across Data.pk2, and do any *item* meshes (weapons) carry it?
    /// Gates the UV2-basis alchemy-streak mode (gap #10 in
    /// docs/rendering-mobile-shader-comparison.md): the mobile port pans its
    /// enhancement glow along the weapon's second UV set, but our format doc
    /// (docs/formats/bms-jmxvbms.md) names the flag `LightMapUV` with a
    /// trailing lightmap path — i.e. static world geometry, not items. For
    /// every flagged mesh this samples the UV2 value range (a per-mesh 0..1
    /// parameterization would be usable; a world-atlas spread would not) and
    /// the lightmap path. Run:
    /// cargo test -p client probe_bms_uv2_census -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic; needs real assets/Data.pk2"]
    fn probe_bms_uv2_census() {
        use std::collections::BTreeMap;
        use std::io::Cursor;

        use super::header::Header;
        use super::seek_section;
        use super::vertex::VertexData;

        let archive = Archive::configured(&PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../assets/Data.pk2"
        )));
        let mut paths: Vec<PathBuf> = archive
            .root
            .get_all_entries()
            .into_iter()
            .filter(|(path, entry)| {
                entry.is_file()
                    && path
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("bms"))
            })
            .map(|(path, _)| path)
            .collect();
        paths.sort();

        let mut failures = 0u32;
        // path prefix (first three components, lowercased) → (meshes, 0x400 hits)
        let mut by_prefix: BTreeMap<String, (u32, u32)> = BTreeMap::new();
        let mut flag_histogram: BTreeMap<u32, u32> = BTreeMap::new();
        let mut hit_details: Vec<String> = Vec::new();
        let mut hits = 0u32;
        let mut item_hits = 0u32;

        for path in &paths {
            let Some(bytes) = archive.read_file_bytes(path) else {
                failures += 1;
                continue;
            };
            // header + (for flagged meshes) vertex data; malformed files
            // report an error and are counted as failures
            let parsed = std::panic::catch_unwind(|| {
                let mut cursor = Cursor::new(bytes.as_slice());
                cursor.set_position(12);
                let header = Header::read(&mut cursor).ok()?;
                let detail = if header.vertex_flag & 0x400 != 0 {
                    seek_section(&mut cursor, header.vertex_offset as u64, "vertex").ok()?;
                    let vertex_data = VertexData::from(&mut cursor, header.vertex_flag).ok()?;
                    let mut min = bevy::math::Vec2::MAX;
                    let mut max = bevy::math::Vec2::MIN;
                    for v in &vertex_data.vertices {
                        if let Some(uv) = v.uv_1 {
                            min = min.min(uv);
                            max = max.max(uv);
                        }
                    }
                    Some(format!(
                        "{} vertices, uv2 range ({:.3},{:.3})..({:.3},{:.3}), lightmap {:?}",
                        vertex_data.vertices.len(),
                        min.x,
                        min.y,
                        max.x,
                        max.y,
                        vertex_data.lightmap_path
                    ))
                } else {
                    None
                };
                Some((header.vertex_flag, detail))
            });
            let Ok(Some((vertex_flag, detail))) = parsed else {
                failures += 1;
                continue;
            };

            let prefix: String = path
                .iter()
                .take(3)
                .map(|c| c.to_string_lossy().to_ascii_lowercase())
                .collect::<Vec<_>>()
                .join("/");
            let is_item = prefix.starts_with("res/item");
            let entry = by_prefix.entry(prefix).or_default();
            entry.0 += 1;
            *flag_histogram.entry(vertex_flag).or_default() += 1;
            if let Some(detail) = detail {
                entry.1 += 1;
                hits += 1;
                if is_item {
                    item_hits += 1;
                }
                if hit_details.len() < 30 || is_item {
                    hit_details.push(format!("{}: {detail}", path.display()));
                }
            }
        }

        println!(
            "{} .bms files, {failures} unreadable/unparseable, {hits} with flag 0x400 \
             ({item_hits} under res/item)",
            paths.len()
        );
        println!("vertex_flag histogram:");
        for (flag, count) in &flag_histogram {
            println!("  0x{flag:04x}: {count}");
        }
        println!("meshes / 0x400 hits by prefix:");
        for (prefix, (total, flagged)) in &by_prefix {
            println!("  {prefix}: {total} meshes, {flagged} with uv2");
        }
        for d in &hit_details {
            println!("uv2 hit: {d}");
        }
    }

    /// All indices a nav mesh stores must point inside its own arrays.
    fn check_navmesh(nav: &BmsNavMesh) -> Result<(), String> {
        let vertex_count = nav.vertices.len();
        for (i, cell) in nav.cells.iter().enumerate() {
            for v in cell.vertices {
                if v as usize >= vertex_count {
                    return Err(format!(
                        "cell {i}: vertex index {v} out of range ({vertex_count} vertices)"
                    ));
                }
            }
        }
        for (kind, edges) in [
            ("outline", &nav.outline_edges),
            ("inline", &nav.inline_edges),
        ] {
            for (i, edge) in edges.iter().enumerate() {
                if edge.src_vertex as usize >= vertex_count
                    || edge.dst_vertex as usize >= vertex_count
                {
                    return Err(format!("{kind} edge {i}: vertex index out of range"));
                }
                for cell in [edge.src_cell, edge.dst_cell] {
                    if cell != u16::MAX && cell as usize >= nav.cells.len() {
                        return Err(format!(
                            "{kind} edge {i}: cell index {cell} out of range ({} cells)",
                            nav.cells.len()
                        ));
                    }
                }
            }
        }
        let grid = &nav.outline_lookup;
        if grid.cells.len() != (grid.width * grid.height) as usize {
            return Err(format!(
                "outline grid has {} cells, expected {}x{}",
                grid.cells.len(),
                grid.width,
                grid.height
            ));
        }
        for (i, cell) in grid.cells.iter().enumerate() {
            for &outline in cell {
                if outline as usize >= nav.outline_edges.len() {
                    return Err(format!(
                        "grid cell {i}: outline index {outline} out of range ({} outline edges)",
                        nav.outline_edges.len()
                    ));
                }
            }
        }
        Ok(())
    }

    /// Builds a structurally valid JMXVBMS: one sub-primitive, `vertex_count`
    /// vertices, an optional bone table, one sentinel triangle, and empty
    /// cloth/occlusion/unk9 sections. `skin_records` is appended verbatim so a
    /// test can pin the exact per-vertex stride the parser must consume.
    fn build_bms(
        version: &[u8; 4],
        vertex_flag: u32,
        vertex_count: usize,
        morph: &[u8],
        bones: &[&str],
        skin_records: &[u8],
    ) -> Vec<u8> {
        fn len_prefixed(out: &mut Vec<u8>, s: &[u8]) {
            out.extend((s.len() as u32).to_le_bytes());
            out.extend(s);
        }

        let mut vertex = Vec::new();
        vertex.extend((vertex_count as u32).to_le_bytes());
        for i in 0..vertex_count {
            vertex.extend([0u8; 12]); // position
            vertex.extend([0u8; 12]); // normal
            vertex.extend([0u8; 8]); // uv0
            if vertex_flag & 0x400 != 0 {
                vertex.extend([0u8; 8]);
            }
            if vertex_flag & 0x800 != 0 {
                vertex.extend(morph);
            }
            vertex.extend(0f32.to_le_bytes()); // float
            vertex.extend(0u32.to_le_bytes()); // int0
            vertex.extend((i as u32).to_le_bytes()); // int1 = per-vertex sentinel
        }

        let mut skin = Vec::new();
        skin.extend((bones.len() as u32).to_le_bytes());
        for b in bones {
            len_prefixed(&mut skin, b.as_bytes());
        }
        skin.extend(skin_records);

        let mut face = Vec::new();
        face.extend(1u32.to_le_bytes());
        face.extend(1u16.to_le_bytes());
        face.extend(2u16.to_le_bytes());
        face.extend(3u16.to_le_bytes());

        // 12 signature + 15 u32 fields + two 1-char strings + trailing u32.
        const HEADER_LEN: usize = 12 + 15 * 4 + (4 + 1) + (4 + 1) + 4;
        let vertex_offset = HEADER_LEN;
        let skin_offset = vertex_offset + vertex.len();
        let face_offset = skin_offset + skin.len();
        let cloth_vertex = face_offset + face.len();
        let cloth_edge = cloth_vertex + 4;
        let bounding_box = cloth_edge + 4;
        let occlusion = bounding_box + 24;
        let unknown = occlusion + 4;

        let mut out = Vec::new();
        out.extend(b"JMXVBMS ");
        out.extend(version);
        for v in [
            vertex_offset,
            skin_offset,
            face_offset,
            cloth_vertex,
            cloth_edge,
            bounding_box,
            occlusion,
            0, // navmesh
            0, // skinned navmesh
            unknown,
            0, // unknown uint
            0, // nav flag
        ] {
            out.extend((v as u32).to_le_bytes());
        }
        out.extend(1u32.to_le_bytes()); // sub prim count
        out.extend(vertex_flag.to_le_bytes());
        out.extend(0u32.to_le_bytes()); // unknown uint 2
        len_prefixed(&mut out, b"m"); // name
        len_prefixed(&mut out, b"x"); // material
        out.extend(0u32.to_le_bytes()); // unknown uint 3
        assert_eq!(out.len(), HEADER_LEN, "header builder drifted");

        out.extend(vertex);
        out.extend(skin);
        out.extend(face);
        out.extend([0u8; 8]); // cloth vertex + edge counts
        for f in [-1f32, -1., -1., 1., 1., 1.] {
            out.extend(f.to_le_bytes());
        }
        out.extend([0u8; 8]); // occlusion + unknown
        out
    }

    /// #279: `JMXVBMS 0109` stores four influences per vertex. Read at the
    /// 0110 stride of 6 B, vertex 1 decoded as `(0xFF, 0)(0xFF, 0)` — a zero
    /// weight sum, which `mesh.rs` turns into NaN joint weights.
    #[test]
    fn v0109_reads_four_influences_per_vertex() {
        const RECORD: [u8; 12] = [0x00, 0xff, 0xff, 0xff, 0, 0, 0xff, 0, 0, 0xff, 0, 0];
        let bytes = build_bms(b"0109", 0, 2, &[], &["Bone"], &[RECORD, RECORD].concat());

        let bms = parse_bms(&bytes).expect("0109 fixture must parse");
        let bones = bms.bone_data.expect("skin section");
        assert_eq!(bones.bone_data.len(), 2);
        for data in &bones.bone_data {
            assert_eq!((data.index1, data.weight1), (0x00, 0xFFFF));
            assert_eq!((data.index2, data.weight2), (0xFF, 0x0000));
        }
        // A wrong stride also drags the face section out of alignment.
        assert_eq!(bms.indices, vec![(1, 2, 3)]);
    }

    /// The other side of the version split: 0110 keeps the 6 B stride.
    #[test]
    fn v0110_keeps_two_influences_per_vertex() {
        const RECORD: [u8; 6] = [0x00, 0xff, 0xff, 0xff, 0, 0];
        let bytes = build_bms(b"0110", 0, 2, &[], &["Bone"], &[RECORD, RECORD].concat());

        let bms = parse_bms(&bytes).expect("0110 fixture must parse");
        assert_eq!(bms.bone_data.expect("skin section").bone_data.len(), 2);
        assert_eq!(bms.indices, vec![(1, 2, 3)]);
    }

    /// #279: the morph record is 36 B, not the 64 the old code consumed
    /// (`copy_to_bytes(32)` already advances, and it advanced again).
    #[test]
    fn morph_vertices_consume_36_bytes() {
        let morph = [0xAAu8; 36];
        let bytes = build_bms(b"0110", 0x800, 2, &morph, &[], &[]);

        let bms = parse_bms(&bytes).expect("0x800 fixture must parse");
        assert_eq!(bms.vertex_data.vertices.len(), 2);
        assert_eq!(
            bms.vertex_data.vertices[0].morphing_data.as_deref(),
            Some(&morph[..])
        );
        // The int1 sentinel pins the stride: over-reading desynchronises it.
        assert_eq!(bms.vertex_data.vertices[1].int1, 1);
    }

    /// #279: file input must never panic the loader.
    #[test]
    fn truncated_file_errors_instead_of_panicking() {
        let bytes = build_bms(b"0110", 0, 2, &[], &[], &[]);
        for cut in [0, 8, 40, 60] {
            let mut truncated = bytes.clone();
            truncated.truncate(cut);
            assert!(
                parse_bms(&truncated).is_err(),
                "{cut}-byte file must be rejected"
            );
        }
    }

    /// A garbage count must be rejected before it reaches
    /// `Vec::with_capacity` — the corpus carries face counts near `u32::MAX`,
    /// and that allocation aborts the process instead of unwinding.
    #[test]
    fn bogus_counts_error_without_allocating() {
        let bytes = build_bms(b"0110", 0, 2, &[], &[], &[]);
        let face_offset = u32::from_le_bytes(bytes[20..24].try_into().unwrap()) as usize;
        let mut bad_faces = bytes.clone();
        bad_faces[face_offset..face_offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse_bms(&bad_faces).is_err());

        let vertex_offset = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        let mut bad_vertices = bytes;
        bad_vertices[vertex_offset..vertex_offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse_bms(&bad_vertices).is_err());
    }

    /// #279: 224 files in the user's Data.pk2 had a bone name appended without
    /// the offset table being rewritten, so every offset from FaceOffset on is
    /// short by that name's length. The skin section's true end reveals the
    /// shift.
    #[test]
    fn recovers_offsets_understated_by_an_appended_bone() {
        const RECORD: [u8; 6] = [0x00, 0xff, 0xff, 0xff, 0, 0];
        let good = build_bms(b"0110", 0, 1, &[], &["Bone"], &RECORD);
        let skin_offset = u32::from_le_bytes(good[16..20].try_into().unwrap()) as usize;

        let mut repacked = good.clone();
        repacked[skin_offset..skin_offset + 4].copy_from_slice(&2u32.to_le_bytes());
        let insert_at = skin_offset + 4 + 4 + "Bone".len();
        let mut extra = 4u32.to_le_bytes().to_vec();
        extra.extend(b"Xtra");
        repacked.splice(insert_at..insert_at, extra); // +8 B, header untouched

        let bms = parse_bms(&repacked).expect("repack-shaped file must parse");
        assert_eq!(bms.indices, vec![(1, 2, 3)]);
        assert_eq!(
            bms.bounding_box,
            (Vec3::splat(-1.0), Vec3::splat(1.0)),
            "bounding box must come from the corrected offset"
        );
    }
}
