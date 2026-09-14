#!/usr/bin/env python3
"""Fixture tests for check_reference_data.py — stdlib unittest, no PK2, no DB.

Run: `python3 -m unittest discover -s scripts/re -p 'test_*.py'`

The fixtures are trimmed excerpts of the two real shapes: a `Setup.Add` line
from Settings.cs and a `#[repr(usize)]` enum from our textdata parsers.
"""

import importlib.util
import unittest
from pathlib import Path

_spec = importlib.util.spec_from_file_location(
    "check_reference_data", Path(__file__).with_name("check_reference_data.py")
)
crd = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(crd)


# A REPLACE()-wrapped column sits between two plain ones, so a naive comma
# split shifts every index after it.
SETTINGS_FIXTURE = (
    '                Setup.Add(new Query2Path() { Path = "characterdata_5000.txt", '
    'Query = "SELECT _RefObjCommon.Service, _RefObjCommon.ID, '
    "REPLACE(_RefObjItem.ERInc, '.', ''), _RefObjCommon.NameStrID128, "
    '_RefObjChar.Lvl, _RefObjChar.CharGender FROM _RefObjCommon" });'
)

SELECT_STAR_FIXTURE = (
    '                Setup.Add(new Query2Path() { Path = "skilldata_5000.txt", '
    'Query = "SELECT * FROM _RefSkill" });'
)

ENUM_FIXTURE = """
#[repr(usize)]
enum ChardataFields {
    ID = 1,
    CodeName,
    // A comment mentioning Lvl(57) and MaxHP(59) must not be parsed.
    TypeId1 = 9,
    TypeId2,
    TypeId3,
    TypeId4,
    Level = 57,
    Gender = 58,
}
"""


class SplitColumns(unittest.TestCase):
    """The bug that produced a confident false drift report."""

    def test_a_function_call_is_one_column_not_three(self):
        clause = "a.Service, REPLACE(a.ERInc, '.', ''), a.ID"
        self.assertEqual(len(crd.split_columns(clause)), 3)

    def test_commas_inside_a_call_do_not_shift_later_indices(self):
        clause = "a.First, REPLACE(a.Mid, '.', ''), a.Last"
        columns = crd.split_columns(clause)
        self.assertEqual(crd.normalize(columns[2]), "last")

    def test_the_column_name_unwraps_the_function(self):
        self.assertEqual(crd.normalize("REPLACE(_RefObjItem.ERInc, '.', '')"), "erinc")

    def test_a_comma_inside_a_string_literal_does_not_split(self):
        """The real Settings.cs form: `REPLACE(col,',','.')` converts a decimal
        comma, so the delimiter itself appears as a quoted argument."""
        clause = "a.First, REPLACE(a.Mid,',','.'), a.Last"
        columns = crd.split_columns(clause)
        self.assertEqual(len(columns), 3)
        self.assertEqual(crd.normalize(columns[1]), "mid")
        self.assertEqual(crd.normalize(columns[2]), "last")

    def test_a_trim_wrapper_is_also_one_column(self):
        """Index 121 of itemdata is uniquely wrapped in TRIM rather than REPLACE."""
        columns = crd.split_columns("a.First, TRIM(a.Desc2_128), a.Last")
        self.assertEqual(len(columns), 3)
        self.assertEqual(crd.normalize(columns[1]), "desc2")


class Normalization(unittest.TestCase):
    def test_mechanical_differences_dissolve(self):
        self.assertEqual(crd.normalize("_RefObjCommon.NameStrID128"), "namestrid")
        self.assertEqual(crd.normalize("_RefObjCommon.TypeID1"), "typeid1")
        self.assertEqual(crd.normalize("_RefObjChar.MaxHP"), "maxhp")

    def test_semantic_renames_use_the_alias_table(self):
        self.assertEqual(crd.normalize("_RefObjChar.Lvl"), "level")
        self.assertEqual(crd.normalize("_RefObjChar.CharGender"), "gender")
        self.assertEqual(crd.normalize("_RefObjCommon.AssocFileObj128"), "resourcepath")


class SettingsParsing(unittest.TestCase):
    def test_shard_suffix_is_stripped_and_columns_indexed(self):
        tables = crd.settings_columns(SETTINGS_FIXTURE)
        self.assertIn("characterdata", tables)
        columns = tables["characterdata"]
        self.assertEqual(columns["service"], 0)
        self.assertEqual(columns["id"], 1)
        self.assertEqual(columns["erinc"], 2)
        self.assertEqual(columns["namestrid"], 3)
        self.assertEqual(columns["level"], 4)
        self.assertEqual(columns["gender"], 5)

    def test_select_star_tables_are_present_but_empty(self):
        """Empty, not absent, so the caller can tell the two cases apart: a
        `SELECT *` export has no assertable order, whereas a table missing from
        Settings.cs means the reference checkout itself moved."""
        self.assertEqual(crd.settings_columns(SELECT_STAR_FIXTURE), {"skilldata": {}})


class RustEnumParsing(unittest.TestCase):
    def test_implicit_successors_are_numbered(self):
        fields = crd.rust_enum(ENUM_FIXTURE, "ChardataFields")
        self.assertEqual(fields["codename"], 2)
        self.assertEqual(fields["typeid2"], 10)
        self.assertEqual(fields["typeid3"], 11)
        self.assertEqual(fields["typeid4"], 12)

    def test_explicit_discriminants_win(self):
        fields = crd.rust_enum(ENUM_FIXTURE, "ChardataFields")
        self.assertEqual(fields["id"], 1)
        self.assertEqual(fields["typeid1"], 9)
        self.assertEqual(fields["gender"], 58)

    def test_numbers_inside_comments_are_not_fields(self):
        fields = crd.rust_enum(ENUM_FIXTURE, "ChardataFields")
        self.assertNotIn("lvl", fields)
        self.assertNotIn("maxhp", fields)


class Drift(unittest.TestCase):
    """The issue's second acceptance criterion."""

    def test_matching_indices_report_no_drift(self):
        ours = {"gender": 58, "level": 57}
        theirs = {"gender": 58, "level": 57}
        mismatches, _, checked = crd.compare_table("characterdata", ours, theirs)
        self.assertEqual(mismatches, [])
        self.assertEqual(checked, 2)

    def test_an_injected_gender_drift_is_named(self):
        drifted = ENUM_FIXTURE.replace("Gender = 58,", "Gender = 57,")
        ours = crd.rust_enum(drifted, "ChardataFields")
        mismatches, _, _ = crd.compare_table(
            "characterdata", ours, {"gender": 58}
        )
        self.assertEqual(len(mismatches), 1)
        self.assertIn("characterdata.gender", mismatches[0])
        self.assertIn("ours=57", mismatches[0])
        self.assertIn("settings=58", mismatches[0])

    def test_a_field_settings_does_not_name_is_unchecked_not_drift(self):
        mismatches, unchecked, checked = crd.compare_table(
            "itemdata", {"itemclass": 61}, {"gender": 58}
        )
        self.assertEqual(mismatches, [])
        self.assertEqual(checked, 0)
        self.assertEqual(len(unchecked), 1)


class StatRanges(unittest.TestCase):
    THEIRS = {"dur_l": 63, "dur_u": 64, "pd_l": 65, "pd_u": 66}

    def test_a_correct_pair_passes(self):
        mismatches, _, checked = crd.compare_stat_ranges(
            {"durability": (63, 64)}, self.THEIRS
        )
        self.assertEqual(mismatches, [])
        self.assertEqual(checked, 2)

    def test_swapped_bounds_are_caught(self):
        """Both indices stay individually plausible, so only the pair shows it."""
        mismatches, _, _ = crd.compare_stat_ranges(
            {"durability": (64, 63)}, self.THEIRS
        )
        self.assertEqual(len(mismatches), 1)
        self.assertIn("StatRange::durability", mismatches[0])

    def test_a_pair_straying_onto_the_neighbouring_stat_is_caught(self):
        mismatches, _, _ = crd.compare_stat_ranges(
            {"durability": (65, 66)}, self.THEIRS
        )
        self.assertEqual(len(mismatches), 1)

    def test_an_unmapped_variant_is_unchecked_not_drift(self):
        mismatches, unchecked, checked = crd.compare_stat_ranges(
            {"somethingnew": (1, 2)}, self.THEIRS
        )
        self.assertEqual(mismatches, [])
        self.assertEqual(checked, 0)
        self.assertEqual(len(unchecked), 1)

    def test_the_real_match_arms_parse(self):
        source = """
        fn columns(self) -> (usize, usize) {
            match self {
                StatRange::Durability => (63, 64),
                StatRange::PhyAtkMin => (95, 96),
            }
        }
        """
        pairs = crd.stat_range_columns(source)
        self.assertEqual(pairs["durability"], (63, 64))
        self.assertEqual(pairs["phyatkmin"], (95, 96))


if __name__ == "__main__":
    unittest.main()
