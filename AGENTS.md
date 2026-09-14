# Agent Instructions

## Priorities
- Keep changes minimal and focused on the user request.
- Preserve existing architecture and coding style.
- Avoid adding dependencies unless necessary.
- Do not modify or delete user-owned assets or proprietary files.
- For a file or function implementing a non-trivial concept (a shader, an algorithm, a
  non-obvious system interaction), lead with a short comment stating the *idea* of the
  implementation — what it's built on top of and why, not what each line does. A reader should
  understand the approach before reading the code. Example: `water_hq_material.rs` and
  `water_hq.wgsl` both open with why the water is a `StandardMaterial` extension rather than a
  standalone material, and what each of its three visual tricks is doing.

## Reference doctrine (ADR 0009 — clone, not replica)
- OpenRoad is a **clone** of the v1.188 client, not a byte-for-byte replica. The original
  client and the user's own PK2/vSRO data are the **default reference and tie-breaker** —
  that is what stops us inventing numbers.
- A **deliberate deviation with a stated rationale** (modern rendering, UX, accessibility,
  cross-platform portability, testability) is legitimate and wanted. The defect is an
  **unsourced, unexplained magic number**, not a deviation as such: every value must have
  either its origin (which data file or measurement it came from) or its rationale written down.
- Non-negotiable regardless: wire compatibility with a real vSRO server, and the Safety
  rules below.

## Project Context
- OpenRoad is a **clone** of the Silkroad Online client, **not a 1:1 replica**. The original
  v1.188 client and the user's own PK2 data are the default reference and the tie-breaker when
  there is no reason to deviate, but building something more modern and simply better (data
  structures, architecture, rendering, UX) is wanted, not a defect. A deliberate improvement is
  legitimate; an unsourced, unexplained magic number is not. State either a value's origin or
  its rationale — and when a change departs from the original on purpose, say so and why in one
  line in the PR body.
- This is a Rust workspace; the toolchain is pinned by `rust-toolchain.toml` (currently stable).
- The primary client lives in `client/`.
- PK2 reading is implemented in `bevy_pk2/`.
- Tools and CLIs live in `tools/`.
- Every received network packet is dumped to `packet_dump/<opcode>.log` (one timestamped
  hex line per packet, gitignored). Use these dumps to analyze packet payloads offline
  instead of requiring a live server session.
- **Every system under `client/src/plugins/net/**` must run without the HUD.** The headless
  netcheck harness builds `MinimalPlugins + NetworkCorePlugin` and registers no HUD
  resources, and Bevy does not skip a system whose `Res`/`ResMut` is missing — it fails
  parameter validation and panics the schedule, killing the process before login. So a net
  system that wants a HUD resource takes it as `Option<Res<_>>` and degrades (log instead of
  chat line), or the reporting moves into a HUD system behind a message. Registering the HUD
  resource in the harness is the wrong fix: it drags HUD state into something that
  deliberately has none, and only postpones the failure to the next HUD access.
- `make ci` **builds and unit-tests; it never starts the app.** Two whole failure classes are
  therefore invisible to it: missing-resource parameter-validation panics, and plugin/system
  tuples that outgrow the arity Bevy implements (`Plugins` stops at 15 — a 16th entry makes
  the *whole* nested tuple fail the trait, and the error names no type at all). Before merging
  a change that touches systems, resources or a plugin registry, run the 40-second smoke:
  `NETCHECK=1 cargo run -p client` (add `NETCHECK_ACTIONS=1` for the action path).

## Documentation
- Update `README.md` and `docs/*.md` when behavior or workflows change.
- ADRs go in `docs/adrs/adr-xxxx-*.md` and should include Date and Status.

## Commands
- Prefer `rg` for searching.
- Use `cargo run -p <crate>` for workspace binaries.
- New grouped Make commands must use space-separated subcommands, not hyphenated target names (for example `make pk2 list`, not `make pk2-list`).
- Avoid destructive git commands unless explicitly requested.
- Reverting a merged PR: state the reason in one sentence on the revert (defect /
  visual veto / collateral / precaution) and reopen the issue it closed
  (`docs/adrs/adr-0010-revert-rationale.md`).

## External sources and licence classes (merge gate)
- OpenRoad is **GPL-3.0**. If a change was informed by anything outside this repository,
  **cite the source and its class in the PR** — the class decides what may be used.

| Class | Sources | Allowed use |
|---|---|---|
| **MIT** | Veykril/pk2 crate, JMX-File-Editor (C# — structures portable), NVMTerrainExtractor, SRO-2DT-Editor, SR_Db2Media (skilldata cipher) | Depend on / port code and structures |
| **DBAD** | florian0/SRO_DevKit, go-sro | Read/reference only for GPL-3.0 purposes |
| **No license** (all-rights-reserved by default) | SilkroadDoc wiki, sr_formats, Silkroad-Effect-Viewer, both Blender importers, NVMEditor, threejs-pyside6, srodevs-docs, devtekve Security.cs | Learn facts and field layouts; **never copy code or text** |
| **AGPL-3.0** | skrillax (server), RSBot | Behavior/spec reference only; **no code porting** |
| **Same-owner workspace** | sibling `sro-rs-client` docs, scripts, stub server | Adopt freely (verify headers when porting code) |
| **Proprietary** | "Silkroad Origin" mobile remake data | Static inspection of a user-supplied install only; never redistribute, never commit; **not** v1.188 ground truth |

- Same table in `CONTRIBUTING.md`, which is the source of truth for source classes.
- The two easy mistakes: **AGPL-3.0** sources are observed, never ported; **no license**
  sources are all-rights-reserved — learn facts and field layouts, never copy code or text.

## What must not enter the repository
- Decompiler output and everything shaped by it: pseudocode, function addresses, the
  original binary's internal source-file names and assert strings. Facts (wire layouts,
  opcode numbers, field orders) may be written down; the evidence behind them may not.
- Verbatim copies of the original's data files (`resinfo` trees, `textdata` strings, 2DT
  tables) — cite the values a change needs, do not transcribe the file.
- Circumvention detail: key material, key-derivation walkthroughs, anti-cheat internals,
  passcode mechanics.
- Third-party proprietary data, including "Silkroad Origin" mobile extracts.
- Process documents: roadmaps, dashboards, backlogs, session reports, RE worklogs. Keep
  them local; the issue tracker carries what is planned.
- Rationale and the full policy: `CONTRIBUTING.md` § "What this repository does not publish".

## Safety
- Do not add or distribute SRO assets. Users must supply their own.
- No links or downloads for SRO client data.
- SRO-scene downloads (clients, server files, tools) are frequently trojaned: never execute
  them and never download binaries/archives during research — catalog links only.
- PK2/data files are read as pure data via our own parsers only.
- Network testing only against the user's own local stubs — never against live official servers.
