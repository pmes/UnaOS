#!/usr/bin/env bash
# kepler-bunker.sh — KEPLERGOLD (rmbp-ledger B656). Cuts the GK107's four FECS/GPCCS context-switch firmware files
# (fuc409c fuc409d fuc41ac fuc41ad) out of NVIDIA's own Linux driver INTO THE BUNKER, b43-fwcutter style.
#
#   R119 (Peter, 2026-10-14): "it is assumed all users will be creating a bunker since all hardware seems to have
#        proprietary bits who's quick solution is the bunker."
#   R127 (Peter, 2026-10-14, "1"): the four files are CUT FROM NVIDIA'S OWN DRIVER INTO THE BUNKER
#        (`drivers/nvidia-gk107/`, SHA256SUMS, PROVENANCE: the driver version, its URL, the offsets, the date),
#        digests pinned in-tree (`unaos/firmware/kepler.pins`); proprietary bits never enter the tree.
#
# RUNS ON THE BENCH ONLY. It downloads nothing: the operator fetches the driver package (the URL is printed by
# `--help`) and hands it in. It writes nothing under unaos/ — the pin rows it prints are pasted into
# unaos/firmware/kepler.pins by the bench (no digest enters the tree that was not measured on the bench).
#
# THE RECIPE is the public record of WHICH BYTES of the driver are these files: envytools' extract_firmware.py
# (https://github.com/envytools/firmware, MIT, Copyright 2013 Ilia Mirkin). Its source could not be fetched verbatim
# from the session that wrote this script, so the facts below are cited by identifier, not line
# (docs/dev/evidence/keplergold/RESUME.md §3, KEPLERGOLD.md §1):
#   * VERSIONS = 319.17 319.23 319.32 325.08 325.15 340.32 340.108; ARCHES = x86_64 x86. Input: the extracted
#     NVIDIA-Linux-<ARCH>-<VERSION>/ tree; the PGRAPH firmware lives in kernel/nv-kernel.o.
#   * The PGRAPH archives: every gzip magic 1f 8b 08 in nv-kernel.o starts a stream that runs to the next magic or
#     EOF; inflate stream[10:] as raw deflate (zlib wbits -MAX_WBITS); a stream is an ARCHIVE iff it inflates and
#     its first LE u32 (magic) is 0. Header <II (magic, count); entry i at 8+12*i is <III (id, length, offset),
#     offset from the start of the inflated data; the file is data[offset:offset+length].
#   * ARCHIVE_FILES = {0: fuc409d, 1: fuc409c, 2: fuc41ad, 3: fuc41ac}.
#   * ARCHIVE_ORDERS carries ONE version, 325.15: nvc0 nvc8 nvc3 nvc4 nvce nvcf nvc1 nvd7 nvd9 nve4 nve7 nve6 nvf0
#     nvf1 nv108 — fifteen archives; nve7 (GK107) is index 10. Another count is "graph fw likely wrong".
#   * A CODE file (name ending in c) gains 0x200 - len % 0x200 zero bytes, unconditionally; data files are not padded.
#     (So the bunker's bytes equal a nouveau user's /lib/firmware/nouveau/nve7_fuc409c etc.)
# The envytools script checks no package digest; this script MEASURES the package's sha256 and records it.
#
# Output (in <bunker dir>): fuc409c fuc409d fuc41ac fuc41ad (the names drivers/gpu/kepler_ucode.rs::SET reads from
# the ESP's FW/KEPLER/), SHA256SUMS, PROVENANCE; on stdout the four kepler.pins rows `<role> <sha256>`.
set -euo pipefail

USAGE='usage: kepler-bunker.sh [--order nvc0,nvc8,...] <NVIDIA-Linux-x86_64-325.15.run | NVIDIA-Linux-x86_64-325.15/> <bunker>/drivers/nvidia-gk107'
ORDER_325_15='nvc0,nvc8,nvc3,nvc4,nvce,nvcf,nvc1,nvd7,nvd9,nve4,nve7,nve6,nvf0,nvf1,nv108'
VERSIONS=' 319.17 319.23 319.32 325.08 325.15 340.32 340.108 '

order=''
case "${1:-}" in
  -h|--help)
    echo "$USAGE"
    echo "the package (fetched by the operator, never by this script):"
    echo "  http://us.download.nvidia.com/XFree86/Linux-x86_64/325.15/NVIDIA-Linux-x86_64-325.15.run"
    echo "  (x86: http://us.download.nvidia.com/XFree86/Linux-x86/325.15/NVIDIA-Linux-x86-325.15.run)"
    exit 0 ;;
  --order) order="${2:-}"; shift 2 || { echo "$USAGE" >&2; exit 2; } ;;
esac
[ $# -eq 2 ] || { echo "$USAGE" >&2; exit 2; }
src="$1"; out="$2"

base="$(basename "${src%/}")"
base="${base%.run}"
arch=''; ver=''
case "$base" in
  NVIDIA-Linux-x86_64-*) arch='x86_64'; ver="${base#NVIDIA-Linux-x86_64-}" ;;
  NVIDIA-Linux-x86-*)    arch='x86';    ver="${base#NVIDIA-Linux-x86-}" ;;
  *) echo "kepler-bunker: REFUSED — '$base' is not NVIDIA-Linux-<x86_64|x86>-<version>[.run]" >&2; exit 2 ;;
esac
case "$VERSIONS" in
  *" $ver "*) ;;
  *) echo "kepler-bunker: REFUSED — version $ver is not in extract_firmware.py's VERSIONS ($VERSIONS)" >&2; exit 2 ;;
esac
if [ -z "$order" ]; then
  if [ "$ver" = "325.15" ]; then
    order="$ORDER_325_15"
  else
    echo "kepler-bunker: REFUSED — extract_firmware.py's ARCHIVE_ORDERS names only 325.15; for $ver pass --order <list>" >&2
    exit 2
  fi
fi

mkdir -p "$out"
work=''
pkg_sha='unmeasured (an extracted tree was given, not the .run)'
url="http://us.download.nvidia.com/XFree86/Linux-${arch}/${ver}/NVIDIA-Linux-${arch}-${ver}.run"
if [ -f "$src" ]; then
  pkg_sha="$(sha256sum "$src" | awk '{print $1}')"
  work="$out/.extract-$ver"
  rm -rf "$work"
  sh "$src" --extract-only --target "$work" > "$out/.extract.log" 2>&1 || {
    echo "kepler-bunker: REFUSED — '$src' did not extract (see $out/.extract.log)" >&2; exit 2; }
  tree="$work"
elif [ -d "$src" ]; then
  tree="${src%/}"
else
  echo "kepler-bunker: REFUSED — '$src' is neither a file nor a directory" >&2; exit 2
fi
obj="$tree/kernel/nv-kernel.o"
[ -f "$obj" ] || { echo "kepler-bunker: REFUSED — no kernel/nv-kernel.o under $tree" >&2; exit 2; }
obj_sha="$(sha256sum "$obj" | awk '{print $1}')"

# The walk (extract_firmware.py's PGRAPH-archive arm, GK107 only). Prints one `entry` line per file written.
entries="$(python3 -I - "$obj" "$out" "$order" <<'PY'
import struct, sys, zlib
obj, out, order = sys.argv[1], sys.argv[2], sys.argv[3].split(',')
data = open(obj, 'rb').read()
names = {0: 'fuc409d', 1: 'fuc409c', 2: 'fuc41ad', 3: 'fuc41ac'}   # ARCHIVE_FILES
starts, i = [], data.find(b'\x1f\x8b\x08')
while i >= 0:
    starts.append(i)
    i = data.find(b'\x1f\x8b\x08', i + 1)
archives = []
for k, s in enumerate(starts):
    e = starts[k + 1] if k + 1 < len(starts) else len(data)
    try:
        blob = zlib.decompressobj(-zlib.MAX_WBITS).decompress(data[s + 10:e])
    except zlib.error:
        continue
    if len(blob) < 8 or struct.unpack_from('<I', blob, 0)[0] != 0:
        continue
    archives.append((s, blob))
if len(archives) != len(order):
    sys.stderr.write('kepler-bunker: REFUSED — %d PGRAPH archives in nv-kernel.o, the order names %d '
                     '("Unexpected quantity of archives in blob, graph fw likely wrong.")\n' % (len(archives), len(order)))
    sys.exit(3)
if 'nve7' not in order:
    sys.stderr.write('kepler-bunker: REFUSED — the order names no nve7\n')
    sys.exit(3)
idx = order.index('nve7')
off, blob = archives[idx]
count = struct.unpack_from('<I', blob, 4)[0]
got = set()
for n in range(count):
    eid, length, eoff = struct.unpack_from('<III', blob, 8 + n * 12)
    name = names.get(eid)
    if name is None:
        continue
    f = blob[eoff:eoff + length]
    if len(f) != length:
        sys.stderr.write('kepler-bunker: REFUSED — entry %d runs past its archive\n' % eid)
        sys.exit(3)
    raw = length
    if name.endswith('c'):
        f += b'\x00' * (0x200 - length % 0x200)
    open('%s/%s' % (out, name), 'wb').write(f)
    got.add(name)
    print('entry archive=%d stream_offset=0x%x id=%d name=%s length=%d offset=0x%x written=%d' % (idx, off, eid, name, raw, eoff, len(f)))
if got != set(names.values()):
    sys.stderr.write('kepler-bunker: REFUSED — archive %d carries %s, not all four\n' % (idx, sorted(got)))
    sys.exit(3)
PY
)" || exit $?

[ -n "$work" ] && rm -rf "$work"

( cd "$out" && sha256sum fuc409c fuc409d fuc41ac fuc41ad > SHA256SUMS )
{
  echo "# PROVENANCE — R127: the GK107 FECS/GPCCS firmware cut from NVIDIA's own driver (unaos/scripts/kepler-bunker.sh)"
  echo "driver=NVIDIA-Linux-${arch}-${ver}"
  echo "url=$url"
  echo "package_sha256=$pkg_sha"
  echo "nv_kernel_o_sha256=$obj_sha"
  echo "recipe=envytools extract_firmware.py PGRAPH archives, ARCHIVE_ORDERS[$ver]=$order, nve7"
  echo "$entries"
  echo "date=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
} > "$out/PROVENANCE"

cat "$out/PROVENANCE"
echo "--- SHA256SUMS ($out)"
cat "$out/SHA256SUMS"
echo "--- kepler.pins rows (paste into unaos/firmware/kepler.pins):"
for pair in fecs_code:fuc409c fecs_data:fuc409d gpccs_code:fuc41ac gpccs_data:fuc41ad; do
  role="${pair%%:*}"; file="${pair#*:}"
  echo "$role $(sha256sum "$out/$file" | awk '{print $1}')"
done
