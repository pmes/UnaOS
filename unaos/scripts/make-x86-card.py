#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 The Architect & Una
#
# UNAFSX86 M3 (rmbp-ledger B298, ROADMAP SH-2 rung 1) — the x86 card image with a UnaFS system volume.
#
#   p1  EFI System Partition  FAT32, holds target/x86_64_esp/ + target/x86_64_data/ (the trees
#                             `arroyo esp-x86` stages). The firmware and the bootloader find the ESP as
#                             partition 1 exactly as on the plain FAT card; the kernel mounts it at /boot
#                             and /apps.
#   p2  UnaFS system volume   the bytes `tools/unafs init` produced (the same KAT-pinned format the kernel
#                             mounts). Found by the kernel's `locate_unafs` by SUPERBLOCK MAGIC, never by
#                             the type GUID; the GUID below only stops other OSes guessing at it. The
#                             kernel binds it as / (bootdisk::bind_root) when built with UNAOS_UNAFS=1.
#
# The GPT is written by hand (sfdisk is absent on some benches — the make-gpt-fixture.py precedent) in
# the same layout `install/gpt.rs` writes: protective MBR, primary header at LBA 1, 128 x 128-byte array
# at LBA 2..33, the backup array + header at the tail, CRC-32 over the header and the array.
#
# Usage: make-x86-card.py --esp DIR --data DIR --unafs IMG -o OUT [--fat-mb N]
import argparse
import binascii
import os
import shutil
import struct
import subprocess
import sys
import tempfile

SECTOR = 512
ENTRIES = 128
ENTRY_SIZE = 128
ARRAY_SECTORS = ENTRIES * ENTRY_SIZE // SECTOR  # 32
ALIGN = 2048  # 1 MiB

EFI_SYSTEM = bytes([0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11,
                    0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B])  # C12A7328-F81F-11D2-BA4B-00A0C93EC93B
# UnaFS system volume: "UNAFS\0\0\0" + a fixed tail, mixed-endian as on disk. Advisory only (see header).
UNAFS_TYPE = bytes([0x55, 0x4E, 0x41, 0x46, 0x53, 0x00, 0x00, 0x40,
                    0x80, 0x55, 0x4E, 0x41, 0x4F, 0x53, 0x00, 0x02])


def crc32(b):
    return binascii.crc32(b) & 0xFFFFFFFF


def guid(seed):
    g = bytearray(16)
    for i in range(16):
        g[i] = seed[i] if i < len(seed) else ((0x11 * (i + 1)) & 0xFF) ^ 0x5A
    g[7] = (g[7] & 0x0F) | 0x40
    g[8] = (g[8] & 0x3F) | 0x80
    return bytes(g)


def protective_mbr(total):
    m = bytearray(SECTOR)
    e = 446
    m[e + 2] = 0x02
    m[e + 4] = 0xEE
    m[e + 5] = m[e + 6] = m[e + 7] = 0xFF
    struct.pack_into("<I", m, e + 8, 1)
    struct.pack_into("<I", m, e + 12, min(total - 1, 0xFFFFFFFF))
    m[510], m[511] = 0x55, 0xAA
    return bytes(m)


def header(cur, backup, first_usable, last_usable, disk_guid, entries_lba, entries_crc):
    h = bytearray(SECTOR)
    h[0:8] = b"EFI PART"
    struct.pack_into("<I", h, 8, 0x00010000)
    struct.pack_into("<I", h, 12, 92)
    struct.pack_into("<Q", h, 24, cur)
    struct.pack_into("<Q", h, 32, backup)
    struct.pack_into("<Q", h, 40, first_usable)
    struct.pack_into("<Q", h, 48, last_usable)
    h[56:72] = disk_guid
    struct.pack_into("<Q", h, 72, entries_lba)
    struct.pack_into("<I", h, 80, ENTRIES)
    struct.pack_into("<I", h, 84, ENTRY_SIZE)
    struct.pack_into("<I", h, 88, entries_crc)
    struct.pack_into("<I", h, 16, crc32(bytes(h[0:92])))
    return bytes(h)


def entry(type_guid, first, last, name, seed):
    e = bytearray(ENTRY_SIZE)
    e[0:16] = type_guid
    e[16:32] = guid(seed)
    struct.pack_into("<Q", e, 32, first)
    struct.pack_into("<Q", e, 40, last)
    n = name.encode("utf-16-le")[:70]
    e[56:56 + len(n)] = n
    return bytes(e)


def tree_bytes(d):
    total = 0
    for root, _, files in os.walk(d):
        for f in files:
            total += os.path.getsize(os.path.join(root, f))
    return total


def build_fat(path, sectors, trees):
    for t in ("mkfs.vfat", "mcopy"):
        if shutil.which(t) is None:
            sys.exit("make-x86-card.py: '%s' not found — install dosfstools + mtools" % t)
    with open(path, "wb") as f:
        f.truncate(sectors * SECTOR)
    subprocess.run(["mkfs.vfat", "-F", "32", "-n", "UNAOS", path], check=True, stdout=subprocess.DEVNULL)
    for d in trees:
        for e in sorted(os.listdir(d)):
            subprocess.run(["mcopy", "-s", "-b", "-o", "-Q", "-i", path, os.path.join(d, e), "::/"], check=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--esp", required=True)
    ap.add_argument("--data", required=True)
    ap.add_argument("--unafs", required=True)
    ap.add_argument("-o", "--out", required=True)
    ap.add_argument("--fat-mb", type=int, default=0, help="0 = sized from the trees (+64 MiB, floor 128)")
    a = ap.parse_args()

    trees = [d for d in (a.esp, a.data) if os.path.isdir(d)]
    fat_mb = a.fat_mb or max(128, (sum(tree_bytes(d) for d in trees) >> 20) + 64)
    fat_sectors = fat_mb * 2048
    unafs_bytes = os.path.getsize(a.unafs)
    if unafs_bytes % 4096:
        sys.exit("make-x86-card.py: %s is not a whole number of 4 KiB blocks" % a.unafs)
    unafs_sectors = unafs_bytes // SECTOR

    p1_first = ALIGN
    p1_last = p1_first + fat_sectors - 1
    p2_first = (p1_last + 1 + ALIGN - 1) // ALIGN * ALIGN
    p2_last = p2_first + unafs_sectors - 1
    total = p2_last + 1 + ARRAY_SECTORS + 1
    total = (total + ALIGN - 1) // ALIGN * ALIGN
    last_usable = total - ARRAY_SECTORS - 2

    with tempfile.TemporaryDirectory(dir=os.path.dirname(os.path.abspath(a.out))) as tmp:
        fat = os.path.join(tmp, "esp.fat")
        build_fat(fat, fat_sectors, trees)
        arr = entry(EFI_SYSTEM, p1_first, p1_last, "UNAOS-ESP", b"UNAOS-X86-ESP") + \
            entry(UNAFS_TYPE, p2_first, p2_last, "UNAOS-UNAFS", b"UNAOS-X86-UFS")
        arr = arr + bytes(ARRAY_SECTORS * SECTOR - len(arr))
        acrc = crc32(arr)
        dg = guid(b"UNAOS-X86-CARD")
        with open(a.out, "wb") as o:
            o.truncate(total * SECTOR)
            o.seek(0)
            o.write(protective_mbr(total))
            o.write(header(1, total - 1, 34, last_usable, dg, 2, acrc))
            o.write(arr)
            o.seek((total - 1 - ARRAY_SECTORS) * SECTOR)
            o.write(arr)
            o.write(header(total - 1, 1, 34, last_usable, dg, total - 1 - ARRAY_SECTORS, acrc))
            for src, first in ((fat, p1_first), (a.unafs, p2_first)):
                o.seek(first * SECTOR)
                with open(src, "rb") as i:
                    shutil.copyfileobj(i, o, 1 << 20)
    print(":: X86-CARD: p1=esp lba=%d sectors=%d p2=unafs lba=%d sectors=%d total_mb=%d ::"
          % (p1_first, fat_sectors, p2_first, unafs_sectors, total // 2048))


if __name__ == "__main__":
    main()
