"""X11/AT-SPI recorder helper; fixed source embedded in the Rust binary.

Keyboard literals are never observed. Editable text is read only from focused,
non-password AT-SPI controls after a text change. No shell interpolation is used.
"""
import ctypes
import ctypes.util
import json
import os
import re
import shutil
import subprocess
import sys
import threading

_output = threading.Lock()


def emit(kind, **payload):
    with _output:
        print(json.dumps({"kind": kind, "payload": payload}, ensure_ascii=False), flush=True)


def make_accessibility_handler(Atspi, focus, emit):
    def accessibility(event, *_user_data):
        try:
            focus_changed = event.type.startswith("object:state-changed:focused")
            if focus_changed:
                # Focus-loss events precede focus gain in another application.
                # Never retain the old field's permission during that gap.
                focus["safe"] = False
            source = event.source
            states = source.get_state_set()
            if not states.contains(Atspi.StateType.FOCUSED):
                return
            role = source.get_role_name() or ""
            name = source.get_name() or ""
            sensitive = (source.get_role() == Atspi.Role.PASSWORD_TEXT
                         or any(word in (role + " " + name).lower()
                                for word in ("password", "passwd", "protected", "secure", "密码", "口令")))
            if sensitive:
                focus["safe"] = False
            application = source.get_application()
            app = application.get_name() if application is not None else ""
            if focus_changed:
                focus["safe"] = not sensitive
                emit("focus_field", app=app, pid=0, window_title="", focused_role=role,
                     focused_name=name, focused_value="")
                return
            if sensitive or not states.contains(Atspi.StateType.EDITABLE):
                return
            text = source.get_text_iface()
            if text is None or text.get_character_count() > 262144:
                return
            value = text.get_text(0, -1)
            emit("text", field=app + ":" + str(hash(source)), text=value, safe=True, replace=True,
                 select_all="ctrl+a")
        except Exception:
            # Inaccessible/vanished controls fail closed: never read keystrokes
            # as a fallback for password or custom widgets.
            focus["safe"] = False
            return

    return accessibility


def capture_key(key, modifiers, safe):
    """Serialize shortcuts/navigation only, with positive focus authorization."""
    navigation = {"enter", "esc", "tab", "backspace", "delete", "home", "end", "pageup", "pagedown",
                  "up", "down", "left", "right"}
    shortcut = bool(set(modifiers) & {"ctrl", "alt", "cmd"})
    function = re.fullmatch(r"f(?:[1-9]|1[0-2])", key) is not None
    known = (len(key) == 1 and key.isascii() and key.isalnum()) or key == "space" or key in navigation or function
    return safe and known and (shortcut or key in navigation or function)


def main():
    if os.environ.get("XDG_SESSION_TYPE", "").lower() == "wayland" or not os.environ.get("DISPLAY"):
        raise RuntimeError("Desktop recording currently requires an X11 session. Wayland global recording is not supported.")
    for command in ("xinput", "stdbuf"):
        if not shutil.which(command):
            raise RuntimeError("Install xinput and coreutils for X11 desktop recording.")
    try:
        import gi
        gi.require_version("Atspi", "2.0")
        from gi.repository import Atspi
    except (ImportError, ValueError) as exc:
        raise RuntimeError("Install python3-gi, gir1.2-atspi-2.0 and at-spi2-core; enable desktop accessibility.") from exc

    x11 = ctypes.CDLL(ctypes.util.find_library("X11") or "libX11.so.6")
    x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
    x11.XOpenDisplay.restype = ctypes.c_void_p
    x11.XCloseDisplay.argtypes = [ctypes.c_void_p]
    x11.XkbKeycodeToKeysym.argtypes = [ctypes.c_void_p, ctypes.c_ubyte, ctypes.c_int, ctypes.c_int]
    x11.XkbKeycodeToKeysym.restype = ctypes.c_ulong
    x11.XKeysymToString.argtypes = [ctypes.c_ulong]
    x11.XKeysymToString.restype = ctypes.c_char_p
    display = x11.XOpenDisplay(None)
    if not display:
        raise RuntimeError("Cannot open DISPLAY. Run inside an authorized X11 desktop session.")

    Atspi.init()
    desktop = Atspi.get_desktop(0)
    if desktop is None:
        raise RuntimeError("The AT-SPI accessibility bus is unavailable.")
    monitor = subprocess.Popen(["stdbuf", "-oL", "xinput", "test-xi2", "--root"],
                               stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                               stderr=subprocess.DEVNULL, text=True, bufsize=1)
    listeners = []
    focus = {"safe": False}

    accessibility = make_accessibility_handler(Atspi, focus, emit)

    for event_name in ("object:text-changed", "object:state-changed:focused"):
        listener = Atspi.EventListener.new(accessibility)
        if not listener.register(event_name):
            raise RuntimeError("Cannot subscribe to AT-SPI events: " + event_name)
        listeners.append((listener, event_name))

    aliases = {"Return": "enter", "KP_Enter": "enter", "Escape": "esc", "Tab": "tab",
               "ISO_Left_Tab": "tab", "BackSpace": "backspace", "Delete": "delete",
               "Home": "home", "End": "end", "Prior": "pageup", "Next": "pagedown",
               "Up": "up", "Down": "down", "Left": "left", "Right": "right", "space": "space"}

    def handle(block):
        if not block:
            return
        match = re.search(r"EVENT type\s+(\d+)", block[0])
        if not match:
            return
        event_type = int(match.group(1))
        if event_type not in (2, 4, 5):
            return
        data = "\n".join(block)
        detail = re.search(r"detail:\s+(\d+)", data)
        if not detail:
            return
        detail = int(detail.group(1))
        point = re.search(r"root:\s+(-?[\d.]+)/(-?[\d.]+)", data)
        if event_type in (4, 5):
            if detail in (4, 5, 6, 7):
                if event_type == 4:
                    emit("scroll", dx={6: -1, 7: 1}.get(detail, 0), dy={4: -1, 5: 1}.get(detail, 0))
            elif detail in (1, 2, 3) and point:
                emit("pointer_down" if event_type == 4 else "pointer_up",
                     button={1: "left", 2: "middle", 3: "right"}[detail],
                     x=float(point.group(1)), y=float(point.group(2)))
            return
        # Capture shortcuts/navigation only. Text is exclusively AT-SPI data.
        if not focus["safe"]:
            return
        effective = re.search(r"effective:\s*(0x[0-9a-fA-F]+|\d+)", data)
        mask = int(effective.group(1), 0) if effective else 0
        modifiers = [name for flag, name in ((1, "shift"), (4, "ctrl"), (8, "alt"), (64, "cmd")) if mask & flag]
        symbol = x11.XKeysymToString(x11.XkbKeycodeToKeysym(display, detail, 0, 0))
        if not symbol:
            return
        symbol = symbol.decode("ascii", "ignore")
        key = aliases.get(symbol, symbol.lower())
        if capture_key(key, modifiers, focus["safe"]):
            emit("key", key=key, modifiers=modifiers, secure=False)

    def read_input():
        block = []
        try:
            for line in monitor.stdout:
                if line.startswith("EVENT type"):
                    handle(block)
                    block = [line]
                else:
                    block.append(line)
                    # XI2 terminates every event with a blank line.
                    if not line.strip() and block and block[0].startswith("EVENT type"):
                        handle(block)
                        block = []
            handle(block)
        finally:
            emit("capture_error", message="The X11 input monitor stopped. Stop and restart recording.")
            Atspi.event_quit()

    reader = threading.Thread(target=read_input, name="lumo-xinput", daemon=True)
    emit("ready", backend="linux-x11-atspi")
    reader.start()
    try:
        Atspi.event_main()
    finally:
        for listener, event_name in listeners:
            listener.deregister(event_name)
        monitor.terminate()
        monitor.wait(timeout=3)
        reader.join(timeout=3)
        x11.XCloseDisplay(display)


if __name__ == "__main__":
    try:
        main()
    except Exception as exc:
        emit("error", message=str(exc))
        sys.exit(1)
