//! Which particle effect an item plays when it is used.
//!
//! Idea: the original plays a visible effect on a consumed item — the swirl on
//! a potion, the flash on a return scroll. **No table in the archive maps a
//! consumable to an effect.** There *is* an `itemeffect.txt`
//! (`server_dep/silkroad/textdata/`), but all 286 of its rows are
//! `ITEM_MALL_AVATAR` / `ITEM_EVENT_AVATAR` / `ITEM_PRE_MALL` mapped to
//! `ITEMSKILL_*` codenames — no consumable appears in it. Nor does `itemdata`
//! name one: not one of its 15,533 rows contains an `.efp` path in any column.
//! So for consumables the selection really is code-side in the original.
//!
//! But the *effects themselves are named*, which is enough. `Particles.pk2`
//! holds `system/item_hpotion.efp`, `item_mpotion.efp`, `item_hgppotion.efp`,
//! `item_life.efp`, `item_returnscroll_use.efp` and the `item_qt_*` scroll set,
//! plus `battle/status_cure_*.efp` — the filenames say what they are. Pairing
//! those with the type ids observed on the wire in `packet_dump/c2s/0x704c.log`
//! gives a **derived** default table rather than an invented one, and every row
//! below cites both halves of its evidence.
//!
//! The four potion-family rows corroborate each other: `item_hpotion`,
//! `item_mpotion`, `item_hgppotion` and `item_life` share both meshes
//! (`cho-won-001/002.bms`) and differ only in tint — `-r`, `-b`, `-y`, `-v`.
//! One family, four colours, exactly as mapped.
//!
//! Rows we do *not* ship are the ones where only half the evidence exists: the
//! `item_qt_*` scrolls have obvious filenames but no captured type id, so which
//! `(3,3,x,y)` each belongs to would be a guess. They stay out until one use is
//! captured. A wrong `.efp` path fails **silently** (the runtime despawns a
//! wrapper whose asset never loads), which is precisely why guessing here is
//! worse than an absent row.
//!
//! Keys are **type-id prefixes** — `"3.3.1"` is every potion, `"3.3.1.1"` only
//! the HP line — and the longest matching prefix wins, so a family default and
//! a per-item override can coexist. Setting `item_use` in `config.yaml`
//! replaces this table wholesale. To extend it, run at `RUST_LOG=client=debug`
//! and use the item: the type id is logged for exactly this purpose. To see
//! what your own archive offers:
//!
//! ```text
//! make pk2 list PK2=assets/Particles.pk2 | grep -i '^system/item_'
//! ```

use std::collections::HashMap;

use serde::Deserialize;

/// One effect, or several played together.
///
/// An untagged enum rather than a plain `Vec` so both YAML spellings parse:
///
/// ```yaml
/// "3.3.1.1": "particles://system/item_hpotion.efp"
/// "3.3.2":
///   - "particles://battle/status_cure_poison.efp"
///   - "particles://skill/china/water_cure_effect_a.efp"
/// ```
///
/// The scalar form is kept because overriding `item_use` replaces the table
/// wholesale, so anyone who already wrote one out by hand would otherwise find
/// their config rejected by an upgrade that has nothing to do with them.
#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(untagged)]
pub enum EffectPaths {
    One(String),
    Many(Vec<String>),
}

impl EffectPaths {
    pub fn as_slice(&self) -> &[String] {
        match self {
            EffectPaths::One(path) => std::slice::from_ref(path),
            EffectPaths::Many(paths) => paths,
        }
    }
}

/// Effect-playback knobs.
#[derive(Deserialize, Debug, Clone)]
#[serde(default)]
pub struct EffectSettings {
    /// Type-id prefix → the `particles://…efp` path (or paths) played on the
    /// user when the server confirms an item use (0xB04C). Defaults to
    /// [`default_item_use_effects`]; setting it in `config.yaml` replaces the
    /// whole table.
    pub item_use: HashMap<String, EffectPaths>,
    /// How long a played item-use effect is kept alive, in seconds.
    ///
    /// Ours: `.efp` programs carry their own loop length but no "play once and
    /// stop" flag the runtime reads, and effect wrappers are not auto-reaped,
    /// so something has to bound them. 2s covers the one-shot bursts these are.
    pub item_use_seconds: f32,
}

impl Default for EffectSettings {
    fn default() -> Self {
        Self {
            item_use: default_item_use_effects(),
            item_use_seconds: 2.0,
        }
    }
}

/// The shipped item-use effects.
///
/// Each row is `(type-id prefix, effect, evidence)`. **Both halves are
/// required** before a row ships: the effect name comes from `Particles.pk2`'s
/// own `system/` folder, and the type id from a decoded body in
/// `packet_dump/c2s/0x704c.log`. Neither alone is enough — the filenames say
/// what an effect *is* but not which item plays it, and the capture says which
/// items exist but not what they look like.
pub fn default_item_use_effects() -> HashMap<String, EffectPaths> {
    // (type ids decoded through `hud::underbar::cast::pack_type_id`)
    const ROWS: [(&str, &[&str]); 6] = [
        // `36ec10`/`38ec08` etc. in the c2s capture; the 0xB04C successes echo
        // the same words back with a decrementing stack count.
        ("3.3.1.1", &["particles://system/item_hpotion.efp"]),
        ("3.3.1.2", &["particles://system/item_mpotion.efp"]),
        // The third potion line. `item_hgppotion` is the only remaining
        // `item_*potion` in the archive, and HGP is the third vitals bar.
        ("3.3.1.3", &["particles://system/item_hgppotion.efp"]),
        // The revival item (Grass of Life): the capture uses `(3,3,1,6)` as a
        // `WithSlot` body targeting a dead pet's scroll — the only item class
        // that does — and `item_life.efp` is the archive's revival effect.
        ("3.3.1.6", &["particles://system/item_life.efp"]),
        // `_use` rather than the bare `item_returnscroll.efp`: the archive
        // ships both, and the `_use` cut is the one named for the action
        // (`_green`/`_red` are its per-destination variants).
        ("3.3.3.1", &["particles://system/item_returnscroll_use.efp"]),
        // The whole cure branch, as a FAMILY key rather than per leaf, because
        // the branch is one kind of thing: `(3,3,2,1)` ITEM_ETC_CURE_RANDOM_*,
        // `(3,3,2,6)` ITEM_ETC_CURE_ALL_* (the universal pill — the type id the
        // capture actually carries, `126c31`), `(3,3,2,7)` ITEM_COS_P_CURE_ALL_*
        // and `(3,3,2,8)` ITEM_MALL_CURE_SUPERSET_*, all named from itemdata.
        //
        // TWO effects, because one alone is not what a cure looks like:
        //
        // 1. `status_cure_poison.efp` — the burst. The archive keys cure art to
        //    the AILMENT, not the item: it ships
        //    `battle/status_cure_{blind,burn,eshock,frostbite,poison,zombie}.efp`
        //    beside 24 `status_bad_*`, and no "cure all" at all, so a pill that
        //    clears everything has no burst of its own. Poison is **our pick**
        //    among the six — the one line in this table whose art is a choice
        //    rather than a transcription.
        // 2. `water_cure_effect_a.efp` — the ring around the character, and this
        //    one *is* sourced: `skilleffect.txt` binds it to
        //    `SKILL_CH_WATER_CURE_A` ("Force Cure - Poison", the CH Force
        //    mastery) as its `ACT_S` release on `Bip01` with zero offset. It is
        //    `meshes\cho-won-002.bms` (won = 원 = circle, the expanding body
        //    ring) plus `meshes\levelup03.bms` (the rising ring column).
        //
        // Playing the effect for the ailment actually removed is the faithful
        // behaviour and wants a bad-status model first (we have
        // `Stunned`/`Frozen` against the archive's 24), so that stays a later
        // job.
        (
            "3.3.2",
            &[
                "particles://battle/status_cure_poison.efp",
                "particles://skill/china/water_cure_effect_a.efp",
            ],
        ),
    ];
    ROWS.iter()
        .map(|(key, paths)| {
            (
                (*key).to_string(),
                match paths {
                    [one] => EffectPaths::One((*one).to_string()),
                    many => EffectPaths::Many(many.iter().map(|p| (*p).to_string()).collect()),
                },
            )
        })
        .collect()
}

/// The dotted type-id key for an item, longest-first, e.g. `3.3.1.1` then
/// `3.3.1` then `3.3` then `3`.
///
/// Split out so the lookup order is stated once and testable: a caller that
/// searched shortest-first would let a family default shadow the per-item
/// override that exists precisely to escape it.
pub fn type_id_keys(type_ids: (u32, u32, u32, u32)) -> [String; 4] {
    let (a, b, c, d) = type_ids;
    [
        format!("{a}.{b}.{c}.{d}"),
        format!("{a}.{b}.{c}"),
        format!("{a}.{b}"),
        format!("{a}"),
    ]
}

impl EffectSettings {
    /// The effects for an item's type ids, in the order they should play, or
    /// `None` when the table says nothing about it.
    ///
    /// Still longest-prefix-wins and still first-match-only: a per-item override
    /// replaces its family's list rather than appending to it, which is the
    /// point of having one.
    pub fn item_use_effects(&self, type_ids: (u32, u32, u32, u32)) -> Option<&[String]> {
        type_id_keys(type_ids)
            .iter()
            .find_map(|key| self.item_use.get(key))
            .map(EffectPaths::as_slice)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn settings(rows: &[(&str, &str)]) -> EffectSettings {
        EffectSettings {
            item_use: rows
                .iter()
                .map(|(k, v)| (k.to_string(), EffectPaths::One(v.to_string())))
                .collect(),
            item_use_seconds: 2.0,
        }
    }

    /// The specific entry wins over the family it belongs to. Searching the
    /// other way round would make a per-item override unreachable whenever a
    /// family default existed — which is the only reason to write one.
    #[test]
    fn the_longest_matching_prefix_wins() {
        let s = settings(&[
            ("3.3.1", "particles://system/family.efp"),
            ("3.3.1.1", "particles://system/hp.efp"),
        ]);
        assert_eq!(
            s.item_use_effects((3, 3, 1, 1)),
            Some(["particles://system/hp.efp".to_string()].as_slice())
        );
        // a sibling of the overridden item still takes the family default
        assert_eq!(
            s.item_use_effects((3, 3, 1, 2)),
            Some(["particles://system/family.efp".to_string()].as_slice())
        );
    }

    /// An unlisted item plays nothing — the table is the whole authority.
    #[test]
    fn an_unlisted_item_has_no_effect() {
        assert_eq!(settings(&[]).item_use_effects((3, 3, 1, 1)), None);
        let s = settings(&[("3.3.1", "particles://system/potion.efp")]);
        assert_eq!(s.item_use_effects((3, 2, 1, 1)), None);
    }

    /// Both YAML spellings parse, and a scalar behaves exactly like a one-item
    /// list. The scalar form is the one anyone who already overrode `item_use`
    /// will have written, and overriding replaces the whole table — so dropping
    /// it would reject a config that has nothing to do with this change.
    #[test]
    fn a_row_may_be_one_path_or_several() {
        let parsed: HashMap<String, EffectPaths> = serde_yaml::from_str(
            r#"
"3.3.1.1": "particles://system/hp.efp"
"3.3.2":
  - "particles://battle/burst.efp"
  - "particles://skill/china/ring.efp"
"#,
        )
        .expect("both spellings must parse");
        assert_eq!(
            parsed["3.3.1.1"].as_slice(),
            ["particles://system/hp.efp".to_string()]
        );
        assert_eq!(
            parsed["3.3.2"].as_slice(),
            [
                "particles://battle/burst.efp".to_string(),
                "particles://skill/china/ring.efp".to_string(),
            ]
        );
    }

    /// The shipped table covers the items the capture actually shows being
    /// used. This is the regression that matters: the table was empty on
    /// release and every item use silently played nothing.
    #[test]
    fn the_shipped_table_covers_the_captured_items() {
        let s = EffectSettings::default();
        for (ids, expected) in [
            ((3, 3, 1, 1), "item_hpotion"),
            ((3, 3, 1, 2), "item_mpotion"),
            ((3, 3, 1, 3), "item_hgppotion"),
            ((3, 3, 1, 6), "item_life"),
            ((3, 3, 3, 1), "item_returnscroll_use"),
        ] {
            let got = s.item_use_effects(ids).unwrap_or_default();
            assert!(
                got.iter().any(|path| path.contains(expected)),
                "{ids:?} -> {got:?}, wanted {expected}"
            );
        }
    }

    /// Every shipped path must point into the particle archive, because that is
    /// the only source the effect runtime can load — and a path it cannot load
    /// fails *silently*, which is indistinguishable from the feature being off.
    #[test]
    fn every_shipped_effect_is_a_particles_path() {
        for (key, paths) in default_item_use_effects() {
            assert!(!paths.as_slice().is_empty(), "{key} -> no paths");
            for path in paths.as_slice() {
                assert!(
                    path.starts_with("particles://") && path.ends_with(".efp"),
                    "{key} -> {path}",
                );
            }
        }
    }

    /// Rows only ship with both halves of their evidence, so the table stays
    /// small on purpose. If this count grows, the new rows need a captured type
    /// id — not just a plausible filename.
    #[test]
    fn the_shipped_table_is_only_what_is_evidenced() {
        let table = default_item_use_effects();
        assert_eq!(table.len(), 6);
        // the `item_qt_*` scroll family is deliberately absent: real effects,
        // but no captured type id says which item plays which
        assert!(!table
            .values()
            .flat_map(EffectPaths::as_slice)
            .any(|p| p.contains("item_qt_")));
    }

    /// The universal pill is `(3,3,2,6)` — `ITEM_ETC_CURE_ALL_*` in itemdata,
    /// and `126c31` on the wire. It is pinned because the notes previously had
    /// it as `(3,3,2,1)` (which is `ITEM_ETC_CURE_RANDOM_*`), and that one digit
    /// was the whole reason pills played nothing.
    ///
    /// It plays **two** effects: the ailment burst and the Force Cure ring. The
    /// ring is the half a playtest found missing, so its presence is pinned by
    /// name rather than by count.
    #[test]
    fn the_cure_family_row_covers_the_universal_pill() {
        let settings = EffectSettings::default();
        let pill = settings
            .item_use_effects((3, 3, 2, 6))
            .expect("the universal pill must resolve");
        assert!(
            pill.iter().any(|p| p.contains("status_cure")),
            "the ailment burst is missing: {pill:?}"
        );
        assert!(
            pill.iter().any(|p| p.contains("water_cure_effect_a")),
            "the Force Cure ring is missing: {pill:?}"
        );
        // ...and its siblings on the same branch resolve through the same key.
        for tid4 in [1, 7, 8] {
            assert_eq!(settings.item_use_effects((3, 3, 2, tid4)), Some(pill));
        }
        // The family key must not swallow the potions, which are `(3,3,1,*)`.
        assert!(settings
            .item_use_effects((3, 3, 1, 1))
            .is_some_and(|paths| paths.iter().any(|p| p.contains("item_hpotion"))));
    }

    /// Keys are searched most-specific first; pinned as a list because the
    /// lookup above depends on the order, not just the contents.
    #[test]
    fn the_keys_are_generated_longest_first() {
        assert_eq!(
            type_id_keys((3, 3, 1, 2)),
            ["3.3.1.2", "3.3.1", "3.3", "3"].map(String::from)
        );
    }
}
