#!/usr/bin/env python3
"""Read and bump `[workspace.package] version` in the root Cargo.toml.

Run: `python3 scripts/bump_version.py --print`            -> current version
     `python3 scripts/bump_version.py --bump minor`       -> next version, no write
     `python3 scripts/bump_version.py --bump minor --write`
     `python3 scripts/bump_version.py --set 1.2.3 --write`

IDEA. The release workflow (.github/workflows/release.yml) owns the version, and it
needs the arithmetic to be runnable and testable off a runner — hence a script here
rather than inline YAML, matching the other stdlib-only gate helpers in scripts/.

The parse is table-scoped on purpose. `version = "..."` appears many times in this
workspace — every entry under `[workspace.dependencies]`, and `uuid`'s `version =
"1.2.2"` in client/Cargo.toml — so a whole-file regex would rewrite a dependency
pin instead of the package version. We therefore walk to the `[workspace.package]`
header and take the first `version =` before the next table header, and fail loudly
if that table or key is missing rather than guessing.

The version is emitted bare (`0.2.0`, no `v`); the tag name `v<version>` is the
workflow's business, not this script's.
"""

import argparse
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
CARGO_TOML = REPO_ROOT / "Cargo.toml"

TABLE_RE = re.compile(r"^\s*\[([^\]]+)\]\s*$")
VERSION_RE = re.compile(r'^(\s*version\s*=\s*")([^"]*)(".*)$')
# Plain three-part semver. Deliberately strict: pre-release/build metadata is out of
# scope for the release workflow, so a value we cannot round-trip is an error, not a
# thing to pass through silently.
SEMVER_RE = re.compile(r"^(\d+)\.(\d+)\.(\d+)$")


class BumpError(Exception):
    """A malformed manifest or version — reported without a traceback."""


def find_version_line(lines, table="workspace.package", source=CARGO_TOML):
    """Return (index, current_version) for the version key inside `table`."""
    in_table = False
    for i, line in enumerate(lines):
        m = TABLE_RE.match(line)
        if m:
            if in_table:
                break  # left the table without finding a version key
            in_table = m.group(1).strip() == table
            continue
        if in_table:
            v = VERSION_RE.match(line)
            if v:
                return i, v.group(2)
    raise BumpError(f"no `version` key found in [{table}] of {source}")


def parse(version):
    m = SEMVER_RE.match(version.strip())
    if not m:
        raise BumpError(f"not a plain X.Y.Z semver: {version!r}")
    return tuple(int(p) for p in m.groups())


def bump(version, level):
    major, minor, patch = parse(version)
    if level == "major":
        return f"{major + 1}.0.0"
    if level == "minor":
        return f"{major}.{minor + 1}.0"
    if level == "patch":
        return f"{major}.{minor}.{patch + 1}"
    raise BumpError(f"unknown bump level: {level!r}")


def write_version(lines, index, new_version):
    # Re-attach the original line ending by hand. VERSION_RE's `$` matches *before* a
    # trailing newline, so group(3) stops at the closing quote — rebuilding the line
    # from the groups alone would swallow the newline and weld this line onto the next.
    original = lines[index]
    ending = original[len(original.rstrip("\r\n")):]
    m = VERSION_RE.match(original)
    lines[index] = f"{m.group(1)}{new_version}{m.group(3)}{ending}"
    return lines


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    mode = ap.add_mutually_exclusive_group(required=True)
    mode.add_argument("--print", dest="show", action="store_true",
                      help="print the current version and exit")
    mode.add_argument("--bump", choices=("major", "minor", "patch"),
                      help="compute the next version from the current one")
    mode.add_argument("--set", dest="exact", metavar="X.Y.Z",
                      help="use this exact version")
    ap.add_argument("--write", action="store_true",
                    help="rewrite Cargo.toml in place (otherwise the result is only printed)")
    ap.add_argument("--manifest", type=Path, default=CARGO_TOML,
                    help="manifest to read/write (default: the workspace root Cargo.toml)")
    args = ap.parse_args(argv)

    try:
        lines = args.manifest.read_text(encoding="utf-8").splitlines(keepends=True)
        index, current = find_version_line(lines, source=args.manifest)

        if args.show:
            parse(current)  # validate even on a read, so a broken manifest is caught early
            print(current)
            return 0

        new_version = bump(current, args.bump) if args.bump else args.exact
        parse(new_version)

        if args.write:
            args.manifest.write_text(
                "".join(write_version(lines, index, new_version)), encoding="utf-8"
            )
        print(new_version)
        return 0
    except BumpError as exc:
        print(f"bump_version: {exc}", file=sys.stderr)
        return 1
    except OSError as exc:
        print(f"bump_version: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
