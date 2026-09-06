# P5-SWEEP — the orin backlog sweep (LEDGER.md P5, one-time; seat orin 14, 2026-09-05)

Law: LEDGER.md **P5** — every track-named file on any plan surface with no closure marker and an
mtime older than 14 days (before 2026-08-22) gets a ledger row or a DROPPED ruling. Method: P10 —
determine DIRECTION before triage (is the change ABSENT or already PRESENT? prove it with a command
that could hit). Tip swept against: `6cc8de8c` (hw-jetson). Script + raw output:
`~/unaos-bench/scratch/orin14/p5sweep/{census.py,direction.py,direction.out}` (unversioned; the
verdicts below are the deliverable, RULINGS R13).

**Filter definitions (so the counts are reproducible):** *orin-named* = filename or first 20 lines
match `(?<![a-z])orin|jetson|(?<![a-z])tegra` (case-insensitive). *stale* = `lstat` mtime <
2026-08-22. *unmarked* = no `CLOSED|DONE|LANDED|DROPPED|superseded|→ orin-N|RETIRED|ARCHIVED|
shipped|merged` anywhere in the file; two files whose ONLY marker was a `DONE` that reads as a
gate name (`DONE gate`, `DONE =`) rather than a closure were pulled back in (marked `†`).

**Tool blind spot found and fixed (S17 class):** the first census used `orin|jetson|tegra` without
word boundaries; `in-TEGRA-tor`, `in-TEGRA-ted`, `monit-ORIN-g` all matched. That inflated the
orin-named population from 355 to 380 and the unmarked set from 39 to 43. The four false
positives are listed at the bottom for the record — one of them carried a real, unclosed item.

## 1. Census (25 surfaces, 767 files)

| surface | files | orin-named | stale (>14d) | unmarked |
|---|---|---|---|---|
| `~/.claude/plans/unaos/active/` | 10 | 2 | 2 | 0 |
| `~/.claude/plans/unaos/batons/` | 22 | 10 | 1 | 0 |
| `~/.claude/plans/unaos/bin/` | 1 | 0 | 0 | 0 |
| `~/.claude/plans/unaos/fox/` | 107 | 47 | 47 | 25 |
| `~/.claude/plans/unaos/future/` | 21 | 5 | 5 | 1 |
| `~/.claude/plans/unaos/lc/` | 9 | 1 | 1 | 0 |
| `~/.claude/plans/unaos/metal/` | 5 | 2 | 2 | 1 |
| `~/.claude/plans/unaos/past/` | 180 | 77 | 77 | 5 (+1 †) |
| `~/.claude/plans/unaos/queue/` | 5 | 3 | 2 | 0 |
| `~/.claude/plans/unaos/review/` | 121 | 48 | 39 | 0 |
| `~/.claude/plans/unaos/templates/` | 5 | 1 | 1 | 0 |
| `~/.claude/plans/unaos/whiteboards/` | 10 | 8 | 0 | 0 |
| `~/.claude/plans/unaos/wip/` | 5 | 1 | 0 | 0 |
| `~/.claude/plans/unaos/` (top files) | 4 | 0 | 0 | 0 |
| `~/.claude/plans/` top-level `unaos-*` | 5 | 0 | 0 | 0 |
| shared memory dir `…/-UnaOS/memory/` | 14 | 7 | 3 | 2 |
| shared memory `…/memory/archive/` | 56 | 13 | 13 | 5 (+1 †) |
| pi4 bridge memory `…/-UnaOS-hw-pi4/memory/` | 26 | 15 | 0 | 0 |
| `~/unaos-bench/scratch/orin10/` (top docs) | 109 | 74 | 0 | 0 |
| `~/unaos-bench/scratch/orin11/` (top docs) | 18 | 13 | 0 | 0 |
| `~/unaos-bench/scratch/orin12/` (top docs) | 6 | 3 | 0 | 0 |
| `~/unaos-bench/scratch/orin13/` (top docs) | 11 | 9 | 0 | 0 |
| `~/unaos-bench/scratch/orin14/` (top docs) | 2 | 2 | 0 | 0 |
| `docs/dev/evidence/orin12/` | 2 | 2 | 0 | 0 |
| `docs/dev/evidence/orin13/` | 13 | 12 | 0 | 0 |
| **total** | **767** | **355** | **193** | **39 (+2 †) = 41** |

No `plans/` directory exists inside the repo. `~/.claude/plans/unaos/unaos` is a symlink loop
(`→ ~/.claude/plans/plans/unaos`) and was not descended. Every stale orin-named file on the
`review/`, `active/`, `batons/`, `queue/`, `templates/` surfaces carries a closure marker — the rot
is concentrated in `fox/` (the July Fox-seat executor briefs, 25 of 41) and the June/July
`past/` proposals.

## 2. Verdict summary

| verdict | count |
|---|---|
| already-landed (sha cited; no row needed) | 30 |
| DROPPED / superseded (ruling cited) | 2 |
| not-a-work-item (rules, indexes, seeds, pointers) | 9 |
| still-open → new ledger row | 0 from the 41; **2 rows** from what the sweep exposed on the way (C10 orin-ledger; S27 LEDGER.md, owner rmbp) |

Zero of the 41 stale files carries an obligation that is not already on a ledger or in the tree:
every executor brief in `fox/` landed as a named commit within days of being written and was
simply never marked; every `past/` proposal was executed and folded into
`docs/dev/OS/01_BOOT_HAL/arch_arm64.md`. The backlog's danger was phantom obligations (P10), not
lost work — the same shape pi 6 found (S22, S23) and rmbp found (E2).

## 3. Per-file table

Paths: `fox/…`, `past/…`, `metal/…`, `future/…` are under `~/.claude/plans/unaos/`; `memory/…` is
`~/.claude/projects/-home-pmes-src-github-com-pmes-UnaOS/memory/`. Direction commands ran from the
worktree root at `6cc8de8c`; `git log --all` unless stated. Every sha below passed
`git cat-file --batch-check` (42 shas, 0 missing).

| # | path | mtime | distinctive content | direction command | verdict | ledger id |
|---|---|---|---|---|---|---|
| 1 | `fox/brief-hid-regress-b12.md` | 2026-07-19 | keyboard+mouse dead on boot 12 after JC3 made APs stealers; "pin the HID poll" | `git log --grep='HID-REGRESS'` → `46760e41 sched: HID-REGRESS-B12 — WFI-park self-ticking APs so input coexists on Orin`; `grep -rl HID-REGRESS unaos/crates` → sched.rs, timer.rs, mod.rs | already-landed `46760e41` | — |
| 2 | `fox/brief-l4t-facts-net.md` | 2026-07-19 | research arc: produce `L4T-FACTS-NET.md` (GPLv2 facts only) for NET-4q | `ls fox/L4T-FACTS-NET.md` → 20348 B, 2026-07-21; consumed by NET-4C `5e15d766`, NET-4z `cdd09761` (both cite it) | already-landed (facts note exists; consumers cite it) | — |
| 3 | `fox/brief-net4k-poll-cadence.md` | 2026-07-19 | OFFER accepted, socket never REQUESTs; poll/dispatch cadence suspect | `git log --grep='NET-4k'` → `a59ec973 net: NET-4k — refute the DHCP poll-cadence suspect…`; residue folded by NET-4V `1c23485f` ("the no-lease is the ACK") | already-landed `a59ec973`; residue = A12 | A12 |
| 4 | `fox/brief-net4m-per-pop-invalidate.md` | 2026-07-19 | RX buffers cache-invalidated on the FIRST pop only | `git log -S'[net4m]' -- …/rtl8168_tegra.rs` → `9751636d net: NET-4m — the per-pop RX invalidate is already live…`; NET-4u `c5e765e0` full-span invalidate | already-landed `9751636d` (the brief's premise was refuted: the invalidate was already per-pop) | A12 |
| 5 | `fox/brief-net4n-inbound-dma-addr.md` | 2026-07-19 | RX payload DMA lands at fabric 0x200; >4 GiB buffers + PCIDAC | `git log --grep='NET-4n'` → `a6839dfc net: NET-4n — enable RTL8168 64-bit DMA (CPlusCmd.PCIDAC)…`; premise later dropped by NET-4q `f1e65408` | already-landed `a6839dfc`; superseded by `f1e65408` | A12 |
| 6 | `fox/brief-net4n2-pcidac-latch.md` | 2026-07-19 | PCIDAC written but reverts; order it last / 9346 unlock | `git log --grep='NET-4n2'` → `cd17dc95 net: NET-4n2 — re-apply RTL8168 PCIDAC as the final CPlusCmd write…`; "PCIDAC is a relic" (NET-4t hard-won facts) | already-landed `cd17dc95`; premise retired by `f1e65408` | A12 |
| 7 | `fox/brief-net4o-sub4g-arena.md` | 2026-07-19 | sub-4 GiB NIC DMA arena (PCIDAC won't latch) | `git log --grep='NET-4o'` → `24d4ed49` landed, then `4d8f2a54 net: NET-4p — revert NET-4o, alias…`, then `f1e65408 NET-4q — …drop the PCIDAC premise and the iATU alias`; `grep -rl net4o unaos/crates` → 0 hits | DROPPED — landed and REVERTED by NET-4p; superseded by NET-4q (64-bit descriptor address) and NET-4B `a9247f7e` (Normal-NC window) | A12 |
| 8 | `fox/brief-simmer.md` | 2026-07-19 | shell verb `simmer`, per-core load animator, `:: SIMMER: staged` | `git log -i --grep=simmer` → `11d23a64 sched: SIMMER — a per-core load animator…`; `grep -n 'SIMMER: staged' …/sched.rs` → :6299 | already-landed `11d23a64` (survived the in-kernel vug deletion `ee6bfd97`) | — |
| 9 | `fox/brief-vug-ras-analyze.md` | 2026-07-19 | FillWrite at 0x26b900000: sweep-caused (H1) vs revealed (H2) | `git log --grep='VUG-RAS'` → `9177a719 vug: settle boot-15 RAS as H2 and carveout-bound the localizer's above-heap sweep` | already-landed `9177a719`; the writer hunt was closed by XCARVE-3 (no software writer) | — |
| 10 | `fox/brief-vug-ras-localizer.md` | 2026-07-19 | knob `UNAOS_VUGRAS`, `:: VUGRAS: frame N swept ::` | `git log --grep='VUG-RAS-LOCALIZER'` → `db48da7f vug: VUG-RAS-LOCALIZER — force the Orin FillWrite RAS to name its writer`; `grep -rl 'VUGRAS:' unaos/crates` → vugras.rs, allocator.rs, xhci/mod.rs | already-landed `db48da7f` | — |
| 11 | `fox/brief-xcarve-snoc-ras.md` | 2026-07-19 | SNOC/ACI FillWrite family; lead = xHCI event ring in .bss (shared-core touch, Peter-approved) | `git log --grep='XCARVE-SNOC\|XCARVE-2'` → `425090fd xhci: move the event ring + ERST out of image .bss into the heap (XCARVE-SNOC)`, `74eecb76 vug: XCARVE-2 — hunt the 0x26b900000 SNOC writer by store-site bracketing` | already-landed `425090fd`, `74eecb76`; theory retired by XCARVE-3 | C10 (extent) |
| 12 | `fox/restore-metal-r23s1.md` | 2026-07-19 | mid-sitting restore pointer → `brief-metal-r23s1b.md`; "main needs `git reset --hard fe91119e`" | `ls fox/brief-metal-r23s1b.md` → exists (8566 B); `git merge-base --is-ancestor fe91119e main` → rc=0 (main contains it; 6 weeks of trunk since) | not-a-work-item — a July restore pointer; the reset question was Peter's call that day and is moot | — |
| 13 | `fox/brief-xcarve3-carveout-hole.md` | 2026-07-20 | 0x26b900000 is a protected carveout — punch it out of the cacheable map | `git log --grep='XCARVE-3'` → `c5d44b5f tegra: XCARVE-3 — punch the 0x26b900000 protected carveout out of the cacheable map`; then XCARVE-5 `05e9918b`, -6 `a2e6a48e` | already-landed `c5d44b5f` | C10 |
| 14 | `fox/brief-xcarve7-fb-l2-patch.md` | 2026-07-20 | XCARVE-6 punched the framebuffer; make `map_fb_region` L2-aware; witness `fb L2 repair` | `git log --grep='XCARVE-7'` → `e487e36d tegra: XCARVE-7 — make map_fb_region L2-aware so a punched fb block is repaired`; `grep -rl 'fb L2 repair'` → mmu_tegra.rs + render2/render3 boot logs (the witness fires on 2026-09-05 boots) | already-landed `e487e36d`, flown (orin13 captures) | — |
| 15 | `fox/brief-genet5-rx-classification.md` | 2026-07-21 | **Pi** GENET: 151 frames popped, zero classified | `git log --grep='PI-GENET-5'` → `0370ebc1 net: PI-GENET-5 — invalidate RX buffers before read…` | already-landed `0370ebc1` (pi-owned; orin-named only by "the Orin's disease in a different coat") | — |
| 16 | `fox/brief-net4A-stuck-desc1.md` | 2026-07-21 | NIC sticks at desc[1]'s buffer address | `git log --grep='NET-4A —'` → `a7992053 net: NET-4A — the stuck RX payload address is NIC-internal descriptor-address reuse, not a cache bug`; corrected by NET-4F `ca80655c` ("NET-4A was reading its own witness") | already-landed `a7992053`; residue is the live row | A12 |
| 17 | `fox/brief-net4B-nc-ring.md` | 2026-07-21 | rings+buffers in Normal-NC memory; `[net4B]` banner | `git log --grep='NET-4B'` → `a9247f7e net: NET-4B — put the RTL8168 rings + buffers in a Normal-NC DMA window`; `grep -rl net4B` → mmu_tegra.rs, rtl8168_tegra.rs, orin-ledger.md | already-landed `a9247f7e` | A12 |
| 18 | `fox/brief-net4C-mps-mrrs.md` | 2026-07-21 | audit PCIe MPS/MRRS both sides; `[net4C]` DevCtl witness | `git log --grep='NET-4C'` → `5e15d766 net: NET-4C — audit + reconcile PCIe MPS/MRRS on both sides of the controller-0 link`; `grep -rl net4C unaos/crates` → rtl8168_tegra.rs | already-landed `5e15d766` | A12 |
| 19 | `fox/brief-net4t-other-frames.md` | 2026-07-21 | what are the 8 "other" frames; peel VLAN tags | `git log --grep='NET-4t'` → `68a22574 net: NET-4t — witness the 8 'other' frames + peel VLAN tags in the RX classifier` | already-landed `68a22574` | A12 |
| 20 | `fox/brief-net4x-desc-clean.md` | 2026-07-21 | descriptor ring never cleaned to PoC; NIC fetches stale zeros | `git log --grep='NET-4x'` → `bcc99f4c net: NET-4x — clean the descriptor rings to PoC…` | already-landed `bcc99f4c` | A12 |
| 21 | `fox/brief-net4y-cplus-mode.md` | 2026-07-21 | C+ RX-mode audit; `buf0-holds-the-frame` witness | `git log --grep='NET-4y'` → `ba18c1d9 net: NET-4y — 8168 C+ RX-mode audit…`; `grep -rl buf0-holds-the-frame unaos/crates` → rtl8168_tegra.rs | already-landed `ba18c1d9` | A12 |
| 22 | `fox/brief-net4z-rxdesc-addressing.md` | 2026-07-21 | why every RX payload lands at buffer[0]; scan-all witness | `git log --grep='NET-4z'` → `cdd09761 net: NET-4z — refute a driver-side RX-addressing bug; add the scan-all destination witness` | already-landed `cdd09761` | A12 |
| 23 | `fox/brief-netarp1-arp-reply.md` | 2026-07-21 | both boards: do we ever ANSWER an ARP; `[netarp1] smoltcp emitted N frames` | `git log --grep='NET-ARP-1'` → `3ca5adae net: NET-ARP-1 — poll with real time so smoltcp can answer ARP; count what it emits` (+ `c666b8c6` on hw-pi4); `grep -rl netarp1` → genet.rs, rtl8168_tegra.rs, net_phy.rs | already-landed `3ca5adae` (cross-lane by Fox law; both drivers carry it) | A12 |
| 24 | `fox/brief-xcarve8-carveout-extent.md` | 2026-07-21 | the 0xbe carveout extends past 0xc0000000; widen the QUIRK | `git log --grep='XCARVE-8'` → `81b2a415 tegra: XCARVE-8 — widen the 0xbe carveout QUIRK to 96 MiB and split every GiB a window straddles` | already-landed `81b2a415`; the 96 MiB is a labeled GUESS → tracked | C10 |
| 25 | `fox/brief-xcarve9-gib9-extent.md` | 2026-07-21 | the GiB-9 0x26b9 hole extends beyond its guess | `git log --grep='XCARVE-9'` → `0d0beee6 tegra: XCARVE-9 …`; then XCARVE-10 `a6a8d63a`, XCARVE-11 `8154bc1c` (exclude the whole undeclared gap up to the fb carveout) | already-landed `0d0beee6`; superseded by `8154bc1c`; the gap is a labeled GUESS → tracked | C10 |
| 26 | `future/unaos-gneiss-distribution.md` | 2026-07-16 | design SEED: one download (`gneiss_pal`), gneiss resolves the machine; "no arc, no owner, awaits Peter's moment" | `grep -n gneiss docs/ROADMAP.md` → §1c SH-4 (`0b2c1180 docs: ROADMAP §1c SH-4 — gneiss_pal is the host seam…`, Peter 2026-08-17 ruling quoted there); `ls libs/gneiss_pal` → exists | not-a-work-item — a seed, since consumed by ROADMAP §1c (Peter's 2026-08-17 provider-agnostic/offline-arm ruling); no arc owed by orin | — |
| 27 | `metal/unaos-sitting-orin-net4-r22.md` | 2026-07-18 | NET-4 sitting brief: first RTL8168 drive; grep prefix `PCIE4`; three verdict shapes | `grep -n 'ORIN-NET-4' docs/dev/OS/01_BOOT_HAL/arch_arm64.md` → §ORIN-NET-4 (:5786), §ORIN-NET-4b (:5858), NET-4c…4V (:8392–8972); `ls unaos/scripts/orin-net4-bench.md` → exists | already-landed — the sitting flew (boots 2–38 of R23s1 follow it); the surviving open question is the lease | A12 |
| 28 | `past/unaos-metal-jetson.md` | 2026-06-26 | pre-arrival brief: add a `tegra` serial feature (NS16550 at UART_A), GICv3, first UEFI boot | `git log -S'tegra = ' -- unaos/crates/kernel/Cargo.toml \| tail -1` → `38e23a75 Jetson (aarch64): add tegra serial feature — NS16550 UART for Orin Nano`; `grep -n '^tegra' unaos/crates/kernel/Cargo.toml` → :726; orin-ledger §F (serial, GIC, SMP, xHCI all WORKS ON METAL) | already-landed `38e23a75` and everything after it; NVMe is the one listed item still ABSENT (orin-ledger §F row "NVMe", a roadmap gap, not a defect) | — |
| 29 | `past/unaos-jetson-orin-smp6-FORPETER.md` | 2026-07-16 | proposal: legs 21–23 (real entry × rapid 5-core wake) | `git log --grep='ORIN-SMP-6'` → `d3ecf488 arch/aarch64: (ORIN-SMP-6) the LAST-DIFFERENCES legs 21-23`, `a7062721 docs: ORIN-SMP-6 sitting verdict — real entry + wake concurrency both INNOCENT on silicon`; `ls unaos/scripts/orin-smp6-bench.md` → exists | already-landed `d3ecf488`, flown `a7062721`; the SMP residue today is A15 | A15 |
| 30 | `past/unaos-jetson-smp8-relink.md` | 2026-07-16 | executor brief: tegrasmp RELINK, build-only; runbook `orin-smp8-bench.md` | `git log --grep='ORIN-SMP-8'` → `1ebb5d63 docs+bench: (ORIN-SMP-8) the tegrasmp RELINK — layout-axis close-out (BUILD-ONLY)`, merge `ce1f20ca`; `ls unaos/scripts/orin-smp8-bench.md` → exists; arch_arm64 §ORIN-SMP-DEFAULT (:6136) — the 6-core bring-up is now the default | already-landed `1ebb5d63`; superseded by ORIN-SMP-DEFAULT | A15 |
| 31 | `past/unaos-jetson-tegrasmp-relink-FORPETER.md` | 2026-07-16 | the proposal behind #30 | as #30; `grep -rl xcarve_relink unaos/crates/kernel/Cargo.toml unaos/arroyo` → both | already-landed (executed as ORIN-SMP-8 `1ebb5d63`) | A15 |
| 32 | `past/unaos-jetson-xcarve-crcr-FORPETER.md` | 2026-07-16 | proposal: CRCR quiesce (CA / wait CRR=0) before JB9i; event-ring re-seat audit | `grep -n CRCR docs/dev/OS/01_BOOT_HAL/arch_arm64.md` → :4869–4883 (§JETSON-XCARVE census covers DCBAAP/CRCR/ERST; xHCI 5.4.5 CRCR-reads-zero caveat recorded); the smp8 brief names "the CRCR+SMP-7 sitting verdict"; wall later answered by JB11 `abcc1edb` ("the Falcon serves its own firmware header, and that answers the wall") | already-landed — executed as the CRCR+SMP-7 sitting (§ORIN-SMP-7 :5121); the wall itself closed by XCARVE-3…11 (no software writer) and JB11 | C10 |
| 33 | `past/unaos-jetson-xhci-carveout-FORPETER.md` † | 2026-07-16 | proposal: JETSON-XCARVE — relink experiment + inherited-pointer census | `git log --grep='JETSON-XCARVE'` → `360e8209 arch/aarch64: (JETSON-XCARVE) diagnose the xHCI-takeover carveout wall — inherited-pointer census + relink experiment`; `grep -rl UNAOS_XCARVE unaos/arroyo` → yes | already-landed `360e8209` | C10 |
| 34 | `memory/BOOT-metal.md` | 2026-08-01 | seat boot file: "execute BENCH-PROCESS first" | rules file; no tree change implied | not-a-work-item (live seat file; its content is now `docs/dev/LAWS.md` §BENCH) | — |
| 35 | `memory/MEMORY.md` | 2026-08-18 | index; "jetson resume — last verified 2026-07-21" | index; no tree change implied | not-a-work-item | — |
| 36 | `memory/archive/MEMORY.md` † | 2026-08-01 | pre-consolidation index (GR6 baton line, 2026-07-26) | live `memory/MEMORY.md` line 22: "`archive/` holds every pre-consolidation file verbatim — never loaded, never deleted" | superseded by the 2026-08-01 consolidation (Peter: one rule, one place) | — |
| 37 | `memory/archive/pmes-full-push-line.md` | 2026-07-16 | "full push line, every branch, one command" | rule; now CLAUDE.md "Name every push Peter will need in your FIRST turn, batched" | superseded by the consolidation → `UNAOS-LAWS.md` / CLAUDE.md | — |
| 38 | `memory/archive/unaos-check-dont-ask.md` | 2026-07-16 | bench: check state before asking; one-line replies | rule | superseded by the consolidation → `UNAOS-LAWS.md` §BENCH | — |
| 39 | `memory/archive/unaos-enduro-naming.md` | 2026-07-14 | vehicle name open; `Squawk` vessel "not yet scaffolded" | `grep -n -i 'squawk\|talus' docs/NAMING_ATLAS.md` → :121 squawk settled as a comscan capability (2026-07-14), :196 project renamed TALUS (`ffa7148`); atlas commit `39f17b0b` | superseded by `docs/NAMING_ATLAS.md` (both open questions answered) | — |
| 40 | `memory/archive/unaos-flash-staging.md` | 2026-07-15 | never hand a `target/` path; stage to `~/unaos-bench/flash/` | rule | superseded by the consolidation → `UNAOS-LAWS.md` §BENCH & FLASH / `docs/dev/LAWS.md` | — |
| 41 | `memory/archive/unaos-one-executor-at-a-time.md` | 2026-07-23 | one executor per lane; Fox runs 3; refill without being told | rule; superseded twice over: Fox seat retired, then the nine-executor focus fleet (CLAUDE.md 2026-08-26) and `nine-is-a-ceiling` (2026-08-27) | superseded by CLAUDE.md §Worktrees & lanes + LAWS §Throughput | — |

### v1 false positives (matched `tegra` inside `integrator`/`integrated`; not orin-named)

| path | mtime | why listed | verdict |
|---|---|---|---|
| `lc/maestro-intro.md` | 2026-07-17 | Maestro seat intro ("architect–integrator–reviewer") | superseded — the integrator seat was abolished (CLAUDE.md, Peter 2026-08-18) |
| `past/unaos-metal-pi4.md` | 2026-06-26 | Pi bring-up brief ("the integrated kernel") | already-landed (pi track; `kernel8` targets in arroyo) — pi-owned |
| `past/unaos-metal-rmbp.md` | 2026-06-26 | rMBP bring-up brief | already-landed (rmbp track) — rmbp-owned |
| `queue/unaos-wc-video-merge-request.md` | 2026-07-25 | "INTEGRATOR REQUEST — merge hw-pi4's video/ arc to main"; **also**: "138 prune candidates await Peter's explicit OK" | merge: already-landed (`400ff065 video: WC-X86 — activate the window compositor on the x86 panel path`; `video/cursor.rs`, `wcf.rs` present). **Prune list: still open** — `review/unaos-branch-triage-REPORT.md` (2026-07-25) counts 138 PRUNE-CANDIDATE refs; `git branch -r` piped to `wc -l` → 408 remote-tracking refs in this clone today, so the prune never happened and no ruling recorded it. → **S27** (owner rmbp; decision Peter) |

## 4. Rows this sweep adds

- **orin-ledger C10** — the carveout exclusion set carries two bounded GUESS windows (0xbe 96 MiB;
  GiB-9 gap `[0x26c400000,0x279e00000)`) that no DTB node or readable register confirms; the
  extent question re-opens the moment a SNOC Carveout RAS lands outside the set (5 files above
  point at it; nothing tracked it).
- **LEDGER.md S27** — the 2026-07-25 branch triage's 138 prune candidates were never pruned and
  never ruled on (owner rmbp, decision Peter; relayed to the rmbp seat the same turn per R14).

## 5. Marking the files

The sweep does not modify the files it audited (brief rule). Their closure is THIS table: a future
sweep that finds them again resolves them by row number here. `LEDGER.md` P5 is ticked
`owed (pi, rmbp) — orin swept 2026-09-05 6cc8de8c`.
