#!/usr/bin/env python3
"""Tests for the persistence-contract harvester (stdlib unittest, no deps).

The fixtures are verbatim trims of the real go-sro sources and of the shipped
certification SQL, so the parser is exercised against the byte shapes it will
actually meet — including the two that broke it during development: the
UTF-16LE BOM on `SRO_CERTIFICATION.sql`, and `ITEMDATA` being read with
`SELECT *` so its column names exist nowhere in the source.

Run: `python3 scripts/re/test_harvest_db_schema.py`
"""

import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import harvest_db_schema as h  # noqa: E402

# Verbatim trim of <refs>/go-sro-agent-server/model/character_repository.go
# (struct at :10-34, SQL constants at :37-43).
CHARACTER_REPOSITORY_GO = '''package model

type Char struct {
	ID          int
	RefObjID    int
	User        int
	Shard       int
	Name        string
	Scale       byte
	Level       int
	MaxLevel    int
	Exp         int64
	Str         int
	Int         int
	StatPoints  int
	IsDeleting  bool
	PosX        float32
	Region      int16
}

const (
	SelectCharsByAccountId string = "SELECT ID, REF_OBJ_ID, CHAR_NAME, CHAR_SCALE, CURRENT_LEVEL, EXP, STRENGTH, INTELLECT, STAT_POINTS, DELETING, FK_REGION FROM `SRO_SHARD`.`CHAR` WHERE FK_USER=? ORDER BY CTIME ASC"
	insert_char            string = "INSERT INTO `SRO_SHARD`.`CHAR`(REF_OBJ_ID, FK_USER, FK_SHARD, CHAR_NAME, CURRENT_LEVEL, POS_X) VALUES(?,?,?,?,?,?)"
	update_is_deleting     string = "UPDATE `SRO_SHARD`.`CHAR` SET DELETING=? WHERE CHAR_NAME=?"
)
'''

# Verbatim trim of model/inventory_repository.go — the aliased JOIN that must
# attribute each column to the right table.
INVENTORY_REPOSITORY_GO = '''package model

const (
	SelectCharacterInventory string = "SELECT inv.SLOT, inv.FK_ITEM, it.VARIANCE, it.FK_REF_ITEM FROM `SRO_SHARD`.`INVENTORY` inv INNER JOIN `SRO_SHARD`.`ITEM` AS it ON inv.FK_ITEM = it.ID WHERE inv.FK_CHAR=?"
	InsertInventoryItem      string = "INSERT INTO `SRO_SHARD`.`INVENTORY` (FK_CHAR, SLOT, FK_ITEM) VALUES(?, ?, ?);"
)
'''

# Verbatim trim of model/ref_items.go plus model/ref_object.go's embed: the
# star select carries no column names, so the ordering has to come from Scan.
REF_ITEMS_GO = '''package model

type SRObject struct {
	ID       uint32
	CodeName string
}

type RefItem struct {
	SRObject
	StackSize      int
	RequiredGender int
}

const (
	SelectAllRefItems = "SELECT * FROM `SRO_SHARD`.`ITEMDATA`;"
)

func LoadRefItems() {
	for queryHandle.Next() {
		queryHandle.Scan(
			&refItem.ID,
			&refItem.CodeName,
			&refItem.StackSize,
			&refItem.RequiredGender,
		)
	}
}
'''

GAME_SERVER_REGION_GO = '''package model

type GameServerRegion struct {
	ContinentName string
	Regions       []int16
}

const (
	SelectGameserverRegions string = "SELECT gr.Continent_Name FROM `SRO_SHARD`.`GAME_SERVER_REGION` gr WHERE gr.Game_Server_ID=?"
)
'''

ITEM_GO = '''package model

type Item struct {
	SRObject
	ID       int
	Variance uint64
}

const (
	InsertItem string = "INSERT INTO `SRO_SHARD`.`ITEM` (FK_REF_ITEM, VARIANCE) VALUES (?, ?);"
)
'''

# Verbatim trim of SRO_CERTIFICATION.sql:136-144 (MSSQL DDL, [dbo] topology).
CERTIFICATION_SQL = """USE [SRO_CERTIFICATION]
GO
CREATE TABLE [dbo].[Shard](
\t[ID] [smallint] IDENTITY(1,1) NOT NULL,
\t[FarmID] [tinyint] NULL,
\t[Name] [varchar](32) NOT NULL,
\t[MaxUser] [smallint] NOT NULL,
) ON [PRIMARY]
GO
"""


def write_source_tree(root: Path, sources: dict) -> Path:
    """Lay out a fake `<checkout>/model/*.go` tree."""
    model = root / "model"
    model.mkdir(parents=True, exist_ok=True)
    for name, text in sources.items():
        (model / name).write_text(text, encoding="utf-8")
    return root


class GuardTests(unittest.TestCase):
    """The one rule the whole tool exists to keep: never open a DB dump."""

    def test_database_dumps_are_refused(self):
        for suffix in (".bak", ".mdf", ".ldf"):
            with self.assertRaises(h.HarvestError) as caught:
                h.guard_path(Path(f"/tmp/whatever{suffix}"))
            self.assertIn(suffix, str(caught.exception))

    def test_only_go_and_sql_may_be_opened(self):
        with self.assertRaises(h.HarvestError):
            h.guard_path(Path("/tmp/notes.txt"))
        # These must not raise.
        h.guard_path(Path("/tmp/model.go"))
        h.guard_path(Path("/tmp/schema.sql"))

    def test_an_mdf_input_exits_nonzero(self):
        argv = sys.argv
        sys.argv = ["harvest_db_schema.py", "--sql", "/tmp/nope.mdf"]
        try:
            self.assertEqual(h.main(), 1)
        finally:
            sys.argv = argv


class GoParsingTests(unittest.TestCase):
    def test_struct_fields_and_embeds(self):
        structs = h.parse_go_structs(REF_ITEMS_GO)
        self.assertEqual(structs["RefItem"]["embeds"], ["SRObject"])
        names = [field for field, _, _ in structs["RefItem"]["fields"]]
        self.assertEqual(names, ["StackSize", "RequiredGender"])
        # The embed supplies ID/CodeName and their types.
        types = h.field_types("RefItem", structs)
        self.assertEqual(types["ID"], "uint32")
        self.assertEqual(types["CodeName"], "string")
        self.assertEqual(types["StackSize"], "int")

    def test_aliased_join_attributes_columns_to_the_right_table(self):
        sql = h.extract_sql_literals(INVENTORY_REPOSITORY_GO)[0][0]
        parsed = dict((table, cols) for _, table, cols, _ in h.parse_sql(sql))
        self.assertIn("SLOT", parsed["INVENTORY"])
        self.assertIn("FK_ITEM", parsed["INVENTORY"])
        # VARIANCE belongs to ITEM via the `it` alias, not to INVENTORY.
        self.assertIn("VARIANCE", parsed["ITEM"])
        self.assertNotIn("VARIANCE", parsed["INVENTORY"])

    def test_scan_order_is_the_physical_column_order(self):
        order = h.parse_scan_order(REF_ITEMS_GO)
        self.assertEqual(
            order["refItem"], ["ID", "CodeName", "StackSize", "RequiredGender"]
        )


class FieldMappingTests(unittest.TestCase):
    """The documented go-sro renamings, and a refusal to guess past them."""

    def setUp(self):
        structs = h.parse_go_structs(CHARACTER_REPOSITORY_GO)
        self.types = h.field_types("Char", structs)
        self.fields = {h._normalize(f): f for f in self.types}

    def match(self, column):
        return h.match_field(column, self.fields, "CHAR")

    def test_documented_renamings(self):
        self.assertEqual(self.match("CURRENT_LEVEL"), "Level")
        self.assertEqual(self.match("FK_USER"), "User")
        self.assertEqual(self.match("CHAR_NAME"), "Name")
        self.assertEqual(self.match("DELETING"), "IsDeleting")
        self.assertEqual(self.match("STRENGTH"), "Str")
        self.assertEqual(self.match("INTELLECT"), "Int")
        self.assertEqual(self.match("STAT_POINTS"), "StatPoints")
        self.assertEqual(self.match("POS_X"), "PosX")
        self.assertEqual(self.match("REF_OBJ_ID"), "RefObjID")

    def test_ambiguity_and_absence_stay_unmapped(self):
        # No field resembles this at all.
        self.assertEqual(self.match("CTIME"), "UNMAPPED")
        # `EXP` must not swallow a longer field by loose prefixing.
        self.assertEqual(self.match("EXP"), "Exp")


class CertificationSqlTests(unittest.TestCase):
    def test_utf16_with_bom_is_decoded(self):
        """The shipped SQL is UTF-16LE; reading it as UTF-8 silently matches nothing."""
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "SRO_CERTIFICATION.sql"
            path.write_bytes(b"\xff\xfe" + CERTIFICATION_SQL.encode("utf-16-le"))
            tables = h.parse_mssql_tables(h.read_text(path))
        self.assertIn("SHARD", tables)
        self.assertEqual(tables["SHARD"]["NAME"][0], "VARCHAR(32)")
        self.assertFalse(tables["SHARD"]["NAME"][1])  # NOT NULL
        self.assertTrue(tables["SHARD"]["FARMID"][1])  # NULL
        self.assertEqual(tables["SHARD"]["MAXUSER"][0], "SMALLINT")


class ContractTests(unittest.TestCase):
    def build(self, with_sql=True):
        tmp = tempfile.mkdtemp()
        root = write_source_tree(
            Path(tmp),
            {
                "character_repository.go": CHARACTER_REPOSITORY_GO,
                "inventory_repository.go": INVENTORY_REPOSITORY_GO,
                "ref_items.go": REF_ITEMS_GO,
                "item.go": ITEM_GO,
                "game_server_region.go": GAME_SERVER_REGION_GO,
            },
        )
        sql_path = None
        if with_sql:
            sql_path = root / "SRO_CERTIFICATION.sql"
            sql_path.write_bytes(b"\xff\xfe" + CERTIFICATION_SQL.encode("utf-16-le"))
        return h.harvest(root, sql_path)

    def test_acceptance_tables_are_covered(self):
        contract = self.build()
        for table in ("CHAR", "ITEM", "INVENTORY", "ITEMDATA", "GAME_SERVER_REGION"):
            self.assertIn(table, contract, f"{table} missing from the contract")

    def test_char_current_level_is_mapped_from_level_with_provenance(self):
        """The design doc's named acceptance case."""
        columns = {c["name"]: c for c in self.build()["CHAR"]["columns"]}
        level = columns["CURRENT_LEVEL"]
        self.assertEqual(level["go_field"], "Level")
        self.assertEqual(level["go_type"], "int")
        self.assertRegex(level["provenance"], r"^model/character_repository\.go:\d+$")
        # Every column carries provenance, not just the interesting ones.
        for column in self.build()["CHAR"]["columns"]:
            self.assertRegex(column["provenance"], r"^model/\w+\.go:\d+$")

    def test_foreign_keys_are_flagged(self):
        columns = {c["name"]: c for c in self.build()["CHAR"]["columns"]}
        self.assertTrue(columns["FK_USER"]["fk"])
        self.assertTrue(columns["FK_SHARD"]["fk"])
        self.assertFalse(columns["CHAR_NAME"]["fk"])

    def test_star_select_table_gets_ordinals_instead_of_invented_names(self):
        itemdata = self.build()["ITEMDATA"]
        self.assertTrue(itemdata["star_select"])
        self.assertEqual(itemdata["go_struct"], "RefItem")
        columns = itemdata["columns"]
        self.assertEqual([c["ordinal"] for c in columns], [1, 2, 3, 4])
        self.assertEqual([c["go_field"] for c in columns],
                         ["ID", "CodeName", "StackSize", "RequiredGender"])
        # Names are honestly unknown — the source never states them.
        self.assertEqual({c["name"] for c in columns}, {"UNKNOWN"})
        # ...but the Go types are resolved through the embed.
        self.assertEqual(columns[0]["go_type"], "uint32")

    def test_topology_tables_come_from_the_certification_sql(self):
        contract = self.build()
        self.assertIn("SHARD (topology)", contract)
        entry = contract["SHARD (topology)"]
        self.assertEqual(entry["schema"], "SRO_CERTIFICATION")
        columns = {c["name"]: c for c in entry["columns"]}
        self.assertEqual(columns["NAME"]["db_type"], "VARCHAR(32)")
        self.assertTrue(columns["NAME"]["provenance"].startswith("SRO_CERTIFICATION.sql:"))

    def test_contract_without_certification_sql_still_builds(self):
        contract = self.build(with_sql=False)
        self.assertIn("CHAR", contract)
        self.assertNotIn("SHARD (topology)", contract)

    def test_emitted_yaml_records_the_missing_list(self):
        yaml = h.emit_yaml(self.build())
        self.assertIn("missing:", yaml)
        for table in h.MISSING_TABLES:
            self.assertIn(f"  - {table}", yaml)
        self.assertIn("GENERATED by scripts/re/harvest_db_schema.py", yaml)

    def test_a_source_tree_without_model_is_an_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(h.HarvestError):
                h.harvest(Path(tmp), None)


if __name__ == "__main__":
    unittest.main(verbosity=2)
