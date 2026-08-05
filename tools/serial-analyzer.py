#!/usr/bin/env python3
import sys
import re
import argparse

def strip_control_bytes(text):
    # Strip \x00-\x1F except \n, \r, \t
    return re.sub(r'[\x00-\x08\x0b-\x0c\x0e-\x1f]', '', text)

def parse_log(filepath):
    with open(filepath, 'r', errors='replace') as f:
        content = f.read()
    
    content = strip_control_bytes(content)
    
    # Split by boots
    # We look for lines containing "MARK " and " boot<N> "
    # The actual line from logs is e.g.
    # 2026-07-19T22:29:39Z MARK MARK R23s1 boot12 ORIN ...
    
    lines = content.split('\n')
    
    boots = []
    current_boot = None
    
    for line in lines:
        if ' MARK ' in line and ' boot' in line:
            match = re.search(r'boot(\d+)', line)
            if match:
                boot_num = match.group(1)
                current_boot = {
                    'number': boot_num,
                    'start_line': line,
                    'witnesses': [],
                    'lines': []
                }
                boots.append(current_boot)
        if current_boot:
            current_boot['lines'].append(line)
            # Extract witness lines
            if re.search(r'::.*witness.*::', line):
                current_boot['witnesses'].append(line)
    
    # Classify each boot
    for boot in boots:
        boot_text = '\n'.join(boot['lines'])
        classes = []
        if 'RAS' in boot_text:
            classes.append('RAS')
        if 'panic' in boot_text.lower():
            classes.append('panic')
        if 'PASS' in boot_text:
            classes.append('PASS')
        if 'lease' in boot_text.lower():
            classes.append('lease')
        
        boot['classes'] = classes if classes else ['unknown']
        
    return boots

def print_boot_summary(boot):
    print(f"Boot {boot['number']}: {', '.join(boot['classes'])}")
    print(f"  Start: {boot['start_line']}")
    for w in boot['witnesses']:
        print(f"  Witness: {w.strip()}")
    print("")

def diff_boots(boots1, boots2):
    print("=== DIFF BETWEEN TWO LOG FILES ===")
    
    # We'll just compare boot counts and classifications for now, or match by number
    b1_dict = {b['number']: b for b in boots1}
    b2_dict = {b['number']: b for b in boots2}
    
    all_nums = sorted(list(set(b1_dict.keys()) | set(b2_dict.keys())), key=int)
    for num in all_nums:
        b1 = b1_dict.get(num)
        b2 = b2_dict.get(num)
        
        if b1 and not b2:
            print(f"Boot {num} only in file 1")
        elif b2 and not b1:
            print(f"Boot {num} only in file 2")
        else:
            classes1 = set(b1['classes'])
            classes2 = set(b2['classes'])
            w1 = set([w.strip() for w in b1['witnesses']])
            w2 = set([w.strip() for w in b2['witnesses']])
            
            if classes1 != classes2 or w1 != w2:
                print(f"Boot {num} differs:")
                if classes1 != classes2:
                    print(f"  Classes: {list(classes1)} vs {list(classes2)}")
                if w1 != w2:
                    added = w2 - w1
                    removed = w1 - w2
                    if added:
                        print(f"  Added witnesses: {added}")
                    if removed:
                        print(f"  Removed witnesses: {removed}")
            else:
                print(f"Boot {num} matches exactly.")

# --- logts gap analysis -------------------------------------------------
#
# With the logts feature armed every serial line carries a fixed 12-column
# prefix. Three forms exist, and they are three different states, not two:
#
#   '[  NNNNNms] '  monotonic ms since kernel entry (same origin as the
#                   BPACE/GPACE since-entry ledger figures, so the numbers are
#                   directly comparable to them);
#   '[HH:MM:SSZ] '  civil time, once a wall-clock anchor is set;
#   '[      ?ms] '  PREFIXED BUT UNKNOWN -- the line was emitted before the
#                   bootpace entry stamp or before TSC calibration, so the
#                   kernel refuses to invent a number.
#
# A line with NO prefix at all is a different thing again: it was deferred
# under lock contention. Both unknown and deferred lines are listed between
# their timestamped neighbours and never given an interpolated number; gaps are
# only ever measured between two numeric stamps. They are counted separately
# because a capture that is entirely '?ms' is a machine whose counter was never
# calibrated -- a real failure, reported as such and exited nonzero -- whereas a
# capture that is entirely unprefixed is simply not a logts capture.

TS_MONO_RE = re.compile(r'^\[\s*(\d+)ms\]\s')
TS_CIVIL_RE = re.compile(r'^\[(\d{2}):(\d{2}):(\d{2})Z\]\s')
TS_UNKNOWN_RE = re.compile(r'^\[\s*\?ms\]\s')
HZ_RE = re.compile(r'\bhz=(\d+)')

# UNAOS.LOG-only fixed lines, written by the flight recorder DIRECTLY into the
# file (never through the serial taps, so never prefixed): the FRSTAMP boot
# stamp, the self-identifying header, the dropped note and the end-of-log
# marker. In a --gaps run over a saved UNAOS.LOG these must be classed as file
# metadata, not as contention-deferred serial lines -- otherwise every log
# starts with guaranteed false deferral counts. Note boot_stamp renders bools
# as true/false, not 1/0.
FILE_META_RE = re.compile(
    r'^:: (FR-BOOT: hz=\d+ cy=\d+ reused=(true|false) state=(reserved|flushed)'
    r'|UnaOS flight-recorder boot log \(UNAOS\.LOG\)'
    r'|FLIGHTREC: (\d+ byte\(s\) dropped|end of log)) '
)

KEPLER_START = 'Initializing Kepler'
KEPLER_END = 'GPACE: span'


def parse_ts(line):
    """Return (kind, milliseconds, body) or None when the line has no prefix.

    `kind` is 'mono', 'civil' or 'unknown'; milliseconds is None for 'unknown'.
    """
    m = TS_MONO_RE.match(line)
    if m:
        return ('mono', int(m.group(1)), line[m.end():])
    m = TS_CIVIL_RE.match(line)
    if m:
        h, mi, s = int(m.group(1)), int(m.group(2)), int(m.group(3))
        return ('civil', ((h * 60 + mi) * 60 + s) * 1000, line[m.end():])
    m = TS_UNKNOWN_RE.match(line)
    if m:
        # Prefixed, but the counter could not answer. No number, ever.
        return ('unknown', None, line[m.end():])
    return None


def segment_by_hz(rows):
    """Split rows into per-boot segments keyed on the hz= token, which is
    unique per boot. hz appears mid-boot, so the cut is refined to the
    timestamp reset inside the ambiguous window when one is visible."""
    marks = []  # (index, hz)
    for i, r in enumerate(rows):
        m = HZ_RE.search(r['line'])
        if m:
            marks.append((i, m.group(1)))
    if not marks:
        return [(None, rows)]

    cuts = []
    for (pi, phz), (ni, nhz) in zip(marks, marks[1:]):
        if phz == nhz:
            continue
        cut = ni
        # Seed with the last stamp at or before the previous hz sighting, so a
        # reset on the very first line of the window is still seen as a reset.
        prev_ts = None
        for j in range(pi, -1, -1):
            if rows[j]['ts'] is not None:
                prev_ts = rows[j]['ts']
                break
        for j in range(pi + 1, ni + 1):
            ts = rows[j]['ts']
            if ts is None:
                continue
            if prev_ts is not None and ts < prev_ts:
                cut = j
                break
            prev_ts = ts
        cuts.append(cut)

    segments = []
    bounds = [0] + cuts + [len(rows)]
    for a, b in zip(bounds, bounds[1:]):
        chunk = rows[a:b]
        hz = None
        for r in chunk:
            m = HZ_RE.search(r['line'])
            if m:
                hz = m.group(1)
                break
        segments.append((hz, chunk))
    return segments


def trunc(text, width=90):
    text = text.rstrip()
    return text if len(text) <= width else text[:width - 1] + '…'


def top_gaps(rows, top):
    """Gaps between consecutive numerically stamped rows. Deferred (unprefixed)
    and unknown-time ('?ms') rows are carried along as context, never as gap
    endpoints -- neither may fabricate or split a measurement."""
    gaps = []
    prev = None
    pending = []
    for r in rows:
        if r['ts'] is None:
            if prev is not None:
                pending.append(r)
            continue
        if prev is not None and prev['kind'] == r['kind']:
            gaps.append({
                'delta': r['ts'] - prev['ts'],
                'from': prev,
                'to': r,
                'deferred': pending,
            })
        prev = r
        pending = []
    gaps.sort(key=lambda g: g['delta'], reverse=True)
    return gaps[:top]


def print_gap_table(title, rows, top):
    span_rows = [r for r in rows if r['ts'] is not None]
    if not span_rows:
        print(f"  {title}: no timestamped lines")
        return
    span = span_rows[-1]['ts'] - span_rows[0]['ts']
    unknown = sum(1 for r in rows if r['kind'] == 'unknown')
    deferred = sum(1 for r in rows if r['kind'] is None)
    print(f"  {title}: {len(span_rows)} timestamped lines, "
          f"{deferred} deferred, span {span}ms "
          f"[{span_rows[0]['ts']}ms .. {span_rows[-1]['ts']}ms]")
    print(f"    unknown-time lines: {unknown}")
    gaps = top_gaps(rows, top)
    if not gaps:
        print("    (no measurable gaps)")
        return
    print(f"    {'delta':>10}  {'at':>10}  line")
    for g in gaps:
        print(f"    {str(g['delta']) + 'ms':>10}  {str(g['from']['ts']) + 'ms':>10}  < {trunc(g['from']['body'])}")
        for d in g['deferred']:
            tag = {'unknown': '(?ms)', 'filemeta': '(file)'}.get(d['kind'], '(deferred)')
            print(f"    {'':>10}  {tag:>10}  ~ {trunc(d['line'])}")
        print(f"    {'':>10}  {str(g['to']['ts']) + 'ms':>10}  > {trunc(g['to']['body'])}")
    print("")


def load_rows(content):
    rows = []
    for line in content.split('\n'):
        if not line.strip():
            continue
        parsed = parse_ts(line)
        if parsed:
            kind, ts, body = parsed
            rows.append({'line': line, 'kind': kind, 'ts': ts, 'body': body})
        elif FILE_META_RE.match(line):
            # UNAOS.LOG fixed lines are written straight to the file, never through
            # the serial taps -- unprefixed by construction, not deferred.
            rows.append({'line': line, 'kind': 'filemeta', 'ts': None, 'body': line})
        else:
            rows.append({'line': line, 'kind': None, 'ts': None, 'body': line})
    return rows


def gaps_mode(filepath, top):
    with open(filepath, 'r', errors='replace') as f:
        content = strip_control_bytes(f.read())
    rows = load_rows(content)

    if not any(r['ts'] is not None for r in rows):
        unknown = sum(1 for r in rows if r['kind'] == 'unknown')
        if unknown:
            # Every line carried a prefix and every prefix read '?': the counter
            # was never calibrated (no invariant TSC). A real failure, not a
            # missing-feature diagnostic.
            print(f"{filepath}: counter never calibrated "
                  f"({unknown} unknown-time lines, no numeric stamp anywhere)")
        else:
            print(f"{filepath}: no logts timestamps found; --gaps needs a logts-prefixed capture")
        return False

    print(f"--- gaps {filepath} ---")
    for n, (hz, chunk) in enumerate(segment_by_hz(rows), 1):
        label = f"boot {n} (hz={hz})" if hz else f"boot {n} (hz unknown)"
        print(f"{label}")
        print_gap_table("whole boot", chunk, top)

        start = end = None
        for i, r in enumerate(chunk):
            if start is None and KEPLER_START in r['line']:
                start = i
            elif start is not None and KEPLER_END in r['line']:
                end = i
                break
        if start is None:
            print("  kepler window: 'Initializing Kepler' not seen\n")
        elif end is None:
            print("  kepler window: 'GPACE: span' not seen after Kepler init\n")
        else:
            print_gap_table("kepler window", chunk[start:end + 1], top)
    return True


# --- synthetic self-test -------------------------------------------------

# NOTE: no ':: FR-BOOT:' lines here on purpose. FRSTAMP is FILE-only (flight_recorder.rs appends
# it raw, bypassing the serial taps), so a serial capture can never contain it — a fixture carrying
# it would train a regex against a line that cannot occur, and prefixed at that (boot_stamp output
# is never prefixed). When analyzing an UNAOS.LOG file, see FILE_META_RE below.
SELFTEST_MIXED = """\
[      ?ms] serial: early init
[      0ms] bootpace: entry
[     12ms] Initializing Kepler
a deferred line with no prefix at all
[   1712ms] kepler: takeover complete
[   1730ms] GPACE: span 1718ms
[   1740ms] desktop up
"""

SELFTEST_ALL_UNKNOWN = """\
[      ?ms] serial: early init
[      ?ms] clock: no invariant TSC
[      ?ms] desktop up
"""


def selftest(top):
    """Two synthetic captures exercising the third prefix form: one mixed
    capture where '?ms' lines must be counted but must not become gap
    endpoints, and one all-'?ms' capture that must fail."""
    import tempfile
    import os

    ok = True
    for name, text, expect_ok in (
        ('mixed (numeric + ?ms + deferred)', SELFTEST_MIXED, True),
        ('all-?ms (counter never calibrated)', SELFTEST_ALL_UNKNOWN, False),
    ):
        fd, path = tempfile.mkstemp(prefix='serial-selftest-', suffix='.log')
        with os.fdopen(fd, 'w') as f:
            f.write(text)
        print(f"=== selftest: {name} ===")
        got = gaps_mode(path, top)
        os.unlink(path)
        verdict = 'PASS' if got == expect_ok else 'FAIL'
        if got != expect_ok:
            ok = False
        print(f"=== selftest: {name}: {verdict} "
              f"(expected {'ok' if expect_ok else 'failure'}, got "
              f"{'ok' if got else 'failure'})\n")
    return ok


def main():
    parser = argparse.ArgumentParser(
        description="Analyze serial captures",
        epilog=("logts prefixes: '[  NNNNNms] ' is an absolute stamp in ms since KERNEL ENTRY "
                "-- the same origin the BPACE/GPACE since-entry figures use, so the numbers can "
                "be compared with those ledger lines directly. '[HH:MM:SSZ] ' is civil time. "
                "'[      ?ms] ' is prefixed-but-unknown (emitted before the bootpace entry stamp "
                "or before TSC calibration): counted separately, never a gap endpoint. A capture "
                "that is entirely '?ms' is reported as 'counter never calibrated' and exits "
                "nonzero."))
    parser.add_argument("logs", nargs='*', help="Log files to parse (1 or 2 files)")
    parser.add_argument("--gaps", action="store_true",
                        help="report the largest inter-line time gaps in a logts-prefixed capture")
    parser.add_argument("--top", type=int, default=15,
                        help="how many gaps to list per table (default 15)")
    parser.add_argument("--selftest", action="store_true",
                        help="run the synthetic prefix-parsing fixtures and exit")
    args = parser.parse_args()

    if args.selftest:
        sys.exit(0 if selftest(args.top) else 1)

    if not args.logs:
        parser.error("no log files given")

    if args.gaps:
        ok = True
        for log_file in args.logs:
            if not gaps_mode(log_file, args.top):
                ok = False
        sys.exit(0 if ok else 1)

    if len(args.logs) > 2:
        print("Please provide 1 or 2 log files.")
        sys.exit(1)

    boots_list = []
    for log_file in args.logs:
        print(f"--- Parsing {log_file} ---")
        boots = parse_log(log_file)
        boots_list.append(boots)
        for b in boots:
            print_boot_summary(b)
            
    if len(boots_list) == 2:
        diff_boots(boots_list[0], boots_list[1])

if __name__ == '__main__':
    main()
