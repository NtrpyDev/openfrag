#!/usr/bin/env bash
# Runs the machine-checked whole-app verifier through its shared local and CI shard graph.
set -euo pipefail

profile=${1:-complete}
if [[ $profile != complete && $profile != fast ]]; then
    printf '%s\n' 'Usage: scripts/verify-ci.sh [complete|fast]' >&2
    exit 2
fi

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo"
exec python3 scripts/test_suite.py "$profile"
