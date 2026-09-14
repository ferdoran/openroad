#!/usr/bin/env python3
"""Unit tests for `stringbind.py`.

Fixtures are inline excerpts in the real `strings.tsv` shape, so the tests run
anywhere — the quarantine corpus is machine-local and must never be a
prerequisite for checking the parser (same rule as `test_refgrep.py`).

Run: `python3 scripts/re/test_stringbind.py`
"""

import io
import unittest

import stringbind


# Verbatim column shape of `corpus/client/strings.tsv`, including the header,
# a multi-reference row, and the two rows that carry the same value at two
# addresses (the compiler does not always pool string literals).
TSV = """addr\tnrefs\treffuncs\tvalue
00d81d40\t5\t0097a5c0,00978140\tNULL != pStrEnd
00e10000\t1\t00824560\tUIIT_STT_TC_JOIN_ANNOUNCE
00e10020\t2\t00890ab0,00892c50\tUIIT_MSG_TC_MACHING_JOINING_MEMBER
00e10040\t1\t008986c0\tUIIT_MSG_TC_MACHING_JOINING_MEMBER
00e10060\t1\t00821e40\tSubMentor is Over than 2
00e10080\t1\t00821e40\tApprenticeShip is Over than 5
00e100a0\t1\t004f3310\tSTRONG
00e100c0\t0\t\tUIIT_STT_UNREFERENCED_BUT_PRESENT
"""


def table():
    return stringbind.StringTable(stringbind.parse_strings_tsv(io.StringIO(TSV)))


class Parsing(unittest.TestCase):
    def test_the_header_row_is_not_a_string(self):
        values = [v for v, _ in stringbind.parse_strings_tsv(io.StringIO(TSV))]
        self.assertNotIn("value", values)
        self.assertEqual(len(values), 8)

    def test_a_value_containing_a_tab_survives(self):
        # the value column is the remainder of the line, not split()[3]
        rows = stringbind.parse_strings_tsv(
            io.StringIO("00e1\t1\t00aa\tformat\twith a tab\n")
        )
        self.assertEqual(rows[0][0], "format\twith a tab")

    def test_references_merge_across_duplicate_addresses(self):
        # the same key at two addresses must answer with BOTH functions, or
        # "who binds this key" depends on which copy was found first
        refs = table().references("UIIT_MSG_TC_MACHING_JOINING_MEMBER")
        self.assertEqual(refs, ["00890ab0", "00892c50", "008986c0"])

    def test_an_absent_key_is_empty_not_an_error(self):
        self.assertEqual(table().references("UIIT_STT_NOT_IN_THE_BINARY"), [])

    def test_a_referenced_but_unreferenced_row_still_carries_the_key(self):
        # nrefs 0: the string exists in the binary with no reference recorded,
        # which is existence evidence but names no function
        self.assertEqual(table().references("UIIT_STT_UNREFERENCED_BUT_PRESENT"), [])
        self.assertIn("UIIT_STT_UNREFERENCED_BUT_PRESENT", table().keys())


class KeyShape(unittest.TestCase):
    def test_prose_and_paths_are_not_keys(self):
        keys = table().keys()
        self.assertNotIn("NULL != pStrEnd", keys)
        self.assertNotIn("SubMentor is Over than 2", keys)

    def test_a_single_word_is_not_a_key(self):
        # STRONG is a shipped enum token, not a two-segment string key
        self.assertNotIn("STRONG", table().keys())

    def test_key_families_beyond_uiit_are_kept(self):
        # an over-narrow `^UIIT_` filter is how a sweep silently under-reports
        self.assertTrue(stringbind.KEY_SHAPE.match("TC_MACHING_COMPLETE"))
        self.assertTrue(stringbind.KEY_SHAPE.match("SN_EVENT_START"))


class KeyLists(unittest.TestCase):
    def test_textuisystem_rows_and_bare_key_lines_both_parse(self):
        keys = stringbind.extract_keys(
            io.StringIO(
                "1986\tUIIT_MSG_STATE_GAIN_EXP\t0\t[%I64d]Experience Points gained.\n"
                "UIIT_STT_TC_JOIN_ANNOUNCE\n"
                "\n"
                "// a comment line with no key\n"
            )
        )
        self.assertEqual(
            keys, ["UIIT_MSG_STATE_GAIN_EXP", "UIIT_STT_TC_JOIN_ANNOUNCE"]
        )

    def test_duplicates_are_dropped_so_the_denominator_is_distinct_keys(self):
        keys = stringbind.extract_keys(io.StringIO("A_KEY\nA_KEY\nB_KEY\n"))
        self.assertEqual(keys, ["A_KEY", "B_KEY"])


class ShippedFileEncoding(unittest.TestCase):
    def test_a_utf16_textdata_file_is_decoded_by_its_bom(self):
        # the shipped textuisystem.txt is UTF-16-LE with a BOM; read as UTF-8
        # it yields NUL-separated bytes, matches no key, and the sweep reports
        # zero keys while looking perfectly healthy
        import tempfile
        from pathlib import Path

        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "textuisystem.txt"
            path.write_bytes(
                "1\tUIC_STT_FONTNAME\t\t\tArial\r\n".encode("utf-16")
            )
            keys = stringbind.extract_keys(stringbind.read_key_source(path))
        self.assertEqual(keys, ["UIC_STT_FONTNAME"])

    def test_a_plain_ascii_key_list_still_reads(self):
        import tempfile
        from pathlib import Path

        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "keys.txt"
            path.write_text("A_KEY\nB_KEY\n", encoding="utf-8")
            self.assertEqual(
                stringbind.extract_keys(stringbind.read_key_source(path)),
                ["A_KEY", "B_KEY"],
            )


class AssertionBounds(unittest.TestCase):
    def test_the_academy_roster_bound_is_recovered_from_the_binary(self):
        # the corollary #549 is really about: 2 + 5 = 7 with a stated origin
        found = dict(stringbind.bound_strings(table()))
        self.assertIn("SubMentor is Over than 2", found)
        self.assertIn("ApprenticeShip is Over than 5", found)
        self.assertEqual(found["SubMentor is Over than 2"], ["00821e40"])

    def test_a_number_without_an_assertion_phrase_is_not_a_bound(self):
        found = dict(stringbind.bound_strings(table()))
        self.assertNotIn("UIIT_MSG_TC_MACHING_JOINING_MEMBER", found)

    def test_grep_narrows_the_result(self):
        found = dict(stringbind.bound_strings(table(), grep="submentor"))
        self.assertEqual(list(found), ["SubMentor is Over than 2"])


if __name__ == "__main__":
    unittest.main(verbosity=2)
