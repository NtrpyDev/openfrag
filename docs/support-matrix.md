# openfrag v1 support matrix

## Claim policy

This file is the canonical public support record for openfrag v1. The README gives only the current headline and links here. The deferred website must eventually render or link this same record instead of maintaining a second matrix.

Support is claimed per capability on an exact tested environment. A distribution, GPU vendor, display protocol, compositor family, Steam package, or nearby version does not inherit a claim from another row. Synthetic tests and throwaway prototypes are useful evidence, but they never qualify a release artifact.

The status words are:

| Status | Public meaning |
| --- | --- |
| **Qualified** | An installed release artifact passed every required gate for this capability on the recorded environment. |
| **Limited** | An installed release artifact passed, with the exact published limitation. |
| **Unsupported** | Testing or a settled security decision proves that this capability is not offered on this environment. |
| **Not tested** | Evidence is absent, stale, or too narrow for a public claim. |
| **Prototype only** | A throwaway probe passed, but the production package and identity were not exercised. |

Missing software or inaccessible hardware is a host state, not proof that a GPU or desktop is unsupported. Such a cell remains **Not tested** unless the production product has an explicit supported mechanism and the environment was tested to a conclusive unsupported result.

## Current public headline

No environment is release-qualified yet. The current implementation builds and its headless suite passes on PC1. Physical and read-only prototypes establish limited evidence on KDE and GNOME Wayland, but no static, AUR, or COPR release artifact has passed the complete installed-package gates under the production application identity.

| Environment | Static release | AUR | COPR | Overall v1 claim |
| --- | --- | --- | --- | --- |
| PC1, AMD, KDE Plasma Wayland, native Steam | Not tested | Not tested | Not tested | Prototype only |
| PC1, AMD, KDE Plasma Wayland, Flatpak Steam | Not tested | Not tested | Not tested | Not tested |
| PC1, AMD, X11 | Not tested | Not tested | Not tested | Not tested |
| PC2, NVIDIA RTX 3070 Ti, GNOME Wayland, no Steam | Not tested | Not tested | Not tested | Prototype only |

The product must not say “supports AMD,” “supports NVIDIA,” “supports Wayland,” “supports X11,” “works on KDE,” “works with Flatpak Steam,” or “works on any distro” from this table. It may say that openfrag is being developed for Linux and link to the exact evidence here.

## PC1 first pass

Collected 2026-07-14 on the local PC1 session.

| Dimension | Observed value |
| --- | --- |
| Distribution and kernel | CachyOS, Linux `7.1.3-2-cachyos`, x86-64 |
| GPU | AMD Navi 48 Radeon RX 9070-class device using `amdgpu`; Intel UHD 770 also present |
| Desktop | KDE Plasma Wayland `6.7.2`, KWin `6.7.2` |
| Portal | xdg-desktop-portal `1.22.1`, KDE backend `6.7.2`, Global Shortcuts version 2, host Registry version 1 |
| Steam | Native Steam `1.0.0.86`; app 730 is in a secondary external library |
| CS2 library filesystem | FUSE-mounted external library; the cfg directory is writable but does not preserve private POSIX modes |
| Media tools | FFmpeg and FFprobe `8.1.2` available |
| Recorder | Neither native nor Flatpak gpu-screen-recorder installed |
| Production package | No production desktop file, canonical user unit, AUR package, or COPR package installed |

### PC1 results

| Gate | Result | Interpretation |
| --- | --- | --- |
| Current implementation CI | Passed | All first-party format, test, clippy, release build, staged static install, deterministic archive, packaging boundary, and headless end-to-end checks passed at local implementation commit `102b59a0161ecf34b8dd6c540a7078110a881c62`. Vendored parser warnings remain, but the first-party clippy gates passed. |
| Static package scaffold | Passed headlessly, not qualified | The deterministic archive and temporary-home install passed without activating systemd. The scaffold still uses legacy `openfrag.desktop` and `openfragd.service`, so it fails the settled production identity contract and is not a release candidate. |
| AUR and COPR artifacts | Not tested | Neither artifact exists in the inspected implementation tree. |
| Storage and local Demo readiness | Prototype only | The current read-only Doctor skeleton reported a private temporary data directory and local Demo import ready. It does not yet implement the final storage, fingerprint, or remediation contract. |
| Native Steam discovery | Prototype only | The app-730 manifest and real CS2 cfg directory were found in an external library by direct inspection and explicit Doctor override. Current automatic discovery does not parse `libraryfolders.vdf`, so the final discovery gate is not implemented. |
| Flatpak Steam discovery | Not tested | Flatpak Steam is not installed on PC1. |
| GSI cfg write | Prototype only | Earlier safe prototype work wrote and verified the cfg on this filesystem and found that mode `0600` was not preserved. The production cfg is currently absent. This is a named filesystem limitation, not a secret-loss claim because the GSI marker is not a Steam credential. |
| GSI delivery | Prototype only | Official local Competitive and Deathmatch emitted authenticated state snapshots under the [GSI listener spike](https://github.com/NtrpyDev/openfrag/issues/10). Premier delivery remains untested, and no current installed package repeated the test. |
| Recorder capability and test Clip | Not tested | gpu-screen-recorder is absent, so the current Doctor correctly blocked capture and Manual Flag end-to-end testing. No AMD codec or real replay-save claim is made. |
| Audio routing | Prototype only, limited topology | The safe synthetic PipeWire monitor test passed again on 2026-07-14. The run found no non-monitor microphone and no selectable CS2 or voice application stream, while gpu-screen-recorder remained absent. Three-track capture is not claimed. See the [audio routing decision](https://github.com/NtrpyDev/openfrag/issues/18). |
| Global Shortcuts portal behavior | Prototype only | The [Manual Flag portal spike](https://github.com/NtrpyDev/openfrag/issues/23) passed approval, exactly-once holds, daemon restart, lock and unlock, and fullscreen CS2 on this KDE stack. It used a temporary probe application identity, not the production package. |
| Production portal identity and restored binding | Not tested | The canonical desktop file and `app-io.github.ntrpydev.openfrag.service` are not installed. Prototype restoration cannot qualify the production identity. |
| One hold to one validated Clip | Not tested | Portal activation has prototype evidence, but the host lacks the recorder and no production service joined activation to a validated saved Clip. |
| Service idle and startup behavior | Not tested | No canonical production user service is installed. Headless unit inspection cannot prove real login, idle-no-capture, enable, disable, or restart behavior. |
| Local Demo import through installed UI | Not tested | Parser and pipeline fixtures pass, but no installed production artifact completed browser selection through canonical Match, Rating, and Receipt display on this host. |
| Local-only and no-upload boundary | Passed headlessly, not qualified | The headless smoke passed Doctor, GSI auth, local HTTP APIs, invalid import, deduplication, redaction, and shutdown without remote assets. An installed network-denied end-to-end run remains required. |

The PC1 result is therefore **Prototype only** for KDE Wayland GSI, synthetic audio routing, and Global Shortcuts behavior, with no release-qualified capture or package claim.

## PC2 first pass

Collected 2026-07-14 through an authenticated SSH session as the active desktop user. Only the redacted facts below are published. No screen, microphone, personal media, Steam identifier, home path, filename, GSI marker, Demo, or Clip was collected.

| Dimension | Observed value |
| --- | --- |
| Distribution and kernel | CachyOS, Linux `7.1.3-2-cachyos`, x86-64 |
| GPU | NVIDIA GeForce RTX 3070 Ti using the proprietary `nvidia` driver `610.43.03` |
| Desktop | GNOME `50.3`, Mutter `50.3`, active Wayland session |
| Portal | xdg-desktop-portal `1.22.1`, GNOME backend `50.0`, Global Shortcuts version 1 |
| Steam and CS2 | Neither native nor Flatpak Steam is installed; no CS2 cfg directory was found |
| Media tools | FFmpeg and FFprobe `8.1.2` available; PipeWire `1.6.7` and WirePlumber `0.5.15` active |
| Recorder | Neither native nor Flatpak gpu-screen-recorder installed |
| Production package | No production desktop file, canonical user unit, static package, AUR package, or COPR package installed |

### PC2 results

| Gate | Result | Interpretation |
| --- | --- | --- |
| Exact candidate integrity | Passed transfer, not qualified | `openfragd 1.0.0` was built from commit `102b59a0161ecf34b8dd6c540a7078110a881c62`; the local and PC2 binaries matched SHA-256 `c566bde02955ee8a0a109edeb0d6c16957d84085f82ae397b7411ffe8678ba6d`. It ran from a private runtime directory and was not an installed package. |
| Compatibility Doctor | Prototype only | The read-only Doctor reported temporary private storage, local Demo import, and FFprobe ready. GSI was blocked because no CS2 cfg directory exists. Capture was blocked because gpu-screen-recorder is absent, and Manual Flag was consequently blocked. |
| NVIDIA host verifier | Failed due to verifier defect | The exact `--probe-nvenc` verifier reported the driver, FFmpeg, FFprobe, PipeWire, and output directory ready, but recorder and NVENC blocked. The NVENC failure came from its fixed `128x128` test frame, which driver `610.43.03` rejected as smaller than the supported minimum. This is not evidence that NVENC is unavailable. |
| Direct NVENC control | Prototype only | Repeating the same bounded synthetic `h264_nvenc` test at `256x256` completed successfully. This proves encoder initialization on this exact stack, not screen capture or gpu-screen-recorder integration. |
| Native and Flatpak Steam discovery | Not tested | Neither Steam package is installed, so app-730 discovery, external-library discovery, cfg writing, and authenticated GSI delivery cannot run. Missing host software is not a product failure. |
| Recorder capability and validated test Clip | Not tested | gpu-screen-recorder is absent. No replay buffer, screen capture, codec integration, preview, or validated Clip was attempted. |
| Audio routing | Prototype only, limited topology | A synthetic PipeWire null-sink monitor produced a verified 1.2-second stereo WAV with nonzero signal without selecting a real microphone. The probe found one non-monitor source and no active CS2 or voice-application candidates. Its `pw-record` process returned status 1 after producing the complete file, so the result does not qualify production recording. |
| Global Shortcuts portal | Not tested | The host exposes Global Shortcuts version 1, but no production identity is installed and no approval, activation, restoration, lock and unlock, or fullscreen test was attempted over SSH. |
| One hold to one validated Clip | Not tested | Both the production shortcut binding and replay recorder are absent. |
| Service idle and startup behavior | Not tested | No canonical or legacy openfrag user service is installed. Login startup, explicit enable and disable, idle-no-capture, restart, and logout behavior were not exercised. |
| Local Demo import through installed UI | Not tested | The headless Doctor says local import is available, but no installed dashboard, local Demo selection, Local Player proof, Match, Rating, or Receipt was exercised. |
| Static, AUR, and COPR artifacts | Not tested | No release artifact is installed. A temporary candidate binary is not evidence for any package channel. |
| Local-only and no-upload boundary | Not tested | The Doctor emitted only the expected redacted local facts, but no installed network-denied end-to-end gate was run. |

The PC2 result is therefore **Prototype only** for read-only Doctor behavior, direct NVENC initialization, and synthetic audio routing. NVIDIA screen capture, GNOME Global Shortcuts activation, Steam integration, packages, services, and Clips remain **Not tested**.

### PC2 work still required for qualification

1. Install one exact candidate package through the channel under test. Verify the canonical desktop entry, canonical user unit, `app.slice`, explicit startup choice, idle behavior without capture, and no legacy identity files.
2. Install the intended native or Flatpak Steam configuration and CS2. Exercise discovery, including an external library when present, then confirm only `gamestate_integration_openfrag.cfg` is written after consent and authenticated GSI delivery follows a CS2 restart or map reload.
3. Install gpu-screen-recorder from a trusted package source. With explicit screen and audio consent, run the 60-second replay-mode test, save after warmup, inspect the Clip with FFprobe, preview it locally, and record the exact codec, video result, audio topology, and limitations.
4. Bind `manual_flag` under the production identity, test one press-and-hold, verify one validated Clip, restart the daemon, verify restoration without another bind, lock and unlock, then repeat with fullscreen CS2.
5. Select and import a local Demo through the installed dashboard. Verify an explicit result, Local Player proof, Match, Rating availability, and Receipts without Steam authentication or outbound acquisition.
6. Repeat the network-denied privacy gate and export only the redacted result.

Any step that needs a desktop portal, screen capture, audio capture, playback, or CS2 is a human-observed test. It must not be marked passed from SSH alone.

## Release qualification gates

Every claimed row is keyed by package channel, exact artifact SHA-256, openfrag commit, architecture, distribution release, kernel, GPU and driver, session type, compositor, portal and backend versions, Steam package, recorder version, and media-tool versions. A material change makes the result stale until rerun. Public copy may name the exact tested stack and limitation; it may not infer a family-wide claim.

For each of static release, AUR, and COPR, the following must all pass before any row becomes **Qualified**:

1. Build the exact release candidate from a clean checkout and record reproducible artifact hashes.
2. Install through the real channel, not a source-tree launcher or temporary probe identity.
3. Pass desktop-file and systemd verification under `io.github.ntrpydev.openfrag` and `app-io.github.ntrpydev.openfrag.service`; prove no legacy identity remains.
4. Pass private config and data-root behavior, SQLite locking, safe filesystem semantics, and action-specific space checks.
5. Discover the intended native or Flatpak Steam installation and app-730 library, safely write the cfg after consent, and observe current authenticated GSI delivery.
6. Probe the installed recorder, codec, capture target, FFprobe, FFmpeg, and selected audio topology; save and preview a validated replay Clip.
7. Bind and restore `manual_flag` under the production application identity; prove exactly one activation and one validated Clip for one hold, including fullscreen CS2 and portal restart behavior.
8. Import a local Demo through the dashboard and reach the correct explicit pipeline result, Local Player proof, Match, Rating availability, and Receipts.
9. Enable and disable the user service through the explicit product choice; prove service start, restart, idle-no-capture, GSI-triggered capture, staleness stop, and clean logout behavior.
10. Run the installed application with outbound networking denied and verify that local setup and import need no account, Steam sign-in, Share Code, Game Coordinator, automatic Demo discovery, Demo URL, telemetry, upload, website, raw input, injection, or memory reading.
11. Save a redacted Doctor result and the human observations with timestamps. Record failures and limitations with the same prominence as passes.
12. Repeat the row on every stack named by public copy. One PC or package channel never qualifies another.

## Publication format

The README contains a short **Support status** section with the current headline and a link to this file. It does not embed a second table. The future website is outside v1 app readiness; when website work begins, it must link to an immutable revision or generate its table from a reviewed machine-readable successor to this record. Until then, no website support claim exists.

Release notes link to the matrix revision used for that release. A change to a result requires the evidence date, exact environment fingerprint, affected capability, and reviewer-visible reason. Deleting a failed or unsupported result to simplify marketing is prohibited; superseded rows remain available through Git history.
