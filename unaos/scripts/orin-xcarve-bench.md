# ORIN-XCARVE bench runbook — the xHCI-takeover carveout wall (attended; diagnosis-only)

This bench diagnoses the NEW wall the ORIN-SMP-6 sitting (2026-07-16 attended) surfaced: some kernel
images RAS-power-off the box in ordinary early boot, during the xHCI inherited-state takeover, and
WHETHER an image faults is decided by its build layout, not by what it does. Full characterization:
`docs/dev/OS/01_BOOT_HAL/arch_arm64.md` §JETSON-XCARVE and §ORIN-SMP-6 sitting verdict.

**Signature (do not re-derive — this cost 11 attended boots):** RAS SNOC `SERR=0xd` "Illegal address
(software fault)" + IERR Carveout Uncorrectable `0x3`, paired ACI `SERR=0x4` IERR FillWrite `0x9`;
**FIXED fault ADDR `0x800000027767dc80`** (once `+0x200`) — bit 63 set, low part `0x27767dc80`
(~9.86 GiB, near the DRAM top of the [2 GiB, 10 GiB) window); fires right after the xHCI `JB9i`
inherited-slot-eviction step (`JB9i — inherited-slot eviction: DISABLE_SLOT 1..8 issued + drained` is
the last line before the box dies). Image-layout-correlated: leg-23 image faulted 4/4; leg-21/22 ~50%;
0/19 across all prior sittings. Keyboard EXONERATED (fault reproduced with only the boot stick on the
bus). **This arc is DIAGNOSIS ONLY — no fix ships here.**

## What a sitting needs (complete)

Boot stick only (NO keyboard, NO data card — keyboard exonerated); barrel supply (RAS recovery = DC
cut); serial bridge (Pi Debug Probe, 115200). 2–4 boots per the schedule below. Every image
hash-verified on-stick pre-boot (`tegra:` count/hash, never size). Media staged + shas at landing.

## Hard rules

1. **Predictions are PRE-REGISTERED (the table below, written BEFORE any boot).** A boot that behaves
   where the opposite was predicted = STOP the sitting, record exactly what was seen, report.
2. **Power-fault boots are DATA.** Recover with a FULL DC CUT (unplug the barrel supply, wait, replug
   — a warm reset can leave the CBB/MCE poisoned) and continue only per the schedule.
3. **Read-only diagnosis.** The `JBXC:` instrumentation is pure CPU reads of inherited state, printed
   BEFORE the takeover overwrites it and BEFORE JB9i — so even a FAULTING boot yields the pointer
   census. Nothing writes persistent state.
4. **Serial capture:** `awk` / `grep -a` on the log, never plain `grep` (control bytes).

## The images (staged; hash-verify each on-stick before boot)

| tag | knobs | what it is |
|---|---|---|
| **instrumented** | `UNAOS_XCARVE=1 UNAOS_TEGRA=1` | default layout + the inherited-pointer census (`JBXC:` lines) |
| **relinked-leg23** | `UNAOS_XCARVE_RELINK=1 UNAOS_SMPPROBE=23 UNAOS_TEGRA=1` | the 4/4-faulting leg-23 image, relinked (+16 KiB pad ⇒ whole image shifts) |
| **relinked-default** | `UNAOS_XCARVE_RELINK=1 UNAOS_TEGRA=1` | default, relinked — layout cross-check |
| **knob-off default** | `UNAOS_TEGRA=1` | byte-identical-to-baseline control |

## The boots (one image per boot; STOP at the first prediction-violating result)

### Boot 1 — instrumented image (the pointer census)
**Pre-registered prediction:** the boot prints a block of `JBXC:` lines at `jb2b_attach` entry,
pre-takeover / pre-JB9i. **Among the inherited pointers, expect exactly one whose raw 64-bit value is
`0x800000027767dc80`** (or a class carrying hi-half `0x80000000` / lo-half `0x7767dc80`) — that pointer
NAMES the FillWrite target. The inherited pointers are firmware-set, so they appear whether or not THIS
image itself goes on to fault. Capture EVERY `JBXC:` line. If NO pointer carries the `0x80000000`
hi-half, the FillWrite target is not a directly-inherited pointer we dump (it is a controller-internal
latched pointer) — record that and note it for the fix step.

### Boot 2 — relinked-leg23 image (the decisive layout test)
**Pre-registered prediction:** the +16 KiB relink shifts the entire image (`.text`/`.data`/`.bss` all
move by `0x4000`; see §JETSON-XCARVE). Expect the 4/4 fault to **VANISH or MOVE** (no fault, or a
different address / different step). If it vanishes, layout correlation is **PROVEN on this image AND
leg 23 is unblocked** — the boot should then reach `sel=23`, run the print-free 5-core burst, and print
`CORE_READY[1..5]` + five `:: AARCH64 SMP: AP <n> online … ::` lines (its SMP-3-replay verdict rides
this same boot — record it against `scripts/orin-smp6-bench.md` leg-23 row). If it STILL faults 4/4 at
`0x800000027767dc80`, layout correlation is **REFUTED** for this image ⇒ STOP: the fault is not (purely)
layout, and the fix theory must change.

### Boot 3 — relinked-default (layout cross-check; run if boots 1–2 leave the question open)
**Pre-registered prediction:** default-class images fault 0/19 historically, so this is a control —
expect a clean boot. A fault here would be new information (the relink moved a previously-safe image
INTO the carveout) and = STOP + record.

### Boot 4 — spare repro (instrumented OR relinked-leg23, operator's call)
Re-boot whichever result most needs a second sample (e.g. re-confirm the `JBXC:` `0x80000000` pointer
value is STABLE across boots — a firmware-deterministic value diffs identically; a garbage read would
not).

## After the sitting

Hand the serial log to LC-orin. The `JBXC:` census + the relink verdict together name the mechanism:
which inherited pointer class carries `0x800000027767dc80`, and whether shifting our image's layout
moves us off the carveout. That pair is the input to the (separate, reviewed) fix step — see
§JETSON-XCARVE "Proposed fix direction".
