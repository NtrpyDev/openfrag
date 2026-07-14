#!/usr/bin/env bash
# Throwaway issue #18 host probe. It never opens a UI or selects a real microphone.
set -euo pipefail

for command in pactl pw-dump pw-record pw-play ffmpeg ffprobe jq; do
  command -v "$command" >/dev/null || {
    echo "missing required command: $command" >&2
    exit 1
  }
done

out_dir="${1:-/tmp/openfrag-audio-routing-$(date +%Y%m%dT%H%M%S)}"
mkdir -p "$out_dir"
sink_name="openfrag_issue18_synthetic_${$}"
module_id=""

cleanup() {
  if [[ -n "$module_id" ]]; then
    pactl unload-module "$module_id" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

capture_state() {
  local prefix="$1"
  pactl info >"$out_dir/${prefix}-pactl-info.txt"
  wpctl status >"$out_dir/${prefix}-wpctl-status.txt" 2>&1 || true
  pactl -f json list sinks >"$out_dir/${prefix}-pulse-sinks.json"
  pactl -f json list sources >"$out_dir/${prefix}-pulse-sources.json"
  pactl -f json list sink-inputs >"$out_dir/${prefix}-pulse-sink-inputs.json"
  pactl -f json list source-outputs >"$out_dir/${prefix}-pulse-source-outputs.json"
  pw-dump >"$out_dir/${prefix}-pipewire-dump.json"
}

capture_state initial

if command -v gpu-screen-recorder >/dev/null; then
  gpu-screen-recorder --help >"$out_dir/gpu-screen-recorder-help.txt" 2>&1 || true
  gpu_screen_recorder="present"
  if grep -Eqi '(^|[[:space:]])(-a|--audio)([[:space:],]|$)|audio.*source|source.*audio' "$out_dir/gpu-screen-recorder-help.txt"; then
    gpu_audio_selection="advertised-in-help"
  else
    gpu_audio_selection="not-advertised-in-help"
  fi
else
  printf '%s\n' 'gpu-screen-recorder is not installed on this host.' >"$out_dir/gpu-screen-recorder-help.txt"
  gpu_screen_recorder="absent"
  gpu_audio_selection="unavailable"
fi

# This temporary null sink is the only capture target. No physical source is read.
module_id="$(pactl load-module module-null-sink "sink_name=$sink_name" "sink_properties=device.description=Openfrag_Issue18_Synthetic")"
source_index="$(pactl -f json list sources | jq -r --arg name "$sink_name.monitor" '.[] | select(.name == $name) | .index')"
if [[ -z "$source_index" ]]; then
  echo "temporary null-sink monitor was not exposed by Pulse compatibility" >&2
  exit 1
fi
tone="$out_dir/synthetic-997hz.wav"
capture="$out_dir/synthetic-monitor.wav"
ffmpeg -nostdin -v error -f lavfi -i 'sine=frequency=997:sample_rate=48000:duration=1' -ac 2 -c:a pcm_s16le "$tone"

set +e
pw-record --target "$source_index" --rate 48000 --channels 2 --format s16 --sample-count 57600 "$capture" >"$out_dir/pw-record.log" 2>&1 &
record_pid=$!
sleep 0.15
pw-play --target "$sink_name" "$tone" >"$out_dir/pw-play.log" 2>&1
play_status=$?
wait "$record_pid"
record_status=$?
set -e

if [[ "$play_status" -eq 0 && -s "$capture" ]]; then
  ffprobe -v error -show_entries format=duration,size -of json "$capture" >"$out_dir/synthetic-ffprobe.json"
  ffmpeg -nostdin -v info -i "$capture" -af astats=metadata=1:reset=0 -f null - >"$out_dir/synthetic-astats.txt" 2>&1
  duration="$(jq -r '.format.duration // 0' "$out_dir/synthetic-ffprobe.json")"
  if awk -v duration="$duration" 'BEGIN { exit !(duration >= 0.5) }'; then
    synthetic_status="passed"
  else
    synthetic_status="failed"
  fi
else
  synthetic_status="failed"
  printf '{"error":"synthetic null-sink capture failed","pw_play_status":%s,"pw_record_status":%s}\n' "$play_status" "$record_status" >"$out_dir/synthetic-ffprobe.json"
fi

capture_state final

jq -n \
  --slurpfile sinks "$out_dir/initial-pulse-sinks.json" \
  --slurpfile sources "$out_dir/initial-pulse-sources.json" \
  --slurpfile inputs "$out_dir/initial-pulse-sink-inputs.json" \
  --slurpfile outputs "$out_dir/initial-pulse-source-outputs.json" \
  --arg generated_at "$(date --iso-8601=seconds)" \
  --arg synthetic_status "$synthetic_status" \
  --arg gpu_screen_recorder "$gpu_screen_recorder" \
  --arg gpu_audio_selection "$gpu_audio_selection" \
  --argjson pw_play_status "$play_status" \
  --argjson pw_record_status "$record_status" \
  --arg sink_name "$sink_name" \
  --arg out_dir "$out_dir" '
  def properties: .properties // {};
  def persistence:
    {pulse_name: .name,
     node_name: (properties["node.name"] // null),
     device_serial: (properties["device.serial"] // null),
     device_bus_path: (properties["device.bus_path"] // null),
     device_name: (properties["device.name"] // null),
     stream_restore_id: (properties["module-stream-restore.id"] // null),
     ephemeral_object_serial: (properties["object.serial"] // null)};
  def app_text:
    [properties["application.name"], properties["application.process.binary"], properties["media.name"], properties["node.name"]]
    | map(select(. != null)) | join(" ") | ascii_downcase;
  def classify:
    app_text as $t |
    if ($t | test("cs2|counter.strike|steam_app_730")) then "game-candidate"
    elif ($t | test("discord|mumble|teamspeak|steam voice|voice")) then "voice-candidate"
    else "unclassified" end;
  {
    generated_at: $generated_at,
    probe: "issue-18-audio-routing-throwaway",
    safety: "Synthetic null-sink monitor only; no microphone was selected or recorded.",
    gpu_screen_recorder: {availability: $gpu_screen_recorder, audio_source_selection: $gpu_audio_selection, help_file: "gpu-screen-recorder-help.txt"},
    synthetic_capture: {status: $synthetic_status, sink: $sink_name, source: ($sink_name + ".monitor"), file: "synthetic-monitor.wav", pw_play_status: $pw_play_status, pw_record_status: $pw_record_status, verification: "synthetic-ffprobe.json and synthetic-astats.txt"},
    persistence_rule: "Use node.name plus device.serial/device.bus_path/device.name when present; numeric object serial is diagnostic only.",
    fallback: "If no separate CS2 and voice candidates are routable, capture one mixed_game_voice sink monitor; capture microphone separately only from a non-monitor source. Otherwise three tracks are unavailable.",
    sinks: [$sinks[0][] | {description, persistence: persistence}],
    non_monitor_sources: [$sources[0][] | select((.name | endswith(".monitor")) | not) | {description, persistence: persistence}],
    output_streams: [$inputs[0][] | {index, sink, classification: classify, application: {name: properties["application.name"], binary: properties["application.process.binary"], media_name: properties["media.name"]}, persistence: persistence}],
    input_streams: [$outputs[0][] | {index, source, classification: classify, application: {name: properties["application.name"], binary: properties["application.process.binary"], media_name: properties["media.name"]}, persistence: persistence}],
    raw_state: ["initial-pactl-info.txt", "initial-wpctl-status.txt", "initial-pulse-sinks.json", "initial-pulse-sources.json", "initial-pulse-sink-inputs.json", "initial-pulse-source-outputs.json", "initial-pipewire-dump.json", "final-pactl-info.txt", "final-wpctl-status.txt", "final-pulse-sinks.json", "final-pulse-sources.json", "final-pulse-sink-inputs.json", "final-pulse-source-outputs.json", "final-pipewire-dump.json"]
  }' >"$out_dir/report.json"

sha256sum "$capture" >"$out_dir/synthetic-monitor.sha256" 2>/dev/null || true
printf 'Audio routing probe complete: %s\n' "$out_dir"
printf 'Inspect: %s/report.json\n' "$out_dir"
printf 'Synthetic capture: %s (%s)\n' "$capture" "$synthetic_status"
