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


def read_capture(filepath):
    with open(filepath, 'r', errors='replace') as f:
        return strip_control_bytes(f.read())


def refuse_unless_logts(label, rows, mode):
    """Guard both timing modes. A capture with no numeric stamp anywhere has no
    measurement in it; the answer is a refusal, never an estimate."""
    if any(r['ts'] is not None for r in rows):
        return True
    unknown = sum(1 for r in rows if r['kind'] == 'unknown')
    if unknown:
        # Every line carried a prefix and every prefix read '?': the counter
        # was never calibrated (no invariant TSC). A real failure, not a
        # missing-feature diagnostic.
        print(f"{label}: counter never calibrated "
              f"({unknown} unknown-time lines, no numeric stamp anywhere)")
    else:
        print(f"{label}: no logts timestamps found; {mode} needs a logts-prefixed capture")
    return False


def find_kepler_window(chunk):
    """Locate the kepler window inside one boot segment. Returns (start, end)
    row indices inclusive, or a string naming which anchor was missing. The two
    anchor strings are load-bearing -- they are what --gaps cuts on too."""
    start = end = None
    for i, r in enumerate(chunk):
        if start is None and KEPLER_START in r['line']:
            start = i
        elif start is not None and KEPLER_END in r['line']:
            end = i
            break
    if start is None:
        return "'Initializing Kepler' not seen"
    if end is None:
        return "'GPACE: span' not seen after Kepler init"
    return (start, end)


def gaps_mode(filepath, top):
    return gaps_report(filepath, read_capture(filepath), top)


def gaps_report(label, content, top):
    rows = load_rows(content)
    if not refuse_unless_logts(label, rows, '--gaps'):
        return False

    print(f"--- gaps {label} ---")
    for n, (hz, chunk) in enumerate(segment_by_hz(rows), 1):
        boot_label = f"boot {n} (hz={hz})" if hz else f"boot {n} (hz unknown)"
        print(f"{boot_label}")
        print_gap_table("whole boot", chunk, top)

        window = find_kepler_window(chunk)
        if isinstance(window, str):
            print(f"  kepler window: {window}\n")
        else:
            start, end = window
            print_gap_table("kepler window", chunk[start:end + 1], top)
    return True


# --- witness-cost decomposition (--wcg) ---------------------------------
#
# GR16/s73 proved on metal that the "kepler=17129ms" block is not GPU bring-up.
# The real Kepler takeover is ~1.4 s (kepler=1401/1402ms across two witness-OFF
# boots, 1 ms apart); the rest of the block is the witness battery running inside
# the measured span -- four [wc-g] glass-verify passes of ~2.87 s each plus the
# [wc-d] verifies. See docs/dev/OS/01_BOOT_HAL/bootpace.md section 10g.
#
# --wcg re-derives that decomposition from any logts capture instead of by hand.
# The unit of attribution is the one --gaps already uses: a line's COST is the gap
# from the previous numerically stamped line, so the cost of a [wc-g] sample line
# is the work that produced it. Every line in the window is classified and costed,
# so the group costs sum to the window span by construction -- the table always
# reconciles, and nothing can be quietly dropped into a rounding error.
#
# Two things this mode will not do:
#
#   * it will not interpolate across an unprefixed (contention-deferred) or '?ms'
#     line. A deferred line's time lands inside the cost of the NEXT stamped line
#     and is reported as such; it is never split by guess. Deferred witness lines
#     are listed by name so a reader can see exactly which costs are inflated;
#   * on a capture with no numeric stamp anywhere it refuses, like --gaps does,
#     rather than estimating a decomposition out of line counts.
#
# The gap-derived cost of a wc-g pass is an upper bound on the pass itself: it
# includes whatever else ran between the previous stamped line and the sample.
# A future kernel build closes that by emitting, immediately after each sample:
#
#   [wc-g] prof win={id} seq={seq} surf_bytes={n} cks_blit_us={n} civac_us={n} \
#          cks_after_us={n} probes={n} readback_us={n}
#
# When those lines are present --wcg prints the per-phase table and the leftover
# (cost minus the summed phases) as an explicit UNATTRIBUTED remainder. When they
# are absent -- which is every capture taken to date -- it says so and gives the
# gap-only decomposition. The mode is useful either way; it just says which one
# it is rather than letting a reader assume the sharper answer.

WITNESS_TAG_RE = re.compile(r'^\[(wc-g|wc-h|wc-d|wc-k|wcn)\]')
BRINGUP_RE = re.compile(r'^(?:\[NVIDIA\]|:: (?:kepler|kdisp): )')
WCG_PASS_RE = re.compile(r'^\[wc-g\] win=(\d+) seq=(\d+)\b')
WCG_PROF_RE = re.compile(r'^\[wc-g\] prof win=(\d+) seq=(\d+)\b')
PROF_FIELD_RE = re.compile(
    r'\b(surf_bytes|cks_blit_us|civac_us|cks_after_us|probes|readback_us)=(\d+)\b')

# (prof field, column heading) -- the phases that consume wall time. probes and
# surf_bytes are carried alongside as scale, not as time.
PROF_PHASES = (('cks_blit_us', 'cks_blit'), ('civac_us', 'civac'),
               ('cks_after_us', 'cks_after'), ('readback_us', 'readback'))

# Display order. 'bring-up' is the Kepler/kdisp takeover proper -- the only group
# here that is actual GPU work; everything else in a witness-armed window is the
# instrument measuring it.
WCG_GROUPS = ('wc-g', 'wc-d', 'wc-h', 'wc-k', 'wcn', 'bring-up', 'other')
WCG_GROUP_NOTE = {
    'wc-g': 'glass verify (checksum passes)',
    'wc-d': 'surface verify',
    'wc-h': 'present/tear witness',
    'wc-k': 'erase witness',
    'wcn': 'window-lifecycle witness',
    'bring-up': 'kepler/kdisp takeover (real GPU work)',
    'other': 'everything else in the window',
}

# 8N1: one start bit and one stop bit per octet, so 10 line bits per byte.
SERIAL_BAUD = 115200
SERIAL_BITS_PER_BYTE = 10


def wcg_group(body):
    m = WITNESS_TAG_RE.match(body)
    if m:
        return m.group(1)
    if BRINGUP_RE.match(body):
        return 'bring-up'
    return 'other'


def wcg_stats(win_rows):
    """Cost every line in the kepler window and fold it into groups.

    Cost is the gap from the previous numerically stamped line of the same kind,
    so the first line of the window is the origin and carries no cost, and a
    mono/civil kind change breaks the chain rather than subtracting two clocks
    from each other. Both cases are counted as unmeasured lines and reported."""
    groups = {g: {'lines': 0, 'cost': 0, 'costed': 0} for g in WCG_GROUPS}
    passes = []
    profs = []
    bare_witness = []
    unmeasured = 0
    kinds = set()
    serial_lines = 0
    serial_bytes = 0

    prev = None
    pending = []
    for r in win_rows:
        body = r['body']
        group = wcg_group(body)
        if WITNESS_TAG_RE.match(body):
            serial_lines += 1
            serial_bytes += len(r['line']) + 1  # + the newline that also went out

        if r['ts'] is None:
            # Deferred or '?ms': carried as context for the next stamped line.
            pending.append(r)
            if WITNESS_TAG_RE.match(body):
                bare_witness.append(r)
            groups[group]['lines'] += 1
            continue

        kinds.add(r['kind'])
        cost = None
        if prev is not None and prev['kind'] == r['kind']:
            cost = r['ts'] - prev['ts']
        if cost is None:
            unmeasured += 1
        else:
            groups[group]['cost'] += cost
            groups[group]['costed'] += 1
        groups[group]['lines'] += 1

        m = WCG_PROF_RE.match(body)
        if m:
            fields = {k: int(v) for k, v in PROF_FIELD_RE.findall(body)}
            profs.append({'win': m.group(1), 'seq': m.group(2),
                          'fields': fields, 'row': r})
        else:
            m = WCG_PASS_RE.match(body)
            if m:
                passes.append({'win': m.group(1), 'seq': m.group(2),
                               'cost': cost, 'row': r, 'prof': None,
                               'deferred': list(pending)})

        prev = r
        pending = []

    # Attach each prof line to the most recent pass with the same win/seq. A
    # prof line whose pass is missing (truncated capture) is reported, not
    # silently merged into the neighbour.
    orphan_profs = []
    for p in profs:
        for cand in reversed(passes):
            if cand['win'] == p['win'] and cand['seq'] == p['seq'] and cand['prof'] is None:
                cand['prof'] = p['fields']
                break
        else:
            orphan_profs.append(p)

    stamped = [r for r in win_rows if r['ts'] is not None]
    span = stamped[-1]['ts'] - stamped[0]['ts'] if len(stamped) > 1 else 0
    wcg_pass_cost = sum(p['cost'] for p in passes if p['cost'] is not None)

    return {
        'span': span,
        'first': stamped[0]['ts'] if stamped else None,
        'last': stamped[-1]['ts'] if stamped else None,
        'lines': len(win_rows),
        'deferred': sum(1 for r in win_rows if r['kind'] is None),
        'filemeta': sum(1 for r in win_rows if r['kind'] == 'filemeta'),
        'unknown': sum(1 for r in win_rows if r['kind'] == 'unknown'),
        'kinds': kinds,
        'groups': groups,
        'passes': passes,
        'orphan_profs': orphan_profs,
        'has_prof': bool(profs),
        'bare_witness': bare_witness,
        'unmeasured': unmeasured,
        'pass_cost': wcg_pass_cost,
        'wcg_other_lines': groups['wc-g']['lines'] - len(passes),
        'wcg_other_cost': groups['wc-g']['cost'] - wcg_pass_cost,
        'serial_lines': serial_lines,
        'serial_bytes': serial_bytes,
        'serial_ms': serial_bytes * SERIAL_BITS_PER_BYTE * 1000.0 / SERIAL_BAUD,
    }


def pct(part, whole):
    return f"{100.0 * part / whole:5.1f}%" if whole else "    --"


def print_wcg_stats(st):
    print(f"  kepler window: {st['lines']} lines, span {st['span']}ms "
          f"[{st['first']}ms .. {st['last']}ms]")
    notes = []
    if st['deferred']:
        notes.append(f"{st['deferred']} deferred")
    if st['unknown']:
        notes.append(f"{st['unknown']} unknown-time")
    if st['filemeta']:
        notes.append(f"{st['filemeta']} file-meta")
    if st['unmeasured']:
        notes.append(f"{st['unmeasured']} unmeasured (window origin or clock-kind change)")
    if notes:
        print(f"    lines carrying no cost: {', '.join(notes)}")
    if 'civil' in st['kinds'] and 'mono' not in st['kinds']:
        print("    NOTE: civil-time stamps only -- resolution is 1 SECOND, not 1 ms. "
              "Costs below are quantised accordingly.")
    elif 'civil' in st['kinds']:
        print("    NOTE: window mixes monotonic and civil stamps; the kind change is "
              "not costed (never subtract two different clocks).")
    for r in st['bare_witness']:
        print(f"    deferred witness line (its time is inside the NEXT cost): "
              f"~ {trunc(r['line'])}")
    print("")

    total = sum(g['cost'] for g in st['groups'].values())
    costed = sum(g['costed'] for g in st['groups'].values())
    print(f"    {'group':<10} {'lines':>6} {'cost':>10} {'share':>7} {'ms/line':>8}  what it is")
    for g in WCG_GROUPS:
        e = st['groups'][g]
        if not e['lines']:
            continue
        mean = f"{e['cost'] / e['costed']:.2f}" if e['costed'] else "--"
        print(f"    {g:<10} {e['lines']:>6} {str(e['cost']) + 'ms':>10} "
              f"{pct(e['cost'], st['span']):>7} {mean:>8}  {WCG_GROUP_NOTE[g]}")
    mean = f"{total / costed:.2f}" if costed else "--"
    print(f"    {'accounted':<10} {st['lines']:>6} {str(total) + 'ms':>10} "
          f"{pct(total, st['span']):>7} {mean:>8}  (must equal the window span)")
    if total != st['span']:
        print(f"    RECONCILE: accounted {total}ms != span {st['span']}ms "
              f"-- {st['unmeasured']} line(s) could not be costed")
    # ms/line is the group's mean per costed line. It is here because the witness
    # cost is not only the big passes: on s73 the identical 229 kepler/kdisp lines
    # cost 0.69 ms each on a witness-off boot and 6.28 ms each on the witness-armed
    # one, so the bring-up GROUP inflates without any single gap looking large.
    # A group whose ms/line jumps between two builds is paying a per-print tax.
    print("    ms/line = mean cost per costed line in that group; a group whose ms/line")
    print("      moves between builds is paying a distributed per-print cost, not a block.")
    print("")

    if st['passes']:
        print("    wc-g passes (cost = gap from the previous stamped line)")
        print(f"      {'#':>3} {'win/seq':>9} {'cost':>10}")
        for i, p in enumerate(st['passes'], 1):
            cost = f"{p['cost']}ms" if p['cost'] is not None else "(no cost)"
            print(f"      {i:>3} {p['win'] + '/' + p['seq']:>9} {cost:>10}")
        print(f"      {'':>3} {'TOTAL':>9} {str(st['pass_cost']) + 'ms':>10}  "
              f"({len(st['passes'])} passes)")
        if st['wcg_other_lines']:
            print(f"      {'':>3} {'other':>9} {str(st['wcg_other_cost']) + 'ms':>10}  "
                  f"({st['wcg_other_lines']} wc-g rollup/non-sample lines)")
        print("")

    if st['has_prof']:
        print_wcg_prof_table(st)
    else:
        print("    prof lines: ABSENT -- gap-only decomposition. A build emitting")
        print("      '[wc-g] prof win=.. seq=.. surf_bytes=.. cks_blit_us=.. civac_us=..")
        print("       cks_after_us=.. probes=.. readback_us=..' after each sample would")
        print("      split each pass cost into phases; without it each pass cost is an")
        print("      UPPER BOUND that also contains whatever else ran in that gap.")
        print("")

    print(f"    serial overhead (ESTIMATE, not measured): {st['serial_lines']} "
          f"witness-tagged lines, {st['serial_bytes']} bytes")
    print(f"      {st['serial_bytes']}B x {SERIAL_BITS_PER_BYTE} bits / {SERIAL_BAUD} baud "
          f"= {st['serial_ms']:.0f}ms  ({pct(st['serial_ms'], st['span']).strip()} of the window)")
    print("      assumes one newline per line and no flow-control stalls; it bounds the")
    print("      transmit time of the witness text itself, NOT the work behind it.")
    print("")


def print_wcg_prof_table(st):
    profiled = [p for p in st['passes'] if p['prof']]
    print(f"    wc-g phase table (from '[wc-g] prof' lines; {len(profiled)}/"
          f"{len(st['passes'])} passes profiled)")
    head = (f"      {'#':>3} {'win/seq':>8} {'surf_KiB':>9}" +
            ''.join(f" {name:>10}" for _, name in PROF_PHASES) +
            f" {'phases':>10} {'cost':>9} {'remainder':>10}")
    print(head)
    totals = {key: 0 for key, _ in PROF_PHASES}
    total_cost = 0
    total_phase = 0
    for i, p in enumerate(st['passes'], 1):
        if not p['prof']:
            continue
        f = p['prof']
        phase_us = sum(f.get(key, 0) for key, _ in PROF_PHASES)
        for key, _ in PROF_PHASES:
            totals[key] += f.get(key, 0)
        cost = p['cost']
        row = (f"      {i:>3} {p['win'] + '/' + p['seq']:>8} "
               f"{f.get('surf_bytes', 0) // 1024:>9}")
        for key, _ in PROF_PHASES:
            row += f" {f.get(key, 0) / 1000.0:>8.1f}ms"
        row += f" {phase_us / 1000.0:>8.1f}ms"
        if cost is None:
            row += f" {'(no cost)':>9} {'--':>11}"
        else:
            total_cost += cost
            total_phase += phase_us
            row += f" {str(cost) + 'ms':>9} {cost - phase_us / 1000.0:>8.1f}ms"
        print(row)
    row = f"      {'':>3} {'BATTERY':>8} {'':>9}"
    for key, _ in PROF_PHASES:
        row += f" {totals[key] / 1000.0:>8.1f}ms"
    row += (f" {total_phase / 1000.0:>8.1f}ms {str(total_cost) + 'ms':>9} "
            f"{total_cost - total_phase / 1000.0:>8.1f}ms")
    print(row)
    print("      remainder = gap-derived pass cost minus the summed phases: time inside")
    print("      the pass that no prof counter claims. It is UNATTRIBUTED, not idle.")
    for p in st['orphan_profs']:
        print(f"      orphan prof (no matching sample line): win={p['win']} seq={p['seq']}")
    print("")


def wcg_mode(filepath, boot_sel):
    return wcg_report(filepath, read_capture(filepath), boot_sel)


def wcg_report(label, content, boot_sel):
    rows = load_rows(content)
    if not refuse_unless_logts(label, rows, '--wcg'):
        return False

    print(f"--- wcg {label} ---")
    segments = segment_by_hz(rows)
    if boot_sel is not None and not (1 <= boot_sel <= len(segments)):
        print(f"  --boot {boot_sel}: capture has {len(segments)} boot(s)")
        return False

    windows = 0
    for n, (hz, chunk) in enumerate(segments, 1):
        if boot_sel is not None and n != boot_sel:
            continue
        boot_label = f"boot {n} (hz={hz})" if hz else f"boot {n} (hz unknown)"
        print(boot_label)
        window = find_kepler_window(chunk)
        if isinstance(window, str):
            print(f"  kepler window: {window}\n")
            continue
        start, end = window
        windows += 1
        print_wcg_stats(wcg_stats(chunk[start:end + 1]))

    if not windows:
        print(f"{label}: no kepler window in any boot; nothing to decompose")
        return False
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

# --wcg fixture 1 is REAL. It is the kepler window of the LAST boot in
# ~/unaos-bench/capture/rmbp-gr16-s73/ttyUSB0.log -- the witness-armed,
# full-millisecond boot of GR16/s73, the capture that
# docs/dev/OS/01_BOOT_HAL/bootpace.md section 10g was written from.
#
# It is TRIMMED, not edited. Runs of consecutive lines in the same group had
# their interior dropped, which cannot move a group total: the merged gap lands
# on the run's last line and that line is in the same group. Every wc-* line and
# both window anchors survive. 437 lines became 46, and every expected value in
# wcg_expect() below is bit-identical to what the untrimmed window produces --
# checked against the capture when the fixture was cut.
#
# The capture path is deliberately NOT read at runtime. A fixture that needs a
# bench directory to still exist is a fixture that quietly stops running.
WCG_FIXTURE_S73 = """\
[   2855ms] [NVIDIA] Initializing Kepler GPU at BDF 1:0:0
[   2861ms] :: x86 mmio-map: 0x90000000..0xa0000000 uc=113 (PAT PA3) wc-kept=15 ::
[   4052ms] :: kdisp: fb-draw done ::
[   4067ms] :: [    2392 ms] portsw:flip ::
[   4067ms] :: kdisp: console-repaint rows=4 ::
[   4082ms] [wc-a] create win=1 asid=0xffffff01 surf=1312x736 stride=5248 scale=1x at (784,457) z=1
[   6955ms] [wc-g] win=1 seq=0 own=no scale=1x app=0xcbf29ce484222325 blit=0x2088f1de4724e325 civac=0x2088f1de4724e325 after=0x2088f1de4724e325 fbbad=0/965632 us=5131 rectscan_us=6814 slow=no -> CLEAN
[   6955ms] [wc-h] win=1 box=1314x750 span=750 band=no bytes=3942000 compose_us=2282 present_us=2660 rectscan_us=6944 torn=no -> BUFFERED
[   6955ms] [wc-a] composite windows=1 drawn=1
[   9831ms] [wc-g] win=1 seq=1 own=yes scale=1x app=0x6ea90580b6e52525 blit=0x6ea90580b6e52525 civac=0x6ea90580b6e52525 after=0x6ea90580b6e52525 fbbad=0/965632 us=429 rectscan_us=6814 slow=no -> CLEAN
[   9831ms] [wc-h] win=1 box=1314x750 span=64 band=yes bytes=336384 compose_us=194 present_us=233 rectscan_us=592 torn=no -> BUFFERED
[  10328ms] [wc-d] verify win=1 surf=1312x736 band=0..64 scale=1x at (784,457) panel=2880x1800 checked=83968 bad_cache=0 bad_ram=0 ram_indep=no moved=0 nonzero=8300 cksum=0x6ea90580b6e52525 first=none -> PASS
[  10333ms] [wcn] rollup scope=live wins=1 att=1 comp=2 hid=0 bel=0 stale=0 passes=2 aborted=0 att_rate=0.1/s comp_rate=0.2/s span=9967ms -> LIVE
[  10333ms] [wc-x] console-window win=1 panel=2880x1800 surf=1312x736 box=1314x750 at (783,444) cell=16x16 cols=82 rows=46
[  13194ms] [wc-g] win=1 seq=2 own=yes scale=1x app=0x21f6b51b832d1525 blit=0x21f6b51b832d1525 civac=0x21f6b51b832d1525 after=0x21f6b51b832d1525 fbbad=0/965632 us=846 rectscan_us=6814 slow=no -> CLEAN
[  13194ms] [wc-h] win=1 box=1314x750 span=128 band=yes bytes=672768 compose_us=385 present_us=460 rectscan_us=1185 torn=no -> BUFFERED
[  13194ms] [wc-x] console-window panic-fallback armed win=1 (panic paints the PANEL, not the window)
[  16072ms] [wc-g] win=1 seq=3 own=yes scale=1x app=0x21f6b51b832d1525 blit=0x21f6b51b832d1525 civac=0x21f6b51b832d1525 after=0x21f6b51b832d1525 fbbad=0/965632 us=4924 rectscan_us=6814 slow=no -> CLEAN
[  16072ms] [wc-g] rollup win=1 scope=window samples=4 coher=0 race=0 blit=0 clean=4 slow=0 maxus=5131 frame_us=16667 -> CLEAN
[  16072ms] [wc-h] win=1 box=1314x750 span=750 band=no bytes=3942000 compose_us=2263 present_us=2660 rectscan_us=6944 torn=no -> BUFFERED
[  16072ms] [wcn] rollup scope=live wins=1 att=2 comp=2 hid=0 bel=0 stale=0 passes=2 aborted=0 att_rate=0.3/s comp_rate=0.3/s span=5740ms -> LIVE
[  16073ms] [comp2] rollup passes=2 pass_us=2863838 max_us=2872941 sprite_us=0 wait_us=0 blit_us=2863836 cache_us=0 bytes_pp=2307384 dmg_px_pp=576846 box_px_pp=985500 rate=0.3/s span=5740ms
[  16079ms] [wc-h] win=1 box=1314x750 span=96 band=yes bytes=504576 compose_us=289 present_us=347 rectscan_us=888 torn=no -> BUFFERED
[  16079ms] [wc-x] activate panel=2880x1800 console_win=1
[  16085ms] [wc-h] rollup win=1 scope=window-band emit=1 age_ms=11986 pop=budgeted samples=4 budget=4 pop=all-presents torn=0 declines=0 fixture=0 whole=3 banded=4 lines=6 minspan=64 minspan_bytes=336384 maxpresent_us=2660 pop=constant frame_us=16667 -> TEAR-FREE
[  16108ms] [wc-g] win=2 seq=0 own=no scale=8x app=0xcbf29ce484222325 blit=0x47b750fe2093a4da civac=0x47b750fe2093a4da after=0x47b750fe2093a4da fbbad=0/6144 us=1236 rectscan_us=4740 slow=no -> CLEAN
[  16108ms] [wc-h] win=2 box=770x526 span=526 band=no bytes=1620080 compose_us=138 present_us=1097 rectscan_us=4870 torn=no -> BUFFERED
[  16121ms] [wc-x] spawn-place win=2 box=770x526 at (2102,1104) (created in place, no move)
[  16144ms] [wc-g] win=2 seq=1 own=yes scale=8x app=0x47b750fe2093a4da blit=0x47b750fe2093a4da civac=0x47b750fe2093a4da after=0x47b750fe2093a4da fbbad=0/6144 us=1233 rectscan_us=4740 slow=no -> CLEAN
[  16144ms] [wc-h] win=2 box=770x526 span=526 band=no bytes=1620080 compose_us=135 present_us=1097 rectscan_us=4870 torn=no -> BUFFERED
[  18465ms] [wc-d] verify win=2 surf=96x64 band=none scale=8x at (2103,1117) panel=2880x1800 checked=393216 bad_cache=0 bad_ram=0 ram_indep=no moved=0 nonzero=91840 cksum=0x47b750fe2093a4da first=none -> PASS
[  18478ms] [wc-x] present win=2 rows=1104..1630 ok=true
[  18485ms] [wc-g] win=3 seq=0 own=no scale=8x app=0xcbf29ce484222325 blit=0xda5b3a56c0971925 civac=0xda5b3a56c0971925 after=0xda5b3a56c0971925 fbbad=0/64 us=22 rectscan_us=592 slow=no -> CLEAN
[  18485ms] [wc-h] win=3 box=66x78 span=78 band=no bytes=20592 compose_us=7 present_us=13 rectscan_us=722 torn=no -> BUFFERED
[  18485ms] [wc-a] create win=3 asid=0x0 surf=8x8 stride=32 scale=8x at (9,21) z=3
[  18485ms] [wc-g] win=3 seq=1 own=yes scale=8x app=0xda5b3a56c0971925 blit=0xda5b3a56c0971925 civac=0xda5b3a56c0971925 after=0xda5b3a56c0971925 fbbad=0/64 us=29 rectscan_us=592 slow=no -> CLEAN
[  18485ms] [wc-h] win=3 box=66x78 span=78 band=no bytes=20592 compose_us=7 present_us=21 rectscan_us=722 torn=no -> BUFFERED
[  18508ms] [wc-d] verify win=3 surf=8x8 band=none scale=8x at (9,21) panel=2880x1800 checked=4096 bad_cache=0 bad_ram=0 ram_indep=no moved=0 nonzero=4096 cksum=0xda5b3a56c0971925 first=none -> PASS
[  18514ms] [wc-k] erase box=66x78 staged=yes rowbytes=264 runs=78 contig=yes compose_us=0 present_us=14 rectscan_us=722 torn=no -> BUFFERED
[  18515ms] [wc-g] win=3 seq=1 own=no scale=8x app=0xda5b3a56c0971925 blit=0xda5b3a56c0971925 civac=0xda5b3a56c0971925 after=0xda5b3a56c0971925 fbbad=0/64 us=22 rectscan_us=592 slow=no -> CLEAN
[  18515ms] [wc-h] rollup win=3 scope=window emit=1 age_ms=30 pop=budgeted samples=4 budget=4 pop=all-presents torn=0 declines=0 fixture=0 whole=4 banded=0 lines=3 minspan=0 minspan_bytes=0 maxpresent_us=21 pop=constant frame_us=16667 -> TEAR-FREE
[  18521ms] [wc-a] close win=3
[  18527ms] [wc-k] erase box=66x78 staged=yes rowbytes=264 runs=78 contig=yes compose_us=0 present_us=21 rectscan_us=722 torn=no -> BUFFERED
[  18533ms] [wc-x] move-vacate win=3 scale=8x from=(8,8) to=(90,8) box=66x78 painted=true desktop=5/5 stale=0/5 -> PASS
[  19984ms] [NVIDIA] Initialization complete (Phases 1-4)
[  20127ms] :: GPACE: span=17267ms anchor=enum:p1 since-entry=20114ms hz=2693817020 build=kepler+takeover+fifo+ivb+wc+ == the pci-usb d= split ::
"""

# --wcg fixture 2 is SYNTHETIC, and exists for the case no capture has yet: a
# build that emits '[wc-g] prof' after each sample. It also carries the two
# things the real window happens not to contain -- a contention-deferred witness
# line (unprefixed, so its time lands inside the NEXT cost) and a '?ms' line
# inside the window -- so the no-fabrication rules are exercised here too.
WCG_FIXTURE_PROF = """\
[      ?ms] serial: early init
[      0ms] bootpace: entry
[     10ms] [NVIDIA] Initializing Kepler GPU at BDF 1:0:0
[    110ms] :: kepler: takeover complete ::
[   1110ms] [wc-g] win=1 seq=0 own=no scale=1x app=0x1 blit=0x1 civac=0x1 after=0x1 fbbad=0/16 us=10 rectscan_us=20 slow=no -> CLEAN
[   1110ms] [wc-g] prof win=1 seq=0 surf_bytes=3942000 cks_blit_us=120000 civac_us=8000 cks_after_us=115000 probes=4 readback_us=750000
[wc-h] win=1 box=8x8 span=8 band=no bytes=256 compose_us=1 present_us=2 rectscan_us=4 torn=no -> BUFFERED
[   2110ms] [wc-g] win=1 seq=1 own=yes scale=1x app=0x1 blit=0x1 civac=0x1 after=0x1 fbbad=0/16 us=10 rectscan_us=20 slow=no -> CLEAN
[   2110ms] [wc-g] prof win=1 seq=1 surf_bytes=3942000 cks_blit_us=118000 civac_us=8000 cks_after_us=114000 probes=4 readback_us=700000
[   2610ms] [wc-d] verify win=1 surf=8x8 band=none scale=1x at (0,0) panel=8x8 checked=64 bad_cache=0 bad_ram=0 ram_indep=no moved=0 nonzero=64 cksum=0x1 first=none -> PASS
[      ?ms] :: kdisp: stamp lost ::
[   2620ms] :: kdisp: landed trace [0] ::
[   2630ms] :: GPACE: span=2620ms anchor=enum:p1 since-entry=2630ms hz=123456 build=kepler+takeover+fifo+ivb+wc+ == the pci-usb d= split ::
"""

# --wcg fixture 3: a capture with witness lines and a kepler window but no logts
# prefix anywhere. There is no measurement in it, so the only honest output is a
# refusal -- never a decomposition inferred from line counts or from the us=
# fields the witness happens to print.
WCG_FIXTURE_NO_LOGTS = """\
[NVIDIA] Initializing Kepler GPU at BDF 1:0:0
[wc-g] win=1 seq=0 own=no scale=1x app=0x1 blit=0x1 civac=0x1 after=0x1 fbbad=0/16 us=10 rectscan_us=20 slow=no -> CLEAN
[wc-d] verify win=1 surf=8x8 band=none scale=1x at (0,0) panel=8x8 checked=64 bad_cache=0 bad_ram=0 ram_indep=no moved=0 nonzero=64 cksum=0x1 first=none -> PASS
:: GPACE: span=2620ms anchor=enum:p1 since-entry=2630ms hz=123456 build=kepler+takeover+fifo+ivb+wc+ == the pci-usb d= split ::
"""

# --wcg fixture 4: properly stamped, but the kepler anchors never appear (a
# default(no-gpu-knobs) boot). Nothing to decompose is also a refusal, not an
# empty table that reads like a zero.
WCG_FIXTURE_NO_WINDOW = """\
[      0ms] bootpace: entry
[     12ms] :: ehci: init ::
[   1000ms] :: BPACE: total gui=1000ms ftdi=none n=23 dropped=0 hz=1 result=LEDGER ::
"""


def wcg_window_stats(text, boot=1):
    """Costed stats for one fixture's kepler window, or None when it has none."""
    rows = load_rows(text)
    _hz, chunk = segment_by_hz(rows)[boot - 1]
    window = find_kepler_window(chunk)
    if isinstance(window, str):
        return None
    start, end = window
    return wcg_stats(chunk[start:end + 1])


def wcg_expect(st):
    """Expected values for the real s73 window. These are METAL numbers: the
    four ~2.87 s [wc-g] passes and the [wc-d] verifies that section 10g named,
    read back out of the capture by this code path."""
    g = st['groups']
    return [
        ('window span', st['span'], 17272),
        ('wc-g cost', g['wc-g']['cost'], 11542),
        ('wc-d cost', g['wc-d']['cost'], 2841),
        ('wc-h cost', g['wc-h']['cost'], 12),
        ('wc-k cost', g['wc-k']['cost'], 12),
        ('wcn cost', g['wcn']['cost'], 5),
        ('bring-up cost', g['bring-up']['cost'], 2642),
        ('other cost', g['other']['cost'], 218),
        ('accounted == span', sum(e['cost'] for e in g.values()), 17272),
        ('per-pass costs', [p['cost'] for p in st['passes']],
         [2873, 2876, 2861, 2878, 23, 23, 7, 0, 1]),
        ('pass total', st['pass_cost'], 11542),
        ('prof lines present', st['has_prof'], False),
        ('witness-tagged lines', st['serial_lines'], 28),
        ('witness-tagged bytes', st['serial_bytes'], 4821),
    ]


def wcg_prof_expect(st):
    p0, p1 = st['passes']
    return [
        ('window span', st['span'], 2620),
        ('prof lines present', st['has_prof'], True),
        ('per-pass costs', [p['cost'] for p in st['passes']], [1000, 1000]),
        ('pass 1 phases (us)', sum(p0['prof'][k] for k, _ in PROF_PHASES), 993000),
        ('pass 2 phases (us)', sum(p1['prof'][k] for k, _ in PROF_PHASES), 940000),
        ('pass 1 remainder (ms)', p0['cost'] - 993, 7),
        ('pass 2 remainder (ms)', p1['cost'] - 940, 60),
        ('deferred witness lines', len(st['bare_witness']), 1),
        ('unknown-time lines in window', st['unknown'], 1),
        ('bring-up cost', st['groups']['bring-up']['cost'], 110),
        ('wc-d cost', st['groups']['wc-d']['cost'], 500),
        ('accounted == span', sum(e['cost'] for e in st['groups'].values()), 2620),
    ]


def selftest(top):
    """Fixtures for both timing modes.

    --gaps: a mixed capture where '?ms' lines must be counted but must not become
    gap endpoints, and an all-'?ms' capture that must fail.

    --wcg: the real GR16/s73 kepler window (values asserted against metal), a
    synthetic window carrying the not-yet-shipped '[wc-g] prof' lines plus a
    deferred witness line and a '?ms' line, and two captures that must be
    REFUSED -- one with no logts prefixes, one with no kepler window."""
    ok = True

    for name, text, expect_ok in (
        ('gaps: mixed (numeric + ?ms + deferred)', SELFTEST_MIXED, True),
        ('gaps: all-?ms (counter never calibrated)', SELFTEST_ALL_UNKNOWN, False),
    ):
        print(f"=== selftest: {name} ===")
        got = gaps_report(f'<{name}>', text, top)
        verdict = 'PASS' if got == expect_ok else 'FAIL'
        if got != expect_ok:
            ok = False
        print(f"=== selftest: {name}: {verdict} "
              f"(expected {'ok' if expect_ok else 'failure'}, got "
              f"{'ok' if got else 'failure'})\n")

    for name, text, expect_ok, checker in (
        ('wcg: real s73 kepler window (witness-armed, no prof lines)',
         WCG_FIXTURE_S73, True, wcg_expect),
        ('wcg: synthetic window WITH [wc-g] prof lines',
         WCG_FIXTURE_PROF, True, wcg_prof_expect),
        ('wcg: no logts prefixes (must refuse)', WCG_FIXTURE_NO_LOGTS, False, None),
        ('wcg: no kepler window (must refuse)', WCG_FIXTURE_NO_WINDOW, False, None),
    ):
        print(f"=== selftest: {name} ===")
        got = wcg_report(f'<{name}>', text, None)
        case_ok = got == expect_ok
        if case_ok and checker:
            st = wcg_window_stats(text)
            if st is None:
                print("    BAD no kepler window found in fixture")
                case_ok = False
            else:
                for label, actual, want in checker(st):
                    good = actual == want
                    if not good:
                        case_ok = False
                    print(f"    {'ok ' if good else 'BAD'} {label}: "
                          f"got {actual!r}, want {want!r}")
        if not case_ok:
            ok = False
        print(f"=== selftest: {name}: {'PASS' if case_ok else 'FAIL'} "
              f"(expected {'ok' if expect_ok else 'refusal'}, got "
              f"{'ok' if got else 'refusal'})\n")

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
    parser.add_argument("--wcg", action="store_true",
                        help="decompose the witness cost inside the kepler window: per-instrument "
                             "attribution, per-pass [wc-g] costs, the [wc-g] prof phase table when "
                             "the build emits one, and a serial-overhead estimate")
    parser.add_argument("--boot", type=int, default=None,
                        help="with --wcg, restrict the report to boot N (1-based)")
    parser.add_argument("--top", type=int, default=15,
                        help="how many gaps to list per table (default 15)")
    parser.add_argument("--selftest", action="store_true",
                        help="run the synthetic prefix-parsing fixtures and exit")
    args = parser.parse_args()

    if args.selftest:
        sys.exit(0 if selftest(args.top) else 1)

    if not args.logs:
        parser.error("no log files given")

    if args.wcg:
        ok = True
        for log_file in args.logs:
            if not wcg_mode(log_file, args.boot):
                ok = False
        sys.exit(0 if ok else 1)

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
