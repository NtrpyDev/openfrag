# GSI listener spike (throwaway)

Question: does a real CS2 session POST useful live state to a localhost listener, with the expected transitions for alive, dead, spectating, round end, and reconnect?

This is a throwaway spike, not production ingestion. It is for one local session, stores captures only under this prototype, and must be removed after the decision.

## Run and install

First, generate one ignored local token file and install a local config copy. Never commit a real token. This setup refuses an existing or dangling-symlink destination, writes a temporary file with mode 0600, then atomically installs it:

```sh
GAME='/mnt/shared/SteamLibrary/steamapps/common/Counter-Strike Global Offensive'
umask 077
TOKEN_FILE=prototypes/gsi-listener/.local-token
test ! -e "$TOKEN_FILE" || { echo "refusing existing token file: $TOKEN_FILE" >&2; exit 1; }
openssl rand -hex 24 > "$TOKEN_FILE"
chmod 600 "$TOKEN_FILE"
DEST="$GAME/game/csgo/cfg/gamestate_integration_openfrag.cfg"
test ! -e "$DEST" && test ! -L "$DEST" || { echo "refusing existing or symlink destination: $DEST" >&2; exit 1; }
TMP="$(mktemp "${DEST}.tmp.XXXXXX")"
trap 'rm -f "$TMP"' EXIT
sed "s/REPLACE_WITH_RANDOM_LOCAL_TOKEN/$(cat "$TOKEN_FILE")/" \
  prototypes/gsi-listener/gamestate_integration_openfrag_prototype.cfg > "$TMP"
chmod 600 "$TMP"
mv -n "$TMP" "$DEST"
test ! -e "$TMP" && test ! -L "$TMP" || { echo "temporary config was not moved" >&2; exit 1; }
test -f "$DEST" && test ! -L "$DEST" || { echo "installed config is not a regular file" >&2; rm -f "$DEST"; exit 1; }
MODE="$(stat -c '%a' "$DEST")"
test "$MODE" = 600 || { echo "installed config mode is $MODE, expected 600" >&2; rm -f "$DEST"; exit 1; }
trap - EXIT
```

### Noah's shared Steam library

The current `/mnt/shared` Steam library is a `fuseblk` mount with
`allow_other`. It reports cfg files as mode 0755 and ignores both `chmod` and
POSIX ACL changes, so the protected-token installation above correctly fails
its mode check on this machine. Production openfrag must detect this condition
and explain that the cfg token cannot be confidential on this library.

For this throwaway loopback-only capture, use a deliberately non-secret marker
instead of pretending the token is protected:

```sh
DEST='/mnt/shared/SteamLibrary/steamapps/common/Counter-Strike Global Offensive/game/csgo/cfg/gamestate_integration_openfrag.cfg'
test ! -e "$DEST" && test ! -L "$DEST"
sed 's/REPLACE_WITH_RANDOM_LOCAL_TOKEN/OPENFRAG_PROTOTYPE_PUBLIC_MARKER/' \
  prototypes/gsi-listener/gamestate_integration_openfrag_prototype.cfg > "$DEST"
GSI_TOKEN=OPENFRAG_PROTOTYPE_PUBLIC_MARKER \
cargo run --release --manifest-path prototypes/gsi-listener/Cargo.toml
```

The marker is not authentication. Loopback binding is the only exposure
boundary in this fallback test.

Then, from the repository root, start the listener with one command. The token is explicit and the prototype uses its fixed capture path:

```sh
mkdir -p prototypes/gsi-listener/captures
chmod 700 prototypes/gsi-listener/captures
GSI_TOKEN="$(cat prototypes/gsi-listener/.local-token)" \
cargo run --release --manifest-path prototypes/gsi-listener/Cargo.toml
```

Keep that terminal running.

Restart CS2 or load a map after installing the file. The receiver is bound to `127.0.0.1` only. Captures are written to `prototypes/gsi-listener/captures/gsi-listener-capture.jsonl` and may contain names, SteamIDs, team state, scores, weapons, and map/round details.

## Capture checklist

Use a disposable local account or a session whose data you are comfortable retaining. Record the listener output and timestamps for each case:

1. Use an official match or official Practice map for contract evidence. On CS2 build 24134959, several minutes of active Aimbotz play continued to report `player.activity` as `menu` and omitted `map`, `round`, weapons, scores, and deltas, so this workshop map is not a valid live-state fixture.
2. Start in Premier and verify a POST arrives after entering the lobby and again when the match starts.
3. Capture a live alive state, then die and confirm the player state changes to dead.
4. Spectate a teammate and confirm the payload identifies the spectating state without treating it as alive.
5. Observe freeze time, round start, a round in progress, and round end. Check phase, round number, team scores, and bomb state where applicable.
6. Disconnect the game or stop the listener, wait for a timeout, then reconnect or restart the listener. Confirm recovery and whether the first recovered payload is a complete state.
7. Repeat the minimum alive/dead/spectating/round-end checks in Deathmatch. Note which fields are absent or differ from Premier.
8. Confirm no request is accepted on a non-loopback address and that an invalid token is rejected.

## What to report

Provide the CS2 build, map and mode, listener start time, each transition timestamp, HTTP status, payload filenames, missing or null fields, reconnect behavior, and whether any payload contained unexpected personal data. A successful POST proves delivery only. It does not prove event ordering, durable history, exact tick timing, or that every documented component exists in current CS2.

The listener is intentionally minimal: its request-body read has no slow-client deadline. Do not expose it beyond loopback or treat it as a production HTTP server.

## Cleanup

Stop the listener, remove the installed config and captures, and verify no token remains in shell history or tracked files:

```sh
rm -f '/mnt/shared/SteamLibrary/steamapps/common/Counter-Strike Global Offensive/game/csgo/cfg/gamestate_integration_openfrag.cfg'
rm -f prototypes/gsi-listener/captures/gsi-listener-capture.jsonl
rm -f prototypes/gsi-listener/.local-token
```

The tracked config contains only the literal placeholder `REPLACE_WITH_RANDOM_LOCAL_TOKEN`; do not replace it in place.
