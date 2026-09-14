use std::collections::HashMap;
use std::ops::Deref;

use crate::assets::ifo::environment::JMXVENVI;
use crate::assets::ifo::object::{ObjectInfo, ObjectInfoIndex};
use crate::assets::ifo::tile::{TileInfo, TileInfoIndex};
use crate::assets::textdata::decode::decode_textdata;
use bevy::asset::io::Reader;
use bevy::asset::{Asset, AssetLoader, LoadContext};
use bevy::prelude::{warn, TypePath};
use thiserror::Error;

pub mod environment;
pub mod object;
pub mod tile;

#[derive(Default, bevy::reflect::TypePath)]
pub struct IFOLoader;

#[derive(Error, Debug)]
pub enum IFOLoaderError {
    #[error("failed to convert OsStr to &str")]
    OsStrToStr,
    #[error("no file steam")]
    NoFileStem,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// A `.ifo` stem this loader has no parser for. The extension is claimed
    /// for every `.ifo`, but only three of the thirteen shipped files are
    /// modelled; the rest are skipped instead of aborting the process.
    #[error("no parser for {0}.ifo")]
    UnsupportedKind(String),
    #[error("malformed {kind}.ifo: {reason}")]
    Malformed {
        kind: &'static str,
        reason: &'static str,
    },
}

/// Read the `signature` + `count` preamble both text `.ifo` tables share.
fn read_count<I: Iterator<Item = String>>(
    lines: &mut I,
    kind: &'static str,
) -> Result<usize, IFOLoaderError> {
    let _sig = lines.next().ok_or(IFOLoaderError::Malformed {
        kind,
        reason: "missing signature line",
    })?;
    lines
        .next()
        .ok_or(IFOLoaderError::Malformed {
            kind,
            reason: "missing count line",
        })?
        .trim()
        .parse::<usize>()
        .map_err(|_| IFOLoaderError::Malformed {
            kind,
            reason: "count line is not a number",
        })
}

#[derive(Asset, Default, TypePath)]
#[allow(dead_code)]
pub struct IFOAsset {
    pub environment: Option<JMXVENVI>,
    pub object_info_index: Option<ObjectInfoIndex>,
    pub tile_info_index: Option<TileInfoIndex>,
}

impl AssetLoader for IFOLoader {
    type Asset = IFOAsset;
    type Settings = ();
    type Error = IFOLoaderError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await?;
        let bytes = buf.deref();
        match load_context
            .path()
            .path()
            .file_stem()
            .map(|stem| stem.to_str())
            .ok_or(IFOLoaderError::OsStrToStr)?
            .ok_or(IFOLoaderError::NoFileStem)?
        {
            "environment" => {
                let env = JMXVENVI::from(bytes);
                return Ok(IFOAsset {
                    environment: Some(env),
                    tile_info_index: None,
                    object_info_index: None,
                });
                // load_context.add_labeled_asset()
                // let loaded_asset = load_context.finish(env, None);
                // return Ok(env);
            }
            "object" => {
                // Decode the whole buffer up front instead of BufRead::lines():
                // that yields Result per line and the old `.flatten()` silently
                // dropped every Err — which is every CP949 line, since Korean
                // text is not valid UTF-8. That was the line loss this fixes.
                let text = decode_textdata(bytes);
                let mut lines = text.lines().map(str::to_owned);
                let count = read_count(&mut lines, "object")?;
                let mut object_infos = HashMap::new();
                for _ in 0..count {
                    let Some(line) = lines.next() else { break };
                    match ObjectInfo::parse(&line) {
                        Some(object_info) => {
                            object_infos.insert(object_info.id, object_info);
                        }
                        None => warn!("ifo: skipping malformed object row: {line:?}"),
                    }
                }
                return Ok(IFOAsset {
                    environment: None,
                    tile_info_index: None,
                    object_info_index: Some(ObjectInfoIndex(object_infos)),
                });
                // return Ok(ObjectInfoIndex(object_infos));
            }
            "tile2d" => {
                let text = decode_textdata(bytes);
                let mut lines = text.lines().map(str::to_owned);
                let count = read_count(&mut lines, "tile2d")?;
                let mut tiles = HashMap::new();
                for _ in 0..count {
                    if let Some(line) = lines.next() {
                        let tile_info = TileInfo::from(line.to_owned());
                        tiles.insert(tile_info.id, tile_info);
                    }
                }
                return Ok(IFOAsset {
                    tile_info_index: Some(TileInfoIndex { tiles }),
                    environment: None,
                    object_info_index: None,
                });
            }
            // objectstring / eventstring / layerobjectlist / objext / tile3d
            // / config are all shipped but unmodelled. Two are not even the
            // same shape: tile3d.ifo is 2 bytes ("0\n", no signature and no
            // count), and config.ifo is a JMXVCAMR1002 binary that belongs to
            // the CAMR parser, not here.
            other => Err(IFOLoaderError::UnsupportedKind(other.to_string())),
        }
    }

    fn extensions(&self) -> &[&str] {
        &["ifo"]
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// #294: `object.ifo` holds 4 CP949 lines (e.g. id 00090 `박스.bsr`).
    /// `BufReader::lines()` yields `Err` for those and the old `.flatten()`
    /// dropped them silently, so 4 of 2,767 map objects vanished and the grass
    /// pairs referencing them logged "missing from object.ifo". Decoding the
    /// whole file as CP949 keeps them — and matches `bevy_pk2`, which already
    /// decodes archive entry names as Korean.
    #[test]
    fn cp949_object_lines_survive_instead_of_being_dropped() {
        let mut bytes = b"JMXVOBJI1000\r\n2\r\n00090 ".to_vec();
        bytes.extend_from_slice(&[0xB9, 0xDA, 0xBD, 0xBA]); // CP949 "박스"
        bytes.extend_from_slice(b".bsr\r\n00091 plain.bsr\r\n");

        // what BufReader::lines() did with that line
        assert!(
            String::from_utf8(bytes.clone()).is_err(),
            "fixture must be invalid UTF-8, else it proves nothing"
        );

        let text = decode_textdata(&bytes);
        let lines: Vec<&str> = text.lines().collect();

        assert_eq!(lines.len(), 4, "no line may be dropped");
        assert!(lines[2].contains("박스.bsr"), "got {:?}", lines[2]);
    }
}
