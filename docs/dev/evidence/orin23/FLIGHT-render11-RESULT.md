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
