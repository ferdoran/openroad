//! skilldata.txt — the skill reference table (id → codename, icon, display
//! name, activity class, leveling requirements). Same shard scheme as
//! itemdata: a master `skilldata.txt` lists the `skilldata_5000.txt`-style
//! shards, rows are tab-separated with the id in column 1. Column semantics
//! verified against the v1.188 corpus: activity (col 8) is 0 = passive,
//! 1 = instant/auto self-skill (imbues, GYEONGGONG, potion skills),
//! 2 = normal castable skill (all attacks, buffs, chain steps). Skill levels
//! of one skill share a `basic_group` codename (col 5, the join key into
//! skilleffect.txt) and a `group_id` (col 2, what prereq columns reference);
//! `basic_level` (col 7) orders the ladder. Trailing columns from 69 on are
//! a fourcc-tagged parameter stream (`'att'`, `'dura' <ms>`, ...).

use bevy::asset::Asset;
use bevy::prelude::TypePath;
use std::collections::HashMap;
use std::ops::Deref;

#[derive(Debug, Clone)]
pub struct SkillDataRow(pub(crate) Vec<String>);

#[derive(Asset, TypePath, Debug, Clone)]
pub struct SkillData(pub HashMap<i32, SkillDataRow>);

#[repr(usize)]
enum SkilldataFields {
    GroupId = 2,
    CodeName = 3,
    BasicGroup = 5,
    BasicLevel = 7,
    Activity = 8,
    ChainCode = 9,
    PreparingTime = 11,
    CastingTime = 12,
    ActionDuration = 13,
    ReuseDelay = 14,
    ActionFlyingSpeed = 16,
    ActionAutoAttackType = 19,
    ActionRange = 21,
    TargetRequired = 22,
    TargetGroupSelf = 26,
    TargetGroupAlly = 27,
    TargetGroupParty = 28,
    TargetGroupEnemyMonster = 29,
    TargetGroupEnemyPlayer = 30,
    TargetSelectDeadBody = 33,
    ReqMastery1 = 34,
    ReqMasteryLevel1 = 36,
    ReqSkillGroup1 = 40,
    ReqSkillLevel1 = 43,
    ReqLearnSp = 46,
    ReqCastWeapon1 = 50,
    ReqCastWeapon2 = 51,
    ConsumeHp = 52,
    ConsumeMp = 53,
    UiTab = 57,
    UiPage = 58,
    UiColumn = 59,
    UiRow = 60,
    IconPath = 61,
    NameStrId = 62,
    TooltipDescId = 64,
    StudyDescId = 65,
    ParamsStart = 69,
}

impl Deref for SkillData {
    type Target = HashMap<i32, SkillDataRow>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl SkillDataRow {
    /// Per-level codename (col 3), e.g. `SKILL_CH_SWORD_SMASH_A_01`.
    pub fn code_name(&self) -> &String {
        &self.0[SkilldataFields::CodeName as usize]
    }

    /// A column as a trimmed string, `None` when the row is too short or the
    /// value is empty / the `xxx` placeholder.
    fn field(&self, index: usize) -> Option<&str> {
        let value = self.0.get(index)?.trim();
        (!value.is_empty() && value != "xxx" && value != "0").then_some(value)
    }

    /// UI icon of the skill as an asset path (`media://icon/...`), or `None`
    /// when the column is empty or a placeholder.
    pub fn icon_path(&self) -> Option<String> {
        let path = self.field(SkilldataFields::IconPath as usize)?;
        Some(format!(
            "media://icon/{}",
            path.replace('\\', "/").to_lowercase()
        ))
    }

    /// Key of the skill's display name in the textdata name tables (`SN_*`).
    pub fn name_key(&self) -> Option<&str> {
        self.field(SkilldataFields::NameStrId as usize)
    }

    /// A column parsed as a number, 0 when missing or malformed.
    fn number(&self, index: usize) -> i64 {
        self.0
            .get(index)
            .and_then(|v| v.trim().parse::<i64>().ok())
            .unwrap_or(0)
    }

    /// Skill group id (col 2) — shared by all levels of one skill and what
    /// the prereq columns of other skills reference.
    pub fn group_id(&self) -> i32 {
        self.number(SkilldataFields::GroupId as usize) as i32
    }

    /// Level-less group codename (col 5), e.g. `SKILL_CH_SWORD_SMASH_A` —
    /// the join key into skilleffect.txt's aniset table.
    pub fn basic_group(&self) -> Option<&str> {
        self.field(SkilldataFields::BasicGroup as usize)
    }

    /// Skill level within its group ladder (1-based).
    pub fn basic_level(&self) -> u8 {
        self.number(SkilldataFields::BasicLevel as usize) as u8
    }

    /// Activity class: 0 = passive, 1 = instant/auto self-skill,
    /// 2 = normal castable skill.
    pub fn activity(&self) -> u8 {
        self.number(SkilldataFields::Activity as usize) as u8
    }

    /// Whether the skill can be triggered by the player (activity 1 or 2).
    pub fn is_castable(&self) -> bool {
        matches!(self.activity(), 1 | 2)
    }

    /// Whether this row is the entry point of a learnable skill ladder:
    /// level 1, has an icon, and belongs to a mastery.
    pub fn is_learnable_root(&self) -> bool {
        self.basic_level() == 1 && self.icon_path().is_some() && self.mastery_req().is_some()
    }

    /// Chain code (col 9): the skill id of the NEXT segment of a combo,
    /// `None` on standalone skills and on the last segment of a chain.
    /// Consumed by `plugins::skills::cast::advance_chain_casts`.
    pub fn chain_code(&self) -> Option<i32> {
        match self.number(SkilldataFields::ChainCode as usize) {
            0 => None,
            code => Some(code as i32),
        }
    }

    /// Pre-cast wind-up in milliseconds (col 11); together with
    /// [`Self::casting_time_ms`] this is the charge phase (ready animation +
    /// READY emissions) before the shot fires.
    pub fn preparing_time_ms(&self) -> u32 {
        self.number(SkilldataFields::PreparingTime as usize) as u32
    }

    /// Cast wind-up in milliseconds (col 12).
    pub fn casting_time_ms(&self) -> u32 {
        self.number(SkilldataFields::CastingTime as usize) as u32
    }

    /// Action duration in milliseconds (col 13) — roughly the shot animation.
    pub fn action_duration_ms(&self) -> u32 {
        self.number(SkilldataFields::ActionDuration as usize) as u32
    }

    /// Reuse delay (cooldown) in milliseconds (col 14).
    pub fn reuse_delay_ms(&self) -> u32 {
        self.number(SkilldataFields::ReuseDelay as usize) as u32
    }

    /// Projectile flight speed in world units/sec (col 16,
    /// Action_FlyingSpeed); `None` when 0 = instant / no projectile.
    /// Corpus: 400 on bow/crossbow skills, 100-500 elsewhere.
    pub fn flying_speed(&self) -> Option<f32> {
        match self.number(SkilldataFields::ActionFlyingSpeed as usize) {
            0 => None,
            speed => Some(speed as f32),
        }
    }

    /// Auto-attack behaviour after the cast (col 19, Action_AutoAttackType):
    /// 0 = none (buffs), 1 = resume auto-attacking the target (most
    /// attacks), 2 = unverified variant on bow normal/call skills.
    pub fn auto_attack_type(&self) -> u8 {
        self.number(SkilldataFields::ActionAutoAttackType as usize) as u8
    }

    /// Cast range in world units (col 21, Action_Range); `None` when 0,
    /// which on player weapon attacks means "use the equipped weapon's
    /// range".
    pub fn action_range(&self) -> Option<f32> {
        match self.number(SkilldataFields::ActionRange as usize) {
            0 => None,
            range => Some(range as f32),
        }
    }

    /// Whether casting needs a target entity (col 22).
    pub fn target_required(&self) -> bool {
        self.number(SkilldataFields::TargetRequired as usize) != 0
    }

    /// Whether the skill applies to the caster (col 26).
    pub fn targets_self(&self) -> bool {
        self.number(SkilldataFields::TargetGroupSelf as usize) != 0
    }

    /// Whether the skill applies to hostile monsters or players (cols 29/30).
    pub fn targets_enemy(&self) -> bool {
        self.number(SkilldataFields::TargetGroupEnemyMonster as usize) != 0
            || self.number(SkilldataFields::TargetGroupEnemyPlayer as usize) != 0
    }

    /// Whether the skill lands on friendly characters (cols 27/28,
    /// TargetGroup_Ally / TargetGroup_Party) — heals, resurrections and
    /// buffs castable on others.
    pub fn targets_ally(&self) -> bool {
        self.number(SkilldataFields::TargetGroupAlly as usize) != 0
            || self.number(SkilldataFields::TargetGroupParty as usize) != 0
    }

    /// Whether the target must be a dead body (col 33,
    /// TargetEtc_SelectDeadBody) — resurrection skills.
    pub fn targets_dead(&self) -> bool {
        self.number(SkilldataFields::TargetSelectDeadBody as usize) != 0
    }

    /// Required mastery and mastery level to learn (cols 34/36); `None` for
    /// rows outside a mastery (system/NPC skills).
    pub fn mastery_req(&self) -> Option<(u32, u32)> {
        match self.number(SkilldataFields::ReqMastery1 as usize) {
            0 => None,
            mastery => Some((
                mastery as u32,
                self.number(SkilldataFields::ReqMasteryLevel1 as usize) as u32,
            )),
        }
    }

    /// Required previously-learned skill as (group id, level) — the group id
    /// references col 2 of the prerequisite ladder (cols 40/43).
    pub fn skill_prereq(&self) -> Option<(i32, u8)> {
        self.skill_prereqs().next()
    }

    /// All prerequisite skills as (group id, level) — up to three, from the
    /// paired column blocks 40-42 (groups) / 43-45 (levels). The second slot
    /// is used by ~900 rows, so branch chaining and learn checks must
    /// consider more than the first.
    pub fn skill_prereqs(&self) -> impl Iterator<Item = (i32, u8)> + '_ {
        (0..3).filter_map(|slot| {
            match self.number(SkilldataFields::ReqSkillGroup1 as usize + slot) {
                0 => None,
                group => Some((
                    group as i32,
                    self.number(SkilldataFields::ReqSkillLevel1 as usize + slot) as u8,
                )),
            }
        })
    }

    /// SP cost to learn this level (col 46).
    pub fn sp_cost(&self) -> u32 {
        self.number(SkilldataFields::ReqLearnSp as usize) as u32
    }

    /// Weapon classes (item tid4) the skill can be cast with — cols 50/51,
    /// `255` = unrestricted. Corpus-verified: sword skills carry (2,3) =
    /// sword+blade, spear (4,5), bow (6,255); EU 7=1h sword, 8=2h sword,
    /// 9=dual axe, 10=warlock rod, 11=staff, 12=crossbow, 13=dagger,
    /// 14=harp, 15=cleric rod.
    pub fn required_weapons(&self) -> Option<(u8, u8)> {
        let w1 = self.number(SkilldataFields::ReqCastWeapon1 as usize) as u8;
        let w2 = self.number(SkilldataFields::ReqCastWeapon2 as usize) as u8;
        (w1 != 255 && w1 != 0).then_some((w1, w2))
    }

    /// HP consumed per cast (col 52, Consume_HP); nonzero on ~11 rows
    /// corpus-wide.
    pub fn hp_cost(&self) -> u32 {
        self.number(SkilldataFields::ConsumeHp as usize) as u32
    }

    /// MP consumed per cast (col 53, the Consume_MP slot of the 52-56
    /// Consume_* block). 0 on passives and on chain CONTINUATION sub-steps —
    /// only the priced chain entry step carries the cost.
    pub fn mp_cost(&self) -> u32 {
        self.number(SkilldataFields::ConsumeMp as usize) as u32
    }

    /// The vanilla skill window's native placement of this skill (cols
    /// 57-60: UI_SkillTab, Page, Column, Row); `None` when the tab is 255 =
    /// hidden (monster/NPC rows). Corpus (Bicheon): the smash ladder sits at
    /// column 0 rows 0-5, chains column 1, shield column 2, …
    pub fn ui_grid(&self) -> Option<(u8, u8, u8, u8)> {
        let at = |f: SkilldataFields| self.number(f as usize) as u8;
        let tab = at(SkilldataFields::UiTab);
        (tab != 255).then(|| {
            (
                tab,
                at(SkilldataFields::UiPage),
                at(SkilldataFields::UiColumn),
                at(SkilldataFields::UiRow),
            )
        })
    }

    /// Key of the skill's learn/study description (`SN_*_STUDY`, col 65) in
    /// the textdata name tables; `None` for the `xxx` placeholder.
    pub fn study_key(&self) -> Option<&str> {
        self.field(SkilldataFields::StudyDescId as usize)
    }

    /// Key of the skill's tooltip description (`SN_*_TT_DESC`) in the
    /// textdata name tables.
    pub fn tooltip_key(&self) -> Option<&str> {
        self.field(SkilldataFields::TooltipDescId as usize)
    }

    /// The value following a fourcc parameter tag in the trailing parameter
    /// stream (col 69+), e.g. `param_after("dura")` = effect duration in ms.
    /// The stream mixes tags and their arguments; a lookup keyed on the tag
    /// is enough for single-argument tags.
    pub fn param_after(&self, tag: &str) -> Option<i64> {
        let tag = fourcc(tag)?;
        let params = &self.0.get(SkilldataFields::ParamsStart as usize..)?;
        params
            .iter()
            .position(|v| v.trim().parse::<i64>() == Ok(tag))
            .and_then(|idx| params.get(idx + 1))
            .and_then(|v| v.trim().parse::<i64>().ok())
    }

    /// Human-readable stat lines from the parameter stream — the damage /
    /// buff numbers the vanilla tooltip lists (attack power, critical,
    /// attack distance, duration, …).
    ///
    /// The stream is a linear list of fourcc tags each followed by a fixed
    /// argument count, so it is walked left to right: displayed tags emit a
    /// line, known plumbing tags are consumed silently, and the walk STOPS
    /// at the first unknown tag — argument counts beyond the corpus-verified
    /// set are guesses, and a misaligned walk would read arguments as tags
    /// (e.g. `'da'` rows carry the literal fourcc `'cr'` as an argument).
    /// Better to drop trailing stats than show wrong ones.
    pub fn stat_lines(&self) -> Vec<String> {
        let Some(params) = self.0.get(SkilldataFields::ParamsStart as usize..) else {
            return Vec::new();
        };
        let vals: Vec<i64> = params
            .iter()
            .map_while(|v| v.trim().parse::<i64>().ok())
            .collect();
        let mut lines = Vec::new();
        let mut i = 0;
        while i < vals.len() {
            let Some(tag) = decode_fourcc(vals[i]) else {
                // plain number outside any known tag: skip it
                i += 1;
                continue;
            };
            let arg = |n: usize| vals.get(i + n).copied().unwrap_or(0);
            // (arg count, emitted line) per corpus-verified tag
            let (argc, line): (usize, Option<String>) = match tag.as_str() {
                // 'att' <kind> <pct> <min> <max> <x>
                "att" => {
                    let (pct, min, max) = (arg(2), arg(3), arg(4));
                    let line = if min > 0 || max > 0 {
                        format!("Attack power {pct}% + {min} ~ {max}")
                    } else {
                        format!("Attack power {pct}%")
                    };
                    (5, Some(line))
                }
                // 'mc' <hits> <x> — chain/combo hit count
                "mc" => (
                    2,
                    (arg(1) > 1).then(|| format!("{} consecutive hits", arg(1))),
                ),
                "dura" => (1, Some(format!("Duration: {}s", arg(1) / 1000))),
                "cr" => (1, Some(format!("Critical +{}", arg(1)))),
                "ru" => (1, Some(format!("Attack distance +{}", arg(1)))),
                "heal" => (1, Some(format!("HP recovery +{}", arg(1)))),
                "defp" => (1, Some(format!("Defense power +{}", arg(1)))),
                // 'hr'/'er' <flat> <pct> — hit/parry rate; exactly one of the
                // two args is non-zero across the corpus
                "hr" => (2, Some(rate_line("Hit rate", arg(1), arg(2)))),
                "er" => (2, Some(rate_line("Parry rate", arg(1), arg(2)))),
                // 'summ' <lifetime_ms> <10> <3500> <power> <0> — the Hawk
                // Training attack bird (args 1/2 are corpus-wide constants)
                "summ" => (
                    5,
                    Some(format!("Summons an attack hawk (attack power {})", arg(4))),
                ),
                // 'st' <duration_ms> <prob%> <level> (see skills/status.rs)
                "st" => (3, Some(format!("Stun {}% for {}s", arg(2), arg(1) / 1000))),
                // plumbing consumed without display: consume cost, area
                // geometry, value getters, required-item gate
                "cnsm" => (3, None),
                "efr" => (6, None),
                "getv" => (1, None),
                "MAAT" => (0, None),
                "reqi" => (2, None),
                // unknown tag: stop rather than risk a misaligned read
                _ => break,
            };
            lines.extend(line);
            i += 1 + argc;
        }
        lines
    }
}

/// "<what> +N" / "+N%" for the two-arg rate tags (`hr`/`er`), whichever arg
/// is authored.
fn rate_line(what: &str, flat: i64, pct: i64) -> String {
    if pct != 0 {
        format!("{what} +{pct}%")
    } else {
        format!("{what} +{flat}")
    }
}

/// Decodes a parameter value back into its ASCII fourcc tag (2-4 alphabetic
/// chars); `None` for plain numbers.
fn decode_fourcc(value: i64) -> Option<String> {
    if value <= 0 {
        return None;
    }
    let mut s = String::new();
    let mut x = value;
    while x > 0 {
        s.insert(0, (x & 0xff) as u8 as char);
        x >>= 8;
    }
    ((2..=4).contains(&s.len()) && s.chars().all(|c| c.is_ascii_alphabetic())).then_some(s)
}

/// Encodes an ASCII tag (`att`, `dura`) as the big-endian integer the
/// skilldata parameter stream uses.
fn fourcc(tag: &str) -> Option<i64> {
    if tag.is_empty() || tag.len() > 4 || !tag.is_ascii() {
        return None;
    }
    Some(tag.bytes().fold(0i64, |acc, b| (acc << 8) | b as i64))
}

#[cfg(test)]
mod test {
    use super::*;

    fn row() -> SkillDataRow {
        let mut fields = vec![String::new(); 119];
        fields[1] = String::from("100");
        fields[2] = String::from("174");
        fields[3] = String::from("SKILL_CH_COLD_BINGBYEOK_B_01");
        fields[5] = String::from("SKILL_CH_COLD_BINGBYEOK_B");
        fields[7] = String::from("1");
        fields[8] = String::from("2");
        fields[12] = String::from("411");
        fields[14] = String::from("3000");
        fields[22] = String::from("1");
        fields[29] = String::from("1");
        fields[34] = String::from("273");
        fields[36] = String::from("7");
        fields[46] = String::from("2");
        fields[61] = String::from("skill\\china\\cold_bingbyeok_b.ddj");
        fields[62] = String::from("SN_SKILL_CH_COLD_BINGBYEOK_B");
        // fourcc parameter stream: 'dura' 6000, 'att' 8
        fields[69] = String::from("1685418593");
        fields[70] = String::from("6000");
        fields[71] = String::from("6386804");
        fields[72] = String::from("8");
        SkillDataRow(fields)
    }

    #[test]
    fn icon_path_builds_media_icon_url() {
        assert_eq!(
            row().icon_path(),
            Some(String::from(
                "media://icon/skill/china/cold_bingbyeok_b.ddj"
            ))
        );
        // placeholder and short rows yield no icon
        let mut placeholder = row();
        placeholder.0[61] = String::from("xxx");
        assert_eq!(placeholder.icon_path(), None);
        assert_eq!(SkillDataRow(vec![String::new(); 4]).icon_path(), None);
    }

    #[test]
    fn name_key_and_codename() {
        assert_eq!(row().name_key(), Some("SN_SKILL_CH_COLD_BINGBYEOK_B"));
        assert_eq!(row().code_name(), "SKILL_CH_COLD_BINGBYEOK_B_01");
    }

    #[test]
    fn activity_classes() {
        assert!(row().is_castable());
        let mut passive = row();
        passive.0[8] = String::from("0");
        assert!(!passive.is_castable());
        let mut instant = row();
        instant.0[8] = String::from("1");
        assert!(instant.is_castable());
    }

    #[test]
    fn leveling_columns() {
        let row = row();
        assert_eq!(row.group_id(), 174);
        assert_eq!(row.basic_group(), Some("SKILL_CH_COLD_BINGBYEOK_B"));
        assert_eq!(row.basic_level(), 1);
        assert!(row.is_learnable_root());
        assert_eq!(row.mastery_req(), Some((273, 7)));
        assert_eq!(row.sp_cost(), 2);
        assert_eq!(row.skill_prereq(), None);
        assert_eq!(row.casting_time_ms(), 411);
        assert_eq!(row.reuse_delay_ms(), 3000);
        assert!(row.target_required());
        assert!(row.targets_enemy());
        assert!(!row.targets_self());
    }

    #[test]
    fn weapon_and_consume_columns() {
        let mut row = row();
        // cols 50/51 = required cast weapons; 53 = Consume_MP; 11 = prep
        row.0[50] = String::from("2");
        row.0[51] = String::from("3");
        row.0[53] = String::from("19");
        row.0[11] = String::from("166");
        assert_eq!(row.required_weapons(), Some((2, 3)));
        assert_eq!(row.mp_cost(), 19);
        assert_eq!(row.preparing_time_ms(), 166);
        // 255 = unrestricted → no requirement
        row.0[50] = String::from("255");
        assert_eq!(row.required_weapons(), None);
        // passives and chain continuations are free
        row.0[53] = String::from("0");
        assert_eq!(row.mp_cost(), 0);
    }

    #[test]
    fn consume_and_flight_columns() {
        let mut row = row();
        // bow-style projectile: 400 u/s flight, weapon-range attack (col 21
        // = 0), resumes auto-attack; 165 MP / no HP cost (the ice nuke)
        row.0[16] = String::from("400");
        row.0[19] = String::from("1");
        row.0[53] = String::from("165");
        assert_eq!(row.flying_speed(), Some(400.0));
        assert_eq!(row.auto_attack_type(), 1);
        assert_eq!(row.action_range(), None);
        assert_eq!(row.hp_cost(), 0);
        assert_eq!(row.mp_cost(), 165);
        // monster nuke: authored range, no projectile
        row.0[16] = String::from("0");
        row.0[21] = String::from("150");
        row.0[52] = String::from("495");
        assert_eq!(row.flying_speed(), None);
        assert_eq!(row.action_range(), Some(150.0));
        assert_eq!(row.hp_cost(), 495);
    }

    #[test]
    fn target_group_columns() {
        // ally via col 27 alone, via col 28 alone, dead-body via col 33
        let mut row = row();
        assert!(!row.targets_ally() && !row.targets_dead());
        row.0[27] = String::from("1");
        assert!(row.targets_ally());
        row.0[27] = String::from("0");
        row.0[28] = String::from("1");
        assert!(row.targets_ally());
        row.0[33] = String::from("1");
        assert!(row.targets_dead());
    }

    #[test]
    fn ui_grid_and_study_columns() {
        let mut row = row();
        // fixture leaves 57-60 empty = tab 0, first cell (player skill)
        assert_eq!(row.ui_grid(), Some((0, 0, 0, 0)));
        row.0[57] = String::from("1");
        row.0[59] = String::from("3");
        assert_eq!(row.ui_grid(), Some((1, 0, 3, 0)));
        // 255 = hidden monster/NPC row
        row.0[57] = String::from("255");
        assert_eq!(row.ui_grid(), None);
        // study key filters the placeholder
        assert_eq!(row.study_key(), None);
        row.0[65] = String::from("SN_SKILL_CH_COLD_BINGBYEOK_B_STUDY");
        assert_eq!(row.study_key(), Some("SN_SKILL_CH_COLD_BINGBYEOK_B_STUDY"));
    }

    #[test]
    fn stat_lines_walk_the_param_stream() {
        // real corpus streams: Anti Devil Bow - Missile lvl 1
        // ('att' 6 150 13 18 150, 'cnsm' 4 1 1, 'cr' 20, 'getv' 'MAAT')
        let mut r = row();
        let stream = [
            6386804, 6, 150, 13, 18, 150, 1668182893, 4, 1, 1, 25458, 20, 1734702198, 1296122196,
        ];
        for (i, v) in stream.iter().enumerate() {
            if 69 + i >= r.0.len() {
                r.0.push(String::new());
            }
            r.0[69 + i] = v.to_string();
        }
        assert_eq!(
            r.stat_lines(),
            vec!["Attack power 150% + 13 ~ 18", "Critical +20"]
        );

        // Demon Soul Arrow lvl 1: 'dura' 390756, 'ru' 20, 'reqi' 6 6
        let mut r = row();
        let stream = [1685418593, 390756, 29301, 20, 1919250793, 6, 6];
        for (i, v) in stream.iter().enumerate() {
            r.0[69 + i] = v.to_string();
        }
        assert_eq!(
            r.stat_lines(),
            vec!["Duration: 390s", "Attack distance +20"]
        );

        // unknown tag ('da') stops the walk — its args carry literal 'cr'
        // fourccs that a misaligned read would report as bogus crit lines
        let mut r = row();
        let stream = [
            6386804, 6, 100, 5, 9, 100, 25697, 150, 1919250787, 1, 25458, 5,
        ];
        for (i, v) in stream.iter().enumerate() {
            r.0[69 + i] = v.to_string();
        }
        assert_eq!(r.stat_lines(), vec!["Attack power 100% + 5 ~ 9"]);

        // Hawk Training: White Hawk Summon lvl 1 = 'dura' 345378, 'hr' 9 0
        let mut r = row();
        let stream = [1685418593, 345378, 26738, 9, 0];
        for (i, v) in stream.iter().enumerate() {
            r.0[69 + i] = v.to_string();
        }
        assert_eq!(r.stat_lines(), vec!["Duration: 345s", "Hit rate +9"]);

        // Black Hawk Summon lvl 1 = 'dura' 456303, 'summ' 456303 10 3500 255 0
        let mut r = row();
        let stream = [1685418593, 456303, 1937075565, 456303, 10, 3500, 255, 0];
        for (i, v) in stream.iter().enumerate() {
            r.0[69 + i] = v.to_string();
        }
        assert_eq!(
            r.stat_lines(),
            vec![
                "Duration: 456s",
                "Summons an attack hawk (attack power 255)"
            ]
        );
    }

    #[test]
    fn multiple_prereqs() {
        let mut row = row();
        row.0[40] = String::from("174");
        row.0[43] = String::from("9");
        row.0[41] = String::from("175");
        row.0[44] = String::from("3");
        let prereqs: Vec<_> = row.skill_prereqs().collect();
        assert_eq!(prereqs, vec![(174, 9), (175, 3)]);
        // a prereq only in the second slot is still found by skill_prereq
        row.0[40] = String::new();
        row.0[43] = String::new();
        assert_eq!(row.skill_prereq(), Some((175, 3)));
    }

    #[test]
    fn fourcc_params() {
        let row = row();
        assert_eq!(fourcc("dura"), Some(1685418593));
        assert_eq!(fourcc("att"), Some(6386804));
        assert_eq!(row.param_after("dura"), Some(6000));
        assert_eq!(row.param_after("att"), Some(8));
        assert_eq!(row.param_after("onff"), None);
    }

    /// Anchor test against the real archive locking the verified column map.
    #[test]
    #[ignore = "needs Media.pk2; run with: cargo test -p client skilldata_real_columns -- --ignored"]
    fn skilldata_real_columns() {
        use bevy_pk2::prelude::Archive;
        use std::collections::HashMap;
        use std::path::{Path, PathBuf};

        let assets = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("assets");
        let archive = Archive::configured(assets.join("Media.pk2"));
        let bytes = archive
            .read_file_bytes(Path::new("server_dep/silkroad/textdata/skilldata_5000.txt"))
            .expect("skilldata_5000.txt in Media.pk2");
        let content = super::super::skilleffect::decode_utf16le(&bytes);
        let rows: Vec<SkillDataRow> = content
            .lines()
            .map(|l| SkillDataRow(l.split('\t').map(String::from).collect()))
            .filter(|r| r.0.len() > 100)
            .collect();
        assert!(rows.len() > 4000, "only {} rows", rows.len());

        // activity histogram: passives, instants, castables
        let mut activity: HashMap<u8, usize> = HashMap::new();
        for row in &rows {
            *activity.entry(row.activity()).or_default() += 1;
        }
        assert_eq!(activity[&0], 84);
        assert_eq!(activity[&1], 191);
        assert_eq!(activity[&2], 4570);

        // sword smash A lv2: Bicheon lv7 required, 2 SP, 'att' param present
        let smash = rows
            .iter()
            .find(|r| r.code_name() == "SKILL_CH_SWORD_SMASH_A_02")
            .unwrap();
        assert_eq!(smash.group_id(), 174);
        assert_eq!(smash.basic_group(), Some("SKILL_CH_SWORD_SMASH_A"));
        assert_eq!(smash.basic_level(), 2);
        assert_eq!(smash.mastery_req(), Some((257, 7)));
        assert_eq!(smash.sp_cost(), 2);
        assert_eq!(smash.casting_time_ms(), 411);
        assert_eq!(smash.reuse_delay_ms(), 3000);
        assert!(smash.target_required() && smash.targets_enemy());
        assert!(smash.param_after("att").is_some());
        // consume / action / UI columns: 23 MP, melee (weapon range, no
        // projectile), resumes auto-attack, first board cell, study text
        assert_eq!(smash.mp_cost(), 23);
        assert_eq!(smash.hp_cost(), 0);
        assert_eq!(smash.flying_speed(), None);
        assert_eq!(smash.action_range(), None);
        assert_eq!(smash.auto_attack_type(), 1);
        assert_eq!(smash.ui_grid(), Some((0, 0, 0, 0)));
        assert_eq!(smash.study_key(), Some("SN_SKILL_CH_SWORD_SMASH_A_STUDY"));
        assert!(!smash.targets_ally() && !smash.targets_dead());

        // chain CONTINUATION step: free and hidden from the board
        let chain_2s = rows
            .iter()
            .find(|r| r.code_name() == "SKILL_CH_SWORD_CHAIN_A_2S_01")
            .unwrap();
        assert_eq!(chain_2s.mp_cost(), 0);
        assert_eq!(chain_2s.ui_grid(), None);

        // resurrection: ally-group + dead-body targeting
        let resurrection = rows
            .iter()
            .find(|r| r.code_name() == "SKILL_CH_WATER_RESURRECTION_A_01")
            .unwrap();
        assert!(resurrection.targets_ally() && resurrection.targets_dead());

        // smash B lv1 requires smash A (group 174) at level 9
        let smash_b = rows
            .iter()
            .find(|r| r.code_name() == "SKILL_CH_SWORD_SMASH_B_01")
            .unwrap();
        assert_eq!(smash_b.skill_prereq(), Some((174, 9)));
        assert!(smash_b.is_learnable_root());

        // imbue: activity 1, self-target, 'dura' 6000
        let imbue = rows
            .iter()
            .find(|r| r.code_name() == "SKILL_CH_FIRE_GIGONGTA_A_01")
            .unwrap();
        assert_eq!(imbue.activity(), 1);
        assert_eq!(imbue.param_after("dura"), Some(6000));

        // EU player skills live in the 10000+ shards; same column scheme
        let bytes = archive
            .read_file_bytes(Path::new(
                "server_dep/silkroad/textdata/skilldata_10000.txt",
            ))
            .expect("skilldata_10000.txt in Media.pk2");
        let content = super::super::skilleffect::decode_utf16le(&bytes);
        let rows: Vec<SkillDataRow> = content
            .lines()
            .map(|l| SkillDataRow(l.split('\t').map(String::from).collect()))
            .filter(|r| r.0.len() > 100)
            .collect();
        let eu = rows
            .iter()
            .find(|r| r.code_name() == "SKILL_EU_WARRIOR_FRENZYA_TOUNT_A_01")
            .unwrap();
        assert_eq!(eu.group_id(), 536);
        assert_eq!(eu.basic_group(), Some("SKILL_EU_WARRIOR_FRENZYA_TOUNT_A"));
        assert_eq!(eu.mastery_req(), Some((513, 10)));
        assert_eq!(eu.sp_cost(), 4);
        assert!(eu.is_learnable_root());

        // bow projectile skill (id 7821): authored 400 u/s flight speed
        let bow = rows
            .iter()
            .find(|r| r.code_name() == "SKILL_CH_BOW_CRITICAL_D_01")
            .unwrap();
        assert_eq!(bow.flying_speed(), Some(400.0));
        assert_eq!(bow.mp_cost(), 1012);
        assert_eq!(bow.ui_grid(), Some((0, 2, 0, 3)));
    }
}
