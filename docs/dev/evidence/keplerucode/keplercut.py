#!/usr/bin/env python3
# KEPLERCUT (rmbp-ledger B637, R127) — cut the GK107 (chipset E7) FECS/GPCCS ctxsw firmware out of NVIDIA's
# 325.15 nv-kernel.o. OUR OWN reimplementation of the PGRAPH-archive leg of envytools/firmware extract_firmware.py
# (MIT, Ilia Mirkin; commit a0b9f9be0efad90cc84b8b2eaf587c3d7d350ea9, file sha256
# b684b37b69a78f58fb3a6cf73684698a65927e701adedbae736ddbb0d1768ea7, lines 364-421). That script was READ, never run.
#
# The method it encodes (no fixed byte offsets — the archives are located by content):
#   * every b"\x1f\x8b\x08" (gzip magic + deflate) in nv-kernel.o starts a candidate; the candidate ends at the next one;
#   * skip the 10-byte fixed gzip header, raw-inflate the rest; a PGRAPH archive is `<u32 magic=0><u32 count>` then
#     `count` x `<u32 id><u32 length><u32 offset>`; ids 0/1/2/3 = fuc409d/fuc409c/fuc41ad/fuc41ac (script :364-369);
#   * for 325.15 the VALID archives (magic 0), in file order, belong to (script :371-375)
#     nvc0 nvc8 nvc3 nvc4 nvce nvcf nvc1 nvd7 nvd9 nve4 nve7 nve6 nvf0 nvf1 nv108 — nve7 (GK107) is index 10;
#   * code files (…c) are zero-padded by `0x200 - len % 0x200` bytes (script :407-409; a full 0x200 when already
#     aligned — reproduced exactly so the bytes equal what nouveau users load).
#
# Usage (run with `python3 -I` from a directory outside the download dir): keplercut.py <nv-kernel.o> <out-dir>
import os
import re
import struct
import sys
import zlib

ORDER_325_15 = ["nvc0", "nvc8", "nvc3", "nvc4", "nvce", "nvcf", "nvc1",
                "nvd7", "nvd9", "nve4", "nve7", "nve6", "nvf0", "nvf1", "nv108"]
NAMES = {0: "fuc409d", 1: "fuc409c", 2: "fuc41ad", 3: "fuc41ac"}
WANT = "nve7"


def main():
    src, out = sys.argv[1], sys.argv[2]
    with open(src, "rb") as f:
        kernel = f.read()
    starts = [m.start(0) for m in re.finditer(re.escape(b"\x1f\x8b\x08"), kernel)]
    print("candidates=%d" % len(starts))
    idx = 0
    found = None
    for start, end in zip(starts, starts[1:] + [len(kernel)]):
        s = kernel[start:end]
        try:
            data = zlib.decompressobj(-zlib.MAX_WBITS).decompress(s[10:])
        except Exception:
            continue
        if len(data) < 8:
            continue
        magic, count = struct.unpack("<II", data[:8])
        if magic != 0:
            continue
        if idx >= len(ORDER_325_15):
            print("EXTRA valid archive at 0x%x — order table exhausted; refusing" % start)
            sys.exit(2)
        chip = ORDER_325_15[idx]
        entries = [struct.unpack("<III", data[8 + i * 12:20 + i * 12]) for i in range(count)]
        print("archive %2d %-5s gzip@0x%07x inflated=%d entries=%s" % (
            idx, chip, start, len(data),
            " ".join("id%d:len=0x%x@0x%x" % e for e in entries)))
        if chip == WANT:
            found = (start, data, entries)
        idx += 1
    if idx != len(ORDER_325_15):
        print("valid archives=%d, expected %d — refusing (graph fw likely wrong)" % (idx, len(ORDER_325_15)))
        sys.exit(2)
    if found is None:
        print("no %s archive" % WANT)
        sys.exit(2)
    start, data, entries = found
    os.makedirs(out, exist_ok=True)
    for eid, length, off in entries:
        if eid not in NAMES:
            continue
        name = NAMES[eid]
        blob = data[off:off + length]
        if len(blob) != length:
            print("%s truncated" % name)
            sys.exit(2)
        if name.endswith("c"):
            blob += b"\0" * (0x200 - length % 0x200)
        with open(os.path.join(out, name), "wb") as f:
            f.write(blob)
        print("wrote %s raw_len=0x%x (%d) inflated_off=0x%x file_bytes=%d" % (name, length, length, off, len(blob)))


if __name__ == "__main__":
    main()
