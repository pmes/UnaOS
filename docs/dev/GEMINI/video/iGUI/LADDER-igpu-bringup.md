# iGPU Flight 1 — the IVB eDP internal-panel bring-up ladder

**Machine:** MacBookPro10,1 (2012 15" Retina). Intel HD 4000 / Ivy Bridge GT2 at BDF `0:2:0`
(`VID:DID=0x01668086`), Panther Point PCH. Internal panel **2880x1800**, eDP on **Port A**
(CPU-attached / North Display Engine). Discrete GK107 Kepler currently owns the panel through the
Apple gmux (`SW_DISPLAY=0x03 DIS`, `SW_DDC=0x02 DIS`, `DISC_POWER=0x03 ON`, gmux v3.2.19).

**Scope decision (Peter, 2026-08-07):** Flight 1 **grows** to include full IVB display bring-up.
The gmux switch alone cannot light the panel — firmware left the entire iGPU display pipeline
unconfigured. Panel risk is accepted; **the serial link is the debug path.**

**Base:** `wt/gmux-igd-x86` @ `2be56eb2`. All `igpu.rs:NNN` line references below are into that sha.

---

## 0. Ground truth — what metal actually says (Boot Z, `~/unaos-bench/capture/s73-UNAOS.LOG.saved`)

Every number in this table is read off the wire, not assumed. It is the starting state each rung
must move.

| Register | in-tree offset | metal value | reading |
|---|---|---|---|
| `PIPEACONF`/`B`/`C` | `0x70008`/`0x71008`/`0x72008` (`igpu.rs:5-7`) | `0x00000000` | all three pipes off |
| `DSPACNTR`/`B`/`C` | `0x70180`/`0x71180`/`0x72180` (`:15-17`) | `0x00000000` | all three planes off — bit 31 clear |
| `DSPASURF` | `0x7019C` (`:20`) | `0x00000000` | no scanout surface |
| `DP_A` | `0x64000` (`:38`) | `0x0000001C` | port **disabled** (bit 31 clear); low bits are firmware residue — **decode TBV** |
| `PP_STATUS` (CPU) | `0x61200` (`:40`) | `0x00000000` | — |
| `PP_CONTROL` (CPU) | `0x61204` (`:41`) | `0x00000000` | — |
| `PCH_PP_STATUS` | `0xC7200` (`:63`) | `0x00000000` | panel power **off** |
| `PCH_PP_CONTROL` | `0xC7204` (`:64`) | **`0xABCD0008`** | unlock key `0xABCD` in `31:16` + bit 3 set. **This is the live PPS.** |
| `PCH_PP_ON_DELAYS` | `0xC7208` (`:65`) | `0x00000000` | **T1..T8 NOT programmed** |
| `PCH_PP_OFF_DELAYS` | `0xC720C` (`:66`) | `0x00000000` | **T9..T10 NOT programmed** |
| `PCH_PP_DIVISOR` | `0xC7210` (`:67`) | `0x00186904` | reference divider **is** programmed (`>>8 = 0x1869 = 6249`), cycle-delay field `= 4` |
| `DPLL_A_CTRL` | `0x06014` (`:42`) | `0x00000000` | no PLL |
| `FPA0`/`FPA1` | `0x06040`/`0x06044` (`:59-60`) | `0x00000000` | no divisors |
| `FDI_RXA_CTL` / `FDI_TXA_CTL` | `0xF000C` / `0x60100` (`:55-56`) | `0x00000040` / `0x00040000` | reset defaults; **eDP on port A bypasses FDI** (tree's own citation, `igpu.rs:728`) |
| `PCH_GMBUS2` | `0xC5108` (`:72`) | `0x00000800` | bit 11 set — controller idle/ready (**TBV**) |
| `PCH_GMBUS0/1/3/4` | `0xC5100`… (`:70-74`) | `0x00000000` | — |

Panel geometry, also from Boot Z:
`:: video: WRITER seeded base=90020000 len=29491200 panel=2880x1800 stride=4096px pitch=16384B bpp=4 ::`
— i.e. the console framebuffer is **Kepler VRAM**, stride padded to 4096 px. The iGPU cannot scan
out of it (see rung 1).

**Three findings in that table do real work in this ladder and must not be lost:**

1. **`PCH_PP_CONTROL = 0xABCD0008` while `PP_CONTROL` (CPU, `0x61200` block) reads `0x00000000`.**
   That is empirical proof that the **PCH** PPS at `0xC7204` is the sequencer for this panel, not
   the CPU-block one at `0x61204` — settled by measurement, not by doctrine. If `0x0008` is
   `EDP_FORCE_VDD` (**TBV**, i915 `intel_pps.c` names bit 3 that way), firmware has already forced
   panel VDD on, which means **AUX may already work before we touch the PPS at all.** Flight 1b
   tests exactly that.
2. **`PP_ON_DELAYS`/`PP_OFF_DELAYS` are zero but `PP_DIVISOR` is not.** The T1–T12 timings the panel
   needs are **not** programmed. Firing the PPS with zero delays is the single most likely way to
   damage or hard-hang the panel. Rung 2 must program them before it asserts power.
3. **`igpu-blt: ring=absent why=no-active-surface`** is twice-confirmed on metal. The existing
   liveness criterion (`PIPE_FRMCOUNT_A` advance + `DSPACNTR` bit 31) reads zero because *nothing
   programs the pipe* — which is precisely what this ladder fixes. The criterion is correct; it was
   simply unreachable.

---

## 1. Corrections to the assumed rung list

Three items in the brief's straw-man ladder are wrong or incomplete for Ivy Bridge. Fixing them
before code is written saves a bench sitting each.

- **`DP_TP_CTL` / `DP_TP_STATUS` do not exist on Ivy Bridge.** They are the **Haswell+ DDI**
  transport-control registers. On IVB, eDP link training is driven through **`DP_A` (`0x64000`)
  bit fields** (port enable, link-train pattern select, port width, pipe select) paired with
  **DPCD writes over the `DPA_AUX` channel**. Any lane code that reaches for `DP_TP_*` on this
  machine is programming a register that isn't there. **NEEDS-VERIFICATION** against PRM Vol 3
  Part 4 — but do not start from the HSW register names.
- **The eDP PLL is not `DPLL_A`.** On ILK/SNB/IVB, eDP on port A drives a dedicated eDP PLL
  configured from **bits inside `DP_A` itself** (enable + a 162/270 MHz frequency select), not from
  `DPLL_A_CTRL`/`FPA0`/`FPA1`. The metal reading `DPLL_A = FPA0 = FPA1 = 0` is therefore *not*
  necessarily a gap for this path. **NEEDS-VERIFICATION** — and note that i915's own headers appear
  to overlap `DP_PORT_EN` (bit 31), `DP_PLL_ENABLE` (bit 30) and `DP_PIPE_SEL_MASK_IVB` (bits
  30:29); **the PRM must settle that overlap before a single `DP_A` write is issued.**
- **Two rungs are missing from the straw man and both produce a black panel:**
  - **Where the pixels live** (rung 1). `DSPASURF` is a **GGTT offset**, not a physical address.
    There is currently no iGPU-visible framebuffer at all.
  - **Display watermarks** (rung 8b). On IVB, unprogrammed `WM0_PIPE*` values make the display FIFO
    underrun. Symptom: the pipe enables, `PIPE_FRMCOUNT_A` **advances**, and the panel is black or
    torn. That failure looks like success on every other rung's predicate, which is exactly why it
    needs its own.

A fourth item, **backlight**, is deliberately *not* a liveness gate — see rung 10.

---

## 2. Blockers inherited from `2be56eb2` — fix before rung 0

These are from the GR20 adversarial review (`~/unaos-bench/scratch/gr20/review-igpu-fixes.md`).
Each one becomes materially more dangerous once the ladder actually programs hardware.

| # | Defect | Why it blocks the ladder |
|---|---|---|
| N4.1 | `IGPU_BAR0.store(bar0)` at `igpu.rs:602` runs **before** `map_mmio_window` (`:603`) and before the `translate().is_none()` early return (`:604-607`) | With an unmapped BAR0 the guard at `:1011` (`bar0 == 0`) passes, the mux write at `:1033` lands, and the first `mmio_read` at `:1038` **page-faults with the panel dark and no revert**. The ladder issues hundreds of such reads. **Move the store below the `translate` check.** |
| N2 | The `PROTOCOL PROVEN` gate does not exist — `pci.rs:641-642` is gated on cfg only; the arm block at `igpu.rs:677-683` is an empty comment | Source, RUNBOOK and PROPOSAL all claim an unproven gmux gets no write. Set a flag inside the PROTOCOL PROVEN arm and gate the `pci.rs` call on it, **or** delete all three claims. Do not ship the pairing. |
| N1 | `deadline_ms` is only ever `0` (`:1029`), so `gmux_dwell()` (`:504`) exits on iteration 0; `GMUX_DWELL_MS` is compiler-confirmed dead | Every "~10 s" in the RUNBOOK is false and `dwell ended by=itercap` is an instrument that cannot fire. The ladder's dwell is a real observation window; it must actually dwell. |
| N4.2 | On the success path the revert sits behind a heap alloc, a page-table walk, a lock and a dozen UART lines (`:1076-1090`) | Ordering is backwards for a hardware-programming flight. **Unwind first, census after** — or better, put the census inside the unwind-protected region (rung discipline, §3). |
| N6 | Six ungated constants at `igpu.rs:243, 246-250` (`GMUX_READ_DDC`, `GMUX_READ_DISPLAY`, `GMUX_READ_EXTERNAL`, `GMUX_DDC_DIS`, `GMUX_DISPLAY_DIS`, `GMUX_EXTERNAL_DIS`) carry no `#[cfg]` | Dead-code warnings on aarch64 and on `intel-ivb`-without-`gmux_igd`. Cheap; fix it while adding the new register block, or the ladder's own new warnings will be invisible in the noise. |
| cond.7 | `arroyo`'s `KERNEL_CFG_MATRIX` x86 leg is all-on (`…,intel-ivb,unaos_ivb,gmux_igd,…`) | **The mixed combination — `intel-ivb` ON, `gmux_igd` OFF — is the one that burned a build and it is still not in the gate.** See §7. |

---

## 3. The spine: the unwind stack, and the law it enforces

**Law: the gmux revert must be reachable from every instruction of every rung.**

`gmux_revert_now()` (`igpu.rs:479`) is already the right executor — public, idempotent,
compare-exchange-claimed, decides from read-back. It is pure port I/O on `0x7C2/0x7D0/0x7D4` and
touches **no** iGPU state, so it survives any amount of display-engine wreckage. The only things
that can make it unreachable are (a) a fault, (b) an unbounded wait, (c) an unmapped MMIO read.

Three mechanisms, all of which must exist before rung 0 writes anything:

**(1) `DisplayUnwind` — a pre-image stack.**

```
struct UnwindEntry { off: u32, pre: u32 }   // MMIO offset from BAR0, value read before first write
```
A fixed-size array (32 entries is ample) plus a length. **Every display-register write in this
ladder goes through one helper** that pushes `(off, mmio_read(off))` the first time that offset is
touched, then writes. Unwind replays the stack in **reverse push order**. This is the same
discipline `RevertState` (`igpu.rs:356-390`) already applies to the three gmux bytes, generalised —
and it is the reason the ladder can be aborted at any rung without a power cycle.

Unwind is **not** a plain replay for two registers and the code must say so:
- `PCH_PP_CONTROL` — never restore by raw store. Sequence the panel **off** through the PPS
  (assert power-down, wait `PP_STATUS` clear against the T-delays), *then* write back the
  pre-image. A raw restore mid-sequence is the panel-damage path.
- `DSPACNTR` / `PIPEACONF` — disable plane before pipe, and **wait for `PIPECONF` bit 30 (pipe
  state) to clear** before proceeding. Yanking a pipe while a plane is fetching is an underrun.

**(2) Every wait is TSC-bounded, never `arch::ms()`-bounded.**

`crate::arch::ms()` is `apic::ticks()` (`arch/x86_64/mod.rs:100-102`) and **only advances while the
BSP timer ISR runs**. `crate::arch::now_cycles()` is `rdtsc` and "advances regardless of EFLAGS.IF
or whether the APIC-timer ISR runs" (`mod.rs:116-126`), with `crate::arch::hw_wait_budget()`
(`mod.rs:142-149`) giving an honest wall-clock budget in those units. **The ladder's T1..T12 panel
delays and every status-bit poll use `now_cycles()`.** This is not a style preference: a PPS delay
measured on a stopped clock is either instantaneous (panel damage) or infinite (dark hang).

**(3) A total ladder budget.**

The whole ladder runs synchronously inside `pci::init`, before xHCI. One outer `now_cycles()`
deadline (suggest 8 s) bounds the entire climb. On expiry: unwind, revert, print, continue boot.
`arch::ms()` may still be *printed* for human readability — it must never be *compared against*.

**Fault handling.** A fault inside a rung strands the mux. Mitigations, in order of value:
(i) fix N4.1 so BAR0 is provably mapped before `IGPU_BAR0` is published; (ii) validate every
computed GGTT offset against the aperture size **before** the store; (iii) accept that a triple
fault means power cycle, and say so in the RUNBOOK instead of promising otherwise. Note for the
RUNBOOK: with the mux on IGD, **panic output is invisible** — the panic path paints a panel the
mux is not pointing at. Serial is the only channel. That is a fact to state, not a bug to fix here.

**Witness idiom.** Existing tags are `:: igpu: … ::` and `:: igpu-blt: … ::`. Add one more so the
whole ladder is a single `awk`:

```
:: igpu-dpy: rung=NN name=<short> ok=<0|1> <k=v …> ::            # per rung
:: igpu-dpy: rung=NN name=<short> UNWIND pre=0x… post=0x… ok=… ::  # per unwind entry
:: igpu-dpy: LADDER highest=NN/10 name=<short> ok=<0|1> pending=<n> gmux=<MATCH|FAILED> why=<token> elapsed_ms=<n> ::
```

The `LADDER` line is the single awk target and **must be emitted on every exit path** — including
the abort paths. (Contrast `igpu.rs:1064`'s SUMMARY, which the review found on 1 of 3 exits.)
`awk '/igpu-dpy: LADDER/'` answering nothing is itself a finding: the ladder faulted.

---

## 4. The rungs

Ordering rationale: PPS VDD must precede AUX (AUX needs the panel's DP sink powered); AUX must
precede the PLL (the link rate that sets the PLL frequency comes from DPCD); link training must
precede pipe enable (on ILK/IVB the pipe is required to be **off** during training —
**NEEDS-VERIFICATION**); plane follows pipe. **The whole order is NEEDS-VERIFICATION against PRM
Vol 3 Part 4 and against i915's `g4x_dp.c` / `intel_pps.c` enable sequence.**

---

### Rung 0 — preflight census, unwind arm, and mux switch

**Writes:** gmux ports only (`0x7C2` value, `0x7D0` read-index, `0x7D4` write-index) —
`SWITCH_DDC 0x28 = 0x01`, `SWITCH_DISPLAY 0x10 = 0x02`, `SWITCH_EXTERNAL 0x40 = 0x02`.
**No display-engine write.**

**Reads:** the full census already in `igpu.rs:688-750`, **plus** four new ones:
- MCH `0:0:0` config `0x50` (GGC) and `0xB0` (BDSM) — stolen-memory base/size. Offsets **TBV**;
  `crate::arch::pci::read_config_32` (`pci.rs:7`) takes a `u8` offset so both are reachable.
- `regs::GTT_BASE` (`0x200000`, `igpu.rs:77`) entries 0, 1, 2, and `N-1` — is the GGTT populated by
  firmware, or all zero?
- `PIPE_FRMCOUNT_A` — currently the bare literal `0x70040` at `igpu.rs:1038/1041`. **Promote it to
  `regs::PIPE_FRMCOUNT_A` and mark the offset TBV**; an uncited literal in the liveness criterion is
  the one number nobody may guess.
- `DPA_AUX_CH_CTL` and `DPA_AUX_CH_DATA1..5` — expected adjacent to `DP_A` (`0x64000`), i.e. the
  `0x64010..0x64024` window. **Offsets TBV.** Read-only here; just prove the window is not `0xFFFFFFFF`.

**Read-back predicate:** `SWITCH_DDC/DISPLAY/EXTERNAL` read back (indices `0x29`/`0x11`/`0x41`) as
`0x01/0x02/0x02` — the existing MATCH (`igpu.rs:432-461`). Non-zero-on-success
additions: `BDSM != 0 && BDSM != 0xFFFFFFFF`, and the AUX window not reading all-ones.
**Note:** the DDC read index `0x29` (`igpu.rs:243`) is still uncited — the branch's own citation
block (`igpu.rs:226-231`) lists only `0x28`. Cite it or drop the read-back to `0x28`.

**Witness:**
```
:: igpu-dpy: rung=00 name=census ok=1 bdsm=0x… gsm_mb=… ggtt0=0x… ggtt1=0x… aux_ctl=0x… frmcnt=0x… ::
:: igpu-dpy: rung=00 name=mux ok=1 ddc=0x01 disp=0x02 ext=0x02 verdict=MATCH ::
```

**Black-panel failure mode:** expected and unavoidable — the mux now points at an unconfigured
display engine. Discrimination: `rung=00 name=mux ok=0` with the per-register `MISMATCH` lines
already emitted names *which* of the three did not land.

**Unwind:** nothing pushed yet; `gmux_revert_now()` alone.

---

### Rung 1 — a framebuffer the iGPU can actually see (GGTT scanout surface)

The brief's ladder assumed a framebuffer exists. It does not. `DSPASURF` takes a **GGTT offset**;
the console's pixels are at physical `0x90020000` in **Kepler VRAM**, which the iGPU's GGTT cannot
address. `bring_up_blt_ring`'s own comment says so (`igpu.rs:918-922`).

**Heap is not an option at full panel size.** `HEAP_SIZE = 48 MiB` (`allocator.rs:30`) and the video
back buffer is already ~29.5 MiB of it (`allocator.rs:24-29`). A second 2880x1800x4 surface
(19.8 MiB unpadded / 29.5 MiB at stride 4096) will not fit. **Use stolen memory (DSM)**, which the
firmware already reserved outside the usable map: read BDSM/GGC at rung 0, then program GGTT PTEs
to cover the scanout extent.

**Writes:** GGTT PTEs at `regs::GTT_BASE + page*4`, one dword per 4 KiB page, `(phys & ~0xFFF) | 1`
(valid bit) — the exact pattern already proven-in-shape at `igpu.rs:966-984`, including its
**neighbour-PTE smear check** (`:971-984`) and its **>4 GiB refusal** (`:941-944`, extended address
bits 39:32 live in PTE bits 7:4 and are not programmed). Reuse both guards verbatim.

**Read-back predicate:** every PTE written reads back equal to what was written **and** the two
neighbours bracketing the range are unchanged; then a CPU write of a known pattern to the surface's
first and last dword reads back through the same mapping. Non-zero on success:
`ggtt_ptes_ok=<count>` with `count == pages` and `pages == ceil(h*stride_bytes/4096)`.

**Witness:**
```
:: igpu-dpy: rung=01 name=surface ok=1 src=<dsm|heap> gtt_off=0x… pages=… phys=0x… stride_b=… bytes=… pat_ok=1 ::
```

**Black-panel failure mode:** a PTE range that overlaps something live. This is the rung that can
corrupt memory rather than merely fail. Discrimination: `pat_ok=0` (mapping wrong), or
`why=neighbour-pte-changed` (the existing message at `igpu.rs:979` — smeared store), or
`why=phys-above-4g`.

**Unwind:** zero every PTE written, in reverse. Push `(GTT_BASE + page*4, pre)` for each — this is
where a 32-entry stack is not enough, so **record the PTE range as one span entry** (`first_page`,
`count`, and the assertion that every pre-image was `0`; refuse the rung if any pre-image was
non-zero, because then something else owns that GGTT range).

---

### Rung 2 — panel power sequencing (PCH PPS)

**The riskiest rung, and the one metal has already told us most about.**

**Registers written:** `PCH_PP_ON_DELAYS` (`0xC7208`), `PCH_PP_OFF_DELAYS` (`0xC720C`),
`PCH_PP_DIVISOR` (`0xC7210`, only if its pre-image is implausible — metal says it is already
`0x00186904`, so **leave it alone**), `PCH_PP_CONTROL` (`0xC7204`).
**Registers read:** `PCH_PP_STATUS` (`0xC7200`), and `PCH_PP_CONTROL` read-back.

Field layouts of `PP_ON_DELAYS` / `PP_OFF_DELAYS` / `PP_DIVISOR` (T1+T2, T3, T9, T10, T11+T12, and
the unit — likely 100 µs ticks derived from the reference divider) are **all TBV, PRM Vol 3 Part 4
"Panel Power Sequencing"**. So is the `PP_CONTROL` bit map (`31:16` unlock key `0xABCD` —
*strongly* corroborated by the metal read `0xABCD0008`; bit 0 power-on, bit 1 power-reset, bit 2
backlight enable, bit 3 force-VDD, per i915 `intel_pps.c` naming). **Every write to `PP_CONTROL`
must carry the `0xABCD` key in the upper half or it is silently dropped** — TBV, but the metal
read-back is the evidence that the key field is real on this part.

**Do not invent the T-values.** Two honest sources, in order: (i) the EDID/panel DTD if rung 3
lands first — but rung 3 needs VDD, so this is circular unless firmware's forced VDD holds;
(ii) conservative maxima. A panel powered with delays that are **too long** works; too short
does not. Bias long, print the values, and let one metal boot tighten them.

**Read-back predicate:** after asserting power-on, `PCH_PP_STATUS` bit 31 (panel-on, **TBV**) reads
**1** within the programmed T-budget, polled on `now_cycles()`. Non-zero on success:
`pp_status=0x8……` and `t_on_us=<measured>`. The *measured* on-time is the number worth having —
it is what makes the guessed delays falsifiable on the next boot.

**Witness:**
```
:: igpu-dpy: rung=02 name=pps ok=1 pp_ctl_pre=0xABCD0008 on_delays=0x… off_delays=0x… div=0x00186904 pp_status=0x… t_on_us=… ::
:: igpu-dpy: rung=02 name=pps ok=0 why=<status-timeout|readback-mismatch|locked> pp_status=0x… waited_us=… ::
```

**Black-panel failure mode, and how serial names it:**
- `why=locked` — the write did not stick because the `0xABCD` key was omitted: `PP_CONTROL`
  read-back differs from intent. **Distinguishable, and the most likely first-boot error.**
- `why=status-timeout` — the write stuck but `PP_STATUS` never asserted: the PPS is sequencing on
  delays the panel cannot meet, or VDD is not actually present.
- **The dangerous one:** the panel powers but the *Kepler* console does not come back after unwind,
  because the panel power rails are shared board-level between the two GPUs on this machine. See
  risk #1 in §9. If serial shows `rung=02 ok=1` and `LADDER … gmux=MATCH` and the operator still
  reports a dark panel after the revert, **that is this failure**, and it is the one outcome the
  ladder can produce that a power cycle is required to clear.

**Unwind:** sequence off through the PPS (clear power-on, poll `PP_STATUS` clear within T10+T11+T12),
then restore `PP_CONTROL`, `PP_OFF_DELAYS`, `PP_ON_DELAYS` pre-images in reverse. **Never** restore
`PP_CONTROL` by raw store mid-sequence. Honour T11+T12 (power-cycle delay) before any re-assert.

---

### Rung 3 — DPA AUX channel, DPCD, and EDID

**Registers:** `DPA_AUX_CH_CTL` and `DPA_AUX_CH_DATA1..5` — **offsets TBV**, expected at
`0x64010`/`0x64014..0x64024` (adjacent to the in-tree `DP_A = 0x64000`). Field layout (send-busy
bit, done bit, timeout/receive-error bits, message-size field, precharge/clock-divider field) is
**entirely TBV, PRM Vol 3 Part 4 "AUX Channel"**.

Two transactions:
1. **Native AUX read of DPCD `0x00000..0x0000F`** → `DPCD_REV`, `MAX_LINK_RATE` (`0x0A`=1.62,
   `0x14`=2.7 Gbps), `MAX_LANE_COUNT`, `MAX_DOWNSPREAD`, `eDP_CONFIGURATION_CAP`.
2. **I2C-over-AUX read of the EDID at slave `0x50`**, 128 bytes.

Both are reads; the only writes are the AUX transaction registers themselves, which carry no
persistent display state (they still go on the unwind stack for completeness).

**Read-back predicate:** the AUX transaction's done-bit sets with no timeout/receive-error, and the
returned bytes are structurally valid: `DPCD_REV` in `{0x10, 0x11, 0x12}` and `MAX_LANE_COUNT & 0x1F`
in `{1,2,4}`; the EDID block starts `00 FF FF FF FF FF FF 00` and its 128-byte sum mod 256 is 0.
Non-zero on success: `dpcd_rev`, `max_rate`, `lanes`, `edid_sum_ok=1`, and the first DTD's decoded
active area.

**Witness:**
```
:: igpu-dpy: rung=03 name=aux ok=1 dpcd_rev=0x11 max_rate=0x0A lanes=4 edid=OK hdr=OK csum=OK dtd=2880x1800 pclk_khz=… ::
:: igpu-dpy: rung=03 name=aux ok=0 why=<aux-timeout|aux-rxerr|edid-header-corrupt|edid-checksum-bad|dpcd-implausible> ctl=0x… ::
```

**Black-panel failure mode:** none directly — this rung writes no display state. Its failure mode is
**silent wrongness downstream**: a bad link rate or lane count makes rung 5 fail, and a hardcoded
mode makes rung 6 fail. `why=aux-timeout` most likely means VDD is not actually on, which **retro-
actively falsifies the `0xABCD0008` = forced-VDD reading** — a genuinely valuable negative result.

**Result:** Flight 1b (Boot AK) returned `why=aux-timeout-error`. Hypothesis 3 dictates the Retina gmux cannot switch AUX separately; it requires switching `GMUX_SWITCH_DISPLAY` to reach the panel.

**Round 13 — the flight that can answer it.** Moving `GMUX_SWITCH_DISPLAY` (and `EXTERNAL`) to IGD
blanks the panel for the probe window, so the round-13 flight is built around three things:

1. **A pre-switch positive control.** A one-byte DPCD native read at address 0 runs BEFORE the first
   mux-moving write — same `dp_aux_transfer`, same inherited clock divider, same 1-byte buffer, its
   own budget consumed before the dark window's deadline exists. Without it, a post-switch success
   proves nothing (AUX might already have reached the panel) and a post-switch failure proves nothing
   either (the AUX block might be broken independent of routing). This control is what makes the
   flight able to answer the question at all.
2. **Everything buffered.** Not one serial print happens between the first gmux write and
   `unwind.execute()`. A failure therefore leaves a fully readable capture instead of a hang behind a
   black screen.
3. **A parachute of validated constants.** The pre-switch gate checks every mux read against a set
   of named constants, and the LIFO unwind writes back **the member it validated** — never a live
   re-read, because a timed-out gmux read returns `0xFFFFFFFF`, which truncates to `0xFF` and would
   leave the panel dark.

The accepted set, and why it is a set. `DDC` and `DISPLAY` must be DIS — every capture the tree
records reads `DDC=0x02 DISP=0x03`, so neither has a second legitimate value. `EXTERNAL` may be DIS
**or** `GMUX_EXTERNAL_KEPLER_OWNED` (`0x21`), the Boot AK metal norm: `0x21` is what this machine
actually reads when the firmware leaves the port Kepler-owned, which is why round 11 admitted it and
why round 13 continues to. Demanding DIS there would make the flight refuse on the only machine it
was written for. Both EXTERNAL registers are relaxed — the recorded `0x21` was seen on
`READ_EXTERNAL`, and `SWITCH_EXTERNAL` has never been captured on a Kepler-owned boot.

The restore follows the validation: EXTERNAL goes back to the value that was found, not to a blanket
DIS, because forcing a Kepler-owned port to DIS is not a restore but a silent state change. The
MATCH verdict likewise compares EXTERNAL against the validated pre-image, so a correct restore on a
Kepler-owned machine reports MATCH rather than FAILED. The `0xFFFFFFFF` sentinel is neither
constant, so it can never pass the gate; anything outside the set REFUSES with
`pre-switch-not-accepted` before a mux is touched.

`why=aux-short-read` is KNOWN AND ACCEPTED, not a defect: it is a legal partial I2C reply where
upstream i915 clamps instead of erroring, and seeing it is itself proof that AUX answered.

**Unwind:** nothing persistent. But **`SWITCH_DDC` must be on IGD** (rung 0) or AUX talks to nothing;
if rung 0's DDC leg mismatched, refuse this rung rather than time out.

---

### Rung 4 — eDP port PLL

**Registers:** `DP_A` (`0x64000`, `igpu.rs:38`) — PLL-enable and PLL-frequency-select bits.
**Bit positions TBV**, and see §1: i915's headers appear to overlap `DP_PORT_EN` (31),
`DP_PLL_ENABLE` (30) and `DP_PIPE_SEL_MASK_IVB` (30:29). **Resolve that in the PRM before writing.**
Also read `DPLL_A_CTRL`/`FPA0`/`FPA1` as a cross-check that they stay zero (proving the eDP path
does not use them, which the census already suggests).

Frequency select comes from rung 3's `MAX_LINK_RATE`: 162 MHz for 1.62 Gbps, 270 MHz for 2.7 Gbps.

**Read-back predicate:** `DP_A` read-back has the PLL-enable bit set, and a **lock/ready indication**
(register and bit **TBV** — it may be a `DP_A` status bit or a separate one) asserts within the
PRM's stated PLL warm-up (~20 µs, TBV), timed on `now_cycles()`. Non-zero on success:
`dp_a=0x4……`, `lock_us=<measured>`.

**Witness:**
```
:: igpu-dpy: rung=04 name=edp-pll ok=1 dp_a_pre=0x0000001C dp_a=0x… freq=270 lock_us=… dplla=0x00000000 ::
:: igpu-dpy: rung=04 name=edp-pll ok=0 why=<no-lock|readback-mismatch> dp_a=0x… waited_us=… ::
```

**Black-panel failure mode:** a PLL that never locks makes rung 5's link training fail with zero
clock recovery — the panel stays black through both rungs. Serial separates them cleanly:
`rung=04 ok=0 why=no-lock` (PLL) vs `rung=05 ok=0 why=cr-fail` (training). **Without this rung's
own predicate, a training failure is indistinguishable from a clock failure** — that is the whole
argument for keeping the PLL a rung rather than folding it into training.

**Unwind:** clear the PLL-enable bit, restore the `DP_A` pre-image `0x0000001C`.

---

### Rung 5 — eDP link training (clock recovery + channel equalisation)

**Registers written:** `DP_A` (`0x64000`) — port enable, link-train pattern select, port width
(lane count), pipe select; **all field positions TBV**. Plus DPCD writes over rung 3's AUX:
`LINK_BW_SET` (`0x100`), `LANE_COUNT_SET` (`0x101`), `TRAINING_PATTERN_SET` (`0x102`),
`TRAINING_LANE0..3_SET` (`0x103..0x106`).
**Registers read:** DPCD `LANE0_1_STATUS` (`0x202`), `LANE2_3_STATUS` (`0x203`),
`LANE_ALIGN_STATUS_UPDATED` (`0x204`), `ADJUST_REQUEST_LANE0_1` (`0x206`),
`ADJUST_REQUEST_LANE2_3` (`0x207`). DPCD addresses and bit meanings are DP-spec, not IVB-specific,
but still **NEEDS-VERIFICATION** against the DP 1.1/1.2 spec.

Standard two-phase loop: TPS1 until `CR_DONE` on all lanes (max 5 attempts per voltage swing level,
max 4 swing levels), then TPS2 until `CHANNEL_EQ_DONE` + `SYMBOL_LOCKED` + `INTERLANE_ALIGN_DONE`
(max 5 attempts), then pattern-off. **Vswing/pre-emphasis levels are written to both the DPCD
`TRAINING_LANE*_SET` registers and the corresponding `DP_A` field — TBV whether IVB port A carries
the swing/emphasis in `DP_A` or in a separate register.**

**Read-back predicate:** `CR_DONE` set on every active lane after phase 1, then `CHANNEL_EQ_DONE &&
SYMBOL_LOCKED` on every active lane **and** `INTERLANE_ALIGN_DONE` after phase 2. Non-zero on
success: `cr=0x…` / `eq=0x…` / `align=1`, plus `cr_tries` / `eq_tries` / final `vswing` / `preemph`.

**Witness:**
```
:: igpu-dpy: rung=05 name=link-train ok=1 rate=0x0A lanes=4 cr_tries=1 eq_tries=1 vswing=0 preemph=0 lane01=0x77 lane23=0x77 align=1 ::
:: igpu-dpy: rung=05 name=link-train ok=0 why=<cr-fail|eq-fail|aux-fail|max-swing> lane01=0x… lane23=0x… tries=… vswing=… ::
```
Emitting the per-attempt lane status (`lane01`/`lane23` each round, at least on failure) is what
turns a black panel into a diagnosis rather than a mystery.

**Black-panel failure mode:** an untrained link means no pixels, ever — the pipe will still count
frames in rung 9. **This is the most important discrimination in the whole ladder:**
`rung=09 ok=1` (frames advancing) with `rung=05 ok=0` means *the display engine is fine and the
wire is dead*. Without rung 5's own predicate, that state reads as total success on the liveness
criterion and total failure on the panel.

**Unwind:** write DPCD `TRAINING_PATTERN_SET = 0` (disable), clear the port-enable bit in `DP_A`,
restore the `DP_A` pre-image. Order matters — **TBV** whether IVB requires the pattern-off DPCD
write before the port disable, but do it in that order regardless; it is harmless if unnecessary.

---

### Rung 6 — transcoder/pipe timings from the mode

**Registers written:** the CPU-side per-pipe timing block for pipe A —
`HTOTAL_A`, `HBLANK_A`, `HSYNC_A`, `VTOTAL_A`, `VBLANK_A`, `VSYNC_A` (**offsets TBV**, expected
`0x60000`, `0x60004`, `0x60008`, `0x6000C`, `0x60010`, `0x60014`) and `PIPEASRC` (`0x6001C`,
**in-tree**, `igpu.rs:10`). Plus the DP M/N ratio registers `PIPEA_DATA_M1` / `DATA_N1` /
`LINK_M1` / `LINK_N1` (**offsets TBV**).

*Corroboration, not proof:* the in-tree `PIPEASRC = 0x6001C` sits at `0x60000 + 0x1C` — the last
slot of a 32-byte block — and the in-tree `FDI_TXA_CTL = 0x60100` is the next block up. That is
consistent with a `0x60000`-based per-pipe timing block. **Still TBV.**

Each timing register packs `(end-1) << 16 | (start-1)` in the usual Intel convention — **TBV**.
`PIPEASRC` packs `(width-1) << 16 | (height-1)` — **TBV**, though `dump_pipe` (`igpu.rs:756-762`)
already prints it, so one boot with a known-good value settles the encoding.

**Read-back predicate:** every timing register reads back exactly as written (these are plain
read/write registers with no side effects while the pipe is off), and `PIPEASRC` decodes to
`2880x1800`. Non-zero on success: `htotal=0x…` etc. plus a decoded
`mode=2880x1800@<refresh> pclk=<khz>` line.

**Witness:**
```
:: igpu-dpy: rung=06 name=timings ok=1 mode_src=<aux-edid|bootinfo-edid|hardcoded> mode=2880x1800 pclk_khz=… htot=0x… hbl=0x… hsy=0x… vtot=0x… vbl=0x… vsy=0x… src=0x… m1=0x… n1=0x… ::
```
**`mode_src=` is mandatory on this line.** A ladder that silently fell back to hardcoded timings and
lit nothing must be distinguishable from one that used the panel's own DTD.

**Black-panel failure mode:** wrong timings + trained link = the panel receives a signal it cannot
sync to; most eDP panels show black, some show a scrambled image. Frames still count in rung 9.
Discrimination: this rung's read-back **always** passes (the registers are dumb), so its failure is
only visible as `mode_src=hardcoded` plus a dark panel with every other rung green. That is why
rung 3's EDID matters and why `mode_src` is on the wire.

**Unwind:** restore all seven pre-images (all `0x00000000` per the census). Safe as raw stores
**only while the pipe is off** — so unwind order (rung 7 before rung 6) is load-bearing.

---

### Rung 7 — pipe enable

**Registers:** `PIPEACONF` (`0x70008`, **in-tree** `igpu.rs:5`). Write bit 31 (enable) plus the bpc
field and dither control (**field positions TBV**; 8 bpc for this panel). Read `PIPEACONF` bit 30
(pipe state) — the in-tree `dump_pipe` already treats bit 31 as enable (`igpu.rs:759`), so the
bit-31 semantics are corroborated in-tree; bit 30 as *state* is **TBV**.

**Read-back predicate:** `PIPEACONF` bit 31 reads 1 **and** bit 30 (state) reads 1 within one frame
time (~17 ms at 60 Hz — poll on `now_cycles()`, not `arch::ms()`). Non-zero on success:
`pipeconf=0xC……`. **Bit 31 alone is not the predicate** — it is what we wrote; bit 30 is what the
hardware says.

**Witness:**
```
:: igpu-dpy: rung=07 name=pipe ok=1 pipeconf=0x… state=1 t_us=… ::
:: igpu-dpy: rung=07 name=pipe ok=0 why=<state-never-set> pipeconf=0x… waited_us=… ::
```

**Black-panel failure mode:** `why=state-never-set` means the pipe has no clock — which points back
at rung 4 (PLL) or at the pipe→port routing bits in `DP_A` (rung 5). Clean discrimination: a pipe
that will not enter the enabled state is *upstream*; a pipe that enables but shows nothing is
*downstream* (rungs 8/8b) or *off-wire* (rung 5/6).

**Unwind:** clear bit 31, **poll bit 30 clear** before touching rung 6's registers, then restore the
`0x00000000` pre-image.

---

### Rung 8 — primary plane configuration

**Registers written, in this order** (**TBV**, but surface-last is the standard Intel convention
because the `DSPASURF` write is the arming/flip trigger):
`DSPASTRIDE` (`0x70188`, `igpu.rs:25`), `DSPALINOFF` (`0x70184`, `:30`), `DSPATILEOFF` (`0x701A4`,
`:31`), `DSPACNTR` (`0x70180`, `:15`), then `DSPASURF` (`0x7019C`, `:20`).

`DSPACNTR` fields are **partly in-tree already**: bit 31 enable (`igpu.rs:766`), format field bits
29:26 (`:767` reads `>>26 & 0xF`), tiled bit 10 (`:768`). The **format code for BGRX8888 is TBV**,
as is whether stride is in bytes or 64-byte units. Set **linear** (tiled bit clear) — the GGTT
surface from rung 1 is linear, and a tiled/linear mismatch is a scrambled panel that passes every
predicate below.

`DSPASURF` takes the **GGTT offset from rung 1**, not a physical address. `DSPALINOFF` and
`DSPATILEOFF` = 0 (no panning).

**Read-back predicate:** `DSPACNTR` bit 31 reads 1, `DSPASURF` reads back the programmed GGTT
offset, `DSPASTRIDE` reads back the programmed stride. Non-zero on success: `dspcntr=0x8……`,
`dspsurf=0x…`. This is exactly the criterion the branch already waits on at `igpu.rs:1044` — for
the first time it can pass.

**Witness:**
```
:: igpu-dpy: rung=08 name=plane ok=1 dspcntr=0x… fmt=0x… tiled=0 stride=0x… surf=0x… linoff=0 tileoff=0 ::
```

**Black-panel failure mode:** a plane pointed at an unmapped or wrong GGTT offset scans garbage or
nothing while every register reads back correct. Discrimination: fill the rung-1 surface with a
**known non-black test pattern before enabling the plane** (a solid colour plus a distinguishable
corner marker). Then the operator's report "uniform colour X" vs "black" vs "noise" separates
*plane-not-fetching* from *plane-fetching-the-wrong-address* — from serial alone we cannot, but the
operator's one-word observation plus the register read-backs can.

**Unwind:** clear `DSPACNTR` bit 31, **wait one frame**, then restore all five pre-images
(`0x00000000` each per the census).

---

### Rung 8b — display watermarks (not in the brief's list; a black panel lives here)

**Registers:** `WM0_PIPEA_ILK` and the LP watermark set (`WM1_LP_ILK`, `WM2_LP_ILK`, `WM3_LP_ILK`),
plus `WM_LINETIME` if IVB carries one. **All offsets TBV** — expected in the `0x45100` block.

**Read-back predicate:** registers read back as written **and** no underrun is reported (the
underrun/FIFO status bit — register and bit **TBV**) across a 500 ms observation window. Non-zero on
success: `wm0=0x…` plus `underruns=0` over `frames=<n>`.

**Witness:**
```
:: igpu-dpy: rung=08b name=wm ok=1 wm0=0x… lp=0x…,0x…,0x… underruns=0 frames=… ::
```

**Black-panel failure mode:** underrun. The pipe counts frames (rung 9 goes green), the plane says
enabled, the link is trained — and the panel is black or tearing. **This is the failure that looks
like success everywhere else**, which is the entire justification for making it a rung with its own
counter. If the operator reports a dark panel with `rung=09 ok=1`, read `underruns=` first.

**Unwind:** restore pre-images. Safe as raw stores at any time.

---

### Rung 9 — liveness (the existing criterion, finally able to read non-zero)

**Registers read:** `PIPE_FRMCOUNT_A` (currently the literal `0x70040`, `igpu.rs:1038/1041` —
promote to `regs::` and mark **TBV**) and `DSPACNTR` bit 31 (`0x70180`, `igpu.rs:15`).

**Read-back predicate — this is the arc's headline claim:**
> Sampled twice at least 100 ms apart on a `now_cycles()` budget, `PIPE_FRMCOUNT_A` **increases**,
> and `DSPACNTR` bit 31 is **set** at both samples.

Report the *delta and the elapsed time*, not a boolean: `frames=<Δ>` over `elapsed_ms=<n>` gives a
computed refresh rate that can be checked against rung 6's mode. A Δ of 6 over 100 ms is 60 Hz; a Δ
of 6 over 100 ms when the mode says 48 Hz is a finding. **A boolean "it advanced" is exactly the
kind of instrument this round exists to stop shipping.**

**Witness:**
```
:: igpu-dpy: rung=09 name=liveness ok=1 frm0=0x… frm1=0x… frames=6 elapsed_ms=100 hz_est=60 dspcntr=0x8… ::
:: igpu-dpy: rung=09 name=liveness ok=0 why=<no-advance|plane-off> frm0=0x… frm1=0x… elapsed_ms=… ::
```

**Black-panel failure mode:** `why=no-advance` with rung 7 green means the pipe enabled but has no
pixel clock — back to rung 4. `ok=1` with a dark panel is the interesting case and the ladder now
localises it: check `rung=05` (link), then `rung=08b` (underrun), then `rung=06 mode_src`, then
rung 10 (backlight). **That decision tree is the deliverable of the whole flight**, more than the
lit panel itself.

**Unwind:** nothing written.

---

### Rung 10 — backlight (explicitly NOT a liveness gate)

Metal reads gmux `MAX_BRIGHTNESS = 0x000003FF` (`igpu.rs:197`, gmux index `0x70`), so on this
machine the **gmux owns the backlight**, not the iGPU's `BLC_PWM_*`. A perfectly trained link with
the backlight at zero is visually identical to total failure.

**Registers:** gmux index `0x74` (BRIGHTNESS) — **index TBV**, cite `apple-gmux.c` at the call site
the way `igpu.rs:226-231` already does for the switch ports. Optionally `PCH_PP_CONTROL` bit 2
(`EDP_BLC_ENABLE`, **TBV**) as a cross-check.

**Read-back predicate:** brightness reads back within tolerance of what was written.
Non-zero on success: `brt=0x…`.

**Witness:** `:: igpu-dpy: rung=10 name=backlight ok=1 brt=0x… max=0x3FF ::`

**Black-panel failure mode:** this rung *is* the black-panel-with-everything-green explanation. Keep
it last and keep it out of the liveness predicate: **rung 9 must be able to pass with the backlight
off**, or the arc's success criterion becomes hostage to a register nobody has read yet.

**Unwind:** restore the pre-switch brightness (read it at rung 0, alongside the DDC/DISPLAY/EXTERNAL
triple, and carry it the same way `RevertState` carries those).

---

## 5. Where the mode comes from

Ranked. The lane should implement **tier C now with the refusal guard**, and land **tier B as its
own flight** (1b) because it is boot-testable with zero panel risk.

**Tier A — raw EDID via BootInfo (best value per line, but OUT OF LANE).**
`crates/bootloader/src/main.rs:62-80` **already reads the full EDID block** through
`EFI_EDID_ACTIVE_PROTOCOL` / `EFI_EDID_DISCOVERED_PROTOCOL` and then throws the bytes away, keeping
only width/height (`parse_edid_native`, `:38`). `BootInfo` carries `edid_native_width`,
`edid_native_height`, `edid_source` (`crates/boot-info/src/lib.rs:63-66`) — **and no timings**.
Adding `pub edid_block: [u8; 128]` + a validity flag is ~10 lines across `boot-info`, `bootloader`
and `main.rs`. **All three are outside the igpu lane** → **STOP and report to the seat.** Note also
that `main.rs:220-226` extracts the existing EDID fields only under `#[cfg(feature = "bootlog")]`,
so even the width/height are unreachable on a normal build without widening that gate.

**Tier B — I2C-over-AUX EDID read (rung 3).** The correct long-term source, and it works on any
panel. Requires `SWITCH_DDC = IGD` (rung 0) and VDD (rung 2, or firmware's forced VDD). Its own
flight; its deliverable is a checksum-valid EDID on the wire and nothing else.

**Tier C — hardcoded 2880x1800 DTD, guarded.** For Flight 1 only. Two obligations:
- **Refuse to proceed** if `boot_info.framebuffer_info.width/height != 2880/1800`. Metal says
  `panel=2880x1800 stride=4096px` today; a different panel (or a `UNAOS_FBW`/`UNAOS_FBH` override)
  must abort the ladder, not drive a wrong mode into an eDP panel. This is the same class of guard
  as `bring_up_blt_ring`'s live-geometry check at `igpu.rs:946-960`, which exists precisely because
  a hardcoded 2880x1800 was the defect a review bounced.
- **Print `mode_src=hardcoded`** on rung 6's line, every time.

The DTD itself (pixel clock and blanking) is **NEEDS-VERIFICATION**: derive it from CVT-RB for
2880x1800 @ 60 Hz and state in the source that it is computed, not measured. **Do not copy a pixel
clock out of memory.**

---

## 6. Reusable code, per rung

| Rung | Reuse from `2be56eb2` | Note |
|---|---|---|
| all | `mmio_read(base, offset)` — `igpu.rs:786-788` | **There is no `mmio_write` in `igpu.rs`.** `kepler.rs:2189` has `pub unsafe fn mmio_write(base, offset, val)` — do **not** reach across into the kepler lane. Add a local one, and make it the unwind-recording helper (§3). |
| all | `regs` module — `igpu.rs:3-84` | Already has 30+ offsets including every PPS, GMBUS, FDI, pipe and plane register the ladder needs. Extend it; do not start a second table. |
| all | The citation-comment style — `igpu.rs:226-231`, `724-728` | Cite the PRM section at the point of use. Two reviews have already re-litigated an uncited gmux wait; the same will happen to every uncited display bit. |
| 0 | gmux write logic — `igpu.rs:432-461`; `gmux_index_read`/`_write` — `:326`, `:341`; `gmux_wait_ready`/`_complete` — `:290`, `:308` | Verdict decided by read-back, iteration-bounded waits, `0xFFFFFFFF` sentinel. Reuse unchanged. |
| 0 | `dump_pipe` / `dump_plane` — `igpu.rs:756-784` | Already print exactly the fields the rungs need. Make them the post-rung census. |
| 0 | `crate::arch::pci::read_config_32` — `pci.rs:7` | `u8` offset reaches GGC (`0x50`) and BDSM (`0xB0`) at BDF `0:0:0`. |
| all | `RevertState` pack/unpack + `gmux_state_update` — `igpu.rs:354-421` | The compare-exchange discipline is the model for `DisplayUnwind`'s claim. One encode point, one decode point. |
| all | `gmux_revert_now` — `igpu.rs:479-493` | Already correct: idempotent, claims atomically, does the writes itself, reports from read-back. **Do not restructure it.** |
| all | `crate::arch::now_cycles` / `hw_wait_budget` — `arch/x86_64/mod.rs:122`, `:142` | The only legal wait basis for this ladder (§3). |
| 1 | GGTT PTE write + neighbour-smear check + >4 GiB refusal — `igpu.rs:941-984` | The single most reusable block in the branch. Its `'ring:` labelled-break refusal pattern (`:915`) is also the right shape for rung refusals: **an accelerator must degrade, never kill the boot.** |
| 1 | `crate::arch::memory::translate` — `memory.rs:242` | Per-page VA→PA for a heap-backed surface, if DSM is unusable. |
| 2 | `crate::video::WRITER.try_lock().map(|w| w.info())` — `igpu.rs:951-960` | The refuse-if-contended pattern for reading live panel geometry. Reuse for the tier-C guard. |
| 9 | The liveness loop — `igpu.rs:1035-1071` | Correct in shape, wrong in three details: the bare `0x70040` literal, a boolean instead of a delta, and `GMUX_WAIT_ITERS` (5000) as the bound instead of a TSC budget. |
| — | `gmux_dwell` — `igpu.rs:504-524` | Fix N1 first (`deadline_ms` is always 0). Then it becomes the rung-9 observation window. |

---

## 7. Feature knobs — and the combination that must not break again

Current wiring, verified in-tree:
- `Cargo.toml:37` — `gmux_igd = []`; `:27` — `intel-ivb = []`; `:28` — `unaos_ivb = ["unaos-boot-info/unaos_ivb"]`.
- `arroyo:249` — `UNAOS_GMUX_IGD=1` → `gmux_igd`; `builder/src/main.rs:173` — same.
- `pci.rs:641` — the switch call is `#[cfg(all(feature = "gmux_igd", feature = "intel-ivb"))]`.
- **`unaos_ivb` is a cross-crate feature**: `BootInfo`'s `igpu_trace_*` / `gmux_trace_0` /
  `kdisp_trace_*` fields are `#[cfg(feature = "unaos_ivb")]` (`boot-info/src/lib.rs:84-97`), so
  bootloader and kernel must agree or the struct layout diverges. **This is the knob that can
  corrupt a boot silently rather than fail to compile.**

Recommendation:

- **Put the whole ladder behind a new `igpu_bringup` feature**, implied-by/independent-of
  `gmux_igd`. Reason: the mux switch and the display bring-up have different risk profiles and
  different revert obligations. A boot that wants "prove the mux moved" (the seat's option B) must
  remain buildable without compiling one line of PPS or link-training code.
- **`igpu_bringup` requires `gmux_igd`** (the panel must be routed to the IGD before any of this is
  meaningful) and `intel-ivb`. Express it as `igpu_bringup = ["gmux_igd"]` in `Cargo.toml` so it
  cannot be selected alone.
- **Four combinations must compile clean (zero new warnings) on both arches:**

  | # | knobs | status today |
  |---|---|---|
  | 1 | none (default) | in the gate |
  | 2 | `intel-ivb` + `unaos_ivb`, **`gmux_igd` OFF** | **NOT in the gate — this is the combination that burned a build** (review condition 7). `KERNEL_CFG_MATRIX`'s single x86 leg is all-on. It is green today by luck. |
  | 3 | `intel-ivb` + `unaos_ivb` + `gmux_igd` | in the gate |
  | 4 | `intel-ivb` + `unaos_ivb` + `gmux_igd` + `igpu_bringup` | new |

  **Add legs 2 and 4 to `arroyo`'s `KERNEL_CFG_MATRIX`.** `arroyo` is the integrator's file — if the
  lane cannot touch it, **stop and report** rather than shipping a knob the gate never builds.
- **Clean the six ungated constants at `igpu.rs:243, 246-250`** (review N6) before adding the new
  register block, or the ladder's own dead-code warnings will be invisible.
- **Every new `regs::` constant is unconditional** (a `pub const` in a `pub mod` used only under a
  cfg is a warning source). Put the constants in `regs` unconditionally and cfg-gate only the code
  that uses them — which is what the existing `regs` block already does.

---

## 8. Flight decomposition

Five flights. **Each is independently boot-testable, each has its own falsifiable witness, and each
ends with the machine back on the Kepler console.** No boot proves nothing.

### Flight 1a — harness (ZERO display writes)
**Rungs:** 0 + the §2 blocker fixes + the `DisplayUnwind` machinery + a **forced-unwind self-test**.
**Deliverable:** the unwind path is proven *before* it is needed — arm the stack, push two synthetic
entries against a harmless scratch register, force an unwind, prove every pre-image restored and the
gmux reverted. Plus the rung-0 census (BDSM/GGC/GGTT/AUX window/FRMCOUNT).
**Witness:** `LADDER highest=00/10 … pending=2 gmux=MATCH`.
**Risk:** same as `2be56eb2` today — mux moves, panel dark for the dwell, comes back.
**Why first:** an unwind stack that has never executed is an instrument that cannot execute in the
state it reports on. Every later flight bets the machine on it.

### Flight 1b — AUX, DPCD, EDID (reads only)
**Rungs:** 3 (and rung 2 **only if** AUX times out — test firmware's forced VDD first).
**Deliverable:** DPCD rev / max link rate / lane count, and a checksum-valid 128-byte EDID with its
first DTD decoded, on the wire.
**Witness:** `rung=03 … edid=OK csum=OK dtd=2880x1800 pclk_khz=…`.
**Risk:** near zero if VDD is already forced — no persistent display state is written.
**Why second:** it supplies rung 4's link rate and rung 6's timings. Doing it before any panel
programming means the risky flights start with real numbers instead of guesses. **And it settles
the `0xABCD0008` = forced-VDD reading either way**, which is a result worth a boot on its own.

### Flight 1c — power + clock + wire
**Rungs:** 2, 4, 5. Pipe and plane stay off.
**Deliverable:** a trained eDP link. `cr=…`, `eq=…`, `align=1`, measured `t_on_us` and `lock_us`.
**Witness:** `LADDER highest=05/10 ok=1 … gmux=MATCH`.
**Risk:** **first real panel risk.** This is where a shared panel power rail could fail to restore
the Kepler console (risk #1). Fly it with the RUNBOOK's power-cycle instruction already written.
**Why here:** it is the natural boundary — everything before it is reversible by register restore;
rung 2 is the first thing that touches a physical rail.

### Flight 1d — pixels
**Rungs:** 1, 6, 7, 8, 8b, 9.
**Deliverable:** the arc's objective — `PIPE_FRMCOUNT_A` advancing with `DSPACNTR` bit 31 set,
reported as a frame delta over a measured interval, plus `underruns=0`.
**Witness:** `LADDER highest=09/10 ok=1 frames=… hz_est=…`.
**Risk:** memory corruption via a bad GGTT range (rung 1) is the new class here — hence rung 1's
neighbour check and all-pre-images-zero refusal.
**Why last-but-one:** it is the only flight that needs *all* the earlier numbers to be right, so it
is the one whose failure is most expensive to diagnose. Everything before it exists to make its
failure legible.

### Flight 1e — the panel is actually visible (optional)
**Rungs:** 10 + hand the console over.
**Deliverable:** an operator-visible image on the iGPU-driven panel; the mux **stays** on IGD.
**Note:** this is the first flight whose success criterion is not on the serial wire. Do not merge
it with 1d — a lit panel and a live pipe are separate claims and must be separately falsifiable.

---

## 9. Top risks

**1. The panel power rail is shared, and rung 2 may not be reversible.**
On a dual-GPU rMBP the internal panel's power/backlight are board-level and the gmux mediates them.
Whether the iGPU's PCH PPS can assert panel power *at all* while the mux has just been pointed at
it — and whether the panel comes back under Kepler after we sequence it off — is **unknown and
unverified**. The census gives one hint (`PCH_PP_CONTROL = 0xABCD0008`, panel power off, VDD
possibly forced) and nothing more. Mitigations: rung 2's unwind sequences off through the PPS and
honours T11+T12 rather than raw-restoring; flight 1c is the first flight to touch it; and the
RUNBOOK must say plainly that a power cycle may be required, because the serial log can show
`LADDER … gmux=MATCH` while the panel stays dark. **This is the risk that can end a bench sitting.**

**2. Register offsets and bit positions are the load-bearing unknown, and one class of them is
actively misleading.** The tree supplies ~30 offsets, all census-verified against plausible metal
values. It supplies **none** of the AUX, timing, M/N, watermark, `DP_A` field, `PIPECONF` field or
`DSPACNTR` format encodings — every one of those is TBV. Worse, the obvious reference (i915) mixes
generations: `DP_TP_CTL`/`DP_TP_STATUS` are Haswell registers that **do not exist** on this part,
and i915's own `DP_A` bit definitions appear to overlap `DP_PORT_EN`/`DP_PLL_ENABLE`/
`DP_PIPE_SEL_MASK_IVB`. **A wrong bit in `DP_A` or `PP_CONTROL` is a dark panel with a green
read-back.** Every offset in this document that is not cited to `igpu.rs` must be verified against
Intel IVB PRM Vol 3 Part 4 before it is written, and cited at the call site.

**3. The instruments can pass while the panel is black — in at least four distinct ways.**
Rung 9 (`FRMCOUNT` advancing + plane enabled) is satisfied by an untrained link (rung 5), by a
FIFO underrun (rung 8b), by wrong timings (rung 6), and by a backlight at zero (rung 10). Three of
those four write registers that read back exactly as written. **The mitigation is not more
predicates on rung 9 — it is that each of those four has its own rung with its own falsifiable,
non-zero witness**, so `LADDER` plus four field values give a decision tree instead of a mystery.
If the flight decomposition is collapsed to save boots, this is what is lost, and the boots will be
spent anyway on bisecting a black panel.

**Standing hazards, already known, restated because they intersect this ladder:** the media is
single-use (`PROBED` is per-boot only — every boot from an armed stick re-runs the whole ladder);
there is **no `gmux-revert` shell verb** and `shell.rs` is outside the igpu lane; the input chain
under a switched mux is still unverified; and with the mux on IGD, **panic output goes to a panel
nobody is looking at** — serial is the only channel, and the rig is kernel-TX-only.

---
**SEAT CORRECTION (GR20, 2026-08-07):** the closing claim that intel-ivb-ON/gmux_igd-OFF
"is still not in arroyo's KERNEL_CFG_MATRIX" is WRONG — it was read from the branch's
pre-rebase arroyo (base 0421aa15). Trunk's arroyo (since 3aa2b7a4) carries the
GATE-CFG-MIX covering array; the gmux_igd x intel-ivb pair is covered several times over
(arroyo:1126-1160) and the rebased branch inherits it. The EDID-in-bootloader finding
stands and is OUT OF THE IGPU LANE (three files); the seat owns that call.
