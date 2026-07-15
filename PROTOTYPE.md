# Issue 23 portal probe

## Question

Can the current KDE Plasma Wayland session bind one user-approved XDG Global Shortcuts accelerator for Manual Flag, emit exactly one activation per press and hold while CS2 owns fullscreen focus, and preserve the binding across daemon restart and desktop lock/unlock?

This is a throwaway prototype. It does not save a Clip or read input devices.

## Run

The inspection check registers the temporary host application identity and reports the portal version:

```sh
cargo run --quiet -- inspect
```

The command creates a hidden `app.openfrag.PortalProbe.desktop` entry under the user's XDG applications directory so the portal can validate the temporary identity. It refuses to overwrite an existing file with that name.

The live check creates a session, restores an existing binding or asks KDE to bind `manual_flag`, observes press and release signals, and closes the session after the requested number of cycles:

```sh
cargo run --quiet -- live 3
```

The live command may show a KDE approval dialog. Its preferred trigger is `CTRL+SHIFT+F10`, but the returned portal binding is authoritative. The command prints activation, deactivation, duplicate, and stray-release counts. It never installs an input hook or requests access to `/dev/input`.

Remove the prototype's KDE binding and temporary desktop entry after testing:

```sh
cargo run --quiet -- cleanup
```

## Environment

- KDE Plasma Wayland 6.7.2
- xdg-desktop-portal 1.22.1
- xdg-desktop-portal-kde 6.7.2
- `org.freedesktop.portal.GlobalShortcuts` version 2
- CS2 in fullscreen on the tested KDE session

The implementation follows the official [Global Shortcuts portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.GlobalShortcuts.html), [Shortcuts specification](https://specifications.freedesktop.org/shortcuts/latest), and [host Registry](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.host.portal.Registry.html) contracts.

## Observations

KDE displayed the initial binding dialog and accepted `Ctrl+Shift+F10`. Three consecutive press-and-hold cycles emitted exactly three `Activated` and three `Deactivated` signals, with no duplicate activations or stray deactivations. The same exactly-once result held:

- after stopping and restarting the probe;
- after locking and unlocking the desktop;
- while CS2 owned fullscreen focus.

Every run closed its portal session cleanly. Closing the session removed the live registration but intentionally retained KDE's user-approved shortcut assignment for later sessions. The portal has no portable delete-binding method.

The first prototype runs inherited Konsole's application identity because the process was launched from Konsole. That proved activation behavior but was not valid persistence evidence for openfrag. The prototype was corrected to call `org.freedesktop.host.portal.Registry.Register` as `app.openfrag.PortalProbe` on the same D-Bus connection before any portal method. A matching temporary desktop file was present. KDE then stored the binding under that identity, and a fresh process restored it without another dialog and emitted exactly one activation and deactivation. The Konsole and probe test bindings were removed after verification.

KDE reported timestamp `0` on the observed activation and deactivation signals. openfrag must not use the optional portal timestamp for ordering, deduplication, or Clip timing.

## Decision

Go for the XDG Global Shortcuts portal as the primary Manual Flag mechanism on the tested KDE Plasma Wayland stack.

The daemon saves on `Activated` and treats `Deactivated` only as lifecycle evidence. It must establish a stable application identity before its first portal call, subscribe before binding, accept the portal's returned shortcut subset and trigger description, and recreate a session after daemon or portal loss. A restored binding does not require another dialog. Lock/unlock and fullscreen focus do not require rebinding on the tested stack.

The packaged application must ship a desktop file whose basename matches its stable application ID. A host build should register that ID through the host Registry when available before any other portal call. Because the official Registry is expected to be deprecated eventually, packaged launchers and user services must also use the standard application cgroup identity, and openfrag must handle a missing Registry interface.

The Setup Wizard presents the portal dialog and reports cancellation, an empty returned shortcut set, an unavailable interface, or a conflicting accelerator as an explicit Manual Flag failure. It never silently substitutes another key. Compatibility Doctor verifies interface version, app identity, restored binding, and one consented activation. Unsupported desktops proceed to the separate device-scoped fallback investigation.

Support claims remain scoped to KDE Plasma Wayland 6.7.2 with portal frontend 1.22.1 and KDE backend 6.7.2 until the support-matrix ticket validates other versions and desktops.
