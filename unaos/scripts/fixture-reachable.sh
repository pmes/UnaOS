#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# fixture-reachable.sh [repo-root]  — GATE-DEADFIXTURE (orin session, 2026-09-13).
#
# A FIXTURE MUST LIVE ON AN ENTRY POINT THE GATE'S OWN BOOT EXECUTES. A leg added to an
# INTERACTIVE-ONLY entry point compiles, ships in the image, carries a perfectly good go-red — and
# is never run by `./arroyo test` or `./arroyo test-arm`. It gates NOTHING, and a reviewer reading
# the diff cannot tell: the diff of a dead fixture and the diff of a live one are the same diff.
# This is the fourth member of 2026-09-13's family and the worst of them. The other three are "the
# gate ran and proved nothing"; this one is "the gate never ran, and nothing said so".
#
# FOUND BY MEASUREMENT, NOT BY READING: executor LUN2 had `crates/kernel/src/selftest.rs` in its
# brief's FILES list, DECLINED to put its fixture there, and said why — then put it on
# `homesoil_selftest`/`unafsroot_selftest`, which both QEMU verbs execute, and proved it by `awk`
# over both serial captures. That verification is the only reason anyone knows.
#
# ── THE TRUTH, MEASURED AT f164b6fd ON A HEALTHY 2,089-LINE `UNAOS_QEMU_FULL=1 UNAOS_WC=1
#    ./arroyo test 90` CAPTURE ────────────────────────────────────────────────────────────────────
#
# NOT REACHED HEADLESSLY — `crates/kernel/src/selftest.rs::run()` (:428), the in-OS `tste` suite.
#   Its SOLE caller is `crates/kernel/src/shell.rs:5727`, the `tste` shell verb, and a headless run
#   types no shell command. So `run()` and its whole subtree are dead to both gates:
#   `test_sched_introspection` (:522), `test_heap_roundtrip` (:556), `test_video_geometry` (:626),
#   `video::witness::run` (called at :469), `run_sync_section` (:823) and `run_storage_section`
#   (:900) — thirteen legs. PROOF ON THE WIRE: `:: TSTE: suite start ::` 0 hits and
#   `:: TSTE: suite complete ::` 0 hits in that capture, both printed unconditionally by `run()`.
#
# REACHED HEADLESSLY, AND EASY TO MISTAKE FOR THE ABOVE — the same capture carries 23 `:: TSTE:`
#   lines (`midden.dispatch`, `shell.basics.*`, `shell.relics.*`, `vfs.aclsym.dir`, `fatverb.*`,
#   `vfsroute.*`, `layout.volid`). NONE of them comes from `selftest.rs`. EXACTLY TWO FILES print a
#   `:: TSTE:` line at all — `selftest.rs` (6 sites) and `shell.rs` (20 sites), counted with
#   `grep -rn 'serial_println!(":: TSTE:'` — and every one of the 23 in the capture is a `shell.rs`
#   BOOT-PATH fixture (:2896, :3010, :3156, :3342) that deliberately borrows the SAME WIRE SHAPE so
#   the `tste` boot-replay ring can pick it up. (`main.rs:491` mentions the shape in a COMMENT and
#   prints nothing; it is named here because it is the near-miss a grep for `TSTE` turns up.) A
#   reader who greps the capture for `TSTE` concludes the suite ran. It did not.
#
# ALSO REACHED, BUT NOT AN ENTRY POINT FOR FIXTURES — `selftest::capture()` (:230). Its only callers
#   are the two serial print paths (`arch/x86_64/serial.rs:81,202`, `arch/aarch64/serial.rs:194,271`):
#   it is a passive TAP that sniffs `-> PASS/FAIL` verdicts off the wire into a boot ring. Adding a
#   fixture "to selftest.rs" does not land there and cannot.
#
# SO: put a headless-gated fixture on a BOOT-PATH entry point — the shape LUN2 used
# (`homesoil_selftest` riding `unafsroot_selftest`), or any of the `shell.rs` boot fixtures above —
# and prove it the way LUN2 did: its VERDICT in the CAPTURE, never its STRING in the image.
#
# ── WHAT THIS GATE CHECKS, and what it deliberately does not ───────────────────────────────────
#
# It does NOT attempt general reachability. "Every `-> PASS` string in the ELF must appear in some
# capture" is the tempting universal form and it is unshippable: knob-gated, arch-gated and
# legitimately-skipping fixtures are absent from a healthy capture by design, so that check would
# red on dozens of correct builds — wrong-strict, which LAWS §5 ranks below wrong-lenient. What is
# cheap and sound is to name the ONE entry point measured dead and hold its roster frozen, the way
# `k8-reach.registry` holds unarmed knobs: a leg there is not forbidden, it is ACCOUNTED FOR.
#
#   LEG A — THE DECLARATION IS STILL TRUE. `crate::selftest::run(` must have exactly ONE caller and
#           it must be in `shell.rs`. A second caller means a boot path may now reach it and the
#           declaration above has to be re-measured rather than trusted. ZERO callers is exit 2, NO
#           VERDICT — that is the pattern breaking (a rename), not the tree being clean, and it is
#           this gate's control probe.
#   LEG B — NOTHING NEW IS HIDING THERE. The fixture-leg NAMES inside `selftest.rs` must be exactly
#           the declared roster below. A new or renamed leg reds by NAME, with the instruction.
#
# Exit 0 = clean · 1 = a leg was added to the dead entry point, or a second caller appeared ·
#          2 = no verdict (files unreadable, or the caller pattern matched nothing).
set -u
ROOT="${1:-$(cd "$(dirname "$0")/../.." && pwd)}"
K="$ROOT/unaos/crates/kernel/src"
ST="$K/selftest.rs"
[ -r "$ST" ] || { echo "DEADFIXTURE -> NO VERDICT: cannot read $ST"; exit 2; }

# The roster, sorted, one per line. These fourteen are INTERACTIVE-ONLY BY DESIGN — `tste` is a
# shell verb and this suite is what it prints. They are listed so that a FIFTEENTH is visible.
#
# `net.sntp` ADDED 2026-09-22 (TOOLRESCUE, the fold that first ran this gate), and by the SECOND of
# the two cures this script names, not the first. ORINTIME (ad549988) put it here because it had no
# drive seam to put it on; that seam is now landed one commit back (`net_phy.rs:1041`), but the
# FIXTURE is `#[cfg(all(feature = "sntp6", feature = "net6", target_arch = "aarch64"))]` and NO
# headless gate in this tree arms `sntp6` — `test` is x86 and `test-arm` is virt without it. So
# MOVING it to a boot-path entry point would turn this gate green while leaving the leg exactly as
# unrun as it is today, which is laundering the finding rather than closing it. Declared instead,
# the way k8-reach.registry accounts for an unarmed knob: the roster still names it, a fifteenth
# leg still reds, and the honest reading is "written, gated, and waiting for a gate that arms it".
# ⚠ MUST NOT OUTLIVE THAT: the day a headless verb arms `sntp6`, this entry is the stale one and
# the leg moves to the boot wire the way `video.font.aa` just did.
DECLARED='heap.roundtrip
net.sntp
sched.introspection
storage.mount
storage.readfile
storage.rootwalk
sync.channel
sync.condvar
sync.join
sync.mutex
sync.rwlock
sync.semaphore
video.geometry
video.present'

rc=0

# ── LEG A ──────────────────────────────────────────────────────────────────────────────────────
# Comments stripped first: `selftest.rs`'s own module note quotes `selftest::run` in prose, and a
# gate that counts its subject's documentation as a call site is the same defect one file over.
callers=$(find "$K" -name '*.rs' -print0 \
    | xargs -0 grep -n 'crate::selftest::run(' 2>/dev/null \
    | sed 's/[[:space:]]*\/\/.*$//' \
    | grep 'crate::selftest::run(' || true)
ncall=$(printf '%s' "$callers" | grep -c . || true)
if [ "$ncall" -eq 0 ]; then
    echo "DEADFIXTURE -> NO VERDICT: not one call of \`crate::selftest::run(\` found under $K."
    echo "  This gate's control probe: the entry point it declares dead must still EXIST and still be"
    echo "  called from somewhere. Zero means it was renamed or removed — re-measure the declaration"
    echo "  in this file's header; do not read this as clean."
    exit 2
fi
printf '%s\n' "$callers" | sed "s|^$ROOT/||"
if [ "$ncall" -ne 1 ] || ! printf '%s' "$callers" | grep -q '/shell\.rs:'; then
    echo "DEADFIXTURE: \`selftest::run\` has $ncall caller(s) and this gate's header declares exactly one"
    echo "  (the \`tste\` verb in shell.rs). A new caller may put the suite on a path a headless boot"
    echo "  DOES execute — which would be good news — but the claim has to be RE-MEASURED on a capture"
    echo "  (\`:: TSTE: suite start ::\` appearing in target/serial.log) before the header is rewritten."
    rc=1
fi

# ── LEG B ──────────────────────────────────────────────────────────────────────────────────────
found=$(sed 's/[[:space:]]*\/\/.*$//' "$ST" | grep -o '"[a-z][a-z0-9]*\.[a-z0-9_.]*"' | tr -d '"' | sort -u)
nfound=$(printf '%s\n' "$found" | grep -c . || true)
added=$(comm -13 <(printf '%s\n' "$DECLARED" | sort) <(printf '%s\n' "$found") || true)
gone=$(comm -23 <(printf '%s\n' "$DECLARED" | sort) <(printf '%s\n' "$found") || true)
if [ -n "$added" ]; then
    echo "DEADFIXTURE: NEW fixture leg(s) in an entry point a headless boot NEVER EXECUTES —"
    printf '    %s\n' $added
    echo "  \`selftest.rs::run\` is the \`tste\` shell suite (sole caller shell.rs's \`tste\` verb), so a leg"
    echo "  here ships in the image and is run by NEITHER \`./arroyo test\` NOR \`./arroyo test-arm\`."
    echo "  Put it on a boot-path entry point instead and prove it by its VERDICT IN THE CAPTURE."
    echo "  If it really is meant to be interactive-only, add the name to DECLARED in this script"
    echo "  with the reason — accounted for, the way k8-reach.registry accounts for an unarmed knob."
    rc=1
fi
if [ -n "$gone" ]; then
    echo "DEADFIXTURE: declared leg(s) no longer present in $ST —"
    printf '    %s\n' $gone
    echo "  A rename or a removal. Update DECLARED in the same commit so the roster keeps meaning something."
    rc=1
fi

echo "DEADFIXTURE selftest.rs::run callers=$ncall legs_found=$nfound legs_declared=$(printf '%s\n' "$DECLARED" | grep -c .) -> $([ "$rc" -eq 0 ] && echo PASS || echo FAIL)"
exit $rc
