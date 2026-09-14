# `scripts/re/` — reverse-engineering tooling

Stdlib-only helpers for the RE lane. Nothing here downloads anything, and nothing here
executes an analyzed binary.

## `safe-analyze.sh` — quarantine front door (never-execute rule)

SRO-scene drops (clients, server files, tools) are treated as **trojaned by default** —
the reference quarantine really did contain Swisyn-infected server EXEs. So a drop is
never run, never decompiled and never fed to our own parsers before it has a verdict,
and the verdict is produced **without executing the file**.

```bash
scripts/re/safe-analyze.sh --build            # one-time image build (bakes ClamAV sigs)
scripts/re/safe-analyze.sh /path/to/drop.exe  # local path only — never a URL
```

Requirements: a container runtime, Docker Desktop **or** Colima
(`brew install colima docker && colima start`; Colima is the reference runtime, lighter
and CI-friendlier). The image assets live next to the script in
`scripts/re/safe-analyze/{Dockerfile,analyze.py,rules.yar}`.

### The workflow

1. The target is **copied** into a per-run staging dir and made non-writable
   (`chmod -R a-w`); the original is never touched.
2. A disposable container reads that copy through a read-only bind mount and writes a
   single file back: `report.json`.
3. The verdict is the process exit code, and the report is kept as the audit record of
   "this file, this hash, this verdict, this date".

Layout under `${OPENROAD_QUARANTINE_DIR:-$HOME/sro-quarantine}`:

```
safe-analyze/<basename>-<RFC3339Z>/
├── input/<copy>        # read-only staging
└── output/report.json  # the verdict
```

Convention for a positive hit: isolate the file with a `.INFECTED.<signature>` suffix
rename (e.g. `FarmManager.exe.INFECTED.Swisyn-9968222-0`) and record it in
`SHA256SUMS.txt` (`shasum -a 256`).

### Container envelope (the security is the flags, not trust)

| flag | why |
|---|---|
| `--network=none` | no exfiltration, no C2, no download |
| `--read-only` | immutable root filesystem |
| `--tmpfs /tmp:rw,size=512m,exec` | bounded scratch — zip-bomb cap |
| `--cap-drop=ALL` | no Linux capabilities |
| `--security-opt=no-new-privileges` | no privilege escalation |
| `--memory=2g --cpus=2` | resource caps |
| `--pids-limit=512` | fork-bomb cap |
| `-v <stage>:/work/input:ro` | the target is data, mounted read-only |
| `-v <out>:/work/output:rw` | the report is the only writable path |

### Verdict codes

| code | meaning |
|---|---|
| `0` | clean — proceed |
| `10` | suspicious — human review before the next step |
| `20` | infected — stop; do not run, do not decompile unsandboxed |
| `30` | analysis itself failed |

ClamAV signatures are baked at **build** time, so the run needs no network. An image
built without reachable mirrors reports `clamav.available=false`: that is a **warning,
not a pass** — rebuild before trusting a clean verdict.

### Deliberate limits

- **No download path.** A URL argument is refused. Drops are supplied by the user,
  by hand; tooling never fetches binaries.
- **No execution, ever** — not by the host script, not inside the container. Only the
  sandbox image's own entrypoint is executed. `scripts/test_safe_analyze.sh` asserts
  this with a canary sample that would leave a file behind if it were ever run.
- **Docker is not tested in CI.** `make ci` runs `scripts/test_safe_analyze.sh`, which
  substitutes a fake `docker` (`SAFE_ANALYZE_DOCKER`) and asserts the command line the
  script builds. Whether a real container run produces the right verdict is verified by
  hand with the EICAR test string (expect `verdict=infected`, exit `20`) and a plain
  text file (expect `clean`, exit `0`).


Ported from the same-owner sibling workspace `sro-rs-client` (`scripts/safe-analyze.sh`,
`docker/safe-analyze/*`); the three image assets are vendored byte-for-byte.

## `stringbind.py` — the third binding sweep (#549)

Every RE unit runs two *descriptor* binding sweeps (`resinfo/if*.txt`, `res_ui/*.2dt`).
The exe's own string table is a third, and a much larger one: measured on this machine's
corpus, **2,498 of `textuisystem.txt`'s 5,321 distinct keys** are referenced by
`sro_client.exe` itself, against ~560 that both descriptor generations bind together.
So "both sweeps return zero, therefore only a screenshot can identify this string" is
usually answerable offline.

```bash
scripts/re/stringbind.py refs UIIT_MSG_GUILD_WAREHOUSE_USE     # key -> function VAs
scripts/re/stringbind.py sweep --keys .../textuisystem.txt     # bound/unbound counts
scripts/re/stringbind.py bounds --grep 'Over than'             # asserted constants
```

What a hit proves, and what it does not: `[V]` for **existence** (the shipped binary
carries the key and a function references it), `[S]` for **placement** — a reference
says the client uses the string, never which control shows it. A hit does not upgrade a
rect or a control binding.

`bounds` is the corollary: the client ships its own invariants as assertion text, so a
capacity a unit was about to file as `[U]` often has a stated origin
(`SubMentor is Over than 2` + `ApprenticeShip is Over than 5` settle the academy roster
at 7 without a screenshot). Under ADR 0009 that is the whole difference between a
sourced value and a magic number.

Input is `corpus/client/strings.tsv` (static Ghidra output, machine-local; override the
tree with `SRO_QUARANTINE_CORPUS`). Nothing is executed — it reads a TSV.
