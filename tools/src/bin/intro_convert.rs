//! `intro_convert` — turn an original `Media/script/intro/<name>.txt` camera
//! block into an openroad `.intro` camera path (#569).
//!
//! Idea: the cutscene scripts and our `.intro` asset are the same nine values
//! per keyframe, so the only thing standing between the three unconverted
//! cutscenes (`constantinople`, `egypt`, `roc`) and openroad was a manual
//! transcription step. This binary is that step, run against the *user's own*
//! `Media/` — no SRO data enters this repository, which is why converting is
//! a tool and not a commit.
//!
//! ```text
//! cargo run -p tools --bin intro_convert -- \
//!     --script /path/to/Media/script/intro/egypt.txt --out assets/intros/egypt.intro
//! ```
//!
//! The script files are UTF-16LE with a BOM, so they go through the shared
//! textdata decoder rather than `String::from_utf8`.

use std::fs;
use std::path::{Path, PathBuf};

use client::assets::intro_scene::IntroScene;
use client::assets::textdata::decode::decode_textdata;

fn main() {
    let matches = clap::App::new("intro_convert")
        .about("Convert an original script/intro camera block into an .intro camera path")
        .arg(
            clap::Arg::with_name("script")
                .long("script")
                .takes_value(true)
                .required(true)
                .help("Path to the extracted Media/script/intro/<name>.txt"),
        )
        .arg(
            clap::Arg::with_name("out")
                .long("out")
                .takes_value(true)
                .help("Output .intro path (default: assets/intros/<name>.intro)"),
        )
        .arg(
            clap::Arg::with_name("name")
                .long("name")
                .takes_value(true)
                .help("Scene name (default: the script's file stem)"),
        )
        .arg(
            clap::Arg::with_name("music")
                .long("music")
                .takes_value(true)
                .help("Music asset path written into the scene"),
        )
        .get_matches();

    let script_path = PathBuf::from(matches.value_of("script").expect("required"));
    let stem = script_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("intro")
        .to_string();
    let name = matches.value_of("name").unwrap_or(&stem).to_string();
    // The splash's track comes from `Media/config/option.txt:12`; the other
    // cutscenes' tracks live in their own scripts' sound rows, so this is a
    // flag with the splash's value as its default rather than an invented
    // per-scene table.
    let music = matches
        .value_of("music")
        .unwrap_or("music://maintheme_cut.ogg");
    let out = matches
        .value_of("out")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new("assets/intros").join(format!("{name}.intro")));

    let bytes = match fs::read(&script_path) {
        Ok(bytes) => bytes,
        Err(err) => {
            eprintln!("cannot read {}: {err}", script_path.display());
            std::process::exit(2);
        }
    };
    let scene = match IntroScene::from_camera_script(&name, music, &decode_textdata(&bytes)) {
        Ok(scene) => scene,
        Err(err) => {
            eprintln!("{}: {err}", script_path.display());
            std::process::exit(1);
        }
    };
    let yaml = match serde_yaml::to_string(&scene) {
        Ok(yaml) => yaml,
        Err(err) => {
            eprintln!("cannot serialize {name}: {err}");
            std::process::exit(1);
        }
    };
    if let Err(err) = fs::write(&out, yaml) {
        eprintln!("cannot write {}: {err}", out.display());
        std::process::exit(2);
    }
    println!("{} -> {}", script_path.display(), out.display());
}
