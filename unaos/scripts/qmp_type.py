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
#
# XHCIKBD (rmbp 2026-09-15) — the BURST mode, `--bursts N --burst P`, is the x86 keyboard-report-loss
# fixture's typist (`UNAOS_XHCIKBD=1 ./arroyo test`). It is an EXTENSION of this script, not a
# second QMP path: same `Qmp` class, same port plumbing, same `--wait`.
#
#   ONE BURST = the STALL KEY pressed (default F11: the kernel's `xhcikbd_note` spins 600 ms inside
#   the keyboard completion branch on its press edge — a deliberately slow pass, the shape of a
#   92 ms present made long enough to hold the whole stream), then `--event-gap` (20 ms) later its
#   release, then P alternating press+release pairs of `--burst-keys`, every event its OWN
#   `input-send-event` command, `--event-gap` apart. Under the single-TRB driver the endpoint has
#   no TD for the whole stall (the re-arm is the branch's last statement); under B44's N TRBs it
#   is armed throughout. After the bursts (`--burst-gap` apart) and `--sentinel-delay` of quiet,
#   the SENTINEL (default F12, types nothing) is pressed once; its press edge makes the kernel
#   print `:: XHCIKBD: ... -> PASS|FAIL ::`. A run with no such line is a run whose typist never
#   got here — arroyo reds it for that, separately.
#
#   WHY THE STREAM IS SHAPED SO (measured in the arc's logs 03/05): QEMU's usb-kbd delivers at most
#   ONE report per endpoint polling interval (8 ms here) no matter how many TRBs wait, and queues
#   at most 16 undelivered events (hw/input/hid.c QUEUE_LENGTH), dropping the rest — an atomic
#   20-event `input-send-event` arrived as exactly 17 at BOTH depths. Spacing the events wider
#   than the interval lets each armed TRB absorb one during the stall; 19 events per burst
#   (release + 9 pairs) then means 3 lost with zero TRBs armed and none lost with four. `send-key`
#   was rejected: its hold-time queue makes the press/release timing QEMU's, not ours.
#
#   P=9 is the `XHCIKBD_BURST_PAIRS` constant in drivers/xhci/mod.rs and N=4 its `XHCIKBD_BURSTS`;
#   the kernel computes `expected=` from its constants, so the two MUST agree or `lost=` lies.
#
#   THE MARKER. `--marker TEXT --marker-log PATH` (repeatable) holds the bursts until the guest's
#   serial log contains every TEXT, so the stream lands after the desktop is up and the boot
#   battery is over. `--wait` then still applies on top, as settle time.
#
#   python3 scripts/qmp_type.py --port 4464 --marker '[hidkeys] set-idle ok' \
#       --marker-log target/serial.log --wait 5 --bursts 4 --burst 9 --burst-gap 2 --sentinel f12

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


def key_event(name, down):
    """One `input-send-event` key event, by qcode name."""
    return {"type": "key", "data": {"down": down, "key": {"type": "qcode", "data": name}}}


def wait_for_marker(path, text, timeout):
    """XHCIKBD: block until the serial log at `path` contains `text` (bytes search — the log may
    carry control bytes). Returns True on hit, False on timeout. The file may not exist yet:
    arroyo removes it before the builder starts QEMU."""
    needle = text.encode("utf-8")
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            with open(path, "rb") as f:
                if needle in f.read():
                    return True
        except OSError:
            pass
        time.sleep(0.25)
    return False


def main():
    ap = argparse.ArgumentParser(description="Type a string into headless QEMU and screendump.")
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, required=True)
    ap.add_argument("--wait", type=float, default=11.0, help="seconds to boot before typing")
    ap.add_argument("--text", default="", help="characters to type via send-key")
    ap.add_argument("--enter", action="store_true", help="press Enter after the text")
    ap.add_argument("--char-delay", type=float, default=0.18)
    ap.add_argument("--postwait", type=float, default=2.0, help="seconds after typing before the shot")
    # XHCIKBD: `--out` is optional now — the burst fixture wants no screendump. Every pre-existing
    # caller passes it, so nothing they do changes.
    ap.add_argument("--out", default="", help="screendump path (omit for no screendump)")
    ap.add_argument("--connect-timeout", type=float, default=45.0)
    # XHCIKBD burst mode (see the header). All default OFF; `--bursts 0` is the old script exactly.
    ap.add_argument("--marker", action="append", default=[], help="hold until the serial log contains this text (repeatable: all must appear)")
    ap.add_argument("--marker-log", default="", help="serial log to watch for --marker")
    ap.add_argument("--marker-timeout", type=float, default=60.0)
    ap.add_argument("--bursts", type=int, default=0, help="number of atomic key bursts to inject")
    ap.add_argument("--burst", type=int, default=10, help="press+release PAIRS per burst (2P events)")
    ap.add_argument("--burst-keys", default="a,b", help="qcodes to alternate through within a burst")
    ap.add_argument("--burst-gap", type=float, default=2.0, help="seconds between bursts")
    ap.add_argument("--stall-key", default="f11", help="qcode pressed at the head of each burst ('' = none)")
    ap.add_argument("--event-gap", type=float, default=0.02, help="seconds between the events of one burst")
    ap.add_argument("--sentinel", default="f12", help="qcode pressed once after the bursts ('' = none)")
    ap.add_argument("--sentinel-delay", type=float, default=1.0, help="quiet seconds before the sentinel")
    a = ap.parse_args()

    qmp = Qmp(connect(a.host, a.port, a.connect_timeout))
    qmp._read_obj()  # greeting
    qmp.execute("qmp_capabilities")

    t0 = time.time()
    for m in a.marker:
        # The log only grows, so waiting for the markers in the given order is order-independent:
        # a marker that appeared earlier is already there when its turn comes.
        left = a.marker_timeout - (time.time() - t0)
        print(f"[qmp] waiting up to {max(left, 0):.0f}s for {m!r} in {a.marker_log}", file=sys.stderr)
        if not wait_for_marker(a.marker_log, m, max(left, 0)):
            # A harness fault, not a verdict: the guest never reached the state the burst is
            # meant to land in. Nothing is typed, so the kernel prints no XHCIKBD line, and
            # arroyo's presence check reds the leg on that — this exit code is for the log.
            raise SystemExit(f"[qmp] marker {m!r} not seen within {a.marker_timeout:.0f}s — nothing typed")
        print(f"[qmp] marker {m!r} seen at +{time.time() - t0:.1f}s", file=sys.stderr)

    print(f"[qmp] boot {a.wait:.1f}s, then type {a.text!r} (enter={a.enter})", file=sys.stderr)
    time.sleep(a.wait)
    for ch in a.text:
        qmp.execute("send-key", {"keys": [{"type": "qcode", "data": qcode(ch)}]})
        time.sleep(a.char_delay)
    if a.enter:
        qmp.execute("send-key", {"keys": [{"type": "qcode", "data": "ret"}]})

    if a.bursts > 0:
        keys = [k for k in a.burst_keys.split(",") if k]
        if not keys:
            raise SystemExit("--burst-keys is empty")
        sent = 0
        for b in range(a.bursts):
            evs = []
            if a.stall_key:
                evs.append(key_event(a.stall_key, True))
                evs.append(key_event(a.stall_key, False))
            for i in range(a.burst):
                k = keys[i % len(keys)]
                evs.append(key_event(k, True))
                evs.append(key_event(k, False))
            t0 = time.time()
            for n, ev in enumerate(evs):
                r = qmp.execute("input-send-event", {"events": [ev]})
                if "error" in r:
                    raise SystemExit(f"[qmp] input-send-event failed on burst {b + 1} event {n + 1}: {r['error']}")
                # Pace to the grid from t0 so a slow round trip does not accumulate.
                target = t0 + (n + 1) * a.event_gap
                now = time.time()
                if target > now:
                    time.sleep(target - now)
            dt = (time.time() - t0) * 1000.0
            sent += len(evs)
            print(f"[qmp] burst {b + 1}/{a.bursts}: {len(evs)} events (stall key {a.stall_key!r} + {a.burst} pairs over {keys}), one command each, {a.event_gap * 1000:.0f} ms apart, {dt:.0f} ms total", file=sys.stderr)
            if b + 1 < a.bursts:
                time.sleep(a.burst_gap)
        print(f"[qmp] bursts done: {sent} events total = expected reports if none were lost", file=sys.stderr)
        if a.sentinel:
            time.sleep(a.sentinel_delay)
            r = qmp.execute("input-send-event", {"events": [key_event(a.sentinel, True), key_event(a.sentinel, False)]})
            if "error" in r:
                raise SystemExit(f"[qmp] sentinel {a.sentinel!r} failed: {r['error']}")
            print(f"[qmp] sentinel {a.sentinel!r} pressed after {a.sentinel_delay:.1f}s quiet", file=sys.stderr)

    time.sleep(a.postwait)

    if a.out:
        r = qmp.execute("screendump", {"filename": a.out, "format": "png"}, timeout=30.0)
        if "error" in r:
            r = qmp.execute("screendump", {"filename": a.out}, timeout=30.0)
            if "error" in r:
                raise SystemExit(f"screendump failed: {r['error']}")
        print(f"[qmp] screendump -> {a.out}", file=sys.stderr)


if __name__ == "__main__":
    main()
