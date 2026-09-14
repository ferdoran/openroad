#!/usr/bin/env python3
"""Index the quarantine Ghidra decompile corpus by its compiled-in source anchors.

The original binaries shipped with their assert helpers intact, so a decompiled
`FUN_<va>` that is otherwise anonymous often sits next to a literal
`"D:\\...\\ReferenceData.cpp"` plus the line number and the asserted condition.
That triple pins an anonymous function to a real `File.cpp:line`, which is what
makes a machine-local corpus queryable at all. Its output is analysis material
and stays local — see CONTRIBUTING.md, "What this repository does not publish".

Design notes:

* **Anchors are harvested from the decompiled `.c` corpus, never from the
  extractor JSON's `routing_strings[]`.** That array is routing-smell filtered
  and drops almost every `.cpp` anchor (server 53 entries / 0 `.cpp`, client
  30 / 1), so using it would silently under-report. The JSON is read only for
  the complementary `class::method()` symbol index, where it is authoritative.
* The index is a pointer, not a second home: it locates the evidence file for a
  formula, it never restates the formula.

Read-only static analysis over already-decompiled text. Nothing here executes,
downloads, or modifies a binary.

Usage:
    refgrep.py histogram                 # per-file .cpp anchor histogram
    refgrep.py anchors [--file NAME]     # file -> [line, VA, function, condition]
    refgrep.py methods [--grep PATTERN]  # class::method() symbol index
    refgrep.py search PATTERN            # grep the corpus, reporting VA + source anchor
    refgrep.py emit-index                # the full reference-index.md, to stdout

`emit-index` writes to stdout on purpose: the tool produces the index, the analyst
decides where it lands — outside this repository.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from collections import Counter, defaultdict
from pathlib import Path

# The quarantine tree is outside the repo and machine-local; override with
# SRO_QUARANTINE for a different checkout.
DEFAULT_ROOT = Path.home() / "sro-vsro-quarantine" / "ghidra-extract"

# `FUN_<helper>(<line>, "<path.cpp>"[, "<condition>"])`.
#
# Both known helpers are matched by shape rather than by address, so a third
# one shows up in the histogram instead of being silently dropped. Ghidra wraps
# these calls across lines, hence DOTALL and the tolerant whitespace.
ANCHOR_CALL = re.compile(
    r"FUN_([0-9a-fA-F]{6,8})\s*\(\s*"
    r"(0x[0-9a-fA-F]+|\d+)\s*,\s*"
    r'"((?:[^"\\]|\\.)*\.cpp)"'
    r'(?:\s*,\s*"((?:[^"\\]|\\.)*)")?'
    r"\s*\)",
    re.S,
)

# Every `.cpp` path literal, whether or not it sits in a call we can parse.
# The histogram uses this, so a call shape we do not understand still counts.
CPP_LITERAL = re.compile(r'"((?:[^"\\]|\\.)*\.cpp)"')

# `writer_0x006A3D50_FUN_006a3d50.c` -> the VA of the containing function.
# The theme prefix is optional: some files are named `0x006A58F0_FUN_...c`.
FILE_VA = re.compile(r"(?:^|_)0x([0-9a-fA-F]{6,8})_", re.I)

# The symbols are embedded inside diagnostic messages rather than stored as
# bare values — e.g. `"## Illegal ##  IGObj::SendQuestEventMessage Entered! ..."`
# — so they have to be extracted from the string, not matched against it.
METHOD_SYMBOL = re.compile(r"\b([A-Za-z_][A-Za-z0-9_]*::[A-Za-z_][A-Za-z0-9_]*)\b")


def corpus_files(root: Path) -> list[Path]:
    """Every decompiled `.c` under the themed `decompiled*` directories."""
    return sorted(
        p
        for d in sorted(root.glob("decompiled*"))
        if d.is_dir()
        for p in d.rglob("*.c")
    )


def win_basename(path: str) -> str:
    """Basename of a Windows source path as it appears in the binary."""
    return path.replace("\\\\", "\\").rsplit("\\", 1)[-1]


def win_dirname(path: str) -> str:
    return path.replace("\\\\", "\\").rsplit("\\", 1)[0]


def containing_va(path: Path) -> str:
    match = FILE_VA.search(path.name)
    return f"0x{match.group(1).upper()}" if match else "?"


class Anchor:
    __slots__ = ("cpp", "line", "helper", "condition", "va", "source")

    def __init__(self, cpp, line, helper, condition, va, source):
        self.cpp = cpp
        self.line = line
        self.helper = helper
        self.condition = condition
        self.va = va
        self.source = source

    def sort_key(self):
        return (self.cpp, self.line, self.va)


def dedupe(rows: list["Anchor"]) -> list["Anchor"]:
    """One row per distinct anchor.

    A function that belongs to several feature groups is decompiled into each
    themed directory, so the same `File.cpp:line` is seen once per copy. The
    histogram deliberately keeps the raw occurrence count (that is the number
    the corpus census is stated in); the tables show each anchor once.
    """
    seen: set[tuple] = set()
    unique: list[Anchor] = []
    for row in sorted(rows, key=Anchor.sort_key):
        key = (row.cpp, row.line, row.va, row.helper, row.condition)
        if key in seen:
            continue
        seen.add(key)
        unique.append(row)
    return unique


def parse_line_number(raw: str) -> int:
    """Anchor line numbers are emitted in hex by Ghidra, decimal by hand."""
    return int(raw, 16) if raw.lower().startswith("0x") else int(raw)


def collect(root: Path):
    """Returns (anchors, literal_histogram, unparsed_count)."""
    anchors: list[Anchor] = []
    literals: Counter[str] = Counter()
    parsed_spans = 0
    literal_total = 0

    for path in corpus_files(root):
        text = path.read_text(errors="replace")
        va = containing_va(path)
        for match in ANCHOR_CALL.finditer(text):
            anchors.append(
                Anchor(
                    cpp=win_basename(match.group(3)),
                    line=parse_line_number(match.group(2)),
                    helper=f"0x{match.group(1).upper()}",
                    condition=match.group(4),
                    va=va,
                    source=path.name,
                )
            )
            parsed_spans += 1
        for match in CPP_LITERAL.finditer(text):
            literals[win_basename(match.group(1))] += 1
            literal_total += 1

    return anchors, literals, literal_total - parsed_spans


def source_tree(root: Path) -> Counter:
    counts: Counter[str] = Counter()
    for path in corpus_files(root):
        text = path.read_text(errors="replace")
        for match in CPP_LITERAL.finditer(text):
            counts[win_dirname(match.group(1))] += 1
    return counts


def load_methods(root: Path) -> dict[str, list[tuple[str, str]]]:
    """`class::method()` symbols from the extractor JSON, per binary.

    This is the one place the JSON is authoritative: these symbols are exactly
    what its routing-smell filter keeps, and they never appear as `.cpp`
    literals in the corpus.
    """
    out: dict[str, list[tuple[str, str]]] = {}
    for js in sorted(root.glob("*.json")):
        try:
            data = json.loads(js.read_text(errors="replace"))
        except (json.JSONDecodeError, OSError):
            continue
        found: list[tuple[str, str]] = []
        # `routing_strings` is a top-level array, not a per-function field.
        for entry in data.get("routing_strings", []) or []:
            if not isinstance(entry, dict):
                continue
            value = entry.get("value")
            if not isinstance(value, str):
                continue
            for symbol in METHOD_SYMBOL.findall(value):
                found.append((symbol, entry.get("address", "")))
        if found:
            out[js.name] = sorted(set(found))
    return out


def cmd_histogram(args, root: Path) -> int:
    _, literals, _ = collect(root)
    for name, count in sorted(literals.items(), key=lambda kv: (-kv[1], kv[0])):
        print(f"{count:4}  {name}")
    print(f"\n{len(literals)} distinct .cpp files, {sum(literals.values())} occurrences")
    return 0


def cmd_anchors(args, root: Path) -> int:
    anchors, _, unparsed = collect(root)
    by_file: dict[str, list[Anchor]] = defaultdict(list)
    for anchor in anchors:
        if args.file and args.file.lower() not in anchor.cpp.lower():
            continue
        by_file[anchor.cpp].append(anchor)

    for cpp in sorted(by_file, key=lambda c: (-len(by_file[c]), c)):
        rows = dedupe(by_file[cpp])
        print(f"\n## {cpp}  ({len(rows)} distinct anchors)")
        print("| line | VA | helper | condition |")
        print("|---|---|---|---|")
        for row in rows:
            condition = f"`{row.condition}`" if row.condition else "—"
            print(f"| {row.line} | `{row.va}` | `{row.helper}` | {condition} |")
    if unparsed:
        print(
            f"\n{unparsed} .cpp literal(s) are not in a recognised anchor-call "
            "shape and appear only in the histogram.",
            file=sys.stderr,
        )
    return 0


def cmd_methods(args, root: Path) -> int:
    methods = load_methods(root)
    if not methods:
        print("no class::method() symbols found in the extractor JSON", file=sys.stderr)
        return 1
    for binary, entries in methods.items():
        shown = [
            (sym, addr)
            for sym, addr in entries
            if not args.grep or args.grep.lower() in sym.lower()
        ]
        if not shown:
            continue
        print(f"\n## {binary}  ({len(shown)} symbols)")
        for sym, addr in shown:
            print(f"  {sym:<52} {addr}")
    return 0


def cmd_search(args, root: Path) -> int:
    pattern = re.compile(args.pattern, re.I)
    anchors, _, _ = collect(root)
    by_source: dict[str, list[Anchor]] = defaultdict(list)
    for anchor in anchors:
        by_source[anchor.source].append(anchor)

    hits = 0
    for path in corpus_files(root):
        for number, line in enumerate(
            path.read_text(errors="replace").splitlines(), start=1
        ):
            if not pattern.search(line):
                continue
            hits += 1
            anchor = by_source.get(path.name)
            where = (
                f"  [{anchor[0].cpp}:{anchor[0].line}]" if anchor else ""
            )
            print(f"{path.name}:{number}{where}: {line.strip()[:120]}")
    if not hits:
        print("no matches", file=sys.stderr)
        return 1
    return 0


def cmd_emit_index(args, root: Path) -> int:
    anchors, literals, unparsed = collect(root)
    trees = source_tree(root)
    methods = load_methods(root)
    helpers = Counter(a.helper for a in anchors)
    arity = Counter((a.helper, "3-arg" if a.condition else "2-arg") for a in anchors)

    print("# Reference index — quarantine decompile corpus")
    print()
    print(
        "Generated by `scripts/re/refgrep.py emit-index`. Every row below is "
        "harvested from the decompiled `.c` corpus, not from the extractor "
        "JSON's routing-smell-filtered `routing_strings[]` (the `class::method()` "
        "section is the one exception, where the JSON is authoritative)."
    )
    print()
    print("## Original source tree")
    print()
    print("| occurrences | directory |")
    print("|---:|---|")
    for directory, count in trees.most_common():
        print(f"| {count} | `{directory}` |")
    print()
    print("## Assert helpers")
    print()
    print("| helper VA | calls | 2-arg | 3-arg |")
    print("|---|---:|---:|---:|")
    for helper, count in helpers.most_common():
        print(
            f"| `{helper}` | {count} | {arity[(helper, '2-arg')]} "
            f"| {arity[(helper, '3-arg')]} |"
        )
    print()
    print(
        "The helper address, not the argument count, is what separates the "
        "binaries: both shapes occur for the same helper."
    )
    print()
    print("## Anchor histogram")
    print()
    print("| occurrences | file |")
    print("|---:|---|")
    for name, count in sorted(literals.items(), key=lambda kv: (-kv[1], kv[0])):
        print(f"| {count} | `{name}` |")
    print()
    print(
        f"{len(literals)} distinct files, {sum(literals.values())} path-literal "
        f"occurrences, of which {len(anchors)} parse as anchor calls "
        f"({unparsed} do not)."
    )

    by_file: dict[str, list[Anchor]] = defaultdict(list)
    for anchor in anchors:
        by_file[anchor.cpp].append(anchor)
    print()
    print("## Anchors by file")
    for cpp in sorted(by_file, key=lambda c: (-len(by_file[c]), c)):
        rows = dedupe(by_file[cpp])
        print(f"\n### {cpp}")
        print()
        print("| line | containing VA | helper | condition |")
        print("|---|---|---|---|")
        for row in rows:
            condition = f"`{row.condition}`" if row.condition else "—"
            print(f"| {row.line} | `{row.va}` | `{row.helper}` | {condition} |")

    if methods:
        print()
        print("## `class::method()` symbol index")
        for binary, entries in methods.items():
            print(f"\n### {binary}")
            print()
            print("| symbol | address |")
            print("|---|---|")
            for sym, addr in entries:
                print(f"| `{sym}` | `{addr}` |")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--root",
        type=Path,
        default=Path(os.environ.get("SRO_QUARANTINE", DEFAULT_ROOT)),
        help="quarantine ghidra-extract directory (env: SRO_QUARANTINE)",
    )
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("histogram")
    anchors = sub.add_parser("anchors")
    anchors.add_argument("--file", help="restrict to files matching this substring")
    methods = sub.add_parser("methods")
    methods.add_argument("--grep", help="restrict to symbols matching this substring")
    search = sub.add_parser("search")
    search.add_argument("pattern")
    sub.add_parser("emit-index")

    args = parser.parse_args()
    root: Path = args.root
    if not root.is_dir():
        print(
            f"quarantine corpus not found at {root}\n"
            "set SRO_QUARANTINE to the ghidra-extract directory",
            file=sys.stderr,
        )
        return 2

    return {
        "histogram": cmd_histogram,
        "anchors": cmd_anchors,
        "methods": cmd_methods,
        "search": cmd_search,
        "emit-index": cmd_emit_index,
    }[args.command](args, root)


if __name__ == "__main__":
    sys.exit(main())
