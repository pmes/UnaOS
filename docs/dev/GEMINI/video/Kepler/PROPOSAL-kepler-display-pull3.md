STATUS: PROPOSED

# PROPOSAL — kepler-display pull 3: candidate decode

## 1. Intent & Scope
Following `BRIEF-kepler-display-pull3-decode.md`, this pull is strictly **read-only**. It will gather data across three time-separated passes over three dense register windows for both head 0 and head 1. We will also perform in-kernel arithmetic cross-checks on the stable head-0 candidates to decode their possible meanings (address vs pitch/format vs timing).

No implementation code will be written until this proposal is reviewed and approved.

## 2. Implementation Steps

### Milestone 1: Dense Window Reads
We will read three narrow windows sequentially for both head 0 and head 1:
- `0x300–0x35F`
- `0x3F0–0x40F`
- `0x5F0–0x61F`

Zeros will be printed as zeros (no skip-zeros filtering).
We will use the exact serial markers for each row and window completion:
- `:: kdisp: window head<H> pass<P> off=XXX val=XXXXXXXX ::`
- `:: kdisp: window head<H> pass<P> done rows=N ::`

### Milestone 2: Time-Separated Passes
We will execute the dense window reads (Milestone 1) across **three passes** separated by bounded delays (using the existing bounded-poll idiom) that approximate a minimum of ~2 raster frames of delay between passes. This will distinguish frame-varying live telemetry from stable config registers.

### Milestone 3: Arithmetic Cross-Check
For the stable head-0 candidates identified in pull 2 (`0x310`, `0x520`, `0x604`, `0x614`), we will perform in-kernel arithmetic and print the results alongside the known truth (GOP fb `0x90020000`, vram_off `0x20000`, and panel timing from HEAD_STAT).

The derived interpretations calculated and printed will be:
- `value << 8`
- `value << 12`
- `value` as pitch (bytes and `/4`)
- `value` vs hsync/vsync totals

We will emit the following markers per candidate:
- `:: kdisp: cand off=XXX stable=<yes|no> v0=XXXXXXXX v1=XXXXXXXX v2=XXXXXXXX ::`
- `:: kdisp: cand off=XXX shl8=XXXXXXXX shl12=XXXXXXXX pitch4=DDDD ::`

We will preserve the existing begin-trace/caps/stat header markers exactly.

## 3. Gates
Before concluding this pull, I will ensure the following gates are passed:
- **Read-only execution**: No register writes will exist in this pull.
- **Full-knob check**: `UNAOS_IVB UNAOS_KEPLER UNAOS_KEPLER_TAKEOVER UNAOS_KEPLER_FIFO ./arroyo check` runs successfully on both x86_64 and aarch64 arches.
- **Builder-path build**: `esp-x86` builds properly.
- **Strings proof**: `strings` shows all new `:: kdisp:` markers in both `kernel.elf` and `BOOTX64.EFI`.
- **QEMU Regression**: Default QEMU regression runs green.
- **Hygiene**: Bounded delays/polls are used everywhere. All docs and code will be committed. Scratch files will be deleted, and `git status` will be clean.
