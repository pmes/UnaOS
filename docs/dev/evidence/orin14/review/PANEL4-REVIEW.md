# PANEL4 adversarial review — hw-jetson 6cc8de8c..518dca3e (code commits), 2026-09-05

Reviewer: independent agent panel seat, read-only (private worktree; tip taken from the shared object
store as `hw-jetson` = `518dca3e`; nothing committed, nothing stashed, nothing pushed).
Commits in scope (`git log --oneline 6cc8de8c..hw-jetson -- unaos/`): DESKSCENE `8cbfaadf` (main.rs),
RXDISCRIM `fe6fe3b5` (arch/aarch64/serial.rs), PRTSCR2 `f0db58bf` (video/prtscr.rs).
`git diff --stat 6cc8de8c hw-jetson | grep -v ' docs/'` → exactly those three files (50 / 92 / 92 lines);
`git log --oneline 6cc8de8c..hw-jetson -- unaos/arroyo unaos/scripts` → empty.
Flight read: `docs/dev/evidence/orin14/FLIGHT-RESULT-render4.md` + `render4-boot1.log` (A15/A18/A17/A16 PASS).

**Verdict: no blocking finding.** One MEDIUM (a battery gap, not a code defect) and five LOWs. The code
as flown is sound; nothing below argues against landing once M1's leg has run.

---

## MEDIUM

### M1. The Pi bare-metal leg has not run on a tree that contains PRTSCR2 — and PRTSCR2 is Pi-compiled code
`unaos/crates/kernel/src/video/mod.rs:168` — `pub mod prtscr;` is UNGATED; `capture()` is reachable on the
Pi through `shell.rs:3546` (`screenshot` verb) and the Print Screen key. PRTSCR2 is mid-file
(`git diff 6cc8de8c hw-jetson -- unaos/crates/kernel/src/video/prtscr.rs | grep '^@@'` → hunks at base
lines 56, 99, 128, 141, 175, 218, 242, 315, 338; file 511 → 597 lines), so the Pi's `kernel8.img` changes
in bytes AND behaviour (a new door, a new wire line, `panic::Location` shifts of up to +86 for the indexing
sites at prtscr.rs:578-583). That is legitimate — A17-PRTSCR.md:106 says so honestly ("a code change in
all images, not a knob-off byte-identity question") and rmbp 11 granted it — but the landing battery's Pi
row (`LANDING-REPORT-2.md:43`, `UNAOS_FBW=1920 UNAOS_FBH=1200 ./arroyo kernel8-test 210`, MBENCH 119/119)
ran on `d11cd56e`, whose tree `== 6cc8de8c` — BEFORE all three commits. PRTSCR2's own proof was x86
`test-fat` + `test-arm` (the virt board, `LANDING-REPORT-2.md:61`); the "knob-off kernel8.img byte-identical"
claim at `:60` is attached to RXDISCRIM only. Net: the one commit that changes what the Pi runs has never
been through the Pi's regression gate.
Failure scenario: a Pi boot where `prtscr::service()`'s device-service pass now takes the `IN_FLIGHT`
compare-exchange each sweep, or the `screenshot` verb path, trips something MBENCH counts (a FORBID on
`refused`, a line-count budget) — nobody has looked.
Fix: run `UNAOS_FBW=1920 UNAOS_FBH=1200 ./arroyo kernel8-test 210` on the merge preview (arc tip merged on
current trunk) before the `--no-ff`, and put that sha in the Pi row. Tell pi 6 in the same message that
prtscr.rs is +86 lines mid-file (their knob-off identity check must exclude it, not be surprised by it).

---

## LOW

### L1. `orin_desk_scene_up` — a new board-named symbol in a shared file (Peter, 2026-09-03: name by subsystem)
`unaos/crates/kernel/src/main.rs:8121` (definition) + 2 uses (`:8149`, `:8281`); base has 0 (`git show
6cc8de8c:unaos/crates/kernel/src/main.rs | grep -c orin_desk_scene_up` → 0; tip → 3). Comment-stripped
count of new board tokens in the main.rs hunks: `orin_desk_scene_up` ×3, plus one new `[orinrender]` string
site (the `strip=kept reason=no-scene` line, `:8285`) in an already-tabled family. prtscr.rs and serial.rs
add none (two "Orin" mentions in prtscr.rs are `//!`/`//` prose).
Fix: rename to `render_desk_scene_up` (it sits beside `orin_render_service`, so either rename with that
family at GATE-NEUTRAL time and add the row to `NEUTRAL-TABLE.md` now, or rename before landing). No
behaviour change either way.

### L2. `ovrf=` counts polls that SAW the bit, not overrun events — and a decode-error word counts too
`unaos/crates/kernel/src/arch/aarch64/serial.rs:742-744` (`note_lsr`): `OVRF += 1` when
`status != 0xFFFF_FFFF && status & (1<<1) != 0`. LSR is read on every poll and the read clears OE, so N
overrun events between two polls collapse to ONE count — and the model this field is meant to discriminate
(the ~2.3 ms `KEY` echo stall with no polls) is exactly the case where several bytes die between two polls.
The comment at `:721-722` ("each overrun event counts once") and A16-SCORE.md:18 overstate; the A16 verdict
table is unaffected because it tests `ovrf > 0`, never equality (A16-SCORE.md:81-83), and render4 read
`ovrf=0` on both legs (`awk '/\[serialrx\] rx=/ && !/ovrf=0 /' render4-boot1.log` → nothing).
Second: the only excluded word is open-bus. A SoC decode-error word (`0xdead....`, the third class
`classify` names) has bit 1 set (`0xdead_beef & 2 != 0`) and would count on every poll — ~325k/s of noise
under a verdict that already says "decode error".
Fix: `if (status & 0xFFFF_0000) == 0 && (status & LSR_OVRF) != 0` (Tegra's `UART_LSR_0` uses bits 8-9,
so mask the top half, not `>> 8`), and reword both comments to "polls that observed OVRF (≥ 1 lost byte
each)". `(+d)` on the census line still means bytes: `RX` is bumped once per `push_event` in `drain()`
(`:777-779`), untouched by this arc.

### L3. The IIR read is not side-effect-free for the co-owner; the comment says it is
`serial.rs:786-790`: one `read_volatile(base + (2 << 2))`, guarded by `LSR_PRINTED.swap(true)` at `:783`
— genuinely ONCE per boot (`awk '/\[serialrx\] lsr=/' render4-boot1.log | wc -l` → 1). 16550 semantics:
reading IIR acknowledges a pending THRE interrupt when it is the highest-priority one. The SPE/TCU firmware
is the other reader of this port; if it drives its TX from the THRE interrupt, an IIR read landing in the
window between THRE assertion and the SPE's own ISR read would eat that edge and its TX pump would sit
until the next event. Probability: small, once per boot; render4's `iir=0xc1` has bit 0 SET (no interrupt
pending at the read), so on the flown boot nothing was acknowledged.
Fix: keep the read (FCR is write-only; there is no other read-only FIFO witness) but (a) correct the
comment — "side-effect-free for us; it CAN acknowledge the co-owner's THRE interrupt, accepted as a
once-per-boot hazard" — and (b) print `pending={}` = `(iir & 1) == 0` on the witness line so the wire
says whether the read ate anything on THAT boot.

### L4. A17's scorer has no partner pattern for a `Short` refusal after `-> capturing`
`prtscr.rs:277-280` prints `… short write N of M bytes — capture INCOMPLETE ::` for `Refusal::Short`, the
one refusal that can only occur AFTER the `-> capturing` line (`:421-427`). The module doc (`:70-72`) lists
three outcomes (`-> OK`, `capture skipped`, nothing) and A17-PRTSCR.md:128-129 scores "`capturing` without a
partner = boot cut mid-capture". A short write on a full card would be read as a power cut.
Fix: add `capture INCOMPLETE` to the partner set in the scorer and the doc; four outcomes, not three.

### L5. `pulsewin::service()` now runs at the pass rate, and the pass rate is ~320k/s
`main.rs:8358` — the fold appends `pulsewin::service()` to the `tick` line. `service()` calls
`ui_status::loads()` (`ui_status.rs:946-949`: `PULSE.lock()` + 32-word copy) and `frame_sig` (FNV over
`ncpu` words) on EVERY pass; the loop has no pacing (`sed -n '8310,8360p' main.rs | grep -E 'yield|wfe|wait'`
→ none), and the flight shows it: `census passes=99690759` on the last of 312 census lines ≈ 320k passes/s,
`presents=1`. Same task as `tick`'s own `PULSE.lock()`, so no new contention — a duplicated lock + hash on
cpu 0, the console-pump core. The Pi carries the same fold, so this is parity, not a regression, and it is
not a landing question; it is S7's (render_service convergence): call `service()` only on the pass `tick`
sampled, or pace the loop on `cntpct`.

---

## Questions answered

**(1) Knob-off byte identity for the Pi.**
`git diff 6cc8de8c hw-jetson -- unaos/crates/kernel/src/arch/aarch64/serial.rs | grep '^@@'` → hunks at
base 713, 720, 751, 777; `git show hw-jetson:…/serial.rs | grep -n 'pub mod serialrx'` → 701, under
`#[cfg(all(feature = "tegra", feature = "orinrx"))]` (`:700`), and it is the LAST item (base 785 → tip 829,
nothing follows). Every serial.rs hunk is inside the cfg-erased tail module; no Pi-lexed line above 701
moves. serial.rs is in the LIB crate (`lib.rs:27 pub mod arch`); the seat's own comment at `:53` records
the ThinLTO `.llvm.<hash>` symtab caveat for knob-ON builds. **main.rs**: hunks at base 8106+;
`awk 'NR>=8100 && /^(pub )?(fn|static|const) /'` on the tip lists nine items from 8107 to 8552, every one
under `#[cfg(all(target_arch = "aarch64", feature = "orinrender"|"deskcascade"[, "witness"]))]`, and no
item after 8552 — so no Pi- or x86-compiled statement sits below the +18-line insertion at 8106; no
`Location` shifts on either. **prtscr.rs**: NOT line-neutral and Pi-compiled → M1 (by design, granted).

**(2) DESKSCENE.** Every hunk is inside an `orinrender` region — `:8120` (`orin_desk_scene_up`), `:8132`
(`tegra_render_arm`, hunks at 8139/8190), `:8241` (`orin_render_service`, hunks at 8264/8346/8381/8402);
`orinrender = ["desktop_firmware", "tegra_el0"]` (Cargo.toml:2356), so the inner
`#[cfg(feature = "desktop_firmware")]` on the `service()` call and the ungated `pulsewin::win()` /
`desktop_scene_owns_backdrop()` (both gated `any(x86 wc, aarch64 desktop_firmware)`) always resolve.
Presents: `pal.render()` is the ONE present through `pal` per pass, and `dirty |= passes == 1` (`:8372`)
makes pass 1 present by construction; on the cascaded scene `tick` is masked to `false` forever
(`ui_status.rs:1285`), so `presents=1` and stays (flight: `presents=1` on all 312 census lines). The pulse
window's repaints are NOT `pal`'s: `service()` → `paint()` → `wm::present(id)` (`pulsewin.rs:569`) presents
through its own row, sets nothing in `dirty`, and is invisible to `presents=` (a `redraws=` in `[pstrip]`
is the nearest count). Pass-1 order is open (`[pulsewin] open`, log :516) THEN the backdrop present
(`census passes=1`, :523); `Screen::flush` → `present_background` re-composites the windows over the
backdrop (`screen.rs:1131`, `:1241`), so the fresh window is not overdrawn. On an un-cascaded board
`service()` is one `loads()` call (ARMED false) and `tick`'s arming pass returns dirty anyway.
Idempotence: `retire_desktop_chrome` is `DESKTOP_SCENE.swap(true)` (`video/mod.rs:730`), prints `was=`;
it runs once in the task prologue and `ORINRENDER_ARMED.swap` (`:8152`) forbids a second spawn.

**(3) PRTSCR2.** Door: `capture()` (`prtscr.rs:382-391`) takes `IN_FLIGHT` by `compare_exchange(false,true)`,
calls `capture_inner()`, stores `false` — Ok and every `?`-Err release through the single site. Panic: no
release, and none needed — `aarch64-unaos.json:18 "panic-strategy": "abort"`, the kernel does not unwind.
`-> capturing` prints AFTER `panel_snapshot`/`NoFormat` (`:399-408`), AFTER `mount_program_source` +
`write_veto` (`:411-414`: no-volume and read-only refuse BEFORE it), AFTER `next_free_name` (`AllTaken`
before it); only `Encode`/`Fat`/`Short` can follow it, each named (L4 for the scorer). Witness strings:
`git diff 6cc8de8c hw-jetson -- unaos/crates/kernel/src/video/prtscr.rs | grep '^-.*PRTSCR'` → empty (rc 1).
Shell verb: `shell.rs:3546` calls the same `prtscr::capture()`, so it takes the same door and gets
`Refusal::InFlight` (`report()` + `sentence()`) when the key's capture holds it.

**(4) RXDISCRIM.** IIR read is once (L3); `ovrf=` is per poll-that-saw-the-bit (L2); `(+d)` still bytes.

**(5) `unaos/arroyo` / `unaos/scripts`.** `git log --oneline 6cc8de8c..hw-jetson -- unaos/arroyo unaos/scripts`
→ nothing. Note: A16's injector (`~/unaos-bench/tools/inject-paced.sh`) and the scorers
(`~/unaos-bench/scratch/orin14/scorers.run.sh`) live outside the repo; the A16 recipe depends on an
unversioned tool. No spec under `unaos/scripts/specs/` matches `[serialrx]`, `RENDER-LIVE`, `refused=` or
`PRTSCR` (`grep -rn -E 'PRTSCR|serialrx|RENDER-LIVE|refused=' unaos/scripts/specs` → comments only), so the
widened census lines break no gate.

**(6) GATE-FAMILY / GATE-NEUTRAL exposure.** `git diff 6cc8de8c hw-jetson -- unaos/crates/kernel/src/main.rs |
grep '^+' | sed -E 's#//.*$##' | grep -oE '\b(orin_[a-z_]+|tegra_[a-z_]+|\[orin[a-z]*\])' | sort | uniq -c`
→ `3 orin_desk_scene_up` (new symbol, L1) + `[orinrender]` ×1 new site in string literals (`sed` strips
`//` only; the string sites are at `:8142`, `:8285`, `:8405` — two modified, one new). video/prtscr.rs and
serial.rs: 0 board tokens outside comments.
