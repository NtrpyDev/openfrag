#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 || $# -gt 4 ]]; then
  echo "usage: $0 <fixture.dem> [iterations] [output.txt] [baseline.txt]" >&2
  exit 2
fi

fixture=$1
iterations=${2:-3}
output=${3:-target/openfrag-parser-benchmark.txt}
baseline=${4:-}
manifest=benchmarks/parser-fixtures.json
binary=target/release/openfrag-parser-benchmark
time_output=$(mktemp)
trap 'rm -f "$time_output"' EXIT

cargo build --release -p openfrag-import --features demoparser --bin openfrag-parser-benchmark
mkdir -p "$(dirname "$output")"
/usr/bin/time \
  -f $'process_wall_seconds=%e\nuser_cpu_seconds=%U\nsystem_cpu_seconds=%S\nmax_rss_kib=%M' \
  -o "$time_output" \
  "$binary" "$fixture" "$manifest" "$iterations" > "$output"
cat "$time_output" >> "$output"
printf 'kernel=%s\n' "$(uname -sr)" >> "$output"
printf 'cpu_model=%s\n' "$(lscpu | awk -F: '$1 == "Model name" { sub(/^[[:space:]]+/, "", $2); print $2; exit }')" >> "$output"
printf 'rustc=%s\n' "$(rustc --version)" >> "$output"
cat "$output"

if [[ -n "$baseline" ]]; then
  [[ -f "$baseline" ]] || { echo "baseline file not found: $baseline" >&2; exit 2; }
  ratio=${OPENFRAG_BENCH_MAX_REGRESSION_RATIO:-1.20}
  for metric in median_wall_seconds normalized_events_per_second user_cpu_seconds system_cpu_seconds max_rss_kib; do
    current=$(awk -F= -v key="$metric" '$1 == key { print $2 }' "$output")
    previous=$(awk -F= -v key="$metric" '$1 == key { print $2 }' "$baseline")
    [[ -n "$current" && -n "$previous" ]] || { echo "missing benchmark metric: $metric" >&2; exit 2; }
    awk -v key="$metric" -v current="$current" -v previous="$previous" -v ratio="$ratio" '
      BEGIN {
        tolerance = 0
        if (key == "median_wall_seconds") tolerance = 0.05
        if (key == "user_cpu_seconds" || key == "system_cpu_seconds") tolerance = 0.05
        if (key == "max_rss_kib") tolerance = 8192
        if (key == "normalized_events_per_second") tolerance = 50
        if (key == "normalized_events_per_second") {
          if (current * ratio + tolerance < previous) exit 1
        } else if (current > previous * ratio + tolerance) {
          exit 1
        }
      }
    ' || { echo "benchmark regression: $metric current=$current baseline=$previous ratio=$ratio" >&2; exit 1; }
  done
fi
