#!/usr/bin/env bash
# mutate11.sh — CAN-FIRE PROOF for scorer11.sh.
#
# Builds a fixture set by MUTATING DATA (never the scorer) and drives every leg through every
# outcome it can produce. Two axes:
#   WIRE     — mutations of the render9 capture's last boot (~/unaos-bench/scratch/orin19/
#              pointerlag/boot-render9.log, NUL-stripped, lines 19900+), edited into the render11
#              contract's shape: the retired `[sdmmc] root` family deleted, the `[vfs] root …`
#              line inserted. Nothing is invented that the contract does not specify.
#   ARTIFACT — four kernel.elf fixtures built from real builds:
#                armed      = unaos/target/aarch64_esp/kernel.elf + the arming literal appended
#                unarmed    = that same kernel.elf, untouched (control fires, arming literal 0)
#                oldfamily  = armed + the retired `[sdmmc] root` literal appended
#                nocontrol  = a file carrying the arming literal and NO control (not a kernel)
#                missing    = a path that does not exist
#
# usage: mutate11.sh          (builds fixtures, runs the matrix, writes MUTATIONS.md)
set -uo pipefail
D=~/unaos-bench/scratch/orin22/stage11
F="$D/fix"; O="$D/out"; mkdir -p "$F" "$O"
S="$D/scorer11.sh"
SRC=~/unaos-bench/scratch/orin19/pointerlag/boot-render9.log
REPO_ELF=/home/pmes/src/github.com/pmes/UnaOS-orin/unaos/target/aarch64_esp/kernel.elf

BIND='[vfs] root = boot volume serial=0xde001a13 source=usb ::'
OLD='[sdmmc] root mount source=tegra-sd card_blocks=62333952 -> OK label="UNAOS-PI" vol_id=0xabfbdefa ::'

# ── artifact fixtures ────────────────────────────────────────────────────────────────────────────
{ cat "$REPO_ELF"; printf '\n[vfs] root = boot volume serial=0x%%08x source=%%s ::\n'; } > "$F/elf-armed.bin"
cp "$REPO_ELF" "$F/elf-unarmed.bin"
{ cat "$F/elf-armed.bin"; printf '\n[sdmmc] root mount source=tegra-sd ::\n'; } > "$F/elf-oldfamily.bin"
printf '[vfs] root = boot volume serial=0x%%08x ::\nnot a kernel image\n' > "$F/elf-nocontrol.bin"

# ── wire fixtures ────────────────────────────────────────────────────────────────────────────────
LC_ALL=C tr -d '\000' < "$SRC" | LC_ALL=C awk 'NR>=19900' > "$F/w-slice.log"
# W0 base PASS: retired family deleted, contract bind line inserted, serial == the loader's.
LC_ALL=C awk 'index($0,"[sdmmc] root"){next} {print}' "$F/w-slice.log" > "$F/W0.log"; echo "$BIND" >> "$F/W0.log"
mk(){ cp "$F/W0.log" "$F/$1.log"; }                                  # start from the PASS fixture
del(){ LC_ALL=C awk -v p="$2" 'index($0,p){next}{print}' "$F/$1.log" > "$F/$1.tmp" && mv "$F/$1.tmp" "$F/$1.log"; }
add(){ printf '%s\n' "$2" >> "$F/$1.log"; }

mk W1;  del W1 "boot volume FAT serial"
mk W2;  del W2 "crates/bootloader/src/main.rs@"
mk W3;  del W3 "[vfs] root = boot volume"; add W3 '[vfs] root = boot volume serial=0x0badcafe source=usb ::'
mk W4;  del W4 "[vfs] root = boot volume"
mk W5;  del W5 "[vfs] root = boot volume"; add W5 '[vfs] root -> NONE reason=kernel-not-found-on-any-volume serial=0x00000000 disks=3 ::'
mk W6;  del W6 "[vfs] root = boot volume"; add W6 '[vfs] root -> NONE reason=multiple-kernels serial=0xde001a13 disks=global,sdhc0 ::'
mk W7;  add W7 "$OLD"
mk W8;  add W8 '[ INFO]: crates/bootloader/src/main.rs@961: boot volume FAT serial 0xfeed0001 (e'
mk W9;  del W9 "[vfs] root = boot volume"; add W9 '[vfs] root = boot volume serial=0xde001a13 source=potato ::'
mk W10; del W10 "[vfs] root = boot volume"; add W10 '[vfs] root = boot volume source=usb ::'
LC_ALL=C awk 'index($0,"crates/bootloader/src/main.rs@")||index($0,"boot volume FAT serial"){print}' "$F/W0.log" > "$F/W11.log"
mk W12; del W12 "[vfs] root = boot volume"; add W12 '[vfs] root -> NONE reason=disk-enumeration-timed-out serial=0xde001a13 disks=0 ::'
mk W13; del W13 "[vfs] root = boot volume"; add W13 '[vfs] root = boot volume serial=0xde001a13 ::'
mk W14; add W14 '[vfs] root = boot volume serial=0x0badcafe source=sdhc ::'
: > "$F/W15.log"

# ── the matrix: id | wire | artifact | what was changed | legs to watch ─────────────────────────
run(){ # run <id> <wire-fixture> <artifact-fixture> <description>
  id="$1"; w="$2"; a="$3"; desc="$4"; out="$O/$id.out"
  LC_ALL=C bash "$S" "$F/$w" "$a" > "$out" 2>&1 < /dev/null; ec=$?
  rows=$(LC_ALL=C awk '/^== render11 verdict table ==/{t=1;next} t&&/^$/{exit} t&&NF>=3&&$1!="PREDICATE"&&$1!~/^-/{printf "%s=%s ",$1,$3}' "$out")
  [ -n "$rows" ] || rows="(refused before scoring)"
  sum=$(LC_ALL=C awk '/^== [0-9]+ red/{print; exit}' "$out"); [ -n "$sum" ] || sum="-"
  printf '%s\t%s\texit=%s\t%s\t%s\n' "$id" "$desc" "$ec" "$rows" "$sum"
}

TSV="$D/MUTATIONS.tsv"; : > "$TSV"
{
run M00 W0.log  "$F/elf-armed.bin"      "BASE (contract-shaped PASS wire, armed artifact)"
run M01 W1.log  "$F/elf-armed.bin"      "loader serial line DELETED (loader still speaks)"
run M02 W2.log  "$F/elf-armed.bin"      "ALL loader lines DELETED (loader control zero)"
run M03 W3.log  "$F/elf-armed.bin"      "bind serial ALTERED to 0x0badcafe (mismatch)"
run M04 W4.log  "$F/elf-armed.bin"      "bind line DELETED, no NONE line"
run M05 W5.log  "$F/elf-armed.bin"      "NONE inserted, reason=kernel-not-found-on-any-volume"
run M06 W6.log  "$F/elf-armed.bin"      "NONE inserted, reason=multiple-kernels"
run M07 W7.log  "$F/elf-armed.bin"      "retired [sdmmc] root line INSERTED on the wire"
run M08 W8.log  "$F/elf-armed.bin"      "second loader serial 0xfeed0001 INSERTED (two boots)"
run M09 W9.log  "$F/elf-armed.bin"      "bind source ALTERED to source=potato"
run M10 W10.log "$F/elf-armed.bin"      "bind line with NO serial= field"
run M11 W11.log "$F/elf-armed.bin"      "loader-only wire (kernel control zero)"
run M12 W12.log "$F/elf-armed.bin"      "NONE with an UNDOCUMENTED reason"
run M13 W13.log "$F/elf-armed.bin"      "bind line with NO source= field"
run M14 W14.log "$F/elf-armed.bin"      "second bind line, DIFFERENT serial"
run M15 W15.log "$F/elf-armed.bin"      "EMPTY wire, armed artifact"
run M16 W4.log  "$F/elf-unarmed.bin"    "UNARMED artifact + wire with no [vfs] lines"
run M17 W0.log  "$F/elf-unarmed.bin"    "UNARMED artifact + wire that HAS [vfs] lines"
run M18 W0.log  "$F/elf-nocontrol.bin"  "artifact with arming literal but NO positive control"
run M19 W0.log  "$F/elf-missing.bin"    "artifact path does not exist"
run M20 W0.log  "$F/elf-oldfamily.bin"  "clean wire, artifact still carries [sdmmc] root"
} | tee "$TSV"
echo
echo "fixtures: $F   outputs: $O   tsv: $TSV"
