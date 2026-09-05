# ORIN-BROKEN — what does not work on the Jetson Orin Nano that works on the Pi 4 and/or the x86 rMBP

Executor ORINBROKEN, 2026-09-05. Read-only. Tree `/home/pmes/src/github.com/pmes/UnaOS-orin`, `hw-jetson`
tip `2000608a` (PRTSCR-ORIN; tree clean). Card holds `render2-20260903T2157Z-8085c9c` — one docs commit
and one code commit (PRTSCR-ORIN) behind the tip. North star: a desktop (`docs/ROADMAP.md:13`,
`docs/dev/OS/08_VIDEO/orin-desktop.md` §6).

Every row cites a file:line in this tree or a line in `~/unaos-bench/scratch/orin13/render2-boot.log`
(924 lines, pinned by `KELF min=0x0 max=0x2d92a8` at line 1). Nothing below is recalled. Where a
root cause is an inference from the evidence rather than a statement in the tree, it is marked
*(inference)*.

Abbreviations: `od` = `docs/dev/OS/08_VIDEO/orin-desktop.md`; `AC` = `~/unaos-bench/scratch/orin12/ARCH-CONFORMANCE.md`;
`SD` = `~/unaos-bench/scratch/orin12/SERIAL-DEVLOOP.md`; `RM` = `~/.claude/projects/-home-pmes-src-github-com-pmes-UnaOS/memory/unaos-jetson-resume.md`;
`log` = `render2-boot.log`; `main.rs` = `unaos/crates/kernel/src/main.rs`; `sc64` = `unaos/crates/kernel/src/arch/aarch64/syscall.rs`.

## What the Orin runs, so the tables make sense

`kernel_main` enters `tegra_early_stop` at `main.rs:190` (`-> !`, `main.rs:2029`). Its terminus is
the single folded line `main.rs:2717`: `el1_oneshot_proof(); tegra_el0_start_maybe(); [tegradesk]
tegra_desk_arm(); [orinconwin] orin_conwin(); [orintenant] orin_tenant_arm(); [orinladder]
orin_ladder_arm(); [orinfurn] tegra_desk_furn(); [orinrender] tegra_render_arm(); tegra_rast_demo_maybe();
[orinwdt] boot_ok_disarm(); run_capstone_boot_core(0)` (default) or `run_bsp_tegra(0)` under `bsprun`.
`run_capstone_boot_core` is documented as boot-core-only, cooperative, no preemption, busy-poll
(`arch/aarch64/sched.rs:10110-10134`), and on metal prints `boot core 0 at EL1 — running the full M4
CAPSTONE cooperatively` (log:495). The Orin's only input drain is `jd2_console_pump` (`main.rs:2801`),
which services exactly `vugras::idle_sweep`, the four `orin_*_census` calls, and (at the tip, behind
`holocron`) `prtscr::service()` (`main.rs:3014`). The `orinrender` pass (`main.rs:8222`) services
`pulsewin::service()` and `ui_status::tick()` only (`main.rs:8312`, `:8316`).

Everything `kernel_main` runs below `main.rs:190` never runs on tegra. Of the services in that region
(`main.rs:1160-1300`): `fat::probe_once` (:1174), `holocron::service` (:1189), `prtscr::service`
(:1199), `wifi::service` (:1236, x86-gated), `flight_recorder::service` (:1259, `#[cfg(target_arch =
"x86_64")]`), `termring::service` (:2754), `instgui::service` (:1796). The Pi reaches this region
(`kernel_main` continues past :190 on `pi`); the Orin reaches none of it, and each tegra twin has to
be wired by hand onto the terminus or the pump.

## Table 1 — BROKEN, ranked by user-visible impact on the desktop north star

"Works on Pi / x86" is read from the gates and the docs cited in the row, not assumed.

| # | What is broken (what Peter sees / cannot do) | Evidence | Pi? | x86? | Root cause (one sentence) | Size | Lane | Blocked on |
|---|---|---|---|---|---|---|---|---|
| 1 | **No desktop furniture.** No menu bar, no dock strip, no crystal on the panel; every click on the desktop lands on nothing. The render pass runs but composites only the console window and the pulse window (`wcn wins=3`, log:909-923). | `od` §3.12 "the Orin has no furniture at all"; `menubar::set_enabled(true)` has two callers, `video/desktop_uefi.rs:552` (x86) and `video/desktop_firmware.rs:292` (inside `activate()`), neither reachable on tegra (`od:2113-2116`); `orin_render arm … furn=0` (log:489). | Yes (`pidesk`+`baremetal`, `main.rs:1316` via `pidesk_activate_maybe`) | Yes (`wc`, `desktop_uefi::activate`) | `desktop_firmware::activate()` is the only path that arms the furniture and it sits behind `TEGRADESK_CASCADE_OK = false` — the §5.2 stop-line — so the seam refuses (`od` §3.2.1 REFUSE table; §5.2). | L | jetson (`main.rs` terminus tail, `display_tegra.rs`) + shared `video/` reads | **§5.2** — needs a boot-stack high-water measurement of the cascade; `stk_probe` returns early at the terminus (`od:2117-2130`); DEPTH half taken by ORIN-STKDEPTH, HEADROOM still `DEPTH-UNAVAILABLE` and UNFLOWN (`od:3218-3223`). `orinfurn` (bar only, two of nine steps) landed default-off and never reached its seam on metal (`od` §3.12.1: desk1/desk2 parked at `main.rs:2621`). |
| 2 | **No EL0 program can own a window; `bg` is refused outright.** `run` parks the shell for the tenant's whole life, and a `bg`-launched program is refused on every core but 0. Peter cannot run vug in a window beside the shell. | `bg` → `SCHED: EL0 entry REFUSED … CurrentEL is EL2, not EL1 (EL0-EL1CORE)` (`arch/aarch64/sched.rs:1740`); "`bg /fat/vug.elf` stays REFUSED (EL0-EL1CORE)" (`od:1703`); `RM:198-199` "5× `BGRUN: rejected`, zero successful launches"; rung 6 `orintenant` UNFLOWN (`od` §3.10, §6 rung 6). | Yes (EL0 tenants on Pi, `od` §2/§6) | Yes | Only the BSP drops EL2→EL1 (`main.rs:2717` JM6); PSCI-woken APs stay at EL2 (`RM:271-274`), so EL0 dispatch is filtered to core 0 and only `run` (which pins core 0) can launch. | L | jetson (`arch/aarch64/sched.rs`, `boot`/EL2 drop) | A ruling on the SMP design (D1–D5 "Pending Peter", `RM:349`); "EVAC ruled unendorsable; do not hack it" (`RM:327`). |
| 3 | **Keystrokes ignore focus.** Every key goes to the shell regardless of which window is focused; a windowed tenant that holds focus while the shell runs is "robbed of its keystrokes". While a tenant is running under `run`, nothing pumps the controller at all unless `orininput` is armed. | `arch/aarch64/xusb_tegra.rs:1964-1974` (defects 1 and 2, verbatim); `main.rs:2916` ("this pump feeds `handle_key` REGARDLESS of focus … `xusb_tegra.rs`'s defect-2 note, out of this arc's scope"); `orininput` adds a pump only inside `run_user_image` (`xusb_tegra.rs:1984-1990`) and is knob-gated. | Yes (`route_input_to_active_el0` / `pump_usb_into_gui`, `cfg(all(aarch64, baremetal))`, `xusb_tegra.rs:1970-1972`) | Yes | The Pi's `usb_pump` task and its focus router are `baremetal`-gated and `baremetal` implies `pi`, which is a `compile_error!` with `tegra` (`arch/aarch64/serial.rs:22-23`); the Orin got a pump with no router. | M | jetson (`main.rs::jd2_console_pump` phase 2, `xusb_tegra.rs`) | The `pidesk` pseudo-ASID question (`xusb_tegra.rs:1991-1997`): routing keys into `KERNEL_OWNER_DESKTOP` would lock the operator out of the shell. Same rung as #1. |
| 4 | **Nothing can be typed at the board from the host — serial RX is dead.** Every re-run of a command costs a card write and a power cycle (ten steps, two card handlings). | `SD` §1-§2; render2 q7: one byte over the FIFO → "nothing … KEY echoes 0" (`FLIGHT-RESULT.md:16`); `tegra::read_byte` exists (`arch/aarch64/serial.rs:81-101`) but the console pump drains only `pal::next_event()` (`SD:3`). ORINRX is built in scratch (`~/unaos-bench/scratch/orin13/orinrx/kernel-on.elf`, commit-msg written) and **not in the tree** (0 hits for `orinrx` in `main.rs`, `Cargo.toml`, `arroyo`). | Yes (PL011 RX, `baremetal`, `serial.rs:402-436`) | No (x86 serial is TX-only, `SD:150`) | No caller of `arch::poll_input` on the console path (`SD:3`); a four-site fold plus an LSR probe, "backburner per Peter unless he raises it" (`batons/orin-14.md` OPEN 5). | S (+1 attended flight) | shared `main.rs` (grant precedent `:2102/:2517/:2717`), jetson `serial.rs`, `xusb_tegra.rs` | Peter's backburner ruling; `BASE=0x0C28_0000` is "TO VERIFY ON THE BOARD" (`serial.rs:28-48`) — the flight's negative answer is itself the deliverable (`SD` §5). |
| 5 | **The status strip and the pulse window paint once and freeze.** `presents=2` for the entire 98 s boot; the pulse window opens labelled `view=Pi LED lamps` on an Orin. | log:924 `presents=2`; log:528 `[pulsewin] open … view=Pi LED lamps`; log:594/658/763/869 `win=3 att=0 comp=1`; log:507/510 `[pulse5] live … span_max=0ms`; log:509 `SCHED: load c0=100% c1..c5=0%`. | Yes (`pulsewin` gated `all(aarch64, desktop_firmware)`, `video/mod.rs:668-670`; the Pi's render task dirties from `shell_console`) | Yes (own path) | *(inference)* `ui_status::tick` dirties only when the composed line's FNV hash changes (`ui_status.rs:1079`, `hash` at `:566`); its aarch64 source is `sched::core_load(cpu).busy_pct_recent` (`:553`), which on a single-core cooperative terminus reads a constant `c0=100%`, so the line never changes and nothing repaints. The label is `View::Lamps => "Pi LED lamps"` (`video/pulsewin.rs:143`), a board name in shared code. | S–M | jetson (`main.rs:8222` pass) + shared `ui_status.rs`/`pulsewin.rs` for the label (GATE-NEUTRAL, `RM:31`) | Nothing structural; the LOADSAMPLER executor left only a `check.log`, no report. #6 (preemption) is the real fix if the inference holds. |
| 6 | **One core, no preemption.** Five APs come online, run zero work, and the boot core busy-polls; a task that sleeps on core 0 is never woken (the `run /fat/stat.elf` hang suspected in `RM:284-287`). Under load the panel and the shell share one cooperative core. | log:495 (cooperative), log:498 `0 steals total across 5 online cores, 0 core(s) ran work`, log:509 `c0=100%`; `sched.rs:10126-10133` "No preemption … busy-polls"; `sched.rs:3153-3157` "`run_capstone_boot_core` has no `drain_due_sleepers` and never sets `SCHED_ACTIVE`, so a task forced there that then SLEEPS … is never woken"; `wcpar cores=1` (log). | Yes (preemptive `run()` on 4 cores) | Yes | The tegra terminus is the JC3 QEMU-proof loop, not the scheduler's `run()`; the `bsprun`+`bsptick` knobs that would fix it are "QEMU/build only — not yet flown on metal" (`arch_arm64.md:10659`, `:10752`). | M (mechanism exists) + 1 flight | jetson (`sched.rs` tail, `timer.rs` tail) | SMP ruling (D1–D5, `RM:349`); the ~30% ORIN-SMP-3-PARK RAS abort inside the first `CPU_ON` (`SD:135`, `od` §3.12.1) makes every SMP flight a coin toss. |
| 7 | **The dock's minimise disc has never been shown to bring a window back**, and the pinned shell tile is a dead button on aarch64. The console window's minimise control is a possible one-way trip (§6.1's exact hazard). | `[dock] … presses=0` on every dock line (log:909); `od` §3.9.1 "the minimise disc was never clicked"; `od` §3.11 rung (b) UNFLOWN; `AC` #21: `dock::press_at` latches `SHELL_REOPEN` (`video/dock.rs:997-1003`), sole drain at `main.rs:6763` inside `#[cfg(target_arch = "x86_64")] fn x86_render_service` (`main.rs:6322-6323`) — confirmed at tip: only x86 callers of `take_shell_reopen`. | Partly (Pi has the dock; its shell tile is equally dead, `AC` #21) | Yes | No aarch64 consumer of the reopen latch; the round trip has no metal witness. | S (drain) + 1 flight | shared `main.rs` (Pi render task) + jetson pass | A click on metal (`orinladder` flight card, `od:2032-2045`). |
| 8 | **No file browser.** `quarry` is compiled on tegra now (`arm-tegra-desk` leg carries `quarry`, `arroyo:2934`; `uslots` fix in `video/quarry/live.rs:1167`) but is `desktop_firmware`-gated (`video/mod.rs:684-685`) and nothing on the terminus opens it. | `video/mod.rs:684-685`; `od` §1 row "Quarry ❌ REACHABLE ❌ PROVEN"; no `quarry` call on `main.rs:2717`. | Yes | Yes | Same as #1 — quarry opens from the furniture/cascade the stop-line forbids. | M | jetson + shared `video/quarry` | §5.2 (via #1). |
| 9 | **Print Screen does nothing on the card that is in the board.** The key edge is decoded (`xhci/mod.rs:4927`) but the service call landed at the tip (`2000608a`, `main.rs:3014` under `holocron`) after render2 was written; the jetson image does not carry `holocron` by default (`arroyo:506`). | `batons/orin-14.md` OPEN 2; `main.rs:3014` comment "on tegra the flag was armed and never serviced"; PRTSCR-ORIN commit date 2026-09-05 vs card `8085c9c8` 2026-09-03. | Yes (`main.rs:1689`, ungated at all three storage-ready passes) | Yes | Fixed in source, UNFLOWN; needs `UNAOS_HOLOCRON=1` and the FAT program-source handle verified at press time (baton item 2c). | S (flight) | jetson | A card write + one flight. |
| 10 | **`<Esc>` cannot dismiss the pulse window's View menu** (the menu the `view=Pi LED lamps` line invites Peter to click). | `AC` #20; at tip `pulsewin::key_escape` (`video/pulsewin.rs:902`) still has zero callers — the aarch64 router `sc64:13211` calls only `crystal::key_escape`; x86 `:6739` likewise. | No (same router) | No | Dead `pub` fn whose own doc asserts a caller (`pulsewin.rs:901`). | S | shared `sc64` router line + x86 twin | None (needs a `wc_route_event`-position call on both arches). |
| 11 | **TAB mid-drag leaves the grab steering the old window** on aarch64 (x86 cancels the drag). | `AC` #7; at tip `sc64:13354 fn wc_focus_key` body contains no `drag` (grep 13354-13400: 0 hits) vs `arch/x86_64/syscall.rs:5331-5334` `drag_cancel("focus-key")`. | No (shared aarch64 twin) | Yes | One-sided fix on the x86 twin never mirrored (`AC` §1). | S | shared `sc64` (Pi + Orin) | A pi-seat grant (file is Pi-lane per `AC` #6 precedent: CAPREVOKE needed pi 6's grant). |
| 12 | **No DHCP lease; no network.** A net-armed boot sends DISCOVER, receives nothing, and falls back to a static `192.168.1.2/24`. Default jetson image carries no `net4` at all (0 net lines in render2). | `arch_arm64.md:9079` `NO-OFFER`, `:9086` "falling back to static 192.168.1.2/24"; `NET-4A` buffer-17 latch (`RM:356-357`); `ROADMAP.md:343` "Orin: NET-4 = NIC claim + smoltcp bind" open. | No (Pi networking also open, `ROADMAP.md:343`) | Yes (`smolnet` in the trunk battery banner, `LANDING-REPORT.md`) | RX ring dies after one pass / single-address latch inside the NIC or RC (`arch_arm64.md:8796-8830`). | L | jetson (`rtl8168_tegra.rs`, `pcie*`) | A ruling on priority vs the desktop; not on the desktop's critical path. |
| 13 | **No `flight_recorder`** (post-mortem ring) on aarch64 at all. | `main.rs:1257-1259` `#[cfg(target_arch = "x86_64")] flight_recorder::service()`; `:1747` "That function is x86-only". | No | Yes | x86-only service; the Pi lacks it too. | M | shared `flight_recorder.rs` + `main.rs` passes | Not desktop-critical. |
| 14 | **No Wi-Fi.** | `main.rs:1236` `#[cfg(all(x86_64, wifi))]`. | No | Yes (b43) | No driver; out of the north star. | L | — | — |

Deliberately NOT listed as broken: FAT read/write on the boot volume (`JD3 — mass storage ready;
panel shell ls/cat live`, log:390; write path ADMITTED and metal-corroborated, `SD:116`); the panel
(JD1/JD2/JD20, `od` §1); keyboard and pointer reaching the pump (log:389 `1 keyboard(s), 2
pointer(s) armed`); click routing into the window layer (`od` §3.8.1, boot7g); the console as a
routed window (`od` §3.9.1, boot7h); the render pass and its six audited defects (`od` §3.14, render2);
the Kepler-class GPU (out of scope by ruling, `od` §4).

On the render2 pointer question the brief asked: **a mouse was present** — two HID pointers
enumerated (slot 4 relative boot-mouse, log:318; slot 5 keyboard+absolute pointer) and armed
(log:389). `[orinclick] … btn=0 press=0 … -> IDLE-NO-CLICKS` on all nine censuses (log:923) means
nobody clicked during the 98 s sitting, not that the route failed; the route was proven on boot7g
(`od:3237`). Likewise `KEY` echoes are 0 because nothing was typed.

## Table 2 — NOT BROKEN, JUST UNFLOWN (landed, gated green, never on Orin metal)

| Item | Landed | What the flight would answer | Evidence |
|---|---|---|---|
| PRTSCR-ORIN — Print Screen service on the pump | `2000608a` (tip) | does a PNG land on the FAT from a lone Print Screen press with `UNAOS_HOLOCRON=1`? | `main.rs:3014`; card is `8085c9c8` |
| TABKEY — `<TAB>` reaches the focus ring from the shell door | `ac9c0701` 2026-08-31 | `KEY 0x09` immediately followed by `[wc-c] focus tab-cycle` (a regressed board reads `KEY 0x09` alone) | `main.rs:2916` fold; measured defect `orin.log:13071-13080` (5× KEY 0x09, 0× tab-cycle) |
| Rung 6 `orintenant` — EL0 window tenants (CRYSTAL-HD geometry parity) | `od` §3.10 | `run /fat/vug.elf` → `[orintenant] create … surf=288x288 wm-bound=1 -> TENANT-WINDOW`; `FAIL reason=geometry-refused` = wrong media | `od:1691-1704`; desk5 carried `[orintenant]` census lines only (`RM:420`) |
| `orinladder` rung (a) — glyphs-on-glass read-back of the console window | `od` §3.11 | `[oringlass] … -> GLYPHS-ON-GLASS` six probes | `od:1989-2000` |
| `orinladder` rung (b) — the dock minimise round trip | `od` §3.11 | click the disc at the printed `(X+D/2, Y+D/2)` → `CONSUMED`, `[wm-act] minimise`, then the dock tile unhides it | `od:2032-2045` |
| `orinfurn` — the menu bar half of rung 5 | `od` §3.12 | `[orinfurn] ARMED … -> BAR-ON-GLASS` with a non-`None` `rect=`; a crystal press consumed by the band | `od:2245-2260`; desk1/desk2 parked before the seam (`od` §3.12.1) |
| `tegradesk` seam floors (rung 2) | `od` §3.2.1 | `[deskseam] floors …` + `REFUSE reason=stop-line-5.2` printed on an armed boot | `od:493-494` "No Orin boot has been taken with `UNAOS_TEGRADESK=1`" |
| ORIN-STKDEPTH / PANELOWN / PANELREFUSE / supstate / `live=` read-back (five terminus instruments) | `od` §3.13 | any number at all — "no `[orinstkdepth]` number exists … no `[panel-owner]` line has been seen on Orin metal" | `od:2955-2970` |
| `bsprun` + `bsptick` — the boot core joins preemptive `run()` | `arch_arm64.md:10603`, `:10673` | does the terminus survive a periodic EL1 tick and preempt; does `SCHED-BAL` show steals > 0 | `arch_arm64.md:10659`, `:10752` "not yet flown on metal" |
| REDZONE absorber on Orin | `f0106408` | a `[redzone]` line ever firing (render2's 0 lines = "never fired", not "held") | `od:3096`, `:3126-3128` |
| ORINRX — serial RX drain + LSR probe | scratch only (`orin13/orinrx/`), commit-msg written, not committed | `[orinrx] lsr=… -> RX-LIVE / RX-ZERO / RX-DEAD / RX-OPENBUS`, and `reboot` fired for the first time | `SD` §3 A1; 0 hits for `orinrx` in the tree |
| `reboot` verb (PSCI `SYSTEM_RESET`) | in tree, ungated (`shell.rs:4084-4087` → `power.rs:38`) | whether ATF honours it or the board goes dark | `SD:79` "never been fired on any board" |
| NET-4 fix (ISR.RDU / ring re-arm) | folded (`RM:338-339`) then the theory was refuted on boot7h (`RM:355-356`) | a lease, or the buffer-17 latch reproduced | `arch_arm64.md:9079-9086` |
| Persistence of a composited window body between composites now that `orinconwin` subtracts occluders | `od` §3.9 | a post-routing probe of win=1 after a click recomposites it | `od:3411-3424` "unmeasured" |

## Open ARCH-CONFORMANCE findings that still affect the Orin (of 24 ledgered)

Fixed at tip: #2 (32 KiB stack), #3 (back-buffer seed), #4 (`pulsewin::arm`, `main.rs:8178`), #6
(CAPREVOKE), #15 (CNTPCT census), #16 (`DECLINE reason=no-painter`), #22 (`reserve_stage`) — all
flown on render2 except #6. Still open and Orin-affecting: **#1** (knob→leg coverage check cannot
fire — `arroyo:3978` still iterates `_rows`; rmbp's KNOBLEG fix "on hw-rmbp, not yet in trunk",
`LANDING-REPORT.md`), **#7** (Table 1 #11), **#8/#9** (three service passes, five shell loops — each
Orin service is wired by hand, which is how #4, #9, #13 happened), **#12** (`test-arm` negative-only;
the trunk battery notes "3 positive witnesses" counted by hand), **#20** (Table 1 #10), **#21**
(Table 1 #7), **#5/#10** (twin ledger — the mechanism by which #7 and #11 were born), **#14** (nine
mbench specs replayed by nothing; `jetson-sync1.spec` has no green reference until a post-`405b21f6`
image flies, `RM:179-180`). #11/#13/#17/#18/#19/#23/#24 are doc or non-Orin.

## Proposed ORDER for the next arcs

The ranking principle: the top of Table 1 is one blocker (§5.2) wearing four costumes (#1, #3, #8,
and the cascade half of #7), and §5.2 is blocked on a *measurement*, not a design. Two of the top
six (#4, #6) are the reasons every measurement costs a card write and a coin-toss boot. So the order
is: make flights cheap, take the one measurement the stop-line asks for, then arm the cascade.

**Arc A — ORINRX + BSPRUN (make the board answer questions).** 3 milestones.
A1: land the ORINRX drain (four-site fold, `orinrx` knob, LSR probe) from `orin13/orinrx/` with its
`arm-tegra-rx` leg and go-red proof; rmbp grant for `main.rs` recorded. A2: build the
`bsprun`+`bsptick` image with the redzone witness ungated, plus the `[orinclick]`/`[orintenant]`
censuses so the flight is scoreable. A3: one attended flight, three questions in order: `printf
'help\r'` over the FIFO → `KEY` echo (`RX-LIVE` or which of the three negatives); `reboot` fires (ATF
honours `SYSTEM_RESET` or the jack comes out); then on the bsprun image, does the terminus survive a
periodic tick and does `SCHED-BAL` show any core other than 0 running work.
*Metal question:* **can a byte typed on the host reach the Orin's shell, and can the boot core run
preemptively?** Answers Table 1 #4 and #6, and if `presents` climbs past 2 on the bsprun image it
answers #5 for free (the inference in row 5 becomes a measurement).

**Arc B — STACKHEADROOM (clear §5.2 honestly).** 3 milestones.
B1: a boot-stack high-water probe that works before `run_capstone_boot_core` drives the queue —
`stk_probe` returns early with no current task (`od:2117-2130`); the fix is either a boot-stack
variant reading the linker's stack extent or moving the arming point onto a task stack (a
`spawn_stack` task like `orin-render`, which is already measured at `hw=22256` on a 32 KiB stack,
log:541). B2: fly `UNAOS_TEGRADESK=1` alone to get the `[deskseam] floors` line and the refusal — the
rung-2 floors have never printed on metal. B3: fly `orinfurn` on a clean (non-parked) boot with the
new probe at the `[orinfurn] arm` line.
*Metal question:* **how deep is the desktop-arming cascade on this board's stack?** — the one number
§5.2 has asked for since 2026-08-22 (`od:3152`, `:3216-3223`). Answers whether Table 1 #1 is a
flip of `TEGRADESK_CASCADE_OK` or a stack-size change first.

**Arc C — THE CASCADE (rung 5 proper).** 4 milestones, gated on B's number.
C1: flip `TEGRADESK_CASCADE_OK` to `cfg!(orindesk-cascade)` on a knob, arm `desktop_firmware::activate()`
from the seam on a sized task stack, with the DESKTOP-CLEAR's "table is empty" premise handled
(`REFUSE reason=table-not-empty`, `od:363`). C2: the aarch64 drain of `dock::take_shell_reopen`
beside the Pi's render task (Table 1 #7 / `AC` #21) and `pulsewin::key_escape` in the router
(Table 1 #10 / `AC` #20) — both S, both shared-file, both need a pi grant. C3: fly with `orinladder`
(rung (a) glyphs, rung (b) the dock round trip) on the same image. C4: doc `od` §3.15, §6 rung 5 row.
*Metal question:* **does the Orin come up to a desktop with a bar, a dock, and a console window whose
minimise disc is not a one-way trip?** Answers Table 1 #1, #7, #8.

**Arc D — INPUT OWNERSHIP (keys follow focus).** 3 milestones.
D1: decide the `KERNEL_OWNER_DESKTOP` pseudo-ASID question the `xusb_tegra.rs:1991-1997` note defers
(a focus that owns no ring must fall through to the shell, never lock the operator out). D2: fold a
`user_input_active() != 0` route into `jd2_console_pump` phase 2 through the same
`user_input_enqueue` seam `orininput` already uses, defaulting `orininput` on. D3: fly `run
/fat/vug.elf` on the four-knob conjunction (`od:1693-1694`) and type into it.
*Metal question:* **does a keystroke reach a focused EL0 window instead of the shell?** Answers Table
1 #3, and lands rung 6's first `TENANT-WINDOW` on the way (the two flights share an image).

**Arc E — EL1 APs + `bg` (multi-core desktop).** 3 milestones; needs Peter's SMP D1–D5 ruling first.
E1: drop the APs to EL1 on the PSCI entry path (the mirror of JM6 on the BSP), keeping EL0-EL1CORE's
filter as the fail-closed guard. E2: `drain_due_sleepers` on whichever loop core 0 runs (bsprun
already has it via `run()`, `sched.rs:6447`). E3: fly `bg /fat/vug.elf` and `run /fat/stat.elf`.
*Metal question:* **can an EL0 program run in the background on a core that is not the boot core,
and does a sleeping task wake?** Answers Table 1 #2 and the sleeper half of #6.

Not sequenced above, by design: #12 network (a different north star), #13/#14 (x86-only by nature),
and the `render_service` convergence arc (structural, no user-visible change; it is the *cure* for
`AC` #8/#9 and should ride behind Arc C once the Orin member has its input half so the abstraction
is designed from a complete instance rather than "the thinnest member", `LANDING-REPORT.md` GATE-FAMILY
part 1).
