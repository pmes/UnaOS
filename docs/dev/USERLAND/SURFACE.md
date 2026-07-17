# Surface — the material layer beneath quartzite content

*Status: SURFACE-1 complete through M2. The taste-gate **PASSED 2026-07-17**;
the picked paper is the `Surface::Paper` default (see "Taste-gate record").
Views still opt in explicitly — adoption stays OFF everywhere until a
view-owner's arc.*

## What a Surface is

A **Surface** is a material layer owned by a *region* (a panel, a window, a text
area), not a per-widget style attribute. Content composites onto it; widgets
inherit the material they sit on. One sheet of paper, with ink laid on top —
never per-widget texture (that is the skeuomorphism-kitsch path).

Paper is the first (and hardest-to-overdo) material. Brushed metal, glass, and
the sci-fi control panel are future variants — not built here.

The design of record is Peter's seed, `~/.claude/plans/unaos/future/
unaos-texture-and-retro-kits.md`. The acceptance bar is his, verbatim:

> **"Nailing it would be a person wishing they could touch it."**

Texture as *tactility*, never decoration.

## The two layers

| Layer | Where | Concern |
|-------|-------|---------|
| `quartzite::surface` | platform-agnostic, no `unsafe` | the procedural paper algorithms + the contrast budget + determinism. Fully unit-tested. |
| `platforms::macos::paper_board` | AppKit backend | composites a `surface` raster under **native** text; the sample-board view. |

A backend asks the core for a device-pixel field or an RGBA8 raster and draws
content *on top*. Text never composites over the texture — the paper sits under
the glyphs, so glyph antialiasing stays entirely the OS's and is unharmed by
construction.

## Paper = light behaving correctly

Three ingredients, in payoff order (from the settled design):

1. **Ink into the fibers** — a hint of edge-darkening/bleed at glyph
   boundaries. This perturbs glyph edges, so it needs the glyph coverage and
   belongs to the **GPU path** (a euclase shader). It is *not* on the CPU/AppKit
   board path, where the texture is strictly under native text. Flagged as a
   future ingredient.
2. **Micro-relief** — a fixed, very-low-amplitude perturbation that reads as
   surface tooth (lambertian variation, not flat noise). Shipped.
3. **No tiling artifacts, ever** — hash-based procedural noise seeded per
   region. Shipped: every field is a pure function of an integer seed; adjacent
   regions get distinct seeds, so no seam is ever visible across a layout.

### The three algorithms (the sample space)

Genuinely different characters, so the taste-gate spans a real space rather than
one look at three strengths:

- **`grain`** — isotropic micro-relief: the emboss (directional gradient) of
  fine hash value-noise, lit from the top-left. Paper *tooth*.
- **`laid`** — the directional structure of laid / mould-made stock: closely
  spaced wire (laid) lines crossed by sparse chain lines, their amplitude
  modulated by low-frequency noise so they read as irregular fibre, never a
  mechanical grating.
- **`blotch`** — low-frequency luminance cloudiness (a few octaves of fBm), the
  gentle unevenness of real stock held to the light.

## The contrast budget

Subtlety is the whole game; the Gemini failure was *execution* — an over-strong
texture reads as kitsch instantly. Every generated field is bounded, **by
construction**, to a caller-declared maximum *relative luminance deviation*
(`PaperParams::amplitude`, a fraction — e.g. `0.02` = ±2 %).

The field modulates the base colour multiplicatively — `out = base × (1 +
amplitude × field)`, per channel, with `field ∈ [-1, 1]`. Because the scale is
uniform across channels, the relative luminance deviation of every pixel equals
`amplitude × field ∈ [-amplitude, +amplitude]`. `measure_max_deviation`
recovers that number from a rendered raster, so the budget is asserted
**measured**, not by eye (`surface::tests::contrast_budget_respected`).

## CPU-path honesty

A backend that cannot render a surface well renders it as **nothing** — a clean
flat fill — never a bad approximation (Peter, verbatim). The board's control
column is exactly this: honest flat stock, the zero every algorithm is judged
against.

## Retina scale-2

The board raster is generated at `cell_points × backingScaleFactor` device
pixels and the `NSBitmapImageRep` is tagged at point size, so it presents 1:1 on
a 2× display (the 2012 rMBP target class). Native text is already
resolution-independent.

## The sample board (the M1 deliverable)

One window, a grid of real text views each setting the same paragraph over a
candidate paper surface:

- **rows** — the three algorithms (`grain` / `laid` / `blotch`);
- **columns** — `control` (no texture) · `subtle` (±1.0 %) · `medium` (±2.0 %) ·
  `strong` (±3.5 %);
- each cell labelled with its exact parameters (algo, amplitude %, feature
  scale, octaves, seed).

Deterministic — the same pixels every launch, no shimmer.

Run it, from the repo root, on the Mac the taste-gate runs on:

```
cargo run -p quartzite --example paper_board
```

## Taste-gate record

**PASSED — 2026-07-17, attended (Peter + Maestro).**

The winner: the **laid** algorithm at the board's **medium** cell —
**amp 2.0 %, scale 4 px**, the board's laid octaves (3) and that cell's exact
seed (`0xFBB6_0E9F` = `SEED_BASE + cell_seed(row 1, col 2)`).

Peter's words, verbatim:

> "lets default to this for now and we'll put in sliders to play with later so
> people can dial it in or turn it off."

## M2 — the capability (landed)

The gated pick is encoded once, as `quartzite::surface::GATED_PAPER`:

```rust
PaperParams { algo: Laid, amplitude: 0.020, scale: 4.0, octaves: 3, seed: 0xFBB6_0E9F }
```

- **`Surface`** (re-exported as `quartzite::Surface`) is what a region declares:
  `Surface::None` (the universal default — clean flat, the honest zero) or
  `Surface::Paper(Paper)`.
- **`Surface::Paper` with no explicit params IS the gated look**:
  `Paper::default()` / `PaperParams::default()` / the `Surface::paper()`
  shorthand all resolve to `GATED_PAPER` on the canonical `PAPER_STOCK`. Tested
  exactly (`surface::tests::default_params_are_the_gated_pick`), and tied to the
  board's laid/medium cell byte-for-byte
  (`paper_board::tests::gated_default_is_the_laid_medium_cell`) so the default
  keeps meaning the gated pixels if anything drifts.
- **Views still opt in explicitly.** No view or vessel was rewired in this arc;
  adopting paper in tabula / midden / the chat view is a future arc per
  view-owner. A backend renders a declared surface via
  `Surface::render_rgba8(w, h)` (`None` for `Surface::None` — paint clean flat).
- **Everything stays parametric in one place** (`PaperParams`) — the hook a
  future theming/kit layer (retro-kits thread 2) addresses.

## SURFACE-2 (candidate scope — pre-registered, NOT built)

Peter's "sliders to play with later", named here as the follow-on arc:
**per-user surface settings** — dial-in controls for the paper parameters
(amplitude/scale at minimum) **plus an off switch**, addressing `PaperParams` /
`GATED_PAPER` as the single source. Do not build ahead of its own brief.

