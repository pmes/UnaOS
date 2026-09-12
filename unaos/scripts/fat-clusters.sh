#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# fat-clusters.sh <image-or-device> [p1_start_lba]  — FATCLUST C15 (orin session, 2026-09-12; orin-ledger A58).
#
# Parse the FAT boot sector at <p1_start_lba> (default: partition 1's start read from the MBR at
# byte 454) and print the volume's cluster count and the FAT kind THAT COUNT implies. The FAT type
# is defined solely by the count of data clusters (Microsoft FAT spec; EDK2 FatDxe; fs/fat.rs
# `FatKind`): < 4085 FAT12, < 65,525 FAT16, else FAT32 — whatever the BPB's FS-type string says.
# A FAT32 BPB on a FAT16-sized volume is exactly what mkfs.fat -F 32 produces at 127 MiB with
# 8 sectors per cluster (32,440 clusters): Linux mounts it, EDK2 lists no boot option, our kernel
# reads the 32-bit FAT as 16-bit. Exit 0 only when the written BPB is a FAT32 record AND the
# count is >= 65,525; exit 1 with the numbers otherwise; exit 2 when the sector is not a BPB.
# Output (one line, stable for callers): FATCLUST p1_start=<lba> bps= spc= rsvd= nfats= tot= fatsz= data= clusters= kind= -> PASS|FAIL
set -u
IMG="${1:?image or device}"; P1="${2:-}"
rd() { dd if="$IMG" bs=1 skip="$1" count="$2" 2>/dev/null | od -An -v -t u1 | tr -s ' \n' ' '; }
le() { local s=0 i=0; for b in $1; do s=$(( s + (b << (8*i)) )); i=$((i+1)); done; echo "$s"; }
if [ -z "$P1" ]; then P1=$(le "$(rd 454 4)"); fi
B=$(( P1 * 512 ))
bps=$(le "$(rd $((B+11)) 2)"); spc=$(le "$(rd $((B+13)) 1)"); rsvd=$(le "$(rd $((B+14)) 2)")
nfats=$(le "$(rd $((B+16)) 1)"); rootent=$(le "$(rd $((B+17)) 2)"); tot16=$(le "$(rd $((B+19)) 2)")
fatsz16=$(le "$(rd $((B+22)) 2)"); tot32=$(le "$(rd $((B+32)) 4)"); fatsz32=$(le "$(rd $((B+36)) 4)")
sig=$(le "$(rd $((B+510)) 2)")
if [ "$sig" -ne 43605 ] || [ "$bps" -eq 0 ] || [ "$spc" -eq 0 ] || [ "$nfats" -eq 0 ]; then
    echo "FATCLUST p1_start=$P1 -> NOT-A-BPB (sig=$sig bps=$bps spc=$spc nfats=$nfats)"; exit 2
fi
tot=$tot16; [ "$tot" -eq 0 ] && tot=$tot32
fatsz=$fatsz16; [ "$fatsz" -eq 0 ] && fatsz=$fatsz32
rootsec=$(( (rootent * 32 + bps - 1) / bps ))
data=$(( tot - rsvd - nfats * fatsz - rootsec ))
clusters=$(( data / spc ))
if [ "$clusters" -lt 4085 ]; then kind=FAT12; elif [ "$clusters" -lt 65525 ]; then kind=FAT16; else kind=FAT32; fi
bpb32=no; [ "$fatsz16" -eq 0 ] && [ "$rootent" -eq 0 ] && bpb32=yes
verdict=FAIL; [ "$kind" = FAT32 ] && [ "$bpb32" = yes ] && verdict=PASS
echo "FATCLUST p1_start=$P1 bps=$bps spc=$spc rsvd=$rsvd nfats=$nfats tot=$tot fatsz=$fatsz data=$data clusters=$clusters kind=$kind bpb32=$bpb32 -> $verdict"
[ "$verdict" = PASS ]
