#!/bin/bash
# identify-card.sh — classify a mounted UnaOS boot card by its CONTENTS: which platform's media is
# this? Prints one verdict line "PLATFORM<TAB>reason" where PLATFORM ∈ {JETSON, RMBP, PI4, UNKNOWN}.
#
# All three tracks flash the same FAT32 stick/SD ("UNAOS"), so at a shared bench you cannot tell from
# the label whose card it is — you have to look inside. The authoritative discriminant is the kernel
# image's ELF e_machine (aarch64 vs x86_64); the EFI stub name (BOOTAA64 vs BOOTX64) and the tegra
# string count corroborate; a Pi 4 card is VideoCore-firmware-booted (kernel8.img, no UEFI).
#
#   JETSON  aarch64 UEFI  — EFI/BOOT/BOOTAA64.EFI + aarch64 kernel.elf (Jetson Orin Nano)
#   RMBP    x86_64  UEFI  — EFI/BOOT/BOOTX64.EFI  + x86_64  kernel.elf (2012 rMBP / x86)
#   PI4     bare-metal    — kernel8.img / Pi firmware, no EFI/ (Raspberry Pi 4)
#   UNKNOWN — none of the above
#
# Usage:  identify-card.sh [MOUNTPOINT]        (default: /Volumes/UNAOS)
# Always exits 0 — UNKNOWN is a result, not an error. Read only; never writes to the card.
set -u

MP="${1:-/Volumes/UNAOS}"
if [ ! -d "$MP" ]; then
    printf 'UNKNOWN\tno such mount point: %s\n' "$MP"
    exit 0
fi

# ELF e_machine (2 bytes @ offset 0x12, little-endian) of $1 -> "aarch64" | "x86_64" | "".
elf_machine() {
    local f="$1"
    [ -f "$f" ] || { echo ""; return; }
    # Confirm the ELF magic (7f 45 4c 46) before trusting the offset.
    [ "$(od -An -tx1 -N4 "$f" 2>/dev/null | tr -d ' \n')" = "7f454c46" ] || { echo ""; return; }
    case "$(od -An -tx1 -j18 -N2 "$f" 2>/dev/null | tr -d ' \n')" in
        3e00) echo "x86_64" ;;
        b700) echo "aarch64" ;;
        *)    echo "" ;;
    esac
}

# 1. Pi 4 bare-metal: VideoCore firmware boot — kernel8.img / start4.elf / (config.txt without EFI).
if [ -f "$MP/kernel8.img" ] || [ -f "$MP/start4.elf" ] || [ -f "$MP/bootcode.bin" ] \
   || { [ -f "$MP/config.txt" ] && [ ! -d "$MP/EFI" ]; }; then
    printf 'PI4\tkernel8.img / Pi firmware present, no UEFI — Raspberry Pi 4 bare-metal media\n'
    exit 0
fi

# 2. UEFI media (jetson aarch64 / rmbp x86). Trust the kernel ELF machine first; fall back to the
#    EFI stub name if kernel.elf is absent/unreadable.
mach="$(elf_machine "$MP/kernel.elf")"
efi=""
[ -f "$MP/EFI/BOOT/BOOTAA64.EFI" ] && efi="aarch64"
[ -f "$MP/EFI/BOOT/BOOTX64.EFI" ]  && efi="x86_64"
arch="${mach:-$efi}"

case "$arch" in
    aarch64)
        # Only the jetson track ships aarch64 UEFI media in this repo.
        tegra="$(strings "$MP/kernel.elf" 2>/dev/null | grep -c 'tegra:')"
        printf 'JETSON\taarch64 UEFI (BOOTAA64.EFI + aarch64 kernel.elf, %s tegra: strings) — Jetson Orin Nano media\n' "${tegra:-0}"
        ;;
    x86_64)
        printf 'RMBP\tx86_64 UEFI (BOOTX64.EFI + x86_64 kernel.elf) — 2012 rMBP / x86 media\n'
        ;;
    *)
        printf 'UNKNOWN\t%s, kernel.elf machine=%s, efi-stub=%s\n' \
            "$( [ -d "$MP/EFI" ] && echo 'has EFI/' || echo 'no EFI/' )" "${mach:-none}" "${efi:-none}"
        ;;
esac
exit 0
