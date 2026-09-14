use std::collections::HashMap;

/// Parsed `server_dep/silkroad/textdata/leveldata.txt`, keyed by the row's
/// level column. Column 1 is the **within-level requirement**: the exp needed
/// to advance from that level to the next (row "1 118" = 118 to go 1→2, row
/// "6 5640" = 5640 to go 6→7), which is exactly the value the character's
/// within-level exp fills. (Verified against Media.pk2: differencing
/// consecutive rows instead made the bar reach 100% before the real threshold
/// at higher levels, e.g. level 3 needs 1058 but the difference gave 822.)
/// Column 2 is the SP cost of raising a *mastery* to that level (1 SP at low
/// levels, growing: 1,1,1,2,2,4,5,6,...).
#[derive(Debug, Clone, Default)]
pub struct LevelData {
    /// level → exp to advance from it.
    pub exp: HashMap<u8, u64>,
    /// target mastery level → SP cost of the raise.
    pub mastery_sp: HashMap<u8, u32>,
}

impl LevelData {
    /// Exp required to advance from `level` to `level + 1`.
    pub fn max_exp(&self, level: u8) -> Option<u64> {
        self.exp.get(&level).copied()
    }

    /// SP cost of raising a mastery to `target_level`.
    pub fn mastery_sp_cost(&self, target_level: u8) -> Option<u32> {
        self.mastery_sp.get(&target_level).copied()
    }
}

/// Max HP (from STR) or max MP (from INT) for a character of `level`.
///
/// Idea: the server does not send a formula, it sends the result — but the
/// same result is reproducible offline, which is what the character-select
/// screen needs before any 0x303D arrives (#203). The vSRO server computes a
/// per-level growth factor `1.02^(level-1)` into attributes 0x36/0x37
/// (`FUN_004e3200`, with the f64 `1.02` pinned at `.rdata` 0x00B45EA0) and
/// scales the stat by it; the public formula `1.02^(level-1) · stat · 10` is
/// the same thing, and go-sro codes it verbatim (`utils/formulas.go:25-27`).
///
/// **Verified against our own captures** (`docs/stat-derivation-server-spec.md`
/// §5): every one of the 20 `packet_dump/0x303d.log` bodies matches exactly,
/// cross-checked against the level in `packet_dump/0x3013.log` — level 17 with
/// STR 36 / INT 84 gives max HP 494 / max MP 1153, level 1 with 20/20 gives
/// 200/200. Truncation, not rounding: level 2 with 21 gives 214 (214.2).
///
/// Server-authoritative: this is display math for screens that have no 0x303D
/// yet, never a correction of what the server sent. Its one consumer is the
/// character-select info box (`scenes/intro_v2/character_select.rs`, #203).
pub fn max_hp_or_mp(level: u8, stat: u16) -> u32 {
    let growth = 1.02f64.powi(level.max(1) as i32 - 1);
    (growth * stat as f64 * 10.0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real captured pairs: the level comes from `packet_dump/0x3013.log`
    /// (CHARACTER_DATA `level` byte), the stats and maxima from the
    /// `packet_dump/0x303d.log` body captured in the same session.
    #[test]
    fn max_hp_or_mp_matches_the_captured_0x303d_bodies() {
        // 2026-08-10/11 session: 0x3013 ref 1907, level 0x11 = 17
        assert_eq!(max_hp_or_mp(17, 36), 494);
        assert_eq!(max_hp_or_mp(17, 84), 1153);
        // 2026-08-11T11:24 session: 0x3013 ref 1908, level 1, 20/20
        assert_eq!(max_hp_or_mp(1, 20), 200);
        // same session after one stat point: 21 at level 1
        assert_eq!(max_hp_or_mp(1, 21), 210);
        // level 2 with 21: 1.02 · 210 = 214.2, captured as 214 — truncated
        assert_eq!(max_hp_or_mp(2, 21), 214);
    }

    #[test]
    fn max_hp_or_mp_treats_level_zero_as_level_one() {
        // char-select rows can arrive before a level is known; no panic and
        // no growth rather than a negative exponent
        assert_eq!(max_hp_or_mp(0, 20), 200);
    }
}
