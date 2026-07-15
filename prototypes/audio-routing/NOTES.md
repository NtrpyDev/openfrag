# Audio routing verdict

## Evidence

The safe host probe ran on CachyOS under KDE Wayland with PipeWire 1.6.7 and the Pulse compatibility server. A temporary null sink carried a 997 Hz tone into its monitor source, producing a 1.2 second stereo WAV with a measured peak of -21.07 dB. No physical microphone was selected or recorded.

The host currently exposes one output monitor and no non-monitor audio source. Its configured default input is that output monitor, so `default_input` must never be accepted as a microphone without checking that the resolved source is not a monitor.

gpu-screen-recorder is not installed. The signed 5.14.1 package from the configured CachyOS repository was unpacked under `/tmp` without installation. Its discovery commands reported the default output, the misleading default input, the output monitor, and the currently running Brave application.

Current upstream source at commit [`37d282774d6d06b230aefbf0f71f0d6c0fbf32c1`](https://git.dec05eba.com/gpu-screen-recorder/commit/?id=37d282774d6d06b230aefbf0f71f0d6c0fbf32c1) establishes the recorder contract used by this decision:

- each repeated `-a` argument becomes one audio track;
- inputs joined with `|` are mixed into one track;
- `app:<name>` matches a PipeWire application name case-insensitively;
- application capture requires PipeWire and build-time application-audio support;
- device capture uses Pulse-compatible source names;
- an application may start after the recorder, but its name must be re-resolved and validated when capture begins.

A full gpu-screen-recorder media capture was not run. The recorder is not installed, no microphone is exposed, and starting a GPU capture path would be unsafe while the host's separate graphics-stability incident is unresolved. The Setup Wizard test below is the required runtime proof on each supported host.

## Decision

Three independently playable tracks are conditional, not guaranteed:

1. `game` uses a live PipeWire application selected from gpu-screen-recorder's application list.
2. `voice_chat` requires a different PipeWire application stream, such as Discord.
3. `microphone` requires a non-monitor source selected from the device list.

In-game CS2 voice cannot be separated when it shares CS2's application stream. Application capture selects the whole application, not categories inside its mix. In that case openfrag records `mixed_game_voice` plus `microphone` when available. With no valid microphone it records only `mixed_game_voice`. Native PulseAudio supports the mixed device path but not gpu-screen-recorder application capture, so openfrag does not claim three-track separation there.

The recorder argv for a valid three-track PipeWire setup has this shape:

```text
-a name:game|app:<selected-cs2-name>
-a name:voice_chat|app:<selected-voice-name>
-a name:microphone|device:<selected-non-monitor-source>
```

Numeric Pulse and PipeWire indices are never persisted. Device roles persist `node.name` with available `device.serial`, `device.bus_path`, and `device.name` evidence. Application roles persist the user's selected application name and re-resolve it against gpu-screen-recorder's live list at every recorder start. Missing or ambiguous matches degrade explicitly instead of silently switching sources.

## Setup Wizard and Compatibility Doctor

The Setup Wizard must:

1. require PipeWire for application-separated tracks;
2. ask the player to start CS2 and any separate voice application, then select each live application by role;
3. list microphone choices only from non-monitor sources;
4. explain that CS2 in-game voice remains in the game track;
5. offer `mixed_game_voice` plus optional microphone when separation is unavailable;
6. make a short consented test Clip and use FFprobe to require the expected audio-stream count, track titles, nonzero duration, and independent track selection before saving the configuration.

Compatibility Doctor repeats discovery at each recorder start. If a persisted device identity or application name is missing or ambiguous, it reports the affected role and applies the saved fallback. It never substitutes `default_input`, an output monitor, or an unrelated application silently.
