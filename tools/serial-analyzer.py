#!/usr/bin/env python3
"""
serial-analyzer.py — one reader for every UnaOS bench serial capture.

CANONICAL COPY: tools/serial-analyzer.py in the UnaOS repo (git-tracked).  The
bench copy at ~/unaos-bench/tools/serial-analyzer.py is a byte-identical copy of
it; edit the repo copy and re-copy, never the other way round.  Until GR18 the
two were separate lineages with DISJOINT feature sets — the repo copy had the
timing modes (--gaps, --wcg) and could not split an x86 capture into boots at
all, the bench copy had the census and the GR18 sections and knew nothing about
gap costing.  A caller had to know which file it was holding.  This file is the
merge; nothing was dropped from either side.

WHAT IT DOES

  (default)     split the capture into boots, pull every instrument witness line
                out of each boot by an explicit family table, and classify
                defects.  Loud failure on 0 boots / 0 witnesses.
  --wxprobe     the 8-line WXPROBE reconnaissance block, DIFFed across boots.
  --slowxfer    EPACE-TRIM M8 SLOW-XFER, with the control request decoded.
  --smc         SMC-BATT gap/busy/late plus the SMC-SCOUT '#KEY' index walk.
  --gaps        the largest inter-line time gaps in a logts-stamped capture.
  --wcg         the witness cost inside the kepler window, decomposed by
                instrument, plus the whole-boot pay-as-you-go (paygo) census.
  --selftest    both fixture sets, in one run, exit 0 required.

BOOT SPLITTING (explicit marker table, one entry per capture format):
  * x86_64  / rMBP    ':: X86_64 Memory Init ::'      once per boot
  * aarch64 / Pi 4    ':: AARCH64 Memory Init ::'     once per boot
  * Orin    (legacy)  '... MARK ... boot<N> ...'      once per boot

Each marker carries a list of EPOCH ANCHORS: the earliest per-boot line of
that platform.  The parser walks back from the marker to the earliest anchor
above it (bounded by the previous boot) so a boot's pre-marker head is
attributed to the boot it belongs to.  This matters: on the Pi the whole V3D
bring-up campaign runs ~1600 lines BEFORE ':: AARCH64 Memory Init ::', and on
x86 the fb-wc / APIC / SMEP / SPLASH head runs ~9 lines before its marker.  The
Orin 'MARK ... boot<N>' entry is the older lineage's splitter, kept as a
platform path: do not fix one rig by breaking another.

The TIMING modes split differently and deliberately so.  --gaps and --wcg cut on
the per-boot 'hz=' token (segment_by_hz) because they need the boot's own
timestamp origin, not its witness inventory, and an hz cut is refined to the
visible stamp RESET.  Two splitters, two questions; both are exercised by
--selftest.  Since GR19 the two are also CROSS-CHECKED: when the marker table
finds a different boot count from the hz cut, the timing modes say so and exit
EXIT_FINDING instead of printing a per-boot table that blends two boots.

EXIT CODES: 0 clean; 1 refused (no measurement was possible — an unstamped
capture, a --boot out of range, no kepler window); 2 no boots parsed; 3 no
witnesses; 4 the section's wire is absent from the capture; 5 the run found the
thing it exists to catch.  ALL SIX modes carry the 5 channel — --gaps and --wcg
were report-only until GR19, which meant a caller reading $? from them was
reading a constant.

Witness extraction is an EXPLICIT FAMILY TABLE (WITNESS_FAMILIES), not a shape
heuristic.  The predicate this replaced was `::.*witness.*::`, which captured
only lines that self-declare '== witness ::' and therefore silently dropped
every 'xHCI: ccs-margin' line (no '::' at all), every bracket-tagged Pi line
([spread5], [wc-h], [prio], ...), and the ':: GPACE: OVERLAP ... ::' tripwire.
Lines that match no family are NOT silently discarded — they are counted and
sampled in the 'unclassified' report, and the witness-shaped subset of them is
called out as a coverage gap so the table can be extended.

Standing law this tool must obey: an instrument's silence is evidence only if
the instrument could execute in the state it reports on.  Zero boots or zero
witnesses is therefore a LOUD failure on stderr with a non-zero exit — never a
printed header and a clean exit.

LOGTS PREFIXES, AND THE ONE STRIPPING LAW.  Captures gained a per-line stamp —
'[   1851ms] ', '[      ?ms] ', '[15:30:45Z] '.  The census/GR18 lineage was
anchored with '^\\s*::' throughout and NONE of it matched a stamped line: the tool
reported '0 boots, 0 witnesses, 103 families absent' on every current x86
capture — not a stale reader, a DEAD one, and a quiet one, because the census
still printed a clean-looking page.  Matching there is now done against a
PREFIX-STRIPPED probe copy of each line while the original (stamp included) is
what gets stored and printed.

The timing lineage never had that blindness — it PARSES the stamp (parse_ts) and
matches its patterns against the parsed body — but the two implementations of
"where does the stamp end" had drifted apart in their tolerances, which is the
same bug waiting to happen from the other side.  There is now one law:
LOGTS_PREFIX_RE defines the stamp, strip_logts() removes it, the parse_ts
readers accept exactly the shapes it accepts, and --selftest asserts the two
agree on all three forms.  The stamp is evidence; it just is not part of any
pattern.
"""

import sys
import re
import io
import argparse
import contextlib
from collections import Counter, OrderedDict

EXIT_OK = 0
EXIT_USAGE = 1
EXIT_NO_BOOTS = 2
EXIT_NO_WITNESSES = 3
# A section that parsed nothing at all: the wire is absent from the capture, so
# the section cannot answer the question it was run to answer.
EXIT_NO_DATA = 4
# A section parsed its wire and found the thing it exists to catch.  Non-zero so
# an automated caller cannot mistake a finding for a clean run.
EXIT_FINDING = 5


def strip_control_bytes(text):
    # Strip \x00-\x1F except \n, \r, \t.  Captures carry raw control bytes;
    # this is also why the bench inspects them with awk, never bare grep.
    return re.sub(r'[\x00-\x08\x0b-\x0c\x0e-\x1f]', '', text)


# The three logts prefix shapes, as the serial taps emit them:
#   '[   1851ms] '  monotonic ms since kernel entry
#   '[      ?ms] '  prefixed but the counter could not answer
#   '[15:30:45Z] '  civil time
# Anchored at the line head and matched ONCE — a stamp only ever appears there,
# and a body that happens to contain a bracketed number (e.g. ':: [ 2392 ms]
# portsw:flip ::', which the TIMELINE family exists to claim) must survive.
LOGTS_PREFIX_RE = re.compile(
    r'^\s*\[\s*(?:\d+\s*ms|\?\s*ms|\d{2}:\d{2}:\d{2}Z)\s*\]\s?')


def strip_logts(line):
    """Return the line body with any leading logts stamp removed.

    Matching-only.  The caller keeps the original line: the stamp is the whole
    reason --gaps-style timing work is possible, so it is never thrown away,
    it is merely not part of any pattern."""
    return LOGTS_PREFIX_RE.sub('', line, count=1)


# ---------------------------------------------------------------------------
# Boot splitting
# ---------------------------------------------------------------------------

BOOT_MARKERS = [
    {
        'platform': 'x86_64',
        'label': ':: X86_64 Memory Init ::',
        'marker': re.compile(r'^\s*::\s*X86_64 Memory Init\s*::'),
        'epoch_anchors': [
            re.compile(r'^\s*::\s*x86 fb-wc:'),
            re.compile(r'^\s*APIC:\s*x2APIC software-enabled'),
        ],
        'number_re': None,
    },
    {
        'platform': 'aarch64',
        'label': ':: AARCH64 Memory Init ::',
        'marker': re.compile(r'^\s*::\s*AARCH64 Memory Init\s*::'),
        'epoch_anchors': [
            re.compile(r'Read start4\.elf'),
            re.compile(r'arm_loader:\s*Starting ARM with'),
            re.compile(r'^\s*::\s*MAILBOX:\s*framebuffer'),
        ],
        'number_re': None,
    },
    {
        # Legacy Orin capture format, e.g.
        #   2026-07-19T22:29:39Z MARK MARK R23s1 boot12 ORIN ...
        # Kept working deliberately: do not fix one rig by breaking another.
        # Note this must NOT match the bench's session banner
        #   === SQUAWK MARK 2026-08-01T16:28:57Z session-start ===
        # which has ' MARK ' but no ' boot<N>'.
        'platform': 'orin',
        'label': 'MARK ... boot<N>',
        'marker': re.compile(r'\sMARK\s.*\sboot\d+'),
        'epoch_anchors': [],
        'number_re': re.compile(r'boot(\d+)'),
    },
]


# ---------------------------------------------------------------------------
# Witness families — explicit list.  First match wins, so tighter families
# must be listed above looser ones (CCS-MARGIN above XHCI, SCHED-X86 above
# SCHED).  The 'platform' column is documentary and drives the coverage
# report; matching itself is platform-independent.
# ---------------------------------------------------------------------------

def _f(name, platform, pattern):
    return (name, platform, re.compile(pattern))


WITNESS_FAMILIES = [
    # ---- cross-platform ---------------------------------------------------
    _f('SERWIT-1',        'any',     r'^\s*::\s*SERWIT-1\b'),
    _f('SERWIT-2',        'any',     r'^\s*::\s*SERWIT-2\b'),

    # ---- x86_64 / rMBP ----------------------------------------------------
    _f('BPACE',           'x86_64',  r'^\s*::\s*BPACE:'),
    _f('EPACE',           'x86_64',  r'^\s*::\s*EPACE:'),
    # Three line shapes share this prefix: the class split
    # ('xtail= bench= detect= igpu= kepler= sdhc= nic= resid= == witness ::'),
    # the self-check ('span= anchor= since-entry= hz= build= ...'), and the
    # ':: GPACE: OVERLAP ... ::' tripwire.  Anchored on the prefix and on NO
    # interior field, so the ten-fragment 'build=' (therm+/pcilink+/vrom+ and
    # the 'default(no-gpu-knobs)' case) and the ms/cy unit switch cannot
    # unmatch the family.
    _f('GPACE',           'x86_64',  r'^\s*::\s*GPACE:'),
    # No '::' anywhere on these; the old heuristic could never see them.
    # Covers the healthy line (now carrying 'ppc=', and 'latest=none
    # margin_ms=none' when no port ever asserted) and all three '!!' variants:
    # BLOWN, TIGHT, and LATE — the last with 't_seen_ms=' and 'short_by_ms<='.
    # Field names are deliberately absent from the pattern.
    _f('CCS-MARGIN',      'x86_64',  r'xHCI:\s*(?:!!\s*)?ccs-margin\b'),
    # SPACE, not colon, after 'SCHED-X86' on these three.  Different producers
    # (smp.rs publish/confirm, syscall.rs bg placement) from the
    # ':: SCHED-X86: ...' dispatch lines the next entry claims.  Named
    # separately so a placement campaign that stops printing shows up as a hard
    # zero in the census instead of hiding inside a bulk SCHED-X86 count.
    _f('SCHED-X86-PLACE-CHECK', 'x86_64', r'^\s*::\s*SCHED-X86 PLACE-CHECK:'),
    _f('SCHED-X86-PLACE',       'x86_64', r'^\s*::\s*SCHED-X86 PLACE:'),
    _f('SCHED-X86-BGPLACE',     'x86_64', r'^\s*::\s*SCHED-X86 BG-PLACE:'),
    _f('SCHED-X86',       'x86_64',  r'^\s*::\s*SCHED-X86\b'),
    _f('SCHED-X86-DEPTH', 'x86_64',  r'\[schedx86\]'),
    # All seven lines of the one-shot dump share this prefix.  Line 1 now reads
    # 'NO-COMPLETIONS class=never-completed|went-quiet', line 2 gained
    # 'qtd_driven=', line 3's data-toggle field is 'tog=' (never 'dt='), and
    # line 7 is 'frindex arm= fire= post= ok= post_ok= adv= post_ms='.  None of
    # those field names is in the pattern, on purpose.
    _f('KBDWIT',          'x86_64',  r'^\s*::\s*KBDWIT:'),
    # The STOP-NOTEs ride this prefix too, including the EP0-timeout one that
    # now prints 'ASS on=/off=' where it used to print 'PSS on=/off='.
    _f('EHCI-HID',        'x86_64',  r'^\s*(?:::\s*)?EHCI-HID:'),
    _f('EHCI-CONFIG',     'x86_64',  r'^\s*(?:::\s*)?EHCI-CONFIG:'),
    _f('EHCI-SCOUT',      'x86_64',  r'^\s*::\s*EHCI-SCOUT:'),
    _f('EHCI-MT',         'x86_64',  r'^\s*::\s*EHCI-MT:'),
    _f('MOUSE',           'x86_64',  r'^\s*::\s*MOUSE-\d'),
    _f('PTR',             'x86_64',  r'^\s*::\s*PTR:'),
    _f('BOT',             'x86_64',  r'^\s*::\s*BOT:'),
    _f('STOR',            'x86_64',  r'^\s*::\s*(?:STOR-\d\b|bx-blockreq:|BLK:)'),
    _f('PORTSW',          'x86_64',  r'^\s*::\s*PORTSW-\d'),
    _f('PWR',             'x86_64',  r'^\s*::\s*PWR:'),
    _f('SMC-BATT',        'x86_64',  r'^\s*::\s*SMC-BATT:'),
    _f('SMC-SCOUT',       'x86_64',  r'^\s*::\s*SMC-SCOUT\b'),
    _f('SMC-DIAG',        'x86_64',  r'^\s*::\s*SMC-DIAG\b'),
    _f('PART',            'x86_64',  r'^\s*::\s*PART:'),
    # The beacon-save / beacon-restore / runlist-rebuild / post-bind campaign,
    # split out of the bulk KEPLER family for the same reason as the placement
    # lines above: this is the arc under test and it has to be countable on its
    # own.  Anchored on the sub-tag, not on 'restored=' / 'exit=' / 'clk=',
    # so a renamed field does not silently unmatch the family.
    # ':: kepler: witness post-bind ...' is a DIFFERENT line and stays in
    # KEPLER — the prefix here requires the sub-tag immediately after 'kepler:'.
    _f('KEPLER-RUNLIST',  'x86_64',  r'^\s*::\s*kepler:\s*'
                                     r'(?:beacon-save|beacon-restore'
                                     r'|runlist-rebuild|post-bind)\b'),
    _f('KEPLER',          'x86_64',  r'^\s*::\s*kepler:'),
    _f('KDISP',           'x86_64',  r'^\s*::\s*kdisp:'),
    _f('IGPU',            'x86_64',  r'^\s*::\s*igpu:'),
    _f('SDHC',            'x86_64',  r'\[sdhc\]'),
    # bench_ride.rs — the rMBP probes whose build fragments GPACE's 'build='
    # advertises (therm+ / pcilink+ / vrom+).
    _f('BENCH-RIDE',      'x86_64',  r'^\s*::\s*(?:therm|pcilink|vrom):'),
    _f('FLIGHTREC',       'x86_64',  r'^\s*::\s*(?:FLIGHTREC|FR)\b'),
    _f('CLOCK-X1',        'x86_64',  r'^\s*::\s*CLOCK-X1\b'),
    # GR18 / Boot V.  Eight lines per boot, read in detail by --wxprobe; the
    # family entry is what keeps them out of the 'witness-shaped but
    # unclassified' coverage gap.  Listed before SMEP because the cpu line
    # carries 'smep=1' and SMEP's own family is a word-boundary match.
    _f('WXPROBE',         'x86_64',  r'^\s*::\s*WXPROBE\s+(?:cpu|map|elf):'),
    _f('SMEP',            'x86_64',  r'^\s*::\s*SMEP\b'),
    _f('ACPI',            'x86_64',  r'^\s*::\s*ACPI:'),
    _f('BOOTLOG',         'x86_64',  r'^\s*::\s*BOOTLOG:'),
    # ':: x86 fb-wc: ...', ':: x86_64 PCI Init: ...', ':: X86_64 Memory Init ::'
    _f('X86',             'x86_64',  r'^\s*::\s*[xX]86(?:_64)?\b'),
    _f('FTDI',            'x86_64',  r'^\s*::\s*FTDI:'),
    # boot timeline stamps, e.g. ':: [   842 ms] ehci:kbd-armed ::'
    _f('TIMELINE',        'x86_64',  r'^\s*::\s*\[\s*\d+\s*ms\]'),
    _f('PCI-PROBE',       'x86_64',  r'\[PCI(?:-PROBE|-STOR)?\]|\[MSI-X\]'),
    _f('GPU-PROBE',       'x86_64',  r'\[GPU\]|\[NVIDIA\]|\[Intel\b'),
    # USB hot-plug announcer, e.g. '>>> [CONTACT ESTABLISHED] SLOT 1'
    _f('HOTPLUG',         'x86_64',  r'^\s*>>>\s'),

    # ---- aarch64 / Pi 4 ---------------------------------------------------
    # ':: V3D: ...' plus its indented bracket-tagged continuation lines,
    # e.g. '::   [v3d60] MMU_ILLEGAL_ADDR ... ::'
    _f('V3D',             'aarch64', r'^\s*::\s*(?:V3D:|\[v3d\d+)'),
    _f('PI-GENET',        'aarch64', r'^\s*::\s*PI-GENET:'),
    # not Pi-only: the rMBP capture also emits ':: PIUSB: [usbw] ... ::'
    _f('PIUSB',           'any',     r'^\s*::\s*(?:PIUSB|piusb\d+):|\[piusb\d+\]'),
    _f('SCHED',           'aarch64', r'^\s*::\s*SCHED(?:-LOAD)?:|\[sched\d+\]'
                                     r'|^\s*::\s*INPUT on core\b'),
    _f('AARCH64',         'aarch64', r'^\s*(?:::\s*)?AARCH64\b'),
    _f('MAILBOX',         'aarch64', r'^\s*::\s*MAILBOX:'),
    _f('SPREAD',          'aarch64', r'\[spread\d+\]'),
    _f('PULSE',           'aarch64', r'\[pulse\d+\]'),
    _f('PRIO',            'aarch64', r'\[prio\]'),
    _f('PSTRIP',          'aarch64', r'\[pstrip\]'),
    _f('KILLBOUND',       'aarch64', r'\[killbound\]|^\s*::\s*KILLBOUND:'),
    _f('SKILL',           'aarch64', r'\[skill\]|^\s*::\s*SKILL-\d'),
    _f('VUG',             'aarch64', r'\[vugmin\]|\[uvug\d+\]|\[u\d+fix\]'
                                     r'|^\s*::\s*(?:VUG|UVUG|uvug\d+)\b'),
    _f('FLUID',           'aarch64', r'\[fluid\d+\]'),
    _f('COMPOSITE',       'aarch64', r'\[comp\d+\]'),
    _f('WEDGE',           'aarch64', r'\[wedge\d+\]'),
    _f('SPINHUNT',        'aarch64', r'\[spinhunt\]|^\s*::\s*SPINHUNT\b'),
    _f('KEYSTAT',         'aarch64', r'\[keystat\]'),
    _f('STORM',           'aarch64', r'\[storm\]'),
    _f('SWEEP',           'aarch64', r'^\s*SWEEP\s+phys='),
    _f('CAPSTONE',        'aarch64', r'^\s*::\s*CAPSTONE\b'),
    _f('EL0',             'aarch64', r'^\s*::\s*EL0\b'),
    # ':: BGRUN-SCAV:' / ':: BGRUN-ST:' AND the plain ':: BGRUN: bg ...' job
    # lines.  The '-\w+' form dropped the colon variant outright.
    _f('BGRUN',           'aarch64', r'^\s*::\s*BGRUN[-:]'),
    _f('M6',              'aarch64', r'^\s*::\s*M6[a-z]\b'),
    _f('SERROR-DRAIN',    'aarch64', r'^\s*::\s*SERROR-DRAIN\b'),
    _f('SMPBAL',          'aarch64', r'\[smpbal\]|^\s*::\s*SMPBAL\b'),
    _f('VFS2',            'aarch64', r'^\s*::\s*VFS2\b'),
    _f('VFS3',            'aarch64', r'^\s*::\s*VFS3\b'),
    _f('NET-GATE',        'aarch64', r'^\s*::\s*NET\d*-GATE:'),
    # Jetson / Tegra track, and the aarch64 platform bring-up chatter.
    _f('TEGRA',           'aarch64', r'^\s*::\s*(?:tegra:|SDMMC:)'),
    _f('PCIE',            'aarch64', r'^\s*::\s*PCIE\d*\b'),
    _f('AARCH64-PLAT',    'aarch64', r'^\s*::\s*(?:GIC\b|DTB\b|A78AE|JB1f'
                                     r'|NS-SPAN)'),
    _f('EXEC',            'aarch64', r'^\s*::\s*(?:ELF\d*|EXEC[\w-]*|BGSPREAD)\s*:'),
    # native-fs / ACL / codec acceptance witnesses
    _f('UNAFS',           'aarch64', r'^\s*::\s*(?:K\d[\w.\-]*|FAT[A-Z]+|CLOCK\d+-\w+'
                                     r'|BANDY-[\w-]+|IMG-SIG|F\d+-witness|ls\d+)\s*:'
                                     r'|^\s*UNAFS:'),
    _f('BANNER',          'aarch64', r'^\s*::\s*UnaOS\b'),

    # ---- families that fire on both rigs; listed last so the platform
    # ---- specific entries above claim their lines first -------------------
    # ':: wc-x86: ...' is the same compositor speaking without the bracket tag.
    _f('WINCOMP',         'any',     r'\[wc-[a-z]+\]|\[wcn\]|^\s*::\s*wc-x86:'),
    _f('CLICK',           'any',     r'\[click\d*\]|\[clickroute\]'),
    _f('CURSOR',          'any',     r'\[cursor\d*\]'),
    _f('SMP',             'any',     r'^\s*SMP:|\[smp\d+\]'),
    # ':: U1a: ...' and the parenthesised form ':: U11m2(1): ...', which the
    # ':'-only terminator dropped.
    _f('U-SUITE',         'any',     r'^\s*::\s*U\d+[\w.\-]*[:(]'),
    # ring-3 acceptance batteries.  Each anchored on its own stable prefix.
    _f('WINX',            'any',     r'^\s*::\s*(?:WINX-\d|winx\d)\b'),
    _f('SOCK',            'any',     r'^\s*::\s*SOCK-\d'),
    _f('PULSE-W',         'any',     r'^\s*::\s*PULSE-W:'),
    _f('ZEOLITE',         'any',     r'^\s*::\s*zeolite:'),
    # the filesystem / capability acceptance ladder: ':: S3:', ':: S4-race(2):',
    # ':: S5 FAIL —', ':: S6-witness:', ':: S7-openany', ':: S8-write',
    # ':: S9-grow', ':: CFU:' / ':: CFU FAIL'.
    _f('FS-ACCEPT',       'any',     r'^\s*::\s*(?:S\d[\w.\-]*|CFU)\b'),
    _f('INSTALL',         'any',     r'^\s*::\s*(?:INSTALL|install|PIINSTALL)\b'),
    _f('NET-X86',         'any',     r'^\s*::\s*(?:SMOLNET|DNS-X86-GATE'
                                     r'|SNTP-X86-GATE)\b'
                                     r'|\[dns-x86\]|\[sntp-x86\]'),
    _f('SHELL',           'any',     r'^\s*::\s*(?:fs\d+:|ui\d+:|vfsw:|JC\d+:)'),
    _f('SELFTEST',        'any',     r'^\s*::\s*TSTE\b'),
    _f('XHCI',            'any',     r'^\s*(?:::\s*)?xHCI\b|\[XHCI\]|\[xhciint\]'),
    # 'Framebuffer' capitalised is the announce line; the geometry lines that
    # follow it are lower-case ':: framebuffer WxH stride=... ::' and
    # ':: fb_addr=... fb_size=... ::', which the case-sensitive alternation
    # dropped.
    _f('FB',              'any',     r'^\s*::\s*(?:FB|[Ff]ramebuffer|fbcon|video'
                                     r'|SPLASH|fb_addr=)\b'),
    # ':: WARNING: No framebuffer detected ::' and friends.  Classified, not
    # alarmed: at least one of them is the normal reading on a headless build,
    # so it belongs in the census rather than in DEFECT_SIGNALS.
    _f('WARN',            'any',     r'^\s*::\s*WARNING:'),
    _f('VIDEO',           'any',     r'^\s*::\s*(?:VIDEO\b|VWIT:|vperf:|RAST\b'
                                     r'|EDID\b)'),
    _f('UI',              'any',     r'^\s*::\s*UI\d\b'),
    _f('STAT',            'any',     r'^\s*::\s*(?:STAT|SVC)\b'),
    _f('SYSCALL',         'any',     r'^\s*::\s*SYSCALL:'),
    _f('KERNEL',          'any',     r'^\s*::\s*KERNEL\b'),
    # listed after SCHED, which claims ':: INPUT on core ...'
    _f('INPUT',           'any',     r'^\s*::\s*INPUT\b'),
    _f('GUI',             'any',     r'\[gui\]'),
    # Last entry deliberately: an acceptance battery announces itself with a
    # witness mask, e.g. ':: NET2-GATE: ... PASS [w=0xff] ::'.  Listed after
    # every named family so those claim their own lines first.
    _f('GATE-BATTERY',    'any',     r'\[w=0x[0-9a-fA-F]+\]'),
]

# A line that no family claimed but which *looks* like an instrument speaking.
# Reported as a coverage gap, never counted as a witness.
WITNESS_SHAPED = re.compile(
    r'==\s*witness\s*::'          # self-declared witness
    r'|verdict='                  # carries a verdict
    r'|^\s*::\s.*\s::\s*$'        # fully '::'-delimited emission
)


# ---------------------------------------------------------------------------
# Defect classifier — alarm signals only.
#
# The classes this replaced were bare substring tests over the whole boot text
# and were false-positive generators, so they are deliberately gone:
#   'PASS'  — 'verdict=PASS' is on every healthy boot; the class was
#             unconditional and therefore carried no information.
#   'panic' — matched '[wc-x] console-window panic-fallback armed', i.e. the
#             ARMING of the panic path on a perfectly healthy boot.
#   'lease' — matched 'released' and 'hold released'.
#   'RAS'   — a three-letter substring with no word boundary; never fired on
#             either reference capture and cannot be trusted if it does.
# ---------------------------------------------------------------------------

#
# Two rules govern what may go in this list:
#
#   1. A signal must match a line the tree can actually emit TODAY.  Every
#      entry below was checked against its emitter at source.
#   2. A signal must not fire on a reading the emitter itself declares
#      unusable.  KBDWIT's 'ok=' flags are exactly that declaration, and the
#      old single KBDWIT-STALL class ignored them — see below.
#
# Deliberately NOT alarmed on:
#   'post_ms=0'          — the HEALTHY reading of KBDWIT line 7 on this rig.
#                          Printing costs tens of microseconds against
#                          FRINDEX's 125 us tick, so post == fire is normal.
#   'place=DECLINED-EMPTY' — a real refusal, but the one every small-`aps`
#                          configuration reaches.  DECLINED-COLLIDE is the
#                          premise guard and IS alarmed.
#   'verdict=PARTIAL'    — coverage is short, the placement rule is intact.
#   'CCSMARGIN' (bare)   — the healthy token; only 'CCSMARGIN-' is a finding.
#
DEFECT_SIGNALS = [
    ('GPACE-OVERLAP',
     re.compile(r'GPACE:\s*OVERLAP'),
     'GPACE tripwire: measured phase spans overlap — that instrument is lying'),
    ('CCSMARGIN-DEFECT',
     re.compile(r'result=CCSMARGIN-'),
     'xHCI ccs-margin returned a qualified result (BLOWN/TIGHT/LATE); '
     'the healthy line ends "result=CCSMARGIN" with nothing after'),
    ('VERDICT-FAIL',
     re.compile(r'verdict=FAIL\b'),
     'an instrument reported verdict=FAIL'),
    # CONCLUSIVE.  Both bits are read out of USBSTS on KBDWIT line 6, and both
    # are latched fault states.  Guarded on 'ok=1': when the MMIO read failed
    # the emitter prints the placeholder zeros hch=0/hse=0, so an unguarded
    # test here would read a failed read as a clean controller.
    ('KBDWIT-HALT',
     re.compile(r'KBDWIT:.*\bok=1\b.*\b(?:hch=1|hse=1)\b'),
     'keyboard witness: controller halted (hch=1) or host system error '
     '(hse=1), read successfully (ok=1). A latched fault state'),
    # NOT CONCLUSIVE ON ITS OWN, and labelled so.  A stopped FRINDEX gives
    # arm == fire and therefore adv=0x0000 unconditionally, so there is no
    # false-clean mode — but a HEALTHY counter whose advance across the window
    # lands on an exact multiple of 16384 microframes reads adv=0x0000 too.
    # That is a 1-in-16384 FALSE ALARM.  Worth a second boot before it is
    # worth a conviction; adv!=0 needs no corroboration.
    # Guarded on 'ok=1' for a stronger reason than the halt class above: when
    # the FRINDEX read fails the emitter's own match arm falls through to
    # adv = 0, so ok=0 GUARANTEES adv=0x0000.  Ungated, this class would fire
    # on every failed read.  '\bok=' does not match 'post_ok=' — '_' is a word
    # character, so there is no boundary before that 'ok'.
    ('KBDWIT-FRINDEX-FROZEN',
     re.compile(r'KBDWIT:.*\bfrindex\b.*\bok=1\b.*\badv=0x0000\b'),
     'keyboard witness: FRINDEX did not advance across a >=4 s silence window '
     '(adv=0x0000, read valid). SUSPECT, NOT PROOF — a healthy counter landing '
     'on an exact 16384-microframe multiple reads the same. Corroborate with a '
     'second boot, and with hch=/hse=/pss= on line 6'),
    # The instrument declaring itself unusable.  On line 6 ok=0 means hch=/hse=
    # are placeholder zeros (a false CLEAN); on line 7 it means adv= is a
    # placeholder 0x0000 (a false ALARM).  Either way the dump cannot answer
    # the question it was emitted to answer, and silence from a broken
    # instrument is not evidence of health.
    ('KBDWIT-UNREAD',
     re.compile(r'KBDWIT:.*\bok=0\b'),
     'keyboard witness: an MMIO read FAILED (ok=0), so the fields on that line '
     'are placeholders, not measurements. Neither a clean nor a stall reading '
     'may be taken from it'),
    ('KEPLER-RESTORE-CORRUPT',
     re.compile(r'::\s*kepler:.*\brestored=CORRUPT\b'),
     'kepler beacon-restore / runlist-rebuild read back words that are not the '
     'words written, or found a surviving 0xBEAC000n beacon sentinel where real '
     'data should be (beacon_resid/mismatch nonzero)'),
    ('KEPLER-BIND-DEADLINE',
     re.compile(r'::\s*kepler:\s*post-bind\b.*\bexit=deadline\b'),
     'kepler post-bind: PLAYLIST_RD/_RD_LEN never echoed the runlist we '
     'submitted within budget — the submit was not accepted'),
    ('BGPLACE-COLLIDE',
     re.compile(r'SCHED-X86 BG-PLACE:.*\bplace=DECLINED-COLLIDE\b'),
     'bg placement handed back the core it was asked from. Not reachable '
     'through today\'s worker_pool, so this firing means a premise changed '
     '(a second caller, a re-pinned shell, or worker_pool stopped filtering)'),
    ('PANIC',
     re.compile(r'\bPANIC\b|\bpanic:'),
     'kernel panic'),
]


# ---------------------------------------------------------------------------
# Parsing
# ---------------------------------------------------------------------------

def classify_line(line):
    """Return the witness family name for a line, or None."""
    for name, _platform, pattern in WITNESS_FAMILIES:
        if pattern.search(line):
            return name
    return None


def _find_markers(lines):
    """Return [(index, spec, boot_number_or_None)] for every boot marker hit.

    `lines` here is the PREFIX-STRIPPED probe copy — see strip_logts()."""
    hits = []
    for i, line in enumerate(lines):
        for spec in BOOT_MARKERS:
            if spec['marker'].search(line):
                num = None
                if spec['number_re']:
                    m = spec['number_re'].search(line)
                    if not m:
                        continue          # ' MARK ' without a boot number
                    num = m.group(1)
                hits.append((i, spec, num))
                break
    return hits


def _valid_anchors(lines, anchors, n_boots):
    """Keep only epoch anchors that fire exactly once per boot in THIS file.

    An anchor that fires more than once per boot silently corrupts the split:
    'APIC: x2APIC software-enabled' looks like a boot epoch but every AP emits
    it, so backtracking would drag a boot's start into the middle of the
    previous one.  Counting first is cheap and makes the anchor list safe to
    extend."""
    keep = []
    for anchor in anchors:
        if sum(1 for line in lines if anchor.search(line)) == n_boots:
            keep.append(anchor)
    return keep


def _boot_start_index(lines, marker_idx, lower_bound, anchors):
    """Walk back from a boot marker to the earliest epoch anchor lying in
    (lower_bound, marker_idx], so the boot's pre-marker head is attributed to
    the boot it belongs to.  Falls back to the marker line itself."""
    if not anchors:
        return marker_idx
    for i in range(lower_bound + 1, marker_idx + 1):
        for anchor in anchors:
            if anchor.search(lines[i]):
                return i
    return marker_idx


def parse_log(filepath):
    """Parse a capture file into a result dict.  Read-only: this tool never
    writes to a capture."""
    with open(filepath, 'r', errors='replace') as f:
        content = f.read()
    return parse_content(filepath, content)


def parse_content(label, content):
    """Parse capture TEXT.  Split out of parse_log so a self-test fixture goes
    through the identical path a real capture does — a fixture that exercises a
    private shortcut certifies the shortcut, not the tool."""
    lines = strip_control_bytes(content).split('\n')
    # Every pattern in this file is head-anchored, and 87% of the lines in a
    # current capture carry a logts stamp in front of that head.  `probe` is
    # what patterns are matched against; `lines` (stamp intact) is what is
    # stored, printed and diffed.  Index-for-index, so a hit in one names the
    # same line in the other.
    probe = [strip_logts(line) for line in lines]
    hits = _find_markers(probe)

    # Validate each format's epoch anchors against this file's boot count
    # before using them to backtrack.
    anchor_cache = {}
    for spec in BOOT_MARKERS:
        n = sum(1 for _i, s, _n in hits if s['platform'] == spec['platform'])
        anchor_cache[spec['platform']] = (
            _valid_anchors(probe, spec['epoch_anchors'], n) if n else [])

    # Resolve each marker to a boot span.
    spans = []
    prev_marker = -1
    for idx, (marker_idx, spec, num) in enumerate(hits):
        start = _boot_start_index(probe, marker_idx, prev_marker,
                                  anchor_cache[spec['platform']])
        spans.append([start, marker_idx, spec, num])
        prev_marker = marker_idx
    for i, span in enumerate(spans):
        span.append(spans[i + 1][0] if i + 1 < len(spans) else len(lines))

    boots = []
    for seq, (start, marker_idx, spec, num, end) in enumerate(spans, start=1):
        body = lines[start:end]
        boot = {
            'number': num if num is not None else str(seq),
            'platform': spec['platform'],
            'marker_label': spec['label'],
            'start_index': start,
            'end_index': end,
            'marker_index': marker_idx,
            'start_line': lines[start],
            'marker_line': lines[marker_idx],
            'lines': body,
            # prefix-stripped, index-for-index with 'lines'; what the GR18
            # sections match against.
            'probe': probe[start:end],
            'witnesses': [],          # [{'family': str, 'line': str}]
            'families': Counter(),
            'unclassified': [],
            'dropped_witness_shaped': [],
            'defects': [],            # [{'class','line'}]
        }
        for line, probe_line in zip(body, probe[start:end]):
            if not line.strip():
                continue
            family = classify_line(probe_line)
            if family:
                boot['witnesses'].append({'family': family, 'line': line})
                boot['families'][family] += 1
            else:
                boot['unclassified'].append(line)
                if WITNESS_SHAPED.search(probe_line):
                    boot['dropped_witness_shaped'].append(line)
            for cls, pattern, _desc in DEFECT_SIGNALS:
                if pattern.search(probe_line):
                    boot['defects'].append({'class': cls, 'line': line})

        boot['classes'] = (sorted({d['class'] for d in boot['defects']})
                           or ['CLEAN'])
        boots.append(boot)

    return {
        'path': label,
        'total_lines': len(lines),
        'boots': boots,
        'preamble_lines': spans[0][0] if spans else len(lines),
        'markers_tried': [s['label'] for s in BOOT_MARKERS],
    }


# ---------------------------------------------------------------------------
# Reporting
# ---------------------------------------------------------------------------

def _shape(line, width=100):
    """Collapse digits so near-identical lines group together in a census."""
    return re.sub(r'\d+', 'N', line.strip())[:width]


def print_boot_summary(boot, samples=2, full=False, family_filter=None):
    verdict = ', '.join(boot['classes'])
    print(f"Boot {boot['number']}  [{boot['platform']}]  "
          f"lines {boot['start_index'] + 1}..{boot['end_index']}  "
          f"verdict: {verdict}")
    print(f"  start:  {boot['start_line'].strip()}")
    print(f"  marker: {boot['marker_line'].strip()}")

    fams = boot['families']
    if family_filter:
        fams = Counter({k: v for k, v in fams.items()
                        if family_filter.search(k)})
    total = sum(fams.values())
    print(f"  witnesses: {total} line(s) across {len(fams)} family/families")

    if total == 0:
        print("  !! NO WITNESSES IN THIS BOOT — the family table did not "
              "match anything here")

    for name in sorted(fams):
        count = fams[name]
        print(f"    {name:<18} {count:>5}")
        picks = [w['line'] for w in boot['witnesses'] if w['family'] == name]
        if not family_filter and not full:
            picks = picks[:samples]
        for line in picks:
            print(f"        | {line.strip()}")

    if boot['defects']:
        print(f"  defects: {len(boot['defects'])}")
        seen = OrderedDict()
        for d in boot['defects']:
            seen.setdefault(d['class'], []).append(d['line'])
        for cls, hits in seen.items():
            desc = next(x[2] for x in DEFECT_SIGNALS if x[0] == cls)
            print(f"    !! {cls} x{len(hits)} — {desc}")
            for line in (hits if full else hits[:samples]):
                print(f"        | {line.strip()}")
    else:
        print("  defects: none")

    dropped = boot['dropped_witness_shaped']
    print(f"  unclassified: {len(boot['unclassified'])} line(s), "
          f"of which {len(dropped)} witness-shaped")
    if dropped:
        print("    (coverage gap — these look like instruments speaking but "
              "match no family in WITNESS_FAMILIES)")
        for shape, count in Counter(_shape(x) for x in dropped).most_common(
                len(dropped) if full else 5):
            print(f"      {count:>5}  {shape}")
    print("")


def print_file_report(result, samples=2, full=False, family_filter=None):
    boots = result['boots']
    platforms = sorted({b['platform'] for b in boots})
    print(f"=== {result['path']} ===")
    print(f"  lines: {result['total_lines']}   boots: {len(boots)}   "
          f"platform(s): {', '.join(platforms) if platforms else 'none'}   "
          f"pre-boot preamble: {result['preamble_lines']} line(s)")
    print("")

    for boot in boots:
        print_boot_summary(boot, samples=samples, full=full,
                           family_filter=family_filter)

    # Whole-file family census, so a family that vanished between runs is
    # visible without diffing every line.
    census = Counter()
    for boot in boots:
        census.update(boot['families'])
    print(f"--- witness families in {result['path']} ---")
    for name in sorted(census):
        platform = next(p for n, p, _ in WITNESS_FAMILIES if n == name)
        print(f"  {name:<18} {census[name]:>6}   [{platform}]")
    print("")

    # Families the table knows about that this capture did not exercise.
    absent = [(n, p) for n, p, _ in WITNESS_FAMILIES if n not in census]
    if absent:
        print(f"--- families in the table but absent from this capture "
              f"({len(absent)}) ---")
        print("  " + ", ".join(f"{n}[{p}]" for n, p in absent))
        print("")


def diff_boots(boots1, boots2):
    """Compare two parses boot-by-boot.  Unchanged in contract: it consumes
    the per-boot class list and witness set."""
    print("=== DIFF BETWEEN TWO LOG FILES ===")

    b1_dict = {b['number']: b for b in boots1}
    b2_dict = {b['number']: b for b in boots2}

    def _key(n):
        try:
            return (0, int(n), '')
        except ValueError:
            return (1, 0, n)

    all_nums = sorted(set(b1_dict) | set(b2_dict), key=_key)
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
            w1 = {w['line'].strip() for w in b1['witnesses']}
            w2 = {w['line'].strip() for w in b2['witnesses']}
            f1 = set(b1['families'])
            f2 = set(b2['families'])

            if classes1 != classes2 or w1 != w2:
                print(f"Boot {num} differs:")
                if classes1 != classes2:
                    print(f"  Classes: {sorted(classes1)} vs {sorted(classes2)}")
                if f1 != f2:
                    if f2 - f1:
                        print(f"  Families only in file 2: {sorted(f2 - f1)}")
                    if f1 - f2:
                        print(f"  Families only in file 1: {sorted(f1 - f2)}")
                added = w2 - w1
                removed = w1 - w2
                print(f"  Witness lines: +{len(added)} / -{len(removed)}")
                for line in sorted(added)[:5]:
                    print(f"    + {line}")
                for line in sorted(removed)[:5]:
                    print(f"    - {line}")
            else:
                print(f"Boot {num} matches exactly.")


# ---------------------------------------------------------------------------
# GR18 sections — the three wires Boot V added
#
# One reader each, and each one reports its own ABSENCE rather than printing an
# empty table: a section that answers "nothing to see" identically whether the
# wire was silent or the parser stopped matching is a section that cannot
# falsify anything.
# ---------------------------------------------------------------------------

def _kv(body):
    """Scan 'key=value' tokens out of a witness body into an OrderedDict.

    Read by NAME, never by column.  `late=` was APPENDED to SMC-BATT (GR18)
    exactly so that pre-existing fields keep their positions for a positional
    awk — but a reader in this file has no such constraint and should not
    acquire one, because the next append would break it."""
    return OrderedDict(re.findall(r'([A-Za-z_][\w.]*)=(\S+)', body))


def _num(text):
    """Parse a counter that may carry a unit or a sign ('94%', '50ms', '-2286mA',
    '1447').  Returns None when there is no number to read — never 0, which is a
    value every one of these counters can legitimately hold."""
    if text is None:
        return None
    m = re.match(r'^([+-]?\d+)', text)
    return int(m.group(1)) if m else None


# --- WXPROBE (--wxprobe) --------------------------------------------------

WXPROBE_RE = re.compile(r'^\s*::\s*WXPROBE\s+(cpu|map|elf):\s*(.*?)\s*::\s*$')

# The framebuffer leaf's expected typing.  Under the PAT MSR's default layout
# the PAT/PCD/PWT triple selects the entry: 1/0/0 is PA4 = write-combining,
# which is what ':: x86 fb-wc: retyped N leaf(s) WC (PAT PA4) ...' claims to
# have installed.  0/0/0 is PA0 = write-back and 0/1/0 is PA2 = uncacheable —
# and UC is not a hypothetical: map_mmio_window silently un-typed this exact
# leaf from WC to UC for two weeks (GR15), costing 8.7-9.1x on every blit, and
# nothing in the capture said so.  That is what this cross-check exists for.
WC_EXPECT = OrderedDict([('pat', '1'), ('pcd', '0'), ('pwt', '0')])
WC_PAT_ENTRY = {
    ('0', '0', '0'): 'PA0 (write-back)',
    ('0', '0', '1'): 'PA1 (write-through)',
    ('0', '1', '0'): 'PA2 (UC-)',
    ('0', '1', '1'): 'PA3 (uncacheable)',
    ('1', '0', '0'): 'PA4 (write-combining)',
}
# The leaf the WC cross-check applies to.  Named, not guessed from the address:
# 'fb' is the stable name the emitter promises a capture is grepped by.
WC_LEAF = 'fb'


def _wc_typing(fields):
    """Decode a leaf's PAT/PCD/PWT triple against the WC expectation.

    Shared by --wxprobe's 'at=fb' row and --wxn's WXN-FBWC line: the two wires
    read the same three bits off the same leaf at two different moments of the
    boot, and a disagreement between them is itself a finding — which cannot be
    seen at all if each section carries its own copy of this table.
    Returns (ok, got_triple, pat_entry_name)."""
    got = tuple(fields.get(k, '-') for k in WC_EXPECT)
    return (got == tuple(WC_EXPECT.values()), got,
            WC_PAT_ENTRY.get(got, 'not a PAT combination this reader knows'))


def _leaf_entry_change(section, at, prev_e, cur_e, prev_n, cur_n):
    """The consecutive-boot compare on a RAW leaf entry, shared by --wxprobe
    and --wxn.

    The raw 'e=' word is the subject, not the decoded flags: every flag either
    section prints is derived from it, so an entry that changed while the
    decoded columns did not is still a changed mapping.  A changed fb entry is
    the GR15 regression signature and gets its own loud class in both sections;
    `section` only names which wire saw it.
    Returns a finding string, or None when the entry did not move."""
    if prev_e == cur_e:
        return None
    is_fb = (at == WC_LEAF)
    cls = (f"WARN {section}-FB-ENTRY-CHANGED" if is_fb
           else f"WARN {section}-ENTRY-CHANGED")
    tail = (" — this is the GR15 regression signature: the framebuffer "
            "leaf was retyped between boots" if is_fb else "")
    return (f"{cls}: leaf '{at}' entry {prev_e} -> {cur_e} across "
            f"boots {prev_n} -> {cur_n}{tail}")


def wxprobe_boot(boot):
    """Pull one boot's WXPROBE block apart, or None if the boot carries none."""
    cpu, elf, maps, n = None, None, OrderedDict(), 0
    for line, probe_line in zip(boot['lines'], boot['probe']):
        m = WXPROBE_RE.match(probe_line)
        if not m:
            continue
        kind, body = m.group(1), m.group(2)
        n += 1
        fields = _kv(body)
        if kind == 'cpu':
            cpu = {'fields': fields, 'line': line}
        elif kind == 'elf':
            elf = {'fields': fields, 'line': line}
        else:
            at = fields.get('at')
            if at is None:
                continue
            # Last one wins, and duplicates are reported: the emitter probes
            # each name once per boot, so a second line for the same 'at' means
            # the block ran twice and the two readings are not interchangeable.
            maps.setdefault(at, []).append({'fields': fields, 'line': line})
    if n == 0:
        return None
    return {'cpu': cpu, 'elf': elf, 'maps': maps, 'lines': n,
            'number': boot['number']}


def print_wxprobe_boot(wx):
    print(f"Boot {wx['number']}  WXPROBE  ({wx['lines']} line(s), "
          f"{len(wx['maps'])} leaf/leaves)")
    if wx['cpu']:
        f = wx['cpu']['fields']
        print("  cpu:  " + "  ".join(
            f"{k}={f.get(k, '-')}" for k in
            ('cr0', 'wp', 'cr4', 'pge', 'smep', 'smap', 'la57', 'efer',
             'nxe', 'lme')))
    else:
        print("  cpu:  !! MISSING — the block printed map/elf lines but no "
              "cpu line")

    print(f"  {'at':<8} {'lvl':<5} {'va':<12} {'entry':<20} "
          f"{'p w u nx g':<12} {'pat pcd pwt':<12} {'fw fx fu'}")
    for at, entries in wx['maps'].items():
        if len(entries) > 1:
            print(f"    !! {at}: {len(entries)} lines for one leaf in one boot "
                  f"— the probe ran more than once; readings are not "
                  f"interchangeable")
        for e in entries:
            f = e['fields']
            print(f"  {at:<8} {f.get('lvl', '-'):<5} {f.get('va', '-'):<12} "
                  f"{f.get('e', '-'):<20} "
                  f"{f.get('p', '-')} {f.get('w', '-')} {f.get('u', '-')} "
                  f"{f.get('nx', '-'):<2} {f.get('g', '-'):<4} "
                  f"{f.get('pat', '-'):<3} {f.get('pcd', '-'):<3} "
                  f"{f.get('pwt', '-'):<5} "
                  f"{f.get('fw', '-')}  {f.get('fx', '-')}  {f.get('fu', '-')}")

    if wx['elf']:
        f = wx['elf']['fields']
        segs = " ".join(f"{k}={f[k]}" for k in ('s0', 's1', 's2', 's3')
                        if k in f)
        print(f"  elf:  ehdr={f.get('ehdr', '-')} ok={f.get('ok', '-')} "
              f"phnum={f.get('phnum', '-')} load={f.get('load', '-')}  {segs}")
        if f.get('ok') == '0':
            print("    !! WARN WXPROBE-ELF-UNREAD: ok=0 — the ELF header did "
                  "not validate, so phnum/load/s* are placeholders, not "
                  "measurements")
    else:
        print("  elf:  !! MISSING — the block printed no elf line")
    print("")


def wxprobe_fb_typing(wx):
    """Cross-check the fb leaf against the WC expectation.

    Returns (ok, [message]).  A MISSING fb leaf is not ok: the check exists to
    catch a silent retyping, and a reader that treats an absent leaf as a pass
    is the same failure one level up."""
    entries = wx['maps'].get(WC_LEAF)
    if not entries:
        return False, [f"WARN FB-TYPING: boot {wx['number']} has no "
                       f"'at={WC_LEAF}' leaf — the WC cross-check could not "
                       f"run, which is not the same as passing"]
    msgs, ok = [], True
    for e in entries:
        f = e['fields']
        good, got, entry = _wc_typing(f)
        want = tuple(WC_EXPECT.values())
        if good:
            msgs.append(f"ok   boot {wx['number']} fb: pat={got[0]} "
                        f"pcd={got[1]} pwt={got[2]} -> {entry}")
        else:
            ok = False
            msgs.append(
                f"WARN FB-TYPING: boot {wx['number']} fb is pat={got[0]} "
                f"pcd={got[1]} pwt={got[2]} -> {entry}; WC needs "
                f"pat={want[0]} pcd={want[1]} pwt={want[2]} (PA4). The panel "
                f"is not write-combining on this boot")
    return ok, msgs


def wxprobe_diff(prev, cur):
    """DIFF two consecutive WXPROBE blocks on the RAW leaf entry.

    The raw 'e=' word is the subject, not the decoded flags: every flag this
    tool prints is derived from it, so an entry that changed while the decoded
    columns did not is still a changed mapping.  A changed fb entry is the GR15
    regression signature and is reported as its own loud class."""
    findings, notes = [], []
    for at in list(prev['maps']) + [a for a in cur['maps']
                                    if a not in prev['maps']]:
        p = prev['maps'].get(at)
        c = cur['maps'].get(at)
        if not p or not c:
            notes.append(f"leaf '{at}' present in boot "
                         f"{prev['number'] if p else cur['number']} only")
            continue
        pe, ce = p[-1]['fields'].get('e'), c[-1]['fields'].get('e')
        finding = _leaf_entry_change('WXPROBE', at, pe, ce,
                                     prev['number'], cur['number'])
        if finding:
            findings.append(finding)
    # The cpu line is per-boot state the split is designed from; a change there
    # is reported, never alarmed — a different EFER/CR4 is a different build or
    # a different firmware handoff, both legitimate, both worth seeing.
    if prev['cpu'] and cur['cpu']:
        for k in ('cr0', 'wp', 'cr4', 'pge', 'smep', 'smap', 'la57', 'efer',
                  'nxe', 'lme'):
            pv, cv = prev['cpu']['fields'].get(k), cur['cpu']['fields'].get(k)
            if pv != cv:
                notes.append(f"cpu {k}: {pv} -> {cv} across boots "
                             f"{prev['number']} -> {cur['number']}")
    return findings, notes


def wxprobe_report(result):
    """Return (parsed_anything, clean)."""
    blocks = [wx for wx in (wxprobe_boot(b) for b in result['boots']) if wx]
    print(f"=== WXPROBE — {result['path']} ===")
    if not blocks:
        print(f"  no WXPROBE lines in {len(result['boots'])} boot(s)")
        return False, True
    print(f"  {len(blocks)} of {len(result['boots'])} boot(s) carry a WXPROBE "
          f"block\n")
    for wx in blocks:
        print_wxprobe_boot(wx)
        if wx['lines'] != 8:
            print(f"  !! WARN WXPROBE-SHORT: boot {wx['number']} printed "
                  f"{wx['lines']} of the 8 lines the block emits\n")

    clean = True
    print("--- fb WC typing cross-check (pat=1 pcd=0 pwt=0 => PA4) ---")
    for wx in blocks:
        ok, msgs = wxprobe_fb_typing(wx)
        if not ok:
            clean = False
        for msg in msgs:
            print(f"  {msg}")
    print("")

    print("--- consecutive-boot DIFF (raw leaf entry) ---")
    if len(blocks) < 2:
        # NOT a pass.  One block is one sample, and a regression signature that
        # needs two boots to appear cannot appear in one.
        print(f"  only {len(blocks)} boot carries a WXPROBE block in this "
              f"capture — NO DIFF POSSIBLE. The fb-entry regression signature "
              f"needs two boots to be visible; this section has not cleared "
              f"it, it has not tested it.")
    else:
        any_finding = False
        for prev, cur in zip(blocks, blocks[1:]):
            findings, notes = wxprobe_diff(prev, cur)
            for note in notes:
                print(f"  note {note}")
            for finding in findings:
                any_finding = True
                clean = False
                print(f"  !! {finding}")
        if not any_finding:
            print(f"  {len(blocks) - 1} consecutive pair(s) compared: every "
                  f"leaf entry identical across boots")
    print("")
    return True, clean


def wxprobe_mode(result):
    parsed, clean = wxprobe_report(result)
    if not parsed:
        print(f"ERROR: {result['path']}: --wxprobe parsed 0 WXPROBE lines. "
              f"Either the capture predates the block or the wire moved.",
              file=sys.stderr)
        return EXIT_NO_DATA
    if not clean:
        print(f"WARNING: {result['path']}: --wxprobe reported at least one "
              f"finding (see WARN lines above).", file=sys.stderr)
        return EXIT_FINDING
    return EXIT_OK


# --- EHCI EPACE-TRIM M8 SLOW-XFER (--slowxfer) ----------------------------

SLOWXFER_RE = re.compile(
    r'^\s*::\s*EHCI-HID:\s*\[(\d+)\]\s*EPACE-TRIM\s+M8\s+SLOW-XFER\s+'
    r'(?!cap\s+reached)(.*?)\s*==\s*witness\s*::\s*$')
# The overflow line's payload is prose, not key=value, so it gets its own
# pattern.  Anchored on 'cap reached' and on the numbers, NOT on the em dash
# that separates them — a dash is a rendering detail and this capture carries
# it as raw UTF-8.
SLOWCAP_RE = re.compile(
    r'^\s*::\s*EHCI-HID:\s*\[(\d+)\]\s*EPACE-TRIM\s+M8\s+SLOW-XFER\s+'
    r'cap\s+reached\b.*?(\d+)\s+transfers\s+crossed\s+the\s+(\d+)\s*ms\s+'
    r'threshold,\s*(\d+)\s+printed,\s*(\d+)\s+suppressed')

USB_STD_REQ = {
    0x00: 'GET_STATUS', 0x01: 'CLEAR_FEATURE', 0x03: 'SET_FEATURE',
    0x05: 'SET_ADDRESS', 0x06: 'GET_DESCRIPTOR', 0x07: 'SET_DESCRIPTOR',
    0x08: 'GET_CONFIGURATION', 0x09: 'SET_CONFIGURATION',
    0x0A: 'GET_INTERFACE', 0x0B: 'SET_INTERFACE', 0x0C: 'SYNCH_FRAME',
}
USB_DESC_TYPE = {
    1: 'DEVICE', 2: 'CONFIG', 3: 'STRING', 4: 'INTERFACE', 5: 'ENDPOINT',
    6: 'DEVICE_QUALIFIER', 7: 'OTHER_SPEED_CONFIG', 0x0F: 'BOS',
    0x21: 'HID', 0x22: 'HID_REPORT', 0x29: 'HUB',
}
# The controller a SLOW-XFER on which refutes the device-floor verdict.  The
# enum46 argument is that the cost is the DEVICE's, and [1] has no device on
# it — so a slow transfer there is the controller's own, and the verdict does
# not survive it.
ENUM46_FALSIFIER_IDX = 1


def _hexnum(text):
    """Parse '0x80' / '0x0100'.  None when it is not a hex literal — a request
    decoded from a field that was not read is a fabrication."""
    if text is None:
        return None
    try:
        return int(text, 16)
    except ValueError:
        return None


def decode_request(fields):
    """Name the control request a SLOW-XFER line describes.

    Decoded from bmreq/breq/wval/wlen as the USB 2.0 spec defines them; any
    field that will not parse yields '?' rather than a guess."""
    bm, br = _hexnum(fields.get('bmreq')), _hexnum(fields.get('breq'))
    wval, wlen = _hexnum(fields.get('wval')), _num(fields.get('wlen'))
    if bm is None or br is None:
        return '?', ''
    rtype = (bm >> 5) & 0x3
    if rtype == 1:
        return f'CLASS(breq=0x{br:02x})', ''
    if rtype == 2:
        return f'VENDOR(breq=0x{br:02x})', ''
    name = USB_STD_REQ.get(br, f'STD(breq=0x{br:02x})')
    if br == 0x06:
        # wValue high byte is the descriptor type, low byte the index; wLength
        # is what makes the two GET_DESCRIPTOR(DEVICE) calls of an enumeration
        # tell themselves apart (8 = the first, address-0 probe; 18 = the full
        # device descriptor).
        detail = ''
        if wval is not None:
            dtype, didx = (wval >> 8) & 0xFF, wval & 0xFF
            detail = f"{USB_DESC_TYPE.get(dtype, f'type=0x{dtype:02x}')}"
            if didx:
                detail += f" idx={didx}"
        return (f'GET_DESCRIPTOR({wlen})' if wlen is not None
                else 'GET_DESCRIPTOR(?)'), detail
    if br == 0x05 and wval is not None:
        return name, f'addr={wval}'
    if br == 0x09 and wval is not None:
        return name, f'cfg={wval}'
    return name, ''


def slowxfer_boot(boot):
    """Pull one boot's SLOW-XFER lines and cap line, or None if it has none."""
    xfers, caps = [], []
    for line, probe_line in zip(boot['lines'], boot['probe']):
        m = SLOWCAP_RE.match(probe_line)
        if m:
            caps.append({'idx': int(m.group(1)), 'crossed': int(m.group(2)),
                         'threshold_ms': int(m.group(3)),
                         'printed': int(m.group(4)),
                         'suppressed': int(m.group(5)), 'line': line})
            continue
        m = SLOWXFER_RE.match(probe_line)
        if m:
            fields = _kv(m.group(2))
            name, detail = decode_request(fields)
            xfers.append({'idx': int(m.group(1)), 'fields': fields,
                          'request': name, 'detail': detail, 'line': line})
    if not xfers and not caps:
        return None
    return {'number': boot['number'], 'xfers': xfers, 'caps': caps}


def print_slowxfer_boot(sx):
    print(f"Boot {sx['number']}  EPACE-TRIM M8 SLOW-XFER  "
          f"({len(sx['xfers'])} transfer(s), {len(sx['caps'])} cap line(s))")
    if sx['xfers']:
        print(f"    {'ctl':<4} {'seq':<6} {'request':<22} {'detail':<20} "
              f"{'addr':<5} {'spd':<4} {'stg':<4} {'xfer':<8} {'act':<8} "
              f"{'ass'}")
    for x in sx['xfers']:
        f = x['fields']
        print(f"    [{x['idx']}]  {f.get('seq', '-'):<6} "
              f"{x['request']:<22} {x['detail']:<20} "
              f"{f.get('addr', '-'):<5} {f.get('spd', '-'):<4} "
              f"{f.get('stg', '-'):<4} {f.get('xfer', '-'):<8} "
              f"{f.get('act', '-'):<8} {f.get('ass', '-')}")
    print("")


def slowxfer_report(result):
    """Return (parsed_anything, clean)."""
    blocks = [sx for sx in (slowxfer_boot(b) for b in result['boots']) if sx]
    print(f"=== EPACE-TRIM M8 SLOW-XFER — {result['path']} ===")
    if not blocks:
        # The EXPECTED baseline is one line on [0] and none on [1], so a
        # capture with none at all is worth naming: either every transfer was
        # under the 8 ms threshold, or the instrument did not run.
        print(f"  no SLOW-XFER lines in {len(result['boots'])} boot(s) — "
              f"either no control transfer crossed the M8 threshold, or the "
              f"instrument did not execute. These are not the same reading "
              f"and this wire alone cannot tell them apart.")
        return False, True
    print(f"  {len(blocks)} of {len(result['boots'])} boot(s) carry "
          f"SLOW-XFER lines\n")
    for sx in blocks:
        print_slowxfer_boot(sx)

    clean = True
    print("--- controller [1] check (the enum46 falsifier) ---")
    on_one = [(sx['number'], x) for sx in blocks for x in sx['xfers']
              if x['idx'] == ENUM46_FALSIFIER_IDX]
    if on_one:
        clean = False
        print(f"  !! WARN SLOW-XFER-ON-[1]: {len(on_one)} slow transfer(s) on "
              f"controller [1]. This REFUTES the device-floor verdict: the "
              f"enum46 argument holds only while the cost is the device's, "
              f"and [1] carries no device.")
        for num, x in on_one:
            print(f"      boot {num}: {x['line'].strip()}")
    else:
        print(f"  no slow transfer on controller [{ENUM46_FALSIFIER_IDX}] in "
              f"any boot — the device-floor verdict is not refuted by this "
              f"capture")
    print("")

    print("--- print-cap overflow ---")
    caps = [(sx['number'], c) for sx in blocks for c in sx['caps']]
    if caps:
        clean = False
        for num, c in caps:
            print(f"  !! WARN SLOW-XFER-CAP: boot {num} controller [{c['idx']}]"
                  f" — {c['crossed']} transfer(s) crossed the "
                  f"{c['threshold_ms']} ms threshold, {c['printed']} printed, "
                  f"{c['suppressed']} suppressed. The per-boot table above is "
                  f"TRUNCATED for that controller.")
    else:
        print("  no cap-reached line — every crossing that occurred was "
              "printed")
    print("")

    print("--- request census ---")
    census = Counter(f"[{x['idx']}] {x['request']}"
                     for sx in blocks for x in sx['xfers'])
    for key, count in census.most_common():
        print(f"  {count:>4}  {key}")
    print("")
    return True, clean


def slowxfer_mode(result):
    parsed, clean = slowxfer_report(result)
    if not parsed:
        print(f"ERROR: {result['path']}: --slowxfer parsed 0 SLOW-XFER lines.",
              file=sys.stderr)
        return EXIT_NO_DATA
    if not clean:
        print(f"WARNING: {result['path']}: --slowxfer reported at least one "
              f"finding (see WARN lines above).", file=sys.stderr)
        return EXIT_FINDING
    return EXIT_OK


# --- SMC-BATT / SMC-SCOUT (--smc) -----------------------------------------

# Anchored on 'present=' rather than on the SMC-BATT prefix alone: the same
# prefix also carries prose lines (the AC-W absence note), which have no
# counters on them and must not enter the census as a sample.
SMCBATT_RE = re.compile(
    r'^\s*::\s*SMC-BATT:\s*(present=.*?)\s*==\s*witness\s*::\s*$')
KEYCOUNT_RE = re.compile(r'^\s*::\s*SMC-SCOUT:\s*#KEY\s+count=(\d+)\b')
KEYWALK_RE = re.compile(
    r'^\s*::\s*SMC-SCOUT:\s*index\s+walk\s+done\s*\((\d+)\s+of\s+(\d+)\s+'
    r'names\)')

# Cumulative-since-boot counters.  GREATEST WINS, NEVER SUM: each line reprints
# the running total, so adding two lines together counts the same events twice.
#
# This capture alone carries FOUR generations of the line — no counters at all
# (406 lines), 'stall0='/'resid=' (46), '...gap= busy=' (3), and
# '...gap= busy= late=' (7).  'stall0' is listed alongside 'st0' because they
# are the same quantity under its former name: reading only the new name would
# report the old wire's counter as absent, which is a false silence rather than
# a measurement.
SMC_COUNTERS = ('gap', 'busy', 'late', 'rfail', 'short', 'unc', 'st0',
                'stall0', 'resid')


def smc_boot(boot):
    """Pull one boot's SMC-BATT counter samples and #KEY walk, or None."""
    samples, keycount, keywalk = [], None, None
    for line, probe_line in zip(boot['lines'], boot['probe']):
        m = SMCBATT_RE.match(probe_line)
        if m:
            samples.append({'fields': _kv(m.group(1)), 'line': line})
            continue
        m = KEYCOUNT_RE.match(probe_line)
        if m:
            keycount = {'count': int(m.group(1)), 'line': line}
            continue
        m = KEYWALK_RE.match(probe_line)
        if m:
            keywalk = {'named': int(m.group(1)), 'total': int(m.group(2)),
                       'line': line}
    if not samples and keywalk is None and keycount is None:
        return None

    stats = OrderedDict()
    for key in SMC_COUNTERS:
        vals = [_num(s['fields'].get(key)) for s in samples]
        vals = [v for v in vals if v is not None]
        stats[key] = {
            # 'present' distinguishes a counter this build does not emit from
            # one that is emitted and reads zero.  'late=' is absent on every
            # pre-GR18 boot and that absence must not read as late=0.
            'present': bool(vals),
            'n': len(vals),
            'max': max(vals) if vals else None,
            'first': vals[0] if vals else None,
            'delta': (max(vals) - vals[0]) if vals else None,
        }
    return {'number': boot['number'], 'samples': samples, 'stats': stats,
            'keycount': keycount, 'keywalk': keywalk}


def print_smc_boot(sm):
    print(f"Boot {sm['number']}  SMC  ({len(sm['samples'])} SMC-BATT "
          f"sample(s))")
    if sm['samples']:
        last = sm['samples'][-1]['fields']
        print("  last reading: " + "  ".join(
            f"{k}={last.get(k, '-')}" for k in
            ('present', 'soc', 'volt', 'amp', 'rem', 'full', 'ac',
             'retries')))
        print(f"    {'counter':<8} {'max':>10} {'first':>10} {'delta':>10}   "
              f"note")
        for key, st in sm['stats'].items():
            if not st['present']:
                print(f"    {key:<8} {'absent':>10} {'-':>10} {'-':>10}   "
                      f"not emitted on this boot"
                      + ("  (pre-GR18 SMC-BATT wire)" if key == 'late' else ""))
                continue
            note = f"greatest of {st['n']} cumulative sample(s)"
            print(f"    {key:<8} {st['max']:>10} {st['first']:>10} "
                  f"{st['delta']:>10}   {note}")
    else:
        print("  no SMC-BATT counter sample in this boot")

    if sm['keycount'] or sm['keywalk']:
        kc = sm['keycount']['count'] if sm['keycount'] else None
        if sm['keywalk']:
            kw = sm['keywalk']
            verdict = ('complete' if kw['named'] == kw['total'] and kw['total']
                       else 'INCOMPLETE' if kw['total'] else 'empty')
            print(f"  #KEY index walk: {kw['named']} of {kw['total']} names "
                  f"({verdict}); #KEY count={kc if kc is not None else '-'}")
            if kc is not None and kc != kw['total']:
                print(f"    !! WARN SMC-KEYWALK-COUNT: #KEY said {kc} keys, "
                      f"the walk enumerated {kw['total']}")
        else:
            print(f"  #KEY count={kc} — the walk never reported done")
    else:
        print("  #KEY index walk: not attempted in this boot")
    print("")


def smc_report(result):
    """Return (parsed_anything, clean)."""
    blocks = [sm for sm in (smc_boot(b) for b in result['boots']) if sm]
    print(f"=== SMC — {result['path']} ===")
    if not blocks:
        print(f"  no SMC-BATT / SMC-SCOUT lines in {len(result['boots'])} "
              f"boot(s)")
        return False, True
    print(f"  {len(blocks)} of {len(result['boots'])} boot(s) carry SMC "
          f"lines\n")
    for sm in blocks:
        print_smc_boot(sm)

    clean = True
    print("--- gap / busy / late across the capture (cumulative: greatest "
          "wins, never summed) ---")
    print(f"  {'boot':<6} {'samples':>8} {'gap':>10} {'busy':>10} "
          f"{'late':>10}")
    for sm in blocks:
        row = []
        for key in ('gap', 'busy', 'late'):
            st = sm['stats'][key]
            row.append(str(st['max']) if st['present'] else 'absent')
        print(f"  {sm['number']:<6} {len(sm['samples']):>8} "
              f"{row[0]:>10} {row[1]:>10} {row[2]:>10}")
    print("")

    # 'late' is the GR18 append.  A capture that mixes boots with and without
    # it is the normal reading mid-arc and is reported as such, not alarmed.
    with_late = [sm['number'] for sm in blocks if sm['stats']['late']['present']]
    without = [sm['number'] for sm in blocks
               if sm['samples'] and not sm['stats']['late']['present']]
    print(f"  late= present on boot(s): "
          f"{', '.join(with_late) if with_late else 'none'}")
    if without:
        print(f"  late= ABSENT on boot(s): {', '.join(without)} — pre-GR18 "
              f"wire. Absent is not zero and is not reported as zero.")
    for sm in blocks:
        st = sm['stats']['late']
        if st['present'] and st['max']:
            clean = False
            print(f"  !! WARN SMC-LATE: boot {sm['number']} late={st['max']} "
                  f"— value bytes arrived after the read stopped and were "
                  f"drained-and-discarded")
        unc = sm['stats']['unc']
        if unc['present'] and unc['max']:
            clean = False
            print(f"  !! WARN SMC-UNC: boot {sm['number']} unc={unc['max']} "
                  f"— a read the SMC would not close")
    print("")
    return True, clean


def smc_mode(result):
    parsed, clean = smc_report(result)
    if not parsed:
        print(f"ERROR: {result['path']}: --smc parsed 0 SMC lines.",
              file=sys.stderr)
        return EXIT_NO_DATA
    if not clean:
        print(f"WARNING: {result['path']}: --smc reported at least one finding "
              f"(see WARN lines above).", file=sys.stderr)
        return EXIT_FINDING
    return EXIT_OK


# --- WXN-x86 M1 sweep + WXAUDIT census (--wxn) -----------------------------
#
# THE DIVISION OF LABOUR, stated because it is the whole reason this section
# exists.  `unaos/scripts/specs/x86-witness.spec` pins the PRESENCE and the
# SHAPE of the four lines below, and deliberately pins no value on any of them:
# `pdpt_seen` / `nx_set` / `residue_leaves` / `kern_WX` are functions of the
# firmware's map, and a spec that fixed them would be firmware-specific for no
# gain (its own comment says so).  A line-at-a-time regex file also structurally
# cannot state an ORDERING claim or an AGGREGATE one.  Those are this section's:
#
#   * l1 + l2 + l3 == leaves — an identity over three separate call sites in the
#     walk.  The kernel asserts it too (memory.rs, right under the census line);
#     this is the belt to that brace, and it is the half that survives a build
#     with the assert compiled out.
#   * kern_WX vs residue_leaves — two INDEPENDENT counts of the same set, taken
#     by two different walkers at two different moments.  The sweep publishes
#     `residue_leaves`; the audit re-derives it as `kern_WX`.  They should agree
#     up to the leaves the firmware left read-only, which is platform-dependent
#     and therefore REPORTED as a delta, never asserted.
#   * kern_WX across boots — the one that matters most.  `kern_WX` counts the
#     kernel leaves that are both writable and executable, and every milestone
#     in the WXN arc drives it down (66047 before the sweep existed -> 1535 at
#     M1 -> ~253 at M2 -> 0 at M3).  A RISE between two consecutive boots is
#     coverage lost, and it is lost QUIETLY: every line still prints, every spec
#     rule still matches, and the verdict is still `SWEPT`.  Nothing else in the
#     tree would say a word.
#
# ABSENCE, and why it is not zero.  This capture family spans three build eras
# and the sections must not average across them: boots that predate `a0a2d163`
# carry a WXAUDIT census and nothing else, boots that predate `32724cb4` carry
# no leaf histogram and no `WXN-FBWC`, and boots that predate both carry none of
# it.  A missing wire is rendered as such and never as 0 — a `nx_set` of zero is
# a VACUOUS sweep, which is a fault, and a build that never had the sweep is
# not.

WXN_SWEEP_RE = re.compile(
    r'^\s*::\s*WXN-x86:\s*(.*?)\s*->\s*(SWEPT|VACUOUS|REFUSED)\b\s*(.*?)'
    r'\s*::\s*$')
# M2 — the huge-leaf splitter (`e8b11513`), and the wire whose absence from this
# file made the check below narrate a cause it had no reader for.  Same
# three-terminal shape as M1's sweep, deliberately: one line per boot, one
# verdict per line.  The `already run` refusal arm carries NO fields at all
# (':: WXN-M2: -> REFUSED (already run; …) ::'), which is why the body group is
# allowed to match empty.
WXN_M2_RE = re.compile(
    r'^\s*::\s*WXN-M2:\s*(.*?)\s*->\s*(SPLIT|VACUOUS|REFUSED)\b\s*(.*?)'
    r'\s*::\s*$')
WXN_CENSUS_RE = re.compile(r'^\s*::\s*WXAUDIT\s+x86:\s*(.*?)\s*::\s*$')
WXN_NXE_RE = re.compile(
    r'^\s*::\s*WXAUDIT-NXE:\s*(.*?)\s*->\s*(\S+)\s*::\s*$')
WXN_FBWC_RE = re.compile(r'^\s*::\s*WXN-FBWC:\s*(.*?)\s*->\s*(.*?)\s*::\s*$')
# `residue_leaves=1535 (1g=0 2m=1023 4k=512 pt=1)` — its own reader, because the
# breakdown's keys start with a DIGIT and the generic _kv() scanner (whose names
# must start with a letter) would silently read '1g=0' as 'g=0'.  A parenthetical
# is also why the bodies are stripped of parens before _kv() runs: 'pt=1)' would
# otherwise arrive with the bracket attached.
WXN_RESIDUE_RE = re.compile(r'residue_leaves=(\d+)\s*\(([^)]*)\)')
WXN_BREAKDOWN_RE = re.compile(r'(\d*[a-z]+)=(\d+)')
WXN_MIB_RE = re.compile(r'kern_WX=\d+\s*\((\d+)\s*MiB\)')
WXN_PARENS_RE = re.compile(r'\([^)]*\)')
# What a missing wire prints.  One string, so a reader who greps the output for
# it finds every instance.
WXN_ABSENT = 'wire absent (pre-32724cb4 build)'
# CR0.WP is M3a's, and NEITHER HALF OF THE OLD RULE WAS A CONSTANT.
#
# THE TARGET.  `wp_mask` is a per-core bitmask over the cores THIS boot brought
# online, so the full mask is `(1 << cores) - 1` read off the boot's own
# `cores=` field.  The literal 0xFF that used to live here was an 8-core rMBP
# assumption: it reads every healthy 6-core QEMU boot (`wp_mask=0x3F`, every core
# armed) as short, i.e. it could not tell a satisfied milestone from a failed one
# on the very captures the milestone is developed against.
#
# THE EXCUSE.  Before M3a nothing in this kernel set WP — the rMBP firmware
# leaves it clear on all eight cores, QEMU sets it on the BSP only — so a short
# mask was the documented state of the milestone and a NOTE.  From M3a
# (`syscall::init` arms CR0.WP on every core) a short mask is a FINDING: WP is
# what makes PTE.W bind ring 0 at all, so a core without it is a core on which
# every read-only page M3b/M3c create is advisory.  Which of the two eras a
# capture belongs to is DECIDED FROM THE CAPTURE — `wxn_m3a_era` below — and
# never from a constant, because the sentence that excuses a short mask must be
# unprintable on a boot that carries the arm.
WXN_M3A_PRE = 'pre-M3a'
WXN_M3A_POST = 'M3a+'
WXN_M3A_UNKNOWN = 'undecidable'


def _wxn_hexmask(text):
    """Parse '0xFF' / '0x0'.  None when it is not a hex literal — a mask this
    reader did not actually read must not be compared against the M3a target."""
    if text is None:
        return None
    try:
        return int(text, 16)
    except ValueError:
        return None


def _wxn_wp_target(cores):
    """The full per-core WP mask for a boot with `cores` cores online: the low
    `cores` bits.  None when the boot did not say how many cores it had, or said
    something this reader cannot use as a shift count — a target the TOOL
    invented is a target that cannot convict anyone."""
    if cores is None or cores <= 0 or cores > 64:
        return None
    return (1 << cores) - 1


def wxn_m3a_era(w):
    """Does the kernel that printed THIS boot carry WXN-x86 M3a (CR0.WP armed
    per core)?  Returns (era, evidence).

    Decided from the capture.  M3a adds no new serial line — it changes the
    VALUE of `wp=`/`wp_mask=` and widens the rollup's PASS condition — so the
    evidence is the coherence between the verdict and the masks on the wire:

      * M3a+    — a non-PASS verdict on a boot whose `nxe_mask` is COMPLETE.
        Only M3a's widened condition (`nxe == cores && wp == cores`, smp.rs
        `wxn_nxe_report`) can fail a boot that proved NXE on every core; the
        pre-M3a rollup fails on NXE alone.
      * pre-M3a — a PASS verdict with a COMPLETE `nxe_mask` and a SHORT
        `wp_mask`.  A kernel whose PASS required `wp == cores` could not have
        emitted that line.
      * pre-M3a (fallback) — no such verdict/mask combination, but the BSP's own
        CR0.WP reads 0 on the `WXN-x86:` sweep line.  `wxn_pdpt_sweep` runs ten
        lines AFTER `syscall::init` on the BSP (arch/x86_64/mod.rs), which is
        where M3a arms WP.  Weaker than the two above, and deliberately LAST:
        a post-M3a kernel whose arm FAILED on core 0 reads exactly the same way,
        so this must never outrank the verdict evidence.

    Anything else is undecidable and says so — an era this reader could not read
    is not an era it may assume."""
    nxe, sweep = w['nxe'], w['sweep']
    if nxe:
        f = nxe['fields']
        cores, armed = _num(f.get('cores')), _num(f.get('nxe'))
        complete = cores is not None and armed is not None and armed == cores
        wp_mask = _wxn_hexmask(f.get('wp_mask'))
        target = _wxn_wp_target(cores)
        if complete and nxe['verdict'] != 'PASS':
            return (WXN_M3A_POST,
                    f"the rollup printed -> {nxe['verdict']} on a boot whose "
                    f"nxe_mask={f.get('nxe_mask')} is COMPLETE ({armed} of "
                    f"{cores}), and only M3a's widened PASS condition "
                    f"(nxe == cores AND wp == cores) can fail such a boot")
        if (complete and nxe['verdict'] == 'PASS' and target is not None
                and wp_mask is not None and wp_mask != target):
            return (WXN_M3A_PRE,
                    f"the rollup printed -> PASS with wp_mask="
                    f"{f.get('wp_mask')} short of 0x{target:X}, and a kernel "
                    f"whose PASS required wp == cores could not have emitted "
                    f"that line")
    if sweep and _num(sweep['fields'].get('wp')) == 0:
        return (WXN_M3A_PRE,
                "the BSP's own CR0.WP reads 0 on the WXN-x86 sweep line, and "
                "wxn_pdpt_sweep runs after syscall::init — where M3a arms it")
    return (WXN_M3A_UNKNOWN,
            "no verdict/mask combination on this boot separates the two eras, "
            "and the sweep line does not show a clear BSP CR0.WP")


def wxn_boot(boot):
    """Pull one boot's WXN/WXAUDIT block apart, or None if it carries none.

    Every one of the four wires is optional and each records its own absence,
    because the eras above mean 'not in this boot' is three different facts."""
    sweep = census = nxe = fbwc = m2 = None
    dups = Counter()
    for line, probe_line in zip(boot['lines'], boot['probe']):
        mm = WXN_M2_RE.match(probe_line)
        if mm:
            body, verdict, tail = mm.group(1), mm.group(2), mm.group(3)
            dups['m2'] += 1
            m2 = {'fields': _kv(WXN_PARENS_RE.sub(' ', body)),
                  'verdict': verdict, 'tail': tail, 'line': line}
            continue
        m = WXN_SWEEP_RE.match(probe_line)
        if m:
            body, verdict, tail = m.group(1), m.group(2), m.group(3)
            fields = _kv(WXN_PARENS_RE.sub(' ', body))
            res = WXN_RESIDUE_RE.search(body)
            breakdown = OrderedDict(WXN_BREAKDOWN_RE.findall(res.group(2))) \
                if res else OrderedDict()
            dups['sweep'] += 1
            sweep = {'fields': fields, 'verdict': verdict, 'tail': tail,
                     'residue': _num(res.group(1)) if res else None,
                     'breakdown': breakdown,
                     'capped': bool(res) and 'CAPPED' in res.group(2),
                     'line': line}
            continue
        m = WXN_CENSUS_RE.match(probe_line)
        if m:
            body = m.group(1)
            fields = _kv(WXN_PARENS_RE.sub(' ', body))
            mib = WXN_MIB_RE.search(body)
            dups['census'] += 1
            census = {'fields': fields,
                      'mib': _num(mib.group(1)) if mib else None,
                      # The histogram is `32724cb4`'s APPEND; its absence is an
                      # era, not a zero.
                      'hist': all(k in fields for k in ('l1', 'l2', 'l3')),
                      'truncated': 'TRUNCATED' in body,
                      'line': line}
            continue
        m = WXN_NXE_RE.match(probe_line)
        if m:
            dups['nxe'] += 1
            nxe = {'fields': _kv(m.group(1)), 'verdict': m.group(2),
                   'line': line}
            continue
        m = WXN_FBWC_RE.match(probe_line)
        if m:
            dups['fbwc'] += 1
            fbwc = {'fields': _kv(m.group(1)), 'verdict': m.group(2),
                    'line': line}
    if not any((sweep, census, nxe, fbwc, m2)):
        return None
    # The build era, read off the wires themselves rather than off a version
    # string the capture does not carry.
    if m2:
        era = 'e8b11513+ (M1 sweep + M2 splitter)'
    elif (census and census['hist']) or fbwc:
        era = '32724cb4+ (verdict, histogram, FBWC)'
    elif sweep or nxe:
        era = 'a0a2d163 (sweep + NXE, no histogram/FBWC)'
    else:
        era = 'pre-a0a2d163 (WXAUDIT census only)'
    return {'number': boot['number'], 'sweep': sweep, 'census': census,
            'nxe': nxe, 'fbwc': fbwc, 'm2': m2, 'era': era, 'dups': dups,
            'modern': (census and census['hist']) or bool(fbwc)}


# Splitting one huge leaf replaces it with 512 children: the map gains 511
# leaves.  True at both levels M2 edits — a 1 GiB leaf demoted to 512 x 2 MiB
# (`demote_1g`) and a 2 MiB leaf split to 512 x 4 KiB (`split_2m`) — and it is
# the term that made Boot Y's `leaves=` go 66047 -> 66558 for one `split_2m=1`.
WXN_SPLIT_GAIN = 511


def wxn_m2_reconcile(w, residue, kern_wx):
    """Attribute the sweep/audit delta to the stage that actually owns it.

    THE DEFECT THIS EXISTS TO FIX, recorded because the shape of it matters more
    than the arithmetic.  Through Boot X this reader printed, for delta=0:

        the audit's count is the sweep's less the leaf/leaves the firmware
        already left read-only (platform-dependent; reported, not asserted)

    On Boot Y the delta was 1230 and that sentence was FALSE: the 1230 leaves
    are M2's splitter retiring them, and `WXN-M2` appeared nowhere in this file,
    so the tool had no wire for the cause it was naming.  Nothing went red —
    the sentence is hedged — which is precisely the failure mode this tree keeps
    convicting: an instrument narrating a cause it cannot see.  Left alone it
    would have explained away a REAL walker disagreement as firmware on the next
    boot.

    THE CLOSED FORM, derived from the emitter rather than fitted to one boot
    (memory.rs `wxn_split_stage`).  M1 leaves `residue` present leaves inside the
    spared GiBs; M2 then walks exactly those, so

        kern_WX == residue
                   + 511 * (split_2m + demote_1g)   leaves the splits ADD
                   - nx_2m - nx_4k                  leaves retired one at a time
                   - already_nx                     leaves the firmware had NX'd

    Boot Y: 1535 + 511*(1+0) - 1022 - 719 - 0 = 305 == kern_WX, exactly.

    IT IS NOT ALWAYS EVALUABLE, and this reader says so rather than guessing.
    `nx_pdpt` and `nx_pt` retire a whole SUBTREE with one write (512 or 262144
    leaves), and the count under it is not on the wire; `skip_user` abandons a
    subtree the same way.  When any of those is nonzero the closed form is
    unavailable — reported as such, never approximated — and `keep_x` vs
    `kern_WX` carries the check instead.  That one holds unconditionally: it is
    the kernel's own stated cross-check (two independent walks of the same
    tables, `keep_x` counted during the edit and `kern_WX` by the audit walk),
    and the emitter's comment names it in as many words."""
    m2 = w.get('m2')
    delta = residue - kern_wx
    head = (f"residue_leaves={residue}, audit kern_WX={kern_wx}, "
            f"delta={delta}")
    if not m2:
        # UNCHANGED wording for the no-M2 case, with its precondition now said
        # out loud.  Through Boot X this was the whole truth; it is still the
        # truth on any boot whose build predates the splitter.
        if delta >= 0:
            return True, [f"ok   residue/kern_WX: sweep {head} — no WXN-M2 "
                          f"line on this boot, so the audit's count is the "
                          f"sweep's less the leaf/leaves the firmware already "
                          f"left read-only (platform-dependent; reported, not "
                          f"asserted)"]
        return False, [
            f"WARN WXN-KERNWX-EXCEEDS-RESIDUE: boot {w['number']} "
            f"kern_WX={kern_wx} > residue_leaves={residue} (by {-delta}). "
            f"Read-only firmware leaves can only make the audit's count "
            f"SMALLER; the two walkers disagree about the map"]

    clean, msgs = True, []
    f = m2['fields']
    if m2['verdict'] != 'SPLIT':
        # M2 ran and wrote nothing.  The delta is NOT M2's, and the milestone
        # not happening is the finding — `wxn_verdicts` raises it; here the job
        # is only to stop attributing the delta to a stage that made no edit.
        return True, [
            f"note residue/kern_WX: sweep {head} — WXN-M2 is present but "
            f"-> {m2['verdict']}, so it wrote no entry and this delta is NOT "
            f"its doing (see the WXN-M2 warn above)"]

    def g(name):
        return _num(f.get(name))

    split_2m, demote_1g = g('split_2m'), g('demote_1g')
    nx_2m, nx_4k = g('nx_2m'), g('nx_4k')
    nx_pdpt, nx_pt = g('nx_pdpt'), g('nx_pt')
    already_nx, skip_user = g('already_nx'), g('skip_user')
    keep_x, xpages = g('keep_x'), g('xpages')

    terms = (split_2m, demote_1g, nx_2m, nx_4k, nx_pdpt, nx_pt, already_nx)
    if None in terms:
        clean = False
        msgs.append(
            f"WARN WXN-M2-UNREAD: boot {w['number']} the WXN-M2 line is "
            f"present but this reader could not parse its retirement fields "
            f"(demote_1g={f.get('demote_1g')} split_2m={f.get('split_2m')} "
            f"nx_pdpt={f.get('nx_pdpt')} nx_2m={f.get('nx_2m')} "
            f"nx_pt={f.get('nx_pt')} nx_4k={f.get('nx_4k')}). The delta cannot "
            f"be attributed and MUST NOT be read as firmware read-only leaves")
        return clean, msgs

    subtree = [(n, v) for n, v in (('nx_pdpt', nx_pdpt), ('nx_pt', nx_pt),
                                   ('skip_user', skip_user)) if v]
    if subtree:
        detail = ', '.join(f"{n}={v}" for n, v in subtree)
        msgs.append(
            f"note residue/kern_WX: sweep {head} — this delta is WXN-M2's "
            f"splitter, not firmware read-only leaves. The closed form cannot "
            f"be evaluated on this boot ({detail}): each of those retires or "
            f"abandons a whole SUBTREE with one write and the wire does not "
            f"count the leaves under it. keep_x vs kern_WX below is the check "
            f"that still holds")
    else:
        expect = (residue + WXN_SPLIT_GAIN * (split_2m + demote_1g)
                  - nx_2m - nx_4k - already_nx)
        form = (f"{residue} + {WXN_SPLIT_GAIN}*({split_2m} split_2m + "
                f"{demote_1g} demote_1g) - {nx_2m} nx_2m - {nx_4k} nx_4k "
                f"- {already_nx} already_nx = {expect}")
        if expect == kern_wx:
            msgs.append(
                f"ok   residue/kern_WX: sweep {head} — this delta is WXN-M2's "
                f"SPLITTER, not firmware read-only leaves, and it closes "
                f"exactly: {form} == kern_WX")
        else:
            clean = False
            msgs.append(
                f"WARN WXN-M2-UNRECONCILED: boot {w['number']} the WXN-M2 "
                f"arithmetic does not close — {form}, but the audit reports "
                f"kern_WX={kern_wx} (off by {kern_wx - expect}). M1's residue, "
                f"M2's own retirement counts and the audit walk are three "
                f"derivations of one number; when they disagree at least one "
                f"walker is wrong about the map, and the delta must NOT be "
                f"read as firmware read-only leaves")

    # The kernel's own cross-check, and the one that needs no guard.
    if keep_x is None:
        clean = False
        msgs.append(f"WARN WXN-M2-UNREAD: boot {w['number']} the WXN-M2 line "
                    f"carries no readable keep_x, so the splitter's own count "
                    f"cannot be checked against the audit walk")
    elif keep_x == kern_wx:
        msgs.append(f"ok   M2/kern_WX: keep_x={keep_x} == audit "
                    f"kern_WX={kern_wx} — the splitter's count and the audit "
                    f"walk are two independent walks of the same tables and "
                    f"they agree")
    elif kern_wx < keep_x:
        msgs.append(f"note M2/kern_WX: keep_x={keep_x} vs audit "
                    f"kern_WX={kern_wx} (short by {keep_x - kern_wx}) — the "
                    f"audit counts W^X leaves, so an executable leaf the "
                    f"firmware left READ-ONLY is kept by M2 and not counted by "
                    f"the audit. That direction is explicable; reported, not "
                    f"asserted")
    else:
        clean = False
        msgs.append(
            f"WARN WXN-M2-KEEPX-EXCEEDED: boot {w['number']} audit "
            f"kern_WX={kern_wx} > keep_x={keep_x} (by {kern_wx - keep_x}). M2 "
            f"kept {keep_x} leaf/leaves executable and the audit found MORE "
            f"writable-executable kernel leaves than that — read-only firmware "
            f"leaves can only make the audit's count SMALLER, so the two "
            f"walkers disagree about the map")

    # The third derivation, from the ELF header alone.  A NOTE and never a WARN:
    # `xpages + 1` is the executable extent plus the AP trampoline page, and a
    # host whose trampoline landed inside the extent would legitimately read
    # `keep_x == xpages`.  Reported so a drift is visible without being a red on
    # a machine nobody has booted.
    if keep_x is not None and xpages is not None:
        if keep_x == xpages + 1:
            msgs.append(f"ok   M2/ELF: keep_x={keep_x} == xpages+1 = "
                        f"{xpages + 1} (the executable extent plus the AP "
                        f"trampoline page) — a third derivation of the same "
                        f"number, from the ELF header alone")
        else:
            msgs.append(f"note M2/ELF: keep_x={keep_x} vs xpages+1 = "
                        f"{xpages + 1} — the ELF-derived prediction and the "
                        f"edit's own count differ by {keep_x - xpages - 1}; "
                        f"reported, not asserted (the +1 is the trampoline "
                        f"page, which need not lie outside the extent)")
    return clean, msgs


def wxn_selfchecks(w):
    """The three cross-field checks.  Returns (clean, [message]).

    Only the histogram identity is a WARN; the other two are reports whose
    correct value is platform- or milestone-dependent, and a WARN on either
    would be a rule that fires on a healthy bench."""
    msgs, clean = [], True
    census, sweep, nxe = w['census'], w['sweep'], w['nxe']

    # 1. l1 + l2 + l3 == leaves.
    if census and census['hist']:
        f = census['fields']
        l1, l2, l3 = (_num(f.get('l1')), _num(f.get('l2')), _num(f.get('l3')))
        leaves = _num(f.get('leaves'))
        if None in (l1, l2, l3, leaves):
            clean = False
            msgs.append(f"WARN WXN-HIST-UNREAD: boot {w['number']} census "
                        f"carries a histogram this reader could not parse "
                        f"(l1={f.get('l1')} l2={f.get('l2')} l3={f.get('l3')} "
                        f"leaves={f.get('leaves')})")
        elif l1 + l2 + l3 == leaves:
            msgs.append(f"ok   histogram sums: {l1} + {l2} + {l3} = "
                        f"{l1 + l2 + l3} == leaves={leaves}")
        else:
            clean = False
            msgs.append(
                f"WARN WXN-HIST-MISMATCH: boot {w['number']} l1+l2+l3 = "
                f"{l1 + l2 + l3} but leaves={leaves} (off by "
                f"{l1 + l2 + l3 - leaves}). Every `record` increments `leaves` "
                f"and exactly one bucket, so this is an identity — a walk that "
                f"breaks it is counting leaves it is not classifying")
    elif census:
        msgs.append(f"note histogram: {WXN_ABSENT} — l1/l2/l3 are "
                    f"`32724cb4`'s append and this census predates it; the "
                    f"sum check could not run, which is not the same as "
                    f"passing")
    else:
        msgs.append(f"note histogram: no WXAUDIT census on this boot — "
                    f"{WXN_ABSENT}")

    # 2. kern_WX vs residue_leaves.
    if census and sweep and sweep['residue'] is not None:
        kern_wx = _num(census['fields'].get('kern_WX'))
        residue = sweep['residue']
        if kern_wx is None:
            clean = False
            msgs.append(f"WARN WXN-KERNWX-UNREAD: boot {w['number']} census "
                        f"has no readable kern_WX")
        else:
            # WHICH STAGE OWNS THE DELTA is the whole question, and it is not
            # answerable from these two lines alone — see wxn_m2_reconcile.
            m2clean, m2msgs = wxn_m2_reconcile(w, residue, kern_wx)
            if not m2clean:
                clean = False
            msgs.extend(m2msgs)
    elif census and not sweep:
        msgs.append(f"note residue/kern_WX: no WXN-x86 sweep line on this "
                    f"boot — {WXN_ABSENT}; kern_WX="
                    f"{census['fields'].get('kern_WX', '-')} stands alone "
                    f"with nothing to cross-check it against")

    # 3. wp_mask vs nxe_mask, against the M3a target THIS BOOT IMPLIES, and read
    #    against the era THIS BOOT PROVES.  Four outcomes, and only one of them
    #    is the old note: satisfied, a pre-M3a milestone note, a post-M3a
    #    FINDING, and an era this reader could not decide (which is neither
    #    excused nor convicted — it is stated as unjudged).
    if nxe:
        f = nxe['fields']
        wp_mask = _wxn_hexmask(f.get('wp_mask'))
        nxe_mask = _wxn_hexmask(f.get('nxe_mask'))
        cores, armed = _num(f.get('cores')), _num(f.get('nxe'))
        target = _wxn_wp_target(cores)
        era, why = wxn_m3a_era(w)
        if wp_mask is None or target is None:
            msgs.append(
                f"note wp_mask={f.get('wp_mask')} cores={f.get('cores')} — the "
                f"target is (1 << cores) - 1 and this boot gave this reader "
                f"neither a readable mask nor a usable core count, so the WP "
                f"half was NOT checked here, which is not the same as passing")
        elif wp_mask == target:
            msgs.append(
                f"ok   wp_mask={f.get('wp_mask')} == 0x{target:X} = "
                f"(1 << {cores}) - 1 — M3a target met (CR0.WP armed on every "
                f"one of THIS boot's {cores} core(s)); nxe_mask="
                f"{f.get('nxe_mask')}")
        elif era == WXN_M3A_POST:
            clean = False
            msgs.append(
                f"WARN WXN-WP-SHORT: boot {w['number']} wp_mask="
                f"{f.get('wp_mask')} ({f.get('wp', '-')} of {cores} core(s)) is "
                f"short of the 0x{target:X} this boot's own cores= implies, and "
                f"this boot CARRIES M3a — {why}. CR0.WP is what makes PTE.W "
                f"bind ring 0, so on the core(s) missing it every read-only "
                f"kernel page is advisory and M3b/M3c are vacuous there. This "
                f"is a FAULT, not the milestone note that preceded it")
        elif era == WXN_M3A_PRE:
            msgs.append(
                f"note wp_mask={f.get('wp_mask')} ({f.get('wp', '-')} of "
                f"{cores} core(s)) vs nxe_mask={f.get('nxe_mask')} ({armed} of "
                f"{cores}) — the target is 0x{target:X} = (1 << {cores}) - 1, "
                f"and this boot PREDATES M3a ({why}), so CR0.WP was nobody's "
                f"job on it: the documented state of that milestone, not a "
                f"fault. The SAME reading on a boot that carries M3a is a "
                f"FINDING, not this note")
        else:
            msgs.append(
                f"note wp_mask={f.get('wp_mask')} ({f.get('wp', '-')} of "
                f"{cores} core(s)) is short of 0x{target:X}, and this reader "
                f"could NOT decide from the capture whether the boot carries "
                f"M3a ({why}) — so the shortfall is UNJUDGED here rather than "
                f"excused; whatever else this boot reports above is what "
                f"stands")
        if nxe_mask is not None and cores is not None and armed != cores:
            clean = False
            msgs.append(f"WARN WXN-NXE-SHORT: boot {w['number']} {armed} of "
                        f"{cores} core(s) proved EFER.NXE — every AP runs on "
                        f"the BSP's CR3, so a core without NXE ignores every "
                        f"bit the sweep wrote")
    else:
        msgs.append(f"note per-core NXE: {WXN_ABSENT}")
    return clean, msgs


def print_wxn_boot(w):
    print(f"Boot {w['number']}  WXN   (build era: {w['era']})")
    for kind, label in (('sweep', 'sweep'), ('m2', 'm2'), ('census', 'census'),
                        ('nxe', 'nxe'), ('fbwc', 'fbwc')):
        if w['dups'][kind] > 1:
            print(f"  !! {label}: {w['dups'][kind]} lines in one boot — the "
                  f"block prints one per boot, so the readings are not "
                  f"interchangeable; the LAST is reported below")

    if w['sweep']:
        s, f = w['sweep'], w['sweep']['fields']
        print(f"  sweep : -> {s['verdict']}"
              f"{(' ' + s['tail']) if s['tail'] else ''}")
        print(f"          ehdr={f.get('ehdr', '-')} img={f.get('img', '-')} "
              f"gib_img={f.get('gib_img', '-')} "
              f"gib_tramp={f.get('gib_tramp', '-')} "
              f"spare_n={f.get('spare_n', '-')}")
        print(f"          pdpt_seen={f.get('pdpt_seen', '-')} "
              f"nx_set={f.get('nx_set', '-')} "
              f"huge_leaf_nx={f.get('huge_leaf_nx', '-')} "
              f"already_nx={f.get('already_nx', '-')}")
        print("          skips  " + " ".join(
            f"{k}={f.get('skip_' + k, '-')}" for k in
            ('spare', 'user', 'pml4_user', 'selfmap', 'fb_lock', 'fb_base',
             'fb_walk')))
        if s['residue'] is not None:
            bd = " ".join(f"{k}={v}" for k, v in s['breakdown'].items())
            print(f"          residue_leaves={s['residue']} ({bd})"
                  f"{'  CAPPED' if s['capped'] else ''}  "
                  f"pge={f.get('pge', '-')} flush={f.get('flush', '-')} "
                  f"wp={f.get('wp', '-')}")
    else:
        print(f"  sweep : {WXN_ABSENT}")

    if w.get('m2'):
        m, f = w['m2'], w['m2']['fields']
        print(f"  m2    : -> {m['verdict']}"
              f"{(' ' + m['tail']) if m['tail'] else ''}")
        if f:
            print(f"          xseg={f.get('xseg', '-')} "
                  f"xsegs={f.get('xsegs', '-')} xpages={f.get('xpages', '-')} "
                  f"tramp={f.get('tramp', '-')} "
                  f"spare_n={f.get('spare_n', '-')}")
            print(f"          demote_1g={f.get('demote_1g', '-')} "
                  f"split_2m={f.get('split_2m', '-')} "
                  f"pool_used={f.get('pool_used', '-')}")
            print(f"          retired  nx_pdpt={f.get('nx_pdpt', '-')} "
                  f"nx_2m={f.get('nx_2m', '-')} nx_pt={f.get('nx_pt', '-')} "
                  f"nx_4k={f.get('nx_4k', '-')}   "
                  f"keep_x={f.get('keep_x', '-')} "
                  f"already_nx={f.get('already_nx', '-')} "
                  f"skip_user={f.get('skip_user', '-')}")
            print(f"          fb={f.get('fb', '-')} "
                  f"fb_delta={f.get('fb_delta', '-')} "
                  f"pge={f.get('pge', '-')} flush={f.get('flush', '-')}")
    else:
        print(f"  m2    : no WXN-M2 line on this boot — the splitter "
              f"(`e8b11513`) is not in this build")

    if w['census']:
        c, f = w['census'], w['census']['fields']
        print(f"  census: leaves={f.get('leaves', '-')} "
              f"user={f.get('user', '-')} user_WX={f.get('user_WX', '-')} "
              f"kern_WX={f.get('kern_WX', '-')} ({c['mib']} MiB) "
              f"tables={f.get('tables', '-')} nxe={f.get('nxe', '-')} "
              f"walk={f.get('walk', '-')}")
        if c['hist']:
            print(f"          histogram l1={f.get('l1')} (1G) "
                  f"l2={f.get('l2')} (2M) l3={f.get('l3')} (4K)")
        else:
            print(f"          histogram {WXN_ABSENT}")
    else:
        print(f"  census: {WXN_ABSENT}")

    if w['nxe']:
        f = w['nxe']['fields']
        print(f"  nxe   : cores={f.get('cores', '-')} nxe={f.get('nxe', '-')} "
              f"nxe_mask={f.get('nxe_mask', '-')} wp={f.get('wp', '-')} "
              f"wp_mask={f.get('wp_mask', '-')} -> {w['nxe']['verdict']}")
    else:
        print(f"  nxe   : {WXN_ABSENT}")

    if w['fbwc']:
        f = w['fbwc']['fields']
        print(f"  fbwc  : fb={f.get('fb', '-')} lvl={f.get('lvl', '-')} "
              f"e={f.get('e', '-')} pat={f.get('pat', '-')} "
              f"pcd={f.get('pcd', '-')} pwt={f.get('pwt', '-')} "
              f"w={f.get('w', '-')} fx={f.get('fx', '-')} "
              f"-> {w['fbwc']['verdict']}")
    elif w['sweep'] and w['sweep']['verdict'] == 'REFUSED':
        print("  fbwc  : not emitted — the sweep REFUSED and returned before "
              "the interlock, which is the correct behaviour for that arm")
    else:
        print(f"  fbwc  : {WXN_ABSENT}")
    print("")


def wxn_verdicts(w):
    """The terminal words, which are the whole assertion in three of the four
    wires.  Returns (clean, [message])."""
    msgs, clean = [], True
    s = w['sweep']
    if s and s['verdict'] == 'VACUOUS':
        clean = False
        msgs.append(
            f"WARN WXN-VACUOUS: boot {w['number']} sweep wrote NOTHING "
            f"(nx_set={s['fields'].get('nx_set', '-')} "
            f"pdpt_seen={s['fields'].get('pdpt_seen', '-')} "
            f"skip_pml4_user={s['fields'].get('skip_pml4_user', '-')}). The "
            f"milestone is a no-op that logs like a success; the failure is "
            f"fail-SAFE, which is exactly why nothing else notices it")
    elif s and s['verdict'] == 'REFUSED':
        clean = False
        msgs.append(f"WARN WXN-REFUSED: boot {w['number']} sweep refused "
                    f"{s['tail']} — no entry was written and the kernel map is "
                    f"unprotected on this boot")
    # M2, and the reason its terminals get their own warns rather than leaning
    # on the kern_WX trend: a REFUSED or VACUOUS M2 leaves `kern_WX` at M1's
    # value, so the trend reports "kern_WX never rose" — a TRUE sentence that
    # reads as success.  The milestone failing to happen is otherwise
    # indistinguishable from the milestone happening.
    m2 = w.get('m2')
    if m2 and m2['verdict'] == 'VACUOUS':
        clean = False
        msgs.append(
            f"WARN WXN-M2-VACUOUS: boot {w['number']} the splitter wrote "
            f"NOTHING (keep_x={m2['fields'].get('keep_x', '-')} "
            f"skip_user={m2['fields'].get('skip_user', '-')}). kern_WX keeps "
            f"M1's whole residue and every other line still reads normal")
    elif m2 and m2['verdict'] == 'REFUSED':
        clean = False
        msgs.append(
            f"WARN WXN-M2-REFUSED: boot {w['number']} the splitter refused "
            f"{m2['tail']} — no entry was written, so the kernel map keeps "
            f"M1's whole residue on this boot. A pool too small for the "
            f"pre-pass is the arm the M2 playbook named as its falsifier")
    if w['census'] and w['census']['truncated']:
        clean = False
        msgs.append(
            f"WARN WXAUDIT-TRUNCATED: boot {w['number']} the audit walk ran "
            f"out of budget, so every number on the census line is an "
            f"UNDERESTIMATE — including user_WX=0, which is the audit's whole "
            f"point")
    if w['nxe'] and w['nxe']['verdict'] != 'PASS':
        clean = False
        msgs.append(f"WARN WXN-NXE-{w['nxe']['verdict']}: boot {w['number']} "
                    f"per-core NXE rollup did not pass")
    fb = w['fbwc']
    if fb and fb['verdict'] == 'SKIPPED':
        clean = False
        msgs.append(
            f"WARN WXN-FBWC-SKIPPED: boot {w['number']} the GR15 tripwire DID "
            f"NOT RUN (skip_lock={fb['fields'].get('skip_lock', '-')} "
            f"skip_base={fb['fields'].get('skip_base', '-')} "
            f"skip_walk={fb['fields'].get('skip_walk', '-')}) — the sweep's "
            f"effect on the panel mapping was never checked on this boot")
    elif fb:
        good, got, entry = _wc_typing(fb['fields'])
        if good:
            msgs.append(f"ok   boot {w['number']} fb: pat={got[0]} "
                        f"pcd={got[1]} pwt={got[2]} -> {entry}")
        else:
            clean = False
            want = tuple(WC_EXPECT.values())
            msgs.append(
                f"WARN WXN-FBWC-TYPING: boot {w['number']} fb is pat={got[0]} "
                f"pcd={got[1]} pwt={got[2]} -> {entry}; WC needs pat={want[0]} "
                f"pcd={want[1]} pwt={want[2]} (PA4). The panel is not "
                f"write-combining after the sweep on this boot")
    return clean, msgs


# --- what a kern_WX RISE actually means ------------------------------------
#
# Until WXN M3b clears W from the executable extent, every kernel code page is
# both writable and executable, so `kern_WX` is not "how much coverage was lost"
# — it is "how many code pages exist".  A boot that ADDS CODE therefore raises
# `kern_WX` by exactly the pages it added, with W^X coverage of the map
# unchanged and total.  Boot Z did precisely that: 304 -> 318 `xpages` and
# 305 -> 319 `kern_WX`, +14 on both wires.
#
# The discriminator is an identity the reader ALREADY prints three ways and then
# used to ignore:
#
#     kern_WX == keep_x == xpages + 1
#
# `xpages` is the ELF-derived executable extent, +1 is the AP trampoline page,
# `keep_x` is the splitter's own count of the leaves it left executable, and
# `kern_WX` is the audit walk's independent recount.  When all three agree,
# EVERY W^X leaf is an executable page — no DATA page is W^X, coverage is total,
# and a rise can only be code growth.  When `kern_WX` exceeds `xpages + 1`,
# there are W^X leaves the executable extent does not account for: pages that
# are writable AND executable without being kernel code.  That is the fault the
# WARN was written for, and it is a different finding from code growth.
#
# The two must therefore read differently — and the third case, a boot that
# cannot supply all three terms, must read differently again: it is not evidence
# of health, so it keeps the conservative WARN.  Never downgrade on absence.
WXN_CODEGROWTH_TAG = 'CODE GROWTH'


def _pages_bytes(pages):
    """Page count as a bytes-equivalent, for a reader who thinks in KiB."""
    total = pages * 4096
    if total >= (1 << 20):
        text = f"{total / float(1 << 20):.1f}"
        return f"{text[:-2] if text.endswith('.0') else text} MiB"
    return f"{total // 1024} KiB"


def wxn_extent(w):
    """Evaluate `kern_WX == keep_x == xpages + 1` on ONE boot.

    Returns a dict carrying the three terms, `holds` (True / False / None), and
    `why` — the sentence the report uses.  `holds is None` means the identity
    could not be evaluated at all, which is a THIRD answer and never a pass:
    a boot missing any term cannot testify that its coverage is total, and it
    cannot testify that it is not."""
    c, m2 = w['census'], w['m2']
    kern_wx = _num(c['fields'].get('kern_WX')) if c else None
    keep_x = _num(m2['fields'].get('keep_x')) if m2 else None
    xpages = _num(m2['fields'].get('xpages')) if m2 else None
    out = {'number': w['number'], 'kern_wx': kern_wx, 'keep_x': keep_x,
           'xpages': xpages, 'holds': None, 'why': None, 'unaccounted': None}
    missing = []
    if kern_wx is None:
        missing.append('its WXAUDIT census carries no readable kern_WX')
    if m2 is None:
        # NOT `WXN_ABSENT`: that string names the 32724cb4 histogram/FBWC era.
        # The M2 wire arrived later, at e8b11513, and mis-naming the era is how
        # a reader talks a reader out of looking.
        missing.append('it carries no WXN-M2 line at all — the executable '
                       'extent was never printed on this boot (WXN-M2 wire '
                       'absent, pre-e8b11513 build)')
    else:
        if xpages is None:
            missing.append('its WXN-M2 line carries no readable xpages')
        if keep_x is None:
            missing.append('its WXN-M2 line carries no readable keep_x')
    if missing:
        out['why'] = (f"boot {w['number']} cannot be tested against "
                      f"kern_WX == keep_x == xpages+1 because "
                      f"{' and '.join(missing)}")
        return out
    predicted = xpages + 1
    out['unaccounted'] = kern_wx - predicted
    out['holds'] = (kern_wx == keep_x == predicted)
    if out['holds']:
        out['why'] = (f"boot {w['number']}: kern_WX={kern_wx} == "
                      f"keep_x={keep_x} == xpages+1 = {predicted}")
    else:
        out['why'] = (f"boot {w['number']}: kern_WX={kern_wx} vs "
                      f"keep_x={keep_x} vs xpages+1 = {predicted} — the three "
                      f"walkers do not agree")
    return out


def wxn_classify_rise(prev_w, cur_w, pk, ck):
    """Classify a kern_WX rise.  Returns (kind, text).

    `kind` is 'codegrowth' (informational, NOT a finding), 'coverage' (the
    strengthened WARN) or 'undecidable' (the conservative WARN kept intact)."""
    pe, ce = wxn_extent(prev_w), wxn_extent(cur_w)
    head = (f"kern_WX {pk} -> {ck} across boots {prev_w['number']} -> "
            f"{cur_w['number']} (+{ck - pk})")
    warn = (f"WARN WXN-KERNWX-ROSE: {head}. W^X coverage of the kernel map "
            f"SHRANK between these two boots and every line still printed "
            f"clean")
    # The genuine alarm first: unaccounted leaves convict regardless of what the
    # earlier boot could or could not say.
    if ce['holds'] is False and ce['unaccounted'] > 0:
        return 'coverage', (
            f"{warn}. {ce['unaccounted']} W^X leaf/leaves on boot "
            f"{cur_w['number']} are NOT accounted for by the executable "
            f"extent — kern_WX={ce['kern_wx']} exceeds xpages+1 = "
            f"{ce['xpages'] + 1} (keep_x={ce['keep_x']}) by "
            f"{ce['unaccounted']}. That many leaves are writable AND "
            f"executable without being kernel code: coverage LOST, not code "
            f"added")
    if pe['holds'] and ce['holds']:
        grew = ce['xpages'] - pe['xpages']
        return 'codegrowth', (
            f"{WXN_CODEGROWTH_TAG}: {head} — and the identity "
            f"kern_WX == keep_x == xpages+1 holds on BOTH boots ({pe['why']}; "
            f"{ce['why']}), so every W^X leaf is an executable page: no DATA "
            f"page is W^X and coverage of the kernel map is still TOTAL. The "
            f"rise is CODE GROWTH — the executable extent grew by {grew} "
            f"page(s) (~{_pages_bytes(grew)}) — which is expected until WXN "
            f"M3b clears W from the executable extent. NOT a finding")
    # Everything else: the discrimination did not happen.  Say so, and keep the
    # conservative reading — an unevaluable identity is not evidence of health.
    if ce['holds'] is None or pe['holds'] is None:
        reason = "; ".join(e['why'] for e in (pe, ce) if e['holds'] is None)
    else:
        reason = "; ".join(e['why'] for e in (pe, ce) if not e['holds'])
    return 'undecidable', (
        f"{warn}. The code-growth discrimination was NOT POSSIBLE on this "
        f"pair: {reason}. A rise this reader cannot attribute to the "
        f"executable extent is reported at its conservative reading, never "
        f"downgraded on missing evidence")


def wxn_trend(blocks):
    """The cross-boot table and its two verdicts.

    Returns (rows, findings, notes).  `rows` is one tuple per boot; a field the
    boot did not carry is None and prints as the absence string, never as 0.

    `xpages` and `keep_x` ride in the row so that CODE GROWTH is DATA the reader
    can see, not an inference from a sentence underneath the table.  They are
    APPENDED rather than inserted: the row is positional, and the selftest reads
    `kern_WX` and `nx_set` off fixed indices."""
    rows = []
    for w in blocks:
        c, s, m2 = w['census'], w['sweep'], w['m2']
        rows.append((
            w['number'],
            _num(c['fields'].get('kern_WX')) if c else None,
            s['residue'] if s else None,
            _num(s['fields'].get('nx_set')) if s else None,
            _num(c['fields'].get('walk')) if c else None,
            w['era'],
            _num(m2['fields'].get('xpages')) if m2 else None,
            _num(m2['fields'].get('keep_x')) if m2 else None,
        ))
    findings, notes = [], []
    for i, (prev, cur) in enumerate(zip(rows, rows[1:])):
        prev_w, cur_w = blocks[i], blocks[i + 1]
        pk, ck = prev[1], cur[1]
        if pk is None or ck is None:
            notes.append(f"boots {prev[0]} -> {cur[0]}: no kern_WX on "
                         f"{'both' if pk is None and ck is None else (prev[0] if pk is None else cur[0])}"
                         f" — no comparison possible, which is not a pass")
            continue
        if ck > pk:
            kind, text = wxn_classify_rise(prev_w, cur_w, pk, ck)
            (notes if kind == 'codegrowth' else findings).append(text)
        elif ck < pk:
            notes.append(f"MILESTONE: kern_WX {pk} -> {ck} across boots "
                         f"{prev[0]} -> {cur[0]} (-{pk - ck}) — the sweep "
                         f"covers more of the map than it did on the previous "
                         f"boot")
    return rows, findings, notes


def print_wxn_trend(rows, findings, notes):
    print("--- cross-boot TREND (kern_WX / xpages / keep_x / residue_leaves / "
          "nx_set / walk) ---")
    print(f"  {'boot':>5}  {'kern_WX':>9}  {'xpages':>7}  {'keep_x':>7}  "
          f"{'residue':>9}  {'nx_set':>7}  {'walk':>10}   era")
    for num, kern_wx, residue, nx_set, walk, era, xpages, keep_x in rows:
        def cell(v, width, unit=''):
            return (f"{v}{unit}".rjust(width) if v is not None
                    else '-'.rjust(width))
        print(f"  {str(num):>5}  {cell(kern_wx, 9)}  {cell(xpages, 7)}  "
              f"{cell(keep_x, 7)}  {cell(residue, 9)}  "
              f"{cell(nx_set, 7)}  {cell(walk, 10, 'kcyc')}   {era}")
    for note in notes:
        print(f"  note {note}")
    for finding in findings:
        print(f"  !! {finding}")
    if not findings and len(rows) > 1:
        # 'never rose' would be FALSE on a capture that rose by code growth, and
        # a footer that contradicts the note above it is the instrument lying in
        # its own summary line.  Count them and say which claim is being made.
        grew = sum(1 for n in notes if n.startswith(WXN_CODEGROWTH_TAG))
        if grew:
            print(f"  ok   no UNACCOUNTED kern_WX rise across "
                  f"{len(rows) - 1} consecutive pair(s) — {grew} of them rose "
                  f"with kern_WX == keep_x == xpages+1 intact on both sides "
                  f"(code growth, noted above), which is not coverage loss")
        else:
            print(f"  ok   kern_WX never rose across {len(rows) - 1} "
                  f"consecutive pair(s)")
    if len(rows) < 2:
        print("  only one boot carries the WXN wires in this capture — NO "
              "TREND POSSIBLE. The kern_WX-rise signature needs two boots to "
              "be visible; this section has not cleared it, it has not tested "
              "it.")
    print("")


def wxn_fbwc_comparable(prev, cur):
    """Can this pair of boots actually be diffed?

    Its own predicate because the report's SUMMARY must count comparisons that
    happened, not pairs it walked past: 'N pair(s) compared: nothing changed'
    over a run where the leaf was never read twice is the exact shape of an
    instrument that reports its own silence as a pass."""
    return all(w['fbwc'] and w['fbwc']['fields'].get('e') is not None
               for w in (prev, cur))


def wxn_fbwc_diff(prev, cur):
    """DIFF the WXN-FBWC leaf across two consecutive boots.

    Same subject and same shared helper as --wxprobe's leaf DIFF, one wire over:
    WXPROBE reads the fb leaf as reconnaissance BEFORE the split stage, and
    WXN-FBWC reads it across the sweep's own edit window.  Reported as its own
    section rather than folded into --wxprobe because a leaf that moved between
    the two READS inside one boot is a different fault from one that moved
    between boots."""
    findings, notes = [], []
    if not prev['fbwc'] or not cur['fbwc']:
        which = [w['number'] for w in (prev, cur) if not w['fbwc']]
        notes.append(f"boots {prev['number']} -> {cur['number']}: no WXN-FBWC "
                     f"line on boot(s) {', '.join(str(n) for n in which)} — "
                     f"{WXN_ABSENT}; no comparison possible")
        return findings, notes
    pe = prev['fbwc']['fields'].get('e')
    ce = cur['fbwc']['fields'].get('e')
    # A SKIPPED line has no `e=` at all — the interlock never read the leaf.
    # Diffing a real entry against a missing one would report the GR15
    # signature on a boot where the leaf was never looked at, which is the
    # loudest possible way to say nothing.
    if pe is None or ce is None:
        which = [w['number'] for w in (prev, cur)
                 if w['fbwc']['fields'].get('e') is None]
        notes.append(
            f"boots {prev['number']} -> {cur['number']}: no leaf entry on "
            f"boot(s) {', '.join(str(n) for n in which)} — the interlock did "
            f"not read the leaf there (see the WXN-FBWC-SKIPPED warn), so "
            f"there is nothing to diff; this pair is UNTESTED, not clean")
        return findings, notes
    finding = _leaf_entry_change('WXN-FBWC', WC_LEAF, pe, ce,
                                 prev['number'], cur['number'])
    if finding:
        findings.append(finding)
    for k in ('fb', 'lvl'):
        pv, cv = (prev['fbwc']['fields'].get(k), cur['fbwc']['fields'].get(k))
        if pv != cv:
            notes.append(f"fbwc {k}: {pv} -> {cv} across boots "
                         f"{prev['number']} -> {cur['number']}")
    if prev['fbwc']['verdict'] != cur['fbwc']['verdict']:
        notes.append(f"fbwc verdict: {prev['fbwc']['verdict']} -> "
                     f"{cur['fbwc']['verdict']} across boots "
                     f"{prev['number']} -> {cur['number']}")
    return findings, notes


def wxn_report(result):
    """Return (parsed_anything, clean)."""
    blocks = [w for w in (wxn_boot(b) for b in result['boots']) if w]
    print(f"=== WXN — {result['path']} ===")
    if not blocks:
        print(f"  no WXN-x86 / WXAUDIT lines in {len(result['boots'])} boot(s)")
        return False, True
    silent = len(result['boots']) - len(blocks)
    print(f"  {len(blocks)} of {len(result['boots'])} boot(s) carry a WXN "
          f"block; {silent} carry none at all — {WXN_ABSENT}\n")

    clean = True
    for w in blocks:
        print_wxn_boot(w)
        vclean, vmsgs = wxn_verdicts(w)
        sclean, smsgs = wxn_selfchecks(w)
        if not (vclean and sclean):
            clean = False
        print("  self-checks:")
        for msg in vmsgs + smsgs:
            print(f"    {msg}")
        print("")

    rows, findings, notes = wxn_trend(blocks)
    if findings:
        clean = False
    print_wxn_trend(rows, findings, notes)

    print("--- WXN-FBWC consecutive-boot DIFF (raw leaf entry) ---")
    if len(blocks) < 2:
        print(f"  only {len(blocks)} boot carries a WXN block in this capture "
              f"— NO DIFF POSSIBLE. The fb-entry regression signature needs two "
              f"boots to be visible; this section has not cleared it, it has "
              f"not tested it.")
    else:
        any_finding, compared = False, 0
        for prev, cur in zip(blocks, blocks[1:]):
            if wxn_fbwc_comparable(prev, cur):
                compared += 1
            fnd, nts = wxn_fbwc_diff(prev, cur)
            for note in nts:
                print(f"  note {note}")
            for f in fnd:
                any_finding = True
                clean = False
                print(f"  !! {f}")
        if compared == 0:
            print(f"  NO PAIR COMPARED. {len(blocks) - 1} consecutive pair(s) "
                  f"were walked and not one of them had a readable leaf entry "
                  f"on both sides; this section has not cleared the fb-entry "
                  f"regression signature, it has not tested it.")
        elif not any_finding:
            print(f"  {compared} of {len(blocks) - 1} consecutive pair(s) "
                  f"comparable: no WXN-FBWC leaf entry changed across them "
                  f"(the rest are the notes above, and are untested rather "
                  f"than clean)")
    print("")
    return True, clean


def wxn_mode(result):
    parsed, clean = wxn_report(result)
    if not parsed:
        print(f"ERROR: {result['path']}: --wxn parsed 0 WXN-x86/WXAUDIT lines. "
              f"Either the capture predates the sweep or the wire moved.",
              file=sys.stderr)
        return EXIT_NO_DATA
    if not clean:
        print(f"WARNING: {result['path']}: --wxn reported at least one finding "
              f"(see WARN lines above).", file=sys.stderr)
        return EXIT_FINDING
    return EXIT_OK


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

# THE SAME THREE SHAPES LOGTS_PREFIX_RE STRIPS, one reader per kind so the
# number can be read out.  Their tolerances are deliberately identical to it --
# optional leading whitespace, optional space before 'ms' and inside the
# brackets, at most one space after the ']' -- because a shape that strip_logts
# removes but parse_ts does not recognise is a line the census reads as
# timestamped and the timing modes read as contention-deferred.  That divergence
# existed between the two lineages before the merge (this half wanted exactly one
# space after the ']'), it never fired on a real capture, and it is now closed by
# construction and asserted by selftest_logts().
TS_MONO_RE = re.compile(r'^\s*\[\s*(\d+)\s*ms\s*\]\s?')
TS_CIVIL_RE = re.compile(r'^\s*\[\s*(\d{2}):(\d{2}):(\d{2})Z\s*\]\s?')
TS_UNKNOWN_RE = re.compile(r'^\s*\[\s*\?\s*ms\s*\]\s?')
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
    """Rows for the timing modes: the ORIGINAL line, its clock kind and stamp,
    and the prefix-stripped `body` every pattern below is matched against.

    `body` comes from strip_logts() -- the same function the census/GR18 half
    uses for its probe copy -- rather than from the parse_ts match end, so there
    is exactly one definition in this file of where a stamp stops and a line
    starts.  On an unstamped row it is a no-op, which is why it can be applied
    unconditionally."""
    rows = []
    for line in content.split('\n'):
        if not line.strip():
            continue
        parsed = parse_ts(line)
        if parsed:
            kind, ts, _body = parsed
            rows.append({'line': line, 'kind': kind, 'ts': ts,
                         'body': strip_logts(line)})
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


# --- THE TIMING MODES' EXIT-CODE CHANNEL ----------------------------------
#
# WHY THIS EXISTS.  Four modes (--wxprobe / --slowxfer / --smc / --wxn) return
# EXIT_FINDING=5 when they find the thing they exist to catch.  --gaps and --wcg
# were report-only and ALWAYS exited 0 — so a GR19 probe that flipped a wc-d
# window to `state=sealed -> UNPAID` made --wcg print
#
#     WARN [wc-d] win=3: coverage was never bought
#
# and still exit 0.  Anyone who scripts these two modes and reads `$?` is reading
# a constant, and a warn nobody can gate on is a warn that gets scrolled past.
# Both modes now return an exit code, and the REFUSAL path keeps the 1 it always
# had so a caller can still tell "could not measure" (1) from "measured, and
# here is a finding" (5).
#
# WHAT COUNTS AS A FINDING is deliberately narrow.  These modes report
# MEASUREMENTS, and a slow boot is not a fault — nothing here keys on a duration.
# Exactly two conditions qualify:
#
#   * the paygo honesty check firing (--wcg): a window the kernel SEALED, or one
#     still presenting past the deferral horizon with no full-coverage pass.
#     That is coverage that was never bought — a fault about the instrument, not
#     a reading about the clock;
#   * the two splitters DISAGREEING about how many boots the capture holds.
#     --gaps/--wcg cut on `hz=`; the census half cuts on the boot-start marker.
#     Two independent splitters converging is a cross-check these modes have
#     always had available and never made; when they diverge, every per-boot
#     table below is a blend of two boots and the numbers in it are measurements
#     of nothing.  Claimed ONLY when markers were actually found, so a capture
#     with no marker at all (a fixture, a partial log) is never accused of a
#     disagreement it cannot have.


def boot_marker_count(rows):
    """How many boot-start markers the CENSUS splitter would see in these rows.

    Returns 0 when the capture carries no marker at all — which is 'no claim',
    not 'zero boots'.  Matched against the prefix-stripped body, exactly as
    _find_markers does, so the two splitters are compared on equal terms."""
    n = 0
    for r in rows:
        for spec in BOOT_MARKERS:
            if spec['marker'].search(r['body']):
                if spec['number_re'] and not spec['number_re'].search(r['body']):
                    continue
                n += 1
                break
    return n


def segmentation_findings(label, rows, segments):
    """The splitter cross-check.  Returns [message]; prints nothing."""
    markers = boot_marker_count(rows)
    if not markers or markers == len(segments):
        return []
    return [f"WARN SEGMENTATION-DISAGREES: {label}: the hz= splitter these "
            f"timing modes cut on found {len(segments)} boot(s), while the "
            f"boot-start marker the census half cuts on appears {markers} "
            f"time(s). One of the two is wrong, and every per-boot table above "
            f"is scoped by the hz= answer — a merged pair reads as one long "
            f"boot with an invented gap where the reset was."]


def gaps_mode(filepath, top):
    return gaps_report(filepath, read_capture(filepath), top)


def gaps_report(label, content, top):
    rows = load_rows(content)
    if not refuse_unless_logts(label, rows, '--gaps'):
        return EXIT_USAGE

    print(f"--- gaps {label} ---")
    segments = segment_by_hz(rows)
    for n, (hz, chunk) in enumerate(segments, 1):
        boot_label = f"boot {n} (hz={hz})" if hz else f"boot {n} (hz unknown)"
        print(f"{boot_label}")
        print_gap_table("whole boot", chunk, top)

        window = find_kepler_window(chunk)
        if isinstance(window, str):
            print(f"  kepler window: {window}\n")
        else:
            start, end = window
            print_gap_table("kepler window", chunk[start:end + 1], top)
    findings = segmentation_findings(label, rows, segments)
    for f in findings:
        print(f"  {f}")
    return EXIT_FINDING if findings else EXIT_OK


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


# --- paygo: the pay-as-you-go witness battery (GR17) ---------------------
#
# GR17 made the battery pay for itself as it is used, and the two lines below are how it
# says so on the wire. Pass 1 over a window samples every PAYGO_LATTICE_N-th source pixel
# and marks itself `coverage=lattice16`; passes 2..budget are FULL coverage and are DEFERRED
# until the window is past `defer_ms` since kernel entry. That took the witness-armed
# `kepler=` block from 17 077 ms to 2 564 ms on metal (bootpace.md section 10h).
#
#   [wc-g] win=1 seq=0 own=no scale=1x app=.. .. fbbad=0/60352 coverage=lattice16 us=.. -> CLEAN
#   [wc-g] paygo win=1 state=waiting emit=1 lattice_n=16 deferred=1 defer_ms=15000 \
#          since_entry_ms=5005 clock=entry taken=1 budget=4 -> DEFERRED
#
# SCOPE, and it is NOT the kepler window. This is the one thing about this section that a
# reader must not get wrong, so it is enforced in code rather than trusted to a comment: on
# a paygo boot the deferral horizon (15 000 ms) falls LONG AFTER the kepler window closes.
# Measured on the s73 capture, boot 7: the window is 3646..6227 ms and holds three of the
# seven paygo lines, while the deferred full pass on the console window (17 403 ms) and both
# `-> PAID` completions (16 753 / 22 563 ms) are outside it. A paygo section scoped to the
# kepler window would therefore report the console window as lattice-only and never-paid and
# WARN about it — a false alarm — and would not see a single completion in the boot. So
# `paygo_stats` runs over the WHOLE BOOT SEGMENT, and the report says which scope it used.
#
# Three rules this section will not break:
#
#   * `deferred=` IS A RUNNING CENSUS, NOT AN INCREMENT. The emitter prints the window's
#     total-so-far on every line, so the reading is the value beside the GREATEST `emit=`
#     and summing the column is always wrong. On the console window of boot 7 that is the
#     difference between 264 (right) and 265 (a number with no meaning);
#   * a window is only reported PAID when a `-> PAID` line was actually seen. UNPAID here is
#     a statement about the CAPTURE, not a verdict about the kernel: a window that is still
#     presenting when the log ends is spending its budget exactly as designed;
#   * the WARN is the honesty check section 10h asks for, and it is deliberately narrow — a
#     window that kept presenting PAST the deferral horizon and never once got a
#     full-coverage pass. That is the shape in which paygo would be a coverage CUT rather
#     than a deferral: 1/16 of the surface verified, forever, with every verdict CLEAN.

# TWO INSTRUMENTS, ONE READER RULE. `wcg.rs` deferred the glass read-back first; `wm.rs`
# (peer commit 0f1d3dfc) then gave the wc-d scan-out verify the same treatment, and gave it
# deliberately the SAME KEY SET -- its own comment says so: "The key set is `[wc-g] paygo`'s
# exactly, so one reader rule serves both". So this reader is parameterised by tag rather
# than duplicated, and a third instrument adopting the shape costs one table entry.
#
# What differs between them is real, and is read off the line rather than assumed:
#   * battery depth. wc-g budgets FOUR samples per window; wc-d has TWO STAGES (lattice then
#     full) and prints `budget=2` as a literal, with `taken=` counting stages CLOSED;
#   * the sample line. wc-g's is `[wc-g] win=N seq=N …`, wc-d's is `[wc-d] verify win=N …`,
#     and wc-d's `coverage=` marker is inserted between `checked=` and `bad_cache=` where
#     wc-g's sits between `fbbad=` and `us=`. Both are INSERTIONS, which is what keeps the
#     pi4 gate's spans and `-> PASS`/`-> FAIL` terminals matching what they always matched.
PAYGO_RE = re.compile(
    r'^\[(wc-g|wc-d)\] paygo win=(\d+) state=(\w+) emit=(\d+) lattice_n=(\d+) deferred=(\d+) '
    r'defer_ms=(\d+) since_entry_ms=(\d+) clock=(\w+) taken=(\d+) budget=(\d+) -> (\w+)\b')
# ANCHORED TO A REAL TERMINAL, and this was a defect rather than a tightening. The first cut
# was `^\[wc-d\] verify win=(\d+)\b`, which matches EVERY wc-d line including all four of its
# SKIP arms (wm.rs:2917 teardown, :3141 degenerate row/geometry, :3149 no visible content rect,
# :3197 no memory for the source snapshot), and that broke the census in two directions at once:
#
#   * an ABORTED verdict counted as a pass. `-> SKIP (teardown)` carries a `coverage=` marker
#     (it is emitted from the same field list), so a teardown abort at full coverage was
#     counted in the `full` column -- and `full > 0` is exactly what the honesty check reads
#     to decide a window got its deferred pass. An abort could therefore SATISFY the WARN it
#     should have triggered: the window's coverage was never bought, and the check said it was;
#   * the three GEOMETRY/OOM skips (`-> SKIP (degenerate row/geometry)`, `(no visible content
#     rect)`, `(no memory for NxN source snapshot)`) are short lines carrying no `coverage=`
#     at all, so each one inflated the `unmarked` column -- which is meant to mean "a pass that
#     did not say what it covered", not "a pass that never happened".
#
# A verdict is now a line that ADJUDICATED: PASS, FAIL, or LIVE. SKIPs are counted apart, in
# their own column, because "the verify declined to run" is a third thing and folding it into
# either of the other two is how the check above went quietly wrong.
WCD_VERIFY_RE = re.compile(r'^\[wc-d\] verify win=(\d+)\b.*-> (?:PASS|FAIL|LIVE)\b')
# The reason is taken to the CLOSING PAREN, then digit runs are folded to `N`. Both halves are
# needed for the census key to be stable AND readable. A character-class cut stops at the first
# character it does not list, which on the OOM arm ("no memory for {}x{} source snapshot") lands
# mid-phrase and keys the bucket as "no memory for" -- an entry that reads like a truncation
# bug. Capturing to the paren fixes the readability but not the stability: the geometry is
# interpolated, so `128x128` and `8x8` would open two buckets for one reason. Folding the digits
# gives one key, "no memory for NxN source snapshot", whatever the surface was -- and it keeps
# working for any future arm that interpolates a number.
WCD_SKIP_RE = re.compile(r'^\[wc-d\] verify win=(\d+)\b.*-> SKIP \(([^)]*)\)')
WCD_SKIP_NUM_RE = re.compile(r'\d+')
# `coverage=` is an INSERTION on a sample line. Absent on any build without the knob, which
# is a different thing from `coverage=full` and is counted apart: an unmarked pass says
# nothing about what it covered.
COVERAGE_RE = re.compile(r'\bcoverage=(?:lattice(\d+)|(full))\b')

# tag -> (sample-line matcher, what one sample IS, what the battery buys)
PAYGO_TAGS = {
    'wc-g': (WCG_PASS_RE, 'samples', 'glass read-back (checksum passes)'),
    'wc-d': (WCD_VERIFY_RE, 'verifies', 'scan-out read-back (panel verify)'),
}


def paygo_stats(chunk, tag='wc-g'):
    """Per-window paygo census for ONE instrument over ONE WHOLE BOOT SEGMENT (see the note
    above for why the kepler window is the wrong scope). Returns None when the boot carries
    no paygo line for that tag -- a witness-armed boot without the knob, or a capture that
    predates the instrument, where every field here would be a zero pretending to be a
    measurement."""
    sample_re = PAYGO_TAGS[tag][0]
    wins = {}

    def win(wid):
        return wins.setdefault(wid, {
            'id': wid, 'lattice': 0, 'full': 0, 'unmarked': 0, 'lattice_n': None,
            'paygo': [], 'first_ms': None, 'last_ms': None, 'samples': 0,
            'skips': 0, 'skip_reasons': {},
        })

    any_paygo = False
    defer_ms = None
    for r in chunk:
        body = r['body']
        m = PAYGO_RE.match(body)
        if m:
            # One regex serves both instruments, so the tag is a FILTER here, not a
            # formality: folding wc-d's two-stage battery into wc-g's four-sample one would
            # produce a table that reconciles with neither.
            if m.group(1) != tag:
                continue
            any_paygo = True
            w = win(m.group(2))
            w['paygo'].append({
                'state': m.group(3), 'emit': int(m.group(4)),
                'lattice_n': int(m.group(5)), 'deferred': int(m.group(6)),
                'defer_ms': int(m.group(7)), 'since_entry_ms': int(m.group(8)),
                'clock': m.group(9), 'taken': int(m.group(10)),
                'budget': int(m.group(11)), 'verdict': m.group(12), 'row': r,
            })
            defer_ms = int(m.group(7))
            continue
        if tag == 'wc-d':
            m = WCD_SKIP_RE.match(body)
            if m:
                # A declined verify is neither a pass nor a coverage statement. Counted here
                # and nowhere else -- see the note on WCD_VERIFY_RE for what folding it into
                # the pass columns did to the honesty check.
                w = win(m.group(1))
                w['skips'] += 1
                reason = WCD_SKIP_NUM_RE.sub('N', m.group(2).strip())
                w['skip_reasons'][reason] = w['skip_reasons'].get(reason, 0) + 1
                continue
        m = sample_re.match(body)
        if not m:
            continue
        w = win(m.group(1))
        w['samples'] += 1
        cov = COVERAGE_RE.search(body)
        if cov is None:
            w['unmarked'] += 1
        elif cov.group(2):
            w['full'] += 1
        else:
            w['lattice'] += 1
            w['lattice_n'] = int(cov.group(1))
        if r['ts'] is not None:
            if w['first_ms'] is None:
                w['first_ms'] = r['ts']
            w['last_ms'] = r['ts']

    if not any_paygo:
        return None

    for w in wins.values():
        # THE CENSUS RULE: greatest emit= wins, never a sum.
        peak = max(w['paygo'], key=lambda p: p['emit'], default=None)
        w['deferred'] = peak['deferred'] if peak else 0
        w['peak_emit'] = peak['emit'] if peak else 0
        paid = [p for p in w['paygo'] if p['verdict'] == 'PAID']
        w['paid'] = bool(paid)
        # THE `UNPAID` VOCABULARY COLLISION, reconciled. This tool used "UNPAID" from the start
        # for a statement about the CAPTURE -- a window still spending its budget when the log
        # ended. The kernel then took the same word for a VERDICT: `wcd_seal` emits
        # `[wc-d] paygo … state=sealed … -> UNPAID` when a window's teardown-abort budget is
        # spent, meaning the battery closed and the coverage was never bought. Those are
        # opposite in severity -- one is "not finished yet", the other is "will never finish"
        # -- so printing one word for both would be the worst kind of wrong: plausible.
        # The kernel's verdict wins the capital letters. The capture statement is reworded to
        # "open" below, and the two can no longer be confused in a table or in a grep.
        w['sealed'] = [p for p in w['paygo']
                       if p['state'] == 'sealed' or p['verdict'] == 'UNPAID']
        # GR18 close terminal (45ebbd3d): `state=closed` at the greatest `emit=` means the tenant
        # said its last word; distinct from `sealed` (teardown-abort) and judged at the PEAK line
        # per the census rule, so a stale `closed` from an earlier tenant cannot shadow a live one.
        w['closed'] = bool(peak and peak['state'] == 'closed')
        w['taken'] = paid[0]['taken'] if paid else (
            max((p['taken'] for p in w['paygo']), default=0))
        w['budget'] = w['paygo'][0]['budget'] if w['paygo'] else 0
        w['clocks'] = sorted({p['clock'] for p in w['paygo']})
        w['horizon'] = w['paygo'][0]['defer_ms'] if w['paygo'] else defer_ms
        # The honesty check. Both halves are required: a window that stopped presenting
        # before the horizon was never owed a deferred pass, and a window that got one is
        # covered however long it ran.
        h = w['horizon']
        w['starved'] = (h is not None and w['last_ms'] is not None
                        and w['last_ms'] >= h and w['full'] == 0)
        # A SEALED window warns unconditionally, with no timing test. `-> UNPAID` is the
        # kernel's own statement that this battery closed without buying its coverage, which
        # is the WARN's exact subject arriving as a verdict rather than as an inference -- and
        # it can be reached EARLY, before the horizon, where the timing test would miss it.
        w['warn'] = w['starved'] or bool(w['sealed'])

    return {'wins': [wins[k] for k in sorted(wins, key=int)], 'defer_ms': defer_ms}


def print_paygo_stats(pg, tag='wc-g'):
    unit, what = PAYGO_TAGS[tag][1], PAYGO_TAGS[tag][2]
    if pg is None:
        print(f"  paygo [{tag}]: no '[{tag}] paygo' line in this boot — that battery ran "
              f"unbudgeted (no")
        print(f"    UNAOS_WCG_PAYGO), or the capture predates the instrument. Either way "
              f"there is no deferral")
        print("    to report, and a table of zeros here would read like a measurement.\n")
        return
    print(f"  paygo battery [{tag}] — {what} (WHOLE BOOT scope, not the kepler window: the "
          f"deferral")
    print(f"    horizon is {pg['defer_ms']}ms since entry, which on a paygo boot falls well "
          f"after the kepler")
    print("    window closes — see the note in the source)")
    skipcol = ' ' + f"{'skips':>6}" if tag == 'wc-d' else ''
    print(f"    {'win':>4} {unit:>9} {'lattice':>8} {'full':>6} {'unmarked':>9}{skipcol} "
          f"{'deferred':>9} {'emit':>5} {'taken/budget':>13} {'clock':>8}  status")
    for w in pg['wins']:
        # Precedence: the kernel's own verdict outranks anything this tool infers.
        if w['sealed']:
            status = 'SEALED -> UNPAID (kernel verdict)'
        elif w['paid']:
            status = 'PAID'
        elif w['closed']:
            # GR18 close terminal: `state=closed -> UNSPENT` is the tenant's last word — the
            # battery is TERMINATED, not open. (A close-paid battery reports PAID above.)
            status = 'CLOSED -> UNSPENT (terminal)'
        elif w['paygo']:
            status = 'open at capture end'
        else:
            status = 'no paygo line'
        skips = ' ' + f"{w['skips']:>6}" if tag == 'wc-d' else ''
        print(f"    {w['id']:>4} {w['samples']:>8} {w['lattice']:>8} {w['full']:>6} "
              f"{w['unmarked']:>9}{skips} {w['deferred']:>9} {w['peak_emit']:>5} "
              f"{str(w['taken']) + '/' + str(w['budget']):>13} "
              f"{','.join(w['clocks']) or '--':>8}  {status}")
    print("    deferred = the census beside the GREATEST emit=, never the column sum "
          "(it is a running total).")
    # The two senses of "unpaid", kept apart on the page as well as in the code.
    print("    SEALED -> UNPAID is the KERNEL's verdict: the battery closed and the coverage "
          "was never bought.")
    print("    'open at capture end' is this tool's statement about the CAPTURE: a window "
          "still running when")
    print("      the log ends is spending its budget as it earns it, which is what paygo is. "
          "Not the same thing.")
    if tag == 'wc-d':
        print("    skips = verifies that DECLINED to adjudicate (teardown abort, degenerate "
              "geometry, no rect,")
        print("      no memory). Not passes, and deliberately not folded into any coverage "
              "column — an aborted")
        print("      verdict that counted as a full pass would satisfy the honesty check "
              "below instead of firing it.")
    warned = [w for w in pg['wins'] if w['warn']]
    for w in warned:
        print(f"    WARN [{tag}] win={w['id']}: coverage was never bought.")
        if w['sealed']:
            s = w['sealed'][0]
            print(f"      The kernel SEALED this window at since_entry_ms={s['since_entry_ms']} "
                  f"(state=sealed -> UNPAID):")
            print("      its teardown-abort budget was spent, so it will not be verified again. "
                  "This is a verdict,")
            print("      not an inference, and it can arrive BEFORE the horizon — which is why "
                  "no timing test guards it.")
        if w['starved']:
            print(f"      Still running at {w['last_ms']}ms — past the {w['horizon']}ms "
                  f"deferral horizon — yet NO")
            print(f"      full-coverage pass ever ran ({w['lattice']} lattice, 0 full). That is "
                  f"a coverage CUT wearing a")
            print("      deferral's clothes, and every verdict it printed is CLEAN about the "
                  "pixels it looked at.")
            print("      Check the gate's clock (a `clock=unarmed` paygo line can never open it).")
    if not warned:
        print("    honesty check: no window was sealed, and every window that ran past the "
              "horizon got at least")
        print("      one full-coverage pass.")
    unmarked = sum(w['unmarked'] for w in pg['wins'])
    if unmarked:
        print(f"    NOTE: {unmarked} adjudicated line(s) carry no `coverage=` marker at all. An "
              f"unmarked pass is")
        print("      not a full pass — it is a pass that did not say, and it is counted apart "
              "rather than assumed.")
    skipped = sum(w['skips'] for w in pg['wins'])
    if skipped:
        reasons = {}
        for w in pg['wins']:
            for k, v in w['skip_reasons'].items():
                reasons[k] = reasons.get(k, 0) + v
        detail = ', '.join(f"{k} x{v}" for k, v in sorted(reasons.items()))
        print(f"    NOTE: {skipped} verify(s) declined to adjudicate — {detail}.")
    print("")


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


def wcg_paygo_findings(pg, tag, boot_n):
    """The paygo honesty check, as findings rather than as printed prose.

    Reads the SAME `warn` flag print_paygo_stats renders, so the exit code and
    the page can never disagree — a warn on the page with a green exit is the
    exact defect this was added to close, and computing it twice is how that
    comes back."""
    if pg is None:
        return []
    return [f"WARN [{tag}] boot {boot_n} win={w['id']}: coverage was never "
            f"bought ({'kernel SEALED the window' if w['sealed'] else 'still '
            'presenting past the deferral horizon with no full-coverage pass'})"
            for w in pg['wins'] if w['warn']]


def wcg_report(label, content, boot_sel):
    rows = load_rows(content)
    if not refuse_unless_logts(label, rows, '--wcg'):
        return EXIT_USAGE

    print(f"--- wcg {label} ---")
    segments = segment_by_hz(rows)
    if boot_sel is not None and not (1 <= boot_sel <= len(segments)):
        print(f"  --boot {boot_sel}: capture has {len(segments)} boot(s)")
        return EXIT_USAGE

    windows = 0
    findings = []
    for n, (hz, chunk) in enumerate(segments, 1):
        if boot_sel is not None and n != boot_sel:
            continue
        boot_label = f"boot {n} (hz={hz})" if hz else f"boot {n} (hz unknown)"
        print(boot_label)
        window = find_kepler_window(chunk)
        if isinstance(window, str):
            print(f"  kepler window: {window}\n")
            # The paygo census is still worth printing: it is whole-boot scoped, so a boot
            # whose kepler anchors never appeared can still have a complete deferral story.
            for _tag in PAYGO_TAGS:
                pg = paygo_stats(chunk, _tag)
                print_paygo_stats(pg, _tag)
                findings += wcg_paygo_findings(pg, _tag, n)
            continue
        start, end = window
        windows += 1
        print_wcg_stats(wcg_stats(chunk[start:end + 1]))
        for _tag in PAYGO_TAGS:
            pg = paygo_stats(chunk, _tag)
            print_paygo_stats(pg, _tag)
            findings += wcg_paygo_findings(pg, _tag, n)

    findings += segmentation_findings(label, rows, segments)
    if not windows:
        print(f"{label}: no kepler window in any boot; nothing to decompose")
        return EXIT_USAGE
    for f in findings:
        print(f"  {f}")
    return EXIT_FINDING if findings else EXIT_OK



# ---------------------------------------------------------------------------
# Fixtures — both lineages.  Timing fixtures first (--gaps / --wcg / paygo),
# then the GR18 section fixtures.  Every one of them is fed through the same
# entry point a real capture goes through: a fixture that exercises a private
# shortcut certifies the shortcut, not the tool.
# ---------------------------------------------------------------------------

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


# --wcg fixture 5 is REAL, and it is the PAYGO wire. It is boot 7 of
# ~/unaos-bench/capture/rmbp-gr16-s73/ttyUSB0.log -- the first of the two
# pay-as-you-go boots GR17 flew (`kepler=2564ms` on both, down from 17 077 ms;
# bootpace.md section 10h). Two regions are stitched, and the seam is the point:
#
#   * the KEPLER WINDOW, trimmed by the same rule fixture 1 was cut with -- runs of
#     consecutive lines in the same group had their interior dropped, which cannot move a
#     group total because the merged gap lands on the run's last line and that line is in
#     the same group. Every wc-* line and both anchors survive. 434 lines became 56;
#   * every `[wc-g]` line AFTER the window, untouched. These are not decoration. On a paygo
#     boot the deferral horizon (15 000 ms) falls long after the window closes at 6227 ms,
#     so the console window's deferred full pass (17 403 ms) and BOTH `-> PAID` completions
#     (16 753 / 22 563 ms) live out here. A fixture cut at the window would have proved a
#     paygo reader that cannot see paygo.
#
# Every expected value in wcg_paygo_expect() below was checked bit-identical against the
# UNTRIMMED boot before this text was embedded -- both the kepler-window decomposition (span,
# all seven group costs, per-pass costs, serial census) and the whole-boot paygo census (per
# window: samples, lattice/full/unmarked split, deferral census, peak emit, taken/budget,
# PAID, last-present stamp). 76 comparisons, zero mismatches.
#
# The capture path is deliberately NOT read at runtime, for fixture 1's reason: a fixture
# that needs a bench directory to still exist is a fixture that quietly stops running.
WCG_FIXTURE_PAYGO = """\
[   3646ms] [NVIDIA] Initializing Kepler GPU at BDF 1:0:0
[   3646ms] :: x86 mmio-map: 0xc0000000..0xc1000000 uc=8 (PAT PA3) wc-kept=0 ::
[   3652ms] :: x86 mmio-map: 0x90000000..0xa0000000 uc=113 (PAT PA3) wc-kept=15 ::
[   3652ms] [NVIDIA] Chipset: 0xE7, Stepping: 1.20642
[   4843ms] :: kdisp: fb-draw done ::
[   4857ms] :: fbcon: glyphs-active base=90020000 pitch=16384 cell=16x16 cols=180 rows=112 scale=2 ::
[   4858ms] :: [    3184 ms] portsw:flip ::
[   4858ms] :: kdisp: console-repaint rows=4 ::
[   4872ms] [wc-x] desktop-clear panel=2880x1800 bg=002D2B55
[   4873ms] [wc-a] create win=1 asid=0xffffff01 surf=1312x736 stride=5248 scale=1x at (784,457) z=1
[   4999ms] [wc-g] win=1 seq=0 own=no scale=1x app=0xcbf29ce484222325 blit=0x2088f1de4724e325 civac=0x2088f1de4724e325 after=0x2088f1de4724e325 fbbad=0/60352 coverage=lattice16 us=5119 rectscan_us=6814 slow=no -> CLEAN
[   4999ms] [wc-g] prof win=1 seq=0 surf_bytes=3862528 cks_blit_us=5738 civac_us=5738 cks_after_us=5744 probes=60352 readback_us=102831
[   4999ms] [wc-h] win=1 box=1314x750 span=750 band=no bytes=3942000 compose_us=2267 present_us=2660 rectscan_us=6944 torn=no -> BUFFERED
[   4999ms] [wc-a] composite windows=1 drawn=1
[   5005ms] [wc-h] win=1 box=1314x750 span=64 band=yes bytes=336384 compose_us=192 present_us=234 rectscan_us=592 torn=no -> BUFFERED
[   5005ms] [wc-g] paygo win=1 state=waiting emit=1 lattice_n=16 deferred=1 defer_ms=15000 since_entry_ms=5005 clock=entry taken=1 budget=4 -> DEFERRED
[   5180ms] [wc-d] verify win=1 surf=1312x736 band=0..64 scale=1x at (784,457) panel=2880x1800 checked=83968 bad_cache=0 bad_ram=0 ram_indep=no moved=0 nonzero=8300 cksum=0x6ea90580b6e52525 first=none -> PASS
[   5185ms] [wc-x] console-route first-paint win=1 (glyphs -> window surface, damage-limited)
[   5185ms] [wc-x] console-window win=1 panel=2880x1800 surf=1312x736 box=1314x750 at (783,444) cell=16x16 cols=82 rows=46
[   5185ms] [wc-h] win=1 box=1314x750 span=80 band=yes bytes=420480 compose_us=239 present_us=290 rectscan_us=740 torn=no -> BUFFERED
[   5185ms] [wc-x] console-window panic-fallback armed win=1 (panic paints the PANEL, not the window)
[   5190ms] [wc-h] win=1 box=1314x750 span=750 band=no bytes=3942000 compose_us=2254 present_us=2660 rectscan_us=6944 torn=no -> BUFFERED
[   5190ms] [wc-h] win=1 box=1314x750 span=48 band=yes bytes=252288 compose_us=144 present_us=177 rectscan_us=444 torn=no -> BUFFERED
[   5190ms] [wc-x] activate panel=2880x1800 console_win=1
[   5191ms] [wc-h] win=1 box=1314x750 span=64 band=yes bytes=336384 compose_us=191 present_us=233 rectscan_us=592 torn=no -> BUFFERED
[   5191ms] [wc-h] rollup win=1 scope=window-band emit=1 age_ms=300 pop=budgeted samples=4 budget=4 pop=all-presents torn=0 declines=0 fixture=0 whole=3 banded=4 lines=6 minspan=48 minspan_bytes=252288 maxpresent_us=2660 pop=constant frame_us=16667 -> TEAR-FREE
[   5193ms] [wc-g] win=2 seq=0 own=no scale=8x app=0xcbf29ce484222325 blit=0x47b750fe2093a4da civac=0x47b750fe2093a4da after=0x47b750fe2093a4da fbbad=0/384 coverage=lattice16 us=1233 rectscan_us=4740 slow=no -> CLEAN
[   5193ms] [wc-g] prof win=2 seq=0 surf_bytes=24576 cks_blit_us=36 civac_us=36 cks_after_us=36 probes=384 readback_us=622
[   5193ms] [wc-h] win=2 box=770x526 span=526 band=no bytes=1620080 compose_us=135 present_us=1097 rectscan_us=4870 torn=no -> BUFFERED
[   5193ms] [wc-c] side-by-side windows=2 drawn=2
[   5200ms] [wc-x] spawn-place win=2 box=770x526 at (2102,1104) (created in place, no move)
[   5201ms] [wc-h] win=2 box=770x526 span=526 band=no bytes=1620080 compose_us=135 present_us=1097 rectscan_us=4870 torn=no -> BUFFERED
[   5201ms] [wc-g] paygo win=2 state=waiting emit=1 lattice_n=16 deferred=1 defer_ms=15000 since_entry_ms=5201 clock=entry taken=1 budget=4 -> DEFERRED
[   6028ms] [wc-d] verify win=2 surf=96x64 band=none scale=8x at (2103,1117) panel=2880x1800 checked=393216 bad_cache=0 bad_ram=0 ram_indep=no moved=0 nonzero=91840 cksum=0x47b750fe2093a4da first=none -> PASS
[   6029ms] [wcn] win=1 asid=0xffffff01 live=yes above=yes att=6 comp=7 hid=0 bel=0 rate=30.0/s comp_rate=35.0/s active=200ms parked=0ms gap=1..186ms
[   6029ms] [wcn] win=2 asid=0xffffff02 live=yes above=yes att=1 comp=2 hid=0 bel=0 rate=0.1/s comp_rate=0.3/s active=0ms parked=0ms gap=0..0ms
[   6029ms] [wcn] rollup scope=live wins=2 att=7 comp=9 hid=0 bel=0 stale=0 passes=9 aborted=0 att_rate=1.2/s comp_rate=1.5/s span=5663ms -> LIVE
[   6030ms] [comp2] rollup passes=9 pass_us=128380 max_us=829460 sprite_us=0 wait_us=0 blit_us=127685 cache_us=0 bytes_pp=2078282 dmg_px_pp=519570 box_px_pp=1011006 rate=1.5/s span=5663ms
[   6031ms] [wc-x] present win=2 rows=1104..1630 ok=true
[   6031ms] [wc-g] win=3 seq=0 own=no scale=8x app=0xcbf29ce484222325 blit=0xda5b3a56c0971925 civac=0xda5b3a56c0971925 after=0xda5b3a56c0971925 fbbad=0/64 coverage=full us=22 rectscan_us=592 slow=no -> CLEAN
[   6031ms] [wc-g] prof win=3 seq=0 surf_bytes=256 cks_blit_us=0 civac_us=0 cks_after_us=0 probes=64 readback_us=62
[   6031ms] [wc-h] win=3 box=66x78 span=78 band=no bytes=20592 compose_us=7 present_us=14 rectscan_us=722 torn=no -> BUFFERED
[   6031ms] [wc-a] create win=3 asid=0x0 surf=8x8 stride=32 scale=8x at (9,21) z=3
[   6031ms] [wc-h] win=3 box=66x78 span=78 band=no bytes=20592 compose_us=7 present_us=21 rectscan_us=722 torn=no -> BUFFERED
[   6031ms] [wc-g] paygo win=3 state=waiting emit=1 lattice_n=16 deferred=1 defer_ms=15000 since_entry_ms=6031 clock=entry taken=1 budget=4 -> DEFERRED
[   6039ms] [wc-d] verify win=3 surf=8x8 band=none scale=8x at (9,21) panel=2880x1800 checked=4096 bad_cache=0 bad_ram=0 ram_indep=no moved=0 nonzero=4096 cksum=0xda5b3a56c0971925 first=none -> PASS
[   6039ms] [wc-k] erase box=66x78 staged=yes rowbytes=264 runs=78 contig=yes compose_us=0 present_us=14 rectscan_us=722 torn=no -> BUFFERED
[   6040ms] [wc-h] win=3 box=66x78 span=78 band=no bytes=20592 compose_us=7 present_us=14 rectscan_us=722 torn=no -> BUFFERED
[   6040ms] [wc-h] rollup win=3 scope=window emit=1 age_ms=8 pop=budgeted samples=4 budget=4 pop=all-presents torn=0 declines=0 fixture=0 whole=4 banded=0 lines=3 minspan=0 minspan_bytes=0 maxpresent_us=21 pop=constant frame_us=16667 -> TEAR-FREE
[   6040ms] [wc-a] close win=3
[   6040ms] [wc-k] erase box=66x78 staged=yes rowbytes=264 runs=78 contig=yes compose_us=0 present_us=21 rectscan_us=722 torn=no -> BUFFERED
[   6041ms] [wc-x] move-vacate win=3 scale=8x from=(8,8) to=(90,8) box=66x78 painted=true desktop=5/5 stale=0/5 -> PASS
[   6041ms] :: kdisp: landed trace [917D0210 0000FFFF DEAD0000 DEAD0000 DEAD0000 DEAD0000 DEAD0000] ::
[   6210ms] [NVIDIA] Initialization complete (Phases 1-4)
[   6211ms] [PCI-STOR] storage-class census (class 0x01 mass-storage, class 0x08/0x05 SDHCI)...
[   6227ms] :: GPACE: span=2586ms anchor=enum:p1 since-entry=6226ms hz=2693865979 build=kepler+takeover+fifo+ivb+wc+smc+ == the pci-usb d= split ::
[  15453ms] [wc-g] win=3 seq=0 own=no scale=6x app=0xcbf29ce484222325 blit=0xeb05052ea5b62325 civac=0xeb05052ea5b62325 after=0xeb05052ea5b62325 fbbad=0/16384 coverage=full us=1885 rectscan_us=7111 slow=no -> CLEAN
[  15453ms] [wc-g] prof win=3 seq=0 surf_bytes=65536 cks_blit_us=97 civac_us=97 cks_after_us=97 probes=16384 readback_us=24967
[  15453ms] [wc-g] paygo win=3 state=waiting emit=2 lattice_n=16 deferred=2 defer_ms=15000 since_entry_ms=15453 clock=entry taken=2 budget=4 -> DEFERRED
[  15485ms] [wc-g] win=3 seq=1 own=yes scale=6x app=0xdd0a17e4be02a325 blit=0xdd0a17e4be02a325 civac=0xdd0a17e4be02a325 after=0xdd0a17e4be02a325 fbbad=0/16384 coverage=full us=2870 rectscan_us=7111 slow=no -> CLEAN
[  15485ms] [wc-g] prof win=3 seq=1 surf_bytes=65536 cks_blit_us=97 civac_us=97 cks_after_us=97 probes=16384 readback_us=27879
[  16753ms] [wc-g] win=3 seq=2 own=yes scale=6x app=0xdd0a17e4be02a325 blit=0xdd0a17e4be02a325 civac=0xdd0a17e4be02a325 after=0xdd0a17e4be02a325 fbbad=0/16384 coverage=full us=1839 rectscan_us=7111 slow=no -> CLEAN
[  16753ms] [wc-g] prof win=3 seq=2 surf_bytes=65536 cks_blit_us=97 civac_us=97 cks_after_us=97 probes=16384 readback_us=24914
[  16753ms] [wc-g] paygo win=3 state=complete emit=3 lattice_n=16 deferred=2 defer_ms=15000 since_entry_ms=16753 clock=entry taken=4 budget=4 -> PAID
[  16753ms] [wc-g] rollup win=3 scope=window paygo=yes samples=4 coher=0 race=0 blit=0 clean=4 slow=0 maxus=2870 wit_us=78695 frame_us=16667 -> CLEAN
[  17403ms] [wc-g] win=1 seq=0 own=no scale=1x app=0xcbf29ce484222325 blit=0x980bc87c8385e125 civac=0x980bc87c8385e125 after=0x980bc87c8385e125 fbbad=0/965632 coverage=full us=4908 rectscan_us=6814 slow=no -> CLEAN
[  17403ms] [wc-g] prof win=1 seq=0 surf_bytes=3862528 cks_blit_us=5786 civac_us=5738 cks_after_us=5742 probes=965632 readback_us=622315
[  17403ms] [wc-g] paygo win=1 state=waiting emit=2 lattice_n=16 deferred=264 defer_ms=15000 since_entry_ms=17403 clock=entry taken=2 budget=4 -> DEFERRED
[  17420ms] [wc-g] win=2 seq=0 own=no scale=8x app=0xcbf29ce484222325 blit=0x47b750fe2093a4da civac=0x47b750fe2093a4da after=0x47b750fe2093a4da fbbad=0/6144 coverage=full us=1251 rectscan_us=4740 slow=no -> CLEAN
[  17420ms] [wc-g] prof win=2 seq=0 surf_bytes=24576 cks_blit_us=36 civac_us=36 cks_after_us=36 probes=6144 readback_us=16286
[  21230ms] [wc-g] win=4 seq=0 own=no scale=6x app=0xcbf29ce484222325 blit=0xeb05052ea5b62325 civac=0xeb05052ea5b62325 after=0xeb05052ea5b62325 fbbad=0/1024 coverage=lattice16 us=1845 rectscan_us=7111 slow=no -> CLEAN
[  21230ms] [wc-g] prof win=4 seq=0 surf_bytes=65536 cks_blit_us=97 civac_us=97 cks_after_us=97 probes=1024 readback_us=1699
[  21258ms] [wc-g] win=4 seq=0 own=no scale=6x app=0xcbf29ce484222325 blit=0xeb05052ea5b62325 civac=0xeb05052ea5b62325 after=0xeb05052ea5b62325 fbbad=0/16384 coverage=full us=1844 rectscan_us=7111 slow=no -> CLEAN
[  21258ms] [wc-g] prof win=4 seq=0 surf_bytes=65536 cks_blit_us=97 civac_us=97 cks_after_us=97 probes=16384 readback_us=24848
[  21260ms] [wc-g] win=5 seq=0 own=no scale=8x app=0xcbf29ce484222325 blit=0x9c1bda7f8c872325 civac=0x9c1bda7f8c872325 after=0x9c1bda7f8c872325 fbbad=0/256 coverage=lattice16 us=879 rectscan_us=2370 slow=no -> CLEAN
[  21260ms] [wc-g] prof win=5 seq=0 surf_bytes=16384 cks_blit_us=24 civac_us=24 cks_after_us=24 probes=256 readback_us=405
[  21290ms] [wc-g] win=4 seq=1 own=yes scale=6x app=0xeb05052ea5b62325 blit=0xeb05052ea5b62325 civac=0xeb05052ea5b62325 after=0xeb05052ea5b62325 fbbad=0/16384 coverage=full us=1781 rectscan_us=7111 slow=no -> CLEAN
[  21290ms] [wc-g] prof win=4 seq=1 surf_bytes=65536 cks_blit_us=97 civac_us=97 cks_after_us=97 probes=16384 readback_us=27354
[  22563ms] [wc-g] win=4 seq=2 own=yes scale=6x app=0xeb05052ea5b62325 blit=0xeb05052ea5b62325 civac=0xeb05052ea5b62325 after=0xeb05052ea5b62325 fbbad=0/16384 coverage=full us=23 rectscan_us=7111 slow=no -> CLEAN
[  22563ms] [wc-g] prof win=4 seq=2 surf_bytes=65536 cks_blit_us=97 civac_us=97 cks_after_us=97 probes=16384 readback_us=24601
[  22563ms] [wc-g] paygo win=4 state=complete emit=1 lattice_n=16 deferred=0 defer_ms=15000 since_entry_ms=22563 clock=entry taken=4 budget=4 -> PAID
[  22563ms] [wc-g] rollup win=4 scope=window paygo=yes samples=4 coher=0 race=0 blit=0 clean=4 slow=0 maxus=1845 wit_us=79666 frame_us=16667 -> CLEAN
[  22571ms] [wc-g] win=5 seq=1 own=yes scale=8x app=0x9c1bda7f8c872325 blit=0x9c1bda7f8c872325 civac=0x9c1bda7f8c872325 after=0x9c1bda7f8c872325 fbbad=0/4096 coverage=full us=795 rectscan_us=2370 slow=no -> CLEAN
[  22571ms] [wc-g] prof win=5 seq=1 surf_bytes=16384 cks_blit_us=24 civac_us=24 cks_after_us=24 probes=4096 readback_us=6538
[  23705ms] [wc-g] win=1 seq=0 own=no scale=1x app=0xcbf29ce484222325 blit=0x980bc87c8385e125 civac=0x980bc87c8385e125 after=0x980bc87c8385e125 fbbad=0/965632 coverage=full us=4938 rectscan_us=6814 slow=no -> CLEAN
[  23705ms] [wc-g] prof win=1 seq=0 surf_bytes=3862528 cks_blit_us=5785 civac_us=5759 cks_after_us=5759 probes=965632 readback_us=563537
[  23715ms] [wc-g] win=5 seq=1 own=no scale=8x app=0x9c1bda7f8c872325 blit=0x9c1bda7f8c872325 civac=0x9c1bda7f8c872325 after=0x9c1bda7f8c872325 fbbad=0/4096 coverage=full us=884 rectscan_us=2370 slow=no -> CLEAN
[  23715ms] [wc-g] prof win=5 seq=1 surf_bytes=16384 cks_blit_us=24 civac_us=24 cks_after_us=24 probes=4096 readback_us=6555
"""

# --wcg fixture 6 is SYNTHETIC and exists for the case the metal has never produced: the
# DEFERRAL THAT NEVER PAID. No capture shows it, which is exactly why it needs a fixture --
# the WARN is the honesty check section 10h asks for, and an untested warning is a warning
# that fires wrong the first time it matters.
#
# win=1 is the defect: it keeps presenting to 20 000 ms, well past the 15 000 ms horizon,
# and every one of its passes is `coverage=lattice16`. Its gate is stuck (`clock=unarmed`
# on the second paygo line -- the emitter's way of saying `since_entry_ms` could not be
# read, so the deferral can never open). It must WARN and report UNPAID.
#
# win=2 is the CONTROL, and it is what makes the WARN meaningful rather than universal: same
# boot, same horizon, lattice pass 1 then a deferred full pass, `-> PAID`. A check that
# warns about every window says nothing about any of them.
#
# win=3 exercises two counters that must not be conflated with the above: a sample carrying
# NO `coverage=` marker at all (counted apart -- an unmarked pass is not a full pass, it is
# a pass that did not say), on a window that stops BEFORE the horizon and is therefore owed
# no deferred pass and must NOT warn.
#
# The deferral census is also under test here: win=1 prints `deferred=1` then `deferred=99`.
# The reading is 99 -- the value beside the greatest `emit=` -- and the column sum, 100, is
# a number with no meaning. A reader that adds them gets a plausible-looking wrong answer,
# which is the worst kind.
WCG_FIXTURE_PAYGO_WARN = """\
[      0ms] bootpace: entry
[     10ms] [NVIDIA] Initializing Kepler GPU at BDF 1:0:0
[    110ms] :: kepler: takeover complete ::
[    200ms] [wc-g] win=1 seq=0 own=no scale=1x app=0x1 blit=0x1 civac=0x1 after=0x1 fbbad=0/64 coverage=lattice16 us=10 rectscan_us=20 slow=no -> CLEAN
[    205ms] [wc-g] paygo win=1 state=waiting emit=1 lattice_n=16 deferred=1 defer_ms=15000 since_entry_ms=205 clock=entry taken=1 budget=4 -> DEFERRED
[    220ms] [wc-g] win=2 seq=0 own=no scale=8x app=0x2 blit=0x2 civac=0x2 after=0x2 fbbad=0/16 coverage=lattice16 us=8 rectscan_us=20 slow=no -> CLEAN
[    225ms] [wc-g] paygo win=2 state=waiting emit=1 lattice_n=16 deferred=1 defer_ms=15000 since_entry_ms=225 clock=entry taken=1 budget=4 -> DEFERRED
[    240ms] [wc-g] win=3 seq=0 own=no scale=8x app=0x3 blit=0x3 civac=0x3 after=0x3 fbbad=0/16 us=8 rectscan_us=20 slow=no -> CLEAN
[    300ms] :: GPACE: span=290ms anchor=enum:p1 since-entry=300ms hz=123456 build=kepler+takeover+fifo+ivb+wc+ == the pci-usb d= split ::
[  16000ms] [wc-g] win=1 seq=0 own=no scale=1x app=0x1 blit=0x1 civac=0x1 after=0x1 fbbad=0/64 coverage=lattice16 us=10 rectscan_us=20 slow=no -> CLEAN
[  16005ms] [wc-g] paygo win=1 state=waiting emit=2 lattice_n=16 deferred=99 defer_ms=15000 since_entry_ms=0 clock=unarmed taken=1 budget=4 -> DEFERRED
[  16100ms] [wc-g] win=2 seq=1 own=yes scale=8x app=0x2 blit=0x2 civac=0x2 after=0x2 fbbad=0/256 coverage=full us=40 rectscan_us=20 slow=no -> CLEAN
[  16110ms] [wc-g] paygo win=2 state=complete emit=2 lattice_n=16 deferred=1 defer_ms=15000 since_entry_ms=16110 clock=entry taken=4 budget=4 -> PAID
[  20000ms] [wc-g] win=1 seq=0 own=no scale=1x app=0x1 blit=0x1 civac=0x1 after=0x1 fbbad=0/64 coverage=lattice16 us=10 rectscan_us=20 slow=no -> CLEAN
"""


# --wcg fixture 7 is SYNTHETIC and covers the SECOND instrument to adopt the paygo shape:
# the wc-d scan-out verify (peer commit 0f1d3dfc, `video/wm.rs`). No capture carries these
# lines yet -- the s73 boots predate the commit -- so every line below was generated from
# the EXACT format strings at that commit rather than transcribed from a log:
#
#   wm.rs:2809  "[wc-d] verify win={} surf={}x{} band={} scale={}x at ({},{}) panel={}x{} \
#                checked={}{} bad_cache=0 bad_ram=0 ram_indep={} moved={} nonzero={} \
#                cksum={:#018x} first=none -> PASS"
#   wm.rs:3374  fn wcd_coverage_note(step) -> " coverage=lattice16" | " coverage=full"
#   wm.rs:3335  "[wc-d] paygo win={} state={} emit={} lattice_n={} deferred={} defer_ms={} \
#                since_entry_ms={} clock={} taken={} budget=2 -> {}"
#
# The field VALUES are boot 7's real win=1 console-window verify, so the only synthetic part
# is the paygo insertion itself. Note `budget=2` is a LITERAL in that format string: wc-d
# has two STAGES (lattice, then full) where wc-g budgets four samples, and `taken=` counts
# stages CLOSED. A reader that assumed wc-g's depth would mis-state every wc-d row.
#
# THE FIXTURE ALSO CARRIES wc-g PAYGO LINES, and that is its second job. One regex now
# serves both instruments, so the tag FILTER is load-bearing: folding wc-d's two-stage
# battery into wc-g's four-sample one would produce a table that reconciles with neither.
# win=7 is wc-g's alone and win=1/win=2 are wc-d's, so a filter that leaked would show up
# immediately as a window in the wrong census.
#
# win=1 pays: lattice verify, deferral, full verify past the horizon, `-> PAID`.
# win=2 is the defect the WARN exists for, on THIS instrument: it keeps verifying to
# 20000 ms and never gets past the lattice, so the panel read-back covers one source column
# in sixteen for the rest of the boot while every verdict it prints says PASS.
WCD_FIXTURE_PAYGO = """\
[      0ms] bootpace: entry
[     10ms] [NVIDIA] Initializing Kepler GPU at BDF 1:0:0
[    110ms] :: kepler: takeover complete ::
[   5180ms] [wc-d] verify win=1 surf=1312x736 band=0..64 scale=1x at (784,457) panel=2880x1800 checked=83968 coverage=lattice16 bad_cache=0 bad_ram=0 ram_indep=no moved=0 nonzero=8300 cksum=0x6ea90580b6e52525 first=none -> PASS
[   5186ms] [wc-d] paygo win=1 state=waiting emit=1 lattice_n=16 deferred=1 defer_ms=15000 since_entry_ms=5186 clock=entry taken=1 budget=2 -> DEFERRED
[   5200ms] [wc-d] verify win=2 surf=96x64 band=none scale=8x at (2103,1117) panel=2880x1800 checked=393216 coverage=lattice16 bad_cache=0 bad_ram=0 ram_indep=no moved=0 nonzero=91840 cksum=0x47b750fe2093a4da first=none -> PASS
[   5205ms] [wc-d] paygo win=2 state=waiting emit=1 lattice_n=16 deferred=1 defer_ms=15000 since_entry_ms=5205 clock=entry taken=1 budget=2 -> DEFERRED
[   5210ms] [wc-g] win=7 seq=0 own=no scale=1x app=0x7 blit=0x7 civac=0x7 after=0x7 fbbad=0/64 coverage=lattice16 us=10 rectscan_us=20 slow=no -> CLEAN
[   5215ms] [wc-g] paygo win=7 state=waiting emit=1 lattice_n=16 deferred=1 defer_ms=15000 since_entry_ms=5215 clock=entry taken=1 budget=4 -> DEFERRED
[   6000ms] :: GPACE: span=5990ms anchor=enum:p1 since-entry=6000ms hz=123456 build=kepler+takeover+fifo+ivb+wc+ == the pci-usb d= split ::
[  17410ms] [wc-d] verify win=1 surf=1312x736 band=0..64 scale=1x at (784,457) panel=2880x1800 checked=83968 coverage=full bad_cache=0 bad_ram=0 ram_indep=no moved=0 nonzero=8300 cksum=0x6ea90580b6e52525 first=none -> PASS
[  17410ms] [wc-d] paygo win=1 state=complete emit=2 lattice_n=16 deferred=7 defer_ms=15000 since_entry_ms=17410 clock=entry taken=2 budget=2 -> PAID
[  20000ms] [wc-d] verify win=2 surf=96x64 band=none scale=8x at (2103,1117) panel=2880x1800 checked=393216 coverage=lattice16 bad_cache=0 bad_ram=0 ram_indep=no moved=0 nonzero=91840 cksum=0x47b750fe2093a4da first=none -> PASS
[  20005ms] [wc-d] paygo win=2 state=waiting emit=2 lattice_n=16 deferred=42 defer_ms=15000 since_entry_ms=20005 clock=entry taken=1 budget=2 -> DEFERRED
"""


# --wcg fixture 8 is SYNTHETIC and is the REGRESSION GATE for the census defect described on
# WCD_VERIFY_RE. Interlock round-3 (`6f1225b9`) + the verifier fixups (`98ffcf02`) gave wc-d two
# more terminals, and both of them broke the first cut of this reader:
#
#   * `-> SKIP (teardown)` (wm.rs:2860) carries a FULL `coverage=` marker, because it is emitted
#     from the same field list as the adjudicating arms. Counted as a verdict it lands in the
#     `full` column, and `full > 0` is precisely what the honesty check reads — so an ABORT
#     could satisfy the WARN it should have raised. win=4 below is that case, exactly;
#   * `-> SKIP (degenerate row/geometry)` (wm.rs:3084) is a short line with no `coverage=` at
#     all, so it inflated `unmarked`, which is supposed to mean "a pass that did not say what
#     it covered" and not "a pass that never happened". win=5 is that case.
#
# win=4 is also the SEALED path (wm.rs:2886): once the teardown-abort budget is spent
# (`aborts=7/6`, so `retry=no`), `wcd_seal` closes the battery and the paygo wire says so with
# `state=sealed … -> UNPAID`. It is deliberately placed at since_entry_ms=9000, BEFORE the
# 15000 ms horizon, so the fixture proves the WARN fires with no timing test — a sealed window
# is a battery that never adjudicated, whenever it happened.
#
# Generated from the format strings at `98ffcf02`; the field values are Boot R's win=3 shape.
WCD_FIXTURE_ABORT = """\
[      0ms] bootpace: entry
[     10ms] [NVIDIA] Initializing Kepler GPU at BDF 1:0:0
[    110ms] :: kepler: takeover complete ::
[   3000ms] [wc-d] verify win=4 surf=128x128 band=none scale=6x at (9,21) panel=2880x1800 checked=36864 coverage=lattice16 bad_cache=0 bad_ram=0 ram_indep=no moved=0 nonzero=36864 cksum=0x1122334455667788 first=none fills=3->3 fact=0/0 desk=1->2 dact=0/0 stable=yes -> PASS
[   3005ms] [wc-d] paygo win=4 state=waiting emit=1 lattice_n=16 deferred=1 defer_ms=15000 since_entry_ms=3005 clock=entry taken=1 budget=2 -> DEFERRED
[   4000ms] :: GPACE: span=3990ms anchor=enum:p1 since-entry=4000ms hz=123456 build=kepler+takeover+fifo+ivb+wc+ == the pci-usb d= split ::
[   8990ms] [wc-d] verify win=4 surf=128x128 band=none scale=6x at (9,21) panel=2880x1800 checked=36864 coverage=full bad_cache=0 bad_ram=0 ram_indep=no moved=12 nonzero=36864 cksum=0x1122334455667788 first=(9,21) got=0x000000 want=0x1e1e1e rect=768x768+9+21 fills=15->17 fact=0/1 desk=3->4 dact=0/0 aborts=7/6 retry=no -> SKIP (teardown)
[   9000ms] [wc-d] paygo win=4 state=sealed emit=2 lattice_n=16 deferred=2 defer_ms=15000 since_entry_ms=9000 clock=entry taken=1 budget=2 -> UNPAID
[   9100ms] [wc-d] verify win=5 -> SKIP (degenerate row/geometry)
[   9200ms] [wc-d] verify win=6 -> SKIP (no memory for 128x128 source snapshot)
[   9300ms] [wc-d] verify win=6 -> SKIP (no memory for 8x8 source snapshot)
"""


def paygo_expect_abort(pg):
    """The regression gate. Every one of these was WRONG before the WCD_VERIFY_RE anchor."""
    w4 = paygo_window(pg, '4')
    w5 = paygo_window(pg, '5')
    return [
        # The teardown SKIP carries `coverage=full`. It must NOT reach the coverage columns:
        # pre-fix this read (1, 1) and the window looked fully covered.
        ('win4 lattice/full (SKIP must not count as full)',
         (w4['lattice'], w4['full']), (1, 0)),
        ('win4 adjudicated verdicts', w4['samples'], 1),
        ('win4 declined verifies', w4['skips'], 1),
        ('win4 skip reasons', w4['skip_reasons'], {'teardown': 1}),
        # The kernel's own verdict, and the vocabulary that is no longer shared with the
        # capture statement.
        ('win4 sealed by the kernel', bool(w4['sealed']), True),
        ('win4 PAID', w4['paid'], False),
        # Sealed at 9000ms, i.e. BEFORE the 15000ms horizon: the timing test alone would miss
        # it, so this asserts the WARN does not depend on it.
        ('win4 last adjudicated (ms)', w4['last_ms'], 3000),
        ('win4 starved-by-timing', w4['starved'], False),
        ('win4 WARN (sealed, regardless of timing)', w4['warn'], True),
        # The geometry SKIP: counted as a skip, and NOT as an unmarked pass (pre-fix: 1).
        ('win5 unmarked (geometry SKIP must not inflate)', w5['unmarked'], 0),
        ('win5 adjudicated verdicts', w5['samples'], 0),
        ('win5 declined verifies', w5['skips'], 1),
        ('win5 skip reasons', w5['skip_reasons'], {'degenerate row/geometry': 1}),
        ('win5 WARN', w5['warn'], False),
        # The OOM arm interpolates the surface geometry. Two skips at DIFFERENT sizes must
        # fold to ONE census key: a character-class cut keyed this "no memory for" (reads like
        # a truncation bug) and a paren cut alone would open a bucket per geometry.
        ('win6 declined verifies', paygo_window(pg, '6')['skips'], 2),
        ('win6 skip reasons (geometry folded to one key)',
         paygo_window(pg, '6')['skip_reasons'],
         {'no memory for NxN source snapshot': 2}),
        ('windows warned', [w['id'] for w in pg['wins'] if w['warn']], ['4']),
    ]


def paygo_expect_wcd(pg):
    """The wc-d census: a two-STAGE battery, one window paid and one stuck on the lattice."""
    w1 = paygo_window(pg, '1')
    w2 = paygo_window(pg, '2')
    return [
        ('deferral horizon', pg['defer_ms'], 15000),
        # The tag filter: wc-g's win=7 must NOT appear in the wc-d census.
        ('windows in the wc-d census', [w['id'] for w in pg['wins']], ['1', '2']),
        ('win1 lattice/full', (w1['lattice'], w1['full']), (1, 1)),
        # budget=2, not 4: wc-d counts STAGES closed, wc-g counts samples taken.
        ('win1 taken/budget (stages)', (w1['taken'], w1['budget']), (2, 2)),
        ('win1 PAID', w1['paid'], True),
        ('win1 deferral census', w1['deferred'], 7),
        ('win1 WARN', w1['warn'], False),
        ('win2 lattice/full', (w2['lattice'], w2['full']), (2, 0)),
        ('win2 PAID', w2['paid'], False),
        # 42, not 1+42: the census rule again, on the second instrument.
        ('win2 deferral census', w2['deferred'], 42),
        ('win2 last verify (ms)', w2['last_ms'], 20000),
        ('win2 WARN (lattice-only past the horizon)', w2['warn'], True),
        ('windows warned', [w['id'] for w in pg['wins'] if w['warn']], ['2']),
    ]


def paygo_expect_wcg_side(pg):
    """The SAME fixture read as wc-g: only win=7, at wc-g's four-sample depth. This is the
    other half of the tag-filter proof -- one regex serves both instruments, so a leak would
    put wc-d's windows here."""
    w7 = paygo_window(pg, '7')
    return [
        ('windows in the wc-g census', [w['id'] for w in pg['wins']], ['7']),
        ('win7 taken/budget (samples)', (w7['taken'], w7['budget']), (1, 4)),
        ('win7 PAID', w7['paid'], False),
    ]


def paygo_window(pg, wid):
    for w in pg['wins']:
        if w['id'] == wid:
            return w
    raise AssertionError(f'no paygo window {wid} in fixture')


def wcg_paygo_expect(st):
    """Expected values for the real boot-7 paygo window. METAL numbers, and the point of
    them is the CONTRAST with wcg_expect() above: same bench, same battery, same four
    samples per window -- and the kepler window's wc-g cost falls from 11 542 ms to 128 ms
    because pass 1 now walks a 1-in-16 lattice and the full passes are deferred out of the
    measured span. That 90x is what section 10h claims; this is it, re-derived from the
    wire by this code path."""
    g = st['groups']
    return [
        ('window span', st['span'], 2581),
        ('wc-g cost', g['wc-g']['cost'], 128),
        ('wc-d cost', g['wc-d']['cost'], 1010),
        ('wc-h cost', g['wc-h']['cost'], 14),
        ('wc-k cost', g['wc-k']['cost'], 0),
        ('wcn cost', g['wcn']['cost'], 1),
        ('bring-up cost', g['bring-up']['cost'], 1360),
        ('other cost', g['other']['cost'], 68),
        ('accounted == span', sum(e['cost'] for e in g.values()), 2581),
        ('per-pass costs', [p['cost'] for p in st['passes']], [126, 2, 0]),
        ('pass total', st['pass_cost'], 128),
        ('prof lines present', st['has_prof'], True),
        ('witness-tagged lines', st['serial_lines'], 30),
        ('witness-tagged bytes', st['serial_bytes'], 4785),
    ]


def paygo_expect_real(pg):
    """The whole-boot paygo census for the same fixture. Read against the untrimmed boot."""
    w1 = paygo_window(pg, '1')
    w3 = paygo_window(pg, '3')
    w5 = paygo_window(pg, '5')
    return [
        ('deferral horizon', pg['defer_ms'], 15000),
        ('windows seen', [w['id'] for w in pg['wins']], ['1', '2', '3', '4', '5']),
        # The console window: one lattice pass in the kepler window, two deferred full
        # passes long after it, and still spending its budget when the log ends.
        ('win1 samples', w1['samples'], 3),
        ('win1 lattice/full', (w1['lattice'], w1['full']), (1, 2)),
        ('win1 lattice_n', w1['lattice_n'], 16),
        # 264, NOT 265: the census is the value beside the greatest emit=, never the sum.
        ('win1 deferral census', w1['deferred'], 264),
        ('win1 peak emit', w1['peak_emit'], 2),
        ('win1 taken/budget', (w1['taken'], w1['budget']), (2, 4)),
        ('win1 PAID at capture end', w1['paid'], False),
        ('win1 last present (ms)', w1['last_ms'], 23705),
        ('win1 WARN', w1['warn'], False),
        ('win3 all-full battery', (w3['lattice'], w3['full']), (0, 4)),
        ('win3 PAID', w3['paid'], True),
        ('win3 taken/budget', (w3['taken'], w3['budget']), (4, 4)),
        ('win4 PAID', paygo_window(pg, '4')['paid'], True),
        # A window that presented but never emitted a paygo line of its own: reported as
        # 'no paygo line', which is not the same statement as UNPAID.
        ('win5 paygo lines', len(w5['paygo']), 0),
        ('win5 lattice/full', (w5['lattice'], w5['full']), (1, 2)),
        ('unmarked samples in boot', sum(w['unmarked'] for w in pg['wins']), 0),
        ('windows warned', [w['id'] for w in pg['wins'] if w['warn']], []),
    ]


def paygo_expect_warn(pg):
    """The synthetic never-paid fixture. The WARN must fire on win=1 and ONLY on win=1."""
    w1 = paygo_window(pg, '1')
    w2 = paygo_window(pg, '2')
    w3 = paygo_window(pg, '3')
    return [
        ('deferral horizon', pg['defer_ms'], 15000),
        ('win1 lattice/full', (w1['lattice'], w1['full']), (3, 0)),
        # 99, not 1+99: the census rule, asserted where getting it wrong is plausible.
        ('win1 deferral census', w1['deferred'], 99),
        ('win1 peak emit', w1['peak_emit'], 2),
        ('win1 PAID', w1['paid'], False),
        ('win1 last present (ms)', w1['last_ms'], 20000),
        ('win1 WARN (never paid past the horizon)', w1['warn'], True),
        ('win1 clocks seen', w1['clocks'], ['entry', 'unarmed']),
        # The control: same boot, same horizon, deferred pass arrived.
        ('win2 lattice/full', (w2['lattice'], w2['full']), (1, 1)),
        ('win2 PAID', w2['paid'], True),
        ('win2 WARN', w2['warn'], False),
        # Unmarked pass, and a window that stopped before the horizon: no warn is owed.
        ('win3 unmarked samples', w3['unmarked'], 1),
        ('win3 lattice/full', (w3['lattice'], w3['full']), (0, 0)),
        ('win3 WARN', w3['warn'], False),
        ('windows warned', [w['id'] for w in pg['wins'] if w['warn']], ['1']),
    ]


def paygo_boot_stats(text, boot=1, tag='wc-g'):
    """Whole-boot paygo census for one fixture (never the kepler window -- see the note
    on paygo_stats)."""
    rows = load_rows(text)
    _hz, chunk = segment_by_hz(rows)[boot - 1]
    return paygo_stats(chunk, tag)


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

# ---------------------------------------------------------------------------
# Fixtures and --selftest
#
# Fixture 1 is REAL: every line is quoted byte-for-byte out of Boot V
# (~/unaos-bench/scratch/bootV.log, the slice of
# ~/unaos-bench/capture/rmbp-gr16-s73/ttyUSB0.log).  Lines between the ones
# quoted were dropped; nothing was edited.  The capture path is deliberately
# NOT read at runtime — a fixture that needs a bench directory to still exist
# is a fixture that quietly stops running.
#
# Fixture 2 is the same real block plus a SECOND boot carrying four things
# metal has not produced and which therefore cannot be quoted: a retyped fb
# leaf, a SLOW-XFER on controller [1], a cap-reached overflow line, and a
# pre-GR18 SMC-BATT line with no 'late='.  Each mutation is marked below.  They
# are exactly the readings the three sections exist to catch, so leaving them
# untested until metal produces one would mean shipping three instruments whose
# alarm paths have never executed.
# ---------------------------------------------------------------------------

# The pre-GR18 SMC-BATT wire, REAL: this is the tail of the PREVIOUS boot,
# still in the bootV.log slice at line 35.  It ends '... gap=376 busy=0' with
# no 'late=' at all, which is the whole point of quoting it.
_SMC_PRE_GR18 = (
    "[ 174541ms] :: SMC-BATT: present=true soc=94% volt=12310mV amp=-2286mA "
    "full=9962mAh rem=9438mAh ac=derived:discharging retries=0/0 st0=0 "
    "rfail=0 rok=1 short=0 unc=0 gap=376 busy=0 == witness ::")

GR18_FIXTURE_BOOTV = """\
[      ?ms] :: x86 fb-wc: retyped 15 leaf(s) WC (PAT PA4) over 0x90020000..0x91c40000 ::
[      ?ms] :: WXPROBE cpu: cr0=0x0000000080000013 wp=0 cr4=0x0000000000100668 pge=0 smep=1 smap=0 la57=0 efer=0x0000000000000D01 nxe=1 lme=1 ::
[      ?ms] :: WXPROBE map: at=kimg va=0x7B238000 lvl=2M e=0x000000007B2000E3 p=1 w=1 u=0 nx=0 g=0 pat=0 pcd=0 pwt=0 fw=1 fx=1 fu=0 ::
[      ?ms] :: WXPROBE map: at=ktext va=0x7B338A90 lvl=2M e=0x000000007B2000E3 p=1 w=1 u=0 nx=0 g=0 pat=0 pcd=0 pwt=0 fw=1 fx=1 fu=0 ::
[      ?ms] :: WXPROBE map: at=ap8000 va=0x8000 lvl=4K e=0x0000000000008003 p=1 w=1 u=0 nx=0 g=0 pat=0 pcd=0 pwt=0 fw=1 fx=1 fu=0 ::
[      ?ms] :: WXPROBE map: at=fb va=0x90020000 lvl=2M e=0x00000000900010E3 p=1 w=1 u=0 nx=0 g=0 pat=1 pcd=0 pwt=0 fw=1 fx=1 fu=0 ::
[      ?ms] :: WXPROBE map: at=bss va=0x7B4AF000 lvl=2M e=0x000000007B4000E3 p=1 w=1 u=0 nx=0 g=0 pat=0 pcd=0 pwt=0 fw=1 fx=1 fu=0 ::
[      ?ms] :: WXPROBE map: at=lapic va=0xFEE00000 lvl=2M e=0x00000000FEE000E3 p=1 w=1 u=0 nx=0 g=0 pat=0 pcd=0 pwt=0 fw=1 fx=1 fu=0 ::
[      ?ms] :: WXPROBE elf: ehdr=0x7B238000 ok=1 phnum=9 load=4 s0=0x0+0x30024/R-- s1=0x31030+0xF7EAF/R-X s2=0x129EE0+0x7120/RW- s3=0x1312C0+0x580448/RW- ::
[      ?ms] :: X86_64 Memory Init ::
[    968ms] :: EHCI-HID: [0] EPACE-TRIM M8 SLOW-XFER addr=0 hub=0.0 spd=HS bmreq=0x80 breq=0x06 wval=0x0100 widx=0x0000 wlen=8 stg=3 xfer=50ms act=50ms ass=0ms seq=1/8 == witness ::
[   1745ms] :: SMC-SCOUT: key #KEY present len=4 bytes=[00 00 01 ed] (key count (metal-only; enables index enumeration)) ::
[   1749ms] :: SMC-SCOUT: #KEY count=493 — walking index list ::
[   1849ms] :: SMC-SCOUT: index walk done (493 of 493 names) ::
[   1850ms] :: SMC-BATT: AC-W is absent on this SMC (clean negative answer, not a fault) — AC presence is UNKNOWN; ac=derived:* is inferred from the B0AC sign, and the key is re-probed every 60000 ms == witness ::
[   1851ms] :: SMC-BATT: present=true soc=94% volt=12306mV amp=-2519mA full=9962mAh rem=9387mAh ac=derived:discharging retries=0/0 st0=0 rfail=0 rok=0 short=0 unc=0 gap=1447 busy=49 late=0 == witness ::
[  63953ms] :: SMC-BATT: present=true soc=93% volt=12292mV amp=-2221mA full=9962mAh rem=9339mAh ac=derived:discharging retries=0/0 st0=0 rfail=0 rok=0 short=0 unc=0 gap=1680 busy=49 late=0 == witness ::
[ 198939ms] :: SMC-BATT: present=true soc=92% volt=12250mV amp=-2294mA full=9962mAh rem=9239mAh ac=derived:discharging retries=0/0 st0=0 rfail=0 rok=0 short=0 unc=0 gap=2312 busy=49 late=0 == witness ::
"""

# Boot 2's mutations, each named where it appears:
#   * fb leaf   e=...900010E3 pat=1 -> e=...900000F3 pat=0 pcd=1.  The PAT bit
#     for a 2M leaf is bit 12, so clearing it and setting PCD is the exact bit
#     pattern of a WC leaf that came back UC — the GR15 shape.
#   * SLOW-XFER real [0] line with the controller index changed to [1].
#   * a cap-reached line (metal has never exceeded the cap).
#   * the real pre-GR18 SMC-BATT line, which has no 'late='.
GR18_FIXTURE_TWOBOOT = GR18_FIXTURE_BOOTV + """\
[      ?ms] :: x86 fb-wc: retyped 15 leaf(s) WC (PAT PA4) over 0x90020000..0x91c40000 ::
[      ?ms] :: WXPROBE cpu: cr0=0x0000000080000013 wp=0 cr4=0x0000000000100668 pge=0 smep=1 smap=0 la57=0 efer=0x0000000000000D01 nxe=1 lme=1 ::
[      ?ms] :: WXPROBE map: at=kimg va=0x7B238000 lvl=2M e=0x000000007B2000E3 p=1 w=1 u=0 nx=0 g=0 pat=0 pcd=0 pwt=0 fw=1 fx=1 fu=0 ::
[      ?ms] :: WXPROBE map: at=ktext va=0x7B338A90 lvl=2M e=0x000000007B2000E3 p=1 w=1 u=0 nx=0 g=0 pat=0 pcd=0 pwt=0 fw=1 fx=1 fu=0 ::
[      ?ms] :: WXPROBE map: at=ap8000 va=0x8000 lvl=4K e=0x0000000000008003 p=1 w=1 u=0 nx=0 g=0 pat=0 pcd=0 pwt=0 fw=1 fx=1 fu=0 ::
[      ?ms] :: WXPROBE map: at=fb va=0x90020000 lvl=2M e=0x00000000900000F3 p=1 w=1 u=0 nx=0 g=0 pat=0 pcd=1 pwt=0 fw=1 fx=1 fu=0 ::
[      ?ms] :: WXPROBE map: at=bss va=0x7B4AF000 lvl=2M e=0x000000007B4000E3 p=1 w=1 u=0 nx=0 g=0 pat=0 pcd=0 pwt=0 fw=1 fx=1 fu=0 ::
[      ?ms] :: WXPROBE map: at=lapic va=0xFEE00000 lvl=2M e=0x00000000FEE000E3 p=1 w=1 u=0 nx=0 g=0 pat=0 pcd=0 pwt=0 fw=1 fx=1 fu=0 ::
[      ?ms] :: WXPROBE elf: ehdr=0x7B238000 ok=1 phnum=9 load=4 s0=0x0+0x30024/R-- s1=0x31030+0xF7EAF/R-X s2=0x129EE0+0x7120/RW- s3=0x1312C0+0x580448/RW- ::
[      ?ms] :: X86_64 Memory Init ::
[    968ms] :: EHCI-HID: [1] EPACE-TRIM M8 SLOW-XFER addr=0 hub=0.0 spd=HS bmreq=0x80 breq=0x06 wval=0x0100 widx=0x0000 wlen=18 stg=3 xfer=50ms act=50ms ass=0ms seq=1/8 == witness ::
[   1000ms] :: EHCI-HID: [1] EPACE-TRIM M8 SLOW-XFER cap reached — 11 transfers crossed the 8 ms threshold, 8 printed, 3 suppressed == witness ::
""" + _SMC_PRE_GR18 + "\n"


def gr18_bootv_expect(result):
    """Boot V, read end to end.  The first two assertions are the LOGTS
    REGRESSION itself: every line in this fixture carries a stamp, and before
    the strip_logts() fix this file parsed a stamped capture as zero boots and
    zero witnesses while still printing a census that looked clean."""
    boot = result['boots'][0]
    wx = wxprobe_boot(boot)
    fb = wx['maps']['fb'][-1]['fields']
    fb_ok, _ = wxprobe_fb_typing(wx)
    sx = slowxfer_boot(boot)
    x0 = sx['xfers'][0]
    sm = smc_boot(boot)
    return [
        ('boots parsed from a logts-stamped capture', len(result['boots']), 1),
        ('WXPROBE reached the family table', boot['families']['WXPROBE'], 8),
        ('wxprobe lines', wx['lines'], 8),
        ('wxprobe leaves', list(wx['maps']),
         ['kimg', 'ktext', 'ap8000', 'fb', 'bss', 'lapic']),
        ('cpu smep/nxe', (wx['cpu']['fields']['smep'],
                          wx['cpu']['fields']['nxe']), ('1', '1')),
        ('elf ok/load', (wx['elf']['fields']['ok'],
                         wx['elf']['fields']['load']), ('1', '4')),
        ('fb raw entry', fb['e'], '0x00000000900010E3'),
        ('fb pat/pcd/pwt', (fb['pat'], fb['pcd'], fb['pwt']), ('1', '0', '0')),
        ('fb typing cross-check', fb_ok, True),
        ('slow transfers', len(sx['xfers']), 1),
        ('slow transfer controller', x0['idx'], 0),
        ('slow transfer request decoded', x0['request'], 'GET_DESCRIPTOR(8)'),
        ('slow transfer descriptor', x0['detail'], 'DEVICE'),
        ('slow transfer cost', x0['fields']['xfer'], '50ms'),
        ('cap lines', len(sx['caps']), 0),
        ('smc samples (the prose AC-W line is NOT one)', len(sm['samples']), 3),
        ('smc gap: greatest of 1447/1680/2312', sm['stats']['gap']['max'], 2312),
        ('smc gap delta first->max', sm['stats']['gap']['delta'], 865),
        ('smc gap is not the sum', sm['stats']['gap']['max'] == 1447 + 1680 + 2312,
         False),
        ('smc busy', sm['stats']['busy']['max'], 49),
        ('smc late present', sm['stats']['late']['present'], True),
        ('smc late', sm['stats']['late']['max'], 0),
        ('#KEY count', sm['keycount']['count'], 493),
        ('#KEY walk', (sm['keywalk']['named'], sm['keywalk']['total']),
         (493, 493)),
    ]


def gr18_twoboot_expect(result):
    """The alarm paths.  Every one of these is a reading metal has not yet
    produced, which is precisely why it is asserted here."""
    b1, b2 = result['boots']
    wx1, wx2 = wxprobe_boot(b1), wxprobe_boot(b2)
    findings, _notes = wxprobe_diff(wx1, wx2)
    fb2_ok, fb2_msgs = wxprobe_fb_typing(wx2)
    sx2 = slowxfer_boot(b2)
    sm2 = smc_boot(b2)
    cap = sx2['caps'][0]
    return [
        ('boots parsed', len(result['boots']), 2),
        ('diff findings', len(findings), 1),
        ('the finding names the fb leaf and the GR15 signature',
         findings[0].startswith('WARN WXPROBE-FB-ENTRY-CHANGED') and
         'GR15 regression signature' in findings[0], True),
        ('boot 2 fb typing refused', fb2_ok, False),
        ('the refusal decodes the PAT entry',
         'PA2 (UC-)' in fb2_msgs[0], True),
        ('slow transfer on [1] seen', [x['idx'] for x in sx2['xfers']], [1]),
        ('the [1] transfer decodes as the full device descriptor',
         sx2['xfers'][0]['request'], 'GET_DESCRIPTOR(18)'),
        ('cap line crossed', cap['crossed'], 11),
        ('cap line printed/suppressed', (cap['printed'], cap['suppressed']),
         (8, 3)),
        ('cap line threshold', cap['threshold_ms'], 8),
        ('cap line is not read as a transfer', len(sx2['xfers']), 1),
        ('boot 2 SMC-BATT has no late=', sm2['stats']['late']['present'], False),
        ('absent late does NOT read as zero', sm2['stats']['late']['max'], None),
        ('boot 2 gap still read', sm2['stats']['gap']['max'], 376),
    ]


# A properly stamped x86 boot that carries none of the three GR18 wires: the
# absence case.  Every section must REFUSE on it rather than print an empty
# table that reads like a measurement.
SELFTEST_NO_GR18_WIRE = """\
[      ?ms] :: x86 fb-wc: retyped 15 leaf(s) WC (PAT PA4) over 0x90020000..0x91c40000 ::
[      ?ms] :: X86_64 Memory Init ::
[   1744ms] :: EPACE: [0] wake=0ms(n=1) hcrst=47ms(n=2) smoke=29ms(n=1) rootrst=320ms(n=3) hseprobe=0ms(n=1) enum=285ms(n=1) [hubpwr=200ms(n=1) hubrst=12ms(n=1) hidcfg=5ms(n=1) resid=67ms] {xfer=60ms(n=28) ass=0ms act=58ms} == witness ::
[   3513ms] :: BPACE: entry t=0ms d=0ms ::
"""


# --- --wxn fixtures --------------------------------------------------------
#
# THE HAPPY PATH IS REAL, quoted byte-for-byte out of the bench capture
# (~/unaos-bench/scratch/bootW.log, the 32724cb4 build).  The alarm boots that
# follow are SYNTHESIZED, and every one of them is a reading metal has not
# produced — which is exactly why they are here.  A section whose alarm paths
# have never executed is a section whose alarm paths are a hypothesis.
#
# The epoch anchor (':: x86 fb-wc:') opens every fixture boot because the sweep
# and the census both print BEFORE ':: X86_64 Memory Init ::' — that is where
# they sit in the real capture, and a fixture that put them after the marker
# would be testing a boot shape the bench does not produce.
WXN_FIXTURE_BOOTW = """\
[      ?ms] :: x86 fb-wc: retyped 15 leaf(s) WC (PAT PA4) over 0x90020000..0x91c40000 ::
[      ?ms] :: WXN-x86: ehdr=0x7B233000 img=[0x7B233000,0x7B8E6DEA) gib_img=1 gib_tramp=0 spare_n=2 pdpt_seen=1024 nx_set=1022 huge_leaf_nx=0 skip_spare=2 skip_user=0 skip_pml4_user=0 skip_selfmap=0 already_nx=0 skip_fb_lock=0 skip_fb_base=0 skip_fb_walk=0 residue_leaves=1535 (1g=0 2m=1023 4k=512 pt=1) pge=0 flush=cr3-reload wp=0 -> SWEPT ::
[      ?ms] :: WXN-FBWC: fb=0x90020000 lvl=2 e=0x00000000900010E3 pat=1 pcd=0 pwt=0 w=1 fx=0 -> LEAF BIT-IDENTICAL ::
[      ?ms] :: WXAUDIT-0: classifier fires on W+X, clears RO-X, honours parent NX, voids on NXE=0 -> PASS ::
[      ?ms] :: WXAUDIT x86: leaves=66047 user=0 user_WX=0 kern_WX=1535 (2048 MiB) tables=1028 nxe=1 walk=1720kcyc l1=0 l2=65535 l3=512 ::
[      ?ms] :: X86_64 Memory Init ::
[    171ms] :: WXAUDIT-NXE: cores=8 nxe=8 nxe_mask=0xFF wp=0 wp_mask=0x0 -> PASS ::
[    176ms] :: WXAUDIT-SLOT: ring-3 window W^X verified (slot 0, leaves=4, wx=0, nxe=1) ::
"""

# Four alarm boots on top of the real one.  Each mutation is named where it
# appears; none of them is a shape this reader invented, they are all generated
# from the emitters' own format strings.
#
#   boot 2 — the U/S case, coherent end to end: firmware set U/S on its identity
#     PML4 entries, so every descent took `skip_pml4_user`, `nx_set` is 0 and the
#     sweep wrote nothing (-> VACUOUS).  Its `residue_leaves` is therefore the
#     whole map, the audit independently re-derives it as `kern_WX=66047` (a RISE
#     from boot 1's 1535 — coverage lost), and the fb interlock never ran because
#     the pre-sweep walk failed (-> SKIPPED).  Four alarms from one real failure.
#   boot 3 — the histogram identity broken (l3=511, one leaf counted and not
#     classified) and the walk TRUNCATED, on an otherwise healthy SWEPT boot.
#   boot 4 — the NXE refusal leg: no entry written, no FBWC line at all (the
#     sweep returns before the interlock, which this reader must NOT read as the
#     tripwire being skipped), and a per-core rollup that FAILs to match.
#   boot 5 — a pre-a0a2d163 build: a WXAUDIT census and nothing else, which is
#     the exact shape of boots 12-15 in ~/unaos-bench/capture/rmbp-gr16-s73.
#     Every missing wire must print the absence string, never a zero.
WXN_FIXTURE_ALARMS = WXN_FIXTURE_BOOTW + """\
[      ?ms] :: x86 fb-wc: retyped 15 leaf(s) WC (PAT PA4) over 0x90020000..0x91c40000 ::
[      ?ms] :: WXN-x86: ehdr=0x7B233000 img=[0x7B233000,0x7B8E6DEA) gib_img=1 gib_tramp=0 spare_n=2 pdpt_seen=0 nx_set=0 huge_leaf_nx=0 skip_spare=0 skip_user=0 skip_pml4_user=1024 skip_selfmap=0 already_nx=0 skip_fb_lock=0 skip_fb_base=0 skip_fb_walk=1 residue_leaves=66047 (1g=0 2m=65535 4k=512 pt=1) pge=0 flush=cr3-reload wp=0 -> VACUOUS ::
[      ?ms] :: WXN-FBWC: fb=0x90020000 skip_lock=0 skip_base=0 skip_walk=1 -> SKIPPED ::
[      ?ms] :: WXAUDIT-0: classifier fires on W+X, clears RO-X, honours parent NX, voids on NXE=0 -> PASS ::
[      ?ms] :: WXAUDIT x86: leaves=66047 user=0 user_WX=0 kern_WX=66047 (131072 MiB) tables=1028 nxe=1 walk=1693kcyc l1=0 l2=65535 l3=512 ::
[      ?ms] :: X86_64 Memory Init ::
[    171ms] :: WXAUDIT-NXE: cores=8 nxe=8 nxe_mask=0xFF wp=0 wp_mask=0x0 -> PASS ::
[      ?ms] :: x86 fb-wc: retyped 15 leaf(s) WC (PAT PA4) over 0x90020000..0x91c40000 ::
[      ?ms] :: WXN-x86: ehdr=0x7B233000 img=[0x7B233000,0x7B8E6DEA) gib_img=1 gib_tramp=0 spare_n=2 pdpt_seen=1024 nx_set=1022 huge_leaf_nx=0 skip_spare=2 skip_user=0 skip_pml4_user=0 skip_selfmap=0 already_nx=0 skip_fb_lock=0 skip_fb_base=0 skip_fb_walk=0 residue_leaves=1535 (1g=0 2m=1023 4k=512 pt=1) pge=0 flush=cr3-reload wp=0 -> SWEPT ::
[      ?ms] :: WXN-FBWC: fb=0x90020000 lvl=2 e=0x00000000900010E3 pat=1 pcd=0 pwt=0 w=1 fx=0 -> LEAF BIT-IDENTICAL ::
[      ?ms] :: WXAUDIT x86: leaves=66047 user=0 user_WX=0 kern_WX=1535 (2048 MiB) tables=1028 nxe=1 walk=1721kcyc l1=0 l2=65535 l3=511 TRUNCATED ::
[      ?ms] :: X86_64 Memory Init ::
[    171ms] :: WXAUDIT-NXE: cores=8 nxe=8 nxe_mask=0xFF wp=0 wp_mask=0x0 -> PASS ::
[      ?ms] :: x86 fb-wc: retyped 15 leaf(s) WC (PAT PA4) over 0x90020000..0x91c40000 ::
[      ?ms] :: WXN-x86: nxe=0 -> REFUSED (bit 63 is RESERVED with EFER.NXE clear; no entry written) ::
[      ?ms] :: WXAUDIT x86: leaves=66047 user=0 user_WX=0 kern_WX=66047 (131072 MiB) tables=1028 nxe=0 walk=1690kcyc l1=0 l2=65535 l3=512 ::
[      ?ms] :: X86_64 Memory Init ::
[    171ms] :: WXAUDIT-NXE: cores=8 nxe=0 nxe_mask=0x0 wp=0 wp_mask=0x0 -> FAIL ::
[      ?ms] :: x86 fb-wc: retyped 15 leaf(s) WC (PAT PA4) over 0x90020000..0x91c40000 ::
[      ?ms] :: WXAUDIT-0: classifier fires on W+X, clears RO-X, honours parent NX, voids on NXE=0 -> PASS ::
[      ?ms] :: WXAUDIT x86: leaves=66047 user=0 user_WX=0 kern_WX=66047 (131072 MiB) tables=1028 nxe=1 walk=1693kcyc ::
[      ?ms] :: X86_64 Memory Init ::
[    175ms] :: WXAUDIT-SLOT: ring-3 window W^X verified (slot 0, leaves=4, wx=0, nxe=1) ::
"""


# --- --wxn: Boot Y, the FIRST capture carrying WXN-M2 ----------------------
#
# Real lines, quoted byte-for-byte out of the 2026-08-07 metal capture
# (~/unaos-bench/scratch/gr19/liveness/bootY.slice.log; image built at
# `776fb13c`).  This fixture exists because the delta between the sweep and the
# audit stopped being zero on this boot — 1535 vs 305 — and the reader was
# explaining that gap as firmware read-only leaves while the real cause,
# `WXN-M2`, appeared nowhere in this file.  The arithmetic is asserted here so
# the explanation can never drift back to a cause the tool cannot see.
WXN_FIXTURE_BOOTY = """\
[      ?ms] :: x86 fb-wc: retyped 15 leaf(s) WC (PAT PA4) over 0x90020000..0x91c40000 ::
[      ?ms] :: WXN-x86: ehdr=0x7B21C000 img=[0x7B21C000,0x7B8E1F00) gib_img=1 gib_tramp=0 spare_n=2 pdpt_seen=1024 nx_set=1022 huge_leaf_nx=0 skip_spare=2 skip_user=0 skip_pml4_user=0 skip_selfmap=0 already_nx=0 skip_fb_lock=0 skip_fb_base=0 skip_fb_walk=0 residue_leaves=1535 (1g=0 2m=1023 4k=512 pt=1) pge=0 flush=cr3-reload wp=0 -> SWEPT ::
[      ?ms] :: WXN-FBWC: fb=0x90020000 lvl=2 e=0x00000000900010E3 pat=1 pcd=0 pwt=0 w=1 fx=0 -> LEAF BIT-IDENTICAL ::
[      ?ms] :: WXN-M2: xseg=[0x7B21C000,0x7B34C000) xsegs=2 xpages=304 tramp=0x8000 spare_n=2 demote_1g=0 split_2m=1 pool_used=1/16 nx_pdpt=0 nx_2m=1022 nx_pt=0 nx_4k=719 keep_x=305 already_nx=0 skip_user=0 fb=0x90020000 fb_delta=0x0 pge=0 flush=cr3-reload -> SPLIT ::
[      ?ms] :: WXAUDIT-0: classifier fires on W+X, clears RO-X, honours parent NX, voids on NXE=0 -> PASS ::
[      ?ms] :: WXAUDIT x86: leaves=66558 user=0 user_WX=0 kern_WX=305 (1 MiB) tables=1029 nxe=1 walk=1721kcyc l1=0 l2=65534 l3=1024 ::
[      ?ms] :: X86_64 Memory Init ::
[    171ms] :: WXAUDIT-NXE: cores=8 nxe=8 nxe_mask=0xFF wp=0 wp_mask=0x0 -> PASS ::
"""

# Five M2 alarm boots on top of the real one.  None of these is a shape this
# reader invented: every M2 line below is Boot Y's own, with exactly one field
# or one terminal changed, and every terminal comes from `wxn_split_stage`'s own
# format strings.
#
#   boot 2 — M2 REFUSED on the pool-sizing arm, which is the falsifier the M2
#     playbook committed to in writing.  kern_WX stays at M1's 1535, so the
#     sweep/audit delta is ZERO and the kern_WX trend reports no rise: the
#     milestone failing to happen looks EXACTLY like the milestone happening,
#     and the M2 warn is the only thing that says otherwise.
#   boot 3 — SPLIT, but nx_4k reads 700 where the map says 719.  keep_x still
#     equals kern_WX, so this boot proves the two checks are independent: the
#     arithmetic convicts while the cross-walk agrees.
#   boot 4 — SPLIT with nx_pt=2: a PD-level retirement covers a whole page
#     TABLE, and the wire does not count the leaves under it.  The closed form
#     must report itself UNAVAILABLE rather than produce a number.
#   boot 5 — SPLIT whose keep_x (290) is BELOW the audit's kern_WX (305).
#     Read-only firmware leaves can only push the audit's count down, so this
#     direction means the two walkers disagree about the map.
#   boot 6 — VACUOUS: every descent took skip_user, the pass wrote nothing, and
#     the line still reads like a report.
WXN_FIXTURE_M2_ALARMS = WXN_FIXTURE_BOOTY + """\
[      ?ms] :: x86 fb-wc: retyped 15 leaf(s) WC (PAT PA4) over 0x90020000..0x91c40000 ::
[      ?ms] :: WXN-x86: ehdr=0x7B21C000 img=[0x7B21C000,0x7B8E1F00) gib_img=1 gib_tramp=0 spare_n=2 pdpt_seen=1024 nx_set=1022 huge_leaf_nx=0 skip_spare=2 skip_user=0 skip_pml4_user=0 skip_selfmap=0 already_nx=0 skip_fb_lock=0 skip_fb_base=0 skip_fb_walk=0 residue_leaves=1535 (1g=0 2m=1023 4k=512 pt=1) pge=0 flush=cr3-reload wp=0 -> SWEPT ::
[      ?ms] :: WXN-FBWC: fb=0x90020000 lvl=2 e=0x00000000900010E3 pat=1 pcd=0 pwt=0 w=1 fx=0 -> LEAF BIT-IDENTICAL ::
[      ?ms] :: WXN-M2: xseg=[0x7B21C000,0x7B34C000) need_pd=1 need_pt=33 pool_cap=16 -> REFUSED (static pool too small; no entry written) ::
[      ?ms] :: WXAUDIT x86: leaves=66047 user=0 user_WX=0 kern_WX=1535 (2048 MiB) tables=1028 nxe=1 walk=1720kcyc l1=0 l2=65535 l3=512 ::
[      ?ms] :: X86_64 Memory Init ::
[    171ms] :: WXAUDIT-NXE: cores=8 nxe=8 nxe_mask=0xFF wp=0 wp_mask=0x0 -> PASS ::
[      ?ms] :: x86 fb-wc: retyped 15 leaf(s) WC (PAT PA4) over 0x90020000..0x91c40000 ::
[      ?ms] :: WXN-x86: ehdr=0x7B21C000 img=[0x7B21C000,0x7B8E1F00) gib_img=1 gib_tramp=0 spare_n=2 pdpt_seen=1024 nx_set=1022 huge_leaf_nx=0 skip_spare=2 skip_user=0 skip_pml4_user=0 skip_selfmap=0 already_nx=0 skip_fb_lock=0 skip_fb_base=0 skip_fb_walk=0 residue_leaves=1535 (1g=0 2m=1023 4k=512 pt=1) pge=0 flush=cr3-reload wp=0 -> SWEPT ::
[      ?ms] :: WXN-FBWC: fb=0x90020000 lvl=2 e=0x00000000900010E3 pat=1 pcd=0 pwt=0 w=1 fx=0 -> LEAF BIT-IDENTICAL ::
[      ?ms] :: WXN-M2: xseg=[0x7B21C000,0x7B34C000) xsegs=2 xpages=304 tramp=0x8000 spare_n=2 demote_1g=0 split_2m=1 pool_used=1/16 nx_pdpt=0 nx_2m=1022 nx_pt=0 nx_4k=700 keep_x=305 already_nx=0 skip_user=0 fb=0x90020000 fb_delta=0x0 pge=0 flush=cr3-reload -> SPLIT ::
[      ?ms] :: WXAUDIT x86: leaves=66558 user=0 user_WX=0 kern_WX=305 (1 MiB) tables=1029 nxe=1 walk=1721kcyc l1=0 l2=65534 l3=1024 ::
[      ?ms] :: X86_64 Memory Init ::
[    171ms] :: WXAUDIT-NXE: cores=8 nxe=8 nxe_mask=0xFF wp=0 wp_mask=0x0 -> PASS ::
[      ?ms] :: x86 fb-wc: retyped 15 leaf(s) WC (PAT PA4) over 0x90020000..0x91c40000 ::
[      ?ms] :: WXN-x86: ehdr=0x7B21C000 img=[0x7B21C000,0x7B8E1F00) gib_img=1 gib_tramp=0 spare_n=2 pdpt_seen=1024 nx_set=1022 huge_leaf_nx=0 skip_spare=2 skip_user=0 skip_pml4_user=0 skip_selfmap=0 already_nx=0 skip_fb_lock=0 skip_fb_base=0 skip_fb_walk=0 residue_leaves=1535 (1g=0 2m=1023 4k=512 pt=1) pge=0 flush=cr3-reload wp=0 -> SWEPT ::
[      ?ms] :: WXN-FBWC: fb=0x90020000 lvl=2 e=0x00000000900010E3 pat=1 pcd=0 pwt=0 w=1 fx=0 -> LEAF BIT-IDENTICAL ::
[      ?ms] :: WXN-M2: xseg=[0x7B21C000,0x7B34C000) xsegs=2 xpages=304 tramp=0x8000 spare_n=2 demote_1g=0 split_2m=1 pool_used=1/16 nx_pdpt=0 nx_2m=510 nx_pt=2 nx_4k=719 keep_x=305 already_nx=0 skip_user=0 fb=0x90020000 fb_delta=0x0 pge=0 flush=cr3-reload -> SPLIT ::
[      ?ms] :: WXAUDIT x86: leaves=66558 user=0 user_WX=0 kern_WX=305 (1 MiB) tables=1029 nxe=1 walk=1721kcyc l1=0 l2=65534 l3=1024 ::
[      ?ms] :: X86_64 Memory Init ::
[    171ms] :: WXAUDIT-NXE: cores=8 nxe=8 nxe_mask=0xFF wp=0 wp_mask=0x0 -> PASS ::
[      ?ms] :: x86 fb-wc: retyped 15 leaf(s) WC (PAT PA4) over 0x90020000..0x91c40000 ::
[      ?ms] :: WXN-x86: ehdr=0x7B21C000 img=[0x7B21C000,0x7B8E1F00) gib_img=1 gib_tramp=0 spare_n=2 pdpt_seen=1024 nx_set=1022 huge_leaf_nx=0 skip_spare=2 skip_user=0 skip_pml4_user=0 skip_selfmap=0 already_nx=0 skip_fb_lock=0 skip_fb_base=0 skip_fb_walk=0 residue_leaves=1535 (1g=0 2m=1023 4k=512 pt=1) pge=0 flush=cr3-reload wp=0 -> SWEPT ::
[      ?ms] :: WXN-FBWC: fb=0x90020000 lvl=2 e=0x00000000900010E3 pat=1 pcd=0 pwt=0 w=1 fx=0 -> LEAF BIT-IDENTICAL ::
[      ?ms] :: WXN-M2: xseg=[0x7B21C000,0x7B34C000) xsegs=2 xpages=304 tramp=0x8000 spare_n=2 demote_1g=0 split_2m=1 pool_used=1/16 nx_pdpt=0 nx_2m=1022 nx_pt=0 nx_4k=734 keep_x=290 already_nx=0 skip_user=0 fb=0x90020000 fb_delta=0x0 pge=0 flush=cr3-reload -> SPLIT ::
[      ?ms] :: WXAUDIT x86: leaves=66558 user=0 user_WX=0 kern_WX=305 (1 MiB) tables=1029 nxe=1 walk=1721kcyc l1=0 l2=65534 l3=1024 ::
[      ?ms] :: X86_64 Memory Init ::
[    171ms] :: WXAUDIT-NXE: cores=8 nxe=8 nxe_mask=0xFF wp=0 wp_mask=0x0 -> PASS ::
[      ?ms] :: x86 fb-wc: retyped 15 leaf(s) WC (PAT PA4) over 0x90020000..0x91c40000 ::
[      ?ms] :: WXN-x86: ehdr=0x7B21C000 img=[0x7B21C000,0x7B8E1F00) gib_img=1 gib_tramp=0 spare_n=2 pdpt_seen=1024 nx_set=1022 huge_leaf_nx=0 skip_spare=2 skip_user=0 skip_pml4_user=0 skip_selfmap=0 already_nx=0 skip_fb_lock=0 skip_fb_base=0 skip_fb_walk=0 residue_leaves=1535 (1g=0 2m=1023 4k=512 pt=1) pge=0 flush=cr3-reload wp=0 -> SWEPT ::
[      ?ms] :: WXN-FBWC: fb=0x90020000 lvl=2 e=0x00000000900010E3 pat=1 pcd=0 pwt=0 w=1 fx=0 -> LEAF BIT-IDENTICAL ::
[      ?ms] :: WXN-M2: xseg=[0x7B21C000,0x7B34C000) xsegs=2 xpages=304 tramp=0x8000 spare_n=2 demote_1g=0 split_2m=0 pool_used=0/16 nx_pdpt=0 nx_2m=0 nx_pt=0 nx_4k=0 keep_x=0 already_nx=0 skip_user=1024 fb=0x90020000 fb_delta=0x0 pge=0 flush=cr3-reload -> VACUOUS ::
[      ?ms] :: WXAUDIT x86: leaves=66047 user=0 user_WX=0 kern_WX=1535 (2048 MiB) tables=1028 nxe=1 walk=1720kcyc l1=0 l2=65535 l3=512 ::
[      ?ms] :: X86_64 Memory Init ::
[    171ms] :: WXAUDIT-NXE: cores=8 nxe=8 nxe_mask=0xFF wp=0 wp_mask=0x0 -> PASS ::
"""


# --- the three readings of a kern_WX RISE ----------------------------------
#
# Boot Z, the second boot below, is REAL: quoted byte-for-byte out of
# ~/unaos-bench/scratch/gr19/bootYZ-pair.log, the metal capture whose +14 the old
# reader called coverage loss.  It follows the real Boot Y, so this fixture IS
# the pair that provoked the refinement.
#
#   kern_WX 305 -> 319, and on BOTH boots kern_WX == keep_x == xpages+1
#   (305 == 305 == 304+1, 319 == 319 == 318+1).  Every W^X leaf is an executable
#   page, so no DATA page is W^X: coverage is total on both sides and the rise is
#   the 14 code pages Boot Z added.  Informational — and it must NOT set the
#   finding exit code, which is what the report-path leg below holds it to.
WXN_FIXTURE_CODEGROWTH = WXN_FIXTURE_BOOTY + """\
[      ?ms] :: x86 fb-wc: retyped 15 leaf(s) WC (PAT PA4) over 0x90020000..0x91c40000 ::
[      ?ms] :: WXN-x86: ehdr=0x7B1FB000 img=[0x7B1FB000,0x7B8D2000) gib_img=1 gib_tramp=0 spare_n=2 pdpt_seen=1024 nx_set=1022 huge_leaf_nx=0 skip_spare=2 skip_user=0 skip_pml4_user=0 skip_selfmap=0 already_nx=0 skip_fb_lock=0 skip_fb_base=0 skip_fb_walk=0 residue_leaves=1535 (1g=0 2m=1023 4k=512 pt=1) pge=0 flush=cr3-reload wp=0 -> SWEPT ::
[      ?ms] :: WXN-FBWC: fb=0x90020000 lvl=2 e=0x00000000900010E3 pat=1 pcd=0 pwt=0 w=1 fx=0 -> LEAF BIT-IDENTICAL ::
[      ?ms] :: WXN-M2: xseg=[0x7B1FB000,0x7B339000) xsegs=2 xpages=318 tramp=0x8000 spare_n=2 demote_1g=0 split_2m=2 pool_used=2/16 nx_pdpt=0 nx_2m=1021 nx_pt=0 nx_4k=1217 keep_x=319 already_nx=0 skip_user=0 fb=0x90020000 fb_delta=0x0 pge=0 flush=cr3-reload -> SPLIT ::
[      ?ms] :: WXAUDIT x86: leaves=67069 user=0 user_WX=0 kern_WX=319 (1 MiB) tables=1030 nxe=1 walk=1729kcyc l1=0 l2=65533 l3=1536 ::
[      ?ms] :: X86_64 Memory Init ::
[    171ms] :: WXAUDIT-NXE: cores=8 nxe=8 nxe_mask=0xFF wp=0 wp_mask=0x0 -> PASS ::
"""

# THE ALARM ARM, and the reason it is worth its own fixture: this boot is a
# COHERENT boot, not a corrupted line.  Boot Z's splitter retires 81 fewer 4-KiB
# leaves (`nx_4k` 1217 -> 1136), so it keeps 81 more executable — `keep_x` 319 ->
# 400 — and the audit walk independently recounts the same 400.  Every in-boot
# self-check therefore PASSES: the closed form still closes
# (1535 + 511*2 - 1021 - 1136 - 0 = 400 == kern_WX) and keep_x still equals the
# audit.  Only two readers see it: the M2/ELF derivation, which notes that
# keep_x is 81 above xpages+1, and the trend, which must convict.  That makes
# this the exact shape the WARN exists for — 81 leaves writable AND executable
# that are not kernel code — and it proves the trend arm carries the exit code
# on its own, with no other WARN in the capture to hide behind.
WXN_FIXTURE_XLOSS = WXN_FIXTURE_BOOTY + """\
[      ?ms] :: x86 fb-wc: retyped 15 leaf(s) WC (PAT PA4) over 0x90020000..0x91c40000 ::
[      ?ms] :: WXN-x86: ehdr=0x7B1FB000 img=[0x7B1FB000,0x7B8D2000) gib_img=1 gib_tramp=0 spare_n=2 pdpt_seen=1024 nx_set=1022 huge_leaf_nx=0 skip_spare=2 skip_user=0 skip_pml4_user=0 skip_selfmap=0 already_nx=0 skip_fb_lock=0 skip_fb_base=0 skip_fb_walk=0 residue_leaves=1535 (1g=0 2m=1023 4k=512 pt=1) pge=0 flush=cr3-reload wp=0 -> SWEPT ::
[      ?ms] :: WXN-FBWC: fb=0x90020000 lvl=2 e=0x00000000900010E3 pat=1 pcd=0 pwt=0 w=1 fx=0 -> LEAF BIT-IDENTICAL ::
[      ?ms] :: WXN-M2: xseg=[0x7B1FB000,0x7B339000) xsegs=2 xpages=318 tramp=0x8000 spare_n=2 demote_1g=0 split_2m=2 pool_used=2/16 nx_pdpt=0 nx_2m=1021 nx_pt=0 nx_4k=1136 keep_x=400 already_nx=0 skip_user=0 fb=0x90020000 fb_delta=0x0 pge=0 flush=cr3-reload -> SPLIT ::
[      ?ms] :: WXAUDIT x86: leaves=67069 user=0 user_WX=0 kern_WX=400 (1 MiB) tables=1030 nxe=1 walk=1729kcyc l1=0 l2=65533 l3=1536 ::
[      ?ms] :: X86_64 Memory Init ::
[    171ms] :: WXAUDIT-NXE: cores=8 nxe=8 nxe_mask=0xFF wp=0 wp_mask=0x0 -> PASS ::
"""

# THE THIRD ANSWER: the same +14 rise onto the same real Boot Z, but the EARLIER
# boot is Boot Y with its `WXN-M2` line deleted — a pre-e8b11513 build, which is
# the only thing every capture before tonight was.  The identity cannot be
# evaluated on that side, so the reader must say the discrimination did not
# happen and keep the conservative WARN.  This is the leg that stops the benign
# arm from becoming a way to explain any rise away: silence about the executable
# extent is not evidence that the extent grew.
WXN_FIXTURE_XBLIND = "".join(
    line + "\n" for line in WXN_FIXTURE_CODEGROWTH.split("\n")[:-1]
    if 'WXN-M2:' not in line or 'xpages=318' in line)


# --- the M3a era fixture ---------------------------------------------------
#
# THE CONDITION THIS EXISTS FOR (M3 review, C3).  `WXN_WP_TARGET = 0xFF` made
# the reader unable to say anything true about CR0.WP on a machine that is not
# an 8-core rMBP, and its note ("M1 does not set WP ... the documented state of
# this milestone, not a fault") is a sentence that becomes FALSE the moment M3a
# lands — printed, in the old reader, on the very capture that proves it landed.
#
# Boots 1 and 2 are REAL, quoted byte-for-byte out of the controlled six-core
# QEMU pair the M3 review built at `c6442b49`
# (~/unaos-bench/scratch/gr19/m3-review/serial-pre.log and serial-post.log,
# clean tree vs. both patches applied).  Boot 3 is the falsifier: boot 2's own
# line with the WP arm failing on one core, which is the reading that MUST be a
# FINDING and that no capture has yet produced.
#
#   boot 1 — pre-M3a, six cores, `wp=1 wp_mask=0x1` (QEMU's firmware arms the
#     BSP only) with `-> PASS`.  A benign NOTE: the old reader called this
#     "M3 pending, the target is wp_mask=0xFF", which was wrong about the target
#     (0x3F here) and right about the era by accident.
#   boot 2 — post-M3a, `wp=6 wp_mask=0x3F -> PASS`.  SATISFIED, and this is the
#     boot the old constant read as a shortfall on a machine where every core
#     it has is armed.
#   boot 3 — post-M3a with a genuinely short mask: `nxe_mask=0x3F` COMPLETE and
#     `wp=5 wp_mask=0x1F -> FAIL`.  Only M3a's widened PASS condition can fail a
#     boot whose NXE is complete, so the era is decided BY THE CAPTURE and the
#     shortfall is a FINDING — under its own name, not the NXE name the review
#     objected to.
WXN_FIXTURE_M3A_CLEAN = """\
[      ?ms] :: x86 fb-wc: retyped 2 leaf(s) WC (PAT PA4) over 0x80000000..0x803e8000 ::
[      ?ms] :: WXN-x86: ehdr=0x3D61E000 img=[0x3D61E000,0x3DCC4E58) gib_img=0 gib_tramp=0 spare_n=1 pdpt_seen=1024 nx_set=1023 huge_leaf_nx=0 skip_spare=1 skip_user=0 skip_pml4_user=0 skip_selfmap=0 already_nx=0 skip_fb_lock=0 skip_fb_base=0 skip_fb_walk=0 residue_leaves=4089 (1g=0 2m=505 4k=3584 pt=7) pge=0 flush=cr3-reload wp=1 -> SWEPT ::
[      ?ms] :: WXN-FBWC: fb=0x80000000 lvl=2 e=0x00000000800010E3 pat=1 pcd=0 pwt=0 w=1 fx=0 -> LEAF BIT-IDENTICAL ::
[      ?ms] :: WXAUDIT x86: leaves=528887 user=0 user_WX=0 kern_WX=278 (1 MiB) tables=1036 nxe=1 walk=9782kcyc l1=0 l2=524279 l3=4608 ::
[      ?ms] :: X86_64 Memory Init ::
[    171ms] :: WXAUDIT-NXE: cores=6 nxe=6 nxe_mask=0x3F wp=1 wp_mask=0x1 -> PASS ::
[      ?ms] :: x86 fb-wc: retyped 2 leaf(s) WC (PAT PA4) over 0x80000000..0x803e8000 ::
[      ?ms] :: WXN-x86: ehdr=0x3D620000 img=[0x3D620000,0x3DCC4E58) gib_img=0 gib_tramp=0 spare_n=1 pdpt_seen=1024 nx_set=1023 huge_leaf_nx=0 skip_spare=1 skip_user=0 skip_pml4_user=0 skip_selfmap=0 already_nx=0 skip_fb_lock=0 skip_fb_base=0 skip_fb_walk=0 residue_leaves=4089 (1g=0 2m=505 4k=3584 pt=7) pge=0 flush=cr3-reload wp=1 -> SWEPT ::
[      ?ms] :: WXN-FBWC: fb=0x80000000 lvl=2 e=0x00000000800010E3 pat=1 pcd=0 pwt=0 w=1 fx=0 -> LEAF BIT-IDENTICAL ::
[      ?ms] :: WXAUDIT x86: leaves=528887 user=0 user_WX=0 kern_WX=278 (1 MiB) tables=1036 nxe=1 walk=10842kcyc l1=0 l2=524279 l3=4608 ::
[      ?ms] :: X86_64 Memory Init ::
[    171ms] :: WXAUDIT-NXE: cores=6 nxe=6 nxe_mask=0x3F wp=6 wp_mask=0x3F -> PASS ::
"""

# The falsifier, on top of the two real boots: post-M3a, one core short.
WXN_FIXTURE_M3A = WXN_FIXTURE_M3A_CLEAN + """\
[      ?ms] :: x86 fb-wc: retyped 2 leaf(s) WC (PAT PA4) over 0x80000000..0x803e8000 ::
[      ?ms] :: WXN-x86: ehdr=0x3D620000 img=[0x3D620000,0x3DCC4E58) gib_img=0 gib_tramp=0 spare_n=1 pdpt_seen=1024 nx_set=1023 huge_leaf_nx=0 skip_spare=1 skip_user=0 skip_pml4_user=0 skip_selfmap=0 already_nx=0 skip_fb_lock=0 skip_fb_base=0 skip_fb_walk=0 residue_leaves=4089 (1g=0 2m=505 4k=3584 pt=7) pge=0 flush=cr3-reload wp=1 -> SWEPT ::
[      ?ms] :: WXN-FBWC: fb=0x80000000 lvl=2 e=0x00000000800010E3 pat=1 pcd=0 pwt=0 w=1 fx=0 -> LEAF BIT-IDENTICAL ::
[      ?ms] :: WXAUDIT x86: leaves=528887 user=0 user_WX=0 kern_WX=278 (1 MiB) tables=1036 nxe=1 walk=10842kcyc l1=0 l2=524279 l3=4608 ::
[      ?ms] :: X86_64 Memory Init ::
[    171ms] :: WXAUDIT-NXE: cores=6 nxe=6 nxe_mask=0x3F wp=5 wp_mask=0x1F -> FAIL ::
"""


def wxn_m3a_expect(result):
    """The three eras of CR0.WP, read off the wire and not off a constant."""
    blocks = [wxn_boot(b) for b in result['boots']]
    b1, b2, b3 = blocks
    s1, n1 = wxn_selfchecks(b1)
    s2, n2 = wxn_selfchecks(b2)
    s3, n3 = wxn_selfchecks(b3)
    v3, m3 = wxn_verdicts(b3)
    j1, j2, j3 = ("\n".join(x) for x in (n1, n2, n3))
    return [
        ('boots parsed', len(result['boots']), 3),
        # THE TARGET IS DERIVED, NOT DECLARED.
        ('a six-core boot targets 0x3F, not 0xFF',
         (_wxn_wp_target(6), _wxn_wp_target(8), _wxn_wp_target(1)),
         (0x3F, 0xFF, 0x1)),
        ('a boot that did not say how many cores it had gets no target',
         (_wxn_wp_target(None), _wxn_wp_target(0), _wxn_wp_target(99)),
         (None, None, None)),
        # boot 1 — pre-M3a, short mask, benign.
        ('boot 1 era decided from the capture', wxn_m3a_era(b1)[0],
         WXN_M3A_PRE),
        ('boot 1 era cites the verdict/mask coherence, not a version',
         'printed -> PASS with wp_mask=0x1 short of 0x3F' in
         wxn_m3a_era(b1)[1], True),
        ('boot 1 self-checks clean — an old build is not a fault', s1, True),
        ('boot 1 names the DERIVED target', 'the target is 0x3F = (1 << 6) - 1'
         in j1, True),
        ('boot 1 excuses the era it PROVED, not one it assumed',
         'PREDATES M3a' in j1 and 'not a fault' in j1, True),
        ('the excuse now carries its own expiry',
         'The SAME reading on a boot that carries M3a is a FINDING' in j1,
         True),
        # boot 2 — post-M3a, satisfied.  The regression the old constant caused.
        ('boot 2 wp_mask=0x3F reads as SATISFIED, not as a shortfall',
         'ok   wp_mask=0x3F == 0x3F = (1 << 6) - 1' in j2, True),
        ('boot 2 raises nothing', s2, True),
        ('the milestone-pending note is UNPRINTABLE on a satisfied boot',
         ('M3 pending' in j2, 'PREDATES M3a' in j2), (False, False)),
        # boot 3 — post-M3a, short mask: the falsifier.
        ('boot 3 era decided from the capture', wxn_m3a_era(b3)[0],
         WXN_M3A_POST),
        ('boot 3 era cites the widened PASS condition',
         'only M3a\'s widened PASS condition' in wxn_m3a_era(b3)[1], True),
        ('boot 3 is a FINDING, not a note', s3, False),
        ('the finding has its OWN name, not the NXE name',
         (any(m.startswith('WARN WXN-WP-SHORT') for m in n3),
          any(m.startswith('WARN WXN-NXE-SHORT') for m in n3)), (True, False)),
        ('the finding names the mask, the target and the cores',
         'wp_mask=0x1F (5 of 6 core(s)) is short of the 0x3F' in j3, True),
        ('the finding refuses the milestone excuse',
         ('CARRIES M3a' in j3, 'not a fault' in j3), (True, False)),
        ('the rollup verdict still convicts on its own',
         any(m.startswith('WARN WXN-NXE-FAIL') for m in m3), True),
        ('boot 3 verdicts NOT clean', v3, False),
    ]


def wxn_booty_expect(result):
    """Boot Y read end to end — the M2 wire, and the delta it owns."""
    w = wxn_boot(result['boots'][0])
    vclean, vmsgs = wxn_verdicts(w)
    sclean, smsgs = wxn_selfchecks(w)
    joined = "\n".join(vmsgs + smsgs)
    return [
        ('boots parsed', len(result['boots']), 1),
        ('M2 verdict', w['m2']['verdict'], 'SPLIT'),
        ('M2 keep_x', w['m2']['fields']['keep_x'], '305'),
        ('M2 pool_used survives as a ratio, not a number',
         w['m2']['fields']['pool_used'], '1/16'),
        ('M2 xseg survives the bracket',
         w['m2']['fields']['xseg'], '[0x7B21C000,0x7B34C000)'),
        ('build era names the splitter', w['era'],
         'e8b11513+ (M1 sweep + M2 splitter)'),
        ('verdicts clean', vclean, True),
        ('self-checks clean', sclean, True),
        # THE REGRESSION THIS FIXTURE EXISTS FOR.  Both halves are asserted: the
        # delta must be attributed to M2, and the firmware sentence must be gone.
        ('the delta is attributed to the SPLITTER',
         'delta=1230 — this delta is WXN-M2\'s SPLITTER' in joined, True),
        ('the closed form is printed with its terms',
         '1535 + 511*(1 split_2m + 0 demote_1g) - 1022 nx_2m - 719 nx_4k '
         '- 0 already_nx = 305 == kern_WX' in joined, True),
        ('the firmware read-only sentence is NOT used on an M2 boot',
         'firmware already left read-only' in joined, False),
        ('keep_x is cross-walked against the audit',
         'keep_x=305 == audit kern_WX=305' in joined, True),
        ('the ELF derivation is the third opinion',
         'keep_x=305 == xpages+1 = 305' in joined, True),
        ('the histogram identity still holds after the split',
         'histogram sums: 0 + 65534 + 1024 = 66558 == leaves=66558' in joined,
         True),
    ]


def wxn_m2_alarms_expect(result):
    """Every M2 warn proven fireable, on boots metal has not produced."""
    blocks = [wxn_boot(b) for b in result['boots']]
    b1, b2, b3, b4, b5, b6 = blocks
    v2, m2msgs = wxn_verdicts(b2)
    s2, n2 = wxn_selfchecks(b2)
    s3, n3 = wxn_selfchecks(b3)
    s4, n4 = wxn_selfchecks(b4)
    s5, n5 = wxn_selfchecks(b5)
    v6, m6 = wxn_verdicts(b6)
    j2, j3, j4, j5 = ("\n".join(x) for x in (n2, n3, n4, n5))
    return [
        ('boots parsed', len(result['boots']), 6),
        # boot 2 — REFUSED, and the trend that would call it healthy
        ('boot 2 M2 verdict', b2['m2']['verdict'], 'REFUSED'),
        ('boot 2 keeps the refusal reason', b2['m2']['tail'],
         '(static pool too small; no entry written)'),
        ('boot 2 verdicts NOT clean', v2, False),
        ('the refusal warn names the pool arm',
         any(m.startswith('WARN WXN-M2-REFUSED') and 'pool too small' in m
             for m in m2msgs), True),
        ('a refused M2 leaves kern_WX at M1\'s residue — delta 0, which the '
         'kern_WX trend reads as healthy',
         (b2['sweep']['residue'], _num(b2['census']['fields']['kern_WX'])),
         (1535, 1535)),
        ('a refused M2 is NOT credited with the delta',
         'is NOT its doing' in j2, True),
        # boot 3 — the arithmetic convicts while the cross-walk agrees
        ('boot 3 self-checks NOT clean', s3, False),
        ('the unreconciled warn shows the sum and the miss',
         any(m.startswith('WARN WXN-M2-UNRECONCILED') and 'off by -19' in m
             and '- 700 nx_4k' in m for m in n3), True),
        ('the two checks are independent: keep_x still agrees on boot 3',
         'keep_x=305 == audit kern_WX=305' in j3, True),
        # boot 4 — a subtree retirement makes the closed form unavailable
        ('boot 4 says the closed form cannot be evaluated',
         'closed form cannot be evaluated' in j4 and 'nx_pt=2' in j4, True),
        ('boot 4 still attributes the delta to M2 rather than to firmware',
         ("this delta is WXN-M2's splitter" in j4,
          'firmware already left read-only' in j4), (True, False)),
        ('an unavailable closed form is not a WARN on its own', s4, True),
        # boot 5 — the audit outcounts the splitter
        ('boot 5 self-checks NOT clean', s5, False),
        ('the keep_x warn says the walkers disagree',
         any(m.startswith('WARN WXN-M2-KEEPX-EXCEEDED') and
             'walkers disagree' in m for m in n5), True),
        # boot 6 — VACUOUS
        ('boot 6 M2 verdict', b6['m2']['verdict'], 'VACUOUS'),
        ('boot 6 verdicts NOT clean', v6, False),
        ('the vacuous warn names the skip that caused it',
         any(m.startswith('WARN WXN-M2-VACUOUS') and 'skip_user=1024' in m
             for m in m6), True),
        # and the boot that is fine stays fine
        ('boot 1 is still clean beside five alarm boots',
         (wxn_verdicts(b1)[0], wxn_selfchecks(b1)[0]), (True, True)),
    ]


def wxn_codegrowth_expect(result):
    """The real Boot Y -> Boot Z pair: a rise that is CODE GROWTH, not loss."""
    blocks = [wxn_boot(b) for b in result['boots']]
    rows, findings, notes = wxn_trend(blocks)
    joined = "\n".join(notes)
    buf = io.StringIO()
    with contextlib.redirect_stdout(buf):
        print_wxn_trend(rows, findings, notes)
    page = buf.getvalue()
    return [
        ('boots parsed', len(result['boots']), 2),
        # the identity, evaluated per boot
        ('boot 1 identity holds', wxn_extent(blocks[0])['holds'], True),
        ('boot 2 identity holds', wxn_extent(blocks[1])['holds'], True),
        ('boot 2 has no unaccounted leaves',
         wxn_extent(blocks[1])['unaccounted'], 0),
        # THE REFINEMENT.  A rise with the identity intact is not a finding.
        ('the rise raises NO finding', findings, []),
        ('the rise is classified as code growth',
         wxn_classify_rise(blocks[0], blocks[1], 305, 319)[0], 'codegrowth'),
        ('the note names the growth in pages AND bytes',
         'grew by 14 page(s) (~56 KiB)' in joined, True),
        ('the note states coverage is still total',
         'coverage of the kernel map is still TOTAL' in joined, True),
        ('the note names the milestone that ends it',
         'expected until WXN M3b clears W' in joined, True),
        ('the note shows the identity on BOTH boots, not just one',
         ('boot 1: kern_WX=305 == keep_x=305 == xpages+1 = 305' in joined,
          'boot 2: kern_WX=319 == keep_x=319 == xpages+1 = 319' in joined),
         (True, True)),
        ('the WARN wording is NOT used', 'WXN-KERNWX-ROSE' in joined, False),
        # xpages reaches the table as DATA, so the reader sees code growth
        # without having to trust the sentence underneath it
        ('trend xpages column', [r[6] for r in rows], [304, 318]),
        ('trend keep_x column', [r[7] for r in rows], [305, 319]),
        ('the table header names the new columns',
         'xpages' in page and 'keep_x' in page, True),
        # the footer must not claim a rise never happened
        ('the footer does not claim kern_WX never rose',
         'never rose' in page, False),
        ('the footer claims exactly what was checked',
         'no UNACCOUNTED kern_WX rise across 1 consecutive pair(s)' in page,
         True),
        # and the per-boot readers are unmoved: this is a healthy capture
        ('both boots clean', [wxn_verdicts(w)[0] and wxn_selfchecks(w)[0]
                              for w in blocks], [True, True]),
    ]


def wxn_xloss_expect(result):
    """The genuine coverage loss: 81 W^X leaves the code cannot account for."""
    blocks = [wxn_boot(b) for b in result['boots']]
    rows, findings, notes = wxn_trend(blocks)
    ext = wxn_extent(blocks[1])
    joined = "\n".join(findings)
    return [
        ('boots parsed', len(result['boots']), 2),
        ('boot 2 identity BROKEN', ext['holds'], False),
        ('boot 2 unaccounted leaves', ext['unaccounted'], 81),
        ('the rise IS a finding', len(findings), 1),
        ('classified as coverage loss',
         wxn_classify_rise(blocks[0], blocks[1], 305, 400)[0], 'coverage'),
        ('the WARN keeps its name and its old sentence',
         (joined.startswith('WARN WXN-KERNWX-ROSE'),
          'W^X coverage of the kernel map SHRANK' in joined), (True, True)),
        ('the WARN says exactly how many leaves are unaccounted for',
         '81 W^X leaf/leaves on boot 2 are NOT accounted for' in joined, True),
        ('the WARN shows the derivation it convicted on',
         'kern_WX=400 exceeds xpages+1 = 319 (keep_x=400) by 81' in joined,
         True),
        ('the WARN refuses the code-growth excuse in words',
         'coverage LOST, not code added' in joined, True),
        ('no CODE GROWTH note is emitted beside it',
         any(n.startswith(WXN_CODEGROWTH_TAG) for n in notes), False),
        # THE POINT OF THE FIXTURE.  This boot is internally consistent: the
        # closed form closes and the two walkers agree.  If the trend arm did
        # not convict, NOTHING would.
        ('the boot\'s own self-checks are clean — only the trend convicts',
         (wxn_verdicts(blocks[1])[0], wxn_selfchecks(blocks[1])[0]),
         (True, True)),
        ('the ELF derivation still notes the 81, without warning on it',
         any('keep_x=400 vs xpages+1 = 319' in m
             for m in wxn_selfchecks(blocks[1])[1]), True),
    ]


def wxn_xblind_expect(result):
    """A rise the reader CANNOT attribute: the conservative WARN stands."""
    blocks = [wxn_boot(b) for b in result['boots']]
    rows, findings, notes = wxn_trend(blocks)
    joined = "\n".join(findings)
    return [
        ('boots parsed', len(result['boots']), 2),
        ('boot 1 carries no WXN-M2 line', blocks[0]['m2'], None),
        ('boot 1 identity is UNEVALUABLE, not False',
         wxn_extent(blocks[0])['holds'], None),
        ('boot 2 identity holds on its own', wxn_extent(blocks[1])['holds'],
         True),
        ('an unevaluable identity does NOT downgrade the rise', len(findings),
         1),
        ('classified as undecidable',
         wxn_classify_rise(blocks[0], blocks[1], 305, 319)[0], 'undecidable'),
        ('the WARN keeps its name', joined.startswith('WARN WXN-KERNWX-ROSE'),
         True),
        ('the WARN says the discrimination did not happen',
         'code-growth discrimination was NOT POSSIBLE' in joined, True),
        ('it names the missing evidence and the right era',
         ('carries no WXN-M2 line at all' in joined,
          'pre-e8b11513 build' in joined), (True, True)),
        ('it refuses to downgrade on absence',
         'never downgraded on missing evidence' in joined, True),
        ('no CODE GROWTH note is emitted',
         any(n.startswith(WXN_CODEGROWTH_TAG) for n in notes), False),
        ('the trend table renders the missing extent as absence, not zero',
         [r[6] for r in rows], [None, 318]),
    ]


def wxn_bootw_expect(result):
    """Boot W, the real 32724cb4 lines, read end to end.

    The CR0.WP line is asserted as a NOTE and not as a failure on purpose, and
    the boot must EARN that: this eight-core bench's firmware leaves WP clear on
    every core and the image predates M3a, which the capture itself proves
    (`-> PASS` with `wp_mask=0x0` short of the 0xFF its own `cores=8` implies —
    a kernel whose PASS required `wp == cores` could not have printed it).  A
    reader that WARNed here would be red on every healthy boot of that era; a
    reader that printed this note on a POST-M3a boot would be excusing a
    fault, which is what `wxn_m3a_expect` holds it to."""
    w = wxn_boot(result['boots'][0])
    vclean, vmsgs = wxn_verdicts(w)
    sclean, smsgs = wxn_selfchecks(w)
    joined = "\n".join(vmsgs + smsgs)
    return [
        ('boots parsed', len(result['boots']), 1),
        ('sweep verdict', w['sweep']['verdict'], 'SWEPT'),
        ('sweep pdpt_seen/nx_set', (w['sweep']['fields']['pdpt_seen'],
                                    w['sweep']['fields']['nx_set']),
         ('1024', '1022')),
        ('sweep img bounds survive the bracket',
         w['sweep']['fields']['img'], '[0x7B233000,0x7B8E6DEA)'),
        ('residue_leaves', w['sweep']['residue'], 1535),
        ('residue breakdown reads the digit-leading keys',
         list(w['sweep']['breakdown'].items()),
         [('1g', '0'), ('2m', '1023'), ('4k', '512'), ('pt', '1')]),
        ('residue not CAPPED', w['sweep']['capped'], False),
        ('census kern_WX', w['census']['fields']['kern_WX'], '1535'),
        ('census MiB', w['census']['mib'], 2048),
        ('census walk kcyc', _num(w['census']['fields']['walk']), 1720),
        ('census histogram present', w['census']['hist'], True),
        ('census not truncated', w['census']['truncated'], False),
        ('nxe verdict', w['nxe']['verdict'], 'PASS'),
        ('nxe cores/armed', (w['nxe']['fields']['cores'],
                             w['nxe']['fields']['nxe']), ('8', '8')),
        ('fbwc verdict', w['fbwc']['verdict'], 'LEAF BIT-IDENTICAL'),
        ('fbwc raw entry', w['fbwc']['fields']['e'], '0x00000000900010E3'),
        ('build era read off the wires', w['era'],
         '32724cb4+ (verdict, histogram, FBWC)'),
        ('verdicts clean', vclean, True),
        ('self-checks clean', sclean, True),
        ('histogram identity asserted, not assumed',
         'histogram sums: 0 + 65535 + 512 = 66047 == leaves=66047' in joined,
         True),
        ('residue/kern_WX delta reported',
         'residue_leaves=1535, audit kern_WX=1535, delta=0' in joined, True),
        ('fb typing decoded through the shared WC table',
         'pat=1 pcd=0 pwt=0 -> PA4 (write-combining)' in joined, True),
        ('the 8-core target is DERIVED from this boot, not declared',
         'the target is 0xFF = (1 << 8) - 1' in joined, True),
        ('the excuse is conditioned on the era this boot PROVES',
         ('PREDATES M3a' in joined,
          'printed -> PASS with wp_mask=0x0 short of 0xFF' in joined),
         (True, True)),
        ('one boot means the trend is REFUSED, not passed',
         wxn_trend([w])[1:], ([], [])),
    ]


def wxn_alarms_expect(result):
    """The four alarm boots.  Every WARN this section can raise is proven
    fireable here, and the absence renderings are proven not to be zeros."""
    blocks = [wxn_boot(b) for b in result['boots']]
    b1, b2, b3, b4, b5 = blocks
    v2, m2 = wxn_verdicts(b2)
    s3, n3 = wxn_selfchecks(b3)
    v3, w3 = wxn_verdicts(b3)
    v4, m4 = wxn_verdicts(b4)
    s5, n5 = wxn_selfchecks(b5)
    rows, findings, notes = wxn_trend(blocks)
    fb_findings, fb_notes = wxn_fbwc_diff(b1, b2)
    joined2 = "\n".join(m2)
    return [
        ('boots parsed', len(result['boots']), 5),
        # boot 2 — VACUOUS, FBWC SKIPPED
        ('boot 2 verdict', b2['sweep']['verdict'], 'VACUOUS'),
        ('boot 2 verdicts NOT clean', v2, False),
        ('the VACUOUS warn names the U/S cause',
         'WARN WXN-VACUOUS' in joined2 and 'skip_pml4_user=1024' in joined2,
         True),
        ('boot 2 fbwc verdict', b2['fbwc']['verdict'], 'SKIPPED'),
        ('the SKIPPED warn says the tripwire did not run',
         'WARN WXN-FBWC-SKIPPED' in joined2 and 'DID NOT RUN' in joined2, True),
        # boot 3 — histogram mismatch + TRUNCATED
        ('boot 3 self-checks NOT clean', s3, False),
        ('the histogram warn states the identity and the miss',
         any(m.startswith('WARN WXN-HIST-MISMATCH') and 'off by -1' in m
             for m in n3), True),
        ('boot 3 census truncated', b3['census']['truncated'], True),
        ('the truncation warn calls the numbers underestimates',
         any(m.startswith('WARN WXAUDIT-TRUNCATED') and 'UNDERESTIMATE' in m
             for m in w3), True),
        ('boot 3 verdicts NOT clean', v3, False),
        # boot 4 — REFUSED, no FBWC, NXE FAIL
        ('boot 4 verdict', b4['sweep']['verdict'], 'REFUSED'),
        ('boot 4 keeps the refusal reason',
         b4['sweep']['tail'],
         '(bit 63 is RESERVED with EFER.NXE clear; no entry written)'),
        ('a REFUSED sweep has no fbwc line', b4['fbwc'], None),
        ('boot 4 nxe verdict', b4['nxe']['verdict'], 'FAIL'),
        ('the refusal and the NXE failure both warn',
         (any(m.startswith('WARN WXN-REFUSED') for m in m4),
          any(m.startswith('WARN WXN-NXE-FAIL') for m in m4)), (True, True)),
        # boot 5 — pre-a0a2d163: absence, never zero
        ('boot 5 era', b5['era'], 'pre-a0a2d163 (WXAUDIT census only)'),
        ('boot 5 has no sweep', b5['sweep'], None),
        ('boot 5 has no nxe wire', b5['nxe'], None),
        ('boot 5 has no fbwc wire', b5['fbwc'], None),
        ('boot 5 histogram absent, not zero', b5['census']['hist'], False),
        ('boot 5 says wire absent rather than reporting a value',
         all(WXN_ABSENT in m for m in n5
             if m.startswith('note histogram') or
             m.startswith('note residue/kern_WX') or
             m.startswith('note per-core NXE')), True),
        ('boot 5 raises no WARN of its own — an old build is not a fault',
         s5, True),
        # the trend
        ('trend rows', len(rows), 5),
        ('trend kern_WX column', [r[1] for r in rows],
         [1535, 66047, 1535, 66047, 66047]),
        ('trend nx_set column keeps absence as None', [r[3] for r in rows],
         [1022, 0, 1022, None, None]),
        ('kern_WX rises caught', len(findings), 2),
        ('the rise warn names coverage lost',
         findings[0].startswith('WARN WXN-KERNWX-ROSE') and
         'kern_WX 1535 -> 66047 across boots 1 -> 2' in findings[0], True),
        ('the fall is a milestone note, not a warn',
         any(n.startswith('MILESTONE: kern_WX 66047 -> 1535') for n in notes),
         True),
        # the FBWC diff
        # The regression this fixture caught on its first run: a SKIPPED line
        # carries no `e=`, and the diff reported 'leaf retyped ... -> None' —
        # the GR15 signature, raised on a boot where the leaf was never read.
        ('a SKIPPED leaf is UNTESTED, not a retyped leaf',
         (fb_findings, len(fb_notes),
          'UNTESTED, not clean' in fb_notes[0]), ([], 1, True)),
        ('boot 1 -> boot 3 fbwc entries are identical',
         wxn_fbwc_diff(b1, b3), ([], [])),
        ('comparability is per pair, not per boot',
         [wxn_fbwc_comparable(p, c) for p, c in zip(blocks, blocks[1:])],
         [False, False, False, False]),
    ]


def selftest_wxn():
    """Drive --wxn through parse_content and through its own report path."""
    ok = True
    for name, text, checker in (
        ('WXN: Boot W, real 32724cb4 lines', WXN_FIXTURE_BOOTW,
         wxn_bootw_expect),
        ('WXN: four alarm boots — VACUOUS, histogram mismatch + TRUNCATED, '
         'REFUSED + NXE FAIL, pre-a0a2d163 absence',
         WXN_FIXTURE_ALARMS, wxn_alarms_expect),
        ('WXN: Boot Y, the first capture carrying WXN-M2 (the delta the '
         'reader used to blame on firmware)', WXN_FIXTURE_BOOTY,
         wxn_booty_expect),
        ('WXN: five M2 alarm boots — REFUSED, arithmetic unreconciled, a '
         'subtree retirement, keep_x exceeded, VACUOUS',
         WXN_FIXTURE_M2_ALARMS, wxn_m2_alarms_expect),
        ('WXN: the three CR0.WP eras — a real pre-M3a six-core boot, the real '
         'post-M3a one, and a post-M3a boot one core short',
         WXN_FIXTURE_M3A, wxn_m3a_expect),
        # One fixture per ARM of the kern_WX-rise classifier.  An arm with no
        # fixture is an arm nobody re-tests, and the benign arm is the one that
        # could quietly learn to explain away a real regression.
        ('WXN: the real Boot Y -> Boot Z rise — code growth with the identity '
         'kern_WX == keep_x == xpages+1 intact on both boots',
         WXN_FIXTURE_CODEGROWTH, wxn_codegrowth_expect),
        ('WXN: a rise with 81 W^X leaves the executable extent cannot account '
         'for — genuine coverage loss, and the only WARN in the capture',
         WXN_FIXTURE_XLOSS, wxn_xloss_expect),
        ('WXN: the same rise onto a boot with no WXN-M2 line — the '
         'discrimination is impossible and the conservative WARN stands',
         WXN_FIXTURE_XBLIND, wxn_xblind_expect),
    ):
        print(f"=== selftest: {name} ===")
        result = parse_content(f'<{name}>', text)
        case_ok = True
        if not result['boots']:
            print("    BAD fixture parsed 0 boots")
            case_ok = False
        else:
            for label, actual, want in checker(result):
                good = actual == want
                if not good:
                    case_ok = False
                print(f"    {'ok ' if good else 'BAD'} {label}: "
                      f"got {actual!r}, want {want!r}")
        if not case_ok:
            ok = False
        print(f"=== selftest: {name}: {'PASS' if case_ok else 'FAIL'}\n")

    print("=== selftest: WXN: report path and exit codes ===")
    wired = []
    for label, fixture, want in (
        ('--wxn on Boot W exits OK', WXN_FIXTURE_BOOTW, EXIT_OK),
        ('--wxn on the alarm capture exits FINDING', WXN_FIXTURE_ALARMS,
         EXIT_FINDING),
        ('--wxn on a capture with no WXN/WXAUDIT lines exits NO_DATA',
         SELFTEST_NO_GR18_WIRE, EXIT_NO_DATA),
        ('--wxn on Boot Y (WXN-M2 present, everything reconciles) exits OK',
         WXN_FIXTURE_BOOTY, EXIT_OK),
        ('--wxn on the M2 alarm capture exits FINDING', WXN_FIXTURE_M2_ALARMS,
         EXIT_FINDING),
        # THE FALSE-ALARM LEG, and the reason C3 was raised: a real six-core
        # QEMU pair — one boot before M3a and one after, both healthy — must
        # exit OK.  Under `WXN_WP_TARGET = 0xFF` the second of them read as a
        # shortfall on a machine where every core it has is armed.
        ('--wxn on the real pre-M3a + post-M3a six-core pair exits OK',
         WXN_FIXTURE_M3A_CLEAN, EXIT_OK),
        ('--wxn on a post-M3a boot one core short exits FINDING',
         WXN_FIXTURE_M3A, EXIT_FINDING),
        # THE EXIT-CODE CLAIM the refinement rests on: a code-growth rise must
        # not set the finding code, and neither of the other two arms may lose
        # it.  Asserted through the real report path, not through wxn_trend.
        ('--wxn on the real Boot Y -> Boot Z code-growth rise exits OK',
         WXN_FIXTURE_CODEGROWTH, EXIT_OK),
        ('--wxn on a rise with unaccounted W^X leaves exits FINDING',
         WXN_FIXTURE_XLOSS, EXIT_FINDING),
        ('--wxn on a rise that cannot be discriminated exits FINDING',
         WXN_FIXTURE_XBLIND, EXIT_FINDING),
    ):
        result = parse_content(f'<{label}>', fixture)
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            got = wxn_mode(result)
        out = buf.getvalue()
        wired.append((label, (got, bool(out.strip())), (want, True)))
    # The absence rendering is a claim about what reaches the PAGE, so it is
    # asserted against the report's own output rather than against a helper.
    buf = io.StringIO()
    with contextlib.redirect_stdout(buf):
        wxn_mode(parse_content('<alarms>', WXN_FIXTURE_ALARMS))
    page = buf.getvalue()
    wired.append(("the page says 'wire absent' for the pre-a0a2d163 boot",
                  WXN_ABSENT in page, True))
    wired.append(("the page never prints a fabricated nx_set=0 for an absent "
                  "sweep", page.count('sweep : ' + WXN_ABSENT), 1))
    wired.append(("the trend table reaches the page",
                  'cross-boot TREND' in page and 'WXN-KERNWX-ROSE' in page,
                  True))
    # No pair in the alarm capture has a readable leaf on both sides, so the
    # DIFF must say it tested nothing rather than count the pairs it walked.
    wired.append(("a run with no comparable pair refuses instead of "
                  "reporting 'nothing changed'",
                  ('NO PAIR COMPARED' in page,
                   'pair(s) comparable' in page), (True, False)))
    wired_ok = True
    for label, actual, want in wired:
        good = actual == want
        if not good:
            wired_ok = False
            ok = False
        print(f"    {'ok ' if good else 'BAD'} {label}: got {actual!r}, "
              f"want {want!r}")
    print(f"=== selftest: WXN: report path: "
          f"{'PASS' if wired_ok else 'FAIL'}\n")
    return ok


# --- --gaps: the two-splitter cross-check ----------------------------------
#
# Two boots, each carrying BOTH splitters' evidence: an `hz=` token (what the
# timing modes cut on) and a ':: X86_64 Memory Init ::' marker (what the census
# half cuts on).  The stamp resets between them, which is the reset the hz cut is
# refined to.
GAPS_FIXTURE_TWO_BOOTS = """\
[      1ms] :: X86_64 Memory Init ::
[    100ms] Initializing Kepler
[    200ms] :: GPACE: span=100ms anchor=enum:p1 since-entry=200ms hz=111 == the pci-usb d= split ::
[    300ms] :: BPACE: total gui=300ms ftdi=none n=3 dropped=0 hz=111 result=LEDGER ::
[      1ms] :: X86_64 Memory Init ::
[    100ms] Initializing Kepler
[    200ms] :: GPACE: span=100ms anchor=enum:p1 since-entry=200ms hz=222 == the pci-usb d= split ::
[    300ms] :: BPACE: total gui=300ms ftdi=none n=3 dropped=0 hz=222 result=LEDGER ::
"""

# The same capture with every `hz=` token destroyed.  The timing modes then see
# ONE boot with an impossible gap in the middle of it and a 'boot 1 (hz unknown)'
# label; the marker table still sees two.  Before GR19 this printed at exit 0.
GAPS_FIXTURE_HZ_LOST = GAPS_FIXTURE_TWO_BOOTS.replace('hz=', 'hz#')


def selftest_timing(top):
    """Fixtures for both timing modes.

    --gaps: a mixed capture where '?ms' lines must be counted but must not become
    gap endpoints, and an all-'?ms' capture that must fail.

    --wcg: the real GR16/s73 kepler window (values asserted against metal), a
    synthetic window carrying the not-yet-shipped '[wc-g] prof' lines plus a
    deferred witness line and a '?ms' line, and two captures that must be
    REFUSED -- one with no logts prefixes, one with no kepler window.

    --wcg paygo: the real GR17 boot-7 paygo wire (kepler window AND the deferred
    passes that land after it), and a synthetic boot in which the deferral never
    pays -- the one case metal has not produced, and the only one that exercises
    the coverage WARN."""
    ok = True

    # EXIT CODES, not booleans: GR19 gave both timing modes the EXIT_FINDING
    # channel the four section modes already had, so every case here names the
    # code it expects.  EXIT_USAGE is the refusal these two always had.
    for name, text, want in (
        ('gaps: mixed (numeric + ?ms + deferred)', SELFTEST_MIXED, EXIT_OK),
        ('gaps: all-?ms (counter never calibrated)', SELFTEST_ALL_UNKNOWN,
         EXIT_USAGE),
        ('gaps: two boots whose hz tokens were destroyed — the splitters '
         'disagree and it must NOT exit clean', GAPS_FIXTURE_HZ_LOST,
         EXIT_FINDING),
        ('gaps: the same two boots intact — the splitters agree',
         GAPS_FIXTURE_TWO_BOOTS, EXIT_OK),
    ):
        print(f"=== selftest: {name} ===")
        got = gaps_report(f'<{name}>', text, top)
        verdict = 'PASS' if got == want else 'FAIL'
        if got != want:
            ok = False
        print(f"=== selftest: {name}: {verdict} "
              f"(expected exit {want}, got exit {got})\n")

    for name, text, want_rc, checker in (
        ('wcg: real s73 kepler window (witness-armed, no prof lines)',
         WCG_FIXTURE_S73, EXIT_OK, wcg_expect),
        ('wcg: synthetic window WITH [wc-g] prof lines',
         WCG_FIXTURE_PROF, EXIT_OK, wcg_prof_expect),
        ('wcg: real GR17 boot-7 paygo wire (lattice pass 1, deferred full passes)',
         WCG_FIXTURE_PAYGO, EXIT_OK, wcg_paygo_expect),
        ('wcg: a deferral that NEVER PAID — the coverage warn must reach the '
         'EXIT CODE and not only the page',
         WCG_FIXTURE_PAYGO_WARN, EXIT_FINDING, None),
        ('wcg: no logts prefixes (must refuse)', WCG_FIXTURE_NO_LOGTS,
         EXIT_USAGE, None),
        ('wcg: no kepler window (must refuse)', WCG_FIXTURE_NO_WINDOW,
         EXIT_USAGE, None),
    ):
        print(f"=== selftest: {name} ===")
        got = wcg_report(f'<{name}>', text, None)
        case_ok = got == want_rc
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
              f"(expected exit {want_rc}, got exit {got})\n")

    # The paygo census is WHOLE-BOOT scoped, so it is asserted through its own entry point
    # rather than through the kepler-window helper above. Doing it any other way is the very
    # mistake the scope note on paygo_stats warns about, and a self-test that made it would
    # certify the bug.
    for name, text, checker, tag in (
        ('paygo/wc-g: real GR17 boot-7 census (deferred passes land AFTER the kepler window)',
         WCG_FIXTURE_PAYGO, paygo_expect_real, 'wc-g'),
        ('paygo/wc-g: synthetic deferral that NEVER PAID (the coverage WARN)',
         WCG_FIXTURE_PAYGO_WARN, paygo_expect_warn, 'wc-g'),
        ('paygo/wc-d: the scan-out verify adopts the shape (two stages, budget=2)',
         WCD_FIXTURE_PAYGO, paygo_expect_wcd, 'wc-d'),
        ('paygo/wc-d fixture read as wc-g: the tag filter does not leak',
         WCD_FIXTURE_PAYGO, paygo_expect_wcg_side, 'wc-g'),
        ('paygo/wc-d: teardown abort + seal — a declined verify is not a covered pass',
         WCD_FIXTURE_ABORT, paygo_expect_abort, 'wc-d'),
    ):
        print(f"=== selftest: {name} ===")
        pg = paygo_boot_stats(text, tag=tag)
        case_ok = pg is not None
        if pg is None:
            print(f"    BAD no [{tag}] paygo lines found in fixture")
        else:
            print_paygo_stats(pg, tag)
            for label, actual, want in checker(pg):
                good = actual == want
                if not good:
                    case_ok = False
                print(f"    {'ok ' if good else 'BAD'} {label}: "
                      f"got {actual!r}, want {want!r}")
        if not case_ok:
            ok = False
        print(f"=== selftest: {name}: {'PASS' if case_ok else 'FAIL'}\n")

    # A boot with no paygo line for an instrument must report the ABSENCE, not a table of
    # zeros that reads like a measurement. Three real cases, and they are three different
    # reasons for the same silence:
    #   * the s73 fixture is a witness-armed boot with no knob at all — neither instrument;
    #   * the GR17 boot-7 fixture HAS wc-g paygo lines and no wc-d ones, because it predates
    #     peer commit 0f1d3dfc. That asymmetry is the case a shared reader gets wrong, so it
    #     is asserted rather than assumed.
    # THE REPORT PATH ITSELF, and this case exists because its absence let a real bug ship.
    # Every paygo assertion above calls `paygo_stats`/`print_paygo_stats` DIRECTLY, which means
    # none of them touches `wcg_report` -- the function that actually decides which censuses a
    # `--wcg` run prints. A wiring edit left the main branch (boot WITH a kepler window) still
    # printing only the wc-g census while the fixtures stayed green, and it was caught by
    # running the tool on Boot R, not by the self-test. So the self-test now drives the whole
    # report and asserts BOTH instruments reach the page.
    print("=== selftest: paygo: --wcg prints a census for EVERY instrument ===")
    _buf = io.StringIO()
    with contextlib.redirect_stdout(_buf):
        wcg_report('<report-path fixture>', WCD_FIXTURE_ABORT, None)
    _out = _buf.getvalue()
    # ASSERTED PER TAG, not "either shape appeared". An `A or B` test passes when a boot WITH
    # data is routed down the absence path -- which is a regression that silently deletes a
    # census, i.e. exactly the class of bug this case was added to catch. The fixture's content
    # is known, so the expectation is known: WCD_FIXTURE_ABORT carries wc-d paygo lines and no
    # wc-g ones, so wc-d MUST print a battery table and wc-g MUST print the absence notice --
    # and each must NOT print the other's.
    wired = [
        ("--wcg prints the [wc-d] battery table (fixture has wc-d data)",
         "paygo battery [wc-d]" in _out),
        ("--wcg does NOT route [wc-d] to the absence path",
         "paygo [wc-d]: no" not in _out),
        ("--wcg prints the [wc-g] absence notice (fixture has no wc-g data)",
         "paygo [wc-g]: no" in _out),
        ("--wcg does NOT invent a [wc-g] battery table",
         "paygo battery [wc-g]" not in _out),
    ]
    wired_ok = all(good for _, good in wired)
    if not wired_ok:
        ok = False
    for label, good in wired:
        print(f"    {'ok ' if good else 'BAD'} {label}: got {good!r}, want True")
    print(f"=== selftest: paygo: report wiring: {'PASS' if wired_ok else 'FAIL'}\n")

    # The sealed window in that same fixture must reach the EXIT CODE, not only
    # the page.  Asserted through the report function a `--wcg` run actually
    # calls, for the reason the wiring case above exists.
    print("=== selftest: paygo: a sealed window reaches the EXIT CODE ===")
    _buf = io.StringIO()
    with contextlib.redirect_stdout(_buf):
        _rc = wcg_report('<sealed exit-code fixture>', WCD_FIXTURE_ABORT, None)
    sealed = [
        ("--wcg exits FINDING on a capture whose window was SEALED",
         _rc, EXIT_FINDING),
        ("the warn is on the page as well as in the code",
         'coverage was never bought' in _buf.getvalue(), True),
    ]
    sealed_ok = all(a == b for _, a, b in sealed)
    if not sealed_ok:
        ok = False
    for label, actual, want in sealed:
        print(f"    {'ok ' if actual == want else 'BAD'} {label}: "
              f"got {actual!r}, want {want!r}")
    print(f"=== selftest: paygo: sealed exit code: "
          f"{'PASS' if sealed_ok else 'FAIL'}\n")

    print("=== selftest: paygo: instruments that emitted nothing must report ABSENCE ===")
    absent = [
        ('s73 (witness-armed, no paygo knob) has no wc-g census',
         paygo_boot_stats(WCG_FIXTURE_S73, tag='wc-g')),
        ('s73 has no wc-d census either',
         paygo_boot_stats(WCG_FIXTURE_S73, tag='wc-d')),
        ('GR17 boot 7 has no wc-d census (it predates 0f1d3dfc)',
         paygo_boot_stats(WCG_FIXTURE_PAYGO, tag='wc-d')),
    ]
    absent_ok = all(pg is None for _, pg in absent)
    if not absent_ok:
        ok = False
    for label, pg in absent:
        print(f"    {'ok ' if pg is None else 'BAD'} {label}: got {pg is None!r}, want True")
    print(f"=== selftest: paygo: absence reporting: {'PASS' if absent_ok else 'FAIL'}\n")

    return ok


def selftest_gr18():
    """Drive the three GR18 sections through their real entry points.

    Each case parses fixture TEXT with parse_content — the same function a
    capture file goes through — then asserts against the section helpers AND
    runs the section's own report, so a wiring edit that leaves the helpers
    right and the report empty cannot pass."""
    ok = True
    for name, text, checker in (
        ('GR18: Boot V, real lines (wxprobe + slowxfer + smc)',
         GR18_FIXTURE_BOOTV, gr18_bootv_expect),
        ('GR18: two boots — retyped fb leaf, SLOW-XFER on [1], cap overflow, '
         'pre-GR18 SMC wire',
         GR18_FIXTURE_TWOBOOT, gr18_twoboot_expect),
    ):
        print(f"=== selftest: {name} ===")
        result = parse_content(f'<{name}>', text)
        case_ok = True
        if not result['boots']:
            print("    BAD fixture parsed 0 boots")
            case_ok = False
        else:
            for label, actual, want in checker(result):
                good = actual == want
                if not good:
                    case_ok = False
                print(f"    {'ok ' if good else 'BAD'} {label}: "
                      f"got {actual!r}, want {want!r}")
        if not case_ok:
            ok = False
        print(f"=== selftest: {name}: {'PASS' if case_ok else 'FAIL'}\n")

    # THE REPORT PATH ITSELF.  Every assertion above calls the section helpers
    # directly, so none of them touches the functions that decide what a
    # --wxprobe / --slowxfer / --smc run actually prints and what it exits
    # with.  A wiring edit could leave all of the above green while a section
    # printed nothing.  These cases drive the report entry points and assert
    # the exit code, which is the thing a caller reads.
    print("=== selftest: GR18: section report paths and exit codes ===")
    wired = []
    for label, fixture, mode, want in (
        ('--wxprobe on Boot V exits OK', GR18_FIXTURE_BOOTV, wxprobe_mode,
         EXIT_OK),
        ('--wxprobe on the retyped-leaf capture exits FINDING',
         GR18_FIXTURE_TWOBOOT, wxprobe_mode, EXIT_FINDING),
        ('--slowxfer on Boot V exits OK', GR18_FIXTURE_BOOTV, slowxfer_mode,
         EXIT_OK),
        ('--slowxfer on the [1] capture exits FINDING', GR18_FIXTURE_TWOBOOT,
         slowxfer_mode, EXIT_FINDING),
        ('--smc on Boot V exits OK', GR18_FIXTURE_BOOTV, smc_mode, EXIT_OK),
        ('--wxprobe on a capture with no WXPROBE exits NO_DATA',
         SELFTEST_NO_GR18_WIRE, wxprobe_mode, EXIT_NO_DATA),
        ('--slowxfer on a capture with no SLOW-XFER exits NO_DATA',
         SELFTEST_NO_GR18_WIRE, slowxfer_mode, EXIT_NO_DATA),
        ('--smc on a capture with no SMC lines exits NO_DATA',
         SELFTEST_NO_GR18_WIRE, smc_mode, EXIT_NO_DATA),
    ):
        result = parse_content(f'<{label}>', fixture)
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            got = mode(result)
        # A report that exits with a verdict and prints nothing is not a
        # report; assert it put something on the page too.
        wired.append((label, (got, bool(buf.getvalue().strip())),
                      (want, True)))
    wired_ok = True
    for label, actual, want in wired:
        good = actual == want
        if not good:
            wired_ok = False
            ok = False
        print(f"    {'ok ' if good else 'BAD'} {label}: got {actual!r}, "
              f"want {want!r}")
    print(f"=== selftest: GR18: report paths: "
          f"{'PASS' if wired_ok else 'FAIL'}\n")
    return ok


def selftest_logts():
    """The one stripping law: strip_logts() and the parse_ts readers must agree
    on where a stamp ends.

    They came from different lineages and had drifted — the timing reader wanted
    exactly one space after the ']', the census stripper allowed none and also
    tolerated a space before 'ms' and leading whitespace.  Any shape one accepts
    and the other does not is a line that is timestamped for one half of this
    file and deferred for the other, which is a miscount no output would
    announce.  Asserted, not commented."""
    print("=== selftest: logts: one stripping law (strip_logts == parse_ts) ===")
    cases = [
        ('monotonic',        '[   1851ms] :: BPACE: entry t=0ms d=0ms ::', 'mono'),
        ('civil',            '[15:30:45Z] [wc-g] win=1 seq=0 -> CLEAN', 'civil'),
        ('unknown',          '[      ?ms] :: X86_64 Memory Init ::', 'unknown'),
        ('narrow monotonic', '[0ms] :: entry ::', 'mono'),
        ('no stamp',         ':: FR-BOOT: hz=1 cy=2 reused=true state=flushed ::', None),
    ]
    checks = []
    for label, line, want_kind in cases:
        parsed = parse_ts(line)
        kind = parsed[0] if parsed else None
        body = parsed[2] if parsed else line
        checks.append((f'{label}: parse_ts kind', kind, want_kind))
        # The load-bearing one: same body from both implementations.
        checks.append((f'{label}: body agrees with strip_logts',
                       body, strip_logts(line)))
    ok = True
    for label, actual, want in checks:
        good = actual == want
        if not good:
            ok = False
        print(f"    {'ok ' if good else 'BAD'} {label}: got {actual!r}, want {want!r}")
    print(f"=== selftest: logts: one stripping law: {'PASS' if ok else 'FAIL'}\n")
    return ok


def selftest(top):
    """ONE self-test, both lineages.

    The suites are kept as separate functions because they assert different
    things through different entry points, but there is a single --selftest and a
    single exit code: a merged tool whose two halves could pass independently
    while the merged file was broken would be a tool nobody has actually tested.
    """
    results = []
    print("########## self-test 1/4: logts stripping law ##########\n")
    results.append(('logts stripping law', selftest_logts()))
    print("########## self-test 2/4: timing modes (--gaps, --wcg, paygo) ##########\n")
    results.append(('timing modes (--gaps / --wcg / paygo)', selftest_timing(top)))
    print("########## self-test 3/4: census + GR18 sections ##########\n")
    results.append(('census + GR18 sections (--wxprobe / --slowxfer / --smc)',
                    selftest_gr18()))
    print("########## self-test 4/4: WXN-x86 sweep + WXAUDIT census (--wxn) ##########\n")
    results.append(('WXN sweep + audit (--wxn)', selftest_wxn()))
    print("########## self-test summary ##########")
    for label, good in results:
        print(f"  {'PASS' if good else 'FAIL'}  {label}")
    ok = all(good for _, good in results)
    print(f"  ==> {'ALL SUITES PASS' if ok else 'FAILURE'}")
    return ok


def main():
    parser = argparse.ArgumentParser(
        description="Analyze UnaOS bench serial captures (x86_64 / aarch64 / Orin)",
        epilog=("logts prefixes: '[  NNNNNms] ' is an absolute stamp in ms since KERNEL ENTRY "
                "-- the same origin the BPACE/GPACE since-entry figures use, so the numbers can "
                "be compared with those ledger lines directly. '[HH:MM:SSZ] ' is civil time. "
                "'[      ?ms] ' is prefixed-but-unknown (emitted before the bootpace entry stamp "
                "or before TSC calibration): counted separately, never a gap endpoint. A capture "
                "that is entirely '?ms' is reported as 'counter never calibrated' and exits "
                "nonzero."))
    parser.add_argument("logs", nargs='*', help="Log files to parse (1 or 2 files)")
    parser.add_argument("--full", action="store_true",
                        help="print every witness and defect line, not samples")
    parser.add_argument("--samples", type=int, default=2, metavar="N",
                        help="sample lines to show per family (default 2)")
    parser.add_argument("--family", metavar="REGEX",
                        help="only report families whose name matches REGEX "
                             "(implies full output for those families)")
    parser.add_argument("--wxprobe", action="store_true",
                        help="read the 8-line WXPROBE block: per-boot table, "
                             "consecutive-boot DIFF on the raw leaf entry, and "
                             "the fb write-combining cross-check")
    parser.add_argument("--wxn", action="store_true",
                        help="read the WXN-x86 M1 sweep, the WXAUDIT census "
                             "and its leaf histogram, WXAUDIT-NXE and "
                             "WXN-FBWC: per-boot block with cross-field "
                             "self-checks, the cross-boot kern_WX trend (a "
                             "RISE is coverage silently lost unless "
                             "kern_WX == keep_x == xpages+1 holds on both "
                             "boots, which makes it code growth), and the "
                             "WXN-FBWC leaf DIFF")
    parser.add_argument("--slowxfer", action="store_true",
                        help="read EPACE-TRIM M8 SLOW-XFER: per-boot table with "
                             "the control request decoded, the controller-[1] "
                             "check, and the print-cap overflow line")
    parser.add_argument("--smc", action="store_true",
                        help="read SMC-BATT gap/busy/late (cumulative: greatest "
                             "wins, never summed) and the SMC-SCOUT #KEY index "
                             "walk")
    parser.add_argument("--gaps", action="store_true",
                        help="report the largest inter-line time gaps in a logts-prefixed capture")
    parser.add_argument("--wcg", action="store_true",
                        help="decompose the witness cost inside the kepler window: per-instrument "
                             "attribution, per-pass [wc-g] costs, the [wc-g] prof phase table when "
                             "the build emits one, the pay-as-you-go census, and a serial-overhead "
                             "estimate")
    parser.add_argument("--boot", type=int, default=None,
                        help="with --wcg, restrict the report to boot N (1-based)")
    parser.add_argument("--top", type=int, default=15,
                        help="how many gaps to list per table (default 15)")
    parser.add_argument("--selftest", action="store_true",
                        help="run every fixture set (timing modes + census/GR18 sections) and exit")
    args = parser.parse_args()

    if args.selftest:
        sys.exit(EXIT_OK if selftest(args.top) else 1)

    if not args.logs:
        parser.error("no log files given")

    # The TIMING modes cut the capture on 'hz=' rather than on the boot-marker
    # table (see the module docstring), so they run off their own file read and
    # are selected first.  Their exits are EXIT_OK / EXIT_USAGE (refused: the
    # measurement could not be taken) / EXIT_FINDING (measured, and the report
    # carries a finding) — worst across the files, as the section modes do it.
    if args.wcg:
        worst = EXIT_OK
        for log_file in args.logs:
            worst = max(worst, wcg_mode(log_file, args.boot))
        sys.exit(worst)

    if args.gaps:
        worst = EXIT_OK
        for log_file in args.logs:
            worst = max(worst, gaps_mode(log_file, args.top))
        sys.exit(worst)

    # The GR18 sections are per-boot readers over the same parse the census
    # uses, so they share parse_log and are selected here rather than each
    # re-walking the file.
    sections = [(flag, fn) for flag, fn in
                (('--wxprobe', wxprobe_mode), ('--wxn', wxn_mode),
                 ('--slowxfer', slowxfer_mode), ('--smc', smc_mode))
                if getattr(args, flag.lstrip('-').replace('-', '_'))]
    if sections:
        worst = EXIT_OK
        for log_file in args.logs:
            result = parse_log(log_file)
            if not result['boots']:
                print(f"ERROR: {log_file}: parsed 0 boots — no boot-start "
                      f"marker matched. Markers tried: "
                      f"{'; '.join(result['markers_tried'])}", file=sys.stderr)
                worst = max(worst, EXIT_NO_BOOTS)
                continue
            for _flag, fn in sections:
                worst = max(worst, fn(result))
        sys.exit(worst)

    if len(args.logs) > 2:
        print("Please provide 1 or 2 log files.", file=sys.stderr)
        sys.exit(EXIT_USAGE)

    family_filter = re.compile(args.family) if args.family else None

    results = []
    for log_file in args.logs:
        result = parse_log(log_file)
        results.append(result)
        print_file_report(result, samples=args.samples, full=args.full,
                          family_filter=family_filter)

    if len(results) == 2:
        diff_boots(results[0]['boots'], results[1]['boots'])

    # ---- loud-failure gate -------------------------------------------------
    # This tool previously printed a header and exited 0 while parsing
    # nothing.  An instrument that can report silence as success cannot
    # falsify anything, so silence is now fatal and goes to stderr.
    failed = False
    for result in results:
        if not result['boots']:
            print(f"ERROR: {result['path']}: parsed 0 boots — no boot-start "
                  f"marker matched. Markers tried: "
                  f"{'; '.join(result['markers_tried'])}", file=sys.stderr)
            failed = EXIT_NO_BOOTS
    if failed:
        sys.exit(EXIT_NO_BOOTS)

    for result in results:
        witnesses = sum(len(b['witnesses']) for b in result['boots'])
        if witnesses == 0:
            print(f"ERROR: {result['path']}: parsed "
                  f"{len(result['boots'])} boot(s) but 0 witnesses — no entry "
                  f"in WITNESS_FAMILIES matched. The capture format has moved "
                  f"and the family table is stale.", file=sys.stderr)
            failed = EXIT_NO_WITNESSES
        for boot in result['boots']:
            if not boot['witnesses']:
                print(f"WARNING: {result['path']}: boot {boot['number']} "
                      f"({boot['platform']}) has 0 witnesses.", file=sys.stderr)
    if failed:
        sys.exit(EXIT_NO_WITNESSES)

    sys.exit(EXIT_OK)


if __name__ == '__main__':
    main()
