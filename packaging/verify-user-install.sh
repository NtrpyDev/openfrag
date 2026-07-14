#!/usr/bin/env bash
# Verifies a staged per-user install without touching the real home or desktop session.
set -euo pipefail

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
stage=$(mktemp -d "${TMPDIR:-/tmp}/openfrag-user-install.XXXXXXXX")
trap 'rm -rf -- "$stage"' EXIT HUP INT TERM

home="$stage/home"
guard_bin="$stage/guard-bin"
fixture="$stage/openfragd"
systemctl_log="$stage/systemctl-called"
mkdir -p "$home" "$guard_bin"

cat >"$fixture" <<'EOF'
#!/usr/bin/env bash
exit 97
EOF
chmod 0755 "$fixture"

cat >"$guard_bin/systemctl" <<EOF
#!/usr/bin/env bash
touch '$systemctl_log'
exit 96
EOF
chmod 0755 "$guard_bin/systemctl"

(
    cd "$stage"
    HOME="$home" PATH="$guard_bin:$PATH" "$root/packaging/install-user.sh" --staged "$fixture"
)

binary="$home/.local/bin/openfragd"
dashboard="$home/.local/bin/openfrag-open-dashboard"
desktop="$home/.local/share/applications/openfrag.desktop"
service="$home/.config/systemd/user/openfragd.service"

test -x "$binary"
test -x "$dashboard"
test -f "$desktop"
test -f "$service"
test "$(stat -c '%a' "$binary")" = 755
test "$(stat -c '%a' "$dashboard")" = 755
test "$(stat -c '%a' "$desktop")" = 644
test "$(stat -c '%a' "$service")" = 644
cmp -s "$fixture" "$binary"
cmp -s "$root/packaging/linux/openfrag-open-dashboard" "$dashboard"
cmp -s "$root/packaging/linux/openfrag.desktop" "$desktop"
cmp -s "$root/packaging/linux/openfragd.service" "$service"
grep -Fqx 'Exec=openfrag-open-dashboard' "$desktop"
grep -Fqx 'ExecStart=%h/.local/bin/openfragd serve --bind 127.0.0.1:7130' "$service"
test ! -e "$systemctl_log"

printf '%s\n' 'Staged per-user installation passed headless verification.'
