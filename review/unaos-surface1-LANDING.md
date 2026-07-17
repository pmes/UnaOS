# SURFACE-1 landing report — the quartzite Surface primitive + the paper taste-gate (hw-jetson, host-native)

## Summary

**Full arc (M1 + taste-gate + M2).** SURFACE-1 built the Surface machinery and
an honest paper sample space (M1, first commit); Peter ran the board and the
attended **taste-gate PASSED 2026-07-17** — the pick is the **laid / medium**
cell (amp 2.0 %, scale 4 px, oct 3, seed `0xFBB6_0E9F`), his words verbatim in
`docs/dev/USERLAND/SURFACE.md`: "lets default to this for now and we'll put in
sliders to play with later so people can dial it in or turn it off." M2 (second
commit) folds the pick into the API as the default of `Surface::Paper`.

### M2 — the capability, as encoded

- `quartzite::surface::Surface` (re-exported as `quartzite::Surface`): what a
  region declares. `Surface::None` is the universal `Default` (clean flat, the
  honest zero); `Surface::Paper(Paper)` is the material.
- **The gated default, exactly** (Peter's amendment: no-params `Surface::Paper`
  = the gated look):

  ```rust
  pub const GATED_PAPER: PaperParams = PaperParams {
      algo: PaperAlgo::Laid,
      amplitude: 0.020,   // ±2.0 % max relative luminance deviation
      scale: 4.0,         // laid-line pitch, device px
      octaves: 3,
      seed: 0xFBB6_0E9F,  // the board's laid/medium cell: SEED_BASE + cell_seed(1, 2)
  };
  ```

  `PaperParams::default()` / `Paper::default()` (on `PAPER_STOCK`) /
  `Surface::paper()` all resolve to it; backends render a declared surface via
  `Surface::render_rgba8(w, h)` (`None` for `Surface::None`).
- Views still opt in explicitly — no view/vessel rewiring this arc; adoption
  stays OFF until each view-owner's arc. Parameters stay fully parametric in
  one place (`PaperParams`); **SURFACE-2 candidate scope pre-registered** in
  SURFACE.md (per-user sliders: dial-in + off switch), not built.

Two layers landed at M1:

- **`libs/quartzite/src/surface.rs`** — the platform-agnostic, `unsafe`-free
  core. A `Paper` surface = a base stock colour + `PaperParams { algo, amplitude,
  scale, octaves, seed }`. Three procedural, hash-based (tile-free), fully
  deterministic paper algorithms:
  - `grain` — isotropic micro-relief, the emboss (directional gradient) of fine
    value-noise fBm, lit top-left (paper tooth);
  - `laid` — closely spaced wire (laid) lines + sparse chain lines, amplitude
    modulated by low-freq noise so they read as irregular fibre;
  - `blotch` — low-frequency fBm luminance cloudiness (real-stock unevenness).
  The signed field is bounded to `[-1, 1]`; the raster applies it
  multiplicatively (`out = base × (1 + amplitude × field)`) so the relative
  luminance deviation of every pixel is exactly `amplitude × field ∈
  [-amplitude, +amplitude]` — the **contrast budget**, honoured by construction
  and `measure_max_deviation`-recoverable.
- **`libs/quartzite/src/platforms/macos/paper_board.rs`** — the AppKit sample
  board view. Each cell draws its paper raster (generated at
  `cell_points × backingScaleFactor` device pixels, `NSBitmapImageRep` tagged
  sRGB, sized to points → 1:1 at Retina scale-2) as the card background, then
  draws the specimen paragraph with the **native** text stack on top. Texture is
  strictly under the glyphs; glyph antialiasing is the OS's and unharmed.
  Control cells render honest flat stock (the "render nothing" CPU-path honesty
  rule).
- **`libs/quartzite/examples/paper_board.rs`** — the runnable host.

## The exact command Peter runs

From the repo root, on the Mac the taste-gate runs on:

```
cargo run -p quartzite --example paper_board
```

Opens one 1180×900-pt window titled "SURFACE-1 · paper sample board".

## What each cell shows

A 3-row × 4-column grid of real text views, each setting the **same** paragraph
over its candidate surface, each labelled with its exact parameters:

| row \ col | col 0 | col 1 (subtle) | col 2 (medium) | col 3 (strong) |
|-----------|-------|----------------|----------------|----------------|
| **grain**  | control — no texture | amp 1.0 % | amp 2.0 % | amp 3.5 % |
| **laid**   | control — no texture | amp 1.0 % | amp 2.0 % | amp 3.5 % |
| **blotch** | control — no texture | amp 1.0 % | amp 2.0 % | amp 3.5 % |

Per-algorithm feature parameters (fixed across the amplitude ladder):

| algo | scale (device px / lattice unit or laid pitch) | octaves |
|------|-----------------------------------------------|---------|
| grain  | 2.5 | 3 |
| laid   | 4.0 | 3 |
| blotch | 6.0 | 4 |

`amplitude` = the contrast budget (max % relative luminance deviation): the three
subtlety levels are **±1.0 % / ±2.0 % / ±3.5 %**. Each cell also carries a per-cell
seed so no two cards share a field (no visible tiling across the layout). The
base stock is warm off-white `sRGB (0.960, 0.949, 0.918)`; ink is warm near-black
`#1E1B18`; the board field is a neutral grey so the stock is judged calm.

The first column is the honest zero — every algorithm is judged beside a
no-texture control at reading distance.

## Gate results (verbatim)

`cargo test -p quartzite` (after M2):
```
test platforms::macos::paper_board::tests::gated_default_is_the_laid_medium_cell ... ok
test surface::tests::default_params_are_the_gated_pick ... ok
test surface::tests::deterministic_field ... ok
test surface::tests::seed_decorrelates ... ok
test surface::tests::deterministic_rgba ... ok
test surface::tests::field_bounded ... ok
test surface::tests::texture_is_present ... ok
test surface::tests::contrast_budget_respected ... ok

test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```
(The two determinism tests answer "same params = same pixels"; the
`contrast_budget_respected` test answers "budget respected — measured, not
asserted by eye"; `default_params_are_the_gated_pick` asserts
`Surface::Paper`'s no-params default equals the gated parameters exactly, and
`gated_default_is_the_laid_medium_cell` ties that default byte-for-byte to the
board cell Peter picked.)

`cargo build -p quartzite --example paper_board` → Finished, clean. The built
binary launches, opens the window, and runs `drawRect:` for every cell without
panic (a panic in an objc callback aborts; it stayed alive).

Mac-native quartzite consumers unbroken:
```
cargo check   (default-members)          → Finished, clean
cargo check -p facet -p lumen -p pulse -p phonolite → Finished, clean
```

`./arroyo check` (both arches — zero kernel surface):
```
✅ x86_64 OK
✅ aarch64 OK
```

## Flagged

1. **Taste-gate PASSED; M2 landed.** The pick (laid/medium, amp 2.0 %, scale
   4 px, oct 3, seed `0xFBB6_0E9F`) is recorded verbatim in
   `docs/dev/USERLAND/SURFACE.md` → "Taste-gate record", dated 2026-07-17.
   Per Peter's amendment, the pick is the *default* of `Surface::Paper`; views
   still opt in explicitly and no view/vessel was rewired. SURFACE-2 candidate
   scope (per-user sliders: dial-in + off switch) is pre-registered in the doc
   and deliberately NOT built.

2. **Ink-into-fibers is NOT on the board.** Ingredient (a) of the settled design
   (edge-darkening/bleed at glyph boundaries) perturbs glyph edges, so it needs
   glyph coverage and belongs to the euclase GPU path — out of an honest
   "texture strictly under native text" CPU board. The board samples ingredients
   (b) micro-relief and (c) tone. Documented in SURFACE.md; a GPU-path future.

3. **No agent-side visual confirmation.** `screencapture` is blocked for the
   background-agent shell (no screen-recording permission; rc=1). This is fine —
   the visual judgment is definitionally the attended taste-gate's, and pixel
   *correctness* (determinism, bounds, budget, presence) is covered numerically.

4. **`cargo check --workspace` fails on gtk/gio (pre-existing, environmental).**
   The gtk-featured crates want `pkg-config`/`gio-2.0`, absent on this Mac —
   unrelated to this change and present before it. The Mac-native consumer set
   (above) is clean. Not a regression.

## Lane / tripwire notes

Lane held: touched only `libs/quartzite/**` (new `surface.rs`, new
`platforms/macos/paper_board.rs`, one-line registrations in `lib.rs` and
`platforms/macos/mod.rs`, new `examples/paper_board.rs`) plus the named docs
(`docs/dev/USERLAND/SURFACE.md`, `docs/MILESTONES.md`, this report). No
handler/vessel rewiring, no kernel surface, euclase untouched. No STOP tripwire
tripped. Committed on `hw-jetson` only; no merge, no push, no stash.

