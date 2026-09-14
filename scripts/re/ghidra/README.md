# Ghidra binary oracle (`scripts/re/ghidra/`)

Headless Ghidra tooling for reverse-engineering the **original v1.188 client** (`sro_client.exe`) and
the **vSRO server** (`SR_GameServer_Clean.exe`, `SR_ShardManager.exe`) from the maintainer's local
quarantine drop. It produces machine-local analysis output only: nothing it emits — addresses,
decompiled C, string tables — is committed to this repository
(see [`CONTRIBUTING.md`](../../../CONTRIBUTING.md) § "What this repository does not publish").

## Security

**Static analysis only.** These scripts open an already-imported Ghidra project `-readOnly` and read
bytes, xrefs, strings and decompiled C. Nothing from the drop is ever executed, no DB is restored,
and nothing is downloaded. That constraint is not incidental — vSRO scene binaries are routinely
trojaned, and six of the drop's server executables are confirmed infected (they are quarantined
under an `.INFECTED.*` suffix and are never touched by these scripts).

## Prerequisites

- Ghidra 12 (`brew install ghidra`) and a JDK 21. Override with `GHIDRA_HOME` / `JAVA_HOME`.
- An analysed project at `<quarantine>/ghidra-proj` (project name `vsro`) containing the
  imported binaries. The repo carries **no** binaries and no project — this is machine-local.

The project is single-writer: only one headless run at a time. Copy the project directory if you
need parallelism (~500 MB per copy).

## `requery.sh` — batch query

A Ghidra startup costs ~30 s, so one run answers many questions. Job file, one query per line:

```
id|decompile|<addrOrName>       # decompiled C + signature + size
id|bytes|<addr>|<len>           # hex + u32/i32/f32/u64/f64 LE + ascii  (.rdata constants)
id|xrefs|<addr>                 # references to, with containing function
id|strings|<javaRegex>|<limit>  # defined strings + their referencing functions
id|funcs|<javaRegex>|<limit>    # function name search
id|callers|<f> · id|callees|<f> # call graph, one hop
id|disasm|<addr>|<n> · id|data|<addr>
```

```bash
scripts/re/ghidra/requery.sh SR_GameServer_Clean.exe job.txt out.json
```

Results are one JSON object with a `results` array keyed by your `id`s.

## `redump.sh` — bulk corpus

```bash
# whole-program index: functions.tsv, strings.tsv, srcpaths.tsv
scripts/re/ghidra/redump.sh <quarantine>/ghidra-proj sro_client.exe out/ index

# one <VA>_<name>.c per address in the list (skips files that already exist, so it resumes)
scripts/re/ghidra/redump.sh <quarantine>/ghidra-proj sro_client.exe out/ decompile addrs.txt
```

`srcpaths.tsv` isolates the source paths a build left embedded in the binary together with the
functions that reference them. That output stays on the analyst's machine.

`decompile` mode defines a function in memory when a registration table points at a raw `LAB_`
address Ghidra never claimed as code; because the project is opened read-only, that stays in-memory.

## Recipes

```bash
# the client's inbound opcode -> handler table
printf 'a|decompile|<VA>\n' > job.txt && ./requery.sh sro_client.exe job.txt t.json
#   -> the opcode/handler pairs the table registers

# every outbound packet builder
printf 'a|callers|<VA>\n' > job.txt   # the packet-builder constructor

# a .rdata float/double constant behind a formula
printf 'a|bytes|<VA>|8\n' > job.txt  # -> the constant behind a formula
```
