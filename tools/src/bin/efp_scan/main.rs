//! Corpus-wide validator for the JMXVEFF (.efp) parser.
//!
//! Parses every .efp inside a Particles.pk2 archive and reports parse
//! failures plus histograms of format features. This is the ground truth for
//! the effect runtime: which render shapes, commands, and blend combinations
//! actually occur, and whether the parser consumes every file exactly.

use std::collections::BTreeMap;
use std::path::PathBuf;

use bevy::asset::io::AssetReader;
use bevy_pk2::prelude::Archive;
use clap::{App, Arg};
use futures_lite::future::block_on;
use futures_lite::io::AsyncReadExt;

#[path = "../../../../client/src/assets/efp/format.rs"]
mod format;

use format::{
    AngleVector1, EfController, EfStoredEffect, EfStoredObject, EffectCommand, FrameTextureSlide,
};

#[derive(Default)]
struct Stats {
    files_ok: usize,
    files_failed: usize,
    failures: Vec<(String, String)>,
    versions: BTreeMap<String, usize>,
    render_shapes: BTreeMap<String, usize>,
    view_modes: BTreeMap<String, usize>,
    commands: BTreeMap<String, usize>,
    controllers: BTreeMap<String, usize>,
    blend_combos: BTreeMap<(u32, u32), usize>,
    texture_stages: BTreeMap<[u32; 6], usize>,
    pad_sizes: BTreeMap<usize, usize>,
    emit_ints: BTreeMap<[u32; 4], usize>,
    max_depth: usize,
    max_nodes: usize,
    max_emit_texture_count: usize,
    // Semantics-calibration histograms (topics 3-8 of the gap audit):
    link_modes: BTreeMap<[i32; 4], usize>,
    link_mode_shapes: BTreeMap<(String, [i32; 4]), usize>,
    cone_shapes: BTreeMap<(String, String), usize>,
    cone_values: BTreeMap<(String, String), usize>,
    cone_radians_rel: BTreeMap<(String, String), usize>,
    tslide_left: BTreeMap<String, usize>,
    tslide_left_values: BTreeMap<(String, String), usize>,
    tslide_frame_counts: BTreeMap<usize, usize>,
    tslide_class: BTreeMap<String, usize>,
    random_scale_vals: BTreeMap<String, usize>,
    random_scale_coocc: BTreeMap<String, usize>,
    emit_invariant: BTreeMap<String, usize>,
    spawn_rates: BTreeMap<String, usize>,
    program_lens: BTreeMap<u32, usize>,
    int3_gt1_files: Vec<String>,
}

fn main() {
    let matches = App::new("efp_scan")
        .about("Parse-validate all .efp files in a Particles.pk2 archive")
        .arg(
            Arg::with_name("pk2")
                .long("pk2")
                .short("p")
                .takes_value(true)
                .default_value("assets/Particles.pk2")
                .help("Path to Particles.pk2"),
        )
        .arg(
            Arg::with_name("verbose")
                .long("verbose")
                .short("v")
                .help("Print every failure with its parse offset"),
        )
        .arg(
            Arg::with_name("prefix")
                .long("prefix")
                .takes_value(true)
                .help("Only scan files with this path prefix"),
        )
        .arg(
            Arg::with_name("find")
                .long("find")
                .takes_value(true)
                .help("Print paths of files containing this command name instead of the report"),
        )
        .get_matches();

    let pk2_path = PathBuf::from(matches.value_of("pk2").unwrap());
    let verbose = matches.is_present("verbose");
    let prefix = matches.value_of("prefix").map(PathBuf::from);
    let find = matches.value_of("find").map(str::to_string);

    let archive = Archive::configured(&pk2_path);
    let mut entries: Vec<_> = archive
        .root
        .get_all_entries()
        .into_iter()
        .filter(|(path, entry)| {
            entry.is_file()
                && path
                    .extension()
                    .map(|e| e.eq_ignore_ascii_case("efp"))
                    .unwrap_or(false)
        })
        .collect();
    entries.sort_by_key(|(path, _)| path.to_string_lossy().to_string());

    let mut stats = Stats::default();

    for (path, _) in entries {
        if let Some(ref pfx) = prefix {
            if !path.starts_with(pfx) {
                continue;
            }
        }

        let mut reader = match block_on(archive.read(&path)) {
            Ok(reader) => reader,
            Err(err) => {
                eprintln!("read error {}: {err}", path.display());
                continue;
            }
        };
        let mut data = Vec::new();
        if let Err(err) = block_on(async { reader.read_to_end(&mut data).await }) {
            eprintln!("read error {}: {err}", path.display());
            continue;
        }

        match format::parse_efp(&data) {
            Ok(effect) => {
                stats.files_ok += 1;
                if let Some(ref name) = find {
                    if effect_has_command(&effect, name) {
                        println!("{}", path.display());
                    }
                    continue;
                }
                record(&mut stats, &path.to_string_lossy(), &effect);
            }
            Err(err) => {
                stats.files_failed += 1;
                stats
                    .failures
                    .push((path.to_string_lossy().to_string(), err.to_string()));
            }
        }
    }

    if find.is_some() {
        return;
    }
    report(&stats, verbose);
    if stats.files_failed > 0 {
        std::process::exit(1);
    }
}

fn record(stats: &mut Stats, path: &str, effect: &EfStoredEffect) {
    *stats
        .versions
        .entry(effect.version_str.clone())
        .or_default() += 1;
    stats.max_nodes = stats.max_nodes.max(effect.nodes.len());
    stats.max_depth = stats.max_depth.max(depth_of(effect, effect.root, 0));

    for node in &effect.nodes {
        record_node(stats, path, node);
    }
}

/// True when any node in the effect carries a source command with this name
/// (in emitters, lifetime, programs, decorations, or a Program controller).
fn effect_has_command(effect: &EfStoredEffect, name: &str) -> bool {
    effect.nodes.iter().any(|node| {
        node.emitters
            .iter()
            .chain(node.lifetime.iter())
            .chain(node.programs.iter())
            .chain(node.decorations.iter())
            .any(|s| s.command.name() == name)
            || node.controllers.iter().any(|c| match c {
                EfController::Program(sources) => sources.iter().any(|s| s.command.name() == name),
                _ => false,
            })
    })
}

fn depth_of(effect: &EfStoredEffect, node: usize, depth: usize) -> usize {
    effect.nodes[node]
        .children
        .iter()
        .map(|&c| depth_of(effect, c, depth + 1))
        .max()
        .unwrap_or(depth)
}

fn record_node(stats: &mut Stats, path: &str, node: &EfStoredObject) {
    *stats.program_lens.entry(node.program_len).or_default() += 1;
    let shape = format!("{:?}", node.render_shape);
    *stats.render_shapes.entry(shape.clone()).or_default() += 1;
    *stats
        .view_modes
        .entry(format!("{:?}", node.view_mode))
        .or_default() += 1;
    *stats.pad_sizes.entry(node.timeline.pad.len()).or_default() += 1;

    if node.resource.src_blend != 0 || node.resource.dst_blend != 0 {
        *stats
            .blend_combos
            .entry((node.resource.src_blend, node.resource.dst_blend))
            .or_default() += 1;
    }
    *stats
        .texture_stages
        .entry(node.resource.texture_stage)
        .or_default() += 1;
    stats.max_emit_texture_count = stats
        .max_emit_texture_count
        .max(node.resource.texture_paths().count());

    let mut has_random_scale = false;
    let mut has_scale_graph = false;

    for controller in &node.controllers {
        let name = match controller {
            EfController::NormalTimeLife => "NormalTimeLife",
            EfController::NormalTimeLoopLife => "NormalTimeLoopLife",
            EfController::StaticEmit(emit) => {
                record_emit(stats, path, emit);
                "StaticEmit"
            }
            EfController::Program(_) => "Program",
            EfController::LinkMode(v) => {
                *stats.link_modes.entry(*v).or_default() += 1;
                *stats
                    .link_mode_shapes
                    .entry((shape.clone(), *v))
                    .or_default() += 1;
                "LinkMode"
            }
            EfController::Ban(_) => "BAN",
            EfController::ViewMode(_) => "ViewMode",
            EfController::Shape { .. } => "Shape",
            EfController::ScaleGraph { .. } => {
                has_scale_graph = true;
                "ScaleGraph"
            }
            EfController::DiffuseGraph { .. } => "DiffuseGraph",
        };
        *stats.controllers.entry(name.to_string()).or_default() += 1;
        if let EfController::Program(sources) = controller {
            for source in sources {
                *stats
                    .commands
                    .entry(source.command.name().to_string())
                    .or_default() += 1;
                record_command(
                    stats,
                    path,
                    &source.command,
                    &mut has_random_scale,
                    &mut has_scale_graph,
                );
            }
        }
    }

    for source in node
        .emitters
        .iter()
        .chain(node.lifetime.iter())
        .chain(node.programs.iter())
        .chain(node.decorations.iter())
    {
        *stats
            .commands
            .entry(source.command.name().to_string())
            .or_default() += 1;
        record_command(
            stats,
            path,
            &source.command,
            &mut has_random_scale,
            &mut has_scale_graph,
        );
    }

    if has_random_scale {
        let key = if has_scale_graph {
            "with_scale_graph"
        } else {
            "without_scale_graph"
        };
        *stats.random_scale_coocc.entry(key.to_string()).or_default() += 1;
    }
}

/// Records the calibration histograms for one source command (from any of the
/// source lists or a Program controller).
fn record_command(
    stats: &mut Stats,
    path: &str,
    command: &EffectCommand,
    has_random_scale: &mut bool,
    has_scale_graph: &mut bool,
) {
    match command {
        EffectCommand::StaticEmit(emit) => record_emit(stats, path, emit),
        EffectCommand::SetConePos(v)
        | EffectCommand::SetConeVel(v)
        | EffectCommand::ConeForce(v) => {
            record_cone(stats, command.name(), v);
        }
        EffectCommand::TextureSlide(slide) => record_texture_slide(stats, slide),
        EffectCommand::SetGraphRandomScale(v) => {
            *has_random_scale = true;
            let bucket = match *v {
                0 => "zero".to_string(),
                v if v < 0x100 => format!("small {v}"),
                v if v >= 0x0001_0000 => "pointer-like (>=0x10000)".to_string(),
                _ => "mid (0x100..0x10000)".to_string(),
            };
            *stats.random_scale_vals.entry(bucket).or_default() += 1;
        }
        EffectCommand::SetGraphScale(_) => *has_scale_graph = true,
        _ => {}
    }
}

fn record_emit(stats: &mut Stats, path: &str, emit: &format::EfStaticEmit) {
    *stats.emit_ints.entry(emit.ints).or_default() += 1;
    *stats
        .spawn_rates
        .entry(format!("{:.3}", emit.spawn_rate))
        .or_default() += 1;
    let [_, max_alive, per_burst, bursts] = emit.ints;
    let key = if bursts == 0 {
        "int3_zero"
    } else if max_alive == per_burst * bursts {
        "invariant ints[1]==ints[2]*ints[3]"
    } else {
        "invariant VIOLATED"
    };
    *stats.emit_invariant.entry(key.to_string()).or_default() += 1;
    if bursts > 1
        && stats.int3_gt1_files.len() < 200
        && stats.int3_gt1_files.last().map(String::as_str) != Some(path)
    {
        stats.int3_gt1_files.push(path.to_string());
    }
}

/// Classifies an AngleVector1: is `degrees.xy` a unit direction, a speed-like
/// magnitude pair, or zero — and is `radians` a full or z-only conversion of
/// `degrees`? A z-only conversion means xy are NOT angles.
fn record_cone(stats: &mut Stats, command: &'static str, v: &AngleVector1) {
    let d = v.degrees;
    let xy_norm = (d.x * d.x + d.y * d.y).sqrt();
    let xy = if d.x == 0.0 && d.y == 0.0 {
        "xy_zero"
    } else if (xy_norm - 1.0).abs() < 0.05 {
        "xy_unit"
    } else if d.x.abs() > 1.5 || d.y.abs() > 1.5 {
        "xy_large"
    } else {
        "xy_other"
    };
    let z = if d.z == 0.0 {
        "z=0"
    } else if d.z.abs() <= 90.0 {
        "z<=90"
    } else if d.z.abs() <= 180.0 {
        "z<=180"
    } else {
        "z>180"
    };
    *stats
        .cone_shapes
        .entry((command.to_string(), format!("{xy} {z}")))
        .or_default() += 1;
    *stats
        .cone_values
        .entry((
            command.to_string(),
            format!("({:.1},{:.1},{:.1})", d.x, d.y, d.z),
        ))
        .or_default() += 1;

    let conv = d * (std::f32::consts::PI / 180.0);
    let close = |a: f32, b: f32| (a - b).abs() <= 1e-3 * b.abs().max(1.0);
    let all_conv =
        close(v.radians.x, conv.x) && close(v.radians.y, conv.y) && close(v.radians.z, conv.z);
    let z_only = v.radians.x == d.x && v.radians.y == d.y && close(v.radians.z, conv.z);
    let rel = match (all_conv, z_only) {
        (true, true) => "ambiguous (xy zero/tiny)",
        (true, false) => "all components converted",
        (false, true) => "z-only converted, xy copied",
        (false, false) => "other",
    };
    *stats
        .cone_radians_rel
        .entry((command.to_string(), rel.to_string()))
        .or_default() += 1;
}

/// Buckets a TextureSlide by which parts are used and whether the frame
/// keyframes look like discrete flipbook grid cells (offsets that are exact
/// multiples of a constant cell size in zw) or a continuous curve.
fn record_texture_slide(stats: &mut Stats, slide: &FrameTextureSlide) {
    let l = slide.left;
    let eps = 1e-6;
    let left = match (l.x.abs() > eps || l.y.abs() > eps, l.z.abs() > eps) {
        (false, false) => "left_zero",
        (true, false) => "left_xy",
        (false, true) => "left_z_only",
        (true, true) => "left_xyz",
    };
    *stats.tslide_left.entry(left.to_string()).or_default() += 1;
    let frames_kind = if slide.frames.is_empty() {
        "no_frames"
    } else {
        "with_frames"
    };
    *stats
        .tslide_left_values
        .entry((
            frames_kind.to_string(),
            format!("({:.2},{:.2},{:.2})", l.x, l.y, l.z),
        ))
        .or_default() += 1;
    *stats
        .tslide_frame_counts
        .entry(slide.frames.len())
        .or_default() += 1;

    let frames = &slide.frames;
    let frame_class = if frames.is_empty() {
        "no_frames"
    } else if frames.len() == 1 {
        "one_frame"
    } else {
        let zw0 = (frames[0].z, frames[0].w);
        let zw_const = frames
            .iter()
            .all(|f| (f.z - zw0.0).abs() < 1e-4 && (f.w - zw0.1).abs() < 1e-4);
        let is_multiple = |value: f32, cell: f32| {
            if cell.abs() < 1e-4 {
                return false;
            }
            let ratio = value / cell;
            (ratio - ratio.round()).abs() < 1e-3
        };
        let grid_cells = zw_const
            && frames
                .iter()
                .all(|f| is_multiple(f.x, zw0.0) && is_multiple(f.y, zw0.1));
        if grid_cells {
            "grid (offsets = n*zw cell)"
        } else if zw_const {
            "zw const, offsets continuous"
        } else {
            "continuous"
        }
    };
    *stats
        .tslide_class
        .entry(format!("{left} {frame_class}"))
        .or_default() += 1;
}

fn report(stats: &Stats, verbose: bool) {
    println!(
        "parsed {}/{} files ({} failed)",
        stats.files_ok,
        stats.files_ok + stats.files_failed,
        stats.files_failed
    );
    println!(
        "max nodes/file: {}, max depth: {}, max textures/node: {}",
        stats.max_nodes, stats.max_depth, stats.max_emit_texture_count
    );

    print_map("versions", &stats.versions);
    print_map("timeline pad sizes", &stats.pad_sizes);
    print_map("render shapes (per node)", &stats.render_shapes);
    print_map("view modes (per node)", &stats.view_modes);
    print_map("controllers", &stats.controllers);
    print_map("commands", &stats.commands);

    println!("\nblend combos (src, dst) -> count:");
    let mut combos: Vec<_> = stats.blend_combos.iter().collect();
    combos.sort_by_key(|(_, &count)| std::cmp::Reverse(count));
    for ((src, dst), count) in combos {
        println!("  ({src:2}, {dst:2}) {count:6}");
    }

    println!("\ntexture stages [sArg1,sArg2,sOp,dArg1,dArg2,dOp] -> count:");
    let mut stages: Vec<_> = stats.texture_stages.iter().collect();
    stages.sort_by_key(|(_, &count)| std::cmp::Reverse(count));
    for (stage, count) in stages.iter().take(15) {
        println!("  {stage:?} {count:8}");
    }

    println!("\ntexture-stage ARG slots (value -> count, weighted by node):");
    for (label, slot) in [
        ("SrcArg1", 0),
        ("SrcArg2", 1),
        ("DstArg1", 3),
        ("DstArg2", 4),
    ] {
        let mut values: BTreeMap<u32, usize> = BTreeMap::new();
        for (stage, count) in &stats.texture_stages {
            *values.entry(stage[slot]).or_default() += count;
        }
        let pretty: Vec<String> = values
            .iter()
            .map(|(v, c)| format!("{}={c}", d3dta_name(*v)))
            .collect();
        println!("  {label}: {}", pretty.join(", "));
    }

    println!("\nStaticEmit ints -> count (top 20):");
    let mut emits: Vec<_> = stats.emit_ints.iter().collect();
    emits.sort_by_key(|(_, &count)| std::cmp::Reverse(count));
    for (ints, count) in emits.iter().take(20) {
        println!("  {ints:?} {count:6}");
    }
    print_map("StaticEmit ints invariant", &stats.emit_invariant);
    print_map("StaticEmit spawn_rate values", &stats.spawn_rates);
    print_map("node program lengths (frames)", &stats.program_lens);
    if !stats.int3_gt1_files.is_empty() {
        println!(
            "\nfiles with StaticEmit ints[3] > 1 ({} shown):",
            stats.int3_gt1_files.len()
        );
        let shown = if verbose { 200 } else { 20 };
        for path in stats.int3_gt1_files.iter().take(shown) {
            println!("  {path}");
        }
    }

    println!("\nLinkMode values -> count:");
    let mut links: Vec<_> = stats.link_modes.iter().collect();
    links.sort_by_key(|(_, &count)| std::cmp::Reverse(count));
    for (v, count) in links {
        println!("  {v:?} {count:6}");
    }
    println!("\nLinkMode by render shape -> count:");
    let mut link_shapes: Vec<_> = stats.link_mode_shapes.iter().collect();
    link_shapes.sort_by_key(|(_, &count)| std::cmp::Reverse(count));
    for ((shape, v), count) in link_shapes {
        println!("  {shape:10} {v:?} {count:6}");
    }

    print_pair_map("cone AngleVector1 shape buckets", &stats.cone_shapes);
    print_pair_map("cone radians-vs-degrees relation", &stats.cone_radians_rel);
    println!("\ncone raw degrees values (top 20):");
    let mut cones: Vec<_> = stats.cone_values.iter().collect();
    cones.sort_by_key(|(_, &count)| std::cmp::Reverse(count));
    for ((cmd, val), count) in cones.iter().take(20) {
        println!("  {cmd:12} {val:24} {count:6}");
    }

    print_map("TextureSlide left usage", &stats.tslide_left);
    println!("\nTextureSlide left values by frames kind (top 25):");
    let mut lefts: Vec<_> = stats.tslide_left_values.iter().collect();
    lefts.sort_by_key(|(_, &count)| std::cmp::Reverse(count));
    for ((kind, val), count) in lefts.iter().take(25) {
        println!("  {kind:12} {val:24} {count:6}");
    }
    print_map("TextureSlide frame counts", &stats.tslide_frame_counts);
    print_map("TextureSlide classification", &stats.tslide_class);

    print_map(
        "SetGraphRandomScale value buckets",
        &stats.random_scale_vals,
    );
    print_map(
        "SetGraphRandomScale scale-graph co-occurrence (per node)",
        &stats.random_scale_coocc,
    );

    if verbose || stats.files_failed <= 30 {
        for (path, err) in &stats.failures {
            println!("FAIL {path}: {err}");
        }
    } else {
        for (path, err) in stats.failures.iter().take(30) {
            println!("FAIL {path}: {err}");
        }
        println!(
            "... and {} more failures (use -v)",
            stats.failures.len() - 30
        );
    }
}

fn print_map<K: std::fmt::Display + Ord>(title: &str, map: &BTreeMap<K, usize>) {
    println!("\n{title}:");
    let mut entries: Vec<_> = map.iter().collect();
    entries.sort_by_key(|(_, &count)| std::cmp::Reverse(count));
    for (key, count) in entries {
        println!("  {key:24} {count:6}");
    }
}

fn print_pair_map(title: &str, map: &BTreeMap<(String, String), usize>) {
    println!("\n{title}:");
    let mut entries: Vec<_> = map.iter().collect();
    entries.sort_by_key(|((cmd, _), &count)| (cmd.clone(), std::cmp::Reverse(count)));
    for ((cmd, bucket), count) in entries {
        println!("  {cmd:12} {bucket:36} {count:6}");
    }
}

/// D3DTA texture-stage argument names (the low nibble selects the source).
fn d3dta_name(v: u32) -> String {
    match v & 0xF {
        0 => format!("DIFFUSE({v:#x})"),
        1 => format!("CURRENT({v:#x})"),
        2 => format!("TEXTURE({v:#x})"),
        3 => format!("TFACTOR({v:#x})"),
        4 => format!("SPECULAR({v:#x})"),
        _ => format!("{v:#x}"),
    }
}
