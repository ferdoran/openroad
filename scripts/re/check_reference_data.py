#!/usr/bin/env python3
"""Textdata column-index drift check (EP-02).

Textdata columns are positional: `itemdata_5000.txt` is a tab-separated dump
with no header row, so our parsers address fields by number. If a column moves
upstream, or one of our indices is off by one, every row mis-parses silently —
no crash, just wrong data. This gate makes that a build failure.

The authoritative order is not ours to choose. `SR_Db2Media/Settings.cs` is the
tool that *produces* those .txt files, one SQL `SELECT` per table, and a
column's 0-based position in the SELECT clause is its column index in the
export. This check parses that clause and asserts our indices agree with it,
matched by field name rather than by position (matching positionally would
assume the very thing under test).

The reference tree is a separate checkout the user supplies, so when it is
absent the check reports that and exits 0 rather than failing a build for a
missing optional input.

Run: `python3 scripts/re/check_reference_data.py`
"""

import os
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
TEXTDATA = ROOT / "client" / "src" / "assets" / "textdata"

# `Setup.Add(new Query2Path() { Path = "characterdata_5000.txt", Query = "SELECT ..." });`
QUERY2PATH = re.compile(r'Path\s*=\s*"([^"]+)".*?Query\s*=\s*"([^"]+)"', re.S)
SELECT_CLAUSE = re.compile(r"^\s*SELECT\s+(.*?)\s+FROM\s", re.S | re.I)
# Shard suffix: characterdata_5000 -> characterdata
SHARD_SUFFIX = re.compile(r"_\d+$")

RUST_COMMENT = re.compile(r"//[^\n]*")
RUST_VARIANT = re.compile(r"^\s*(\w+)\s*(?:=\s*(\d+))?\s*,\s*$")
# `StatRange::Durability => (63, 64),`
STAT_RANGE_ARM = re.compile(r"StatRange::(\w+)\s*=>\s*\((\d+)\s*,\s*(\d+)\)")

# Our enum name -> (file, table it describes).
OUR_ENUMS = [
    ("ItemdataFields", "itemdata.rs", "itemdata"),
    ("ChardataFields", "characterdata.rs", "characterdata"),
    ("SkilldataFields", "skilldata.rs", "skilldata"),
]

# Renames that no mechanical rule derives. Keys and values are already
# normalized (lowercased, `128` stripped). Each maps a Settings.cs column onto
# the name our parsers use for the same column.
ALIASES = {
    "assocfileobj": "resourcepath",
    "assocfiledrop": "dropresourcepath",
    "assocfileicon": "iconpath",
    "lvl": "level",
    "chargender": "gender",
    "reqgender": "gender",
    "orgobjcodename": "orgobjcode",
}


def split_columns(clause: str) -> list[str]:
    """Split a SELECT clause on its *top-level* commas.

    Splitting on every comma is wrong and quietly so: the itemdata query wraps
    many columns in `REPLACE(_RefObjItem.ERInc, '.', '')`, whose two inner
    commas each invent a column and shift every index after it. That turns 160
    real columns into 298 and produces confident, entirely false drift reports.
    """
    parts: list[str] = []
    depth = 0
    quoted = False
    current: list[str] = []
    for char in clause:
        if char == "'":
            quoted = not quoted
        elif not quoted:
            if char == "(":
                depth += 1
            elif char == ")":
                depth -= 1
            elif char == "," and depth == 0:
                parts.append("".join(current))
                current = []
                continue
        current.append(char)
    if "".join(current).strip():
        parts.append("".join(current))
    return parts


# The `Table.Column` identifier inside a possibly function-wrapped expression.
QUALIFIED_COLUMN = re.compile(r"(\w+)\.(\w+)")


def column_name(expression: str) -> str:
    """The column an expression selects, unwrapping any SQL function call.

    `REPLACE(_RefObjItem.PAttackMin_L, '.', '')` selects `PAttackMin_L`. The
    quoted `'.'` arguments cannot match, since a quote is not a word character.
    """
    m = QUALIFIED_COLUMN.search(expression)
    return m.group(2) if m else expression.strip()


# `StatRange` variant -> the Settings.cs abbreviation of its column pair, whose
# `_L`/`_U` suffixes are the lower and upper bound. These names are abbreviated
# past guessing (`PAR` = physical absorption rate, `PDStr` = physical defence
# strength, `CHR` = critical hit rate), so they are mapped explicitly rather
# than derived.
STAT_RANGE_ALIASES = {
    "durability": "dur",
    "defense": "pd",
    "phyabsorb": "par",
    "blockrate": "br",
    "magabsorb": "mar",
    "armorphyreinforce": "pdstr",
    "armormagreinforce": "mdint",
    "phyatkmin": "pattackmin",
    "phyatkmax": "pattackmax",
    "magatkmin": "mattackmin",
    "magatkmax": "mattackmax",
    "phyreinforcemin": "pastrmin",
    "phyreinforcemax": "pastrmax",
    "magreinforcemin": "maint_min",
    "magreinforcemax": "maint_max",
    "attackrate": "hr",
    "critical": "chr",
}


def normalize(name: str) -> str:
    """Reduce a column name to a form both sides agree on.

    Mechanical differences (`_RefObjCommon.` prefixes, the `128` string-length
    suffix, and casing like `TypeID` vs `TypeId` or `MaxHP` vs `MaxHp`) are
    dissolved by rule; only genuine renames need `ALIASES`.
    """
    name = column_name(name).strip().lower()
    name = re.sub(r"_?128$", "", name)
    return ALIASES.get(name, name)


def settings_path() -> Path:
    refs = Path(os.environ.get("SRO_REFS_PATH", Path.home() / "sro-refs"))
    return refs / "SR_Db2Media" / "SR_Db2Media" / "Settings.cs"


def settings_columns(source: str) -> dict[str, dict[str, int]]:
    """table -> {normalized column name: 0-based index}.

    A `SELECT *` table maps to an **empty** dict rather than being dropped: its
    order lives in the DB schema, so there is nothing to assert against, but the
    caller must still be able to tell "exported without a column list" from
    "not exported at all". Collapsing the two would report a table that vanished
    from Settings.cs as merely unasserted.
    """
    tables: dict[str, dict[str, int]] = {}
    for line in source.splitlines():
        m = QUERY2PATH.search(line)
        if not m:
            continue
        path, query = m.group(1), m.group(2)
        table = SHARD_SUFFIX.sub("", Path(path).stem.lower())
        clause = SELECT_CLAUSE.match(query)
        if not clause:
            continue
        columns = split_columns(clause.group(1))
        if any(c.strip() == "*" for c in columns):
            tables.setdefault(table, {})
            continue
        # Shards of one table repeat the same query; first one wins.
        tables.setdefault(table, {normalize(c): i for i, c in enumerate(columns)})
    return tables


def rust_enum(source: str, enum_name: str) -> dict[str, int]:
    """normalized field name -> index, honouring C-style implicit successors.

    `TypeId1 = 9, TypeId2, TypeId3, TypeId4` assigns 10/11/12 without writing
    them, and those are exactly the fields worth asserting, so a regex for
    `name = digits` alone would silently check less than it appears to.
    """
    body = re.search(rf"enum\s+{enum_name}\s*\{{(.*?)\n\}}", source, re.S)
    if not body:
        sys.exit(f"check_reference_data: enum {enum_name} not found")
    fields: dict[str, int] = {}
    nxt = 0
    for line in RUST_COMMENT.sub("", body.group(1)).splitlines():
        m = RUST_VARIANT.match(line)
        if not m:
            continue
        name, explicit = m.group(1), m.group(2)
        index = int(explicit) if explicit is not None else nxt
        fields[normalize(name)] = index
        nxt = index + 1
    return fields


def stat_range_columns(source: str) -> dict[str, tuple[int, int]]:
    """`StatRange` variant -> its (lower, upper) column pair."""
    return {
        normalize(name): (int(lo), int(hi))
        for name, lo, hi in STAT_RANGE_ARM.findall(source)
    }


def compare_table(
    table: str, ours: dict[str, int], theirs: dict[str, int]
) -> tuple[list[str], list[str], int]:
    """Assert our indices against the authoritative ones, matched by name.

    Returns (mismatches, unchecked, checked). Fields we index but Settings.cs
    does not name are reported as unchecked rather than as drift — an absent
    name is a gap in the mapping, not evidence of a wrong index.
    """
    mismatches: list[str] = []
    unchecked: list[str] = []
    checked = 0
    for field, index in sorted(ours.items(), key=lambda kv: kv[1]):
        if field not in theirs:
            unchecked.append(f"{table}.{field}: not named in Settings.cs")
            continue
        checked += 1
        if theirs[field] != index:
            mismatches.append(
                f"col mismatch: {table}.{field} ours={index} settings={theirs[field]}"
            )
    return mismatches, unchecked, checked


def compare_stat_ranges(
    pairs: dict[str, tuple[int, int]], theirs: dict[str, int]
) -> tuple[list[str], list[str], int]:
    """Assert each `(lower, upper)` column pair against its `_L`/`_U` columns.

    Checked as a pair rather than as two fields, because the failure that
    matters here is the bounds being swapped or one of them straying onto a
    neighbouring stat — both of which keep every index individually plausible.
    """
    mismatches: list[str] = []
    unchecked: list[str] = []
    checked = 0
    for variant, (lo, hi) in sorted(pairs.items(), key=lambda kv: kv[1]):
        alias = STAT_RANGE_ALIASES.get(variant)
        if alias is None:
            unchecked.append(f"itemdata.StatRange::{variant}: no Settings.cs mapping")
            continue
        want_lo, want_hi = theirs.get(f"{alias}_l"), theirs.get(f"{alias}_u")
        if want_lo is None or want_hi is None:
            unchecked.append(
                f"itemdata.StatRange::{variant}: {alias}_L/_U not in Settings.cs"
            )
            continue
        checked += 2
        if (lo, hi) != (want_lo, want_hi):
            mismatches.append(
                f"col mismatch: itemdata.StatRange::{variant} "
                f"ours=({lo},{hi}) settings=({want_lo},{want_hi})"
            )
    return mismatches, unchecked, checked


def main() -> int:
    settings = settings_path()
    if not settings.is_file():
        print(f"check_reference_data: SKIPPED (no reference source at {settings})")
        print("  set SRO_REFS_PATH to a checkout of JellyBitz/SR_Db2Media to enable")
        return 0

    tables = settings_columns(settings.read_text(encoding="utf-8", errors="replace"))

    mismatches: list[str] = []
    unchecked: list[str] = []
    checked = 0

    for enum_name, filename, table in OUR_ENUMS:
        source = (TEXTDATA / filename).read_text(encoding="utf-8")
        ours = rust_enum(source, enum_name)
        if table not in tables:
            # Not merely unasserted: the export we derive authority from no
            # longer mentions this table, so the reference tree itself moved.
            unchecked.append(
                f"{table}: no export declared in Settings.cs at all — "
                f"{len(ours)} of our indices unasserted, and the reference "
                f"checkout may be a different version than this check expects"
            )
            continue
        if not tables[table]:
            unchecked.append(
                f"{table}: no explicit column list in Settings.cs "
                f"(SELECT *) — {len(ours)} of our indices unasserted"
            )
            continue
        table_mismatches, table_unchecked, table_checked = compare_table(
            table, ours, tables[table]
        )
        mismatches += table_mismatches
        unchecked += table_unchecked
        checked += table_checked

        if table == "itemdata":
            pair_mismatches, pair_unchecked, pair_checked = compare_stat_ranges(
                stat_range_columns(source), tables[table]
            )
            mismatches += pair_mismatches
            unchecked += pair_unchecked
            checked += pair_checked

    if mismatches:
        print("check_reference_data: textdata column indices drifted\n")
        for line in mismatches:
            print(f"  {line}")
        print("\nOur indices must match the SELECT order in SR_Db2Media/Settings.cs.")
        return 1

    print(f"check_reference_data: OK ({checked} column indices match)")
    for line in unchecked:
        print(f"  unchecked: {line}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
