#!/usr/bin/env bash
# Exercises NVIDIA host verification entirely through isolated command fixtures.
set -euo pipefail

root=$(CDPATH= cd -- "$(dirname -- "$0")/../../.." && pwd)
verifier="$root/scripts/hardware/verify-nvidia-host.sh"
stage=$(mktemp -d "${TMPDIR:-/tmp}/openfrag-nvidia-self-test.XXXXXXXX")
trap 'rm -rf -- "$stage"' EXIT HUP INT TERM

bin="$stage/bin"
runtime="$stage/runtime"
output_directory="$stage/clips"
launch_log="$stage/recorder-launched"
mkdir -p "$bin" "$runtime" "$output_directory"

cat >"$bin/nvidia-smi" <<'EOF'
#!/usr/bin/env bash
if [[ ${MOCK_NVIDIA_FAIL:-0} == 1 ]]; then
    exit 1
fi
printf '%s\n' 'NVIDIA GeForce RTX 4070, 555.58'
EOF

cat >"$bin/ffmpeg" <<'EOF'
#!/usr/bin/env bash
if [[ " $* " == *' -encoders '* ]]; then
    printf '%s\n' ' V....D h264_nvenc NVIDIA NVENC H.264 encoder'
else
    printf '%s\n' 'ffmpeg fixture version 1'
fi
EOF

cat >"$bin/ffprobe" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' 'ffprobe fixture version 1'
EOF

cat >"$bin/gpu-screen-recorder" <<EOF
#!/usr/bin/env bash
touch '$launch_log'
exit 95
EOF

cat >"$bin/flatpak" <<EOF
#!/usr/bin/env bash
if [[ \${1:-} == info && \${2:-} == com.dec05eba.gpu_screen_recorder ]]; then
    printf '%s\n' 'gpu-screen-recorder fixture'
    exit 0
fi
touch '$launch_log'
exit 94
EOF
chmod 0755 "$bin"/*

python3 - "$runtime/pipewire-0" <<'PY'
import socket
import sys

pipewire = socket.socket(socket.AF_UNIX)
pipewire.bind(sys.argv[1])
pipewire.close()
PY

run_verifier() {
    PATH="$bin:/usr/bin:/bin" \
        XDG_RUNTIME_DIR="$runtime" \
        PROBE_LAUNCH_LOG="$launch_log" \
        "$verifier" --output-dir "$output_directory"
}

native_json=$(run_verifier)
python3 -c 'import json,sys; value=json.load(sys.stdin); assert value["ready"] is True; assert value["checks"]["recorder"]["detail"] == "native"' <<<"$native_json"
test ! -e "$launch_log"

rm "$bin/gpu-screen-recorder"
flatpak_json=$(run_verifier)
python3 -c 'import json,sys; value=json.load(sys.stdin); assert value["ready"] is True; assert value["checks"]["recorder"]["detail"] == "flatpak"' <<<"$flatpak_json"
test ! -e "$launch_log"

set +e
blocked_json=$(MOCK_NVIDIA_FAIL=1 run_verifier)
blocked_status=$?
set -e
test "$blocked_status" -eq 1
python3 -c 'import json,sys; value=json.load(sys.stdin); assert value["ready"] is False; assert value["checks"]["nvidia_driver"]["status"] == "blocked"' <<<"$blocked_json"
test ! -e "$launch_log"

printf '%s\n' 'NVIDIA host verifier mocked self-test passed.'
