#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

cargo build --quiet --package openfragd
exec python3 scripts/e2e/headless_smoke.py --binary target/debug/openfragd
