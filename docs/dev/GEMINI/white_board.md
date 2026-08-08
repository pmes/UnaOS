# WHITE BOARD — 2026-08-08 (GR22 close)

## ✅ ALL FIVE CRISPY QUESTIONS ANSWERED BY THE TASTE GATE (Peter, 2026-08-08)

Recorded verbatim, with what each one costs to build. **The shared-source law applies to every
one of these: the value changes in `kits/crispy/theme.json` FIRST, then is re-lifted into
`video/theme.rs`.** Nothing here has been started — GR22 closed on "no new work".

### A1 — the desktop. **A SCENE, and DAY/NIGHT ON THE CLOCK.**
> *"do a take on plasma sub-arctic theme (we must have day/night mode that switches with the
> clock) that could be a scene from pristine little northern minnesota lake where UM researchers
> invented the honeycrisp had their orchard"*

The reference is the University of Minnesota Horticultural Research Center at Excelsior on Lake
Minnetonka — where Honeycrisp was bred. Cold clear water, birch and granite, low northern light,
an orchard on the shore. A sub-arctic Plasma take, rendered as a scene rather than a flat fill.

⚠ **THIS CHANGES THE THEME'S ARCHITECTURE, and it is the most important consequence on this
board.** `video/theme.rs` is today *"byte-inert by construction (all `const`, no statics, no code,
compile-time-only assertions)"* — `engine.md` §9 verified that by hashing `kernel8.img` with and
without it. **Day/night on the clock means two palettes and a clock-driven selector**, i.e. state
and code where there is now only data. Options, cheapest first, for whoever takes the arc:
  a) two `const` palettes + a selector function; the table stays data, one small function chooses.
  b) a palette *pair* per role with the selector inlined at each read site.
  c) something richer (dusk/dawn interpolation) — but note the tree has an integer-only gradient
     interpolator now (`blend_q16`), so a timed crossfade is reachable without float.
The clock exists and is real: the boot already runs SNTP (`[sntp-x86]` on the AN capture) and has a
calibrated ms timebase. **Whoever builds this must say which option and why, and must re-verify the
byte-inertness claim in §9 or explicitly retire it.**

Also: a *scene* is not a palette role. It is a material — and §9 already draws that line, deliberately
excluding the kit's `content_surface.Paper` block because *"lifting it means porting a multi-octave
noise generator, a rasterizer concern."* A lake scene sits on the same side of that line. Decide
whether it is a procedural material, a lifted image, or a gradient-plus-silhouette built from
palette roles — and note the kernel has no float and no image decoder.

### A2 — the middle and zoom controls: **whatever the Mac standard is.**
> *"whatever mac standard is"*

So: **close / minimise / zoom.** Minimise has a home already — the compositor's `set_hidden` is
merged and metal-proven (a hidden window parks instead of starving the compositor). Zoom needs
maximise/restore, which means remembering a pre-zoom rect per window. Close already works.
Note the current paint is right-aligned with close leftmost; macOS is upper-LEFT. See A5 — Peter
has explicitly deferred placement.

### A3 — hover / pressed / disabled: **yes, and the vocabulary is named.**
> *"oh yes we want it very modern i guess this will be very Scandinavian refined but minimal"*

Add the states to the kit. The idiom is **modern, Scandinavian, refined, minimal** — which reads as
restraint: small deltas, no ornament, no drop shadows for their own sake. Kit json first.

### A4 — focus: **make the whole window more distinguishable, not just the caption.**
> *"tweak to make it more distinguishable as a whole?"*

Today the two title gradients differ by a few LSBs and only the ink differs strongly, so focus reads
almost entirely in the caption. Peter wants the **whole window** to read as focused. ⚠ Constraint
that must not be broken while doing it: **FOCUS-HL's law is that focus never moves a pixel** — the
distinction has to come from colour, not geometry, or the four-rect damage subtraction in
COMPOSITE-2 stops being focus-independent. Kit json first.

### A5 — control side and order: **leave it, revisit later.**
> *"ok we can always change things later!"*

Right-aligned, close leftmost, stands for now.

---

**Known residual, unchanged:** rounded corners are cut against the desktop colour rather than
sampled, because WC-H requires the pass to write every pixel of the box. Visible only when a manual
drag stacks two windows — about 31 px per top corner. A1's scene will make this more visible than
the flat purple does, so it likely wants fixing in the same arc.
