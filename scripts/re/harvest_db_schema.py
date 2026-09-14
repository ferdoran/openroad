#!/usr/bin/env python3
"""Persistence-contract harvest for the #93 stub server, from go-sro source.

The real SRO database ships as trojaned `.bak`/`.mdf` dumps, so the stub server's
schema is reconstructed by *statically reading plain text*: go-sro's Go structs
plus the SQL it embeds as string constants, and the plain-text certification SQL.
Nothing is ever restored or executed. See
`docs/re/machinery/db-schema-harvest.md` for the design and the citations.

What is recoverable, and what is honestly not:

* Tables whose SQL constants carry an explicit column list (`CHAR`, `ITEM`,
  `INVENTORY`, `GAME_SERVER_REGION`, ...) yield real DB column names, and FKs are
  visible from the `FK_*` naming convention.
* Tables read with `SELECT *` (`ITEMDATA`, `CHAR_REF_DATA`) carry **no column
  names in the source at all**. What *is* recoverable is the physical column
  *order*, from the positional `.Scan(&x.Field, ...)` list that consumes the
  star select — so those columns are emitted in ordinal order with
  `db_column: UNKNOWN` rather than guessed.
* The Go-field -> DB-column map is a documented name heuristic (see
  `match_field`); a column with no confident match is emitted as
  `go_field: UNMAPPED` instead of being paired up hopefully.

Run: `python3 scripts/re/harvest_db_schema.py [--source DIR] [--sql FILE] [--out FILE]`
"""

import argparse
import re
import sys
from pathlib import Path
from typing import Optional

# Paths that must never be opened: restoring or attaching a SRO database dump is
# the one locked security rule this tool exists to avoid.
FORBIDDEN_SUFFIXES = (".bak", ".mdf", ".ldf")
ALLOWED_SUFFIXES = (".go", ".sql")

# Tables the design doc names as owned by the `systems-go-sro-db-harvest` unit
# and not yet harvested; emitted explicitly so the gap is visible, not silent.
MISSING_TABLES = ("GUILD", "PARTY", "CONSIGNMENT_LIST", "CHAR_QUEST")

# `type Char struct {` ... `}`
GO_STRUCT = re.compile(r"^type\s+(\w+)\s+struct\s*\{", re.MULTILINE)
# `	Level       int` / `	PosX  float32` — a named field with a type.
GO_FIELD = re.compile(r"^\s+([A-Z]\w*)\s+([\w\.\[\]\*]+)\s*(?://.*)?$")
# `	RefObject` / `	SRObject` — an embedded struct (a bare type, no field name).
GO_EMBED = re.compile(r"^\s+([A-Z]\w*)\s*(?://.*)?$")
# Any double-quoted Go string literal (SQL lives in `const` blocks as these).
GO_STRING = re.compile(r'"([^"\\]*(?:\\.[^"\\]*)*)"')
# `&refItem.CodeName` inside a `.Scan(` argument list.
SCAN_ARG = re.compile(r"&(\w+)\.(\w+)")

# `` `SRO_SHARD`.`CHAR` `` with optional alias, or bare SRO_SHARD.CHAR
TABLE_REF = re.compile(
    r"`?(?P<schema>SRO_[A-Z]+)`?\s*\.\s*`?(?P<table>\w+)`?"
    r"(?:\s+(?:AS\s+)?(?P<alias>(?!ON\b|WHERE\b|INNER\b|LEFT\b|SET\b|VALUES\b)[A-Za-z]\w*))?",
    re.IGNORECASE,
)
# `CREATE TABLE [dbo].[Shard](` for the plain-text certification SQL.
MSSQL_TABLE = re.compile(r"CREATE\s+TABLE\s+\[dbo\]\.\[(\w+)\]", re.IGNORECASE)
# `[Name] [varchar](64) NOT NULL,`
MSSQL_COLUMN = re.compile(
    r"^\s*\[(?P<col>\w+)\]\s*\[(?P<type>\w+)\](?P<len>\([^)]*\))?(?P<rest>.*)$"
)

SQL_KEYWORDS = ("SELECT ", "INSERT INTO", "UPDATE ", "DELETE FROM")


class HarvestError(Exception):
    """A refused input path or an unreadable source tree."""


def guard_path(path: Path) -> Path:
    """Refuse database dumps outright; only plain-text `.go`/`.sql` may be read.

    This is the tool's whole safety premise, so it is a hard failure rather than
    a warning, and it is checked on the suffix before any open() happens.
    """
    suffix = path.suffix.lower()
    if suffix in FORBIDDEN_SUFFIXES:
        raise HarvestError(
            f"refusing to read {path}: {suffix} is a database dump. "
            "This tool reconstructs the schema from source precisely so no "
            "dump is ever restored or attached (see docs/re/machinery/"
            "db-schema-harvest.md)."
        )
    if suffix not in ALLOWED_SUFFIXES:
        raise HarvestError(
            f"refusing to read {path}: only {', '.join(ALLOWED_SUFFIXES)} are allowed"
        )
    return path


def read_text(path: Path) -> str:
    """Read a guarded plain-text source, honouring its BOM.

    The go sources are UTF-8, but SRO's shipped SQL is UTF-16LE with a BOM (the
    same encoding as its textdata), and decoding that as UTF-8 yields NUL-riddled
    text that silently matches nothing.
    """
    guard_path(path)
    raw = path.read_bytes()
    if raw.startswith((b"\xff\xfe", b"\xfe\xff")):
        return raw.decode("utf-16", errors="replace")
    return raw.decode("utf-8-sig", errors="replace")


def parse_go_structs(text: str) -> dict:
    """Struct name -> `{"fields": [(field, go_type, line)], "embeds": [name]}`.

    Embedded types matter: `RefItem` embeds `RefObject`, which is where `ID` and
    `CodeName` — the first columns `ITEMDATA`'s star select scans — actually get
    their types, so they have to be resolved rather than skipped.
    """
    structs = {}
    lines = text.split("\n")
    for match in GO_STRUCT.finditer(text):
        name = match.group(1)
        start = text[: match.start()].count("\n") + 1  # 1-based line of `type X`
        fields, embeds = [], []
        for offset, line in enumerate(lines[start:], start=start + 1):
            if line.startswith("}"):
                break
            field = GO_FIELD.match(line)
            if field:
                fields.append((field.group(1), field.group(2), offset))
                continue
            embedded = GO_EMBED.match(line)
            if embedded:
                embeds.append(embedded.group(1))
        structs[name] = {"fields": fields, "embeds": embeds}
    return structs


def field_types(struct: str, structs: dict, seen=None) -> dict:
    """Field -> Go type for a struct, following its embedded structs."""
    if seen is None:
        seen = set()
    if not struct or struct in seen or struct not in structs:
        return {}
    seen.add(struct)
    types = {}
    for embedded in structs[struct]["embeds"]:
        types.update(field_types(embedded, structs, seen))
    for field, go_type, _ in structs[struct]["fields"]:
        types[field] = go_type
    return types


def extract_sql_literals(text: str) -> list:
    """[(sql, line)] for every Go string literal that looks like a statement."""
    found = []
    for match in GO_STRING.finditer(text):
        sql = match.group(1)
        upper = sql.upper()
        if any(keyword in upper for keyword in SQL_KEYWORDS):
            line = text[: match.start()].count("\n") + 1
            found.append((sql, line))
    return found


def _split_columns(blob: str) -> list:
    """Column names out of a `SELECT`/`INSERT` list, alias prefixes stripped."""
    columns = []
    for raw in blob.split(","):
        token = raw.strip()
        if not token or "(" in token or token == "*":
            continue
        token = token.split()[0]  # drop `AS x` tails
        if "." in token:  # `inv.SLOT` -> `SLOT`
            token = token.rsplit(".", 1)[1]
        token = token.strip("`[]")
        if token and re.fullmatch(r"\w+", token) and not token.isdigit():
            columns.append(token.upper())
    return columns


def parse_sql(sql: str) -> list:
    """[(schema, table, [columns], star)] for the tables a statement touches.

    Aliases matter: `SELECT inv.SLOT, it.VARIANCE FROM INVENTORY inv INNER JOIN
    ITEM AS it` must attribute SLOT to INVENTORY and VARIANCE to ITEM, not dump
    both on the first table.
    """
    refs = list(TABLE_REF.finditer(sql))
    if not refs:
        return []

    alias_map = {}
    for ref in refs:
        alias = ref.group("alias")
        if alias:
            alias_map[alias.lower()] = ref.group("table").upper()

    upper = sql.upper()
    star = "SELECT *" in upper or "SELECT COUNT(*)" in upper

    # Which column list belongs to which table.
    per_table = {ref.group("table").upper(): [] for ref in refs}
    primary = refs[0].group("table").upper()

    insert = re.search(r"INSERT\s+INTO\s+[^(]*\(([^)]*)\)", sql, re.IGNORECASE)
    update = re.search(r"UPDATE\s+.*?\sSET\s+(.*?)(?:\sWHERE\s|$)", sql, re.IGNORECASE)
    select = re.search(r"SELECT\s+(.*?)\s+FROM\s", sql, re.IGNORECASE | re.DOTALL)

    if insert:
        per_table[primary].extend(_split_columns(insert.group(1)))
    elif update:
        assignments = ",".join(
            part.split("=")[0] for part in update.group(1).split(",") if "=" in part
        )
        per_table[primary].extend(_split_columns(assignments))
    elif select and not star:
        for raw in select.group(1).split(","):
            token = raw.strip()
            if not token or "(" in token:
                continue
            owner = primary
            if "." in token:
                prefix = token.rsplit(".", 1)[0].strip().strip("`[]").lower()
                owner = alias_map.get(prefix, primary)
            for column in _split_columns(token):
                per_table.setdefault(owner, []).append(column)

    schema_of = {ref.group("table").upper(): ref.group("schema").upper() for ref in refs}
    return [
        (schema_of.get(table, "UNKNOWN"), table, columns, star and table == primary)
        for table, columns in per_table.items()
    ]


def parse_scan_order(text: str) -> dict:
    """Receiver -> [field] in `.Scan()` order, i.e. physical column order.

    For a `SELECT *` table this is the only source-visible ordering, so it is how
    `ITEMDATA` gets an ordered contract without inventing column names.
    """
    orders = {}
    for match in re.finditer(r"\.Scan\(", text):
        depth, index = 1, match.end()
        while index < len(text) and depth:
            if text[index] == "(":
                depth += 1
            elif text[index] == ")":
                depth -= 1
            index += 1
        args = SCAN_ARG.findall(text[match.end() : index])
        if not args:
            continue
        receiver = args[0][0]
        fields = [field for holder, field in args if holder == receiver]
        if len(fields) > len(orders.get(receiver, [])):
            orders[receiver] = fields
    return orders


def parse_mssql_tables(text: str) -> dict:
    """Table -> {column: (db_type, nullable, line)} from plain-text MSSQL DDL."""
    tables = {}
    current = None
    for number, line in enumerate(text.split("\n"), start=1):
        header = MSSQL_TABLE.search(line)
        if header:
            current = header.group(1).upper()
            tables[current] = {}
            continue
        if current is None:
            continue
        if line.strip().startswith(")") or line.strip().upper().startswith("GO"):
            current = None
            continue
        column = MSSQL_COLUMN.match(line)
        if column:
            db_type = column.group("type").upper()
            if column.group("len"):
                db_type += column.group("len")
            nullable = "NOT NULL" not in column.group("rest").upper()
            tables[current][column.group("col").upper()] = (db_type, nullable, number)
    return tables


def _normalize(name: str) -> str:
    return name.replace("_", "").lower()


def match_field(column: str, fields: dict, table: str) -> str:
    """Best Go field for a DB column, or `UNMAPPED`.

    go-sro renames freely between struct and column, and the `.Scan()` mapping is
    positional so it cannot be read directly. These rules cover the documented
    cases (`Level -> CURRENT_LEVEL`, `Str -> STRENGTH`, `IsDeleting -> DELETING`,
    `User -> FK_USER`) and refuse to guess beyond them.
    """
    target = _normalize(column)
    if target in fields:
        return fields[target]

    # `FK_USER` -> `User`, `CURRENT_LEVEL` -> `Level`, `CHAR_NAME` -> `Name`.
    for prefix in ("fk", "current", _normalize(table)):
        if prefix and target.startswith(prefix):
            stripped = target[len(prefix) :]
            if stripped in fields:
                return fields[stripped]

    # `DELETING` -> `IsDeleting` (Go booleans carry the `Is` prefix).
    if ("is" + target) in fields:
        return fields["is" + target]

    # `STRENGTH` -> `Str`, `INTELLECT` -> `Int`: the struct abbreviates. Only
    # accept a unique prefix match so `EXP` cannot swallow `SkillExp`.
    candidates = [
        field for normalized, field in fields.items() if target.startswith(normalized)
    ]
    if len(candidates) == 1:
        return candidates[0]

    return "UNMAPPED"


def harvest(source: Path, cert_sql: Optional[Path]) -> dict:
    """Build the contract: table -> ordered column records with provenance."""
    model_dir = source / "model"
    if not model_dir.is_dir():
        raise HarvestError(f"no model/ directory under {source}")

    go_files = sorted(model_dir.glob("*.go"))
    if not go_files:
        raise HarvestError(f"no .go sources under {model_dir}")

    structs: dict = {}
    struct_lines: dict = {}
    tables: dict = {}
    scan_orders: dict = {}
    star_tables: dict = {}

    for path in go_files:
        text = read_text(path)
        relative = f"model/{path.name}"
        for name, entry in parse_go_structs(text).items():
            structs[name] = entry
            struct_lines[name] = relative
        for receiver, fields in parse_scan_order(text).items():
            scan_orders.setdefault(receiver, (fields, relative))
        for sql, line in extract_sql_literals(text):
            for schema, table, columns, star in parse_sql(sql):
                entry = tables.setdefault(
                    table, {"schema": schema, "columns": {}, "star": False}
                )
                if star:
                    entry["star"] = True
                    star_tables[table] = (relative, line)
                for column in columns:
                    entry["columns"].setdefault(column, (relative, line))

    cert_tables = parse_mssql_tables(read_text(cert_sql)) if cert_sql else {}

    contract = {}
    for table, entry in sorted(tables.items()):
        struct = _pick_struct(table, structs)
        types = field_types(struct, structs)
        fields = {_normalize(f): f for f in types}
        ddl = cert_tables.get(table, {})

        records = []
        for column, (relative, line) in sorted(entry["columns"].items()):
            go_field = match_field(column, fields, table)
            db_type, nullable, _ = ddl.get(column, ("UNKNOWN", None, 0))
            records.append(
                {
                    "name": column,
                    "go_field": go_field,
                    "go_type": types.get(go_field, "UNKNOWN"),
                    "db_type": db_type,
                    "nullable": nullable,
                    "fk": column.startswith("FK_"),
                    "provenance": f"{relative}:{line}",
                }
            )

        # A `SELECT *` table has no column names in the source; the `.Scan()`
        # order is the physical order, so emit that instead of guessing names.
        if entry["star"] and struct and not records:
            # `.Scan()` receivers are local variables (`refItem`), so match them
            # to the struct name case-insensitively.
            key = next(
                (name for name in scan_orders if name.lower() == struct.lower()), struct
            )
            ordered, relative = scan_orders.get(key, ([], struct_lines.get(struct, "")))
            for ordinal, field in enumerate(ordered, start=1):
                records.append(
                    {
                        "name": "UNKNOWN",
                        "ordinal": ordinal,
                        "go_field": field,
                        "go_type": types.get(field, "UNKNOWN"),
                        "db_type": "UNKNOWN",
                        "nullable": None,
                        "fk": False,
                        "provenance": f"{relative}:{star_tables.get(table, ('', 0))[1]}",
                    }
                )

        contract[table] = {
            "schema": entry["schema"],
            "go_struct": struct or "UNMAPPED",
            "star_select": entry["star"],
            "columns": records,
        }

    for table, columns in sorted(cert_tables.items()):
        contract.setdefault(
            f"{table} (topology)",
            {
                "schema": "SRO_CERTIFICATION",
                "go_struct": "UNMAPPED",
                "star_select": False,
                "columns": [
                    {
                        "name": column,
                        "go_field": "UNMAPPED",
                        "go_type": "UNKNOWN",
                        "db_type": db_type,
                        "nullable": nullable,
                        "fk": column.upper().startswith("FK_"),
                        "provenance": f"{cert_sql.name}:{line}",
                    }
                    for column, (db_type, nullable, line) in sorted(columns.items())
                ],
            },
        )
    return contract


def _pick_struct(table: str, structs: dict) -> str:
    """The struct backing a table: `CHAR` -> `Char`, `ITEMDATA` -> `RefItem`."""
    target = _normalize(table)
    for name in structs:
        if _normalize(name) == target:
            return name
    aliases = {
        "itemdata": "RefItem",
        "charrefdata": "RefChar",
        "gameserverregion": "GameServerRegion",
        "regionreference": "GameServerRegion",
    }
    candidate = aliases.get(target)
    return candidate if candidate in structs else ""


def _yaml_scalar(value) -> str:
    if value is None:
        return "null"
    if isinstance(value, bool):
        return "true" if value else "false"
    return str(value)


def emit_yaml(contract: dict) -> str:
    """Hand-rolled YAML so the tool stays stdlib-only (no PyYAML)."""
    out = [
        "# Persistence contract for the #93 stub server.",
        "# GENERATED by scripts/re/harvest_db_schema.py — do not edit by hand.",
        "# Harvested statically from go-sro source; no database dump is ever read.",
        "# UNKNOWN/UNMAPPED are honest gaps, not placeholders to fill in blindly.",
        "tables:",
    ]
    for table, entry in contract.items():
        out.append(f"  {table}:")
        out.append(f"    schema: {entry['schema']}")
        out.append(f"    go_struct: {entry['go_struct']}")
        out.append(f"    star_select: {_yaml_scalar(entry['star_select'])}")
        out.append("    columns:")
        for column in entry["columns"]:
            out.append(f"      - name: {column['name']}")
            if "ordinal" in column:
                out.append(f"        ordinal: {column['ordinal']}")
            out.append(f"        go_field: {column['go_field']}")
            out.append(f"        go_type: {column['go_type']}")
            out.append(f"        db_type: {column['db_type']}")
            out.append(f"        nullable: {_yaml_scalar(column['nullable'])}")
            out.append(f"        fk: {_yaml_scalar(column['fk'])}")
            out.append(f"        provenance: {column['provenance']}")
    out.append("missing:")
    out.append("  # Owned by systems-go-sro-db-harvest; not yet harvested.")
    for table in MISSING_TABLES:
        out.append(f"  - {table}")
    return "\n".join(out) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument(
        "--source",
        type=Path,
        default=Path.home() / "sro-refs" / "go-sro-agent-server",
        help="go-sro agent-server checkout (reads model/*.go only)",
    )
    parser.add_argument(
        "--sql",
        type=Path,
        default=None,
        help="optional plain-text SRO_CERTIFICATION.sql for the topology tables",
    )
    parser.add_argument("--out", type=Path, default=None, help="write YAML here")
    args = parser.parse_args()

    try:
        contract = harvest(args.source, args.sql)
    except HarvestError as error:
        print(f"harvest_db_schema: {error}", file=sys.stderr)
        return 1

    required = ("CHAR", "ITEM", "INVENTORY", "ITEMDATA", "GAME_SERVER_REGION")
    absent = [table for table in required if table not in contract]
    if absent:
        print(
            f"harvest_db_schema: contract is missing {', '.join(absent)}",
            file=sys.stderr,
        )
        return 1

    yaml = emit_yaml(contract)
    if args.out:
        if args.out.suffix.lower() in FORBIDDEN_SUFFIXES:
            print(
                f"harvest_db_schema: refusing to write {args.out}", file=sys.stderr
            )
            return 1
        args.out.write_text(yaml, encoding="utf-8")
        print(f"harvest_db_schema: wrote {args.out} ({len(contract)} tables)")
    else:
        print(yaml, end="")
    return 0


if __name__ == "__main__":
    sys.exit(main())
