#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# make-pi-install-src.sh — build the INSTALL-PI-2 self-clone SOURCE card.
#
# The Pi self-clone reproduces the running system's OWN boot media. This helper stages a raw MBR/FAT32
# disk image that looks like a real Pi boot card — a FAT32 boot partition carrying the freshly-built boot
# files (kernel8.img + config.txt + the GPU firmware) — for `./arroyo kernel8-install` to attach in the
# single raspi4b SD slot. The booted installer mounts THIS partition, snapshots its boot tree into memory,
# then repartitions the same card to GPT+FAT32 and clones the buffered tree back, sha-verifying every file.
#
# Usage: make-pi-install-src.sh <kernel8-dir> <out.img> [size_mb]
#
# Only 8.3-CLEAN names are staged so the clone round-trips exactly (no LFN short-name mangling), which lets
# the host-side verify byte-compare each cloned file against its source. Long-name (LFN) preservation and
# whether the CLONED card actually BOOTS a real Pi are the named metal follow-ups (installer_engine.md).
#
# macOS only (uses hdiutil / diskutil / newfs_msdos — no mtools dependency). The image is a virtual disk
# image the whole time; this script never touches a physical disk.
set -euo pipefail

SRC="${1:?kernel8 boot dir}"
OUT="${2:?out.img}"
SIZE_MB="${3:-128}"

if [ "$(uname)" != "Darwin" ]; then
    echo "make-pi-install-src.sh: this helper uses macOS hdiutil/diskutil/newfs_msdos." >&2
    exit 1
fi
[ -d "$SRC" ] || { echo "make-pi-install-src.sh: boot dir not found: $SRC" >&2; exit 1; }

DISK=""
cleanup() {
    if [ -n "$DISK" ]; then
        diskutil unmountDisk force "$DISK" >/dev/null 2>&1 || true
        hdiutil detach "$DISK" >/dev/null 2>&1 || true
    fi
}
trap cleanup EXIT

echo "==> Building Pi self-clone SOURCE card: ${OUT} (${SIZE_MB} MiB, MBR FAT32)"
rm -f "$OUT"
mkfile -n "${SIZE_MB}m" "$OUT"

# Attach the raw image as a whole-disk virtual device (no mount attempt).
DISK="$(hdiutil attach -nomount -imagekey diskimage-class=CRawDiskImage "$OUT" | head -1 | awk '{print $1}')"
if [ -z "$DISK" ] || [ ! -e "$DISK" ]; then
    echo "make-pi-install-src.sh: failed to attach image" >&2; exit 1
fi
# SAFETY: refuse to operate on anything that is not a hdiutil-backed virtual disk image.
if ! diskutil info "$DISK" | grep -q "Virtual:.*Yes"; then
    echo "make-pi-install-src.sh: $DISK is not a virtual disk image — refusing to partition/format" >&2
    exit 1
fi
echo "    attached as ${DISK}"

diskutil partitionDisk "$DISK" MBR "MS-DOS FAT32" UNAOS-PI 100% >/dev/null
VOLDEV="${DISK}s1"
# Resolve the mount point, retrying (and explicitly mounting) since the auto-mount after partitionDisk
# can lag; never assume /Volumes/UNAOS (a stale physical mount could shift it).
MNT=""
for _try in 1 2 3 4 5 6; do
    MNT="$(diskutil info "$VOLDEV" 2>/dev/null | awk -F': +' '/Mount Point/ {print $2}')"
    if [ -n "$MNT" ] && [ -d "$MNT" ]; then
        break
    fi
    MNT=""
    diskutil mount "$VOLDEV" >/dev/null 2>&1 || true
    sleep 1
done
if [ -z "$MNT" ] || [ ! -d "$MNT" ]; then
    echo "make-pi-install-src.sh: could not find mount point for ${VOLDEV}" >&2; exit 1
fi
echo "    mounted at ${MNT}"

# Stage a curated, 8.3-CLEAN boot set (exact round-trip through the 8.3 TreeWriter). kernel8.img IS the
# running system's own kernel image — the truest "self". A subdir exercises the multi-level clone path.
stage() { # <src-name> <dest-name>
    if [ -f "${SRC}/$1" ]; then
        COPYFILE_DISABLE=1 cp "${SRC}/$1" "${MNT}/$2"
        echo "    staged $2 ($(wc -c < "${SRC}/$1" | tr -d ' ') bytes)"
    else
        echo "    WARNING: ${SRC}/$1 absent — not staged"
    fi
}
stage kernel8.img KERNEL8.IMG
stage config.txt  CONFIG.TXT
stage start4.elf  START4.ELF
stage fixup4.dat  FIXUP4.DAT
# A subdirectory (mirrors a real ESP's EFI/BOOT depth) with one clean-named file, so the clone exercises
# subdirectory recursion, not just a flat root.
mkdir -p "${MNT}/OVERLAYS"
if [ -f "${SRC}/overlays/miniuart-bt.dtbo" ]; then
    COPYFILE_DISABLE=1 cp "${SRC}/overlays/miniuart-bt.dtbo" "${MNT}/OVERLAYS/OVERLAY1.DAT"
    echo "    staged OVERLAYS/OVERLAY1.DAT"
else
    printf 'unaos self-clone subdir fixture\n' > "${MNT}/OVERLAYS/OVERLAY1.DAT"
    echo "    staged OVERLAYS/OVERLAY1.DAT (placeholder)"
fi

# Strip macOS metadata so the cloned tree is clean.
sync
find "$MNT" -name '._*' -delete 2>/dev/null || true
rm -rf "${MNT}/.fseventsd" "${MNT}/.Spotlight-V100" "${MNT}/.Trashes" "${MNT}/.TemporaryItems" 2>/dev/null || true
sync

diskutil unmountDisk "$DISK" >/dev/null
hdiutil detach "$DISK" >/dev/null
DISK=""
echo "==> Done: ${OUT}  ($(du -h "$OUT" | cut -f1))"
