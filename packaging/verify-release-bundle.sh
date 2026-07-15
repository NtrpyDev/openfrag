#!/usr/bin/env bash
# Verifies deterministic release bytes and bundle installation without host activation.
set -euo pipefail

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
source "$root/packaging/application-identity.sh"
version=$(sed -n '/^\[workspace\.package\]$/,/^\[/s/^version = "\([0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*\)"$/\1/p' "$root/Cargo.toml")
if [[ ! $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    printf '%s\n' 'Cargo.toml must contain exactly one numeric workspace package version.' >&2
    exit 1
fi
bundle_name="openfrag-v${version}-linux-x86_64"
stage=$(mktemp -d "${TMPDIR:-/tmp}/openfrag-release-bundle.XXXXXXXX")
trap 'rm -rf -- "$stage"' EXIT HUP INT TERM

fixture="$stage/openfragd-fixture"
first="$stage/first"
second="$stage/second"
extract="$stage/extract"
home="$stage/home"
guard_bin="$stage/guard-bin"
systemctl_log="$stage/systemctl-called"
mkdir -p "$first" "$second" "$extract" "$home" "$guard_bin"
printf '#!/usr/bin/env bash\nprintf "fixture-openfragd-v1\\n"\n' >"$fixture"
chmod 0755 "$fixture"

"$root/packaging/build-release-bundle.sh" "$fixture" "$first"
"$root/packaging/build-release-bundle.sh" "$fixture" "$second"

archive_name="$bundle_name.tar.gz"
checksum_name="$archive_name.sha256"
cmp -s "$first/$archive_name" "$second/$archive_name"
cmp -s "$first/$checksum_name" "$second/$checksum_name"
(
    cd "$first"
    sha256sum -c "$checksum_name"
)

expected="$stage/expected-files"
actual="$stage/actual-files"
printf '%s\n' \
    "$bundle_name/LICENSE" \
    "$bundle_name/THIRD_PARTY_NOTICES.md" \
    "$bundle_name/packaging/application-identity.sh" \
    "$bundle_name/packaging/install-user.sh" \
    "$bundle_name/packaging/linux/app-io.github.ntrpydev.openfrag.service" \
    "$bundle_name/packaging/linux/io.github.ntrpydev.openfrag.desktop" \
    "$bundle_name/packaging/linux/openfrag-launch" \
    "$bundle_name/target/release/openfragd" >"$expected"
tar -tzf "$first/$archive_name" | grep -v '/$' >"$actual"
cmp -s "$expected" "$actual"

tar -xzf "$first/$archive_name" -C "$extract"
bundle="$extract/$bundle_name"
cmp -s "$fixture" "$bundle/target/release/openfragd"
cmp -s "$root/LICENSE" "$bundle/LICENSE"
cmp -s "$root/packaging/linux/THIRD_PARTY_NOTICES.md" "$bundle/THIRD_PARTY_NOTICES.md"
cmp -s "$root/packaging/application-identity.sh" "$bundle/packaging/application-identity.sh"
cmp -s "$root/packaging/install-user.sh" "$bundle/packaging/install-user.sh"
cmp -s "$root/packaging/linux/$OPENFRAG_USER_SERVICE" \
    "$bundle/packaging/linux/$OPENFRAG_USER_SERVICE"
cmp -s "$root/packaging/linux/$OPENFRAG_DESKTOP_NAME" \
    "$bundle/packaging/linux/$OPENFRAG_DESKTOP_NAME"
cmp -s "$root/packaging/linux/$OPENFRAG_LAUNCHER_NAME" \
    "$bundle/packaging/linux/$OPENFRAG_LAUNCHER_NAME"
test "$(stat -c '%a' "$bundle/target/release/openfragd")" = 755
test "$(stat -c '%a' "$bundle/LICENSE")" = 644
test "$(stat -c '%a' "$bundle/THIRD_PARTY_NOTICES.md")" = 644
test "$(stat -c '%a' "$bundle/packaging/application-identity.sh")" = 644
test "$(stat -c '%a' "$bundle/packaging/install-user.sh")" = 755
test "$(stat -c '%a' "$bundle/packaging/linux/$OPENFRAG_LAUNCHER_NAME")" = 755
test "$(stat -c '%a' "$bundle/packaging/linux/$OPENFRAG_DESKTOP_NAME")" = 644
test "$(stat -c '%a' "$bundle/packaging/linux/$OPENFRAG_USER_SERVICE")" = 644

cat >"$guard_bin/systemctl" <<EOF
#!/usr/bin/env bash
touch '$systemctl_log'
exit 96
EOF
chmod 0755 "$guard_bin/systemctl"
(
    cd "$bundle"
    HOME="$home" XDG_DATA_HOME="$home/.local/share" XDG_CONFIG_HOME="$home/.config" \
        PATH="$guard_bin:$PATH" packaging/install-user.sh --staged
)
cmp -s "$fixture" "$home/.local/bin/openfragd"
test -f "$home/.local/share/applications/io.github.ntrpydev.openfrag.desktop"
test -f "$home/.config/systemd/user/app-io.github.ntrpydev.openfrag.service"
grep -Fqx "Exec=\"$home/.local/bin/openfrag-launch\"" \
    "$home/.local/share/applications/io.github.ntrpydev.openfrag.desktop"
grep -Fqx "TryExec=$home/.local/bin/openfrag-launch" \
    "$home/.local/share/applications/io.github.ntrpydev.openfrag.desktop"
test ! -e "$home/.local/share/applications/openfrag.desktop"
test ! -e "$home/.config/systemd/user/openfragd.service"
test ! -e "$systemctl_log"

printf '#!/usr/bin/env bash\nexit 99\n' >"$home/.local/bin/openfragd"
(
    cd "$bundle"
    HOME="$home" XDG_DATA_HOME="$home/.local/share" XDG_CONFIG_HOME="$home/.config" \
        PATH="$guard_bin:$PATH" packaging/install-user.sh --staged
)
cmp -s "$fixture" "$home/.local/bin/openfragd"
(
    cd "$bundle"
    HOME="$home" XDG_DATA_HOME="$home/.local/share" XDG_CONFIG_HOME="$home/.config" \
        PATH="$guard_bin:$PATH" packaging/install-user.sh --staged --uninstall
)
test ! -e "$home/.local/bin/openfragd"
test ! -e "$home/.local/bin/openfrag-launch"
test ! -e "$home/.local/share/applications/io.github.ntrpydev.openfrag.desktop"
test ! -e "$home/.config/systemd/user/app-io.github.ntrpydev.openfrag.service"
test ! -e "$systemctl_log"

printf '%s\n' 'Deterministic Linux release bundle and staged lifecycle passed headless verification.'
