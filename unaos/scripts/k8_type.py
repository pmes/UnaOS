#!/usr/bin/env python3
"""k8_type.py — TYPE at the Pi 4 QEMU shell, over a bidirectional chardev socket.

WHY THIS EXISTS. `arroyo kernel8-test` attaches the PL011 (UART0) with
`-serial file:<log>`, which is WRITE-ONLY: the guest can talk, nothing can talk
back. That is why the Pi's regression suite has never been able to witness a
single thing the operator TYPES — every interactive path (bare-name launch, the
unknown-command refusal, `jobs`) was reachable only by hand-driving QEMU.
`exec-barename` proved the bare name launches by doing exactly that by hand
(docs/dev/OS/08_VIDEO/PARITY.md §6.6a-closed) and said so plainly: "the Pi's
regression suite still cannot type, so none of the above is *gated*". This
script is the standing half of that proof.

WHAT IT DOES. `test_kernel8()` in `arroyo`, when `UNAOS_K8_SCRIPT` is set,
swaps the write-only file chardev for

    -chardev socket,id=k8u0,host=127.0.0.1,port=<P>,server=on,wait=off,logfile=<log>
    -serial chardev:k8u0

which is BIDIRECTIONAL and still writes the very same `<log>` (QEMU's own
chardev `logfile=`), so mbench replays a byte-for-byte equivalent capture and
the transcript path is unchanged. This script is the socket's peer: it connects,
watches the stream for the shell to reach steady state, and types.

SCRIPT GRAMMAR — deliberately the SAME grammar `mbench.py`'s `Injector` reads
for the metal bridge (`--inject`/`--script`), so one file drives QEMU and metal:

    # comment
    SLEEP <secs>            wait unconditionally
    WAIT  <secs> <regex>    wait until the guest prints <regex> (or time out, loudly)
    <anything else>         type it, character by character, then CR

TYPING IS PACED (default 50 ms/char, `--keydelay`) because this is a real UART
into a real line discipline, not a paste buffer; and a CR is sent as `\\r`, which
is what the shell's reader expects.

DRAINING IS NOT OPTIONAL. The peer must keep READING for the whole run: a socket
chardev whose peer stops consuming backs up, and the guest's console writes then
block or drop — which would corrupt the very capture we are asserting. So after
the last scripted step this process keeps draining until it is killed or
`--budget` expires; it never exits early on its own.

EXIT: 0 when the script ran to completion (WAIT timeouts are reported but do not
fail here — the VERDICT belongs to mbench and the spec, never to the typist).
2 when the socket never came up at all, which is a harness fault, not a result.
"""

import argparse
import re
import socket
import sys
import time


def parse_script(path):
    """Same grammar as mbench.Injector._parse — see the module docstring."""
    steps = []
    with open(path, "rb") as f:
        for i, raw in enumerate(f.read().splitlines(), 1):
            line = raw.decode("utf-8", errors="replace").strip()
            if not line or line.startswith("#"):
                continue
            parts = line.split(None, 1)
            head = parts[0].upper()
            if head == "SLEEP":
                if len(parts) != 2:
                    sys.exit(f"k8_type: script line {i}: SLEEP wants '<secs>'")
                steps.append(("SLEEP", float(parts[1])))
            elif head == "WAIT":
                sub = parts[1].split(None, 1) if len(parts) > 1 else []
                if len(sub) != 2:
                    sys.exit(f"k8_type: script line {i}: WAIT wants '<secs> <regex>'")
                steps.append(("WAIT", (float(sub[0]), re.compile(sub[1]))))
            else:
                steps.append(("CMD", line))
    return steps


class Stream:
    """The receive side: drains the socket and answers 'has the guest said X yet'.

    Keeps a bounded TAIL rather than the whole boot (a pi4 boot is megabytes and
    the WAIT patterns are all near-term), and rescans only from a cursor with a
    small overlap so a pattern straddling two reads is still found."""

    TAIL = 1 << 18          # 256 KiB of context is far more than any WAIT needs
    OVERLAP = 4096

    def __init__(self, sock):
        self.sock = sock
        self.buf = ""
        self.cursor = 0

    def pump(self, timeout=0.2):
        self.sock.settimeout(timeout)
        try:
            data = self.sock.recv(65536)
        except socket.timeout:
            return
        except OSError:
            return
        if not data:
            return
        self.buf += data.decode("utf-8", errors="replace")
        if len(self.buf) > self.TAIL:
            drop = len(self.buf) - self.TAIL
            self.buf = self.buf[drop:]
            self.cursor = max(0, self.cursor - drop)

    def seen(self, rx):
        start = max(0, self.cursor - self.OVERLAP)
        if rx.search(self.buf, start):
            return True
        self.cursor = len(self.buf)
        return False


def connect(host, port, timeout):
    """QEMU binds the listening socket some way into its own startup, so retry."""
    deadline = time.monotonic() + timeout
    last = None
    while time.monotonic() < deadline:
        try:
            s = socket.create_connection((host, port), timeout=2.0)
            s.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
            return s
        except OSError as e:
            last = e
            time.sleep(0.2)
    print(f"k8_type: could not connect to {host}:{port} within {timeout}s ({last})",
          file=sys.stderr, flush=True)
    return None


def main(argv=None):
    ap = argparse.ArgumentParser(
        prog="k8_type",
        description="Type a script at the raspi4b QEMU shell over a chardev socket.")
    ap.add_argument("--port", type=int, required=True, help="chardev socket TCP port")
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--script", required=True, help="inject script (mbench grammar)")
    ap.add_argument("--keydelay", type=float, default=0.05,
                    help="seconds between typed characters (default 0.05)")
    ap.add_argument("--settle", type=float, default=2.0,
                    help="seconds to wait after each typed line (default 2.0)")
    ap.add_argument("--connect-timeout", type=float, default=30.0)
    ap.add_argument("--budget", type=float, default=3600.0,
                    help="hard stop; normally the harness kills us first")
    args = ap.parse_args(argv)

    steps = parse_script(args.script)
    sock = connect(args.host, args.port, args.connect_timeout)
    if sock is None:
        return 2
    print(f"  • k8_type: attached to {args.host}:{args.port}, {len(steps)} step(s) "
          f"from {args.script}", flush=True)

    stream = Stream(sock)
    t0 = time.monotonic()

    def idle(secs):
        end = time.monotonic() + secs
        while time.monotonic() < end:
            stream.pump(0.1)

    for kind, arg in steps:
        if time.monotonic() - t0 > args.budget:
            print("  ⚠ k8_type: budget exhausted before the script finished", flush=True)
            break
        if kind == "SLEEP":
            idle(arg)
        elif kind == "WAIT":
            secs, rx = arg
            end = time.monotonic() + secs
            hit = False
            while time.monotonic() < end:
                stream.pump(0.2)
                if stream.seen(rx):
                    hit = True
                    break
            dt = time.monotonic() - t0
            if hit:
                print(f"  • k8_type: saw /{rx.pattern}/ at t={dt:.1f}s", flush=True)
            else:
                # Not fatal HERE on purpose: the verdict is mbench's, and a missing
                # REQUIRE downstream is a far more informative red than a typist
                # that gave up. Type anyway — the shell may well be ready.
                print(f"  ⚠ k8_type: WAIT timed out on /{rx.pattern}/ after {secs}s "
                      f"— typing anyway", flush=True)
        else:
            dt = time.monotonic() - t0
            print(f"  → k8_type: typing {arg!r} at t={dt:.1f}s", flush=True)
            try:
                for ch in arg.encode("utf-8"):
                    sock.send(bytes([ch]))
                    idle(args.keydelay)
                sock.send(b"\r")
            except OSError as e:
                print(f"  ⚠ k8_type: send failed: {e}", flush=True)
                break
            idle(args.settle)

    print("  • k8_type: script complete — draining until the harness stops us",
          flush=True)
    # See the module docstring: a peer that stops reading corrupts the capture.
    while time.monotonic() - t0 < args.budget:
        stream.pump(0.5)
    return 0


if __name__ == "__main__":
    sys.exit(main())
