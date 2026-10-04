# KBLIT — the first GPU-executed operation on this machine, by checksum

**Ledger: rmbp B319. Branch `exec-rmbp-kblit`. Knob `UNAOS_KEPLER_CE=1` / feature
`nvidia-kepler-ce` (the existing CE knob — no new knob, per the brief). Default OFF.**
Register of record: `docs/dev/OS/08_VIDEO/SHUTOUT-REGISTER.md` §2 (the FIFO wall) and §3 (the
CE ladder). Code: `unaos/crates/kernel/src/drivers/gpu/kepler_ce.rs` (extended).

## Finding

The Kepler GK107 owns the rMBP panel through the takeover and has **never executed a command
we gave it** — the FIFO ladder's metal verdict is `NEITHER-ANSWERS / STILL-DARK, writes=0`
(KF27/KF28/KF29) and the CE ladder (`nvidia-kepler-ce`) was built but `never-run`. Every pixel
on the glass is CPU-composited (WCPAR). Peter: "what about the GPUs?" — the goal is KCOMP (the
compositor's blits on the copy engine), and this arc is its first proof: **one copy-engine blit,
verified by checksum.**

Two structural problems were in the way and both are fixed here:

1. **The CE ladder ran at BOOT** (`kepler::init` called `kepler_ce::ladder(...)` under the knob),
   which R80 forbids ("nothing runs at boot but the boot"). Moved: `kepler::init` now only
   **stores the boot context** (`arm_context`) and the ladder runs from `tests ce`.
2. **There was no copy submission at all** — the CE ladder is read-only reconnaissance. M2/M3
   build a copy-engine channel and attempt the blit from `tests kblit`.

## The seam

`CHARTER: Kernel — driver`. The copy engine is a device; its driver is the kernel's. `kepler_ce.rs`
is under `drivers/gpu/`, outside the GATE-CHARTER app-domain scope (video/ fs/ install/ selfhost/
and the named shell files), so charter-check does not require a header on it; the seam is named
here for the record. No handler owns a GPU copy engine.

## Why this path, named (M2)

The FIFO/PGRAPH channel has **never validated** across ten eliminations (§2) — the strip
signature never moved, and the remaining actor is FECS context-switch microcode we do not have and
may not clean-room. So KBLIT does **not** take the graphics channel / GRCOPY-on-PGRAPH path: it
would inherit the FECS wall. It takes the **dedicated copy-engine channel** path (PCOPY on the
GK107), which §3's CE ladder judged the most alive avenue precisely because it is "independent of
`nvidia-kepler-fifo`" and "a CE falcon as a bare DMA microcontroller, no PFIFO, no channel, no FECS
wall" (draft §4.2 path 2, named at `kepler_ce.rs` R2's FALCON-REST verdict). KF24 (BAR1 identity,
`proven` on Boot A) is the fact everything stands on: **a BAR1 offset IS a physical VRAM address**,
so every GPU pointer below is a VRAM offset the CPU can also reach through `bar1_base + off`, and the
blit is **VRAM→VRAM** — the reachable form of draft R7, which the CE ladder deferred.

⚠ **This is the honest reachable target, and M2 may STOP.** M2's milestone is the pushbuffer GET
pointer advancing past PUT (the engine fetched). Ten sittings say it may not. If it does not, the
arc STOPS at M2 with the full instance-block dump — that is the deliverable, not a hack (brief).

## Milestones

- **M1 — the ladder as a behavioural witness, not a boot census (R80).** `tests ce` runs the
  existing CE rungs R1/R2/R2b/R3/R5 **in order**, each printing its own readbacks, and **STOPS at
  the first that fails**, printing `:: CE: rung=<r> verdict=<v> ctl=<pre>/<post> ::`. The §3
  falsification story for each rung is in the rung's own verdict line (unchanged). Nothing runs at
  boot. `kepler::init` stores `(bar0, bar1_base, bar1_size, vram_size)` via `arm_context`.
- **M2 — the channel.** A PCOPY channel: instance block, an identity page directory (a small
  VRAM window so GPU-virtual == VRAM offset for the scratch region), RAMFC, the USERD doorbell, and
  a pushbuffer in VRAM the GPU can reach through BAR1 identity. Bind the copy engine; submit a
  minimal pushbuffer; ring the doorbell; poll the GET pointer with a **bounded** wait. The first
  write that proves the GPU READ something we wrote is **GET advancing past PUT**:
  `[ce] get=<g> put=<p> advanced=<0|1>`. `advanced=1` is the milestone of the whole campaign. If it
  never advances, the arc STOPS with the instance-block dump (`[ce] instblk off=<o> [..8 dwords..]`).
- **M3 — the blit.** A 256×256 ARGB source in VRAM with a known pattern, a destination in VRAM, the
  copy-class methods (SET src/dst address, pitch, line length, line count, LAUNCH_DMA), a semaphore
  release at the end; wait on the semaphore with a timeout; read the destination back through BAR1
  and checksum it against the CPU's own copy of the pattern. Witness:
  `:: KBLIT: engine=<ce0|gr> get_advanced=<0|1> sem=<ok|timeout> csum=<match|mismatch> us=<n> -> PASS|FAIL ::`.
- **M4 — hygiene.** Every register write has its readback on the same line. The whole arc is behind
  `nvidia-kepler-ce`, default OFF (knob-off image is byte-identical). A **wedge detector**: every
  wait is bounded by `arch::hw_wait_budget()` (rdtsc wall-clock) and prints on timeout, so a stalled
  GPU never costs the console (the BAR1WEDGE lesson). `tests ce` and `tests kblit` are the only
  entry points.

## The witness lines, and what boot 20's `tests kblit` says in each failure mode

| condition | `tests kblit` prints |
| --- | --- |
| no FALCON-REST CE base this boot | `:: KBLIT: engine=none get_advanced=0 sem=skip csum=skip us=0 -> FAIL ::` (channel not built; `[ce] no FALCON-REST CE base — nothing submitted`) |
| channel built, engine never fetches | `[ce] get=0 put=<p> advanced=0` then the instance-block dump, then `:: KBLIT: engine=ce0 get_advanced=0 sem=skip csum=skip us=<n> -> FAIL ::` (the STOP point; the expected metal outcome) |
| engine fetched, semaphore never released | `[ce] get=<g> put=<p> advanced=1`, `:: KBLIT: engine=ce0 get_advanced=1 sem=timeout csum=<..> us=<n> -> FAIL ::` |
| engine fetched, blit landed | `:: KBLIT: engine=ce0 get_advanced=1 sem=ok csum=match us=<n> -> PASS ::` — **the first GPU-executed operation on this machine** |
| engine fetched, blit wrong | `... sem=ok csum=mismatch ... -> FAIL ::` |

## GK107 register offsets used, with source

All BAR0 MMIO unless noted. Citation classes as `kepler_ce.rs` header defines them.

- `NV_PMC_ENABLE` 0x200 — [TREE] `kepler.rs::regs`. The copy-engine + PFIFO enable bits.
- PFIFO base 0x2000; `RUNLIST_SUBMIT` 0x2270/0x2274 — [TREE] `kepler.rs` (the pair this driver
  submits to). PCOPY runlist id is a **[EXT-UNPINNED]** hypothesis the CE ladder R1/R3 would pin.
- PCOPY0 PRI base 0x104000 (also 0x105000/0x106000 probed) — **[EXT-UNPINNED]** `kepler_ce.rs::CE_BASES`.
- USERD `+0x40` GP_PUT, `+0x44` GP_GET, `+0x88` IB_GET — **[EXT-UNPINNED]** / [TREE: envytools
  dma-pusher "Channel control area" quoted in §2's KF27 bullet; IB_GET is the register KF27 read
  for the first time]. KBLIT reads GET at `+0x44` (the gpfifo GET) and, on stall, IB_GET at `+0x88`.
- `PBUS_BAR0_WINDOW` 0x1700 (PRAMIN window, 64 KiB units), PRAMIN aperture 0x700000 — **[METAL,
  Boot A]** `kepler_ce.rs::PBUS_BAR0_WINDOW` (bar1-identity, KF24).
- Copy class **0xA0B5** (KEPLER_DMA_COPY_A / GK104_COPY) methods: `LAUNCH_DMA` 0x0300,
  `OFFSET_IN_UPPER/LOWER` 0x0400/0x0404, `OFFSET_OUT_UPPER/LOWER` 0x0408/0x040C, `PITCH_IN` 0x0410,
  `PITCH_OUT` 0x0414, `LINE_LENGTH_IN` 0x0418, `LINE_COUNT` 0x041C, `SET_SEMAPHORE_A/B/PAYLOAD`
  0x0240/0x0244/0x0248 — **[EXT-UNPINNED]** (class method numbers recalled; no rnndb file opened,
  clean-room §2). Marked at the site. A metal boot's `advanced`/`csum` is what pins or kills them.

The instance-block layout reuses `kepler.rs`'s UNAUDITED RAMFC constants (clean-room §5 disclaimer
stands; `kepler_ce.rs` R5 is the rung that would supply a Group-A layout, `never-run`). KBLIT does
not claim they are validated; it is an experiment whose witness line convicts them either way.

## What stays owed

- The GPU page tables for a **sysmem** source (M3's buffers are both in VRAM via BAR1 identity; a
  sysmem source needs a real GMMU walk the identity PD does not provide).
- The clean-room-legal RAMFC layout (CE ladder R5, `never-run`) and the PCOPY runlist id (R1/R3).
- Any visible blit into the live scanout: §5 proves Kepler owns the panel and a blit into it would
  be a write into the live framebuffer; KBLIT's destination is CPU-readable scratch, deferred visible.
- The copy-class method numbers above are [EXT-UNPINNED] until a metal boot moves `csum` to `match`.
