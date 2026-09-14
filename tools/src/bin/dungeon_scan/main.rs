//! Corpus-wide validator for the dungeon formats (EP-18 / #100).
//!
//! Parses every `.dof` (JMXVDOF) and every `navmesh/ainavdata_*.dat`
//! (AINavData) in a Data.pk2, cross-checks the `dungeon/dungeoninfo.txt`
//! id→path table, and histograms the unknown fields so newly-observed values
//! land in `docs/formats/`. Two nav-semantics probes de-risk the dungeon nav
//! runtime before it exists:
//!  - transition edges: every ConnectedBlockIndices pair should share at
//!    least one world-space-coincident outline edge (the geometric-link
//!    reading of RSBot's LinkBlock behaviour);
//!  - stacked floors: blocks sharing a voxel should sit at distinct Y so a
//!    nearest-Y pick can disambiguate floors.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use bevy::math::{Mat4, Quat, Vec3};
use bevy_pk2::prelude::Archive;
use clap::{App, Arg};

use client::assets::ainav;
use client::assets::bms::parse_bms;
use client::assets::bsr::bsr::parse_bsr;
use client::assets::dof::format as dof;
use client::assets::textdata::decode::decode_textdata;
use client::assets::textdata::dungeoninfo::DungeonInfo;

fn main() {
    let matches = App::new("dungeon_scan")
        .about("Validate all JMXVDOF + AINavData files in a Data.pk2 against the documented layout")
        .arg(
            Arg::with_name("pk2")
                .long("pk2")
                .takes_value(true)
                .default_value("assets/Data.pk2")
                .help("Path to Data.pk2"),
        )
        .arg(
            Arg::with_name("verbose")
                .long("verbose")
                .short("v")
                .help("Print per-file stats"),
        )
        .get_matches();

    let pk2_path = PathBuf::from(matches.value_of("pk2").unwrap());
    let verbose = matches.is_present("verbose");
    let archive = Archive::configured(&pk2_path);

    let entries = archive.root.get_all_entries();
    let dof_paths: Vec<PathBuf> = entries
        .iter()
        .filter(|(p, e)| e.is_file() && has_ext(p, "dof"))
        .map(|(p, _)| p.clone())
        .collect();
    let dat_paths: Vec<PathBuf> = entries
        .iter()
        .filter(|(p, e)| {
            e.is_file()
                && has_ext(p, "dat")
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.to_lowercase().starts_with("ainavdata_"))
        })
        .map(|(p, _)| p.clone())
        .collect();

    println!(
        "== dungeon_scan: {} .dof files, {} ainavdata .dat files in {}",
        dof_paths.len(),
        dat_paths.len(),
        pk2_path.display()
    );

    let dungeon_info = scan_dungeoninfo(&archive);
    let dofs = scan_dofs(&archive, &dof_paths, verbose);
    cross_check(&dungeon_info, &dofs);
    scan_ainav(&archive, &dat_paths, verbose);
    let nav_meshes = scan_block_resources(&archive, &dofs);
    probe_transition_edges(&dofs, &nav_meshes);
    probe_stacked_floors(&dofs);
}

fn has_ext(path: &Path, ext: &str) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

fn read(archive: &Archive, path: &Path) -> Option<Vec<u8>> {
    archive.read_file_bytes(path)
}

// ---------------------------------------------------------------- dungeoninfo

fn scan_dungeoninfo(archive: &Archive) -> DungeonInfo {
    let Some(bytes) = read(archive, Path::new("dungeon/dungeoninfo.txt")) else {
        println!("!! dungeon/dungeoninfo.txt not found");
        return DungeonInfo::default();
    };
    let info = DungeonInfo::parse(&decode_textdata(&bytes));
    println!("\n== dungeoninfo.txt: {} enabled rows", info.0.len());
    let mut missing = 0;
    for entry in info.entries() {
        let rel = entry.dof_path.replace('\\', "/").to_lowercase();
        if read(archive, Path::new(&rel)).is_none() {
            println!("  !! id {} -> {} missing from archive", entry.id, rel);
            missing += 1;
        }
    }
    if missing == 0 {
        println!("  all referenced .dof paths resolve in the archive");
    }
    info
}

// ------------------------------------------------------------------- DOF scan

struct ParsedDof {
    path: PathBuf,
    dof: dof::JMXVDOF,
}

fn scan_dofs(archive: &Archive, paths: &[PathBuf], verbose: bool) -> Vec<ParsedDof> {
    let mut ok = Vec::new();
    let mut failures = Vec::new();
    let mut unk_uint0 = BTreeMap::new();
    let mut unk_uint1 = BTreeMap::new();
    let mut unk_byte1 = BTreeMap::new();
    let mut is_entrance = BTreeMap::new();
    let mut obj_flags = BTreeMap::new();
    let mut obj_unk0 = BTreeMap::new();
    let mut group_flags = BTreeMap::new();
    let mut info_tuples = BTreeMap::new();
    let mut height_fog_blocks = 0usize;
    let mut payload_blocks = 0usize;
    let mut total_blocks = 0usize;
    let mut light_counts: Vec<usize> = Vec::new();
    let mut index_violations: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut violation_samples: Vec<String> = Vec::new();
    let mut room_sentinels = 0usize;

    for path in paths {
        let Some(bytes) = read(archive, path) else {
            failures.push((path.clone(), "read failed".to_string()));
            continue;
        };
        match dof::parse(&bytes) {
            Err(e) => failures.push((path.clone(), e.to_string())),
            Ok(d) => {
                if verbose {
                    println!(
                        "  {} region {:#06x} blocks {} grid {}x{}x{} voxels {} labels {}/{}{}",
                        path.display(),
                        d.region_id,
                        d.blocks.len(),
                        d.grid.width,
                        d.grid.height,
                        d.grid.length,
                        d.grid.voxels.len(),
                        d.room_names.len(),
                        d.floor_names.len(),
                        if d.legacy_block_layout { " LEGACY" } else { "" },
                    );
                }
                if d.links.len() != d.blocks.len() {
                    println!(
                        "  !! {}: linkCount {} != blockCount {}",
                        path.display(),
                        d.links.len(),
                        d.blocks.len()
                    );
                }
                *info_tuples
                    .entry(format!(
                        "({}, {}, {:?}, {}, {})",
                        d.info.type_id,
                        d.info.category,
                        d.info.name,
                        d.info.unk0 as i32,
                        d.info.unk1 as i32
                    ))
                    .or_insert(0usize) += 1;
                let block_count = d.blocks.len() as u32;
                let mut violation = |kind: &'static str, block_idx: usize, value: u32| {
                    *index_violations.entry(kind).or_insert(0) += 1;
                    if violation_samples.len() < 12 {
                        violation_samples.push(format!(
                            "{} block {} {} = {}",
                            path.display(),
                            block_idx,
                            kind,
                            value
                        ));
                    }
                };
                for (block_idx, block) in d.blocks.iter().enumerate() {
                    total_blocks += 1;
                    *unk_uint0.entry(block.unk_uint0).or_insert(0usize) += 1;
                    *unk_uint1.entry(block.unk_uint1).or_insert(0usize) += 1;
                    *unk_byte1.entry(block.unk_byte1).or_insert(0usize) += 1;
                    *is_entrance.entry(block.is_entrance).or_insert(0usize) += 1;
                    if block.fog.height_fog.is_some() {
                        height_fog_blocks += 1;
                    }
                    if block.unk_byte1_payload.is_some() {
                        payload_blocks += 1;
                    }
                    light_counts.push(block.lights.len());
                    for obj in &block.objects {
                        *obj_flags.entry(obj.flag).or_insert(0usize) += 1;
                        *obj_unk0.entry(obj.unk0).or_insert(0usize) += 1;
                    }
                    for idx in &block.connected_block_indices {
                        if *idx >= block_count {
                            violation("connected-index", block_idx, *idx);
                        }
                    }
                    for idx in &block.visible_block_indices {
                        if *idx >= block_count {
                            violation("visible-index", block_idx, *idx);
                        }
                    }
                    // RoomIndex u32::MAX (-1 as i32) = "no room label" sentinel.
                    if !d.room_names.is_empty()
                        && block.room_index != u32::MAX
                        && block.room_index as usize >= d.room_names.len()
                    {
                        violation("room-index", block_idx, block.room_index);
                    }
                    if block.room_index == u32::MAX {
                        room_sentinels += 1;
                    }
                    if !d.floor_names.is_empty()
                        && block.floor_index as usize >= d.floor_names.len()
                    {
                        violation("floor-index", block_idx, block.floor_index);
                    }
                }
                for group in &d.groups {
                    *group_flags.entry(group.flag).or_insert(0usize) += 1;
                }
                for voxel in &d.grid.voxels {
                    for idx in &voxel.block_indices {
                        if *idx >= block_count {
                            violation("voxel-block-index", usize::MAX, *idx);
                        }
                    }
                }
                ok.push(ParsedDof {
                    path: path.clone(),
                    dof: d,
                });
            }
        }
    }

    println!(
        "\n== DOF: {} parsed ok ({} legacy layout), {} failed",
        ok.len(),
        ok.iter().filter(|p| p.dof.legacy_block_layout).count(),
        failures.len()
    );
    for (path, err) in &failures {
        println!("  !! {}: {}", path.display(), err);
    }
    light_counts.sort_unstable();
    let median_lights = light_counts
        .get(light_counts.len() / 2)
        .copied()
        .unwrap_or(0);
    println!(
        "  blocks {} · height-fog {} · unkByte1==2 payloads {} · room-index -1 sentinels {} · index violations {:?}",
        total_blocks, height_fog_blocks, payload_blocks, room_sentinels, index_violations
    );
    for sample in &violation_samples {
        println!("    !! {}", sample);
    }
    println!(
        "  lights per block: min {} median {} max {}",
        light_counts.first().copied().unwrap_or(0),
        median_lights,
        light_counts.last().copied().unwrap_or(0)
    );
    println!("  objInfo tuples: {:?}", info_tuples);
    println!("  block.unkUInt0: {:?}", unk_uint0);
    println!("  block.unkUInt1 (field_18): {:?}", trim(&unk_uint1));
    println!("  block.unkByte1: {:?}", unk_byte1);
    println!("  block.IsEntrance: {:?}", is_entrance);
    println!("  obj.Flag: {:?}", obj_flags);
    println!("  obj.Int0: {:?}", trim(&obj_unk0));
    println!("  group.Flag: {:?}", group_flags);
    ok
}

/// Cap a histogram at its 12 most frequent entries for printing.
fn trim<K: Clone + Ord + std::fmt::Debug>(map: &BTreeMap<K, usize>) -> Vec<(K, usize)> {
    let mut entries: Vec<_> = map.iter().map(|(k, v)| (k.clone(), *v)).collect();
    entries.sort_by(|a, b| b.1.cmp(&a.1));
    entries.truncate(12);
    entries
}

fn cross_check(info: &DungeonInfo, dofs: &[ParsedDof]) {
    println!("\n== dungeoninfo <-> DOF region-id cross-check");
    let by_path: HashMap<String, u16> = dofs
        .iter()
        .map(|p| {
            (
                p.path.to_string_lossy().to_lowercase().replace('\\', "/"),
                p.dof.region_id,
            )
        })
        .collect();
    let mut mismatches = 0;
    for entry in info.entries() {
        let rel = entry.dof_path.replace('\\', "/").to_lowercase();
        match by_path.get(&rel) {
            None => {}
            Some(&dof_region) if dof_region == entry.region_id => {}
            Some(&dof_region) => {
                println!(
                    "  id {} ({}): dungeoninfo region {:#06x} vs DOF header {:#06x}",
                    entry.id,
                    entry.name(),
                    entry.region_id,
                    dof_region
                );
                mismatches += 1;
            }
        }
    }
    println!(
        "  {} entries checked, {} header mismatches (dungeoninfo is authoritative)",
        info.0.len(),
        mismatches
    );
}

// ------------------------------------------------------------------ AINavData

fn scan_ainav(archive: &Archive, paths: &[PathBuf], verbose: bool) {
    let mut ok = 0;
    let mut contiguous = 0;
    let mut aligned_edge_blocks = 0usize;
    let mut total_ref_blocks = 0usize;
    let mut failures = Vec::new();
    for path in paths {
        let Some(bytes) = read(archive, path) else {
            failures.push((path.clone(), "read failed".to_string()));
            continue;
        };
        match ainav::parse(&bytes) {
            Err(e) => failures.push((path.clone(), e.to_string())),
            Ok(data) => {
                ok += 1;
                if data.sections_contiguous {
                    contiguous += 1;
                }
                // Filename `ainavdata_<decimal region id>.dat` should match
                // the embedded region id.
                let stem_id: Option<u16> = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .and_then(|s| s.rsplit('_').next())
                    .and_then(|s| s.parse().ok());
                if stem_id != Some(data.region_id) {
                    println!(
                        "  !! {}: embedded region {:#06x} does not match filename",
                        path.display(),
                        data.region_id
                    );
                }
                if data.blocks.len() != data.simple_blocks.len() {
                    println!(
                        "  !! {}: refDungeon blocks {} != simpleDungeonData blocks {}",
                        path.display(),
                        data.blocks.len(),
                        data.simple_blocks.len()
                    );
                }
                for (block, simple) in data.blocks.iter().zip(&data.simple_blocks) {
                    total_ref_blocks += 1;
                    if block.edge_count as usize == simple.edge_centers.len() {
                        aligned_edge_blocks += 1;
                    }
                }
                if verbose {
                    println!(
                        "  {} region {:#06x} blocks {} links {}",
                        path.display(),
                        data.region_id,
                        data.blocks.len(),
                        data.blocks.iter().map(|b| b.links.len()).sum::<usize>()
                    );
                }
            }
        }
    }
    println!(
        "\n== AINavData: {} parsed EOF-exact, {} failed · contiguous sections {}/{} · edge-aligned blocks {}/{}",
        ok,
        failures.len(),
        contiguous,
        ok,
        aligned_edge_blocks,
        total_ref_blocks
    );
    for (path, err) in &failures {
        println!("  !! {}: {}", path.display(), err);
    }
}

// -------------------------------------------- block resources -> BmsNavMesh

/// Outline segments of a block-resource nav mesh, in resource-local space.
struct ResourceNav {
    outlines: Vec<(Vec3, Vec3)>,
    /// Y range of nav vertices, for the floor probe.
    has_navmesh: bool,
}

/// Load every unique `Block.Path` `.bsr` and check it carries a `BmsNavMesh`
/// (the load-bearing reuse assumption of the dungeon nav runtime).
fn scan_block_resources(archive: &Archive, dofs: &[ParsedDof]) -> HashMap<String, ResourceNav> {
    let unique_paths: HashSet<String> = dofs
        .iter()
        .flat_map(|p| p.dof.blocks.iter())
        .map(|b| b.path.replace('\\', "/").to_lowercase())
        .collect();

    let mut result = HashMap::new();
    let mut with_nav = 0;
    let mut without_nav = Vec::new();
    let mut unreadable = Vec::new();
    for path in &unique_paths {
        let Some(bsr_bytes) = read(archive, Path::new(path)) else {
            unreadable.push(path.clone());
            continue;
        };
        let Ok(bsr) = parse_bsr(&bsr_bytes) else {
            unreadable.push(path.clone());
            continue;
        };
        let mut outlines = Vec::new();
        let mut has_navmesh = false;
        for mesh_path in &bsr.mesh_paths {
            let rel = mesh_path
                .to_string_lossy()
                .replace('\\', "/")
                .to_lowercase();
            let Some(bms_bytes) = read(archive, Path::new(&rel)) else {
                continue;
            };
            let Ok(bms) = parse_bms(&bms_bytes) else {
                continue;
            };
            if let Some(nav) = &bms.navmesh {
                has_navmesh = true;
                for edge in &nav.outline_edges {
                    let a = nav.vertices[edge.src_vertex as usize].position;
                    let b = nav.vertices[edge.dst_vertex as usize].position;
                    outlines.push((a, b));
                }
            }
        }
        if has_navmesh {
            with_nav += 1;
        } else {
            without_nav.push(path.clone());
        }
        result.insert(
            path.clone(),
            ResourceNav {
                outlines,
                has_navmesh,
            },
        );
    }
    println!(
        "\n== block resources: {} unique .bsr · {} with BmsNavMesh · {} without · {} unreadable",
        unique_paths.len(),
        with_nav,
        without_nav.len(),
        unreadable.len()
    );
    for path in without_nav.iter().take(10) {
        println!("  no navmesh: {}", path);
    }
    for path in unreadable.iter().take(10) {
        println!("  unreadable: {}", path);
    }
    result
}

// ------------------------------------------------------- nav semantics probes

fn block_transform(block: &dof::DofBlock) -> Mat4 {
    Mat4::from_translation(block.position) * Mat4::from_quat(Quat::from_rotation_y(-block.yaw))
}

/// For every ConnectedBlockIndices pair, check the two blocks share at least
/// one world-space-coincident outline edge (endpoints within tolerance,
/// either orientation) — the geometric transition-link reading.
fn probe_transition_edges(dofs: &[ParsedDof], nav_meshes: &HashMap<String, ResourceNav>) {
    const TOLERANCE: f32 = 5.0;
    let mut pairs = 0usize;
    let mut linked = 0usize;
    let mut unlinked_samples: Vec<String> = Vec::new();

    for parsed in dofs {
        let d = &parsed.dof;
        let world_outlines: Vec<Option<Vec<(Vec3, Vec3)>>> = d
            .blocks
            .iter()
            .map(|block| {
                let key = block.path.replace('\\', "/").to_lowercase();
                nav_meshes.get(&key).map(|nav| {
                    let m = block_transform(block);
                    nav.outlines
                        .iter()
                        .map(|(a, b)| (m.transform_point3(*a), m.transform_point3(*b)))
                        .collect()
                })
            })
            .collect();

        for (i, block) in d.blocks.iter().enumerate() {
            for &j in &block.connected_block_indices {
                let j = j as usize;
                if j <= i || j >= d.blocks.len() {
                    continue; // count each undirected pair once
                }
                let (Some(a_edges), Some(b_edges)) = (&world_outlines[i], &world_outlines[j])
                else {
                    continue;
                };
                pairs += 1;
                let coincident = a_edges.iter().any(|(a0, a1)| {
                    b_edges.iter().any(|(b0, b1)| {
                        (a0.distance(*b0) < TOLERANCE && a1.distance(*b1) < TOLERANCE)
                            || (a0.distance(*b1) < TOLERANCE && a1.distance(*b0) < TOLERANCE)
                    })
                });
                if coincident {
                    linked += 1;
                } else if unlinked_samples.len() < 10 {
                    unlinked_samples.push(format!(
                        "{} blocks {}<->{}",
                        parsed.path.display(),
                        i,
                        j
                    ));
                }
            }
        }
    }
    println!(
        "\n== transition-edge probe: {}/{} connected pairs share a coincident outline edge (tolerance {})",
        linked, pairs, TOLERANCE
    );
    for sample in &unlinked_samples {
        println!("  no coincident edge: {}", sample);
    }
}

/// For voxels holding multiple blocks, histogram the pairwise Y separation of
/// the blocks' collision-box centers: stacked floors need distinct Y bands
/// for a nearest-Y resolve to disambiguate. `CollisionBox0` is BLOCK-LOCAL
/// (measured: 0/151 Donwhang boxes sit near their block's position), so the
/// centers are lifted through the block placement first.
fn probe_stacked_floors(dofs: &[ParsedDof]) {
    let mut multi_voxels = 0usize;
    let mut separation_buckets: BTreeMap<&'static str, usize> = BTreeMap::new();
    for parsed in dofs {
        let d = &parsed.dof;
        for voxel in &d.grid.voxels {
            if voxel.block_indices.len() < 2 {
                continue;
            }
            multi_voxels += 1;
            let centers: Vec<f32> = voxel
                .block_indices
                .iter()
                .filter_map(|&idx| d.blocks.get(idx as usize))
                .map(|b| {
                    let local = (b.collision_box.min + b.collision_box.max) * 0.5;
                    block_transform(b).transform_point3(local).y
                })
                .collect();
            let mut min_sep = f32::MAX;
            for (n, a) in centers.iter().enumerate() {
                for b in &centers[n + 1..] {
                    min_sep = min_sep.min((a - b).abs());
                }
            }
            let bucket = match min_sep {
                s if s < 1.0 => "<1 (same floor)",
                s if s < 30.0 => "1-30 (step range)",
                s if s < 100.0 => "30-100",
                s if s < 300.0 => "100-300",
                _ => ">=300",
            };
            *separation_buckets.entry(bucket).or_insert(0) += 1;
        }
    }
    println!(
        "\n== stacked-floor probe: {} voxels with >=2 candidate blocks; min pairwise bbox-center Y separation: {:?}",
        multi_voxels, separation_buckets
    );
}
