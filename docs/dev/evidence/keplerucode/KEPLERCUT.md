# KEPLERCUT — the GK107 ctxsw set cut from NVIDIA's driver into the bunker (rmbp-ledger B637, R127)

Executor: exec-rmbp-local-keplercut, cut from d0f907978 (the KEPLERUCODE tip), seat rmbp-0915/local3, 2026-10-13.
R127 (Peter: "1") answers keplerucode.md §3.1: the four files are cut from NVIDIA's own driver into the bunker.
This file holds the download, the cut, the digests and the build line that checked them.

## 1. Sources (each fetched as DATA into its own new empty dir under ~/unaos-bench/scratch/rmbp-0915/local3/keplercut/dl/)

| what | URL | bytes | sha256 |
|---|---|---|---|
| nouveau's extraction script (READ, never run) | envytools/firmware `master` `extract_firmware.py` (raw, from the envytools/firmware repository on GitHub); last commit touching it `a0b9f9be0efad90cc84b8b2eaf587c3d7d350ea9` 2020-05-26 "Support Python 3…"; the ctxsw leg fixed for 325.15 in `7d29d955` 2020-05-04 | 13869 | `b684b37b69a78f58fb3a6cf73684698a65927e701adedbae736ddbb0d1768ea7` |
| NVIDIA driver 325.15 x86_64 | https://us.download.nvidia.com/XFree86/Linux-x86_64/325.15/NVIDIA-Linux-x86_64-325.15.run | 48034648 | `01446fbd94f6eb3b2e1b3d3f2b06970bf470c3ee2cb1838b3d0c2416e6c74500` |

Why 325.15: the script lists seven versions (`VERSIONS`, :40-48) but carries a PGRAPH archive order for ONE —
`ARCHIVE_ORDERS = {"325.15": [...]}` (:371-375); any other version prints "Unknown PGRAPH archive order" and names the
archives blob0…blobN (:423-430). 325.15 is still served by NVIDIA (HTTP 301 http→https, then 200).

## 2. The payload, taken without executing the installer

The `.run` is a makeself archive; its header (read with `head -n 1101 | grep -a`) carries `MD5=848d516534827a721bcd76c6107456d8`
(line 3), `skip=1101` (line 4), and the stub's own listing command `tail -n +$skip $0 | xz -d | tar tvf -` (line 738).

```
tail -n +4 NVIDIA-Linux-x86_64-325.15.run | md5sum   -> 848d516534827a721bcd76c6107456d8   (= the header's MD5)
tail -n +1101 NVIDIA-Linux-x86_64-325.15.run > payload/payload.tar.xz ; xz -d -k payload/payload.tar.xz      rc=0
tar tvf payload/payload.tar                          -> 167 members, none absolute, none with ..
tar xf payload/payload.tar -C x --no-same-owner --no-same-permissions ./kernel/nv-kernel.o ./LICENSE
nv-kernel.o  12638296 B  sha256 a418369198dc8ef3096c56cf8e4a010e3d39c0fb687af2ff6e51c41e4dc1943f  (2013-07-31)
```

Nothing from the package was executed; only `nv-kernel.o` (and `LICENSE`, to read) left the tarball.

## 3. The cut — our own script, `keplercut.py` (beside this file)

`keplercut.py` reimplements the script's PGRAPH-archive leg (:364-421) — no fixed byte offsets exist in it: the
archives are located by content. Every `1f 8b 08` in nv-kernel.o starts a candidate (to the next one); the 10-byte
gzip header is skipped and the rest raw-inflated; an archive is `<u32 0><u32 count>` + `count` × `<id, length, offset>`;
ids 0/1/2/3 = fuc409d/fuc409c/fuc41ad/fuc41ac (:364-369); the valid archives in file order are the 325.15 table, nve7 =
index 10; code files get `0x200 - len % 0x200` zero bytes (:407-409). It refuses if the count of valid archives differs
from the table (15). Run: `python3 -I <evidence>/keplercut.py <dl>/x/kernel/nv-kernel.o <scratch>/out` from the worktree
(outside the dl dir), rc=0:

```
candidates=17                     (15 valid archives = the table's 15 chipsets; 2 non-archive gzip streams skipped)
archive  9 nve4  gzip@0x076b6c4 inflated=56525 …
archive 10 nve7  gzip@0x07706c4 inflated=56525 entries=id1:len=0x40b3@0x104 id0:len=0x700@0x41b7 id3:len=0x250a@0x48b7 id2:len=0x600@0x6dc1 …
archive 11 nve6  gzip@0x07756c4 inflated=56525 …
wrote fuc409c raw_len=0x40b3 (16563) inflated_off=0x104 file_bytes=16896
wrote fuc409d raw_len=0x700 (1792) inflated_off=0x41b7 file_bytes=1792
wrote fuc41ac raw_len=0x250a (9482) inflated_off=0x48b7 file_bytes=9728
wrote fuc41ad raw_len=0x600 (1536) inflated_off=0x6dc1 file_bytes=1536
```

The offsets that pin this cut: gzip stream at nv-kernel.o file offset **0x07706c4**; inside the inflated archive
fuc409c @0x104 len 0x40b3, fuc409d @0x41b7 len 0x700, fuc41ac @0x48b7 len 0x250a, fuc41ad @0x6dc1 len 0x600.
(nve4/nve7/nve6 — the three GK10x — carry archives of identical layout; the full 15-row log is in the bench scratch
`cut.log`, not cited further.)

## 4. The set — bunker `drivers/nvidia-gk107/` (bunker commit 58ab57e on its main; not pushed)

| file | role (kepler.pins) | bytes | sha256 |
|---|---|---|---|
| fuc409c | fecs_code (FECS IMEM) | 16896 (0x4200) | `d6433d02c2cd0798cdd0fd6aea0709cfa97f60cccacb1c83fdba02f181ba28b9` |
| fuc409d | fecs_data (FECS DMEM) | 1792 (0x700) | `69086258f93c8d37685a04999fdb73af30e2cc6a2ccb10c2d78f4e7e734a4a9a` |
| fuc41ac | gpccs_code (GPCCS IMEM) | 9728 (0x2600) | `9127043ea43960e71de1ff31c51eae6e344d06eff8abf4e57bef6210a39fc02c` |
| fuc41ad | gpccs_data (GPCCS DMEM) | 1536 (0x600) | `63adc4049a3080aaca7a836e9842499a13c1087f0d198ae91610630478f5b08e` |

`sha256sum -c SHA256SUMS` in the bunker dir: 4 × OK. `PROVENANCE.md` carries the entry (version, URL, both digests,
the script's source and commit, the offsets, the licence).

**Sizes against what keplerucode.md / kepler_ucode.rs expect.** keplerucode.md fixes no numeric IMEM/DMEM size — the
kernel reads each falcon's HWCFG (falcon+0x108: IMEM = (r & 0x1ff) << 8, DMEM = (r & 0x3fe00) >> 1, base.c:273-275,
kepler_ucode.rs:277-282) on the metal and refuses `reason=hwcfg`/`too-large` if a file exceeds it; the only static
bounds are the stage check `4 ..= MAX_FILE` (128 KiB, kepler_ucode.rs:63, :187): all four pass. Code files are
whole 256-byte IMEM blocks (0x4200 = 66 blocks, 0x2600 = 38), data files whole words. Whether 0x4200/0x700 and
0x2600/0x600 fit the GK107's FECS/GPCCS is answered by the first `tests kgr` line `[kgr] ucode limits … -> fits`.

## 5. Licence, stated plainly

NVIDIA's proprietary driver licence (`LICENSE` in the payload, 5867 B, 2013-06-15). §2.1.2 (Linux/FreeBSD exception)
allows redistributing the driver only with its binary files unmodified; nothing grants redistribution of extracted
blobs. §2.1.3 forbids reverse engineering and the separation of components — which any extraction is by construction;
this is the established nouveau-user practice, done on Peter's ruling R127, and recorded rather than resolved. The
files live ONLY in the private bunker; this tree holds their digests (`unaos/firmware/kepler.pins`) and this script.

## 6. The build check (arroyo KEPLER-FW, one full media build, in-tree)

The bench line, verbatim, from `unaos/` (wrapped in a scratch shell script because the agent sandbox refuses `.`
on a command line; the script's body is exactly this):

```
export UNAOS_WIFI_FW_PATH=/home/pmes/src/github.com/pmes/UnaOS-bunker/drivers/broadcom-b43 UNAOS_KEPLER_FW_PATH=/home/pmes/src/github.com/pmes/UnaOS-bunker/drivers/nvidia-gk107
set -a; . ./scripts/image-line.env; set +a
export UNAOS_AETHER=1 UNAOS_MIDDEN=1 UNAOS_UNA=1 UNAOS_SONDE=1
./arroyo esp-x86            -> rc=0 (no QEMU: "packaging the ESP (no QEMU)")
```

The build's lines (log filtered with `awk 'index($0,"KEPLER-FW")'`, ANSI colours stripped):

```
⚡ KEPLER-FW: GK107 ctxsw set (4 files) from /home/pmes/src/github.com/pmes/UnaOS-bunker/drivers/nvidia-gk107 onto the ESP at FW/KEPLER/ (bench-only, never in-tree)
⚡ KEPLER-FW: fuc409c sha256=d6433d02c2cd0798cdd0fd6aea0709cfa97f60cccacb1c83fdba02f181ba28b9 bytes=16896 pin-row: fecs_code d6433d02c2cd0798cdd0fd6aea0709cfa97f60cccacb1c83fdba02f181ba28b9
⚡ KEPLER-FW: fuc409d sha256=69086258f93c8d37685a04999fdb73af30e2cc6a2ccb10c2d78f4e7e734a4a9a bytes=1792 pin-row: fecs_data 69086258f93c8d37685a04999fdb73af30e2cc6a2ccb10c2d78f4e7e734a4a9a
⚡ KEPLER-FW: fuc41ac sha256=9127043ea43960e71de1ff31c51eae6e344d06eff8abf4e57bef6210a39fc02c bytes=9728 pin-row: gpccs_code 9127043ea43960e71de1ff31c51eae6e344d06eff8abf4e57bef6210a39fc02c
⚡ KEPLER-FW: fuc41ad sha256=63adc4049a3080aaca7a836e9842499a13c1087f0d198ae91610630478f5b08e bytes=1536 pin-row: gpccs_data 63adc4049a3080aaca7a836e9842499a13c1087f0d198ae91610630478f5b08e
⚡ KEPLER-FW: firmware/kepler.pins vs the staged set -> MATCH (the kernel uploads only a pinned match; UNPINNED/MISMATCH reads reason=unpinned/pin-mismatch at tests kgr)
⚡ WIFI-FW: firmware/b43.pins vs the staged set -> MATCH (…)
✅ x86 CARD (UnaFS root): …/unaos/target/unaos-x86-card.img      sha256: 6ade404a48ebc6c3de0f256e4061a954dc599737252e1fac34c903d2325c4945
```

`target/x86_64_esp/FW/KEPLER/` held the four at 16896/1792/9728/1536 bytes. The card was built from the working tree
with these pins uncommitted (its SRC.TGZ therefore reads dirty); it is a check of the pins, not a flight image, and
every `target/` dir of the worktree was deleted after the run.

## 7. Seat knowledge

`UNAOS_KEPLER_FW_PATH=/home/pmes/src/github.com/pmes/UnaOS-bunker/drivers/nvidia-gk107` belongs on the bench media
line beside `UNAOS_WIFI_FW_PATH` (not added to `scripts/image-line.env` by this arc). It stages FW/KEPLER/ on p1;
the kernel only touches it at `tests kgr` (R80). Expected first metal read: keplerucode.md §5's second block, most
likely ending `-> ready-wall` (GR's own init, `gk104_gr_init`, has never run) or `DECLINED reason=no-golden-context`.
