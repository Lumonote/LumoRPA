# Desktop recording

Desktop capture produces replayable `desktop.click`, `desktop.drag`,
`desktop.scroll`, `desktop.key` and `desktop.type` steps. Review the generated
patch before running it: coordinates are absolute screen coordinates, so the
same display arrangement, scaling and application state are required. The
recorder does not silently activate or reposition an application during replay.

The recorder never captures printable raw keystrokes. Text comes from controls
positively identified as editable and non-password. Password fields and custom
controls without readable accessibility text are omitted; add a vault-backed
input step manually. Text edits become select-all plus replacement so Unicode,
IME composition, selected-range edits and deletion preserve the final value.
Recording data can still contain sensitive non-password text: review the patch
before saving or sharing it.

- **macOS:** enable Input Monitoring and Accessibility for the application in
  System Settings → Privacy & Security. A listen-only Quartz event tap captures
  pointer/shortcut activity; Accessibility supplies supported text controls.
- **Windows:** run in an interactive desktop session. Win32 input hooks capture
  pointer/shortcut activity. Standard Edit/RichEdit controls are supported for
  text, excluding `ES_PASSWORD`. Custom application text controls may be omitted.
- **Linux:** use an X11 session with desktop accessibility enabled. Install the
  distribution packages `python3`, `python3-gi`, `gir1.2-atspi-2.0`,
  `at-spi2-core`, `xinput` and `coreutils`. The fixed embedded helper consumes
  XInput2 events and AT-SPI editable-text events. It opens the existing display
  and accessibility bus; it does not ask for root or capture `/dev/input`.
  Wayland is explicitly rejected because compositor-authorized global capture
  is not available through this backend. A missing bus/package fails startup.

Each recording owns its event source. Stop/drop unhooks the native listener,
stops the worker or helper process group, and waits for it before returning.
Mixed recording correlates browser DOM gestures with desktop events and prefers
selector-based browser steps when the two lanes observed the same gesture.

## Verification

`cargo test -p lumo-recorder --lib` covers replay conversion, text coalescing,
double-click preservation, mixed-lane correlation, password exclusion and
recorder start/stop behavior without monitoring the developer's desktop.

Linux native capture has a runnable isolated-desktop acceptance entry point:

```sh
cargo build -p lumo-recorder --example desktop_capture
dbus-run-session -- xvfb-run -a /usr/bin/python3 scripts/linux-recorder-e2e.py \
  --capture-binary "$PWD/target/debug/examples/desktop_capture"
```

The fixture additionally needs `xdotool`, Gtk 3 GI bindings (`gir1.2-gtk-3.0`),
`libatk-adaptor`, `xvfb` and `dbus-x11`. It injects test input **only after checking
that DISPLAY belongs to Xvfb**, then verifies real native click/scroll events,
AT-SPI text, password omission and capture stop. The `Linux native recorder
acceptance` CI workflow runs it on Ubuntu; the macOS development host cannot
establish its Linux execution result. No fixture is run on the user's desktop.

On each target platform, perform this acceptance check in a disposable document:
start Desktop recording, click a text field, type Unicode text, edit/delete a
selection, drag/select, scroll and press a shortcut. Stop, review the patch and
replay with the same window/display layout. Verify the password field's value
and key presses never appear. Repeat start/stop and exit the application while
recording; verify no listener/helper process remains. On Linux also check that a
Wayland or missing-AT-SPI session fails with the actionable startup error.
Real desktop acceptance needs an interactive session and OS permissions; unit
tests do not establish native platform input accuracy.
