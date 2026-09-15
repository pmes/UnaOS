#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 The Architect & Una
#
# FTDIRX (rmbp A9 / LEDGER S29, x86 half) — TYPE AT THE EMULATED FTDI CABLE.
#
# WHY THIS EXISTS. The U2.5 console gate attaches QEMU's `-device usb-serial` (an FT232 model) with a
# `file` chardev, and a file cannot be written INTO — so there has never been a way to send bytes
# TOWARD the kernel. `UNAOS_FTDIRX_INJECT=<path>` (builder/src/main.rs) swaps that file for a
# listening UNIX socket, and this script is the other end of it. On metal none of this exists: the
# cable is the chardev and the operator is the injector.
#
# CONNECT FIRST, THEN WAIT — AND THE ORDER IS NOT A PREFERENCE. Measured 2026-09-15, and it cost a
# whole fixture run to find: QEMU's `usb_serial_realize` attaches the USB device only
# `if (qemu_chr_fe_backend_open(&s->cs))`, and a `socket,server=on,wait=off` backend is NOT open
# until a peer connects. So with nobody on the socket the FT232 is never plugged in at all — the
# kernel enumerates two devices instead of three, there is no `FTDI USB-SERIAL DETECTED` line, and
# the console-up witness this script used to wait for BEFORE connecting could never arrive. That is
# a deadlock, and it looks exactly like a broken RX path. The connection is the plug; make it first.
#
# THE WITNESS WAIT IS STILL THE POINT, and it is not a sleep. After connecting, `--wait-for-log`
# polls the SERIAL log — the kernel's 16550, a different channel from the cable — for
# `:: U2.5: FTDI console up`, printed at the end of `service_ftdi`'s bring-up immediately before
# `ftdi::set_live(true)`. Bytes written earlier are NOT lost (the FT232 holds them in its RX FIFO),
# but a run that typed into a console which never came up would be indistinguishable from a run
# whose RX path is broken, and this gate must be able to tell those apart.
#
# HOLD THE LINE AFTER SENDING. Closing the socket raises CHR_EVENT_CLOSED, which DETACHES the
# emulated device — a hot-unplug in the middle of the run. `--hold` keeps the socket open and
# draining until the run is over, so the fixture measures RX and not teardown. Everything the FT232
# writes out comes back over this same socket and is saved to `--capture`, which is the cable-side
# twin of `target/ftdi.log` and the only place the cable's own bytes are recorded in this mode.
#
# Usage:
#   scripts/ftdi_inject.py <socket> --text 'help\n' [--after SECS] [--wait-for-log PATH]
#                                   [--wait-for-text TEXT] [--hold SECS] [--capture PATH]
#                                   [--timeout SECS] [--gap MS]
#
# THE EXACT ENV, both halves, because neither works alone (run them from `unaos/`, injector FIRST —
# it retries the connect while the kernel builds, and the connect is what plugs the FT232 in):
#
#   SOCK=<abs path>/ftdi0.sock
#   python3 scripts/ftdi_inject.py "$SOCK" \
#       --wait-for-log "$PWD/target/serial.log" --text 'help\n' \
#       --hold 90 --timeout 2400 --capture <abs path>/cable.log &
#   UNAOS_USBSERIAL=1 UNAOS_FTDIRX=1 UNAOS_FTDIRX_INJECT="$SOCK" \
#   UNAOS_WC=1 UNAOS_QEMU_FULL=1 ./arroyo test 90
#
#   UNAOS_USBSERIAL=1     attaches the emulated FT232 at all (a BUILDER knob, not a kernel feature)
#   UNAOS_FTDIRX=1        compiles the RX half — without it the cable is write-only and nothing types
#   UNAOS_FTDIRX_INJECT=  swaps the file chardev for the listening socket this script connects to;
#                         NOTE it REPLACES target/ftdi.log, so --capture is the cable's only record
#
# RBTDRAIN (rmbp-ledger A3) adds `--wait-for-text` and two more env terms, for a verb that ENDS THE
# RUN (`reboot`). Same two commands, with:
#
#       --wait-for-text ':: zeolite: metrics' --text 'reboot\n'
#   ... UNAOS_QEMU_EXTRA="-no-reboot" ./arroyo test 90
#
#   --wait-for-text       hold off until the boot has reached the COMPLETE marker of
#                         scripts/specs/x86-test.spec. Typing `reboot` at console-up instead resets
#                         the machine long before that marker, and `./arroyo test` then reports
#                         TRUNCATED — correctly, and for a reason unrelated to what is being measured.
#   UNAOS_QEMU_EXTRA=-no-reboot   the x86 builder's QEMU line does NOT carry -no-reboot (the two
#                         aarch64 lines in `arroyo` do), so without this the reset RESTARTS the guest
#                         and the capture's tail is a second boot instead of the reboot ladder. With
#                         it, the reset EXITS QEMU and the last bytes on the cable are the ladder.
#
# Exit status: 0 = every byte written; 1 = the socket never accepted a connection, or the console-up
# witness never appeared, within the timeout. The gate reads this, so it must never be 0 on a no-op.

import argparse
import socket
import sys
import threading
import time

CONSOLE_UP = ":: U2.5: FTDI console up"


def log(msg: str) -> None:
    print(f"[ftdi_inject] {msg}", flush=True)


def connect(sock_path: str, deadline: float) -> socket.socket:
    """QEMU creates the socket at startup, but this script is normally launched BEFORE QEMU has been
    exec'd at all — the gate starts both from one shell, and the kernel build runs first. Retry until
    the deadline rather than racing. This connection is what plugs the FT232 in (see the header)."""
    while time.time() < deadline:
        try:
            s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            s.connect(sock_path)
            return s
        except (FileNotFoundError, ConnectionRefusedError, OSError):
            time.sleep(0.25)
    raise TimeoutError(f"no connection to {sock_path}")


def reader_thread(s: socket.socket, capture_path: str, stop: threading.Event) -> None:
    """Drain the cable continuously so the kernel's awaited bulk-OUT pump is never backpressured by a
    peer that only reads at exit, and record what came back."""
    try:
        with open(capture_path, "wb", buffering=0) as cap:
            while not stop.is_set():
                try:
                    data = s.recv(4096)
                except (socket.timeout, TimeoutError):
                    continue
                except OSError:
                    return
                if not data:
                    return
                cap.write(data)
    except OSError:
        return


def wait_for_text(path: str, needle: str, deadline: float) -> bool:
    while time.time() < deadline:
        try:
            with open(path, "rb") as fh:
                if needle.encode() in fh.read():
                    return True
        except FileNotFoundError:
            pass
        time.sleep(0.25)
    return False


def wait_for_witness(path: str, deadline: float) -> bool:
    return wait_for_text(path, CONSOLE_UP, deadline)


def main() -> int:
    ap = argparse.ArgumentParser(description="Type bytes at the emulated FTDI console.")
    ap.add_argument("socket", help="UNIX socket path given to UNAOS_FTDIRX_INJECT")
    ap.add_argument("--text", default="help\n",
                    help=r"text to send; \n and \r are interpreted (default: 'help\n')")
    ap.add_argument("--after", type=float, default=0.0,
                    help="extra settle time AFTER the console-up witness, in seconds")
    ap.add_argument("--wait-for-log", default=None,
                    help="serial log to poll for '%s'; omit to skip the wait" % CONSOLE_UP)
    # RBTDRAIN (rmbp-ledger A3): console-up is the earliest moment the cable can carry anything, and
    # for an RX gate that is also the right moment to type. A gate whose typed verb ENDS THE RUN needs
    # a later one: `reboot` at console-up resets the machine long before the boot reaches the COMPLETE
    # marker `./arroyo test` scores, and the harness would call that a TRUNCATED run — correctly, and
    # for a reason that has nothing to do with what the fixture was measuring. This flag names a second
    # string to wait for in the SAME log, polled after the console-up witness, so the fixture can say
    # "once the boot has finished, type this". Additive: unset, the script behaves byte-for-byte as before.
    ap.add_argument("--wait-for-text", default=None,
                    help="after the console-up witness, also wait for this literal text in "
                         "--wait-for-log (e.g. the spec's COMPLETE marker) before sending")
    ap.add_argument("--hold", type=float, default=0.0,
                    help="seconds to keep the socket open after sending (a close hot-unplugs the "
                         "emulated device, so hold past the end of the run)")
    ap.add_argument("--capture", default=None,
                    help="file for everything the cable sends back (default: <socket>.cable.log)")
    ap.add_argument("--timeout", type=float, default=600.0,
                    help="wall budget for connect + witness (default 600 s)")
    ap.add_argument("--gap", type=float, default=40.0,
                    help="milliseconds between bytes (default 40 — a fast typist, and well over the "
                         "FT232's 16 ms latency timer, so each byte gets its own packet)")
    args = ap.parse_args()

    text = args.text.replace("\\n", "\n").replace("\\r", "\r").replace("\\t", "\t")
    capture = args.capture or (args.socket + ".cable.log")
    deadline = time.time() + args.timeout

    log(f"connecting to {args.socket} (the connection is what plugs the FT232 in)")
    try:
        s = connect(args.socket, deadline)
    except TimeoutError as e:
        log(f"FAIL: {e}")
        return 1
    log("connected")
    s.settimeout(1.0)
    stop = threading.Event()
    rt = threading.Thread(target=reader_thread, args=(s, capture, stop), daemon=True)
    rt.start()
    log(f"cable capture -> {capture}")

    if args.wait_for_log:
        log(f"waiting for '{CONSOLE_UP}' in {args.wait_for_log}")
        if not wait_for_witness(args.wait_for_log, deadline):
            log("FAIL: the console-up witness never appeared — nothing injected")
            stop.set()
            s.close()
            return 1
        log("console is up")
    if args.wait_for_text:
        if not args.wait_for_log:
            log("FAIL: --wait-for-text needs --wait-for-log to name the log to poll")
            stop.set()
            s.close()
            return 1
        log(f"waiting for {args.wait_for_text!r} in {args.wait_for_log}")
        if not wait_for_text(args.wait_for_log, args.wait_for_text, deadline):
            log("FAIL: that text never appeared — nothing injected")
            stop.set()
            s.close()
            return 1
        log("text seen")
    if args.after > 0:
        time.sleep(args.after)

    log(f"sending {len(text)} byte(s): {text!r}")
    try:
        for ch in text.encode():
            s.sendall(bytes([ch]))
            time.sleep(args.gap / 1000.0)
    except OSError as e:
        log(f"FAIL: send: {e}")
        stop.set()
        s.close()
        return 1
    log("sent")

    if args.hold > 0:
        log(f"holding the line for {args.hold}s (a close would hot-unplug the device)")
        time.sleep(args.hold)
    stop.set()
    s.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
