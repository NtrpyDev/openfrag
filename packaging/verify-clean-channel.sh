#!/usr/bin/env bash
# Builds and exercises one native package inside its clean distribution container.
set -euo pipefail

if (( $# != 2 )); then
    printf '%s\n' 'Usage: packaging/verify-clean-channel.sh <aur|fedora43|fedora44> <source-archive>' >&2
    exit 2
fi

channel=$1
source_archive=$(realpath -- "$2")
repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
case "$channel" in
    aur) cleanup_image=archlinux:latest ;;
    fedora43|fedora44) cleanup_image="fedora:${channel#fedora}" ;;
    *)
        printf 'Unknown native channel: %s\n' "$channel" >&2
        exit 2
        ;;
esac
stage=$(mktemp -d "${TMPDIR:-/tmp}/openfrag-clean-channel.XXXXXXXX")
container="openfrag-${channel//[^a-zA-Z0-9_.-]/-}-$$"
cleanup() {
    docker rm -f "$container" >/dev/null 2>&1 || true
    if ! rm -rf -- "$stage" 2>/dev/null; then
        docker run --rm --mount "type=bind,src=$stage,dst=/work" \
            "$cleanup_image" sh -c 'find /work -mindepth 1 -delete' >/dev/null 2>&1 || true
        rm -rf -- "$stage"
    fi
}
trap cleanup EXIT HUP INT TERM

test -f "$source_archive"
expected_sha=$(sed -n "s/^sha256sums=('\([0-9a-f]\{64\}\)')$/\1/p" "$repo/packaging/aur/PKGBUILD")
actual_sha=$(sha256sum "$source_archive")
actual_sha=${actual_sha%% *}
test "$actual_sha" = "$expected_sha"
install -m 0644 "$source_archive" "$stage/openfrag-v1.0.0-source.tar.gz"
docker info >/dev/null

case "$channel" in
    aur)
        install -m 0644 "$repo/packaging/aur/PKGBUILD" "$stage/PKGBUILD"
        install -m 0644 "$repo/packaging/aur/.SRCINFO" "$stage/.SRCINFO"
        docker run --rm --name "$container" \
            --mount "type=bind,src=$repo,dst=/repo,readonly" \
            --mount "type=bind,src=$stage,dst=/work" \
            --env CARGO_BUILD_JOBS="${OPENFRAG_PACKAGE_BUILD_JOBS:-2}" \
            archlinux:latest bash -lc '
                set -euo pipefail
                pacman -Syu --noconfirm --needed base-devel cargo rust desktop-file-utils ffmpeg systemd xdg-utils namcap diffutils python >/dev/null
                useradd --create-home builder
                chown -R builder:builder /work
                su builder -c "cd /work && makepkg --cleanbuild --force --nodeps --noconfirm"
                package=$(find /work -maxdepth 1 -type f -name "openfrag-[0-9]*-x86_64.pkg.tar.zst" -print -quit)
                test -n "$package"
                namcap /work/PKGBUILD "$package"
                /repo/packaging/aur/verify-package.sh "$package"
            '
        ;;
    fedora43|fedora44)
        release=${channel#fedora}
        docker run --rm --name "$container" \
            --mount "type=bind,src=$repo,dst=/repo,readonly" \
            --mount "type=bind,src=$stage,dst=/work" \
            --env CARGO_BUILD_JOBS="${OPENFRAG_PACKAGE_BUILD_JOBS:-2}" \
            "fedora:$release" bash -lc '
                set -euo pipefail
                dnf install -y cargo rust rpm-build rpmlint desktop-file-utils ffmpeg-free systemd xdg-utils python3 diffutils >/dev/null
                mkdir -p /work/rpmbuild/{BUILD,BUILDROOT,RPMS,SOURCES,SPECS,SRPMS}
                install -m 0644 /work/openfrag-v1.0.0-source.tar.gz /work/rpmbuild/SOURCES/
                rpmbuild -ba /repo/packaging/rpm/openfrag.spec --define "_topdir /work/rpmbuild"
                rpm=$(find /work/rpmbuild/RPMS -type f -name "openfrag-[0-9]*.x86_64.rpm" -print -quit)
                srpm=$(find /work/rpmbuild/SRPMS -type f -name "openfrag-[0-9]*.src.rpm" -print -quit)
                test -n "$rpm" && test -n "$srpm"
                rpmlint "$rpm" "$srpm"
                /repo/packaging/rpm/verify-package.sh "$rpm"
                rm -rf /work/rpmbuild
            '
        ;;
esac

printf 'Clean %s source build and package lifecycle passed.\n' "$channel"
