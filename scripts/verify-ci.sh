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

for package in "${packages[@]}"; do
    cargo fmt --check --package "$package"
done

cargo metadata --no-deps --format-version 1 >/dev/null
for package in "${packages[@]}"; do
    cargo test -p "$package"
    cargo clippy -p "$package" --no-deps -- -D warnings
done
cargo test -p openfrag-import --features demoparser
cargo test -p openfrag-pipeline --features demoparser
cargo build -p openfragd --release
packaging/verify-linux-assets.sh

if rg -n 'https?://(?!127\.0\.0\.1)' --pcre2 packaging/linux crates/openfragd; then
    printf '%s\n' 'remote runtime endpoint found' >&2
    exit 1
fi
