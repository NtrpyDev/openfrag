#!/usr/bin/env bash
# Checks formatting only for first-party workspace crates and excludes vendored parser sources.
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
