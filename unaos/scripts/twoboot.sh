#!/bin/bash
# twoboot.sh — TWOBOOT (rmbp-ledger B526): the two-boot fold leg. Boot 1 on a fresh volume to the desktop, boot 2 of the
# same image on the same volume to the login screen.
#
# WHY. Flight 29 (FLIGHT29.md §2–3) found two defects no fold leg could reach: SESSIONWEDGE (boot 1 created the first
# user and wedged after `[login] desktop bare` — no `[desktop] built`, no services) and RESOLVEHANG (every LATER boot of
# that card, with a user in the store, hung at "Resolving the session", silently). `foldleg` is a compile; `./arroyo
# test` is one boot, no user, the QEMU lane's knobs; `bootleg` is one boot of an older feature list. "A kernel build AND
# a two-boot metal-shaped leg in every fold" (§3) — this is the second half.
#
# WHAT IT RUNS.
#   knobs  `scripts/image-line.env` (the image's line, FOLDBUILD B517) minus/plus `scripts/twoboot-knobs.env` (one
#          list: each dropped metal-only knob and each fixture addition, with its reason). The caller's UNAOS_* are
#          cleared first (the leg boots THE LINE, not the shell) except UNAOS_QEMU_MACHINE and UNAOS_TWOBOOT_*.
#          No SELFBUILD/LLD knob on the line or in the list (asserted, NO VERDICT rc 2), and the toolchain staging
#          `test_x86_64` reaches through build_user_vug_x86 is NOT entered (arroyo's `twoboot)` case stubs
#          build_selfbuild_x86; asserted below off target/selfbuild's mtimes, as foldleg asserts it).
#          `twoboot` is not in arroyo's battery-arming case, so `tests-at-boot` and `census` stay OFF — the metal shape.
#   build  ONCE, through the builder, exactly as `./arroyo test` does: the boot is `test_x86_64` itself (arroyo's
#          `twoboot) --boot <n>` arm), with X86_TEST_SPEC = this leg's spec, so FASTTEST's `qemu_test_wall` ends the run
#          at the spec's COMPLETE marker + grace. Boot 2 re-enters the same path; every cargo build is then a
#          fingerprint no-op (same tree, same env) and the leg proves it: kernel.elf's sha256 before and after boot 2.
#   boot 1 FRESH volumes: target/sdcard.img is deleted first, so the builder stages a new card, and the DEFAULTMEDIUM
#          stick is rebuilt fresh as every run. BOTH matter: on the first run under this line the store LOADED from the
#          card (`[boot] step=users-load end … via=sdhc users=0`, at 2.4 s, before the USB stick enumerates) and the
#          first user's FLUSH landed on the stick (`USERS   DAT` on the stick copy, none on the card) — the ladder's
#          rung moved between load and write. The QMP typist (scripts/qmp_type.py's own client, imported — never a second
#          QMP path) waits for `[login] installer: first-user form open`, then types `una`, Tab, `correct-horse`, Tab,
#          `correct-horse`, Enter: the first-user form, the path flight 29's boot 1 took at the keyboard.
#          Spec scripts/specs/x86-twoboot-1.spec: COMPLETE `[boot] services up first=`, `[desktop] built` required.
#   keep   QEMU is dead and waited by then. The card stays in place (the builder leaves a staged sdcard.img alone: "a
#          previous boot's record survives"); the stick is copied to target/twoboot-vol.img, since the builder rebuilds
#          builder/usb-boot.img every run. The leg then reads both images host-side for the store's directory entry
#          (`USERS   DAT`) and prints `store_on_volume=card|stick|none` — boot 1's writes on the medium, not on the wire.
#   boot 2 UNAOS_FATIMG=target/twoboot-vol.img + the same card — the SAME volumes, no reformat. A boot 2 whose
#          users-load reads `users=0` is not a second boot of a volume with a user: boot2=no-user@users-load, FAIL. Spec scripts/specs/x86-twoboot-2.spec:
#          `[boot] step=stage-resolve end`, the `store-has-users` stage, COMPLETE the login window's open line.
#          Run only when boot 1 wrote the user (`create-user user=una`); else boot2=not-run.
#
# KNOWN DIFFERENCES FROM THE METAL BOOT (said, not hidden): QEMU's users store is the emulated SD card's FAT16
# (`via=sdhc`, `seat store=fat:/USERS.DAT`), the bench card's is SDHC/UnaFS `/system`; the builder re-stages the ESP directory each run, so boot-1 writes to the ESP (not to the
# stick) do not reach boot 2; no LIB/TCC/rustc payload on the ESP (the selfbuild staging is not entered).
#
# usage: ./arroyo twoboot [--judge <boot1 log> [<boot2 log>]]      (= twoboot.sh <unaos dir> …)
#        `--judge` reads captured logs only (no build, no QEMU).
# env:   UNAOS_TWOBOOT_CAP1 / _CAP2 (s, default 60 each: a hang pays its spec's wall, bounded), UNAOS_TWOBOOT_GRACE (s,
#        default 5: the FASTTEST grace after each COMPLETE), UNAOS_QEMU_MACHINE (auto: the newest pc-q35 this QEMU has
#        when it lacks the builder's pinned pc-q35-10.0).
# exit:  0 PASS · 1 FAIL · 2 NO VERDICT (the line or the list refused, SELFBUILD staging ran, no capture) · 127 no qemu/arroyo
# wire:  TWOBOOT: boot1=<desktop-built|wedge@"<last line>"> boot2=<console|hang@<last step>|no-user@users-load|not-run(<why>)> wall_s=<n> -> PASS|FAIL
#        (wall_s = the two QEMU walls, the leg's own cost; the one build is reported beside it as build_s.)
set -u
U="${1:?usage: twoboot.sh <unaos dir> [--judge <boot1 log> [<boot2 log>]]}"; shift
U="$(cd "$U" && pwd)"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CAP1="${UNAOS_TWOBOOT_CAP1:-60}" CAP2="${UNAOS_TWOBOOT_CAP2:-60}" GRACE="${UNAOS_TWOBOOT_GRACE:-5}"

# --- the judges (awk, never bare grep — LAWS) -------------------------------------------------------------------------
# last_line <log>: the last line with printable text, control bytes stripped, cut to 160.
last_line() { awk '{ gsub(/[\r\001-\010\013-\037\177]/, "") } /[^ \t]/ { l = $0 } END { print substr(l, 1, 160) }' "$1" 2>/dev/null; }
# boot1 <log> -> desktop-built | wedge@"<last line>"
judge1() {
    local log="$1"
    [ -s "$log" ] || { echo 'wedge@"<no serial bytes>"'; return 1; }
    if awk 'index($0,"[desktop] built at=") { d = 1 } index($0,"[boot] services up first=") { s = 1 } END { exit !(d && s) }' "$log"; then
        echo "desktop-built"; return 0
    fi
    echo "wedge@\"$(last_line "$log" | sed 's/"/'"'"'/g')\""; return 1
}
# boot2 <log> -> console | hang@<last step>  (the last `[boot] step=<name> begin` with no `end`, else the last line)
judge2() {
    local log="$1" st
    [ -s "$log" ] || { echo 'hang@"<no serial bytes>"'; return 1; }
    if awk 'index($0,"[login] screen open window=") && index($0,"power_row=1") { w = 1 } index($0,"why=store-has-users -> coherent") { s = 1 } END { exit !(w && s) }' "$log"; then
        echo "console"; return 0
    fi
    if awk 'index($0,"[boot] step=users-load end") && index($0," users=0") { f = 1 } END { exit !f }' "$log"; then
        echo "no-user@users-load"; return 1   # the store boot 1 wrote is not on the volume boot 2 read: no second boot happened
    fi
    st="$(awk 'match($0, /\[boot\] step=[A-Za-z0-9_-]+ (begin|end)/) { split(substr($0, RSTART + 12, RLENGTH - 12), a, " "); open[a[1]] = (a[2] == "begin"); if (a[2] == "begin") last = a[1] }
               END { if (last != "" && open[last]) print last }' "$log")"
    if [ -n "$st" ]; then echo "hang@${st}"; else echo "hang@\"$(last_line "$log" | sed 's/"/'"'"'/g')\""; fi
    return 1
}
verdict() { # b1 b2 wall_s rc1 rc2
    local res=FAIL; [ "$4" -eq 0 ] && [ "$5" -eq 0 ] && res=PASS
    echo "TWOBOOT: boot1=$1 boot2=$2 wall_s=$3 -> ${res}"
    [ "$res" = PASS ]
}

if [ "${1:-}" = "--judge" ]; then
    l1="${2:?--judge needs the boot-1 log}" l2="${3:-}"
    b1="$(judge1 "$l1")"; r1=$?
    if [ -n "$l2" ]; then b2="$(judge2 "$l2")"; r2=$?; else b2="not-run(no-log)"; r2=1; fi
    [ "$r1" -ne 0 ] && echo "TWOBOOT: boot-1 last line: $(last_line "$l1")"
    [ "$r2" -ne 0 ] && [ -n "$l2" ] && echo "TWOBOOT: boot-2 last line: $(last_line "$l2")"
    verdict "$b1" "$b2" "-" "$r1" "$r2"; exit $?
fi

[ -x "$U/arroyo" ] && [ -f "$U/builder/src/main.rs" ] || { echo "TWOBOOT: ${U} is not an unaos dir (no arroyo/builder) -> MISSING rc=127"; exit 127; }
command -v qemu-system-x86_64 >/dev/null 2>&1 || { echo "TWOBOOT: qemu-system-x86_64 not installed -> MISSING rc=127"; exit 127; }
LINE="$U/scripts/image-line.env" LIST="$HERE/twoboot-knobs.env"
[ -f "$LINE" ] || { echo "TWOBOOT: no image line at ${LINE} -> NO VERDICT rc=2"; exit 2; }
[ -f "$LIST" ] || { echo "TWOBOOT: no knob list at ${LIST} -> NO VERDICT rc=2"; exit 2; }

# --- the knob set: the line, minus the list's drops, plus its adds ----------------------------------------------------
if awk '/^export UNAOS_(SELFBUILD|LLD_LNX)/ { f = 1 } END { exit !f }' "$LINE" \
   || awk '/^(drop|add)[ \t]+UNAOS_(SELFBUILD|LLD_LNX)/ { f = 1 } END { exit !f }' "$LIST"; then
    echo "TWOBOOT: a SELFBUILD/LLD knob is armed (image line or twoboot-knobs.env) — this leg never stages the toolchain -> NO VERDICT rc=2"; exit 2
fi
bad="$(awk '!/^[ \t]*(#|$)/ && !/^drop[ \t]+UNAOS_[A-Z0-9_]+[ \t]*(#|$)/ && !/^add[ \t]+UNAOS_[A-Z0-9_]+=[^ \t#]*[ \t]*(#|$)/ { print NR ": " $0; exit }' "$LIST")"
[ -z "$bad" ] || { echo "TWOBOOT: twoboot-knobs.env line ${bad:0:100} is not 'drop UNAOS_X # why' / 'add UNAOS_X=v # why' -> NO VERDICT rc=2"; exit 2; }
declare -A drop=()
for k in $(awk '$1 == "drop" { print $2 }' "$LIST"); do drop[$k]=1; done
cleared=0
for v in $(compgen -e | awk '/^UNAOS_/'); do
    case "$v" in UNAOS_QEMU_MACHINE|UNAOS_TWOBOOT_*) continue ;; esac
    unset "$v"; cleared=$((cleared+1))
done
nk=0 nd=0 added=""
while IFS= read -r raw || [ -n "$raw" ]; do
    case "$raw" in export\ UNAOS_*) ;; *) continue ;; esac
    k="${raw#export }"; k="${k%%=*}"
    if [ -n "${drop[$k]:-}" ]; then nd=$((nd+1)); continue; fi
    if [[ "$raw" =~ :\? ]]; then
        echo "TWOBOOT: ${k} is a bench-local [media] value on the line and twoboot-knobs.env does not drop it — name it there, with why -> NO VERDICT rc=2"; exit 2
    fi
    eval "$raw" || { echo "TWOBOOT: image-line.env: cannot evaluate: ${raw:0:80} -> NO VERDICT rc=2"; exit 2; }
    nk=$((nk+1))
done < "$LINE"
for d in "${!drop[@]}"; do
    awk -v k="$d" 'index($0, "export " k "=") == 1 { f = 1 } END { exit !f }' "$LINE" \
        || { echo "TWOBOOT: twoboot-knobs.env drops ${d}, which image-line.env does not carry — a stale drop -> NO VERDICT rc=2"; exit 2; }
done
while read -r _ kv _; do export "$kv"; added="${added}${added:+,}${kv%%=*}"; done < <(awk '$1 == "add" { print }' "$LIST")
[ "${UNAOS_LOGIN:-}" = "1" ] || { echo "TWOBOOT: UNAOS_LOGIN is not 1 after the derivation — the leg has no login path -> NO VERDICT rc=2"; exit 2; }
for v in $(compgen -e | awk '/^UNAOS_(SELFBUILD|LLD_LNX)/'); do echo "TWOBOOT: ${v} is set after the derivation -> NO VERDICT rc=2"; exit 2; done
# The machine type: the builder pins pc-q35-10.0; a QEMU that predates it (the cloud's 8.2) gets its own newest q35.
if [ -z "${UNAOS_QEMU_MACHINE:-}" ] && ! qemu-system-x86_64 -machine help 2>/dev/null | awk '$1 == "pc-q35-10.0" { f = 1 } END { exit !f }'; then
    m="$(qemu-system-x86_64 -machine help 2>/dev/null | awk '$1 ~ /^pc-q35-[0-9]+\.[0-9]+$/ { print $1 }' | sort -t- -k3 -V | tail -1)"
    [ -n "$m" ] && export UNAOS_QEMU_MACHINE="$m"
fi
export UNAOS_QEMU_GRACE="$GRACE"
_sha="$(git -C "$U" log -1 --format=%h -- "$LINE" 2>/dev/null)"
{ [ -n "$_sha" ] && git -C "$U" diff --quiet HEAD -- "$LINE" 2>/dev/null; } || _sha="uncommitted"
echo "TWOBOOT: line=image-line.env @${_sha} knobs=${nk} dropped=${nd}($(printf '%s,' "${!drop[@]}" | sed 's/,$//')) added=${added} caller_knobs_cleared=${cleared} machine=${UNAOS_QEMU_MACHINE:-pc-q35-10.0(builder pin)} caps=${CAP1}/${CAP2}s grace=${GRACE}s"

T="$U/target" LOG="$U/target/serial.log" VOL="$U/target/twoboot-vol.img"
mkdir -p "$T"
rm -f "$T/sdcard.img"   # a FRESH card: the builder stages a new one (it keeps a staged image across runs, which boot 2 relies on)
rm -f "$T"/twoboot-1.log "$T"/twoboot-2.log "$T"/twoboot-1.log.run "$T"/twoboot-2.log.run "$T"/twoboot-typist.log "$VOL"
# The QMP socket the typist rides: the first free port from 4470 (arroyo's own idiom; 4464.. are its battery verbs').
port="${UNAOS_TWOBOOT_QMP_PORT:-4470}"
while lsof -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1; do port=$((port + 1)); done
export UNAOS_QEMU_EXTRA="-qmp tcp:127.0.0.1:${port},server,nowait"
stamp="$(mktemp "$T/.twoboot.XXXXXX")"
t0=$(date +%s)

# --- boot 1: the fresh volume, the typist on the first-user form -------------------------------------------------------
# The typist: qmp_type.py's Qmp/connect/wait_for_marker/qcode, imported by path (the module's main() is guarded), plus
# the two keys its --text cannot spell (Tab, Enter). Its connect waits out the build; its marker wait is boot 1's cap.
python3 - "$HERE" "$port" "$LOG" "$CAP1" > "$T/twoboot-typist.log" 2>&1 <<'PY' &
import sys, time
sys.path.insert(0, sys.argv[1])
import qmp_type as q
port, log, cap = int(sys.argv[2]), sys.argv[3], float(sys.argv[4])
qmp = q.Qmp(q.connect("127.0.0.1", port, 7200))
qmp._read_obj()
r = qmp.execute("qmp_capabilities")
if "error" in r:
    raise SystemExit(f"[typist] qmp_capabilities refused: {r['error']} — nothing typed")
t0 = time.time()
mark = "[login] installer: first-user form open"
print(f"[typist] connected :{port}; waiting up to {cap:.0f}s for {mark!r}", flush=True)
if not q.wait_for_marker(log, mark, cap):
    raise SystemExit(f"[typist] {mark!r} not seen in {cap:.0f}s — nothing typed")
print(f"[typist] form open at +{time.time() - t0:.1f}s; settling 4s", flush=True)
time.sleep(4)
keys = [q.qcode(c) for c in "una"] + ["tab"] + [q.qcode(c) for c in "correct-horse"] + ["tab"] \
     + [q.qcode(c) for c in "correct-horse"] + ["ret"]
for k in keys:
    r = qmp.execute("send-key", {"keys": [{"type": "qcode", "data": k}]})
    if "error" in r:
        raise SystemExit(f"[typist] send-key {k!r} refused: {r['error']}")
    time.sleep(0.12)
print(f"[typist] typed name + Tab + password + Tab + retype + Enter ({len(keys)} keys) at +{time.time() - t0:.1f}s", flush=True)
ok = q.wait_for_marker(log, "[login] key taken by the screen", 10)
print(f"[typist] the screen {'took' if ok else 'did NOT report taking'} a key", flush=True)
PY
typist=$!
echo "TWOBOOT: boot 1 — build + boot on a fresh volume, cap ${CAP1}s (output: target/twoboot-1.out; typist: target/twoboot-typist.log, QMP :${port})"
tb=$(date +%s)
( cd "$U" && ./arroyo twoboot --boot 1 "$CAP1" ) > "$T/twoboot-1.out" 2>&1
rc1run=$?
wait "$typist" 2>/dev/null; trc=$?
[ -f "$LOG" ] && cp "$LOG" "$T/twoboot-1.log"
[ -f "$LOG.run" ] && cp "$LOG.run" "$T/twoboot-1.log.run"
awk '/^\[typist\]/ { print "TWOBOOT:   " $0 }' "$T/twoboot-typist.log"
qw1="$(awk -F= '$1 == "qemu_wall" { print $2 }' "$T/twoboot-1.log.run" 2>/dev/null)"
vw1="$(awk -F= '$1 == "wall" { print $2 }' "$T/twoboot-1.log.run" 2>/dev/null)"
[ -n "$qw1" ] && [ "$qw1" != "-" ] || qw1="$CAP1"   # no waiter stamp (mode=full): the cap was paid
elf="$T/x86_64_esp/kernel.elf"
sha1="$(sha256sum "$elf" 2>/dev/null | cut -c1-16)"
banner="$(sed 's/\x1b\[[0-9;]*m//g' "$T/twoboot-1.out" | awk 'index($0,"⚡ kernel features:") { print; exit }')"
echo "TWOBOOT: boot 1 arroyo rc=${rc1run} typist rc=${trc} qemu_wall=${qw1:--} verb_wall=${vw1:--} kernel.elf=${sha1:-none} ${banner}"
if [ ! -s "$T/twoboot-1.log" ]; then
    awk '/^error(\[E[0-9]+\])?:/ { n++; if (n <= 8) print "TWOBOOT:   " $0 }' "$T/twoboot-1.out"
    echo "TWOBOOT: boot 1 left no serial capture (the build or QEMU did not start; target/twoboot-1.out) -> NO VERDICT rc=2"; rm -f "$stamp"; exit 2
fi
if [ -d "$T/selfbuild" ] && [ -n "$(find "$T/selfbuild" -newer "$stamp" -print -quit 2>/dev/null)" ]; then
    echo "TWOBOOT: SELFBUILD staging ran (target/selfbuild touched) — the stub did not hold -> NO VERDICT rc=2"; rm -f "$stamp"; exit 2
fi
b1="$(judge1 "$T/twoboot-1.log")"; r1=$?
# Boot 1's store, read off the media host-side: the 8.3 directory entry `USERS   DAT` (USERS.DAT, fs/users.rs).
son=""
for m in "card:$T/sdcard.img" "stick:$U/builder/usb-boot.img"; do
    LC_ALL=C grep -a -q -F 'USERS   DAT' "${m#*:}" 2>/dev/null && son="${son}${son:+,}${m%%:*}"
done
echo "TWOBOOT: store_on_volume=${son:-none} (USERS.DAT's directory entry on target/sdcard.img / builder/usb-boot.img after boot 1)"

# --- keep the volume; boot 2 on it -------------------------------------------------------------------------------------
b2="" r2=1 qw2="" vw2=""
if ! awk 'index($0,"[login] installer: create-user user=una ") { f = 1 } END { exit !f }' "$T/twoboot-1.log"; then
    b2="not-run(boot1-wrote-no-user)"
elif ! cp "$U/builder/usb-boot.img" "$VOL"; then
    b2="not-run(volume-copy-failed)"
else
    echo "TWOBOOT: boot 2 — the same image on the same volume (target/twoboot-vol.img), cap ${CAP2}s (output: target/twoboot-2.out)"
    ( cd "$U" && UNAOS_FATIMG="$VOL" ./arroyo twoboot --boot 2 "$CAP2" ) > "$T/twoboot-2.out" 2>&1
    rc2run=$?
    [ -f "$LOG" ] && cp "$LOG" "$T/twoboot-2.log"
    [ -f "$LOG.run" ] && cp "$LOG.run" "$T/twoboot-2.log.run"
    qw2="$(awk -F= '$1 == "qemu_wall" { print $2 }' "$T/twoboot-2.log.run" 2>/dev/null)"
    vw2="$(awk -F= '$1 == "wall" { print $2 }' "$T/twoboot-2.log.run" 2>/dev/null)"
    [ -n "$qw2" ] && [ "$qw2" != "-" ] || qw2="$CAP2"
    sha2="$(sha256sum "$elf" 2>/dev/null | cut -c1-16)"
    same=0; [ -n "$sha1" ] && [ "$sha1" = "$sha2" ] && same=1
    echo "TWOBOOT: boot 2 arroyo rc=${rc2run} qemu_wall=${qw2:--} verb_wall=${vw2:--} kernel.elf=${sha2:-none} image_unchanged=${same}"
    if [ "$same" != 1 ]; then
        echo "TWOBOOT: boot 2 booted a DIFFERENT kernel.elf than boot 1 (${sha1:-none} -> ${sha2:-none}) — not the same image -> NO VERDICT rc=2"; rm -f "$stamp"; exit 2
    fi
    if [ -s "$T/twoboot-2.log" ]; then b2="$(judge2 "$T/twoboot-2.log")"; r2=$?; else b2='hang@"<no serial bytes>"'; fi
fi
rm -f "$stamp"
t1=$(date +%s)
wall="$(awk -v a="${qw1:-0}" -v b="${qw2:-0}" 'BEGIN { a += 0; b += 0; printf "%.0f", a + b }')"
echo "TWOBOOT: build_s=$(awk -v v="${vw1:-0}" -v q="${qw1:-0}" 'BEGIN { printf "%.0f", v - q }') total_s=$((t1 - t0)) logs=target/twoboot-1.log,target/twoboot-2.log volume=target/twoboot-vol.img"
[ "$r1" -ne 0 ] && echo "TWOBOOT: boot-1 last line: $(last_line "$T/twoboot-1.log")"
[ "$r2" -ne 0 ] && [ -s "$T/twoboot-2.log" ] && echo "TWOBOOT: boot-2 last line: $(last_line "$T/twoboot-2.log")"
verdict "$b1" "$b2" "$wall" "$r1" "$r2"
