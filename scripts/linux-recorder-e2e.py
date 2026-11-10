#!/usr/bin/env python3
"""Exercise native X11/AT-SPI capture in an isolated Xvfb session, never a live desktop."""
import argparse
import json
import os
from pathlib import Path
import re
import selectors
import shutil
import subprocess
import sys
import tempfile
import time

APP = r'''
import gi
gi.require_version("Gtk", "3.0")
from gi.repository import Gtk
window = Gtk.Window(title="Lumo isolated recorder fixture")
window.set_default_size(500, 230)
box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
window.add(box)
normal = Gtk.Entry()
normal.set_name("plain-text")
box.pack_start(normal, False, False, 20)
password = Gtk.Entry()
password.set_visibility(False)
password.set_name("password")
box.pack_start(password, False, False, 20)
window.connect("destroy", Gtk.main_quit)
window.show_all()
Gtk.main()
'''


def wait_ready(process, timeout=15):
    selector = selectors.DefaultSelector()
    selector.register(process.stderr, selectors.EVENT_READ)
    deadline = time.monotonic() + timeout
    output = b""
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError("Capture exited before ready: " + output.decode(errors="replace"))
        for key, _ in selector.select(.1):
            chunk = os.read(key.fileobj.fileno(), 4096)
            output += chunk
            if b"CAPTURE_READY" in output:
                return
    raise RuntimeError("Capture startup timed out: " + output.decode(errors="replace"))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--capture-binary", required=True)
    args = parser.parse_args()
    if sys.platform != "linux" or not re.search(r"(?:^|[\s/])Xvfb(?:\s|$)", subprocess.check_output(["ps", "-eo", "comm"], text=True)):
        raise RuntimeError("Run only under an isolated Linux Xvfb: dbus-run-session -- xvfb-run -a python3 scripts/linux-recorder-e2e.py ...")
    for tool in ("xdotool", "xinput"):
        if not shutil.which(tool):
            raise RuntimeError("Required fixture tool missing: " + tool)
    # The active DISPLAY must belong to Xvfb, not merely coexist with one.
    display = os.environ.get("DISPLAY", "").split(".")[0]
    processes = subprocess.check_output(["ps", "-eo", "args"], text=True).splitlines()
    if not display or not any("Xvfb " in line and display in line.split() for line in processes):
        raise RuntimeError("The current DISPLAY does not belong to Xvfb; refusing to inject test input")
    environment = dict(os.environ, XDG_SESSION_TYPE="x11", GTK_MODULES="gail:atk-bridge", NO_AT_BRIDGE="0")
    with tempfile.TemporaryDirectory(prefix="lumo-recorder-fixture-") as directory:
        root = Path(directory)
        (root / "app.py").write_text(APP)
        app = subprocess.Popen(["/usr/bin/python3", str(root / "app.py")], env=environment,
                               stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
        capture = None
        try:
            window = subprocess.check_output(["xdotool", "search", "--sync", "--name", "Lumo isolated recorder fixture"], env=environment, text=True, timeout=15).splitlines()[0]
            capture = subprocess.Popen([args.capture_binary, "7"], env=environment, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            wait_ready(capture)
            subprocess.run(["xdotool", "windowfocus", window, "mousemove", "--window", window, "80", "45", "click", "1"], env=environment, check=True, timeout=5)
            time.sleep(.2)
            subprocess.run(["xdotool", "type", "--clearmodifiers", "native-safe-text"], env=environment, check=True, timeout=5)
            time.sleep(.3)
            subprocess.run(["xdotool", "key", "Tab"], env=environment, check=True, timeout=5)
            subprocess.run(["xdotool", "type", "--clearmodifiers", "must-never-record-this-password"], env=environment, check=True, timeout=5)
            subprocess.run(["xdotool", "mousemove", "--window", window, "300", "180", "click", "4"], env=environment, check=True, timeout=5)
            stdout, stderr = capture.communicate(timeout=15)
            if capture.returncode:
                raise RuntimeError("Native capture failed: " + stderr)
            result = json.loads(stdout)
            assert "desktop.click" in result["yaml"], result
            assert "desktop.type" in result["yaml"] and "native-safe-text" in result["yaml"], result
            assert "desktop.scroll" in result["yaml"], result
            assert "must-never-record-this-password" not in stdout, "Password leaked into raw events or YAML"
            for event in result["events"]:
                if event["kind"] == "text":
                    assert "password" not in event["payload"].get("text", ""), event
            print("Native Linux pointer, AT-SPI text, password exclusion and stop acceptance passed")
        finally:
            if capture and capture.poll() is None:
                capture.terminate()
                capture.wait(timeout=5)
            app.terminate()
            app.wait(timeout=5)


if __name__ == "__main__":
    main()
