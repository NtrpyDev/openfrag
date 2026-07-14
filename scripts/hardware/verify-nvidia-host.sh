#!/usr/bin/env bash
# Reports read-only NVIDIA capture prerequisites as machine-readable JSON.
set -euo pipefail

usage() {
    printf '%s\n' 'Usage: scripts/hardware/verify-nvidia-host.sh --output-dir <existing-directory>'
}

output_directory=
while (( $# > 0 )); do
    case $1 in
        --output-dir)
            if (( $# < 2 )); then
                usage >&2
                exit 2
            fi
            output_directory=$2
            shift 2
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            usage >&2
            exit 2
            ;;
    esac
done
if [[ -z $output_directory ]]; then
    usage >&2
    exit 2
fi

declare -A statuses details
blocked=0
set_check() {
    local name=$1 status=$2 detail=$3
    statuses[$name]=$status
    details[$name]=$detail
    if [[ $status != ready ]]; then
        blocked=1
    fi
}

sanitize_detail() {
    local sanitized
    sanitized=$(printf '%s' "$1" | LC_ALL=C tr -cd 'A-Za-z0-9 .,_:+/@()-')
    printf '%.240s' "$sanitized"
}

if command -v nvidia-smi >/dev/null 2>&1 \
    && driver_output=$(nvidia-smi --query-gpu=name,driver_version --format=csv,noheader 2>/dev/null) \
    && [[ -n $driver_output ]]; then
    driver_line=${driver_output%%$'\n'*}
    set_check nvidia_driver ready "$(sanitize_detail "$driver_line")"
else
    set_check nvidia_driver blocked unavailable
fi

ffmpeg_ready=false
if command -v ffmpeg >/dev/null 2>&1 && ffmpeg -version >/dev/null 2>&1; then
    ffmpeg_ready=true
    set_check ffmpeg ready available
else
    set_check ffmpeg blocked unavailable
fi

if [[ $ffmpeg_ready == true ]] \
    && encoder_output=$(ffmpeg -hide_banner -encoders 2>/dev/null) \
    && nvenc_encoders=$(printf '%s\n' "$encoder_output" | grep -Eo '(h264|hevc|av1)_nvenc' | sort -u | paste -sd, -) \
    && [[ -n $nvenc_encoders ]]; then
    set_check nvenc ready "$nvenc_encoders"
else
    set_check nvenc blocked unavailable
fi

if command -v ffprobe >/dev/null 2>&1 && ffprobe -version >/dev/null 2>&1; then
    set_check ffprobe ready available
else
    set_check ffprobe blocked unavailable
fi

if command -v gpu-screen-recorder >/dev/null 2>&1; then
    set_check recorder ready native
elif command -v flatpak >/dev/null 2>&1 \
    && flatpak info com.dec05eba.gpu_screen_recorder >/dev/null 2>&1; then
    set_check recorder ready flatpak
else
    set_check recorder blocked unavailable
fi

pipewire_socket=${XDG_RUNTIME_DIR:-}/pipewire-0
if [[ -n ${XDG_RUNTIME_DIR:-} && -S $pipewire_socket && ! -L $pipewire_socket ]]; then
    set_check pipewire ready socket_available
else
    set_check pipewire blocked socket_unavailable
fi

if [[ -d $output_directory && -w $output_directory && ! -L $output_directory ]]; then
    set_check output_directory ready writable
else
    set_check output_directory blocked not_writable
fi

if (( blocked == 0 )); then
    ready=true
else
    ready=false
fi

keys=(nvidia_driver nvenc recorder ffmpeg ffprobe pipewire output_directory)
printf '{"schema_version":1,"ready":%s,"checks":{' "$ready"
separator=
for key in "${keys[@]}"; do
    printf '%s"%s":{"status":"%s","detail":"%s"}' \
        "$separator" "$key" "${statuses[$key]}" "${details[$key]}"
    separator=,
done
printf '}}\n'

if (( blocked != 0 )); then
    exit 1
fi
