#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Build a flashable FAT32/MBR disk image (.img) from a directory of Pi boot files, for Raspberry Pi
# Imager ("Use custom"). macOS sandboxes direct writes to removable disks, so we build an image file
# (a disk *image*, not the physical card — which TCC allows) and let Imager, which has the
# removable-volume permission, flash it.
#
# Usage: make-pi-img.sh <src-dir> <out.img> [size_mb] [unafs.img]
#
# BeFS-K3: an optional 4th arg names a raw UnaFS volume image to carry as MBR partition 2
# (type 0x7f). The FAT32 boot partition then takes size_mb-8 MB and the unafs volume rides
# in the reserved 8 MB tail. The tail is carved as diskutil "Free Space" (NO volume, NO
# auto-mount — see the /Volumes hazard note below) and the partition-2 MBR entry + content
# are written into the image file by byte offset after everything is detached.
#
# BUILD-2 (concurrency safety): the whole image-build critical section is serialized on a
# per-OUT lock dir ("$OUT".buildlock). Concurrent kernel8/kernel8-test builds share one OUT
# image; without the lock a second run races the first's hdiutil attach and fails
# `diskutil partitionDisk` with -69772 "A writable disk is required", and a crashed run can
# strand a stale attach that blocks every later build. Guards, all keyed on OUT:
#   1. Lock: mkdir-based (atomic on macOS). Holder writes its PID; a waiter breaks the lock
#      only when the holder PID is dead OR the lock is older than $LOCK_STALE_SECS, printing a
#      loud "stale lock reclaimed" line. A fresh waiter blocks (bounded, dots to stderr) up to
#      $LOCK_WAIT_SECS, then errors naming the holder PID.
#   2. Always-detach: a single EXIT/signal trap detaches whatever hdiutil device this run
#      attached, on every exit path, and releases the lock.
#   3. Stale-attach reclaim: on entry, any pre-existing hdiutil attach of THIS OUT (a strand
#      from a crashed prior run) is detached with a loud "stale attach reclaimed" line before
#      we proceed.
# Everything below the guards is byte-identical to the pre-BUILD-2 script (layout, ditto,
# UNAOS-PI label, MBR patching). Env knobs: MAKEPI_LOCK_WAIT_SECS, MAKEPI_LOCK_STALE_SECS.
set -euo pipefail
SRC="${1:?src dir}"; OUT="${2:?out.img}"; SIZE_MB="${3:-256}"; UNAFS_IMG="${4:-}"

# ---- BUILD-2 concurrency guards -------------------------------------------------------------
LOCK_WAIT_SECS="${MAKEPI_LOCK_WAIT_SECS:-1200}"   # a waiter blocks at most this long
LOCK_STALE_SECS="${MAKEPI_LOCK_STALE_SECS:-900}"  # a lock older than this (w/ dead PID or not) is breakable
LOCKDIR="${OUT}.buildlock"
LOCK_HELD=""   # set once we own LOCKDIR
DEV=""         # current hdiutil device; cleanup detaches it
MNT=""         # current private mountpoint; cleanup rmdir's it

# Absolute OUT path, for matching hdiutil info's resolved image-path (dirname must exist; the
# .img itself may not yet). Falls back to OUT verbatim if the dir cannot be resolved.
if OUT_DIR_ABS=$(cd "$(dirname "$OUT")" 2>/dev/null && pwd); then
    OUT_ABS="$OUT_DIR_ABS/$(basename "$OUT")"
else
    OUT_ABS="$OUT"
fi

cleanup() {
    local rc=$?
    [ -n "$DEV" ] && hdiutil detach "$DEV" >/dev/null 2>&1 || true
    [ -n "$MNT" ] && rmdir "$MNT" 2>/dev/null || true
    [ -n "$LOCK_HELD" ] && rm -rf "$LOCKDIR" 2>/dev/null || true
    return $rc
}
trap cleanup EXIT INT TERM

# Detach any lingering hdiutil attach of THIS image (crashed-run strand). Whole-disk devices only.
reclaim_stale_attach() {
    local devs
    devs=$(hdiutil info 2>/dev/null | awk -v want="$OUT_ABS" '
        /^image-path[ \t]*:/ { p=$0; sub(/^image-path[ \t]*:[ \t]*/,"",p); path=p }
        /^\/dev\/disk[0-9]+[ \t]/ { if (path==want) print $1 }
    ')
    local d
    for d in $devs; do
        echo "make-pi-img: stale attach reclaimed — detaching $d (leftover mount of $OUT_ABS)" >&2
        hdiutil detach "$d" >/dev/null 2>&1 || hdiutil detach "$d" -force >/dev/null 2>&1 || true
    done
}

acquire_lock() {
    local waited=0 dotted=0 holder mtime now age
    while ! mkdir "$LOCKDIR" 2>/dev/null; do
        holder=$(cat "$LOCKDIR/pid" 2>/dev/null || echo "")
        mtime=$(stat -f %m "$LOCKDIR" 2>/dev/null || echo 0)
        now=$(date +%s); age=$(( now - mtime ))
        if [ -n "$holder" ] && ! kill -0 "$holder" 2>/dev/null; then
            echo "make-pi-img: stale lock reclaimed — holder PID $holder is dead (lock age ${age}s)" >&2
            rm -rf "$LOCKDIR" 2>/dev/null || true; continue
        fi
        if [ "$age" -gt "$LOCK_STALE_SECS" ]; then
            echo "make-pi-img: stale lock reclaimed — lock age ${age}s > ${LOCK_STALE_SECS}s (holder PID ${holder:-unknown})" >&2
            rm -rf "$LOCKDIR" 2>/dev/null || true; continue
        fi
        if [ "$waited" -ge "$LOCK_WAIT_SECS" ]; then
            echo "" >&2
            echo "make-pi-img: timed out after ${waited}s waiting for build lock held by PID ${holder:-unknown} ($LOCKDIR)" >&2
            exit 1
        fi
        if [ "$dotted" -eq 0 ]; then
            printf 'make-pi-img: waiting for build lock held by PID %s ' "${holder:-unknown}" >&2
            dotted=1
        fi
        printf '.' >&2
        sleep 3; waited=$(( waited + 3 ))
    done
    [ "$dotted" -eq 1 ] && echo "" >&2
    echo $$ > "$LOCKDIR/pid"
    LOCK_HELD=1
}

acquire_lock
reclaim_stale_attach
# ---- end BUILD-2 guards; critical section below is unchanged --------------------------------

# Reserve an 8 MB tail for the unafs partition when one is staged.
UNAFS_MB=8
if [ -n "$UNAFS_IMG" ]; then
    [ -f "$UNAFS_IMG" ] || { echo "unafs image not found: $UNAFS_IMG" >&2; exit 1; }
    UNAFS_BYTES=$(stat -f %z "$UNAFS_IMG")
    [ "$UNAFS_BYTES" -le $((UNAFS_MB * 1024 * 1024)) ] || {
        echo "unafs image ${UNAFS_BYTES}B exceeds the ${UNAFS_MB}MB reserved tail" >&2; exit 1; }
    FAT_SPEC="$((SIZE_MB - UNAFS_MB))M"
else
    FAT_SPEC="100%"
fi

dd if=/dev/zero of="$OUT" bs=1m count="$SIZE_MB" 2>/dev/null
DEV=$(hdiutil attach -nomount "$OUT" | awk 'NR==1{print $1; exit}')
# (cleanup trap already installed above detaches $DEV on any exit path)
if [ -n "$UNAFS_IMG" ]; then
    diskutil partitionDisk "$DEV" 2 MBR "MS-DOS FAT32" UNAOS-PI "$FAT_SPEC" "Free Space" FREE R >/dev/null
else
    diskutil partitionDisk "$DEV" 1 MBR "MS-DOS FAT32" UNAOS-PI 100% >/dev/null
fi
# NEVER address the volume by /Volumes name: if a physical card/stick named UNAOS is mounted,
# partitionDisk's auto-mount lands the image at "/Volumes/UNAOS 1" and a name-addressed ditto
# writes onto the PHYSICAL medium (found the hard way, 2026-07-07: a Pi boot set ditto'd onto
# the Orin boot stick). Detach and re-attach the image at a private mountpoint instead — the
# ditto target is then the image by construction, whatever else is mounted.
hdiutil detach "$DEV" >/dev/null
DEV=""
MNT=$(mktemp -d "${TMPDIR:-/tmp}/unaos-pi-img.XXXXXX")
DEV=$(hdiutil attach -mountpoint "$MNT" "$OUT" | awk 'NR==1{print $1; exit}')
# (cleanup trap detaches $DEV and rmdir's $MNT on any exit path)
ditto "$SRC" "$MNT"
dot_clean -m "$MNT" 2>/dev/null || true
hdiutil detach "$DEV" >/dev/null
DEV=""
rmdir "$MNT" 2>/dev/null || true
MNT=""
# The Pi GPU ROM wants the FAT32 partition typed 0x0C (LBA); diskutil makes 0x0B (CHS). Patch the MBR
# partition-1 type byte at offset 450.
printf '\x0c' | dd of="$OUT" bs=1 seek=450 count=1 conv=notrunc 2>/dev/null

# BeFS-K3: write the unafs partition — MBR entry 2 (type 0x7f, offset 462) pointing at the
# reserved tail, then the raw volume bytes at that LBA. Done entirely by byte offset into the
# detached image file, so no volume ever mounts and no /Volumes name is ever addressed.
if [ -n "$UNAFS_IMG" ]; then
    le32() { # $1 = u32 value -> 4 LE bytes on stdout
        printf "$(printf '\\x%02x\\x%02x\\x%02x\\x%02x' \
            $(($1 & 255)) $((($1 >> 8) & 255)) $((($1 >> 16) & 255)) $((($1 >> 24) & 255)))"
    }
    # Partition 1 extent from its MBR entry (LBA start at 454, sector count at 458, both LE u32).
    P1_START=$(dd if="$OUT" bs=1 skip=454 count=4 2>/dev/null | od -An -tu4 | tr -d ' ')
    P1_COUNT=$(dd if="$OUT" bs=1 skip=458 count=4 2>/dev/null | od -An -tu4 | tr -d ' ')
    # Partition 2 starts at the next 2048-sector (1 MB) boundary after partition 1 and runs to
    # the end of the image.
    P2_START=$(( (P1_START + P1_COUNT + 2047) / 2048 * 2048 ))
    P2_COUNT=$(( SIZE_MB * 2048 - P2_START ))
    [ "$P2_COUNT" -ge $(( UNAFS_BYTES / 512 )) ] || {
        echo "unafs tail too small: ${P2_COUNT} sectors < image" >&2; exit 1; }
    # Entry 2 at 462: status 0x00, CHS fillers 0xFF (LBA world), type 0x7f, start, count.
    { printf '\x00\xff\xff\xff\x7f\xff\xff\xff'; le32 "$P2_START"; le32 "$P2_COUNT"; } \
        | dd of="$OUT" bs=1 seek=462 count=16 conv=notrunc 2>/dev/null
    dd if="$UNAFS_IMG" of="$OUT" bs=512 seek="$P2_START" conv=notrunc 2>/dev/null
    echo "unafs partition: LBA $P2_START, $P2_COUNT sectors (volume $(du -h "$UNAFS_IMG" | cut -f1))"
fi
echo "built $OUT  ($(du -h "$OUT" | cut -f1), sha256 $(shasum -a 256 "$OUT" | cut -c1-16)...)"
