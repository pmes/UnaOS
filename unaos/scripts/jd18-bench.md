# JD18 bench card — read-only tree tools: `find` + `du` + `uptime` (attended)

JD18 adds three read-only surveying tools to the panel shell, built entirely from primitives the file-manager
verb set already ships (the JD9/JD13 `read_dir` SNAPSHOT walk, the JD12 `glob_match`, and the JD17 clock's
additive `uptime_secs()` helper) — **zero mutation, no new `fat.rs` surface.** `find <root> <pattern>` searches
the tree by wildcard; `du <dir>` tallies subtree sizes; `uptime` reports seconds since boot. This card confirms
each on silicon, including durability across a power-cycle. Thanks to JD11 it leaves a durable serial
transcript. Detail + rationale: [`arch_arm64.md` §JD18](../../docs/dev/OS/01_BOOT_HAL/arch_arm64.md). Pairs
cleanly with the JD15–JD17 benches in one attended session.

## 0. Prep the media (Peter flashes — session cannot write `/Volumes`)
- **Kernel:** rebuild the tegra ESP LAST (any `test-arm` clobbers it) and validate by COUNT:
  `UNAOS_TEGRA=1 ./arroyo esp-jetson` → `strings target/aarch64_esp/kernel.elf | grep -c 'tegra:'`
  must be **UNCHANGED from JD17** (JD18 adds no `tegra:` token). Copy `EFI` + `kernel.elf` to the boot stick,
  `dot_clean`, eject. Validate by count, not size.
- **Data card:** a **separate** FAT16 card (the tegra pattern — the boot stick is NOT the block device),
  present AT BOOT, in the reader behind the hub. **⚠ `dot_clean` the DATA card too** and strip `._*` AppleDouble
  sidecars — they are glob-visible on FAT 8.3 short names and would inflate `find`/`du` counts (a dirty fixture,
  not a bug — this bit JD12). No specific host file is required; §2 seeds its own nested tree from the shell.
- Hub-MSC enumeration is intermittently flaky (`vid=0000`); on a miss the shell comes up honestly with
  "no FAT filesystem" — re-seat + power-cycle.

## 1. Connect the serial console — VERIFY CAPTURE FIRST
```
scripts/jetson-bench-connect.sh          # RPi Debug Probe on the TTL header; tail ~/jetson-serial.log
```
⚠ With JD11 the serial bridge is the **primary output-evidence channel** — confirm it is logging a full boot to
`~/unaos-bench/` BEFORE spending bench time (§JB1f: the round-6/8 host capture froze mid-bench). Screen-on-boot
(JD4) brings the panel to a prompt on its own (~8 s). Type on the USB keyboard.

## 2. Seed a small NESTED tree with known sizes
```
mkdir TREE
mkdir TREE/SUB
write TREE/A.TXT hello           # 5 bytes ("hello")
write TREE/B.LOG world!          # 6 bytes ("world!")
write TREE/SUB/C.TXT abc         # 3 bytes ("abc")
ls -l TREE                       # confirm A.TXT=5, B.LOG=6, SUB=<DIR>
ls -l TREE/SUB                   # confirm C.TXT=3
```
- Note the exact byte sizes `ls -l` reports (the shell `write` verb writes the whitespace-collapsed remainder of
  the line, so a single word = its literal length). Use those numbers to check `du` in §4.

## 3. `find` — recursive glob search
```
find TREE *.TXT                  # A.TXT and SUB/C.TXT match; B.LOG does not
find TREE C*                     # only SUB/C.TXT
find TREE *                      # every entry under TREE (files + dirs, dirs trailing /)
find *.TXT                       # one arg: root defaults to '.', searches the whole card
find TREE NOSUCH                 # a literal that matches nothing
find NOSUCH *.TXT                # missing root
find TREE/A.TXT *.TXT            # a FILE root: the POSIX self-match test
```
- **PASS:** `find TREE *.TXT` prints `/TREE/A.TXT` and `/TREE/SUB/C.TXT` (canonical paths), then
  `2 match(es), 2 dir(s) scanned` (TREE + TREE/SUB). `find TREE *` prints every entry, directories with a
  trailing `/`. `find *.TXT` (one arg) searches from the card root. `find TREE NOSUCH` prints
  `0 match(es), 2 dir(s) scanned`. `find NOSUCH *.TXT` prints an `-ENOENT`. `find TREE/A.TXT *.TXT` prints
  `/TREE/A.TXT` then `1 match(es), 0 dir(s) scanned` (a file root is a single self-match test). Matching is
  case-insensitive — `find TREE *.txt` gives the same hits.

## 4. `du` — subtree size tally
```
du TREE                          # per direct child, then the total
du TREE/SUB                      # just C.TXT
du TREE/A.TXT                    # a single FILE
du                               # the cwd (whole card root)
```
- **PASS:** `du TREE` prints one line per direct child — `A.TXT` (5), `B.LOG` (6), and `SUB/` as the recursive
  sum of its subtree (3, from C.TXT) — then `total: 14 byte(s) in 3 file(s), 1 dir(s)` (3 files A/B/C across the
  subtree, and 1 directory SUB; the `TREE` argument itself is not counted, only its descendants). The `SUB/`
  line shows **3**, not 0: directory ENTRIES report size 0 on FAT, so a directory's reported size is the
  recursive sum of its files.
  `du TREE/A.TXT` prints a single `5  /TREE/A.TXT` line then `total: 5 byte(s) in 1 file(s), 0 dir(s)`. The
  numbers match the `ls -l` sizes noted in §2.

## 5. `uptime` — seconds since boot, monotonic, plus the wall clock when set
```
uptime                           # up 00:0x:xx  (no clock set yet)
setdate 2026-07-15 14:30:00      # seed the JD17 wall clock
uptime                           # up 00:0x:xx (clock: 2026-07-15 14:3x:xx)
uptime                           # a few seconds later — the up-time has advanced
```
- **PASS:** the first `uptime` prints `up HH:MM:SS` with no clock suffix; after `setdate`, `uptime` appends
  `(clock: 2026-07-15 14:3x:xx)`; a later `uptime` shows a **larger** elapsed time (the arch counter is
  free-running and monotonic). A ±1 s granularity is expected (whole-second counter division), not a bug.

## 6. Durability across a power-cycle
- Note `du TREE`'s total and a couple of `find TREE *.TXT` hits. **Pull power** (genuine cold cut, the JD13–K4
  discipline), reboot to the prompt, then:
```
find TREE *.TXT                  # same hits — the tree is on-disk, durable
du TREE                          # same total: 14 byte(s) ...
uptime                           # small again (a fresh boot; the counter reset)
```
- **PASS:** the seeded tree, its `find` hits, and its `du` total are identical across the power-cycle (they read
  the on-disk directory entries). `uptime` is small again — the counter resets at boot, exactly as designed.

## Verdict
Record per section (PASS/FAIL + the observed lines from the serial transcript). The arc's headline claims:
(a) `find` matches by wildcard across the tree, prints canonical paths + an honest scanned tally, and degrades a
file root to a self-match test; (b) `du` tallies subtree sizes (directory entries = 0 bytes, a dir's size = the
recursive file sum) matching the `ls -l` sizes; (c) `uptime` reports monotonic seconds since boot and appends
the wall clock when set; (d) the tree, its `find` hits and `du` totals survive a real power-cycle while `uptime`
resets. Note the serial log filename (`~/unaos-bench/jetson-serial-<date>.log`). ⚠ `dot_clean` BOTH cards.
