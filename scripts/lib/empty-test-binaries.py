"""Names the test binaries that collected no test, from `cargo nextest list --message-format json` on stdin.

    python3 scripts/lib/empty-test-binaries.py every
    python3 scripts/lib/empty-test-binaries.py integration

A test binary that collects zero tests is either dead code or gated out of the run. Both are defects, and both pass
green: cargo prints "running 0 tests" and exits 0. bc-error-rs once shipped five serde tests that had never executed,
because its test step omitted the feature flag its clippy step carried.

`every` judges every binary the listing names; `integration` only the `tests/` suites, for a member whose libraries
hold no unit tests by layout, which would otherwise need the allowlist this refuses to have.

Exit codes: 0 every judged binary holds a test; 1 some did not, each named on stdout; 2 the listing could not be read
or judged nothing, which is never a pass; 64 a malformed command line.
"""

from __future__ import annotations

import json
import sys

KINDS = {"every": None, "integration": "test"}


def empty_binaries(listing: dict, kind: str | None) -> tuple[list[str], int]:
    """The judged binaries that hold no test, and how many binaries were judged."""
    judged = 0
    empty: list[str] = []
    for binary_id, suite in listing["rust-suites"].items():
        if kind is not None and suite.get("kind") != kind:
            continue
        judged += 1
        if not suite.get("testcases"):
            empty.append(binary_id)
    return sorted(empty), judged


def main(argv: list[str], listing_text: str) -> int:
    if len(argv) != 1 or argv[0] not in KINDS:
        print(__doc__.strip().split("\n\n")[0], file=sys.stderr)
        return 64
    try:
        empty, judged = empty_binaries(json.loads(listing_text), KINDS[argv[0]])
    except (json.JSONDecodeError, KeyError, AttributeError) as error:
        print(f"error: the nextest listing could not be read: {error!r}", file=sys.stderr)
        return 2
    if judged == 0:
        print("error: the nextest listing named no test binary to judge, so this guard verified nothing",
              file=sys.stderr)
        return 2
    for binary_id in empty:
        print(binary_id)
    return 1 if empty else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:], sys.stdin.read()))
