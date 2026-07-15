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
nvenc_probe_log="$stage/nvenc-probed"
nvenc_probe_args_log="$stage/nvenc-probe-args"
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
    if [[ ${MOCK_NVENC_UNAVAILABLE:-0} != 1 ]]; then
        printf '%s\n' ' V....D h264_nvenc NVIDIA NVENC H.264 encoder'
    fi
elif [[ " $* " == *' lavfi '* ]]; then
    touch "$NVENC_PROBE_LOG"
    printf '%s\n' "$*" >"$NVENC_PROBE_ARGS_LOG"
    if [[ " $* " != *' color=c=black:s=256x256:r=1:d=0.1 '* ]]; then
        exit 96
    fi
    exit "${MOCK_NVENC_FAIL:-0}"
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
        NVENC_PROBE_LOG="$nvenc_probe_log" \
        NVENC_PROBE_ARGS_LOG="$nvenc_probe_args_log" \
        "$verifier" --output-dir "$output_directory" "$@"
}

native_json=$(run_verifier)
python3 -c 'import json,sys; value=json.load(sys.stdin); assert value["ready"] is True; assert value["checks"]["recorder"]["detail"] == "native"' <<<"$native_json"
test ! -e "$launch_log"

probe_json=$(run_verifier --probe-nvenc)
python3 -c 'import json,sys; value=json.load(sys.stdin); assert value["ready"] is True; assert value["checks"]["nvenc"]["detail"] == "runtime_verified"' <<<"$probe_json"
test -e "$nvenc_probe_log"
grep -Fq -- '-f lavfi -i color=c=black:s=256x256:r=1:d=0.1 -frames:v 1 -an -c:v h264_nvenc -f null -' "$nvenc_probe_args_log"
test ! -e "$launch_log"

rm "$nvenc_probe_log"
set +e
failed_probe_json=$(MOCK_NVENC_FAIL=1 run_verifier --probe-nvenc)
failed_probe_status=$?
set -e
test "$failed_probe_status" -eq 1
python3 -c 'import json,sys; value=json.load(sys.stdin); assert value["ready"] is False; assert value["checks"]["nvenc"]["detail"] == "runtime_probe_failed"' <<<"$failed_probe_json"
test -e "$nvenc_probe_log"
test ! -e "$launch_log"

rm "$nvenc_probe_log" "$nvenc_probe_args_log"
set +e
unavailable_json=$(MOCK_NVENC_UNAVAILABLE=1 run_verifier --probe-nvenc)
unavailable_status=$?
set -e
test "$unavailable_status" -eq 1
python3 -c 'import json,sys; value=json.load(sys.stdin); assert value["ready"] is False; assert value["checks"]["nvenc"]["detail"] == "unavailable"' <<<"$unavailable_json"
test ! -e "$nvenc_probe_log"
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
