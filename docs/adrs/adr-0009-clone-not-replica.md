# ADR 0009: Clone, not replica — where modernisation is wanted and where it is not

Date: 2026-08-15
Status: Accepted

## Context

OpenRoad is a Rust/Bevy **clone** of the Silkroad Online v1.188 client. Until now the
project's normative rules read as if it were a *reproduction*: "the reference is the
original v1.188 client", "non-original behavior goes behind config/feature flags". Read
literally, that made every improvement a defect and made "the original did it this way"
a winning argument in review.

That is not what the project is. The owner's ruling (2026-08-15): building things in a
more modern and simply **better** way — better data structures, cleaner architecture,
modern rendering, better UX — is explicitly *wanted*.

The rule was never useless, though. What it actually bought us is protection against
**invented numbers**: a layout constant, a formula coefficient or a timing value that
came from nobody's data and nobody's reasoning. That protection has to survive. So the
review question changes shape rather than disappearing:

> not "does this match the original?" but "is this value's **origin** or its
> **rationale** stated?"

What this ADR adds is the part that question does not answer on its own: *where* the
original still has binding authority, because in some places deviating is not an
improvement, it is a broken client.

## Decision

Every choice falls into one of three buckets. The bucket is decided by **who observes
the result**, not by how important the code is.

### FIXED — the interoperability surface

Something outside our process — a server, a user's PK2 file, a game rule players
already know — defines the correct answer. We do not get a vote. "Modernising" here
does not produce a better client, it produces one that does not work.

1. **Network wire format and opcodes.** A real server defines them. The
   `packets! { opcode => Name }` table (`packets/src/lib.rs:119`) is the single source
   of truth for opcode↔type↔event wiring, and every byte layout under
   `packets/src/{agent,gateway,global,login}` is dictated, not designed. Little-endian
   field order, `size_field`, `when`-conditional `Option`s — all fixed.
2. **On-disk SRO formats.** PK2, JMXV*, DDJ, 2DT, NVM, textdata. The user's own files
   must load. `client/src/assets/twodt.rs:22-25` is the shape of a correct claim here:
   `ENTRY_SIZE = 976` is asserted because it holds exactly across 42 corpus files, not
   because 976 is a nice number.
3. **Game rules and formulas** where matching the original *is* the product — damage,
   mastery/skill math, drop and stat derivation. A player who notices our numbers differ
   has found a bug, not a feature.

A FIXED value needs a **source**, never a rationale: cite the packet dump, the resinfo
line, the corpus probe, the public doc. If no source exists, mark it `UNKNOWN` rather
than guessing (`client/src/plugins/hud/chat/ui.rs:60-64` does exactly this — an
inferred value labelled as inferred).

### NEGOTIABLE — everything internal

Nothing outside the process can observe it. The original's C++-era choices carry **no
authority at all** here; idiomatic Rust/Bevy wins by default, and "the original used a
global manager" is not an argument. This covers in-memory data structures, ECS component
layout, system scheduling and ordering, caching, the asset pipeline, error handling,
threading, rendering technique, and UI layout mechanism.

`bevy_pk2/` is the model example of deliberately doing better. The archive is shared as
`Arc<File>` with no lock because all reads after indexing are positional
(`bevy_pk2/src/pk2/archive.rs:22-31`, `read_exact_at` at `:33-58`), so concurrent asset
loads from one archive do not serialise on a cursor mutex. Untrusted-input handling is
likewise ours, not the original's: block and directory chain walks are bounded and
loop-detected (`ChainGuard` in `bevy_pk2/src/pk2/util.rs:56-68`, `MAX_CHAIN_BLOCKS =
4096` in `constants.rs:15`), and the reader returns `Error`
(`bevy_pk2/src/pk2/errors.rs:3-12`) instead of panicking. None of that is how the
original loaded PK2s. All of it is right.

A NEGOTIABLE choice needs no justification for *being* non-original. It needs only the
usual review bar: is it correct, is it the smallest change that works.

### JUDGEMENT — user-visible behaviour and UI

Here the original's data stays the **default and the tie-breaker**. That is what keeps
us from inventing numbers: when there is no reason to deviate, use the value from the
user's PK2 data (resinfo/`ginterface.txt`, 2DT/newinterface, textdata). But a
**deliberate, stated** improvement is legitimate and must not be rejected merely for
being non-original.

Rule of thumb for the config flag — ADR-0003-style "hide it behind a flag" is now
*guidance*, not an obligation:

- **Add a flag** when a user might reasonably want the original back: pacing and feel
  (camera behaviour, cooldown/animation timing), anything nostalgic, anything that
  changes what the game *is* rather than how well it runs. Also add one when the
  non-original behaviour touches a live server's expectations — e.g. the auto-reply
  note on `0x2113` at `packets/src/lib.rs:121-125`, where "answer XTrap automatically"
  is a behavioural change against someone else's server and stays gated.
- **Do not add a flag** for things nobody wants the worse version of: crash fixes,
  precision fixes, resolution independence, faster loading, better error messages,
  fixing an original bug that is plainly a bug. A `restore_1024x768_letterboxing`
  option is ceremony, not choice.

ADR-0006 (floating world origin) is the precedent: the original had no such thing, we
added one because SRO's ~3×10⁵-unit coordinates make f32 skinning tremble
(`client/src/plugins/world_origin.rs:1-27`), and it shipped with no flag — nobody wants
trembling characters back.

## Worked examples

**1. Resolution-independent layout from the rects — don't hardcode 1024×768 offsets.**
*Do* what `client/src/plugins/hud/minimap.rs:45-63` does: take the rect verbatim from
the data (`resinfo/ginterface.txt:807`, `Rect=RECT,"892,6,140,184"`), record the design
canvas it was authored against as its own constant (`DESIGN_SCREEN_W = 1024.0`), and
derive screen margins from it (`WINDOW_RIGHT = DESIGN_SCREEN_W - x - w`) so the window
anchors correctly at any resolution — including preserving the original's 8px
right-edge overhang. *Don't* paste `Node { left: px(892.0), top: px(6.0) }` and call it
faithful; that is faithful only at one resolution, and it loses the provenance of 892.

**2. Byte-exact on the wire, ergonomic in the game layer.**
*Do* what `client/src/plugins/net/inventory.rs:1-9` does: the CHARACTER_DATA (0x3013)
payload keeps its dictated layout in `packets/`, and the game scene distills it into an
`Inventory` component of `Vec<Option<InventoryItem>>` — empty slots first-class, O(1)
lookup, change detection driving the UI refresh. *Don't* let the wire struct leak into
gameplay so every window has to walk a raw item list, and don't "improve" the wire
struct to make the game layer nicer.

**3. Asset handles and the ECS instead of a mirrored global manager.**
*Do* what `client/src/plugins/dynamic_resource_loader.rs` does: an
`UnloadedResource(Handle<SroResource>)` component plus a system that resolves it, with
per-entity state as components (`MirroredResource`, `SkeletonBinding`). *Don't*
reproduce the original's global resource-manager singleton in Rust as a `Resource<HashMap>`
that every system reaches into — that is porting a C++ constraint we do not have.
`client/src/plugins/sro_v188/mod.rs` is the vestige of that shape and is marked for
removal in favour of `dynamic_resource_loader`; extend the dynamic path, not it.

**4. Modern engine facilities over reimplementing the original's UI machinery.**
*Do* what `client/src/plugins/ui_v2/mod.rs:13-15` does: build on bevy 0.19 headless
widgets (`bevy_ui_widgets`) and style them with the game's own image assets, keeping the
*art and rects* original while the *layout engine* is Bevy's. Where the engine genuinely
lacks something, say so in the code — `ui_v2/style.rs:19-24` explains that bevy_text 0.19
has no password masking, hence the transparent-glyph + overlay trick. *Don't* hand-roll a
retained-mode widget tree because the original had one. Prefer the v2/dynamic
generations (`intro_v2`, `ui_v2`) over `intro`/`ui` for new work.

**5. An inferred number, labelled.**
*Do* what `client/src/plugins/hud/chat/ui.rs:57-64` does: the large list height is the
resinfo value; the collapsed one is not in any PK2 file, so the constant carries its
derivation (138 total between board top 546 and underbar top 684, minus the same 55px of
chrome) and the words "not a read value". *Don't* write `const LIST_H_SMALL: f32 = 83.0;` bare. That —
not the deviation — is the actual defect this ADR still prosecutes.

## Documentation obligation

Deliberately light. Two places, one line each:

- **PR body:** one line per deliberate deviation — what we do instead of the original,
  and why. "Minimap anchors from the right edge instead of a fixed x, so the vanilla
  overhang survives at non-4:3 resolutions."
- **Code comment:** only when the reason is not obvious from the diff. Per AGENTS.md, a
  file implementing a non-trivial concept already opens with a comment stating the
  *idea*; for a deviation, that opening comment is where the rationale belongs — see
  `world_origin.rs:1-27` and `net/inventory.rs:1-9`. A one-line `///` on the constant is
  enough for a single value.

No template, no checklist, no review gate beyond normal review. If a reviewer cannot
tell where a number came from or why we deviated, that is the bug to report.

## Descriptive vs normative documents

This ADR changes **normative** documents only — rules, runbooks, checklists, acceptance
criteria, review gates. **Descriptive** documents (`docs/formats/**`, the protocol
notes) record what the original client *does*. They are evidence, not orders,
their factual claims are unchanged, and they must not be weakened, hedged or deleted.

"The original draws the bar at x=310" stays exactly as written. "Our implementation MUST
draw the bar at x=310 or it is wrong" becomes "x=310 is the original's value; deviate
only deliberately and say why."

## Unchanged and still absolute

Nothing in this ADR touches the safety rules, which are not engineering preferences:

- Never add, distribute, link, download or execute SRO assets or binaries. Users supply
  their own PK2 files (ADR-0004). SRO-scene downloads are frequently trojaned; research
  catalogs links only.
- PK2 and data files are handled exclusively as **pure data** through our own parsers
  (`bevy_pk2/`, `client/src/assets/`).
- Network testing runs only against the user's own local stubs — never live official
  servers, and no protection circumvention (ADR-0003).

## Consequences

- "It is not what the original did" is no longer, by itself, a review objection. The
  reviewer must say which bucket the change is in and why the original binds there.
- "Where does this number come from?" remains a blocking objection, in all three
  buckets. In FIXED it demands a source; in JUDGEMENT a source *or* a stated rationale;
  in NEGOTIABLE it is just ordinary code review.
- Bucket disputes are the new failure mode, and they are cheap to settle: ask who
  observes the result. A server or a user's file? FIXED. Only us? NEGOTIABLE. A player?
  JUDGEMENT.
- Config flags shrink to the cases where someone would plausibly flip them back, so the
  options surface stops accumulating dead switches.
- Any fidelity checklist a maintainer keeps should be read through this ADR: its job is
  provenance, not identity.

---

## Amendment (2026-08-15) — consequences for the RE program

This ADR was published from the implementation side. The reverse-engineering program needs two
concrete changes to stop producing findings that nobody can act on.

**1. The `compare_verdict` taxonomy grows the missing distinction.** It was
`matches | drift | stub | missing`, which had no way to say "differs on purpose". It is now:

| verdict | meaning |
|---|---|
| `matches` | we already do what the original does — cite the proof, no issue |
| `drift` | we differ **and it is probably wrong** (no rationale, or the rationale does not survive the evidence) |
| `deliberate-improvement` | we differ **on purpose**, rationale stated — not a defect, normally no issue |
| `missing` | the original has it, we do not |
| `NEW` | a unit openroad never knew existed (301 opcodes arrived this way in round 2) |
| `do-not-wire` | understood and deliberately out of scope — say why once, so nobody re-opens it |

`stub` is retained as a sub-case of `drift`, so no existing ledger row needs migrating.

**2. Every unit doc now carries a mandatory `### Proposed openroad implementation` section.**
A finding that only says what the original does is half a deliverable: the knowledge base was
accumulating well-cited units that an implementer still could not start from without re-reading
the decompile. The section states **Shape** (the Rust types, in modern idiom — enums over magic
discriminators, newtypes over sentinels, `bitflags` over hand masks, ECS Components/Messages over
god-structs), **Where** (the concrete path in this tree), **Deviation and its one-line rationale**,
and **Cost**. Both critics enforce it.

The rule behind the rule: a 1:1 transliteration of a 2005 C struct is itself a design choice, and
it needs the same justification as any other. The original's data structures were shaped by
constraints — no generics, no sum types, no borrow checker — that we do not have.
