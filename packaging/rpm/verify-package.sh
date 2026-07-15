#!/usr/bin/env bash
# Verifies the complete COPR RPM lifecycle inside a disposable Fedora environment.
set -euo pipefail

if (( $# != 1 )) || (( EUID != 0 )); then
    printf '%s\n' 'Usage: sudo packaging/rpm/verify-package.sh <openfrag.rpm>' >&2
    exit 2
fi

repo=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
package=$(realpath -- "$1")
stage=$(mktemp -d "${TMPDIR:-/tmp}/openfrag-rpm-lifecycle.XXXXXXXX")
installed=false
cleanup() {
    if $installed; then
        dnf remove -y openfrag >/dev/null 2>&1 || true
    fi
    rm -rf -- "$stage"
}
trap cleanup EXIT HUP INT TERM

source /etc/os-release
test "${ID:-}" = fedora
test -f "$package"
if rpm -q openfrag >/dev/null 2>&1; then
    printf '%s\n' 'Refusing to replace an existing openfrag installation.' >&2
    exit 1
fi
if [[ -n $(rpm -qp --scripts "$package") ]]; then
    printf '%s\n' 'RPM package must not contain install or upgrade scriptlets.' >&2
    exit 1
fi

dnf install -y "$package" >/dev/null
installed=true
"$repo/packaging/verify-channel-packages.sh" --root-only /
PYTHONDONTWRITEBYTECODE=1 HOME="$stage/home" python3 "$repo/scripts/e2e/headless_smoke.py" --binary /usr/bin/openfragd

mkdir -p "$stage/home/.local/share/openfrag" "$stage/home/.config/openfrag"
printf '%s\n' preserved >"$stage/home/.local/share/openfrag/package-lifecycle"
printf '%s\n' preserved >"$stage/home/.config/openfrag/package-lifecycle"
dnf reinstall -y "$package" >/dev/null
test -f "$stage/home/.local/share/openfrag/package-lifecycle"
test -f "$stage/home/.config/openfrag/package-lifecycle"

dnf remove -y openfrag >/dev/null
installed=false
test ! -e /usr/bin/openfragd
test ! -e /usr/bin/openfrag-launch
test ! -e /usr/share/applications/io.github.ntrpydev.openfrag.desktop
test ! -e /usr/lib/systemd/user/app-io.github.ntrpydev.openfrag.service
test -f "$stage/home/.local/share/openfrag/package-lifecycle"
test -f "$stage/home/.config/openfrag/package-lifecycle"
printf '%s\n' 'COPR install, upgrade, smoke, and removal lifecycle passed.'
