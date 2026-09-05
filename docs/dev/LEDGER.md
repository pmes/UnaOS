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
| S14 | Print Screen: key edge decoded in shared xHCI/EHCI on every arch; the SERVICE half is reachable on x86 and now the Orin (`2000608a`); **Pi: unverified** | all | pi to check | open (Pi) | orin-ledger A9 |

## P. Process hazards (every seat)

| id | item | status |
|---|---|---|
| P1 | Agent worktrees are seeded at `main`'s tip, not the track tip. Every executor brief: `git log -1` first; `git reset --hard <tip>` on the private branch before editing | standing |
| P2 | `git merge -F -` does not read stdin (write the message to a file); `unwrap80.sh` takes a FILE, not stdin | standing |
| P3 | A boot's loader anchor may land in `raw.log`/`unknown.log`, not the board log; pin by anchor in whichever file carries it, then prove board purity | standing |
| P4 | A peer's relay of Peter's word is a report, not the word; focus is assigned in your own session | standing |
