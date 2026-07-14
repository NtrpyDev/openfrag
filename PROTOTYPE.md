# Issue 23 portal probe

Question: does this KDE session expose the XDG Global Shortcuts portal needed
for a Manual Flag activation path?

Run the non-invasive probe with:

```sh
cargo run --quiet
```

The probe calls only D-Bus `Introspect` on
`org.freedesktop.portal.Desktop/org/freedesktop/portal/desktop`. It does not
create a session, configure or bind a shortcut, list shortcuts, register an
accelerator, or subscribe to activation as a live client.

Observed on Noah's KDE Plasma Wayland session: `xdg-desktop-portal` and
`plasma-xdg-desktop-portal-kde` were active; introspection reported
`org.freedesktop.portal.GlobalShortcuts` version 2 with the expected methods and
signals. The command printed `global_shortcuts_interface=true`.

Not run by design: live registration and activation, portal restart, lock or
unlock, and fullscreen-game tests. Those operations can show a portal dialog,
change desktop state, steal focus, or require explicit human interaction.
