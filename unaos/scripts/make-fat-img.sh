#!/bin/bash
# make-fat-img.sh — build a FAT32/FAT16 test image for the UnaOS FAT reader/writer.
#
# Produces a raw disk image the QEMU usb-storage device can back (see the UNAOS_FATIMG
# knob in builder/src/main.rs and arroyo), populated from the packaged ESP + the x86 DATA
# volume so the kernel's `ls`/`cat`/`run`/`bg` see the same layout a real stick has.
#
#   ./scripts/make-fat-img.sh part [out.img]   MBR-partitioned FAT32 (MBR@LBA0 -> BPB)   [default]
#   ./scripts/make-fat-img.sh gpt  [out.img]   GPT-partitioned FAT32 (EFI PART@LBA1 -> BPB)
#   ./scripts/make-fat-img.sh p16  [out.img]   MBR-partitioned FAT16 (fixed root dir, 16-bit FAT)
#   ./scripts/make-fat-img.sh sf   [out.img]   superfloppy FAT32     (BPB@LBA0, no MBR)
#
# TWO HOSTS, ONE CONTENT TREE. The volume's contents are built ONCE into a host staging
# directory (`stage_contents`), then installed into the image by whichever backend this host
# has:
#   * macOS — hdiutil / diskutil / newfs_msdos (a virtual disk image the whole time; this
#     script never touches a physical disk).
#   * Linux — mkfs.vfat + mtools (mcopy) + sfdisk. FATIMG-CI: added 2026-07-29. Before it,
#     this helper was macOS-only, so `./arroyo fat-img` / `test-fat` — and therefore the whole
#     FAT-ATTACHED configuration, including the WINX-2 `bg /fat/STAT.ELF` end-to-end witness —
#     could not run on a Linux box and silently skipped in every automated suite run there.
#     A fixture that only runs when someone remembers to attach a stick is a fixture that will
#     be wrong again. Nothing here needs root: mtools writes the filesystem image as a plain
#     file and sfdisk stamps the partition table into another plain file. No loop device, no
#     mount, no physical disk ever addressed.
#
# Env: FAT_IMG_MB sets the image size (default 96 MiB — must stay >= ~34 MiB so the FS is
# FAT32, not FAT16).
set -euo pipefail

WORKSPACE_DIR="$(cd "$(dirname "$0")/.." && pwd)"
LAYOUT="${1:-part}"
ESP_DIR="${WORKSPACE_DIR}/target/x86_64_esp"
DATA_DIR="${WORKSPACE_DIR}/target/x86_64_data"
FSTYPE="MS-DOS FAT32"   # p16 overrides to FAT16 (macOS newfs wording)
FATBITS=32              # p16 overrides to 16 (mkfs.vfat -F)
DEFAULT_MB=96

case "$LAYOUT" in
    part) OUT="${2:-${WORKSPACE_DIR}/builder/fat.img}";     DESC="MBR FAT32"; SCHEME=MBR;;
    gpt)  OUT="${2:-${WORKSPACE_DIR}/builder/fat-gpt.img}"; DESC="GPT FAT32"; SCHEME=GPT;;
    p16)  OUT="${2:-${WORKSPACE_DIR}/builder/fat16.img}";   DESC="MBR FAT16"; SCHEME=MBR; FSTYPE="MS-DOS FAT16"; FATBITS=16; DEFAULT_MB=32;;
    sf)   OUT="${2:-${WORKSPACE_DIR}/builder/fat-sf.img}";  DESC="superfloppy FAT32"; SCHEME=NONE;;
    *) echo "usage: $0 [part|gpt|p16|sf] [out.img]" >&2; exit 1;;
esac
SIZE_MB="${FAT_IMG_MB:-$DEFAULT_MB}"

# ---------------------------------------------------------------------------------------------
# Stage the volume's CONTENTS into a plain host directory. Host-agnostic: every fixture below is
# built with mkdir/cp/printf, and the per-OS backends install this tree verbatim. Keeping it in
# one place is what stops the macOS and Linux images from drifting apart.
# ---------------------------------------------------------------------------------------------
stage_contents() {
    local S="$1"
    mkdir -p "$S"

    # Populate from the packaged ESP (real bootable layout) when present; otherwise leave a
    # minimal placeholder so `ls`/`cat` still have something to read.
    if [ -d "$ESP_DIR" ]; then
        echo "    staging from ${ESP_DIR}"
        COPYFILE_DISABLE=1 cp -R "${ESP_DIR}/EFI" "${S}/"
        [ -f "${ESP_DIR}/kernel.elf" ] && COPYFILE_DISABLE=1 cp "${ESP_DIR}/kernel.elf" "${S}/kernel.elf"
    else
        echo "    WARNING: ${ESP_DIR} absent — run './arroyo esp-x86' first for the real payload."
        mkdir -p "${S}/EFI/BOOT"
        printf 'placeholder BOOTX64.EFI\n' > "${S}/EFI/BOOT/BOOTX64.EFI"
        printf 'placeholder kernel.elf\n'   > "${S}/kernel.elf"
    fi
    # Make this image NON-bootable: rename the UEFI removable-media fallback (\EFI\BOOT\BOOTX64.EFI) so
    # OVMF boots the separate fat:rw ESP drive (which always carries the freshly built kernel under
    # test) instead of this usb-storage image. That leaves the usb-storage as a pristine data disk the
    # kernel enumerates cleanly -- avoiding the OVMF-USB-boot-then-kernel-re-enumerate flakiness -- and
    # means the image only needs rebuilding when its *file contents* change, not on every kernel edit.
    # (`ls` lists the root only, so EFI/ + kernel.elf still show; a real metal stick keeps BOOTX64.EFI.)
    if [ -f "${S}/EFI/BOOT/BOOTX64.EFI" ]; then
        mv "${S}/EFI/BOOT/BOOTX64.EFI" "${S}/EFI/BOOT/BOOTX64.REM"
    fi
    # SELFHOST-2: the SOURCE-ALONG pair, staged ONLY when the caller asks for it (UNAOS_SRCFIXTURE=1 —
    # `./arroyo test-selfhost`). Every other lane through here sets UNAOS_NOSRC=1 and deliberately keeps
    # the pair off, because these images are size-budgeted and their root directory is enumerated by
    # tests; two extra root entries would move counts that other fixtures assert. Opt-in keeps those
    # fixtures byte-stable while giving the source-verify gate a REAL payload to read.
    if [ -n "${UNAOS_SRCFIXTURE:-}" ]; then
        if [ -f "${ESP_DIR}/SRC.TGZ" ] && [ -f "${ESP_DIR}/SRC.SHA" ]; then
            COPYFILE_DISABLE=1 cp "${ESP_DIR}/SRC.TGZ" "${S}/SRC.TGZ"
            COPYFILE_DISABLE=1 cp "${ESP_DIR}/SRC.SHA" "${S}/SRC.SHA"
            echo "    added SRC.TGZ ($(wc -c < "${ESP_DIR}/SRC.TGZ" | tr -d ' ') bytes) + SRC.SHA for the SELFHOST-2 verify"
        else
            echo "    ERROR: UNAOS_SRCFIXTURE=1 but ${ESP_DIR}/SRC.{TGZ,SHA} are absent —" >&2
            echo "           the source-verify gate would boot against a payload-less image and read as 'not packed'." >&2
            exit 1
        fi
    fi

    # A small text file for the `cat` milestone.
    printf 'hello from the UnaOS FAT reader\nthis file lives on a real FAT32 volume\n' > "${S}/hello.txt"
    printf 'UnaOS read-only FAT32/16 reader test volume (%s layout).\n' "$LAYOUT" > "${S}/readme.txt"

    # U2: the x86 ring-3 "hello from disk" program (crates/user-blob-x86 → target/hello.bin, built by
    # arroyo's build_user_hello_x86). Copy it onto the image as HELLO.BIN so the kernel's U2 FAT loader
    # finds + runs it in ring 3. Read straight from target/hello.bin (fresh — every x86 build path builds
    # it before make-fat-img runs), independent of whether the ESP payload carried it.
    local HELLO_BIN="${WORKSPACE_DIR}/target/hello.bin"
    if [ -f "$HELLO_BIN" ]; then
        COPYFILE_DISABLE=1 cp "$HELLO_BIN" "${S}/HELLO.BIN"
        echo "    added HELLO.BIN ($(wc -c < "$HELLO_BIN" | tr -d ' ') bytes) for the U2 loader"
    else
        echo "    WARNING: ${HELLO_BIN} absent — image has no HELLO.BIN (run './arroyo fat-img' via arroyo, not make-fat-img.sh directly)"
    fi

    # WINX-5: the x86 EL0 persistence program (crates/user-stat → target/STAT-X86.ELF, built by arroyo's
    # build_user_stat_x86). Copy it onto the image as STAT.ELF so the x86 shell's `run`/`bg` — which read the
    # FAT boot partition's root — can load it (`bg /fat/STAT.ELF`). Same shape and same freshness argument as
    # the HELLO.BIN hunk above. Un-suffixed on the volume so the operator command reads the same on both
    # arches; the -X86 suffix exists only in target/, where both arches' images share one directory.
    local STAT_ELF="${WORKSPACE_DIR}/target/STAT-X86.ELF"
    if [ -f "$STAT_ELF" ]; then
        COPYFILE_DISABLE=1 cp "$STAT_ELF" "${S}/STAT.ELF"
        echo "    added STAT.ELF ($(wc -c < "$STAT_ELF" | tr -d ' ') bytes) for the run/bg loader"
    else
        echo "    WARNING: ${STAT_ELF} absent — image has no STAT.ELF (run './arroyo fat-img' via arroyo)"
    fi

    # WINX-7: the x86 EL0 mini-vug (crates/user-vug → target/VUG-X86.ELF, built by arroyo's
    # build_user_vug_x86). Same shape, same freshness argument and same un-suffixed on-volume name as the
    # STAT.ELF hunk above — `bg /fat/VUG.ELF` reads identically on both arches.
    local VUG_ELF="${WORKSPACE_DIR}/target/VUG-X86.ELF"
    if [ -f "$VUG_ELF" ]; then
        COPYFILE_DISABLE=1 cp "$VUG_ELF" "${S}/VUG.ELF"
        echo "    added VUG.ELF ($(wc -c < "$VUG_ELF" | tr -d ' ') bytes) for the run/bg loader"
    else
        echo "    WARNING: ${VUG_ELF} absent — image has no VUG.ELF (run './arroyo fat-img' via arroyo)"
    fi

    # PULSE-1: the x86 EL0 cpu-pulse monitor (crates/user-pulse → target/PULSE-X86.ELF, built by arroyo's
    # build_user_pulse_x86). Same shape, same freshness argument and same un-suffixed on-volume name as the
    # STAT.ELF / VUG.ELF hunks above. This is the file the PULSE-W end-to-end witness looks for by name.
    local PULSE_ELF="${WORKSPACE_DIR}/target/PULSE-X86.ELF"
    if [ -f "$PULSE_ELF" ]; then
        COPYFILE_DISABLE=1 cp "$PULSE_ELF" "${S}/PULSE.ELF"
        echo "    added PULSE.ELF ($(wc -c < "$PULSE_ELF" | tr -d ' ') bytes) for the run/bg loader"
    else
        echo "    WARNING: ${PULSE_ELF} absent — image has no PULSE.ELF (run './arroyo fat-img' via arroyo)"
    fi

    # U9x M2: plant a DEDICATED writable scratch file (NEVER HELLO.BIN — other fixtures load that as EL0
    # code). 1 KiB of 0xEE filler, mirroring the pi4 image plant (arroyo's kernel8 SCRATCH.BIN block): the
    # U9x fixture opens it RW, seeks to 520, overwrites a 16-byte pattern, and the launcher flushes that
    # staged write to disk and raw-re-reads the sector to prove it landed (bytes changed + size unchanged).
    # 0356 octal == 0xEE; matches the kernel's in-memory const seed (U9X_SCRATCH_FILL) byte-for-byte, so the
    # no-FAT in-memory core and the on-disk backing serve identical pre-image bytes.
    head -c 1024 /dev/zero | tr '\000' '\356' > "${S}/SCRATCH.BIN"
    echo "    added SCRATCH.BIN (1024 bytes of 0xEE) for the U9x File-write demo"

    # U10 GROW: plant a DEDICATED 1-cluster file the growth fixture extends across the cluster boundary.
    # 512 bytes of 0xC1 (0301 octal) == exactly one 512-B cluster on the FAT32 layouts (part/gpt/sf); the
    # fixture seeks to 512 (EOF) and appends a 16-byte pattern, so the file grows to 528 and (on 512-B
    # clusters) allocates + zero-fills + chains a SECOND cluster. 0xC1 matches the kernel's staged seed
    # (U10_GROW_SEED) byte-for-byte, so a read-back before any write sees the original filler. (The launcher
    # self-heals a prior boot's grown copy on a persistent metal card, so re-runs stay honest.)
    head -c 512 /dev/zero | tr '\000' '\301' > "${S}/GROW.BIN"
    echo "    added GROW.BIN (512 bytes of 0xC1) for the U10 file-growth demo"

    # STOR-1 S8: plant a DEDICATED writable-dynamic scratch file (NEVER a staged name — HELLO/SCRATCH/GROW.BIN —
    # nor README.TXT, whose prefix the S7 witness checks). 64 bytes of 0xA5 (0245 octal): the S8 write witness
    # opens it RW off the pre-stage set, overwrites a 16-byte pattern at offset 8 live, reads it back, then
    # RESTORES the 0xA5 seed — so the image is left pristine and the witness is idempotent across boots/power-cuts.
    # Overwrite-only: the file never grows, so its 64-byte size is immutable for the boot.
    head -c 64 /dev/zero | tr '\000' '\245' > "${S}/S8W.BIN"
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
    cat > "${S}/BLOCK.TXT" <<'EOF'
# zeolite DNS sinkhole blocklist (hosts format)
0.0.0.0 ads.example
0.0.0.0 track.example   # inline comment tolerated

; semicolon comments and blank lines are skipped
127.0.0.1 telemetry.example
EOF
    echo "    added BLOCK.TXT (zeolite hosts-format blocklist: ads.example, track.example, telemetry.example)"

    # PI-FS-3: plant VFAT LONG FILENAMES + a NESTED subdirectory tree so the read-only reader's LFN parse
    # and arbitrary-depth traversal are exercised in QEMU (UNAOS_FATIMG=1 ./arroyo test-arm). Both backends'
    # copy tools generate real 0x0F LFN component slots for any name that is not a valid 8.3 short name; a
    # mixed-case and a spaces-and-mixed-case name each force an LFN run (the short entry mangles to e.g.
    # LONGFI~1.TXT).
    printf 'this file has a VFAT long filename\n' > "${S}/Long Filename Example.txt"
    printf 'mixed-case long name (LFN, distinct short entry)\n' > "${S}/MixedCaseName.md"
    echo "    added LFN files (Long Filename Example.txt, MixedCaseName.md)"
    # Nested tree: root/subdir/nested with a file at the deepest level — proves traversal past one level.
    mkdir -p "${S}/subdir/Nested Directory"
    printf 'a file two levels deep\n' > "${S}/subdir/Nested Directory/deep file.txt"
    printf 'level one file\n' > "${S}/subdir/level1.txt"
    echo "    added nested tree (subdir/ -> 'Nested Directory'/ -> 'deep file.txt')"

    # WINX-7 PKG: the x86 DATA volume tree (target/x86_64_data) is what the KERNEL reads on metal —
    # HELLO.BIN / STAT.ELF / VUG.ELF / PULSE.ELF / hello.txt. Copied LAST so a one-device QEMU image carries exactly
    # the bytes a staged stick would, including hello.txt's "DATA volume" probe line.
    if [ -d "$DATA_DIR" ]; then
        COPYFILE_DISABLE=1 cp -R "${DATA_DIR}/." "${S}/"
        echo "    staged the x86 DATA volume tree (${DATA_DIR})"
    fi
}

# ---------------------------------------------------------------------------------------------
# macOS backend — hdiutil / diskutil / newfs_msdos, as this script has always worked. Only the
# population step changed: it installs the staged tree instead of building the content inline.
# ---------------------------------------------------------------------------------------------
DISK=""
cleanup_macos() {
    # Best-effort: unmount + detach the virtual disk if we still hold it.
    if [ -n "$DISK" ]; then
        diskutil unmountDisk force "$DISK" >/dev/null 2>&1 || true
        hdiutil detach "$DISK" >/dev/null 2>&1 || true
    fi
}

build_macos() {
    local STAGE="$1"
    trap 'cleanup_macos; rm -rf "$STAGE_DIR"' EXIT

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

    local VOLDEV
    if [ "$LAYOUT" = "sf" ]; then
        # Superfloppy: format the whole raw device as FAT32 (BPB at LBA0, no partition table).
        local RDISK="${DISK/disk/rdisk}"
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
    local MNT
    MNT="$(diskutil info "$VOLDEV" | awk -F': +' '/Mount Point/ {print $2}')"
    if [ -z "$MNT" ] || [ ! -d "$MNT" ]; then
        echo "make-fat-img.sh: could not find mount point for ${VOLDEV}" >&2; exit 1
    fi
    echo "    mounted at ${MNT}"

    COPYFILE_DISABLE=1 cp -R "${STAGE}/." "${MNT}/"
    echo "    populated the volume from ${STAGE}"

    # Strip macOS metadata (AppleDouble ._ files, Spotlight/fseventsd) so `ls` shows a clean tree.
    sync
    find "$MNT" -name '._*' -delete 2>/dev/null || true
    rm -rf "${MNT}/.fseventsd" "${MNT}/.Spotlight-V100" "${MNT}/.Trashes" "${MNT}/.TemporaryItems" 2>/dev/null || true
    sync

    diskutil unmountDisk "$DISK" >/dev/null
    hdiutil detach "$DISK" >/dev/null
    DISK=""
}

# ---------------------------------------------------------------------------------------------
# Linux backend (FATIMG-CI) — mkfs.vfat + mtools + sfdisk. No root, no loop device, no mount.
#
# The filesystem is built as a STANDALONE image sized to the partition, populated with `mcopy -s`
# (which emits genuine VFAT LFN slots for the long names above, as newfs_msdos + Finder do on the
# macOS path), then `dd`'d into the disk image at the partition's start LBA; sfdisk stamps the
# partition table into the disk image file.
#
# Geometry parity with the macOS images the kernel fixtures are calibrated against:
#   * 1 sector per cluster (`-s 1`) → 512-byte clusters, which is what the U10 GROW.BIN fixture's
#     "512 bytes == exactly one cluster" assumption requires (macOS: `newfs_msdos -c 1`);
#   * partition 1 starts at LBA 2048 (the 1 MiB alignment every modern formatter uses);
#   * volume label UNAOS, matching `newfs_msdos -v UNAOS` / `diskutil ... UNAOS`.
# ---------------------------------------------------------------------------------------------
build_linux() {
    local STAGE="$1"
    local t
    for t in mkfs.vfat mcopy; do
        command -v "$t" >/dev/null 2>&1 || {
            echo "make-fat-img.sh: '$t' not found — install dosfstools + mtools" >&2; exit 1; }
    done
    if [ "$LAYOUT" != "sf" ]; then
        command -v sfdisk >/dev/null 2>&1 || {
            echo "make-fat-img.sh: 'sfdisk' not found — install util-linux" >&2; exit 1; }
    fi

    local TOTAL_SECTORS=$(( SIZE_MB * 2048 ))
    local PART_LBA=2048
    local FS_SECTORS
    if [ "$LAYOUT" = "sf" ]; then
        PART_LBA=0
        FS_SECTORS=$TOTAL_SECTORS
    elif [ "$LAYOUT" = "gpt" ]; then
        # GPT keeps a BACKUP header + entry array at the very end of the disk (1 + 32 sectors); the
        # partition must stop short of it or sfdisk refuses the layout.
        FS_SECTORS=$(( TOTAL_SECTORS - PART_LBA - 33 ))
    else
        FS_SECTORS=$(( TOTAL_SECTORS - PART_LBA ))
    fi

    local FSIMG="${OUT}.fs.tmp"
    rm -f "$OUT" "$FSIMG"
    truncate -s $(( FS_SECTORS * 512 )) "$FSIMG"
    mkfs.vfat -F "$FATBITS" -s 1 -n UNAOS "$FSIMG" >/dev/null
    echo "    formatted FAT${FATBITS} (${FS_SECTORS} sectors, 512-byte clusters)"

    # Populate. `-s` recurses, `-b` streams, `-o` overwrites without prompting, `-Q` aborts the whole
    # run on the first failure so a truncated volume can never masquerade as a good one. Each staged
    # top-level entry is copied individually so `::/` keeps the tree's own root names.
    local e
    for e in "$STAGE"/* "$STAGE"/.[!.]*; do
        [ -e "$e" ] || continue
        mcopy -s -b -o -Q -i "$FSIMG" "$e" "::/"
    done
    echo "    populated the volume from ${STAGE}"

    truncate -s $(( TOTAL_SECTORS * 512 )) "$OUT"
    if [ "$LAYOUT" = "sf" ]; then
        dd if="$FSIMG" of="$OUT" bs=1M conv=notrunc status=none
    else
        # Partition type: MBR 0x0c (FAT32 LBA) / 0x06 (FAT16), GPT "uefi" (EFI System).
        local PTYPE LABEL
        case "$LAYOUT" in
            gpt) PTYPE="uefi"; LABEL="gpt";;
            p16) PTYPE="6";    LABEL="dos";;
            *)   PTYPE="c";    LABEL="dos";;
        esac
        printf 'label: %s\nunit: sectors\n\nstart=%d, size=%d, type=%s\n' \
            "$LABEL" "$PART_LBA" "$FS_SECTORS" "$PTYPE" | sfdisk -q "$OUT" >/dev/null
        dd if="$FSIMG" of="$OUT" bs=512 seek="$PART_LBA" conv=notrunc status=none
        echo "    stamped a ${LABEL} partition table (partition 1 @ LBA ${PART_LBA}, type ${PTYPE})"
    fi
    rm -f "$FSIMG"
}

echo "==> Building ${DESC} image: ${OUT} (${SIZE_MB} MiB)"
STAGE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/unaos-fatimg.XXXXXX")"
trap 'rm -rf "$STAGE_DIR"' EXIT
stage_contents "$STAGE_DIR"

case "$(uname)" in
    Darwin) build_macos "$STAGE_DIR";;
    Linux)  build_linux "$STAGE_DIR";;
    *) echo "make-fat-img.sh: unsupported host '$(uname)' — macOS and Linux backends only." >&2; exit 1;;
esac

echo "==> Done: ${OUT}"
echo "    Test in QEMU:  UNAOS_FATIMG=1 ./arroyo test 25   (partitioned)"
echo "                   UNAOS_FATIMG=sf ./arroyo test 25  (superfloppy)"
