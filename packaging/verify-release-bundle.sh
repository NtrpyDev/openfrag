#!/usr/bin/env bash
# Verifies deterministic release bytes and bundle installation without host activation.
set -euo pipefail

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
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

archive_name=openfrag-v1.0.0-linux-x86_64.tar.gz
checksum_name="$archive_name.sha256"
cmp -s "$first/$archive_name" "$second/$archive_name"
cmp -s "$first/$checksum_name" "$second/$checksum_name"
(
    cd "$first"
    sha256sum -c "$checksum_name"
)

expected="$stage/expected-files"
actual="$stage/actual-files"
cat >"$expected" <<'EOF'
openfrag-v1.0.0-linux-x86_64/packaging/install-user.sh
openfrag-v1.0.0-linux-x86_64/packaging/linux/openfrag-open-dashboard
openfrag-v1.0.0-linux-x86_64/packaging/linux/openfrag.desktop
openfrag-v1.0.0-linux-x86_64/packaging/linux/openfragd.service
openfrag-v1.0.0-linux-x86_64/target/release/openfragd
EOF
tar -tzf "$first/$archive_name" | grep -v '/$' >"$actual"
cmp -s "$expected" "$actual"

tar -xzf "$first/$archive_name" -C "$extract"
bundle="$extract/openfrag-v1.0.0-linux-x86_64"
cmp -s "$fixture" "$bundle/target/release/openfragd"
cmp -s "$root/packaging/install-user.sh" "$bundle/packaging/install-user.sh"
cmp -s "$root/packaging/linux/openfragd.service" "$bundle/packaging/linux/openfragd.service"
cmp -s "$root/packaging/linux/openfrag.desktop" "$bundle/packaging/linux/openfrag.desktop"
cmp -s "$root/packaging/linux/openfrag-open-dashboard" "$bundle/packaging/linux/openfrag-open-dashboard"
test "$(stat -c '%a' "$bundle/target/release/openfragd")" = 755
test "$(stat -c '%a' "$bundle/packaging/install-user.sh")" = 755
test "$(stat -c '%a' "$bundle/packaging/linux/openfrag-open-dashboard")" = 755
test "$(stat -c '%a' "$bundle/packaging/linux/openfrag.desktop")" = 644
test "$(stat -c '%a' "$bundle/packaging/linux/openfragd.service")" = 644

cat >"$guard_bin/systemctl" <<EOF
#!/usr/bin/env bash
touch '$systemctl_log'
exit 96
EOF
chmod 0755 "$guard_bin/systemctl"
(
    cd "$bundle"
    HOME="$home" PATH="$guard_bin:$PATH" packaging/install-user.sh --staged
)
cmp -s "$fixture" "$home/.local/bin/openfragd"
test ! -e "$systemctl_log"

printf '%s\n' 'Deterministic Linux release bundle passed headless verification.'
