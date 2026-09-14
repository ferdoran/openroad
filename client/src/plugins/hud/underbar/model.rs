//! Underbar state: quickslot assignments and the player's progression numbers.
//!
//! Idea: both resources are session-local mirrors of CHARACTER_DATA (0x3013)
//! fields, re-seeded on every join. The vanilla client stores the quickslot
//! layout server-side (the `hotkeys` list inside 0x3013's extras block) and
//! syncs changes with a config-update packet; the reference server (go-sro)
//! sends an empty list and the sync packet is not reverse-engineered yet, so
//! assignments made here (drag from the inventory) live in memory only. As a
//! stopgap until a skill window exists, page 1 is auto-populated with the
//! character's known active skills so the bar is usable at all.

use bevy::prelude::*;

use packets::agent::prelude::{CharacterPointsUpdate, ReceiveExperience};

use crate::plugins::hud::player_mini_info::PlayerVitals;
use crate::plugins::net::character_info::CharacterInfo;
use crate::plugins::player::Player;
use crate::plugins::textdata::{ClientItemData, ClientLevelData, ClientSkillData};

/// Number of quickslot pages and visible slots per page. Vanilla has 4 pages:
/// `ifunderbar.txt`'s `TMPQS_1..40` occupy exactly 10 distinct rects
/// (x 289..613, pitch 36, y 11, 32x32), four IDs deep per position
/// (20-29 / 30-39 / 40-49 / 50-59).
///
/// The original has **three** distinct quickslot mechanisms and only the
/// first two live in this tree (`docs/re/ui/ext-quickslot-widget.md`):
/// ① this bar plus its banked pages, ② `ifunderbar.txt`'s separate
/// `AdditionalQuickSlot` section (`TMPQS_41..50`) — **not** a fifth page,
/// still unmodeled — and ③ `GDR_EXT_QUICK_SLOT`, a standalone
/// position-persisted 10-slot window (IDs 100-109) with its own option
/// dialog, which we do not implement at all (#358).
///
/// The `UIIT_STT_ADD_QUICKSLOT_*` string family belongs to ③, not to ②: it
/// occurs 5 times in 2 files (`ifextquickslot.txt`, `ifextquickslotoption.txt`)
/// and **zero** times in `ifunderbar.txt`, whose controls all carry `Text=""`.
/// The name "Additional Quick Slot" pointing at ② is the boundary error this
/// comment used to inherit.
pub const PAGES: u8 = 4;
pub const SLOTS_PER_PAGE: u8 = 10;

/// What a quickslot triggers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SlotAction {
    Skill { ref_id: u32 },
    Item { ref_id: u32 },
}

/// The quickslot bar: 4 pages x 10 slots plus the special leftmost "M" slot.
#[derive(Resource, Debug)]
pub struct QuickSlots {
    /// Absolute slot index = page * 10 + column.
    pub slots: [Option<SlotAction>; (PAGES * SLOTS_PER_PAGE) as usize],
    /// The special slot left of the bar. Not unknown: `UIIT_STT_MOUSE_RIGHT_BUTTON`
    /// (textuisystem L916) reads "Mouse quickslot", i.e. this is the
    /// right-mouse-button slot. Only click-to-cast is wired — binding it to RMB
    /// needs a video check of the original's behaviour first (#281).
    pub special: Option<SlotAction>,
    /// Visible page, 0-based.
    pub page: u8,
    /// Absolute index of the armed slot (the arrow indicator), if any.
    ///
    /// Arming expires — see [`QuickSlots::arm`] and `ui::fade_armed_slot`.
    pub armed: Option<u8>,
    /// When [`Self::armed`] was last set, on the `Time::elapsed_secs_f64`
    /// clock. `None` whenever `armed` is `None`; keep the two in step by
    /// arming through [`Self::arm`] rather than assigning the field.
    pub armed_at: Option<f64>,
    /// Slot pressed while already armed — the activation fires on the
    /// matching `Click` (release without drag), so a press-and-drag lifts
    /// the slot content instead of casting.
    pub pending_activate: Option<u8>,
}

impl Default for QuickSlots {
    fn default() -> Self {
        Self {
            slots: [None; (PAGES * SLOTS_PER_PAGE) as usize],
            special: None,
            page: 0,
            armed: None,
            armed_at: None,
            pending_activate: None,
        }
    }
}

impl QuickSlots {
    /// The action in the given column of the visible page.
    pub fn visible(&self, column: u8) -> Option<SlotAction> {
        self.slots[(self.page * SLOTS_PER_PAGE + column) as usize]
    }

    /// Arm `index`, stamping the moment so the indicator can expire.
    ///
    /// The single writer for both fields: arming is what the ring draws *and*
    /// what a second mouse press casts from, so a stamp that drifted from
    /// `armed` would leave a slot that fires with no ring to say so.
    pub fn arm(&mut self, index: u8, now: f64) {
        self.armed = Some(index);
        self.armed_at = Some(now);
    }

    /// Drop the arming. The ring disappears with it, and the next press on
    /// that slot arms again instead of casting.
    pub fn disarm(&mut self) {
        self.armed = None;
        self.armed_at = None;
    }

    /// How far through the hold-then-fade window the current arming is:
    /// `0.0` while held, ramping to `1.0` at the end of the fade. `None` when
    /// nothing is armed.
    pub fn arm_fade(&self, now: f64, hold: f32, fade: f32) -> Option<f32> {
        let armed_at = self.armed_at?;
        self.armed?;
        // A zero/negative fade means "vanish at the end of the hold" rather
        // than dividing by zero into NaN.
        if fade <= 0.0 {
            return Some(if (now - armed_at) as f32 >= hold {
                1.0
            } else {
                0.0
            });
        }
        let elapsed = (now - armed_at) as f32;
        Some(((elapsed - hold) / fade).clamp(0.0, 1.0))
    }
}

/// The player's progression numbers for the EXP/SP bars, seeded from
/// CHARACTER_DATA and kept live by 0x304E (SP). No live EXP packet is modeled
/// yet, so `exp_offset` is static between joins.
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct PlayerProgress {
    pub level: u8,
    /// Exp gathered within the current level (same semantics as the lobby's
    /// per-character exp shown at char-select).
    pub exp_offset: u64,
    /// Skill exp gathered toward the next skill point — the server sends the
    /// within-point offset directly (0x3013 dump-verified).
    pub skill_exp: u32,
    pub skill_points: u32,
}

/// Skill exp needed per skill point (dump-verified: 0x3013 carries
/// `skill_exp` as a 0..400 offset).
pub const SKILL_EXP_PER_SP: u32 = 400;

/// Copy level/exp/SP from the `CharacterInfo` the game scene inserts once
/// CHARACTER_DATA resolves.
pub fn seed_progress_from_character_info(
    added: Query<&CharacterInfo, (With<Player>, Added<CharacterInfo>)>,
    mut progress: ResMut<PlayerProgress>,
) {
    for info in added.iter() {
        let Some(stats) = &info.stats else {
            continue;
        };
        *progress = PlayerProgress {
            level: stats.level,
            exp_offset: stats.exp,
            skill_exp: stats.skill_exp,
            skill_points: stats.skill_points,
        };
        debug!(
            "underbar: seeded progress (level {} exp {} skill_exp {} sp {})",
            stats.level, stats.exp, stats.skill_exp, stats.skill_points
        );
    }
}

/// Seed the quickslots from CHARACTER_DATA: apply the server's hotkey list,
/// then fill the remaining page-1 slots with known active skills. Runs on a
/// fresh `CharacterInfo` AND again when the skilldata table streams in (the
/// textdata usually loads after the join) — the re-seed is idempotent but
/// deliberately skipped once the user has made manual assignments.
pub fn seed_quickslots_from_character_info(
    players: Query<&CharacterInfo, With<Player>>,
    added: Query<(), (With<Player>, Added<CharacterInfo>)>,
    skill_data: Res<ClientSkillData>,
    item_data: Res<ClientItemData>,
    mut quickslots: ResMut<QuickSlots>,
    mut seeded: Local<bool>,
) {
    if !added.is_empty() {
        // a (re)join invalidates everything, including manual assignments
        *seeded = false;
    }
    if *seeded || !skill_data.is_loaded() {
        return;
    }
    let Ok(info) = players.single() else {
        return;
    };

    let mut slots = [None; (PAGES * SLOTS_PER_PAGE) as usize];

    // server-sent hotkeys first (go-sro sends none; layout unverified — log
    // whatever arrives so a live capture can pin the slot base and `kind`)
    if let Some(extras) = &info.extras {
        for hotkey in &extras.hotkeys {
            let action = if skill_data.get(&(hotkey.data as i32)).is_some() {
                Some(SlotAction::Skill {
                    ref_id: hotkey.data,
                })
            } else if item_data.get(&(hotkey.data as i32)).is_some() {
                Some(SlotAction::Item {
                    ref_id: hotkey.data,
                })
            } else {
                None
            };
            info!(
                "underbar: server hotkey slot {} kind {} data {} -> {:?}",
                hotkey.slot, hotkey.kind, hotkey.data, action
            );
            if let Some(slot) = slots.get_mut(hotkey.slot as usize) {
                *slot = action;
            }
        }
    }

    // stopgap until a skill window exists: known active skills onto page 1
    let mut column = 0usize;
    for skill in &info.skills {
        if column >= SLOTS_PER_PAGE as usize {
            break;
        }
        if skill.enabled != 1 {
            continue;
        }
        let Some(row) = skill_data.get(&(skill.id as i32)) else {
            continue;
        };
        if !row.is_castable() || row.icon_path().is_none() {
            continue;
        }
        let action = SlotAction::Skill { ref_id: skill.id };
        if slots.iter().flatten().any(|a| *a == action) {
            continue;
        }
        while column < SLOTS_PER_PAGE as usize && slots[column].is_some() {
            column += 1;
        }
        if let Some(slot) = slots.get_mut(column) {
            *slot = Some(action);
        }
    }

    quickslots.slots = slots;
    // the special "M" slot has no known server-side source yet
    quickslots.special = None;
    *seeded = true;
    debug!(
        "underbar: seeded {} quickslots from CHARACTER_DATA",
        quickslots.slots.iter().flatten().count()
    );
}

/// The level/exp result of one 0x3056 gain, given whether the packet flagged a
/// level-up and the media threshold for the level being left.
///
/// The level advances **only** on `leveled_up` — the server's own signal, which
/// is the presence of the packet's trailing stat-point field. It is deliberately
/// not derived from the client's leveldata curve: that only matched because the
/// reference server ships the retail curve, and it drifts on any custom-rate
/// server (#209). The trailing field's *value* is not a level anchor either — it
/// is the *unspent* stat-point wallet (assigned absolutely in
/// `character_info::model::on_experience_stat_points`, seeded from 0x3013 and
/// refreshed by 0x304E), so `points / 3 + 1` holds only until a point is spent.
///
/// The threshold is consumed best-effort so the bar restarts near zero; on a
/// custom-rate server the real requirement was lower, so it floors at 0 rather
/// than wrapping.
///
/// APPROX: one signal counts as one level. Whether a single gain crossing
/// several levels sends one 0x3056 or several is UNKNOWN — resolving it needs a
/// high-rate-server capture (docs/net-object-action-0x7074.md).
fn apply_exp_gain(
    level: u8,
    exp_offset: u64,
    leveled_up: bool,
    threshold: Option<u64>,
) -> (u8, u64) {
    if !leveled_up {
        return (level, exp_offset);
    }
    let exp_offset = match threshold {
        Some(need) => exp_offset.saturating_sub(need),
        None => exp_offset,
    };
    (level.saturating_add(1), exp_offset)
}

/// Apply 0x3056: the signed EXP/SP-exp delta — a per-kill gain, or the
/// negative EXP penalty charged on death (#306). EXP accumulates within the
/// level; **the level only changes when the packet flags a level-up** (its
/// trailing stat-point field is present) — never from client-side leveldata
/// thresholds, because private servers run custom (much lower) requirements
/// the client cannot know (#209). On a level-up the completed level's media
/// threshold is consumed best-effort (floors at 0 when the server's real
/// threshold was lower). SP-exp accumulates toward skill points (400 per
/// point — 0x304E later overwrites the authoritative total). Any
/// [`PlayerProgress`] change re-renders the underbar gauges.
pub fn on_experience_gain(
    mut reader: MessageReader<ReceiveExperience>,
    mut progress: ResMut<PlayerProgress>,
    mut vitals: ResMut<PlayerVitals>,
    level_data: Res<ClientLevelData>,
) {
    for msg in reader.read() {
        let progress = &mut *progress;
        // The delta is signed: death charges the EXP penalty as a negative
        // value, which drains the within-level offset and floors at 0 — losing
        // exp never de-levels, so only a gain walks the curve below (#306).
        // Saturating: a large/bogus server EXP value must never panic the client
        // (debug overflow-checks) — the display just clamps. See #218.
        progress.exp_offset = progress.exp_offset.saturating_add_signed(msg.experience);
        let leveled_up = msg.stat_points().is_some();
        let (level, exp_offset) = apply_exp_gain(
            progress.level,
            progress.exp_offset,
            leveled_up,
            level_data.max_exp(progress.level),
        );
        if level != progress.level {
            info!("underbar: level up! now {level}");
        }
        progress.level = level;
        progress.exp_offset = exp_offset;
        if vitals.level != progress.level {
            vitals.level = progress.level;
        }

        progress.skill_exp = progress.skill_exp.saturating_add(msg.sp_exp as u32);
        if progress.skill_exp >= SKILL_EXP_PER_SP {
            progress.skill_points = progress
                .skill_points
                .saturating_add(progress.skill_exp / SKILL_EXP_PER_SP);
            progress.skill_exp %= SKILL_EXP_PER_SP;
        }
        debug!(
            "underbar: +{} exp (offset {}), +{} sp-exp (offset {}, {} SP)",
            msg.experience,
            progress.exp_offset,
            msg.sp_exp,
            progress.skill_exp,
            progress.skill_points
        );
    }
}

/// Apply 0x304E: the SP amount (the berserk variant feeds the mini info).
pub fn on_sp_update(
    mut reader: MessageReader<CharacterPointsUpdate>,
    mut progress: ResMut<PlayerProgress>,
) {
    for msg in reader.read() {
        if let CharacterPointsUpdate::Sp { amount, .. } = msg {
            debug!("underbar: skill points now {}", amount);
            if progress.skill_points != *amount {
                progress.skill_points = *amount;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Nothing armed means nothing to age out — the ring is already hidden.
    #[test]
    fn an_unarmed_bar_has_no_fade() {
        let slots = QuickSlots::default();
        assert_eq!(slots.arm_fade(10.0, 2.5, 0.5), None);
    }

    /// The ring holds at full opacity for the whole hold, then ramps to fully
    /// faded exactly at hold + fade. Before this the arming never expired at
    /// all, leaving the last-used slot framed for the rest of the session.
    #[test]
    fn the_ring_holds_then_ramps_to_gone() {
        let mut slots = QuickSlots::default();
        slots.arm(3, 100.0);
        assert_eq!(slots.arm_fade(100.0, 2.5, 0.5), Some(0.0), "just armed");
        assert_eq!(slots.arm_fade(102.5, 2.5, 0.5), Some(0.0), "end of hold");
        assert_eq!(slots.arm_fade(102.75, 2.5, 0.5), Some(0.5), "mid fade");
        assert_eq!(slots.arm_fade(103.0, 2.5, 0.5), Some(1.0), "fade complete");
        // and it stays expired rather than wrapping past 1.0
        assert_eq!(slots.arm_fade(500.0, 2.5, 0.5), Some(1.0));
    }

    /// Re-arming restamps, so holding a rotation keeps the ring lit instead of
    /// letting it fade under the player's own keypresses.
    #[test]
    fn re_arming_restarts_the_hold() {
        let mut slots = QuickSlots::default();
        slots.arm(3, 100.0);
        let mid = slots.arm_fade(102.9, 2.5, 0.5).expect("armed");
        assert!((mid - 0.8).abs() < 1e-4, "part-faded, got {mid}");
        slots.arm(3, 102.9);
        assert_eq!(slots.arm_fade(102.9, 2.5, 0.5), Some(0.0));
    }

    /// A zero fade must vanish at the end of the hold, not divide by zero.
    #[test]
    fn a_zero_fade_snaps_off_without_producing_nan() {
        let mut slots = QuickSlots::default();
        slots.arm(0, 0.0);
        assert_eq!(slots.arm_fade(2.4, 2.5, 0.0), Some(0.0));
        assert_eq!(slots.arm_fade(2.5, 2.5, 0.0), Some(1.0));
    }

    /// Disarming clears both halves — a stamp left behind would age a slot
    /// that is no longer armed.
    #[test]
    fn disarming_clears_the_stamp_too() {
        let mut slots = QuickSlots::default();
        slots.arm(7, 100.0);
        slots.disarm();
        assert_eq!(slots.armed, None);
        assert_eq!(slots.armed_at, None);
        assert_eq!(slots.arm_fade(100.0, 2.5, 0.5), None);
    }

    /// The regression #209 names: a custom-rate server grants far more exp than
    /// the client's leveldata curve expects, and the level must NOT advance off
    /// that curve — only when the packet flags a level-up.
    #[test]
    fn exp_past_the_media_threshold_does_not_level_without_the_server_signal() {
        // 10x the media requirement, no level-up flag on the packet
        let (level, exp) = apply_exp_gain(5, 1_180, false, Some(118));
        assert_eq!(level, 5, "level must come from the server, not the curve");
        assert_eq!(exp, 1_180, "exp keeps accumulating within the level");
    }

    /// And the converse: a custom-rate server can level the character on far
    /// less exp than the curve wants. The signal still wins.
    #[test]
    fn level_advances_on_the_signal_even_below_the_media_threshold() {
        let (level, exp) = apply_exp_gain(5, 10, true, Some(118));
        assert_eq!(level, 6);
        // the threshold was higher than what was actually earned -> floor at 0,
        // never wrap around
        assert_eq!(exp, 0);
    }

    /// The retail case this used to rely on still behaves: threshold consumed,
    /// remainder carried into the new level.
    #[test]
    fn retail_curve_carries_the_remainder_into_the_new_level() {
        let (level, exp) = apply_exp_gain(1, 130, true, Some(118));
        assert_eq!(level, 2);
        assert_eq!(exp, 12);
    }

    /// Past the end of the leveldata table (max level) there is no threshold to
    /// consume, and the exp offset must survive.
    #[test]
    fn a_missing_threshold_still_levels_and_keeps_the_offset() {
        let (level, exp) = apply_exp_gain(140, 500, true, None);
        assert_eq!(level, 141);
        assert_eq!(exp, 500);
    }

    /// Bogus server values must never panic the client in debug builds (#218).
    #[test]
    fn saturates_instead_of_overflowing() {
        let (level, exp) = apply_exp_gain(u8::MAX, u64::MAX, true, Some(1));
        assert_eq!(level, u8::MAX);
        assert_eq!(exp, u64::MAX - 1);
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::assets::textdata::leveldata::LevelData;
    use bytes::Bytes;
    use std::collections::HashMap;

    /// Both captured deaths happened at level 17; leveldata.txt col 1 for 17
    /// is 161798 and for 18 is 198810.
    fn app_at_level_17(exp_offset: u64) -> App {
        let mut app = App::new();
        app.add_message::<ReceiveExperience>()
            .init_resource::<PlayerVitals>()
            .insert_resource(PlayerProgress {
                level: 17,
                exp_offset,
                ..Default::default()
            })
            .insert_resource(ClientLevelData::from_data(LevelData {
                exp: HashMap::from([(17, 161_798), (18, 198_810)]),
                ..Default::default()
            }))
            .add_systems(Update, on_experience_gain);
        app
    }

    /// The 0x3056 death penalty is a negative i64. Read as u64 it was
    /// 18446744073709548381, so `exp_offset` saturated and the level-up loop
    /// walked the whole leveldata curve — the underbar jumped to the table
    /// maximum on every death (#306).
    #[test]
    fn death_exp_penalty_drains_the_level_and_never_levels_up() {
        // real packet_dump/0x3056.log line (09:45:51.937Z): -3235, which is
        // 2% of the level-17 requirement, charged on the player's own uid.
        let mut app = app_at_level_17(89_890);
        app.world_mut().write_message(ReceiveExperience {
            exp_origin: 0x1AB80,
            experience: -3235,
            sp_exp: 0,
            unknown: 0,
            tail: Default::default(),
        });
        app.update();

        let progress = app.world().resource::<PlayerProgress>();
        assert_eq!(progress.exp_offset, 86_655);
        assert_eq!(progress.level, 17);
        assert_eq!(app.world().resource::<PlayerVitals>().level, 17);
    }

    /// Dying inside the first 2% of a level underflows the within-level
    /// offset. It floors at 0: a v1.188 character never loses a level (the
    /// textdata has job-level-down strings but no character-level-down one).
    #[test]
    fn death_exp_penalty_floors_at_zero() {
        let mut app = app_at_level_17(1_000);
        app.world_mut().write_message(ReceiveExperience {
            exp_origin: 0x1AB80,
            experience: -3235,
            sp_exp: 0,
            unknown: 0,
            tail: Default::default(),
        });
        app.update();

        let progress = app.world().resource::<PlayerProgress>();
        assert_eq!(progress.exp_offset, 0);
        assert_eq!(progress.level, 17);
    }

    /// A kill still levels the character up against the leveldata curve.
    #[test]
    fn kill_exp_still_levels_up() {
        let mut app = app_at_level_17(161_000);
        // A real level-up always carries the trailing stat-point field, and
        // since #209 that field — not the client's leveldata curve — is what
        // advances the level. 3*(18-1) = 51 unspent points at the new level.
        app.world_mut().write_message(ReceiveExperience {
            exp_origin: 0x1AB9C,
            experience: 1_000,
            sp_exp: 23,
            unknown: 0,
            tail: Bytes::copy_from_slice(&51u16.to_le_bytes()),
        });
        app.update();

        let progress = app.world().resource::<PlayerProgress>();
        assert_eq!(progress.level, 18);
        assert_eq!(progress.exp_offset, 202);
        assert_eq!(progress.skill_exp, 23);
    }
}
