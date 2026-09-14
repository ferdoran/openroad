#!/usr/bin/env python3
"""Fixture tests for bump_version.py — stdlib unittest, no cargo, no deps.

Run: `python3 scripts/test_bump_version.py` (also run by `make ci` via `make re-tools`).

The load-bearing test here is TABLE SCOPING. This workspace's root Cargo.toml has a
`version = "..."` under [workspace.package] *and* a dozen more under
[workspace.dependencies]; client/Cargo.toml has uuid's `version = "1.2.2"`. A regex
that matched the wrong one would silently rewrite a dependency pin during a release,
so both "picks the right line" and "leaves the others alone" are pinned below rather
than left to review.
"""

import importlib.util
import tempfile
import unittest
from pathlib import Path

_spec = importlib.util.spec_from_file_location(
    "bump_version", Path(__file__).with_name("bump_version.py")
)
bv = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(bv)


# Shaped like the real root manifest: the package version is NOT the first
# `version =` a naive scan would hit going the other way, and dependency pins
# bracket it on both sides.
MANIFEST = """\
[workspace]
resolver = "2"

members = ["client", "packets"]

[workspace.package]
version = "0.1.0"
edition = "2021"
license = "GPL-3.0-or-later"

[workspace.dependencies]
bevy = { version = "0.19", features = ["dds"] }
bytes = "1.3.0"
thiserror = "1"

[profile.dev]
opt-level = 1
"""


class TestFindVersionLine(unittest.TestCase):
    def test_finds_the_workspace_package_version(self):
        lines = MANIFEST.splitlines(keepends=True)
        index, current = bv.find_version_line(lines)
        self.assertEqual(current, "0.1.0")
        self.assertIn("0.1.0", lines[index])

    def test_ignores_dependency_versions(self):
        # A manifest whose ONLY [workspace.package] key is edition must fail rather
        # than fall through to a [workspace.dependencies] pin.
        broken = "[workspace.package]\nedition = \"2021\"\n\n[workspace.dependencies]\nbytes = \"1.3.0\"\n"
        with self.assertRaises(bv.BumpError):
            bv.find_version_line(broken.splitlines(keepends=True))

    def test_missing_table(self):
        with self.assertRaises(bv.BumpError):
            bv.find_version_line("[workspace]\nresolver = \"2\"\n".splitlines(keepends=True))


class TestBump(unittest.TestCase):
    def test_levels(self):
        self.assertEqual(bv.bump("0.1.0", "patch"), "0.1.1")
        self.assertEqual(bv.bump("0.1.0", "minor"), "0.2.0")
        self.assertEqual(bv.bump("0.1.0", "major"), "1.0.0")

    def test_minor_and_major_reset_lower_parts(self):
        self.assertEqual(bv.bump("1.4.7", "minor"), "1.5.0")
        self.assertEqual(bv.bump("1.4.7", "major"), "2.0.0")

    def test_rejects_non_semver(self):
        for bad in ("0.1", "1.2.3-rc1", "v1.2.3", "", "1.2.3.4", "a.b.c"):
            with self.assertRaises(bv.BumpError, msg=bad):
                bv.bump(bad, "patch")


class TestCli(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.manifest = Path(self.tmp.name) / "Cargo.toml"
        self.manifest.write_text(MANIFEST, encoding="utf-8")

    def tearDown(self):
        self.tmp.cleanup()

    def run_cli(self, *args):
        return bv.main([*args, "--manifest", str(self.manifest)])

    def test_print_does_not_write(self):
        self.assertEqual(self.run_cli("--print"), 0)
        self.assertEqual(self.manifest.read_text(encoding="utf-8"), MANIFEST)

    def test_bump_without_write_leaves_file_untouched(self):
        self.assertEqual(self.run_cli("--bump", "minor"), 0)
        self.assertEqual(self.manifest.read_text(encoding="utf-8"), MANIFEST)

    def test_write_updates_only_the_package_version(self):
        self.assertEqual(self.run_cli("--bump", "minor", "--write"), 0)
        out = self.manifest.read_text(encoding="utf-8")
        self.assertIn('version = "0.2.0"', out)
        # the dependency pins must be byte-identical
        self.assertIn('bevy = { version = "0.19", features = ["dds"] }', out)
        self.assertIn('bytes = "1.3.0"', out)
        self.assertNotIn('version = "0.1.0"', out)
        # and nothing else moved
        self.assertEqual(len(out.splitlines()), len(MANIFEST.splitlines()))

    def test_set_exact(self):
        self.assertEqual(self.run_cli("--set", "2.5.1", "--write"), 0)
        self.assertIn('version = "2.5.1"', self.manifest.read_text(encoding="utf-8"))

    def test_set_rejects_non_semver_without_writing(self):
        self.assertEqual(self.run_cli("--set", "1.2.3-rc1", "--write"), 1)
        self.assertEqual(self.manifest.read_text(encoding="utf-8"), MANIFEST)

    def test_missing_manifest_is_an_error_not_a_traceback(self):
        missing = str(Path(self.tmp.name) / "nope.toml")
        self.assertEqual(bv.main(["--print", "--manifest", missing]), 1)


class TestRealManifest(unittest.TestCase):
    """The shipped Cargo.toml must be parseable — this is what the workflow reads."""

    def test_repo_manifest_parses(self):
        lines = bv.CARGO_TOML.read_text(encoding="utf-8").splitlines(keepends=True)
        _, current = bv.find_version_line(lines)
        bv.parse(current)  # raises if the real manifest ever stops being plain semver


if __name__ == "__main__":
    unittest.main()
