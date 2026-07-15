# Setup Wizard and Compatibility Doctor for v1

## Decision

The Setup Wizard is the first-run dashboard flow that configures local capabilities. The Compatibility Doctor is the reusable diagnostic engine behind that flow and the Settings diagnostics page. They share one probe model, one dependency graph, one set of stable check IDs, and one remediation catalog. The Wizard may perform a mutation only after the player selects the corresponding action. A normal Doctor run is read-only and never opens a portal, starts a capture, records audio, writes a CS2 file, imports a Demo, changes a systemd unit, or requests elevated privileges.

Setup is capability-based, not one all-or-nothing gate. Writable private storage is the only requirement for finishing first run and entering the dashboard. A player may finish with capture, live GSI, Manual Flag, or Demo analytics unavailable and return from Settings later. Every unavailable capability remains visible with its exact dependency and action. There is no silent downgrade, fake success, or unsafe fallback.

The v1 flow contains no Steam sign-in, QR code, Share Code input, Game Coordinator access, automatic Demo discovery, Demo URL fetch, raw input permission, or website dependency. Local Demo import works when Steam, CS2, gpu-screen-recorder, and the Global Shortcuts portal are absent.

This decision refines the settled [storage layout](https://github.com/NtrpyDev/openfrag/issues/13), [local Demo policy](https://github.com/NtrpyDev/openfrag/issues/19), [recorder boundary](https://github.com/NtrpyDev/openfrag/issues/4), [audio model](https://github.com/NtrpyDev/openfrag/issues/18), [Manual Flag portal contract](https://github.com/NtrpyDev/openfrag/issues/23), rejected [evdev fallback](https://github.com/NtrpyDev/openfrag/issues/24), and [packaged application identity](https://github.com/NtrpyDev/openfrag/issues/28).

## Status model

Every Doctor check returns one of five statuses for the current configuration:

| Status | Meaning |
| --- | --- |
| `ready` | Current evidence proves this check passed. |
| `limited` | The capability works, but a named optional feature is absent. |
| `needs_action` | A supported, explicit player action can make the check pass. |
| `unavailable` | The supported mechanism is absent or unsafe on this host. No fallback is attempted. |
| `not_tested` | The check requires a consented active test that has not passed for the current inputs. |

Each result contains `check_id`, `status`, a one-sentence summary, observed facts, the exact affected capability, a remediation action or `null`, `observed_at`, and an input fingerprint. It never contains a GSI token, raw GSI body, input event, audio sample, Steam credential, remote endpoint, or unredacted child-process output.

Changing an input invalidates that check and every dependent active-test result. Examples include a different storage root, CS2 installation, local SteamID, recorder executable, codec, capture target, audio route, desktop portal owner, application identity, or shortcut trigger. Invalidated active tests return to `not_tested`; they are never presented as historical proof of the new configuration.

## Exact first-run flow

The dashboard resumes at the first unresolved step after a restart. Back and Continue never discard a passed independent capability. Every step has **Run check again**, and Settings exposes the same steps after first run.

### 1. Welcome and boundaries

Show these facts before any choice:

- openfrag is a local application with no account, backend, telemetry, or upload path.
- v1 imports a Demo file selected from disk and analyzes it locally.
- openfrag does not sign into Steam or obtain Demos from Steam.
- GSI is Valve's local interface for provisional live state; imported Demos remain authoritative for analytics and Receipts.
- gpu-screen-recorder is a separately installed and supervised process.
- Manual Flag uses the desktop's user-approved Global Shortcuts portal only.

The player continues without accepting terms, creating an account, or enabling a network feature.

### 2. Storage

Default the data root to `${XDG_DATA_HOME:-~/.local/share}/openfrag`. The picker is labeled **Storage location** and shows the derived database, Demo, Clip, staging, and export directories. Selecting another root before first use configures the Clip location by moving the whole openfrag data root. v1 does not split the SQLite ledger, Demos, and canonical Clips across independent roots because the storage contract requires one relative, atomic artifact namespace.

Configuration has one fixed private root at `${XDG_CONFIG_HOME:-~/.config}/openfrag`. It contains `setup.toml` for player choices and input fingerprints plus `gsi-marker` for the local GSI request marker. The directory is `0700` and both files are `0600`. `setup.toml` records the selected data root, so a custom data root remains discoverable. Canonical database and artifact bytes never move into the config root, XDG state root, cache root, or runtime directory. Failure to create and preserve the private config root blocks setup just like failure of the selected data root.

The storage check creates a private probe inside the selected root, verifies directory mode `0700` and new-file mode `0600` where the filesystem supports POSIX modes, writes and fsyncs bytes, atomically renames the probe, opens SQLite with the production settings, commits and rolls back a transaction, checks locking from a second connection, then deletes the probe. It rejects symlink components, non-directories, path traversal, failed atomic rename, failed fsync, failed SQLite locking, and roots not owned and writable by the current user. It reports filesystem type, available bytes, and whether requested private modes were preserved.

Mode preservation is required for the openfrag data root. A filesystem that ignores private modes is `unavailable` for canonical data even if it is writable. A mounted local volume may be used when the behavioral checks pass. Network filesystems are not supported for the SQLite ledger. No fixed free-space amount makes storage globally ready: Demo import and capture perform action-specific headroom checks. The storage card always shows free space.

If the existing root contains a valid openfrag database, the Wizard opens it in place and does not reinitialize it. Once canonical artifacts exist, changing the root is blocked in v1 because backup and migration are not yet specified. The action is **Keep this location**. The Wizard never suggests copying a live SQLite file by hand.

### 3. Locate CS2 and install GSI

This step is optional for local Demo analytics and required for provisional Auto Highlights.

Discover these Steam roots without requiring Steam to run:

1. `${XDG_DATA_HOME:-~/.local/share}/Steam`
2. `~/.steam/steam`, canonicalized and deduplicated against the first root
3. `~/.var/app/com.valvesoftware.Steam/data/Steam` for Flatpak Steam

For each root, parse `steamapps/libraryfolders.vdf` with a bounded VDF parser. Treat every declared library path as untrusted local input. A CS2 candidate must contain `steamapps/appmanifest_730.acf`; its `installdir` selects `steamapps/common/<installdir>`, and the selected game root must contain `game/csgo/gameinfo.gi` plus a real `game/csgo/cfg` directory. This discovers external and secondary libraries instead of assuming CS2 is under the Steam root. Malformed VDF, an inaccessible library, a symlinked target, and a stale manifest are separate diagnostics.

If no candidate is valid, offer **Choose CS2 cfg folder**. A manual choice must be the real `game/csgo/cfg` directory and must have the sibling `game/csgo/gameinfo.gi` at the expected relative location. If multiple candidates are valid, show native or Flatpak Steam, the canonical game path, manifest build ID when present, and filesystem type, then require the player to choose. Never select the first candidate silently.

The **Install openfrag GSI config** action:

1. Generates a cryptographically random 32-byte base64url marker on first install. It is a loopback request marker, not a Steam credential.
2. Writes the marker to `${XDG_CONFIG_HOME:-~/.config}/openfrag/gsi-marker` with mode `0600`.
3. Writes only `gamestate_integration_openfrag.cfg` in the chosen CS2 cfg directory, pointing to `http://127.0.0.1:7130/gsi/router` with the matching auth marker and the settled v1 component set.
4. Rejects a symlink, directory, or other unsafe target. If a regular file differs, show that the openfrag-owned file will be replaced and require confirmation. Never edit another cfg file.
5. Uses a same-directory temporary file, fsync, and atomic rename when the CS2 filesystem supports it. If atomic replacement is unavailable, leave the prior file intact and report `gsi_config_write_failed`.
6. Verifies exact bytes after writing. If the CS2 filesystem ignores mode `0600`, report `limited` and explain that the marker is not confidential while retaining the loopback and auth checks. Do not call it a secret or a Steam token.

Installing the cfg does not require a local SteamID. If CS2 was already running, tell the player to restart CS2 or reload a map. The active test waits for a correctly authenticated POST with provider app ID `730`, records only redacted bounded diagnostics, and reports staleness. A listener restart alone is not claimed to restore delivery.

### 4. Identify the local player

openfrag needs one verified SteamID64 to select the local player in GSI and Demo evidence. It never obtains that identity through Steam authentication.

There are two valid local proofs:

- **GSI proof:** an authenticated app-730 payload supplies a decimal 17-digit provider SteamID64. When a player SteamID is present, it must agree. Show the ID and ask **Use this local player** before persisting it.
- **Demo proof:** after a selected Demo parses, show its ten-player roster and ask **Which player is you?** Persist that participant's SteamID64. This selection is local and does not query Steam.

The player may defer identity here and resolve it from the Demo step. Free-form SteamID entry is not a verified proof and is not offered in the v1 dashboard. If a later GSI payload or imported Demo excludes the persisted ID, do not switch automatically. Report `local_player_mismatch`, keep existing Matches unchanged, and require an explicit identity review. Changing the identity after a Match exists is blocked in v1 because it would change the meaning of personal Rating and trends.

### 5. Replay capture and audio

This step is optional for Demo analytics and required for Auto Highlight Clips and Manual Flag.

Probe native `gpu-screen-recorder` on `PATH` and Flatpak application `com.dec05eba.gpu_screen_recorder`. If both exist, show both and require a choice. Run the selected installation's version, `--info`, capture-target enumeration, audio-device enumeration, and application-audio enumeration with bounded output and timeouts. Do not install packages, run a shell, use `sudo`, or broaden device permissions. `ffprobe` is required to validate a Clip. Missing `ffmpeg` makes trim and export `limited`, not replay capture unavailable.

The player chooses a capture target from the recorder's current enumeration. Wayland never offers X11-only `window` or `focused` targets. Any ScreenCast portal dialog is opened only from **Choose screen or window**. Persist the semantic target and re-resolve ephemeral portal or PipeWire identifiers at each start.

Audio choices follow the proved routing model:

- Three tracks are offered only when PipeWire exposes distinct CS2 application audio, a separate voice application, and a non-monitor microphone.
- CS2 in-game voice that shares the CS2 stream is labeled `mixed_game_voice` and cannot be promised as a separate track.
- A microphone is optional and requires a deliberate choice. Monitor sources are never offered as microphones.
- Missing or ambiguous sources never silently become `default_input`, another application, or another device.
- Application roles are re-resolved on each recorder start; stable device properties are used for hardware roles, never transient numeric object IDs alone.

The **Run test capture** action previews the selected screen and audio disclosure, then launches one owned recorder child in 60-second replay mode. After at least 12 seconds of warmup, openfrag sends `SIGUSR1`, validates the returned path beneath private staging, waits for stable nonzero bytes, and uses `ffprobe` to require one playable video stream, positive dimensions and frame rate, 8 to 20 seconds of duration, the selected codec, and the expected number of audio tracks. The player then watches the local preview and confirms that the intended picture and each chosen audio role are present. Cancel, silence, wrong screen, missing track, recorder exit, timeout, invalid path, and media-probe failure each preserve bounded redacted diagnostics and return `needs_action`. The test file is deleted after the result unless the player explicitly keeps it.

Passing the active test persists the exact recorder installation, version, codec, target, audio-role fingerprints, and input fingerprint. A changed input invalidates the pass. Runtime capture still reports warming, running, saving, backoff, and faulted states; a passed setup test is not a permanent health claim.

### 6. Manual Flag

This step requires a passed replay test but does not require GSI, CS2, or a local SteamID.

First require the packaged identity checks for `io.github.ntrpydev.openfrag`, `io.github.ntrpydev.openfrag.desktop`, and `app-io.github.ntrpydev.openfrag.service` as specified by the identity decision. Any dash-substituted or legacy spelling is invalid. The daemon must be `ExplicitlyRegistered` or `CgroupInferred` before using restored shortcut state.

If a valid `manual_flag` binding is restored, display the portal-returned nonempty trigger description. Otherwise, **Choose Manual Flag shortcut** explains that the desktop will show its own approval UI and makes one `BindShortcuts` attempt. Cancellation, a conflict, an empty returned subset, portal unavailability, invalid application identity, or portal restart is visible and retryable. openfrag never invents the accepted trigger or asks for `input` group membership.

The **Test Manual Flag** action asks the player to press and hold the accepted shortcut once until the UI acknowledges it. A pass requires exactly one current-session `Activated`, no second activation during the hold, a matching `Deactivated` lifecycle signal when the backend supplies it, one `SIGUSR1` request, and one validated saved Clip. Timestamp `0` is valid. The preview identifies whether a failure occurred in portal activation, recorder save, or media validation. No evdev, console-log, memory-reading, injection, or raw-device fallback is installed.

### 7. Import a local Demo

Show the settled promise verbatim: **Import a demo file from disk. openfrag analyzes it locally; no Steam credentials or network match-history access are required.**

The browser file picker accepts `.dem` files and streams the chosen bytes only to the loopback daemon. The pipeline verifies readable bytes, the recognized Demo header, the exact 2 GiB source limit, source-copy space plus 10 percent headroom, and bounded 8 MiB streaming before hashing and deduplication. The result is `ready`, **Already imported**, `error_io`, `error_corrupt`, `error_unsupported`, or `error_size`, followed by the durable parse and analysis states. An error never creates an empty Match.

Importing a Demo during setup is optional. If no local player identity exists, a successful parse continues to the roster choice from step 4 before personal stats or Rating become canonical. The Wizard contains no Share Code field and does not inspect Steam for Demo files. A player without a Demo selects **Import later**, and the dashboard keeps the local import action prominent.

### 8. Background start and summary

The package stages the canonical user service but does not silently enable it. `openfrag-launch` may start it for the current dashboard session. The final step requires an explicit choice:

- **Start openfrag when I sign in** enables `app-io.github.ntrpydev.openfrag.service` in the user manager so local GSI and capture can be ready before play.
- **Start only when I open openfrag** leaves it disabled and explains that live candidates and Manual Flag are unavailable until the app is started.

Neither choice needs root. The daemon may idle at login, but it must not start desktop capture merely because the service started. When capture is enabled, replay capture begins either on the first authenticated app-730 GSI payload or after the player explicitly selects **Start Replay Buffer**. A GSI-started recorder stops after 60 seconds without another authenticated app-730 payload. An explicitly started recorder runs until **Stop Replay Buffer**, service stop, or logout. Restarting after any stop creates a fresh empty Replay Buffer and is surfaced as warming. This preserves Manual Flag without a GSI dependency while avoiding login-long desktop recording without observed CS2 activity or explicit consent.

The final summary shows independent capability cards:

| Capability | Ready when |
| --- | --- |
| Dashboard and local storage | Storage passes. |
| Local Demo analytics | Storage and local player identity pass; an import may remain `not_tested`. |
| Provisional Auto Highlights | Storage, GSI delivery, local player identity, and replay capture pass. |
| Manual Flag | Replay capture, packaged portal identity, binding, and end-to-end flag test pass. |
| Clip trim and export | Replay capture and FFmpeg pass. |

**Finish setup** is available when storage passes and every other step is either resolved, explicitly deferred, or explicitly unavailable. Finishing never changes a red or gray capability to green.

## Compatibility Doctor checks

The Doctor runs the read-only checks immediately and exposes consented active tests beside them.

| Stable check ID | Proof | Player action when not ready |
| --- | --- | --- |
| `storage_root` | Private modes, safe path, atomic I/O, fsync, SQLite transaction and locking | Choose an empty compatible local location |
| `storage_space` | Current available bytes and action-specific estimates | Free space or choose storage before the affected action |
| `service_identity` | Canonical desktop file, user unit, slice, running unit, and portal identity state | Reinstall the package and start the canonical service |
| `service_startup` | User service enabled or intentionally disabled | Choose the background-start preference |
| `steam_roots` | Native and Flatpak roots parsed without treating Steam as authentication | Choose CS2 cfg manually if discovery fails |
| `cs2_install` | App 730 manifest, install directory, `gameinfo.gi`, and real cfg directory | Choose the correct CS2 cfg directory or fix user ownership |
| `gsi_config` | Safe exact openfrag cfg, matching local marker and loopback URI | Install or replace the openfrag cfg with confirmation |
| `gsi_delivery` | Fresh authenticated app-730 payload under the settled evidence contract | Restart CS2 or reload a map, then run the active test |
| `local_player` | Explicitly confirmed GSI or Demo roster proof | Confirm the detected ID or select yourself from an imported Demo |
| `recorder_install` | Selected native or Flatpak recorder answers bounded capability probes | Install gpu-screen-recorder from a trusted package source |
| `capture_target` | Current enumeration contains the selected semantic target | Choose an available target and approve its portal if needed |
| `audio_routes` | Selected roles resolve uniquely under the audio contract | Start the applications, choose explicit routes, or accept a named limitation |
| `ffprobe` | Executable answers a bounded version probe | Install FFprobe, then rerun |
| `ffmpeg` | Executable answers a bounded version probe | Install FFmpeg for trim and export, then rerun |
| `test_capture` | Current input fingerprint passed the consented replay save and preview | Run Test Capture |
| `global_shortcuts` | Global Shortcuts exists under verified application identity | Install or enable the correct desktop portal backend, or accept unavailable |
| `manual_flag_binding` | Current session has exactly one valid `manual_flag` | Choose the shortcut through the desktop approval UI |
| `manual_flag_e2e` | One hold produced one activation and one validated Clip | Run Test Manual Flag and follow the failing layer's action |
| `demo_import` | Most recent selected local Demo reached a durable explicit result | Choose a local Demo, retry the documented safe stage, or import later |
| `loopback_surface` | Dashboard and GSI listeners bind only approved loopback addresses | Stop the service and report a packaging or configuration fault |

Doctor never collapses `unknown`, timeout, permission denied, malformed output, and confirmed absence into one boolean. It includes bounded tool version and exit information. A downloadable diagnostics report requires a separate click, replaces the home path with `$HOME`, omits selected filenames and SteamIDs by default, and never includes tokens, GSI bodies, Clip bytes, Demo bytes, raw audio, or raw child output. The support matrix may consume the nonprivate versions and status IDs only.

## Required remediation behavior

- A permission problem names the exact path and current ownership. openfrag does not run `sudo`, change another user's files, add groups, or generate a privileged helper command.
- A missing package names the missing executable or Flatpak application and tells the player to install it through a trusted distribution mechanism. openfrag does not download or execute an installer.
- An inaccessible external Steam library is not mistaken for absent CS2. Show the library path and offer the validated manual cfg picker.
- A stale GSI stream says **Restart CS2 or reload a map**, then offers the delivery test. It does not claim that restarting openfrag repairs CS2 delivery.
- A local-player mismatch stops personal analytics and asks for identity review. It does not merge evidence from two players.
- A missing or ambiguous audio route names the affected role and offers an explicit mixed or no-microphone limitation when valid.
- A failed capture shows the selected recorder, target, codec, failed stage, and bounded redacted diagnostic. It never accepts an unvalidated output path as a Clip.
- A portal cancellation, conflict, or empty binding remains `needs_action`. An absent portal or invalid package identity is `unavailable` until the host or package changes.
- An unsupported desktop reports Manual Flag unavailable. It never proposes evdev, `input` group access, a raw device ACL, or console-log automation.
- Demo errors retain their stable pipeline code and exact retry or remove action. They never recommend Steam sign-in or automatic acquisition.

## State and API contract

The daemon, not browser JavaScript, owns discovery, mutations, dependency invalidation, and status calculation. The dashboard renders typed results and invokes narrow actions. A browser-supplied path string never authorizes a filesystem mutation by itself.

Persist player choices and their input fingerprints in private local configuration. Persist durable import state in SQLite. GSI and portal session state remain runtime state. Store only the last bounded redacted diagnostic needed for remediation. All mutation endpoints require the loopback session's anti-CSRF mechanism, validate an explicit expected prior fingerprint to prevent stale-tab writes, and return the affected checks after completion.

The implementation dependency graph must express at least these edges:

```text
storage_root -> gsi_config, test_capture, demo_import
cs2_install -> gsi_config -> gsi_delivery -> local_player (GSI proof)
demo_import -> local_player (Demo proof)
recorder_install, capture_target, audio_routes, ffprobe -> test_capture
service_identity -> global_shortcuts -> manual_flag_binding
test_capture, manual_flag_binding -> manual_flag_e2e
local_player, gsi_delivery, test_capture -> provisional Auto Highlights
local_player, demo_import -> local Demo analytics
```

The GSI and Demo identity branches are alternatives, not mutual blockers. Capture failure does not block Demo import. GSI failure does not block Manual Flag. Portal failure does not block Auto Highlights or local Demo analytics.

## Implementation corrections

The current implementation branch is a useful skeleton but is not this contract yet:

- Its ordered setup flow places local Steam identity before GSI installation. Production must install GSI without a SteamID and accept either later GSI proof or Demo roster proof.
- Its Steam discovery checks only fixed default game paths. Production must parse `libraryfolders.vdf` and app 730 manifests so secondary and external libraries work.
- Its Doctor reduces unknown and missing states to booleans, marks portal binding unavailable without performing the identity state machine, and lacks active-test fingerprints.
- Its API exposes summaries without remediation, evidence, timestamps, capabilities, or explicit actions.
- Its replay configuration can start capture with daemon startup. Production must honor the explicit background choice without recording an idle desktop merely because the user service is running.
- Its package assets still use legacy desktop and service names that the packaged identity decision replaced.

These are implementation gaps, not alternate accepted behavior.

## Validation and release gates

1. Unit-test the status enum, dependency graph, alternative identity proofs, invalidation, finish criteria, and every remediation mapping as pure deterministic logic.
2. Fixture-test native Steam, Flatpak Steam, symlinked duplicate roots, multiple libraries, an external library containing app 730, malformed and oversized VDF, stale manifests, multiple valid CS2 installs, and manual cfg validation.
3. Filesystem-test private-mode enforcement, unsupported mode semantics, symlink rejection, safe existing-file handling, atomic replacement failure, fsync failure, SQLite locking failure, low-space reporting, and idempotent reruns without mutating files outside the fixture.
4. Verify the generated GSI cfg byte-for-byte, loopback URI, bounded component set, marker redaction, unsafe-target rejection, differing-file confirmation, CS2 restart guidance, authenticated delivery, staleness, and no local SteamID prerequisite.
5. Test both local-player proofs, refusal of an unverified free-form ID, GSI and Demo mismatch, and the prohibition on changing identity after canonical Matches exist.
6. Fake native and Flatpak recorder probes, timeouts, large output, invalid UTF-8, missing FFprobe, missing FFmpeg, ambiguous targets, changed inputs, and every audio topology. No probe may invoke a shell or elevation.
7. Run the consented replay test with a fake recorder and media probe for every failure stage. On each claimed host stack, run a real installed-package test and inspect video, duration, selected codec, track count, track roles, and local playback confirmation.
8. Fake the portal identity and Global Shortcuts protocols for registration success, cgroup fallback, invalid identity, restore, cancellation, conflict, empty result, owner loss, stale signals, duplicate activation, and exact one-hold-to-one-Clip behavior. Run the real test on each support-matrix claim.
9. Browser-test first run, restart resume, back navigation, skip and return, partial capability readiness, stale-tab mutation rejection, keyboard-only use, focus movement into portal-returned errors, live-region announcements, and Settings reruns.
10. In a network-denied end-to-end environment, finish setup and import a fixture Demo. Assert no Steam login, QR, Share Code, Game Coordinator, Demo discovery, Demo URL, website, telemetry, upload, input-device, injection, or memory-reading path exists in UI, API, subprocesses, or outbound attempts.
11. Test static, AUR, and COPR installed artifacts, native and Flatpak Steam roots, the canonical user service lifecycle, no automatic enablement, explicit enable and disable, and idle service behavior without desktop capture.
12. Produce a redacted Doctor report fixture and prove it excludes home paths, SteamIDs by default, tokens, filenames, GSI bodies, media, and raw subprocess output while retaining versions, check IDs, statuses, and safe failure stages.

The Support Matrix ticket owns the physical host runs and public claims. A green synthetic or local-development test alone does not establish support for a GPU, compositor, portal backend, Steam package, or distribution artifact.
