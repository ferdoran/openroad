# ADR 0010 — A revert of a merged PR carries its reason; the reasons for #326, #350 and #360

Date: 2026-08-15
Status: Accepted

## Context

On 2026-08-12/13 three merged PRs were reverted within hours, with no written
reason anywhere: not in the revert commit messages (`01dc76d`, `1ed8db7`,
`4dca06a`, `eeabad9` — all the unedited `Revert "…"` template), not in the
revert PRs (#391/#392/#397, body `Reverts #NNN`), not in a review, not on the
issues they had closed, not in `docs/`.

| Merged PR | Revert | Delta | Files |
|---|---|---|---|
| #326 `fix(bsr): TexAni carriers + BumpEnv → sheen` | #397 / `01dc76d` | 19 h 20 m | `client/src/assets/bsr/bsr.rs` (+doc) |
| #350 `fix(efp): version 0010, timeline pad, doc layout` | #392 / `1ed8db7` | 9 h 44 m | `client/src/assets/efp/format.rs` (+doc) |
| #360 `fix(map): .o2 LoD count, .o has 4 groups` | #391 / `4dca06a`, `eeabad9` | 40 m | `client/src/assets/o.rs`, `o2.rs` |

Two costs followed. The issues those PRs closed (#287, #291) stayed **closed**
while their fixes were no longer in `main`, so live bugs were marked done — in
`.o2`'s case a defect that drops 9.2 % of world objects. And because nobody
could tell a crash from a taste call, the next author had to re-measure all
three from scratch (#441, #442, #443) before anything could move again.

This ADR records the reasons, reconstructed from the diffs and those audits,
and the rule that keeps the next revert from costing the same.

## Decision

**1. A revert of a merged PR states its reason in one sentence, on the revert
itself.** Commit message body or revert-PR body, either is fine. Four words are
enough if they are the right ones — `panics on every real .o2` is a complete
rationale. The taxonomy that matters to the next reader is:

- **defect** — it crashes, corrupts or regresses something measurable;
- **visual/behavioural veto** — the byte-level RE may be right, the result on
  screen is not what we want (an entirely legitimate reason in a
  re-implementation — ADR-0009 says the original is the default reference, not
  an obligation);
- **collateral** — this hunk is fine, it just shares a commit with one of the
  above;
- **precaution** — no defect found, reverting to de-risk; say so, because it is
  the one class that should come back.

**2. A revert reopens the issue the PR closed**, with a one-line note. An issue
whose fix is no longer in `main` is not done.

**3. Do not mix a pure data/parser fix and a visual change in one commit.** The
#326 revert had to take a correct parser fix with it because both lived in one
commit. Split them and a veto costs one hunk, not two.

**4. A parser fixture must be shaped like the real file.** #360's fixture
omitted the 12-byte signature every `.o2` on disk carries, so its tests passed
while the loader panicked on all 4,506 of them. Fixtures come from — or are
built to the shape of — real data (`packet_dump/`, the user's PK2s).

## The reasons, reconstructed

Reconstruction, not testimony: the maintainer has not stated a reason and none
of this is a claim about intent. Each row says what the record supports and how
confident it is.

### #360 `.o` / `.o2` — **defect. Certain.**

The merged `o2.rs` panics on 4,506 of 4,506 real `.o2` files. It removed the
`if w_temp != 0 { … continue }` branch, which was the only thing consuming the
12-byte `JMXVMAPO1001` signature — by accident, since `JM XV MA PO 10 01` is six
nonzero `u16`s that push six phantom blocks and land the cursor exactly on
offset 12. Without it the reader takes `0x4D4A` as an object count and
`bytes::Buf` underflows, inside an asset-loader task. Verified independently
while re-landing (#287 → PR #522) and measured in #442. **The revert was
necessary and needed no defence.**

Note what it did *not* fix: the accidental skip means the shipped loader parses
real blocks 0..29 into slots 6..35 and never reads blocks 30..35 — 37,599
placements, 6,803 of 74,060 distinct objects (9.2 %) never spawned. The correct
patch is *skip the signature **and** read four groups*, which is what the
re-land does.

### #326 `.bsr` — **visual veto on one hunk, collateral on the other. High confidence.**

The `BumpEnv → sheen` hunk routes +25 resources into `alpha_is_sheen`
resource-wide (13 world props, 4 interface idols, 3 potions, 5 mobs — which lose
their rim, sheen being checked first). That is the same class this project had
already vetoed two days earlier, in writing:
`docs/rendering-mobile-shader-comparison.md` row 3 (commit `9ec02d5`,
2026-08-10) — *"applied per-resource to every EnvMap resource … which read as
wrongly shiny in the playtest"*, dropped because 1.188 has no metallicity
channel. A revert consistent with a written prior decision is a rationale even
unwritten; the author of #326 reached the same conclusion in #443 and withdrew
the hunk.

The `TexAni +32` hunk is a different animal — a parser predicate that is
provably right (0 false positives, 0 false negatives against a full palette
walk; the old zero-check dropped 43 % of carriers). It was reverted because it
shared the commit. Its *runtime* half is genuinely arguable (12 resources start
scrolling the base diffuse instead of the authored MultiTex overlay, and ~105
meshes silently gained `NotShadowCaster`), which is a defensible veto too — but
of the runtime hunk, not of the predicate.

### #350 `.efp` — **no defect found; best-supported reading is precaution. Low confidence, needs the maintainer's word.**

Re-checked here: the revert touches exactly `client/src/assets/efp/format.rs`
and its doc — no shader, material, spawn or scheduling file. The one field with
a rendering-sounding name (`two_sided` → `cull_mode`) has no reader anywhere in
the workspace, and `client/src/plugins/effects/material.rs:261` sets
`descriptor.primitive.cull_mode = None` unconditionally, so no rendering change
was even reachable. #441 reproduced every claim over 3,738 `.efp`.

What the record does support: this revert lands 40 minutes after the `.o2`
crash revert, from the same maintainer, on the same author's same-day batch of
format-parser PRs, with no review anywhere — the shape of a precautionary sweep
after one PR in a batch turned out to crash the client, not of an independent
judgment on `.efp`. Two smaller, defensible objections exist in the diff and
should be answered on re-land: the blanket `0010` version accept is fail-open on
a sample of one (narrow it to the one known file), and the doc it rewrote is
load-bearing for other work.

**Disposition:** re-land, minus the blanket accept.

## Consequences

- The next reader of a revert learns the reason from the revert, not from a
  three-issue re-audit of the PK2 corpus.
- Reverts stay cheap and uncontroversial: with a stated class, "precaution" can
  be undone by anyone who does the measurement, and a "veto" is not re-litigated
  by the next contributor who reads the byte layout.
- Issues stop lying about their state.
- This is a review-time convention, not a CI gate — nothing in the local gate
  can see GitHub. `AGENTS.md` points at this ADR so it is in front of whoever
  writes the next revert.
