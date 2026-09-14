//! Nameplate settings: floating name-label colors as AARRGGBB hex strings so
//! they can be customized in `config.yaml` without recompiling. Invalid hex
//! values warn and fall back to the built-in default at resolve time (mirrors
//! the chat color settings).

use bevy::prelude::*;
use serde::Deserialize;

use crate::plugins::config::chat::parse_argb;

#[derive(Deserialize, Debug, Clone)]
#[serde(default)]
pub struct NameplateSettings {
    pub colors: NameplateColorSettings,
    /// Render the hovered/selected label in a bold face. **Deviation from the
    /// original** (ADR 0009): the PK2's three UI faces have no bold cut, so
    /// this readability tweak is opt-out rather than silent. `false` keeps the
    /// original's single face and leaves only the underline as emphasis.
    pub hover_bold: bool,
}

impl Default for NameplateSettings {
    fn default() -> Self {
        Self {
            colors: NameplateColorSettings::default(),
            hover_bold: true,
        }
    }
}

/// AARRGGBB hex per nameplate kind.
#[derive(Deserialize, Debug, Clone)]
#[serde(default)]
pub struct NameplateColorSettings {
    /// Your own character's name.
    pub local_player: String,
    /// Other players.
    pub player: String,
    pub npc: String,
    pub monster: String,
    /// Unique/world-boss monsters.
    pub unique_monster: String,
    /// A plain ground drop's hover label.
    pub item: String,
    /// A ground drop whose spawn record's rarity byte is non-zero, i.e. one
    /// carrying magic ("blue") options. Matches the inventory tooltip's
    /// `MAGIC_COLOR`.
    pub item_magic: String,
    /// A Seal-grade ("_RARE") ground drop — the ones that also get the sparkle
    /// pillar. Matches the inventory tooltip's `NAME_COLOR`.
    pub item_rare: String,
}

impl Default for NameplateColorSettings {
    fn default() -> Self {
        Self {
            local_player: "FFFFFFFF".into(),
            player: "FFB2D9FF".into(),
            npc: "FFB2D9FF".into(),
            monster: "FFFFFFFF".into(),
            unique_monster: "FFFF8C1A".into(),
            item: "FFFFFFFF".into(),
            // the inventory tooltip's MAGIC_COLOR / NAME_COLOR, so an item
            // reads the same colour on the ground and in the bag
            item_magic: "FF80B3FF".into(),
            item_rare: "FFFFD952".into(),
        }
    }
}

/// The parsed, ready-to-use colors (a resource so the per-frame update system
/// doesn't re-parse hex strings).
#[derive(Resource, Debug, Clone)]
pub struct NameplateColors {
    pub local_player: Color,
    pub player: Color,
    pub npc: Color,
    pub monster: Color,
    pub unique_monster: Color,
    pub item: Color,
    pub item_magic: Color,
    pub item_rare: Color,
}

/// The palette a default (or absent) `NameplateColorSettings` block resolves to.
///
/// Needed because the resource is now `init_resource`d and (re)filled by an
/// apply system whenever `ClientConfig` changes — see
/// [`crate::plugins::settings::live`] — instead of being derived once in
/// `Plugin::build`, where a later config edit could never reach it.
impl Default for NameplateColors {
    fn default() -> Self {
        NameplateColorSettings::default().resolved()
    }
}

impl NameplateColorSettings {
    pub fn resolved(&self) -> NameplateColors {
        let defaults = NameplateColorSettings::default();
        let parse = |field: &str, value: &str, default: &str| {
            parse_argb(value).unwrap_or_else(|| {
                warn!("config: invalid nameplate color {field}={value:?}, using {default}");
                parse_argb(default).expect("default nameplate colors are valid")
            })
        };
        NameplateColors {
            local_player: parse("local_player", &self.local_player, &defaults.local_player),
            player: parse("player", &self.player, &defaults.player),
            npc: parse("npc", &self.npc, &defaults.npc),
            monster: parse("monster", &self.monster, &defaults.monster),
            unique_monster: parse(
                "unique_monster",
                &self.unique_monster,
                &defaults.unique_monster,
            ),
            item: parse("item", &self.item, &defaults.item),
            item_magic: parse("item_magic", &self.item_magic, &defaults.item_magic),
            item_rare: parse("item_rare", &self.item_rare, &defaults.item_rare),
        }
    }
}
