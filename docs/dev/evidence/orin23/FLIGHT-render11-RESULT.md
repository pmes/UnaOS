# render11 — FLIGHT RESULT (orin 23, Orin Nano metal, 2026-09-08)

**Verdict: the BOOTROOT contract holds on metal. scorer11 6/6 PASS, exit 0, scored against the FLASHED kernel.elf.**

## Identity (FLIGHTID → wire, in the order the FLIGHTID demands)
| FLIGHTID field | value | wire line |
|---|---|---|
| IMAGE_SHA256 | 57ae5ec38b933e8eaddf7d218e8f66e86b81b49f65b3ca4c9d5d187a7286d765 | (read-back verified by media-writer.sh, 10/10) |
| ELF_MAX_VADDR | 0x34b748 | `KELF min=0x0 max=0x34b748 pg=844` |
| BOOT_VOL_ID | 0xde001a13 | `boot volume FAT serial 0xde001a13 (extended BPB BS_VolID)` |
| build stamp | 600887c2 | `[vfs] root = boot volume … sha=600887c2 …` |

## The root witness, verbatim
```
[vfs] root = boot volume serial=0xde001a13 source=global match=/kernel.elf sha=600887c2 unafs=absent matches=1 home=- files=1 aliased=usb->global window_off=0x25aad5c3c window_len=4096 file_off=0x8bc3 candidates=… disks=… ::
[vfs] volume mounted /volumes/UNAOS-PI source=tegra-sd rw=no ::
[vfs] unafs volume on tegra-sd — unnamed, not mounted ::
[quarry] open volumes mounts=["/", "/apps", "/boot", "/volumes/UNAOS-PI"] roots=["/"] tree-rows=9
```
Root = the reader card (the disk this kernel was FOUND on, by content). The slot card (UNAOS-PI, BS_VolID 0xabfbdefa) is home soil at
`/volumes/UNAOS-PI`, read-only by the TegraSd veto. `aliased=usb->global` is the one-publish pair (the reader card reachable under two source
names), exactly the dedupe the design intends; the clone-merge gap (integrate/BRIEF-AMENDMENT-01 item 5, rmbp-ledger B98) did NOT fire because the
two cards carry different BS_VolIDs. HOMESOIL selftest legs (witness-gated) printed `PASS` on the wire.

## Window provenance
`~/unaos-bench/capture/line-acm0/raw.log`, NUL-stripped, MARK = the `[0000.066] I> MB1 (version…)` cold-boot line at raw line 192656, END = EOF
(196247) at capture time — ONE boot, 3593 lines, filed here as `boot-render11-A1.log`. Butler: host pid 11670 (restarted this session).

## Scorers (outputs filed beside this file)
* `scorer11.sh <wire> <flashed kernel.elf>` → ARMING, LOADER-SERIAL, ROOT-BIND, ROOT-NONE, OLD-BIND-ABSENT, SOURCE-VOCAB all PASS; **exit 0**.
* `scorers-render10.sh <wire>` → exit 1: 8 red / 10 green / 4 note / 1 n-ex. Of the 8 red, SIX are the KNOWN false-red family (UNAFS-MEDIUM,
  UNAFS-RODE, UNAFS-BIND, UNAFS-CENSUS, GROW-GEOM, GROW-LABEL) that key on the `[sdmmc] root` census render11 DELETES — orin21 BULLETIN §31; not
  render11 findings. The other two: **MENUOWN-COL NOT-SCORED** (no tenant box on any of the 4 bar rows — the PULSE window was never focused this
  sitting; action-dependent, not a defect) and **TEARSCOPE-DOCK CONVICT FLAT-VACATE** (`flat=1 scene=yes uncovered_px=5616`: the dock's vacated
  ends were erased to flat DESKTOP_BG over a scene backdrop — a REAL video-lane finding, to the orin ledger; carried, not fixed in this arc).
* `scorers-render9.sh <wire>` → exit 0: A15 5/5 PASS · HEALTH PASS (0 exceptions) · A18 CASCADED · A20 clicks PASS · A27 drag PASS (steered=2)
  · A8 quarry PASS · A26 conquiet PASS · A17 prtscr PASS (SCREEN6..9 OK, 1 in-flight refusal) · A21 tick PASS (tmax 18750) · A21 run HOSTING
  (no `bg` typed → NO SPAWN, unflown) · A12 net5 Q0 ARMED+MATCH, Q1 PASS, Q2 REFETCH-WRONGSLOT=3 (carried), Q3 no lease · A24 rung3 COMPLETE
  (9 UNREADABLE, same as render8) · rung3b MAILBOX-HELD wrote=read=0x5a5aa5a5 · STOP-CHECK OK · A37 SINGLE-SOURCE · A28 NOT-SCORED (correct:
  no `[sdmmc] root` exists any more) · A25 winmenu PUBLISHED, NEVER OPENED (action-dependent) · A16 TCURX consumer never fired (no injection).

## Observations that are NOT render11 findings
1. **The first power-on booted the SLOT card's stale image**: loader `main.rs@743/@912` (an older loader), `Kernel ELF max_vaddr=0x23b968`,
   `boot volume FAT serial 0xabfbdefa`; it reached JB6 (dummy ACPI) and the board cold-booted eight lines later (`I> MB1`, Boot-mode Coldboot);
   the second cold boot took the reader card. UEFI boot order chose the medium; the kernel then found itself on whichever it was handed —
   which is the design ("boot dumb"). The stale image on the slot card is a bench hygiene item: it is the "alternate boot disk" of BULLETIN §21
   in embryo, and today it is stale rather than last-known-good.
2. `orin.log` (the butler's routed stream) carried several kernel lines TWICE (5/5 SMP, CASCADED, HOSTING) while `raw.log` carries them once —
   score from raw, never from the routed file; a butler routing quirk to note (line-butler.py), not a kernel duplication.
3. The loader's console was 80-column-wrapped this boot (`GOP: 5 modes (firmware current 1920x` / `1200):`); the identity fields
   (`KELF max=`, the serial) landed intact before the wraps. `tools/unwrap80.sh` exists for the general case.

## Still owed from this flight
* **FRIEND-DIFF (FLIGHT-render11 §6, three-boot form)**: this boot is **B** (friend PRESENT: the slot card). Owed: **A1 and A2 with the microSD
  OUT of the slot** (friend absent), normalizer built from A1 vs A2 and frozen, then A1 vs B; positive control = the one-line
  read-only-when-friend-mounted mutation. Peter's bench action: pull the microSD, power-cycle twice.
* §18 batteries at 600887c2 (kernel8-test 300, test-arm, UNAOS_WC=1 test) — started in the background at the metal result; rc lines go in the
  landing report.
* Landing blockers (unchanged): clone-vs-alias fix (B98 design, in-seat, one gate), panel + peer ack, then exec-orin22-bootroot → hw-jetson.

## §18 batteries at 600887c2 (run AFTER the metal result, in the throwaway worktree, log filed as `battery-postmetal-600887c2.log`)
| verb | started (UTC) | rc | verdict line |
|---|---|---|---|
| `./arroyo kernel8-test 300` | 2026-09-09T00:00:25Z | 0 | `✅ MBENCH PASS — 125/125 required witnesses, 0 forbidden hit(s), 5980 lines scanned [fast: completion +12.3s grace 20s wall 32.6s]` |
| `./arroyo test-arm` | 00:02:45Z | 0 | (only expected delta vs baseline: FRGUARD ARMED line on aarch64) |
| `UNAOS_WC=1 ./arroyo test` | 00:03:13Z | 0 | banner `witness,ehcihid,kbdwit,sdhcblk,smolnet,wc`; WXAUDIT/U2/TSTE rows PASS |

125 is the Pi floor AT THIS TREE (pi's own tip reads 120; never quote a floor across trees). pi 9's two readings on this run, recorded:
(a) `[shellup]` (their one fast-mode concern) lands ~0.6 s after completion, inside the 20 s grace — the trade holds; (b) do NOT take
`[u7stk] headroom=` from a fast run — `hw=` is a high-water mark and fast mode discards the late samples where it is largest; the
32 KiB launch-stack question needs `UNAOS_QEMU_FULL=1`. `pi4-regression.spec` at this sha names `/usb`/`/volumes` in 2 COMMENT lines and
0 directive rows: the `/volumes` namespace is unexercised on the Pi by any row — a green Pi run is blind to LABELMOUNT, not safe (pi's item).

## FRIEND-DIFF (FLIGHT-render11 §6, three-boot form) — flown 2026-09-09 00:42Z–00:47Z, filed under `friend/`
Boots, same image (57ae5ec3…, stamp 600887c2), same reader card: **A1** friend ABSENT (microSD out of the slot; raw 220184–222672),
**A2** friend ABSENT again (raw 223170–226030), **B** = the render11 boot (friend PRESENT, raw 192656–). Every window cut at the
UnaOS loader's first line (the firmware/DCE preamble is not UnaOS) and at the 12th `[orinrender] census` (boot-relative, before any
user click). Wire: `[vfs] root = boot volume serial=0xde001a13 … sha=600887c2 matches=1 home=- aliased=usb->global` in all three;
`[vfs] volume mounted /volumes/UNAOS-PI source=tegra-sd rw=no` in B only. 0 exceptions, 0 panics, all three.

**Normalizer built from A1 vs A2 only, then FROZEN** (`friend-norm.sh`; the rule: a field is normalised only if it varies between two
boots in the SAME condition): 80-column console unwrap (`tools/unwrap80.sh`), timing/counter fields, the census families
(`:: SCHED:`, `[spread*]`, `[pulse5]`, `[wc-*]`, `[prio]`, `[noatt]`, `[fluid3]`, `[comp2]`, `[strip]`, `[dock] live`, tick lines),
SMMU fault registers (`sGFAR/SYNR/sid/FAR/SCTLR`), cycle/poll counts, and the loader's JB6 line (two cores' output interleaved
character-wise). A1 vs A2 after freezing: 6 surviving lines, all the JB6 interleave garble and the `[irqel2a]` tail.

**JUDGE A1 (absent) vs B (present), allowed-diff list frozen from §6 plus the friend's own presence witnesses
(`:: TEGRA-SD:`, `:: PSRC:`, `[vfs] volume mounted`, `[vfs] unafs volume`, `TEGRA-UNAFS`, `[quarry] open volumes`, `:: SDMMC:`):
64 surviving lines, every one classified (`judge-A1-vs-B.diff`):**
| lines | class | reading |
|---|---|---|
| ~34 | kernel LOAD ADDRESS (`Allocated kernel at`, VBAR, TTBR0, entry=, heap/table addresses, `window_off=`) | UEFI placed the kernel 0x3f0000 lower with the slot card present — the firmware's allocator, not UnaOS; every UnaOS table moved with it and nothing in UnaOS keyed on the address. Stable within a condition, different across — recorded as a firmware observable, not an entanglement. |
| 10 | user keypress in B (`PRTSCR … capture armed`) | Peter pressed PrintScreen before his first click; A1/A2 were untouched. |
| 4 | `[quarry] open … tree-rows=8→9 dirs=7→8` and `entries=30→26` | +1 dir = `volumes/` (the friend — allowed); the file-count delta is TIME ORDER: SCREEN6–9.PNG were written to the card DURING B, so A1/A2 list four more files. |
| 3 | console width (`CON cur=80x25` vs `240x56`, the ANSI clear) | orin-ledger D1: the 80-column wrap happens on some boots; independent of the friend. |
| 5+3 | loader JB6/JB9d interleave garble; `[irqel2a]` register tail | same-condition noise (also present in the calibrate diff). |
| 2 | capture routing artifacts (`=== butler RESOLVED`, a re-routed `dark-window guard` line) | the butler, not the board. |
| 1 | `[menubar] live … paint=…/171us` vs `/170us` | a timing residue the normalizer's order missed; same class as the census fields. |

**Verdict: no UnaOS entanglement with the friend disk found in the boot-to-desktop window.** The only cross-condition fact that is
UnaOS-visible is the load address, and it is the firmware's. ⚠ **This green is NOT yet trusted:** §6's POSITIVE CONTROL (a one-line
"read this only when the friend is mounted" mutation that must RED the leg) has not been flown — it needs a mutated build and a metal
boot, owed with render12. Until then the leg is "did not red", not "proven able to red".

### FRIEND-DIFF, second pass — rmbp 17's PRE-REGISTERED normalization (B90), run after the first pass and reported beside it
rmbp 17 pre-registered the set before A2 existed (the message arrived after my first judge): strip ONLY (a) monotonic counters and
timestamps, (b) addresses and handles; (c) the mount witnesses B90 exempts — NO line drops, ORDER preserved, and every later addition
is a finding with its own justification. **What the first pass did that this one does not, declared:** it dropped whole census-family
lines (`:: SCHED:`, `[spread*]`, `[pulse5]`, `[wc-*]`, `[prio]`, `[noatt]`, `[fluid3]`, `[comp2]`, `[strip]`, `[dock] live`, tick lines)
instead of the counters in them, and it added three allowed-list entries (`:: TEGRA-SD:`, `:: PSRC:`, the butler lines) AFTER seeing the
cross-condition diff. Those are post-hoc and are not used here. `friend-norm-strict.sh`; one addition after the same-condition
calibrate, justified: ALL hex literals are handles (SMMU stream/sid values are hardware-assigned per boot).

| pass | same-condition (A1 vs A2) survivors | cross-condition (A1 vs B) survivors |
|---|---|---|
| strict | 101 — all periodic census families whose EMISSION COUNT/PHASE differs with window length (`[prio]` 10, `[orinbsptick]` 10, `[wc-h]` 7, `[wc-b]` 6, `[spread4/7/9/10]` 6 each, `[pulse5]` 6, `[serialrx]` 4, `[el0live]` 4, `:: SCHED:` 4) + the JB6 interleave garble | 102 — the SAME families at the same counts (the noise floor), plus the lines below |

**Cross-condition survivors OUTSIDE the noise floor (the deliverable, in wire order):**
1. Console width: `CON cur=80x25` + the ANSI clear banner in B vs `240x56` in A1 — orin-ledger D1, independent of the friend.
2. Loader JB6/JB9d interleave garble — same-condition noise (present in the calibrate diff too).
3. **`:: tegra: HEAP-GUARD … clear of 153 carveout range(s)` (A1) vs `165` (B) — NEW, missed by the first pass.** With the slot card
   present the UEFI memory map carries **12 more carveout ranges**; the kernel's heap-guard exclusion set is built from that map, and
   the kernel load address moved (0x3f0000 lower) for the same reason. This is a FIRMWARE-side input difference that UnaOS consumes
   honestly — it is not UnaOS reorganising around the friend, but it is the one place the friend's presence reaches the kernel's
   memory layout, and it is now named.
4. `[quarry] open census … names: … volumes/ …` — the friend's own mount (B90-exempt) plus the time-order file delta (SCREEN6–9 written during B).
5. `[conquiet]`, `JD2 — OUT`, `[tcu] rx-mbox`, `[menubar] live` — each present on BOTH sides, moved by a few lines: emission phase, not content.
6. `:: PRTSCR:` ×10 — Peter's PrintScreen presses in B; every capture wrote to the ROOT volume (`-> OK`, rw=yes), so B90's staining
   vector 1 (`prtscr.rs::mount_capture_target` falling to a friend handle) did NOT fire this boot — expected, since root accepted writes.

**Verdict unchanged and stated with the asymmetry rmbp named: the invariant was not falsified on one image and one pair of boots.**
The named survivors are the deliverable: one firmware input difference (12 carveout ranges), zero UnaOS-side reorganisation. The
positive control (§6) is still owed; without it this leg has not yet been shown able to red.

### FRIEND-DIFF survivor 3, falsifier run (rmbp 17: "diff A1 against A2 — you hold the second pair already")
rmbp pointed out that UEFI moves an image boot to boot on the rMBP with no friend involved (`x86-witness.spec:547-548`, two boots of one
image at different load addresses), so with one pair the 0x3f0000 shift could not be attributed. The free falsifier, run on the data:
| boot | condition | `Allocated kernel at` | `VBAR_EL1` | `HEAP-GUARD … clear of N carveout range(s)` |
|---|---|---|---|---|
| A1 | friend ABSENT | 0x25ae2a000 | 0x25afbb800 | 153 |
| A2 | friend ABSENT | 0x25ae2a000 | 0x25afbb800 | 153 |
| B  | friend PRESENT | 0x25aa3a000 | 0x25abcb800 | 165 |
A1 and A2 agree EXACTLY; only B differs. On this board the load address is stable across cold boots in one condition and moves with the
friend, and the 12 extra carveout ranges move with it — the correlation survives (n=2 vs 1). Still a firmware-side input, still not a
UnaOS reorganisation; now attributed rather than assumed. (The rMBP's boot-to-boot variance is a different firmware.)

### FRIEND-DIFF: the criterion, the live channel, and the positive control's DESIGN (rmbp 17, B90 ticked with 101/102)
* **Benign-survivor criterion (into B90):** a survivor is benign iff UnaOS's behaviour is a pure function of the changed input AND the
  change is not UnaOS-caused. Survivor 3 (12 carveout ranges → load address) passes, traceably. "Consumed honestly" alone would absorb
  every future survivor; the criterion is what makes the next reader able to refuse one.
* **Named channel, currently harmless:** nothing in UnaOS makes a DECISION on the load address today (x86 specs wildcard `img=[…]`;
  STAMP-MATCH keys on the build sha). The moment anything keys on an absolute address or a load-derived span, the friend gets a
  channel to change behaviour through the firmware without touching our code. Watch for it at every landing.
* **Survivor 6 is a CONDITIONAL pass, not a pass.** B90's staining vector 1 (`video/prtscr.rs::mount_capture_target` falling to rung 2,
  the USB handle) fires only when the boot volume REFUSES WRITES. All ten captures went `-> OK` to root, so the vector never entered its
  firing condition — the flight declined to trigger it, it did not test it.
* **Positive control, designed:** FRIEND PRESENT **and** ROOT REFUSING WRITES. Bench shape on the Orin: rung 2 is the USB handle and the
  slot card is TegraSd (not USB, and vetoed read-only), so the friend for this control must be a USB disk with a FAT volume, and the
  root reader card must refuse writes (its physical lock switch, or FRGUARD's `default_writable()` forced off). Then press PrintScreen:
  captures landing on the friend = invariant falsified, B90 was right; captures REFUSED with a witness = vector 1 genuinely closed.
  Either outcome is worth more than another clean pair. Owed at render12; needs Peter to supply a USB stick.
