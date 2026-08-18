STATUS: APPROVED (2026-07-25, coordinator GR4). No amendments — the
barcode pattern plus the post-latch reg dump discriminates all three
hypotheses, and the falsification table is exactly what was asked for.

# Proposal — kepler-display pull 18: placement-model probe

## Objectives
Since the S27 photo showed our `row=0` white stripe ~62–66% down the panel (with all other stripes pushed off-screen), a simple 1:1 layout with no offset is refuted. We will deploy a discriminating pattern (a 16-row banded barcode) and read back the post-latch register cluster to falsify the remaining hypotheses (vertical scaling, truncated pointer granularity, or a smaller scan window).

## Falsification Table

| Hypothesis | Expected Visual Result in Photo | Expected Register Readback |
| :--- | :--- | :--- |
| **(a) Vertical Scaling** | The 16-row colored bands and their embedded barcodes will appear vertically stretched (e.g., measuring >16 panel pixels tall). The total number of encoded memory rows visible on the panel will be less than 1800. | Viewport/raster registers (`0x4B8`-`0x4C8`) may show scaling factors or window sizes smaller than 1800. |
| **(b) Smaller Scan Window** | The 16-row colored bands will be exactly 16 panel pixels tall (1:1 scaling). The barcode is readable, but the rendered image simply stops before reaching the bottom of the panel, leaving the rest black/unrendered. | Viewport/raster registers (`0x4B8`-`0x4C8`) will indicate a display window smaller than `1800`. |
| **(c) Pointer Granularity / Offset** | The 16-row colored bands will be exactly 16 panel pixels tall (1:1 scaling). The rendered image fills the space, but the barcode row index at the physical top of the panel will **not** be `0`. (e.g., our row 0 is pushed down because the hardware truncated our `0x016000` pointer to something like `0x010000`, scanning from an earlier VRAM base). | `0x640460` armed readback will show missing or shifted bits compared to our `0x016000` write. |

## Plan
We will update `unaos/crates/kernel/src/drivers/gpu/kepler_display.rs`. The code remains a single linear cycle (`pitch = 16384`, `expected_height = 1800`).

1. **Discriminating Fill Pattern**:
   For each row `y` in `0..1800`, calculate `band_idx = y / 16`.
   - `band_color`: Cycles through RED, GREEN, BLUE, YELLOW, CYAN, MAGENTA, WHITE, GRAY based on `band_idx % 8`.
   - For each column `x` in `0..4096`:
     - `x < 16`: `0xFFFFFFFF` (WHITE left-edge alignment marker)
     - `x < 32`: `0xFF000000` (BLACK spacer)
     - `x < 144`: 7-bit binary barcode of `band_idx` (16 pixels per bit). Bit `i = 6 - ((x - 32) / 16)`. If `(band_idx >> i) & 1 == 1`, color is WHITE, else BLACK.
     - `x < 160`: `0xFF000000` (BLACK spacer)
     - `x < 2880`: `band_color` (Solid color band)
     - `x >= 2880`: `0xFF000000` (BLACK padding bytes)

2. **Read-Only Evidence Channel**:
   During the 5-second hold (after the latch has armed and taken effect), we will perform a one-time register dump:
   - Read `0x640460`, `0x640468`, `0x64046C`, `0x640470`.
   - Read `0x6404B8` through `0x6404C8`.
   - Output using `:: kdisp: pm-step reg-dump ...` markers.

3. **Trace Markers**:
   - `:: kdisp: pm-step fill done bytes=01C20000 ::`
   - `:: kdisp: pm-step hold t=<n>s ::`
   - `:: kdisp: pm-step done ::`

## No other changes
Writes remain exactly `0x640460` followed by `0x640080`.
Once approved, I will implement this probe and commit the changes locally for the coordinator to run builds at land-review.
