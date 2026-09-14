#!/usr/bin/env python3
"""Warning policy gate for openroad (`make warnings` and CI).

Policy (from README.md / CLAUDE.md): reject every *rustc compiler* warning
except `dead_code` — parsed-but-not-yet-consumed SRO data structures keep their
fields, methods and variants on purpose (many already carry an explicit
`#[allow(dead_code)]`). This gate is about compiler warnings only; clippy's
style lints are a separate, non-blocking `make clippy` by project convention.

Runs `cargo check` over the workspace's shipped targets (lib + bins). Test
targets are compiled and checked by the `cargo test` step instead.
"""

import json
import subprocess
import sys

CHECK_CMD = [
    "cargo",
    "check",
    "--workspace",
    "--message-format=json",
]

# The single carve-out: intentional dead code for SRO data we parse but do not
# consume yet.
ALLOWED_CODES = {"dead_code"}


def main() -> int:
    proc = subprocess.run(CHECK_CMD, capture_output=True, text=True)

    rejected: list[str] = []
    errors: list[str] = []
    seen: set[str] = set()
    for line in proc.stdout.splitlines():
        try:
            entry = json.loads(line)
        except json.JSONDecodeError:
            continue
        if entry.get("reason") != "compiler-message":
            continue
        message = entry.get("message") or {}
        level = message.get("level")
        rendered = message.get("rendered") or message.get("message", "")
        if level == "error":
            if rendered not in seen:
                seen.add(rendered)
                errors.append(rendered)
            continue
        if level != "warning":
            continue
        code = (message.get("code") or {}).get("code") or ""
        if code in ALLOWED_CODES:
            continue
        if rendered not in seen:
            seen.add(rendered)
            rejected.append(rendered)

    if proc.returncode != 0:
        # A compile error. `--message-format=json` puts the *diagnostics* on
        # stdout and leaves stderr with little more than "could not compile
        # `client` (bin ...) due to 1 previous error" — so echoing stderr alone
        # produces a gate log that says a build failed without saying why, and
        # the next reader has to reproduce the build to find out. Print the
        # error-level messages we already parsed, then stderr for the summary.
        for rendered in errors:
            print(rendered)
        sys.stderr.write(proc.stderr)
        print(f"check_warnings: cargo check failed (exit {proc.returncode})")
        return proc.returncode

    if rejected:
        print(f"check_warnings: {len(rejected)} disallowed compiler warning(s):\n")
        for rendered in rejected:
            print(rendered)
        return 1

    print("check_warnings: OK (no disallowed compiler warnings)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
