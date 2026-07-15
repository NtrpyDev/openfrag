#!/usr/bin/env bash
# Verifies canonical staged install, upgrade, and uninstall without touching the real session.
set -euo pipefail

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
source "$root/packaging/application-identity.sh"
stage=$(mktemp -d "${TMPDIR:-/tmp}/openfrag-user-install.XXXXXXXX")
trap 'rm -rf -- "$stage"' EXIT HUP INT TERM

home="$stage/home with space"
data_home="$home/data"
config_home="$home/config"
guard_bin="$stage/guard-bin"
fixture="$stage/openfragd"
upgrade="$stage/openfragd-upgrade"
systemctl_log="$stage/systemctl-called"
mkdir -p \
    "$home/.local/bin" \
    "$data_home/applications" \
    "$config_home/systemd/user" \
    "$guard_bin"

printf '#!/usr/bin/env bash\nexit 97\n' >"$fixture"
printf '#!/usr/bin/env bash\nprintf "upgraded-openfragd\\n"\n' >"$upgrade"
chmod 0755 "$fixture" "$upgrade"

printf '#!/usr/bin/env bash\ntouch %q\nexit 96\n' "$systemctl_log" >"$guard_bin/systemctl"
chmod 0755 "$guard_bin/systemctl"

cat >"$data_home/applications/openfrag.desktop" <<'EOF'
[Desktop Entry]
Type=Application
Version=1.0
Name=OpenFrag
Comment=Open the local OpenFrag dashboard
Exec=openfrag-open-dashboard
Terminal=false
Categories=Utility;
StartupNotify=true
EOF
cat >"$config_home/systemd/user/openfragd.service" <<'EOF'
[Unit]
Description=OpenFrag local dashboard daemon
After=graphical-session.target

[Service]
Type=simple
Environment=XDG_STATE_HOME=%S
Environment=XDG_CACHE_HOME=%C
Environment=XDG_RUNTIME_DIR=%t
ExecStart=%h/.local/bin/openfragd serve --bind 127.0.0.1:7130
Restart=on-failure
RestartSec=2s
UMask=0077
NoNewPrivileges=yes
PrivateTmp=yes
ProtectSystem=strict
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectControlGroups=yes
RestrictSUIDSGID=yes
LockPersonality=yes
RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6
SystemCallArchitectures=native

[Install]
WantedBy=default.target
EOF
cat >"$home/.local/bin/openfrag-open-dashboard" <<'EOF'
#!/usr/bin/env bash
# Installed as /usr/libexec/openfrag/openfrag-open-dashboard by a distro package.
set -eu

host=127.0.0.1
port=7130
url="http://${host}:${port}/"

if ! (exec 3<>"/dev/tcp/${host}/${port}") 2>/dev/null; then
    if command -v notify-send >/dev/null 2>&1; then
        notify-send --app-name=OpenFrag 'OpenFrag is not running' 'Start the openfragd user service, then try again.'
    else
        printf '%s\n' 'OpenFrag is not running; start the openfragd user service first.' >&2
    fi
    exit 1
fi
exec 3>&-
exec xdg-open "${url}"
EOF
chmod 0755 "$home/.local/bin/openfrag-open-dashboard"

run_installer() {
    HOME="$home" XDG_DATA_HOME="$data_home" XDG_CONFIG_HOME="$config_home" \
        PATH="$guard_bin:$PATH" "$root/packaging/install-user.sh" --staged "$@"
}

run_installer "$fixture"

binary="$home/.local/bin/$OPENFRAG_DAEMON_NAME"
launcher="$home/.local/bin/$OPENFRAG_LAUNCHER_NAME"
desktop="$data_home/applications/$OPENFRAG_DESKTOP_NAME"
service="$config_home/systemd/user/$OPENFRAG_USER_SERVICE"

for path in "$binary" "$launcher" "$desktop" "$service"; do
    test -e "$path"
done
test "$(stat -c '%a' "$binary")" = 755
test "$(stat -c '%a' "$launcher")" = 755
test "$(stat -c '%a' "$desktop")" = 644
test "$(stat -c '%a' "$service")" = 644
cmp -s "$fixture" "$binary"
cmp -s "$root/packaging/linux/$OPENFRAG_LAUNCHER_NAME" "$launcher"
cmp -s "$root/packaging/linux/$OPENFRAG_USER_SERVICE" "$service"
grep -Fqx "Exec=\"$launcher\"" "$desktop"
grep -Fqx "TryExec=$launcher" "$desktop"
grep -Fqx 'Icon=io.github.ntrpydev.openfrag' "$desktop"
grep -Fqx 'Slice=app.slice' "$service"
if command -v desktop-file-validate >/dev/null 2>&1; then
    desktop-file-validate "$desktop"
fi
test ! -e "$data_home/applications/openfrag.desktop"
test ! -e "$config_home/systemd/user/openfragd.service"
test ! -e "$home/.local/bin/openfrag-open-dashboard"
test ! -e "$systemctl_log"

run_installer "$upgrade"
cmp -s "$upgrade" "$binary"
test ! -e "$systemctl_log"

mkdir -p "$data_home/openfrag" "$config_home/openfrag"
touch "$data_home/openfrag/user-data" "$config_home/openfrag/user-config"
run_installer --uninstall
for path in "$binary" "$launcher" "$desktop" "$service"; do
    test ! -e "$path"
done
test -e "$data_home/openfrag/user-data"
test -e "$config_home/openfrag/user-config"
test ! -e "$systemctl_log"

printf '%s\n' '[Desktop Entry]' 'Name=User modified legacy entry' \
    >"$data_home/applications/openfrag.desktop"
if run_installer "$fixture" >"$stage/modified-legacy.stdout" 2>"$stage/modified-legacy.stderr"; then
    printf '%s\n' 'installer accepted a modified legacy identity file' >&2
    exit 1
fi
grep -Fq 'Refusing to replace modified legacy package file' "$stage/modified-legacy.stderr"
test -e "$data_home/applications/openfrag.desktop"
for path in "$binary" "$launcher" "$desktop" "$service"; do
    test ! -e "$path"
done

printf '%s\n' 'Staged per-user install, upgrade, and uninstall passed headless verification.'
