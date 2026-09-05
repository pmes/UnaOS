# rMBP ledger — every finding, ticked once

> Arch ledger for the 2012 rMBP (x86_64) track. Cross-arch and shared-seam items live in
> `docs/dev/LEDGER.md` (ids `S<n>`, `P<n>`); rows below cross-reference it by id.

> Rule (`docs/dev/LAWS.md` §Ledgers, Peter 2026-09-05): **an arc that fixes, flies, or drops a ledger
> item ticks it here in the same commit.** Every audit, inventory, or capture review folds its findings
> into this table the turn it lands. Before spawning any audit, brief it with this file: report only
> what is NEW or CHANGED. A finding re-derived because it was not here is the waste.

Status: **open** · **fixed-unflown** (in tree, not on metal) · **flown** (scored on the wire) ·
**dropped** (ruled out, with the ruling) · **relayed** (another lane owns it) · **design** (needs a
design before a patch).
Sources: `F6` = `~/unaos-bench/scratch/rmbp11/FLIGHT6-POSTMORTEM.md` · `F7` = `…/FLIGHT7-POSTMORTEM.md` ·
`B11`/`B12` = batons `rmbp-11.md` / `rmbp-12.md` · `PR` = `~/unaos-bench/scratch/rmbp10/PANELREFUSE-REVIEW.md` ·
`LR` = `~/unaos-bench/scratch/rmbp11/LANDING-REPORT.md` · `MANIFEST` = `~/unaos-bench/flash/rmbp/MANIFEST`.

## A. What the operator sees (rMBP metal, ranked by what it costs at the keyboard)

| id | item | status | evidence | closed by |
|---|---|---|---|---|
| A1 | BAR1 wedge under paint bursts (`storm`): a core dies mid-blit; REHOME + shell re-mint recover the desktop on the remaining cores, the wedged core stays dead for the boot | open — convicted, unfixed; recovery **flown** (F6 boot 1: c1 at 224 s, `GATE STOLEN … REHOMED … DEAD c1`) | F6; `[pcih] rp-at-wedge lnksta=d081` | — |
| A2 | Screenshot costs 70 s with keyboard and mouse dead: the 15.5 MB PNG is written inside the device-service pass (`[deadman] pmp=0` for the whole write, ~220 KB/s) | open — the number to beat | F6 (70.6 s, 70.5 s); `screenshot.md` §9 | incremental/async capture arc (B12 P3) |
| A3 | `reboot` verb resets the machine (FADT RESET_REG 0xcf9 ← 0x6) but NONE of the ladder's witnesses reach the console: `raw_write_str` is the 16550 at 0x3F8, which this laptop lacks; the reset lands before the xHCI pass drains the FTDI mirror ring | **design** — BOOTFADT (`857c6dc8`) prints the facts at boot instead; a synchronous FTDI drain needs the xHCI lock on a LOCKFIX path | F6 (zero `[orinreboot]` lines, 3 boots); F7 (`value=0x6`) | B12 P2 |
| A4 | Unattended reboots into UnaOS impossible: firmware boots macOS unless ⌥ is held at power-up | open — not kernel work; card must become the default startup volume (macOS `bless`/Startup Disk); Peter's call on his laptop | Peter 2026-09-03; F6 | — |
| A5 | Shell window tears under storm: `[wc-h] win=2 torn=111 banded=13085` while the eight vug windows sit at torn 3–10; `[wcser] declined_pct=51 -> SERIAL`, `[wc-w] amp=1.28x -> WIDENED` | open — measured, not judged (Peter saw "vug tearing"; the counter says the shell) | F6 boot 1 | — |
| A6 | `[clickroute] route … kernel=true desktop=false nofab=true -> FAIL` deterministic on metal, green in QEMU | open — bracket question (new-with-arc or pre-existing?) | flight 4/5 playbooks | — |
| A7 | GMUX switch to the iGPU does not persist (switches and restores on one call stack) | open — `GMUX_SWITCH_EXTERNAL=0x01` is the blocker | B11 P6 | — |
| A8 | BT inquiry deafness is boot-scoped (deaf boot 1, hears boot 2); SSP→A2DP `DISCOVER` unanswered | open | flight 4/5 | — |
| A9 | Serial console is TX-only (FTDI bulk IN 0x81 never driven) — no typing over the wire, no self-driving loop | open — ranked first transport for DEV-LOOP (~11.5 KB/s) | B11 DEV-LOOP | — |

## B. Kernel / x86 lane defects and gaps

| id | item | status | evidence | closed by |
|---|---|---|---|---|
| B1 | LOCKFIX gap: ONE input-band site `arch/x86_64/syscall.rs:6362 click_pointer_pos` takes `WRITER.lock()` blocking; the other four "sites" are `winx_launcher` selftests. Plus the furniture's five masked blocking acquisitions in the present tail (`strip.rs:377/:451`, `dock.rs:729`, `menubar.rs:768`, `crystal.rs:784`) | open | B11 P1 (chain derived end to end) | — |
| B2 | `wc_shim` is an INLINE mod at `syscall.rs:4777` — a filename search finds nothing | standing note | B11 | — |
| B3 | 5 x86-side features uncovered by any board leg (`nvidia-kepler-kdisp-hold rtpi rtwit selfhost vugras`) | open — recorded, not judged | KNOBLEG coverage 142/152 | — |
| B4 | 2 cosmetic `unsafe` warnings at `bootloader/src/main.rs:1340/:1389` under `unaos_ivb` | open, cosmetic | GATE-ROOTS leg 4 | — |
| B5 | `panel_info_nonblocking` HAS x86 callers (`video/quarry/live.rs:1698/:2324`, armed by `UNAOS_QUARRY=1`) — the "x86 has no non-blocking door" premise was false | corrected | B11 (C1 provenance lesson) | — |
| B6 | Cross-arch splash: `splash.rs` stays, call sites x86-gated; the `bootpace.rs:168` same-line trap | open (Peter asked by name) | B11 P2 | — |
| B7 | vug arbiter placement (kernel-side recommended) | **blocked on Peter's call**, still unasked in his hearing | B11 P3 | — |
| B8 | Wired NIC is Broadcom `0x14e4`, nothing drives it; `e1000` is QEMU-only; Peter's two dongles are identical `0b95:1790 ASIX AX88179B` (one USB-ethernet driver would serve every board; `drivers/xhci/ftdi.rs` is the template) | open — DEV-LOOP transport ranking | B11 | — |

## C. Shared-file items this lane OWNS (mirror of `LEDGER.md`; the S-row is authoritative)

| id | item | status | evidence | closed by |
|---|---|---|---|---|
| C1 (→ S1) | Orin devkit hubs fail status-change EP configure (codes 17 / 8); `hub_int_ep`=0 gates hot-plug off all boot. Read: interval/ESIT math in the endpoint context (`drivers/xhci/mod.rs:13424-13462`, gates `:4241/:4610`) | open — fix ships to orin as a sha, flies in their next image | RENDER2-AUDIT N2 | — |
| C2 (→ S2) | `MOUSE-1` prints `vid:pid=0000:0000` for hub-attached pointers (`xhci/mod.rs:4356` reads root-port-only slot fields) | open, cosmetic | RENDER2-AUDIT N3 | — |
| C3 (→ S4) | `video/dock.rs` `SHELL_REOPEN` drained only by `x86_render_service`; the shell tile is dead on aarch64 | open | orin A7 | — |
| C4 (→ S5) | FC-2 structural check unbuilt: unconditional `pub mod` whose consumers all sit under one arch gate (`flight_recorder`, `dock`, `pulsewin`) | open | orin-12 baton | — |
| C5 (→ S6) | GATE-NEUTRAL: board names out of arch-neutral code. x86-measured exposure: `drivers/xhci/mod.rs` 75 `[piusb NN]` sites (17 strings in the x86 kernel; `[piusb40]`/`[piusb41]` PRINTED on the rMBP wire in F7), `main.rs` `[orinfurn]`×9/`[orinclick]`×2/`[piusb]`×3 + 17 `tegra_*` fns, `video/fbcon.rs` `[orinface]`×4, `video/desktop_firmware.rs` `[pidesk]`×14 ungated; identifiers in `drivers/block.rs` 6, `drivers/gpu/mod.rs` 3, `smolnet.rs` 2, `fs/fat.rs` 2 | open — **B12 P0**: ratchet ledger + go-red by mutation; renames per family by ack | F7; memory `name-by-subsystem-not-board` | — |
| C6 (→ S8) | `scan_serial_faults` passes on a MISSING log (`arroyo:2239`); `test`/`test-arm` are negative-only | open — GATE-BLINDNESS | B11 | — |
| C7 (→ S9) | KNOBLEG `647f485a` — knob→leg check can fail now | fixed, **unlanded** | rmbp landing | rmbp landing |
| C8 (→ S10) | GATE-ROOTS `e1bff790` — every binary a named check root | fixed, **unlanded** | rmbp landing | rmbp landing |
| C9 (→ S12) | Core-0 load accounting on x86: **NOT the tegra bug.** The x86 meter is TSC-time-based (`arch/x86_64/sched.rs` "Per-core busy-TIME accounting"), `core_load` folds busy AND idle spans, and since NO-RESERVING-CORES core 0 folds a span every pass (`sched.rs:5411`). Wire: F6 c0 = 5%/12%/12% idle → 99%/88%/96% under storm; F7 idle boot c0 = 6%. A structural 100% cannot print 5%. | **checked — dropped for x86** (2026-09-05) | F6/F7 `[schedx86] load` lines | this row |
| C10 (→ S7) | `render_service` family size 3 with an expiry (convergence arc owed; orin proposes) | open — ledger grown in merge `8c559329` with orin's three-part answer | merge body | convergence arc |
| C11 | PWRNAME `bc10a469` (`[pwrreboot]`/`[pwrshutoff]`/`[orinwdt]`) and BOOTFADT `857c6dc8` — on hw-rmbp, **unlanded**; trunk still prints `[orinreboot]` until the rmbp landing | fixed, flown (F7), unlanded | LR | rmbp landing |

## D. Bench and process (this track's instruments)

| id | item | status | evidence |
|---|---|---|---|
| D1 | `line-butler.py` has NO x86 stream; `NVIDIA` in ORIN_MARKS mis-routed 81% of an rMBP boot into `orin.log` (fixed by orin's BUTLERMARK). Never point the butler at the rMBP's FTDI | standing | B11 |
| D2 | The FTDI re-enumerates (`ttyUSB0`→`ttyUSB1` on 2026-09-03); identify by `/dev/serial/by-id/usb-FTDI_*`, kill bridges by device | standing | F6 |
| D3 | `stage-x86.sh` is two-pass by design (tree first, playbook names it, re-run with the same `--stamp`) | standing | F6/F7 staging |
| D4 | Score what BOOTED by the `WXN-x86 img=[…)` span vs `readelf -lW` LOAD span (F5 0x1351290 · F6 0x134f198 · F7 0x134fca8); the wire prints no sha | standing | F6 |
| D5 | The reboot ladder's post-verb tail is lost on the rMBP by construction — never score its absence as a failure (see A3) | standing | F6 |
| D6 (→ P1) | Executor worktrees cut from `main`, not the track tip | standing | rmbp-10 |
