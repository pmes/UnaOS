#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────────────────────────
# media-writer.sh — the bench's boot-media writer (any round, any image).
#
# HISTORY: born as scratch/orin20/cardready/load-card10.sh (orin 20 / CARDREADY, the render10
# writer). Moved here and renamed by orin 23 on 2026-09-08 on two Peter rulings the same hour:
# "that really is a dumb file name" (a round number baked into a tool every round uses) and
# "anything in ~/unaos-bench/scratch/ could be deleted in 10 seconds as it is temporary use only"
# (a durable tool cannot live in a round's scratch). The scratch copy is left as the record.
#
# SUPERSEDES (does not delete) $HOME/unaos-bench/scratch/orin11/load-card.sh, which staged render9.
# That script is kept: its sha-verify step is good and is carried forward here verbatim in spirit.
# What it could not do is say WHICH MEDIUM it wrote, and it is that gap the render9 flight fell into.
#
# WHAT WENT WRONG ON render9 (the reason this file exists)
#   The Orin has TWO media in play at once and render9's tooling only ever addressed ONE of them:
#     * the BOOT medium — an SD card in a USB card reader ('USB3.0 CRW   -SD'), plugged into a
#       USB-A port, which the UEFI loader read BOOTAA64.EFI + kernel.elf from.  Its identity reaches
#       the wire as   crates/bootloader/src/main.rs@961: boot volume FAT serial 0xde001a13
#     * the ROOT medium — whatever card is seated in the Orin's NATIVE SD slot, which
#       `sdmmc_root_bind` (arch/aarch64/sdmmc_tegra.rs) binds to `/`, `/boot` AND `/apps`.
#       Its identity reaches the wire as
#         [sdmmc] root mount source=tegra-sd ... -> OK label="UNAOS-PI" vol_id=0xabfbdefa
#   On render9 those were DIFFERENT PHYSICAL CARDS. The root was a stale Pi card. The kernel printed
#   both identities, four lines apart, and nothing compared them.
#
#   THIS SCRIPT WRITES ONE MEDIUM AND SAYS SO. It names its target explicitly, refuses a stale label,
#   refuses an unexpected geometry, and emits a FLIGHTID card naming the exact lines the flight log
#   must show — so the boot is scored by the image that LOADED, not by the image someone WROTE.
#
# USAGE
#   media-writer.sh --src <staged-dir-name> --target <dev-or-image> [options]
#       (dry run: every read-only check runs, nothing is written)
#   media-writer.sh --src <staged-dir-name> --target <dev-or-image> --write
#       (writes, then verifies by read-back)
#   media-writer.sh --selftest [dir]
#       (builds file-backed fixtures and proves EVERY check fires in BOTH polarities)
#
#   media-writer.sh --image <card.img> --target <dev-or-image> [--write --erase-target]
#       WHOLE-MEDIUM mode (UNAFSROOT, orin 24). Lays a CARD IMAGE down over the target: partition
#       table, FAT volume and native UnaFS volume in one write. This is the ONE-TIME step that gives
#       the card a native volume for `/` to root on; ordinary staging (--src) runs unchanged after it.
#       ⚠ ERASES THE WHOLE TARGET, which is why it needs BOTH --write and --erase-target.
#       Identity checks (C4/C5/C7/C8) read the IMAGE, not the target: the target's current table is
#       what is being replaced, so refusing on it would refuse exactly the case this mode exists for.
#       C2/C3/C6/C9 still read the target, and C9-NOT-SYSTEM is the one that matters most here.
#
# OPTIONS
#   --src NAME             staged image under $HOME/unaos-bench/flash/orin/ (required unless selftest)
#   --image FILE           a whole-disk card image to write over the target (mutually exclusive
#                          with --src). See WHOLE-MEDIUM mode above.
#   --erase-target         acknowledge that --image ERASES the whole target. Required with --write.
#   --target PATH          THE MEDIUM. A whole-disk block device (/dev/mmcblk0), a partition
#                          (/dev/mmcblk0p1) or a file-backed image. NEVER inferred. (required)
#   --expect-label LABEL   FAT volume label the target MUST carry     (default: UNAOS-ORIN)
#   --expect-geom SPEC     MBR geometry the target MUST match, comma-separated
#                          slot:type:start:count entries, e.g. "1:0c:2048:112640,2:7f:114688:16384".
#                          PARAMETERISED ON PURPOSE — UNAFSGROW is resizing the native UnaFS
#                          partition this round; see --geom-from.
#   --geom-from FILE       read the geometry from a `CARDREADY-GEOM: <spec>` line in FILE
#                          (default: the in-git record docs/dev/evidence/orin24/CARDREADY-GEOM.md,
#                          written from `arroyo esp-jetson`'s own readback off the BUILT image).
#                          If the file or the line is absent, geometry is NOT checked and the run
#                          says so LOUDLY — an unchecked check is never silently skipped.
#   --part N               MBR slot carrying the FAT boot volume       (default: 1)
#   --allow-geom-absent    permit the run to proceed with geometry unchecked (still announced)
#   --write                actually write (default is a dry run)
#
# EXIT CODES  (SPECFLIGHT discipline — tools/foreman/src/verdict.rs)
#   0 RC_PASS     every check passed
#   1 RC_REFUSE   one or more checks REFUSED; nothing was written
#   2 RC_ERROR    usage / environment error; nothing was evaluated
#
# ⛔ THIS SCRIPT NEVER WRITES WITHOUT --write, AND ITS SELFTEST NEVER TOUCHES A BLOCK DEVICE.
#    The selftest operates exclusively on file-backed images under its own scratch directory.
# ─────────────────────────────────────────────────────────────────────────────────────────────────
set -uo pipefail

RC_PASS=0; RC_REFUSE=1; RC_ERROR=2
# Absolute self-path: the selftest cd's into its fixture dir and re-invokes this script, so a
# relative $0 would resolve to nothing there. (The selftest caught exactly that on its first run —
# 23/23 red with rc=127. A harness that could not have noticed is not a harness.)
SELF="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
# ── orin 23 (2026-09-08), from the first REAL card write this tool ever attempted (two refusals on
# Peter's terminal): (a) `sudo` resets HOME to /root, so every $HOME path missed the seat's tree —
# resolve the INVOKING user's home via SUDO_USER; (b) under sudo there is no session bus, so
# `flatpak-spawn --host` cannot run ("Can't find bus: Cannot autolaunch D-Bus") and every host call
# returned EMPTY — which made C6/C9 pass vacuously (a check that cannot fire) and would have made
# do_write FATAL at the mount. As root on the host the commands run directly; the spawn prefix is
# for the sandbox only. Backup of the pre-edit file: load-card10.sh.bak-orin23-20260908T233942Z.
REAL_HOME="$HOME"
if [ -n "${SUDO_USER:-}" ]; then REAL_HOME="$(getent passwd "$SUDO_USER" | cut -d: -f6)"; fi
if [ "$(id -u)" = 0 ]; then HOSTRUN=""; else HOSTRUN="flatpak-spawn --host"; fi
FLASH="$REAL_HOME/unaos-bench/flash/orin"
# UNAFSROOT (orin 24, 2026-09-09): the expected geometry now comes from a RECORD IN GIT, not
# from a round's scratch. R32 — "anything in ~/unaos-bench/scratch/ could be deleted in 10
# seconds" — applies to the file a CHECK reads exactly as it applies to the tool: an expected
# geometry that evaporates turns C7 into "no expected geometry" (a refusal, so at least it is
# loud) or, with --allow-geom-absent, into a check that cannot fire. The file named here is
# written by `arroyo esp-jetson`'s own geometry readback (the `CARDREADY-GEOM:` line it prints
# off the BUILT image), so the expectation cannot drift from the media without a rebuild.
GEOM_DEFAULT_SRC="$REAL_HOME/src/github.com/pmes/UnaOS-orin/docs/dev/evidence/orin24/CARDREADY-GEOM.md"

SRC_NAME=""; TARGET=""; EXPECT_LABEL="UNAOS-ORIN"; EXPECT_GEOM=""; GEOM_FROM=""
PART=1; MODE="dry"; ALLOW_GEOM_ABSENT=0; SELFTEST=0; SELFTEST_DIR=""
# UNAFSROOT: whole-medium mode. `--image` names a CARD IMAGE (an MBR whole-disk file, e.g.
# `arroyo esp-jetson`'s target/UnaOS-orin-card.img) that REPLACES the target's partition table
# and every partition on it. `--src` stages files INTO an existing volume; `--image` lays the
# volumes down. They are mutually exclusive and the second one needs a second word to fire.
IMAGE_SRC=""; ERASE_OK=0

# ── collected refusals. Nothing short-circuits: every offender is named, SPECFLIGHT-style. ────────
FAULTS=(); CHECKS_RUN=(); CHECKS_PASS=()

refuse() { FAULTS+=("$1"); }                       # $1 = "CHECK-ID: what it saw — what it wanted"
ran()    { CHECKS_RUN+=("$1"); }
passed() { CHECKS_PASS+=("$1"); ran "$1"; }
failed() { ran "$1"; refuse "$1: $2"; }

usage() { sed -n '2,72p' "$0"; exit "$RC_ERROR"; }

while [ $# -gt 0 ]; do
    case "$1" in
        --src)               SRC_NAME="${2:-}"; shift 2 ;;
        --image)             IMAGE_SRC="${2:-}"; shift 2 ;;
        --erase-target)      ERASE_OK=1; shift ;;
        --target)            TARGET="${2:-}"; shift 2 ;;
        --expect-label)      EXPECT_LABEL="${2:-}"; shift 2 ;;
        --expect-geom)       EXPECT_GEOM="${2:-}"; shift 2 ;;
        --geom-from)         GEOM_FROM="${2:-}"; shift 2 ;;
        --part)              PART="${2:-}"; shift 2 ;;
        --allow-geom-absent) ALLOW_GEOM_ABSENT=1; shift ;;
        --write)             MODE="write"; shift ;;
        --selftest)          SELFTEST=1; SELFTEST_DIR="${2:-$HOME/unaos-bench/scratch/orin20/cardready/selftest}"; shift 2 2>/dev/null || shift ;;
        -h|--help)           usage ;;
        *) echo "media-writer: unknown argument '$1'" >&2; exit "$RC_ERROR" ;;
    esac
done

# ═════════════════════════════════════════════════════════════════════════════════════════════════
# THE BYTE-LEVEL IDENTITY READER
#
# Deliberately a re-implementation of `parse_fat_volume_serial` in crates/bootloader/src/main.rs
# (the SAME field offsets, the SAME BS_BootSig==0x29 precondition, the SAME absent-sentinel of 0).
# That is the point: the host must predict EXACTLY the `boot volume FAT serial 0x…` line the loader
# will print, so the flight log can be matched against a value computed before the card ever moved.
# Reading bytes (rather than lsblk/blkid) also makes a file-backed image and a real device the same
# case, which is what lets the selftest below prove every polarity without touching a device.
# ═════════════════════════════════════════════════════════════════════════════════════════════════
# ── HOST BLOCK TOPOLOGY, injectable so C6 and C9 can be driven RED without a device ──────────────
# C6-LABEL-UNIQUE and C9-NOT-SYSTEM are the two checks that ask about the machine rather than about
# the medium, so a file-backed fixture cannot exercise them — and a check that cannot fire in the
# proof is exactly what this round is about. LOADCARD10_LSBLK_FIXTURE substitutes a canned
# `lsblk -rno NAME,LABEL,SIZE,MOUNTPOINT` table, so both polarities are provable with no device and
# no privilege. Unset (the seat's real run) it reads the real topology.
host_lsblk() {
    if [ -n "${LOADCARD10_LSBLK_FIXTURE:-}" ]; then cat "$LOADCARD10_LSBLK_FIXTURE"
    else $HOSTRUN lsblk -rno NAME,LABEL,SIZE,MOUNTPOINT 2>/dev/null; fi
}
host_topology_available() {  # $1 = target
    [ -n "${LOADCARD10_LSBLK_FIXTURE:-}" ] && return 0
    [ -b "$1" ] && return 0
    return 1
}

# ── THE `CARDREADY-GEOM:` READER, shared by C7 in both modes. ────────────────────────────────────
# It matches the TOKEN FOLLOWED BY A SPEC, never the token alone. The naive form — take everything
# after the first line containing `CARDREADY-GEOM:` — was shipped and immediately defeated by the
# record file it was pointed at (orin 24, measured): that file's PROSE names the field ("C7 reads
# the `CARDREADY-GEOM:` line below"), the reader took that sentence as the expected geometry, and
# C7 refused a perfectly good image against a fragment of English. A record has to be able to say
# what it is, so the grammar is what discriminates, not the position in the file.
#
# `slot:type:start:count`, comma-separated; type is two hex digits. Look-around free (gawk/mawk/awk
# all take it), first MATCHING line wins, and a line carrying the token with no spec is skipped
# rather than accepted — an expectation that parsed to nothing is the one shape C7 must never trust.
geom_from_file() {
    awk '{
        if (match($0, /CARDREADY-GEOM:[ \t]*[0-9]+:[0-9a-fA-F][0-9a-fA-F]:[0-9]+:[0-9]+(,[0-9]+:[0-9a-fA-F][0-9a-fA-F]:[0-9]+:[0-9]+)*/)) {
            s = substr($0, RSTART, RLENGTH)
            sub(/^CARDREADY-GEOM:[ \t]*/, "", s)
            print s
            exit
        }
    }' "$1"
}

probe_medium() {  # $1 = path ; prints KEY=VALUE lines ; exit 0 on parse, 3 on unrecognized
    python3 - "$1" "$PART" <<'PY'
import sys, os, struct

path, part = sys.argv[1], int(sys.argv[2])
try:
    f = open(path, 'rb')
except OSError as e:
    print("ERR=%s" % e); sys.exit(4)

with f:
    try:
        f.seek(0, 2); size = f.tell()
    except OSError:
        size = 0
    if size == 0:                       # block devices sometimes report 0 via seek
        try:    size = os.stat(path).st_size
        except OSError: size = 0
    f.seek(0); lba0 = f.read(512)

if len(lba0) < 512:
    print("KIND=short"); print("BYTES=%d" % len(lba0)); sys.exit(3)

u16 = lambda b, o: b[o] | (b[o+1] << 8)
u32 = lambda b, o: b[o] | (b[o+1] << 8) | (b[o+2] << 16) | (b[o+3] << 24)

def parse_vbr(sec):
    """crates/bootloader/src/main.rs::parse_fat_volume_serial, field for field."""
    if sec[0] not in (0xEB, 0xE9):            return None, "LBA 0 is not a FAT VBR (no jump instruction)"
    if sec[510] != 0x55 or sec[511] != 0xAA:  return None, "LBA 0 has no 0x55AA signature"
    if u16(sec, 11) != 512:                   return None, "bytes-per-sector is not 512"
    fat32 = (u16(sec, 22) == 0 and u16(sec, 17) == 0)
    off = 0x43 if fat32 else 0x27
    if sec[off-1] != 0x29:                    return None, "FAT VBR carries no extended BPB / BS_VolID is 0"
    serial = u32(sec, off)
    label  = bytes(sec[off+4:off+15]).decode('latin-1').rstrip(' ').rstrip('\x00')
    # C15 FATCLUST (orin-ledger A58): the FAT TYPE is defined by the count of data clusters, not by the
    # BPB's record shape. spc/rsvd/nfats/rootent/tot/fatsz straight off the BPB; clusters = data/spc.
    spc, rsvd, nfats, rootent = sec[13], u16(sec, 14), sec[16], u16(sec, 17)
    tot = u16(sec, 19) or u32(sec, 32); fatsz = u16(sec, 22) or u32(sec, 36)
    rootsec = (rootent * 32 + 511) // 512
    clusters = (tot - rsvd - nfats * fatsz - rootsec) // spc if spc else 0
    kind = 'FAT12' if clusters < 4085 else ('FAT16' if clusters < 65525 else 'FAT32')
    return (serial, label, 'FAT32' if fat32 else 'FAT16', clusters, kind), None

sig_ok = (lba0[510] == 0x55 and lba0[511] == 0xAA)
vbr, vbr_why = parse_vbr(lba0)

if vbr:                                        # the target IS the volume (a partition or a FAT image)
    serial, label, fat, clusters, kind = vbr
    print("KIND=vbr"); print("DEV_BYTES=%d" % size); print("DEV_BLOCKS=%d" % (size // 512))
    print("VOL_OFFSET=0"); print("VOL_SERIAL=0x%08x" % serial); print("VOL_LABEL=%s" % label)
    print("VOL_FAT=%s" % fat); print("VOL_CLUSTERS=%d" % clusters); print("VOL_FATKIND=%s" % kind); print("GEOM=")
    sys.exit(0)

if not sig_ok:
    print("KIND=raw"); print("DEV_BYTES=%d" % size); print("WHY=%s" % (vbr_why or "no 0x55AA at LBA 0"))
    sys.exit(3)

# ── MBR: four 16-byte entries at 446. Mirrors the kernel's `:: PART: mbr …` census. ──
geom = []; chosen = None
for i in range(4):
    e = lba0[446 + i*16 : 446 + (i+1)*16]
    ptype, start, count = e[4], u32(e, 8), u32(e, 12)
    if ptype == 0 or count == 0:
        continue
    geom.append("%d:%02x:%d:%d" % (i+1, ptype, start, count))
    if i + 1 == part:
        chosen = (start, count)

print("KIND=mbr"); print("DEV_BYTES=%d" % size); print("DEV_BLOCKS=%d" % (size // 512))
print("GEOM=%s" % ",".join(geom))

if chosen is None:
    print("WHY=MBR slot %d is empty" % part); sys.exit(3)

start, count = chosen
print("VOL_OFFSET=%d" % (start * 512)); print("VOL_SECTORS=%d" % count)
with open(path, 'rb') as f:
    f.seek(start * 512); sec = f.read(512)
if len(sec) < 512:
    print("WHY=MBR slot %d starts past the end of the medium" % part); sys.exit(3)
vbr, vbr_why = parse_vbr(sec)
if not vbr:
    print("WHY=%s (MBR slot %d)" % (vbr_why, part)); sys.exit(3)
serial, label, fat, clusters, kind = vbr
print("VOL_SERIAL=0x%08x" % serial); print("VOL_LABEL=%s" % label); print("VOL_FAT=%s" % fat)
print("VOL_CLUSTERS=%d" % clusters); print("VOL_FATKIND=%s" % kind)
PY
}

# ═════════════════════════════════════════════════════════════════════════════════════════════════
# THE CHECKS. Each one NAMES ITSELF, states what it saw and what it wanted, and can FIRE — the
# selftest below drives every one of them red as well as green. Nothing short-circuits.
# ═════════════════════════════════════════════════════════════════════════════════════════════════
run_checks() {
    local src="$1" target="$2"
    FAULTS=(); CHECKS_RUN=(); CHECKS_PASS=()

    # ── C1 STAGED-SRC — the staged image is a staged image. (Carried from load-card.sh:14-15.) ──
    if [ -z "$src" ]; then
        failed "C1-STAGED-SRC" "no --src given — the staged image is never inferred"
    elif [ ! -s "$src/kernel.elf" ]; then
        failed "C1-STAGED-SRC" "$src has no kernel.elf — not a staged image"
    elif [ ! -s "$src/MANIFEST" ]; then
        failed "C1-STAGED-SRC" "$src has no MANIFEST — unrecorded media is never flashed"
    else
        passed "C1-STAGED-SRC"
    fi

    # ── C2 TARGET-NAMED — THE render9 LESSON. No implicit "whatever is mounted", no label scan
    #    that silently takes the last match. The medium is named by the operator or there is no run.
    if [ -z "$target" ]; then
        failed "C2-TARGET-NAMED" "no --target given — this script never guesses the medium (the render9 defect: load-card.sh scanned for a label and took whatever carried it)"
        emit_report; return
    fi
    passed "C2-TARGET-NAMED"

    # ── C3 TARGET-READABLE ──
    if [ ! -e "$target" ]; then
        failed "C3-TARGET-READABLE" "$target does not exist"
    elif [ ! -r "$target" ]; then
        failed "C3-TARGET-READABLE" "$target is not readable"
    else
        passed "C3-TARGET-READABLE"
    fi

    # ── probe once; every identity check below reads this ──
    local probe rc
    probe="$(probe_medium "$target" 2>/dev/null)"; rc=$?
    local KIND="" VOL_LABEL="" VOL_SERIAL="" GEOM="" DEV_BLOCKS="" WHY="" VOL_OFFSET="" VOL_FAT="" VOL_CLUSTERS="" VOL_FATKIND=""
    local line k v
    while IFS= read -r line; do
        k="${line%%=*}"; v="${line#*=}"
        case "$k" in
            KIND) KIND="$v";; VOL_LABEL) VOL_LABEL="$v";; VOL_SERIAL) VOL_SERIAL="$v";;
            GEOM) GEOM="$v";; DEV_BLOCKS) DEV_BLOCKS="$v";; WHY) WHY="$v";;
            VOL_OFFSET) VOL_OFFSET="$v";; VOL_FAT) VOL_FAT="$v";;
            VOL_CLUSTERS) VOL_CLUSTERS="$v";; VOL_FATKIND) VOL_FATKIND="$v";;
        esac
    done <<< "$probe"
    PROBE_KIND="$KIND"; PROBE_LABEL="$VOL_LABEL"; PROBE_SERIAL="$VOL_SERIAL"
    PROBE_GEOM="$GEOM"; PROBE_BLOCKS="$DEV_BLOCKS"; PROBE_OFFSET="${VOL_OFFSET:-0}"; PROBE_FAT="$VOL_FAT"

    # ── C4 MEDIUM-KIND — the medium carries a FAT volume this loader would accept ──
    if [ "$rc" -eq 4 ]; then
        failed "C4-MEDIUM-KIND" "$target could not be opened"
    elif [ "$rc" -ne 0 ] || [ -z "$VOL_SERIAL" ]; then
        failed "C4-MEDIUM-KIND" "$target carries no FAT volume the loader accepts — kind=${KIND:-?} ${WHY:+reason=$WHY}. The board would drop to the UEFI shell with no FSx: to launch."
    else
        passed "C4-MEDIUM-KIND"
    fi

    # ── C15 FAT32-BY-COUNT (A58, 2026-09-12): a FAT32 BPB on a volume under 65,525 clusters is FAT16 by
    #    the spec; Linux mounts it, the Orin UEFI lists no boot option, the kernel misreads the FAT. ──
    if [ -z "$VOL_SERIAL" ]; then
        failed "C15-FAT32-BY-COUNT" "no FAT volume to count clusters on (see C4-MEDIUM-KIND)"
    elif [ "$VOL_FAT" != "FAT32" ] || [ "$VOL_FATKIND" != "FAT32" ]; then
        failed "C15-FAT32-BY-COUNT" "$target volume is bpb=$VOL_FAT clusters=${VOL_CLUSTERS:-?} kind-by-count=${VOL_FATKIND:-?} — not FAT32 by cluster count (>= 65,525); the UEFI would list no bootable disk (A58)"
    else
        passed "C15-FAT32-BY-COUNT"
    fi

    # ── C5 LABEL — ⚠ THE UNAOS-PI CATCH. A stale label is a WRONG MEDIUM, and it refuses. ──
    if [ -z "$VOL_SERIAL" ]; then
        failed "C5-LABEL" "no FAT volume to read a label from (see C4-MEDIUM-KIND)"
    elif [ "$VOL_LABEL" != "$EXPECT_LABEL" ]; then
        failed "C5-LABEL" "$target carries label \"$VOL_LABEL\", wanted \"$EXPECT_LABEL\" — THIS IS THE WRONG MEDIUM. render9 booted with a stale \"UNAOS-PI\" card bound to / by sdmmc_root_bind; writing this one would put an Orin image on a card that is not the Orin's."
    else
        passed "C5-LABEL"
    fi

    # ── C6 LABEL-UNIQUE — two media carrying the same label IS the render9 ambiguity itself.
    #    load-card.sh's `while read … DEV=/dev/$name` loop kept the LAST match and never counted:
    #    with two labelled media present it wrote one of them and told you nothing about the choice.
    if host_topology_available "$target"; then
        local n
        n="$(host_lsblk | awk -v w="$EXPECT_LABEL" '$2==w{c++} END{print c+0}')"
        if [ "${n:-0}" -gt 1 ]; then
            failed "C6-LABEL-UNIQUE" "$n block devices carry label \"$EXPECT_LABEL\" — the medium is AMBIGUOUS. Unplug the others; do not let the tool pick (load-card.sh silently took the last one)."
        else
            passed "C6-LABEL-UNIQUE"
        fi
    else
        passed "C6-LABEL-UNIQUE"   # file-backed with no topology to consult: no peers to confuse
    fi

    # ── C7 GEOMETRY — PARAMETERISED. UNAFSGROW is resizing the native UnaFS partition this round,
    #    so a hardcoded extent would be wrong by the time the seat runs this.
    local geom_want="$EXPECT_GEOM" geom_src="--expect-geom"
    if [ -z "$geom_want" ]; then
        local gf="${GEOM_FROM:-$GEOM_DEFAULT_SRC}"
        if [ -r "$gf" ]; then
            geom_want="$(geom_from_file "$gf")"
            geom_src="$gf"
        fi
    fi
    if [ -z "$geom_want" ]; then
        if [ "$ALLOW_GEOM_ABSENT" -eq 1 ]; then
            ran "C7-GEOMETRY"
            GEOM_NOTE="NOT CHECKED — no --expect-geom and no CARDREADY-GEOM: line in ${GEOM_FROM:-$GEOM_DEFAULT_SRC} (permitted by --allow-geom-absent). OBSERVED: ${GEOM:-<none>}"
        else
            failed "C7-GEOMETRY" "no expected geometry: --expect-geom not given and no \`CARDREADY-GEOM: <spec>\` line in ${GEOM_FROM:-$GEOM_DEFAULT_SRC}. OBSERVED geometry is ${GEOM:-<none>}. Pass --expect-geom, or --allow-geom-absent to proceed with this check announced as UNCHECKED."
        fi
    elif [ "$GEOM" != "$geom_want" ]; then
        failed "C7-GEOMETRY" "$target geometry is \"${GEOM:-<none>}\", wanted \"$geom_want\" (from $geom_src) — the partition table is not the one this image was staged for."
    else
        passed "C7-GEOMETRY"; GEOM_NOTE="matched $geom_want (from $geom_src)"
    fi

    # ── C8 VOLSERIAL — an unstamped volume cannot be identified on the wire.
    #    The loader prints BS_VolID; 0 is its absent sentinel, and a 0 makes the flight log unable
    #    to answer "which medium did this boot from?" — which is the whole question render9 lost.
    if [ -z "$VOL_SERIAL" ]; then
        failed "C8-VOLSERIAL" "no FAT volume to read BS_VolID from (see C4-MEDIUM-KIND)"
    elif [ "$VOL_SERIAL" = "0x00000000" ]; then
        failed "C8-VOLSERIAL" "$target has BS_VolID 0 — the loader would print \`boot volume FAT serial\` as absent and the flight could not be tied to this medium. Re-format with a serial (mkfs.vfat -i)."
    else
        passed "C8-VOLSERIAL"
    fi

    # ── C9 NOT-SYSTEM — never write the host's own disks. ──
    if host_topology_available "$target"; then
        local base hit
        base="$(basename "$target")"; base="${base%p[0-9]}"; base="${base%[0-9]}"
        hit="$(host_lsblk | awk -v b="$base" '
            $4=="/" || $4=="/home" || $4=="/boot" || $4=="/boot/efi" {
                if (index($1, b) == 1) { print $4 " (" $1 ")"; exit }
            }')"
        if [ -n "$hit" ]; then
            failed "C9-NOT-SYSTEM" "$target is on the disk carrying $hit — refusing to write a host system disk"
        else
            passed "C9-NOT-SYSTEM"
        fi
    else
        passed "C9-NOT-SYSTEM"
    fi

    emit_report
}

# ═════════════════════════════════════════════════════════════════════════════════════════════════
# WHOLE-MEDIUM MODE (UNAFSROOT, orin 24) — laying a CARD IMAGE down, table and all.
#
# WHY IT IS A SECOND FUNCTION AND NOT A FLAG INSIDE `run_checks`. The checks above all ask ONE
# question — "is the target already the medium this staged tree belongs on?" — and answer it off the
# target's existing FAT volume. That question is exactly backwards here: this mode exists BECAUSE the
# target is not yet that medium (today's Orin card carries ONE FAT partition and no native volume, so
# `fs::bootdisk` finds nothing to root `/` on and falls back to the FAT). Reusing `run_checks` would
# have refused every card this mode is for, and loosening its checks to let them through would have
# loosened them for the staging path too. So the identity checks are re-pointed AT THE IMAGE — the
# medium as it will be a minute from now — and the target keeps only the checks that are about the
# MACHINE (C6, C9) plus two new ones that are about the write itself (C12, C13).
#
# THE C-ROW VOCABULARY IS DELIBERATELY THE SAME. C4/C5/C7/C8 mean what they always meant; the report
# says which object they were read off, so a reader comparing two runs is never guessing.
run_checks_image() {
    local image="$1" target="$2"
    FAULTS=(); CHECKS_RUN=(); CHECKS_PASS=()

    # ── C11 IMAGE-SRC — the image is a card image, and it is not the target. ──
    if [ ! -s "$image" ]; then
        failed "C11-IMAGE-SRC" "--image $image is absent or empty"
    elif [ ! -r "$image" ]; then
        failed "C11-IMAGE-SRC" "--image $image is not readable"
    elif [ -n "$SRC_NAME" ]; then
        failed "C11-IMAGE-SRC" "--image and --src are mutually exclusive: one lays the volumes DOWN, the other stages files INTO one. Run --image first, then --src."
    else
        passed "C11-IMAGE-SRC"
    fi

    # ── C2 TARGET-NAMED / C3 TARGET-READABLE — unchanged, and still about the target. ──
    if [ -z "$target" ]; then
        failed "C2-TARGET-NAMED" "no --target given — this script never guesses the medium"
        emit_report; return
    fi
    passed "C2-TARGET-NAMED"
    if [ ! -e "$target" ]; then
        failed "C3-TARGET-READABLE" "$target does not exist"
    elif [ ! -r "$target" ]; then
        failed "C3-TARGET-READABLE" "$target is not readable"
    else
        passed "C3-TARGET-READABLE"
    fi

    # ── probe BOTH media. The image is the identity; the target is only the destination. ──
    local iprobe irc tprobe
    iprobe="$(probe_medium "$image" 2>/dev/null)"; irc=$?
    tprobe="$(probe_medium "$target" 2>/dev/null)" || true
    local IKIND="" ILABEL="" ISERIAL="" IGEOM="" IWHY="" IBYTES="" IFAT="" ICLUSTERS="" IFATKIND=""
    local line k v
    while IFS= read -r line; do
        k="${line%%=*}"; v="${line#*=}"
        case "$k" in
            KIND) IKIND="$v";; VOL_LABEL) ILABEL="$v";; VOL_SERIAL) ISERIAL="$v";;
            GEOM) IGEOM="$v";; WHY) IWHY="$v";; DEV_BYTES) IBYTES="$v";;
            VOL_FAT) IFAT="$v";; VOL_CLUSTERS) ICLUSTERS="$v";; VOL_FATKIND) IFATKIND="$v";;
        esac
    done <<< "$iprobe"
    local TKIND="" TLABEL="" TGEOM="" TBYTES=""
    while IFS= read -r line; do
        k="${line%%=*}"; v="${line#*=}"
        case "$k" in
            KIND) TKIND="$v";; VOL_LABEL) TLABEL="$v";; GEOM) TGEOM="$v";; DEV_BYTES) TBYTES="$v";;
        esac
    done <<< "$tprobe"
    # A block device reports 0 bytes through seek AND through stat; ask the kernel for its size.
    if [ -b "$target" ] && [ "${TBYTES:-0}" -eq 0 ]; then
        TBYTES="$($HOSTRUN blockdev --getsize64 "$target" 2>/dev/null || echo 0)"
    fi
    PROBE_KIND="$TKIND"; PROBE_LABEL="$ILABEL"; PROBE_SERIAL="$ISERIAL"
    PROBE_GEOM="$IGEOM"; PROBE_BLOCKS="$TKIND"; PROBE_OFFSET=0; PROBE_FAT=""
    IMAGE_TARGET_WAS="kind=${TKIND:-?} label=\"${TLABEL:-<none>}\" geom=${TGEOM:-<none>} bytes=${TBYTES:-?}"

    # ── C4 MEDIUM-KIND, on the IMAGE ──
    if [ "$irc" -ne 0 ] || [ -z "$ISERIAL" ]; then
        failed "C4-MEDIUM-KIND" "--image $image carries no FAT volume the loader accepts — kind=${IKIND:-?} ${IWHY:+reason=$IWHY}. Writing it would leave the board with no FSx: to launch."
    else
        passed "C4-MEDIUM-KIND"
    fi

    # ── C5 LABEL, on the IMAGE — the same UNAOS-PI catch, one step earlier in the pipeline. ──
    if [ -z "$ISERIAL" ]; then
        failed "C5-LABEL" "no FAT volume in the image to read a label from (see C4-MEDIUM-KIND)"
    elif [ "$ILABEL" != "$EXPECT_LABEL" ]; then
        failed "C5-LABEL" "--image carries label \"$ILABEL\", wanted \"$EXPECT_LABEL\" — a card written from it could not be found by label afterwards (C6 and load-card.sh both address the card that way)."
    else
        passed "C5-LABEL"
    fi

    # ── C6 LABEL-UNIQUE — about the MACHINE, so it is unchanged. ──
    if host_topology_available "$target"; then
        local n
        n="$(host_lsblk | awk -v w="$EXPECT_LABEL" '$2==w{c++} END{print c+0}')"
        if [ "${n:-0}" -gt 1 ]; then
            failed "C6-LABEL-UNIQUE" "$n block devices carry label \"$EXPECT_LABEL\" — the medium is AMBIGUOUS. Unplug the others before writing a whole disk."
        else
            passed "C6-LABEL-UNIQUE"
        fi
    else
        passed "C6-LABEL-UNIQUE"
    fi

    # ── C7 GEOMETRY, on the IMAGE. The target's table is what is being replaced. ──
    local geom_want="$EXPECT_GEOM" geom_src="--expect-geom"
    if [ -z "$geom_want" ]; then
        local gf="${GEOM_FROM:-$GEOM_DEFAULT_SRC}"
        if [ -r "$gf" ]; then
            geom_want="$(geom_from_file "$gf")"
            geom_src="$gf"
        fi
    fi
    if [ -z "$geom_want" ]; then
        if [ "$ALLOW_GEOM_ABSENT" -eq 1 ]; then
            ran "C7-GEOMETRY"
            GEOM_NOTE="NOT CHECKED — no --expect-geom and no CARDREADY-GEOM: line in ${GEOM_FROM:-$GEOM_DEFAULT_SRC} (permitted by --allow-geom-absent). IMAGE geometry is ${IGEOM:-<none>}"
        else
            failed "C7-GEOMETRY" "no expected geometry: --expect-geom not given and no \`CARDREADY-GEOM: <spec>\` line in ${GEOM_FROM:-$GEOM_DEFAULT_SRC}. The IMAGE's geometry is ${IGEOM:-<none>}. Pass --expect-geom, or --allow-geom-absent."
        fi
    elif [ "$IGEOM" != "$geom_want" ]; then
        failed "C7-GEOMETRY" "--image geometry is \"${IGEOM:-<none>}\", wanted \"$geom_want\" (from $geom_src) — this is not the layout that was declared."
    else
        passed "C7-GEOMETRY"; GEOM_NOTE="matched $geom_want (from $geom_src) — read off the IMAGE"
    fi

    # ── C8 VOLSERIAL, on the IMAGE ──
    if [ -z "$ISERIAL" ]; then
        failed "C8-VOLSERIAL" "no FAT volume in the image to read BS_VolID from (see C4-MEDIUM-KIND)"
    elif [ "$ISERIAL" = "0x00000000" ]; then
        failed "C8-VOLSERIAL" "--image has BS_VolID 0 — the loader would print \`boot volume FAT serial\` as absent and no flight could be tied to this medium."
    else
        passed "C8-VOLSERIAL"
    fi
    # ── C15 FAT32-BY-COUNT, on the IMAGE (A58, 2026-09-12): the render13 card the UEFI refused passed
    #    C4/C5/C8 with 32,440 clusters under a FAT32 BPB. Count them here, before any card. ──
    if [ -z "$ISERIAL" ]; then
        failed "C15-FAT32-BY-COUNT" "no FAT volume in the image to count clusters on (see C4-MEDIUM-KIND)"
    elif [ "$IFAT" != "FAT32" ] || [ "$IFATKIND" != "FAT32" ]; then
        failed "C15-FAT32-BY-COUNT" "--image p1 is bpb=$IFAT clusters=${ICLUSTERS:-?} kind-by-count=${IFATKIND:-?} — not FAT32 by cluster count (>= 65,525); the Orin UEFI listed no bootable disk for exactly this shape (A58)"
    else
        passed "C15-FAT32-BY-COUNT"
    fi

    # ── C9 NOT-SYSTEM — unchanged, and the most load-bearing check in this mode. ──
    if host_topology_available "$target"; then
        local base hit
        base="$(basename "$target")"; base="${base%p[0-9]}"; base="${base%[0-9]}"
        hit="$(host_lsblk | awk -v b="$base" '
            $4=="/" || $4=="/home" || $4=="/boot" || $4=="/boot/efi" {
                if (index($1, b) == 1) { print $4 " (" $1 ")"; exit }
            }')"
        if [ -n "$hit" ]; then
            failed "C9-NOT-SYSTEM" "$target is on the disk carrying $hit — refusing to write a host system disk"
        else
            passed "C9-NOT-SYSTEM"
        fi
    else
        passed "C9-NOT-SYSTEM"
    fi

    # ── C12 IMAGE-FITS — a truncated write is a corrupt card with a plausible first partition. ──
    if [ -z "${IBYTES:-}" ] || [ "${IBYTES:-0}" -eq 0 ]; then
        failed "C12-IMAGE-FITS" "could not size --image $image"
    elif [ "${TBYTES:-0}" -eq 0 ]; then
        failed "C12-IMAGE-FITS" "could not size the target $target — refusing to write an image of unknown fit"
    elif [ "$IBYTES" -gt "$TBYTES" ]; then
        failed "C12-IMAGE-FITS" "--image is $IBYTES B and $target is $TBYTES B — the tail of the image (the native UnaFS partition) would be cut off, leaving a card whose MBR promises a partition that is not there."
    else
        passed "C12-IMAGE-FITS"
    fi

    # ── C13 TARGET-QUIET — nothing on the target is mounted. Writing under a live mount corrupts
    #    both the medium and the host's page cache view of it. File-backed targets have no mounts.
    if host_topology_available "$target"; then
        local base mounted
        base="$(basename "$target")"; base="${base%p[0-9]}"; base="${base%[0-9]}"
        mounted="$(host_lsblk | awk -v b="$base" '$4!="" && index($1,b)==1 {print $1 " -> " $4}')"
        if [ -n "$mounted" ]; then
            failed "C13-TARGET-QUIET" "$target has a MOUNTED partition ($(printf '%s' "$mounted" | tr '\n' ' ')) — unmount it first (udisksctl unmount -b ...)"
        else
            passed "C13-TARGET-QUIET"
        fi
    else
        passed "C13-TARGET-QUIET"
    fi

    emit_report
}

# THE WHOLE-MEDIUM WRITE. `dd`, then a read-back that compares the bytes actually on the medium with
# the image — the same standard the staging path holds itself to, one layer down: there is no
# MANIFEST here, so the whole written extent is the manifest.
do_write_image() {
    local image="$1" target="$2"
    local ibytes; ibytes="$(stat -c %s "$image" 2>/dev/null || stat -f %z "$image")"
    echo "=== WHOLE-MEDIUM WRITE — $target is being REPLACED ==="
    echo "  before: $IMAGE_TARGET_WAS"
    echo "  image:  $image ($ibytes B, sha256 $(sha256sum "$image" | cut -d' ' -f1))"
    if [ -b "$target" ]; then
        $HOSTRUN dd if="$image" of="$target" bs=4M conv=fsync status=progress || { echo "FATAL: dd failed"; return "$RC_ERROR"; }
        $HOSTRUN sync
    else
        dd if="$image" of="$target" bs=4M conv=notrunc,fsync status=none || { echo "FATAL: dd failed"; return "$RC_ERROR"; }
        sync
    fi
    # ── READ-BACK POLARITY FIXTURE. The read-back below is the only check in this mode that cannot
    #    be driven red by an argument: a `dd` onto a file always lands. So the selftest flips one
    #    byte here, exactly as LOADCARD10_LSBLK_FIXTURE injects a topology for C6/C9. Refuses to act
    #    on a block device, so it can never touch real media even if the variable is left set.
    if [ -n "${MEDIAWRITER_CORRUPT_AFTER_WRITE:-}" ] && [ ! -b "$target" ]; then
        printf 'X' | dd of="$target" bs=1 seek=1024 count=1 conv=notrunc status=none
        echo "  (MEDIAWRITER_CORRUPT_AFTER_WRITE set: one byte flipped on the target — read-back fixture)"
    fi
    echo "=== READ-BACK SHA MATCH (the image vs the first $ibytes B now on $target) ==="
    local want got
    want="$(sha256sum "$image" | cut -d' ' -f1)"
    local DDPFX=""; [ -b "$target" ] && DDPFX="$HOSTRUN"
    got="$($DDPFX dd if="$target" bs=1M count=$(( (ibytes + 1048575) / 1048576 )) iflag=fullblock status=none 2>/dev/null | head -c "$ibytes" | sha256sum | cut -d' ' -f1)"
    if [ "$want" != "$got" ]; then
        printf '  FAIL  whole medium\n    image=%s\n    onmedia=%s\n' "$want" "${got:-<unreadable>}"
        echo "=== THE MEDIUM IS NOT LOADED. Investigate before booting it. ==="
        return "$RC_REFUSE"
    fi
    echo "  OK    whole medium ($ibytes B identical)"
    echo "=== $target now carries the image. Re-probing the medium as written: ==="
    probe_medium "$target" | sed 's/^/    /'
    echo
    echo "NEXT: this laid down the GEOMETRY. Stage the flight tree onto it the ordinary way —"
    echo "  $SELF --src <staged-dir> --target $target --write"
    echo "which is the run that writes the FLIGHTID card the boot is scored against."
    return "$RC_PASS"
}

GEOM_NOTE=""
IMAGE_TARGET_WAS=""
emit_report() {
    echo "=== media-writer PREFLIGHT ==="
    if [ -n "$IMAGE_SRC" ]; then
        # WHOLE-MEDIUM: say plainly which object each identity was read off, and what is about to be
        # destroyed. A report that printed one "FAT volume" line here would be ambiguous about the
        # single fact that matters — is this the medium's label, or the label it is about to get?
        echo "  MODE           WHOLE-MEDIUM (--image) — the target's partition table and ALL its"
        echo "                 partitions are REPLACED. Nothing on it is preserved."
        echo "  image          ${IMAGE_SRC}"
        echo "  target         ${TARGET:-<none>}"
        echo "  target BEFORE  ${IMAGE_TARGET_WAS:-<unprobed>}"
        echo "  image FAT vol  label=\"${PROBE_LABEL:-<none>}\"  BS_VolID=${PROBE_SERIAL:-<none>}"
        echo "  image geometry ${PROBE_GEOM:-<none>}"
    else
    echo "  src            ${SRC_DIR:-<none>}"
    echo "  target         ${TARGET:-<none>}"
    echo "  medium kind    ${PROBE_KIND:-?}   blocks=${PROBE_BLOCKS:-?}  vol@byte ${PROBE_OFFSET:-?}"
    echo "  FAT volume     label=\"${PROBE_LABEL:-<none>}\"  BS_VolID=${PROBE_SERIAL:-<none>}  ${PROBE_FAT:-}"
    echo "  geometry       ${PROBE_GEOM:-<none>}"
    fi
    [ -n "$GEOM_NOTE" ] && echo "                 $GEOM_NOTE"
    echo "  checks         ${#CHECKS_PASS[@]}/${#CHECKS_RUN[@]} passed"
    # Name every check that PASSED, not just the ones that failed. A run that prints only failures
    # cannot be asked "did C8 actually run?" — and an unasked check reads exactly like a passing one.
    for c in "${CHECKS_PASS[@]}"; do echo "    PASS  $c"; done
    if [ "${#FAULTS[@]}" -gt 0 ]; then
        echo
        echo "media-writer REFUSED — ${#FAULTS[@]} check(s) failed; NOTHING WAS WRITTEN."
        for f in "${FAULTS[@]}"; do echo "  $f"; done
        echo
        echo "  Every check above was evaluated; the list is complete, not first-fault."
        return 1
    fi
    return 0
}

# ═════════════════════════════════════════════════════════════════════════════════════════════════
# THE WRITE + THE READ-BACK
#
# WHAT THE SHA-VERIFY PROVES: that the bytes now readable at each MANIFEST path on THIS medium equal
# the bytes in the staged tree. That is a statement about the WRITE and about this medium alone.
# WHAT IT DOES NOT PROVE: that the board will BOOT this medium; that this medium is the one that
# ends up in the reader; that the Orin's native SD slot holds anything related to it. render9's
# read-back passed 10/10 and the flight still scored against a different card. The read-back is
# necessary and it is not sufficient — the FLIGHTID card below is the other half.
# ═════════════════════════════════════════════════════════════════════════════════════════════════
do_write() {
    local src="$1" target="$2" mp="" via=""
    if [ -b "$target" ]; then
        # The FAT volume lives in MBR slot $PART, so the device to MOUNT is the partition node, not
        # the whole disk `--target` names (the whole disk is what C7 parses; mounting it fails, and
        # `lsblk -rno MOUNTPOINT <disk> | head -1` is the disk's own EMPTY row — orin 23's finding
        # on the first real write attempt, 2026-09-08).
        local pdev="$target"
        if [ -b "${target}p${PART}" ]; then pdev="${target}p${PART}"
        elif [ -b "${target}${PART}" ]; then pdev="${target}${PART}"; fi
        WRITE_MP=""
        if [ "$(id -u)" = 0 ]; then
            mp="$(lsblk -rno MOUNTPOINT "$pdev" | head -1)"
            if [ -z "$mp" ]; then
                mp="$(mktemp -d /run/mediawriter.XXXXXX)" && mount "$pdev" "$mp" || { echo "FATAL: mount $pdev failed"; return "$RC_ERROR"; }
                WRITE_MP="$mp"
            fi
        else
            $HOSTRUN udisksctl mount -b "$pdev" >/dev/null 2>&1
            mp="$($HOSTRUN lsblk -rno MOUNTPOINT "$pdev" | head -1)"
        fi
        [ -n "$mp" ] || { echo "FATAL: $pdev did not mount"; return "$RC_ERROR"; }
        via="mount:$pdev@$mp"
        # The harvest (what the card held BEFORE this write) is a record, so it does not go to scratch.
        local harvest="$FLASH/harvest/$(date -u +%Y%m%dT%H%M%SZ)-from-$(basename "$target")"
        mkdir -p "$harvest"
        $HOSTRUN cp -R "$mp/." "$harvest/" 2>/dev/null
        echo "HARVEST: previous card contents -> $harvest"
        $HOSTRUN cp -R "$src/." "$mp/"
        $HOSTRUN sync
        [ -n "${SUDO_UID:-}" ] && chown -R "$SUDO_UID:${SUDO_GID:-$SUDO_UID}" "$harvest" 2>/dev/null
    else
        # mtools addresses the volume inside the whole-disk image as image@@<byte offset>, so a
        # file-backed target needs no loop device and no root. The path must be ABSOLUTE: the copy
        # runs from inside $src, and a relative --target resolved there is a different file (or
        # none). The selftest caught this — "Can't open wgood.img" from within src/.
        case "$target" in /*) ;; *) target="$PWD/$target";; esac
        via="mtools:$target@@$PROBE_OFFSET"
        ( cd "$src" && for e in *; do
            if [ -d "$e" ]; then mcopy -s -o -i "$target@@$PROBE_OFFSET" "$e" "::"
            else mcopy -o -i "$target@@$PROBE_OFFSET" "$e" "::"; fi
          done ) || { echo "FATAL: mcopy failed"; return "$RC_ERROR"; }
    fi

    echo "=== READ-BACK SHA MATCH (staged MANIFEST vs the bytes now on $target) ==="
    local fail=0 n=0 want path got
    while read -r want path; do
        case "$want" in \#*|"") continue;; esac
        if [ -b "$target" ]; then
            got="$($HOSTRUN sha256sum "$mp/$path" 2>/dev/null | cut -d' ' -f1)"
        else
            got="$(mtype -i "$target@@$PROBE_OFFSET" "::${path}" 2>/dev/null | sha256sum | cut -d' ' -f1)"
        fi
        n=$((n+1))
        if [ "$want" = "$got" ]; then printf '  OK    %s\n' "$path"
        else printf '  FAIL  %s\n    staged=%s\n    onmedia=%s\n' "$path" "$want" "${got:-<absent>}"; fail=$((fail+1)); fi
    done < "$src/MANIFEST"

    if [ "$fail" -ne 0 ]; then
        echo "=== $fail of $n FILES DIFFER — MEDIUM IS NOT LOADED. Not unmounting; investigate. ==="
        return "$RC_REFUSE"
    fi
    echo "=== $n/$n match. $target carries $SRC_NAME. ==="
    write_flightid "$src" "$target" "$n" "$via"
    if [ -b "$target" ]; then
        if [ -n "${WRITE_MP:-}" ]; then
            sync && umount "$WRITE_MP" && rmdir "$WRITE_MP" && echo "UNMOUNTED $target — safe to pull."
        else
            $HOSTRUN udisksctl unmount -b "${target}p${PART}" >/dev/null 2>&1 || $HOSTRUN udisksctl unmount -b "$target" >/dev/null 2>&1
            echo "UNMOUNTED $target — safe to pull."
        fi
        [ -n "${SUDO_UID:-}" ] && chown "$SUDO_UID:${SUDO_GID:-$SUDO_UID}" "${src}.FLIGHTID" 2>/dev/null
    fi
    return "$RC_PASS"
}

# ── THE FLIGHTID CARD — the answer to "which image is this?", written BEFORE the board sees it ────
# Every value here appears on the wire from a source that is NOT this script, so the flight log can
# be scored against it without reference to what anyone wrote.
write_flightid() {
    local src="$1" target="$2" n="$3" via="$4"
    local maxv out
    maxv="$(awk '/^# ELF max_vaddr=/{sub(/^# ELF max_vaddr=/,""); sub(/ .*/,""); print; exit}' "$src/MANIFEST")"
    out="${src}.FLIGHTID"
    {
        echo "# FLIGHTID — $SRC_NAME"
        echo "# written $(date -u +%Y-%m-%dT%H:%M:%SZ) by media-writer.sh onto $target ($via)"
        echo "#"
        echo "# SCORE THE BOOT BY THESE LINES, IN THIS ORDER, BEFORE ANY QUESTION IS SCORED."
        echo "# Each is printed by a component that is not this script."
        echo
        echo "IMAGE_SHA256   $(sha256sum "$src/kernel.elf" | cut -d' ' -f1)"
        echo "ELF_MAX_VADDR  ${maxv:-<absent from MANIFEST>}"
        echo "BOOT_VOL_LABEL $PROBE_LABEL"
        echo "BOOT_VOL_ID    $PROBE_SERIAL"
        echo "MEDIA_GEOM     $PROBE_GEOM"
        echo "FILES_VERIFIED $n"
        echo
        echo "# 1. THE IMAGE THAT LOADED — crates/bootloader/src/main.rs, fn main (the #[entry]),"
        echo "#    the KELF image-identity witness. The flight log MUST show:"
        echo "#      KELF min=0x0 max=${maxv:-?} pg=<n>"
        echo "#    A different max = a different kernel. This is FLIGHT.md §C.1 and it is scored FIRST."
        echo
        echo "# 2. THE MEDIUM IT LOADED FROM — crates/bootloader/src/main.rs, fn read_boot_volume_serial,"
        echo "#    printed at the log::info! four lines below the KELF line. The flight log MUST show:"
        echo "#      boot volume FAT serial $PROBE_SERIAL (extended BPB BS_VolID)"
        echo "#    A different serial = the board booted a DIFFERENT PHYSICAL MEDIUM than this write."
        echo
        echo "# 3. THE MEDIUM MOUNTED AT / — arch/aarch64/sdmmc_tegra.rs, fn root_probe (RPS witness)."
        echo "#    ⚠ THIS IS A SECOND, PHYSICALLY DIFFERENT CARD: the Orin's NATIVE SD slot, which"
        echo "#    sdmmc_root_bind binds to /, /boot AND /apps. THIS SCRIPT DOES NOT WRITE IT."
        echo "#    The flight log will show:"
        echo "#      [sdmmc] root mount source=tegra-sd card_blocks=<n> -> OK label=\"<L>\" vol_id=0x<V>"
        echo "#    REQUIRED BEFORE SCORING ANY FILESYSTEM QUESTION: <L> must be \"$EXPECT_LABEL\"."
        echo "#    On render9 it read label=\"UNAOS-PI\" vol_id=0xabfbdefa — a stale Pi card — while the"
        echo "#    boot volume serial was 0xde001a13. TWO MEDIA. Nothing in the kernel compares them"
        echo "#    (see PROGRESS.md §5: drivers/block.rs::set_boot_volume_serial is x86_64-only), so"
        echo "#    THE COMPARISON IS THE SCORER'S JOB UNTIL THAT cfg IS WIDENED."
        echo "#    ⚠ render11 onward (BOOTROOT, exec-orin22-bootroot): the [sdmmc] root family is DELETED."
        echo "#    Its ABSENCE is expected; the root witness is instead"
        echo "#      [vfs] root = boot volume serial=$PROBE_SERIAL source=<usb|global|tegra-sd> ... ::"
        echo "#    and any other UnaOS disk mounts at /volumes/<LABEL>. Score with scorer11.sh."
    } > "$out"
    echo "FLIGHTID: $out"
    echo "  -> score the flight against it BEFORE scoring any question."
}

# ═════════════════════════════════════════════════════════════════════════════════════════════════
# SELFTEST — a check that cannot fire is not a check.
# Every check is driven RED and GREEN against file-backed images. No block device is ever touched.
# ═════════════════════════════════════════════════════════════════════════════════════════════════
mkimg() {  # $1 out  $2 label  $3 volid(hex,no 0x)  $4 p2_start  $5 p2_count  [$6 = "nofat"|"nombr"]
    local out="$1" label="$2" volid="$3" p2s="$4" p2c="$5" special="${6:-}"
    local P1S=2048 P1C=112640
    rm -f "$out"; dd if=/dev/zero of="$out" bs=512 count=$((P1S+P1C+p2c+16)) status=none
    if [ "$special" != "nombr" ]; then
        python3 - "$out" "$P1S" "$P1C" "$p2s" "$p2c" <<'PY'
import sys, struct
out, p1s, p1c, p2s, p2c = sys.argv[1], *map(int, sys.argv[2:6])
mbr = bytearray(512)
def ent(o, t, s, c):
    mbr[o+4] = t
    mbr[o+8:o+12]  = struct.pack('<I', s)
    mbr[o+12:o+16] = struct.pack('<I', c)
ent(446, 0x0c, p1s, p1c)
if p2c: ent(462, 0x7f, p2s, p2c)
mbr[510], mbr[511] = 0x55, 0xAA
with open(out, 'r+b') as f: f.seek(0); f.write(mbr)
PY
    fi
    if [ "$special" != "nofat" ]; then
        local tmp="${out}.p1" spc=""
        [ "$special" = "spc8" ] && spc="-s 8"   # C15 fixture: 14,048 clusters at 55 MiB = FAT16 by count under a FAT32 BPB (the A58 shape)
        dd if=/dev/zero of="$tmp" bs=512 count=$P1C status=none
        mkfs.vfat -F 32 $spc -n "$label" -i "$volid" "$tmp" >/dev/null 2>&1
        dd if="$tmp" of="$out" bs=512 seek=$P1S conv=notrunc status=none
        rm -f "$tmp"
    fi
}

selftest() {
    local D="${SELFTEST_DIR:-$HOME/unaos-bench/scratch/orin20/cardready/selftest}"
    mkdir -p "$D"; cd "$D" || exit "$RC_ERROR"
    local GOODGEOM="1:0c:2048:112640,2:7f:114688:16384"
    local pass=0 fail=0

    arm() {  # $1 name  $2 expect(PASS|REFUSE)  $3 want-substring  shift 3 -> argv
        local name="$1" expect="$2" want="$3"; shift 3
        local out rc
        # An arm may be given a full command line beginning with `env` (the injected-topology
        # arms); otherwise it is argv for this script.
        if [ "${1:-}" = "env" ]; then out="$("$@" 2>&1)"; rc=$?
        else                          out="$("$SELF" "$@" 2>&1)"; rc=$?; fi
        local got="PASS"; [ "$rc" -ne 0 ] && got="REFUSE"
        if [ "$got" = "$expect" ] && printf '%s' "$out" | grep -qF -- "$want"; then
            printf '  ok    %-34s %-7s rc=%d\n' "$name" "$got" "$rc"; pass=$((pass+1))
        else
            printf '  FAIL  %-34s want=%s/%s got=%s rc=%d\n' "$name" "$expect" "$want" "$got" "$rc"
            printf '%s\n' "$out" | sed 's/^/          | /'; fail=$((fail+1))
        fi
    }

    echo "=== media-writer SELFTEST — every check, BOTH polarities, file-backed only ==="
    echo "    dir: $D"
    echo

    # ── fixtures ──
    mkimg good.img  UNAOS-ORIN  de001a13 114688 16384
    mkimg stale.img UNAOS-PI    abfbdefa 114688 16384     # ⚠ the render9 medium, reproduced
    mkimg grown.img UNAOS-ORIN  de001a13 114688 65536     # UNAFSGROW's larger UnaFS partition
    mkimg zeroid.img UNAOS-ORIN 00000000 114688 16384
    mkimg raw.img   UNAOS-ORIN  de001a13 114688 16384 nombr
    mkimg nofat.img UNAOS-ORIN  de001a13 114688 16384 nofat
    mkimg spc8.img  UNAOS-ORIN  de001a13 114688 65536 spc8      # A58: FAT32 BPB, FAT16 by cluster count

    # ── a staged tree, and a corrupted twin, for the read-back polarity ──
    rm -rf src srcbad; mkdir -p src/EFI/BOOT srcbad/EFI/BOOT
    printf 'KERNELBYTES-render10\n' > src/kernel.elf
    printf 'LOADERBYTES\n'          > src/EFI/BOOT/BOOTAA64.EFI
    { echo "# ELF max_vaddr=0x33f480 (this boot's identity on the wire)"
      echo "$(sha256sum src/kernel.elf | cut -d' ' -f1) kernel.elf"
      echo "$(sha256sum src/EFI/BOOT/BOOTAA64.EFI | cut -d' ' -f1) EFI/BOOT/BOOTAA64.EFI"
    } > src/MANIFEST
    cp -r src/. srcbad/; printf 'KERNELBYTES-CORRUPTED\n' > srcbad/kernel.elf   # MANIFEST left stale

    local S="--src $PWD/src --expect-geom $GOODGEOM"

    echo "-- C1 STAGED-SRC --"
    arm  "C1 green (staged tree)"        PASS   "C1-STAGED-SRC"      $S --target good.img
    rm -rf nokern; mkdir -p nokern; touch nokern/MANIFEST
    arm  "C1 RED   (no kernel.elf)"      REFUSE "has no kernel.elf"  --src "$PWD/nokern" --target good.img --expect-geom "$GOODGEOM"
    rm -rf nomanif; mkdir -p nomanif; echo x > nomanif/kernel.elf
    arm  "C1 RED   (no MANIFEST)"        REFUSE "has no MANIFEST"    --src "$PWD/nomanif" --target good.img --expect-geom "$GOODGEOM"

    echo "-- C2 TARGET-NAMED (the render9 defect) --"
    arm  "C2 green (target named)"       PASS   "C2-TARGET-NAMED"    $S --target good.img
    arm  "C2 RED   (no --target)"        REFUSE "never guesses the medium" $S

    echo "-- C3 TARGET-READABLE --"
    arm  "C3 green (exists)"             PASS   "C3-TARGET-READABLE" $S --target good.img
    arm  "C3 RED   (absent)"             REFUSE "does not exist"     $S --target no-such.img

    echo "-- C4 MEDIUM-KIND --"
    arm  "C4 green (FAT present)"        PASS   "C4-MEDIUM-KIND"     $S --target good.img
    arm  "C4 RED   (no MBR sig)"         REFUSE "carries no FAT volume" $S --target raw.img
    arm  "C4 RED   (MBR, no FAT)"        REFUSE "carries no FAT volume" $S --target nofat.img

    echo "-- C5 LABEL  ⚠ the UNAOS-PI catch --"
    arm  "C5 green (UNAOS-ORIN)"         PASS   "C5-LABEL"           $S --target good.img
    arm  "C5 RED   (stale UNAOS-PI)"     REFUSE "THIS IS THE WRONG MEDIUM" $S --target stale.img
    arm  "C5 green (label overridden)"   PASS   "C5-LABEL"           $S --target stale.img --expect-label UNAOS-PI

    echo "-- C7 GEOMETRY (parameterised) --"
    arm  "C7 green (geom matches)"       PASS   "C7-GEOMETRY"        $S --target good.img
    arm  "C15 RED  (FAT16 by count)"     REFUSE "by cluster count"   --src "$PWD/src" --target spc8.img --expect-geom "1:0c:2048:112640,2:7f:114688:65536"
    arm  "C7 RED   (UnaFS grown)"        REFUSE "geometry is"        $S --target grown.img
    arm  "C7 green (grown, declared)"    PASS   "C7-GEOMETRY"        --src "$PWD/src" --target grown.img --expect-geom "1:0c:2048:112640,2:7f:114688:65536"
    printf 'CARDREADY-GEOM: 1:0c:2048:112640,2:7f:114688:65536\n' > fakeprogress.md
    arm  "C7 green (geom from FILE)"     PASS   "C7-GEOMETRY"        --src "$PWD/src" --target grown.img --geom-from "$PWD/fakeprogress.md"
    # ⚠ THE PROSE TRAP, found by pointing C7 at its own record file (orin 24). A record that
    # DOCUMENTS the field mentions `CARDREADY-GEOM:` in a sentence before it carries the spec; the
    # old reader took the sentence. Both polarities: the spec is found past the prose, and a file
    # with the token but NO spec is not accepted as an expectation.
    { echo "C7-GEOMETRY reads the \`CARDREADY-GEOM:\` line below and refuses any other medium."
      echo
      echo "    CARDREADY-GEOM: 1:0c:2048:112640,2:7f:114688:65536"
    } > proseprogress.md
    arm  "C7 green (spec past prose)"    PASS   "C7-GEOMETRY"        --src "$PWD/src" --target grown.img --geom-from "$PWD/proseprogress.md"
    printf 'this file mentions CARDREADY-GEOM: but carries no spec at all\n' > nospecprogress.md
    arm  "C7 RED   (token, no spec)"     REFUSE "no expected geometry" --src "$PWD/src" --target good.img --geom-from "$PWD/nospecprogress.md"
    arm  "C7 RED   (no geom anywhere)"   REFUSE "no expected geometry" --src "$PWD/src" --target good.img --geom-from "$PWD/no-such-progress.md"
    arm  "C7 green (absent, permitted)"  PASS   "NOT CHECKED"        --src "$PWD/src" --target good.img --geom-from "$PWD/no-such-progress.md" --allow-geom-absent

    echo "-- C8 VOLSERIAL --"
    arm  "C8 green (serial stamped)"     PASS   "C8-VOLSERIAL"       $S --target good.img
    arm  "C8 RED   (BS_VolID 0)"         REFUSE "BS_VolID 0"         $S --target zeroid.img

    # ── C6 / C9 ask about the MACHINE, not the medium, so they need an injected topology to fire.
    #    Without this they pass vacuously on every file-backed fixture — a check that cannot fire.
    printf 'mmcblk0  30.2G \nmmcblk0p1 UNAOS-ORIN 30.2G \nnvme0n1  953.9G \nnvme0n1p3 fedora 951.3G /home\n' > topo-one.txt
    printf 'mmcblk0p1 UNAOS-ORIN 30.2G \nsdb1 UNAOS-ORIN 29.7G \nnvme0n1p3 fedora 951.3G /home\n'            > topo-two.txt
    printf 'nvme0n1  953.9G \nnvme0n1p3 fedora 951.3G /home\nnvme0n1p2  2G /boot\n'                          > topo-sys.txt

    echo "-- C6 LABEL-UNIQUE (injected host topology) --"
    arm  "C6 green (one such label)"     PASS   "PASS  C6-LABEL-UNIQUE" env LOADCARD10_LSBLK_FIXTURE="$PWD/topo-one.txt" \
             "$SELF" --src "$PWD/src" --target good.img --expect-geom "$GOODGEOM"
    arm  "C6 RED   (two carry it)"       REFUSE "medium is AMBIGUOUS"  env LOADCARD10_LSBLK_FIXTURE="$PWD/topo-two.txt" \
             "$SELF" --src "$PWD/src" --target good.img --expect-geom "$GOODGEOM"

    echo "-- C9 NOT-SYSTEM (injected host topology) --"
    arm  "C9 green (not a system disk)"  PASS   "PASS  C9-NOT-SYSTEM"  env LOADCARD10_LSBLK_FIXTURE="$PWD/topo-one.txt" \
             "$SELF" --src "$PWD/src" --target good.img --expect-geom "$GOODGEOM"
    # A perfectly good medium that merely happens to be NAMED like the host's system disk: every
    # other check passes, so C9 is the ONLY refusal and the arm isolates it.
    cp good.img nvme0n1p3
    arm  "C9 RED   (target holds /home)" REFUSE "refusing to write a host system disk" env LOADCARD10_LSBLK_FIXTURE="$PWD/topo-sys.txt" \
             "$SELF" --src "$PWD/src" --target nvme0n1p3 --expect-geom "$GOODGEOM"

    echo "-- C10 READ-BACK (write polarity) --"
    cp good.img wgood.img; cp good.img wbad.img
    arm  "C10 green (bytes match)"       PASS   "2/2 match."   --src "$PWD/src"    --target wgood.img --expect-geom "$GOODGEOM" --write
    arm  "C10 RED   (corrupted image)"   REFUSE "FILES DIFFER" --src "$PWD/srcbad" --target wbad.img  --expect-geom "$GOODGEOM" --write

    # ═════════════════════════════════════════════════════════════════════════════════════════
    # WHOLE-MEDIUM MODE (UNAFSROOT, orin 24). `grown.img` is a two-partition UNAOS-ORIN card —
    # STRUCTURALLY the thing `arroyo esp-jetson` builds (MBR, FAT p1, type-0x7f native p2), at a
    # size the selftest can make in a second. Every leg here is file-backed; no device is touched.
    echo "-- C11/C12/C13 WHOLE-MEDIUM (--image) --"
    local GROWNGEOM="1:0c:2048:112640,2:7f:114688:65536"
    dd if=/dev/zero of=tgt-big.img   bs=1M count=200 status=none
    dd if=/dev/zero of=tgt-small.img bs=1M count=1   status=none
    arm  "IMG green (dry run)"           PASS   "C11-IMAGE-SRC"          --image "$PWD/grown.img" --target tgt-big.img --expect-geom "$GROWNGEOM"
    # THE RE-POINTING, asserted rather than argued: the target here carries the stale UNAOS-PI
    # label that C5 refuses in staging mode. In whole-medium mode it is about to be overwritten, so
    # the run must PASS — and the identity checks must have read the IMAGE to do so.
    cp tgt-big.img tgt-stale.img; dd if=stale.img of=tgt-stale.img conv=notrunc status=none
    arm  "IMG green (stale target ok)"   PASS   "WHOLE-MEDIUM"           --image "$PWD/grown.img" --target tgt-stale.img --expect-geom "$GROWNGEOM"
    arm  "IMG RED   (image stale label)" REFUSE "a card written from it" --image "$PWD/stale.img" --target tgt-big.img --expect-geom "$GOODGEOM"
    arm  "IMG RED   (image BS_VolID 0)"  REFUSE "BS_VolID 0"             --image "$PWD/zeroid.img" --target tgt-big.img --expect-geom "$GOODGEOM"
    arm  "IMG RED   (image geom wrong)"  REFUSE "--image geometry is"    --image "$PWD/grown.img" --target tgt-big.img --expect-geom "$GOODGEOM"
    arm  "IMG RED   (image has no FAT)"  REFUSE "carries no FAT volume"  --image "$PWD/raw.img"   --target tgt-big.img --expect-geom "$GROWNGEOM"
    arm  "IMG RED   (image FAT16 by count)" REFUSE "by cluster count"   --image "$PWD/spc8.img"  --target tgt-big.img --expect-geom "$GROWNGEOM"
    arm  "IMG RED   (--image with --src)" REFUSE "mutually exclusive"    --image "$PWD/grown.img" --src "$PWD/src" --target tgt-big.img --expect-geom "$GROWNGEOM"
    arm  "IMG RED   (image does not fit)" REFUSE "would be cut off"      --image "$PWD/grown.img" --target tgt-small.img --expect-geom "$GROWNGEOM"
    # C13: a card the desktop auto-mounted. Its mount point is NOT one of C9's system paths, so C9
    # passes and C13 is the sole refusal — the arm isolates the check it names.
    printf 'mmcblk0  30.2G \nmmcblk0p1 UNAOS-ORIN 30.2G /run/media/pmes/UNAOS-ORIN\nnvme0n1p3 fedora 951.3G /home\n' > topo-mounted.txt
    cp tgt-big.img mmcblk0
    arm  "C13 green (nothing mounted)"   PASS   "PASS  C13-TARGET-QUIET" env LOADCARD10_LSBLK_FIXTURE="$PWD/topo-one.txt" \
             "$SELF" --image "$PWD/grown.img" --target "$PWD/mmcblk0" --expect-geom "$GROWNGEOM"
    arm  "C13 RED   (target mounted)"    REFUSE "MOUNTED partition"      env LOADCARD10_LSBLK_FIXTURE="$PWD/topo-mounted.txt" \
             "$SELF" --image "$PWD/grown.img" --target "$PWD/mmcblk0" --expect-geom "$GROWNGEOM"

    echo "-- C14 WHOLE-MEDIUM WRITE (both polarities) --"
    cp tgt-big.img wimg.img; cp tgt-big.img wimgbad.img
    arm  "IMG RED   (write, no --erase)" REFUSE "--erase-target"         --image "$PWD/grown.img" --target wimg.img --expect-geom "$GROWNGEOM" --write
    arm  "IMG green (write + read-back)" PASS   "identical"              --image "$PWD/grown.img" --target wimg.img --expect-geom "$GROWNGEOM" --write --erase-target
    arm  "IMG RED   (read-back differs)" REFUSE "NOT LOADED"             env MEDIAWRITER_CORRUPT_AFTER_WRITE=1 \
             "$SELF" --image "$PWD/grown.img" --target "$PWD/wimgbad.img" --expect-geom "$GROWNGEOM" --write --erase-target
    # The written medium must now BE the card: same geometry, same label, same BS_VolID as the image
    # — proved by running the STAGING checks against it, which is the very thing the next step does.
    arm  "IMG green (staging accepts it)" PASS  "10/10 passed"             --src "$PWD/src" --target wimg.img --expect-geom "$GROWNGEOM"

    echo
    echo "=== SELFTEST: $pass passed, $fail failed ==="
    [ -f "$PWD/src.FLIGHTID" ] && { echo "--- FLIGHTID produced by the green write ---"; sed 's/^/    /' "$PWD/src.FLIGHTID"; }
    echo "SELFTEST_EXIT=$([ "$fail" -eq 0 ] && echo 0 || echo 1)"
    [ "$fail" -eq 0 ] || return 1
    return 0
}

# ═════════════════════════════════════════════════════════════════════════════════════════════════
[ "$SELFTEST" -eq 1 ] && { selftest; exit $?; }

SRC_DIR=""
if [ -n "$SRC_NAME" ]; then
    case "$SRC_NAME" in /*) SRC_DIR="$SRC_NAME";; *) SRC_DIR="$FLASH/$SRC_NAME";; esac
    SRC_NAME="$(basename "$SRC_DIR")"
fi
PROBE_KIND=""; PROBE_LABEL=""; PROBE_SERIAL=""; PROBE_GEOM=""; PROBE_BLOCKS=""; PROBE_OFFSET=0; PROBE_FAT=""

# WHOLE-MEDIUM MODE takes its own path from here: its own checks, its own dry-run line, its own
# write. The staging path below is untouched, which is the point — this mode was added beside it,
# never through it.
if [ -n "$IMAGE_SRC" ]; then
    case "$IMAGE_SRC" in /*) ;; *) IMAGE_SRC="$PWD/$IMAGE_SRC";; esac
    case "$TARGET" in /*|"") ;; *) TARGET="$PWD/$TARGET";; esac
    run_checks_image "$IMAGE_SRC" "$TARGET" || exit "$RC_REFUSE"
    if [ "$MODE" != "write" ]; then
        echo
        echo "DRY RUN — every check passed and NOTHING was written."
        echo "  To write the WHOLE MEDIUM (this ERASES $TARGET):"
        echo "    $0 --image $IMAGE_SRC --target $TARGET --write --erase-target"
        exit "$RC_PASS"
    fi
    # The second word. `--write` alone means "stage files" everywhere else in this tool; making it
    # also mean "repartition a disk" would make one flag carry two magnitudes of consequence.
    if [ "$ERASE_OK" -ne 1 ]; then
        echo
        echo "media-writer REFUSED — --image --write ERASES the whole of $TARGET (partition table,"
        echo "  FAT volume, native volume, everything). Add --erase-target to say so out loud."
        exit "$RC_REFUSE"
    fi
    do_write_image "$IMAGE_SRC" "$TARGET"; exit $?
fi

run_checks "$SRC_DIR" "$TARGET" || exit "$RC_REFUSE"

if [ "$MODE" != "write" ]; then
    echo
    echo "DRY RUN — every check passed and NOTHING was written."
    echo "  To load: $0 --src $SRC_NAME --target $TARGET${EXPECT_GEOM:+ --expect-geom $EXPECT_GEOM}$([ "$ALLOW_GEOM_ABSENT" -eq 1 ] && printf " --allow-geom-absent") --write"
    exit "$RC_PASS"
fi
do_write "$SRC_DIR" "$TARGET"; exit $?
