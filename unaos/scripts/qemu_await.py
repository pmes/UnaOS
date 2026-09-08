#!/usr/bin/env python3
"""QEMU-FAST: wait until a QEMU run has FINISHED, not until its clock runs out.

Follows a serial capture while QEMU is still writing it and returns as soon as the
verb's OWN checker says the run is over, plus a grace window. Prints exactly one
machine-readable line and exits; it kills nothing and writes nothing into the log —
`unaos/arroyo`'s `qemu_wait_or_complete` owns the kill, the pre-kill hook, and the
`<logfile>.run` sidecar.

THE PREDICATE IS DERIVED, NEVER INVENTED
----------------------------------------
The stop condition is `mbench.Matcher.complete()` — the predicate mbench itself uses
to stop `run_follow()` ("Follow-mode EARLY EXIT") — evaluated over THE SAME SPEC the
calling verb already replays. It fires only once EVERY `REQUIRE` and `COUNT` in that
spec is satisfied AND an end-of-run `COMPLETE` marker has been seen. Two consequences
that make fast mode safe rather than merely quick:

  * a fast capture can never be SHORT OF A REQUIRED WITNESS — completion is defined as
    all of them having landed; and
  * a run carrying a real regression NEVER SHORTENS. The predicate simply never fires,
    the cap is reached, and the whole window is preserved for the replay.

`mbench.py --replay` remains the single verdict authority. This program decides only
WHEN TO STOP READING. It never opens the log for writing.

A verb whose spec declares no `COMPLETE` marker is reported `status=nosignal` and the
caller pays its full wall. Nothing here invents a marker: a gate asserting a line
nobody chose is worse than a slow gate.

THE GRACE WINDOW
----------------
`complete()` says every witness we assert HAS LANDED. It says nothing about a fault
that has not happened yet. So after completion the log is read for `--grace` more
seconds with every FORBID still live, and anything they match in that window lands in
the capture exactly as it would have on the full wall. `forbid_hits=` reports what was
seen there; it does not change the exit — the replay is the authority, and hiding a
hit from it by exiting differently would be this program deciding a verdict.

WHAT FAST MODE COSTS, stated rather than buried: a grace is not a soak. Any MONOTONIC
ACCUMULATOR read out of a fast capture is a FLOOR, not a final value, and a periodic
instrument's window count shrinks with the wall. `UNAOS_QEMU_FULL=1` exists for that
and is the form an arc's DONE gate runs.

OUTPUT (one line, on stdout):
    AWAIT status=<complete|cap|nosignal> complete_at=<s|-> forbid_hits=<n> wall=<s>
EXIT: 0 completed (or nosignal, which is an honest answer), 3 cap reached without
completion, 2 usage/IO error. Every other value means this program broke — the caller
MUST treat a missing or unparseable AWAIT line as "the fast path is broken" and pay the
full wall rather than kill QEMU early on no evidence.
"""

import argparse
import importlib.util
import os
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))


def load_mbench():
    """Import the mbench beside us BY PATH.

    Deliberately not `import mbench`: this is invoked from `arroyo` with an arbitrary
    cwd, and picking up some other `mbench` off `sys.path` would silently change which
    predicate decides when a gate stops reading.
    """
    path = os.path.join(HERE, "mbench.py")
    spec = importlib.util.spec_from_file_location("_qemu_await_mbench", path)
    if spec is None or spec.loader is None:
        raise IOError(f"cannot load {path}")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def main(argv=None):
    ap = argparse.ArgumentParser(description="wait for a QEMU run to finish")
    ap.add_argument("--log", required=True, help="the serial capture QEMU is writing")
    ap.add_argument("--spec", required=True, help="the spec whose COMPLETE markers end the run")
    ap.add_argument("--cap", type=float, required=True, help="hard cap in seconds (the verb's own wall)")
    ap.add_argument("--grace", type=float, default=20.0, help="seconds to keep reading after completion")
    ap.add_argument("--label", default="qemu-run", help="verb name, for the human line on stderr")
    a = ap.parse_args(argv)

    try:
        mb = load_mbench()
    except Exception as e:                       # noqa: BLE001 - report and degrade
        print(f"qemu_await: cannot load mbench: {e}", file=sys.stderr)
        return 2

    try:
        directives = mb.parse_spec(a.spec)
    except Exception as e:                       # noqa: BLE001
        print(f"qemu_await: cannot parse {a.spec}: {e}", file=sys.stderr)
        return 2

    matcher = mb.Matcher(directives)

    # NO COMPLETION SOURCE IS AN ANSWER, NOT AN ERROR. A spec with no COMPLETE marker
    # can never satisfy `complete()`, so waiting on it would just burn the cap and then
    # claim "cap reached" — which reads like a regression and is not one. Say so at once
    # and let the caller pay the wall deliberately.
    if not matcher.markers():
        print("AWAIT status=nosignal complete_at=- forbid_hits=0 wall=0.0")
        return 0

    t0 = time.time()
    deadline = t0 + a.cap
    complete_at = None
    grace_end = None
    forbid_hits_after = 0
    buf = b""
    pos = 0

    while True:
        now = time.time()
        if now >= deadline:
            break
        # Open late and re-open on every pass: QEMU may not have created the file yet,
        # and a file that vanishes under us must not crash the waiter into the caller's
        # "the fast path is broken" branch for a reason that is not a bug.
        try:
            with open(a.log, "rb") as f:
                f.seek(pos)
                chunk = f.read()
                pos = f.tell()
        except FileNotFoundError:
            chunk = b""
        except OSError as e:
            print(f"qemu_await: read error on {a.log}: {e}", file=sys.stderr)
            return 2

        if chunk:
            buf += chunk
            # Only COMPLETE lines are fed. A trailing partial line is held back until
            # its newline arrives, so a marker split across two reads is never missed
            # and never matched twice.
            *lines, buf = buf.split(b"\n")
            for raw in lines:
                for d, _text in matcher.feed_raw(raw):
                    if d.kind == "FORBID" and complete_at is not None:
                        forbid_hits_after += 1

        if complete_at is None and matcher.complete():
            complete_at = time.time() - t0
            grace_end = min(time.time() + a.grace, deadline)
            print(f"⚡ {a.label}: run complete at +{complete_at:.1f}s — holding "
                  f"{a.grace:.0f}s grace with every FORBID live.", file=sys.stderr)

        if grace_end is not None and time.time() >= grace_end:
            break

        # 0.2 s: fast enough that the grace window is measured rather than rounded, slow
        # enough that a 300 s cap costs ~1500 short reads instead of a spin.
        time.sleep(0.2)

    wall = time.time() - t0
    if complete_at is None:
        print(f"AWAIT status=cap complete_at=- forbid_hits=0 wall={wall:.1f}")
        return 3
    print(f"AWAIT status=complete complete_at={complete_at:.1f} "
          f"forbid_hits={forbid_hits_after} wall={wall:.1f}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
