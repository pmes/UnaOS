#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 The Architect & Una
#
# qmp_type.py — drive the kernel shell from headless QEMU: connect to QMP, let the guest boot,
# TYPE a string via `send-key` (one qcode per character), optionally press Enter, then capture a
# framebuffer screendump. The input counterpart to qmp_shoot.py.
#
# Keys are delivered to the emulated usb-kbd (xHCI HID), exactly as a user typing — so this
# exercises the real input path (HID -> Event::Key -> console/shell), not a back door. Used to
# verify the shell on screen without a physical keyboard (e.g. `panic` -> red panic screen,
# `vug` -> test pattern). Launch QEMU with `-display none -qmp tcp:HOST:PORT,server,nowait`
# (via UNAOS_QEMU_EXTRA on x86, or arroyo's arm paths), then point this at that port.
#
#   python3 scripts/qmp_type.py --port 4472 --wait 11 --text panic --enter --out ~/unaos-bench/scratch/p.png
#
# Timing is driven from python (the shell `sleep` binary is unreliable under the sandbox).

import argparse
import json
import socket
import sys
import time

# qcode names for the keys we type. Letters/digits map to themselves; add symbols as needed.
SYMBOLS = {" ": "spc", "-": "minus", ".": "dot", "/": "slash"}


def qcode(ch):
    if ch.isalpha():
        return ch.lower()
    if ch.isdigit():
        return ch
    return SYMBOLS.get(ch, ch)


class Qmp:
    def __init__(self, sock):
        self.sock = sock
        self.buf = b""

    def _read_obj(self, timeout=10.0):
        self.sock.settimeout(timeout)
        while b"\n" not in self.buf:
            chunk = self.sock.recv(4096)
            if not chunk:
                raise SystemExit("QMP connection closed")
            self.buf += chunk
        line, self.buf = self.buf.split(b"\n", 1)
        return json.loads(line.decode("utf-8", "replace"))

    def execute(self, cmd, args=None, timeout=20.0):
        msg = {"execute": cmd}
        if args:
            msg["arguments"] = args
        self.sock.sendall((json.dumps(msg) + "\r\n").encode())
        while True:
            obj = self._read_obj(timeout)
            if "return" in obj or "error" in obj:
                return obj


def connect(host, port, timeout):
    deadline = time.time() + timeout
    last = None
    while time.time() < deadline:
        try:
            return socket.create_connection((host, port), timeout=2.0)
        except OSError as e:
            last = e
            time.sleep(0.25)
    raise SystemExit(f"could not connect to QMP {host}:{port}: {last}")


def main():
    ap = argparse.ArgumentParser(description="Type a string into headless QEMU and screendump.")
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, required=True)
    ap.add_argument("--wait", type=float, default=11.0, help="seconds to boot before typing")
    ap.add_argument("--text", default="", help="characters to type via send-key")
    ap.add_argument("--enter", action="store_true", help="press Enter after the text")
    ap.add_argument("--char-delay", type=float, default=0.18)
    ap.add_argument("--postwait", type=float, default=2.0, help="seconds after typing before the shot")
    ap.add_argument("--out", required=True)
    ap.add_argument("--connect-timeout", type=float, default=45.0)
    a = ap.parse_args()

    qmp = Qmp(connect(a.host, a.port, a.connect_timeout))
    qmp._read_obj()  # greeting
    qmp.execute("qmp_capabilities")

    print(f"[qmp] boot {a.wait:.1f}s, then type {a.text!r} (enter={a.enter})", file=sys.stderr)
    time.sleep(a.wait)
    for ch in a.text:
        qmp.execute("send-key", {"keys": [{"type": "qcode", "data": qcode(ch)}]})
        time.sleep(a.char_delay)
    if a.enter:
        qmp.execute("send-key", {"keys": [{"type": "qcode", "data": "ret"}]})
    time.sleep(a.postwait)

    r = qmp.execute("screendump", {"filename": a.out, "format": "png"}, timeout=30.0)
    if "error" in r:
        r = qmp.execute("screendump", {"filename": a.out}, timeout=30.0)
        if "error" in r:
            raise SystemExit(f"screendump failed: {r['error']}")
    print(f"[qmp] screendump -> {a.out}", file=sys.stderr)


if __name__ == "__main__":
    main()
