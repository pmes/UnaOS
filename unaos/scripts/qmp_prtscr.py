#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 The Architect & Una
#
# qmp_prtscr.py — the PRTSCR two-chord proof, driven over QMP.
#
# `docs/dev/OS/08_VIDEO/screenshot.md` §"The KEY, in QEMU" describes this run in prose and it was
# executed by hand: boot `test-fat sf` headless with a QMP port, press the `print` qcode twice
# 45 s apart, then read `awk '/PRTSCR/' target/serial.log`. PRTSCR-ASYNC (SR2) needs a THIRD chord
# the prose form cannot deliver by hand — one injected *while the first capture is still running* —
# so the schedule lives here instead of in a human's fingers.
#
# What the schedule proves, and where each fact lands on the wire:
#
#   chord A                     -> ':: PRTSCR: ... -> capture armed ::' then '-> capturing' / '-> OK'
#   chord B, --gap after A      -> an 'armed' line BETWEEN A's 'capturing' and A's 'OK'. That line is
#                                  printed by the EHCI HID decoder from inside the device-service
#                                  pass, so its presence there IS the proof that input is still
#                                  being serviced during a capture — the whole of SR2. It is
#                                  followed by the named ':: PRTSCR: refused — capture in flight ...'
#                                  line, and then by B's OWN capture once A reaches its verdict.
#   --text, right after B       -> a plain typed line, the second input-alive probe (it reaches the
#                                  shell, not this module, so it is scored on the console/shell wire).
#   chord C, --settle after B   -> an ordinary third capture, the control: the machine is still
#                                  answering the key long after two captures have run.
#
# It does NOT launch QEMU. Launch it headless with a QMP port and point this at that port:
#
#   export UNAOS_QEMU_EXTRA="-qmp tcp:127.0.0.1:4490,server,nowait"
#   python3 unaos/scripts/qmp_prtscr.py --port 4490 --connect-timeout 900 --boot 60 &
#   UNAOS_WC=1 ./arroyo test-fat sf 200
#
# Timing is driven from python (time.sleep) because the shell `sleep` binary is unreliable here.

import argparse
import json
import socket
import sys
import time

# qcode names for the typed probe. Letters/digits map to themselves — same table as qmp_type.py.
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

    def key(self, name, hold_ms=None):
        args = {"keys": [{"type": "qcode", "data": name}]}
        if hold_ms:
            args["hold-time"] = hold_ms
        return self.execute("send-key", args)


def connect(host, port, timeout):
    deadline = time.time() + timeout
    last = None
    while time.time() < deadline:
        try:
            s = socket.create_connection((host, port), timeout=2.0)
            s.settimeout(5.0)
            return s
        except OSError as e:
            last = e
            time.sleep(0.25)
    raise SystemExit(f"qmp_prtscr: could not connect to {host}:{port} within {timeout}s ({last})")


def main():
    ap = argparse.ArgumentParser(description="PRTSCR two-chord QMP proof.")
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, required=True)
    ap.add_argument("--connect-timeout", type=float, default=900.0,
                    help="seconds to keep retrying the QMP connect (the build runs first)")
    ap.add_argument("--boot", type=float, default=60.0,
                    help="seconds to let the guest boot (storage must be up) before chord A")
    ap.add_argument("--gap", type=float, default=0.4,
                    help="seconds from chord A to chord B — SHORT, so B lands inside A's capture")
    ap.add_argument("--settle", type=float, default=30.0,
                    help="seconds from chord B to chord C")
    ap.add_argument("--tail", type=float, default=20.0,
                    help="seconds to wait after chord C before the screendump")
    ap.add_argument("--text", default="help",
                    help="typed mid-capture as the second input-alive probe ('' to skip)")
    ap.add_argument("--out", default="", help="optional screendump PNG at the end")
    ap.add_argument("--quit", action="store_true", help="ask QEMU to exit when done")
    a = ap.parse_args()

    q = Qmp(connect(a.host, a.port, a.connect_timeout))
    q._read_obj(30.0)               # greeting
    q.execute("qmp_capabilities")
    say = lambda m: print(f"[qmp_prtscr] {m}", file=sys.stderr, flush=True)

    say(f"connected; boot window {a.boot:.1f}s")
    time.sleep(a.boot)

    say("chord A: print")
    q.key("print")

    time.sleep(a.gap)
    say(f"chord B: print (+{a.gap:.2f}s — aimed inside A's capture)")
    q.key("print")

    if a.text:
        time.sleep(0.15)
        say(f"typing {a.text!r} mid-capture (input-alive probe)")
        for ch in a.text:
            q.key(qcode(ch))
            time.sleep(0.03)
        q.key("ret")

    time.sleep(a.settle)
    say(f"chord C: print (+{a.settle:.1f}s — the control)")
    q.key("print")

    time.sleep(a.tail)
    if a.out:
        say(f"screendump -> {a.out}")
        r = q.execute("screendump", {"filename": a.out}, timeout=60.0)
        if "error" in r:
            say(f"screendump error: {r['error']}")
    if a.quit:
        say("quit")
        try:
            q.execute("quit", timeout=5.0)
        except SystemExit:
            pass
    say("done — the verdict is in the serial log, not here")


if __name__ == "__main__":
    main()
