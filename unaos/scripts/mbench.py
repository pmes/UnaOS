#!/usr/bin/env python3
"""
mbench.py — the metal-bench harness: capture + assert + diff for UnaOS serial logs.

The metal analog of `./arroyo battery` (see arroyo's battery()/_step for the assert
idiom this mirrors): read a serial capture — either a finished log (--replay) or a
live bridge log (--follow) — and ASSERT it against a checked-in witness spec,
printing one battery-style verdict table and exiting
0 (PASS) / 1 (FAIL) / 2 (usage/spec error) / 3 (TRUNCATED — inconclusive).

It automates the capture+assert+diff toil of an attended bench. It does NOT touch
serial devices: the *-serial-bridge.py scripts own the port on one never-reopened
fd (macOS resets baud on reopen); mbench only reads the bridge's LOG FILE
(~/rmbp-serial.log, ~/pi-serial.log, ~/jetson-serial.log — symlinks the
*-bench-connect.sh scripts refresh) or a QEMU log (unaos/target/serial*.log —
replay mode doubles as the QEMU-log checker).

Spec format (unaos/scripts/specs/*.spec) — one directive per line, `#` comments:

    REQUIRE <regex>       must match >= 1 line, else FAIL
    COUNT <n> <regex>     must match >= n lines, else FAIL
    OPTIONAL <regex>      reported, never fails
    FORBID <regex>        any match = FAIL (reported instantly in follow mode)
    PENDING <regex>       a witness that needs metal/code not yet flashed —
                          reported as an hourglass, never fails; lets a spec ship
                          ahead of its bench. A PENDING that DOES match is flagged
                          for promotion to REQUIRE.
    COMPLETE <regex>      an END-OF-RUN MARKER (see below). Never counted as a
                          required witness, never fails on its own.

Default FORBID set (always on, per the battery's global FAIL scan):
    "-> FAIL"   "FAIL ::"   "PANIC"

TRUNCATION — the third verdict, and why it exists
-------------------------------------------------
A missing REQUIRE has two completely different causes that the old PASS/FAIL table
could not tell apart:

  * the boot RAN TO COMPLETION and the witness is genuinely absent — a REGRESSION;
  * the CAPTURE STOPPED before the run got there — the log is simply short.

The second is the pi4 gate's standing false-green trap: `./arroyo kernel8-test` at
too small a window kills QEMU mid-boot, the tail witnesses never get a chance to
print, and the shortfall reads as a regression in arcs that touched nothing (four
executors lost time to exactly this in one round; a CLEAN base reproduced the same
shortfall). A tool that answers "FAIL" there is lying with a straight face.

So a spec may declare one or more `COMPLETE <regex>` END-OF-RUN MARKERS: lines the
platform already emits once the run has gone far enough that every witness in the
spec has had its chance. A capture is TRUNCATED when the spec declares markers and
either

  * NONE of them matched, or
  * the capture ends MID-LINE (no terminating newline — direct evidence the writer
    was killed in the middle of a write).

Verdict precedence — `run_verdict()` is the single authority:

  1. any FORBID hit               -> FAIL       (exit 1)  positive evidence of a
                                                          fault; truncation never
                                                          excuses a PANIC
  2. else, capture is TRUNCATED   -> TRUNCATED  (exit 3)  INCONCLUSIVE: not a pass
                                                          and not a regression
  3. else, any REQUIRE/COUNT short-> FAIL       (exit 1)  a real regression
  4. else                         -> PASS       (exit 0)

Specs that declare NO `COMPLETE` line are byte-for-byte unaffected: rule 2 can never
fire for them, so x86/arm/jetson replays keep their exact PASS/FAIL semantics and
exit codes. PASS itself is unchanged everywhere — all REQUIRE/COUNT satisfied and
zero forbidden hits, exactly as before.

Matching is binary-safe: lines are read as BYTES, decoded UTF-8 errors='replace',
then stripped of ANSI escapes and C0 control bytes before regex matching (serial
logs carry control bytes — the reason plain grep is banned on them).

Modes:
    --replay <log> --spec <file>                     static log -> verdict table
    --follow <log> --spec <file> [--timeout <s>]     live tail; prints each witness
                                                     as it lands; final table at
                                                     timeout or completion (all
                                                     REQUIRE + COUNT satisfied, and
                                                     an END-OF-RUN MARKER seen when
                                                     the spec declares one — so a
                                                     follow can never stop early and
                                                     then call its own capture short)
    --self-test                                      no hardware: canned lines incl.
                                                     control bytes through replay
                                                     AND follow, asserts verdicts
    --inject <fifo> --script <cmds> --platform <p>   (with --follow) write commands
                                                     to a bridge inject FIFO with a
                                                     trailing \r + settle delay.
                                                     pi/jetson ONLY — x86/rmbp is
                                                     TX-only (FTDI bulk-IN is a
                                                     kernel stub): REFUSED.

Inject script format — one shell command per line, `#` comments, plus:
    SLEEP <secs>              pause between commands
    WAIT <secs> <regex>       block until the followed log matches <regex>
                              (bounded by <secs>; lets a script wait for boot)

Typical bench:  ./arroyo mbench --follow ~/pi-serial.log \
                    --spec scripts/specs/pi4-regression.spec --timeout 120
"""

import argparse
import contextlib
import io
import os
import re
import sys
import tempfile
import threading
import time

# ---------------------------------------------------------------------------
# Line hygiene: serial logs carry control bytes (ANSI escapes, NULs, bare CRs).
# All matching happens on a cleaned decoded string; raw bytes never meet regex.
# ---------------------------------------------------------------------------

ANSI_RE = re.compile(r"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)?|\x1b[@-_]")
CTRL_RE = re.compile(r"[\x00-\x08\x0b-\x1f\x7f]")  # C0 minus \t \n; \r handled at split


def clean_line(raw: bytes) -> str:
    text = raw.decode("utf-8", errors="replace")
    text = ANSI_RE.sub("", text)
    text = CTRL_RE.sub("", text)
    return text


# ---------------------------------------------------------------------------
# Spec parsing
# ---------------------------------------------------------------------------

DEFAULT_FORBIDS = [r"-> FAIL", r"FAIL ::", r"PANIC"]

KIND_ORDER = {"COMPLETE": 0, "REQUIRE": 1, "COUNT": 2, "PENDING": 3,
              "OPTIONAL": 4, "FORBID": 5}

GLYPH = {"ok": "✅", "fail": "❌", "pending": "⏳", "info": "◦", "cut": "✂️"}

# Exit codes. 0/1/2 are historical; 3 is the truncation verdict.
RC_PASS = 0
RC_FAIL = 1
RC_ERROR = 2
RC_TRUNCATED = 3


class Directive:
    def __init__(self, kind, pattern, need=1, builtin=False, spec_line=0):
        self.kind = kind          # REQUIRE | COUNT | OPTIONAL | FORBID | PENDING | COMPLETE
        self.pattern = pattern    # regex source, as written in the spec
        self.need = need          # threshold (COUNT); 1 otherwise
        self.builtin = builtin    # default FORBID, not from the spec file
        self.spec_line = spec_line
        try:
            self.rx = re.compile(pattern)
        except re.error as e:
            raise SpecError(f"line {spec_line}: bad regex {pattern!r}: {e}")
        self.hits = 0
        self.first_lineno = None
        self.first_text = None

    def feed(self, text, lineno):
        """Returns True if this line matched this directive."""
        if not self.rx.search(text):
            return False
        self.hits += 1
        if self.first_lineno is None:
            self.first_lineno = lineno
            self.first_text = text.strip()
        return True

    # --- verdict -----------------------------------------------------------

    def satisfied(self):
        """For REQUIRE/COUNT: threshold met. Others never gate completion.

        COMPLETE deliberately answers True here so it can never be counted into the
        `got/len(req)` witness tally — the pi4 gate's PASS is 63/63 and adding an
        end-of-run marker must not silently make it 64/64."""
        if self.kind == "REQUIRE":
            return self.hits >= 1
        if self.kind == "COUNT":
            return self.hits >= self.need
        return True

    def failed(self):
        """COMPLETE never fails a run BY ITSELF — an absent marker is the TRUNCATED
        verdict (rule 2 of run_verdict), which is neither a pass nor a regression."""
        if self.kind in ("REQUIRE", "COUNT"):
            return not self.satisfied()
        if self.kind == "FORBID":
            return self.hits > 0
        return False

    def label(self):
        k = self.kind
        if self.kind == "COUNT":
            k = f"COUNT>={self.need}"
        if self.builtin:
            k += "*"
        return k

    def glyph(self):
        if self.failed():
            return GLYPH["fail"]
        if self.kind == "COMPLETE":
            return GLYPH["ok"] if self.hits else GLYPH["cut"]
        if self.kind == "PENDING":
            return GLYPH["ok"] if self.hits else GLYPH["pending"]
        if self.kind == "OPTIONAL":
            return GLYPH["ok"] if self.hits else GLYPH["info"]
        return GLYPH["ok"]

    def note(self):
        if self.kind == "COMPLETE":
            if self.hits:
                return (f"end-of-run marker SEEN @ line {self.first_lineno} "
                        f"— the run got past this point")
            return "NOT SEEN — the capture never reached this point"
        if self.kind == "FORBID":
            if self.hits:
                return f"{self.hits} hit(s), first @ line {self.first_lineno}: {self.first_text}"
            return "0 hits"
        if self.hits == 0:
            if self.kind == "PENDING":
                return "0 hits — awaiting metal/code (never fails)"
            if self.kind == "OPTIONAL":
                return "0 hits (informational)"
            return "0 hits — MISSING"
        n = f"{self.hits} hit(s), first @ line {self.first_lineno}"
        if self.kind == "COUNT" and not self.satisfied():
            n += f" — SHORT of {self.need}"
        if self.kind == "PENDING":
            n += " — MATCHED: consider promoting to REQUIRE"
        return n


class SpecError(Exception):
    pass


def parse_spec(path):
    directives = []
    with open(path, "rb") as f:
        for i, raw in enumerate(f.read().splitlines(), 1):
            line = raw.decode("utf-8", errors="replace").strip()
            if not line or line.startswith("#"):
                continue
            parts = line.split(None, 1)
            kind = parts[0].upper()
            if kind == "COUNT":
                sub = parts[1].split(None, 1) if len(parts) > 1 else []
                if len(sub) != 2 or not sub[0].isdigit():
                    raise SpecError(f"line {i}: COUNT wants '<n> <regex>': {line!r}")
                directives.append(Directive("COUNT", sub[1], need=int(sub[0]), spec_line=i))
            elif kind in ("REQUIRE", "OPTIONAL", "FORBID", "PENDING", "COMPLETE"):
                if len(parts) != 2:
                    raise SpecError(f"line {i}: {kind} wants a regex: {line!r}")
                directives.append(Directive(kind, parts[1], spec_line=i))
            else:
                raise SpecError(f"line {i}: unknown directive {kind!r}")
    for p in DEFAULT_FORBIDS:
        directives.append(Directive("FORBID", p, builtin=True))
    return directives


# ---------------------------------------------------------------------------
# Matching engine
# ---------------------------------------------------------------------------

class Matcher:
    def __init__(self, directives):
        self.directives = directives
        self.lineno = 0
        self.lock = threading.Lock()  # follow-mode reader vs WAIT-polling injector
        # "Where the log stopped" — the last non-blank line fed, reported verbatim on
        # a TRUNCATED verdict so the reader can see the boot phase the capture died in.
        self.last_lineno = 0
        self.last_text = ""
        # Set by run_replay/run_follow when the capture ends MID-LINE (no terminating
        # newline): direct evidence the writer was killed in the middle of a write.
        self.unterminated = False

    def feed_raw(self, raw_line):
        """Feed one raw byte line; returns [(directive, cleaned_text)] new matches."""
        events = []
        text = clean_line(raw_line)
        with self.lock:
            self.lineno += 1
            if text.strip():
                self.last_lineno = self.lineno
                self.last_text = text.strip()
            for d in self.directives:
                if d.feed(text, self.lineno):
                    events.append((d, text))
        return events

    # --- completion / truncation -------------------------------------------

    def markers(self):
        """The spec's declared END-OF-RUN markers (may be empty)."""
        return [d for d in self.directives if d.kind == "COMPLETE"]

    def truncated(self):
        """True when the spec declares end-of-run markers and this capture does not
        show the run reaching one of them — see the module docstring. A spec with no
        COMPLETE directive can never be truncated (old behaviour, byte-identical)."""
        with self.lock:
            ms = [d for d in self.directives if d.kind == "COMPLETE"]
            if not ms:
                return False
            return self.unterminated or not any(d.hits for d in ms)

    def complete(self):
        """Follow-mode EARLY EXIT: stop tailing once the run is genuinely over. That
        means every REQUIRE/COUNT satisfied AND — when the spec declares them — an
        end-of-run marker seen, so a follow can never stop early and then have to call
        its own capture truncated."""
        with self.lock:
            reqs_ok = all(d.satisfied() for d in self.directives
                          if d.kind in ("REQUIRE", "COUNT"))
            ms = [d for d in self.directives if d.kind == "COMPLETE"]
            marker_ok = (not ms) or any(d.hits for d in ms)
            return reqs_ok and marker_ok

    def run_verdict(self):
        """THE single verdict authority — returns ("PASS"|"FAIL"|"TRUNCATED", rc).

        This REPLACED a `passed()` boolean. Two-valued was the shape of the problem:
        with only pass/not-pass there was nowhere to put "the capture stopped early",
        so a short log had to be reported as one of the two things it was not. The
        boolean is gone rather than kept beside this, so nothing can consult a verdict
        that cannot express truncation.

        Precedence (see the module docstring):
          1. a FORBID hit is positive evidence of a fault; a short log never excuses it
          2. a truncated capture is INCONCLUSIVE — never a pass, never a regression
          3. a short REQUIRE/COUNT in a COMPLETE capture is a genuine regression
          4. otherwise PASS — all required witnesses, zero forbidden hits (unchanged)
        """
        with self.lock:
            forbidden = any(d.kind == "FORBID" and d.hits for d in self.directives)
            short = any(d.failed() for d in self.directives
                        if d.kind in ("REQUIRE", "COUNT"))
        if forbidden:
            return "FAIL", RC_FAIL
        if self.truncated():
            return "TRUNCATED", RC_TRUNCATED
        if short:
            return "FAIL", RC_FAIL
        return "PASS", RC_PASS

    def seen(self, rx):
        """Has any fed line matched rx? (for inject WAIT — checks directives'
        recorded state is not enough: WAIT patterns are script-local)."""
        # WAIT keeps its own tail scan; this is only a stub for symmetry.
        raise NotImplementedError


def split_lines(buf):
    """Split a byte buffer on \\n, tolerating \\r\\n and bare \\r; returns
    (complete_lines, remainder)."""
    buf = buf.replace(b"\r\n", b"\n").replace(b"\r", b"\n")
    if b"\n" not in buf:
        return [], buf
    *lines, rest = buf.split(b"\n")
    return lines, rest


# ---------------------------------------------------------------------------
# Verdict table — reads like the battery summary
# ---------------------------------------------------------------------------

def verdict_table(matcher, spec_path, log_path, elapsed=None, out=None):
    out = out or sys.stdout  # resolved at CALL time so redirect_stdout works
    ds = sorted(matcher.directives, key=lambda d: (KIND_ORDER[d.kind], d.spec_line))
    verdict, rc = matcher.run_verdict()
    name = os.path.basename(spec_path)
    hdr = f"════════════ MBENCH VERDICT — {name} vs {log_path}"
    if elapsed is not None:
        hdr += f" ({elapsed:.0f}s)"
    hdr += " ════════════"
    print(hdr, file=out)
    for d in ds:
        print(f"  {d.glyph()} {d.label():<10} {d.pattern}", file=out)
        print(f"       {d.note()}", file=out)
    req = [d for d in ds if d.kind in ("REQUIRE", "COUNT")]
    got = sum(1 for d in req if d.satisfied())
    forb = sum(d.hits for d in ds if d.kind == "FORBID")
    pend = [d for d in ds if d.kind == "PENDING"]
    pmatched = sum(1 for d in pend if d.hits)
    print("  ─────", file=out)
    summary = (f"{got}/{len(req)} required witnesses, {forb} forbidden hit(s)"
               f", {matcher.lineno} lines scanned")
    if pend:
        summary += f", pending {pmatched}/{len(pend)} matched"
    if verdict == "PASS":
        print(f"  {GLYPH['ok']} MBENCH PASS — {summary}", file=out)
    elif verdict == "TRUNCATED":
        # LOUD, and deliberately not shaped like a PASS or a FAIL: this run proved
        # nothing either way. State what was seen vs expected and where it stopped.
        print(f"  {GLYPH['cut']} MBENCH TRUNCATED (INCONCLUSIVE) — {summary}", file=out)
        seen_any = any(d.hits for d in matcher.markers())
        if not seen_any:
            pats = " | ".join(f"/{d.pattern}/" for d in matcher.markers())
            print(f"       end-of-run marker NOT SEEN: {pats}", file=out)
        if matcher.unterminated:
            print("       capture ends MID-LINE (no terminating newline) — the writer "
                  "was killed in the middle of a write", file=out)
        print(f"       log stops at line {matcher.last_lineno}: "
              f"{matcher.last_text[:140]}", file=out)
        print(f"       => NOT a pass and NOT a regression. The boot was cut short, so "
              f"the {len(req) - got} missing witness(es) never got a chance to print.",
              file=out)
        print("       => Re-run with a window long enough to finish the boot "
              "(pi4: `./arroyo kernel8-test 60`) BEFORE reading any of this as a "
              "regression.", file=out)
    else:
        print(f"  {GLYPH['fail']} MBENCH FAIL — {summary}", file=out)
        if matcher.markers():
            # The other half of the honesty claim: say out loud that the run DID reach
            # its end, so a reader knows the shortfall is real and not a short log.
            print("       (the end-of-run marker was seen — the run completed, so a "
                  "missing witness here is a GENUINE regression)", file=out)
    print("  (* = default FORBID, always on)", file=out)
    return rc


# ---------------------------------------------------------------------------
# Replay mode
# ---------------------------------------------------------------------------

def run_replay(log_path, spec_path, quiet=False):
    directives = parse_spec(spec_path)
    matcher = Matcher(directives)
    with open(log_path, "rb") as f:
        data = f.read()
    lines, rest = split_lines(data)
    if rest:
        lines.append(rest)  # unterminated final line still counts
        # …and is itself evidence: a finished capture ends on a newline. A capture cut
        # mid-line was killed while the kernel was still writing. (Only consulted when
        # the spec declares COMPLETE markers — see Matcher.truncated.)
        matcher.unterminated = True
    for raw in lines:
        for d, text in matcher.feed_raw(raw):
            if d.kind == "FORBID" and d.hits == 1 and not quiet:
                print(f"  {GLYPH['fail']} FORBID hit @ line {matcher.lineno}: {text.strip()}")
    return verdict_table(matcher, spec_path, log_path)


# ---------------------------------------------------------------------------
# Follow mode — live tail with rotation/truncation tolerance
# ---------------------------------------------------------------------------

class LogTail:
    """Tails a log PATH (not an fd): the bench-connect scripts refresh the
    ~/{pi,jetson,rmbp}-serial.log symlinks per session, so re-stat every poll
    and reopen from 0 on a new inode or on truncation."""

    def __init__(self, path):
        self.path = path
        self.f = None
        self.ino = None
        self.pos = 0
        self.buf = b""

    def _reopen(self):
        if self.f:
            self.f.close()
        self.f = open(self.path, "rb")
        self.ino = os.fstat(self.f.fileno()).st_ino
        self.pos = 0
        self.buf = b""

    def poll(self):
        """Returns a list of complete raw lines newly appended."""
        try:
            st = os.stat(self.path)  # follows the symlink
        except FileNotFoundError:
            return []
        if self.f is None or st.st_ino != self.ino or st.st_size < self.pos:
            try:
                self._reopen()
            except FileNotFoundError:
                return []
        self.f.seek(self.pos)
        chunk = self.f.read()
        self.pos = self.f.tell()
        if not chunk:
            return []
        self.buf += chunk
        lines, self.buf = split_lines(self.buf)
        return lines

    def close(self):
        if self.f:
            self.f.close()


def run_follow(log_path, spec_path, timeout, injector=None, quiet=False,
               poll_interval=0.2):
    directives = parse_spec(spec_path)
    matcher = Matcher(directives)
    # NOTE: follow mode deliberately never sets `matcher.unterminated`. LogTail only
    # ever feeds WHOLE lines (a partial trailing line stays in its buffer for the next
    # poll), and on a LIVE log a half-written line is the normal steady state, not
    # evidence of a kill. Truncation in follow mode is judged by the marker alone.
    tail = LogTail(log_path)
    t0 = time.monotonic()
    if not quiet:
        print(f"mbench: following {log_path} against {os.path.basename(spec_path)} "
              f"(timeout {timeout:.0f}s) …")
    if injector:
        injector.start(matcher)
    reason = "timeout"
    try:
        while True:
            for raw in tail.poll():
                for d, text in matcher.feed_raw(raw):
                    if quiet:
                        continue
                    if d.kind == "FORBID":
                        print(f"  {GLYPH['fail']} FORBID [{d.pattern}] @ line "
                              f"{matcher.lineno}: {text.strip()}", flush=True)
                    elif d.hits == 1 or d.kind == "COUNT":
                        g = GLYPH["pending"] if d.kind == "PENDING" else GLYPH["ok"]
                        print(f"  {g} {d.label()} [{d.pattern}] @ line "
                              f"{matcher.lineno}: {text.strip()}", flush=True)
            if matcher.complete():
                reason = "completion (all REQUIRE + COUNT satisfied)"
                break
            if time.monotonic() - t0 >= timeout:
                break
            time.sleep(poll_interval)
    except KeyboardInterrupt:
        reason = "interrupted"
    finally:
        if injector:
            injector.stop()
            strip_wait_observers(matcher)
        tail.close()
    elapsed = time.monotonic() - t0
    if not quiet:
        print(f"mbench: follow ended — {reason}")
    return verdict_table(matcher, spec_path, log_path, elapsed=elapsed)


# ---------------------------------------------------------------------------
# Inject scripting (pi/jetson ONLY — x86/rmbp serial is TX-only)
# ---------------------------------------------------------------------------

INJECT_PLATFORMS = ("pi", "jetson")


class Injector:
    """Writes commands to a bridge inject FIFO, one open-write-close per command
    (the proven `printf 'cmd\\r' > /tmp/pi.in` idiom — the bridge holds the FIFO
    O_RDWR so the open never blocks). Runs on a thread beside follow mode."""

    def __init__(self, fifo, script_path, settle):
        self.fifo = fifo
        self.script_path = script_path
        self.settle = settle
        self.matcher = None
        self.thread = None
        self.stopping = threading.Event()
        self.steps = self._parse(script_path)

    @staticmethod
    def _parse(path):
        steps = []
        with open(path, "rb") as f:
            for i, raw in enumerate(f.read().splitlines(), 1):
                line = raw.decode("utf-8", errors="replace").strip()
                if not line or line.startswith("#"):
                    continue
                parts = line.split(None, 1)
                head = parts[0].upper()
                if head == "SLEEP":
                    steps.append(("SLEEP", float(parts[1])))
                elif head == "WAIT":
                    sub = parts[1].split(None, 1)
                    if len(sub) != 2:
                        raise SpecError(f"inject script line {i}: WAIT wants '<secs> <regex>'")
                    steps.append(("WAIT", (float(sub[0]), re.compile(sub[1]))))
                else:
                    steps.append(("CMD", line))
        return steps

    def start(self, matcher):
        self.matcher = matcher
        self.thread = threading.Thread(target=self._run, daemon=True)
        self.thread.start()

    def stop(self):
        self.stopping.set()
        if self.thread:
            self.thread.join(timeout=2.0)

    def _send(self, cmd):
        with open(self.fifo, "wb", buffering=0) as f:
            f.write(cmd.encode("utf-8") + b"\r")
        print(f"  → inject: {cmd}", flush=True)

    def _run(self):
        for kind, arg in self.steps:
            if self.stopping.is_set():
                return
            if kind == "SLEEP":
                self.stopping.wait(arg)
            elif kind == "WAIT":
                secs, rx = arg
                # Register a table-stripped observer directive: the follow
                # reader thread does the matching (WAIT never re-reads the log).
                observer = Directive("OPTIONAL", rx.pattern)
                observer._wait_observer = True
                with self.matcher.lock:
                    self.matcher.directives.append(observer)
                deadline = time.monotonic() + secs
                seen = False
                while time.monotonic() < deadline and not self.stopping.is_set():
                    with self.matcher.lock:
                        seen = observer.hits > 0
                    if seen:
                        break
                    self.stopping.wait(0.2)
                if not seen:
                    print(f"  ⚠ inject WAIT timed out on /{rx.pattern}/ — continuing",
                          flush=True)
            else:
                try:
                    self._send(arg)
                except OSError as e:
                    print(f"  ⚠ inject failed on {self.fifo}: {e}", flush=True)
                    return
                self.stopping.wait(self.settle)


def strip_wait_observers(matcher):
    """Drop the transient WAIT observers before the verdict table prints."""
    with matcher.lock:
        matcher.directives = [d for d in matcher.directives
                              if not getattr(d, "_wait_observer", False)]


# ---------------------------------------------------------------------------
# Self-test — no hardware: canned lines incl. control bytes, replay AND follow
# ---------------------------------------------------------------------------

CANNED = (
    b"\x1b[2J\x1b[HUEFI firmware noise \x00\x01 garbage\r\n"
    b":: CAPSTONE Semaphore: PASS ::\r\n"
    b":: \x1b[32mU4: process model \xe2\x80\x94 reaped -> PASS\x1b[0m ::\r\n"
    b":: U5: capabilities -> PASS ::\r\n"
    b"half a line, no newline yet"
)
CANNED_TAIL = (
    b" \xe2\x80\xa6 now finished\r\n"
    b":: CAPSTONE COMPLETE \xe2\x80\x94 all 6 verified ::\r\n"
)
CANNED_BAD = b":: U9: write-back -> FAIL (sector mismatch) ::\r\n"

SELFTEST_SPEC = """\
# mbench self-test spec
REQUIRE CAPSTONE COMPLETE
COUNT 2 -> PASS
OPTIONAL Semaphore: PASS
PENDING NEVER-FLASHED-WITNESS
"""

# The truncation fixtures. A spec with an END-OF-RUN MARKER, and three logs that must
# land on three DIFFERENT verdicts — the distinction the whole feature exists for.
TRUNC_SPEC = """\
# mbench self-test spec — end-of-run marker declared
COMPLETE RUN-END marker
REQUIRE FIRST-WITNESS PASS
REQUIRE LAST-WITNESS PASS
"""
TRUNC_HEAD = b":: FIRST-WITNESS PASS ::\r\n"
TRUNC_TAIL = b":: LAST-WITNESS PASS ::\r\n:: RUN-END marker ::\r\n"


def _write(path, data):
    with open(path, "wb") as f:
        f.write(data)


def run_self_test():
    ok = True
    results = []

    def check(name, cond):
        nonlocal ok
        results.append((name, cond))
        if not cond:
            ok = False

    def muted(fn, *a, **kw):
        """Run a scenario with its verdict table swallowed (self-test output
        stays a single summary block); the table is replayed on a FAIL."""
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            rc = fn(*a, **kw)
        return rc, buf.getvalue()

    with tempfile.TemporaryDirectory(prefix="mbench-selftest-") as td:
        spec = os.path.join(td, "selftest.spec")
        _write(spec, SELFTEST_SPEC.encode())

        # 1. replay PASS: control bytes + UTF-8 + unterminated-then-finished line
        log1 = os.path.join(td, "good.log")
        _write(log1, CANNED + CANNED_TAIL)
        rc, table = muted(run_replay, log1, spec, quiet=True)
        check("replay: control-byte log PASSes spec", rc == 0)
        if rc != 0:
            print(table)

        # 2. replay FAIL on default FORBID (spec does not list '-> FAIL')
        log2 = os.path.join(td, "bad.log")
        _write(log2, CANNED + CANNED_TAIL + CANNED_BAD)
        rc, _ = muted(run_replay, log2, spec, quiet=True)
        check("replay: default FORBID '-> FAIL' fails the run", rc != 0)

        # 3. replay FAIL on a missing REQUIRE
        log3 = os.path.join(td, "short.log")
        _write(log3, CANNED)  # no CAPSTONE COMPLETE, count short
        rc, _ = muted(run_replay, log3, spec, quiet=True)
        check("replay: missing REQUIRE fails the run", rc != 0)

        # 4. follow: background writer appends canned lines (split mid-line,
        #    mid-UTF-8) — follow must catch every witness and end on completion
        #    well before its timeout.
        log4 = os.path.join(td, "live.log")
        _write(log4, b"")

        def writer():
            time.sleep(0.3)
            with open(log4, "ab", buffering=0) as f:
                f.write(CANNED)          # ends mid-line
                time.sleep(0.4)
                f.write(CANNED_TAIL[:3])  # split INSIDE the UTF-8 ellipsis
                time.sleep(0.3)
                f.write(CANNED_TAIL[3:])

        t = threading.Thread(target=writer, daemon=True)
        t0 = time.monotonic()
        t.start()
        rc, table = muted(run_follow, log4, spec, timeout=15.0, quiet=True,
                          poll_interval=0.05)
        dt = time.monotonic() - t0
        t.join()
        check("follow: live writer witnesses all caught (PASS)", rc == 0)
        check("follow: ended on completion, not timeout", dt < 10.0)
        if rc != 0:
            print(table)

        # 5. follow: REQUIRE never arrives -> FAIL at timeout
        log5 = os.path.join(td, "stall.log")
        _write(log5, CANNED)
        rc, _ = muted(run_follow, log5, spec, timeout=1.0, quiet=True,
                      poll_interval=0.05)
        check("follow: absent REQUIRE fails at timeout", rc != 0)

        # 5b. TRUNCATION — the three verdicts must be genuinely DISTINCT. A change
        #     that cannot separate (b) short-log from (c) real-regression has not done
        #     the job, so the separation is asserted here, not just at the bench.
        tspec = os.path.join(td, "trunc.spec")
        _write(tspec, TRUNC_SPEC.encode())

        # (a) complete run, every witness -> PASS
        tgood = os.path.join(td, "t-good.log")
        _write(tgood, TRUNC_HEAD + TRUNC_TAIL)
        rc, table = muted(run_replay, tgood, tspec, quiet=True)
        check("truncation: complete log with all witnesses PASSes (rc 0)", rc == RC_PASS)
        if rc != RC_PASS:
            print(table)

        # (b) the SAME log cut before the marker -> TRUNCATED, and specifically NOT a pass
        tcut = os.path.join(td, "t-cut.log")
        _write(tcut, TRUNC_HEAD)
        rc, _ = muted(run_replay, tcut, tspec, quiet=True)
        check("truncation: log cut before the end-of-run marker is TRUNCATED (rc 3)",
              rc == RC_TRUNCATED)
        check("truncation: a truncated log is NOT reported as a pass", rc != RC_PASS)

        # (c) a COMPLETE run missing one witness -> FAIL. The other half: truncation
        #     detection must not have turned real regressions into "inconclusive".
        treg = os.path.join(td, "t-regress.log")
        _write(treg, TRUNC_HEAD + b":: RUN-END marker ::\r\n")  # LAST-WITNESS deleted
        rc, _ = muted(run_replay, treg, tspec, quiet=True)
        check("truncation: complete log with a MISSING witness still FAILs (rc 1)",
              rc == RC_FAIL)

        # (d) capture killed mid-line, marker present -> still TRUNCATED
        tmid = os.path.join(td, "t-midline.log")
        _write(tmid, TRUNC_HEAD + TRUNC_TAIL + b":: half a li")
        rc, _ = muted(run_replay, tmid, tspec, quiet=True)
        check("truncation: capture ending mid-line is TRUNCATED (rc 3)",
              rc == RC_TRUNCATED)

        # (e) a FORBID hit outranks truncation — a PANIC in a short log is still a fault
        tpanic = os.path.join(td, "t-panic.log")
        _write(tpanic, TRUNC_HEAD + b"PANIC: something exploded\r\n")
        rc, _ = muted(run_replay, tpanic, tspec, quiet=True)
        check("truncation: a FORBID hit in a short log is FAIL, not TRUNCATED",
              rc == RC_FAIL)

        # (f) FOLLOW mode carries the same three verdicts. The follow-specific change
        #     is Matcher.complete(): it must NOT stop early on a spec whose marker has
        #     not landed, or a follow could end its own capture and then call it short.
        tlive = os.path.join(td, "t-live.log")
        _write(tlive, b"")

        def twriter():
            time.sleep(0.2)
            with open(tlive, "ab", buffering=0) as f:
                f.write(TRUNC_HEAD)
                time.sleep(0.4)
                f.write(TRUNC_TAIL)

        th = threading.Thread(target=twriter, daemon=True)
        t1 = time.monotonic()
        th.start()
        rc, table = muted(run_follow, tlive, tspec, timeout=15.0, quiet=True,
                          poll_interval=0.05)
        dt1 = time.monotonic() - t1
        th.join()
        check("truncation/follow: run reaching the marker PASSes", rc == RC_PASS)
        check("truncation/follow: did not stop before the marker landed", dt1 >= 0.4)
        if rc != RC_PASS:
            print(table)

        # …and a follow whose marker never arrives times out TRUNCATED, not FAIL.
        tstall = os.path.join(td, "t-stall.log")
        _write(tstall, TRUNC_HEAD)
        rc, _ = muted(run_follow, tstall, tspec, timeout=1.0, quiet=True,
                      poll_interval=0.05)
        check("truncation/follow: marker never arrives -> TRUNCATED (rc 3)",
              rc == RC_TRUNCATED)

        # (g) NO REGRESSION for marker-less specs: the original self-test spec must
        #     keep its exact old PASS/FAIL exit codes (x86/arm/jetson callers).
        rc, _ = muted(run_replay, log3, spec, quiet=True)  # short log, no COMPLETE
        check("truncation: a spec with NO COMPLETE line keeps plain FAIL (rc 1)",
              rc == RC_FAIL)

        # 6. spec hygiene: every checked-in spec must parse
        specs_dir = os.path.join(os.path.dirname(os.path.abspath(__file__)), "specs")
        if os.path.isdir(specs_dir):
            for fn in sorted(os.listdir(specs_dir)):
                if not fn.endswith(".spec"):
                    continue
                try:
                    parse_spec(os.path.join(specs_dir, fn))
                    check(f"spec parses: {fn}", True)
                except SpecError as e:
                    print(f"    {fn}: {e}")
                    check(f"spec parses: {fn}", False)

        # 7. inject guard: rmbp/x86 refused, pi accepted (parse-level only)
        script = os.path.join(td, "cmds.txt")
        _write(script, b"# comment\nver\nSLEEP 0.1\nWAIT 1 CAPSTONE\nhelp\n")
        for plat in ("rmbp", "x86", None):
            check(f"inject: platform {plat!r} refused",
                  not inject_platform_allowed(plat))
        for plat in INJECT_PLATFORMS:
            check(f"inject: platform {plat!r} allowed",
                  inject_platform_allowed(plat))
        steps = Injector._parse(script)
        check("inject: script parses (CMD/SLEEP/WAIT)",
              [k for k, _ in steps] == ["CMD", "SLEEP", "WAIT", "CMD"])

        # 8. inject FIFO round-trip: hold a FIFO O_RDWR|O_NONBLOCK exactly the
        #    way the bridges do, send one command, read back `cmd\r`.
        fifo = os.path.join(td, "test.in")
        os.mkfifo(fifo)
        bridge_fd = os.open(fifo, os.O_RDWR | os.O_NONBLOCK)
        try:
            inj = Injector(fifo, script, settle=0.0)
            with contextlib.redirect_stdout(io.StringIO()):
                inj._send("ver")
            time.sleep(0.1)
            got = os.read(bridge_fd, 64)
            check("inject: FIFO round-trip delivers 'ver\\r'", got == b"ver\r")
        finally:
            os.close(bridge_fd)

    print("════════════ MBENCH SELF-TEST ════════════")
    for name, cond in results:
        print(f"  {GLYPH['ok'] if cond else GLYPH['fail']} {name}")
    n = sum(1 for _, c in results if c)
    if ok:
        print(f"  {GLYPH['ok']} SELF-TEST PASS — {n}/{len(results)}")
    else:
        print(f"  {GLYPH['fail']} SELF-TEST FAIL — {n}/{len(results)}")
    return 0 if ok else 1


def inject_platform_allowed(platform):
    return platform in INJECT_PLATFORMS


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

def main(argv=None):
    ap = argparse.ArgumentParser(
        prog="mbench", description="UnaOS metal-bench harness — assert a serial "
        "capture against a witness spec. See the module docstring for the spec "
        "format.",
        epilog="exit codes: 0 PASS | 1 FAIL (a genuine regression) | 2 usage/spec "
               "error | 3 TRUNCATED — the capture stopped before the run finished, "
               "so the result is INCONCLUSIVE: neither a pass nor a regression. "
               "Re-run with a longer window before reading a shortfall as a bug.")
    mode = ap.add_mutually_exclusive_group(required=True)
    mode.add_argument("--replay", metavar="LOG", help="assert a finished log")
    mode.add_argument("--follow", metavar="LOG", help="live-tail a bridge/QEMU log")
    mode.add_argument("--self-test", action="store_true",
                      help="no hardware: verify the harness on canned lines")
    ap.add_argument("--spec", metavar="FILE", help="witness spec (*.spec)")
    ap.add_argument("--timeout", type=float, default=120.0,
                    help="follow-mode bound in seconds (default 120)")
    ap.add_argument("--inject", metavar="FIFO",
                    help="bridge inject FIFO (pi/jetson ONLY; requires --follow, "
                         "--script and --platform)")
    ap.add_argument("--script", metavar="FILE", help="inject command script")
    ap.add_argument("--platform", choices=("pi", "jetson", "rmbp", "x86"),
                    help="bench platform (guards --inject: x86/rmbp are TX-only)")
    ap.add_argument("--settle", type=float, default=1.0,
                    help="seconds between injected commands (default 1.0)")
    ap.add_argument("--quiet", action="store_true",
                    help="suppress live per-witness lines; table only")
    args = ap.parse_args(argv)

    if args.self_test:
        return run_self_test()

    if not args.spec:
        ap.error("--spec is required with --replay/--follow")

    injector = None
    if args.inject:
        if not args.follow:
            ap.error("--inject only rides --follow")
        if not args.script:
            ap.error("--inject requires --script")
        if not inject_platform_allowed(args.platform):
            print(f"mbench: REFUSING --inject for platform "
                  f"{args.platform!r}: injection is pi/jetson only "
                  f"(x86/rmbp serial is TX-only — FTDI bulk-IN is a kernel stub).",
                  file=sys.stderr)
            return RC_ERROR
        injector = Injector(args.inject, args.script, args.settle)

    try:
        if args.replay:
            return run_replay(args.replay, args.spec, quiet=args.quiet)
        rc = run_follow(args.follow, args.spec, args.timeout,
                        injector=injector, quiet=args.quiet)
        return rc
    except SpecError as e:
        print(f"mbench: spec error: {e}", file=sys.stderr)
        return RC_ERROR
    except FileNotFoundError as e:
        print(f"mbench: {e}", file=sys.stderr)
        return RC_ERROR


if __name__ == "__main__":
    sys.exit(main())
