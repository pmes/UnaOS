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
set -euo pipefail
SRC="${1:?src dir}"; OUT="${2:?out.img}"; SIZE_MB="${3:-256}"; UNAFS_IMG="${4:-}"

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
trap 'hdiutil detach "$DEV" >/dev/null 2>&1 || true' EXIT
if [ -n "$UNAFS_IMG" ]; then
    diskutil partitionDisk "$DEV" 2 MBR "MS-DOS FAT32" UNAOS "$FAT_SPEC" "Free Space" FREE R >/dev/null
else
    diskutil partitionDisk "$DEV" 1 MBR "MS-DOS FAT32" UNAOS 100% >/dev/null
fi
# NEVER address the volume by /Volumes name: if a physical card/stick named UNAOS is mounted,
# partitionDisk's auto-mount lands the image at "/Volumes/UNAOS 1" and a name-addressed ditto
# writes onto the PHYSICAL medium (found the hard way, 2026-07-07: a Pi boot set ditto'd onto
# the Orin boot stick). Detach and re-attach the image at a private mountpoint instead — the
# ditto target is then the image by construction, whatever else is mounted.
hdiutil detach "$DEV" >/dev/null
trap - EXIT
MNT=$(mktemp -d "${TMPDIR:-/tmp}/unaos-pi-img.XXXXXX")
DEV=$(hdiutil attach -mountpoint "$MNT" "$OUT" | awk 'NR==1{print $1; exit}')
trap 'hdiutil detach "$DEV" >/dev/null 2>&1 || true; rmdir "$MNT" 2>/dev/null || true' EXIT
ditto "$SRC" "$MNT"
dot_clean -m "$MNT" 2>/dev/null || true
hdiutil detach "$DEV" >/dev/null
rmdir "$MNT" 2>/dev/null || true
trap - EXIT
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
