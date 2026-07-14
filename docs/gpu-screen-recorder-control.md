# Controlling gpu-screen-recorder from the openfrag daemon

This note describes the boundary between openfrag and the separately launched
`gpu-screen-recorder` process. In CONTEXT.md terminology, the recorder owns the
rolling **Replay Buffer** and emits a **Clip** when the daemon handles a
**Manual Flag**.

## Decision

openfrag will launch one `gpu-screen-recorder` child directly, retain its PID,
and supervise it inside the Rust daemon. It will use the CLI for configuration,
`SIGUSR1` for a full Replay Buffer save, `SIGINT` or `SIGTERM` for shutdown,
stdout for save-completion candidates, and stderr for diagnostics. It will not
use a shell, broad process-name signals, the asynchronous `-sc` hook, or a
linked gpu-screen-recorder library.

## Launching replay mode

Replay mode is selected with `-r SECONDS`; `-o` is a directory in this mode.
The upstream example is `gpu-screen-recorder -w screen -f 60 -r 30 -c mp4 -o
~/Videos`. `-w` selects the capture target, `-f` the frame rate, `-c` the
container, and `-a` selects audio input(s). Enumerate valid capture and audio
names before launch with `--list-capture-options`, `--list-audio-devices`, and
`--list-application-audio`; `--info` reports the selected capture/encode device
and supported codecs. [Upstream README, recording and replay sections](https://git.dec05eba.com/gpu-screen-recorder/plain/README.md)

A representative launch is:

```text
gpu-screen-recorder -w <selected-source> -f 60 -c mkv -k <tested-codec> \
  -a default_output -a default_input -r 60 -bm cbr -q <tested-bitrate> \
  -replay-storage ram -restart-replay-on-save no \
  -o /absolute/path/to/openfrag/clips
```

Each `-a` creates a separate audio track. Sources joined with `|` are mixed
into one track. The Setup Wizard should enumerate and persist explicit device
names when the player wants stable game, microphone, or voice-chat routing.
Application audio uses `app:name` and requires PipeWire. openfrag cannot assume
that CS2 game audio and voice chat are independently addressable until the
Compatibility Doctor sees separate sources on that host.
[gpu-screen-recorder manual, audio options](https://man.archlinux.org/man/extra/gpu-screen-recorder/gpu-screen-recorder.1.en)

The encoded replay buffer is held in RAM by default. `-replay-storage disk`
puts it on disk beside the output. `-bm cbr -q BITRATE` makes storage more
predictable in high-motion scenes than constant-quality mode. These choices
trade memory or disk pressure against quality and should be configuration
options in the Setup Wizard. [Upstream README, replay mode](https://git.dec05eba.com/gpu-screen-recorder/plain/README.md)

For an AMD RX 9070, the recorder must use the host's working VAAPI/FFmpeg
stack. AMD documents hardware H.264, HEVC, and AV1 encode support for this GPU,
but that does not prove the installed Mesa, firmware, VAAPI, or FFmpeg path is
usable. Probe with `gpu-screen-recorder --info` and a short test recording.
[AMD Radeon RX 9070 specifications](https://www.amd.com/en/products/graphics/desktops/radeon/9000-series/amd-radeon-rx-9070.html)

## Saving and stopping

There is no documented socket or command protocol in the upstream CLI. Remote
control is by POSIX signals:

| Action | Signal | Meaning |
| --- | --- | --- |
| Save replay | `SIGUSR1` | Save the current Replay Buffer; recording continues |
| Stop | `SIGINT` | Stop; saves a regular recording, but does not save replay mode |
| Pause/unpause | `SIGUSR2` | Only for regular recording, not replay/streaming |
| Start/stop regular recording alongside replay | `SIGRTMIN` | Requires `-ro DIR`; saves to that directory |

The upstream README also documents `SIGRTMIN+1` through `SIGRTMIN+6` for saving
the last 10 seconds, 30 seconds, 60 seconds, 5 minutes, 10 minutes, and 30
minutes respectively. Use a PID captured at launch rather than broad
`killall`, so another user's recorder cannot receive a Manual Flag.
[Upstream README, controlling remotely](https://git.dec05eba.com/gpu-screen-recorder/plain/README.md)

The saved Clip path is written to stdout; diagnostics normally go to stderr.
The current save path also writes some failure text to stdout, so the daemon
must treat each stdout line as an untrusted candidate. Accept it only if it
canonicalizes beneath the configured clip directory and names a new, regular,
nonempty file whose size is stable and whose media container can be probed.
Deduplicate the canonical path in SQLite.
The `-sc SCRIPT` option runs asynchronously after a save, so openfrag should not
use it as a second, racing completion channel.
[Upstream README, replay mode and save hook](https://git.dec05eba.com/gpu-screen-recorder/plain/README.md),
[upstream save implementation](https://git.dec05eba.com/gpu-screen-recorder/tree/src/main.cpp?id=174192d5605c51b41f39bb8c5de1abc1edc8a106)

Upstream helper scripts show the intended lifecycle: `start-replay.sh` starts
one process if none is running, `save-replay.sh` sends `SIGUSR1`, and
`stop-replay.sh` sends `SIGINT`. They are examples, not an IPC API.
[start-replay.sh](https://git.dec05eba.com/gpu-screen-recorder/plain/scripts/start-replay.sh),
[save-replay.sh](https://git.dec05eba.com/gpu-screen-recorder/plain/scripts/save-replay.sh),
[stop-replay.sh](https://git.dec05eba.com/gpu-screen-recorder/plain/scripts/stop-replay.sh)

## Monitoring and crash recovery

The daemon should retain the child PID and process generation, drain both
stdout and stderr, and track whether openfrag requested shutdown. Treat every
unrequested exit as failure, including status zero, because upstream does not
publish a stable exit-code taxonomy. On unexpected exit, mark recording
unavailable, preserve bounded stderr for Compatibility Doctor, and restart the
owned child after delays of 1, 2, 4, 8, 16, then 30 seconds with jitter. Reset
backoff only after five healthy minutes, and open a visible fault after a
bounded number of retries instead of looping forever on bad configuration. A
restart cannot recover frames that were only in the old process's Replay
Buffer; the next Clip can begin only after a fresh buffer has accumulated.

Signals are state flags, not a request queue, and the recorder permits one
asynchronous replay save at a time. openfrag should therefore allow one save in
flight, merge duplicate Highlight requests for that interval, and retain at
most one bounded follow-up save. The stdout path confirms completion, not which
request caused it. A delivered signal never by itself proves that a Clip was
saved.
[upstream signal and save implementation](https://git.dec05eba.com/gpu-screen-recorder/tree/src/main.cpp?id=174192d5605c51b41f39bb8c5de1abc1edc8a106)

On daemon shutdown, send `SIGTERM` to the owned child, wait up to 10 seconds,
then kill only that child's process group if necessary. On Linux, create the
child in its own process group and set `PR_SET_PDEATHSIG` to `SIGTERM` so a
daemon crash does not leave an orphan recorder. Persist no PID across daemon
restarts. Replay shutdown does not imply a final save.

There is no documented ready event. Model startup as `starting`, then `warming`
for at least the configured keyframe interval plus a margin, then `running`.
Saving adds a `saving` state; repeated failures enter `backoff` and then
`faulted`. A warming recorder may reject a save because it has no usable
keyframe yet.
[upstream early-save behavior](https://git.dec05eba.com/gpu-screen-recorder/tree/src/main.cpp?id=174192d5605c51b41f39bb8c5de1abc1edc8a106)

Disk replay storage is not crash recovery. Upstream creates per-process raw
spool chunks and has no documented loader that adopts them after restart. A
crash can leave stale chunks, so openfrag should treat the buffer as lost and
garbage-collect only stale spool directories it can prove it owns.
[upstream disk replay buffer](https://git.dec05eba.com/gpu-screen-recorder/tree/src/replay_buffer/replay_buffer_disk.c?id=174192d5605c51b41f39bb8c5de1abc1edc8a106)

The upstream project provides a user systemd service for startup and says its
environment file is `~/.config/gpu-screen-recorder/gpu-screen-recorder.env`.
openfrag may instead supervise its own child, but should follow the same
single-instance rule. [Upstream README, systemd startup](https://git.dec05eba.com/gpu-screen-recorder/plain/README.md)

## X11, Wayland, and compositor constraints

Upstream states that the recorder supports X11 and Wayland on AMD, Intel, and
NVIDIA. Capture target names and permissions still differ. On Wayland, portal
capture depends on an xdg-desktop-portal backend and PipeWire session; the
portal presents the user with selectable sources and returns a PipeWire stream.
[Upstream README](https://git.dec05eba.com/gpu-screen-recorder/plain/README.md),
[ScreenCast portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html),
[Portal PipeWire documentation](https://flatpak.github.io/xdg-desktop-portal/docs/pipewire.html)

Direct KMS capture has separate permission requirements and uses the installed
`gsr-kms-server`; do not assume it works for every desktop session. Portal
capture avoids that direct KMS permission path but depends on a working portal
backend and user-approved session. X11-only `window` and `focused` modes must
not be offered on Wayland. [gpu-screen-recorder manual](https://man.archlinux.org/man/extra/gpu-screen-recorder/gpu-screen-recorder.1.en), [gsr-kms-server
manual](https://man.archlinux.org/man/extra/gpu-screen-recorder/gsr-kms-server.1.en),
[Linux DRM UAPI](https://www.kernel.org/doc/html/v4.9/gpu/drm-uapi.html)

## GPL boundary

gpu-screen-recorder is GPL-3.0-only. openfrag invokes its installed executable
and exchanges only ordinary command-line arguments, POSIX signals, pipes, and
file paths. It does not copy, link, load, or use the recorder's plugin ABI. This
is the intended separate-program boundary. The GNU GPL FAQ says communication
mechanisms and semantic intimacy both matter when deciding whether separate
programs form one combined work, so subprocess separation is a strong design
fact but not a universal legal safe harbor. This document is engineering
guidance, not legal advice. Distributing the recorder alongside openfrag still
requires satisfying gpu-screen-recorder's GPL-3.0-only distribution duties.
[Upstream LICENSE](https://git.dec05eba.com/gpu-screen-recorder/plain/LICENSE),
[GNU GPL FAQ on aggregation and communication](https://www.gnu.org/licenses/gpl-faq.html#MereAggregation),
[GNU GPL FAQ on plug-in communication](https://www.gnu.org/licenses/gpl-faq.html#GPLPlugins),
[GPLv3 sections 2, 5, and 6](https://www.gnu.org/licenses/gpl-3.0.html)

## Unresolved questions

- Which exact capture target and audio device should the Setup Wizard persist on
  the user's machine?
- Which Wayland compositor and portal backend are in the supported test matrix?
- What bitrate, codec, frame rate, and Replay Buffer length pass the RX 9070
  quality and resource test on Noah's current driver stack?
- Can the host's PipeWire graph expose CS2 game audio, microphone, and voice chat
  as three independently selectable sources?
