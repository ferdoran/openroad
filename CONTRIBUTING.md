# Contributing to OpenRoad

OpenRoad is a cross-platform Rust/Bevy **clone** of the Silkroad Online v1.188 client.
Every crate is **GPL-3.0-or-later** (`LICENSE`). That licence is why the table below
is a merge gate rather than tribal knowledge: a single AGPL or all-rights-reserved
copy-paste would contaminate the whole tree.

Third-party material carries its own obligations regardless — record it in
`THIRD-PARTY.md`, keep the upstream notice with the code, and put the licence text in
`licenses/`.

> **Clone predating 2026-09-01?** The history was rewritten then — every commit
> hash changed. Re-clone and cherry-pick your work onto the new `main` before
> opening a PR.

## The gate

**There is no automatic CI gate.** This repository deliberately runs no GitHub Actions
build on a push or a pull request, and no status check is required to merge, so the gate
is local and it is `make ci`:

```
make ci   # check-target-dir + fmt-check + warnings + opcodes + re-tools
          # + reference-data + deny + test + build
```

Run it before opening a PR, on the toolchain `rust-toolchain.toml` pins (currently **stable**).
The PR template's Definition-of-Done checklist is the same gate spelled out step by step.

The `.github/workflows/build-{linux,windows,macos}.yml` workflows cross-build the client
for the three supported targets and upload a zip per platform, but they run on
**manual dispatch only** (`gh workflow run build-macos.yml`) and are **not** a merge
gate. Use them to check a target you cannot build locally — not instead of `make ci`.

`release.yml` is the **only** workflow that writes to this repository: it pushes a
`chore(release): vX.Y.Z` commit to `main` touching `Cargo.toml` and `Cargo.lock` only,
then tags and publishes. It is still not a merge gate, and it does not run `make ci` —
its three builds are the only check. Never hand-edit `[workspace.package] version`; that
workflow owns it. See the README's *Releases* section.

## Ground rules

- **Smallest correct diff.** One issue, one PR, `Closes #NN` in the body.
- **Clone, not replica** (`docs/adrs/adr-0009-*`). The original client and your own PK2
  data are the default reference and the tie-breaker, but a *deliberate, stated*
  improvement is legitimate. The defect is the **unsourced, unexplained magic number** —
  state either a value's origin (which data file or measurement it came from) or its rationale. When a change
  departs from the original on purpose, say so in one line in the PR body.
- **Wire compatibility with a real vSRO server is non-negotiable.**
- **No SRO assets.** Never add, link, commit or redistribute game data; users supply
  their own PK2 files. `packet_dump/` stays gitignored, and credentials/session tokens
  are redacted before a dump is quoted.
- **Never execute an SRO-scene binary.** Clients, server files and tools from the scene
  are treated as trojaned: static analysis only, inside the quarantine sandbox
  (`scripts/re/safe-analyze.sh`). Never download binaries or archives during research —
  catalog the link instead.
- **Network testing only against your own local stub or private server**, never against
  a live official server.

## External sources and their licence classes (merge gate)

If a change was informed by anything outside this repository, **cite the source and its
class in the PR**. The class decides what you are allowed to do with it:

| Class | Sources | Allowed use |
|---|---|---|
| **MIT** | Veykril/pk2 crate, JMX-File-Editor (C# — structures portable), NVMTerrainExtractor, SRO-2DT-Editor, SR_Db2Media (skilldata cipher) | Depend on / port code and structures |
| **DBAD** | florian0/SRO_DevKit, go-sro | Read/reference only for GPL-3.0 purposes |
| **No license** (all-rights-reserved by default) | SilkroadDoc wiki, sr_formats, Silkroad-Effect-Viewer, both Blender importers, NVMEditor, threejs-pyside6, srodevs-docs, devtekve Security.cs | Learn facts and field layouts; **never copy code or text** |
| **AGPL-3.0** | skrillax (server), RSBot | Behavior/spec reference only; **no code porting** |
| **Same-owner workspace** | sibling `sro-rs-client` docs, scripts, stub server | Adopt freely (verify headers when porting code) |
| **Proprietary** | "Silkroad Origin" mobile remake data | Static inspection of a user-supplied install only; never redistribute, never commit; **not** v1.188 ground truth |

This table is the source of truth for source classes; the same table appears in
`AGENTS.md` for agent sessions.

Two classes are worth restating because they are the easy mistakes: **AGPL-3.0** sources
(skrillax, RSBot) may be *observed*, never ported — describe behaviour in your own words
from your own captures. **No license** sources are all-rights-reserved by default: you may
learn a fact or a field layout from them, but you may not copy their code or their text.

## What this repository does not publish

OpenRoad is written for **interoperability**: an independently written client that
speaks the same file formats and wire protocol, so that data a user already owns stays
usable. Studying the original client for that purpose is one thing; republishing what
was found is another, and we keep the two apart.

Out of the repository, by policy:

- **Decompiler output and everything derived from its form** — pseudocode, function
  addresses, and the internal source-file names and assert strings the original binary
  happens to embed. A wire layout, an opcode number or a field order is a fact and may
  be written down; the decompiled evidence behind it may not.
- **Verbatim copies of the original's data files** — `resinfo` UI trees, `textdata`
  strings, 2DT tables. Cite the handful of values a change actually needs, do not
  transcribe the file.
- **Circumvention detail** — key material, key-derivation walkthroughs, anti-cheat
  challenge internals, second-password/passcode mechanics. openroad implements the
  handshake it needs to talk to a server the user owns (ADR 0003) and documents its own
  code; it does not publish a guide to defeating a protection measure.
- **Third-party proprietary data**, including extracts from or comparisons against the
  "Silkroad Origin" mobile remake.
- **Working process** — roadmaps, dashboards, backlogs, session reports and
  reverse-engineering worklogs. They age badly, they are noise for a reader, and the
  issue tracker already carries what is actually planned.

If you need such material to justify a change, keep it locally and summarise the
*conclusion* in the PR — that is what a reviewer can act on anyway.

## Documentation

Update `README.md` and `docs/*.md` when behaviour or workflows change. Architecture
decisions go to `docs/adrs/adr-xxxx-*.md` with Date and Status. Some older code comments
cite paths under `docs/re/…`; that knowledge base was retired by the documentation
cleanup and is no longer published.
