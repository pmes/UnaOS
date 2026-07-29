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

# WINX-5: the x86 EL0 persistence program (crates/user-stat → target/STAT-X86.ELF, built by arroyo's
# build_user_stat_x86). Copy it onto the image as STAT.ELF so the x86 shell's `run`/`bg` — which read the
# FAT boot partition's root — can load it (`bg /fat/STAT.ELF`). Same shape and same freshness argument as
# the HELLO.BIN hunk above. Un-suffixed on the volume so the operator command reads the same on both
# arches; the -X86 suffix exists only in target/, where both arches' images share one directory.
STAT_ELF="${WORKSPACE_DIR}/target/STAT-X86.ELF"
if [ -f "$STAT_ELF" ]; then
    COPYFILE_DISABLE=1 cp "$STAT_ELF" "${MNT}/STAT.ELF"
    echo "    added STAT.ELF ($(wc -c < "$STAT_ELF" | tr -d ' ') bytes) for the run/bg loader"
else
    echo "    WARNING: ${STAT_ELF} absent — image has no STAT.ELF (run './arroyo fat-img' via arroyo)"
fi

# U9x M2: plant a DEDICATED writable scratch file (NEVER HELLO.BIN — other fixtures load that as EL0
# code). 1 KiB of 0xEE filler, mirroring the pi4 image plant (arroyo's kernel8 SCRATCH.BIN block): the
# U9x fixture opens it RW, seeks to 520, overwrites a 16-byte pattern, and the launcher flushes that
# staged write to disk and raw-re-reads the sector to prove it landed (bytes changed + size unchanged).
# 0356 octal == 0xEE; matches the kernel's in-memory const seed (U9X_SCRATCH_FILL) byte-for-byte, so the
# no-FAT in-memory core and the on-disk backing serve identical pre-image bytes.
head -c 1024 /dev/zero | tr '\000' '\356' > "${MNT}/SCRATCH.BIN"
echo "    added SCRATCH.BIN (1024 bytes of 0xEE) for the U9x File-write demo"

# U10 GROW: plant a DEDICATED 1-cluster file the growth fixture extends across the cluster boundary.
# 512 bytes of 0xC1 (0301 octal) == exactly one 512-B cluster on the FAT32 layouts (part/gpt/sf); the
# fixture seeks to 512 (EOF) and appends a 16-byte pattern, so the file grows to 528 and (on 512-B
# clusters) allocates + zero-fills + chains a SECOND cluster. 0xC1 matches the kernel's staged seed
# (U10_GROW_SEED) byte-for-byte, so a read-back before any write sees the original filler. (The launcher
# self-heals a prior boot's grown copy on a persistent metal card, so re-runs stay honest.)
head -c 512 /dev/zero | tr '\000' '\301' > "${MNT}/GROW.BIN"
echo "    added GROW.BIN (512 bytes of 0xC1) for the U10 file-growth demo"

# STOR-1 S8: plant a DEDICATED writable-dynamic scratch file (NEVER a staged name — HELLO/SCRATCH/GROW.BIN —
# nor README.TXT, whose prefix the S7 witness checks). 64 bytes of 0xA5 (0245 octal): the S8 write witness
# opens it RW off the pre-stage set, overwrites a 16-byte pattern at offset 8 live, reads it back, then
# RESTORES the 0xA5 seed — so the image is left pristine and the witness is idempotent across boots/power-cuts.
# Overwrite-only: the file never grows, so its 64-byte size is immutable for the boot.
head -c 64 /dev/zero | tr '\000' '\245' > "${MNT}/S8W.BIN"
echo "    added S8W.BIN (64 bytes of 0xA5) for the STOR-1 S8 dynamic-write witness"

# SINKHOLE-1/ZEOLITE-2 (zeolite): the DNS resolver's blocklist, in real hosts-file format — the format
# actual sinkhole lists ship in (Steven Black hosts, AdAway, etc.): an IP redirect target (0.0.0.0 or
# 127.0.0.1) followed by whitespace and the domain, with '#'/';' comments and blank lines tolerated. The
# resolver's hardened hosts-format parser (ZEOLITE-2 M1) skips the leading IP field to the DOMAIN, ignores
# comments/blank lines, matches case-insensitively, and does label-boundary SUFFIX matching (M2, so a
# blocked base domain sinkholes its subdomains). The fixture opens this via the S7 dynamic-open path
# (ring-3 SYS_OPEN RO + SYS_READ) — a genuine STOR-feeds-NET composition. ads.example is the blocked name
# the sinkhole witness answers with 0.0.0.0; track.example is the required second entry. "una.os" (the
# forward self-test name) is deliberately ABSENT so it is forwarded upstream, not sinkholed.
cat > "${MNT}/BLOCK.TXT" <<'EOF'
# zeolite DNS sinkhole blocklist (hosts format)
0.0.0.0 ads.example
0.0.0.0 track.example   # inline comment tolerated

; semicolon comments and blank lines are skipped
127.0.0.1 telemetry.example
EOF
echo "    added BLOCK.TXT (zeolite hosts-format blocklist: ads.example, track.example, telemetry.example)"

# PI-FS-3: plant VFAT LONG FILENAMES + a NESTED subdirectory tree so the read-only reader's LFN parse
# and arbitrary-depth traversal are exercised in QEMU (UNAOS_FATIMG=1 ./arroyo test-arm). newfs_msdos +
# cp generate real 0x0F LFN component slots for any name that is not a valid 8.3 short name; a mixed-case
# and a spaces-and-mixed-case name each force an LFN run (the short entry mangles to e.g. LONGFI~1.TXT).
printf 'this file has a VFAT long filename\n' > "${MNT}/Long Filename Example.txt"
printf 'mixed-case long name (LFN, distinct short entry)\n' > "${MNT}/MixedCaseName.md"
echo "    added LFN files (Long Filename Example.txt, MixedCaseName.md)"
# Nested tree: root/subdir/nested with a file at the deepest level — proves traversal past one level.
mkdir -p "${MNT}/subdir/Nested Directory"
printf 'a file two levels deep\n' > "${MNT}/subdir/Nested Directory/deep file.txt"
printf 'level one file\n' > "${MNT}/subdir/level1.txt"
echo "    added nested tree (subdir/ -> 'Nested Directory'/ -> 'deep file.txt')"

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
