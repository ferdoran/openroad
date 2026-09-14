//! Dump the node tree of a single .efp from Particles.pk2: timelines,
//! lifetimes, and emitter parameters. Companion to efp_scan for calibrating
//! the effect runtime's timing semantics against authored data.
//!
//! Usage: cargo run -p tools --bin efp_dump -- hiteffect/hit_1_cut_smash.efp

use std::path::PathBuf;

use bevy::asset::io::AssetReader;
use bevy_pk2::prelude::Archive;
use futures_lite::future::block_on;
use futures_lite::io::AsyncReadExt;

#[path = "../../../client/src/assets/efp/format.rs"]
mod format;

use format::{EeBlend, EeParameter, EeSourceData, EfController, EfStoredEffect, EffectCommand};

fn main() {
    let mut args = std::env::args().skip(1);
    let file = args
        .next()
        .expect("usage: efp_dump <path inside Particles.pk2> [pk2]");
    let pk2 = args.next().unwrap_or_else(|| "assets/Particles.pk2".into());

    let archive = Archive::configured(&PathBuf::from(pk2));
    let mut reader = block_on(archive.read(&PathBuf::from(&file))).expect("file in archive");
    let mut data = Vec::new();
    block_on(async { reader.read_to_end(&mut data).await }).expect("read");
    let effect = format::parse_efp(&data).expect("parse");

    println!(
        "{file}: version {} root_scale {}",
        effect.version_str, effect.root_scale
    );
    dump(&effect, effect.root, 0);
}

/// Compact one-line rendering of an `EeBlend` key list.
fn fmt_blend<T: Copy>(blend: &EeBlend<T>, fmt: impl Fn(T) -> String) -> String {
    let keys: Vec<String> = blend
        .keys
        .iter()
        .map(|&(t, v)| format!("{t:.2}:{}", fmt(v)))
        .collect();
    format!(
        "range=({},{}) keys[{}]={}",
        blend.begin,
        blend.end,
        keys.len(),
        keys.join(" ")
    )
}

fn dump(effect: &EfStoredEffect, idx: usize, depth: usize) {
    let node = &effect.nodes[idx];
    let indent = "  ".repeat(depth);
    let tl = &node.timeline;
    println!(
        "{indent}[{idx}] \"{}\" shape={:?} view={:?} len={} links={:?} attach={}",
        node.name,
        node.render_shape,
        node.view_mode,
        node.program_len,
        tl.ints,
        tl.attach_to_parent(),
    );
    let res = &node.resource;
    if res.src_blend != 0 || res.dst_blend != 0 || res.texture_stage != [0; 6] {
        println!(
            "{indent}    resource blend=({},{}) tex_stage={:?}",
            res.src_blend, res.dst_blend, res.texture_stage
        );
    }
    for (mesh, textures) in &res.meshes {
        if !mesh.is_empty() || textures.iter().any(|t| !t.is_empty()) {
            println!("{indent}    resource mesh=\"{mesh}\" textures={textures:?}");
        }
    }
    if let Some(emit) = node.static_emit() {
        println!(
            "{indent}    StaticEmit ints={:?} spawn_rate={}",
            emit.ints, emit.spawn_rate
        );
    }
    for source in &node.emitters {
        if let EffectCommand::StaticEmit(emit) = &source.command {
            println!(
                "{indent}    emitter-source StaticEmit ints={:?} spawn_rate={} start={:?} end={:?}",
                emit.ints, emit.spawn_rate, source.start, source.end,
            );
        } else {
            println!("{indent}    emitter-source {:?}", source.command);
        }
    }
    if let Some(lifetime) = &node.lifetime {
        println!("{indent}    lifetime {:?}", lifetime.command);
    }
    for (name, param) in &node.global_params {
        match param {
            EeParameter::BlendScaleGraph(blend) => println!(
                "{indent}    global {name} {}",
                fmt_blend(blend, |v: bevy::math::Vec3| format!(
                    "({:.2},{:.2},{:.2})",
                    v.x, v.y, v.z
                ))
            ),
            EeParameter::BlendDiffuseGraph(blend) => println!(
                "{indent}    global {name} {}",
                fmt_blend(blend, |v| format!("{v:08x}"))
            ),
            EeParameter::BsAnimation(names) => {
                println!("{indent}    global {name} {names:?}")
            }
        }
    }
    for controller in &node.controllers {
        match controller {
            EfController::StaticEmit(_) => continue,
            EfController::Shape { shape, .. } => println!("{indent}    ctrl Shape({shape:?})"),
            EfController::NormalTimeLife => println!("{indent}    ctrl NormalTimeLife"),
            EfController::NormalTimeLoopLife => println!("{indent}    ctrl NormalTimeLoopLife"),
            EfController::Program(sources) => {
                println!("{indent}    ctrl Program ({} sources):", sources.len());
                for source in sources {
                    print_source(&format!("{indent}  "), "ctrl-src", source);
                }
            }
            EfController::LinkMode(v) => println!("{indent}    ctrl LinkMode({v:?})"),
            EfController::Ban(paths) => println!("{indent}    ctrl Ban({paths:?})"),
            EfController::ViewMode(mode) => println!("{indent}    ctrl ViewMode({mode:?})"),
            EfController::ScaleGraph { x, y, z, .. } => println!(
                "{indent}    ctrl ScaleGraph x={} y={} z={}",
                fmt_blend(x, |v| format!("{v:.3}")),
                fmt_blend(y, |v| format!("{v:.3}")),
                fmt_blend(z, |v| format!("{v:.3}")),
            ),
            EfController::DiffuseGraph { alpha, color } => println!(
                "{indent}    ctrl DiffuseGraph alpha={} color={}",
                fmt_blend(alpha, |v| format!("{v:02x}")),
                fmt_blend(color, |v| format!("{v:08x}")),
            ),
        }
    }
    for source in &node.programs {
        print_source(&indent, "program", source);
    }
    for source in &node.decorations {
        print_source(&indent, "decoration", source);
    }
    for &child in &node.children {
        dump(effect, child, depth + 1);
    }
}

/// One line per source command; payloads are printed for the commands whose
/// semantics are still being calibrated (TextureSlide, SetGraphRandomScale,
/// cone commands).
fn print_source(indent: &str, kind: &str, source: &EeSourceData) {
    let detail = match &source.command {
        EffectCommand::TextureSlide(slide) => {
            let frames: Vec<String> = slide
                .frames
                .iter()
                .map(|f| format!("({:.4},{:.4},{:.4},{:.4})", f.x, f.y, f.z, f.w))
                .collect();
            format!(
                " left=({:.4},{:.4},{:.4}) frames[{}]={}",
                slide.left.x,
                slide.left.y,
                slide.left.z,
                frames.len(),
                frames.join(" ")
            )
        }
        EffectCommand::SetGraphRandomScale(v) => format!(" value={v:#010x}"),
        EffectCommand::Force(v) | EffectCommand::SetVelocity(v) | EffectCommand::SetPosition(v) => {
            format!(" v={v:?}")
        }
        EffectCommand::SetSpherePos(v) => format!(" radii={v:?}"),
        EffectCommand::SetRotationAxis(a)
        | EffectCommand::SetRVelocityAxis(a)
        | EffectCommand::SetShapeRot(a)
        | EffectCommand::SetShapeRotVel(a) => format!(" axis_angle={:?}", a.axis_angle),
        EffectCommand::SetRotation(r) | EffectCommand::SetRVelocity(r) => {
            format!(" euler_deg={:?}", r.euler_degrees)
        }
        EffectCommand::SetRotationMat(m) | EffectCommand::SetRVelocityMat(m) => {
            let (axis, angle) =
                bevy::math::Quat::from_mat3(&bevy::math::Mat3::from_mat4(*m)).to_axis_angle();
            format!(" axis={axis:?} angle_deg={:.2}", angle.to_degrees())
        }
        EffectCommand::SetBanPos(keys) => {
            let head: Vec<String> = keys
                .iter()
                .take(4)
                .map(|v| format!("({:.2},{:.2},{:.2})", v.x, v.y, v.z))
                .collect();
            format!(" keys[{}]={}...", keys.len(), head.join(" "))
        }
        EffectCommand::SetBanRot(keys) => format!(" keys[{}]", keys.len()),
        EffectCommand::SetGraphScale(keys) => format!(" keys={keys:?}"),
        EffectCommand::SetGraphDiffuse(keys) => {
            let hex: Vec<String> = keys.iter().map(|v| format!("{v:08x}")).collect();
            format!(" keys[{}]={}", hex.len(), hex.join(" "))
        }
        EffectCommand::SetConePos(v)
        | EffectCommand::SetConeVel(v)
        | EffectCommand::ConeForce(v) => {
            format!(" degrees={:?} radians={:?}", v.degrees, v.radians)
        }
        _ => String::new(),
    };
    println!(
        "{indent}    {kind} {} mode={:#04x} start={} step={} end={}{detail}",
        source.command.name(),
        source.mode,
        source.start,
        source.step,
        source.end,
    );
}
