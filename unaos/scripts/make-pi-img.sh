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
#
# SOURCE-ALONG: every image carries the exact source tree that built it, as two extra files in
# the FAT32 root next to kernel8.img:
#   SRC.TGZ — a DETERMINISTIC gzip'd tar of the repo source (no target/, no .git/, nothing
#             .gitignore'd), so a running system (and the ledger) can always answer "what code
#             is this"; same tree in => same bytes out.
#   SRC.SHA — one line: "<sha256>  SRC.TGZ", verifiable with `sha256sum -c` against SRC.TGZ.
# Both 8.3-clean, so the kernel's read-only FAT reader can name them. Default ON (measured
# +1.3 s / 5.24 MiB on the pi4 tree — far under the "slow enough to skip for tests" bar);
# UNAOS_NOSRC=1 skips the whole block. See docs/dev/OS/01_BOOT_HAL/arch_arm64.md §SOURCE-ALONG.
set -euo pipefail
SRC="${1:?src dir}"; OUT="${2:?out.img}"; SIZE_MB="${3:-256}"; UNAFS_IMG="${4:-}"

# Host-OS branch (2026-07-28, bench moved to Linux): Darwin keeps the hdiutil/diskutil path
# below byte-identical; Linux builds the SAME layout rootlessly — hand-written MBR entry,
# mkfs.fat --offset for the FAT32 volume, mcopy (mtools) for the boot files. The byte-offset
# tail (0x0C type patch + unafs partition 2) is shared by both.
OS="$(uname -s)"
fsize() { if [ "$OS" = Darwin ]; then stat -f %z "$1"; else stat -c %s "$1"; fi; }
fmtime() { if [ "$OS" = Darwin ]; then stat -f %m "$1" 2>/dev/null || echo 0; else stat -c %Y "$1" 2>/dev/null || echo 0; fi; }
sha256hex() { if command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1"; else sha256sum "$1"; fi | awk '{print $1}'; }
sha16() { sha256hex "$1" | cut -c1-16; }

# ---- BUILD-2 concurrency guards -------------------------------------------------------------
LOCK_WAIT_SECS="${MAKEPI_LOCK_WAIT_SECS:-1200}"   # a waiter blocks at most this long
LOCK_STALE_SECS="${MAKEPI_LOCK_STALE_SECS:-900}"  # a lock older than this (w/ dead PID or not) is breakable
LOCKDIR="${OUT}.buildlock"
LOCK_HELD=""   # set once we own LOCKDIR
DEV=""         # current hdiutil device; cleanup detaches it
MNT=""         # current private mountpoint; cleanup rmdir's it
SRCPKG=""      # SOURCE-ALONG scratch dir (SRC.TGZ/SRC.SHA); cleanup rm -rf's it

# Absolute OUT path, for matching hdiutil info's resolved image-path (dirname must exist; the
# .img itself may not yet). Falls back to OUT verbatim if the dir cannot be resolved.
if OUT_DIR_ABS=$(cd "$(dirname "$OUT")" 2>/dev/null && pwd); then
    OUT_ABS="$OUT_DIR_ABS/$(basename "$OUT")"
else
    OUT_ABS="$OUT"
fi

cleanup() {
    local rc=$?
    [ -n "$DEV" ] && [ "$OS" = Darwin ] && hdiutil detach "$DEV" >/dev/null 2>&1 || true
    [ -n "$MNT" ] && rmdir "$MNT" 2>/dev/null || true
    [ -n "$SRCPKG" ] && rm -rf "$SRCPKG" 2>/dev/null || true
    [ -n "$LOCK_HELD" ] && rm -rf "$LOCKDIR" 2>/dev/null || true
    return $rc
}
trap cleanup EXIT INT TERM

# Detach any lingering hdiutil attach of THIS image (crashed-run strand). Whole-disk devices only.
reclaim_stale_attach() {
    [ "$OS" = Darwin ] || return 0   # Linux never attaches the image as a device
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
        mtime=$(fmtime "$LOCKDIR")
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
    UNAFS_BYTES=$(fsize "$UNAFS_IMG")
    [ "$UNAFS_BYTES" -le $((UNAFS_MB * 1024 * 1024)) ] || {
        echo "unafs image ${UNAFS_BYTES}B exceeds the ${UNAFS_MB}MB reserved tail" >&2; exit 1; }
    FAT_SPEC="$((SIZE_MB - UNAFS_MB))M"
else
    FAT_SPEC="100%"
fi

# ---- SOURCE-ALONG: build SRC.TGZ + SRC.SHA -------------------------------------------------
# Determinism is the whole point (the ledger compares SRC.SHA across builds), so the tarball is a
# pure function of the source TREE — not of the clock, the uid, the readdir order, or which commit
# happens to be checked out:
#   * content set: taken from git, so target/, .git/ and everything .gitignore'd are excluded by
#     construction (no exclude list to drift).
#   * dirty worktrees: we do NOT stash and do NOT touch the real index. `git read-tree` + `git add -A`
#     against a THROWAWAY GIT_INDEX_FILE builds a tree object from the working tree (uncommitted edits
#     and untracked-but-not-ignored files included), which `git write-tree` names. On a clean worktree
#     that tree is bit-identical to HEAD^{tree}, so clean and dirty take the same code path.
#   * bytes: `git archive` of a TREE (never a commit — a commit-ish would inject a pax_global_header
#     carrying the commit sha, making the same tree hash differently from different branches). Entries
#     come out sorted, uid/gid 0, mode normalized. `--mtime` is REQUIRED and must be an approxidate git
#     actually parses: "@0" is NOT parsed and silently falls back to *now* (measured), so we pin
#     1980-01-01 (@315532800, the FAT epoch) and the tar is stable across runs.
#   * gzip -n: no name, no timestamp in the gzip header. The tar is written to a FILE first and then
#     compressed; `git archive | gzip` streaming was measured to emit different deflate block
#     boundaries run to run.
# Non-git source trees (e.g. an image rebuilt from an unpacked SRC.TGZ — the self-hosting rung) fall
# back to GNU tar's --sort/--mtime/--owner flags over an explicit sorted member list, which gives the
# same determinism (it cannot honor .gitignore — it prunes .git/ and target/ by name). If neither git
# nor a GNU tar is present we FAIL the build rather than ship an image whose SRC.SHA would be a lie;
# UNAOS_NOSRC=1 is how an operator acknowledges building a source-less image on purpose.
SRC_ROOT="${UNAOS_SRC_ROOT:-$(cd "$(dirname "$0")/../.." && pwd)}"
SRC_TGZ=""; SRC_SHA=""
if [ "${UNAOS_NOSRC:-0}" = 1 ]; then
    echo "make-pi-img: SOURCE-ALONG skipped (UNAOS_NOSRC=1) — image will NOT carry its source" >&2
else
    SRCPKG=$(mktemp -d "${TMPDIR:-/tmp}/unaos-srcalong.XXXXXX")
    SRC_TGZ="$SRCPKG/SRC.TGZ"; SRC_SHA="$SRCPKG/SRC.SHA"
    if git -C "$SRC_ROOT" rev-parse --git-dir >/dev/null 2>&1; then
        (
            cd "$SRC_ROOT"
            export GIT_INDEX_FILE="$SRCPKG/index"
            git read-tree HEAD
            git add -A .
            tree=$(git write-tree)
            git archive --format=tar --mtime=@315532800 "$tree" > "$SRCPKG/src.tar"
        )
    elif tar --sort=name --version >/dev/null 2>&1; then
        # No git => no .gitignore to consult; we can only prune .git/ and target/ by name. The member
        # list is built explicitly (find | sed | sort + --no-recursion) so entry names come out
        # repo-root-relative with NO "./" prefix, byte-matching the git-archive path's naming.
        (cd "$SRC_ROOT" && find . -mindepth 1 \
            \( -name .git -o -name target \) -prune -o -print) \
            | sed 's|^\./||' | LC_ALL=C sort > "$SRCPKG/list"
        tar --sort=name --mtime=@315532800 --owner=0 --group=0 --numeric-owner --no-recursion \
            -C "$SRC_ROOT" -cf "$SRCPKG/src.tar" -T "$SRCPKG/list"
        rm -f "$SRCPKG/list"
    else
        echo "make-pi-img: SOURCE-ALONG unavailable — $SRC_ROOT is not a git worktree and this host's" >&2
        echo "             tar lacks --sort/--mtime (no deterministic tarball possible). Set UNAOS_NOSRC=1" >&2
        echo "             to acknowledge, or build from a git checkout." >&2
        exit 1
    fi
    gzip -n -c "$SRCPKG/src.tar" > "$SRC_TGZ"
    rm -f "$SRCPKG/src.tar"
    printf '%s  SRC.TGZ\n' "$(sha256hex "$SRC_TGZ")" > "$SRC_SHA"
fi

# Size check BEFORE we format anything: refuse to build rather than let mcopy/ditto truncate the
# payload into a too-small volume. FAT32 overhead (2 FATs + reserved + cluster round-up) is a low
# single-digit MB at these sizes; SLACK_MB is the honest margin.
FAT_MB=$(( SIZE_MB ))
if [ -n "$UNAFS_IMG" ]; then FAT_MB=$(( SIZE_MB - UNAFS_MB )); fi
SLACK_MB=4
PAYLOAD_BYTES=0
for f in "$SRC"/*; do [ -e "$f" ] || continue; PAYLOAD_BYTES=$(( PAYLOAD_BYTES + $(du -sk "$f" | awk '{print $1}') * 1024 )); done
if [ -n "$SRC_TGZ" ]; then PAYLOAD_BYTES=$(( PAYLOAD_BYTES + $(fsize "$SRC_TGZ") + $(fsize "$SRC_SHA") )); fi
FAT_CAP_BYTES=$(( (FAT_MB - SLACK_MB) * 1024 * 1024 ))
if [ "$PAYLOAD_BYTES" -gt "$FAT_CAP_BYTES" ]; then
    echo "make-pi-img: FAT partition too small — payload $(( PAYLOAD_BYTES / 1024 / 1024 )) MB (incl." >&2
    echo "             $( [ -n "$SRC_TGZ" ] && echo "SRC.TGZ $(( $(fsize "$SRC_TGZ") / 1024 / 1024 )) MB" || echo 'no SRC.TGZ')) exceeds ${FAT_MB} MB FAT minus ${SLACK_MB} MB slack." >&2
    echo "             GROW the size_mb argument in arroyo's make-pi-img.sh call (currently ${SIZE_MB} MB)." >&2
    exit 1
fi

# Read SRC.TGZ back OUT of the finished volume and re-hash it against SRC.SHA. This is the whole
# point of the size check made honest: a FAT that silently dropped or truncated the payload fails
# here, at build time, instead of on a flashed card. $1 = the read-back copy.
srcalong_verify() {
    local got want
    got=$(sha256hex "$1")
    want=$(awk '{print $1}' < "$SRC_SHA")
    if [ "$got" != "$want" ]; then
        echo "make-pi-img: SOURCE-ALONG readback MISMATCH — SRC.TGZ in the image hashes $got," >&2
        echo "             SRC.SHA says $want. The FAT volume did not take the payload intact." >&2
        exit 1
    fi
}

le32() { # $1 = u32 value -> 4 LE bytes on stdout
    printf "$(printf '\\x%02x\\x%02x\\x%02x\\x%02x' \
        $(($1 & 255)) $((($1 >> 8) & 255)) $((($1 >> 16) & 255)) $((($1 >> 24) & 255)))"
}

if [ "$OS" = Darwin ]; then
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
    # SOURCE-ALONG: the tarball lives outside $SRC (make-pi-img never mutates its input staging dir),
    # so it is copied into the FAT root explicitly, after the staging tree.
    if [ -n "$SRC_TGZ" ]; then
        cp "$SRC_TGZ" "$SRC_SHA" "$MNT/"
        srcalong_verify "$MNT/SRC.TGZ"
    fi
    dot_clean -m "$MNT" 2>/dev/null || true
    hdiutil detach "$DEV" >/dev/null
    DEV=""
    rmdir "$MNT" 2>/dev/null || true
    MNT=""
else
    # Linux: no device attach at all — the image is only ever addressed as a file, so the
    # /Volumes-class hazard above cannot arise. Same layout as diskutil produces: partition 1
    # from the 1 MB boundary (LBA 2048) to FAT_SPEC, typed 0x0C below, label UNAOS-PI.
    dd if=/dev/zero of="$OUT" bs=1048576 count="$SIZE_MB" 2>/dev/null
    P1_START=2048
    if [ -n "$UNAFS_IMG" ]; then
        P1_COUNT=$(( (SIZE_MB - UNAFS_MB) * 2048 - P1_START ))
    else
        P1_COUNT=$(( SIZE_MB * 2048 - P1_START ))
    fi
    # MBR entry 1 at 446: status 0x00, CHS fillers 0xFF (LBA world), type 0x0C, start, count;
    # boot signature 0x55AA at 510.
    { printf '\x00\xff\xff\xff\x0c\xff\xff\xff'; le32 "$P1_START"; le32 "$P1_COUNT"; } \
        | dd of="$OUT" bs=1 seek=446 count=16 conv=notrunc 2>/dev/null
    printf '\x55\xaa' | dd of="$OUT" bs=1 seek=510 count=2 conv=notrunc 2>/dev/null
    # -F 32 forced: mkfs.fat would pick FAT16 at this size, and the Pi GPU ROM wants FAT32.
    mkfs.fat -F 32 -n UNAOS-PI -S 512 --offset "$P1_START" "$OUT" $(( P1_COUNT / 2 )) >/dev/null
    mcopy -s -i "$OUT@@$(( P1_START * 512 ))" "$SRC"/* ::/
    # SOURCE-ALONG (see the Darwin branch note): copied explicitly, not via $SRC.
    if [ -n "$SRC_TGZ" ]; then
        mcopy -o -i "$OUT@@$(( P1_START * 512 ))" "$SRC_TGZ" "$SRC_SHA" ::/
        mcopy -n -i "$OUT@@$(( P1_START * 512 ))" ::/SRC.TGZ "$SRCPKG/readback.tgz"
        srcalong_verify "$SRCPKG/readback.tgz"
        rm -f "$SRCPKG/readback.tgz"
    fi
fi
# The Pi GPU ROM wants the FAT32 partition typed 0x0C (LBA); diskutil makes 0x0B (CHS). Patch the MBR
# partition-1 type byte at offset 450. (The Linux branch already wrote 0x0C; idempotent there.)
printf '\x0c' | dd of="$OUT" bs=1 seek=450 count=1 conv=notrunc 2>/dev/null

# BeFS-K3: write the unafs partition — MBR entry 2 (type 0x7f, offset 462) pointing at the
# reserved tail, then the raw volume bytes at that LBA. Done entirely by byte offset into the
# detached image file, so no volume ever mounts and no /Volumes name is ever addressed.
if [ -n "$UNAFS_IMG" ]; then
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
if [ -n "$SRC_TGZ" ]; then
    echo "source-along: SRC.TGZ $(( $(fsize "$SRC_TGZ") / 1024 )) KiB in the FAT root, SRC.SHA $(cut -c1-16 < "$SRC_SHA")... (tree $SRC_ROOT)"
fi
echo "built $OUT  ($(du -h "$OUT" | cut -f1), sha256 $(sha16 "$OUT")...)"
