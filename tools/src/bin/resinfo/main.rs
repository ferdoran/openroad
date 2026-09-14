// This bin keeps its helper module in `lib.rs`; allow the special-module-name lint.
#![allow(special_module_name)]

use std::collections::HashMap;
use std::io::BufRead;

use crate::lib::Case::Unknown;
use crate::lib::{Case, Section, StructDef, Val, Value};

mod convert;
mod lib;

/// Input/output come from the command line rather than constants: this tool
/// used to hardcode absolute paths from one contributor's machine, which made
/// it unrunnable for anyone else (STATUS.md §17).
fn usage() -> ! {
    eprintln!(
        "usage: resinfo <pstitle.txt> [pstitle.yaml]\n\n\
         Reads a resinfo pstitle.txt extracted from your own Media.pk2 and\n\
         writes the parsed YAML beside it (or to the given path)."
    );
    std::process::exit(2);
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(ps_title) = args.next() else { usage() };
    let ps_title_yaml = args
        .next()
        .unwrap_or_else(|| ps_title.replace(".txt", ".yaml"));

    let file =
        std::fs::File::open(&ps_title).unwrap_or_else(|e| panic!("cannot open {ps_title}: {e}"));
    let lines = std::io::BufReader::new(file).lines();

    let mut sections: Vec<Section> = Vec::new();
    let mut last_case = Unknown;
    let mut current_section: Option<Section> = None;
    let mut current_struct: Option<StructDef> = None;

    let l = lines
        .skip(3)
        .map(|line| line.expect("i failed"))
        .map(|line| String::from(line.trim()));
    let mut i = 0;
    for line in l {
        if line.is_empty() || line.starts_with("#") {
            i += 1;
            continue;
        }
        let case = Case::from(&line);
        match last_case {
            Case::Start => {
                match case {
                    Case::Struct(name, _typ) => {
                        if current_struct.is_some() {
                            panic!("tried to open new struct in line {}: current one has not been closed yet", i);
                        }
                        current_struct = Some(StructDef {
                            name,
                            entries: HashMap::new(),
                        });
                    }
                    Case::StructVal(key, typ, val) => {
                        if current_struct.is_none() {
                            panic!("failed to add new struct value in line {}: there is no open struct", i);
                        }
                        current_struct
                            .as_mut()
                            .expect("i failed")
                            .entries
                            .insert(key, Value::from(Val { typ, val }));
                    }
                    _ => {
                        panic!("unexpected case in line {}: {:?}", i, case);
                    }
                }
            }
            Case::End => {
                match case {
                    Case::End => {
                        if current_struct.is_some() {
                            panic!(
                                "failed to close section in line {}: current struct is still open",
                                i
                            );
                        }
                        if current_section.is_none() {
                            panic!("failed to close section in line {}: current section is already closed", i);
                        }
                        sections.push(current_section.expect("i failed"));
                        current_section = None;
                    }
                    Case::Section(name) => {
                        if current_section.is_some() {
                            panic!("begin of new section in line {} although current section is still open", i);
                        }
                        current_section = Some(Section {
                            name,
                            structs: Vec::new(),
                        });
                    }
                    Case::Struct(name, _typ) => {
                        if current_struct.is_some() {
                            panic!("begin of new struct in line {} although current struct is still open", i);
                        }
                        current_struct = Some(StructDef {
                            name,
                            entries: HashMap::new(),
                        });
                    }
                    _ => {
                        panic!("unexpected case in line {}: '{}'(is_empty = {}). case: {:?}. last case: {:?}", i, line, line.is_empty(), case, last_case);
                    }
                }
            }
            Case::Section(_) | Case::Struct(_, _) => match case {
                Case::Start => {}
                _ => {
                    panic!("unexpected case in line {}: {:?}", i, case);
                }
            },
            Case::StructVal(_, _, _) => match case {
                Case::StructVal(key, typ, val) => {
                    if current_struct.is_none() {
                        panic!(
                            "failed to add struct val in line {}: current struct is None",
                            i
                        );
                    }

                    current_struct
                        .as_mut()
                        .expect("i failed")
                        .add_entry(key, Value::from(Val { typ, val }));
                }
                Case::End => {
                    if current_struct.is_none() {
                        panic!(
                            "failed to close struct in line {}: current struct is None",
                            i
                        );
                    }
                    if current_section.is_none() {
                        panic!(
                            "failed to close struct in line {}: current section is None",
                            i
                        );
                    }

                    let cur_struct = current_struct.expect("i failed");
                    let current_section = current_section.as_mut().expect("i failed");
                    current_section.structs.push(cur_struct);
                    current_struct = None;
                }
                _ => {
                    panic!("unexpected case in line {}: {:?}", i, case);
                }
            },
            Unknown => match case {
                Case::Section(name) => {
                    if current_section.is_some() {
                        panic!("failed to open new section in line {}: there already is an open section", i);
                    } else {
                        current_section = Some(Section {
                            name,
                            structs: Vec::new(),
                        });
                    }
                }
                _ => {
                    panic!("unexpected case in line {}: {:?}", i, case);
                }
            },
        };
        last_case = Case::from(&line);
        i += 1;
    }

    let yaml = serde_yaml::to_string(&sections).expect("failed to yamlize data");
    std::fs::write(ps_title_yaml, yaml).expect("failed to write output yaml");
}
