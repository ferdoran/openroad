# textdata: skilleffect.txt (skillaniset2 + skilleffectset)

Location: `Media.pk2/server_dep/silkroad/textdata/skilleffect.txt`. UTF-16LE,
tab-separated, split into `#section` blocks; the two the client consumes are
`skillaniset2` (one row per skill: animations + the aniset effect slots) and
`skilleffectset` (zero or more emission rows per skill, joined by codename).
Parser: `parse_skilleffect` in `client/src/assets/textdata/skilleffect.rs`;
consumed by the cast pipeline (`client/src/plugins/skills/cast.rs`).

Headers recovered from real v1.188 data (0-based indices; ✔ = parsed).

## #section skillaniset2 (27 cols)

| col | field | notes |
|-----|-------|-------|
| 0 | Service | ✔ 1 = active row |
| 1 | (korean name) | e.g. `005__항마궁술 탄` |
| 2 | SkillID | ✔ codename join key = skilldata `basic_group`, e.g. `SKILL_CH_BOW_CRITICAL_A` |
| 3 | Priority | |
| 4 | AniTRUE | |
| 5 | Hide Weapon | |
| 6 | AniGroup | ✔ lowercased to match `.bsr` animation group names (`BOW`, …) |
| 7 | AniReady | ✔ charge/wind-up clip slot (`ANI_READY01`) |
| 8 | AniWait | ✔ |
| 9 | AniShot | ✔ shot clip slot (`ANI_SKILL_40`, mapped by `slot_to_anim_type`) |
| 10–12 | Ani play Timing, Act W, Act S | |
| 13 | DefenseEfp | played on the defender (unparsed) |
| 14 | DamageEfp | ✔ on-hit `.efp` at the target (`hiteffect\hit_3_critical.efp`) |
| 15 | Length | |
| 16 | Color | `0,0,0,0` |
| 17 | Op. | blend op (`ONE`) |
| 18 | Texture | |
| 19 | start timing | |
| 20 | 화살 꼬리 (arrow tail) | ✔ trail `.efp` riding the projectile (`skill\china\mirage_bow_critical.efp`) |
| 21 | 화살 포스 (arrow force) | ✔ force aura `.efp`: draw phase + riding the projectile |
| 22 | Light Effect | `LIGHT_4` (unparsed) |
| 23 | Skill Object | |
| 24 | 허리 돌리기 (waist turn) | `none`/`Roll`/`Yaw`/`Pitch` — torso aim mode (unparsed) |
| 25 | 공격스킬여부 (is attack) | |
| 26 | 피 나올래? (show blood) | |

## #section skilleffectset (28 cols)

| col | field | notes |
|-----|-------|-------|
| 0 | SkillName | korean display name |
| 1 | SkillEffectID | ✔ join key → aniset SkillID |
| 2 | AniType | ✔ phase: `READY` / `SHOT` (and numbered `SHOT` variants) |
| 3 | StartEvent | ✔ which combat-hit keytime fires the row |
| 4–6 | DMG Event, DamageType, Scale | |
| 7–12 | ID, Attach, Trade, Kill, CreateCnt, Fade | |
| 13 | ActType | ✔ `AT_LOOP`, `AT_MOV_1TAR`/`AT_MOV_*` (projectile), `AT_TARGET`, `AT_TARGET_F`, `AT_DMG_POS`, … |
| 14 | MovTypeSpeed | ✔(type) `MOV_<type>,<delay?>,<speed_start>,<speed_end>` — projectile motion. AT_MOV census: 479 `MOV_STRAIGHT`, 116 `MOV_UPR` (parabolic arc: Berserker Arrow `SKILL_CH_BOW_AREA_A`, the base bow shot, Arrow Combo, catapults; plus one `MOV_MOV_UPR` typo row), 6 `MOV_PIERCE`, 4 `MOV_ROUND`. Speeds unparsed; client hardcodes `PROJECTILE_FLIGHT_SECS` |
| 15 | Param | ✔(arc) first number = the `MOV_UPR` arc size (player bows 40–60, catapults/bosses 200–500; straight rows are all `0,0,0`, a few UPR rows author 0 → client derives from distance). Playtest-calibrated reading: an initial vertical **speed** (units/s), apex = `Param·T/4` over flight time T (≈5 units for bows at T = 0.35 s) — direct height and height/4 both read far too steep; exact exe semantics unverified |
| 16 | Act Option | |
| 17 | Obj Path | ✔ directory of the emitted object |
| 18 | ObjName | ✔ `.efp` particle or `.bsr` model (bow arrow: `cha_arrow_normal.bsr`) |
| 19 | StartBone | ✔ caster bone anchor (`Bip01 R Hand`) |
| 20 | StartOffset | ✔ `x,y,z` in SRO units |
| 21 | TargetBone | (unparsed) |
| 22 | TargetOffset | ✔ |
| 23 | ObjName2 | ✔ impact `.efp` at the target/arrival |
| 24 | Rotate | degrees; 90 on the nocked READY arrow, 0 on the flying one (unparsed) |
| 25 | Script | `SCT_ARROW` on the nocked arrow (unparsed) |
| 26–27 | SndBegin, SndEnd | |

## Orientation conventions (why the parsed columns are enough)

Directional `.efp`s are authored along their own local **+Z** while `.bsr`
models face **−Z** like character bodies — see the authoring-convention note
in `efp-jmxveff.md` and the `shot_rotation`/`model_shot_rotation` helpers in
`client/src/plugins/skills/cast.rs`. Effect rows carry no per-row aim data;
the shot frame comes entirely from caster→target geometry. Col 24 `Rotate`
is a static extra rotation (its only prominent use is the 90° nocked arrow)
and col 14 `MovTypeSpeed` the authored projectile speed; both are candidates
for future parsing.

Verification sample (`SKILL_CH_BOW_CRITICAL_A`, "Anti Devil Bow - Missile"):
aniset row carries `DamageEfp = hiteffect\hit_3_critical.efp`, arrow tail
`skill\china\mirage_bow_critical.efp`, arrow force
`skill\china\force_bow_critical_a.efp`; its two effectset rows are the READY
nocked arrow (`Rotate=90`, `SCT_ARROW`) and the SHOT flying arrow
(`AT_MOV_1TAR`, `MOV_STRAIGHT,0,500,500`), both `cha_arrow_normal.bsr` on
`Bip01 R Hand`. Pinned by the `--ignored` test `parses_real_skilleffect` in
`client/src/assets/textdata/skilleffect.rs`.

## AT_TARGET rows can carry motion too (Snow Storm's icicle rain)

`SKILL_CH_COLD_GIGONGSUL_B/_D` author 20 `AT_TARGET` rows each with
`MOV_STRAIGHT,<stagger_ms>,200,300` and `StartOffset (x,100,z)` →
`TargetOffset (x,0,z)`: each icicle is a mini-projectile falling straight
down from 100 units above the target, spawn-staggered 0..900 ms in 100 ms
steps. The icicle `.efp` itself contains **no** rotation/velocity/gravity
commands — the travel frame comes entirely from the offset pair (+Z = travel
axis like every directional `.efp`; the mesh is elongated along local Z, the
trail sits behind on −Z). The client flies such rows as `SkillProjectile`s
with `pitched_shot_rotation` (`cast.rs`).

## Known gaps (audited 2026-08-05)

- **Chain sequencing** (skilldata ChainCode, col 9): **a chain is ONE animation.**
  All of a chain's segments carry the same skilldata `Basic_Group`, and this
  section is keyed on exactly that — so a combo has exactly one `skillaniset2`
  row, one `AniShot`, one clip. Corpus-wide: **1873/1873 chains share one
  group; none has a per-segment animation.** The `.ban` matches to the
  millisecond (`skill_ch_sword_chain_b.ban` = 2433 ms = the sum of Blood
  Chain's four segment durations; its hit keytimes sit on the segment
  boundaries). Online the server announces each segment as its own 0xB070, so
  the client plays the clip once at segment 1 and lands segment *i*'s damage on
  the clip's *i*-th combat-hit keytime (`cast::RunningSwings`) — the
  continuations are damage, not animation. The offline simulator still
  sequences the chain itself (`skills::cast::advance_chain_casts`, #205).
  UNKNOWN: how to pair segments to keytimes when a clip has MORE hit events
  than segments (Billow Chain: 6 events / 5 segments) — segment *i* → event *i*
  is the current mapping, and the segment boundaries are a subset of the
  keytimes, but nothing labels them.
- **Authored projectile speeds** (`MovTypeSpeed` args 2/3) are now honoured on
  **every** path (`cast::projectile_speed`), not just the AT_TARGET one.
  Previously caster→target flights used skilldata col 16 `Action_FlyingSpeed`,
  which is **not a speed**: censused over the user's Media.pk2, every skill
  carrying an `AT_MOV_*` emission has col 16 equal to either **0 (71 skills)
  or 400 (58 skills)** — a has-a-projectile flag. The effectset carries the
  real spread (100/150/200/250/300/350/400/420/450/500/600 plus ramps), and of
  the 606 `AT_MOV_*` rows, 183 join a player skilldata row and only **9
  agree**. So everything flew at a uniform 400: Soul Cut Blade's blade force
  (authored 300 over its 120-unit `Action_Range`) arrived in 0.30 s instead of
  0.40, a bow arrow (authored 500) in 0.375 s instead of 0.30. Col 16 is kept
  as the fallback when a row authors no `MovTypeSpeed`.
  Still simplified: a **ramped** row (`250,300`; `230,0`) keeps only its start
  speed, and the `MovTypeSpeed` delay arg is unused outside the AT_TARGET
  stagger. Flight time is still not added to the damage schedule.
- Effectset col 24 `Rotate` (static extra rotation) and col 13 `DefenseEfp`
  (played on the defender) are parsed-but-unused / unparsed.
- The tooltip stat walker (`skilldata.rs::stat_lines`) still stops at ~35
  unverified tags; top truncators by CH row count: `fz` 210, `bl` 133,
  `kb` 126, `bu` 123, `ko` 87, `sl` 86, `da` 85, `br` 72, `onff` 65, `es` 61.
  Of these, **`kb` and `ko` are no longer unknown**: they are the block and
  crit chance params (`kb` argc 2, `ko` argc 2; sibling `ck` = parry, argc 1),
  resolved from the server-side skill-effect applier — see
. They still truncate the walk
  only because `stat_lines` has not been extended with them. The rest
  (`fz`/`bl`/`bu`/`sl`/`da`/`br`/`onff`/`es`) stay UNKNOWN; the same
  writer-disassembly method resolves each.
- `summ` renders a stat line but no hawk COS entity is spawned; `tele`
  (teleport), `resu` (resurrect), HP/MP drain tags are unimplemented.
- Item-granted buffs (potions/scrolls) have no item-use pipeline; the magic
  state board only shows skill buffs + stun/freeze debuffs.

## Animation slot → `ResourceAnimationType` id

`slot_to_anim_type` (`client/src/assets/textdata/skilleffect.rs`) turns the
symbolic slot strings in cols 7/8/9 into the `typ` id carried by a `.bsr`
`PrimAnimationGroup` entry. **The id space is non-contiguous** — the ATTACK
series is interrupted twice, by `HAMMER`/`HANDLOOF`/`TROW` (187-189) and by
`SHOT` (191) — so it is a table, not a formula.

| slot | id | slot | id |
|---|---|---|---|
| `ANI_ATTACK1` | 2 | `ANI_ATTACK9` | 190 |
| `ANI_ATTACK2` | 5 | `ANI_ATTACK10` | 192 |
| `ANI_ATTACK3` | 16 | `ANI_ATTACK11` | 193 |
| `ANI_ATTACK4` | 17 | `ANI_ATTACK12` | 194 |
| `ANI_ATTACK5` | 183 | `ANI_ATTACK13` | 195 |
| `ANI_ATTACK6` | 184 | `ANI_ATTACK14` | 196 |
| `ANI_ATTACK7` | 185 | `ANI_ATTACK15` | 197 |
| `ANI_ATTACK8` | 186 | `ANI_ATTACK16` | 198 |
| `ANI_SHOT` | 191 | `ANI_ATTACK17`+ | — (no id) |

Sources: `JMX-File-Editor/.../JMXVRES/PrimAnimationType.cs:29-43`; `ATTACK16`
appears only in `SilkroadDoc-wiki/ResourceAnimationType.md:42`. Both are
corpus-confirmed against the user's PK2s (7,714 `.bsr` group tables): every
`*_attackNN.ban` stem for NN = 08..16 sits on the id above at **100%**, and id
187 carries only `skill_{ch,eu}_fortresshammer` — never an `attack09` clip.

Until 2026-08-12 the ATTACK arm extrapolated `178 + n` past ATTACK4, which is
right for 5-8 but put ATTACK9 on `HAMMER` and shifted ATTACK10+ by two. Ten
enabled `skillaniset2` rows hit it (four `ANI_ATTACK9`, plus Seth's
`ANI_ATTACK10`/`12`/`13`/`14`/`15`/`16`): eight resolved to an id their mob's
group table does not carry and silently played no clip, while
`MSKILL_SD_SETH_ATTACK14` and `..._ATTACK16` resolved onto real but *wrong*
clips (`seth_t3_attack10_shot`, `seth_t3_attack12`).

**Still unmapped** (slots the data uses but `slot_to_anim_type` returns `None`
for): `ANI_HAMMER` (187) and `ANI_TROW` (189), used by
`SKILL_{CH,EU}_FORTRESSWEAPON`, `SKILL_FORT_SHOCK_BOMB` and
`SKILL_EVENT_GHOST_AMULET`; and comma-separated multi-slot lists
(`ANI_ATTACK1,ANI_ATTACK2`) on 15 basic-attack rows.
