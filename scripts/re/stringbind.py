#!/usr/bin/env python3
"""Third binding sweep: which UI string keys does `sro_client.exe` itself reference?

Every RE unit runs two binding sweeps over the *descriptors* — `resinfo/if*.txt`
and `res_ui/*.2dt` — and when both come back empty the unit records the string
as unidentifiable without a screenshot. That conclusion is usually wrong: the
client's own string table references the key, and the reference names the
function that uses it. The ui lane measured 2,322 referenced `textuisystem`
keys against 560 bound by both descriptor generations together (#549).

What a hit does and does not prove (this is the whole reason the tool exists):

* `[V]` for **existence** — the shipped binary contains the key and at least one
  function references it.
* `[S]` for **placement** — a reference proves the client uses the string, not
  which control displays it. A hit never upgrades a rect or a control binding.

The stronger corollary the same table supports is `bounds`: the client ships its
own invariants as assertion text (`SubMentor is Over than 2`), so a capacity or
cap that a unit was about to file as `[U]` is often a sourced constant. Under
ADR 0009 that is the difference between a stated origin and a magic number.

Input is `corpus/client/strings.tsv` — static Ghidra output that already exists
on disk (`scripts/re/ghidra/redump.sh ... index`). Nothing here executes,
downloads or modifies a binary; it reads a TSV.

Usage:
    stringbind.py refs KEY [KEY ...]      # key -> referencing function VAs
    stringbind.py sweep --keys FILE       # bound/unbound counts over a key list
    stringbind.py bounds [--grep PAT]     # assertion strings that carry a number
"""

from __future__ import annotations

import argparse
import os
import re
import sys
from pathlib import Path

# The quarantine tree is outside the repo and machine-local; override with
# SRO_QUARANTINE_CORPUS. Same shape as refgrep.py's SRO_QUARANTINE, but a
# different tree: the decompiles live under ghidra-extract, the tables here.
DEFAULT_CORPUS = Path.home() / "sro-vsro-quarantine" / "re-corpus"
STRINGS_TSV = Path("corpus") / "client" / "strings.tsv"

# A textuisystem key as it appears in the data: SHOUTING_SNAKE_CASE, at least
# two segments. Deliberately not anchored to `UIIT_`/`UIO_` — the corpus also
# carries `TC_`, `SN_` and bare families, and an over-narrow filter is how a
# sweep silently under-reports.
KEY_SHAPE = re.compile(r"^[A-Z][A-Z0-9]*(?:_[A-Z0-9]+)+$")

# The assertion-bound shape: a phrase the client asserts with a literal number
# in it. `Illigal` is the original's own spelling and is kept verbatim.
BOUND_HINTS = ("Over than", "Illigal", "Illegal", "Invalid", "Max", "Limit")
BOUND_NUMBER = re.compile(r"\d+")


class StringTable:
    """`value -> referencing function VAs`, read from the exe's string table.

    A value can appear at several addresses (the compiler does not always pool
    them), so the references of every row with the same text are merged: asking
    "who references this key" must not depend on which copy was found first.
    """

    def __init__(self, rows: list[tuple[str, list[str]]]):
        self.refs: dict[str, list[str]] = {}
        for value, funcs in rows:
            merged = self.refs.setdefault(value, [])
            for func in funcs:
                if func not in merged:
                    merged.append(func)

    def __len__(self) -> int:
        return len(self.refs)

    def references(self, key: str) -> list[str]:
        """Functions referencing `key`, or `[]` when the exe does not carry it."""
        return list(self.refs.get(key, []))

    def keys(self) -> list[str]:
        """Every value that looks like a UI string key."""
        return sorted(v for v in self.refs if KEY_SHAPE.match(v))


def parse_strings_tsv(lines) -> list[tuple[str, list[str]]]:
    """`addr, nrefs, reffuncs, value` rows -> `(value, [func VA])`.

    The value column is taken as the remainder of the line rather than
    `split("\t")[3]`, because a shipped string may itself contain a tab; the
    first three columns cannot.
    """
    rows = []
    for raw in lines:
        line = raw.rstrip("\n")
        if not line or line.startswith("addr\t"):
            continue
        parts = line.split("\t", 3)
        if len(parts) < 4:
            continue
        _addr, _nrefs, reffuncs, value = parts
        funcs = [f for f in reffuncs.split(",") if f]
        rows.append((value, funcs))
    return rows


def load_table(corpus: Path) -> StringTable:
    path = corpus / STRINGS_TSV
    with path.open(encoding="utf-8", errors="replace") as handle:
        return StringTable(parse_strings_tsv(handle))


def read_key_source(path: Path) -> list[str]:
    """Lines of a key list or a shipped `textdata` file.

    The shipped `textuisystem.txt` is **UTF-16-LE with a BOM** — reading it as
    UTF-8 does not fail, it silently yields NUL-separated bytes that match no
    key at all, so the sweep reports zero keys and looks like a corpus problem.
    Decode by BOM, fall back to UTF-8 for hand-made key lists.
    """
    data = path.read_bytes()
    if data[:2] in (b"\xff\xfe", b"\xfe\xff"):
        text = data.decode("utf-16", errors="replace")
    else:
        text = data.decode("utf-8", errors="replace")
    return text.lstrip("\ufeff").splitlines()


def extract_keys(lines) -> list[str]:
    """Key names out of a key list or a raw `textuisystem.txt`.

    textuisystem rows are tab-separated with the key in the second column, but
    a hand-made key list is one key per line — so every field of every line is
    tried against the key shape and the first match wins. Order is preserved
    and duplicates are dropped, so a sweep's denominator is the distinct key
    count the unit doc will quote.
    """
    seen: dict[str, None] = {}
    for raw in lines:
        for field in raw.strip().split("\t"):
            field = field.strip()
            if KEY_SHAPE.match(field):
                seen.setdefault(field, None)
                break
    return list(seen)


def bound_strings(table: StringTable, grep: str | None = None) -> list[tuple[str, list[str]]]:
    """Assertion strings that carry a literal number — sourced constants.

    This is `LOOP_PROTOCOL.md` §7b made executable: before filing a capacity or
    a cap as `[U]`, ask the binary whether it asserts the bound itself.
    """
    out = []
    for value, funcs in sorted(table.refs.items()):
        if not BOUND_NUMBER.search(value):
            continue
        if not any(hint in value for hint in BOUND_HINTS):
            continue
        if grep and grep.lower() not in value.lower():
            continue
        out.append((value, funcs))
    return out


def cmd_refs(args, table: StringTable) -> int:
    missing = 0
    for key in args.keys:
        funcs = table.references(key)
        if funcs:
            print(f"{key}\t{len(funcs)}\t{','.join(funcs)}")
        else:
            missing += 1
            print(f"{key}\t0\t-")
    # A key the exe does not carry is a finding, not an error: report it in the
    # exit code so a script can branch, but still print every row.
    return 1 if missing else 0


def cmd_sweep(args, table: StringTable) -> int:
    keys = extract_keys(read_key_source(args.keys))
    bound = [k for k in keys if table.references(k)]
    print(f"keys read:      {len(keys)}")
    print(f"exe-referenced: {len(bound)}")
    print(f"unreferenced:   {len(keys) - len(bound)}")
    if args.list_unbound:
        for key in keys:
            if not table.references(key):
                print(key)
    return 0


def cmd_bounds(args, table: StringTable) -> int:
    for value, funcs in bound_strings(table, args.grep):
        print(f"{value}\t{','.join(funcs)}")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--corpus",
        type=Path,
        default=Path(os.environ.get("SRO_QUARANTINE_CORPUS", DEFAULT_CORPUS)),
        help="quarantine re-corpus directory (env: SRO_QUARANTINE_CORPUS)",
    )
    sub = parser.add_subparsers(dest="command", required=True)
    refs = sub.add_parser("refs")
    refs.add_argument("keys", nargs="+")
    sweep = sub.add_parser("sweep")
    sweep.add_argument("--keys", type=Path, required=True, help="key list or textuisystem.txt")
    sweep.add_argument("--list-unbound", action="store_true")
    bounds = sub.add_parser("bounds")
    bounds.add_argument("--grep", help="restrict to strings containing this substring")

    args = parser.parse_args()
    path = args.corpus / STRINGS_TSV
    if not path.is_file():
        print(
            f"string table not found at {path}\n"
            "set SRO_QUARANTINE_CORPUS to the re-corpus directory",
            file=sys.stderr,
        )
        return 2
    table = load_table(args.corpus)
    return {"refs": cmd_refs, "sweep": cmd_sweep, "bounds": cmd_bounds}[args.command](
        args, table
    )


if __name__ == "__main__":
    sys.exit(main())
