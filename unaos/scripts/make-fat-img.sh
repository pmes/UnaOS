#!/bin/bash
# make-fat-img.sh — build a FAT32 test image for the UnaOS read-only FAT reader.
#
# Produces a raw disk image the QEMU usb-storage device can back (see the UNAOS_FATIMG
# knob in builder/src/main.rs and arroyo), populated from the packaged ESP so the kernel's
# `ls`/`cat` see the same EFI/ + kernel.elf layout a real bootable FAT32 stick has.
#
#   ./scripts/make-fat-img.sh part [out.img]   MBR-partitioned FAT32 (MBR@LBA0 -> BPB)   [default]
#   ./scripts/make-fat-img.sh gpt  [out.img]   GPT-partitioned FAT32 (EFI PART@LBA1 -> BPB)
#   ./scripts/make-fat-img.sh p16  [out.img]   MBR-partitioned FAT16 (fixed root dir, 16-bit FAT)
#   ./scripts/make-fat-img.sh sf   [out.img]   superfloppy FAT32     (BPB@LBA0, no MBR)
#
# macOS only (uses hdiutil / diskutil / newfs_msdos — no mtools dependency). The image is a
# virtual disk image the whole time; this script never touches a physical disk. Env: FAT_IMG_MB
# sets the image size (default 96 MiB — must stay >= ~34 MiB so the FS is FAT32, not FAT16).
set -euo pipefail

WORKSPACE_DIR="$(cd "$(dirname "$0")/.." && pwd)"
LAYOUT="${1:-part}"
ESP_DIR="${WORKSPACE_DIR}/target/x86_64_esp"
FSTYPE="MS-DOS FAT32"   # p16 overrides to FAT16
DEFAULT_MB=96

case "$LAYOUT" in
    part) OUT="${2:-${WORKSPACE_DIR}/builder/fat.img}";     DESC="MBR FAT32"; SCHEME=MBR;;
    gpt)  OUT="${2:-${WORKSPACE_DIR}/builder/fat-gpt.img}"; DESC="GPT FAT32"; SCHEME=GPT;;
    p16)  OUT="${2:-${WORKSPACE_DIR}/builder/fat16.img}";   DESC="MBR FAT16"; SCHEME=MBR; FSTYPE="MS-DOS FAT16"; DEFAULT_MB=32;;
    sf)   OUT="${2:-${WORKSPACE_DIR}/builder/fat-sf.img}";  DESC="superfloppy FAT32";;
    *) echo "usage: $0 [part|gpt|p16|sf] [out.img]" >&2; exit 1;;
esac
SIZE_MB="${FAT_IMG_MB:-$DEFAULT_MB}"

if [ "$(uname)" != "Darwin" ]; then
    echo "make-fat-img.sh: this helper uses macOS hdiutil/diskutil/newfs_msdos." >&2
    exit 1
fi

DISK=""
cleanup() {
    # Best-effort: unmount + detach the virtual disk if we still hold it.
    if [ -n "$DISK" ]; then
        diskutil unmountDisk force "$DISK" >/dev/null 2>&1 || true
        hdiutil detach "$DISK" >/dev/null 2>&1 || true
    fi
}
trap cleanup EXIT

echo "==> Building ${DESC} FAT32 image: ${OUT} (${SIZE_MB} MiB)"
rm -f "$OUT"
mkfile -n "${SIZE_MB}m" "$OUT"

# Attach the raw image as a whole-disk virtual device (no mount attempt).
DISK="$(hdiutil attach -nomount -imagekey diskimage-class=CRawDiskImage "$OUT" | head -1 | awk '{print $1}')"
if [ -z "$DISK" ] || [ ! -e "$DISK" ]; then
    echo "make-fat-img.sh: failed to attach image" >&2; exit 1
fi
# SAFETY: refuse to operate on anything that is not a hdiutil-backed virtual disk image.
if ! diskutil info "$DISK" | grep -q "Virtual:.*Yes"; then
    echo "make-fat-img.sh: $DISK is not a virtual disk image — refusing to partition/format" >&2
    exit 1
fi
echo "    attached as ${DISK}"

if [ "$LAYOUT" = "sf" ]; then
    # Superfloppy: format the whole raw device as FAT32 (BPB at LBA0, no partition table).
    RDISK="${DISK/disk/rdisk}"
    newfs_msdos -F 32 -c 1 -v UNAOS "$RDISK" >/dev/null 2>&1
    diskutil mount "$DISK" >/dev/null
    VOLDEV="$DISK"
else
    # MBR or GPT scheme + one FAT partition spanning the disk. MBR writes a partition table at LBA0
    # pointing at the BPB (typically LBA63); GPT writes a protective MBR at LBA0, an "EFI PART"
    # header at LBA1, and an entry array pointing at the BPB (typically LBA2048).
    diskutil partitionDisk "$DISK" "$SCHEME" "$FSTYPE" UNAOS 100% >/dev/null
    VOLDEV="${DISK}s1"
fi

# Resolve the real mount point (avoid assuming /Volumes/UNAOS — a stale mount could shift it).
MNT="$(diskutil info "$VOLDEV" | awk -F': +' '/Mount Point/ {print $2}')"
if [ -z "$MNT" ] || [ ! -d "$MNT" ]; then
    echo "make-fat-img.sh: could not find mount point for ${VOLDEV}" >&2; exit 1
fi
echo "    mounted at ${MNT}"

# Populate from the packaged ESP (real bootable layout) when present; otherwise leave a
# minimal placeholder so `ls`/`cat` still have something to read.
if [ -d "$ESP_DIR" ]; then
    echo "    populating from ${ESP_DIR}"
    COPYFILE_DISABLE=1 cp -R "${ESP_DIR}/EFI" "${MNT}/"
    [ -f "${ESP_DIR}/kernel.elf" ] && COPYFILE_DISABLE=1 cp "${ESP_DIR}/kernel.elf" "${MNT}/kernel.elf"
else
    echo "    WARNING: ${ESP_DIR} absent — run './arroyo esp-x86' first for the real payload."
    mkdir -p "${MNT}/EFI/BOOT"
    printf 'placeholder BOOTX64.EFI\n' > "${MNT}/EFI/BOOT/BOOTX64.EFI"
    printf 'placeholder kernel.elf\n'   > "${MNT}/kernel.elf"
fi
# Make this image NON-bootable: rename the UEFI removable-media fallback (\EFI\BOOT\BOOTX64.EFI) so
# OVMF boots the separate fat:rw ESP drive (which always carries the freshly built kernel under
# test) instead of this usb-storage image. That leaves the usb-storage as a pristine data disk the
# kernel enumerates cleanly -- avoiding the OVMF-USB-boot-then-kernel-re-enumerate flakiness -- and
# means the image only needs rebuilding when its *file contents* change, not on every kernel edit.
# (`ls` lists the root only, so EFI/ + kernel.elf still show; a real metal stick keeps BOOTX64.EFI.)
if [ -f "${MNT}/EFI/BOOT/BOOTX64.EFI" ]; then
    mv "${MNT}/EFI/BOOT/BOOTX64.EFI" "${MNT}/EFI/BOOT/BOOTX64.REM"
fi
# A small text file for the `cat` milestone.
printf 'hello from the UnaOS FAT reader\nthis file lives on a real FAT32 volume\n' > "${MNT}/hello.txt"
printf 'UnaOS read-only FAT32/16 reader test volume (%s layout).\n' "$LAYOUT" > "${MNT}/readme.txt"

# U2: the x86 ring-3 "hello from disk" program (crates/user-blob-x86 → target/hello.bin, built by
# arroyo's build_user_hello_x86). Copy it onto the image as HELLO.BIN so the kernel's U2 FAT loader
# finds + runs it in ring 3. Read straight from target/hello.bin (fresh — every x86 build path builds
# it before make-fat-img runs), independent of whether the ESP payload carried it.
HELLO_BIN="${WORKSPACE_DIR}/target/hello.bin"
if [ -f "$HELLO_BIN" ]; then
    COPYFILE_DISABLE=1 cp "$HELLO_BIN" "${MNT}/HELLO.BIN"
    echo "    added HELLO.BIN ($(wc -c < "$HELLO_BIN" | tr -d ' ') bytes) for the U2 loader"
else
    echo "    WARNING: ${HELLO_BIN} absent — image has no HELLO.BIN (run './arroyo fat-img' via arroyo, not make-fat-img.sh directly)"
fi

# Strip macOS metadata (AppleDouble ._ files, Spotlight/fseventsd) so `ls` shows a clean tree.
sync
find "$MNT" -name '._*' -delete 2>/dev/null || true
rm -rf "${MNT}/.fseventsd" "${MNT}/.Spotlight-V100" "${MNT}/.Trashes" "${MNT}/.TemporaryItems" 2>/dev/null || true
sync

diskutil unmountDisk "$DISK" >/dev/null
hdiutil detach "$DISK" >/dev/null
DISK=""
echo "==> Done: ${OUT}"
echo "    Test in QEMU:  UNAOS_FATIMG=1 ./arroyo test 25   (partitioned)"
echo "                   UNAOS_FATIMG=sf ./arroyo test 25  (superfloppy)"
