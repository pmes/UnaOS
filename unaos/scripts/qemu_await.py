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

TWO MODES, ONE PREDICATE
------------------------
The default mode above TAILS a log a live QEMU is still writing, and its product is a
STOPPING DECISION. `--settled` asks the same `complete()` predicate of a capture that is
already over, and its product is a FACT ABOUT THE RUN: did it reach its end-of-run
marker? `--settled` exists because the STOPPING DECISION and the VERDICT are different questions — the tail (now run from `builder/src/main.rs`'s `qemu_test_wall`, FASTTEST 2026-09-15) decides when to stop reading, and `--settled` asks the finished capture whether the end of the run is in it, which is the only reading a pass may rest on.

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


def settled(a, matcher):
    """SETTLED MODE — ask the SAME predicate of a capture that is already over.

    `--settled` exists because one QEMU verb does not own its own QEMU: `./arroyo test`
    hands the wall to `builder/src/main.rs`, which spawns QEMU, sleeps the whole
    `UNAOS_TEST_SECS` and kills it. Nothing in `arroyo` is holding that process, so the
    tailing path above has nothing to shorten — but the QUESTION the tail answers is
    exactly the question that verb's verdict was missing: DID THIS RUN REACH ITS END?

    So this mode reads the finished log once and answers only that. It decides no
    verdict and it shortens no wall; it reports whether an end-of-run marker landed, and
    the caller turns that into `-> TRUNCATED` or hands the log to its own fault scan.
    The marker is still `mbench`'s, still read off the spec the caller names — this is
    the same derived-never-invented rule as the tail, evaluated after the fact.

    OUTPUT (one line, on stdout; every value is space-free so the caller's awk split
    on `key=value` fields cannot be broken by guest text):
        SETTLED status=<complete|truncated|nosignal> complete_at=<logline|-> \
forbid_hits=0 wall=0.0 [reason=<why>] [stopped_at=<logline>]
    The line the capture stopped on is quoted VERBATIM on stderr instead, where it can
    carry spaces without turning the machine line into a parsing hazard.
    EXIT: 0 reached a marker (or nosignal, an honest answer), 3 the capture never
    reached one, 2 the log could not be read.
    """
    try:
        with open(a.log, "rb") as f:
            data = f.read()
    except FileNotFoundError:
        # A MISSING LOG IS NOT A FINISHED RUN. Reported truncated, never complete: the
        # caller's `-> TRUNCATED` is the honest verdict for "there is nothing here".
        print("SETTLED status=truncated complete_at=- forbid_hits=0 wall=0.0 reason=no-log")
        return 3
    except OSError as e:                             # noqa: BLE001
        print(f"qemu_await: read error on {a.log}: {e}", file=sys.stderr)
        return 2

    # An UNTERMINATED tail (no closing newline) is direct evidence the writer was killed
    # mid-write — mbench's own truncation signal, set here for the same reason it sets it
    # in run_replay. The final fragment is still FED, so a marker that landed whole but
    # lost its newline to the kill is not thrown away.
    if data and not data.endswith(b"\n"):
        matcher.unterminated = True
    lines = data.split(b"\n")
    if lines and lines[-1] == b"":
        lines.pop()
    for raw in lines:
        matcher.feed_raw(raw)

    # THE PREDICATE IS `complete()`, NOT THE MARKER LIST. This asked `markers()` alone
    # until 2026-09-15 (LADDERTAIL), which made every REQUIRE and COUNT in a settled
    # spec INERT: `./arroyo test` read this status, so a positive witness added to
    # `x86-test.spec` sat in the file looking load-bearing while being incapable of
    # reddening anything — LAWS §5's "a check that cannot fire is an absent one", with
    # the docstring above promising the opposite. Both modes now score the same three
    # things, which is the whole claim of TWO MODES, ONE PREDICATE.
    seen = [d for d in matcher.markers() if d.hits]
    if seen and matcher.complete():
        first = min(seen, key=lambda d: d.first_lineno)
        print(f"SETTLED status=complete complete_at={first.first_lineno} "
              f"forbid_hits=0 wall=0.0")
        return 0
    # REACHED THE END, SHORT OF A WITNESS is a HOLE IN THE LADDER, not a short wall, and
    # the two must not read alike. The status stays within the caller's enum — anything
    # outside it is reported as a broken checker — so the distinction rides a `reason=`
    # field, space-free like every other value here, with the short patterns on stderr
    # where they may carry spaces.
    short = [d for d in matcher.directives
             if d.kind in ("REQUIRE", "COUNT") and not d.satisfied()]
    if seen and short:
        print(f"qemu_await: {a.label}: the capture REACHED its end-of-run marker at line "
              f"{min(d.first_lineno for d in seen)} but {len(short)} required witness(es) "
              f"never printed: " + " | ".join(d.pattern for d in short[:5]), file=sys.stderr)
        print(f"SETTLED status=truncated complete_at=- forbid_hits=0 wall=0.0 "
              f"reason=short-witnesses:{len(short)} stopped_at={matcher.last_lineno}")
        return 3
    # WHERE IT STOPPED is the useful half of a truncation report — the boot phase the
    # capture died in, quoted verbatim, so the reader is not sent back to the log to
    # find out how far it got. On STDERR: guest text contains spaces, and a machine line
    # whose last field can swallow the parse is a hazard, not a convenience.
    print(f"qemu_await: {a.label}: the capture stops at line {matcher.last_lineno}: "
          f"{matcher.last_text[:200]}", file=sys.stderr)
    print(f"SETTLED status=truncated complete_at=- forbid_hits=0 wall=0.0 "
          f"stopped_at={matcher.last_lineno}")
    return 3


def main(argv=None):
    ap = argparse.ArgumentParser(description="wait for a QEMU run to finish")
    ap.add_argument("--log", required=True, help="the serial capture QEMU is writing")
    ap.add_argument("--spec", required=True, help="the spec whose COMPLETE markers end the run")
    ap.add_argument("--cap", type=float, default=None, help="hard cap in seconds (the verb's own wall)")
    ap.add_argument("--grace", type=float, default=20.0, help="seconds to keep reading after completion")
    ap.add_argument("--quiet", type=float, default=1.0,
                    help="QUIETKILL: after the grace, do not report completion (so the caller does not kill) until the "
                         "capture has grown by no byte for this many seconds AND its tail ends in a newline; bounded by --cap")
    ap.add_argument("--label", default="qemu-run", help="verb name, for the human line on stderr")
    ap.add_argument("--settled", action="store_true",
                    help="the capture is FINISHED: read it once and report whether the run "
                         "reached a COMPLETE marker (see SETTLED MODE in the module docstring)")
    a = ap.parse_args(argv)
    if not a.settled and a.cap is None:
        ap.error("--cap is required unless --settled is given")

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
        print(f"{'SETTLED' if a.settled else 'AWAIT'} status=nosignal complete_at=- "
              f"forbid_hits=0 wall=0.0")
        return 0

    # SETTLED MODE: the same predicate, asked of a capture that is already over.
    if a.settled:
        return settled(a, matcher)

    t0 = time.time()
    deadline = t0 + a.cap
    complete_at = None
    grace_end = None
    forbid_hits_after = 0
    buf = b""
    pos = 0
    last_growth = time.time()   # QUIETKILL: when the capture last grew
    quiet_waited = 0.0

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
            last_growth = time.time()
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
            # QUIETKILL (QUEUE.md §5, rmbp gate9a 2026-09-22): the grace is a SOAK for FORBIDs, not a
            # promise that the guest has stopped writing. On a box at load 34 the kill fired after the
            # marker and the 20 s grace while the guest was mid-line (`[click2] depth gui_chan=0 (se`),
            # and the truncation rule then refused a boot that had printed every witness — a false red.
            # So the stopping decision also asks: has the capture gone QUIET (no byte for --quiet
            # seconds) AND does its tail end in a newline (`buf` holds the unterminated remainder)?
            # Both, or keep reading — bounded by --cap, which the deadline test above still enforces.
            # mbench's mid-line check downstream stays; it now fires only for a writer that really
            # died mid-write, not for one the harness killed there.
            if not buf and (time.time() - last_growth) >= a.quiet:
                break
            quiet_waited = time.time() - grace_end

        # 0.2 s: fast enough that the grace window is measured rather than rounded, slow
        # enough that a 300 s cap costs ~1500 short reads instead of a spin.
        time.sleep(0.2)

    wall = time.time() - t0
    if complete_at is None:
        print(f"AWAIT status=cap complete_at=- forbid_hits=0 wall={wall:.1f}")
        return 3
    if quiet_waited > 0.0:
        print(f"⚡ {a.label}: QUIETKILL held the kill {quiet_waited:.1f}s past the grace until the capture went quiet "
              f"({a.quiet:.1f}s without a byte) on a newline-terminated tail"
              + (" — cap reached while still writing" if time.time() >= deadline else ""), file=sys.stderr)
    print(f"AWAIT status=complete complete_at={complete_at:.1f} "
          f"forbid_hits={forbid_hits_after} wall={wall:.1f} quiet_held={quiet_waited:.1f}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
