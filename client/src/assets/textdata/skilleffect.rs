//! Parser for `server_dep/silkroad/textdata/skilleffect.txt` (Media.pk2).
//!
//! Unlike the row tables handled by [`super::TextdataLoader`], the file is
//! split into `#section` blocks. Two of them carry the skill visuals:
//!
//! * `skillaniset2` — one row per skill: which animation group and which
//!   symbolic animation slots (`ANI_SKILL_1`, `ANI_READY01`, ...) the skill
//!   uses.
//! * `skilleffectset` — N rows per skill: the actual `.efp` emissions with
//!   their phase (SHOT/ACT_S/ACT_L/...), attach bone, offset and the hit
//!   event index that times them.
//!
//! The file is UTF-16LE with BOM and tab-separated like the other textdata
//! files; `none` marks empty fields and `//` starts a comment row.

use bevy::math::Vec3;
use bevy::prelude::Resource;
use std::collections::HashMap;

/// One `skillaniset2` row: the animation binding of a skill.
#[derive(Debug, Clone)]
pub struct SkillAniSet {
    /// Skill codename, e.g. `SKILL_CH_SWORD_SMASH_A` (join key).
    pub codename: String,
    /// Animation group name (`SWORD`, `DEFAULT`, ...); matches the .bsr
    /// animation group case-insensitively.
    pub ani_group: String,
    /// Symbolic slots (`ANI_SKILL_1`, `none`, ...), see [`slot_to_anim_type`].
    pub ani_ready: String,
    pub ani_wait: String,
    pub ani_shot: String,
    /// Damage/impact effect (aniset col 14, "DamageEfp") — the hit burst at
    /// the target. Melee also carries an effectset `AT_DMG_POS` impact; bow
    /// skills only have this one, so it is played at the target when the
    /// effectset provides no target impact. `particles://` path.
    pub damage_efp: Option<String>,
    /// Arrow trail effect (aniset col 20, "화살 꼬리") — rides the flying
    /// arrow projectile; only bow skills populate it. `particles://` path.
    pub arrow_tail: Option<String>,
    /// Arrow force/aura effect (aniset col 21, "화살 포스") — rides the
    /// flying arrow projectile; bow skills only. `particles://` path.
    pub arrow_force: Option<String>,
}

/// AniType phase of a `skilleffectset` row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectPhase {
    /// Fires with the shot animation.
    Shot,
    /// One-shot burst when the skill activates.
    ActS,
    /// Loops while the skill/state is active.
    ActL,
    /// Fires with the ready animation.
    Ready,
    /// Fires with the wait animation.
    Wait,
    /// DEACT / S_RETURN / ACT_OS / ACT_OL — parsed but not played.
    Other,
}

/// One `skilleffectset` row: a single `.efp` emission.
#[derive(Debug, Clone)]
pub struct SkillEmission {
    pub phase: EffectPhase,
    /// 0 = at animation start, N = at the Nth typ-1 animation event keytime.
    pub start_event: u32,
    /// `AT_ONE_FOLLOW`, `AT_LOOP`, `AT_MOV_1TAR`, ...
    pub act_type: String,
    /// Path inside Particles.pk2, forward slashes, e.g. `system/system_hwan_keep.efp`.
    pub efp_path: String,
    /// Attach bone (`Bip01 Spine`); `None` = character root.
    pub start_bone: Option<String>,
    /// Offset relative to the attach bone, in SRO resource units.
    pub start_offset: Vec3,
    /// Second effect object (`ObjName2`, col 23) — the impact `.efp` played at
    /// the target: a projectile's (`AT_MOV_*`) explosion on arrival, or an
    /// `AT_TARGET` row's secondary burst. `None` = only the primary object.
    /// `particles://` path.
    pub impact_efp: Option<String>,
    /// Offset for the impact object relative to the target (`TargetOffset`,
    /// col 22), in SRO resource units.
    pub target_offset: Vec3,
    /// `MOV_UPR` rows (col 14 `MovTypeSpeed`) fly a parabolic arc instead of
    /// a straight line (Berserker Arrow, the base bow shot, catapults);
    /// the value is col 15 `Param`'s first number = the arc's peak height in
    /// SRO units (player bows 40-60, catapults 200+; a few rows author 0 —
    /// the flight code substitutes a distance fraction). `None` = straight.
    pub arc: Option<f32>,
    /// Whether col 14 authored ANY projectile motion (`MOV_STRAIGHT`,
    /// `MOV_UPR`, …, as opposed to `MOV_NONE`/empty) — `AT_TARGET` rows can
    /// carry one too (Snow Storm's icicles fall from StartOffset to
    /// TargetOffset as staggered mini-projectiles).
    pub mov: bool,
    /// `MovTypeSpeed` arg 1: per-row emission stagger in ms (Snow Storm
    /// spreads 20 icicles over 0..900 ms).
    pub mov_delay_ms: u32,
    /// `MovTypeSpeed` arg 2: authored projectile speed in units/s (0 when
    /// unauthored).
    pub mov_speed: f32,
}

/// A skill with its animation binding and all its emissions.
#[derive(Debug, Clone)]
pub struct SkillEntry {
    pub aniset: SkillAniSet,
    pub emissions: Vec<SkillEmission>,
}

/// Parsed skilleffect.txt. `skills` preserves file order for UI listings.
#[derive(Resource, Debug, Default, Clone)]
pub struct SkillEffectTable {
    pub skills: Vec<SkillEntry>,
    pub by_codename: HashMap<String, usize>,
}

/// Decodes a UTF-16LE buffer with byte order mark, like the textdata loader.
pub fn decode_utf16le(buf: &[u8]) -> String {
    let units = buf
        .chunks_exact(2)
        .skip(1)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect::<Vec<_>>();
    String::from_utf16_lossy(&units)
}

/// Maps a symbolic animation slot to the .bsr animation type id.
///
/// Verified against chinaman_adventurer.bsr / tahomet.bsr group tables
/// cross-referenced with skill codenames ↔ .ban stems; the type-id blocks
/// are non-contiguous, so the ATTACK series is a table rather than a formula
/// (see the `ANI_ATTACK` arm).
pub fn slot_to_anim_type(slot: &str) -> Option<u32> {
    let slot = slot.trim();
    if slot.is_empty() || slot.eq_ignore_ascii_case("none") {
        return None;
    }
    let digits = |prefix: &str| -> Option<u32> {
        slot.strip_prefix(prefix)
            .and_then(|rest| rest.parse::<u32>().ok())
    };
    if slot == "ANI_SHOT" {
        return Some(191);
    }
    if let Some(n) = digits("ANI_ATTACK") {
        // The ATTACK ids are not a run: 187-189 are HAMMER/HANDLOOF/TROW and
        // 191 is SHOT, so the series jumps twice. Extrapolating `178 + n` past
        // ATTACK8 therefore landed ATTACK9 on HAMMER and shifted ATTACK10+ by
        // two. `ResourceAnimationType` (JMX-File-Editor `PrimAnimationType.cs`
        // :29-43; ATTACK16 only in SilkroadDoc-wiki `ResourceAnimationType.md`
        // :42) — every id below is also corpus-verified against the .bsr group
        // tables, where ATTACK8..16 match their `*_attackNN` clip at 100%.
        return match n {
            1 => Some(2),
            2 => Some(5),
            3 => Some(16),
            4 => Some(17),
            5 => Some(183),
            6 => Some(184),
            7 => Some(185),
            8 => Some(186),
            9 => Some(190),
            10 => Some(192),
            11 => Some(193),
            12 => Some(194),
            13 => Some(195),
            14 => Some(196),
            15 => Some(197),
            16 => Some(198),
            // ATTACK17+ has no id in either enum.
            _ => None,
        };
    }
    if let Some(n) = digits("ANI_SKILL_") {
        return match n {
            1..=10 => Some(25 + n),
            11..=20 => Some(57 + n),
            21..=40 => Some(80 + n),
            n @ 41.. => Some(82 + n),
            _ => None,
        };
    }
    if let Some(x) = digits("ANI_READY") {
        return Some(39 + x);
    }
    if let Some(x) = digits("ANI_WAIT") {
        return Some(90 + x);
    }
    None
}

fn field<'a>(row: &'a [&'a str], idx: usize) -> &'a str {
    row.get(idx).map(|s| s.trim()).unwrap_or("")
}

fn is_none(value: &str) -> bool {
    value.is_empty() || value.eq_ignore_ascii_case("none")
}

fn parse_vec3(value: &str) -> Vec3 {
    let mut parts = value.split(',').map(|p| p.trim().parse::<f32>());
    match (parts.next(), parts.next(), parts.next()) {
        (Some(Ok(x)), Some(Ok(y)), Some(Ok(z))) => Vec3::new(x, y, z),
        _ => Vec3::ZERO,
    }
}

fn parse_phase(value: &str) -> EffectPhase {
    match value {
        "SHOT" => EffectPhase::Shot,
        "ACT_S" => EffectPhase::ActS,
        "ACT_L" => EffectPhase::ActL,
        "READY" => EffectPhase::Ready,
        "WAIT" => EffectPhase::Wait,
        _ => EffectPhase::Other,
    }
}

#[derive(PartialEq)]
enum Section {
    Other,
    AniSet,
    EffectSet,
}

/// Parses the decoded file content into a [`SkillEffectTable`].
pub fn parse_skilleffect(content: &str) -> SkillEffectTable {
    let mut table = SkillEffectTable::default();
    let mut section = Section::Other;
    let mut orphaned = 0usize;
    for line in content.lines() {
        let line = line.trim_end_matches(['\r', '\t']);
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        if let Some(name) = line.strip_prefix("#section") {
            section = match name.trim() {
                "skillaniset2" => Section::AniSet,
                "skilleffectset" => Section::EffectSet,
                _ => Section::Other,
            };
            continue;
        }
        let row = line.split('\t').collect::<Vec<_>>();
        match section {
            Section::AniSet => {
                if field(&row, 0) != "1" {
                    continue;
                }
                let codename = field(&row, 2);
                if codename.is_empty() {
                    continue;
                }
                if table.by_codename.contains_key(codename) {
                    bevy::log::debug!("skilleffect: duplicate aniset row for {codename}");
                    continue;
                }
                table
                    .by_codename
                    .insert(codename.to_string(), table.skills.len());
                let efp_field = |idx: usize| -> Option<String> {
                    let value = field(&row, idx);
                    (!is_none(value)).then(|| value.replace('\\', "/"))
                };
                table.skills.push(SkillEntry {
                    aniset: SkillAniSet {
                        codename: codename.to_string(),
                        ani_group: field(&row, 6).to_string(),
                        ani_ready: field(&row, 7).to_string(),
                        ani_wait: field(&row, 8).to_string(),
                        ani_shot: field(&row, 9).to_string(),
                        damage_efp: efp_field(14),
                        arrow_tail: efp_field(20),
                        arrow_force: efp_field(21),
                    },
                    emissions: Vec::new(),
                });
            }
            Section::EffectSet => {
                let codename = field(&row, 1);
                let Some(&idx) = table.by_codename.get(codename) else {
                    orphaned += 1;
                    continue;
                };
                let obj_name = field(&row, 18);
                if is_none(obj_name) {
                    continue;
                }
                let obj_path = field(&row, 17);
                let separator = if obj_path.is_empty() || obj_path.ends_with('\\') {
                    ""
                } else {
                    "\\"
                };
                let start_bone = field(&row, 19);
                let obj_name2 = field(&row, 23);
                // corpus: 479 MOV_STRAIGHT / 116 MOV_UPR / 1 "MOV_MOV_UPR"
                // typo among AT_MOV rows, hence contains() not equality
                let mov_parts: Vec<&str> = field(&row, 14).split(',').collect();
                let mov_type = mov_parts.first().map(|ty| ty.trim()).unwrap_or("");
                let mov = mov_type.starts_with("MOV_") && mov_type != "MOV_NONE";
                let mov_num = |idx: usize| {
                    mov_parts
                        .get(idx)
                        .and_then(|v| v.trim().parse::<f32>().ok())
                        .unwrap_or(0.0)
                };
                let arced = mov_type.contains("UPR");
                let arc = arced.then(|| {
                    field(&row, 15)
                        .split(',')
                        .next()
                        .and_then(|h| h.trim().parse::<f32>().ok())
                        .unwrap_or(0.0)
                });
                table.skills[idx].emissions.push(SkillEmission {
                    phase: parse_phase(field(&row, 2)),
                    start_event: field(&row, 3).parse().unwrap_or(0),
                    act_type: field(&row, 13).to_string(),
                    efp_path: format!("{obj_path}{separator}{obj_name}").replace('\\', "/"),
                    start_bone: (!is_none(start_bone)).then(|| start_bone.to_string()),
                    start_offset: parse_vec3(field(&row, 20)),
                    impact_efp: (!is_none(obj_name2) && obj_name2.ends_with(".efp"))
                        .then(|| obj_name2.replace('\\', "/")),
                    target_offset: parse_vec3(field(&row, 22)),
                    arc,
                    mov,
                    mov_delay_ms: mov_num(1) as u32,
                    mov_speed: mov_num(2),
                });
            }
            Section::Other => {}
        }
    }
    if orphaned > 0 {
        bevy::log::debug!("skilleffect: {orphaned} effectset rows without aniset row skipped");
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_mapping() {
        assert_eq!(slot_to_anim_type("ANI_ATTACK1"), Some(2));
        assert_eq!(slot_to_anim_type("ANI_ATTACK2"), Some(5));
        assert_eq!(slot_to_anim_type("ANI_ATTACK3"), Some(16));
        assert_eq!(slot_to_anim_type("ANI_ATTACK4"), Some(17));
        assert_eq!(slot_to_anim_type("ANI_ATTACK5"), Some(183));
        assert_eq!(slot_to_anim_type("ANI_ATTACK6"), Some(184));
        assert_eq!(slot_to_anim_type("ANI_ATTACK7"), Some(185));
        assert_eq!(slot_to_anim_type("ANI_ATTACK8"), Some(186));
        // The series jumps over HAMMER/HANDLOOF/TROW (187-189) here, then over
        // SHOT (191) — the `178 + n` extrapolation put ATTACK9 on HAMMER and
        // shifted ATTACK10+ by two.
        assert_eq!(slot_to_anim_type("ANI_ATTACK9"), Some(190));
        assert_eq!(slot_to_anim_type("ANI_ATTACK10"), Some(192));
        assert_eq!(slot_to_anim_type("ANI_ATTACK11"), Some(193));
        assert_eq!(slot_to_anim_type("ANI_ATTACK12"), Some(194));
        assert_eq!(slot_to_anim_type("ANI_ATTACK13"), Some(195));
        assert_eq!(slot_to_anim_type("ANI_ATTACK14"), Some(196));
        assert_eq!(slot_to_anim_type("ANI_ATTACK15"), Some(197));
        assert_eq!(slot_to_anim_type("ANI_ATTACK16"), Some(198));
        // ATTACK17+ has no id in either enum — it must not extrapolate into
        // the READY/WAIT block that follows at 199+.
        assert_eq!(slot_to_anim_type("ANI_ATTACK17"), None);
        assert_eq!(slot_to_anim_type("ANI_SKILL_1"), Some(26));
        assert_eq!(slot_to_anim_type("ANI_SKILL_10"), Some(35));
        assert_eq!(slot_to_anim_type("ANI_SKILL_11"), Some(68));
        assert_eq!(slot_to_anim_type("ANI_SKILL_20"), Some(77));
        assert_eq!(slot_to_anim_type("ANI_SKILL_21"), Some(101));
        assert_eq!(slot_to_anim_type("ANI_SKILL_40"), Some(120));
        assert_eq!(slot_to_anim_type("ANI_SKILL_41"), Some(123));
        assert_eq!(slot_to_anim_type("ANI_READY01"), Some(40));
        assert_eq!(slot_to_anim_type("ANI_READY04"), Some(43));
        assert_eq!(slot_to_anim_type("ANI_WAIT02"), Some(92));
        assert_eq!(slot_to_anim_type("ANI_SHOT"), Some(191));
        assert_eq!(slot_to_anim_type("none"), None);
        assert_eq!(slot_to_anim_type(""), None);
        assert_eq!(slot_to_anim_type("ANI_BOGUS"), None);
    }

    fn row(fields: &[(usize, &str)], len: usize) -> String {
        let mut cols = vec![""; len];
        for &(idx, value) in fields {
            cols[idx] = value;
        }
        cols.join("\t")
    }

    #[test]
    fn parse_sample() {
        let aniset = row(
            &[
                (0, "1"),
                (1, "test skill"),
                (2, "SKILL_TEST_A"),
                (6, "SWORD"),
                (7, "none"),
                (8, "none"),
                (9, "ANI_SKILL_1"),
                (14, "hiteffect\\hit_3_bow.efp"),
                (20, "skill\\china\\mirage_bow_critical.efp"),
                (21, "none"),
            ],
            27,
        );
        let shot = row(
            &[
                (0, "test skill"),
                (1, "SKILL_TEST_A"),
                (2, "SHOT"),
                (3, "1"),
                (13, "AT_ONE_FOLLOW"),
                (17, "hiteffect\\"),
                (18, "hit_1_cut_smash.efp"),
                (19, "Bip01 Spine"),
                (20, "0,10,-13"),
                (22, "0,10,0"),
                (23, "skill\\china\\bow_area_bomb_a.efp"),
            ],
            28,
        );
        let loop_row = row(
            &[
                (0, "-"),
                (1, "SKILL_TEST_A"),
                (2, "ACT_L"),
                (3, "0"),
                (13, "AT_LOOP"),
                (17, "system\\"),
                (18, "system_hwan_keep.efp"),
                (19, "none"),
                (20, "none"),
            ],
            28,
        );
        let orphan = row(&[(1, "SKILL_UNKNOWN"), (18, "x.efp")], 28);
        let disabled = row(&[(0, "0"), (2, "SKILL_DISABLED")], 27);
        let content = format!(
            "#section\tcharacterInfo\nignored row\n\
             #section\tskillaniset2\n//\tSkillName\tSkillID\n{aniset}\n{disabled}\n\
             #section\tskilleffectset\n{shot}\n{loop_row}\n{orphan}\n"
        );

        let table = parse_skilleffect(&content);
        assert_eq!(table.skills.len(), 1);
        let entry = &table.skills[table.by_codename["SKILL_TEST_A"]];
        assert_eq!(entry.aniset.ani_group, "SWORD");
        assert_eq!(entry.aniset.ani_shot, "ANI_SKILL_1");
        assert_eq!(
            entry.aniset.damage_efp.as_deref(),
            Some("hiteffect/hit_3_bow.efp")
        );
        assert_eq!(
            entry.aniset.arrow_tail.as_deref(),
            Some("skill/china/mirage_bow_critical.efp")
        );
        assert_eq!(entry.aniset.arrow_force, None);
        assert_eq!(entry.emissions.len(), 2);

        let shot = &entry.emissions[0];
        assert_eq!(shot.phase, EffectPhase::Shot);
        assert_eq!(shot.start_event, 1);
        assert_eq!(shot.efp_path, "hiteffect/hit_1_cut_smash.efp");
        assert_eq!(shot.start_bone.as_deref(), Some("Bip01 Spine"));
        assert_eq!(shot.start_offset, Vec3::new(0.0, 10.0, -13.0));
        assert_eq!(
            shot.impact_efp.as_deref(),
            Some("skill/china/bow_area_bomb_a.efp")
        );
        assert_eq!(shot.target_offset, Vec3::new(0.0, 10.0, 0.0));

        let keep = &entry.emissions[1];
        assert_eq!(keep.phase, EffectPhase::ActL);
        assert_eq!(keep.act_type, "AT_LOOP");
        assert_eq!(keep.efp_path, "system/system_hwan_keep.efp");
        assert_eq!(keep.start_bone, None);
        assert_eq!(keep.start_offset, Vec3::ZERO);
        assert_eq!(keep.impact_efp, None);
    }

    #[test]
    fn decode_utf16le_bom() {
        let text = "#section\tskillaniset2\nrow";
        let mut buf = vec![0xFF, 0xFE];
        for unit in text.encode_utf16() {
            buf.extend_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(decode_utf16le(&buf), text);
    }

    /// End-to-end check of the skill table load path against the real
    /// archive: read from Media.pk2, decode, parse, and verify anchor facts.
    #[test]
    #[ignore = "needs Media.pk2; run with: cargo test -p client parses_real_skilleffect -- --ignored"]
    fn parses_real_skilleffect_table() {
        use bevy_pk2::prelude::Archive;
        use std::path::{Path, PathBuf};

        let assets = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("assets");
        let archive = Archive::configured(assets.join("Media.pk2"));
        let bytes = archive
            .read_file_bytes(Path::new("server_dep/silkroad/textdata/skilleffect.txt"))
            .expect("skilleffect.txt in Media.pk2");
        let table = parse_skilleffect(&decode_utf16le(&bytes));
        assert!(
            table.skills.len() > 1000,
            "only {} skills",
            table.skills.len()
        );

        // sword smash A: SWORD group, shot = ANI_SKILL_1 = type 26,
        // hit effect fired at hit event 1
        let smash = &table.skills[table.by_codename["SKILL_CH_SWORD_SMASH_A"]];
        assert_eq!(smash.aniset.ani_group, "SWORD");
        assert_eq!(slot_to_anim_type(&smash.aniset.ani_shot), Some(26));
        let cut = smash
            .emissions
            .iter()
            .find(|em| em.efp_path == "hiteffect/hit_1_cut_smash.efp")
            .expect("smash cut emission");
        assert_eq!(cut.phase, EffectPhase::Shot);
        assert_eq!(cut.start_event, 1);

        // bow critical: the arrow trail + force glow live in the aniset row
        // (cols 20/21), not the effectset — they ride the flying arrow.
        let bow = &table.skills[table.by_codename["SKILL_CH_BOW_CRITICAL_C"]];
        assert_eq!(bow.aniset.ani_group, "BOW");
        assert_eq!(
            bow.aniset.damage_efp.as_deref(),
            Some("hiteffect/hit_3_critical.efp")
        );
        assert_eq!(
            bow.aniset.arrow_tail.as_deref(),
            Some("skill/china/mirage_bow_critical.efp")
        );
        assert_eq!(
            bow.aniset.arrow_force.as_deref(),
            Some("skill/china/force_bow_critical_b.efp")
        );

        // bow Area: the flying arrow (ObjName) carries an ObjName2 impact bomb
        // that explodes at the target — the effectset's second object.
        let area = &table.skills[table.by_codename["SKILL_CH_BOW_AREA_A"]];
        let bomb = area
            .emissions
            .iter()
            .find(|em| em.act_type.starts_with("AT_MOV"))
            .expect("bow area projectile");
        assert_eq!(
            bomb.impact_efp.as_deref(),
            Some("skill/china/bow_area_bomb_a.efp")
        );

        // hwan mode: ACT_S burst on Bip01 plus ACT_L keep loops on the spine
        let hwan = &table.skills[table.by_codename["SYSTEM_CH_HWANMODE"]];
        assert!(hwan.emissions.iter().any(|em| {
            em.phase == EffectPhase::ActS && em.start_bone.as_deref() == Some("Bip01")
        }));
        assert!(hwan.emissions.iter().any(|em| {
            em.phase == EffectPhase::ActL
                && em.act_type == "AT_LOOP"
                && em.start_bone.as_deref() == Some("Bip01 Spine")
                && em.efp_path == "system/system_hwan_keep.efp"
        }));
    }
}
