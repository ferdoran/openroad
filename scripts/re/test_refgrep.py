#!/usr/bin/env python3
"""Unit tests for `refgrep.py`.

Fixtures are inline excerpts of real decompiler output, so the tests run
anywhere — the quarantine corpus is machine-local and must never be a
prerequisite for checking the parser.

Run: `python3 scripts/re/test_refgrep.py`
"""

import unittest
from pathlib import Path

import refgrep


# Verbatim shape of a 3-arg server anchor, wrapped across lines the way Ghidra
# emits it (`decompiled-combat-chance/writer_0x006A3D50_FUN_006a3d50.c`).
THREE_ARG = r'''
      local_f8 != *(int *)(param_1 + 0x184))) &&
     (cVar5 = FUN_00964760(0x1cd,
                           "D:\\WORK2005\\Source\\SilkroadOnline\\Server\\ServerCommon\\ReferenceData.cpp"
                           ,"dwItemID == m_dwLink"), cVar5 == '\0')) {
    DebugBreak();
'''

# The same helper without a condition — the 2-arg shape.
TWO_ARG = r'''
    cVar1 = FUN_00964760(0x47b,
                         "D:\\WORK2005\\Source\\JMX_Library\\NavMesh_new\\RegionManagerBody.cpp");
'''


class AnchorParsing(unittest.TestCase):
    def test_three_arg_anchor_yields_line_file_and_condition(self):
        match = refgrep.ANCHOR_CALL.search(THREE_ARG)
        self.assertIsNotNone(match, "the wrapped 3-arg call must match")
        self.assertEqual(match.group(1), "00964760")
        self.assertEqual(refgrep.parse_line_number(match.group(2)), 461)
        self.assertEqual(refgrep.win_basename(match.group(3)), "ReferenceData.cpp")
        self.assertEqual(match.group(4), "dwItemID == m_dwLink")

    def test_two_arg_anchor_has_no_condition(self):
        match = refgrep.ANCHOR_CALL.search(TWO_ARG)
        self.assertIsNotNone(match)
        self.assertEqual(refgrep.parse_line_number(match.group(2)), 1147)
        self.assertEqual(refgrep.win_basename(match.group(3)), "RegionManagerBody.cpp")
        self.assertIsNone(
            match.group(4),
            "the server helper also occurs without a condition — argument count "
            "does not discriminate the binaries, the helper address does",
        )

    def test_line_numbers_are_hex(self):
        # Ghidra prints the assert line as hex; reading it as decimal silently
        # produces a plausible but wrong source line.
        self.assertEqual(refgrep.parse_line_number("0x1cd"), 461)
        self.assertEqual(refgrep.parse_line_number("461"), 461)

    def test_windows_paths_are_split_on_backslashes(self):
        path = r"D:\\vss-od\\Silkroad\\Client\\client\\CObjCharacter.cpp"
        self.assertEqual(refgrep.win_basename(path), "CObjCharacter.cpp")
        self.assertEqual(
            refgrep.win_dirname(path), r"D:\vss-od\Silkroad\Client\client"
        )


class ContainingFunction(unittest.TestCase):
    def test_va_is_read_from_either_filename_shape(self):
        self.assertEqual(
            refgrep.containing_va(Path("writer_0x006A3D50_FUN_006a3d50.c")),
            "0x006A3D50",
        )
        # No theme prefix — this shape exists in the corpus too.
        self.assertEqual(
            refgrep.containing_va(Path("0x006A58F0_FUN_006a58f0.c")), "0x006A58F0"
        )

    def test_unknown_filename_shape_is_marked_not_guessed(self):
        self.assertEqual(refgrep.containing_va(Path("notes.c")), "?")


class Dedupe(unittest.TestCase):
    def test_copies_across_themed_dirs_collapse_to_one_row(self):
        # The same function is decompiled into every feature group it belongs
        # to, so one source anchor is seen once per copy.
        rows = [
            refgrep.Anchor("ReferenceData.cpp", 461, "0x00964760", "a == b",
                           "0x006A3D50", "writer_0x006A3D50_FUN_006a3d50.c"),
            refgrep.Anchor("ReferenceData.cpp", 461, "0x00964760", "a == b",
                           "0x006A3D50", "tableuser_0x006A3D50_FUN_006a3d50.c"),
        ]
        self.assertEqual(len(refgrep.dedupe(rows)), 1)

    def test_distinct_lines_are_kept(self):
        rows = [
            refgrep.Anchor("ReferenceData.cpp", 461, "0x00964760", None,
                           "0x006A3D50", "a.c"),
            refgrep.Anchor("ReferenceData.cpp", 484, "0x00964760", None,
                           "0x006A3D50", "a.c"),
        ]
        self.assertEqual(len(refgrep.dedupe(rows)), 2)


class MethodSymbols(unittest.TestCase):
    def test_symbols_are_extracted_from_inside_diagnostic_strings(self):
        # They are not stored as bare values; matching the whole string finds
        # nothing at all.
        value = "## Illegal ##  IGObj::SendQuestEventMessage Entered! TypeID: %u"
        self.assertEqual(
            refgrep.METHOD_SYMBOL.findall(value), ["IGObj::SendQuestEventMessage"]
        )

    def test_plain_text_yields_no_symbol(self):
        self.assertEqual(refgrep.METHOD_SYMBOL.findall("SendMsg route-hint"), [])


if __name__ == "__main__":
    unittest.main(verbosity=2)
