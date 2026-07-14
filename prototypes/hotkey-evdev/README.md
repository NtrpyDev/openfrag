# Hotkey evdev spike (throwaway)

This prototype answers one question: can openfrag detect a chosen keyboard key or mouse button from `/dev/input` while a fullscreen Wayland game owns the normal application input, with a workable permission and hotplug story?

This is throwaway code. It is not the production hotkey daemon, does not persist configuration, and should be deleted or absorbed after the go/no-go decision.

## Run

From the repository root, run:

```sh
cargo run --release --manifest-path prototypes/hotkey-evdev/Cargo.toml
```

The default binding is evdev code `66` (`F8`). Set `HOTKEY_CODE` to another numeric keyboard or mouse-button code, and optionally set `HOTKEY_DEVICE` to a case-sensitive substring of the device name or event path. For example:

```sh
HOTKEY_DEVICE=Wooting HOTKEY_CODE=66 cargo run --release --manifest-path prototypes/hotkey-evdev/Cargo.toml
```

Without a device filter, more than one matching event node is shown as `Conflict` and the prototype deliberately disarms itself. There is no interactive device chooser. The display reports matching devices, flags, and state; it does not print every release event. The `r` rescan and `q` quit commands are read from stdin and require pressing Enter after each command. If the command fails because the manifest is absent, the spike has not been built in this checkout yet.

## Interpreting the interaction

- `Reading`: exactly one matching device is open and the chosen key or button is
  armed.
- `Conflict`: multiple matching event nodes were found, so the prototype is
  deliberately disarmed until the filter selects one.
- `PermissionDenied`: the process could not open an event node. The operating
  system error is displayed.
- `Disconnected`: a read failed after opening. The periodic rescan looks for the
  device again.
- `MANUAL FLAG`: one up-to-down transition was accepted. Releases and kernel
  auto-repeat events do not increment the counter.

The spike makes keyboard and mouse selection explicit through its filter. A
production implementation would need a stable device fingerprint rather than
persisting the unstable `eventN` path.

## Safety finding

This PC can run the spike because Noah already belongs to the `input` group.
That membership permits reading every group-readable keyboard and mouse, not
only the configured hotkey, so it is not an acceptable production permission
story. The production design should try the XDG Global Shortcuts portal first.
The current KDE Wayland session exposes portal interface version 2. An evdev
fallback would need a separately reviewed, device-scoped broker that sends only
a rate-limited Manual Flag event to openfrag. The main daemon must never receive
a raw input file descriptor or raw key stream.

## Hands-on verification checklist

Run this checklist on the local KDE Wayland session with a fullscreen game or another input-grabbing client.

1. Start the command from a terminal and record session type, selected device, event node, and whether the process is in the `input` group.
2. Set `HOTKEY_DEVICE` to the intended keyboard substring and `HOTKEY_CODE` to a low-conflict key. Press it once, release it, and confirm exactly one `MANUAL FLAG` notice.
3. Hold that key for at least three seconds. Confirm kernel key-repeat does not create repeated Manual Flag candidates.
4. Set `HOTKEY_DEVICE` to the intended mouse substring and `HOTKEY_CODE` to a side-button code if available. Confirm one `MANUAL FLAG`, then confirm ordinary movement and left-click do not trigger the selected code.
5. Launch the game in fullscreen Wayland mode so it grabs normal application input. Repeat steps 2 to 4 while focused on the game. Record whether evdev still receives events.
6. While the prototype is armed, unplug the selected keyboard or mouse, then reconnect it. Confirm a disappearance/reconnect message, no crash, and return to `ARMED` without requiring a restart.
7. If multiple keyboards or mice are present, select each one and verify the chosen physical device only. Check that an identical key on another device does not trigger the flag.
8. Run once as the normal user and once without membership in the `input` group (or with a deliberately unreadable test node). Confirm the failure is an explicit `PERMISSION_DENIED` state naming the node, not a silent empty device list.
9. While the game is focused, test the candidate key against its in-game binding. Record whether the key conflict is acceptable, remappable, or disqualifying.
10. Stop with `q` and confirm all device handles are released so a second run can select the same device.

Linux evdev exposes input events through character devices such as `/dev/input/eventX`; access is governed by device-node permissions and the session's device policy ([kernel evdev documentation](https://docs.kernel.org/input/), [systemd-logind device access](https://www.freedesktop.org/software/systemd/man/latest/loginctl.html)). These links describe the permission and device model. They do not guarantee that a compositor or game will provide an application-level global shortcut.

## Feedback required before go/no-go

Noah must report the exact device and key/button chosen, whether the process was in the `input` group, fullscreen Wayland result, repeat-key result, hotplug result, keyboard-versus-mouse result, and the permission-denied result. Include the prototype output and any relevant `journalctl` or `ls -l /dev/input/event*` evidence. The decision is **go** only if the chosen control is reliable under the focused fullscreen game, survives hotplug, avoids unacceptable in-game conflicts, and fails loudly when permission is missing. Otherwise record the failing case and choose a different mechanism or input control before production work.
