# LEDGER-RECONCILE — orin ledger rows reconciled against the tree and render3b (LEDGERTICK, orin 14, 2026-09-05)

Tree: hw-jetson `6cc8de8c`. Evidence: `docs/dev/evidence/orin13/render3b-boot1.log` (640-line excerpt,
anchor `KELF min=0x0 max=0x2d3488`), `FLIGHT-RESULT-render3b.md`, `RENDER2-AUDIT.md`, `ORIN-BROKEN.md`,
`LANDING-REPORT.md`; reviews `orin13/review/RENDER-REVIEW.md`, `CAPREVOKE-REVIEW.md`,
`orin13/review3/BATCH2-REVIEW.md` (host scratch, unversioned). Every wire count below is `awk '/token/' <log> | wc -l`
(never grep — control bytes); `:N` is a line of the render3b excerpt. Rules applied: a `fixed-unflown` row
whose witness is on the render3b wire becomes `flown`; a `dropped` row keeps its ruling; nothing is
downgraded on inference; rows A15, A16, A17, A18, B1 are owned by other executors this session and were
NOT touched (read-only checks are still recorded for them); no rows appended.

## Broken references found (top of the report)

| # | what | where | finding | disposition |
|---|---|---|---|---|
| 1 | Evidence path `docs/dev/OS/10_INSTALL/orin-unafs-root.md` | §F NVMe row | not in git at that path; the file is `unaos/docs/dev/OS/10_INSTALL/orin-unafs-root.md` (`git ls-files` shows only the `unaos/`-prefixed path). GATE-LEDGER did not catch it because table F has no `status` column | path corrected in the row |
| 2 | A9's and A17's evidence "`render3b-boot1.log` (tail)" | §A A9, A17 | the excerpt in git carries **zero** `PRTSCR` / `capture armed` lines (`awk '/PRTSCR/' … ` = 0; the excerpt ends at the `rx=3` census). The three press lines exist only in the same boot's raw capture on the host (~500 lines past the excerpt's end, same `KELF max=0x2d3488` anchor) | A9's evidence cell now says so (status stays `flown` — Peter's press is witnessed, the excerpt is short); A17 is another executor's row — **reported, not edited**; excerpt extension owed to whoever next touches `docs/dev/evidence/orin13/` |
| 3 | C5 cites "AC#14" | §C C5 | ARCH-CONFORMANCE #14 is the mbench-spec replay finding; the `unaos_ivb` bootloader-leg hole is **#13** (`grep -n unaos_ivb ARCH-CONFORMANCE.md` → row 13, FC-11) | corrected to AC#13 |
| 4 | C5 named no fix commit | §C C5 | the leg is GATE-ROOTS `e1bff790` leg 4 (`x86_64-unknown-uefi --features unaos_ivb`, commit body names it) on hw-rmbp; `git merge-base --is-ancestor e1bff790 6cc8de8c` = **no** (only `hw-rmbp` contains it) | sha added with the ancestry statement; status `fixed-unflown` unchanged (reachable from the hw-rmbp head, as the gate requires) |
| 5 | C1 cites KNOBLEG `647f485a` | §C C1 | `git branch -a --contains 647f485a` = hw-rmbp only; not an ancestor of 6cc8de8c and not of trunk `be3b027e` | consistent with the row's "open — until trunk carries KNOBLEG"; unchanged |
| 6 | D2 cites `be3b027e` | §D D2 | trunk's merge commit, not an ancestor of 6cc8de8c (hw-jetson's side of that merge is `077a8fa1`) | cited as a source, not a fix; unchanged |
| 7 | A4 carried a refuted mechanism | §A A4 | "overran the 1-byte holding register between polls" was refuted by BATCH2-REVIEW M2 and already removed from A16 (b7679a4e) but not from A4 | A4's clause now defers to A16 with the poll-rate command |
| 8 | `Cargo.toml:2474` (`orinrx` comment) says "UNFLOWN — no Orin has booted it" | kernel file | stale since render3b; **kernel file, not this executor's lane** — reported only | owed to the next kernel commit touching `orinrx` |

All twelve shas cited in tables A–E exist (`git cat-file -e`); ten are ancestors of 6cc8de8c; the two that are
not (`647f485a`, `be3b027e`) are explained above. Every `docs/…` path cited in the file is in git after fix 1.
Both `.log` paths cited (`render3-boot1.log`, `render3b-boot1.log`) are in git.

## §A — what the operator sees broken

| id | status before | command | result | status after |
|---|---|---|---|---|
| A1 | flown — render3b | `awk '/deskcascade\] -> CASCADED/' render3b-boot1.log` · `awk '/menubar ENABLED\|menubar PAINTED\|crystal LIVE\|\[dock\] live\|console-window win=1\|post-cascade/'` · `grep -n 'UNAOS_DESKCASCADE' unaos/arroyo` | 1 (:479 `windows=1 bar=1 owns_pixels=1 route=ROUTED`); :460, :466, :467, :429/:538, :444 `(307,158)`, :478 `hw=15472 headroom=17296` — every quoted line present; knob mapped only under `UNAOS_DESKCASCADE` (arroyo:1280) = default-off | unchanged |
| A2 | open — SMP D1–D5 ruling | `grep -n 'D1\|SMP' docs/dev/RULINGS.md` · `awk '/el0core/' render3b-boot1.log` | no SMP ruling in RULINGS.md; render3b `[el0core] el1 core MEASURED: cpu=0 mask=0x1` (:401), `el1cores=0x1` (:494) — still one EL1 core | unchanged |
| A3 | open — design | `grep -n 'REGARDLESS of focus\|wc_shell_focus_key' main.rs` (:2948) · `awk '/midden/' render3b-boot1.log` | the pump still feeds `handle_key` regardless of focus; render3b's injected `st\r` went to the shell (`[midden] cmd="st" -> TerminalError` :611) | unchanged |
| A4 | flown — render3b (mechanism: holding-register overrun) | `awk '/serialrx\] lsr=/'` (:498) · `awk '/JD2 — KEY/'` (:586-588) · `awk '/rx=3 \(\+3\)/'` (:618) · `awk '/polls=/'` deltas (22.75 M → 23.07 M per ~1 s sweep) | witness lines present; the overrun-between-polls claim is refuted by the poll rate (~325k/s vs 87 µs/byte, BATCH2 M2) — A16 already says "undetermined" (b7679a4e), A4 did not | flown — **corrected**: quotes made exact (`lsr=0x00000200`, `KEY 's'`), mechanism deferred to A16, ORINRX sha `9cb779bd` named |
| A5 | flown — render3b | `awk '/srcdelta=23\|presents=29\|load c0=85/' render3b-boot1.log` | :577 `redraws=24 skipped=15 srcdelta=23`, :584 `presents=29`, tail `SCHED: load c0=85%` — all present | unchanged |
| A6 | open — bsprun/bsptick unflown | `awk '/bsprun\|bsptick/'` = 0 · `awk '/SCHED-BAL/'` (:487 `0 steals total across 5 online cores, 0 core(s) ran work`) · `awk '/wcpar/'` (:547 `cores=1`) | not flown; one core still does all the work | unchanged |
| A7 (→ S4) | open | `grep -rn take_shell_reopen unaos/crates/kernel/src` → main.rs:6763 (inside `#[cfg(target_arch = "x86_64")] fn x86_render_service`, :6322-6323) + x86 syscall.rs:5757 only · `awk '/\[dock\] live/'` (:538 `presses=0 … unhides=0`) | no aarch64 drain; no dock press on the wire | unchanged |
| A8 | open — follows A1 | `awk '/quarry/' render3b-boot1.log` = 0 · `grep -n quarry main.rs` = comments only (:3754, :7807-7808, :8366) | A1 flew, quarry still has no opener on the terminus and printed nothing | open — **corrected**: the blocker is the missing opener, not §5.2 |
| A9 | flown — render3b | `awk '/PRTSCR\|capture armed/' render3b-boot1.log` = **0** · raw capture (host) lines 2952-2955: two `capture armed`, one `SCREEN0.PNG 1920x1200 … -> OK`, same boot (`KELF max=0x2d3488` anchor at raw :1602) · `git log --oneline -1 60f7ec5e` (PRTSCRLIVE, ancestor) | the press is real and witnessed but NOT in the git excerpt | flown — **evidence corrected**: excerpt gap stated, "this commit" replaced by `60f7ec5e` |
| A10 | flown — 0 `[pulsewin] open` | `awk '/pulsewin/' render3b-boot1.log` = 0 · `grep -c pulsewin main.rs` = 5 (all comments: "PAINTPULSE RETIRED … `pulsewin::service()` stood here and is gone" :8309) · note `[pidesk] pulse-window ARMED view=Pi LED lamps` (:477) is activate's latch that nothing services (BATCH2 L1) | holds; R17/A18 will reverse the retirement — that is A18's arc, not a downgrade here | unchanged |
| A11 | open | `grep -rn drag_cancel arch/aarch64/syscall.rs` = 0 vs x86 syscall.rs:5332 `drag_cancel("focus-key")` · `awk '/drag/' render3b-boot1.log` = 0 | still one-sided | unchanged |
| A12 | open | render3b build features (`build-render3b.log:1`) carry no `net4` · `awk '/net4\|NO-OFFER\|DHCP/'` = 1 (:126 `[net4B]` DMA window from `mmu_tegra.rs:1793`, ungated) | not on the wire | unchanged |
| A13 (→ S3) | open — FC-2 shape | `grep -n -B2 'flight_recorder::service' main.rs` → :1259 under `#[cfg(target_arch = "x86_64")]`; :1747 "That function is x86-only" · `awk '/flight_recorder/'` = 0 | still x86-only | unchanged |
| A14 | open | `grep -n 'x86_64, wifi\|feature = "wifi"' main.rs` (:1236 cfg all(x86_64, wifi)) | no aarch64 wifi | unchanged |
| A15 | fixed-unflown — 1 pass (APTEXT) | **not touched** (other executor). Read-only: `awk '/CPU_ON AP/'` = 5, `/5\/5 secondaries online/` = 1 (:397); `git merge-base --is-ancestor fef6a184 6cc8de8c` = yes; raw capture :684-:707 shows render3's death (`Exception reason=1 syndrome=0x82000010`, `Powering off core`) BEFORE the render3b anchor — one pass, as the row says | not mine |
| A16 | open — undetermined | **not touched**. Read-only: `grep -n 'FCR\|IIR\|OVRF' arch/aarch64/serial.rs` = 0 — no discriminator landed yet | not mine |
| A17 | open | **not touched**. Read-only: the row's evidence "(tail)" has the same excerpt gap as A9 (finding 2) | not mine — reported |
| A18 | open — ruling recorded | **not touched**. Read-only: `grep -n R17 docs/dev/RULINGS.md` = :20 (live) | not mine |

## §B — capture findings (render2)

| id | status before | command | result | status after |
|---|---|---|---|---|
| B1 | dropped | **not touched** (other executor) | — | not mine |
| B2 (→ S1) | open — owner rmbp | `awk '/status-change/' render3b-boot1.log` | :233 `HUB slot 1 status-change Configure-Endpoint code 17`, :298 `slot 3 … code 8` — reproduced on render3b | open — **evidence advanced** (render3b witness added) |
| B3 (→ S2) | open — owner rmbp | `awk '/vid:pid/'` | :284, :297 `vid:pid=0000:0000` — reproduced | open — **evidence advanced** |
| B4 | open — bench | `awk '/abfbdefa/'` on the excerpt and the raw capture | 0 — no dark boot of the foreign volume this session (render3's two deaths were A15's abort, a different cause) | unchanged |
| B5 (→ S6) | open — GATE-NEUTRAL | `awk '/PIUSB/'` = 5 (:346, :350, :351 …) · `grep -rln PIUSB unaos/crates/kernel/src` = 10 files | still prints on the Orin | open — **evidence advanced** |

## §C — architecture-conformance findings

| id | status before | command | result | status after |
|---|---|---|---|---|
| C1 (→ S9) | open — until trunk carries KNOBLEG | `git branch -a --contains 647f485a` = hw-rmbp only · `git merge-base --is-ancestor 647f485a be3b027e` = no · `grep -n '_rows' unaos/arroyo` (:3962-3970 still the unfailable loop) | not in trunk, not in this tree | unchanged |
| C2 | open (= A11) | as A11 | one-sided | unchanged |
| C3 | open — doc | `grep -rn 'Ring 3' CLAUDE.md docs/ROADMAP.md` (still "Ring 3 host-native userspace") | wording unchanged | unchanged |
| C4 (→ S8) | open — arroyo | `sed -n 2266,2269p unaos/arroyo` → `[ -f "$logf" ] \|\| return 0` | still passes on a missing log | unchanged |
| C5 | fixed-unflown — on hw-rmbp | `grep -n unaos_ivb docs/dev/evidence/orin12/ARCH-CONFORMANCE.md` → row **13** · `git show e1bff790` body: "Leg 4 … `x86_64-unknown-uefi --features unaos_ivb`" · `git merge-base --is-ancestor e1bff790 6cc8de8c` = no · `grep -n unaos_ivb unaos/arroyo` at 6cc8de8c = kernel `x86-all` leg only (:2702) | the hole is AC#13, the fix is `e1bff790` on hw-rmbp | fixed-unflown — **corrected** (AC#13, sha + ancestry named) |
| C6 | dropped (= A10) | keeps its ruling; note R17/A18 will revisit the pulse window | — | unchanged |
| C7 | open (AC#21) | as A7 | no aarch64 drain | unchanged |
| C8 | flown — render2 | `git merge-base --is-ancestor` for `a5a66fc1 7ffd2122 01739a93 8085c9c8` = all yes | shas reachable; render3b adds no new score | unchanged |
| C9 (→ S11) | fixed-unflown — QEMU | `git merge-base --is-ancestor 06858185 6cc8de8c` = yes · `awk '/revoke/' render3b-boot1.log` = 0 | not on the render3b wire (no EL0 program exercised `cap_revoke`) | unchanged |

## §D — decisions

| id | claim | command | result | after |
|---|---|---|---|---|
| D1 | recommendation (no status) | `awk '/cols=\|240x56\|SetMode/' render3b-boot1.log` → only :444 (the console window's `cols=185`) | the loader's console mode is before the excerpt's anchor; render3b does not decide D1 | unchanged |
| D2 (→ S7) | owed, with expiry | `grep -n 'fn orin_render_service\|fn render_service\|fn x86_render_service' main.rs` → :5273, :6323, :8222 | the size-3 family still stands; convergence arc not landed | unchanged |
| D3 | dropped — ruled 2026-08-25 | keeps its ruling (6cc8de8c is the commit that dropped it) | — | unchanged |

## §E — landed but unflown

Rewritten in place as an annotated list (prose, so GATE-LEDGER does not judge it; no rows appended): each
item now carries "answered on render3b" with the quoted line, or "not on the render3b wire". Summary:
answered — PANELOWN (`[panel-owner] … to=owner-console-window` :431), `orinfurn`'s question via the
cascade (`[pidesk] menubar ENABLED … rect=Some((0, 0, 1920, 34))` :460, crystal press :469), PRTSCR-ORIN
(A9), LOADSAMPLER (A5). Not on the wire — TABKEY, `orintenant`, `orinladder` (a)/(b), `tegradesk` floors,
ORIN-STKDEPTH/PANELREFUSE/supstate/`live=`, `bsprun`/`bsptick`, REDZONE (0 lines again), `reboot`,
NET-4 (image carried no `net4`), window-body persistence (no post-click probe of win=1).

## §F — hardware inventory (status-like claims only)

| row | claim before | command | result | after |
|---|---|---|---|---|
| Serial UART — RX | STUB (guarded, never delivered); "No `ORINRX` feature exists in the tree (0 hits)" | `grep -n orinrx unaos/crates/kernel/Cargo.toml` (:2476 `orinrx = []`) · `grep -n serialrx main.rs` (:2854, :2915) · `awk '/serialrx/' render3b-boot1.log` = 23 | both halves of the claim are false at 6cc8de8c | **corrected**: WORKS ON METAL behind `orinrx`, lossy (A4/A16); shipped image still RX-less; "what it would take" updated to A16's discriminators |
| Screenshot to card (prtscr) | COMPILED, UNFLOWN on the shipped image | A9's evidence (raw capture) · `sed -n 1532p unaos/arroyo` (jetson default line carries no `holocron`) · `grep -n 'prtscr::service' main.rs` (:3014) | flown behind `UNAOS_HOLOCRON=1`; still unshipped by default | **corrected**: FLOWN behind the knob (A9/A17), UNSHIPPED by default |
| NVMe | ABSENT | evidence path (finding 1) | path wrong | **path corrected** |
| CPU cores / SMP | WORKS ON METAL, 1 of 6 cores does work | `awk '/SCHED-BAL\|wcpar/'` (:487 0 steals, :547 `cores=1`) | render3b re-confirms | unchanged |
| Timer tick & preemption | STUB at EL1 (one-shot only) | `awk '/bsptick/'` = 0 | not flown | unchanged |
| Power / reset | SYSTEM_RESET COMPILED, UNFLOWN | `awk '/reboot\|SYSTEM_RESET/'` = 0 | not flown | unchanged |
| EL0 window tenants / Self-update | COMPILED, UNFLOWN | `awk '/orintenant\|selfup/'` = 0 | not flown | unchanged |
| Gap 4 (Serial RX) prose | "First metal question: does LSR show DR=1…" | :498 `lsr=0x00000200 -> RX-LIVE`, :618 `rx=3` | answered | **annotated**: answered on render3b, the open question is A16's |

## Tally

Rows in the gated tables A–C: 32. Changed: **8** (A4, A8, A9, B2, B3, B5, C5 — corrections or evidence advances;
none changed status head) + **3** §F status claims corrected (RX, prtscr, NVMe path) + §E rewritten + gap-4
annotated. Status heads advanced: **0** — no `fixed-unflown` row had a new witness on the render3b wire that its
row did not already carry (A15 is the one candidate and is another executor's tick). Unchanged: 24 gated rows.

## Gate

`./arroyo check` at the commit: see the commit body (exit code + the GATE-LEDGER line).
