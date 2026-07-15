#!/usr/bin/env bash
# Builds the deterministic vendored source archive consumed by native package channels.
set -euo pipefail

if (( $# != 1 )); then
    printf '%s\n' 'Usage: packaging/build-source-bundle.sh <output-directory>' >&2
    exit 2
fi

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
output=$1
version=$(sed -n '/^\[workspace\.package\]$/,/^\[/s/^version = "\([0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*\)"$/\1/p' "$root/Cargo.toml")
if [[ ! $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    printf '%s\n' 'Cargo.toml must contain exactly one numeric workspace package version.' >&2
    exit 1
fi

bundle_name="openfrag-v${version}-source"
archive_name="$bundle_name.tar.gz"
checksum_name="$archive_name.sha256"
mkdir -p "$output"
output=$(CDPATH= cd -- "$output" && pwd)
stage=$(mktemp -d "${TMPDIR:-/tmp}/openfrag-source-build.XXXXXXXX")
archive_temporary=$(mktemp "$output/.${archive_name}.XXXXXXXX")
checksum_temporary=$(mktemp "$output/.${checksum_name}.XXXXXXXX")
cleanup() {
    rm -rf -- "$stage"
    rm -f -- "$archive_temporary" "$checksum_temporary"
}
trap cleanup EXIT HUP INT TERM

bundle="$stage/$bundle_name"
mkdir -p "$bundle"
(
    cd "$root"
    git ls-files -z -- \
        Cargo.toml \
        Cargo.lock \
        LICENSE \
        crates \
        packaging/application-identity.sh \
        packaging/linux \
        schema |
        tar --null --files-from=- -cf -
) | tar -xf - -C "$bundle"

mkdir -p "$bundle/.cargo"
(
    cd "$bundle"
    cargo +1.96.0 vendor \
        --locked \
        --versioned-dirs \
        --manifest-path "$root/Cargo.toml" \
        vendor >.cargo/config.toml
)

LC_ALL=C TZ=UTC tar \
    --sort=name \
    --format=gnu \
    --mtime=@0 \
    --owner=0 \
    --group=0 \
    --numeric-owner \
    -C "$stage" \
    -cf - "$bundle_name" | gzip -n -9 >"$archive_temporary"

archive="$output/$archive_name"
checksum="$output/$checksum_name"
mv -f -- "$archive_temporary" "$archive"
archive_temporary=
(
    cd "$output"
    sha256sum "$archive_name" >"$checksum_temporary"
)
mv -f -- "$checksum_temporary" "$checksum"
checksum_temporary=

printf 'Source archive: %s\nChecksum: %s\n' "$archive" "$checksum"
