#!/bin/sh
# Fetches what the generator's TypeScript proof needs — Grafana's Foundation SDK and the
# TypeScript compiler — from the committed lockfile, into
# crates/gen/tests/typescript/node_modules/.
#
# Run it once, by hand or before verify.sh; no test fetches anything. `npm ci` installs
# exactly what package-lock.json names, each package checked against the integrity hash
# recorded there, and `--ignore-scripts` runs none of their install scripts.
# scripts/verify.sh runs the proof where Node and this directory are present, and says by
# name that it skipped it where they are not.
#
# Usage: scripts/fetch-typescript-sdk.sh

set -eu

repository_root="$(cd "$(dirname "$0")/.." && pwd)"
proof="$repository_root/crates/gen/tests/typescript"

if ! command -v npm >/dev/null; then
    echo 'error: npm is not on PATH; the TypeScript proof needs Node.js and npm' >&2
    exit 1
fi

cd "$proof"
npm ci --ignore-scripts --no-audit --no-fund
echo "fetched into $proof/node_modules:"
npm ls --depth=0
