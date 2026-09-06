# CONSOLEQUIET — QUIET-PANEL for the aarch64 routed console (`fbcon.patch` rationale)

Executor CONSOLEQUIET, seat orin 15, track `hw-jetson`. Ledger row: `docs/dev/OS/orin-ledger.md` **A26**.
Patch: [`CONSOLEQUIET.patch`](CONSOLEQUIET.patch) in this directory — **not applied on this branch.**

**Lane.** `unaos/crates/kernel/src/video/fbcon.rs` is rmbp's. rmbp 12 ruled this port COVERED by the
standing parity grant (no new grant needed) and asked to see the patch before any `video/` commit, so
this branch carries DOCS ONLY: the patch text, this rationale, and the ledger row. The edit lived in
the executor's worktree for the gates below and was reverted before the commit.

**Base.** Cut against today's tree: `hw-jetson` tip `a05c2c8e`, trunk `main` `f49ea1e7`. The `index`
line reads `f282096c` — and that is the guarantee, not the commit id:
`a05c2c8e:unaos/crates/kernel/src/video/fbcon.rs` and `f49ea1e7:…` and `191823c2:…` are all
`f282096c368c8bf5544c18ca58bbc8c7de72c9ea`, so `git apply CONSOLEQUIET.patch` at the repo root
succeeds against any of the three.

```
git apply docs/dev/evidence/orin15/CONSOLEQUIET.patch
```

---

## 1. The defect

Peter's render6 boot (2026-09-06). The cascaded desktop's first and largest window is `console`, and
it is a scrolling kernel log for the life of the boot.

The screenshot is `~/unaos-bench/scratch/orin15/SCREEN0-small.png` (bench scratch, per the corpus
convention that the 6.9 MB `SCREEN*.PNG` captures are not committed; the full-size original is
`SCREEN0.PNG` beside it, card sha `3f4ee48a018a0c46`, `FLIGHT-RESULT-render6.md` row A17). What it
shows: three windows over the desktop — `console` (large, top), `shell` (middle), `pulse` (bottom
left), with the dock's `console` / `pulse` / `shell` tabs at the foot. The `shell` window holds a
clean prompt. The `console` window holds `[tcu] rx-mbox raw=…`, `[serialrx] rx=… polls=…`,
`[pulse5] live …`, `[wc-h] rollup …`, `:: SCHED: load …` — the witness census, one line per
instrument per second.

The wire says the same thing. In the committed excerpt [`render6-boot1.log`](render6-boot1.log)
(3944 lines, one boot):

| tag | lines |
|---|---|
| `[tcu]` | 486 |
| `[orinrender]` | 486 |
| `[serialrx]` | 483 |
| `[wc-b]` | 415 |
| `[wc-h]` | 226 |
| `[pulse5]` | 219 |

and the window they land in is named:

```
[wc-x] console-route first-paint win=1 (glyphs -> window surface, damage-limited)
[wc-x] console-window win=1 panel=1920x1200 surf=1295x736 box=1305x780 at (307,158) cell=7x16 cols=185 rows=46
```

**A user boots to a desktop and the biggest thing on it is a debug log.**

### Why "quiet" is not "blank", on this board

The objection the existing code records — a blank console window is worse than the defect — was
answered by A19FIX (`6f56eff8`, orin 14): **the shell gets its own window.** `[realdesk] … shell=win=3
surf=960x466 box=970x510 at (515,402)`, `shell-present win=3 outcome=Composited`, and the screenshot
shows its prompt live. The operator's interactive surface is `win=3`; `win=1` is a mirror of the wire
and nothing else. Making `win=1` quiet costs the desktop no interaction, and it is exactly the shape
an x86 `wc` desktop has had since QUIET-PANEL.

---

## 2. Condition K — WHICH writer this patch suppresses

**Both routed-console writers, through the one gate upstream of both. Nothing is left alone.**

Stated with `sha:symbol` rather than bare line numbers, against `ec0bb52f` (the executor's WIP commit;
the shipped patch is a superset of it by two comment blocks):

| writer | what it is | how it is reached |
|---|---|---|
| `ec0bb52f:video/fbcon.rs FbCon::write_byte` (`:452`; the `wcg::seam_glyph_note(true, CONSOLE_WIN…)` charge on `:455`) | the LOCKED classic per-byte path | one caller — `impl core::fmt::Write for Sink::write_str` (`:1590`), reached from `print_masked` (`:1274`), which is `_print`'s LAST statement (`:738`) |
| `ec0bb52f:video/fbcon.rs PanelSink` (`:1480`; the `seam_glyph_note(false, …)` charge) | the UNLOCKED split layout/paint path | `PanelSink::new()` has exactly one call site — `_print` `:724` |
| `ec0bb52f:video/fbcon.rs milestone` (`:1542`, the third `Sink` construction at `:1562`) | the QUIET-PANEL companion | **not on this arch**: its whole body is `#[cfg(all(target_arch = "x86_64", not(any(feature = "usbdebug", feature = "witness"))))]`, so on aarch64 it compiles to `let _ = (ms, tag);` |

`fbcon::_print` itself has two callers on aarch64, both in `ec0bb52f:arch/aarch64/serial.rs` — `:193`
(the panic escape hatch) and `:238` (the normal path). There is no other route to a console glyph.

So the early `return` at `_print`'s first gate (`ec0bb52f:video/fbcon.rs:681-683`) suppresses the
window writer AND the panel writer, and it is the only place a single test can do both.

### Why the gate already there does not do it

`panel_mirror_held` (`ec0bb52f:video/fbcon.rs:2391`, DESKHOLD) sits in the SAME gate and is a
different predicate. Its third term is

```rust
let unrouted = CONSOLE_WIN.load(Ordering::Relaxed) == wm::WIN_NONE;
…
PANEL_MIRROR_HOLD.load(Ordering::Relaxed) && !overridden && unrouted
```

— it **declines exactly when the console is routed**, by design and with its own written reason. On
the Orin `desktop_firmware::activate` arms the hold at DESKTOP-CLEAR (`video/desktop_firmware.rs:163`)
and opens the window seventy lines below it (`:237`), so the hold lifts inside the same call. rmbp 12's
condition K is right: **a patch that gated only DESKHOLD would pass every gate and leave the
screenshot byte-identical.** `conquiet_held` is DESKHOLD's complement — it holds only when routed —
and the two terms share one line.

---

## 3. The rule, and where its boundary is

> **On aarch64, from the moment the console is routed into a window, the full serial stream stops
> mirroring into the console.**

**THE WIRE IS UNTOUCHED.** Every dropped line was already written to the UART, the staging ring,
`UNAOS.LOG` and the `tste` ring by `arch::aarch64::serial::_print` before it ever called this file —
`fbcon::_print` is the LAST statement of that function, after the `SERIAL_PORT` guard has been
dropped. The gate charges `suppressed` on the SERWIT-2 tap, the ledger's word for "declined by
policy" as opposed to lost, so `submitted == absorbed + dropped + suppressed` still holds exactly
(measured: §8).

**The boundary is the ROUTE INSTALL, not boot, and that is not quite x86's boundary.** x86 goes quiet
at boot and paints `milestone` lines instead (`bootlog::record`). That replacement is
`#[cfg(target_arch = "x86_64")]` and has never been ported, and on aarch64 fbcon IS the bring-up
console. Matching x86 exactly therefore means porting `milestone` first. Until then the closest
honest analogue is "quiet from the moment the desktop has a console window" — which is the whole of
the window the screenshot is about, and it leaves aarch64's pre-desktop panel mirror exactly as it is
today. Stated here rather than implied, because it is the one place this port is not literal parity.

---

## 4. The knob — GATE-FAMILY, no board twin

Re-enabling the mirror uses **x86's own knob and no other**: `bootlog`, whose entire documented
purpose is holding the whole log on the glass. The gate's third cfg term is `not(feature = "bootlog")`,
the same term x86's QUIET-PANEL gate carries two statements below it.

Checked, not assumed. `crates/kernel/Cargo.toml:202` declares `bootlog = []` with no arch condition;
`arroyo:190` is `[ -n "${UNAOS_BOOTLOG:-}" ] && _feats="${_feats}bootlog,"` in the arch-neutral
feature assembly; `esp_jetson` (`arroyo:4695`) builds from that same `_feats`; and the `arm-pi` cfg
leg (`arroyo:2755`) already names `bootlog`, so it type-checks on the Pi too. Measured, not reasoned:
`UNAOS_BOOTLOG=1` + render6's knob line + `./arroyo esp-jetson` — §8E.

No new knob, no `orinquiet`/`piquiet` twin. Symbols are named for the subsystem
(`conquiet_held`, `CONQUIET_DROPPED`, `[conquiet]`), never for a board.

### ⚠ One place the knob does NOT reach today, found by trying it — pi seat, one line

**`kernel8.img` does not get `bootlog`.** `kernel8()` (`arroyo:5227`) builds from the CURATED
`K8_FEATS`, a separate list, and it has no `UNAOS_BOOTLOG` arm — `awk 'NR>=5227 && NR<=6228 && /bootlog/'`
over `arroyo` returns nothing (the same awk for `desktop_firmware` returns `5990`, so the probe can
hit). Measured: `UNAOS_PIDESK=1 UNAOS_LIVECON=1 UNAOS_BOOTLOG=1 … ./arroyo kernel8` banners
`⚡ kernel features: baremetal,skip_xhci,desktop_firmware,livecon` — no `bootlog` — and the resulting
image still goes quiet (§8C control).

So a `UNAOS_PIDESK=1` Pi image has, today, no way to put the mirror back. That is a gap in the Pi's
build script, not in this patch, and it is not this seat's lane to close. **The companion the pi seat
is asked for is one line in `kernel8()`, beside the `UNAOS_PIDESK` arm:**

```sh
[ -n "${UNAOS_BOOTLOG:-}" ] && K8_FEATS="${K8_FEATS},bootlog"
```

It changes no default image (default OFF => `K8_FEATS` unchanged => `kernel8.img` byte-identical), and
without it GATE-FAMILY's "same knob, every board" is true of the source and false of the Pi's media.

---

## 5. Where the patch is inert, and where it is NOT

The armed arm is `#[cfg(all(target_arch = "aarch64", feature = "desktop_firmware", not(feature = "bootlog")))]`;
the other arm is `#[inline(always)] fn conquiet_held() -> bool { false }`.

* **x86** — inert, and byte-identical (§8). Stated with its reason rather than as a boundary of
  convenience: PARFB argued the arch term in `panel_mirror_held` was incidental because nothing on
  x86 called `panel_mirror_hold`, so widening changed no x86 boot. That argument does **not** carry
  here. `_print`'s x86 QUIET-PANEL gate returns for the default build, so a widened term would be
  reached only on the paths that gate lets THROUGH — `PANEL_CONSOLE` (the Kepler-takeover lane) and
  `PANIC_MIRROR` — and on the first of those it would be a live behaviour change in a lane this seat
  does not own. **Widening it is rmbp's call, on rmbp's evidence.** This patch leaves x86 byte-identical.
* **The Pi's DEFAULT `kernel8.img`** — inert and byte-identical. `kernel8()`'s curated `K8_FEATS`
  starts `baremetal,skip_xhci` and never contains `desktop_firmware` by default.
* **The Pi's `UNAOS_PIDESK=1` image** — **ARMED, and its console mirror changes BY DESIGN.**
  `arroyo:5990` is the one place `desktop_firmware` enters `K8_FEATS`, and it is under `UNAOS_PIDESK`
  alone. That Pi desktop routes its console into a window through the same
  `panel_console_window_open` seam, so it has the same defect and takes the same cure. **ONE OS** —
  this is a rule about a routed console, not about a board. (Verified rather than asserted:
  `grep -n 'K8_FEATS="' unaos/arroyo | grep desktop_firmware` returns exactly `5990`, and the same
  grep without the filter returns 43 assignments, so the pattern could have hit and did, once.)
* **`bootlog` builds, any arch** — inert.

---

## 6. Condition M — the prediction, made BEFORE the measurement

**First, a correction to the condition's own coordinates.** rmbp 12 wrote `[wc-h] win=2` from rMBP A5
(`torn=111 banded=13085` under storm). On the Orin the ids are different and must not be transposed:
`FLIGHT-RESULT-render6.md` and the wire give **`win=1` = the console window**, `win=2` = `pulsewin`
(`[pulsewin] open win=2 … box=1290x212 at (10,914)`), `win=3` = the shell. So the prediction below is
about **`win=1`**, and it is checkable against the numbers in this file rather than against a number
from another board.

Baselines, both from render6, quoted verbatim (boot 1 is the committed excerpt; boot 2 is
`~/unaos-bench/capture/line-acm0/orin.log` from its second `KELF` anchor onward, 6399 lines):

```
boot 1, last win=1 rollup:
[wc-h] rollup win=1 scope=window emit=5  age_ms=7326   … torn=0 … whole=10 banded=50  … presspop=60
boot 2, last win=1 rollup:
[wc-h] rollup win=1 scope=window emit=13 age_ms=746715 … torn=0 … whole=48 banded=453 … presspop=501
boot 1, last win=2 (pulse) rollup:
[wc-h] rollup win=2 scope=window emit=207 … torn=0 … whole=1583 banded=0 … presspop=1583
```

**Predictions, falsifiable:**

1. **`[wc-h] … win=1 … banded=` collapses toward 0** on the next flight. `banded=` counts presents
   whose damage was a band, and on `win=1` a band is a console LINE — the console is the only thing
   that damages that window. 453 over boot 2 becomes the handful between the route install and the
   `[conquiet]` switch line, which is one line.
2. **`presspop=` for `win=1` falls with it** (501 → single digits), and `whole=` keeps its fixture and
   first-paint entries (`fixture=1`, and the `whole=3` the first rollup already had at 485 ms).
3. **`torn=` on `win=1` cannot rise**, and rmbp A5's storm reading has nothing left to feed it: a
   window that is not presented cannot tear. On render6 the Orin's `win=1 torn=0` already, so this is
   a prediction that a zero stays zero, not a repair.
4. **`win=2` and `win=3` are unchanged.** They are not written by `_print`. `win=2`'s
   `whole=1583 banded=0` is `pulsewin`'s own service pass and this patch does not touch it. **If
   `win=2` moves, the reading is that something else changed, not that this worked.**

**And the undercount warning rmbp asked for, stated so a drop is not scored as a defect:**

* **WCGSEAM's `glyphs=` falls BY CONSTRUCTION.** `wcg::seam_glyph_note` is called from exactly the two
  sites in §2, and both stop being reached. A `[wcgseam] … glyphs=` that stops climbing is this patch
  working, **not** the seam census undercounting. (The PARFB fold that widened those two charge sites
  to the route's own predicate is untouched and still correct — the charge is simply no longer
  offered.)
* **There is no `[wcgseam]` BASELINE on render6.** `awk '/wcgseam/' ` over both boots of
  `~/unaos-bench/capture/line-acm0/orin.log` returns **0 lines** — the rearm path never fired on that
  image. So the next flight has nothing to compare against on that tag, and a zero there must be read
  as "the census never spoke", not as "the census fell".
* **`[wc-b] rollup presents=` is panel-wide** (3331 by the end of boot 2) and will fall by the console's
  share only. It is not a `win=1` number and must not be read as one.

---

## 7. Condition N — the panic leg, demonstrated

The override is three independent guards, and each alone is sufficient:

1. `panic_screen` (`fbcon.rs:2105-2115`) sets `PANIC_MIRROR` **first** (`:2108`) and clears
   `CONSOLE_WIN` to `wm::WIN_NONE` **second** (`:2115`), both before any panic byte is printed
   (PANIC PATH LAW). The routed test in `conquiet_held` is therefore already false on that path.
2. `PANIC_MIRROR` is tested explicitly. It has been WRITTEN on this arch since PANICARM — and
   PANICARM is precisely the bug this condition exists to prevent recurring: the flag was declared but
   unwritable on aarch64 `desktop_firmware` builds, so *"on a routed desk boot every panic line was
   suppressed in `_print` before it touched the console lock. Red backdrop, no words, on the one path
   that has to work."* (`fbcon.rs:2107`.) The new term is in the same gate PANICARM's defect lived in,
   which is why it carries two more guards than it strictly needs.
3. `serial_ring::in_panic_mode()` is tested as well — the signal `defer_route_open`'s aarch64 arm
   takes, for its own stated reason: the `#[panic_handler]` sets it EARLIER than `panic_screen`, and
   it is true for a panic that never reaches `panic_screen` at all.

**The leg** (results in §8C). It is a Pi leg, because the Pi is the board with a routed-console
desktop that QEMU can boot and this seat can type at:

```
UNAOS_PIDESK=1 UNAOS_LIVECON=1 UNAOS_FBW=1920 UNAOS_FBH=1200 ./arroyo kernel8
qemu-system-aarch64 -machine raspi4b -kernel target/pi_baremetal/kernel8.img \
  -chardev socket,id=k8u0,host=127.0.0.1,port=$PORT,server=on,wait=off,logfile=$LOG \
  -serial chardev:k8u0 -serial null -display none &
python3 scripts/k8_type.py --port $PORT --script panic.inject --budget 300
```

`panic.inject` is mbench's inject grammar — `WAIT 240 :: BANDY-ACL:` (the last required witness of
`pi4-regression.spec`, so nothing typed can interleave with a fixture), `SLEEP 12`, then the word
`panic`, which `shell.rs:3033` turns into `panic!("Manual Panic Requested by Architect!")`.

`UNAOS_LIVECON=1` is not decoration: without it the GUI handoff calls `fbcon::detach()`, `GUI_ACTIVE`
short-circuits `_print`'s first term, and `conquiet_held` is never reached — the leg would prove
nothing about this patch.

Run OUTSIDE the spec gate deliberately: a typed panic halts the boot, so `pi4-regression.spec`'s
end-of-run markers would report TRUNCATED and the base spec's verdict would say nothing about the
panic. The leg is scored by named predicates on its own capture instead (§8C).

---

## 8. Gates

All in the executor's worktree with the patch APPLIED, `hw-jetson` base `a05c2c8e`.

| gate | command | result |
|---|---|---|
| type-check, both arches | `./arroyo check` | **exit 0** (`check3.log`) |
| x86 battery | `UNAOS_WC=1 ./arroyo test 150` | **exit 0**; banner `⚡ kernel features: witness,ehcihid,kbdwit,sdhcblk,smolnet,wc` — `wc` present |
| x86 byte identity | `objcopy -O binary target/x86_64-unaos/release/unaos-kernel` | see §8A |
| aarch64 virt | `./arroyo test-arm 60` | **exit 0** |
| Pi 4 bare-metal | `UNAOS_FBW=1920 UNAOS_FBH=1200 ./arroyo kernel8-test 210` | **exit 0** — `MBENCH PASS — 119/119 required witnesses, 0 forbidden hit(s), 36068 lines scanned` |
| armed jetson media | render6's knob line + `./arroyo esp-jetson` | see §8B |
| panic leg | §7 | see §8C |

### 8A. x86 byte identity

**PROVEN, not argued.** Two full `UNAOS_WC=1 ./arroyo test` builds in the same worktree — the second
with `git checkout 191823c2 -- unaos/crates/kernel/src/video/fbcon.rs` (that blob is `f282096c`, the
pre-patch file) and nothing else changed — then `objcopy -O binary` of each `unaos-kernel`:

```
731c8f5b2e9f8d3a95739c64b70fa8dd66dec06b180fc5a82382042605005a35  x86-baseline.bin
731c8f5b2e9f8d3a95739c64b70fa8dd66dec06b180fc5a82382042605005a35  x86-patched.bin
```

Identical. The `|| conquiet_held()` term folds to `|| false` and the appended block compiles to
nothing on x86, and no `panic::Location` moved (the one in-place hunk is line-neutral, §9).

A third data point that the doc's own edits are free: the patched sha above was taken from a build
made BEFORE two comment-only, tail-only blocks were added to the patch, and again AFTER. Same
`731c8f5b…` both times.

### 8B. The armed jetson image — reachability, not just compilation

A banner is not reachability. The image was built with render6's own knob line and the witness
strings were then looked for IN THE BINARY:

```
UNAOS_TEGRA=1 UNAOS_TEGRA_EL0=1 UNAOS_WITNESS=1 UNAOS_ORINRENDER=1 UNAOS_DESKCASCADE=1 \
  UNAOS_ORINRX=1 UNAOS_HOLOCRON=1 UNAOS_ORINCLICK=1 UNAOS_TCUPROBE=1 ./arroyo esp-jetson
   -> exit 0; target/aarch64-unaos/release/unaos-kernel, 2 677 184 bytes
```

| `grep -a -c` on that ELF | hits |
|---|---|
| `\[conquiet\] mirror=off since=console-window-route` | **1** |
| `\[conquiet\] census dropped=` | **1** |
| `knob=bootlog` | **1** |
| `\[conquiet\] mirror=on since=nothing-at-all` *(known-absent control)* | **0** |
| `\[conqiuet\]` *(known-absent control — the token misspelt)* | **0** |

The controls are the point: a pattern that returns 0 on a string that is not there, and 1 on the
strings that are, is a check that could have failed. The format strings are linked into the armed
jetson image; the arm is not vacuous.

Note the knob line is render6's exactly — `UNAOS_GA10B_PROBE2` is deliberately absent (A24's row says
rung 1 would power the board off behind rung 2, and neither rung belongs in a console gate).

### 8C. The panic leg

Capture: `nleg-serial.log`, 47 064 lines, one boot, one typed word. The image is `kernel8.img` built
`UNAOS_PIDESK=1 UNAOS_LIVECON=1 UNAOS_FBW=1920 UNAOS_FBH=1200` — and at that geometry the Pi's console
window is the SAME box as the Orin's: `[wc-x] console-window win=1 panel=1920x1200 surf=1295x736
box=1305x780 at (307,158) cell=7x16 cols=185 rows=46`.

**The instrument is `[mirror] fbcon:`** (`serial_ring.rs:1397`, `mirror_service`), and it is a
*positive* instrument for this question. It reports `TAP_FBCON.dropped` — lines charged `drop_()`
because the CONSOLE LOCK was contended. A line the new gate holds never gets there: `_print` charges
`suppress()` and returns before it touches `FBCON`. **So the drop counter can only move on lines that
got PAST the gate**, and its movement is the demonstration.

| line | wire |
|---|---|
| 130 | `[conquiet] mirror=off since=console-window-route win=1 lines_dropped=1 knob=bootlog …` |
| 131 | `[wc-x] console-route first-paint win=1 (glyphs -> window surface, damage-limited)` |
| 132 | `[wc-x] console-window panic-fallback armed win=1 (panic paints the PANEL, not the window)` |
| 145 | `[mirror] fbcon: 4 line(s) dropped, …` ← the LAST such report before the quiet stretch |
| 387 | `[conquiet] census dropped=256 win=1 -> QUIET (…)` |
| 146–39 996 | **39 851 lines and not one `[mirror] fbcon:` report.** `grep -a -c` over exactly that range = **0**. The counter is frozen at 4 for the whole quiet stretch. |
| 39 995 | `:: [midden] cmd="panic" -> Host verb=panic ::` — the typed word dispatched |
| 39 997–40 011 | `[mirror] fbcon: 5 … 6 … 7 … 8 … 9 … 10 … 11 … 12 … 13 … 14 … 15 … 16 line(s) dropped` — **twelve increments in fifteen lines**, and the panic text is byte-interleaved INTO them: `13 line(s) dropped, 0 truncated s=== KERince booNEL PANIC ===`, then `… or fulpanicked at l)` / `crates/kernel/src/shell.rs:3035:13:` / `Manual Panic Requested by Architect!` |

**Read it plainly.** The mirror was off for 39 851 lines — the write path was not merely quiet, it was
not entered, and the counter proves it. A panic on the shell core turned it back on within the same
line: the drop counter starts moving again at the instant `=== KERNEL PANIC ===` is being emitted,
which can only happen if `conquiet_held()` declined. It declined because `panic_screen` had set
`PANIC_MIRROR` and cleared `CONSOLE_WIN` before the first panic byte, and because `in_panic_mode()`
was already true — the three guards of §7.

The *contention* those twelve drops record is itself the panic core PAINTING: `panic_screen` holds
`FBCON` while it lays the red backdrop and its text, so the other core's `[click2]` lines lose the
`try_lock` and are charged `dropped`. On serial-less metal that lock holder is the only reason a
panic is visible at all, which is exactly what condition N is about.

Whole-boot tap conservation with the gate ARMED, from the same class of image:
`:: SERWIT-2 tap fbcon: submitted=145 absorbed=92 staged=0 dropped=4 suppressed=49 torn=0 …` —
`92 + 4 + 49 = 145`. Nothing vanished; 49 lines were declined by policy and are all on the wire.

**Control — the same image with the patch REVERTED:**

`git checkout 191823c2 -- unaos/crates/kernel/src/video/fbcon.rs`, same build line, same 150 s window,
no typing (`control-unpatched-serial.log`, 21 129 lines) — against the patched run of the same shape
and window (`control-bootlog-serial.log`, 23 785 lines):

| measured over the same 150 s | patched | **unpatched control** |
|---|---|---|
| `[conquiet]` lines | 2 | **0** |
| `[mirror] fbcon:` reports | 1 | **37** |
| `TAP_FBCON.dropped` reached | 3 | **44** |
| `SERWIT-2 tap fbcon`, at `submitted=144` | `absorbed=94 dropped=3 suppressed=47` | `absorbed=111 dropped=7 suppressed=26` |

The last row is the same instant of the same boot on both images, so it is a paired reading:
**+21 suppressed, −17 absorbed, −4 dropped**, and `absorbed + dropped + suppressed = 144` on both.
The quiet stretch in the N leg is the patch. And every line of the difference is on the wire — the
capture is 21–47 k lines either way.

**A second control, and it found a real gap** (§4): the run intended as a knob-on control
(`UNAOS_BOOTLOG=1` added) produced an image that went quiet ANYWAY — `[conquiet] mirror=off` and
`census dropped=256` both present, `⚡ kernel features: baremetal,skip_xhci,desktop_firmware,livecon`
with no `bootlog` in it. That is not this patch misbehaving; it is `kernel8()`'s curated `K8_FEATS`
never receiving the knob. The knob-on demonstration therefore had to be taken on the Orin's own media
instead (§8E), and the Pi's one-line companion is named in §4.

### 8E. The knob, demonstrated on the Orin's own media

Same render6 knob line, plus `UNAOS_BOOTLOG=1`, `./arroyo esp-jetson` → **exit 0**, banner

```
⚡ kernel features (jetson): witness,bootlog,ehcihid,kbdwit,sdhcblk,holocron,smolnet,tegra,
  orinclick,tegra_el0,tegrasmp,orinrender,desktop_firmware,orinrx,tcuprobe,deskcascade
```

and then `grep -a -c` on that ELF:

| pattern | knob OFF (§8B) | knob ON |
|---|---|---|
| `\[conquiet\] mirror=off since=console-window-route` | 1 | **0** |
| `console-window panic-fallback armed` *(a string that is there either way — the control)* | 1 | **1** |

The gate is compiled OUT by the knob, and the control proves the grep could still find a string in
that binary. Nothing about the mirror survives `UNAOS_BOOTLOG=1` — which is the whole contract.

### 8D. Which Pi spec assertions read console-mirrored text

**None.** `pi4-regression.spec` and `pi4-barename.spec` are read from the SERIAL capture
(`test_kernel8` points QEMU's `-serial` at `$logf` and mbench replays exactly that file); the panel
mirror is never a spec input, on any arch. A grep that could have hit — searching the spec's
`REQUIRE`/`FORBID` lines for `fbcon|console|mirror|panel|glyph|wc-x|serwit|tap` — returns four, and
each is about the WIRE:

| assertion | what it reads | result on the 210 s gate |
|---|---|---|
| `REQUIRE :: SERWIT-2: mirror taps .*-> PASS ::` (`:265`) | the tap CONSERVATION verdict, printed to serial | ✅ — and this is the assertion this patch could most plausibly have broken, because the new gate charges `suppressed`. Wire: `:: SERWIT-2 tap fbcon: submitted=216 absorbed=197 staged=0 dropped=11 suppressed=8 torn=0 …` — `197 + 11 + 8 = 216`. |
| `FORBID :: SERWIT-2: FAIL —` (`:266`) | ditto | ✅ 0 hits |
| `REQUIRE \[pstrip\] armed cores=… panel=…` (`:1423`) | the strip's panel GEOMETRY line | ✅ (geometry, not mirrored text) |
| `FORBID \[pstrip\] armed .*panel=\(…,0x…\)` (`:1426`) | ditto | ✅ 0 hits |

The default `kernel8.img` takes the inert arm in any case (§5), so the 119/119 above is a statement
about an image this patch does not change.

---

## 9. Shape of the patch — the fbcon ONE-LINE rules

`fbcon.rs` is compiled into knob-off images whose byte-identity is a standing proof, and
`core::panic::Location` records embed source LINE NUMBERS — a line added anywhere renumbers every
panic site below it. So:

* **Two hunks, and their shapes are the whole discipline:**

  ```
  @@ -678,7 +678,7 @@ pub fn _print(args: core::fmt::Arguments) {     <- N -> N, in place
  @@ -2904,3 +2904,152 @@ fn defer_note_classic() {}                  <- append at EOF, below all code
  ```

  The first is 1 line → 1 line. The second starts at the file's last line, so nothing above it moves.
* **P7 position proof** — the new call is CODE, not prose after a `//`:

  ```
  $ python3 -c "…"
  line 681 len 1549
  first // at 86
  conquiet_held() at 68 BEFORE //
  panel_mirror_held() at 45 BEFORE //
  code prefix: if GUI_ACTIVE.load(Ordering::Relaxed) || panel_mirror_held() || conquiet_held() {
  next two lines: '        tap.suppress();' '        return;'
  ```
* **The `[conquiet]` witness is one line, >8 bytes, emitted ONCE at the switch** — plus one census at
  256 drops. The switch line is let through by a one-shot `CONQUIET_PASS` flag so the window is not a
  mystery blank box: the line that says why the console went quiet is the last thing painted in it.
  The re-entrancy is safe by inspection: `arch/aarch64/serial.rs::_print` calls `fbcon::_print`
  AFTER the `without_interrupts` closure has ended and the `SERIAL_PORT` guard has dropped, and
  `conquiet_held` runs before `_print` touches `FBCON`, so the nested `serial_println!` acquires
  nothing this core holds. The bound on the race is stated in the code: one `serial_println!` wide,
  on another core, and every later line on every core is still dropped.

---

## 10. What rmbp is being asked to rule on

1. **Take the patch, or take it with the arch term widened.** §5 gives the reason the term is narrow
   and names the two x86 paths a widening would reach. The seat has no evidence about the
   Kepler-takeover lane and has not guessed.
2. **`milestone` for aarch64** (§3) — the only part of x86's QUIET-PANEL this port does not reproduce
   is the pre-desktop replacement channel. Not asked for here; named so it is not mistaken for an
   oversight.
3. **The Pi's `UNAOS_PIDESK=1` image changes** (§5). The Pi seat should know; ONE OS says it should.

Sequencing: rmbp 12 ruled CONSOLEQUIET lands before PULSEOCCL
(`docs/dev/evidence/orin13/pulseoccl-fbcon.patch`, S15), which is re-measured against the A19FIX
layout before any ask.
