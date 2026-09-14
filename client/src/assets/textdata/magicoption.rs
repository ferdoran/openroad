//! magicoption.txt — the definitions of item "blue" (magic) options.
//!
//! An equipment item's magic params (0x3013) reference these rows by id: each
//! [`MagicParam`](packets::agent::character_data::MagicParam)'s `kind` is the
//! option id (col 1) and its `value` is the rolled magnitude. The row supplies
//! the `MATTR_*` code name (col 2) and the display operator (col 3). The table
//! carries no localized `SN_*` key, so [`mattr_display`] maps the codes to
//! English inline.

use bevy::asset::Asset;
use bevy::prelude::TypePath;
use std::collections::HashMap;
use std::ops::Deref;

/// One magic-option ("blue" stat) definition.
#[derive(Debug, Clone)]
pub struct MagicOptionInfo {
    /// `MATTR_*` code name (col 2).
    pub codename: String,
    /// Display operator (col 3): `"+"`, `"-"`, `"-@"`.
    pub op: String,
}

/// magicoption.txt keyed by option id (col 1).
#[derive(Asset, TypePath, Debug, Clone)]
pub struct MagicOptionData(pub HashMap<u32, MagicOptionInfo>);

impl Deref for MagicOptionData {
    type Target = HashMap<u32, MagicOptionInfo>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl MagicOptionInfo {
    /// Whether the option is a flag-like effect shown without a magnitude
    /// (Immortal / Steady / Lucky / Astral and their set/avatar variants).
    pub fn is_flag(&self) -> bool {
        let c = &self.codename;
        c.contains("ATHANASIA")
            || c.contains("SOLID")
            || c.contains("LUCK")
            || c.contains("ASTRAL")
            || c == "MATTR_NOT_REPARABLE"
    }

    /// Whether the option is a **drawback** rather than a bonus — a blue line
    /// that costs the player something.
    ///
    /// Only `MATTR_NOT_REPARABLE` qualifies today, and it is the one the
    /// tooltip has to shout about: it rides along with a large
    /// `MATTR_DUR` ("Durability 400% increased") on the same item, so the two
    /// read as one uniformly-good block unless the drawback is coloured apart
    /// from it. `MATTR_DEC_MAXDUR` is *not* included — it is a percentage
    /// option whose own text already says "decreased", so the number carries
    /// the sign. Add codes here only when the text alone cannot.
    pub fn is_penalty(&self) -> bool {
        self.codename == "MATTR_NOT_REPARABLE"
    }

    /// Whether the option's magnitude is a percentage ("rate"-type blues:
    /// durability, attack/parry/block rate) rather than a flat point bonus
    /// (STR/INT/HP/MP/critical/resistances). The magicoption table has no
    /// unit column, so this is classified by code name.
    pub fn is_percent(&self) -> bool {
        let c = self.codename.as_str();
        matches!(
            c,
            "MATTR_DUR" | "MATTR_DUR_SET" | "MATTR_DEC_MAXDUR" | "MATTR_HR" | "MATTR_AVATAR_HR"
        ) || c.contains("_ER")
            || c.contains("BLOCKRATE")
    }

    /// The tooltip line for this option given the item's rolled `value`.
    pub fn format(&self, value: u32) -> String {
        let name = mattr_display(&self.codename);
        if self.is_flag() {
            name
        } else if self.is_percent() {
            let verb = if self.op.starts_with('-') {
                "decreased"
            } else {
                "increased"
            };
            format!("{name} {value}% {verb}")
        } else {
            let sign = if self.op.starts_with('-') { "-" } else { "+" };
            format!("{name} {sign}{value}")
        }
    }
}

/// English display name for a `MATTR_*` magic-option code. Set/avatar/3-job
/// variants fold onto their base stat; unknown codes fall back to a title-cased
/// version of the stripped code so nothing renders as a raw `MATTR_*` token.
///
/// The two primary stats are abbreviated **STR** / **INT** rather than spelled
/// out. They are the only entries here that name a stat the rest of the client
/// already abbreviates — the character window's own rows are `PARAM_STR` /
/// `PARAM_INT` — and a blue line reading "Strength +3" next to a white one
/// reading "STR" was the same stat under two names in one tooltip.
pub fn mattr_display(codename: &str) -> String {
    let known = match codename {
        "MATTR_STR" | "MATTR_STR_SET" | "MATTR_STR_3JOB" | "MATTR_STR_AVATAR"
        | "MATTR_AVATAR_STR" | "MATTR_AVATAR_STR_2" | "MATTR_AVATAR_STR_3"
        | "MATTR_AVATAR_STR_4" => "STR",
        "MATTR_INT" | "MATTR_INT_SET" | "MATTR_INT_3JOB" | "MATTR_INT_AVATAR"
        | "MATTR_AVATAR_INT" | "MATTR_AVATAR_INT_2" | "MATTR_AVATAR_INT_3"
        | "MATTR_AVATAR_INT_4" => "INT",
        "MATTR_DUR" | "MATTR_DUR_SET" => "Durability",
        "MATTR_DEC_MAXDUR" => "Max Durability",
        "MATTR_HR" | "MATTR_AVATAR_HR" => "Attack Rate",
        "MATTR_ER" | "MATTR_ER_SET" | "MATTR_AVATAR_ER" => "Parry Rate",
        "MATTR_HP" | "MATTR_HP_SET" | "MATTR_AVATAR_HP" => "HP",
        "MATTR_MP" | "MATTR_MP_SET" | "MATTR_AVATAR_MP" => "MP",
        "MATTR_AVATAR_HPRG" => "HP Recovery",
        "MATTR_AVATAR_MPRG" => "MP Recovery",
        "MATTR_REGENHPMP" => "HP/MP Recovery",
        "MATTR_CRITICAL" => "Critical",
        "MATTR_BLOCKRATE" | "MATTR_NASRUN_BLOCKRATE" => "Blocking Rate",
        "MATTR_EVADE_BLOCK" => "Ignore Block",
        "MATTR_EVADE_CRITICAL" => "Ignore Critical",
        "MATTR_ATHANASIA" => "Immortal",
        "MATTR_SOLID" => "Steady",
        "MATTR_LUCK"
        | "MATTR_LUCK_SET"
        | "MATTR_AVATAR_LUCK"
        | "MATTR_AVATAR_LUCK_2"
        | "MATTR_AVATAR_LUCK_3"
        | "MATTR_AVATAR_LUCK_4" => "Lucky",
        "MATTR_ASTRAL" => "Astral",
        "MATTR_REPAIR" | "MATTR_REINFORCE_ITEM" | "MATTR_REINFORCE_ITEM_SET" => "Reinforcement",
        "MATTR_NOT_REPARABLE" => "Not Repairable",
        "MATTR_RESIST_FROSTBITE" => "Frostbite Resistance",
        "MATTR_RESIST_ESHOCK" => "Electric Shock Resistance",
        "MATTR_RESIST_BURN" => "Burn Resistance",
        "MATTR_RESIST_POISON" => "Poison Resistance",
        "MATTR_RESIST_ZOMBIE" => "Zombie Resistance",
        "MATTR_RESIST_STUN" => "Stun Resistance",
        "MATTR_RESIST_DISEASE" => "Disease Resistance",
        "MATTR_RESIST_SLEEP" => "Sleep Resistance",
        "MATTR_RESIST_FEAR" => "Fear Resistance",
        "MATTR_RESIST_CSMP" => "Bleeding Resistance",
        "MATTR_RESIST_ALL_SET" => "All Resistance",
        _ => "",
    };
    if !known.is_empty() {
        return known.to_string();
    }
    // Fallback: "MATTR_SOME_STAT" -> "Some Stat".
    codename
        .strip_prefix("MATTR_")
        .unwrap_or(codename)
        .split('_')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(first) => first
                    .to_uppercase()
                    .chain(c.flat_map(char::to_lowercase))
                    .collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<String>>()
        .join(" ")
}

#[cfg(test)]
mod test {
    use super::*;

    fn opt(codename: &str, op: &str) -> MagicOptionInfo {
        MagicOptionInfo {
            codename: codename.to_string(),
            op: op.to_string(),
        }
    }

    #[test]
    fn flat_option_formats_with_signed_value() {
        assert_eq!(opt("MATTR_STR", "+").format(3), "STR +3");
        assert_eq!(opt("MATTR_HP", "+").format(120), "HP +120");
    }

    /// The two primary stats are abbreviated the way the rest of the client
    /// names them (`PARAM_STR`/`PARAM_INT`), and every set/avatar/3-job variant
    /// folds onto the same abbreviation — one stat, one name.
    #[test]
    fn the_primary_stats_are_abbreviated_everywhere_they_appear() {
        for code in [
            "MATTR_STR",
            "MATTR_STR_SET",
            "MATTR_STR_3JOB",
            "MATTR_STR_AVATAR",
            "MATTR_AVATAR_STR",
            "MATTR_AVATAR_STR_4",
        ] {
            assert_eq!(mattr_display(code), "STR", "{code}");
        }
        for code in [
            "MATTR_INT",
            "MATTR_INT_SET",
            "MATTR_INT_3JOB",
            "MATTR_INT_AVATAR",
            "MATTR_AVATAR_INT",
            "MATTR_AVATAR_INT_4",
        ] {
            assert_eq!(mattr_display(code), "INT", "{code}");
        }
    }

    #[test]
    fn percent_options_read_as_rate() {
        assert_eq!(opt("MATTR_DUR", "+").format(60), "Durability 60% increased");
        assert_eq!(opt("MATTR_ER", "+").format(5), "Parry Rate 5% increased");
        assert_eq!(
            opt("MATTR_DEC_MAXDUR", "-@").format(50),
            "Max Durability 50% decreased"
        );
    }

    #[test]
    fn flag_options_omit_value() {
        assert_eq!(opt("MATTR_ATHANASIA", "+").format(5), "Immortal");
        assert_eq!(opt("MATTR_LUCK", "+").format(1), "Lucky");
    }

    /// The drawback is exactly one code, and none of the bonuses it ships
    /// alongside may be swept into it — a "Durability 400% increased" line
    /// coloured as a penalty would invert the item's own selling point.
    #[test]
    fn only_the_unrepairable_flag_reads_as_a_drawback() {
        assert!(opt("MATTR_NOT_REPARABLE", "+").is_penalty());
        for code in [
            "MATTR_DUR",
            "MATTR_DEC_MAXDUR",
            "MATTR_ATHANASIA",
            "MATTR_STR",
        ] {
            assert!(!opt(code, "+").is_penalty(), "{code}");
        }
    }

    #[test]
    fn unknown_code_falls_back_to_title_case() {
        assert_eq!(mattr_display("MATTR_APE"), "Ape");
        assert_eq!(mattr_display("MATTR_NASRUN_HPNA"), "Nasrun Hpna");
    }
}
