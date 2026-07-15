#!/usr/bin/env bash
# Headless verification only: staged installation stays in a temporary HOME.
set -euo pipefail

if (( $# > 1 )) || (( $# == 1 )) && [[ $1 != --assets-only ]]; then
    printf '%s\n' 'Usage: packaging/verify-linux-assets.sh [--assets-only]' >&2
    exit 2
fi

root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
source "$root/application-identity.sh"
service="$root/linux/$OPENFRAG_USER_SERVICE"
desktop="$root/linux/$OPENFRAG_DESKTOP_NAME"
helper="$root/linux/$OPENFRAG_LAUNCHER_NAME"
installer="$root/install-user.sh"
install_verifier="$root/verify-user-install.sh"
release_builder="$root/build-release-bundle.sh"
release_verifier="$root/verify-release-bundle.sh"
source_builder="$root/build-source-bundle.sh"
channel_verifier="$root/verify-channel-packages.sh"
aur_verifier="$root/aur/verify-package.sh"
rpm_verifier="$root/rpm/verify-package.sh"
verification_root=$(mktemp -d "${TMPDIR:-/tmp}/openfrag-systemd-verify.XXXXXXXX")
trap 'rm -rf -- "$verification_root"' EXIT HUP INT TERM

require_line() {
    grep -Fqx -- "$2" "$1" || { printf 'missing required line: %s\n' "$2" >&2; exit 1; }
}

require_line "$service" 'ExecStart=%h/.local/bin/openfragd serve --bind 127.0.0.1:7130'
require_line "$service" 'Slice=app.slice'
require_line "$service" 'Restart=on-failure'
require_line "$service" 'NoNewPrivileges=yes'
require_line "$service" 'ProtectSystem=strict'
require_line "$service" 'UMask=0077'
require_line "$desktop" 'Exec=openfrag-launch'
require_line "$desktop" 'TryExec=openfrag-launch'
require_line "$desktop" 'Icon=io.github.ntrpydev.openfrag'
require_line "$helper" "service=$OPENFRAG_USER_SERVICE"
require_line "$helper" 'host=127.0.0.1'
require_line "$helper" 'port=7130'
grep -Fq '/dev/tcp/${host}/${port}' "$helper"
grep -Fq 'systemctl --user start "$service"' "$helper"
grep -Fq 'exec xdg-open "${url}"' "$helper"
if grep -Eq 'systemctl .*enable' "$helper"; then
    printf '%s\n' 'the desktop launcher must not enable the user service' >&2
    exit 1
fi
test "$OPENFRAG_APPLICATION_ID" = io.github.ntrpydev.openfrag
test "$OPENFRAG_DESKTOP_NAME" = io.github.ntrpydev.openfrag.desktop
test "$OPENFRAG_LAUNCHER_NAME" = openfrag-launch
test "$OPENFRAG_DAEMON_NAME" = openfragd
test "$OPENFRAG_USER_SERVICE" = app-io.github.ntrpydev.openfrag.service
test "$(basename "$desktop")" = "$OPENFRAG_DESKTOP_NAME"
test "$(basename "$service")" = "$OPENFRAG_USER_SERVICE"
test ! -e "$root/linux/openfrag.desktop"
test ! -e "$root/linux/openfragd.service"
test ! -e "$root/linux/openfrag-open-dashboard"
prototype_identity='app.openfrag.Portal''Probe'
if grep -RFn "$prototype_identity" "$root"; then
    printf '%s\n' 'prototype portal identity is not allowed in package assets' >&2
    exit 1
fi

if grep -Ein 'telemetry|analytics|sentry|metrics' "$service" "$desktop" "$helper"; then
    printf '%s\n' 'telemetry-related content is not allowed in local launch assets' >&2
    exit 1
fi
if grep -En ':[0-9]+' "$service" "$desktop" "$helper" | grep -Ev ':7130([^0-9]|$)'; then
    printf '%s\n' 'only the canonical local port 7130 is allowed in launch assets' >&2
    exit 1
fi
remote_endpoints=$(grep -Eho 'https?://[^[:space:]]+' "$service" "$desktop" "$helper" | grep -Fv 'http://127.0.0.1:7130/' | grep -Fv 'http://${host}:${port}/' || true)
if [ -n "$remote_endpoints" ]; then
    printf '%s\n%s\n' 'remote endpoint is not allowed in local launch assets:' "$remote_endpoints" >&2
    exit 1
fi

bash -n "$helper"
bash -n "$installer"
bash -n "$install_verifier"
bash -n "$release_builder"
bash -n "$release_verifier"
bash -n "$source_builder"
bash -n "$channel_verifier"
bash -n "$aur_verifier"
bash -n "$rpm_verifier"
command -v desktop-file-validate >/dev/null 2>&1 || {
    printf '%s\n' 'desktop-file-validate is required for package verification' >&2
    exit 1
}
command -v systemd-analyze >/dev/null 2>&1 || {
    printf '%s\n' 'systemd-analyze is required for package verification' >&2
    exit 1
}
desktop-file-validate "$desktop"
rendered_service="$verification_root/$OPENFRAG_USER_SERVICE"
sed 's|^ExecStart=.*|ExecStart=/bin/true|' "$service" >"$rendered_service"
systemd-analyze --user --man=no verify "$rendered_service"
if (( $# == 1 )); then
    printf '%s\n' 'Linux desktop and user-service assets passed headless verification.'
    exit 0
fi
"$install_verifier"
"$release_verifier"
"$channel_verifier"
printf '%s\n' 'Linux packaging assets passed headless verification.'
