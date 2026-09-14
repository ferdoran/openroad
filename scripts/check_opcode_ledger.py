#!/usr/bin/env python3
"""Opcode coverage ledger consistency check (EP-04).

The `packets!` macro in `packets/src/lib.rs` is the single source of truth for
which opcodes the client wires. `docs/protocol/opcodes.md` is the human-readable
ledger. This gate fails when the two disagree, so the ledger cannot silently
drift from the code.

It also owns the *count*. A hand-maintained "Wired opcodes: N" line in the
ledger was a serialization point: every net PR had to bump it, so every net PR
collided with every other net PR on that one line (#558) — and because nothing
checked it, main carried a wrong N on 9 of the 51 commits that touched the file.
The count is therefore derived here and printed, never stored; `no_hardcoded_count`
fails the gate if such a line reappears.

Run: `python3 scripts/check_opcode_ledger.py`
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MACRO = ROOT / "packets" / "src" / "lib.rs"
LEDGER = ROOT / "docs" / "protocol" / "opcodes.md"

# `0x7021 => MovementRequest,`
MACRO_ENTRY = re.compile(r"0x([0-9A-Fa-f]{4})\s*=>\s*(\w+)")
# `| `0x7021` | MovementRequest | ...` — opcode in the first table column,
# optionally wrapped in backticks.
LEDGER_ROW = re.compile(r"^\|\s*`?0x([0-9A-Fa-f]{4})`?\s*\|\s*`?(\w+)`?\s*\|")


def macro_opcodes() -> dict[str, str]:
    src = MACRO.read_text()
    block = re.search(r"packets!\s*\{(.*?)\n\}", src, re.S)
    if not block:
        sys.exit("check_opcode_ledger: could not find the packets! macro block")
    return {op.upper(): name for op, name in MACRO_ENTRY.findall(block.group(1))}


def ledger_opcodes() -> dict[str, str]:
    out: dict[str, str] = {}
    for line in LEDGER.read_text().splitlines():
        m = LEDGER_ROW.match(line)
        if m:
            out[m.group(1).upper()] = m.group(2)
    return out


# A prose (non-table) line that states the wired-opcode total, in either word
# order — `**Wired opcodes:** 176` or `176 wired opcodes` — plus the older
# `N (this table)` phrasing. Table rows are exempt: they are the data itself.
COUNT_CLAIM = re.compile(
    r"\b\d+\b[^|]{0,24}?\bwired\s+opcodes?\b"
    r"|\bwired\s+opcodes?\b[^|]{0,24}?\b\d+\b"
    r"|\b\d+\b[^|]{0,8}?\(this table\)",
    re.I,
)


def no_hardcoded_count(text: str) -> list[str]:
    """Prose lines that hardcode the wired-opcode total. Empty list = clean."""
    return [
        line.strip()
        for line in text.splitlines()
        if not line.lstrip().startswith("|") and COUNT_CLAIM.search(line)
    ]


def main() -> int:
    macro = macro_opcodes()
    ledger = ledger_opcodes()

    hardcoded = no_hardcoded_count(LEDGER.read_text())
    if hardcoded:
        print("check_opcode_ledger: the ledger hardcodes a wired-opcode count\n")
        for line in hardcoded:
            print(f"  {line}")
        print(
            "\nThe count is derived by this script and printed below; it must not be\n"
            "written into docs/protocol/opcodes.md, where every net PR would have to\n"
            "bump it and collide with every other net PR (#558)."
        )
        return 1

    missing = sorted(set(macro) - set(ledger))  # wired but not in the ledger
    extra = sorted(set(ledger) - set(macro))  # in the ledger but not wired
    renamed = sorted(op for op in macro.keys() & ledger.keys() if macro[op] != ledger[op])

    ok = not (missing or extra or renamed)
    if ok:
        print(f"check_opcode_ledger: OK ({len(macro)} opcodes match)")
        return 0

    print("check_opcode_ledger: ledger and packets! macro disagree\n")
    for op in missing:
        print(f"  wired but missing from ledger: 0x{op} => {macro[op]}")
    for op in extra:
        print(f"  in ledger but not wired: 0x{op} => {ledger[op]}")
    for op in renamed:
        print(f"  name mismatch 0x{op}: macro={macro[op]} ledger={ledger[op]}")
    print("\nUpdate docs/protocol/opcodes.md to match packets/src/lib.rs.")
    return 1


if __name__ == "__main__":
    sys.exit(main())
