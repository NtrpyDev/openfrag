#!/usr/bin/env bash
# PROTOTYPE ONLY. Runs a local CS2 bot match without player input.

set -Eeuo pipefail
umask 077

readonly ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
readonly PROTOTYPE_DIR="$ROOT_DIR/prototypes/gsi-listener"
readonly GAME_DIR="${CS2_GAME_DIR:-/mnt/shared/SteamLibrary/steamapps/common/Counter-Strike Global Offensive}"
readonly CFG_DIR="$GAME_DIR/game/csgo/cfg"
readonly GSI_CFG="$CFG_DIR/gamestate_integration_openfrag_harness.cfg"
readonly COMPETITIVE_CFG="$CFG_DIR/gamemode_competitive_server.cfg"
readonly CAPTURE_FILE="$PROTOTYPE_DIR/captures/gsi-listener-capture.jsonl"
readonly TERMINAL_LOG="$PROTOTYPE_DIR/captures/listener-terminal.log"
readonly SUMMARY_FILE="$PROTOTYPE_DIR/captures/harness-summary.json"
readonly PUBLIC_MARKER="OPENFRAG_HARNESS_PUBLIC_MARKER"
readonly WAIT_SECONDS="${GSI_HARNESS_WAIT_SECONDS:-180}"
readonly TARGET_DESKTOP="${GSI_HARNESS_DESKTOP:-2}"
readonly KWIN_PLUGIN="openfrag-gsi-harness"
readonly KWIN_SCRIPT="${XDG_RUNTIME_DIR:-/tmp}/openfrag-gsi-harness.js"

usage() {
    cat <<'EOF'
Usage:
  GSI_HARNESS_CONFIRM_RESTART=YES official_map_harness.sh MODE [MAP]

MODE is competitive or deathmatch. MAP defaults to de_dust2 and must be an
allowlisted official map. The harness restarts only the current user's CS2,
places its window on virtual desktop 2 by default, sends no keyboard or mouse
input, records redacted assertions, and cleans up its configs and processes.
EOF
}

mode="${1:-}"
map="${2:-de_dust2}"
case "$mode" in
    competitive|deathmatch) ;;
    -h|--help) usage; exit 0 ;;
    *) printf 'Mode must be competitive or deathmatch.\n' >&2; usage >&2; exit 2 ;;
esac
case "$map" in
    de_ancient|de_anubis|de_dust2|de_inferno|de_mirage|de_nuke|de_overpass|de_train|de_vertigo) ;;
    *) printf 'Refusing non-allowlisted map: %s\n' "$map" >&2; exit 2 ;;
esac
[[ "${GSI_HARNESS_CONFIRM_RESTART:-}" == "YES" ]] || {
    printf 'Set GSI_HARNESS_CONFIRM_RESTART=YES to permit a local CS2 restart.\n' >&2
    exit 2
}
[[ "$TARGET_DESKTOP" =~ ^[1-9][0-9]*$ ]] || {
    printf 'GSI_HARNESS_DESKTOP must be a positive integer.\n' >&2
    exit 2
}

for command in steam cargo jq qdbus6 ss pgrep; do
    command -v "$command" >/dev/null || {
        printf 'Missing required command: %s\n' "$command" >&2
        exit 1
    }
done
[[ -d "$CFG_DIR" ]] || { printf 'CS2 cfg directory not found: %s\n' "$CFG_DIR" >&2; exit 1; }
[[ -f "$PROTOTYPE_DIR/Cargo.toml" ]] || { printf 'Prototype manifest not found.\n' >&2; exit 1; }
[[ ! -e "$GSI_CFG" && ! -L "$GSI_CFG" ]] || {
    printf 'Refusing existing harness GSI config: %s\n' "$GSI_CFG" >&2
    exit 1
}
if compgen -G "$CFG_DIR/gamestate_integration_openfrag*.cfg" >/dev/null; then
    printf 'Remove or rename the existing openfrag GSI config before running the harness.\n' >&2
    exit 1
fi
if [[ "$mode" == competitive && ( -e "$COMPETITIVE_CFG" || -L "$COMPETITIVE_CFG" ) ]]; then
    printf 'Refusing existing competitive server config: %s\n' "$COMPETITIVE_CFG" >&2
    exit 1
fi
if ss -ltn | awk '$4 ~ /:27100$/ {found=1} END {exit !found}'; then
    printf 'Port 27100 is already in use. Stop the existing listener first.\n' >&2
    exit 1
fi
desktop_count="$(qdbus6 org.kde.KWin /VirtualDesktopManager org.freedesktop.DBus.Properties.GetAll org.kde.KWin.VirtualDesktopManager | awk '$1=="count:" {print $2}')"
if [[ ! "$desktop_count" =~ ^[0-9]+$ || "$desktop_count" -lt "$TARGET_DESKTOP" ]]; then
    printf 'Virtual desktop %s is not available.\n' "$TARGET_DESKTOP" >&2
    exit 1
fi

listener_pid=""
game_pid=""
kwin_loaded=false
cleanup() {
    if [[ -n "$game_pid" ]] && kill -0 "$game_pid" 2>/dev/null; then
        kill -TERM "$game_pid" 2>/dev/null || true
        for _ in {1..20}; do
            kill -0 "$game_pid" 2>/dev/null || break
            sleep 0.25
        done
    fi
    if [[ -n "$listener_pid" ]] && kill -0 "$listener_pid" 2>/dev/null; then
        kill -TERM "$listener_pid" 2>/dev/null || true
        wait "$listener_pid" 2>/dev/null || true
    fi
    if [[ "$kwin_loaded" == true ]]; then
        qdbus6 org.kde.KWin /Scripting org.kde.kwin.Scripting.unloadScript "$KWIN_PLUGIN" >/dev/null 2>&1 || true
    fi
    rm -f -- "$GSI_CFG" "$KWIN_SCRIPT"
    if [[ "$mode" == competitive ]]; then
        rm -f -- "$COMPETITIVE_CFG"
    fi
}
trap cleanup EXIT INT TERM

stop_running_cs2() {
    local -a pids=()
    mapfile -t pids < <(pgrep -u "$(id -u)" -f '^.*/game/bin/linuxsteamrt64/cs2 ' || true)
    ((${#pids[@]} == 0)) && return
    kill -TERM "${pids[@]}"
    for _ in {1..20}; do
        mapfile -t pids < <(pgrep -u "$(id -u)" -f '^.*/game/bin/linuxsteamrt64/cs2 ' || true)
        ((${#pids[@]} == 0)) && return
        sleep 0.25
    done
    printf 'CS2 did not exit after SIGTERM. Refusing to use SIGKILL.\n' >&2
    exit 1
}

cat >"$KWIN_SCRIPT" <<EOF
function placeOpenfragCs2(window) {
    const caption = String(window.caption || "").toLowerCase();
    const resourceClass = String(window.resourceClass || "").toLowerCase();
    const resourceName = String(window.resourceName || "").toLowerCase();
    if (!caption.includes("counter-strike 2") && !resourceClass.includes("cs2") && !resourceName.includes("cs2")) return;
    const index = $((TARGET_DESKTOP - 1));
    if (workspace.desktops.length > index) window.desktops = [workspace.desktops[index]];
}
const windows = workspace.stackingOrder;
for (let i = 0; i < windows.length; i++) placeOpenfragCs2(windows[i]);
workspace.windowAdded.connect(placeOpenfragCs2);
EOF
qdbus6 org.kde.KWin /Scripting org.kde.kwin.Scripting.unloadScript "$KWIN_PLUGIN" >/dev/null 2>&1 || true
qdbus6 org.kde.KWin /Scripting org.kde.kwin.Scripting.loadScript "$KWIN_SCRIPT" "$KWIN_PLUGIN" >/dev/null
qdbus6 org.kde.KWin /Scripting org.kde.kwin.Scripting.start
kwin_loaded=true

cat >"$GSI_CFG" <<EOF
"openfrag harness"
{
    "uri" "http://127.0.0.1:27100/gsi"
    "timeout" "5.0"
    "buffer" "0.1"
    "throttle" "0.1"
    "heartbeat" "30.0"
    "auth" { "token" "$PUBLIC_MARKER" }
    "data"
    {
        "provider" "1"
        "map" "1"
        "player_id" "1"
        "player_state" "1"
        "player_match_stats" "1"
        "player_weapons" "1"
        "allplayers_id" "1"
        "allplayers_state" "1"
        "allplayers_match_stats" "1"
        "allplayers_weapons" "1"
        "allplayers_position" "1"
        "round" "1"
        "bomb" "1"
        "allgrenades" "1"
        "phase_countdowns" "1"
    }
}
EOF

if [[ "$mode" == competitive ]]; then
    cat >"$COMPETITIVE_CFG" <<'EOF'
mp_freezetime 0
mp_roundtime_defuse 2
mp_round_restart_delay 2
mp_autoteambalance 0
mp_limitteams 0
bot_kick
bot_quota_mode normal
bot_quota 1
bot_difficulty 3
jointeam 2
bot_add_ct
mp_restartgame 3
EOF
fi

mkdir -p "$PROTOTYPE_DIR/captures"
chmod 700 "$PROTOTYPE_DIR/captures"
touch "$CAPTURE_FILE"
chmod 600 "$CAPTURE_FILE"
: >"$TERMINAL_LOG"
chmod 600 "$TERMINAL_LOG"
baseline_lines="$(wc -l <"$CAPTURE_FILE" 2>/dev/null || printf '0')"

GSI_TOKEN="$PUBLIC_MARKER" cargo run --release --manifest-path "$PROTOTYPE_DIR/Cargo.toml" >"$TERMINAL_LOG" 2>&1 &
listener_pid="$!"
for _ in {1..50}; do
    kill -0 "$listener_pid" 2>/dev/null || {
        printf 'Listener exited before CS2 launch. See %s.\n' "$TERMINAL_LOG" >&2
        exit 1
    }
    ss -ltn | awk '$4=="127.0.0.1:27100" {found=1} END {exit !found}' && break
    sleep 0.1
done
ss -ltn | awk '$4=="127.0.0.1:27100" {found=1} END {exit !found}' || {
    printf 'Listener did not bind exact IPv4 loopback.\n' >&2
    exit 1
}

stop_running_cs2
if [[ "$mode" == competitive ]]; then
    steam -applaunch 730 -condebug +game_type 0 +game_mode 1 +map "$map" >/dev/null 2>&1 &
else
    steam -applaunch 730 -condebug +game_type 1 +game_mode 2 +bot_quota 10 +map "$map" >/dev/null 2>&1 &
fi
for _ in {1..80}; do
    game_pid="$(pgrep -u "$(id -u)" -f '^.*/game/bin/linuxsteamrt64/cs2 ' | head -1 || true)"
    [[ -n "$game_pid" ]] && break
    sleep 0.25
done
[[ -n "$game_pid" ]] || { printf 'CS2 did not launch.\n' >&2; exit 1; }

deadline=$((SECONDS + WAIT_SECONDS))
while (( SECONDS < deadline )); do
    if tail -n "+$((baseline_lines + 1))" "$CAPTURE_FILE" 2>/dev/null |
        jq -se --arg mode "$mode" --arg map "$map" \
            'map(select(.payload.map.mode==$mode and .payload.map.name==$map and .payload.player.activity=="playing" and (.payload.player.state.health|type)=="number")) | length>0' >/dev/null; then
        break
    fi
    sleep 1
done

tail -n "+$((baseline_lines + 1))" "$CAPTURE_FILE" 2>/dev/null |
    jq -s --arg mode "$mode" --arg map "$map" '
        map(select(.payload.map.mode==$mode and .payload.map.name==$map)) as $posts |
        {
            mode:$mode,
            map:$map,
            posts:($posts|length),
            playing_seen:any($posts[]; .payload.player.activity=="playing"),
            health_values:([$posts[].payload.player.state.health?|select(.!=null)]|unique),
            round_values:([$posts[].payload.map.round?|select(.!=null)]|unique),
            round_phases:([$posts[].payload.round.phase?|select(.!=null)]|unique),
            top_level_fields:([$posts[].payload|keys[]]|unique),
            identity_redacted:true
        }
    ' >"$SUMMARY_FILE"
chmod 600 "$SUMMARY_FILE"

jq -e '.posts>0 and .playing_seen==true and (.health_values|length)>0' "$SUMMARY_FILE" >/dev/null || {
    printf 'No meaningful %s evidence after %ss. See %s.\n' "$mode" "$WAIT_SECONDS" "$SUMMARY_FILE" >&2
    exit 1
}
printf 'Automated %s evidence passed. Redacted summary: %s\n' "$mode" "$SUMMARY_FILE"
