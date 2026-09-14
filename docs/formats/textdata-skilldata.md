# textdata: skilldata.txt

Location: `Media.pk2/server_dep/silkroad/textdata/skilldata.txt` — a master
list naming the real shards (`skilldata_5000.txt`, `skilldata_10000.txt`, …;
the `*enc.txt` siblings are ignored). UTF-16LE, tab-separated, one row per
skill *level*; rows with ≤ 100 columns are dropped. Parser:
`client/src/assets/textdata/skilldata.rs` (`SkillDataRow` + the
`SkilldataFields` column map); the `#[ignore]`d anchor test
`skilldata_real_columns` pins the map against a real Media.pk2.

The layout matches the vSRO 1.188 `_RefSkill` table: 69 fixed columns (0–68)
followed by a variable-length fourcc parameter stream. Column semantics below
were corpus-verified 2026-08-05 across all shards (34,919 rows); ✔ = parsed
by the client.

## Fixed columns (0–68)

| col | field | notes |
|-----|-------|-------|
| 0 | Service | always 1 |
| 1 | ID | ✔ the skill id (map key; underbar/cast requests use it) |
| 2 | GroupID | ✔ shared by all levels of one skill; what prereq columns reference |
| 3 | Basic_Code | ✔ per-level codename (`SKILL_CH_SWORD_SMASH_A_01`) |
| 4 | Basic_Name | raw Korean/annotation name, display-unused |
| 5 | Basic_Group | ✔ level-less codename — join key into skilleffect.txt |
| 6 | Basic_Original | numeric group-ish id, semantics unverified |
| 7 | Basic_Level | ✔ 1-based level within the group ladder |
| 8 | Basic_Activity | ✔ 0 = passive, 1 = instant/auto self-skill, 2 = castable |
| 9 | Basic_ChainCode | ✔ combo linkage: the **skill id of the next segment** (0 = standalone / last segment). Chains run 2–7 segments; the continuations are priced at 0 MP and hidden from the board (grid 255). **A chain is ONE animation, not one per segment** — every segment carries the SAME `Basic_Group`, and skilleffect's aniset is keyed on exactly that, so all segments resolve one `AniShot` and one clip. Corpus-wide: **1873 of 1873 chains share one `Basic_Group`; none has a per-segment animation** (85/85 for player chains). The `.ban` confirms it — `skill_ch_sword_chain_b.ban` (Blood Chain, ids 9→10→11→12) is **2433 ms = 596+602+605+630**, the sum of its segments' authored durations, and its four combat-hit keytimes (266/596/1198/1803 ms) sit on the cumulative segment boundaries. Same identity on every chain checked, CH and EU. **Online the SERVER drives the segments** (captured 2026-08-29: one CastSkill → consecutive skill ids on separate 0xB070s), so the client plays the clip ONCE at segment 1 and lands segment *i*'s damage on hit event *i* (`cast::RunningSwings`); presenting each segment as its own swing restarted the clip and showed only its first ~600 ms, N times. Sequenced locally by `plugins::skills::cast::advance_chain_casts` (#205) |
| 10 | Basic_RecycleCost? | only 0 or 99999999 |
| 11 | Action_PreparingTime | ✔ ms, charge phase part 1 |
| 12 | Action_CastingTime | ✔ ms, charge phase part 2 |
| 13 | Action_ActionDuration | ✔ ms, the shot/action window (auto-attack resume timing) |
| 14 | Action_ReuseDelay | ✔ ms cooldown |
| 15 | Action_CoolTime? | 2000–3500 on monster/tower rows, 0 on player skills |
| 16 | Action_FlyingSpeed | ✔ projectile speed in world units/s (bows/crossbows 400; 0 = instant) |
| 17 | Action_Interruptable? | always 0 |
| 18 | Action_Overlap? | unknown bitmask (~200 distinct values, e.g. 0x2000000) |
| 19 | Action_AutoAttackType | ✔ 0 = none (buffs), 1 = resume auto-attack after cast, 2 = bow normal/call variant (unverified) |
| 20 | Action_InTown? | always 0 |
| 21 | Action_Range | ✔ cast range in world units; 0 on player weapon attacks = use weapon range (150–200 on monster skills) |
| 22 | Target_Required | ✔ needs a target entity |
| 23 | TargetType_Animal | 1 on most rows |
| 24 | TargetType_Land | 31 rows |
| 25 | TargetType_Building | 5 rows |
| 26 | TargetGroup_Self | ✔ applies to the caster |
| 27 | TargetGroup_Ally | ✔ heals/res/buffs castable on others (571 rows) |
| 28 | TargetGroup_Party | ✔ party-wide variants (543 rows) |
| 29 | TargetGroup_Enemy_M | ✔ hostile monsters |
| 30 | TargetGroup_Enemy_P | ✔ hostile players |
| 31 | TargetGroup_Neutral | 5 rows |
| 32 | TargetGroup_DontCare | 3 rows |
| 33 | TargetEtc_SelectDeadBody | ✔ resurrection targets a corpse (68 rows) |
| 34 | ReqCommon_Mastery1 | ✔ mastery id required to learn |
| 35 | ReqCommon_Mastery2 | always 0 in 1.188 |
| 36 | ReqCommon_MasteryLevel1 | ✔ |
| 37 | ReqCommon_MasteryLevel2 | always 0 |
| 38 | ReqCommon_Str | always 0 |
| 39 | ReqCommon_Int | always 0 |
| 40–42 | ReqLearn_Skill1–3 | ✔ prerequisite group ids (slot 2 used by ~900 rows) |
| 43–45 | ReqLearn_SkillLevel1–3 | ✔ paired levels |
| 46 | ReqLearn_SP | ✔ SP cost to learn |
| 47 | ReqLearn_Race | 0 / 1 / 3 (race gate; mastery already implies it) |
| 48 | Req_Restriction1 | always 0 |
| 49 | Req_Restriction2 | always 0 |
| 50 | ReqCast_Weapon1 | ✔ weapon class (item tid4); 255 = unrestricted |
| 51 | ReqCast_Weapon2 | ✔ |
| 52 | Consume_HP | ✔ HP per cast (~11 rows) |
| 53 | Consume_MP | ✔ MP per cast — 0 on passives and chain CONTINUATION steps (the priced entry step carries the cost). Historically misread as a "UI order" column |
| 54 | Consume_HPRatio | 10 / 95 on a handful of rows |
| 55 | Consume_MPRatio | always 0 |
| 56 | Consume_WHAN (hwan) | always 0 |
| 57 | UI_SkillTab | ✔ native skill-window grid; 255 = hidden (monster/NPC and chain continuation rows) |
| 58 | UI_SkillPage | ✔ board page — tab/page address the mastery's board inside the window |
| 59 | UI_SkillColumn | ✔ board LANE (one horizontal bar per value: Bicheon smash = 0, chain = 1, shield = 2, …) |
| 60 | UI_SkillRow | ✔ socket within the lane (smash A–F = rows 0–5; gaps are authored, e.g. sword SPECIAL starts at 2) |
| 61 | UI_IconFile | ✔ `skill\china\….ddj` under `media://icon/` |
| 62 | UI_SkillNameString | ✔ `SN_*` display-name key |
| 63 | UI_SkillToolTip | always `xxx` |
| 64 | UI_SkillToolTipDesc | ✔ `SN_*_TT_DESC` tooltip key |
| 65 | UI_SkillStudy_Desc | ✔ `SN_*_STUDY` learn-tooltip key (`xxx` on most non-player rows) |
| 66 | AI_AttackChance | server AI weighting (0–100) |
| 67 | AI_SkillType? | small set of values (80/78/77/72/66…), server AI |
| 68 | ? | small enum 0/1/3/4, semantics unknown |

## Parameter stream (col 69+)

A linear list of fourcc tags (stored as big-endian integers, e.g.
`6386804` = `'att'`), each followed by a fixed argument count. The client's
`stat_lines()` walks it left to right and **stops at the first unknown tag**
— argument counts beyond the verified set are guesses and a misaligned walk
would read arguments as tags. Verified tags (name → argc): `att` 5, `mc` 2,
`dura` 1, `cr` 1, `ru` 1, `heal` 1, `defp` 1, `hr` 2, `er` 2, `summ` 5,
`st` 3, `cnsm` 3, `efr` 6, `getv` 1, `MAAT` 0, `reqi` 2. See
`docs/formats/textdata-skilleffect.md` (Known gaps) for the top truncating
unknown tags.
