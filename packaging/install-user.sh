#!/usr/bin/env bash
# Installs a built OpenFrag daemon and dashboard launcher for one Linux user.
set -euo pipefail

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
staged=false
if [[ ${1:-} == --staged ]]; then
    staged=true
    shift
fi
if (( $# > 1 )); then
    printf '%s\n' 'Usage: packaging/install-user.sh [--staged] [openfragd-binary]' >&2
    exit 2
fi

binary=${1:-$root/target/release/openfragd}
prefix=${HOME}/.local

if [[ ! -x ${binary} ]]; then
    printf 'OpenFrag binary is missing or not executable: %s\n' "${binary}" >&2
    printf '%s\n' 'Run cargo build --release --package openfragd first.' >&2
    exit 1
fi

install -d -m 0755 \
    "${prefix}/bin" \
    "${prefix}/share/applications" \
    "${HOME}/.config/systemd/user"
install -m 0755 "${binary}" "${prefix}/bin/openfragd"
install -m 0755 "$root/packaging/linux/openfrag-open-dashboard" \
    "${prefix}/bin/openfrag-open-dashboard"
install -m 0644 "$root/packaging/linux/openfrag.desktop" \
    "${prefix}/share/applications/openfrag.desktop"
install -m 0644 "$root/packaging/linux/openfragd.service" \
    "${HOME}/.config/systemd/user/openfragd.service"

if [[ $staged == false ]] && command -v systemctl >/dev/null 2>&1; then
    systemctl --user daemon-reload
fi

if [[ $staged == true ]]; then
    printf '%s\n' 'OpenFrag was staged for the current user without activating systemd.'
else
    printf '%s\n' 'OpenFrag was installed for the current user.'
    printf '%s\n' 'Start it with: systemctl --user enable --now openfragd.service'
fi
