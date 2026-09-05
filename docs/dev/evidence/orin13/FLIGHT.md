# FLIGHT — orin 13 · render2 pre-flight (FLIGHTPREP)

Prepared 2026-09-03T21:29Z against `hw-jetson@ac27b8d2` (`git log --oneline -1` → `ac27b8d2 scripts: NOTMP …`),
working tree clean. Read-only: nothing in the repo was edited, built, or staged by this document.
Evidence for every claim is given as `file:line` read, or as the command run and its output.

Paths used throughout:

| name | path |
|---|---|
| repo | `/home/pmes/src/github.com/pmes/UnaOS-orin` |
| kernel sched | `unaos/crates/kernel/src/arch/aarch64/sched.rs` |
| kernel syscall | `unaos/crates/kernel/src/arch/aarch64/syscall.rs` |
| kernel main | `unaos/crates/kernel/src/main.rs` |
| render1 media | `~/unaos-bench/flash/orin/render1-20260901T0347Z-c61b47e/` |
| render1 build log | `~/unaos-bench/scratch/orin12/build-render1.log` |
| card loader | `~/unaos-bench/scratch/orin11/load-card.sh` |
| capture | `~/unaos-bench/capture/line-acm0/orin.log` (append-only, many boots) |
| unwrapper | `~/unaos-bench/tools/unwrap80.sh` |

---

## A. THE WITNESS GATE — what gates the `[u7stk]` probe's CALL on the tegra path

### A.1 The probe itself is `witness`-gated

`sched.rs:79-90`:

```rust
#[cfg(feature = "witness")]
pub const STACK_POISON: u8 = 0xAB;
…
#[cfg(feature = "witness")]
pub fn stk_probe(at: &str) {
```

and the line it prints is `sched.rs:134`:
`"[u7stk] at={} task={}:{} sp={:#x} low={:#x} top={:#x} len={} used={} hw={} headroom={}"`.
The doc comment at `sched.rs:71-76` states the intent: "Both are `witness`-gated, so the plain
`./arroyo kernel8` media build carries neither".

The function returns early with no output when the core has no current task (`sched.rs:96-99`,
`if raw.is_null() { return; }`), which is the "no-op on the terminus line" case `main.rs:7284-7286`
and `main.rs:7816-7818` describe.

### A.2 There is exactly ONE call site in the whole kernel, and it is not reachable on tegra

```
$ grep -rn 'stk_probe' unaos/crates/kernel/src --include=*.rs     # non-comment hits only
unaos/crates/kernel/src/arch/aarch64/syscall.rs:16225:        crate::arch::sched::stk_probe($at)
```

That call is the body of the `u7stk!` macro, `syscall.rs:16222-16233`, gated on BOTH arms:

```rust
#[cfg(feature = "witness")]
macro_rules! u7stk { ($at:expr) => { crate::arch::sched::stk_probe($at) }; }
#[cfg(not(feature = "witness"))]
macro_rules! u7stk { ($at:expr) => {{ let _ = $at; }}; }
```

Every expansion of `u7stk!` (`syscall.rs:16236-16517`, 60 of them, `at=entry` … `at=after:bandy_rt`)
sits inside ONE function, `pub fn u7_launcher(demo_cpu: usize)` (`syscall.rs:16235`). `u7_launcher` has
exactly one spawn site, `main.rs:757-763`:

```rust
unaos_kernel::arch::sched::spawn_stack("u7-launch", unaos_kernel::arch::syscall::u7_launcher, cpu, vcpu, U7_LAUNCH_STACK_SIZE);
```

and that site is in the `kernel_main` body BELOW `main.rs:189-190`:

```rust
#[cfg(all(feature = "tegra", target_arch = "aarch64"))]
tegra_early_stop(boot_info);          // fn tegra_early_stop(..) -> !   (main.rs:2029)
```

`tegra_early_stop` is `-> !` (`main.rs:2029`); its own comment at `main.rs:186-187` says "everything
below is unreachable on tegra". So on a jetson image the gate on the probe's CALL is, exactly:

> **`#[cfg(feature = "witness")]` on the `u7stk!` macro (`syscall.rs:16222`) AND the call must be inside
> `u7_launcher`, which is spawned only from code that `tegra_early_stop -> !` (`main.rs:190`) makes
> unreachable.** On a tegra image the probe is dead code regardless of `witness`.

The prior seat's belief ("no `witness` ⇒ no `[u7stk]` line") is CONFIRMED as far as it goes, and
REFUTED as an explanation for the Orin: `witness` was ON in render1 (banner line 1 of the build log,
MANIFEST `effective-features: witness,…`) and the image still carries no probe at all.

**In-artifact proof (grep -a on the flown ELF, with a positive control):**

```
$ grep -a -o 'u7stk' ~/unaos-bench/flash/orin/render1-20260901T0347Z-c61b47e/kernel.elf | wc -l
0
$ grep -a -o 'RENDER-LIVE' ~/unaos-bench/flash/orin/render1-20260901T0347Z-c61b47e/kernel.elf | wc -l
1                                   # the grep can hit; the token is simply absent
```

The unreachable `u7_launcher` (and every `"[u7stk] at="` format piece with it) is discarded at link
time — the string does not exist in the render1 kernel. The probe DOES fire where `u7_launcher` runs:
`~/unaos-bench/scratch/orin11/serial-pi-green-final.log` (Pi, witness) has 52 `[u7stk]` lines, e.g.
`[u7stk] at=entry task=69:u7-launch … len=32768 used=272 hw=272 headroom=32496`.

### A.3 Consequence for render2 at ac27b8d2

`orin_render_service` (`main.rs:8182-8283`) contains no `stk_probe` call. The comment at
`sched.rs:10308` that mentions a "`[u7stk] at=render:pass` probe folded onto its `[sched6]` cadence" is
comment-only — `grep -rn 'render:pass'` hits only `sched.rs:10257` and `:10308`, both `//` lines.
**A `[u7stk] … task=N:orin-render` line CANNOT appear on a render2 built from ac27b8d2 as it stands.**
Producing one requires a code change outside this document's scope: a
`#[cfg(feature = "witness")] unaos_kernel::arch::sched::stk_probe("render:pass");` inside the pass
(the caller MUST carry its own `#[cfg(feature = "witness")]` because `stk_probe` does not exist on a
knob-off build), ideally on the census cadence so it is one line per census rather than one per pass.

### A.4 `spawn_stack` vs `spawn` — identical for the probe

Both are thin wrappers over the same `spawn_inner` (`sched.rs:3710-3712` and `:3721-3729`):

```rust
pub fn spawn(name, entry, arg, cpu) -> u64        { spawn_inner(name, entry, arg, cpu, PRIO_NORMAL, None, TASK_STACK_SIZE) }
pub fn spawn_stack(name, entry, arg, cpu, stack_bytes) -> u64 { spawn_inner(name, entry, arg, cpu, PRIO_NORMAL, None, stack_bytes) }
```

`spawn_inner` paints the stack under the same gate (`sched.rs:3636-3639`,
`#[cfg(feature = "witness")] stack.fill(STACK_POISON);`) before `build_initial_frame`. The probe reads
the CURRENT task's `stack` bounds (`sched.rs:104-107`) and never asks how it was spawned. So
`[u7stk]` fires the same way for either — the only difference is `len=` (16384 for `spawn`, which is
`TASK_STACK_SIZE = 16 * 1024` at `sched.rs:41`, versus the caller's size for `spawn_stack`).

`orin-render` is spawned with plain `spawn` (`main.rs:8166`), i.e. 16 KiB.

### A.5 A stack finding the flight already made (no probe needed)

The render1 boot printed the dispatch-time low-redzone report (`sched.rs:5482`, NOT witness-gated,
capped at 16 reports by `GUARD_LO_REPORTS < 16`, `sched.rs:41`):

```
[redzone] cpu=0 LOW-REDZONE TRAVERSED task=4:orin-render — this task's OWN SP crossed its usable floor into the 1024 B absorber below it; the absorber is EXHAUSTED … grow this task's stack NOW
[redzone] cpu=0 LOW-REDZONE TRAVERSED task=1:jd2-console — …
```

8 lines for each task, 16 total — the cap was hit. So on render1 `orin-render`'s 16 KiB stack was
already TRAVERSED (hw ≥ len). Any `[u7stk]` added for render2 should be expected to read
`hw=16384 headroom=0` (saturated lower bound, per `sched.rs:85-86`) unless the pass is moved to
`spawn_stack` with a larger size. Watch for the `[redzone]` lines on render2 regardless; they are in
the image by construction.

---

## B. THE RECIPE — build, stage, load `render2-<UTC>-<sha>`

### B.1 What produced render1 (extracted, not inferred)

Build log `~/unaos-bench/scratch/orin12/build-render1.log`, lines 1-2 and 35:

```
⚡ kernel features: witness,ehcihid,kbdwit,sdhcblk,smolnet,tegra,orindesk,orinclick,tegra_el0,tegrasmp,orinconwin,desktop_firmware,orinrender
⚡ kernel features (jetson): witness,ehcihid,kbdwit,sdhcblk,smolnet,tegra,orindesk,orinclick,tegra_el0,tegrasmp,orinconwin,desktop_firmware,orinrender
⚡ aarch64 effective features: witness,ehcihid,tegra,orindesk,orinclick,tegra_el0,tegrasmp,orinconwin,desktop_firmware,orinrender
```

Line 324-326: `SRC.TGZ sha256: 6e2a44b7…` then `✅ Jetson ESP (tegra): …/unaos/target/aarch64_esp`.
The log does not record the invoking command line; the knob line is recorded in the staged
MANIFEST header (`flash/orin/render1-…/MANIFEST` line 2):

```
# KNOBS: UNAOS_TEGRA=1 UNAOS_TEGRA_EL0=1 UNAOS_WITNESS=1 UNAOS_ORINDESK=1 UNAOS_ORINCLICK=1 UNAOS_ORINCONWIN=1 UNAOS_ORINRENDER=1
```

Consistency check of knobs → banner against `unaos/arroyo`: `UNAOS_WITNESS` → `witness,` (`arroyo:153`);
`UNAOS_TEGRA` → `tegra,` (`:758`) and `tegrasmp,tegra,` (`:964`, unless `UNAOS_NOTEGRASMP`);
`UNAOS_TEGRA_EL0` → `tegra_el0,tegra,` (`:954`); `UNAOS_ORINDESK` → `orindesk,` (`:830`);
`UNAOS_ORINCLICK` → `orinclick,` (`:909`); `UNAOS_ORINCONWIN` → `orinconwin,desktop_firmware,tegra_el0,tegra,`
(`:1086`); `UNAOS_ORINRENDER` → `orinrender,desktop_firmware,tegra_el0,tegra,` (`:1261`).
`esp_jetson()` (`arroyo:4515-4528`) forces `tegra` and `tegrasmp` itself and prints the `(jetson)` banner.
`ehcihid,kbdwit,sdhcblk,smolnet` are defaults stripped from aarch64 by `arm_features`. **`witness` is
OFF for `esp-jetson` unless `UNAOS_WITNESS=1` is in the environment** (`arroyo:30-37,45`: only the
battery targets self-arm it). The target is `esp-jetson` (there is no `esp-arm` in this recipe; `esp-arm`
is the QEMU-virt ESP).

The output tree is `unaos/target/aarch64_esp/` (`arroyo:4550`); staging is a plain copy of that tree
into `~/unaos-bench/flash/orin/<name>/` plus a per-dir MANIFEST (the pattern is
`~/unaos-bench/scratch/orin11/stage-two-images.sh:18-38`, `stage()`), which is exactly the layout
render1 has:

```
$ ls ~/unaos-bench/flash/orin/render1-20260901T0347Z-c61b47e/
EFI/BOOT/BOOTAA64.EFI  ELFHELLO.ELF  HELLO.BIN  MANIFEST  PULSE.ELF  SRC.SHA  SRC.TGZ  STAT.ELF  VUG.ELF  VUGK.ELF  kernel.elf
```

(`target/aarch64_esp/kernel.elf` still IS render1's: both sha256 `7160225e163d8242…`.)

Two things about render1's staging to carry or correct:

* `flash/orin/MANIFEST` (the global ledger) has NO render1 line (`grep -c render1 flash/orin/MANIFEST` → 0).
  `validate-manifest.py` still PASSes because the per-dir MANIFEST counts as NESTED. The recipe below
  appends the ledger line `stage-two-images.sh:31-35` would have written, so render2 is recorded in both.
* The name format is `<tag>-$(date -u +%Y%m%dT%H%MZ)-$(git rev-parse --short=7 HEAD)` (`stage-two-images.sh:13,44`).

### B.2 The MANIFEST format rule (from `load-card.sh:52-58`)

```sh
while read -r want path; do
    case "$want" in \#*|"") continue;; esac
    got=$(flatpak-spawn --host sha256sum "$MP/$path" 2>/dev/null | cut -d' ' -f1)
```

So the per-dir MANIFEST may contain ONLY (a) lines starting with `#`, (b) blank lines, and
(c) `<sha256>  <path-relative-to-the-staged-dir>` lines (`sha256sum` output format, path first-column
after the sha). Any other shape (an `IMAGE …` ledger line, a bare journal sentence) is parsed as a sha
and FAILS the on-card match, which refuses the load. Paths must be the on-card paths
(`EFI/BOOT/BOOTAA64.EFI`, `kernel.elf`, …), never absolute.

`load-card.sh` also requires `kernel.elf` and `MANIFEST` present (`:14-15`), finds the card by LABEL
`UNAOS-ORIN` via `flatpak-spawn --host lsblk` (`:8,18-22`), harvests the previous card contents to
`~/unaos-bench/scratch/orin11/harvest-<UTC>/` (`:42-45`), copies, then sha-matches every MANIFEST line
on-card (`:50-58`) and unmounts only on 10/10.

### B.3 Commands to paste (build → stage → load). NOT run by this document.

Set the sha once. `SHA` is the commit to fly; use `ac27b8d2` for the tree as it stands, or the tip
after the render-pass probe lands (see A.3).

```sh
# ---- 0. environment ----------------------------------------------------------
REPO=/home/pmes/src/github.com/pmes/UnaOS-orin
UNA=$REPO/unaos
FLASH=$HOME/unaos-bench/flash/orin
SCR=$HOME/unaos-bench/scratch/orin13
SHA=ac27b8d2                          # <-- the commit to fly
cd "$UNA"
git status --porcelain | wc -l        # must print 0 (a dirty tree is not the sha it names)
git rev-parse --short=8 HEAD          # must print $SHA
GIT7=$(git rev-parse --short=7 HEAD); GITFULL=$(git rev-parse HEAD)

# ---- 1. build: render1's knob line, verbatim (UNAOS_WITNESS=1 included) ----------
UNAOS_TEGRA=1 UNAOS_TEGRA_EL0=1 UNAOS_WITNESS=1 UNAOS_ORINDESK=1 UNAOS_ORINCLICK=1 \
UNAOS_ORINCONWIN=1 UNAOS_ORINRENDER=1 \
  ./arroyo esp-jetson 2>&1 | tee "$SCR/build-render2.log"
echo "ESP_JETSON_EXIT=${PIPESTATUS[0]}"   # must be 0

# banner gate — all three lines must carry `witness` and `orinrender`; effective must equal render1's
awk '/kernel features|effective features/' "$SCR/build-render2.log"
#   expected effective: witness,ehcihid,tegra,orindesk,orinclick,tegra_el0,tegrasmp,orinconwin,desktop_firmware,orinrender

# in-artifact gate — the strings the flight scores must exist in THESE bytes (grep -a, never `strings`)
for t in 'console-already-windowed' 'RENDER-LIVE' 'RENDER-ARMED' '[orinrender] arm' 'u7stk'; do
  printf '%-28s %s\n' "$t" "$(grep -a -o -F "$t" target/aarch64_esp/kernel.elf | wc -l)"
done
#   console-already-windowed >=1 (the a1cf4900 guard is aboard); RENDER-LIVE/ARMED/arm >=1;
#   u7stk: 0 at ac27b8d2 (see A.3) — >=1 only if the render-pass probe landed in $SHA
grep -a -o -F 'KELF min=' target/aarch64_esp/EFI/BOOT/BOOTAA64.EFI | wc -l    # 1 (the boot anchor)

# ---- 2. stage into flash/orin/<name>/ with a per-dir MANIFEST -----------------------
NAME="render2-$(date -u +%Y%m%dT%H%MZ)-$GIT7"
DIR="$FLASH/$NAME"
[ -s target/aarch64_esp/kernel.elf ] && mkdir -p "$DIR" && cp -R target/aarch64_esp/. "$DIR/"
EFF=$(awk -F': ' '/aarch64 effective features/{gsub(/\x1b\[[0-9;]*m/,"",$2); print $2}' "$SCR/build-render2.log")
{
  echo "# $NAME — ORINRENDER-FIX flight: render pass with the a1cf4900 guard + strip; witness aboard"
  echo "# KNOBS: UNAOS_TEGRA=1 UNAOS_TEGRA_EL0=1 UNAOS_WITNESS=1 UNAOS_ORINDESK=1 UNAOS_ORINCLICK=1 UNAOS_ORINCONWIN=1 UNAOS_ORINRENDER=1"
  echo "# effective-features: $EFF"
  echo "# THE QUESTIONS: (1) [orinrender] DECLINE reason=console-already-windowed, never -> SHELL-WINDOW, beside [orinconwin] -> ROUTED;"
  echo "#   (2) presents= climbs past 1 across census lines; (3) census line rate per wall second; (4) [u7stk] task=orin-render headroom (only if the probe is in $SHA);"
  echo "#   (5) one byte injected at the board over the butler FIFO — expect nothing (RX not wired). See $SCR/FLIGHT.md §C."
  echo "# built-from=hw-jetson@$GITFULL  esp-jetson EXIT=0  by=orin13"
  echo "# UNFLOWN — QEMU models no Tegra234; this boot is the only verdict that exists."
} > "$DIR/MANIFEST"
( cd "$DIR" && find . -type f ! -name MANIFEST -printf '%P\n' | sort | xargs sha256sum >> MANIFEST )
# format self-check: every non-# line must be `<64hex>  <relative path>` (load-card.sh parses nothing else)
awk '!/^#/ && !/^$/ && $1 !~ /^[0-9a-f]{64}$/ {bad++; print "BAD MANIFEST LINE: " $0} END{print "manifest_bad_lines=" bad+0}' "$DIR/MANIFEST"
ls -la "$DIR"; wc -l "$DIR/MANIFEST"       # 10 files + 8 header lines expected

# global ledger append (the line render1 never got — stage-two-images.sh:31-35 form)
{
  echo "# $NAME — ORINRENDER-FIX flight  KNOBS: UNAOS_TEGRA=1 UNAOS_TEGRA_EL0=1 UNAOS_WITNESS=1 UNAOS_ORINDESK=1 UNAOS_ORINCLICK=1 UNAOS_ORINCONWIN=1 UNAOS_ORINRENDER=1  built-from=hw-jetson@$GITFULL  effective=$EFF  esp-jetson EXIT=0  by=orin13"
  ( cd "$FLASH" && find "$NAME" -type f ! -name MANIFEST -printf '%p\n' | sort | xargs sha256sum )
} >> "$FLASH/MANIFEST"
python3 "$HOME/unaos-bench/tools/validate-manifest.py" "$FLASH/MANIFEST" --quiet   # expect PASS, 13 staged images
echo "$NAME" > "$SCR/NAME_RENDER2"

# ---- 3. load the card (dry-run first, then --write). Needs the UNAOS-ORIN card in the reader. --------
"$HOME/unaos-bench/scratch/orin11/load-card.sh" "$NAME"            # dry run: prints CARD: /dev/… and SRC:
"$HOME/unaos-bench/scratch/orin11/load-card.sh" "$NAME" --write    # copies, sha-matches 10/10, unmounts
# expected tail:  "=== 10/10 match. Card carries render2-…. ===" then "UNMOUNTED /dev/… — safe to pull."
# then record the write in the ledger, in the machine-checkable form (flash/orin/MANIFEST:554 is the precedent):
KSHA=$(sha256sum "$DIR/kernel.elf" | cut -c1-16)
echo "$NAME WRITTEN TO CARD (UNAOS-ORIN) $(date -u +%Y-%m-%dT%H:%MZ) by orin 13, hand load — sha-verified $KSHA, all 10 files matched against the staged MANIFEST, unmounted clean." >> "$FLASH/MANIFEST"
```

Before the boot: the serial line must be HELD, or the boot is lost (the 2026-08-18 lesson in
`tools/line-butler.py:4-7`). At the time of writing no butler is running (`pgrep -af line-butler` →
nothing; `capture/line-acm0/butler-start.out` ends `cannot open /dev/ttyACM0`; the device exists now,
`crw-rw---- root dialout /dev/ttyACM0`). Start it the way `tools/pi-butler-watch.sh:31` does:

```sh
nohup flatpak-spawn --host python3 "$HOME/unaos-bench/tools/line-butler.py" /dev/ttyACM0 "$HOME/unaos-bench/capture/line-acm0" \
   > "$HOME/unaos-bench/capture/line-acm0/butler-start.out" 2>&1 &
sleep 2; tail -1 "$HOME/unaos-bench/capture/line-acm0/butler-witness.log"   # "=== line-butler holds /dev/ttyACM0 @ …"
"$HOME/unaos-bench/tools/bench-state.sh" | awk '/line-butler/'                 # OK  line-butler ALIVE pid N
```

---

## C. THE SCORING — pinning the last boot and scoring render2

### C.1 Why unwrap, and how a boot is pinned

`orin.log` is one append-only stream across every Orin boot the butler has routed (46,160 lines,
3.4 MB; the butler stamps only its own start, `line-butler.py:246-248`, never per line). Read it
through `unwrap80.sh` always — `unwrap80.sh:6-16` explains the 80-column UEFI hard wrap; the KELF
line itself is under 80 (`:19-22`) but every other loader line is not.

The boot anchor is the loader identity line, `crates/bootloader/src/main.rs:792`:
`log::info!("KELF min={:#x} max={:#x} pg={}", …)`. Captures before orin 12 carry the old wording
`Kernel ELF: min_vaddr=…, max_vaddr=…, pages=…` (`main.rs@743`), so the anchor regex takes both. The
LAST anchor in the log starts the last boot. Verified on the current log:

```
$ ~/unaos-bench/tools/unwrap80.sh orin.log | awk '/KELF min=|Kernel ELF: min_vaddr=/{n=NR} END{print n}'
41343
$ … | awk 'NR==41343' → [ INFO]: crates/bootloader/src/main.rs@792: KELF min=0x0 max=0x2da2a8 pg=731
$ readelf -l -W flash/orin/render1-…/kernel.elf | awk '/LOAD/{v=strtonum($3)+strtonum($6); if(v>m)m=v} END{printf "%#x\n",m}'
0x2da2a8                                   # the last boot in the log IS render1's bytes
```

Score every boot by the LOADED image's identity first (the `max=` must equal render2's ELF), not by
what was staged.

### C.2 Isolate the last boot

```sh
LOG=$HOME/unaos-bench/capture/line-acm0/orin.log
B=$HOME/unaos-bench/scratch/orin13/render2-boot.txt
n=$($HOME/unaos-bench/tools/unwrap80.sh "$LOG" | awk '/KELF min=|Kernel ELF: min_vaddr=/{n=NR} END{print n+0}')
$HOME/unaos-bench/tools/unwrap80.sh "$LOG" | awk -v n="$n" 'NR>=n' > "$B"
echo "anchor_line=$n lines=$(wc -l < "$B")"; head -1 "$B"
# identity pin: the KELF max= on the wire must equal render2's staged kernel.elf
readelf -l -W "$FLASH/$NAME/kernel.elf" | awk '/LOAD/{v=strtonum($3)+strtonum($6); if(v>m)m=v} END{printf "staged elf max=%#x\n",m}'
```

If the wire `max=` differs, the board booted something else (a stale card, or the previous harvest) —
stop and say so; nothing below is about render2.

### C.3 The scorers — each with the positive control that must hit

All were dry-run against the render1 boot (the current last boot) and produce the results quoted in
the "render1 control" column, which is the pre-fix behaviour and proves each awk can fire.

**1. The a1cf4900 ordering assumption — DECLINE, never SHELL-WINDOW, beside a ROUTED console.**
Source: `main.rs:2717` runs `orin_conwin()` BEFORE `tegra_render_arm()` on the terminus line, and
`orin_conwin` stores `CONSOLE_WIN` synchronously (`fbcon.rs:2020`) when it routes; the render task's
first pass runs only after `run_capstone_boot_core` (same line, last) dispatches it, and reads
`fbcon::console_is_routed()` (`fbcon.rs:2242-2244`) at `main.rs:8221`. So with a `-> ROUTED` console
the expected line is `[orinrender] DECLINE reason=console-already-windowed` (`main.rs:8226`); a
`-> SHELL-WINDOW` (`main.rs:8234`) beside `-> ROUTED` means the guard is in the wrong place; a
`-> SHELL-WINDOW` beside `[orinconwin] DECLINE` is the legitimate fallback, not a failure.

```sh
awk '/\[orinconwin\].*-> ROUTED/{r++} /\[orinrender\] DECLINE reason=console-already-windowed/{d++} /\[orinrender\] win=.*-> SHELL-WINDOW/{s++}
     END{printf "conwin_routed=%d decline_windowed=%d shell_window=%d -> %s\n", r,d,s,
     (r&&d&&!s)?"PASS":(r&&s)?"FAIL guard-misplaced":(!r&&s)?"FALLBACK (no route; mint legit)":"NO-VERDICT"}' "$B"
```
Positive control: `conwin_routed>=1` (the `[orinconwin] win=2 … route=true live=LIVE -> ROUTED` line
existed on render1). render1 control: `conwin_routed=1 decline_windowed=0 shell_window=1 -> FAIL guard-misplaced`
(correct for c61b47e bytes: the guard did not exist yet).

**2. `[u7stk]` for `orin-render`, with headroom (lower bound; 0 = saturated).**

```sh
awk '/\[u7stk\]/{a++} /\[u7stk\].*task=[0-9]+:orin-render/{o++; for(i=1;i<=NF;i++) if($i ~ /^headroom=/){sub(/headroom=/,"",$i); if(h==""||$i+0<h) h=$i+0}}
     END{printf "u7stk_any=%d u7stk_orin_render=%d min_headroom=%s -> %s\n", a,o,(h==""?"n/a":h),
     o?(h>0?"PASS":"SATURATED (hw=len; lower bound 0 — grow the stack)"):(a?"NO orin-render probe":"NO [u7stk] AT ALL (probe not compiled/reached — see FLIGHT.md A.3)")}' "$B"
awk '/\[u7stk\].*orin-render/' "$B" | head -3
```
Positive control: `u7stk_any>=1`. render1 control: `u7stk_any=0 … NO [u7stk] AT ALL` — and per A.3
that is also what ac27b8d2 produces; `u7stk_any=0` on render2 is a build fact, not a flight fact,
unless `grep -a -o u7stk kernel.elf` in B.3 step 1 was ≥1. Also count the ungated stack witness:

```sh
awk '/\[redzone\] cpu=[0-9]+ LOW-REDZONE/{split($0,a," task="); split(a[2],b," "); k[b[1]" "(($0 ~ /TRAVERSED/)?"TRAVERSED":"entered")]++} END{for(x in k) print x, k[x]}' "$B"
```
render1 control: `4:orin-render TRAVERSED 8`, `1:jd2-console TRAVERSED 8` (cap of 16 hit).

**3. `presents=` greater than 1 across passes** (the A-half of a1cf4900: the pass no longer retires
the chrome, so `ui_status::tick` dirties it and `pal.render()` runs, `main.rs:8256-8259`).

```sh
awk '/\[orinrender\] census/{n++; for(i=1;i<=NF;i++){if($i ~ /^presents=/){sub(/presents=/,"",$i); if($i+0>p)p=$i+0} if($i ~ /^passes=/){sub(/passes=/,"",$i); q=$i+0}}}
     END{printf "census_lines=%d last_passes=%d max_presents=%d -> %s\n", n,q,p, n?(p>1?"PASS":"FAIL presents<=1"):"NO CENSUS"}' "$B"
```
Positive control: `census_lines>=1`. render1 control: `census_lines=3576 last_passes=71520000 max_presents=1 -> FAIL`.
Expected on render2: `presents` climbing at roughly the strip's ~1 Hz (`ui_status::tick`, `ui_status.rs:1077`),
i.e. `max_presents` ≈ wall seconds.

**4. Census line rate per wall second.** There are no per-line timestamps; the wall-time ruler
inside a boot is the kernel's own `up=<n>s` on `[orinclick] census` (`display_tegra.rs:1518`, ~every
10 s; orinclick is in the knob set) and `age_ms=` on `[wc-h] rollup` (~every 2 s). The scorer takes
the larger of the two.

```sh
awk '/\[orinrender\] census/{c++}
     /\[orinclick\] census/{for(i=1;i<=NF;i++) if($i ~ /^up=/){sub(/up=/,"",$i); sub(/s$/,"",$i); if($i+0>u)u=$i+0}}
     /age_ms=/{for(i=1;i<=NF;i++) if($i ~ /^age_ms=/){sub(/age_ms=/,"",$i); if($i/1000>a)a=$i/1000}}
     END{t=(u>a?u:a); printf "census=%d up_ruler=%ds age_ruler=%.0fs share=%.1f%% rate=%.2f/s -> %s\n", c,u,a,100*c/NR,(t?c/t:0),
     (t==0?"NO RULER":(c/t<=2?"PASS ~1/s":"FAIL flood"))}' "$B"
```
Positive control: a ruler > 0. render1 control: `census=3576 up_ruler=131s age_ruler=149s share=74.5% rate=24.08/s -> FAIL flood`.

⚠ Source-side expectation, stated so the number is not misread: at ac27b8d2 the census cadence is
still pass-counted, `if passes % 20000 == 0` (`main.rs:8266`). render1 ran ≈481k passes/s
(71.52M / 149 s), which is exactly the 24/s measured. A ~1 Hz present per pass will slow the loop
somewhat but not 24-fold; **render2 built from ac27b8d2 will score FAIL here by construction.**
Reaching ~1/s needs the cadence keyed on `arch::ms()` (as `ui_status::tick` already is), a code
change outside this document. Score it anyway — the measured rate is the number the change needs.

**5. Watchdog.** The token this branch prints is `[orinreboot] wdt ARMED — POR reset in …`
(`wdt_tegra.rs:136`) and `[orinreboot] wdt DISARMED — boot reached the EL1 terminus`
(`wdt_tegra.rs:155`); there is no `[orinwdt]` string anywhere in the tree. Both sites are
`#[cfg(feature = "orinwdt")]` (`main.rs:2102` arm, `main.rs:2717` disarm — the disarm is AFTER
`tegra_render_arm()` and BEFORE `run_capstone_boot_core`), and `orinwdt` comes only from
`UNAOS_ORINWDT=1` (`arroyo:976`), which render1's knob line did not carry.

```sh
awk '/\[orinreboot\] wdt DISARMED/{d++} /\[orinreboot\] wdt ARMED/{a++}
     END{printf "armed=%d disarmed=%d -> %s\n",a,d,(a&&d)?"PASS":(a&&!d)?"FAIL armed never disarmed":"ABSENT (orinwdt not in image — expected with render1 knobs)"}' "$B"
```
render1 control: `armed=0 disarmed=0 -> ABSENT`. Same expected for render2 with the recipe as written.
Adding `UNAOS_ORINWDT=1` is a knob decision for the seat, not made here: it arms a POR reset that
fires if the boot does not reach the terminus line (`wdt_tegra.rs:136`), which changes what a hang
looks like on the wire.

**6. Terminus and heartbeat.** Tokens present on render1 and expected on render2, in this order:
`AARCH64: timer heartbeat live (first tick).` (`main.rs:1070` region), `:: tegra: JM6b — EL1 landing: CurrentEL=1 …`,
`[orinrender] arm conwin=1 …` (`main.rs:8124`), `-> RENDER-ARMED` (`:8168`), `-> RENDER-LIVE` (`:8268`).

```sh
awk '/AARCH64: timer heartbeat live/{h++} /JM6b — EL1 landing: CurrentEL=1/{e++} /\[orinrender\] arm conwin=/{t++} /RENDER-ARMED/{ra++} /RENDER-LIVE/{rl++}
     END{printf "heartbeat=%d el1_landing=%d render_arm=%d render_armed=%d render_live=%d -> %s\n",h,e,t,ra,rl,(h&&e&&t&&ra&&rl)?"PASS":"INCOMPLETE"}' "$B"
awk '/\[orinrender\] (arm|spawned|DECLINE|REFUSE|win=)/' "$B" | cut -c1-160     # the arm sequence, verbatim
```
render1 control: `heartbeat=1 el1_landing=1 render_arm=1 render_armed=1 render_live=3576 -> PASS`.

### C.4 THE SECOND QUESTION — one byte at the board (documented step for Peter; do not expect an echo)

The butler forwards anything written to its FIFO straight to the serial TX
(`line-butler.py:368-372`: `cmd = os.read(fifo, 4096); os.write(ser, cmd)`), and the FIFO is
`~/unaos-bench/capture/line-acm0/inject.fifo` (`:230`, held O_RDWR so a one-shot `printf` works). The
repo's other bridge (`unaos/scripts/jetson-serial-bridge.py`, FIFO `~/unaos-bench/scratch/jetson.in`
after ac27b8d2) is NOT the one holding `/dev/ttyACM0` on this bench; use the butler's FIFO.

On the kernel side the Orin console task drains only `pal::next_event()`; the UART RX poll
(`arch::poll_input`, `arch/aarch64/mod.rs:292`) is called from `main.rs:1072/1809/5083/5116`, none of
which is on the tegra terminus path (`~/unaos-bench/scratch/orin12/SERIAL-DEVLOOP.md` §1 names the
missing fold at `main.rs:2854/2915/7602`). So on render2 the expected answer is: **nothing appears**.
The step exists to make that a measurement rather than an assumption, and to leave a mark in the
capture at a known wall time.

```sh
# with the board sitting at RENDER-LIVE for >= 30 s:
FIFO=$HOME/unaos-bench/capture/line-acm0/inject.fifo
L0=$(wc -l < "$HOME/unaos-bench/capture/line-acm0/orin.log")
date -u +%Y-%m-%dT%H:%M:%SZ; printf 'x' > "$FIFO"          # ONE byte, no CR
sleep 5
L1=$(wc -l < "$HOME/unaos-bench/capture/line-acm0/orin.log")
echo "lines during the 5 s window: $((L1-L0))"
$HOME/unaos-bench/tools/unwrap80.sh "$HOME/unaos-bench/capture/line-acm0/orin.log" | awk -v a="$L0" 'NR>a' | awk '!/\[orinrender\] census|\[wc-|\[wcn\]|\[wcpar\]|\[fluid3\]|\[comp2\]|\[dock\]|\[pstrip\]|\[orinclick\]/' | head -20
echo "MARK render2 $GIT7 one-byte-inject at raw=$(wc -c < "$HOME/unaos-bench/capture/line-acm0/raw.log") orin=$L1" >> "$HOME/unaos-bench/capture/line-acm0/marks.txt"
```
Anything other than the periodic rollups in that window (an echo `x`, a `[key]`/`Key(` event line, a
fault) is the finding. The `marks.txt` line follows the existing `MARK <tag> <sha> … at raw=… orin=…`
form (`capture/line-acm0/marks.txt`).

---

## Summary of the three answers

* **A.** `stk_probe` is `#[cfg(feature = "witness")]` (`sched.rs:89`) and has ONE caller, the
  `u7stk!` macro (`syscall.rs:16222-16225`, witness-gated on both arms) used only inside
  `u7_launcher`, whose only spawn (`main.rs:757`) is below `tegra_early_stop -> !` (`main.rs:190`).
  On the Orin the probe is dead code with or without `witness` — render1's kernel.elf, built WITH
  witness, contains zero `u7stk` strings (control `RENDER-LIVE`=1). `spawn` and `spawn_stack` both go
  through `spawn_inner`'s witness-gated paint (`sched.rs:3638-3639`); the probe cannot tell them apart.
  A `[u7stk] … orin-render` line needs a probe added to the pass. render1 already TRAVERSED its
  16 KiB stack's redzone (8 `[redzone]` reports for `4:orin-render`).
* **B.** `UNAOS_TEGRA=1 UNAOS_TEGRA_EL0=1 UNAOS_WITNESS=1 UNAOS_ORINDESK=1 UNAOS_ORINCLICK=1 UNAOS_ORINCONWIN=1 UNAOS_ORINRENDER=1 ./arroyo esp-jetson`,
  copy `target/aarch64_esp/.` to `flash/orin/render2-<UTC>-<sha7>/`, per-dir MANIFEST of `#` lines +
  `sha256sum` lines only, ledger append, `load-card.sh <name>` then `--write`, butler held first.
* **C.** Pin the boot at the LAST `KELF min=` line through `unwrap80.sh` and match its `max=` to the
  staged ELF; six scorers above, each with a control that fired on render1. Two of them are
  predicted to FAIL on ac27b8d2 by source, not by flight: `[u7stk]` (no caller) and census rate
  (pass-counted cadence ≈ 24/s at render1's pass rate).
