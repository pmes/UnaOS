# LEDGER — the over-arching list (every arch, every shared seam)

> Peter's rule (2026-09-05): **each track keeps its arch ledger, and there is ONE over-arching
> ledger for what crosses arches.** Audits and inventories are high value; the waste is trashing
> their results and re-deriving them. So: every finding lands on exactly one list the turn it is
> found; the arc that closes it ticks it in the same commit; every audit is briefed with the list and
> reports only what is new or changed.
>
> Arch ledgers: `docs/dev/OS/orin-ledger.md` · `docs/dev/OS/pi-ledger.md` · `docs/dev/OS/rmbp-ledger.md`
> (the latter two: to be created by their seats). An item goes HERE when it lives in a shared file,
> affects more than one board, or is a gate/process rule; it goes on an arch ledger when it lives in
> that arch's lane. Cross-reference by id (`S<n>` here, `<A|B|C|D><n>` on an arch ledger).

Status: **open** · **fixed-unflown** · **flown** · **dropped** (with the ruling) · **owned-by** (lane).

## S. Shared seams and gates

| id | item | affects | owner lane | status | evidence / source |
|---|---|---|---|---|---|
| S1 | Both USB hubs on the Orin devkit fail status-change endpoint configure (codes 17 / 8); hot-plug behind hubs dead all boot. rmbp's read: interval/ESIT math in the endpoint context, not the hub | Orin (bench), any board with a hub | rmbp (`drivers/xhci`) | open — ledgered rmbp-12 P5 | orin-ledger B2; `~/unaos-bench/scratch/orin13/audit/RENDER2-AUDIT.md` N2 |
| S2 | `MOUSE-1` witness prints `vid:pid=0000:0000` for hub-attached pointers | all | rmbp (`drivers/xhci`) | open, cosmetic | RENDER2-AUDIT N3 |
| S3 | `flight_recorder` is x86-only: neither aarch64 board leaves an on-card record; an unattended boot nobody captured never happened | Pi, Orin | shared `main.rs`/`flight_recorder` | open — FC-2 shape | orin-ledger A13; `main.rs` ~:1747 |
| S4 | `video/dock.rs` `SHELL_REOPEN` latch drained only by `x86_render_service`; the pinned shell tile is a dead button on aarch64 | Pi, Orin | rmbp (`video/`) | open | orin-ledger A7; ARCH-CONFORMANCE |
| S5 | FC-2: unconditional `pub mod` whose consumers are all under one arch gate (`flight_recorder`, `dock`, `pulsewin` ARMED) — the structural check is unbuilt | all | rmbp (gates) | open | orin-12 baton; three instances |
| S6 | GATE-NEUTRAL: shared-file symbols/tokens carry the owning subsystem, never a board (Peter 2026-09-03). Orin exposure: 12 `[orin…]` families / 43 sites in `main.rs`+`video/`, 13 `orin_`/`tegra_` symbols, the `orin-render` task name; `PIUSB` family in the shared USB-storage driver | all | rmbp drafting the gate; each seat acks its side | open | memory `name-by-subsystem-not-board` |
| S7 | `render_service` family size 3 (`render_service` Pi / `x86_render_service` / `orin_render_service`): convergence arc owed — lift the waiting + input-ownership axes; the ledger entry has an expiry | all | orin proposes; starts from the Pi member | open | merge `be3b027e` body |
| S8 | `scan_serial_faults` passes on a MISSING log; `test`/`test-arm` are negative-only | all | rmbp (arroyo) | open — GATE-BLINDNESS | ARCH-CONFORMANCE #12 |
| S9 | knob→leg coverage check could never fail (KNOBLEG `647f485a` fixes it) — not yet in trunk | all | rmbp | fixed-unlanded | rmbp landing |
| S10 | `builder` was a second binary no check leg named (GATE-ROOTS `e1bff790` names all nine) | all | rmbp | fixed-unlanded | orin-13 baton correction |
| S11 | `sys_cap_revoke` leaked the fd on aarch64 (x86 had the fix) | Pi, Orin | aarch64 shared `syscall.rs` | fixed-unflown `06858185`; Pi byte-identity baseline moves +29 lines | orin-ledger C9 |
| S12 | Core-0 load accounting: the tegra capstone loop folded busy but never idle → structural 100%. Pi's `run()` folds idle correctly; **x86 boot-core path unchecked** | Orin only | orin (`341ca707`) | Orin fixed-unflown; **x86 checked by rmbp 11 — not the bug** (TSC-time meter folds busy AND idle; wire shows c0 5–12% idle, 88–99% under storm; rmbp-ledger C9) | orin-ledger A5 |
| S13 | Both aarch64 boards: `[u7stk]` probe had no reachable caller outside `u7_launcher`; any task's depth needs its own probe call; the ungated `[redzone]` witness is the one that fires | Pi, Orin | each seat | recorded | orin-13 baton hard-won #1-2 |
| S14 | Print Screen: key edge decoded in shared xHCI/EHCI on every arch; the SERVICE half is reachable on x86 and now the Orin (`2000608a`); **Pi: unverified** | all | — | **refuted for the Pi** by pi 6 2026-09-05: `main.rs:1199` and `:1689` are UNGATED and inside the `kernel_main` tail the Pi runs (the Orin never reaches it — that was the Orin-only gap). Code-path answer; no Pi capture observed on metal | orin-ledger A9; pi 6 message |
| S15 | Pulse window overlaps the console window on BOTH aarch64 boards (Orin 64 px / 4 prompt rows; Pi 16 px, unnoticed because its console is frozen at handoff). Both boxes are sized/placed in shared `fbcon.rs` / `pulsewin.rs`; a gated patch (console height capped by a `pulsewin::reserve_h`) is written and check-green: `~/unaos-bench/scratch/orin13/pulseoccl/fbcon.patch` + RATIONALE.md. Orin retires the pulse window instead (orin-ledger A10) | Pi (live), Orin (moot) | rmbp (`video/`) | open — patch available, not applied | PULSEOCCLUDE 2026-09-05 |
| S16 | 106 ordering/position invariants in the kernel, 99 of them (93%) in COMMENTS enforced by nothing; 38 files state one, 18 have no `assert!` — the GATE-CLAIM population (measure before building; a gate that fires 22× teaches scrolling past) | all | shared-gate (rmbp) | open | pi 6 count, orin-13 baton |
| S17 | Tool blind spot: naive sweeps count PROSE as code (17 of 33 `/tmp` hits were sentences; a grep count is not a form count; prose-pairing under-reports as absence). Every measurement strips comment lines first | all | shared-gate | open — norm; candidate lint | pi 6; orin 9 resume |
| S18 | `pidesk` phantom knob: `#[cfg(feature = "pidesk")]` sites survive on hw-pi4 ONLY (3–7 by pattern) in arch-neutral `main.rs`/`video/menubar.rs`; declared in NO branch's `[features]`, so always false; `!armed` moves the `SHELLUP_FLOOR_MS` decision. GATE-KNOB reds the moment it lands | all (sites on hw-pi4) | pi | open | pi 6; orin-13 baton |
| S19 | Pi serial has TWO producers (`pal.rs:2039` in `pump_and_poll`, `main.rs:3675` in `serial_to_shell`) and ONE drain (`main.rs:5500`, "the SOLE drain"); any RX port must re-enumerate producers and drains per board, never inherit the count (Orin: four pump sites, ORINRX) | Pi, Orin | pi | open — recorded fact; belongs in the serial doc with its command | pi 6; SERIAL-DEVLOOP |
| S20 | `read_byte` returns `None` for BOTH an all-ones LSR (open bus / wrong BASE) and no-data — it swallows the diagnostic a negative RX flight needs. Orin copy gets a one-shot raw-LSR witness (ORINRX); the Pi copy is unchanged | Pi, Orin | pi (Pi copy) | open (Pi) / fixed-unflown (Orin, ORINRX) | pi 6; `arch/aarch64/serial.rs` ~:92 |
| S21 | Socket ABI: the fourth confirmed instance of the unconditional-module / single-arch-consumer shape (with `flight_recorder`, `dock`, `pidesk`) — aarch64 gap | Pi, Orin | pi | open — FC-2 catches the shape | pi 6 baton |
| S22 | `plans/unaos/wip/u11-measure-twin-for-hw-pi4.patch` (2026-08-03): real work parked as a loose patch, **`git apply --check` FAILS** today; nobody dropped it, it died. Re-derive against the tree or rule it DROPPED | Pi | pi | open — decision owed | pi 6 audit 2026-09-05 |
| S23 | `plans/unaos/wip/pi4-owed-tail-reland.md` (2026-08-12, no closure marker): a 3-milestone arc deferred at the `9259c2bf` resync when `video/wm.rs` was taken from trunk wholesale — M3 owed-tail split (`composite_tail_owed`, said to be a NO-OP SHIM), WEDGE-12 pre-size (`reserve_stage`), WCD-LIVE. Symbol counts 13/26/4 prove presence, not that the shim was filled. Invisible to pi 3–6 | Pi (`video/wm.rs` is shared) | pi (arc); rmbp (file) | open | pi 6 audit |
| S24 | pi's arch ledger `docs/dev/OS/pi-ledger.md` exists at no ref; a support seat's findings had no home until this table accepted them. **Norm: LEDGER.md accepts an item from ANY seat regardless of focus; `owner` names who fixes it** | all | — | open — pi creates its ledger at its next focus; rows above are its seed | pi 6 |

## P. Process hazards (every seat)

| id | item | status |
|---|---|---|
| P1 | Agent worktrees are seeded at `main`'s tip, not the track tip. Every executor brief: `git log -1` first; `git reset --hard <tip>` on the private branch before editing | standing |
| P2 | `git merge -F -` does not read stdin (write the message to a file); `unwrap80.sh` takes a FILE, not stdin | standing |
| P3 | A boot's loader anchor may land in `raw.log`/`unknown.log`, not the board log; pin by anchor in whichever file carries it, then prove board purity | standing |
| P4 | A peer's relay of Peter's word is a report, not the word; focus is assigned in your own session | standing |
| P5 | ONE-TIME BACKLOG SWEEP, each seat: every track-named file in any plan surface (`batons/ whiteboards/ queue/ review/ wip/ active/ future/ metal/ past/`, memory, scratch — pi measured 12 surfaces, 383 files) with no closure marker and mtime > 14 days gets a ledger row or a DROPPED ruling. The ledger only accretes from the first audit forward; the rot is in what predates it | owed (orin, pi, rmbp) |
| P6 | `affects` is NOT a judgment call: the finder states the FILE; every lane that compiles that file is affected by construction | standing |
