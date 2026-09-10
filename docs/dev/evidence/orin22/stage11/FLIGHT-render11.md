# FLIGHT-render11.md — the operator sequence, top to bottom

STAGE11 (orin 22), bench-side, 2026-09-08. Nothing here has been written to the repo, to a card, or
booted. Read the two lines under each heading before running it.

**What render11 flies:** the generic root — *root is the volume the loader was loaded from, found by
its FAT serial among the disks the kernel enumerates.* No board, bus, slot, card serial or geometry.
Executor BOOTROOT builds it on `exec-orin22-bootroot` (measured this turn: that branch is at
`98213b7f`, **0 commits above `hw-jetson`** — the code is not there yet; this file is the sequence for
when it is).

**⛔ NO CARD OPERATION IS PROPOSED HERE.** No regrow, no reformat, no `esp-jetson-img`, no moving a
card between the reader and the slot. The card in the reader **is** the disk. render10's §3c regrow
recipe and its "which card to regrow" question are struck: they were the microSD direction (baton D1–D5).

---

## §1 — The build

`UNAOS_SDMMCROOT` **no longer exists.** It is render10's sixteenth knob and BOOTROOT deletes the
`sdmmcroot` feature with the `[sdmmc] root …` family it gated. (Measured at `hw-jetson 98213b7f`, i.e.
*before* BOOTROOT: `unaos/arroyo` still maps `UNAOS_SDMMCROOT` → `sdmmcroot,sdmmc,` and
`Cargo.toml:sdmmcroot = ["sdmmc"]` still exists. Confirm both are gone at the build sha before you
type the line; a knob the tree does not know is silently dropped by the shell and flies as a no-op.)

The line is `KNOBS-render10.env`'s set minus that one knob — **fifteen knobs, nothing else changed**:

```bash
cd <bootroot-worktree>/unaos
git rev-parse HEAD; git status --porcelain -- . | wc -l      # must be the flown sha, and 0
env UNAOS_TEGRA=1 UNAOS_TEGRA_EL0=1 UNAOS_WITNESS=1 UNAOS_ORINRENDER=1 UNAOS_DESKCASCADE=1 \
    UNAOS_ORINRX=1 UNAOS_HOLOCRON=1 UNAOS_ORINCLICK=1 UNAOS_TCUPROBE=1 UNAOS_TCURX=1 \
    UNAOS_BSPTICK=1 UNAOS_BSPRUN=1 UNAOS_NET4=1 UNAOS_NET5=1 UNAOS_GA10B_PROBE3=2 \
    ./arroyo esp-jetson 2>&1 | tee ~/unaos-bench/scratch/orin22/build-render11.log
```

Banner gate — set-compare against render10's measured 23-feature `# BANNER-BASELINE:` in
`~/unaos-bench/scratch/orin21/stage10/KNOBS-render10.env`:

```bash
EFF=$(awk -F': ' '/aarch64 effective features/{gsub(/\x1b\[[0-9;]*m/,"",$2); print $2}' \
      ~/unaos-bench/scratch/orin22/build-render11.log | head -1)   # render10 §1b's form, verbatim
diff <(printf '%s' "$EFF" | tr ',' '\n' | sort) \
     <(awk -F'BANNER-BASELINE: ' '/^# BANNER-BASELINE: /{print $2; exit}' \
        ~/unaos-bench/scratch/orin21/stage10/KNOBS-render10.env | tr ',' '\n' | sort)
```

**The ONLY acceptable differences: `sdmmcroot` DISAPPEARS, and `sdmmc` is PRESENT WITHOUT `UNAOS_SDMMC=1`
being set** (seat amendment 15:50Z, rmbp 16's type-level blocker: `BlockSource::TegraSd` exists only under
`sdmmc`, so BOOTROOT v4 makes `sdmmc` default-on in `esp_jetson()` with opt-out `UNAOS_NOSDMMC=1`). Therefore
`#require=sdmmc` in the knob env is VACUOUS from this build on — a default cannot be required. The staging
predicate is: (a) `sdmmc` in the effective-features line, (b) `UNAOS_NOSDMMC` NOT in the environment of the
build (`env | grep -c NOSDMMC` → 0, recorded in the build log), (c) the `[sdmmc]` census witness ≥ 1 by
`strings` on the artifact (§2). Drop the `UNAOS_SDMMC=1 #require=sdmmc` line from `KNOBS-render11.env`; the
first render11 build MEASURES the banner and that measurement becomes the env's new baseline.
Any other MISSING or EXTRA line: stop and find out why before the card. `#forbid=ga10bprobe1,ga10bprobe2`
is unchanged and is a safety gate (probe1's rung ends in an unconditional PSCI SYSTEM_OFF).

## §2 — Arming, on the artifact, before the card

```bash
K=<bootroot-worktree>/unaos/target/aarch64_esp/kernel.elf
strings -a "$K" | grep -c -F -- '[vfs] root = boot volume'   # ARMING  — must be >= 1
strings -a "$K" | grep -c -F -- 'crates/kernel/src/'         # CONTROL — must be > 0 (32/47/53 measured)
strings -a "$K" | grep -c -F -- '[sdmmc] root'               # RETIRED — must be 0
strings -a "$K" | grep -c -F -- '[sdmmc]'                    # CENSUS  — must be >= 1 (sdmmc default-on; the slot driver is IN the plain image)
```

`scorer11.sh` re-runs exactly these three on the file you hand it and refuses (exit 2) if the control
is zero, so this step is a *preview* of the score, not a separate ritual. Arming 0 with the control
firing means the emitter did not reach the build: the whole root family will score NOT-EXERCISED and
the flight answers nothing about root. Retired > 0 means the image predates BOOTROOT.

## §3 — The card write — **PETER'S ACTION, on the host, under sudo**

`load-card10.sh` (`~/unaos-bench/scratch/orin20/cardready/`, 644 lines, named refusals) is unchanged
and correct for this round. In-sandbox it refuses C3/C4/C5/C8 because `/dev/mmcblk0*` is
`nobody:nogroup` there and `root:disk` on the host (pmes ∉ disk) — that is the sandbox, not the card.

```bash
# 1. stage the payload into ~/unaos-bench/flash/orin/render11-<UTC>-<sha7>/ (stage-render9.sh's shape:
#    sha-verified copy + MANIFEST with `# ELF max_vaddr=` and `# effective-features:`)
# 2. DRY RUN first — every read-only check runs, nothing is written:
flatpak-spawn --host sudo ~/unaos-bench/scratch/orin20/cardready/load-card10.sh \
    --src render11-<UTC>-<sha7> --target /dev/mmcblk0 --allow-geom-absent
# 3. same line + --write. It harvests, copies, sha-verifies by READ-BACK, writes <src>.FLIGHTID, unmounts.
```

`--target` is the whole disk so the MBR can be read (a bare `p1` gives C7 nothing to parse).
`--allow-geom-absent` is required and honest: there is no `CARDREADY-GEOM:` line to check against and
**this round does not change the card's geometry** — the refusal is announced, not silently skipped.
`--expect-label` defaults to `UNAOS-ORIN`, which is the label the card in the reader already carries.
After the write, the FLIGHTID's `IMAGE_SHA256` must equal `sha256sum` of the `kernel.elf` you scored
in §2 — that identity is what makes the score a score of the image that flew.

## §4 — Serial

The butler already holds the port: host pid **81753** (`~/unaos-bench/capture/line-acm0/butler.pid`;
`flatpak-spawn --host lsof -t /dev/ttyACM0` returns the same pid — verified this turn). **Do not start
a second one and do not kill it.** Reads are content-routed to `orin.log`:

```bash
LOG=~/unaos-bench/capture/line-acm0/orin.log
# find the boot: the loader's first line is the MARK, the next boot's is the END
LC_ALL=C tr -d '\000' < "$LOG" | LC_ALL=C awk 'index($0,"KELF min=")||index($0,"boot volume FAT serial")||index($0,"[vfs] root"){print NR": "substr($0,1,140)}' | tail -8
LC_ALL=C tr -d '\000' < "$LOG" | LC_ALL=C awk -v m=$MARK -v e=$END 'NR>=m && NR<e' \
    > ~/unaos-bench/scratch/orin22/stage11/boot-render11.log
```

`awk 'index($0,"…")'`, never `grep` and never a bracketed regex — control bytes, and CLAUDE.md's
`awk '/[tag]/'` idiom is a character class that false-matches ~always (baton B7).
**Scope the window to ONE boot.** Two boots in one window make the serial comparison meaningless, and
`scorer11.sh` reds LOADER-SERIAL/ROOT-BIND rather than guessing which boot you meant.

## §5 — Score

```bash
~/unaos-bench/scratch/orin22/stage11/scorer11.sh \
    ~/unaos-bench/scratch/orin22/stage11/boot-render11.log \
    ~/unaos-bench/flash/orin/render11-<UTC>-<sha7>/kernel.elf ; echo "exit=$?"
# the FLASHED kernel.elf, not target/ — the arming field must come from the image that booted.
```

Exits: **0** all PASS · **1** any red · **2** the arming field could not be established (nothing was
scored) · **3** the root family is NOT-EXERCISED and nothing is red (documented extension; a
not-exercised family is neither a pass nor a failure and must not exit 0). Six legs, every one proven
to produce more than one outcome by 21 data mutations: `MUTATIONS.md`.

Run render10's scorers ALONGSIDE for the non-root legs (CRYSTAL, MENUOWN, TICK/PREEMPT, EVQ, TEARSCOPE):
`~/unaos-bench/scratch/orin20/scorer10/scorers-render10.sh <boot-render11.log>` — **and read BULLETIN §31
first**: its GROW-GEOM/GROW-LABEL rows are two known FALSE REDs (a pre-fitsland literal, and a
hardcoded 62333952-sector card), and its whole UNAFS/GROW family keys on the `[sdmmc] root` census that
render11 deletes. Those rows are NOT render11 findings.

## §6 — Stop rules

* Banner delta beyond §1's single expected removal → do not write the card.
* ARMING control zero, or retired literal present in the artifact → do not write the card.
* `scorer11.sh` exit 2 → the score is not a verdict; fix the artifact identity and re-score.
* Any wish to change the card's partitioning, label or contents beyond the payload write → out of
  scope for this flight, and out of scope for this file.

## §6 — FRIEND-DIFF (rmbp 16 B90, Peter 17:10Z: "seeing another UnaOS disk is like seeing a friend — nobody wants
## another to wrap their life around them just because they're acquainted.")
Two boots of ONE image on the same root card: (A) the reader card IN, (B) the reader card OUT (one unplug).
Score: normalize both wires (strip control bytes; blank every timing/counter field — `probe_us=`, `list_us=`,
`us=`, `rx=`, `polls=`, `budget=`, `used=`, `peak=`, `[serialrx]` lines, timestamps), then diff modulo the lines
that are ALLOWED to differ: `[vfs] disk mounted …`, `matches=`, `home=`, `disks=` fields, and `ls /usb`/`/sd`
output if the operator ran it. **The diff must be EMPTY.** Every surviving line is an entanglement with a name and
a line number — the leg exists to find the ones nobody suspected (this round's two: a capture ladder writing
SCREEN<n>.PNG to another disk when the boot volume refuses writes; a non-version-unique window moving root).
Not in scorer11.sh (single-wire scorer); run as `diff <(norm A) <(norm B)` with the normalizer kept beside it
(`friend-norm.sh`, to write with the first pair of wires — the field list above is a starting set, and the FIRST
run will show which timing fields were missed; add them to the normalizer, never to the allowed-diff list).
**§6 CORRECTION (rmbp 16, 17:25Z — the normalizer must not be calibrated on the pair it judges).** Three boots,
not two: (A1) friend ABSENT, (A2) friend ABSENT again, (B) friend PRESENT. Build the normalizer from A1 vs A2 —
**a field may be normalised only if it varies between two boots in the SAME condition** — then FREEZE it and apply
to A1 vs B. A field stable within a condition but different across conditions is not noise; it is the finding.
Positive control before any green is trusted: a boot whose boot volume refuses writes + friend attached makes
`video/prtscr.rs::mount_capture_target` fall to its rung-2 USB handle and write SCREEN<n>.PNG to the friend — the
wire says so; FRIEND-DIFF must RED on it (or on a one-line "read this only when the friend is mounted" mutation).
**§6 CONTROL (rmbp 16, 17:55Z): option 1 — a DURABLE control.** The prtscr rung-2 capture is a live defect rmbp is
fixing (B89/B74/Q6); a control that expires when a defect is fixed leaves the leg green with no calibration. The
control is therefore a deliberate one-line mutation, applied and reverted per run: a read that happens ONLY when the
friend is mounted (e.g. stat a file under the first indexed bus mount point and print one witness line). FRIEND-DIFF
must RED on the mutated build and GREEN on the reverted one, every flight. The prtscr case may be used as a second,
non-durable control while it still exists; it is never the only one.
