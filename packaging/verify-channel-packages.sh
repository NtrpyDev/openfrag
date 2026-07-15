#!/usr/bin/env bash
# Verifies native channel recipes and optionally an extracted package filesystem.
set -euo pipefail

if (( $# != 0 && $# != 2 )) || (( $# == 2 )) && [[ $1 != --root ]]; then
    printf '%s\n' 'Usage: packaging/verify-channel-packages.sh [--root <package-root>]' >&2
    exit 2
fi

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
aur="$repo/packaging/aur"
spec="$repo/packaging/rpm/openfrag.spec"
version=$(sed -n '/^\[workspace\.package\]$/,/^\[/s/^version = "\([0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*\)"$/\1/p' "$repo/Cargo.toml")
source_name="openfrag-v${version}-source.tar.gz"
expected_sha=$(sed -n "s/^sha256sums=('\([0-9a-f]\{64\}\)')$/\1/p" "$aur/PKGBUILD")
spec_sha=$(sed -n 's/^%global source_sha256 \([0-9a-f]\{64\}\)$/\1/p' "$spec")

test "$version" = 1.0.0
test ${#expected_sha} -eq 64
test "$expected_sha" = "$spec_sha"
grep -Fqx "pkgver=$version" "$aur/PKGBUILD"
grep -Fqx "Version:        $version" "$spec"
grep -Fq "releases/download/v\${pkgver}/\${pkgname}-v\${pkgver}-source.tar.gz" "$aur/PKGBUILD"
grep -Fq 'releases/download/v%{version}/%{name}-v%{version}-source.tar.gz' "$spec"
grep -Fq 'cargo build --frozen --release -p openfragd' "$aur/PKGBUILD"
grep -Fq 'cargo build --frozen --release -p openfragd' "$spec"

if grep -E '^depends=.*(steam|gpu-screen-recorder)' "$aur/PKGBUILD"; then
    printf '%s\n' 'AUR package has an optional host capability as a hard dependency.' >&2
    exit 1
fi
if grep -Ei '^Requires:.*(steam|gpu-screen-recorder)' "$spec"; then
    printf '%s\n' 'RPM package has an optional host capability as a hard dependency.' >&2
    exit 1
fi
if grep -Ei '(systemctl|%systemd_user_(post|preun|postun))' "$aur/PKGBUILD" "$spec"; then
    printf '%s\n' 'Native package recipes must not enable, start, stop, or disable the user service.' >&2
    exit 1
fi

generated=$(mktemp)
stage=$(mktemp -d "${TMPDIR:-/tmp}/openfrag-channel-verify.XXXXXXXX")
cleanup() {
    rm -f -- "$generated"
    rm -rf -- "$stage"
}
trap cleanup EXIT HUP INT TERM
(
    cd "$aur"
    makepkg --printsrcinfo
) >"$generated"
cmp "$generated" "$aur/.SRCINFO"
if command -v namcap >/dev/null 2>&1; then
    namcap "$aur/PKGBUILD"
fi
if command -v rpmspec >/dev/null 2>&1; then
    rpmspec -P "$spec" >/dev/null
fi

if ! "$repo/packaging/build-source-bundle.sh" "$stage" >"$stage/builder.log" 2>&1; then
    cat "$stage/builder.log" >&2
    exit 1
fi
actual_sha=$(sha256sum "$stage/$source_name")
actual_sha=${actual_sha%% *}
test "$actual_sha" = "$expected_sha"
(
    cd "$stage"
    sha256sum -c "$source_name.sha256"
)

if (( $# == 0 )); then
    printf '%s\n' 'AUR and COPR recipes match the deterministic vendored source contract.'
    exit 0
fi

package_root=$(CDPATH= cd -- "$2" && pwd)
binary="$package_root/usr/bin/openfragd"
launcher="$package_root/usr/bin/openfrag-launch"
desktop="$package_root/usr/share/applications/io.github.ntrpydev.openfrag.desktop"
service="$package_root/usr/lib/systemd/user/app-io.github.ntrpydev.openfrag.service"
license="$package_root/usr/share/licenses/openfrag/LICENSE"
notices="$package_root/usr/share/licenses/openfrag/THIRD_PARTY_NOTICES.md"

test -f "$binary" && test -x "$binary" && test ! -L "$binary"
test -f "$launcher" && test -x "$launcher" && test ! -L "$launcher"
test -f "$desktop" && test ! -L "$desktop"
test -f "$service" && test ! -L "$service"
test -f "$license" && test ! -L "$license"
test -f "$notices" && test ! -L "$notices"
test "$(stat -c '%a' "$binary")" = 755
test "$(stat -c '%a' "$launcher")" = 755
test "$(stat -c '%a' "$desktop")" = 644
test "$(stat -c '%a' "$service")" = 644
grep -Fqx 'Exec=openfrag-launch' "$desktop"
grep -Fqx 'TryExec=openfrag-launch' "$desktop"
grep -Fqx 'Icon=io.github.ntrpydev.openfrag' "$desktop"
grep -Fqx 'ExecStart=/usr/bin/openfragd serve --bind 127.0.0.1:7130' "$service"
if grep -Fq '%h/.local/bin/openfragd' "$service"; then
    printf '%s\n' 'Native package retained the static installer service path.' >&2
    exit 1
fi
cmp "$repo/LICENSE" "$license"
cmp "$repo/packaging/linux/THIRD_PARTY_NOTICES.md" "$notices"
if grep -Ein 'telemetry|analytics|sentry' "$launcher" "$desktop" "$service"; then
    printf '%s\n' 'Native package contains forbidden remote-observability configuration.' >&2
    exit 1
fi
remote_endpoints=$(grep -Eho 'https?://[^[:space:]]+' "$launcher" "$desktop" "$service" | grep -Fv 'http://127.0.0.1:7130/' | grep -Fv 'http://${host}:${port}/' || true)
if [[ -n $remote_endpoints ]]; then
    printf '%s\n%s\n' 'Native package contains a non-loopback endpoint:' "$remote_endpoints" >&2
    exit 1
fi

HOME="$stage/home" XDG_STATE_HOME="$stage/state" XDG_CONFIG_HOME="$stage/config" \
    XDG_CACHE_HOME="$stage/cache" "$binary" doctor >/dev/null
printf '%s\n' 'Native package root passed identity, license, local-only, and daemon smoke verification.'
