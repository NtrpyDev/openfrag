# Device-scoped Manual Flag broker spike (throwaway)

This prototype asks whether a separate evdev broker can open exactly one user-selected stable device identity, accept only one allowlisted key code, and expose only a rate-limited one-bit Manual Flag message to the openfrag daemon. It specifically probes ambiguity, identity changes, hotplug loss, suspend/resume, `SYN_DROPPED`, permission failure, and local IPC peer authorization.

This is throwaway code on a prototype branch. It is not production code and must not be merged into the application.

## State-machine simulator

Run the complete in-memory state model with one command:

```console
cargo run --release --manifest-path prototypes/hotkey-evdev/Cargo.toml -- simulate
```

The simulator renders the full broker state after every command. It can select one device, create an ambiguous match, replace identity metadata, disconnect and reconnect, deny permission, connect or reject an IPC client, drive press/repeat/release and rate-limit cases, recover from `SYN_DROPPED` with the key held or released, and suspend/resume.

## Live broker and client

The live broker selects a keyboard by exact udev `ID_SERIAL` plus USB interface, rejects zero or multiple matches, validates the evdev name and fingerprint on every reopen, reads only the allowlisted code, and never grabs or writes the device. It authenticates a local Unix client with `SO_PEERCRED`. The only protocol message is the literal line `FLAG`; no raw file descriptor, event structure, code, timestamp, or device metadata crosses the socket.

The tested Wooting command is:

```console
cargo run --release --manifest-path prototypes/hotkey-evdev/Cargo.toml -- \
  broker Wooting_Wooting_Two_HE__ARM__A02B2442W043H25541 01 66 \
  /tmp/openfrag-evdev-broker.sock 1000 'Wooting Wooting Two HE (ARM)'
```

In a second terminal, connect the daemon-side probe:

```console
cargo run --release --manifest-path prototypes/hotkey-evdev/Cargo.toml -- \
  client /tmp/openfrag-evdev-broker.sock
```

`SIGUSR1` and `SIGUSR2` stand in for logind `PrepareForSleep(true)` and `PrepareForSleep(false)` during this spike. Resume always starts disarmed and requires udev identity plus evdev metadata revalidation.

## Observed evidence

Host: Noah's CachyOS KDE Plasma Wayland PC, Wooting Two HE keyboard interface `01`, event node `/dev/input/event4` at the time of the run.

- The model rejected two matching candidates as ambiguous.
- The model rejected a changed name or evdev fingerprint after reconnect.
- Device disappearance disarmed the broker; the same fingerprint could re-arm on a different event path.
- Suspend closed the reader. Resume returned to an awaiting state and re-armed only after the same identity was rediscovered.
- `SYN_DROPPED` emitted no flag. Recovery with the key held required a release before a later press.
- Key repeat and duplicate-down events emitted nothing. A 750 ms minimum interval suppressed rapid represses.
- A Unix peer with UID 1000 was accepted; the same client was rejected when the configured UID was 65534.
- `cargo check`, release build, and clippy with warnings denied passed.
- The earlier physical evdev spike already established exactly-once F8 delivery, three-second hold suppression, unplug/replug recovery, and fullscreen CS2 delivery on this device. This spike does not reinterpret those observations as a permission proof.

For process isolation, the live broker was launched with Bubblewrap using a fresh `/dev` that contained only `/dev/input/event4`. Inside the namespace it ran as UID 1000 without the host `input` supplementary group and could not name any other input event node. The broker armed, authenticated the client, disarmed on the suspend signal, returned to awaiting-device on resume, and then revalidated and re-armed the same fingerprint.

## Security verdict

No-go for an evdev fallback in openfrag v1.

The broker process and one-bit IPC boundary can be constrained, but the required narrow host device permission was not demonstrated. On this PC `/dev/input/event4` remains `root:input` mode `0660`, and the launcher can bind it only because Noah belongs to the broad `input` group. Bubblewrap hides every other node after launch, but it does not replace that broad source authorization with a device-specific ACL. Granting a separate service identity one udev-selected node would require privileged account, group, udev-rule, hotplug, and packaging machinery that this spike did not validate.

The ticket's acceptance rule says to reject the fallback when the complete boundary cannot be demonstrated. Therefore:

- XDG Global Shortcuts remains the only v1 global Manual Flag accelerator.
- openfrag must not request `input` group membership, install an evdev broker, open raw input devices, or receive raw input file descriptors or events.
- Unsupported desktops must report the global Manual Flag accelerator as unavailable instead of silently widening privileges.
- A future evdev proposal requires a new security review and real proof of a device-specific host grant; this prototype is not reusable authorization evidence.
