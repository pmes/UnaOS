#!/bin/bash
# boot-leg.sh — GATE-BOOTLEG (SMALLFIX7, rmbp-ledger B501): the metal feature line, BOOTED, before the bench.
#
# WHY. Flight 27's first two image-20 cards never booted: `memory allocation of 96 bytes failed` before the
# console — the stack guard's arm-time `tests::register` met SMALLFIX6's heap-grown table before the heap. Every
# cloud leg was green: the gate compiled the metal line but never booted it, and the QEMU lanes export
# `UNAOS_TESTS_AT_BOOT=1` (arroyo, the lane block at the top), so their kernel carries `tests-at-boot` and
# `register` runs the fixture instead of reaching the table. The metal image has no `tests-at-boot`.
#
# WHAT IT DOES. Maps the seat's metal feature line (below; FLIGHT27 / the merge gate's X86F) to arroyo's knobs,
# runs `./arroyo test <secs>` with `UNAOS_TESTS_AT_BOOT=` (EMPTY: arroyo now honours it — the metal registry
# path), and judges `target/serial.log`: FAIL on a panic shape, FAIL if the boot never reached the heap line,
# the deferred announce or a `:: BOOT:` line; PASS otherwise. arroyo's own spec verdict is NOT this gate's (the
# x86 specs pin boot-time fixtures this leg deliberately defers).
#
# usage: ./arroyo bootleg [--judge <serial log>]  (= boot-leg.sh <unaos dir> …; exit 0 PASS, 1 FAIL, 127 no qemu / no arroyo)
# `--judge` reads a captured log only (no build, no QEMU): `./arroyo bootleg --judge <log>`.
set -u
U="${1:?usage: boot-leg.sh <unaos dir> [--judge <serial log>]}"; shift || true
JUDGE=""
[ "${1:-}" = "--judge" ] && JUDGE="${2:?--judge needs a log}"
METAL="wc,quarry,ftdirx,login,loginst,nvidia-kepler-vblank,smc,usbnet,hda,hda-tone,facet,beam,sdw,sdwrite,sdhcblk,selfhost,linuxabi,ahci,unafs,busreg,lumen,netring3,prefs_reset,census,installdemo,instgui,witness,selfdiag,ahciroot,btc,kvblank_trace,nvidia-kepler-ce,intel-ivb,unaos_ivb,gmux_igd,gen7,gen7r8,gen7blit,lidsleep,videoplayer,nvidia-kepler,nvidia-kepler-takeover,smolnet,ahci-write,wifi,wcdvalve,ehcihid,holocron,kbdwit,usbdebug,svg"
SECS="${UNAOS_BOOTLEG_SECS:-120}"

judge() { # $1 = serial log; prints the verdict line, returns 0/1
    awk -v src="$1" '
        index($0, "memory allocation of") && index($0, "bytes failed") { if (!bad) bad = NR ": " $0 }
        index($0, "panicked at") || index($0, "UnaOS stopped") || index($0, "KERNEL PANIC") { if (!bad) bad = NR ": " $0 }
        index($0, ":: KERNEL HEAP ALLOCATED ::") { heap = 1 }
        index($0, ":: TESTS: deferred=") { deferred = 1 }
        index($0, ":: BOOT:") { boot = 1 }
        END {
            reached = heap || deferred || boot
            if (bad != "") { printf "BOOTLEG: log=%s lines=%d heap=%d deferred=%d boot=%d panic=\"%s\" -> FAIL\n", src, NR, heap, deferred, boot, substr(bad, 1, 160); exit 1 }
            if (!reached) { printf "BOOTLEG: log=%s lines=%d heap=0 deferred=0 boot=0 -> FAIL reason=never-reached-a-boot-line\n", src, NR; exit 1 }
            printf "BOOTLEG: log=%s lines=%d heap=%d deferred=%d boot=%d panic=none -> PASS\n", src, NR, heap, deferred, boot
        }' "$1"
}

if [ -n "$JUDGE" ]; then
    [ -f "$JUDGE" ] || { echo "BOOTLEG: log=$JUDGE -> FAIL reason=no-log"; exit 1; }
    judge "$JUDGE"; exit $?
fi
[ -x "$U/arroyo" ] || { echo "BOOTLEG: no arroyo at $U -> MISSING"; exit 127; }
command -v qemu-system-x86_64 >/dev/null 2>&1 || { echo "BOOTLEG: qemu-system-x86_64 not installed -> MISSING"; exit 127; }

# The knob that arms each feature: arroyo's one-line arms `[ -n "${UNAOS_X:-}" ] && _feats="${_feats}<feat>,"`.
knobs=() unmapped=""
for f in ${METAL//,/ }; do
    k="$(awk -v f="$f" 'index($0, "_feats=\"${_feats}" f ",\"") && match($0, /\$\{UNAOS_[A-Z0-9_]+:-\}/) { print substr($0, RSTART + 2, RLENGTH - 5); exit }' "$U/arroyo")"
    if [ -n "$k" ]; then knobs+=("$k=1"); else unmapped="${unmapped}${unmapped:+,}$f"; fi
done
echo "BOOTLEG: knobs=${#knobs[@]} unmapped=${unmapped:-none} (an unmapped feature is a default, implied by another knob, or armed in a block arm)"
log="$U/target/serial.log"
rm -f "$log"
( cd "$U" && env "${knobs[@]}" UNAOS_TESTS_AT_BOOT= ./arroyo test "$SECS" ) > "$U/target/bootleg.out" 2>&1
rc=$?
echo "BOOTLEG: arroyo test rc=${rc} (its spec verdict is not this gate's; output: target/bootleg.out)"
[ -f "$log" ] || { echo "BOOTLEG: log=$log -> FAIL reason=no-serial-log (the build or QEMU did not start)"; exit 1; }
judge "$log"
