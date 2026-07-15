#!/usr/bin/env bash
# Builds deterministic local Linux v1 release archive and checksum from an existing daemon binary.
set -euo pipefail

if (( $# != 2 )); then
    printf '%s\n' 'Usage: packaging/build-release-bundle.sh <openfragd-binary> <output-directory>' >&2
    exit 2
fi

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
source "$root/packaging/application-identity.sh"
binary=$1
output=$2
bundle_name=openfrag-v1.0.0-linux-x86_64
archive_name="$bundle_name.tar.gz"
checksum_name="$archive_name.sha256"

if [[ ! -f $binary || ! -x $binary || -L $binary ]]; then
    printf 'OpenFrag binary must be an executable regular file, not a symlink: %s\n' "$binary" >&2
    exit 1
fi

mkdir -p "$output"
output=$(CDPATH= cd -- "$output" && pwd)
stage=$(mktemp -d "${TMPDIR:-/tmp}/openfrag-release-build.XXXXXXXX")
archive_temporary=$(mktemp "$output/.${archive_name}.XXXXXXXX")
checksum_temporary=$(mktemp "$output/.${checksum_name}.XXXXXXXX")
cleanup() {
    rm -rf -- "$stage"
    rm -f -- "$archive_temporary" "$checksum_temporary"
}
trap cleanup EXIT HUP INT TERM

bundle="$stage/$bundle_name"
install -D -m 0755 "$binary" "$bundle/target/release/$OPENFRAG_DAEMON_NAME"
install -D -m 0644 "$root/LICENSE" "$bundle/LICENSE"
install -D -m 0644 "$root/packaging/linux/THIRD_PARTY_NOTICES.md" \
    "$bundle/THIRD_PARTY_NOTICES.md"
install -D -m 0644 "$root/packaging/application-identity.sh" \
    "$bundle/packaging/application-identity.sh"
install -D -m 0755 "$root/packaging/install-user.sh" "$bundle/packaging/install-user.sh"
install -D -m 0755 "$root/packaging/linux/$OPENFRAG_LAUNCHER_NAME" \
    "$bundle/packaging/linux/$OPENFRAG_LAUNCHER_NAME"
install -D -m 0644 "$root/packaging/linux/$OPENFRAG_DESKTOP_NAME" \
    "$bundle/packaging/linux/$OPENFRAG_DESKTOP_NAME"
install -D -m 0644 "$root/packaging/linux/$OPENFRAG_USER_SERVICE" \
    "$bundle/packaging/linux/$OPENFRAG_USER_SERVICE"

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

printf 'Archive: %s\nChecksum: %s\n' "$archive" "$checksum"
