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
# QMPCHECK (rmbp 2026-09-22) — the `qmp_capabilities` handshake's REPLY is read and a refusal is
# fatal (see `main`): an unread refusal types into a closed monitor and the capture looks like a
# kernel that ignored every key. QUARRYCLICK paid four runs for that silence.
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
#
# XHCIHUB (rmbp 2026-09-15) — the POINTER mode, `--pointer N`, is the third mode and the first that
# is not a keyboard: it sends `input-send-event` MOVE and BUTTON events (QMP `InputEventKind` `abs`
# / `rel` / `btn`) so the `:: XHCIHUB:` scorer's `evts=` field counts reports the hub-downstream
# usb-tablet actually delivered. Same `Qmp` class, same port, same `--marker` gate as the bursts.
# See the `move_event` / `btn_event` block below for the schema names and what `value` means.
#
#   python3 scripts/qmp_type.py --port 4464 --marker ':: MOUSE-1: HID pointer detected' \
#       --marker-log target/serial.log --wait 1 --pointer 12 --pointer-kind abs --postwait 0
#
# PTRACK (rmbp 2026-09-22, FLAKEFIX, rmbp-ledger B150) — ACKED POINTER DELIVERY, `--pointer-ack N`.
# Every mode above this line paces on WALL CLOCK and never learns whether QEMU's emulated device
# delivered anything. That is not a tolerance question, it is a delivery question, and the measurement
# says so:
#
#   MEASURED (TRACKPAD's go-red capture, 2026-09-22, host load ~23, `./arroyo test-ptr`):
#     [qmp] pointer: 36 rel moves ...            <- this script sent 36, QMP returned {} to all 36
#     :: MOUSE-1: 32 reports, last dx=0 dy=0 ... <- the guest's xHCI decode saw 32
#     :: PTRINSTALL: installs=27 reports=27 ...  <- the producer counted 27 of the 36 typed
#   and the SAME command on the same box one flake later (flakefix repro1, load 13):
#     :: MOUSE-1: 32 reports, last dx=-24 dy=-24 ::  /  installs=36 reports=36
#
# `last dx=0 dy=0` IS THE MECHANISM, and it is a delta this script CANNOT SEND: `--pointer-kind rel`
# alternates +step / -step every move, so every event on the wire is +-24 on both axes and a zero
# delta can only be a SUM. QEMU's `hid_pointer_event` (hw/input/hid.c) folds a new motion event into
# the TAIL queue entry whenever that entry has not been polled yet — "we combine events where possible
# to keep the queue small" — so two adjacent moves 50 ms apart become ONE report of (+24) + (-24) = 0
# the moment the guest's poll gap stretches past the injection gap (the same capture's
# `drain_gap_max_ms=140` against a 50 ms cadence). The guest then DROPS that report before counting
# it: the xHCI decode charges `mouse_report_count` (so MOUSE-1 still climbs) but only emits
# `Event::Mouse` when `dx != 0 || dy != 0`, so `[ptrinstall] reports=` never sees it.
#
# NOTHING IS LOST BETWEEN QMP AND THE ENDPOINT. The QMP socket acked all 36 (an `error` reply is
# already fatal below), the 16-deep `QUEUE_LENGTH` drop path needs >16 undelivered events and 50 ms
# spacing never builds that, and the travel is CONSERVED — only the report COUNT is not. So a fixture
# that counts injected events is, on a loaded host, counting the host's scheduler.
#
# WHAT `--pointer-ack N` DOES. After the wall-clock first pass it stops pacing on time and paces on
# the GUEST'S OWN COUNT, read out of the serial log this typist already holds open for `--marker`:
# settle `--ack-settle`, take the first count line printed AFTER that settle (`--ack-re`, one capture
# group), and RE-SEND exactly the shortfall `--ack-gap` apart. Exactly the shortfall, so the loop can
# undershoot and retry but can never overshoot a fixture that pins an equality.
#
#   THE CAP IS `--ack-rounds` (default 3) and it is a REAL cap: on exhaustion the typist prints
#   `[qmp] pointer ack: GAVE UP after N retry round(s) — guest counted C of T ...` and STOPS. It does
#   not widen anything and it does not exit non-zero: the shortfall is real delivery loss, the guest's
#   own count line carries it, and the fixture reds on that. A typist must never be able to turn a
#   delivery failure into a pass (LAWS §5 — wrong-strict is worse than wrong-lenient, and this is the
#   third option: measure, then say what you measured).
#
#   THE BUDGET, because this lane has a deadline. `[ptrinstall]` rolls up on the depth line's 5 s tick
#   and PTRLANE's moves land at ~32 s of guest uptime, while the one-shot `:: PTRINSTALL:` line that
#   x86-ptr.spec:67 pins closes at `PTRI_LATE_MS` = 60 s. One round costs `--ack-settle` + at most one
#   tick ~= 5.5 s, so 3 rounds ~= 17 s and the last retry is counted by ~50 s — inside the window.
#   Raising `--ack-rounds` past 4 on THIS lane buys nothing: the late line has already printed.
#
#   THE SETTLE IS WHAT MAKES THE READING SOUND. A rollup tick that fires between the last QMP send and
#   the guest's 8 ms poll would read low, the loop would re-send reports that were merely in flight,
#   and the count would land ABOVE the target — the one way an acked typist could red a lane it was
#   meant to green. `--ack-settle` (0.5 s default) is the quiet the reading is taken after, against a
#   measured `lag_max_ms` of 20-27 ms and a `drain_gap_max_ms` of 70-140 ms.
#
#   NOT FOR THE KEYBOARD BURSTS. `--bursts` exists to LOSE events — it measures how many the endpoint
#   drops with no TRB armed — so acking it would erase the thing it measures. This flag is pointer-only
#   on purpose.
#
#   python3 scripts/qmp_type.py --port 4464 --marker ':: MOUSE-1: HID pointer detected' \
#       --marker-log target/serial.log --pointer 36 --pointer-kind rel --pointer-ack 36

import argparse
import json
import os
import re
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


# XHCIHUB (rmbp 2026-09-15, LEDGER S1/S2) — the POINTER half of `input-send-event`, and the three
# QMP schema names it needs, cited rather than guessed (qemu `qapi/ui.json`):
#   * `InputEventKind` — the `type` field of an `InputEvent`. The four kinds are `key`, `btn`,
#     `rel` and `abs`; everything above this line uses `key`, everything below uses the other three.
#   * `InputMoveEvent` — the payload of BOTH `rel` and `abs`: `{ axis: InputAxis, value: int }`,
#     where `InputAxis` is `x` or `y`. The kind is what says how `value` reads — a DELTA for `rel`
#     (a usb-mouse) or an ABSOLUTE position for `abs` (a usb-tablet, which is the device
#     `UNAOS_XHCIHUB=1` parks behind the hub). An `abs` value is scaled 0 .. 0x7FFF on both axes
#     (`INPUT_EVENT_ABS_MAX`), NOT in pixels, so the sweep below is resolution-independent.
#   * `InputBtnEvent` — the payload of `btn`: `{ button: InputButton, down: bool }`, `InputButton`
#     being `left` / `middle` / `right` / `wheel-up` / `wheel-down` / `side` / `extra` / …
# One `input-send-event` carries a LIST of events and syncs once at the end, so an x and a y of the
# same step are sent together and the guest sees one report per step rather than two half-moves.
ABS_MAX = 0x7FFF


def move_event(kind, axis, value):
    """One `InputMoveEvent`, carried as kind `abs` (absolute position, 0..ABS_MAX) or `rel` (delta)."""
    return {"type": kind, "data": {"axis": axis, "value": int(value)}}


def btn_event(button, down):
    """One `InputBtnEvent` — `button` is an `InputButton` name, `down` its edge."""
    return {"type": "btn", "data": {"button": button, "down": bool(down)}}


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


def ack_count_after(path, rx, offset):
    """PTRACK — THE GUEST'S OWN COUNT, read from `path` starting at byte `offset`.

    Returns `(count, offset_now)`. `count` is the LAST match of `rx`'s first capture group among the
    COMPLETE lines appended past `offset`, or `None` if no complete matching line is there yet;
    `offset_now` advances past every complete line read, matching or not, so the next look can never
    re-read a rollup printed before the injection it is supposed to be a reading of. Byte offsets, and
    a bytes pattern, because these logs carry control bytes (the `awk`-not-`grep` rule) and a decode
    would make the offset arithmetic a guess."""
    try:
        with open(path, "rb") as f:
            f.seek(offset)
            data = f.read()
    except OSError:
        return None, offset
    cut = data.rfind(b"\n")
    if cut < 0:
        return None, offset
    last = None
    for m in rx.finditer(data[: cut + 1]):
        last = int(m.group(1))
    return last, offset + cut + 1


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
    # XHCIHUB pointer mode (see the `move_event`/`btn_event` block). Default OFF: `--pointer 0` is
    # the old script exactly, and no pre-existing caller changes behaviour.
    ap.add_argument("--pointer", type=int, default=0, help="XHCIHUB: pointer MOVES to inject (0 = off)")
    ap.add_argument("--pointer-kind", default="abs", choices=("abs", "rel"), help="InputEventKind for the moves: abs (usb-tablet) or rel (usb-mouse)")
    ap.add_argument("--pointer-gap", type=float, default=0.05, help="seconds between pointer events")
    ap.add_argument("--pointer-clicks", type=int, default=1, help="button press+release pairs after the moves")
    ap.add_argument("--pointer-button", default="left", help="InputButton name for --pointer-clicks")
    ap.add_argument("--pointer-rel-step", type=int, default=24, help="--pointer-kind rel: pixels per axis per move")
    # PTRPRESS (S30) — the BUTTON BURST, and it is a different instrument from --pointer-clicks.
    # A click at --pointer-gap is paced so nothing is lost, which is what the XHCIHUB evts clause
    # wants. This burst is paced to be lost: the kernel holds its one-shot stall on the first press
    # edge, the pointer endpoint is dark for the whole of it, and these pairs land inside that
    # window where the device queue is the only thing holding them. The TAIL pairs are injected
    # after the queue has drained, slowly, and are what make a stuck level observable — a level
    # left DOWN turns the tail press into a report byte-identical to its predecessor, which is the
    # only evidence a level-diffed decoder can have that an edge went missing.
    ap.add_argument("--pointer-burst", type=int, default=0, help="PTRPRESS: press+release pairs injected into the stall window (0 = off)")
    ap.add_argument("--pointer-burst-gap", type=float, default=0.02, help="seconds between the burst's individual button events")
    ap.add_argument("--pointer-settle", type=float, default=2.0, help="seconds of quiet after the burst, before the tail pairs")
    ap.add_argument("--pointer-tail", type=int, default=0, help="PTRPRESS: paced press+release pairs after the quiet")
    ap.add_argument("--pointer-tail-gap", type=float, default=0.2, help="seconds between the tail pairs' individual button events")
    # PTRACK (FLAKEFIX, B150) — ACKED delivery. All default OFF: `--pointer-ack 0` is the wall-clock
    # typist exactly, so no pre-existing caller changes behaviour. See the PTRACK block in the header.
    ap.add_argument("--pointer-ack", type=int, default=0, help="PTRACK: reports the GUEST must count; re-send the shortfall until it does (0 = off, wall-clock only)")
    ap.add_argument("--ack-log", default="", help="PTRACK: log to read the guest's count from (default: --marker-log)")
    ap.add_argument("--ack-re", default=r"\[ptrinstall\] installs=\d+ reports=(\d+)", help="PTRACK: regex whose FIRST capture group is the guest's count")
    ap.add_argument("--ack-rounds", type=int, default=3, help="PTRACK: max re-send rounds before giving up loudly")
    ap.add_argument("--ack-settle", type=float, default=0.5, help="PTRACK: quiet seconds after the last report before a count line is authoritative")
    ap.add_argument("--ack-timeout", type=float, default=8.0, help="PTRACK: seconds to wait for a fresh count line in one round")
    ap.add_argument("--ack-gap", type=float, default=0.15, help="PTRACK: seconds between the re-sent reports")
    a = ap.parse_args()

    qmp = Qmp(connect(a.host, a.port, a.connect_timeout))
    # QMPCHECK — READ THE HANDSHAKE'S REPLY, and DIE on a refusal. A fresh QMP monitor is in
    # CAPABILITIES-NEGOTIATION MODE and refuses every command until `qmp_capabilities` succeeds;
    # `Qmp.execute` hands that refusal back as `{"error": ...}` and this line used to throw it away.
    # The cost was measured, not imagined: the QUARRYCLICK fixture (2026-09-17) lost FOUR runs and
    # ~40 injected presses that produced NOT ONE line on the guest's wire, because it inherited this
    # silence. A typist that types into a closed monitor is indistinguishable, on the capture, from a
    # kernel that ignored every key — and the second reading is the one a reader reaches for. Loud
    # here is cheap: nothing has been typed yet, so a `SystemExit` costs the run and no evidence.
    # `qmp_shoot.py:92` has always done both halves; this is that check, in the same shape.
    greeting = qmp._read_obj()
    if "QMP" not in greeting:
        print(f"[qmp] warning: unexpected greeting: {greeting}", file=sys.stderr)
    r = qmp.execute("qmp_capabilities")
    if "error" in r:
        raise SystemExit(f"[qmp] qmp_capabilities failed: {r['error']} — the monitor is still in "
                         f"capabilities-negotiation mode and would refuse every key; nothing typed")

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

    if a.pointer > 0:
        # XHCIHUB — MOVE the pointer, so `evts=` on the `:: XHCIHUB:` line is a fact about the
        # hub-downstream device's interrupt-IN pipe rather than a reading of a pointer nobody
        # touched. Every step is a DISTINCT position (an `abs` device re-reporting the same
        # coordinates delivers nothing), spaced `--pointer-gap` so each report clears the
        # endpoint's polling interval instead of queueing behind the previous one — the same
        # pacing argument the burst block above makes for keys, and the reason the moves are not
        # one atomic `input-send-event`.
        # PTRACK — the step generator, factored out of the loop below so a RE-SENT report is built by
        # the same arithmetic as a first-pass one and simply continues the index: the rel alternation
        # keeps alternating (no retry repeats its predecessor's delta) and an abs step lands on a
        # position the previous one did not (an `abs` device re-reporting the same coordinates
        # delivers nothing at all). `i` is unbounded; the abs fraction wraps.
        def move_step(i):
            if a.pointer_kind == "abs":
                frac = ((i % max(a.pointer, 1)) + 1) / (a.pointer + 1)
                return [move_event("abs", "x", ABS_MAX * frac), move_event("abs", "y", ABS_MAX * (1.0 - frac))]
            step = a.pointer_rel_step if i % 2 == 0 else -a.pointer_rel_step
            return [move_event("rel", "x", step), move_event("rel", "y", step)]

        moves = 0
        for i in range(a.pointer):
            r = qmp.execute("input-send-event", {"events": move_step(i)})
            if "error" in r:
                raise SystemExit(f"[qmp] pointer move {i + 1}/{a.pointer} failed: {r['error']}")
            moves += 1
            time.sleep(a.pointer_gap)
        clicks = 0
        for _ in range(a.pointer_clicks):
            for down in (True, False):
                r = qmp.execute("input-send-event", {"events": [btn_event(a.pointer_button, down)]})
                if "error" in r:
                    raise SystemExit(f"[qmp] pointer button {a.pointer_button!r} down={down} failed: {r['error']}")
                time.sleep(a.pointer_gap)
            clicks += 1
        print(f"[qmp] pointer: {moves} {a.pointer_kind} moves + {clicks} {a.pointer_button} click(s), {a.pointer_gap * 1000:.0f} ms apart", file=sys.stderr)
        # PTRACK — and now stop pacing on the clock. See the PTRACK block in this file's header for
        # the measurement that says why (QEMU folds adjacent motion events into an unpolled queue
        # entry, so a wall-clock typist's report COUNT is a reading of the host's scheduler).
        if a.pointer_ack > 0:
            ack_log = a.ack_log or a.marker_log
            if not ack_log:
                raise SystemExit("[qmp] --pointer-ack needs a log to read the guest's count from (--ack-log or --marker-log)")
            rx = re.compile(a.ack_re.encode())
            sent = moves
            step_i = a.pointer
            rounds = 0
            while True:
                time.sleep(a.ack_settle)
                try:
                    off = os.path.getsize(ack_log)
                except OSError:
                    off = 0
                counted = None
                deadline = time.time() + a.ack_timeout
                while time.time() < deadline:
                    counted, off = ack_count_after(ack_log, rx, off)
                    if counted is not None:
                        break
                    time.sleep(0.2)
                if counted is None:
                    # The guest is not publishing a count, so there is nothing to ack against. Say so
                    # and stop: inventing more reports here would be guessing at the wire.
                    print(f"[qmp] pointer ack: no line matching {a.ack_re!r} in {ack_log} within "
                          f"{a.ack_timeout:.0f}s of the last report — the guest published no count to "
                          f"pace on; {sent} move(s) sent, delivery UNACKED", file=sys.stderr)
                    break
                if counted >= a.pointer_ack:
                    print(f"[qmp] pointer ack: guest counted {counted} of {a.pointer_ack} after "
                          f"{rounds} retry round(s), {sent} move(s) sent", file=sys.stderr)
                    break
                if rounds >= a.ack_rounds:
                    print(f"[qmp] pointer ack: GAVE UP after {rounds} retry round(s) (--ack-rounds) — "
                          f"guest counted {counted} of {a.pointer_ack}, {sent} move(s) sent. The "
                          f"shortfall is real delivery loss; the guest's own count line carries it and "
                          f"the fixture reds on that. Nothing here widens a tolerance.", file=sys.stderr)
                    break
                short = a.pointer_ack - counted
                rounds += 1
                print(f"[qmp] pointer ack: round {rounds}/{a.ack_rounds} — guest counted {counted} of "
                      f"{a.pointer_ack} after {sent} sent; re-sending {short} move(s) "
                      f"{a.ack_gap * 1000:.0f} ms apart", file=sys.stderr)
                for _ in range(short):
                    r = qmp.execute("input-send-event", {"events": move_step(step_i)})
                    if "error" in r:
                        raise SystemExit(f"[qmp] pointer ack re-send {step_i} failed: {r['error']}")
                    step_i += 1
                    sent += 1
                    time.sleep(a.ack_gap)
        # PTRPRESS — the burst, then the quiet, then the tail. One `input-send-event` per edge:
        # QEMU coalesces every event inside ONE call into a single HID report, so a press and a
        # release sent together would be a click the device never emits.
        burst = 0
        for _ in range(a.pointer_burst):
            for down in (True, False):
                r = qmp.execute("input-send-event", {"events": [btn_event(a.pointer_button, down)]})
                if "error" in r:
                    raise SystemExit(f"[qmp] pointer burst {a.pointer_button!r} down={down} failed: {r['error']}")
                time.sleep(a.pointer_burst_gap)
            burst += 1
        if burst:
            print(f"[qmp] pointer burst: {burst} {a.pointer_button} press/release pairs, {a.pointer_burst_gap * 1000:.0f} ms apart", file=sys.stderr)
        if a.pointer_tail > 0:
            time.sleep(a.pointer_settle)
            tail = 0
            for _ in range(a.pointer_tail):
                for down in (True, False):
                    r = qmp.execute("input-send-event", {"events": [btn_event(a.pointer_button, down)]})
                    if "error" in r:
                        raise SystemExit(f"[qmp] pointer tail {a.pointer_button!r} down={down} failed: {r['error']}")
                    time.sleep(a.pointer_tail_gap)
                tail += 1
            print(f"[qmp] pointer tail: {tail} pairs after {a.pointer_settle:.1f}s quiet, {a.pointer_tail_gap * 1000:.0f} ms apart", file=sys.stderr)

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
