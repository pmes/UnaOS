# CURSORFLK: the `[cursor11] … passes=0 … -> FLICKER` red on the knob-off x86 lane

Executor CURSORFLK, branch `exec-rmbp-cursorflk`, parent `98fd8e66` (hw-rmbp), 2026-09-23.
Finding under test: STOR1's (rmbp-ledger B177) one-in-six `❌ FORBID* -> FLICKER` on
`[cursor11] … passes=0 … flicker_frames=5`. Classified in `docs/dev/FIXTURE_FLAKES.md` §3b,
ledgered as rmbp-ledger **B186**.

## 0. Which lane the red is on (this differs from the brief)

The brief named the lane as `env UNAOS_WC=1 UNAOS_QEMU_FULL=1 ./arroyo test 120`. **The red capture was
not on that lane.** `stor1-logs/m2-default2.log:1` prints `⚡ kernel features:
witness,ehcihid,kbdwit,sdhcblk,smolnet,sdwrite`, with no `wc`, so it is the KNOB-OFF `./arroyo test`
(STOR1's "default lane", `./arroyo test 240`, fast mode, `completion +15.0s`). On a `wc` x86 build
`flicker_frames` is 0 by construction: `video/screen.rs:866`
`DESK_SPRITE_OCC = cfg!(all(target_arch = "x86_64", feature = "wc"))`, and `bracket_needed` returns
`(false, true)` at `:1552` for every live arrow, so the count at `:1455-1457`
(`bracket && live && SPRITE_OWNS_PAINT && last_flush_bytes > 0`) cannot fire. Eight runs of the WC
command could not produce the red, so M1 ran the knob-off lane the red came from:
`env UNAOS_QEMU_FULL=1 ./arroyo test 120`.

## 1. M1: the rate on this tree (`98fd8e66`, 20 cores, run sequentially)

Every run COMPLETE: the sidecar reads `mode=full completion=complete`. None were truncated and none were re-run.

| run | loadavg at start (1/5/15) | `[ptrdead]` fpop12 | `[cursor] armed` | `[cursor11]` lines | verdict |
| --- | --- | --- | --- | --- | --- |
| r1 | 3.22 1.93 1.02 | 0 | absent | none | MBENCH PASS 16/16, 0 forbidden, full wall 139.1 s |
| r2 | 8.08 5.24 2.61 | 0 | absent | none | PASS 16/16, 120.4 s |
| r3 | 18.04 11.00 5.11 | 0 | absent | none | PASS 16/16, 120.6 s |
| r4 | 13.00 11.48 6.04 | 0 | absent | none | PASS 16/16, 120.4 s |
| r5 | 13.35 11.87 6.83 | 0 | absent | none | PASS 16/16, 120.6 s |
| r6 | 14.56 13.13 7.95 | 0 | absent | none | PASS 16/16, 120.4 s |
| r7 | 11.86 12.34 8.28 | 0 | absent | none | PASS 16/16, 120.3 s |
| r8 | 9.65 11.25 8.37 | 0 | absent | none | PASS 16/16, 120.5 s |

0 reds in 8. On the knob-off lane `[cursor11]` prints only when `flicker_frames > 0`. Every other
counter on that line is 0 there (`passes=0 bracketed=0 …`), so the digest in
`cursor11_desk_tick` (`video/cursor.rs:1306-1321`) stays equal to its initial 0 and the 5 s beat is
skipped. "No line" on a run of this lane therefore reads as "no flicker, or no arrow".

## 2. The wider population: every x86 capture a peer left readable

The population is every `*.log` file of more than 20 kB under `~/unaos-bench/scratch/rmbp-0915/*-logs/`
that carries a `[ptrdead] backlog` line. That is 726 files. They were de-duplicated to **420 distinct
boots** by the md5 of the 120 serial lines starting at `[ptrdead] backlog`. A wrapper `.log` file and its
`-serial.log` twin hash equal. A boot counts as a `wc` boot when it prints `[dmgovlp] verdict`. The full
table, one row per boot with its capture path, is `population.tsv` beside this file.

| lane | fpop12 | arrow armed | `-> FLICKER` | boots |
| --- | --- | --- | --- | --- |
| knob-off | 0 | no | no | **64** |
| knob-off | 0 | yes | no | 1 (`rendstack-logs/06-gored-4k.log`, which is a `wc` build whose gored image printed no dmgovlp; the banner reads `…,wc,…`) |
| knob-off | ≥1 | **yes** | no | 2 (`dirns-logs/x86-testfat-d.log`, `dirns-logs/x86-green.log`) |
| knob-off | ≥1 | **yes** | **YES** | 2 (`stor1-logs/m2-default2.log`, `ioapic-logs/test-r3.log`) |
| wc / wc+ptr | any | yes | no | 350 |
| wc | 0 | yes | YES | 1 (`ptrrepaint-logs/mutant-test.log`, PTRREPAINT's deliberate `DESK_SPRITE_OCC=false` mutant) |

On a real knob-off build, **the arrow arms if and only if PTRDEAD's backlog leg lost an event to a
foreign drain**: 4 of 4 with fpop12 ≥ 1 armed, 0 of 64 with fpop12 = 0 armed. **Two of the four
flickered.** The natural rate of the red is 2 in 68 knob-off boots (about 3%). Both reds come from the
two boots whose stolen motion put the arrow under a desktop present inside its 1.5 s visible window
(`pal.rs:327` `HIDE_AFTER_MS`).

The two natural reds, verbatim:

```
stor1-logs/m2-default2-serial.log
1432: [ptrdead] -> SKIP (window raced: fpop12=1 fpop3=0) — a competing drain took this fixture's own queued events; cpu=3 svc=Some(5)
1433: [cursor] armed x=641 y=399
1434: [ptrdead] backlog whole=skip nodrop=skip order=true pushed=192 entries=1 travel=(191,-191) folded=191 dropped=0 fpop12=1 fpop3=0 cpu=3 svc=Some(5) -> PASS
1579: [cursor11] compose-through scope=desk passes=0 bracketed=0 px_deferred=0 px_installed=0 px_redrawn=0 flicker_frames=5 px_absorbed=0 absorb_refused=0 -> FLICKER

ioapic-logs/test-r3.log
1916: [ptrdead] -> SKIP (window raced: fpop12=1 fpop3=0) — a competing drain took this fixture's own queued events; cpu=3 svc=Some(5)
1917: [ptrdead] backlog whole=skip nodrop=skip order=true pushed=192 entries=1 travel=(28,-28) folded=191 dropped=0 fpop12=1 fpop3=0 cpu=3 svc=Some(5) -> PASS
1918: [cursor] armed x=804 y=236
2059: [cursor11] compose-through scope=desk passes=0 bracketed=0 px_deferred=0 px_installed=0 px_redrawn=0 flicker_frames=4 px_absorbed=0 absorb_refused=0 -> FLICKER
```

**The arrow's position is the stolen travel, to the pixel.** The panel is 1280x800 and the pointer
starts at its centre (640,400). PTRDEAD pushes 192 motions of (+1,−1) (`arch/x86_64/syscall.rs:7163-7165`).
The leg keeps `travel`, and the foreign drain gets the remainder:

| capture | leg kept | stolen | centre + stolen | `[cursor] armed` |
| --- | --- | --- | --- | --- |
| `stor1-logs/m2-default2` | (191,−191) | (1,−1) | (641,399) | **x=641 y=399** |
| `ioapic-logs/test-r3` | (28,−28) | (164,−164) | (804,236) | **x=804 y=236** |
| `dirns-logs/x86-testfat-d` | (191,−191) | (1,−1) | (641,399) | **x=641 y=399** |
| `dirns-logs/x86-green` | (50,−50) + order leg | (142,−142) + (1,0) | (783,258) | **x=783 y=258** |
| probe runs below | (0,0) | (192,−192) | (832,208) | **x=832 y=208** |

## 3. M2: the probe (scratch, reverted)

`probe-apply.sh` (beside this file, with its `probe.diff`) made a line-neutral same-line append after the push loop's `}` at `syscall.rs:7165`.
The append spins, bounded at 200 ms, until `evq_pops()` moves, which hands the fold accumulator to
`x86_input_service` (`main.rs:6121`, Mouse arm `:6165` → `x86_ptr_install`, `:10159`) every time instead
of only when a timer preemption happens to hand it over. The probe changes nothing else. After the runs
the diff was reverted with `git apply -R`, and `syscall.rs` checked back to its pristine sha256.

| run | build | load at start | wire | verdict |
| --- | --- | --- | --- | --- |
| p1-knoboff-r1 | knob-off | 12.41 | `handed … after 6ms pops=1`, `[cursor] armed x=832 y=208`, `flicker_frames=5 … -> FLICKER` | **rc=1, MBENCH FAIL 16/16, 2 forbidden (`FORBID* -> FLICKER`)** |
| p1-knoboff-r2 | knob-off | 12.13 | `handed … after 3ms pops=1`, `armed x=832 y=208`, `flicker_frames=4 … -> FLICKER` | **rc=1, MBENCH FAIL, 2 forbidden** |
| p1-wc-r1 (CONTROL) | `UNAOS_WC=1` | 9.86 | `handed … after 3ms pops=1`, `armed x=832 y=208`, `[cursor11] scope=desk … flicker_frames=0 px_absorbed=405 absorb_refused=0 -> THROUGH`, `[flick2] … flush_undraw=0 flush_skip=9` | rc=0, MBENCH PASS 16/16, 0 forbidden |

The probe reproduces the red on demand, 2 runs in 2, and it is the natural red character for character:
`passes=0 bracketed=0 … px_absorbed=0 -> FLICKER`, and `flicker_frames` of 4 or 5 matches the 4 and 5
of the two natural sightings. **The control is the proof that the instrument is not the defect.** It puts
the same arrow at the same pixel on the same kind of stimulus. There the desktop presents did meet it:
`px_absorbed=405` counts the pixels PTRREPAINT's withheld copy settled under the arrow. On the `wc`
build they are subtracted and nothing blinks. On the knob-off build the same presents take the
CURSOR-13 bracket, take the front-buffer arrow off the scan-out, and `[cursor11]` counts each one
truthfully.

## 4. Class and disposition

- `[cursor11]` does **not** score a value that a declined step failed to publish (not Class 6). It does
  **not** sample before the cursor has settled (not Class 1). Its count is exact against the control.
- **The flicker is REAL.** On knob-off x86 the arrow is a front-buffer sprite, `SPRITE_OWNS_PAINT` holds,
  and `DESK_SPRITE_OCC` is compiled out, so every desktop present whose damage meets the arrow bracket
  puts a pointerless panel on the glass. The `flicker_frames` rustdoc (`video/cursor.rs:1163-1164`) says
  so: on knob-off x86 the counter is "a live measurement rather than an invariant". Under the brief this
  is the cursor code's, and it is **not fixed in this arc**.
- **The trigger is Class 3.** PTRDEAD's backlog leg pushes synthetic motion into the live
  `pal::EVENT_QUEUE`. The x86 input service on core 5 drains it, and when it takes the accumulator it
  delivers the synthetic travel to the REAL arrow. That puts a pointer on a lane that has none. The
  leg's SKIP arm (SELFTEST-RACE) protects the leg's own verdict. It does not stop the leaked events from
  reaching the product. The aarch64 tree already states the rule this breaks
  (`main.rs:3827-3835`, `ROUTER_SELFTEST`: a fixture's synthetic pointer events must arm nothing, "no
  `[cursor] armed` on a panel with no pointer").
- **No code changed. No knob was added.** Two cures, each a decision for its owner and neither made here:
  (1) PTRDEAD's window quiesces the x86 input service's drain, the `ROUTER_SELFTEST` shape on x86. That
  is `main.rs` work and is Class 3's open design question. It removes the stimulus, and the knob-off lane
  goes back to never having an arrow. (2) Knob-off x86 either compiles PTRREPAINT's subtraction or is
  exempted from mbench's always-on `-> FLICKER` FORBID (`scripts/mbench.py:153`, "required 0 on every
  board"). Today any knob-off x86 boot with a real pointer can red that FORBID.
