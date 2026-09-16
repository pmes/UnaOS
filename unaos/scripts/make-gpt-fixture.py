#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 The Architect & Una
#
# PARTINSTALL — build the QEMU fixture disk the partition installer is proven against.
#
# WHY A SCRIPT AND NOT sfdisk. `sfdisk` is ABSENT on this box (measured by AHCIBOOT), so the GPT is
# written here by hand. The layout mirrors `unaos/crates/kernel/src/install/gpt.rs`'s writer exactly
# — protective MBR at LBA 0, primary header at LBA 1, a 128 x 128-byte entry array at LBA 2..33, the
# backup array + header at the tail, CRC-32/ISO-HDLC on the header (92 bytes, CRC field zeroed) and
# on the whole entry array — so the kernel's READER is being fed the same bytes its own WRITER makes.
#
# WHAT THE FIXTURE IS FOR. The installer must lay UnaOS into ONE partition of an existing GPT and
# leave every neighbour byte-identical (RULINGS R25: a non-UnaOS disk is a STRANGER). So the disk
# carries, deliberately, one partition of each kind the refusal ladder must answer:
#
#   part 0  FOREIGN-FAT    Microsoft Basic Data, a real FAT32 BPB + a marker string
#                          -> content=FAT      -> refused `partition-not-empty content=FAT`
#   part 1  FOREIGN-APFS   Apple APFS type GUID, `NXSB` at +32 of its first sector
#                          -> content=APFS     -> refused `partition-not-empty content=APFS`
#   part 2  UNAOS-TARGET   Microsoft Basic Data, all zero, 48 MiB (above the FAT32 floor)
#                          -> content=empty    -> THE ONE PARTITION THE FIXTURE INSTALLS INTO
#   part 3  ESP-SLOT       EFI System Partition type GUID, all zero
#                          -> content=ESP      -> refused `partition-is-esp`
#   part 4  TINY           Microsoft Basic Data, all zero, 1 MiB (below the FAT32 floor)
#                          -> content=empty    -> refused `partition-too-small`
#
# and the disk AS A WHOLE carries foreign volumes, so a whole-disk target is refused
# `disk-has-foreign-volumes`. Every refusal in the table is a partition on this image; none of them
# is a synthetic in-kernel construction.
#
# The APFS partition is a SIGNATURE, not a filesystem: `NXSB` at offset 32 of the container
# superblock (Apple File System Reference, `nx_superblock_t.nx_magic`) is exactly what the kernel's
# read-only content probe looks at, and writing more would be pretending to model a filesystem we do
# not read. Same for the HFS+ probe, which this fixture does not exercise by default (`--hfs`
# rewrites part 0 as an HFS+ volume header instead, for a one-off).
#
# Usage:  python3 unaos/scripts/make-gpt-fixture.py [-o builder/part-fixture.img] [--hfs]
# Output is deterministic: the same bytes every run, so a neighbours-untouched sha is stable across
# rebuilds of the fixture itself.

import argparse
import binascii
import os
import struct
import sys

SECTOR = 512
DISK_SECTORS = 262144  # 128 MiB
GPT_ENTRIES = 128
GPT_ENTRY_SIZE = 128
GPT_ARRAY_SECTORS = (GPT_ENTRIES * GPT_ENTRY_SIZE) // SECTOR  # 32

# Type GUIDs in GPT mixed-endian on-disk layout (first three fields little-endian).
EFI_SYSTEM = bytes([0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11,
                    0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B])  # C12A7328-F81F-11D2-...
BASIC_DATA = bytes([0xA2, 0xA0, 0xD0, 0xEB, 0xE5, 0xB9, 0x33, 0x44,
                    0x87, 0xC0, 0x68, 0xB6, 0xB7, 0x26, 0x99, 0xC7])  # EBD0A0A2-B9E5-4433-...
APPLE_APFS = bytes([0xEF, 0x57, 0x34, 0x7C, 0x00, 0x00, 0xAA, 0x11,
                    0xAA, 0x11, 0x00, 0x30, 0x65, 0x43, 0xEC, 0xAC])  # 7C3457EF-0000-11AA-...

# The five partitions: (name, type GUID, first LBA, sector count, content kind).
# 1 MiB (2048-sector) aligned starts, the alignment gpt.rs's writer uses.
#
# WHY THE APFS PARTITION IS THE SAME SIZE AS THE TARGET, and it is not decoration. The go-red for
# the content probe is "blind the APFS arm and watch a stranger's volume get written". With an 8 MiB
# APFS partition that mutation does NOT fire: the partition goes eligible, and then the SIZE guard
# refuses it below the FAT32 cluster floor — so the run stays green and the go-red proves the size
# guard rather than the probe. A go-red that is caught by a DIFFERENT guard has not tested the guard
# it names (LAWS §5). Sized equal to the target, the blinded probe selects part 1, writes it, and the
# neighbours sha goes 3/4 -> FAIL, which is the failure the probe is there to prevent.
PARTS = [
    ("FOREIGN-FAT",  BASIC_DATA,   2048,  16384, "fat"),    #  8 MiB
    ("FOREIGN-APFS", APPLE_APFS,  18432,  98304, "apfs"),   # 48 MiB — see the note above
    ("UNAOS-TARGET", BASIC_DATA, 116736,  98304, "empty"),  # 48 MiB
    ("ESP-SLOT",     EFI_SYSTEM, 215040,   8192, "empty"),  #  4 MiB
    ("TINY",         BASIC_DATA, 223232,   2048, "empty"),  #  1 MiB
]


def crc32(b):
    return binascii.crc32(b) & 0xFFFFFFFF


def derive_guid(label):
    """gpt.rs::derive_guid, byte for byte — so a GUID this script writes and one the kernel writes
    for the same seed are the same 16 bytes."""
    g = bytearray(16)
    for i in range(16):
        g[i] = label[i] if i < len(label) else ((0x11 * (i + 1)) & 0xFF) ^ 0x5A
    g[7] = (g[7] & 0x0F) | 0x40  # version 4
    g[8] = (g[8] & 0x3F) | 0x80  # variant RFC 4122
    return bytes(g)


def protective_mbr(total_sectors):
    mbr = bytearray(SECTOR)
    e = 446
    mbr[e + 2] = 0x02
    mbr[e + 4] = 0xEE
    mbr[e + 5] = 0xFF
    mbr[e + 6] = 0xFF
    mbr[e + 7] = 0xFF
    struct.pack_into("<I", mbr, e + 8, 1)
    struct.pack_into("<I", mbr, e + 12, min(total_sectors - 1, 0xFFFFFFFF))
    mbr[510] = 0x55
    mbr[511] = 0xAA
    return bytes(mbr)


def header(current_lba, backup_lba, first_usable, last_usable, disk_guid,
           entries_lba, entries_crc):
    h = bytearray(SECTOR)
    h[0:8] = b"EFI PART"
    struct.pack_into("<I", h, 8, 0x00010000)   # revision 1.0
    struct.pack_into("<I", h, 12, 92)          # header size
    struct.pack_into("<Q", h, 24, current_lba)
    struct.pack_into("<Q", h, 32, backup_lba)
    struct.pack_into("<Q", h, 40, first_usable)
    struct.pack_into("<Q", h, 48, last_usable)
    h[56:72] = disk_guid
    struct.pack_into("<Q", h, 72, entries_lba)
    struct.pack_into("<I", h, 80, GPT_ENTRIES)
    struct.pack_into("<I", h, 84, GPT_ENTRY_SIZE)
    struct.pack_into("<I", h, 88, entries_crc)
    struct.pack_into("<I", h, 16, crc32(bytes(h[0:92])))
    return bytes(h)


def entry(type_guid, first, last, name, seed):
    e = bytearray(GPT_ENTRY_SIZE)
    e[0:16] = type_guid
    e[16:32] = derive_guid(seed)
    struct.pack_into("<Q", e, 32, first)
    struct.pack_into("<Q", e, 40, last)
    utf16 = name.encode("utf-16-le")[:70]
    e[56:56 + len(utf16)] = utf16
    return bytes(e)


def fat32_bpb(total_sectors, hidden, label, marker):
    """A REAL FAT32 boot sector, not a stub: the content probe in install/partition.rs keys on the
    jump instruction, the 0x55AA signature, the bytes-per-sector field and the FAT count, and a
    fixture that only stamped 0x55AA would pass a probe it never actually exercised."""
    bs = bytearray(SECTOR)
    bs[0:3] = bytes([0xEB, 0x58, 0x90])
    bs[3:11] = b"FOREIGN "
    struct.pack_into("<H", bs, 11, SECTOR)        # bytes per sector
    bs[13] = 1                                    # sectors per cluster
    struct.pack_into("<H", bs, 14, 32)            # reserved sectors
    bs[16] = 2                                    # number of FATs
    bs[21] = 0xF8                                 # media descriptor
    struct.pack_into("<H", bs, 24, 63)
    struct.pack_into("<H", bs, 26, 255)
    struct.pack_into("<I", bs, 28, hidden)        # hidden sectors = partition LBA
    struct.pack_into("<I", bs, 32, total_sectors)
    struct.pack_into("<I", bs, 36, 120)           # sectors per FAT (plausible, never mounted)
    struct.pack_into("<I", bs, 44, 2)             # root cluster
    struct.pack_into("<H", bs, 48, 1)             # FSInfo
    struct.pack_into("<H", bs, 50, 6)             # backup boot sector
    bs[64] = 0x80
    bs[66] = 0x29
    struct.pack_into("<I", bs, 67, 0x0BADF00D)    # volume id — deliberately NOT UnaOS's 0x554E4153
    bs[71:82] = label.ljust(11)[:11].encode()
    bs[82:90] = b"FAT32   "
    # The marker lives in the boot code area, where a real FAT32 volume keeps its bootstrap: it is
    # what a human reading a hexdump of the neighbours-untouched check sees.
    bs[90:90 + len(marker)] = marker
    bs[510] = 0x55
    bs[511] = 0xAA
    return bytes(bs)


def apfs_superblock():
    """`NXSB` at offset 32 of the container superblock — nx_superblock_t.nx_magic. Everything else
    is left zero on purpose: we probe this magic and read nothing else, and a fixture that invented
    plausible APFS fields would be claiming a model we do not have."""
    s = bytearray(SECTOR)
    s[32:36] = b"NXSB"
    struct.pack_into("<I", s, 36, SECTOR)  # nx_block_size
    return bytes(s)


def hfs_plus_header():
    """HFS+ volume header: `H+` at offset 1024 of the volume (sector 2 with 512-byte sectors).
    Returned as the two sectors 1..2 so the caller can place it at the partition's +512."""
    s = bytearray(SECTOR * 2)
    s[SECTOR:SECTOR + 2] = b"H+"
    struct.pack_into(">H", s, SECTOR + 2, 4)  # version
    return bytes(s)


def main():
    ap = argparse.ArgumentParser(description="Build the PARTINSTALL GPT fixture disk.")
    ap.add_argument("-o", "--out", default="builder/part-fixture.img",
                    help="output image path (default: builder/part-fixture.img)")
    ap.add_argument("--hfs", action="store_true",
                    help="write part 0 as an HFS+ volume instead of FAT32 (exercises the HFS+ probe)")
    args = ap.parse_args()

    img = bytearray(DISK_SECTORS * SECTOR)

    first_usable = 2 + GPT_ARRAY_SECTORS                       # LBA 34
    backup_header_lba = DISK_SECTORS - 1
    backup_array_lba = DISK_SECTORS - 1 - GPT_ARRAY_SECTORS
    last_usable = backup_array_lba - 1

    entries = bytearray(GPT_ARRAY_SECTORS * SECTOR)
    for i, (name, tguid, first, count, kind) in enumerate(PARTS):
        last = first + count - 1
        if first < first_usable or last > last_usable:
            sys.exit("partition %d (%s) %d..%d escapes the usable range %d..%d"
                     % (i, name, first, last, first_usable, last_usable))
        off = i * GPT_ENTRY_SIZE
        entries[off:off + GPT_ENTRY_SIZE] = entry(
            tguid, first, last, name, ("UNAOS-FIXTURE-P%d" % i).encode())

        base = first * SECTOR
        if kind == "fat" and not args.hfs:
            img[base:base + SECTOR] = fat32_bpb(count, first, "FOREIGN VOL",
                                                b"UNAOS-FIXTURE-FOREIGN-FAT-DO-NOT-TOUCH")
        elif kind == "fat" and args.hfs:
            img[base + SECTOR:base + 3 * SECTOR] = hfs_plus_header()
        elif kind == "apfs":
            img[base:base + SECTOR] = apfs_superblock()
        # "empty" writes nothing: the image is already all zero.

    entries_crc = crc32(bytes(entries))

    img[0:SECTOR] = protective_mbr(DISK_SECTORS)
    disk_guid = derive_guid(b"UNAOS-PARTFIXTURE")
    img[SECTOR:2 * SECTOR] = header(1, backup_header_lba, first_usable, last_usable,
                                    disk_guid, 2, entries_crc)
    img[2 * SECTOR:2 * SECTOR + len(entries)] = entries
    img[backup_array_lba * SECTOR:backup_array_lba * SECTOR + len(entries)] = entries
    img[backup_header_lba * SECTOR:(backup_header_lba + 1) * SECTOR] = header(
        backup_header_lba, 1, first_usable, last_usable, disk_guid, backup_array_lba, entries_crc)

    out = os.path.abspath(args.out)
    os.makedirs(os.path.dirname(out), exist_ok=True)
    with open(out, "wb") as f:
        f.write(img)

    print("PARTINSTALL fixture -> %s (%d sectors, %d MiB)"
          % (out, DISK_SECTORS, DISK_SECTORS * SECTOR // (1024 * 1024)))
    print("  usable LBA %d..%d, backup array @%d, backup header @%d"
          % (first_usable, last_usable, backup_array_lba, backup_header_lba))
    for i, (name, _t, first, count, kind) in enumerate(PARTS):
        print("  part %d  %-13s lba %7d..%-7d (%6d sec, %4d MiB)  content=%s"
              % (i, name, first, first + count - 1, count,
                 count * SECTOR // (1024 * 1024), "hfs+" if (kind == "fat" and args.hfs) else kind))


if __name__ == "__main__":
    main()
