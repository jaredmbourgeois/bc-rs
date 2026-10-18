#!/usr/bin/env bash
# Runs the release benchmarks and prints their table.

set -euo pipefail

cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cargo bench --locked "$@"
