# JD19 bench card — read-only forensic verbs: `stat` + `hexdump` (attended)

JD19 adds two read-only inspection verbs to the panel shell — **zero mutation, no new `fat.rs` surface.**
`stat <path>` prints one directory entry's full on-disk detail (kind, size, the raw FAT attr byte + decoded
flags, first cluster, the FAT last-write stamp, and the forensic dir-entry LBA + slot offset); `hexdump <path>
[off] [len]` is a bounded hexdump of a file's raw bytes at an absolute offset. This card confirms each on
silicon, including a power-cycle re-read. Thanks to JD11 it leaves a durable serial transcript. Detail +
rationale: [`arch_arm64.md` §JD19](../../docs/dev/OS/01_BOOT_HAL/arch_arm64.md). Pairs cleanly with the
JD15–JD18 benches in one attended session.

## 0. Prep the media (Peter flashes — session cannot write `/Volumes`)
- **Kernel:** rebuild the tegra ESP LAST (any `test-arm` clobbers it) and validate by COUNT:
  `UNAOS_TEGRA=1 ./arroyo esp-jetson` → `strings target/aarch64_esp/kernel.elf | grep -c 'tegra:'`
  must be **UNCHANGED from JD18** (JD19 adds no `tegra:` token).
- **⛔ Flash ONLY from the staged artifact, never from `target/`** (the standing hard rule —
  `~/unaos-bench/flash/README.md`). The builder copies the whole `aarch64_esp` payload (`EFI/` + `kernel.elf`)
  to `~/unaos-bench/flash/orin/UnaOS-orin-esp-<UTCstampZ>-<git7>.tar` and appends a `MANIFEST` line with its
  sha256; verify that sha256 before flashing. Untar it onto the boot stick, `dot_clean`, eject. Validate the
  kernel by `tegra:` count, not size. (This arc's staged artifact + sha are quoted in the JD19 landing report.)
- **Data card:** a **separate** FAT16 card (the tegra pattern — the boot stick is NOT the block device),
  present AT BOOT, in the reader behind the hub. **⚠ `dot_clean` the DATA card too** and strip `._*` AppleDouble
  sidecars. Seed it (from the host, before ejecting) with **one known text file** whose exact bytes you record —
  e.g. `HELLO.TXT` containing the 12 bytes `hello world\n` — plus at least one subdirectory (`DOCS/`). §2 also
  creates a kernel-written file from the shell so `stat` can be compared host-stamped vs kernel-written.
- Hub-MSC enumeration is intermittently flaky (`vid=0000`); on a miss the shell comes up honestly with
  "no FAT filesystem" — re-seat + power-cycle.

## 1. Connect the serial console — VERIFY CAPTURE FIRST
```
scripts/jetson-bench-connect.sh          # RPi Debug Probe on the TTL header; tail ~/jetson-serial.log
```
⚠ With JD11 the serial bridge is the **primary output-evidence channel** — confirm it is logging a full boot to
`~/unaos-bench/` BEFORE spending bench time (§JB1f: the round-6/8 host capture froze mid-bench). Screen-on-boot
(JD4) brings the panel to a prompt on its own (~8 s). Type on the USB keyboard.

## 2. `stat` — a host-stamped file, a kernel-written file, a dir, and the root
```
stat HELLO.TXT                   # the host-seeded file: real size + a REAL FAT mtime stamp
stat DOCS                        # a directory
write NOTE.TXT unaos             # kernel-write a fresh file (5 bytes)
stat NOTE.TXT                    # kernel-written: mtime is the JD17 clock, or a bare - if unset this boot
stat /                           # the root — honest: no directory entry of its own
stat NOSUCH.TXT                  # -ENOENT
```
- **PASS:**
  - `stat HELLO.TXT` prints `path: /HELLO.TXT`, `kind: file`, `size: 12 byte(s)`, an `attr:` line like
    `0x20  [ARCHIVE]` (a host tool sets ARCHIVE), a `clus: 0x...` first cluster, a **real** `mtime:` (the date
    the host wrote it), and an `entry: LBA <n> slot +<off>` forensic location (the slot offset is a multiple
    of 32).
  - `stat DOCS` shows `kind: dir`, `size: 0 byte(s)` (FAT directory entries report size 0), `attr:` with the
    `DIR` bit set (e.g. `0x10  [DIR]`), and its own LBA + slot.
  - `stat NOTE.TXT` shows `size: 5 byte(s)`; its `mtime:` is the JD17 wall clock **if `date -s` was run this
    boot**, otherwise a bare `-` (the honest §JD16/§JD17 unset placeholder — the kernel never fabricates a
    stamp). To see a stamped kernel write: `date -s 2026-07-15 14:30:00` then `write NOTE2.TXT x` then
    `stat NOTE2.TXT`.
  - `stat /` prints `kind: dir` and an `entry: root has no directory entry` line (the FAT root has no parent
    slot — no invented LBA).
  - `stat NOSUCH.TXT` prints `stat: /NOSUCH.TXT: not found (-ENOENT)`.
- Case-insensitive: `stat hello.txt` gives the same entry.

## 3. `hexdump` — bounded hexdump at absolute offsets
```
hexdump HELLO.TXT                     # default off=0 len=256 → the whole 12-byte file, one row
hexdump HELLO.TXT 6                   # from offset 6 → "world\n" onward, row labelled 00000006
hexdump HELLO.TXT 0 4                 # first 4 bytes only → a [... 8 more byte(s)] tail note
hexdump HELLO.TXT 0 0x8               # hex length accepted → first 8 bytes
hexdump HELLO.TXT 100                 # offset past EOF → honest "at/past EOF" note, no rows
hexdump DOCS                          # a directory → -EISDIR
hexdump /                             # the root → -EISDIR
```
- **PASS:**
  - `hexdump HELLO.TXT` prints one canonical row: `00000000: 68 65 6c 6c 6f 20 77 6f 72 6c 64 0a  |hello world.|`
    (the `\n` renders as `.` in the ASCII gutter; the short 12-byte row is padded so the gutter aligns). No tail
    note (the whole file fit).
  - `hexdump HELLO.TXT 6` labels its row **00000006** (the ABSOLUTE file offset, not 0) and shows `world\n`.
  - `hexdump HELLO.TXT 0 4` dumps 4 bytes then `[... 8 more byte(s)]` (the honest tail note when the window is
    shorter than the file).
  - `hexdump HELLO.TXT 0 0x8` accepts the hex length and dumps 8 bytes (with a `[... 4 more byte(s)]` note).
  - `hexdump HELLO.TXT 100` prints `hexdump: /HELLO.TXT: offset 100 at/past EOF (12 byte(s)) — nothing to dump`.
  - `hexdump DOCS` and `hexdump /` each print `-EISDIR`.
- **Cap check (optional, needs a >4 KiB file):** `hexdump KERNEL.ELF 0 9999` dumps exactly 4096 bytes (the
  `len ≤ 4096` hard cap) then a `[... N more byte(s)]` note reflecting the real remaining size.

## 4. Durability across a power-cycle
- Note `stat HELLO.TXT`'s `entry:` LBA/slot and the first `hexdump HELLO.TXT` row. **Pull power** (genuine cold cut,
  the JD13–K4 discipline), reboot to the prompt, then:
```
stat HELLO.TXT                   # same size, attr, cluster, LBA/slot — the entry is on-disk
hexdump HELLO.TXT                     # byte-identical first row
```
- **PASS:** `stat` and `hexdump` read the same on-disk bytes after the cut (they touch only the directory entry and
  file data, no cache). The kernel-written `NOTE.TXT` from §2 also survives (`stat NOTE.TXT` still resolves).

## Verdict
Record per section (PASS/FAIL + the observed lines from the serial transcript). The arc's headline claims:
(a) `stat` prints an entry's full on-disk detail — path, kind, size, the raw attr byte + decoded flags, first
cluster, the FAT mtime (real for a host file, the JD17 clock or a bare `-` for a kernel write), and the forensic
dir-entry LBA + slot offset; `stat /` is honest about the root having no entry; (b) `hexdump` hexdumps a bounded
window at the absolute file offset, caps `len` at 4096 with an honest tail note, and refuses a directory;
(c) both survive a real power-cycle. Note the serial log filename (`~/unaos-bench/jetson-serial-<date>.log`).
⚠ `dot_clean` BOTH cards.
