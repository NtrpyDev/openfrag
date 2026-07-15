# Audio routing spike (throwaway)

This is a host probe for issue #18, not openfrag production architecture. It
answers one narrow question: does the current PipeWire/Pulse-compatible host
expose distinct, persistently identifiable streams for game, microphone, and
voice chat, and can a monitor source be captured without touching the real
microphone?

Run one command:

```bash
./prototypes/audio-routing/audio-routing-probe.sh
```

The command writes a timestamped directory under `/tmp/openfrag-audio-routing-*`
and prints its path. `report.json` contains the complete Pulse-compatible state,
PipeWire dump, candidate classification, persistence identifiers, and
gpu-screen-recorder capability probe. It creates a temporary null sink, plays a
997 Hz synthetic tone into it, records only that null sink's monitor to
`synthetic-monitor.wav`, verifies the WAV with FFmpeg, and removes the null sink
on exit. It does not read or record a physical microphone and opens no UI.

Interpretation is deliberately conservative:

- A three-track setup requires three distinct inputs: a CS2 sink-input, a
  voice-chat sink-input, and a non-monitor microphone source. The report names
  candidates but never assumes one exists from a device label alone. Repeated
  `gpu-screen-recorder -a` arguments create separate tracks; sources joined
  with `|` are mixed into one track.
- Persist a Pulse/PipeWire `node.name` plus stable device identity properties
  (`device.serial`, `device.bus_path`, `device.name`) when available. Persist
  an application role by its selected gpu-screen-recorder application name and
  re-resolve it at each start. Never persist a numeric object index/serial as
  the primary identity.
- If CS2 and voice chat both arrive only at the same sink monitor, capture one
  explicit `mixed_game_voice` track (and a microphone track only when a
  non-monitor source exists). Three independently playable tracks are then
  unavailable. The human must choose mixed capture or configure a separate
  virtual sink/routing rule outside this spike.
- In-game CS2 voice that shares CS2's application stream cannot be separated
  by application capture. A separate voice application such as Discord can be
  assigned its own track on PipeWire.
- If `gpu-screen-recorder` is absent, the recording integration is unavailable.
  The synthetic capture proves only PipeWire/Pulse routing and source
  selection, not gpu-screen-recorder media muxing.

Do not commit generated reports or WAV files.
