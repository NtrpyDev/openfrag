#!/usr/bin/env bash
# Headless verification only: staged installation stays in a temporary HOME.
set -euo pipefail

root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
service="$root/linux/openfragd.service"
desktop="$root/linux/openfrag.desktop"
helper="$root/linux/openfrag-open-dashboard"
installer="$root/install-user.sh"
install_verifier="$root/verify-user-install.sh"

require_line() {
    grep -Fqx -- "$2" "$1" || { printf 'missing required line: %s\n' "$2" >&2; exit 1; }
}

require_line "$service" 'ExecStart=%h/.local/bin/openfragd serve --bind 127.0.0.1:7130'
require_line "$service" 'Restart=on-failure'
require_line "$service" 'NoNewPrivileges=yes'
require_line "$service" 'ProtectSystem=strict'
require_line "$service" 'UMask=0077'
require_line "$desktop" 'Exec=openfrag-open-dashboard'
require_line "$helper" 'host=127.0.0.1'
require_line "$helper" 'port=7130'
grep -Fq '/dev/tcp/${host}/${port}' "$helper"
grep -Fq 'exec xdg-open "${url}"' "$helper"

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
if command -v desktop-file-validate >/dev/null 2>&1; then
    desktop-file-validate "$desktop"
fi
"$install_verifier"
printf '%s\n' 'Linux packaging assets passed headless verification.'
