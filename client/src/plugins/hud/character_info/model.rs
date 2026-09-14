//! Character info window state + the local player's stat sheet.
//!
//! Idea: 0x303D (`CharacterStatsUpdate`) already carries the entire vanilla
//! C-window right column — attack/defense ranges, hit/parry, STR/INT — but
//! only max HP/MP were consumed (mini-info). [`PlayerStats`] mirrors the full
//! packet plus the stat-point wallet (seeded from the 0x3013 character data,
//! updated by 0x304E), so the window rides on plain resource change
//! detection. Spending a point sends the EXPERIMENTAL 0x7050/0x7051 requests
//! and applies nothing until the server acks (0xB050/0xB051) — the new
//! STR/INT and max HP/MP then arrive in the follow-up 0x303D refresh.

use bevy::prelude::*;

use packets::agent::prelude::{
    CharacterPointsUpdate, CharacterStatsUpdate, IncreaseIntResponse, IncreaseStrResponse,
    ReceiveExperience,
};

use crate::plugins::hud::chat::model::ChatState;
use crate::plugins::net::character_info::CharacterInfo;
use crate::plugins::settings::keymap::KEY_CHARACTER;
use crate::plugins::settings::options::GameOptions;

/// Open/closed state of the character info window (C key).
#[derive(Resource, Default)]
pub struct CharacterInfoState {
    pub open: bool,
}

/// The local player's full stat sheet (0x303D mirror + points/level/exp from
/// 0x3013/0x304E/0x3056). A resource for the same reason as `PlayerVitals`:
/// the packets carry no unique id — they are local-player-scoped.
#[derive(Resource, Clone, Debug, Default)]
pub struct PlayerStats {
    /// `None` until the first 0x303D arrives.
    pub sheet: Option<CharacterStatsUpdate>,
    pub stat_points: u16,
}

/// Toggle with the `KeyCharacter` shortcut (unless the chat input is capturing
/// keys). Rebindable via the options window's Key Map tab; C by default.
pub fn toggle_character_info(
    keys: Res<ButtonInput<KeyCode>>,
    chat: Res<ChatState>,
    options: Res<GameOptions>,
    mut state: ResMut<CharacterInfoState>,
) {
    let Some(key) = options.key_for(KEY_CHARACTER) else {
        return;
    };
    if keys.just_pressed(key) && !chat.input_open {
        state.open = !state.open;
    }
}

/// Apply 0x303D: the full recomputed stat sheet.
pub fn on_stats_update(
    mut reader: MessageReader<CharacterStatsUpdate>,
    mut stats: ResMut<PlayerStats>,
) {
    for msg in reader.read() {
        stats.sheet = Some(msg.clone());
    }
}

/// Apply 0x304E `StatPoints`: the authoritative stat-point wallet.
pub fn on_points_update(
    mut reader: MessageReader<CharacterPointsUpdate>,
    mut stats: ResMut<PlayerStats>,
) {
    for msg in reader.read() {
        if let CharacterPointsUpdate::StatPoints { amount } = msg {
            if stats.stat_points != *amount {
                stats.stat_points = *amount;
            }
        }
    }
}

/// Apply the 0x3056 level-up stat-point award (its trailing u16, present only
/// on a level-up). This server sends no 0x304E `StatPoints`, so without this the
/// wallet stays frozen at the login seed and points earned by leveling can never
/// be spent (the +STR/+INT buttons stay disabled). Same authoritative-overwrite
/// semantics as [`on_points_update`]. See [`ReceiveExperience::stat_points`].
pub fn on_experience_stat_points(
    mut reader: MessageReader<ReceiveExperience>,
    mut stats: ResMut<PlayerStats>,
) {
    for msg in reader.read() {
        if let Some(points) = msg.stat_points() {
            if stats.stat_points != points {
                stats.stat_points = points;
            }
        }
    }
}

/// Seed the stat-point wallet from the 0x3013 character data (the join-stream
/// snapshot; 0x304E keeps it current afterwards).
pub fn seed_stat_points(
    fresh: Query<&CharacterInfo, Added<CharacterInfo>>,
    mut stats: ResMut<PlayerStats>,
) {
    for info in fresh.iter() {
        if let Some(seed) = info.stats.as_ref() {
            stats.stat_points = seed.stat_points;
        }
    }
}

/// Apply the 0x7050/0x7051 acks: a success deducts one point locally (0x304E
/// usually restates the balance right after, but not every server sends it);
/// the stat change itself lands via the follow-up 0x303D. Failures and
/// unrecognized shapes are logged for the capture-verification loop.
pub fn on_stat_spend_ack(
    mut str_acks: MessageReader<IncreaseStrResponse>,
    mut int_acks: MessageReader<IncreaseIntResponse>,
    mut stats: ResMut<PlayerStats>,
) {
    for ack in str_acks.read() {
        match ack {
            IncreaseStrResponse::Success => {
                stats.stat_points = stats.stat_points.saturating_sub(1);
            }
            IncreaseStrResponse::Failure(code) => {
                warn!("stats: STR increase rejected (code {code:#06x})");
            }
            IncreaseStrResponse::Unknown { result, tail } => {
                warn!(
                    "stats: unknown 0xB050 shape result={result:#04x} tail={} — capture for decode",
                    packets::hexdump(tail, 16)
                );
            }
        }
    }
    for ack in int_acks.read() {
        match ack {
            IncreaseIntResponse::Success => {
                stats.stat_points = stats.stat_points.saturating_sub(1);
            }
            IncreaseIntResponse::Failure(code) => {
                warn!("stats: INT increase rejected (code {code:#06x})");
            }
            IncreaseIntResponse::Unknown { result, tail } => {
                warn!(
                    "stats: unknown 0xB051 shape result={result:#04x} tail={} — capture for decode",
                    packets::hexdump(tail, 16)
                );
            }
        }
    }
}

/// The single-stat ceiling at a level: the most STR *or* INT a character could
/// reach by spending every point into it. `28 + 4·level`.
///
/// `[S]`, not `[V]`: it rests on evolex + go-sro (`formulas.go:17-19` embeds it
/// verbatim, twice) and the recovered Ghidra extract contains no `28`/`4`
/// constant — see `docs/re/gamedata/stat-progression-formulas.md` §3B. It is
/// the reference denominator the balance percentage divides by, and nothing
/// else in the client uses it.
pub fn max_stat(level: u32) -> u32 {
    28 + 4 * level
}

/// One side of the phys/mag balance display, as a percentage — how close this
/// stat is to the ceiling its level allows:
///
/// ```text
/// balance(level, stat) = int(100 − 100·(2/3)·(MaxStat − stat) / MaxStat)
/// ```
///
/// Idea: balance is **not on the wire** — all 492 captured 0x303D bodies are
/// byte-exact 36 bytes and carry no balance field — so the client derives it.
/// `docs/re/gamedata/stat-derivation-model.md:182-184` lists that derivation as
/// legitimate client display-math, unlike the combat formulas, which are
/// server-authoritative and never recomputed here.
///
/// This is go-sro's `PhyBalance` (`formulas.go:17-19`) verbatim, including its
/// `int()` truncation of the *final* value — truncating the quotient instead
/// disagrees by one at e.g. `(level 1, stat 21)`. It replaces our earlier
/// `round(200·own/(own+other))` sum-to-200 model, which was one of three
/// mutually inconsistent candidates recorded as `drift` in
/// `docs/re/gamedata/client-display-units.md` §3C.
///
/// **Both sides use this same shape** with the stats swapped. go-sro's own
/// `MagBalance` (`:21-23`) is *not* a mirror — `100·stat/28 + level·4` is a
/// precedence bug that makes it unbounded — so it is deliberately not copied.
///
/// **Not clamped, and the 120% cap stays UNKNOWN.** With no gear the value
/// lands in 33…100 by construction. 0x303D's STR/INT include equipment
/// bonuses, so gear can push a stat past `MaxStat` and the reading past 100 —
/// which is the likeliest origin of evolex's "caps at 120%" folklore. There is
/// no `120.0` constant anywhere in the recovered findings
/// (`client-display-units.md` §3C), so an inflated stat is reported as it
/// computes rather than clamped to a number nobody can cite.
pub fn balance_percent(level: u32, stat: u16) -> u32 {
    let max = max_stat(level) as f64;
    let value = 100.0 - 100.0 * (2.0 / 3.0) * (max - stat as f64) / max;
    value.max(0.0).trunc() as u32
}

/// The balance cell's text. **The single source for both surfaces that show
/// it** — the character window's `PhyBal`/`MagBal` rows and the mini-info stat
/// drawer's `GDR_PMI_TXT_PHYBAL`/`_MAGBAL` cells (#765). They used to disagree:
/// the drawer printed `-` while this window printed a percentage, i.e. the
/// client gave two answers for one stat, both visible at once.
pub fn balance_text(level: u32, stat: u16) -> String {
    format!("{}%", balance_percent(level, stat))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `MaxStat = 28 + 4·level` (`stat-progression-formulas.md` §3B), the
    /// denominator the whole percentage hangs on.
    #[test]
    fn the_stat_ceiling_grows_four_per_level_from_twenty_eight() {
        assert_eq!(max_stat(0), 28);
        assert_eq!(max_stat(1), 32);
        assert_eq!(max_stat(10), 68);
        assert_eq!(max_stat(20), 108);
        assert_eq!(max_stat(70), 308);
    }

    /// Without gear the reading is bounded by 33…100: a stat of 0 is a third of
    /// the bar, a stat at the level's ceiling is the whole of it. This is the
    /// shape that replaced the sum-to-200 model, and the range is what makes it
    /// a *closeness to the ceiling* rather than a STR↔INT trade-off.
    #[test]
    fn an_un_geared_reading_spans_a_third_of_the_bar_to_all_of_it() {
        for level in [1u32, 10, 40, 90] {
            let ceiling = max_stat(level) as u16;
            assert_eq!(balance_percent(level, ceiling), 100, "level {level}");
            assert_eq!(balance_percent(level, 0), 33, "level {level}");
            for stat in [1u16, ceiling / 3, ceiling / 2, ceiling - 1] {
                let value = balance_percent(level, stat);
                assert!((33..=100).contains(&value), "level {level} stat {stat}");
            }
        }
    }

    /// go-sro's `PhyBalance` truncates the **final** value, not the quotient.
    /// `(level 1, stat 21)` is the case that separates the two — 77.08 truncates
    /// to 77, while truncating the quotient first yields 78.
    #[test]
    fn the_truncation_matches_the_reference_implementation() {
        assert_eq!(balance_percent(1, 20), 75);
        assert_eq!(balance_percent(1, 21), 77);
    }

    /// 0x303D's STR/INT carry equipment bonuses, so a stat can exceed its
    /// level's ceiling and the reading can exceed 100. Reported as it computes:
    /// evolex's 120% cap has no binary anchor (`client-display-units.md` §3C),
    /// and this test exists to make a future clamp a deliberate, cited change
    /// rather than a quiet one.
    #[test]
    fn gear_past_the_ceiling_is_reported_uncapped_because_the_cap_is_unknown() {
        // level 1 ceiling is 32; a geared 60 reads well past 100
        let value = balance_percent(1, 60);
        assert!(value > 100, "expected a reading past 100, got {value}");
    }

    /// Before the first 0x303D there is no level and no sheet; the helper must
    /// still return a number rather than divide by zero.
    #[test]
    fn a_level_less_character_still_produces_a_reading() {
        assert_eq!(balance_percent(0, 0), 33);
    }

    /// Both surfaces render the balance cell through this one function, so the
    /// character window and the mini-info drawer cannot print different text
    /// for the same sheet again (#765).
    #[test]
    fn the_balance_cell_text_has_one_source() {
        assert_eq!(balance_text(1, 20), "75%");
        assert_eq!(
            balance_text(20, 60),
            format!("{}%", balance_percent(20, 60)),
            "the text helper must not re-derive the number"
        );
    }

    /// Source-level guard: neither consumer may keep a private copy of the
    /// formula. The divergence this issue fixed existed precisely because the
    /// number was written down twice — once as a computation and once as `-`.
    #[test]
    fn no_consumer_re_derives_the_balance_formula() {
        for (name, src) in [
            (
                "character_info/ui.rs",
                include_str!("../character_info/ui.rs"),
            ),
            (
                "player_mini_info.rs",
                include_str!("../player_mini_info.rs"),
            ),
        ] {
            assert!(
                !src.contains("fn balance_percent"),
                "{name} defines its own balance formula again"
            );
            assert!(
                src.contains("balance_text"),
                "{name} no longer routes its balance cell through the shared helper"
            );
        }
    }
}
