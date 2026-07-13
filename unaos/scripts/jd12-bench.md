# JD12 bench card — paging (`head`/`tail`) + wildcard globbing on the Orin panel (attended)

JD12 adds two conveniences over the closed file-manager verb set, both `shell.rs`-only (no `fat.rs` change):
`head <path> [n]` / `tail <path> [n]` (first / last n lines, default 10) and `*`/`?` wildcard expansion for
`ls`/`cat`/`rm`/`cp`/`mv`. This card confirms both on silicon and — thanks to JD11 — leaves a durable serial
transcript proving exactly which files each verb touched. Detail + rationale:
[`arch_arm64.md` §JD12](../../docs/dev/OS/01_BOOT_HAL/arch_arm64.md). Mechanics reused from JD2–JD11.

## 0. Prep the media (Peter flashes — session cannot write `/Volumes`)
- **Kernel:** rebuild the tegra ESP LAST (any `test-arm` clobbers it) and validate by COUNT:
  `UNAOS_TEGRA=1 ./arroyo esp-jetson` → `strings target/aarch64_esp/kernel.elf | grep -c 'tegra:'`
  must be **109** (UNCHANGED from JD11 — JD12 adds no `tegra:` token; virt clobber ≈ 0/1). Copy `EFI` +
  `kernel.elf` to the boot stick, `dot_clean`, eject. (The ELF is ~725 KB — grown from the base's SOCK-3 /
  UNAFS-K3 merges, not JD12; validate by count, not size.)
- **Data card:** a **separate** FAT16 card (the tegra pattern — the boot stick is NOT the block device),
  present AT BOOT, in the reader behind the hub. The pi4 fixture `UNAOSRW` card is fine; the panel itself
  creates the bench files below (no pre-seeded layout needed).
- Hub-MSC enumeration is intermittently flaky (`vid=0000`); on a miss the shell comes up honestly with
  "no FAT filesystem" — re-seat + power-cycle.

## 1. Connect the serial console — VERIFY CAPTURE FIRST
```
scripts/jetson-bench-connect.sh          # RPi Debug Probe on the TTL header; tail ~/jetson-serial.log
```
⚠ With JD11 the serial bridge is the **primary output-evidence channel** — confirm it is logging a full boot
to `~/unaos-bench/` BEFORE spending bench time (§JB1f: the round-6/8 host capture froze mid-bench). Screen-on-
boot (JD4) brings the panel to a prompt on its own (~8 s). Type on the USB keyboard.

## 2. Seed a small tree of same-extension files (on the panel)
```
write A.TXT alpha one
append A.TXT alpha two
write B.TXT bravo
write C.LOG charlie log
mkdir DOCS
mkdir ARCHIVE
```
Every line echoes to serial as `:: tegra: JD2 — OUT | … ::` (JD11), so the whole session is replayable.

## 3. Paging — `head` / `tail`
```
head A.TXT                    # first 10 lines (here: both lines of A.TXT) -> OUT lines on serial
tail A.TXT 1                  # last 1 line -> "alpha two"
head NOSUCH                   # -> "head: /NOSUCH: not found (-ENOENT)"  (errno on serial too)
head DOCS                     # -> "head: /DOCS: is a directory (-EISDIR)"
```
Confirm the paged text appears verbatim on the serial capture, and that head/tail never hang (a stalled read
is `-EIO`, not a wedge).

## 4. Wildcard globbing — read verbs first (non-destructive)
```
ls *.TXT                      # lists A.TXT + B.TXT (sorted), with the file/dir tally
cat *.TXT                     # cats A.TXT then B.TXT in order, each rendered as in a single cat
ls *.XYZ                      # -> "ls: *.XYZ: no match"   (honest, no wedge)
```

## 5. Wildcard globbing — copy / move / remove
```
cp *.TXT DOCS/                # copies A.TXT + B.TXT into DOCS/ ; source files untouched
ls DOCS                       # -> A.TXT, B.TXT present under DOCS
cp *.TXT B.TXT                # -> "cp: target B.TXT: not a directory (-ENOTDIR)"  (>1 source, non-dir dst)
mv *.LOG ARCHIVE/             # moves C.LOG into ARCHIVE/ (O(1) relink) ; ls / (root) shows C.LOG gone
cat ARCHIVE/C.LOG             # -> "charlie log"  (content survived the move)
rm *.TXT                      # removes A.TXT + B.TXT from the root ; DOCS/*.TXT copies remain
ls                            # root: A.TXT/B.TXT gone, DOCS + ARCHIVE remain
```
Then POWER-CYCLE and confirm durability:
```
cat DOCS/A.TXT               # -> "alpha one" / "alpha two"  (the glob-copied file survived the cycle)
cat ARCHIVE/C.LOG            # -> "charlie log"               (the glob-moved file survived)
```

## Pass criteria
`head`/`tail` show exactly the requested first/last lines of a file (verbatim on serial), refuse a directory
with `-EISDIR`, and never hang. Wildcards expand only the intended files: `ls *.TXT`/`cat *.TXT` list/print
the matching set in sorted order; `cp *.TXT DOCS/` and `mv *.LOG ARCHIVE/` land each match under the target
directory (sources untouched by `cp`, moved by `mv`); `rm *.TXT` removes exactly the matches; a no-match
pattern reports `no match`; a multi-source copy/move onto a non-directory is `-ENOTDIR`. The glob-copied /
glob-moved files survive a power cycle (write-through durability). No wedge, no wrong file touched, no dropped
lines. The serial transcript (`awk '/:: tegra: JD2 —/' ~/unaos-bench/*.log`) is the durable evidence — it
should read as a clean interleave of the typed keys and each verb's output.
