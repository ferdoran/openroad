//! Chat window settings: per-channel text colors as AARRGGBB hex strings so
//! they can be customized in `config.yaml` without recompiling. Invalid hex
//! values warn and fall back to the built-in default at resolve time.

use bevy::prelude::*;
use serde::Deserialize;

#[derive(Deserialize, Debug, Clone, Default)]
pub struct ChatSettings {
    #[serde(default)]
    pub colors: ChatColorSettings,
    /// Fade the chat chrome out after a few idle seconds.
    ///
    /// Non-original: the v1.188 client has no idle fade — it ships a manual
    /// transparency slider instead (docs/re/ui/hud-chat.md §6-13). Off by
    /// default so the stock client matches the original; the locked rule keeps
    /// non-original behaviour behind a flag.
    #[serde(default)]
    pub idle_fade: bool,
}

/// AARRGGBB hex per chat channel.
///
/// UNKNOWN (docs/re/ui/hud-chat.md §9-U3): these are **not** the original's
/// values. No per-channel text-colour table exists anywhere in the PK2 — every
/// `GDR_LIST_*` in `ifchatviewer.txt` carries white, and the real colours are
/// compiled into the client — so the defaults below are our own choice, kept
/// config-driven precisely because the data cannot settle them.
#[derive(Deserialize, Debug, Clone)]
#[serde(default)]
pub struct ChatColorSettings {
    /// Normal/local chat (and NPC lines).
    pub normal: String,
    pub whisper: String,
    pub party: String,
    pub guild: String,
    /// GM chat, notices and client info lines.
    pub gm_notice: String,
    pub union: String,
    pub academy: String,
    pub global: String,
}

impl Default for ChatColorSettings {
    fn default() -> Self {
        Self {
            normal: "FFFFFFFF".into(),
            whisper: "FF00FFFF".into(),
            party: "FF00FF00".into(),
            guild: "FFFFB541".into(),
            gm_notice: "FFFF00FF".into(),
            union: "FFC2F573".into(),
            academy: "FF64C7FF".into(),
            global: "FFFFFF00".into(),
        }
    }
}

/// The parsed, ready-to-use colors (a resource so UI systems don't re-parse
/// hex strings per frame).
#[derive(Resource, Debug, Clone)]
pub struct ChatColors {
    pub normal: Color,
    pub whisper: Color,
    pub party: Color,
    pub guild: Color,
    pub gm_notice: Color,
    pub union: Color,
    pub academy: Color,
    pub global: Color,
}

impl ChatColorSettings {
    pub fn resolved(&self) -> ChatColors {
        let defaults = ChatColorSettings::default();
        let parse = |field: &str, value: &str, default: &str| {
            parse_argb(value).unwrap_or_else(|| {
                warn!("config: invalid chat color {field}={value:?}, using {default}");
                parse_argb(default).expect("default chat colors are valid")
            })
        };
        ChatColors {
            normal: parse("normal", &self.normal, &defaults.normal),
            whisper: parse("whisper", &self.whisper, &defaults.whisper),
            party: parse("party", &self.party, &defaults.party),
            guild: parse("guild", &self.guild, &defaults.guild),
            gm_notice: parse("gm_notice", &self.gm_notice, &defaults.gm_notice),
            union: parse("union", &self.union, &defaults.union),
            academy: parse("academy", &self.academy, &defaults.academy),
            global: parse("global", &self.global, &defaults.global),
        }
    }
}

/// `AARRGGBB` (or `RRGGBB`, alpha FF) hex → sRGB color.
pub(crate) fn parse_argb(hex: &str) -> Option<Color> {
    let hex = hex.trim().trim_start_matches("0x").trim_start_matches('#');
    let value = u32::from_str_radix(hex, 16).ok()?;
    let (a, r, g, b) = match hex.len() {
        8 => (
            (value >> 24) as u8,
            (value >> 16) as u8,
            (value >> 8) as u8,
            value as u8,
        ),
        6 => (0xFF, (value >> 16) as u8, (value >> 8) as u8, value as u8),
        _ => return None,
    };
    Some(Color::srgba_u8(r, g, b, a))
}

/// The palette a default (or absent) `ChatColorSettings` block resolves to.
///
/// Needed because the resource is now `init_resource`d and (re)filled by an
/// apply system whenever `ClientConfig` changes — see
/// [`crate::plugins::settings::live`] — instead of being derived once in
/// `Plugin::build`, where a later config edit could never reach it.
impl Default for ChatColors {
    fn default() -> Self {
        ChatColorSettings::default().resolved()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_aarrggbb() {
        assert_eq!(
            parse_argb("FFFFB541"),
            Some(Color::srgba_u8(0xFF, 0xB5, 0x41, 0xFF))
        );
        assert_eq!(
            parse_argb("8000FF00"),
            Some(Color::srgba_u8(0x00, 0xFF, 0x00, 0x80))
        );
        assert_eq!(parse_argb("B541"), None);
        assert_eq!(parse_argb("nothex00"), None);
    }
}
