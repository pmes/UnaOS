#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 The Architect & Una
#
# qmp_shoot.py — drive a running QEMU's QMP socket to capture a framebuffer screendump.
#
# This is the headless framebuffer-verification technique used across the bring-up work: a
# laptop / Pi panel can't be observed from CI, and serial only proves the boot *logic*, not
# that pixels actually land. So we run QEMU headless with `-display none -qmp tcp:HOST:PORT,
# server,nowait`, let it boot, then ask QMP to render the guest's active display to a PNG we
# can open. (QEMU 7.1+ supports the `format` arg to `screendump`; this host is 11.x.)
#
# It does NOT launch QEMU — the builder (x86) and arroyo (aarch64) own the correct device sets
# and the fresh-OVMF-vars copy. Launch QEMU headless with a QMP port (via UNAOS_QEMU_EXTRA on
# x86, or arroyo's arm paths), then point this at that port.
#
#   python3 scripts/qmp_shoot.py --port 4445 --wait 9 --out /tmp/shot.png
#   python3 scripts/qmp_shoot.py --port 4445 --wait 9 --out /tmp/shot.png --quit
#
# Timing is driven from python (time.sleep) because the shell `sleep` binary is blocked in this
# sandbox. --quit asks QEMU to exit after the shot (useful when nothing else will kill it).

import argparse
import json
import socket
import sys
import time


def connect(host, port, timeout):
    """Connect to the QMP TCP socket, retrying until QEMU has opened it."""
    deadline = time.time() + timeout
    last_err = None
    while time.time() < deadline:
        try:
            s = socket.create_connection((host, port), timeout=2.0)
            s.settimeout(5.0)
            return s
        except OSError as e:
            last_err = e
            time.sleep(0.25)
    raise SystemExit(f"could not connect to QMP {host}:{port}: {last_err}")


class Qmp:
    def __init__(self, sock):
        self.sock = sock
        self.buf = b""

    def _read_obj(self, timeout=10.0):
        """Read one newline-delimited JSON object from the stream."""
        self.sock.settimeout(timeout)
        while b"\n" not in self.buf:
            chunk = self.sock.recv(4096)
            if not chunk:
                raise SystemExit("QMP connection closed unexpectedly")
            self.buf += chunk
        line, self.buf = self.buf.split(b"\n", 1)
        return json.loads(line.decode("utf-8", "replace"))

    def execute(self, cmd, args=None, timeout=15.0):
        """Send a command and return its `return`/`error` reply, skipping async events."""
        msg = {"execute": cmd}
        if args:
            msg["arguments"] = args
        self.sock.sendall((json.dumps(msg) + "\r\n").encode())
        while True:
            obj = self._read_obj(timeout)
            if "return" in obj or "error" in obj:
                return obj
            # else it's an async {"event": ...} / greeting echo — keep reading.


def main():
    ap = argparse.ArgumentParser(description="Capture a QEMU framebuffer via QMP screendump.")
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, required=True)
    ap.add_argument("--wait", type=float, default=9.0,
                    help="seconds to let the guest boot before the shot")
    ap.add_argument("--out", required=True, help="output PNG path")
    ap.add_argument("--connect-timeout", type=float, default=20.0)
    ap.add_argument("--quit", action="store_true", help="ask QEMU to quit after the shot")
    args = ap.parse_args()

    sock = connect(args.host, args.port, args.connect_timeout)
    qmp = Qmp(sock)

    # Greeting first, then negotiate out of capabilities-negotiation mode.
    greeting = qmp._read_obj()
    if "QMP" not in greeting:
        print(f"warning: unexpected greeting: {greeting}", file=sys.stderr)
    r = qmp.execute("qmp_capabilities")
    if "error" in r:
        raise SystemExit(f"qmp_capabilities failed: {r['error']}")

    print(f"[qmp] connected; letting guest boot {args.wait:.1f}s ...", file=sys.stderr)
    time.sleep(args.wait)

    # Prefer an explicit PNG; fall back to QEMU's default (PPM) if this build rejects `format`.
    r = qmp.execute("screendump", {"filename": args.out, "format": "png"}, timeout=30.0)
    if "error" in r:
        print(f"[qmp] png screendump rejected ({r['error'].get('desc')}); "
              f"retrying without format", file=sys.stderr)
        r = qmp.execute("screendump", {"filename": args.out}, timeout=30.0)
        if "error" in r:
            raise SystemExit(f"screendump failed: {r['error']}")
    print(f"[qmp] screendump written to {args.out}", file=sys.stderr)

    if args.quit:
        try:
            qmp.execute("quit", timeout=3.0)
        except SystemExit:
            pass  # QEMU dropping the connection on quit is expected.
    sock.close()


if __name__ == "__main__":
    main()
