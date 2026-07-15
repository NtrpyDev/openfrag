#!/usr/bin/env bash
# Builds or runs the bounded Demo decoder and normalizer fuzz targets.
set -euo pipefail

manifest="crates/openfrag-import/fuzz/Cargo.toml"

if [[ "${1:-}" == "check" ]]; then
    cargo check --manifest-path "$manifest" --bins
    exit 0
fi

seconds="${OPENFRAG_FUZZ_SECONDS:-60}"
cargo fuzz run raw_decoder --fuzz-dir crates/openfrag-import/fuzz -- \
    -max_len=16777216 -timeout=10 -rss_limit_mb=2048 -max_total_time="$seconds"
cargo fuzz run normalizer --fuzz-dir crates/openfrag-import/fuzz -- \
    -max_len=65536 -timeout=10 -rss_limit_mb=2048 -max_total_time="$seconds"
