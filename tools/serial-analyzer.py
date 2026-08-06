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
--selftest.

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
        got = tuple(f.get(k, '-') for k in WC_EXPECT)
        want = tuple(WC_EXPECT.values())
        entry = WC_PAT_ENTRY.get(got, 'not a PAT combination this reader knows')
        if got == want:
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
        if pe != ce:
            cls = ('WARN WXPROBE-FB-ENTRY-CHANGED'
                   if at == WC_LEAF else 'WARN WXPROBE-ENTRY-CHANGED')
            tail = (" — this is the GR15 regression signature: the framebuffer "
                    "leaf was retyped between boots" if at == WC_LEAF else "")
            findings.append(f"{cls}: leaf '{at}' entry {pe} -> {ce} across "
                            f"boots {prev['number']} -> {cur['number']}{tail}")
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
            # The paygo census is still worth printing: it is whole-boot scoped, so a boot
            # whose kepler anchors never appeared can still have a complete deferral story.
            for _tag in PAYGO_TAGS:
                print_paygo_stats(paygo_stats(chunk, _tag), _tag)
            continue
        start, end = window
        windows += 1
        print_wcg_stats(wcg_stats(chunk[start:end + 1]))
        for _tag in PAYGO_TAGS:
            print_paygo_stats(paygo_stats(chunk, _tag), _tag)

    if not windows:
        print(f"{label}: no kepler window in any boot; nothing to decompose")
        return False
    return True



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
        ('wcg: real GR17 boot-7 paygo wire (lattice pass 1, deferred full passes)',
         WCG_FIXTURE_PAYGO, True, wcg_paygo_expect),
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
    print("########## self-test 1/3: logts stripping law ##########\n")
    results.append(('logts stripping law', selftest_logts()))
    print("########## self-test 2/3: timing modes (--gaps, --wcg, paygo) ##########\n")
    results.append(('timing modes (--gaps / --wcg / paygo)', selftest_timing(top)))
    print("########## self-test 3/3: census + GR18 sections ##########\n")
    results.append(('census + GR18 sections (--wxprobe / --slowxfer / --smc)',
                    selftest_gr18()))
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
    # are selected first.  Their exit is 0/1 as it always was: these two answer a
    # measurement question, and 'refused' is their only failure.
    if args.wcg:
        ok = True
        for log_file in args.logs:
            if not wcg_mode(log_file, args.boot):
                ok = False
        sys.exit(EXIT_OK if ok else 1)

    if args.gaps:
        ok = True
        for log_file in args.logs:
            if not gaps_mode(log_file, args.top):
                ok = False
        sys.exit(EXIT_OK if ok else 1)

    # The GR18 sections are per-boot readers over the same parse the census
    # uses, so they share parse_log and are selected here rather than each
    # re-walking the file.
    sections = [(flag, fn) for flag, fn in
                (('--wxprobe', wxprobe_mode), ('--slowxfer', slowxfer_mode),
                 ('--smc', smc_mode))
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
