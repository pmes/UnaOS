#!/usr/bin/env bash
# ═════════════════════════════════════════════════════════════════════════════════════════════════
# scorer11.sh — score the render11 boot against the BOOTROOT witness contract.
#
#   usage: scorer11.sh <wire.log> <kernel.elf>
#
#   <wire.log>    the scoped serial capture of ONE boot (see FLIGHT-render11.md §4)
#   <kernel.elf>  the kernel image that WAS FLASHED — not the one that was built, not the one in
#                 target/. The arming field is read from THIS file with `strings -a`.
#
#   exit: 0  every leg PASS (or a green NOTE)          — the flight satisfies the contract
#         1  at least one FAIL/ABSENT/CONVICT/WRONG-IMAGE/NOT-SCORED  — do not file this green
#         2  the ARMING FIELD COULD NOT BE ESTABLISHED — nothing was scored, no verdict is offered
#         3  the root family is NOT-EXERCISED (artifact readable, control fires, arming literal
#            absent) and nothing else is red. DOCUMENTED EXTENSION to the brief's 0/1/2 ladder:
#            a NOT-EXERCISED family is NEITHER a pass NOR a failure, so it must not exit 0 (that is
#            how scorer10's n-ex rows were read as green — orin21 BULLETIN §31) and it must not
#            exit 1 (that teaches the next seat to suppress the rule). Collapse 3→2 in the two
#            `EXIT=3` lines below if a caller needs the three-code ladder exactly.
#
# ═════════════════════════════════════════════════════════════════════════════════════════════════
# THE CONTRACT THIS FILE SCORES (executor BOOTROOT, orin 22; Peter's direction 2026-09-08:
# "root is the volume you booted from — the loader names it, the kernel finds it among the block
# devices it enumerates". No board, bus, slot, card serial or geometry.)
#
#   loader (crates/bootloader):   boot volume FAT serial 0x........
#   kernel, on the FIRST mount-table build, EXACTLY ONE of:
#     [vfs] root = boot volume serial=0x%08x source=<global|usb|sdhc|tegra-sd> ::
#     [vfs] root -> NONE reason=<no-disk-enumerated|kernel-not-found-on-any-volume|walk-cap-hit|multiple-kernels> matches=N disks=... candidates=N window_off=.. window_len=.. ::
#     (v4 contract, 15:25Z: root is found by CONTENT — the kernel's own .text window vs candidate files — not by the loader serial; the bind line still carries serial= (the matched volume's BS_VolID) so LOADER-SERIAL == bind serial remains the identity check on UEFI boots)
#   the `[sdmmc] root …` family is DELETED. Its presence on the wire or in the artifact means the
#   image that flew is not a render11 image.
#
# ⛔ NOTHING IN THIS FILE IS KEYED ON A SECTOR COUNT, A CARD SERIAL, A VOLUME LABEL, A PARTITION
#    GEOMETRY OR A CARD NAME. That is what made scorer10 emit two FALSE REDs (BULLETIN §31: the
#    62333952 hardcode, and the pre-fitsland GROW-GEOM literal). The only comparison this file makes
#    between two numbers is LOADER SERIAL == ROOT SERIAL, and both operands are read off the wire it
#    is scoring. Hex is compared by VALUE (leading zeros stripped), never as a string, because the
#    loader's formatter and the kernel's `%08x` need not agree on width.
#
# ⚠ MATCHING DISCIPLINE: every witness is found with awk `index($0,"literal")` — fixed string, never
#   a bracketed regex (control bytes in these logs, and CLAUDE.md B7's character-class trap). The
#   ONLY regexes are the three hex/token EXTRACTORS (`serial=0x[0-9a-f]+`, `reason=…`, `source=…`),
#   which are applied to a line already selected by a fixed-string index() hit. That is deliberate.
#
# ⚠ EVERY LEG IS GATED ON A CONTROL THAT MUST FIRE. A zero counted on a wire that is not a capture,
#   or in a file that is not a kernel image, is not evidence — it scores NOT-SCORED, never PASS and
#   never FAIL. (UNAOS-LAWS: "a check is trusted only when its corpus can produce more than one
#   outcome"; memory: "a check that cannot fire".)
#
# CAN-FIRE PROOF: `mutate11.sh` builds the fixture set (mutations of the render9 wire + four
# artifact fixtures) and drives every leg through every outcome. Table: MUTATIONS.md.
# ═════════════════════════════════════════════════════════════════════════════════════════════════
set -uo pipefail

W="${1:-}"; K="${2:-}"
if [ -z "$W" ] || [ -z "$K" ]; then
  echo "usage: scorer11.sh <wire.log> <kernel.elf>" >&2
  echo "  both are REQUIRED. The kernel.elf is the FLASHED image; the arming field comes from it." >&2
  exit 2
fi
[ -r "$W" ] || { echo "scorer11: cannot read wire log '$W' -> NO-VERDICT (nothing scored)"; exit 2; }

RED=0; GREEN=0; NOTES=0; NEX=0; ROWS=""
# leg <ID> <datum text> <verdict text>
# THE VERDICT IS A SEPARATE ARGUMENT ON PURPOSE. scorer10 classified a green leg RED by splitting a
# printed line on an arrow, and its fix (split on the FIRST ` -> `) only moved the trap: this file's
# ROOT-NONE datum quotes the wire literal `[vfs] root -> NONE`, which contains the separator, and the
# first-arrow rule read the verdict as "NONE". Both traps die if the verdict is never re-parsed out
# of the printed text at all.
leg() {
  local id="$1" datum="$2" verdict="$3" v
  printf '%s -> %s\n' "$datum" "$verdict"
  v="$(printf '%s' "$verdict" | awk '{print $1}')"
  case "$v" in
    PASS)           GREEN=$((GREEN+1)); ROWS="${ROWS}${id}|GREEN|${v}"$'\n' ;;
    NOTE)           NOTES=$((NOTES+1)); ROWS="${ROWS}${id}|note |${v}"$'\n' ;;
    NOT-EXERCISED)  NEX=$((NEX+1));     ROWS="${ROWS}${id}|n-ex |${v}"$'\n' ;;
    *)              RED=$((RED+1));     ROWS="${ROWS}${id}|RED  |${v}"$'\n' ;;
  esac
}

# ═════════════════════════════════════════════════════════════════════════════════════════════════
# LEG (e) — ARMING. Read FIRST, on the ARTIFACT, and it can stop the run.
#
# Two literals are counted with `strings -a` on the flashed kernel.elf:
#   ARM  `[vfs] root = boot volume`   — the bind witness's own format string. Present iff the
#        emitter is compiled into THIS image.
#   CTL  `crates/kernel/src/`         — the POSITIVE CONTROL: panic::Location paths, emitted
#        unconditionally by every kernel build regardless of feature set. Measured 2026-09-08:
#        32 in unaos/target/aarch64_esp/kernel.elf, 47 in x86_64_esp/kernel.elf, 53 in the render10
#        candidate. A ZERO here means `strings` produced nothing usable or the file is not a UnaOS
#        kernel image — in which case ARM's zero says NOTHING and this scorer refuses to score.
#        (orin21 A11: controls must be strings known to be IN the artifact. `KELF` and `ORIN-CARD`
#        are LOADER strings and were the bad controls that produced a meaningless plain-side zero.)
# ═════════════════════════════════════════════════════════════════════════════════════════════════
if [ ! -r "$K" ]; then
  echo "ARMING: kernel.elf '$K' is not readable -> NO-ARMING-FIELD — REFUSING TO SCORE. The arming field is established on the image that was FLASHED, never from the knobs that were typed. Without it, an absent [vfs] root line cannot be told NOT-EXERCISED (emitter not in the image) from ABSENT (emitter in the image and silent) — and this scorer will not guess."
  exit 2
fi
ARM=$(strings -a "$K" 2>/dev/null | grep -c -F -- '[vfs] root = boot volume'); ARM=${ARM:-0}
CTL=$(strings -a "$K" 2>/dev/null | grep -c -F -- 'crates/kernel/src/'); CTL=${CTL:-0}
OLDART=$(strings -a "$K" 2>/dev/null | grep -c -F -- '[sdmmc] root'); OLDART=${OLDART:-0}
if [ "$CTL" -eq 0 ]; then
  echo "ARMING: positive control 'crates/kernel/src/'=0 in '$K' (arming literal count=$ARM, ignored) -> NO-ARMING-FIELD — REFUSING TO SCORE. The control is the panic::Location path prefix that every UnaOS kernel build carries whatever its features; a zero means `strings -a` read nothing usable from this file or the file is not a kernel image. A zero counted next to a control that does not fire is not evidence of anything."
  exit 2
fi
if [ "$ARM" -gt 0 ]; then ARMED=armed; else ARMED=unarmed; fi

# ── the single wire pass. Everything downstream reads these KEY=VALUE facts. ─────────────────────
eval "$(LC_ALL=C tr -d '\000' < "$W" | LC_ALL=C awk '
function norm(h,  x){ x=tolower(h); sub(/^0+/,"",x); if(x=="") x="0"; return x }
{ n_total++ }
index($0,"crates/bootloader/src/main.rs@") { c_loader++ }
index($0,"SCHED:") || index($0,"[menubar]") || index($0,"TEGRA-") || index($0,"[orinrender]") || index($0,"[dock]") { c_kernel++ }
index($0,"boot volume FAT serial 0x") {
  if (match($0,/boot volume FAT serial 0x[0-9a-fA-F]+/)) {
    s=substr($0,RSTART,RLENGTH); sub(/.*0x/,"",s); s=norm(s)
    ld_n++; if (!(s in ld_seen)) { ld_seen[s]=1; ld_d++ } ; ld_last=s; ld_line=$0
  } else { ld_mal++; ld_line=$0 }
}
index($0,"[vfs] root = boot volume") {
  b_n++; b_line=$0
  if (match($0,/serial=0x[0-9a-fA-F]+/)) {
    s=norm(substr($0,RSTART+9,RLENGTH-9))
    if (!(s in b_seen)) { b_seen[s]=1; b_d++ } ; b_last=s
  } else b_mal++
  if (match($0,/source=[A-Za-z0-9_.-]+/)) { src=substr($0,RSTART+7,RLENGTH-7); if(!(src in s_seen)){s_seen[src]=1; s_d++; s_list=s_list (s_list==""?"":",") src} ; s_last=src } else s_mal++
}
index($0,"[vfs] root -> NONE") {
  nn_n++; nn_line=$0
  if (match($0,/reason=[A-Za-z0-9_.-]+/)) nn_reason=substr($0,RSTART+7,RLENGTH-7)
  if (match($0,/serial=0x[0-9a-fA-F]+/))  nn_serial=norm(substr($0,RSTART+9,RLENGTH-9))
}
index($0,"[sdmmc] root") { old_n++; old_line=$0 }
index($0,"[vfs] root")   { vfs_any++ }
function q(s){ gsub(/\047/,"",s); return s }
END{
  printf "N_TOTAL=%d; C_LOADER=%d; C_KERNEL=%d;\n", n_total+0, c_loader+0, c_kernel+0
  printf "LD_N=%d; LD_D=%d; LD_MAL=%d; LD_LAST=%s;\n", ld_n+0, ld_d+0, ld_mal+0, (ld_last==""?"-":ld_last)
  printf "B_N=%d; B_D=%d; B_MAL=%d; B_LAST=%s;\n", b_n+0, b_d+0, b_mal+0, (b_last==""?"-":b_last)
  printf "S_D=%d; S_MAL=%d; S_LAST=%s; S_LIST=%s;\n", s_d+0, s_mal+0, (s_last==""?"-":s_last), (s_list==""?"-":s_list)
  printf "NN_N=%d; NN_REASON=%s; NN_SERIAL=%s;\n", nn_n+0, (nn_reason==""?"-":nn_reason), (nn_serial==""?"-":nn_serial)
  printf "OLD_N=%d; VFS_ANY=%d;\n", old_n+0, vfs_any+0
  printf "LD_LINE=\047%s\047; B_LINE=\047%s\047; NN_LINE=\047%s\047; OLD_LINE=\047%s\047;\n", q(substr(ld_line,1,150)), q(substr(b_line,1,170)), q(substr(nn_line,1,170)), q(substr(old_line,1,150))
}')"

echo "-- scorer11: wire=$W ($N_TOTAL lines) | artifact=$K"
echo "   ARTIFACT strings: arming '[vfs] root = boot volume'=$ARM | positive control 'crates/kernel/src/'=$CTL | retired '[sdmmc] root'=$OLDART -> ARMED=$ARMED"
echo "   WIRE controls: loader lines=$C_LOADER | kernel lines=$C_KERNEL"
echo

leg ARMING "ARMING: arming literal=$ARM, positive control=$CTL (fires), retired-family literal in artifact=$OLDART" "$(
  if [ "$ARMED" = armed ]; then
    echo "PASS ARMING (the bind emitter IS compiled into the flashed image, so an absent [vfs] root line on the wire is a DEFECT and not a configuration. Root family is scoreable.)"
  elif [ "$VFS_ANY" -gt 0 ]; then
    echo "WRONG-IMAGE — the artifact carries NONE of the arming literal (control $CTL fires, so the zero is real) yet $VFS_ANY '[vfs] root' line(s) are on the wire. The kernel.elf handed to this scorer is NOT the kernel that booted. Re-derive the arming field from the FLASHED image and re-score; do not reconcile this by hand."
  else
    echo "NOT-EXERCISED — the flashed image does not contain the bind emitter (arming literal=0 beside a control of $CTL). The absence of the root lines on the wire is STRUCTURAL: neither a pass nor a failure. Do not file it green, do not suppress the rule — rebuild with the emitter aboard, or say in the flight record that render11's root family did not fly."
  fi)"

# ═════════════════════════════════════════════════════════════════════════════════════════════════
# LEG (a) — LOADER-SERIAL. NOT gated by the kernel arming field: this line is printed by the
# BOOTLOADER crate (a different binary, BOOTAA64.EFI), so a kernel built without the bind emitter
# still produces it. Its control is therefore the loader's own presence on the wire.
# ═════════════════════════════════════════════════════════════════════════════════════════════════
leg LOADER-SERIAL "LOADER-SERIAL: 'boot volume FAT serial 0x…' lines=$LD_N (distinct serials=$LD_D, malformed=$LD_MAL, value=0x$LD_LAST) || CONTROL loader lines on wire=$C_LOADER" "$(
  if [ "$C_LOADER" -eq 0 ]; then
    echo "NOT-SCORED — ZERO 'crates/bootloader/src/main.rs@' lines on this wire: the capture window does not contain the loader at all (scope it from the loader's first line, FLIGHT-render11.md §4). A zero without a passing control is not evidence."
  elif [ "$LD_MAL" -gt 0 ] && [ "$LD_N" -eq 0 ]; then
    echo "FAIL — the loader printed a 'boot volume FAT serial' line $LD_MAL time(s) and NOT ONE carries a parseable 0x hex value. The contract's half of the identity is unreadable: $LD_LINE"
  elif [ "$LD_N" -eq 0 ]; then
    echo "ABSENT — the loader spoke on this wire ($C_LOADER line(s)) and NEVER printed 'boot volume FAT serial'. Under the render11 contract the loader always names the volume it loaded from; a silent loader means the flashed BOOTAA64.EFI predates BOOTROOT, or the volume it read had no BS_BootSig==0x29 and the sentinel path is silent. DEFECT."
  elif [ "$LD_D" -gt 1 ]; then
    echo "FAIL — $LD_D DISTINCT loader serials in one capture window ($LD_N line(s)). This window spans more than one boot; every downstream comparison is meaningless until it is re-scoped to a single boot. Re-cut the window and re-score."
  else
    echo "PASS LOADER-SERIAL (the loader named its boot volume: serial=0x$LD_LAST, $LD_N line(s), one distinct value)"
  fi)"

# ═════════════════════════════════════════════════════════════════════════════════════════════════
# LEG (b) — ROOT-BIND. THE leg. Gated on ARMING (unarmed → NOT-EXERCISED, per the brief) and on the
# kernel having spoken at all. The contract promises EXACTLY ONE of the bind line / the NONE line on
# the first mount-table build, so "the kernel ran and printed neither" is ABSENT and is a defect —
# the control does not have to prove the mount table was reached, only that the kernel was alive.
# ═════════════════════════════════════════════════════════════════════════════════════════════════
leg ROOT-BIND "ROOT-BIND: '[vfs] root = boot volume serial=' lines=$B_N (distinct=$B_D, malformed=$B_MAL, value=0x$B_LAST) vs loader 0x$LD_LAST || CONTROLS kernel lines=$C_KERNEL, arming=$ARMED, NONE lines=$NN_N" "$(
  if [ "$ARMED" != armed ]; then
    echo "NOT-EXERCISED — the flashed image does not carry the bind emitter (see ARMING). Neither a pass nor a failure; the leg was not exercised by the image that flew."
  elif [ "$C_KERNEL" -eq 0 ]; then
    echo "NOT-SCORED — the kernel never spoke on this wire (0 hits across SCHED:/[menubar]/TEGRA-/[orinrender]/[dock]). The capture is loader-only or is not a capture. A zero without a passing control is not evidence."
  elif [ "$B_N" -eq 0 ] && [ "$NN_N" -gt 0 ]; then
    echo "NOTE — no bind line, but the kernel DID answer the question, with the NONE line. The contract's exclusive-or held, so this row is NOT a second independent red: ROOT-NONE carries this capture's verdict and is red there. A NOTE here and a FAIL there is the honest shape; two reds would double-count one defect."
  elif [ "$B_N" -eq 0 ]; then
    echo "ABSENT — ARMED image, the kernel spoke ($C_KERNEL line(s)), and it printed NEITHER '[vfs] root = boot volume' NOR '[vfs] root -> NONE'. The contract says the first mount-table build prints exactly one of them, so either the mount table was never built (boot died above it — read the tail) or the emitter is unreachable in this build. DEFECT, not a pass."
  elif [ "$B_MAL" -gt 0 ] && [ "$B_LAST" = "-" ]; then
    echo "FAIL — the bind line printed $B_N time(s) with NO parseable serial= value. The bind cannot be tied to the loader's volume: $B_LINE"
  elif [ "$B_D" -gt 1 ] || [ "$LD_D" -gt 1 ]; then
    echo "FAIL — more than one distinct serial in this window (bind distinct=$B_D, loader distinct=$LD_D). The window spans two boots or root was bound twice with different answers; re-scope and re-score before reading anything into the comparison."
  elif [ "$LD_N" -eq 0 ]; then
    echo "NOT-SCORED — the bind line is present (serial=0x$B_LAST) but the loader half is NOT on this wire, so there is nothing to compare it to. See LOADER-SERIAL. A one-sided identity is not an identity."
  elif [ "$B_LAST" != "$LD_LAST" ]; then
    echo "FAIL — MISMATCH: the loader loaded from 0x$LD_LAST and the kernel rooted on 0x$B_LAST. The kernel picked a volume that is NOT the one it was loaded from; this is precisely the defect the contract exists to catch. bind: $B_LINE"
  else
    echo "PASS ROOT-BIND (root bound to the boot volume: loader 0x$LD_LAST == kernel 0x$B_LAST, source=$S_LAST, $B_N line(s), one distinct value each side)"
  fi)"

# ═════════════════════════════════════════════════════════════════════════════════════════════════
# LEG (c) — ROOT-NONE. Presence of the NONE line is a FAIL and the verdict NAMES THE REASON.
# ═════════════════════════════════════════════════════════════════════════════════════════════════
leg ROOT-NONE "ROOT-NONE: '[vfs] root -> NONE' lines=$NN_N (reason=$NN_REASON serial=0x$NN_SERIAL) || CONTROLS kernel lines=$C_KERNEL, arming=$ARMED" "$(
  if [ "$ARMED" != armed ]; then
    echo "NOT-EXERCISED — the flashed image does not carry the root-choice emitter (see ARMING); its silence is structural."
  elif [ "$C_KERNEL" -eq 0 ]; then
    echo "NOT-SCORED — the kernel never spoke on this wire. A zero NONE-count without a passing control is not evidence that root bound."
  elif [ "$NN_N" -gt 0 ]; then
    echo "FAIL — root did NOT bind: reason=$NN_REASON. $(case "$NN_REASON" in no-disk-enumerated) echo 'The kernel enumerated NO block device at all — a driver/enumeration gap on this image, not a policy gap. Read the disks= field.';; kernel-not-found-on-any-volume) echo 'Disks were enumerated and walked but NO file matched the running kernel .text window (candidates= says how many were compared; window_off/window_len say what was compared). Either the boot medium is not among the enumerated disks, or the window is not stable on this boot path (BSS/relocation) — the disks= and candidates= fields discriminate.';; walk-cap-hit) echo 'The directory walk hit its depth/entry cap before finishing — the medium may hold the kernel beyond the cap; a bound, not a verdict.';; multiple-kernels) echo 'MORE THAN ONE file matched the running kernel (matches= says how many, the line lists source:path) — two media carry this kernel; the kernel REFUSES to pick. Remove the spare medium and re-fly; not a kernel defect.';; *) echo 'UNDOCUMENTED REASON — the v4 contract lists no-disk-enumerated, kernel-not-found-on-any-volume, walk-cap-hit, multiple-kernels. Re-read the emit site before scoring anything from this row.';; esac) line: $NN_LINE"
  else
    echo "PASS ROOT-NONE (no [vfs] root -> NONE line on a wire where the kernel spoke $C_KERNEL time(s) — root was not refused)"
  fi)"

# ═════════════════════════════════════════════════════════════════════════════════════════════════
# LEG (d) — OLD-BIND-ABSENT. The `[sdmmc] root …` family is DELETED by the contract. It is checked
# on BOTH the wire and the artifact: the wire proves what ran, the artifact proves what was flashed,
# and the artifact fires even on a boot that died before the mount table. NOT gated on arming — a
# retired-family hit is a WRONG-IMAGE finding whether or not the new emitter is aboard.
# ═════════════════════════════════════════════════════════════════════════════════════════════════
leg OLD-BIND-ABSENT "OLD-BIND-ABSENT: retired '[sdmmc] root' lines on wire=$OLD_N, in artifact=$OLDART || CONTROLS kernel lines=$C_KERNEL, artifact control=$CTL" "$(
  if [ "$OLD_N" -gt 0 ]; then
    echo "WRONG-IMAGE — the retired '[sdmmc] root' family printed $OLD_N time(s). BOOTROOT deletes it; an image that still emits it is not a render11 image, and every root verdict above was scored against the wrong contract. Check what was flashed against the FLIGHTID sha before reading anything else. line: $OLD_LINE"
  elif [ "$OLDART" -gt 0 ]; then
    echo "WRONG-IMAGE — the wire is clean but the FLASHED ARTIFACT still contains the retired '[sdmmc] root' literal $OLDART time(s) beside a control of $CTL. The image predates the deletion; the wire is silent only because that path was not reached this boot. Do not score this flight as render11."
  elif [ "$C_KERNEL" -eq 0 ]; then
    echo "NOT-SCORED — the wire zero is real but the kernel never spoke on it, so the wire half of this leg proves nothing. The ARTIFACT half is clean ($OLDART hits beside a control of $CTL); re-scope the window and re-run to close the wire half."
  else
    echo "PASS OLD-BIND-ABSENT (retired family absent from both halves: 0 lines on a wire where the kernel spoke $C_KERNEL time(s), 0 literals in an artifact whose control fires $CTL time(s))"
  fi)"

# ═════════════════════════════════════════════════════════════════════════════════════════════════
# LEG (f) — SOURCE-VOCAB. Informational: the contract's own source vocabulary, checked as a set, so
# a fifth backend reaching the wire is SEEN rather than silently accepted. Never a red — an
# unrecognised source is a scorer-behind-the-tree signal, not a flight failure.
# ═════════════════════════════════════════════════════════════════════════════════════════════════
leg SOURCE-VOCAB "SOURCE-VOCAB: source token(s) seen=$S_LIST (distinct=$S_D, lines missing a source field=$S_MAL) || CONTROL bind lines=$B_N, arming=$ARMED" "$(
  if [ "$ARMED" != armed ] || [ "$B_N" -eq 0 ]; then
    echo "NOT-EXERCISED — no bind line on this wire to carry a source token (arming=$ARMED, bind lines=$B_N)."
  elif [ "$S_MAL" -gt 0 ]; then
    echo "NOTE — $S_MAL bind line(s) carry no source= field at all. The contract prints one; the emitter or this parser is behind the other. Read the emit site."
  elif [ "$S_D" -gt 1 ]; then
    echo "NOTE — $S_D DISTINCT source tokens on one wire ($S_LIST). Root was answered more than once with different backends; re-scope the window."
  else
    case "$S_LAST" in
      global|usb|sdhc|tegra-sd) echo "PASS SOURCE-VOCAB (source=$S_LAST, in the contract's set global|usb|sdhc|tegra-sd)" ;;
      *) echo "NOTE — source=$S_LAST is NOT in the contract's set (global|usb|sdhc|tegra-sd). Either a backend was added since this scorer was cut, or the token is being read out of a line this parser does not understand. Not a flight failure; a signal to re-read the emit site before the next round." ;;
    esac
  fi)"

echo
echo "== render11 verdict table =="
printf '  %-17s %-5s %s\n' "PREDICATE" "CLASS" "VERDICT"
printf '  %-17s %-5s %s\n' "-----------------" "-----" "-------"
printf '%s' "$ROWS" | awk -F'|' 'NF>=3{printf "  %-17s %-5s %s\n",$1,$2,$3}'
echo
echo "== $RED red, $GREEN green, $NOTES note(s), $NEX not-exercised =="
[ "$NEX" -eq 0 ] || echo "== n-ex rows are NEITHER passes NOR failures: the witness was not present in the image that flew. Do not file them green and do not suppress the rule. =="
[ "$RED" -eq 0 ] || echo "== RED: the flown image does NOT satisfy the render11 root contract. Do not file this flight green. =="
echo "-- end scorer11 --"
if   [ "$RED" -gt 0 ]; then exit 1
elif [ "$NEX" -gt 0 ]; then exit 3     # EXIT=3 (documented extension; collapse to 2 if required)
else exit 0; fi
