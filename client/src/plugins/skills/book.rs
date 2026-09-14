//! The local player's learned skills and mastery levels, plus the pure
//! learn/level/withdraw rules.
//!
//! Idea: skilldata rows form per-skill "ladders" — all levels of one skill
//! share a `group_id` (col 2) and `basic_group` codename, ordered by
//! `basic_level`. Learning always takes the next ladder rung; the
//! requirements live on that rung (required mastery level, prerequisite
//! skill group + level, SP cost). [`SkillGroupIndex`] pre-sorts the ladders
//! and the per-mastery root list once per skilldata load so the window and
//! the cast pipeline never scan the whole table. The SP wallet is the
//! underbar's [`PlayerProgress::skill_points`]. Offline simplifications
//! (documented for the server iteration): mastery level-up costs no SP and
//! is capped by character level; withdrawing refunds the full SP sum with no
//! gold cost and no dependency cascade onto skills that required the
//! withdrawn levels.

use bevy::prelude::*;
use std::collections::HashMap;

use crate::assets::textdata::skilldata::{SkillData, SkillDataRow};
use crate::plugins::hud::underbar::model::PlayerProgress;
use crate::plugins::textdata::ClientSkillData;

/// One learned skill ladder: the currently learned rung.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LearnedSkill {
    /// skilldata id of the learned level's row (what quickslots store).
    pub skill_id: i32,
    pub level: u8,
}

/// The local player's skill book, keyed the way the data references skills:
/// masteries by mastery id, learned ladders by their `group_id` (which is
/// also what prerequisite columns point at).
#[derive(Resource, Default)]
pub struct SkillBook {
    pub masteries: HashMap<u32, u32>,
    pub learned: HashMap<i32, LearnedSkill>,
}

impl SkillBook {
    pub fn mastery_level(&self, mastery: u32) -> u32 {
        self.masteries.get(&mastery).copied().unwrap_or(0)
    }

    pub fn learned_level(&self, group_id: i32) -> u8 {
        self.learned.get(&group_id).map(|l| l.level).unwrap_or(0)
    }

    /// Whether the given skilldata row (any rung) is currently the learned
    /// level of its ladder — i.e. castable from the book.
    pub fn knows(&self, group_id: i32, skill_id: i32) -> bool {
        self.learned
            .get(&group_id)
            .is_some_and(|l| l.skill_id == skill_id)
    }
}

/// Pre-sorted view over skilldata for the skill window and leveling rules:
/// per-group ladders and per-mastery learnable root groups.
#[derive(Resource, Default)]
pub struct SkillGroupIndex {
    /// group_id → skilldata ids ordered by `basic_level` ascending.
    pub ladders: HashMap<i32, Vec<i32>>,
    /// mastery id → root group_ids ordered by required mastery level (col 36).
    pub by_mastery: HashMap<u32, Vec<i32>>,
}

impl SkillGroupIndex {
    pub fn is_built(&self) -> bool {
        !self.ladders.is_empty()
    }

    /// The next-to-learn rung of a ladder given the book's current level:
    /// level 1 when unlearned, `None` when maxed or unknown.
    pub fn next_level_id(&self, book: &SkillBook, group_id: i32) -> Option<i32> {
        let ladder = self.ladders.get(&group_id)?;
        ladder.get(book.learned_level(group_id) as usize).copied()
    }
}

/// (Re)build the index whenever the skilldata table arrives. Ladders include
/// every castable/passive row with a mastery; the per-mastery root lists only
/// level-1 rows with icons (what the window shows).
pub fn index_skill_groups(skill_data: Res<ClientSkillData>, mut index: ResMut<SkillGroupIndex>) {
    if !skill_data.is_changed() {
        return;
    }
    let Some(data) = skill_data.data() else {
        return;
    };
    let (ladders, by_mastery) = build_index(data);
    index.by_mastery = by_mastery;
    index.ladders = ladders;
    info!(
        "skills: indexed {} ladders across {} masteries",
        index.ladders.len(),
        index.by_mastery.len()
    );
}

/// Build the per-group level ladders and the per-mastery root-group lists
/// from the skilldata table. Pure (testable) core of [`index_skill_groups`].
///
/// A skill-group can carry several rows per level — chain/combo sub-steps
/// (`SKILL_CH_SWORD_CHAIN_A_1S/2S/3S…`) and higher-tier variants
/// (`SMASH_C2/C3`). Those are ONE learnable skill whose combo the entry step
/// triggers, so each level collapses to a single representative rung: prefer
/// the rung with a nonzero MP cost (col 53 Consume_MP — chain CONTINUATION
/// segments are free and carry 0 SP, and can sort lexicographically before
/// the priced entry: `CHAIN_D2_01` < `CHAIN_D_01`, which showed 0-SP
/// skills), then the lexicographically-smallest `basic_code`. Per-mastery
/// roots are ordered by required mastery level.
fn build_index(data: &SkillData) -> (HashMap<i32, Vec<i32>>, HashMap<u32, Vec<i32>>) {
    let mut ladders: HashMap<i32, Vec<(u8, bool, String, i32)>> = HashMap::new();
    let mut roots: HashMap<u32, Vec<(u32, i32)>> = HashMap::new();
    for (id, row) in data.iter() {
        let Some((mastery, req_level)) = row.mastery_req() else {
            continue;
        };
        let group = row.group_id();
        if group == 0 {
            continue;
        }
        ladders.entry(group).or_default().push((
            row.basic_level(),
            // sorts free (0 MP = continuation) rungs last within a level
            row.mp_cost() == 0,
            row.code_name().clone(),
            *id,
        ));
        if row.is_learnable_root() {
            roots.entry(mastery).or_default().push((req_level, group));
        }
    }
    let ladders: HashMap<i32, Vec<i32>> = ladders
        .into_iter()
        .map(|(group, mut rungs)| {
            rungs.sort_unstable();
            let mut ladder = Vec::new();
            let mut last_level: Option<u8> = None;
            for (level, _free, _code, id) in rungs {
                if last_level != Some(level) {
                    ladder.push(id);
                    last_level = Some(level);
                }
            }
            (group, ladder)
        })
        .collect();
    let by_mastery = roots
        .into_iter()
        .map(|(mastery, mut groups)| {
            groups.sort_unstable();
            groups.dedup_by_key(|(_, group)| *group);
            (
                mastery,
                groups
                    .into_iter()
                    .map(|(_, group)| group)
                    .filter(|group| ladders.contains_key(group))
                    .collect(),
            )
        })
        .collect();
    (ladders, by_mastery)
}

/// Why a learn attempt is rejected (surfaced by the window UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LearnBlock {
    MasteryTooLow { mastery: u32, required: u32 },
    MissingPrereq { group_id: i32, level: u8 },
    NotEnoughSp { required: u32 },
}

/// One mastery/skill requirement of a rung (the tooltip lists all of them,
/// met or not; SP is handled separately as the cost line).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LearnReq {
    Mastery { mastery: u32, required: u32 },
    Skill { group_id: i32, level: u8 },
}

/// Every mastery/prereq requirement of a rung with its met status,
/// mastery first (its failure alone locks the cell art).
pub fn learn_requirements(row: &SkillDataRow, book: &SkillBook) -> Vec<(LearnReq, bool)> {
    let mut reqs = Vec::new();
    if let Some((mastery, required)) = row.mastery_req() {
        reqs.push((
            LearnReq::Mastery { mastery, required },
            book.mastery_level(mastery) >= required,
        ));
    }
    for (group_id, level) in row.skill_prereqs() {
        reqs.push((
            LearnReq::Skill { group_id, level },
            book.learned_level(group_id) >= level,
        ));
    }
    reqs
}

/// Check the next rung's requirements against the book and SP wallet.
pub fn can_learn(row: &SkillDataRow, book: &SkillBook, sp: u32) -> Result<(), LearnBlock> {
    for (req, met) in learn_requirements(row, book) {
        if !met {
            return Err(match req {
                LearnReq::Mastery { mastery, required } => {
                    LearnBlock::MasteryTooLow { mastery, required }
                }
                LearnReq::Skill { group_id, level } => {
                    LearnBlock::MissingPrereq { group_id, level }
                }
            });
        }
    }
    if sp < row.sp_cost() {
        return Err(LearnBlock::NotEnoughSp {
            required: row.sp_cost(),
        });
    }
    Ok(())
}

/// Learn the next rung of `group_id`, deducting SP. Returns the newly
/// learned skill id, or `None` when blocked/maxed.
pub fn learn_next(
    book: &mut SkillBook,
    index: &SkillGroupIndex,
    skill_data: &SkillData,
    progress: &mut PlayerProgress,
    group_id: i32,
) -> Option<i32> {
    let next_id = index.next_level_id(book, group_id)?;
    let row = skill_data.get(&next_id)?;
    can_learn(row, book, progress.skill_points).ok()?;
    progress.skill_points -= row.sp_cost();
    book.learned.insert(
        group_id,
        LearnedSkill {
            skill_id: next_id,
            level: row.basic_level(),
        },
    );
    Some(next_id)
}

/// Withdraw a ladder down to `target_level` (0 = unlearn entirely),
/// refunding the SP of every removed rung. Returns the refund.
pub fn withdraw_to(
    book: &mut SkillBook,
    index: &SkillGroupIndex,
    skill_data: &SkillData,
    progress: &mut PlayerProgress,
    group_id: i32,
    target_level: u8,
) -> u32 {
    let current = book.learned_level(group_id);
    if target_level >= current {
        return 0;
    }
    let Some(ladder) = index.ladders.get(&group_id) else {
        return 0;
    };
    let refund: u32 = ladder[target_level as usize..current as usize]
        .iter()
        .filter_map(|id| skill_data.get(id))
        .map(|row| row.sp_cost())
        .sum();
    if target_level == 0 {
        book.learned.remove(&group_id);
    } else if let Some(&id) = ladder.get(target_level as usize - 1) {
        book.learned.insert(
            group_id,
            LearnedSkill {
                skill_id: id,
                level: target_level,
            },
        );
    }
    progress.skill_points += refund;
    refund
}

/// Seed the book from CHARACTER_DATA in the networked scene (mirrors the
/// underbar's quickslot seeding): server-known skills become learned ladder
/// rungs, masteries copy over directly. Re-runs when the skilldata table
/// streams in after the join.
pub fn seed_skillbook_from_character_info(
    players: Query<
        &crate::plugins::net::character_info::CharacterInfo,
        With<crate::plugins::player::Player>,
    >,
    added: Query<
        (),
        (
            With<crate::plugins::player::Player>,
            Added<crate::plugins::net::character_info::CharacterInfo>,
        ),
    >,
    skill_data: Res<ClientSkillData>,
    mut book: ResMut<SkillBook>,
    mut seeded: Local<bool>,
) {
    if !added.is_empty() {
        *seeded = false;
    }
    if *seeded || !skill_data.is_loaded() {
        return;
    }
    let Ok(info) = players.single() else {
        return;
    };
    book.masteries = info
        .masteries
        .iter()
        .map(|m| (m.id, m.level as u32))
        .collect();
    book.learned.clear();
    for skill in &info.skills {
        if skill.enabled != 1 {
            continue;
        }
        let Some(row) = skill_data.get(&(skill.id as i32)) else {
            continue;
        };
        let group = row.group_id();
        if group == 0 {
            continue;
        }
        let level = row.basic_level();
        let entry = book.learned.entry(group).or_insert(LearnedSkill {
            skill_id: skill.id as i32,
            level,
        });
        if level > entry.level {
            *entry = LearnedSkill {
                skill_id: skill.id as i32,
                level,
            };
        }
    }
    *seeded = true;
    debug!(
        "skills: seeded book with {} masteries / {} ladders",
        book.masteries.len(),
        book.learned.len()
    );
}

/// Apply 0xB0A1/0xB0A2 learn acks to the book — the ack is the server's ONLY
/// learn notification (no character-data refresh follows). Mutating SkillBook
/// is all it takes downstream: skill_window_needs_refresh repaints on
/// book.is_changed() and sync_quickslots_with_book upgrades slotted rungs.
/// SP is not deducted here — 0x304E CharacterPointsUpdate stays the
/// authority on PlayerProgress.
pub fn apply_learn_responses(
    mut skill_acks: MessageReader<packets::agent::prelude::SkillLearnResponse>,
    mut mastery_acks: MessageReader<packets::agent::prelude::MasteryLearnResponse>,
    skill_data: Res<ClientSkillData>,
    mut book: ResMut<SkillBook>,
) {
    use packets::agent::prelude::{MasteryLearnResponse, SkillLearnResponse};
    // ResMut derefs below would dirty book.is_changed() (and repaint the
    // window) every frame — bail before touching it on ack-free frames
    if skill_acks.is_empty() && mastery_acks.is_empty() {
        return;
    }
    for ack in skill_acks.read() {
        match ack {
            SkillLearnResponse::Success { ref_skill_id } => {
                let Some(row) = skill_data.get(&(*ref_skill_id as i32)) else {
                    warn!("skills: learned unknown skill id {ref_skill_id}");
                    continue;
                };
                let group = row.group_id();
                if group == 0 {
                    warn!("skills: learned groupless skill {}", row.code_name());
                    continue;
                }
                info!(
                    "skills: server confirmed {} lv {}",
                    row.code_name(),
                    row.basic_level()
                );
                book.learned.insert(
                    group,
                    LearnedSkill {
                        skill_id: *ref_skill_id as i32,
                        level: row.basic_level(),
                    },
                );
            }
            SkillLearnResponse::Failure(code) => {
                warn!("skills: learn rejected (code {code:#06x})");
            }
            SkillLearnResponse::Unknown { result, tail } => {
                warn!("skills: unexpected 0xB0A1 shape (result {result} tail {tail:02x?})");
            }
        }
    }
    for ack in mastery_acks.read() {
        match ack {
            MasteryLearnResponse::Success {
                mastery_id,
                new_level,
            } => {
                info!("skills: server confirmed mastery {mastery_id} lv {new_level}");
                book.masteries.insert(*mastery_id, *new_level as u32);
            }
            MasteryLearnResponse::Failure(code) => {
                warn!("skills: mastery raise rejected (code {code:#06x})");
            }
            MasteryLearnResponse::Unknown { result, tail } => {
                warn!("skills: unexpected 0xB0A2 shape (result {result} tail {tail:02x?})");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2-rung sword ladder (group 174) + a follow-up group (175) that
    /// requires 174 at level 2, mirroring the SMASH_A → SMASH_B shape.
    fn table() -> (SkillData, SkillGroupIndex) {
        let mut rows = HashMap::new();
        let mut row = |id: i32, fields: &[(usize, &str)]| {
            let mut cols = vec![String::new(); 119];
            cols[1] = id.to_string();
            for &(idx, value) in fields {
                cols[idx] = value.to_string();
            }
            rows.insert(id, SkillDataRow(cols));
        };
        row(
            291,
            &[
                (2, "174"),
                (3, "SMASH_A_01"),
                (5, "SKILL_SMASH_A"),
                (7, "1"),
                (8, "2"),
                (34, "257"),
                (36, "1"),
                (46, "2"),
                (61, "skill\\a.ddj"),
            ],
        );
        row(
            292,
            &[
                (2, "174"),
                (3, "SMASH_A_02"),
                (5, "SKILL_SMASH_A"),
                (7, "2"),
                (8, "2"),
                (34, "257"),
                (36, "7"),
                (46, "3"),
                (61, "skill\\a.ddj"),
            ],
        );
        row(
            300,
            &[
                (2, "175"),
                (3, "SMASH_B_01"),
                (5, "SKILL_SMASH_B"),
                (7, "1"),
                (8, "2"),
                (34, "257"),
                (36, "10"),
                (40, "174"),
                (43, "2"),
                (46, "5"),
                (61, "skill\\b.ddj"),
            ],
        );
        // group 176 uses BOTH prereq slots: 174 lv 1 (slot 1) + 175 lv 1 (slot 2)
        row(
            310,
            &[
                (2, "176"),
                (3, "SMASH_C_01"),
                (5, "SKILL_SMASH_C"),
                (7, "1"),
                (8, "2"),
                (34, "257"),
                (36, "10"),
                (40, "174"),
                (43, "1"),
                (41, "175"),
                (44, "1"),
                (46, "5"),
                (61, "skill\\c.ddj"),
            ],
        );
        let data = SkillData(rows);
        let mut index = SkillGroupIndex::default();
        index.ladders.insert(174, vec![291, 292]);
        index.ladders.insert(175, vec![300]);
        index.ladders.insert(176, vec![310]);
        index.by_mastery.insert(257, vec![174, 175, 176]);
        (data, index)
    }

    fn progress(sp: u32) -> PlayerProgress {
        PlayerProgress {
            skill_points: sp,
            ..Default::default()
        }
    }

    #[test]
    fn build_index_collapses_chain_substeps() {
        // group 500 = a chain skill with 3 combo sub-steps at each of 2
        // levels (like SKILL_CH_SWORD_CHAIN_A_1S/2S/3S_01/02) — must NOT be
        // dropped; each level collapses to the _1S entry step.
        let mut rows = HashMap::new();
        let mut add = |id: i32, group: &str, code: &str, level: &str, mp: &str| {
            let mut cols = vec![String::new(); 119];
            cols[1] = id.to_string();
            cols[2] = group.to_string();
            cols[3] = code.to_string();
            cols[5] = "SKILL_CHAIN_A".to_string();
            cols[7] = level.to_string();
            cols[8] = "2".to_string();
            cols[34] = "257".to_string();
            cols[36] = "7".to_string();
            cols[53] = mp.to_string();
            cols[61] = "skill\\c.ddj".to_string();
            rows.insert(id, SkillDataRow(cols));
        };
        // level 1: 3S/2S/1S ; level 2: 1S/2S/3S (insertion order shuffled);
        // only the entry steps carry an MP cost (col 53 Consume_MP)
        add(603, "500", "CHAIN_A_3S_01", "1", "0");
        add(602, "500", "CHAIN_A_2S_01", "1", "0");
        add(601, "500", "CHAIN_A_1S_01", "1", "32");
        add(611, "500", "CHAIN_A_1S_02", "2", "33");
        add(613, "500", "CHAIN_A_3S_02", "2", "0");
        add(612, "500", "CHAIN_A_2S_02", "2", "0");
        // group 510 = the bow-combo naming scheme, where the continuation
        // code sorts lexicographically BEFORE the priced entry
        // (CHAIN_D2_01 < CHAIN_D_01): the MP cost must win the dedup,
        // else every rung shows the 0-SP continuation.
        add(701, "510", "CHAIN_D_01", "1", "790");
        add(702, "510", "CHAIN_D2_01", "1", "0");
        add(703, "510", "CHAIN_D3_01", "1", "0");
        let data = SkillData(rows);

        let (ladders, by_mastery) = build_index(&data);
        // group kept, collapsed to one representative (the _1S entry) per level
        assert_eq!(ladders.get(&500), Some(&vec![601, 611]));
        assert_eq!(ladders.get(&510), Some(&vec![701]));
        assert_eq!(by_mastery.get(&257), Some(&vec![500, 510]));
    }

    #[test]
    fn learn_walks_the_ladder_and_deducts_sp() {
        let (data, index) = table();
        let mut book = SkillBook::default();
        book.masteries.insert(257, 7);
        let mut sp = progress(10);

        assert_eq!(
            learn_next(&mut book, &index, &data, &mut sp, 174),
            Some(291)
        );
        assert_eq!(sp.skill_points, 8);
        assert_eq!(book.learned_level(174), 1);
        assert!(book.knows(174, 291));

        assert_eq!(
            learn_next(&mut book, &index, &data, &mut sp, 174),
            Some(292)
        );
        assert_eq!(sp.skill_points, 5);
        assert!(book.knows(174, 292));
        assert!(!book.knows(174, 291));

        // ladder maxed
        assert_eq!(learn_next(&mut book, &index, &data, &mut sp, 174), None);
    }

    #[test]
    fn learn_blocks_on_requirements() {
        let (data, index) = table();
        let mut book = SkillBook::default();
        let mut sp = progress(100);

        // no mastery at all
        assert_eq!(
            can_learn(data.get(&291).unwrap(), &book, 100),
            Err(LearnBlock::MasteryTooLow {
                mastery: 257,
                required: 1
            })
        );
        book.masteries.insert(257, 20);

        // group 175 requires group 174 at level 2
        assert_eq!(
            can_learn(data.get(&300).unwrap(), &book, 100),
            Err(LearnBlock::MissingPrereq {
                group_id: 174,
                level: 2
            })
        );
        learn_next(&mut book, &index, &data, &mut sp, 174);
        learn_next(&mut book, &index, &data, &mut sp, 174);
        assert!(can_learn(data.get(&300).unwrap(), &book, 100).is_ok());

        // SP wallet short
        assert_eq!(
            can_learn(data.get(&300).unwrap(), &book, 4),
            Err(LearnBlock::NotEnoughSp { required: 5 })
        );
    }

    #[test]
    fn learn_checks_every_prereq_slot() {
        let (data, index) = table();
        let mut book = SkillBook::default();
        book.masteries.insert(257, 20);
        let mut sp = progress(100);
        learn_next(&mut book, &index, &data, &mut sp, 174);

        // group 176: slot-1 prereq (174 lv 1) met, slot-2 (175 lv 1) not
        assert_eq!(
            can_learn(data.get(&310).unwrap(), &book, 100),
            Err(LearnBlock::MissingPrereq {
                group_id: 175,
                level: 1
            })
        );
        learn_next(&mut book, &index, &data, &mut sp, 174);
        learn_next(&mut book, &index, &data, &mut sp, 175);
        assert!(can_learn(data.get(&310).unwrap(), &book, 100).is_ok());
    }

    #[test]
    fn learn_requirements_lists_met_and_unmet() {
        let (data, _index) = table();
        let mut book = SkillBook::default();
        book.masteries.insert(257, 20);
        book.learned.insert(
            174,
            LearnedSkill {
                skill_id: 291,
                level: 1,
            },
        );

        // group 175's rung: mastery 257 lv 10 met, 174 lv 2 not (only lv 1)
        assert_eq!(
            learn_requirements(data.get(&300).unwrap(), &book),
            vec![
                (
                    LearnReq::Mastery {
                        mastery: 257,
                        required: 10
                    },
                    true
                ),
                (
                    LearnReq::Skill {
                        group_id: 174,
                        level: 2
                    },
                    false
                ),
            ]
        );
    }

    #[test]
    fn withdraw_refunds_removed_rungs() {
        let (data, index) = table();
        let mut book = SkillBook::default();
        book.masteries.insert(257, 20);
        let mut sp = progress(10);
        learn_next(&mut book, &index, &data, &mut sp, 174);
        learn_next(&mut book, &index, &data, &mut sp, 174);
        assert_eq!(sp.skill_points, 5);

        // down one level: refund rung 2's cost, learned falls back to rung 1
        assert_eq!(withdraw_to(&mut book, &index, &data, &mut sp, 174, 1), 3);
        assert_eq!(sp.skill_points, 8);
        assert!(book.knows(174, 291));

        // unlearn entirely
        assert_eq!(withdraw_to(&mut book, &index, &data, &mut sp, 174, 0), 2);
        assert_eq!(sp.skill_points, 10);
        assert_eq!(book.learned_level(174), 0);

        // nothing to withdraw
        assert_eq!(withdraw_to(&mut book, &index, &data, &mut sp, 174, 0), 0);
    }
}
