use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::ops::Deref;

use bevy::asset::{io::Reader, AssetLoader, LoadContext};
use bevy::log::warn;
use thiserror::Error;

use crate::assets::resinfo::interface_text::{Case, Element, InterfaceText, Section, Value};

#[derive(Default, bevy::reflect::TypePath)]
pub struct InterfaceTextLoader;

#[derive(Error, Debug)]
pub enum InterfaceTextLoaderError {
    #[error("unsupported file extension: {0}")]
    UnsupportedFileExtension(&'static str),
    #[error("not an interface text")]
    InvalidData,
    #[error("empty file")]
    EmptyFile,
    #[error("IO error: {0}")]
    IO(#[from] std::io::Error),
    #[error("YAML error: {0}")]
    Yaml(serde_yaml::Error),
}

impl AssetLoader for InterfaceTextLoader {
    type Asset = InterfaceText;
    type Settings = ();
    type Error = InterfaceTextLoaderError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await?;
        let bytes = buf.deref();
        if let Some(ext) = load_context.path().path().extension() {
            match ext.to_str() {
                Some("yaml") | Some("yml") => {
                    match serde_yaml::from_reader::<BufReader<&[u8]>, InterfaceText>(
                        BufReader::new(bytes),
                    ) {
                        Ok(interface_text) => {
                            return Ok(interface_text);
                        }
                        Err(e) => {
                            return Err(InterfaceTextLoaderError::Yaml(e));
                        }
                    }
                }
                Some("txt") => {}
                _ => {
                    return Err(InterfaceTextLoaderError::UnsupportedFileExtension("txt"));
                }
            }
        }
        let buf_reader = BufReader::new(bytes);
        let mut lines = buf_reader.lines();
        if let Some(header) = lines.next() {
            match header {
                Ok(header) => {
                    if header != "Interface Text" {
                        return Err(InterfaceTextLoaderError::InvalidData);
                    }
                }
                Err(e) => return Err(InterfaceTextLoaderError::IO(e)),
            }
        } else {
            return Err(InterfaceTextLoaderError::EmptyFile);
        }

        let lines = lines
            .map_while(Result::ok)
            .map(|line| String::from(line.trim()))
            .collect::<Vec<String>>();

        let interface_text = InterfaceText::from(lines);

        Ok(interface_text)
    }

    fn extensions(&self) -> &[&str] {
        &["txt", "yaml"]
    }
}

/// Parse a resinfo body **by key name**, never by line offset.
///
/// Idea: the file is a tiny brace grammar — `Section = Name,"…","…"`, then
/// `Name:Class` blocks of `Key=TYPE,"value"` lines — and the keys inside a
/// block are emitted **alphabetically**, so a block's key *set* decides where
/// any given key lands. A corpus census over all 3740 blocks found seven
/// distinct key-sets: `Rect` sits 9 lines below the header in 3403 of them and
/// 10/11/13/17 lines below in the other 337, because `CommandID` and
/// `HelpString` sort above `Rect` and push it down. Those 337 are exactly the
/// interactive controls — buttons, slots, anything with a help string — so a
/// line-offset reader is wrong precisely where it matters (#477).
///
/// Malformed input is **skipped, not fatal**: this parses user-supplied PK2
/// data, and one stray line in 247 files must not take the client down. The
/// corpus is known to contain at least two malformed blocks and one stray
/// trailing period.
pub fn parse_sections(lines: &[String]) -> Vec<Section> {
    let mut sections: Vec<Section> = Vec::new();
    let mut current_section: Option<Section> = None;
    let mut current_struct: Option<Element> = None;
    let mut skipped = 0usize;

    for (index, line) in lines.iter().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // 1-based, and the caller strips the "Interface Text" header line
        let line_no = index + 2;
        match Case::from(line) {
            Case::Section(name) => {
                // an unclosed previous section still yields what it collected
                if let Some(section) = current_section.take() {
                    sections.push(section);
                }
                current_section = Some(Section {
                    name,
                    structs: Vec::new(),
                });
            }
            Case::Struct(name, typ) => {
                if let (Some(section), Some(element)) =
                    (current_section.as_mut(), current_struct.take())
                {
                    section.structs.push(element);
                }
                current_struct = Some(Element {
                    name,
                    typ,
                    entries: HashMap::new(),
                });
            }
            Case::StructVal(key, typ, value) => match Value::parse(&typ, &value, line_no) {
                Ok(value) => match current_struct.as_mut() {
                    Some(element) => element.add_entry(key, value),
                    None => {
                        skipped += 1;
                        warn!("resinfo: {key} at line {line_no} is outside any block, skipped");
                    }
                },
                Err(e) => {
                    skipped += 1;
                    warn!("resinfo: {e}, skipped");
                }
            },
            Case::Start => {}
            Case::End => {
                // `}` closes the innermost open thing: a block if one is open,
                // otherwise the section
                if let Some(element) = current_struct.take() {
                    match current_section.as_mut() {
                        Some(section) => section.structs.push(element),
                        None => {
                            skipped += 1;
                            warn!("resinfo: block {} at line {line_no} is outside any section, skipped", element.name);
                        }
                    }
                } else if let Some(section) = current_section.take() {
                    sections.push(section);
                }
            }
            Case::Unknown => {
                skipped += 1;
                warn!("resinfo: unparsable line {line_no}: {line:?}, skipped");
            }
        }
    }

    // tolerate a truncated file rather than losing everything before the tear
    if let (Some(section), Some(element)) = (current_section.as_mut(), current_struct.take()) {
        section.structs.push(element);
    }
    if let Some(section) = current_section.take() {
        sections.push(section);
    }
    if skipped > 0 {
        warn!("resinfo: {skipped} malformed line(s) skipped");
    }
    sections
}

impl From<Vec<String>> for InterfaceText {
    fn from(lines: Vec<String>) -> Self {
        Self(parse_sections(&lines))
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::assets::resinfo::interface_text::Value;

    fn lines(body: &str) -> Vec<String> {
        body.lines().map(|l| l.trim().to_string()).collect()
    }

    /// The whole point of #477: a block whose key-set contains `CommandID`
    /// and `HelpString` sorts them above `Rect`, so `Rect` is no longer the
    /// 9th line of the block. Reading by key name is immune; a line-offset
    /// reader gets `HAlign` here.
    #[test]
    fn rect_is_read_by_key_name_not_by_line_offset() {
        let sections = parse_sections(&lines(
            r#"
            Section = Create,"0","0"
            {
                GDR_BTN:CIFButton
                {
                    ClientRect=RECT,"0,0,0,0"
                    Color=COLOR,"255,255,255,255"
                    CommandID=INTEGER,"7"
                    DDJ=STRING,"interface\\btn.ddj"
                    FontColor=COLOR,"255,255,255,255"
                    FontIndex=INTEGER,"0"
                    HAlign=INTEGER,"1"
                    HelpString=STRING,""
                    ID=INTEGER,"11"
                    Rect=RECT,"406,15,16,16"
                    Style=INTEGER,"0"
                    SubSection=STRING,""
                    Text=STRING,""
                }
            }
            "#,
        ));
        let block = &sections[0].structs[0];
        assert_eq!(block.name, "GDR_BTN");
        assert_eq!(block.typ, "CIFButton");
        assert_eq!(
            block.entries.get("Rect"),
            Some(&Value::Rect {
                x: 406,
                y: 15,
                width: 16,
                height: 16
            })
        );
        // the line-offset reading would have landed here instead
        assert_eq!(block.entries.get("HAlign"), Some(&Value::Integer(1)));
    }

    /// `ifallianceguild.txt:78` ships `SubSection=STRING,"".` — a stray
    /// trailing period. It used to miss the regex entirely and panic the
    /// parser; now it parses as the empty string.
    #[test]
    fn a_stray_trailing_period_parses_instead_of_panicking() {
        let sections = parse_sections(&lines(
            r#"
            Section = Create,"0","0"
            {
                GDR_X:CIFStatic
                {
                    SubSection=STRING,"".
                    ID=INTEGER,"3"
                }
            }
            "#,
        ));
        let block = &sections[0].structs[0];
        assert_eq!(
            block.entries.get("SubSection"),
            Some(&Value::Str(String::new()))
        );
        assert_eq!(block.entries.get("ID"), Some(&Value::Integer(3)));
    }

    /// Malformed lines are skipped, never fatal — this parses user PK2 data,
    /// and the corpus is known to contain malformed blocks.
    #[test]
    fn malformed_lines_are_skipped_and_the_rest_of_the_block_survives() {
        let sections = parse_sections(&lines(
            r#"
            Section = Create,"0","0"
            {
                GDR_X:CIFStatic
                {
                    Rect=RECT,"nonsense"
                    Color=COLOR,"1,2,3"
                    Weird=NOTATYPE,"1"
                    this line is not grammar at all
                    ID=INTEGER,"5"
                }
            }
            "#,
        ));
        let block = &sections[0].structs[0];
        assert_eq!(block.entries.get("ID"), Some(&Value::Integer(5)));
        assert_eq!(block.entries.get("Rect"), None);
        assert_eq!(block.entries.get("Color"), None);
        assert_eq!(block.entries.get("Weird"), None);
    }

    /// The corpus stores multi-line strings as literal two-character `\n`
    /// sequences (1160 of them across 28 files); read verbatim every one of
    /// those collapses into a run-on line.
    #[test]
    fn literal_backslash_n_is_unescaped_into_a_newline() {
        let sections = parse_sections(&lines(
            r#"
            Section = Create,"0","0"
            {
                GDR_X:CIFStatic
                {
                    Text=STRING,"first\nsecond"
                }
            }
            "#,
        ));
        let text = sections[0].structs[0].entries.get("Text").cloned();
        assert_eq!(text, Some(Value::Str("first\nsecond".to_string())));
    }
}
