#!/usr/bin/env bash
# Mirrors CI for first-party crates; it does not run vendored parser formatting or private fixtures.
set -euo pipefail

packages=(
    openfrag-analysis
    openfrag-capture
    openfrag-clips
    openfrag-domain
    openfrag-gsi
    openfrag-import
    openfrag-live
    openfrag-pipeline
    openfrag-setup
    openfrag-shortcuts
    openfrag-storage
    openfragd
)

scripts/hardware/tests/verify-nvidia-host-self-test.sh

for package in "${packages[@]}"; do
    cargo fmt --check --package "$package"
done

cargo metadata --locked --no-deps --format-version 1 >/dev/null
for package in "${packages[@]}"; do
    cargo test --locked -p "$package"
    cargo clippy --locked -p "$package" --no-deps -- -D warnings
done
cargo test --locked -p openfrag-import --features demoparser
cargo test --locked -p openfrag-pipeline --features demoparser
cargo check --locked -p openfrag-import --features demoparser --bin openfrag-parser-benchmark
cargo run --locked -q -p openfrag-import --bin openfrag-schema -- \
    --check schema/openfrag-demo-evidence-1.schema.json
cargo run --locked -q -p openfrag-import --features demoparser --bin openfrag-compat-lab -- \
    check-manifest benchmarks/parser-fixtures.json
scripts/fuzz-demo-parser.sh check
bash -n scripts/benchmark-parser.sh
bash -n scripts/fuzz-demo-parser.sh
cargo build --locked -p openfragd --release
packaging/verify-linux-assets.sh
scripts/hardware/tests/verify-nvidia-host-self-test.sh
scripts/e2e/headless-smoke.sh
cargo build --locked -p openfragd --release --features acceptance-fixtures
python3 scripts/e2e/installed_mvp.py --binary target/release/openfragd
