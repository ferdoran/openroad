# Documentation

The documentation openroad publishes is deliberately small: what a user needs to
run a build, what a contributor needs to work on the code, the decisions that
shaped the architecture, and the file/wire formats our own parsers implement.

Working notes — session reports, roadmaps, dashboards and reverse-engineering
evidence — are not published; see [`../CONTRIBUTING.md`](../CONTRIBUTING.md)
§ "What this repository does not publish" for why.

## Running and tooling
- [`RUNNING.md`](RUNNING.md) — running a downloaded build: unblocking the unsigned
  binary, the Linux runtime libraries, where your PK2 files and the archive key go,
  and a symptom/cause troubleshooting table
- [`pk2-structure.md`](pk2-structure.md) — PK2 archive structure overview
- [`bsr2glb.md`](bsr2glb.md) — BSR → glTF/FBX exporter (`make bsr2glb`)
- [`cutscene-convert.md`](cutscene-convert.md) — how the intro cutscene is chosen and
  converted from your own `Media.pk2` at startup (`make cutscene convert`)
- [`perf-remote.md`](perf-remote.md) — remote perf inspection over the Bevy Remote
  Protocol (`make perf ...`)

## Client architecture
- [`settings-live-apply.md`](settings-live-apply.md) — how a changed setting reaches
  its consumer without a restart, and the audit of every `ClientConfig` group

## Network protocol
- [`protocol/opcodes.md`](protocol/opcodes.md) — the opcode coverage ledger; CI fails
  if it and the `packets!` macro in `packets/src/lib.rs` disagree
- [`net-framing.md`](net-framing.md) — the `SilkroadFrame` codec (6-byte header,
  `0x8000` encryption mask, padding), the inbound accumulator and `0x600D`
  massive-packet reassembly
- [`net-entity-spawn-0x3019.md`](net-entity-spawn-0x3019.md) — the group-spawn record,
  the one wire layout complex enough to warrant its own page

Every other payload layout is documented next to its type in `packets/src/**`.

## File formats
- [`formats/README.md`](formats/README.md) — index of the format notes; each page
  describes what openroad's own parser reads and what was verified against a PK2
  corpus the user supplied

## Architecture decisions
ADRs live in [`adrs/`](adrs/) and are numbered:

| ADR | Subject |
|---|---|
| 0001 | Rust and Bevy |
| 0002 | PK2 access and the v1.188 basis |
| 0003 | Networking and non-circumvention |
| 0004 | User-supplied assets and install location |
| 0005 | Launcher GUI framework (iced) |
| 0006 | Floating world origin |
| 0007 | Stateful nav location |
| 0008 | Dungeon pipeline |
| 0009 | Clone, not replica — the reference doctrine |
| 0010 | Revert rationale |
