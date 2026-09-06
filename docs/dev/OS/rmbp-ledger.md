# rMBP ledger — every finding, ticked once

> Arch ledger for the 2012 rMBP (x86_64) track. Cross-arch and shared-seam items live in
> `docs/dev/LEDGER.md` (ids `S<n>`, `P<n>`); rows below cross-reference it by id.

> Rule (`docs/dev/LAWS.md` §Ledgers, Peter 2026-09-05): **an arc that fixes, flies, or drops a ledger
> item ticks it here in the same commit.** Every audit, inventory, or capture review folds its findings
> into this table the turn it lands. Before spawning any audit, brief it with this file: report only
> what is NEW or CHANGED. A finding re-derived because it was not here is the waste.

Sources (all in git): `F6` = `docs/dev/evidence/rmbp11/FLIGHT6-POSTMORTEM.md` · `F7` = `docs/dev/evidence/rmbp11/FLIGHT7-POSTMORTEM.md` ·
`LR` = `docs/dev/evidence/rmbp11/LANDING-REPORT.md` · `PR` = `docs/dev/evidence/rmbp10/PANELREFUSE-REVIEW.md` ·
`B11`/`B12` = batons `rmbp-11.md` / `rmbp-12.md` (seat-local; ids only) · MANIFEST = the bench media record (seat-local).
Columns: `owner` = the lane whose file changes (orin · pi · rmbp · shared-gate); `flies-on` = the bench that scores it.
Status enum (GATE-LEDGER): **open** · **fixed-unflown** · **flown** · **landed** · **dropped** — free text after " — ".

## A. What the operator sees (rMBP metal, ranked by what it costs at the keyboard)

| id | item | owner | flies-on | status | evidence | closed by |
|---|---|---|---|---|---|---|
| A1 | BAR1 wedge under paint bursts (`storm`): a core dies mid-blit; REHOME + shell re-mint recover the desktop on the remaining cores, the wedged core stays dead for the boot | rmbp | rmbp | open — convicted, unfixed; the recovery path is metal-proven (F6 boot 1: c1 at 224 s, `GATE STOLEN … REHOMED … DEAD c1`) | F6; `[pcih] rp-at-wedge lnksta=d081` | — |
| A2 | Screenshot costs 70 s with keyboard and mouse dead: the 15.5 MB PNG is written inside the device-service pass (`[deadman] pmp=0` for the whole write, ~220 KB/s) | rmbp | rmbp | open — the number to beat | F6 (70.6 s, 70.5 s); `docs/dev/OS/08_VIDEO/screenshot.md` §9 | incremental/async capture arc (B12 P3) |
| A3 | `reboot` verb resets the machine (FADT RESET_REG 0xcf9 ← 0x6) but NONE of the ladder's witnesses reach the console: `raw_write_str` is the 16550 at 0x3F8, which this laptop lacks; the reset lands before the xHCI pass drains the FTDI mirror ring | rmbp | rmbp | open — design needed: BOOTFADT (`857c6dc8`, flown F7) prints the facts at boot instead; a synchronous FTDI drain needs the xHCI lock on a LOCKFIX path | F6 (zero `[orinreboot]` lines, 3 boots); F7 (`value=0x6`) | B12 P2 |
| A4 | Unattended reboots into UnaOS impossible: firmware boots macOS unless ⌥ is held at power-up | rmbp | rmbp | open — not kernel work; the card must become the default startup volume (macOS `bless`/Startup Disk); Peter's call on his laptop | `docs/dev/RULINGS.md` R3; F6 | — |
| A5 | Shell window tears under storm: `[wc-h] win=2 torn=111 banded=13085` while the eight vug windows sit at torn 3–10; `[wcser] declined_pct=51 -> SERIAL`, `[wc-w] amp=1.28x -> WIDENED` | rmbp | rmbp | open — measured, not judged (Peter saw "vug tearing"; the counter says the shell) | F6 boot 1 | — |
| A6 | `[clickroute] route … kernel=true desktop=false nofab=true -> FAIL` deterministic on metal, green in QEMU | rmbp | rmbp | open — bracket question (new-with-arc or pre-existing?) | flight 4/5 playbooks (MANIFEST lines 635–636) | — |
| A7 | GMUX switch to the iGPU does not persist (switches and restores on one call stack) | rmbp | rmbp | open — `GMUX_SWITCH_EXTERNAL=0x01` is the blocker | B11 P6 | — |
| A8 | BT inquiry deafness is boot-scoped (deaf boot 1, hears boot 2); SSP→A2DP `DISCOVER` unanswered | rmbp | rmbp | open | flight 4/5 (MANIFEST lines 635–636) | — |
| A9 | Serial console is TX-only (FTDI bulk IN 0x81 never driven) — no typing over the wire, no self-driving loop | rmbp | rmbp | open — ranked first transport for DEV-LOOP (~11.5 KB/s) | B11 DEV-LOOP | — |

## B. Kernel / x86 lane defects and gaps

| id | item | owner | flies-on | status | evidence | closed by |
|---|---|---|---|---|---|---|
| B1 | LOCKFIX gap: ONE input-band site `arch/x86_64/syscall.rs:6362 click_pointer_pos` takes `WRITER.lock()` blocking; the other four "sites" are `winx_launcher` selftests. Plus the furniture's five masked blocking acquisitions in the present tail (`strip.rs:377/:451`, `dock.rs:729`, `menubar.rs:768`, `crystal.rs:784`) | rmbp | rmbp | open | B11 P1 (chain derived end to end) | — |
| B3 | 5 x86-side features uncovered by any board leg (`nvidia-kepler-kdisp-hold rtpi rtwit selfhost vugras`) | rmbp | — | open — recorded, not judged | KNOBLEG coverage 142/152 | — |
| B4 | 2 cosmetic `unsafe` warnings at `bootloader/src/main.rs:1340/:1389` under `unaos_ivb` | rmbp | — | open — cosmetic | GATE-ROOTS leg 4 | — |
| B6 | Cross-arch splash: `splash.rs` stays, call sites x86-gated; the `bootpace.rs:168` same-line trap | rmbp | rmbp | open — Peter asked by name | B11 P2 | — |
| B7 | vug arbiter placement (kernel-side recommended) | rmbp | — | open — blocked on Peter's call, still unasked in his hearing | B11 P3 | — |
| B9 | `[ptrdead] … fpop3=1 -> FAIL` in `UNAOS_WC=1 ./arroyo test-fat sf 200`: the known foreign-drain flake in `arch/x86_64/syscall.rs` fired on 1 of 2 runs of orin 14's A17 proof (run 1 exit 1, ~1000 lines before the first chord; run 2 clean). A gate that fails one run in two on an unrelated leg costs a re-run per proof | rmbp | — | open — flake rate unmeasured; prior fixes 0d509431 / badc8732 did not close it | orin 14's PRTSCR2 commit body (`f0db58bf`, hw-jetson) | — |
| B10 | Shut-out register for the rMBP GPU ladders (RULINGS R19, Peter 2026-09-05: a path that failed once gets shut out, and many boots later it turns out later paths need it open). Re-read every Kepler/gen7 rung recorded as failed (KEPLER-METAL-LOG, the gen7 R1–R7 wake ladder, the GMUX/iGPU probes) as "failed under <conditions>"; keep code and knobs; add a dependencies column so a later rung names the earlier one it needs open | rmbp | rmbp | open — not started; the compile is a reading task before any code | `docs/dev/RULINGS.md` R19; flight 6/7 gen7 lines (`r2 verdict=gt-still-dark`, `tlb-flush-write-silent`) | — |
| B8 | Wired NIC is Broadcom `0x14e4`, nothing drives it; `e1000` is QEMU-only; Peter's two dongles are identical `0b95:1790 ASIX AX88179B` (one USB-ethernet driver would serve every board; `drivers/xhci/ftdi.rs` is the template) | rmbp | rmbp | open — DEV-LOOP transport ranking | B11 | — |

## C. Shared-file items this lane owns — LINKS ONLY (the `docs/dev/LEDGER.md` S-row is the one home and carries the status)

| link | summary | where the work is |
|---|---|---|
| → S1 | Orin devkit hubs fail status-change EP configure (codes 17 / 8); `hub_int_ep`=0 gates hot-plug off all boot; read: interval/ESIT math (`drivers/xhci/mod.rs:13424-13462`, gates `:4241/:4610`) | rmbp code, flies on orin's bench |
| → S2 | `MOUSE-1` prints `vid:pid=0000:0000` for hub-attached pointers (`xhci/mod.rs:4356`) | rmbp code |
| → S4 | `video/dock.rs` `SHELL_REOPEN` drained only by `x86_render_service` | rmbp code, flies on pi/orin |
| → S5 | FC-2 structural check unbuilt (`flight_recorder`, `dock`, `pulsewin`) | rmbp gate |
| → S6 | GATE-NEUTRAL; x86-measured exposure: `drivers/xhci/mod.rs` 75 `[piusb NN]` sites (17 strings in the x86 kernel; `[piusb40]`/`[piusb41]` printed on the rMBP wire, F7), `main.rs` `[orinfurn]`×9/`[orinclick]`×2/`[piusb]`×3 + 17 `tegra_*` fns, `video/fbcon.rs` `[orinface]`×4, `video/desktop_firmware.rs` `[pidesk]`×14 ungated; identifiers in `drivers/block.rs` 6, `drivers/gpu/mod.rs` 3, `smolnet.rs` 2, `fs/fat.rs` 2 | rmbp gate (B12 P0); renames per family by ack |
| → S7 | `render_service` family size 3 with an expiry; grown in merge `8c559329` | orin proposes the convergence arc |
| → S8 | `scan_serial_faults` passes on a missing log (`arroyo:2239`) | rmbp gate |
| → S9 | KNOBLEG `647f485a` — on hw-rmbp, reaches trunk at the rmbp landing | rmbp landing |
| → S10 | GATE-ROOTS `e1bff790` — on hw-rmbp, reaches trunk at the rmbp landing | rmbp landing |
| → S12 | Core-0 load: **checked on x86, not the bug** — TSC-time meter, `core_load` folds busy AND idle, core 0 folds every pass (`sched.rs:5411`); F6 c0 = 5–12% idle / 88–99% storm, F7 idle 6% | orin ticks S12 |
| → S27 | 138 PRUNE-CANDIDATE origin refs from the 2026-07-25 branch triage never got Peter's OK under the never-trash law; 408 remote-tracking refs today. A `git push --delete` set that only Peter runs, on rmbp's say | rmbp prepares the list; PETER decides |
| — | PWRNAME `bc10a469` + BOOTFADT `857c6dc8`: flown (F7), on hw-rmbp, unlanded — trunk prints `[orinreboot]` until the rmbp landing | rmbp landing (no S-row: it is the fix for S6's first instance) |

## D. Bench facts this track measured (not defects — no status; the command that measured each is the evidence)

- **`line-butler.py` has NO x86 stream**, and `NVIDIA` in its ORIN_MARKS mis-routed 81% of an rMBP boot into `orin.log` (fixed by orin's BUTLERMARK). Never point the butler at the rMBP's FTDI. Measured: `awk '/NVIDIA/' capture/rmbp9-flight5/ttyUSB0.log` = 12 hits, first at line 1601 of 8651. (`docs/dev/evidence/rmbp10/BUTLER-NVIDIA-MISROUTE.md`)
- **The FTDI re-enumerates** (`ttyUSB0`→`ttyUSB1` on 2026-09-03). Identify by `ls /dev/serial/by-id/usb-FTDI_*`; kill bridges by `lsof -t /dev/ttyUSBn`, never `pkill -f`. (F6)
- **`stage-x86.sh` is two-pass by design**: pass 1 creates the tree and refuses (exit 5); name the tree in the playbook; re-run with the same `--stamp`. (F6, F7 staging)
- **Score what BOOTED by the mapped span**: `WXN-x86 … img=[lo,hi)` vs `readelf -lW kernel.elf` LOAD span — F5 0x1351290 · F6 0x134f198 · F7 0x134fca8. The wire prints no sha. (F6)
- **The reboot ladder's post-verb tail is lost on the rMBP by construction** (A3); never score its absence as a failure. (F6)
- **`wc_shim` is an INLINE mod** at `arch/x86_64/syscall.rs:4777` — a filename search finds nothing. (B11)
- **`panel_info_nonblocking` HAS x86 callers** (`video/quarry/live.rs:1698/:2324`, armed by `UNAOS_QUARRY=1`); the "x86 has no non-blocking door" premise was false. (B11, the C1 provenance lesson)
- **Executor worktrees are cut from `main`, not the track tip** (→ P1). (rmbp-10)

## E. Closed by the legacy sweep (2026-09-05; LEDGER P-row "one sweep of the legacy surfaces", rmbp slice)

Swept: every rmbp/x86-named file under `~/.claude/plans/unaos/` (43 files, 13 surfaces); `past/` is closed by
location. Four candidates >14 days old carried no closure marker; each now does, and the decision is here.

| id | item | owner | flies-on | status | evidence | closed by |
|---|---|---|---|---|---|---|
| E1 | `fox/proposal-rmbp-pull7.md` (2026-07-22): Kepler display reads idle — gmux ownership / `gp_get=0` hypotheses, "awaiting Peter approval" for 45 days | rmbp | rmbp | dropped — superseded: the Kepler takeover landed and the desktop has rendered on Kepler on every flight since (`94b0ed0c` onward); the surviving residual is the iGPU mux (A7) | flights 1–7 (MANIFEST) | this sweep |
| E2 | `wip/rmbp-arcD-M1-parked.md` (2026-08-19): phantom-press recovery redesign, "next session implements it" | rmbp | rmbp | landed — the motion discriminator is in `drivers/ehci/mod.rs:13991/:14032/:14097` and `BUTTON_UP_GEN` is gone (`pal.rs:333`), commit `4b9432bc`; the metal falsifier (a drag produces zero `recovered` increments) is unscored | `4b9432bc` | this sweep (marker added to the file) |
| E3 | `review/rmbp-s12boot1-headdumps.md` + `rmbp-s12boot2-capture.md` (2026-07-23): raw Kepler head dumps for the GR2/GR3 pull review | rmbp | — | dropped — evidence for a review that closed in July; kept in `past/` as captures, not items | the files' own CORRECTION header | this sweep |
