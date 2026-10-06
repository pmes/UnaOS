#!/usr/bin/env bash
# SPDX-License-Identifier: LGPL-3.0-or-later
# EXFAT (rmbp-ledger B392): make the host KAT fixture for `exfat_core` — a 4 MiB exFAT image written by the
# HOST's own tools (`mkfs.exfat` from exfatprogs, files through the exfat-fuse writer), so the KATs read a
# volume this crate did not write. Nothing is fetched; the image is never committed.
#
#   bash unaos/libs/fs/exfat_core/tests/mkfixture.sh <out.img>
#   EXFAT_FIXTURE=<out.img> cargo test -p exfat_core
#
# Needs root (losetup + a FUSE mount). The contents are what tests/image.rs asserts.
set -euo pipefail
out="${1:?usage: mkfixture.sh <out.img>}"
command -v mkfs.exfat >/dev/null || { echo "mkfs.exfat missing (exfatprogs)"; exit 2; }
command -v mount.exfat-fuse >/dev/null || { echo "mount.exfat-fuse missing (exfat-fuse)"; exit 2; }
rm -f "$out"
truncate -s 4M "$out"
mkfs.exfat -q -c 4K -L UNAEXFAT "$out" >/dev/null
mnt="$(mktemp -d)"
loop="$(losetup -f --show "$out")"
cleanup() { umount "$mnt" 2>/dev/null || true; losetup -d "$loop" 2>/dev/null || true; rmdir "$mnt" 2>/dev/null || true; }
trap cleanup EXIT
mount.exfat-fuse "$loop" "$mnt" >/dev/null
python3 -I - "$mnt" <<'PY'
import os, sys
m = sys.argv[1]
def pat(n, a, b): return bytes(((i * a + b) & 0xff) for i in range(n))
open(os.path.join(m, "ReadMe.txt"), "wb").write(b"Hello, exFAT.\n")
open(os.path.join(m, "empty.bin"), "wb").close()
open(os.path.join(m, "pattern.bin"), "wb").write(pat(300000, 31, 7))
# Two files grown in alternation, each chunk synced, so their clusters interleave: FAT-chained, not NoFatChain.
fa = open(os.path.join(m, "frag_a.bin"), "wb"); fb = open(os.path.join(m, "frag_b.bin"), "wb")
A = pat(100000, 7, 1); B = pat(100000, 13, 3)
for k in range(0, 100000, 5000):
    fa.write(A[k:k+5000]); fa.flush(); os.fsync(fa.fileno())
    fb.write(B[k:k+5000]); fb.flush(); os.fsync(fb.fileno())
fa.close(); fb.close()
open(os.path.join(m, "Ünïcødé — 日本語 😀 a long name past fifteen units.txt"), "wb").write(b"unicode\n")
os.mkdir(os.path.join(m, "Sub"))
for i in range(150):
    open(os.path.join(m, "Sub", "f%03d.txt" % i), "wb").write(("file %d\n" % i).encode())
os.mkdir(os.path.join(m, "Sub", "Deeper"))
open(os.path.join(m, "Sub", "Deeper", "leaf.txt"), "wb").write(b"leaf\n")
PY
sync
umount "$mnt"
echo "fixture: $out ($(stat -c %s "$out") bytes)"
