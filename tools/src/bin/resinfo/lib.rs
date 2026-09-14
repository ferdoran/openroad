extern crate core;

use std::collections::HashMap;
use std::str::FromStr;

use lazy_static::lazy_static;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::Case::Unknown;

lazy_static! {
    static ref SECTION_REGEX: Regex =
        Regex::new(r#"^Section = (?P<name>\w+),"(\d,)*\d","\d"$"#).expect("i failed");
    static ref STRUCT_NAME_REGEX: Regex =
        Regex::new(r#"^(?P<name>[a-zA-Z0-9_]+):(?P<type>[a-zA-Z0-9_]+?)+$"#).expect("i failed");
    /// The trailing `\s*\.?` is not cosmetic: `Media/resinfo/ifallianceguild.txt:78`
    /// is the corpus's one malformed line — `SubSection=STRING,"".` with a stray
    /// period after the closing quote (`docs/re/ui/hud-guild-diplomacy.md`, parser
    /// hazards). Anchored strictly at `"$` it matches 3739 of 3740 value lines and
    /// this one falls through to `Case::Unknown`, which `main` turns into a panic;
    /// a naive comma-split-and-strip instead reads the value as `.` and invents a
    /// child subtree named `.` under `GDR_ALLIANCE_INFO_NOT`. Accepting the period
    /// as trailing noise is the only reading that neither loses the line nor
    /// fabricates a value.
    static ref STRUCT_VAL_REGEX: Regex =
        Regex::new(r#"^(?P<key>[a-zA-Z0-9_]+)=(?P<type>[a-zA-Z0-9_]+),"(?P<value>.*)"\s*\.?\s*$"#)
            .expect("i failed");
}

#[derive(Serialize, Deserialize)]
pub struct Section {
    pub(crate) name: String,
    pub(crate) structs: Vec<StructDef>,
}

#[derive(Serialize, Deserialize)]
pub struct StructDef {
    pub(crate) name: String,
    pub(crate) entries: HashMap<String, Value>,
}

impl StructDef {
    pub fn add_entry(&mut self, key: String, val: Value) {
        self.entries.insert(key, val);
    }
}

#[derive(Serialize, Deserialize)]
pub struct Val {
    pub(crate) typ: String,
    pub(crate) val: String,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Integer(i32),
    Str(String),
    Point {
        x: i32,
        y: i32,
    },
    Rect {
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    },
    Color {
        a: i32,
        r: i32,
        g: i32,
        b: i32,
    },
}

impl From<Val> for Value {
    fn from(value: Val) -> Self {
        match value.typ.as_str() {
            "INTEGER" => Value::Integer(value.val.parse::<i32>().expect("i failed")),
            "STRING" => Value::Str(value.val),
            "POINT" => {
                let mut nums = value
                    .val
                    .split(",")
                    .map(|n| i32::from_str(n).expect("i failed"));
                let x = nums.next().expect("i failed");
                let y = nums.next().expect("i failed");
                Value::Point { x, y }
            }
            "RECT" => {
                let mut nums = value
                    .val
                    .split(",")
                    .map(|n| i32::from_str(n).expect("i failed"));
                let x = nums.next().expect("i failed");
                let y = nums.next().expect("i failed");
                let width = nums.next().expect("i failed");
                let height = nums.next().expect("i failed");
                Value::Rect {
                    x,
                    y,
                    width,
                    height,
                }
            }
            "COLOR" => {
                let mut nums = value
                    .val
                    .split(",")
                    .map(|n| i32::from_str(n).expect("i failed"));
                let a = nums.next().expect("i failed");
                let r = nums.next().expect("i failed");
                let g = nums.next().expect("i failed");
                let b = nums.next().expect("i failed");
                Value::Color { r, g, b, a }
            }
            _ => {
                panic!("invalid type: {}", value.typ);
            }
        }
    }
}

#[derive(Debug)]
pub enum Case {
    Start,
    End,
    Section(String),
    Struct(String, String),
    StructVal(String, String, String),
    Unknown,
}

impl From<&String> for Case {
    fn from(line: &String) -> Self {
        let line = line.as_str();
        match line {
            "{" => Case::Start,
            "}" => Case::End,
            _ => {
                if SECTION_REGEX.is_match(line) {
                    let capt = SECTION_REGEX.captures(line).expect("i failed");
                    let name = capt.name("name").expect("i failed").as_str();
                    Case::Section(String::from(name))
                } else if STRUCT_NAME_REGEX.is_match(line) {
                    let capt = STRUCT_NAME_REGEX.captures(line).expect("i failed");
                    let name = capt.name("name").expect("i failed").as_str();
                    let ty = capt.name("type").expect("i failed").as_str();
                    Case::Struct(String::from(name), String::from(ty))
                } else if STRUCT_VAL_REGEX.is_match(line) {
                    let capt = STRUCT_VAL_REGEX.captures(line).expect("i failed");
                    let key = capt.name("key").expect("i failed").as_str();
                    let ty = capt.name("type").expect("i failed").as_str();
                    let value = capt.name("value").expect("i failed").as_str();
                    Case::StructVal(String::from(key), String::from(ty), String::from(value))
                } else {
                    Unknown
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn struct_val(line: &str) -> (String, String, String) {
        match Case::from(&String::from(line)) {
            Case::StructVal(key, ty, value) => (key, ty, value),
            other => panic!("expected a struct value, got {other:?}"),
        }
    }

    /// Ordinary value lines, one per value type the grammar uses.
    #[test]
    fn struct_values_parse_for_every_value_type() {
        assert_eq!(
            struct_val(r#"Rect=RECT,"6,29,440,285""#),
            ("Rect".into(), "RECT".into(), "6,29,440,285".into())
        );
        assert_eq!(
            struct_val(r#"DDJ=STRING,"com_bg_tile_d.ddj""#),
            ("DDJ".into(), "STRING".into(), "com_bg_tile_d.ddj".into())
        );
        assert_eq!(
            struct_val(r#"FontColor=COLOR,"255,239,218,164""#),
            ("FontColor".into(), "COLOR".into(), "255,239,218,164".into())
        );
    }

    /// `ifallianceguild.txt:78` verbatim — the corpus's single malformed line.
    /// It must parse as an empty `SubSection`, not vanish and not yield `.`.
    #[test]
    fn the_malformed_ifallianceguild_line_parses_as_an_empty_value() {
        assert_eq!(
            struct_val(r#"SubSection=STRING,""."#),
            ("SubSection".into(), "STRING".into(), String::new())
        );
    }

    /// The relaxation must not swallow a real value's own trailing characters:
    /// a period *inside* the quotes still belongs to the value.
    #[test]
    fn a_period_inside_the_quotes_stays_part_of_the_value() {
        assert_eq!(
            struct_val(r#"DDJ=STRING,"gil_war_tab_01.ddj""#),
            ("DDJ".into(), "STRING".into(), "gil_war_tab_01.ddj".into())
        );
        assert_eq!(
            struct_val(r#"Text=STRING,"Done.""#),
            ("Text".into(), "STRING".into(), "Done.".into())
        );
    }
}
