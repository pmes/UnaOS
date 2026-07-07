#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Build a flashable FAT32/MBR disk image (.img) from a directory of Pi boot files, for Raspberry Pi
# Imager ("Use custom"). macOS sandboxes direct writes to removable disks, so we build an image file
# (a disk *image*, not the physical card — which TCC allows) and let Imager, which has the
# removable-volume permission, flash it.
#
# Usage: make-pi-img.sh <src-dir> <out.img> [size_mb]
set -euo pipefail
SRC="${1:?src dir}"; OUT="${2:?out.img}"; SIZE_MB="${3:-256}"

dd if=/dev/zero of="$OUT" bs=1m count="$SIZE_MB" 2>/dev/null
DEV=$(hdiutil attach -nomount "$OUT" | awk 'NR==1{print $1; exit}')
trap 'hdiutil detach "$DEV" >/dev/null 2>&1 || true' EXIT
diskutil partitionDisk "$DEV" 1 MBR "MS-DOS FAT32" UNAOS 100% >/dev/null
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
echo "built $OUT  ($(du -h "$OUT" | cut -f1), sha256 $(shasum -a 256 "$OUT" | cut -c1-16)...)"
