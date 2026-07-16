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
hi-half, the FillWrite target is not a directly-inherited pointer we dump — per the §JETSON-XCARVE
coverage caveats that means a controller-internal latched pointer, an inherited command-ring pointer
(a CRCR read returns the pointer field as ZEROS per xHCI 5.4.5 — the `JBXC: CRCR=` line cannot name
it), or an unwalked endpoint-context/transfer-ring pointer (the census reads slot contexts only).
A null census is therefore three-way ambiguous, NOT proof of "controller-internal" — record it
exactly and note it for the fix step.

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

---

## FIX-arc sitting — census v2 (endpoint contexts) + aimed scrub (attended; pre-registered)

The diagnosis sitting's NULL census left the target three-way ambiguous; the FIX arc walks the unwalked
endpoint-context / TR class AND tries an aimed DRAM-structure scrub. New images (hash-verify each on-stick
before boot; `tegra:` count/hash, never size; scrub images carry `JBXC-SCRUB` strings, census `JBXC`):

| tag | knobs | what it is |
|---|---|---|
| **censusv2** | `UNAOS_XCARVE=1 UNAOS_TEGRA=1` | default layout + census **v2** — endpoint contexts (EP state/type/TRDeq) now walked |
| **censusv2-scrub** | `UNAOS_XCARVE=1 UNAOS_XCARVE_SCRUB=1 UNAOS_TEGRA=1` | census v2 + the aimed neutralization (`JBXC-SCRUB:` lines) |
| **censusv2-scrub-leg23** | `UNAOS_XCARVE=1 UNAOS_XCARVE_SCRUB=1 UNAOS_SMPPROBE=23 UNAOS_TEGRA=1` | the historically 4/4-faulting leg-23 layout, census+scrub — the best scrub testbed |
| **knoboff-default** | `UNAOS_TEGRA=1` | byte-identical-to-baseline control (knob-off restore) |

### Boot A — censusv2 image (name/exonerate the TR class)
**Pre-registered prediction:** the `JBXC:` block now includes, under each Configured slot, `JBXC:   ep[dci=k]
state=… type=… TRDeq=…` lines for that slot's endpoint contexts. Even on a FAULTING boot the census prints
pre-eviction. If ANY `TRDeq` carries hi-half `0x80000000` / is flagged `IMPLAUSIBLE`, the FillWrite target
is a **transfer-ring dequeue pointer** — the class the diagnosis could not see (named at last). If every
TRDeq is sane in-DRAM, the TR arm is EXONERATED and the target is controller-internal or the (CRCR-unreadable)
command ring. Capture every `JBXC:` line.

### Boot B — censusv2-scrub-leg23 image, ×2 boots (the prediction FORK)
**Pre-registered prediction (the decisive fork):**
- **poison FOUND + scrubbed + no fault** ⇒ **FIXED.** A `JBXC-SCRUB:` line names the exact poisoned
  structure (DCBAA slot / scratchpad / TRDeq), then the boot passes JB9i cleanly where the old leg-23
  layout faulted 4/4 (and, with `SMPPROBE=23`, runs on to `CORE_READY[1..5]` + CAPSTONE). Re-boot to
  confirm the scrub is stable.
- **nothing poisoned (`JBXC-SCRUB: … no-op`) + fault at JB9i** ⇒ **controller-internal / command-ring
  CONFIRMED.** The poison is not in any DRAM structure the scrub can reach; the wall persists. This is a
  VALID discriminating result — it eliminates the endpoint/TR + scratchpad + DCBAA arms and steers the
  follow-up (a controlled command-ring / internal-latch lever that does not kill the Falcon). Record the
  exact `JBXC-SCRUB:` no-op line + the fault ADDR.
- Any OTHER combination (e.g. a `JBXC-SCRUB:` rewrite followed by a STILL-faulting boot at the SAME ADDR)
  = STOP + record: the scrubbed value was not the (only) carveout target.

### Boot C — knoboff-default (restore)
**Pre-registered prediction:** clean boot, CAPSTONE 6/6, VUG live. The end-of-sitting restore to the
byte-identical-to-baseline default image (zero `JBXC` / `JBXC-SCRUB` strings). End-restore debt paid within
the sitting.

**Best scrub testbed note:** the leg-23 layout (SMP-6 leg-23 tar, `d3ecf48` era) faulted **4/4** — the
highest-probability faulter on record, so a scrub that clears it is the strongest positive signal; the
censusv2-scrub-leg23 image reproduces that layout with the scrub armed.

---

## CRCR-QUIESCE sitting — the command-ring re-seat (`UNAOS_XCARVE_CRCRQ`; attended; pre-registered)

The scrub no-op'd 4/4 (every DRAM-visible inherited class exonerated), leaving the FillWrite target
controller-INTERNAL or in the CRCR-unreadable command ring. This sitting fires a controller-side lever
that stays inside the Falcon-safety invariant: quiesce and re-seat the inherited command ring (CA if
`CRR=1`, then re-program CRCR at OUR ring) BEFORE the first command doorbell (JB9i). Full mechanism +
Falcon-safety argument + ordering proof: `arch_arm64.md` §JETSON-XCARVE "CRCR-QUIESCE arc". New images
(hash-verify each on-stick before boot; `tegra:` count/hash, never size; quiesce images carry
`JBXC-CRCRQ` strings, census `JBXC`):

| tag | knobs | what it is |
|---|---|---|
| **crcrq-leg23** | `UNAOS_XCARVE_CRCRQ=1 UNAOS_XCARVE=1 UNAOS_SMPPROBE=23 UNAOS_TEGRA=1` | the historically-4/4 leg-23 knobs + census + command-ring quiesce — the decisive testbed |
| **crcrq-default** | `UNAOS_XCARVE_CRCRQ=1 UNAOS_XCARVE=1 UNAOS_TEGRA=1` | default layout + census + quiesce (the quiesce on a low-fault layout) |
| **knoboff-default** | `UNAOS_TEGRA=1` | byte-identical-to-baseline control (zero `JBXC` / `JBXC-CRCRQ` strings) |

⚠ **Layout disclosure (read before pre-registering the fork).** Adding the quiesce code CHANGES the
image layout — the exact 4/4 leg-23 layout CANNOT be byte-preserved. This is the FOURTH distinct layout
of the leg-23 knobs; the prior three sampled 4/4 (original), ~50% (relink), 0/4 (census+scrub). The
fault-rate comparison is therefore **statistical**, not a clean before/after on one image. The
`JBXC-CRCRQ:` lines (CRR-before, CA issued/skipped, CRR-after, CRCR re-seated) are the mechanism
witness regardless of the fault outcome — capture every one.

⚠ **REVIEW-LENS CORRECTION (folded pre-bench — read first):** per xHCI §5.4.5 CRR clears whenever
the controller halts, and both prior censuses read raw `CRCR=0x0` at HCH=1 (CRR bit 3 = 0,
observed). So **`CRR-before=0` is THE PREDICTED result**, the CA branch is expected dead, and this
lever is a CONFIRMING PROBE (closes the command-ring bucket by silicon observation), NOT the
likely fix. The fork below is ordered accordingly.

### Boot 1 — crcrq-leg23 image, ×3+ boots (the fork, predicted-first)
**Pre-registered prediction (the fork):**
- **`CRR-before=0` every boot (PREDICTED)** ⇒ the inherited ring was not running; `init_pointers`'
  CRCR write already took; the command-ring bucket CLOSES by observation. The re-seat still leaves
  no window. On clean boots the leg-23 conjunction runs (5/5 cores, CAPSTONE 6/6).
- **`JBXC-CRCRQ:` lines present but the fault PERSISTS at JB9i (`…dc80`/`dc40`)** ⇒ same conclusion
  with the fault sampled in-window: command ring exonerated, target = controller-internal beyond
  the command ring — the FINAL bucket. Record `CRR-before` + the fault ADDR. A valid
  discriminating result, not a failed arc.
- **`CRR-before=1` + CA issued + CRR→0, then clean JB9i ×3+ (would CONTRADICT §5.4.5 on this
  silicon)** ⇒ record as BOTH a silicon-erratum finding AND a strong FIXED signal — both halves
  matter; capture every line exactly.

### Boot 2 — crcrq-default (the quiesce on a benign layout; run if a control is wanted)
**Pre-registered prediction:** default-class images fault ~0/19 historically, so expect a clean boot with
the `JBXC-CRCRQ:` witness lines present (whatever `CRR-before` reads). Confirms the quiesce is benign on a
non-faulting layout (no regression) and shows the `CRR-before` value the firmware leaves on the default path.

### Boot 3 — knoboff-default (restore)
**Pre-registered prediction:** clean boot, CAPSTONE 6/6, VUG live; zero `JBXC` / `JBXC-CRCRQ` strings.
End-of-sitting restore to the byte-identical-to-baseline default image. End-restore debt paid within the sitting.

**Falcon-safety reminder (absolute):** the quiesce writes ONLY `CRCR` (command-ring control). If any boot
shows the Falcon service loop stopping (no post-takeover FW-alive line, controller dead for commands beyond
the CA handshake), STOP the sitting and record — that would violate the invariant and is not expected (CA is
the same handshake `abort_enum_command` runs during normal enumeration).
