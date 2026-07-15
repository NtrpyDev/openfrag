#!/usr/bin/env bash
# Installs, upgrades, or uninstalls the canonical OpenFrag application for one Linux user.
set -euo pipefail

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
source "$root/packaging/application-identity.sh"
staged=false
uninstall=false
while (( $# > 0 )); do
    case $1 in
        --staged)
            staged=true
            shift
            ;;
        --uninstall)
            uninstall=true
            shift
            ;;
        --)
            shift
            break
            ;;
        -*)
            printf 'Unknown option: %s\n' "$1" >&2
            exit 2
            ;;
        *)
            break
            ;;
    esac
done
if (( $# > 1 )) || [[ $uninstall == true && $# != 0 ]]; then
    printf '%s\n' 'Usage: packaging/install-user.sh [--staged] [--uninstall] [openfragd-binary]' >&2
    exit 2
fi

data_home=${XDG_DATA_HOME:-${HOME}/.local/share}
config_home=${XDG_CONFIG_HOME:-${HOME}/.config}
prefix=${HOME}/.local
binary_path=${prefix}/bin/${OPENFRAG_DAEMON_NAME}
launcher_path=${prefix}/bin/${OPENFRAG_LAUNCHER_NAME}
desktop_path=${data_home}/applications/${OPENFRAG_DESKTOP_NAME}
service_path=${config_home}/systemd/user/${OPENFRAG_USER_SERVICE}

legacy_desktop_hash=6bf9cefbc1af2f1cf8f9f745bc6512fed93b7596167ba58cfd1f0dad70b0e277
legacy_service_hash=64b2db0b70d3d678bb7505a1337f8d9416a9183aa4a4c4bd88bddef124f35190
legacy_launcher_hash=97d2b6fc81275ec2656368ce76ea13d46b4407f77486e4dd6729cb308538e1c7

legacy_desktops=("${data_home}/applications/openfrag.desktop")
if [[ ${HOME}/.local/share/applications/openfrag.desktop != "${legacy_desktops[0]}" ]]; then
    legacy_desktops+=("${HOME}/.local/share/applications/openfrag.desktop")
fi
legacy_services=("${config_home}/systemd/user/openfragd.service")
if [[ ${HOME}/.config/systemd/user/openfragd.service != "${legacy_services[0]}" ]]; then
    legacy_services+=("${HOME}/.config/systemd/user/openfragd.service")
fi
legacy_launcher=${prefix}/bin/openfrag-open-dashboard

verify_owned_legacy_file() {
    local path=$1
    local expected=$2
    [[ ! -e $path && ! -L $path ]] && return 0
    if [[ ! -f $path || -L $path ]]; then
        printf 'Refusing to replace non-regular legacy package path: %s\n' "$path" >&2
        return 1
    fi
    local actual
    actual=$(sha256sum -- "$path")
    actual=${actual%% *}
    if [[ $actual != "$expected" ]]; then
        printf 'Refusing to replace modified legacy package file: %s\n' "$path" >&2
        return 1
    fi
}

remove_legacy_identity() {
    local path
    local service_present=false
    for path in "${legacy_desktops[@]}"; do
        verify_owned_legacy_file "$path" "$legacy_desktop_hash"
    done
    for path in "${legacy_services[@]}"; do
        verify_owned_legacy_file "$path" "$legacy_service_hash"
        [[ -e $path ]] && service_present=true
    done
    verify_owned_legacy_file "$legacy_launcher" "$legacy_launcher_hash"
    if [[ $staged == false && $service_present == true ]] && command -v systemctl >/dev/null 2>&1; then
        systemctl --user disable --now openfragd.service
    fi
    rm -f -- "${legacy_desktops[@]}" "${legacy_services[@]}" "$legacy_launcher"
}

reload_user_manager() {
    if [[ $staged == false ]] && command -v systemctl >/dev/null 2>&1; then
        systemctl --user daemon-reload
    fi
}

install_desktop_entry() {
    local template=$1
    local destination=$2
    local temporary
    local exec_path=$launcher_path
    exec_path=${exec_path//\\/\\\\}
    exec_path=${exec_path//\"/\\\"}
    exec_path=${exec_path//\`/\\\`}
    exec_path=${exec_path//\$/\\\$}
    temporary=$(mktemp "${destination}.XXXXXXXX")
    if ! while IFS= read -r line || [[ -n $line ]]; do
        case $line in
            Exec=*) printf 'Exec="%s"\n' "$exec_path" ;;
            TryExec=*) printf 'TryExec=%s\n' "$launcher_path" ;;
            *) printf '%s\n' "$line" ;;
        esac
    done <"$template" >"$temporary"; then
        rm -f -- "$temporary"
        return 1
    fi
    chmod 0644 "$temporary"
    mv -f -- "$temporary" "$destination"
}

if [[ $uninstall == true ]]; then
    if [[ $staged == false && -e $service_path ]] && command -v systemctl >/dev/null 2>&1; then
        systemctl --user disable --now "$OPENFRAG_USER_SERVICE"
    fi
    remove_legacy_identity
    rm -f -- "$binary_path" "$launcher_path" "$desktop_path" "$service_path"
    reload_user_manager
    printf '%s\n' 'OpenFrag application files were removed; local user data and configuration were preserved.'
    exit 0
fi

binary=${1:-$root/target/release/$OPENFRAG_DAEMON_NAME}
if [[ ! -f $binary || ! -x $binary || -L $binary ]]; then
    printf 'OpenFrag binary must be an executable regular file, not a symlink: %s\n' "$binary" >&2
    printf '%s\n' 'Run cargo build --release --package openfragd first.' >&2
    exit 1
fi

remove_legacy_identity
install -d -m 0755 \
    "${prefix}/bin" \
    "${data_home}/applications" \
    "${config_home}/systemd/user"
install -m 0755 "$binary" "$binary_path"
install -m 0755 "$root/packaging/linux/$OPENFRAG_LAUNCHER_NAME" "$launcher_path"
install_desktop_entry \
    "$root/packaging/linux/$OPENFRAG_DESKTOP_NAME" \
    "$desktop_path"
install -m 0644 "$root/packaging/linux/$OPENFRAG_USER_SERVICE" "$service_path"
reload_user_manager

if [[ $staged == true ]]; then
    printf '%s\n' 'OpenFrag was staged for the current user without activating systemd.'
else
    printf '%s\n' 'OpenFrag was installed for the current user without enabling or starting its service.'
    printf 'Start it with: systemctl --user start %s\n' "$OPENFRAG_USER_SERVICE"
fi
