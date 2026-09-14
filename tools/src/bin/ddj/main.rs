// This bin keeps its helper module in `lib.rs`; allow the special-module-name lint.
#![allow(special_module_name)]

use std::fs;
use std::path::PathBuf;

use clap::{crate_authors, crate_description, crate_name, crate_version};
use clap::{App, Arg, ArgMatches, SubCommand};

mod lib;

fn main() {
    let app = App::new(crate_name!())
        .version(crate_version!())
        .author(crate_authors!())
        .about(crate_description!())
        .subcommand(from_man())
        .subcommand(to_man());
    let matches = app.get_matches();
    match matches.subcommand() {
        ("from", Some(matches)) => {
            match matches.subcommand() {
                ("dds", Some(matches)) => from_dds(matches),
                // ("bmp", Some(matches)) => from_bmp(matches), // TODO
                _ => println!("{}", matches.usage()),
            }
        }
        ("to", Some(matches)) => match matches.subcommand() {
            ("dds", Some(matches)) => to_dds(matches),
            ("bmp", Some(matches)) => to_bmp(matches),
            _ => println!("{}", matches.usage()),
        },
        _ => println!("{}", matches.usage()),
    }
}

fn from_man() -> App<'static, 'static> {
    converter_man("from")
}
fn to_man() -> App<'static, 'static> {
    converter_man("to")
}

fn create_subcommand(name: &str) -> App<'static, 'static> {
    SubCommand::with_name(name)
        .version(crate_version!())
        .author(crate_authors!())
        .about(crate_description!())
        .arg(
            Arg::with_name("path")
                .short("p")
                .long("path")
                .required(true)
                .takes_value(true)
                .help("Path to the file or folder with ddj files."),
        )
        .arg(
            Arg::with_name("out")
                .short("o")
                .long("out")
                .takes_value(true)
                .help("Sets the output path to convert to."),
        )
        .arg(
            Arg::with_name("recursive")
                .short("r")
                .long("recursive")
                .takes_value(true)
                .required(false)
                .default_value("false")
                .help("If true, then recursively through all subfolders. Only works for folders."),
        )
}

fn converter_man(name: &str) -> App<'static, 'static> {
    SubCommand::with_name(name)
        .version(crate_version!())
        .author(crate_authors!())
        .about(crate_description!())
        //.subcommand(create_subcommand("bmp"))
        .subcommand(create_subcommand("ddj"))
        .subcommand(create_subcommand("dds"))
        .subcommand(create_subcommand("bmp"))
}

struct Args {
    path: PathBuf,
    out: PathBuf,
    recursive: bool,
}

fn get_arguments(matches: &ArgMatches) -> Args {
    let path = matches
        .value_of_os("path")
        .map(PathBuf::from)
        .expect("i failed");
    let out_path: PathBuf = matches
        .value_of_os("out")
        .map(PathBuf::from)
        .unwrap_or_else(|| path.with_extension(""));
    let recursive_opt: Option<bool> = matches
        .value_of("recursive")
        .map(|x| x.trim().parse().expect("i failed"));
    let recursive = recursive_opt.unwrap_or(false);

    if recursive && !path.is_dir() {
        panic!("If recursive is true, the path has to be a directoy.");
    }

    Args {
        path,
        out: out_path,
        recursive,
    }
}

type Callback = fn(path: PathBuf, out: PathBuf) -> Result<usize, std::io::Error>;
fn iterate_files(
    from_ext: &str,
    path: PathBuf,
    out: PathBuf,
    is_recursive: bool,
    callback: Callback,
) {
    println!("Converted {:?} -> {:?}", path.to_str(), out.to_str());

    if path.is_file() {
        let ext = path.extension().unwrap_or_default();

        if ext != from_ext {
            return;
        }

        callback(path, out).expect("i failed");
        return;
    }

    if !out.exists() {
        fs::create_dir(out.as_path()).expect("i failed");
    }

    // is_dir
    let paths = fs::read_dir(path).expect("i failed");

    for path_result in paths {
        let dir_entry = path_result.expect("i failed");
        let path = dir_entry.path();

        let file_name = dir_entry.file_name();

        let new_out = out.join(file_name).with_extension("");

        if path.is_file() {
            let ext = path.extension().unwrap_or_default();

            if ext != from_ext {
                continue;
            }

            callback(path.to_path_buf(), new_out).expect("i failed");
        } else if is_recursive {
            iterate_files(from_ext, path, new_out, is_recursive, callback);
        }
    }
}

fn from_dds(matches: &ArgMatches) {
    let args = get_arguments(matches);
    iterate_files("dds", args.path, args.out, args.recursive, lib::from_dds);
}

fn to_dds(matches: &ArgMatches) {
    let args = get_arguments(matches);
    iterate_files("ddj", args.path, args.out, args.recursive, lib::to_dds);
}

fn from_bmp(matches: &ArgMatches) {
    let args = get_arguments(matches);
    iterate_files("bmp", args.path, args.out, args.recursive, lib::from_bmp);
}
fn to_bmp(matches: &ArgMatches) {
    let args = get_arguments(matches);
    iterate_files("ddj", args.path, args.out, args.recursive, lib::to_bmp);
}
