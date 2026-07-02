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
ditto "$SRC" /Volumes/UNAOS
dot_clean -m /Volumes/UNAOS 2>/dev/null || true
hdiutil detach "$DEV" >/dev/null
trap - EXIT
# The Pi GPU ROM wants the FAT32 partition typed 0x0C (LBA); diskutil makes 0x0B (CHS). Patch the MBR
# partition-1 type byte at offset 450.
printf '\x0c' | dd of="$OUT" bs=1 seek=450 count=1 conv=notrunc 2>/dev/null
echo "built $OUT  ($(du -h "$OUT" | cut -f1), sha256 $(shasum -a 256 "$OUT" | cut -c1-16)...)"
