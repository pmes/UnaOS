STATUS: PROPOSED

# PROPOSAL — kepler-display pull 4: EVO core-channel read-out

## 1. Intent & Scope
Following `BRIEF-kepler-display-pull4-evo-core.md`, this pull is strictly **read-only**. It will gather data from the EVO core channel to feed both the display surface hunt and the fence lane's disp-era-USERD fallback.

No implementation code will be written until this proposal is reviewed and approved (transitioning to `STATUS: APPROVED`).

## 2. Implementation Steps

### Milestone 1: Dense Core-Channel Window
We will read the window `0x610480–0x6104FC` sequentially across **two passes** separated by a bounded delay (using the existing `spin_loop` idiom). Zeros will be printed as zeros (no filtering).
We will use the following serial markers, where `off` is relative to `0x610480`:
- `:: kdisp: evo-core pass<P> off=XXX val=XXXXXXXX ::`
- `:: kdisp: evo-core pass<P> done rows=N ::`

### Milestone 2: Known-Value Scan
We will sweep the 4 KB window `0x610000–0x613FFC` without polling. We will only print hits, up to a maximum of 64 printed hits.
A hit is defined as:
- Exact match against one of these keys:
  - `0x00000200`, `0x00020000`, `0x90020000`
  - `0x00002D00` (pitch 2880×4)
  - `0x013C6800` (fb size)
  - `0x07380BAF`, `0x0BAF0738` (raster totals)
- OR `(val & 0xFFF00000) == 0x90000000` (BAR-window-shaped address)
- OR `(val & 0xFFFF) == 0x0B40 || (val >> 16) == 0x0B40` (2880-shaped)
- OR `(val & 0xFFFF) == 0x0708 || (val >> 16) == 0x0708` (1800-shaped)

We will emit the following markers:
- `:: kdisp: evo-scan hit off=XXXXX val=XXXXXXXX key=<keyname|barshape|w2880|h1800> ::`
- `:: kdisp: evo-scan done range=610000-613FFC hits=N capped=<true|false> ::`

We will preserve all existing begin-trace/caps/stat markers unchanged.

## 3. Gates
Before concluding this pull, I will ensure the following gates are passed:
- **Read-only execution**: No register writes will be added.
- **Full-knob check**: `UNAOS_IVB UNAOS_KEPLER UNAOS_KEPLER_TAKEOVER UNAOS_KEPLER_FIFO ./arroyo check` runs successfully on both x86_64 and aarch64 arches.
- **Builder-path build**: `esp-x86` builds properly.
- **Strings proof**: `strings` shows all new `:: kdisp:` markers in `kernel.elf`.
- **QEMU Regression**: Default QEMU regression runs green.
- **Hygiene**: Bounded delays are used. All docs and code will be committed. Scratch files will be deleted, and `git status` will be clean. **No push will be performed.**
