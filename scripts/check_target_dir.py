#!/usr/bin/env python3
"""Refuse to gate against a target dir another git worktree built into (#374).

Cargo's metadata hash does not include the worktree path, and its dep-info
records *absolute* source paths. Two worktrees of this repo sharing one target
dir therefore resolve to the same artifact path, and a freshness check made from
worktree B can be satisfied by worktree A's source files. Both directions were
reproduced on 2026-08-12: a `cargo test` that passed on a binary which never
contained the new tests (false green), and a stale sibling rlib that fabricated
type errors in a file the diff never touched (false red).

There is no cargo-side fix and no way to detect it after the fact, so the gate
stamps the target dir with the worktree that owns it and refuses when that
changes. The stamp lives inside the target dir, so `cargo clean` resets it.

Run: `python3 scripts/check_target_dir.py`
"""

import os
import subprocess
import sys
from pathlib import Path

STAMP_NAME = ".openroad-gate-owner"
OVERRIDE_ENV = "OPENROAD_ALLOW_FOREIGN_TARGET"


def worktree_root() -> Path:
    """The checkout `make` was invoked from, as git sees it."""
    out = subprocess.run(
        ["git", "rev-parse", "--show-toplevel"],
        capture_output=True,
        text=True,
        check=True,
    )
    return Path(out.stdout.strip()).resolve()


def target_dir(root: Path) -> Path:
    """Where cargo will put artifacts. Mirrors cargo's own precedence."""
    # We deliberately do not parse `.cargo/config.toml`'s `build.target-dir`:
    # this repo ships no such file, and guessing cargo's config discovery would
    # make the guard lie. If one is ever added, teach this function about it.
    env = os.environ.get("CARGO_TARGET_DIR")
    return Path(env).resolve() if env else root / "target"


def main() -> int:
    root = worktree_root()
    target = target_dir(root)
    stamp = target / STAMP_NAME

    # Every gate stamps, and every gate checks — including a target dir inside
    # this very worktree. An earlier version of this exempted `./target` on the
    # premise that a dir inside a worktree cannot be shared. That premise is
    # false: a sibling shares it by naming it, and the old runbook told every
    # worker to do exactly that (`CARGO_TARGET_DIR=<openroad>/
    # target` from a per-issue worktree). The exemption meant the anchor never
    # wrote a stamp, so the first sibling to point at the anchor's target found
    # none, was allowed, and claimed it — the precise incident this guard exists
    # for went undetected. Stamping unconditionally also means the reverse is
    # caught: if a sibling got there first, the owner's own next gate fails,
    # which is right, because its artifacts are contaminated too.
    owner = stamp.read_text().strip() if stamp.is_file() else None

    if owner is not None and owner != str(root):
        if os.environ.get(OVERRIDE_ENV):
            print(
                f"check_target_dir: WARNING — {target} was last built by {owner}, "
                f"continuing because {OVERRIDE_ENV} is set. A green gate from here "
                f"is not evidence (#374)."
            )
        else:
            print(
                f"check_target_dir: refusing to gate against a foreign target dir\n"
                f"\n"
                f"  target dir     {target}\n"
                f"  last built by  {owner}\n"
                f"  this worktree  {root}\n"
                f"\n"
                f"Cargo would reuse artifacts fingerprinted against the other\n"
                f"worktree's sources, so `make ci` could pass on a binary that\n"
                f"never contained this diff, or fail on code that is correct (#374).\n"
                f"\n"
                f"Gate against a dir of your own instead:\n"
                f"  CARGO_TARGET_DIR=/tmp/gate-<N> make ci && rm -rf /tmp/gate-<N>\n"
                f"or drop CARGO_TARGET_DIR to use this worktree's own ./target.\n"
                f"Set {OVERRIDE_ENV}=1 only if you know why that is safe.",
                file=sys.stderr,
            )
            return 1

    target.mkdir(parents=True, exist_ok=True)
    stamp.write_text(f"{root}\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
