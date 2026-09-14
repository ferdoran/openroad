#!/usr/bin/env python3
"""Fixture tests for check_opcode_ledger.py — stdlib unittest, no cargo, no deps.

Run: `python3 scripts/test_check_opcode_ledger.py` (also run by `make ci`).

The point of these tests is the count rule (#558): the ledger must not carry a
hand-maintained "Wired opcodes: N" line, but it *must* keep the unrelated
numbers around it (the S->C dispatch-table denominator, byte sizes in the Notes
column). A regex that fails either direction silently breaks the net lane, so
both directions are pinned here rather than left to review.
"""

import importlib.util
import tempfile
import unittest
from pathlib import Path

_spec = importlib.util.spec_from_file_location(
    "check_opcode_ledger", Path(__file__).with_name("check_opcode_ledger.py")
)
col = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(col)


MACRO_FIXTURE = """
packets! {
    // Handshake
    0x2001 => ModuleIdentification,
    0x2002 => KeepAlive,
    0x7021 => MovementRequest,
}
"""

LEDGER_FIXTURE = """# Protocol opcode coverage ledger

- **Original client S→C dispatch table:** ~219 opcodes, per a Ghidra pass.
- **Status legend:** `wired` = typed and fanned out.

| Opcode | Name | Direction | Status | Notes |
|---|---|---|---|---|
| `0x2001` | ModuleIdentification | both | wired | first packet |
| `0x2002` | KeepAlive | C→S | documented | 5s ping |
| `0x7021` | MovementRequest | C→S | wired | 1026 bytes both ways |
"""


class TestParsers(unittest.TestCase):
    def _write(self, macro: str, ledger: str) -> None:
        tmp = Path(tempfile.mkdtemp())
        macro_path = tmp / "lib.rs"
        ledger_path = tmp / "opcodes.md"
        macro_path.write_text(macro)
        ledger_path.write_text(ledger)
        col.MACRO = macro_path
        col.LEDGER = ledger_path

    def tearDown(self) -> None:
        col.MACRO = col.ROOT / "packets" / "src" / "lib.rs"
        col.LEDGER = col.ROOT / "docs" / "protocol" / "opcodes.md"

    def test_macro_and_ledger_agree(self):
        self._write(MACRO_FIXTURE, LEDGER_FIXTURE)
        self.assertEqual(col.macro_opcodes(), col.ledger_opcodes())
        self.assertEqual(col.main(), 0)

    def test_wired_but_missing_from_ledger_fails(self):
        self._write(MACRO_FIXTURE, LEDGER_FIXTURE.replace(
            "| `0x7021` | MovementRequest | C→S | wired | 1026 bytes both ways |\n", ""
        ))
        self.assertEqual(col.main(), 1)

    def test_name_mismatch_fails(self):
        self._write(MACRO_FIXTURE, LEDGER_FIXTURE.replace("KeepAlive", "Heartbeat"))
        self.assertEqual(col.main(), 1)


class TestNoHardcodedCount(unittest.TestCase):
    def test_clean_ledger_prose_passes(self):
        self.assertEqual(col.no_hardcoded_count(LEDGER_FIXTURE), [])

    def test_the_exact_line_that_was_removed_is_rejected(self):
        self.assertEqual(
            col.no_hardcoded_count("- **Wired opcodes:** 176 (this table)."),
            ["- **Wired opcodes:** 176 (this table)."],
        )

    def test_reversed_word_order_is_rejected(self):
        self.assertTrue(col.no_hardcoded_count("The macro wires 176 wired opcodes."))

    def test_this_table_phrasing_alone_is_rejected(self):
        self.assertTrue(col.no_hardcoded_count("- **Total:** 176 (this table)."))

    def test_denominator_line_is_not_a_false_positive(self):
        self.assertEqual(
            col.no_hardcoded_count(
                "- **Original client S→C dispatch table:** ~219 opcodes, per Ghidra."
            ),
            [],
        )

    def test_table_rows_are_exempt(self):
        self.assertEqual(
            col.no_hardcoded_count(
                "| `0x2113` | XTrapIdentification | both | wired | "
                "1026 bytes, 176 wired opcodes mentioned in prose |"
            ),
            [],
        )

    def test_real_ledger_has_no_hardcoded_count(self):
        text = (col.ROOT / "docs" / "protocol" / "opcodes.md").read_text()
        self.assertEqual(col.no_hardcoded_count(text), [])


if __name__ == "__main__":
    unittest.main()
