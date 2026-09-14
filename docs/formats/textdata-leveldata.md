# textdata: leveldata.txt (`_RefLevel`)

Location: `Media.pk2/server_dep/silkroad/textdata/leveldata.txt` — a single file
(no split), UTF-16LE, tab-separated, one row per **character level** (140 data
rows L1..L140 + a `//DBtoMedia` trailer). Parser:
`client/src/assets/textdata/leveldata.rs` (+ `mod.rs:287-307`).

The file is emitted by `SELECT * FROM _RefLevel` (`SR_Db2Media/Settings.cs:66`),
so the **column order is the raw `_RefLevel` DB schema order** — but that table's
real column *names* are not present in any local corpus (the one name-bearing
doc, `silkroad-docs/docs/data_tables.md:689`, is a heuristic auto-labeler whose
names are demonstrably wrong). Names below marked `[S]` are functional
descriptions inferred from the data shape; the DB schema names stay **UNKNOWN**.
Column meanings corpus-verified 2026-08-12 across all 140 rows.

## Columns (0–8; a 9th trailing empty field brings the raw split to 10)

| col | field (functional) | tag | notes |
|-----|--------------------|-----|-------|
| 0 | **Level** | `[V]` | 1..140; row key. Also the job-level index for cols 6-8 |
| 1 | **Char EXP to advance L→L+1** (`Exp_C`) | `[V]` **to L90 only** | **u64** — 118 @L1 … 103,622,218,294 @L140 (exceeds u32 above ~L130). **Per-level**, not cumulative (differencing gives wrong values — `leveldata.rs:3-9`). Parsed → `exp`.<br>**Authoritative only to L90 (round 2, gamedata.md G16).** The client's `leveldata.txt` and the server's `LevelData.txt` are identical, but what the **server actually loads is `_RefLevel`**, and its `RefLevel.txt` diverges from L91 up (L91: 305,888,332 vs 369,337,595; L110: 1,249,891,465 vs 4,044,607,839). Four sources agree to L90 and split into three different curves above it, and the client table is **non-monotonic at L92–96**. Above L90, treat this column as *client display data*, not as the EXP the server charges. |
| 2 | **Mastery-point (SP) cost to raise a mastery TO this level** | `[V]` shape / `[S]` name | 1,1,1,2,2,4,5,6,7,9,12,15,18,21,24,30,… → 116,614 @L140; non-decreasing. Parsed → `mastery_sp` |
| 3 | reserved | `[V]=0` | **0 in every row** (0/140 non-zero); meaning UNKNOWN |
| 4 | reserved | `[V]=0` | **0 in every row**; meaning UNKNOWN |
| 5 | accelerating secondary curve | `[V]` shape | 24,47,71,94,118,… → 30,462 @L140; step ≈round(23.5·L) early then accelerates (65@L40, 491@L120). **Not** cumsum(col2) (0/139 match); int32-range. Meaning UNKNOWN — do not name |
| 6,7,8 | **Job-EXP thresholds** (3 **byte-identical** columns) | `[V]` shape / `[S]` per-vs-total | Identical in 139/139 rows → one curve for all three job types. Positive only for **job-levels 1-7** (70875, 2388750, 8793750, 38745000, 91665000, 240187500, **2147483647=INT32_MAX** sentinel), then **-1** for L8..140. **Job level cap = 7.** Unparsed today |

## Consumers (openroad)

`leveldata.rs` parses **only** cols 0/1/2. Cols 3-8 are ignored (`mod.rs:288`
notes the job columns but never parses them). Col1 → `max_exp(level)`:
`intro_v2/character_select.rs:640`, `hud/character_info/ui.rs:705` (next-exp
text), `hud/underbar/ui.rs:939` (exp-bar fill). Col2 → `hud/skill_window/ui.rs:2443`
(mastery raise cost).

## Notes

- **Level derivation caveat (#209):** the initial level is server-authoritative
  (`underbar/model.rs:100-101`, from 0x3013), but ongoing level-ups are re-derived
  client-side from the col1 curve (`model.rs:226-233`, `while exp_offset >=
  max_exp(level)`). On custom-rate servers whose real thresholds differ from
  Media.pk2, this drifts — col1 should be treated as display-only, not authoritative.
- **Job-EXP (cols 6-8):** `JobExp` on the wire is a **total** (0x3013 `JobInfo`,
  `AGENT_JOB_UPDATE_EXP`), compared against these thresholds; whether the table
  values are per-level or cumulative is UNKNOWN (differencing check or a job-exp
  capture resolves it). No job-exp bar exists in openroad today (curve unloaded).
.
