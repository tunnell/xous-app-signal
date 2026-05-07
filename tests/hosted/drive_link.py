#!/usr/bin/env python3
"""Drive a hosted-mode Xous emulation through the launcher → xas →
Link flow, leaving a device-name TextEntry modal up at the end.
The test wrapper (`test_link_qr.sh`) then watches the kernel log
for `Event::LinkUrl` emission to determine PASS/FAIL.

Usage: drive_link.py <window-id-hex>

Reuses the XSendEvent ctypes pattern from `agent_notes/
drive_to_signal.py` (more reliable than xdotool under SSH X11).
"""

import ctypes
import os
import sys
import time

DISPLAY = os.environ.get("DISPLAY", "localhost:10.0")

X11 = ctypes.cdll.LoadLibrary("libX11.so.6")
c_ulong, c_int, c_uint = ctypes.c_ulong, ctypes.c_int, ctypes.c_uint
X11.XOpenDisplay.restype = ctypes.c_void_p
X11.XOpenDisplay.argtypes = [ctypes.c_char_p]
X11.XSync.argtypes = [ctypes.c_void_p, c_int]
X11.XDefaultRootWindow.restype = c_ulong
X11.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
X11.XKeysymToKeycode.restype = c_uint
X11.XKeysymToKeycode.argtypes = [ctypes.c_void_p, c_ulong]
X11.XFlush.argtypes = [ctypes.c_void_p]
X11.XSendEvent.restype = c_int
X11.XSendEvent.argtypes = [ctypes.c_void_p, c_ulong, c_int, c_ulong, ctypes.c_void_p]


class XEvent(ctypes.Union):
    class XKeyEvent(ctypes.Structure):
        _fields_ = [
            ("type", c_int), ("serial", c_ulong), ("send_event", c_int),
            ("display", ctypes.c_void_p), ("window", c_ulong), ("root", c_ulong),
            ("subwindow", c_ulong), ("time", c_ulong),
            ("x", c_int), ("y", c_int), ("x_root", c_int), ("y_root", c_int),
            ("state", c_uint), ("keycode", c_uint), ("same_screen", c_int),
        ]
    _fields_ = [("key", XKeyEvent), ("pad", ctypes.c_char * 192)]


def press(dpy, win, root, kc, wait, label):
    ev = XEvent()
    ev.key.type = 2
    ev.key.send_event = 1
    ev.key.display = dpy
    ev.key.window = win
    ev.key.root = root
    ev.key.same_screen = 1
    ev.key.keycode = kc
    X11.XSendEvent(dpy, win, 1, 1, ctypes.byref(ev))
    X11.XFlush(dpy)
    time.sleep(0.05)
    ev.key.type = 3
    X11.XSendEvent(dpy, win, 1, 2, ctypes.byref(ev))
    X11.XFlush(dpy)
    print(f"  [{label}] kc={kc}, wait {wait}s")
    sys.stdout.flush()
    time.sleep(wait)


def main():
    if len(sys.argv) != 2:
        print("usage: drive_link.py <window-id-hex>", file=sys.stderr)
        sys.exit(2)
    win = int(sys.argv[1], 0)

    dpy = X11.XOpenDisplay(DISPLAY.encode())
    if not dpy:
        print(f"ERROR: cannot open display {DISPLAY}", file=sys.stderr)
        sys.exit(1)
    root = X11.XDefaultRootWindow(dpy)

    kc_home = X11.XKeysymToKeycode(dpy, 0xFF50)
    kc_down = X11.XKeysymToKeycode(dpy, 0xFF54)
    kc_return = X11.XKeysymToKeycode(dpy, 0xFF0D)

    # Step 1: launcher main menu → Apps → xas (Signal).
    print("=== launcher → Apps → xas ===")
    press(dpy, win, root, kc_home, 1.5, "Home → main menu")
    press(dpy, win, root, kc_down, 0.3, "Down → Apps")
    press(dpy, win, root, kc_home, 4.5, "Home → Apps submenu")
    press(dpy, win, root, kc_down, 0.3, "Down → xas (only app bundled)")
    press(dpy, win, root, kc_home, 2.0, "Home → launch xas")

    # Step 2: xas Menu → Link selected → Enter to open device-name modal.
    # Cursor starts on Link in pre-link Menu, so just Enter selects.
    print("=== xas Menu → Link ===")
    press(dpy, win, root, kc_return, 1.0, "Enter → Link selected")

    # Step 3: device-name modal accepts default ("xas").
    # The TextEntry modal has the field pre-filled; Enter alone
    # commits it.
    print("=== accept device name ===")
    press(dpy, win, root, kc_return, 1.0, "Enter → accept device name")

    # At this point Cmd::LinkDevice has been sent. The worker is
    # connecting to chat.signal.org; expect Event::LinkUrl within
    # ~5–15s. The test wrapper polls the kernel log; this script
    # exits 0 here.
    print("done — driver returns; wrapper polls log for URL emission")


if __name__ == "__main__":
    main()
