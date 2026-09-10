# FLIGHT 5 POSTMORTEM — rmbp9flight5 @ 2980dbe8 · flown 2026-08-28

Image `UnaOS-rmbp-esp-rmbp9flight5-20260828T1740Z-2980dbe`, kernel.elf `a9b6cdf7…`,
SRC.TGZ `d067799f…`, card byte-verified 20/20, MANIFEST PASS.
Knob line = **flight-4's, verbatim** — single variable, only the 5 wave-2 commits differ.
`UNAOS_BAR1EXP` deliberately NOT armed (UC is ~6.8x slower and would corrupt the power numbers).
Capture: `~/unaos-bench/capture/rmbp9-flight5/ttyUSB0.log`, 1 316 881 bytes, two boots.

> **THE HEADLINE: the BT arc is dead on hardware, and the iGPU premise this arc inherited is
> false.** Both were discovered by flying, not by reasoning. Two boots converted three open
> software hypotheses into one hardware fact.

---

## 1. Verdicts vs the playbook's seven predictions

| # | prediction | verdict |
|---|---|---|
| 1 | RESUMEPAINT first-present witness on the vugpause2 resume edge | **FIRED** — `[vugpause2] resume slot=11 edge=unhide woken=1 n=1`, then `n=2` |
| 2 | FURNITUREFOCUS — furniture click keeps the keyboard | **LIVE, unproven** — `deflect=true` on the router, `[wc-fv] focus shell … furniture=2/3`. The keystroke proof could not run: HID was dead for most of the boot (§4) |
| 3 | DEADKBD5 retire names its consequence | **FIRED** — addr=5 armed at 1742 ms, `SILENCE-CUT-BY-HALT class=xact-err-burn … -> retire`, quiet 24 303 ms |
| 4 | ABSENCE CONTROL: `fb-wc` present, `bar1exp` never | **PASS, both boots** — `fb-wc=8, bar1exp=0`. The flight is valid |
| 5 | iGPU: how far the mux switch gets | **PREMISE FALSIFIED — see §3.1** |
| 6 | BT: is AVDTP DISCOVER answered | **UNREACHABLE — hardware, see §3.2** |
| 7 | clickroute FAIL: new-with-arc or pre-existing | **NOT new with these 5 commits** — reproduced identically, `desktop=false` |

---

## 2. Two-boot timeline

**BOOT 1 — DOA at 2 305 bytes.** Died mid-line writing the SECOND `WXPROBE map`. Last complete
line was `at=kimg`; the healthy order is `kimg → ktext → ap8000 → fb → bss → lapic → elf`, so it
died entering `at=ktext`. Everything before that was clean: `fb-wc` retyped 15 leaves WC, WRITER
seeded 2880x1800, x2APIC up, SMEP on, WXN swept, `WXAUDIT-0 -> PASS`, `WXN-FBWC -> LEAF
BIT-IDENTICAL`.

**BOOT 2 — clean, 566 s.** Same card, no changes. Walked straight past `ktext` and every later
probe into memory init, SMP (8 CPUs), the desktop, and 9.4 minutes of operation.

> **DO NOT CONVICT THE ARC ON BOOT 1.** 1-fail / 1-clean is not a bisect. An initial suspicion of
> PHASE31ROOT (the only mm commit in the delta) was raised and **withdrawn** — a single DOA against
> a single clean boot convicts nothing, and orin 10's contemporaneous 2-of-5 warning is the same
> lesson from the other track. If boot 1's shape recurs, bracket it against the flight-4 image
> before naming a commit.

---

## 3. Per-lane findings

### 3.1 iGPU / power — THE INHERITED PREMISE IS FALSE
The rmbp-9 baton states: *"today `UNAOS_GMUX_IGD` switches and restores on the same call stack —
making it stay is design work this arc owns."* **It does not switch at all.**

    :: igpu: [GMUX] REFUSED: pre-switch-not-accepted (status: 0x00000000) ::
    :: igpu-dpy: LADDER highest=00/10 name=harness ok=0 pending=0 gmux=UNTOUCHED
                 why=pre-switch-not-accepted elapsed_ms=1 ::
    :: igpu-blt: ring=absent why=no-active-surface — every iGPU display plane is off
                 (gmux routes the panel elsewhere); CPU path carries the console ::

Ladder rung **0 of 10**, `gmux=UNTOUCHED`, refused at the pre-switch handshake with status 0.
Residency is therefore NOT an "remove the unwind" problem — nothing to unwind, because nothing
starts. The next question is the pre-switch handshake itself.

**SMC has no AC wattage key on this machine:** `SMC-SCOUT: key AC-W absent (x2) — this SMC does
not carry it (clean negative answer, not a fault)`. AC presence is inferred from the B0AC sign.
Keys that DO exist include `WCPW`, `SMBW`, `MSDW`, `MSQW`, `HDSW`. A watt baseline must be built
from those, not from `AC-W`.

### 3.2 BT — ANTENNA FAULT, CONFIRMED. STOP THE SOFTWARE ARC.
Inquiry ran the full 10 240 ms with all three result shapes unmasked (bits 1, 33, 46):
`responses=0 target_found=false read_to_term=true`. Two blind pages, both `status=0x04` PAGE
TIMEOUT. No link this boot.

**The deciding control was run by the operator and it settles it:** both BT devices were on and in
the required state for the whole window *and well after*, and **macOS Catalina on the same machine
cannot discover either device either.** That is not our stack, not firmware, and not a missing
`.hcd`.

**TWO INHERITED CLAIMS CORRECTED HERE — both were repeated into this session before being checked:**

1. **Flight 4 never paired.** The baton and resume say "FIRST BR/EDR PAIRING + encryption + open
   L2CAP AVDTP". The flight-4 capture says the LINK was real and the PAIRING FAILED:
   `Simple Pairing Complete status=0x18 -> NOT PAIRED`, `Authentication Complete status=0x05 ->
   NOT AUTHENTICATED`, `SSP tally … pairing_complete=FAILED encrypted=false -> NOT BONDED`.
   **This project has never completed a BR/EDR pairing.**
2. **The RSSI history was misread as exoneration.** Every LE reading this project has ever taken,
   across 17 captures from gr24 to flight 5, is **−82 to −96 dBm for a device in the same room**,
   where healthy is −40 to −60. The values are FLAT, which was correctly observed and wrongly
   interpreted: flat-and-uniformly-terrible is not "unchanged, therefore fine". It is 25–40 dB
   below healthy and always has been.

⇒ **The "boot-scoped inquiry-substate deafness" model carried since flight 3 is retired.** A link
budget too thin to close explains every observation better: links form occasionally when a blind
page happens to align, and the multi-round-trip SSP exchange never survives.
⇒ **The `.hcd` hunt is closed.** Verified by content, not assumption: the operator's Catalina
`IOBluetoothFamily.kext` contains NO firmware — `Write_RAM` (opcode `0xFC4C`) scores 0–3 across
every binary in the bundle against a control (`00 00`) scoring 12 000–103 000. A real `.hcd`
carries hundreds. `~/Downloads/bcout/b43/` is b43 **Wi-Fi** ucode from `wl_apsta.o` and
structurally cannot contain BT patchram.
⇒ **CONFIRMED BY RANGE TEST, 2026-08-28 (operator): a BT mouse connects only barely and
disconnects at TWO FEET.** Spec range for a class-2 link is ~10 m. A link that dies at ~0.6 m is
roughly **30 dB down** — which is precisely the deficit measured in the RSSI history above, from an
independent direction. Two independent measurements agreeing on the same number closes it.
**THE FAULT IS THE ANTENNA (or its connector), NOT THE STACK, NOT THE FIRMWARE, NOT THE MODULE.**
The operator replaced the machine's battery before this arc; on MacBookPro10,1 the bottom-case
removal routes directly past the AirPort card's U.FL antenna connectors, which are small and easy
to unseat or pinch on reassembly. A partially-seated U.FL gives exactly this signature: enough
parasitic coupling to hear a strong nearby transmitter, nowhere near enough to close a link.
**Fix is physical — reseat the U.FL connectors on the AirPort card.** No software change helps.

**WI-FI IS HEALTHY ON THE SAME MACHINE (operator, 2026-08-28), AND THAT NARROWS IT.** The card
itself is alive and the reassembly did not kill the RF path wholesale. Wi-Fi on this board is
MIMO across multiple antenna leads and degrades gracefully — losing one lead costs throughput, not
association. Bluetooth has no such redundancy: it rides a single lead, so one unseated connector
takes 100% of the BT link budget while Wi-Fi barely notices.
⇒ **The fault is ONE specific antenna lead — the one BT uses — not the assembly.** When reseating,
the target is the connector that is not fully clicked down; a working Wi-Fi is NOT evidence that
the others are seated.

⚠ **THE RECORD-CORRECTION THIS FORCES IS LARGER THAN THIS FLIGHT.** Every BT "finding" this project
has recorded was measured through a ~30 dB deficit and must be re-read as suspect, including:
`BTRX`'s "doubled inquiry heard the Megaboom (first classic RSSI −96 dBm)" — that is the noise
floor, not a detection; "deafness = boot-scoped inquiry substate"; "old inquiry was a systematic
timing miss"; "there is no regression"; and the entire BTDIR / patchram / BTREGRESS line of
reasoning. **None of those are safe to build on.** They are not necessarily wrong, but every one
was inferred from a radio that could not close a link at two feet, and none of them was ever
controlled against a known-good RF path. Re-derive before citing. Do not re-fly BT experiments
until a post-reseat range test passes.

### 3.3 VUG — the detail ladder cannot climb out of its own floor
Six `/fat/VUG.ELF` instances, same binary. The third HUD digit is `lod` (`main.rs:3104`,
`hud3 = lod`): **level 0 is the classic wireframe; 1..=3 are the ray-traced shard**
(`main.rs:806`). Windows rendering "broken wireframe" are not corrupted — they are on the floor
rung. Only **26** `[vuglod]` lines exist and the last is at t=202 s:

    t=192841..192914 (73 ms)   13 demotions, EVERY ONE reading fps 000
    t=202100..202294 (194 ms)  four vugs walk 1→2→3 at fps 246/266/280/284

Both events are **synchronized** — six independent controllers do not fall together in 73 ms and
rise together in 194 ms. `lod_adapt` feeds on ACHIEVED FPS, which on a six-vug desktop is a
contention signal every vug moves, not a property of the machine. The dead band (`LOD_DOWN=24`,
`LOD_UP=55`) and `CALM_WINDOWS=8` set the oscillation period; they cannot remove it.

**Two defects, cleanly separable:**
1. **A transient stall is indistinguishable from a capability verdict.** Every demotion read
   fps **000** — nobody got a frame for a whole window. The ladder treated a freeze as "this
   machine cannot sustain level 3".
2. **The floor is a trap.** On demotion `*ceil = nl`, and promotion requires `lod < *ceil`. A vug
   that reaches 0 has ceiling 0 and can only escape via `*calm >= CALM_WINDOWS` raising the
   ceiling one rung at a time — and the whole adapt path is nested inside `if overlay`, so
   anything clearing `overlay` freezes the ladder AND forces `flod = 0` (`main.rs:3180`).
   **Two vugs finished the boot at lod 0 while running 290 and 243 fps, 364 s after the stall.**

**Operator intent, recorded verbatim (Peter, 2026-08-28):** *"the idea is it loads the level of vug
the machine will support but it ends up bouncing around between versions which is no good."* That
is a CAPABILITY MEASUREMENT — a one-shot property of the hardware — not a live controller reading
a contended signal. `LOD_PIN` already exists (`VUGX.ELF` ships as `pinhi`); what is missing is the
arbiter that picks the number. Open question for the next arc: kernel-side arbiter (one level
published for all vugs, cannot oscillate by construction) vs per-process startup probe (smaller
change, still N independent measurements).

### 3.4 PRTSCR — FIRST SUCCESS ON METAL
    [463310ms] :: PRTSCR: SCREEN2.PNG 2880x1800 15555053 bytes -> OK ::
    [463454ms] :: PRTSCR-ST: SCREEN2.PNG on the medium — PNG signature OK,
               IHDR 2880x1800 depth 8 colour 2 non-interlaced, IEND OK -> PASS ::
Rung 2 works. **Triggered by the volume ARRIVAL selftest, not by a keypress** — HID was dead at
that moment (`hid_ms=197678`), so no chord can have caused it. The boot veto is healthy and
expected: `PRTSCR-ST: program source is sdhc and vetoes writes … still waiting for a writable
volume`.

**Operator ask, and it is a real gap:** the internal Apple keyboard does not emit usage `0x46`,
so no chord the operator presses can trigger a capture. Apple's own chords are **⌘⇧3 / ⌘⇧4**;
binding `GUI+Shift+3/4` is the fix and the modifiers are already in the HID report byte.

### 3.5 Compositor — faithful everywhere except one window
Every window reports `decl_geom=0 decl_cap=0 decl_lock=0 decl_alloc=0 stalls=0`. The compositor
is not the cause of the vug geometry (§3.3). One genuine anomaly:

| win | torn | banded | minspan | whole | maxpresent_us |
|---|---|---|---|---|---|
| **2** | **170** | **0** | **0** | **19 210** | **27 650** |
| 3 | 6 | 419 | 18 | 8 465 | 9 602 |
| 4 | 4 | 288 | 17 | 6 408 | 9 103 |
| 5 | 10 | 25 | 128 | 8 456 | 8 982 |
| 6 | 9 | 26 | 128 | 8 435 | 8 974 |

Window 2 produces **no damage bands at all**, so it full-blits every frame and tears constantly,
at 3x the present cost of any sibling. Matches `[wc-d] paygo-taker STOP-NOTE win=2 — gave up after
16 attempts: the row is marked damaged and the composite declines to sample it`.

---

## 4. The felt symptoms, quantified

**"input stayed live a while but dead now."** `deadman` `hid_ms` is the record:
HID died at **t≈265.7 s**, stayed dead **198 s**, ONE event landed at **t≈464.3 s** (the operator's
screenshot-chord attempts), and it was dead again for the remaining 100 s. `in=0/0/0` throughout
the tail. The compositor stayed alive the whole time (rollups still printing) — this is HID death,
not a wedge.

**No wedge this boot.** `[wcser] steals=0 revenants=0`, `[wedge1] tripwire=silent` at every rollup,
`[wedge11] -> QUIET`. The disease did not fire in 566 s.

**Boot cost:** `BPACE: ehci-hid-done t=25331ms d=25036ms` — EHCI HID enumeration alone is **25.0 s
of a 25.4 s pre-scan boot**, and `EPACE` attributes it entirely to `init` (`selftest=0ms evid=0ms
init=25036ms`).

---

## 5. Lanes for the next arc, ranked

1. **VUG LADDER → CAPABILITY PROBE.** Peter's stated intent, a named mechanism, and two defects
   with wire witnesses. Decide the arbiter (kernel-side vs per-process) first — it is a design
   call, not a tuning one.
2. **THE 192.8 s GLOBAL STALL.** Every vug read fps 000 in the same 73 ms. Unexplained, and it is
   the event that poisons the ladder. Likely the same family as the HID death at 265.7 s. Highest
   information-per-boot of anything left.
3. **iGPU PRE-SWITCH HANDSHAKE.** Rung 0 of 10, `status: 0x00000000`. The centerpiece's surviving
   half, now with a real question instead of an assumed one.
4. **PRTSCR ⌘⇧3/⌘⇧4 BINDING.** Small, operator-requested, unblocks hands-on capture.
5. **BT: BLOCKED — ANTENNA FAULT CONFIRMED (2-foot mouse dropout).** No software lane at all until
   the connector is reseated and a range test passes. Then the FIRST job is not a new experiment:
   it is re-deriving which of this project's recorded BT findings survive a working radio.
6. **Ledger:** `clickroute route … desktop=false -> FAIL` reproduced (QEMU green, metal FAIL, not
   new with this delta); `DOCK … vacate=false :: FAIL ::` on metal where QEMU has it true;
   `wc-w amp=1.33x full_presents=12`.

---

## 6. Appendix — byte offsets for direct seeking
`~/unaos-bench/capture/rmbp9-flight5/ttyUSB0.log` (boot 1 = bytes 0–2305; boot 2 = 2306–end)

| offset | line |
|---|---|
| 16 | `x86 fb-wc: retyped` (boot 1 first line) |
| 2306 | boot 2 begins |
| 47112 | `inquiry summary` — responses=0 |
| 70940 | `AC-W absent` |
| 100251 | `GMUX` REFUSED / ladder 0/10 |
| 216994 | `FB Init` |
| 218399 | `SILENCE-CUT-BY-HALT` (DEADKBD5) |
| 280949 | `clickroute] route` → FAIL |
| 325060 | `vugpause2` (RESUMEPAINT) |
| 335217 | `vuglod` (first of 26) |
| 450344 | `paygo-taker STOP-NOTE win=2` |
| 1077163 | `PRTSCR: SCREEN2.PNG` |

---

## 7. Method notes worth carrying
- **Two of this flight's three analytical errors were INHERITED claims repeated without
  re-derivation** (flight 4 "paired"; the baton's gmux "switches and restores"). A baton is a
  starting point, not evidence. Re-derive before repeating, especially when quoting to a peer.
- **A zero from a broken tool is not a result.** The `.hcd` scan first returned all-zero because
  `xxd` was absent; it was only meaningful once re-run with a working tool AND a control pattern
  that had to hit. Same shape as the scour's citation-gate hazard.
- **`--stat`'s total-changed is not insertions.** A wm.rs figure quoted to orin 10 as "+592 lines"
  was the 582+10 total. The number was real; the label was wrong.
