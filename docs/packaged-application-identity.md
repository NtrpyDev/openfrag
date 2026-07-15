# Packaged application identity for portal-backed Manual Flag

## Question

Which stable application ID, desktop file, launcher, and systemd user unit must every openfrag v1 package ship so XDG Desktop Portal attributes Global Shortcuts calls to openfrag instead of a terminal? How must the daemon use the host Registry while it exists, survive its eventual removal, and prove its identity before accepting a restored Manual Flag binding?

## Decision

The production identity is immutable across the static release, AUR, and COPR packages:

| Role | Canonical value |
| --- | --- |
| Application ID | `io.github.ntrpydev.openfrag` |
| Desktop file basename | `io.github.ntrpydev.openfrag.desktop` |
| Desktop launcher | `openfrag-launch` |
| Daemon binary | `openfragd` |
| Canonical user service | `app-io.github.ntrpydev.openfrag.service` |
| Service slice | `app.slice` |
| Global Shortcut ID | `manual_flag` |
| Icon basename | `io.github.ntrpydev.openfrag` |

`io.github.ntrpydev.openfrag` follows the Desktop Entry Specification's reverse-DNS naming rule and uses a namespace controlled by the repository owner. GitHub defines an account's default Pages namespace as `<owner>.github.io` and requires uppercase owner letters to be lowercased in the Pages repository name. The ID therefore uses `io.github.ntrpydev`, not the display spelling `NtrpyDev`. Acquiring or publishing `openfrag.app` later must not change the application ID. [Desktop Entry file naming](https://specifications.freedesktop.org/desktop-entry/latest/file-naming.html), [GitHub Pages namespace](https://docs.github.com/en/pages/getting-started-with-github-pages/creating-a-github-pages-site)

The prototype-only `app.openfrag.PortalProbe` ID must never reach a package. The current implementation branch's `openfrag.desktop` and `openfragd.service` names are pre-release assets that must be replaced, not retained as alternate identities. [Current desktop asset](https://github.com/NtrpyDev/openfrag/blob/9654e39483cee5cd2635133ecffd57f4c5bdb15f/packaging/linux/openfrag.desktop), [current service asset](https://github.com/NtrpyDev/openfrag/blob/9654e39483cee5cd2635133ecffd57f4c5bdb15f/packaging/linux/openfragd.service)

## Why these names bind together

The Desktop Entry Specification identifies an application by the desktop file ID derived from its filename under an `applications` directory in the XDG data search path. It recommends a valid D-Bus well-known name using a controlled reverse-DNS prefix. Therefore the file basename before `.desktop` is the application ID exactly. [Desktop Entry file naming](https://specifications.freedesktop.org/desktop-entry/latest/file-naming.html)

The host Registry accepts an application ID only for an unsandboxed peer, associates the calling D-Bus connection with that ID, and requires the ID to match the basename of a desktop file describing the application. Registration is allowed once per connection and must happen before every other portal call. Applications should re-register when the portal service reappears after a restart. [Host Registry version 1](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.host.portal.Registry.html)

Registry registration overrides automatic host identity detection. The portal project explicitly warns that this interface is expected to be deprecated or removed and directs host applications to use the XDG application cgroup naming convention as the durable path. [Portal API reference](https://flatpak.github.io/xdg-desktop-portal/docs/api-reference.html)

The systemd desktop convention recognizes `app[-<launcher>]-<ApplicationID>[@<RANDOM>].service` and `app[-<launcher>]-<ApplicationID>-<RANDOM>.scope`. Persistent single-instance applications may omit the random suffix, and services are preferred over scopes. `app-io.github.ntrpydev.openfrag.service` is therefore the canonical no-launcher, no-random service name. [systemd desktop environment integration](https://systemd.io/DESKTOP_ENVIRONMENTS/)

xdg-desktop-portal 1.22.1 calls `sd_pid_get_user_unit`, accepts only an `app-` unit, parses the application ID from the unit name, then resolves `<ApplicationID>.desktop`. Its tests cover service names such as `app-org.kde.amarok.service`. This makes the unit's canonical ID, not merely a friendly alias, part of the compatibility contract. [Host app identity implementation](https://github.com/flatpak/xdg-desktop-portal/blob/1.22.1/src/xdp-app-info-host.c), [unit-name parsing tests](https://github.com/flatpak/xdg-desktop-portal/blob/1.22.1/tests/test-xdp-utils.c), [`sd_pid_get_user_unit`](https://www.freedesktop.org/software/systemd/man/latest/sd_pid_get_owner_uid.html)

## Package layout

All package formats ship byte-equivalent identity fields. A package channel, architecture, distribution, build type, or executable path must never be appended to the application ID.

### AUR and COPR

- Install `io.github.ntrpydev.openfrag.desktop` in `/usr/share/applications/`.
- Install `app-io.github.ntrpydev.openfrag.service` in `/usr/lib/systemd/user/`.
- Install `openfrag-launch` and `openfragd` on the normal executable search path.
- Install icons under the basename `io.github.ntrpydev.openfrag`.

### Static per-user release

- Install the same desktop basename in `$XDG_DATA_HOME/applications/`, defaulting to `~/.local/share/applications/`.
- Install the same service basename in `$XDG_CONFIG_HOME/systemd/user/`, defaulting to `~/.config/systemd/user/`.
- Install `openfrag-launch` and `openfragd` at package-selected absolute paths and render those paths into the desktop and service assets without changing any identity value.

The documented systemd user-unit search path includes both the XDG per-user locations and `/usr/lib/systemd/user`. [systemd user unit search path](https://www.freedesktop.org/software/systemd/man/latest/systemd.unit.html)

The desktop entry contains at least:

```ini
[Desktop Entry]
Type=Application
Version=1.0
Name=openfrag
Comment=Open the local openfrag dashboard
Exec=openfrag-launch
TryExec=openfrag-launch
Icon=io.github.ntrpydev.openfrag
Terminal=false
Categories=Utility;
StartupNotify=true
```

`openfrag-launch` performs only product launch behavior: start `app-io.github.ntrpydev.openfrag.service`, wait for the loopback health endpoint, then open the local dashboard. It does not own a portal connection. Direct terminal use of `openfrag-launch` therefore cannot move Global Shortcuts ownership to the terminal.

The service file's canonical filename is `app-io.github.ntrpydev.openfrag.service`, it declares `Slice=app.slice`, and it starts `openfragd`. Do not ship `openfragd.service` as an alias: current portal implementations query the process's user unit ID, so starting through an alias can make attribution implementation-dependent. The service may retain the existing hardening directives. Lifecycle dependencies and enablement policy belong to the Setup Wizard decision, but they must not rename the canonical unit.

## Portal identity state machine

The daemon owns one dedicated session-bus connection for portal work. The Registry call and every Global Shortcuts proxy must use that same connection.

Identity has three states:

1. `Unverified`: the initial state and the state after the portal bus name loses its owner. No restored shortcut or activation is accepted.
2. `ExplicitlyRegistered`: `Register("io.github.ntrpydev.openfrag", {})` succeeded on a fresh connection before any other portal method.
3. `CgroupInferred`: the Registry interface or method is specifically absent, and local preflight proved both the canonical systemd unit ID and the resolved canonical desktop file.

Only `ExplicitlyRegistered` and `CgroupInferred` may create a Global Shortcuts session.

### Startup algorithm

1. Resolve the highest-precedence `io.github.ntrpydev.openfrag.desktop` in the XDG data search path. Require it to be a regular file with the canonical desktop ID, launcher, and icon. A stale or shadowing file is an identity failure.
2. Determine the current process's systemd user unit using behavior equivalent to `sd_pid_get_user_unit(0)` and record whether it is the canonical unit ID `app-io.github.ntrpydev.openfrag.service` in `app.slice`. Merely finding the expected name among aliases is insufficient for current portal parity. A noncanonical unit is allowed only when explicit Registry registration later succeeds.
3. Open a fresh session-bus connection and watch the owner of `org.freedesktop.portal.Desktop`.
4. Before constructing or calling any other portal proxy, call `Register` directly on `org.freedesktop.host.portal.Registry` at `/org/freedesktop/portal/desktop`, with the canonical application ID and an empty options map. Do not introspect the object or read the Registry version first because registration must precede every other portal method.
5. Success enters `ExplicitlyRegistered`; this is the official override for a valid desktop application launched outside the standard cgroup. Registry properties may be read for diagnostics only after registration succeeds.
6. If the exact failure is `UnknownInterface` or `UnknownMethod`, treat Registry as unavailable or deprecated. Enter `CgroupInferred` only if the desktop entry is valid and step 2 found the exact canonical unit and slice.
7. Any other Registry failure, including invalid ID, missing desktop application info, already registered, registered too late, a transport error, or identification as sandboxed, leaves identity `Unverified`. Discard the connection and do not silently downgrade to cgroup inference on that connection.
8. If the portal name owner disappears, close the shortcut session, discard pending responses and activations, and return to `Unverified`. When a new owner appears, repeat the entire algorithm on the connection before recreating the session.

The current KDE host exposes Registry version 1. The successful portal prototype showed that terminal-launched calls were initially attributed to Konsole, while explicit registration with a matching desktop file persisted and restored the shortcut under the probe identity. [Verified Global Shortcuts prototype](https://github.com/NtrpyDev/openfrag/blob/7d24307ec49c1007a905ef2a0fe913c3cf85340b/PROTOTYPE.md)

## Restored binding gate

The portal specification binds every Global Shortcuts session to the application that created it. Before `BindShortcuts` is called in a new session, `ListShortcuts` returns shortcuts successfully bound in a previous session by that application. [Global Shortcuts portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.GlobalShortcuts.html)

openfrag may treat a binding as restored only when all of these are true:

- Identity is `ExplicitlyRegistered` or `CgroupInferred`.
- The current portal owner created the current session after identity verification.
- `ListShortcuts` returns exactly one entry with ID `manual_flag` for that ID. Unknown IDs are ignored and surfaced diagnostically; a duplicate `manual_flag` is an error.
- The portal-returned `trigger_description` is non-empty and is treated as authoritative display text.
- Activations are accepted only for the current session handle and exact `manual_flag` ID.

If no valid restored binding exists, the Setup Wizard performs the one allowed `BindShortcuts` attempt and accepts the returned subset, including an empty subset. It never copies a binding from another identity and never invents a trigger description. The portal exposes no standard method that returns the caller's effective app ID, so this preflight and state machine are the identity proof; restored state alone is not.

## Failure behavior

| Condition | Required behavior |
| --- | --- |
| Canonical desktop entry missing, shadowed, or malformed | Disable portal Manual Flag and report `desktop_identity_invalid` |
| Daemon outside canonical application unit, Registry succeeds | Continue as `ExplicitlyRegistered`, but diagnose noncanonical package launch |
| Daemon outside canonical application unit, Registry unavailable | Disable portal Manual Flag and report `application_cgroup_unverified` |
| Registry absent, canonical desktop and cgroup valid | Continue as `CgroupInferred` |
| Registry present and registration succeeds | Continue as `ExplicitlyRegistered` |
| Registry present but registration otherwise fails | Discard connection; report `portal_registration_failed` |
| Portal restarts | Invalidate session and identity; re-register or re-infer before use |
| Restored list has no `manual_flag` | Ask for consent through the normal bind flow |
| Restored list has duplicate `manual_flag` | Reject restored state and report `restored_binding_invalid` |
| Global Shortcuts unavailable | Report Manual Flag unavailable; no evdev fallback |

No environment variable, parent process, executable basename, D-Bus unique name, terminal identity, or desktop-file presence by itself is sufficient identity evidence.

## Migration rule

No public release has established the pre-release `openfrag.desktop` or `openfragd.service` identities. The v1 installer replaces them before the first production portal call:

1. Stop and disable `openfragd.service` if present.
2. Remove only package-owned legacy desktop and service files after verifying their expected contents.
3. Install the canonical desktop file before starting the canonical service.
4. Reload the user manager, enable or start only the canonical service according to the Setup Wizard decision, and run identity preflight.
5. Do not migrate prototype bindings from `app.openfrag.PortalProbe`, Konsole, or any other test identity. The user consents once under the production ID.

## Validation and release gates

Every static, AUR, and COPR artifact must pass the same identity verifier:

1. `desktop-file-validate` accepts `io.github.ntrpydev.openfrag.desktop`.
2. The resolved desktop file ID and `Icon` equal the canonical values, `Exec` and `TryExec` resolve to the package-owned `openfrag-launch`, and no package contains `openfrag.desktop` or `app.openfrag.PortalProbe`.
3. `systemd-analyze --user verify app-io.github.ntrpydev.openfrag.service` succeeds, the file declares `Slice=app.slice`, and its canonical name is not an alias.
4. A staged install lands the desktop and unit in valid XDG and systemd search paths, with installation completed before service activation.
5. A running daemon reports the exact unit ID, slice, control group, resolved desktop path and hash, Registry availability/version, identity state, portal owner unique name, shortcut session generation, and restored-binding disposition. These diagnostics contain no input events or remote endpoints.
6. A negative test launched from a terminal with Registry unavailable remains `Unverified` and refuses restored bindings.
7. A fake-portal integration test covers Registry success, exact absence fallback, every non-fallback registration error, portal owner loss/reappearance, stale response rejection, and restored `manual_flag` validation.
8. On each claimed desktop stack, an installed-package smoke test binds once under the production ID, restarts the daemon, restores without another consent dialog, emits exactly one activation for one press-and-hold, and survives portal restart or records the precise unsupported behavior.
9. Artifact comparison proves the static, AUR, and COPR packages use byte-identical identity strings despite different executable paths.

The existing KDE Plasma Wayland result remains the only positive Global Shortcuts support evidence until the Support Matrix ticket expands it. Packaging conformance alone does not justify a claim for another desktop or portal backend.

## Implementation consequences

- Replace the implementation branch's desktop and service filenames before v1 packaging is considered complete.
- Put the application ID and related artifact names in one first-party Rust or generated packaging definition so runtime and verifiers cannot drift.
- Keep the portal bus connection inside `openfragd`; the dashboard launcher and browser never register an identity.
- Make identity failure visible in the Setup Wizard and Compatibility Doctor. Never repair it by opening raw input devices or widening privileges.
- Treat the application ID as durable user-data identity because portal permissions and restored Global Shortcuts state are keyed by it.
