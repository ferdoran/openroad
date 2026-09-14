//! `resinfo/ItemRare.txt` — the rare ("Seal of …") item aura table.
//!
//! Each row maps a rare item code to the effect the original client attaches
//! when the item is equipped: the `.efp` path, a scale (percent), and the
//! **weapon bone** it hangs off (`ai_end` = blade tip, `ai_start` = base,
//! `Bone01`/`Bone03` = bow/shield mounts). Tab-separated ASCII:
//! `CODE \t system\foo.efp \t 130 \t ai_end \t none`.

use bevy::asset::io::Reader;
use bevy::asset::{Asset, AssetLoader, LoadContext};
use bevy::prelude::TypePath;

use crate::assets::textdata::decode::decode_textdata;
use std::collections::HashMap;

/// The aura attached to one rare item when equipped.
#[derive(Clone, Debug)]
pub struct RareEffect {
    /// Effect asset path (`particles://system/...efp`).
    pub effect: String,
    /// Attach scale (the table's percent column ÷ 100).
    pub scale: f32,
    /// Weapon bone the effect hangs off (`ai_end`, `ai_start`, `Bone01`, …).
    pub bone: String,
    /// Minimum enhancement (+N opt level) required — `ItemOptionEfp.txt`
    /// rows carry it in column 5 (e.g. the +8 enchant flare); `None` for
    /// the unconditional `ItemRare.txt` raretype rows.
    pub min_opt: Option<u8>,
}

/// `ItemRare.txt` keyed by item code name (`ITEM_CH_SWORD_01_A_RARE`).
/// One code can carry several rows — e.g. the C-tier blade aura is a blade
/// wrap at `ai_start` *plus* a `_add` tip flash at `ai_end` — so every row
/// is kept.
#[derive(Asset, TypePath, Clone, Debug, Default)]
pub struct ItemRareTable(pub HashMap<String, Vec<RareEffect>>);

#[derive(Default, bevy::reflect::TypePath)]
pub struct ItemRareLoader;

impl AssetLoader for ItemRareLoader {
    type Asset = ItemRareTable;
    type Settings = ();
    type Error = std::io::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await?;
        Ok(ItemRareTable(parse_table(&buf)))
    }

    fn extensions(&self) -> &[&str] {
        // Loaded by explicit typed handle; the ".txt" is shared with other
        // loaders and disambiguated by the requested asset type.
        &[]
    }
}

/// Parse `itemoptionefp.txt` into `item codename -> effects`.
///
/// CP949, not Latin-1: the file carries Korean in its `//` header comment.
/// `decode_textdata` picks the encoding by BOM and falls back to CP949, which
/// decodes plain ASCII unchanged.
///
/// The `//` header row is filtered explicitly: it has 8 columns, so the arity
/// check below let it through and it landed in the table keyed
/// `"//Item Code Name"`.
fn parse_table(bytes: &[u8]) -> HashMap<String, Vec<RareEffect>> {
    let text = decode_textdata(bytes);
    let mut table: HashMap<String, Vec<RareEffect>> = HashMap::new();
    for line in text.lines() {
        if line.trim_start().starts_with("//") {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 4 {
            continue;
        }
        let scale = cols[2].trim().parse::<f32>().unwrap_or(100.0) / 100.0;
        table
            .entry(cols[0].trim().to_string())
            .or_default()
            .push(RareEffect {
                effect: format!("particles://{}", cols[1].trim().replace('\\', "/")),
                scale,
                bone: cols[3].trim().to_string(),
                min_opt: cols.get(5).and_then(|c| c.trim().parse().ok()),
            });
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `//` header row has 8 columns, so the `cols.len() < 4` guard let it
    /// through and it was inserted keyed "//Item Code Name" (#296).
    #[test]
    fn the_header_comment_row_is_not_data() {
        let text = "//Item Code Name\tEffect\tScale\tBone\tx\tMinOpt\ty\tz\r\n\
                    ITEM_A\tres\\fx\\a.efp\t150\tBip01\t0\t3\r\n";
        let table = parse_table(text.as_bytes());
        assert!(
            !table.keys().any(|k| k.starts_with("//")),
            "header leaked in"
        );
        assert_eq!(table.len(), 1);
        let effects = table.get("ITEM_A").expect("real row parsed");
        assert_eq!(effects[0].effect, "particles://res/fx/a.efp");
        assert_eq!(effects[0].scale, 1.5);
        assert_eq!(effects[0].bone, "Bip01");
        assert_eq!(effects[0].min_opt, Some(3));
    }

    /// CP949, not Latin-1: Korean in the header must not corrupt the parse,
    /// and a UTF-16LE BOM must still be honoured.
    #[test]
    fn cp949_and_bom_inputs_both_decode() {
        // CP949 bytes for a Korean word inside a comment line
        let mut cp949 = b"//".to_vec();
        cp949.extend_from_slice(&[0xC7, 0xD1, 0xB1, 0xB9]); // "\ud55c\uad6d"
        cp949.extend_from_slice(b"\r\nITEM_B\tres\\b.efp\t100\tBip01\r\n");
        let table = parse_table(&cp949);
        assert_eq!(table.len(), 1);
        assert!(table.contains_key("ITEM_B"));

        let utf16: Vec<u16> = "ITEM_C\tres\\c.efp\t100\tBip01\r\n"
            .encode_utf16()
            .collect();
        let mut bytes = vec![0xFF, 0xFE];
        for u in utf16 {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
        let table = parse_table(&bytes);
        assert!(
            table.contains_key("ITEM_C"),
            "BOM-prefixed UTF-16LE decodes"
        );
    }
}
