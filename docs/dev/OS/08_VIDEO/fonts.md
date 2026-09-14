# FONTS — which face each surface draws with, and why some of them look blocky

> Companion to `crates/kernel/src/video/font.rs`. That module doc describes the anti-aliased face
> and names a "gap, not a fold" list of surfaces that never took it. This file is the measurement
> of what that gap actually costs on glass, written for SO48, plus the exact remaining work.

## 1. The complaint, and what was inference in it

Peter, on the Orin bench panel (1920x1200): *"the fonts within the in quarry and the login windows
are blocky"*. The Orin queue's row (SO48) recorded the observation together with a derivation:

> The login box is `890x524` where the fixture measured `450x284` on its panel — very close to 2x,
> so these windows are being drawn through a 2x scale of a bitmap font on the 1920x1200 panel.

**The observation is a fact. The mechanism sentence was an inference, and it is wrong in three
ways.** It is not "very close to 2x" — it is exactly 2x, and the arithmetic proves which 2x. It is
not ONE 2x. And it is not the same mechanism in the two windows the row names.

## 2. What a bitmap face actually is, so "blocky" is a measurable word

`font8x8`'s `BASIC_LEGACY` is a **1-bit** 8x8 table: a glyph is 64 bits, each either ink or not.
Every surface still on it renders by REPLICATION — each set bit becomes a `ts`x`ts` square of flat
ink. That is the classic X11 `fixed` recipe and it has two consequences worth separating:

* **The face has no intermediate coverage at all.** A glyph edge is a step at any size. Nothing
  about the scale creates or destroys this; it is a property of a 1-bit table.
* **Replication multiplies the step, not the detail.** At `ts = 2` a diagonal's staircase is 2 px
  per tread; at 4 it is 4. The information content is identical at every `ts`.

So magnification is the AMPLIFIER of blockiness and the 1-bit table is the CAUSE. That distinction
is what makes the fix a face change and not a scale change, and it is why the fixture in §7 asks
about coverage values and not about pixel counts.

The anti-aliased face (`video::font`) is 8-bit alpha pre-rasterized from Noto Sans Mono, blended
against the destination — graded edges, real side bearings, a real baseline.

## 3. The mechanism, per surface, with citations

There are TWO independent magnifications in this tree and they compose. Nothing prevents a surface
from paying both, and the login screen does.

| stage | what it is | where it is decided |
|---|---|---|
| module `ts` | the surface's own block replication of the 1-bit cell, applied as it paints into its cached-RAM surface | per surface, see below |
| window scale | the compositor's integer nearest-neighbour upscale of the WHOLE finished surface onto the panel | `video/wm.rs` `scale_in` / `place_scale` |

### 3.1 The login screen — 4x, and it is the worst surface on the panel

* `video/login.rs:85-87` — `W = 440`, `H = 240`, `TS = 2`; `:88` `CELL = 8 * TS`.
* `video/login.rs:177-193` — `text()` reads `font8x8::legacy::BASIC_LEGACY` and fills a `TS`x`TS`
  rect per set bit. So the 440x240 surface already holds 16 px glyphs made of 2x2 blocks.
* The compositor then upscales that surface. **The queue's own measurement proves the factor**, by
  inverting the box formula `video/login.rs:521-525` (`bw = w * scale + 2 * BORDER`,
  `bh = h * scale + TITLE_H + 2 * BORDER`) with `video/theme.rs:301` `FRAME = 5` and
  `video/theme.rs:308` `TITLE_HEIGHT = 34`:

  * fixture panel, `scale = 1`: `440 + 10 = 450` by `240 + 34 + 10 = 284` — **`450x284`, exact**.
  * bench panel, `scale = 2`: `880 + 10 = 890` by `480 + 34 + 10 = 524` — **`890x524`, exact**.

  `890 = 440 * s + 10` has the single integer solution `s = 2`. The measurement is not "very close
  to" anything; it pins the window scale at 2 and at nothing else.

**Total: an 8x8 1-bit glyph reaches the glass as a 32x32 cell in which every source bit is a 4x4
square of flat colour.** That is the window Peter named first and it is the worst offender in the
tree.

### 3.2 Quarry — 2x, and NOT from the compositor

* `video/quarry/live.rs:321` — `let ts = if pw >= 1280 { 2 } else { 1 };`. This is the whole scale
  decision, and what selects 2 on a 1920x1200 panel is the panel WIDTH, not its height and not the
  compositor.
* `video/quarry/live.rs:1423-1431` — the same 1-bit replication blit, `g.ts`x`g.ts` per set bit.
* The compositor adds nothing here, and this is measured on the wire rather than derived. From
  `docs/dev/evidence/orin28/render14-boot1-desktop-menubar.log`:

  ```
  [quarry] open win=2 surf=1152x720 ts=2 box=1162x764 at (379,183) ...
  [wc-a] create win=2 asid=0xffffff03 surf=1152x720 stride=4608 scale=1x at (384,222) z=2
  ```

  `ts=2` from Quarry's own witness, `scale=1x` from the compositor's. **Quarry's blockiness is
  entirely `live.rs:321` and the window layer is not involved.** `1152x720` is at the surface CEIL
  (`live.rs:244-245`), which is why its window scale bottoms out at 1: `scale_in`'s fit term is
  `pw / 2 / w` = `1920 / 2 / 1152` = 0, raised to 1.

So the row's single sentence covers two different mechanisms, and the compositor half of it is true
of login and false of Quarry.

### 3.3 The rest of the 1-bit list, for completeness

* `crates/kernel/src/pal.rs:159` `draw_text` (and so `pulsewin`'s labels, `console`, `ui_status`) —
  replication at `ui::Metrics::for_height(ph).scale`, which is `clamp(ph / 900, 1, 4)`
  (`crates/kernel/src/ui.rs:37-50`). On this 1200-row panel that is **1**, confirmed on the wire as
  `:: UI1: scale=1 cell=8x8 line=12 ::`. These surfaces are not blocky — they are ~0.8 mm tall and
  effectively invisible, which is the same gap presenting from the other end.
* `video/instgui.rs` — was `TS = 2`; **converted by this arc**, see §5.
* `video/fbcon.rs` before the desktop seam — 1-bit **by design and correctly**; see `font.rs`'s
  module doc. Not part of this.

### 3.4 Why the gap persisted: the seam had the wrong shape

`video::font` shipped exactly two blits, and neither fits an app surface:

* `font.rs:279` `draw_row` — ONE scanline into a strip painter's scratch row.
* `font.rs:313` `draw_glyph_fb` — one glyph into a `FrameBuffer` against a COMPUTED background.

Every surface on the 1-bit list paints a RECTANGLE: it owns a `&mut [u32]` of cached RAM and writes
`px[(y + ry) * stride + x + rx]`. None of them could call either blit without rewriting its painter,
so all four hand-wrote the same sixteen lines of `font8x8` replication instead. **The gap was not a
capability gap and not an architectural one; it was a missing function signature.**

## 4. What this arc landed

`font.rs`'s tail gains `draw_text` — the rectangular-surface blit, same clip and truncation contract
the four hand-written helpers each wrote by hand, returning the pen x. It is appended at the file's
tail so no `panic::Location` below it moves.

## 5. instgui, converted — the worked precedent

`video/instgui.rs` is the one surface on the 1-bit list inside this arc's file list, and it is now
on the shared face. The conversion is three edits and is the template for the other three:

1. `const TS/CELL` → `const CELL_W = font::CELL_W; const CELL_H = font::CELL_H;` plus a `FACE`.
2. `text()`'s body → one `font::draw_text` call (the `\n` break stays local; the shared blit has no
   opinion about control bytes).
3. Every `CELL` use split by AXIS: vertical to `CELL_H`, horizontal to `CELL_W`.

**The metric split is the whole cost of a conversion and it is worth stating plainly.** The old
square cell was an artefact of the 1-bit table, not a metric anyone chose: a 16 px mono face is 7 px
wide, because that is what Noto's own side bearings give it. Since `font::CELL_H` is 16 — *the same
16 the old `CELL` was* — every vertical position in a converted module is unchanged to the pixel,
and only the horizontal arithmetic moves. Lines get narrower; a line that fitted at 16 px per
character cannot fail to fit at 7. Modules that used ONE constant for both axes must split it, and
that is the only place a conversion can go wrong.

instgui is `#[cfg(all(target_arch = "x86_64", feature = "wc", feature = "instgui"))]`
(`video/mod.rs:159`), so this conversion changes no aarch64 image and does not itself improve the
Orin glass.

## 6. The remaining work, exactly

Both files below were outside this arc's file list — `video/login.rs` is held by another executor
and `video/quarry/live.rs` was not named — so the edits are written down rather than made.

### 6.1 `video/login.rs` — the biggest single win on the panel

* Delete `TS` (`:87`); replace `CELL` (`:88`) with `CELL_W = font::CELL_W` / `CELL_H = font::CELL_H`.
* Replace `text()`'s body (`:177-193`) with
  `font::draw_text(px, W, W, H, x, y, s, fg, false, font::Face::Body);`.
* Split the axes: `:180`'s clip and `:206`'s caret x (`x + 6 + n * CELL`) are HORIZONTAL → `CELL_W`;
  `:196`, `:197` (`CELL + 8` field height) and `:207`'s caret height are VERTICAL → `CELL_H`.
* Nothing about the window's 440x240 surface or its scale changes, so `890x524` stays `890x524` and
  no fixture that measures the box moves. The glyphs inside it stop being 4x4 blocks.
* **Consider separately, and do not fold into the same commit:** at window scale 2 the surface is
  upscaled whole, so even a perfect face is resampled 2x. The honest end state is a login surface
  sized in PANEL pixels rather than a 440x240 one magnified — that is a layout arc, not a face one.

### 6.2 `video/quarry/live.rs`

* `Geom::ts` (`:254`) stops being a glyph replication factor. `cell()` (`:259-261`) splits into
  `cell_w()`/`cell_h()` over `font::Face`, and `row_h`/`bar_h`/`tree_w` follow the vertical one.
* `text()` (`:1416-1433`) becomes one `font::draw_text` call.
* `disclosure()` (`:1437+`) and any other `g.ts` geometry KEEP `ts` — it is a legitimate scale for
  drawn ornament; it was only ever wrong for glyphs.
* `:321`'s panel predicate can then be re-read as a FACE choice (`Face::Body` vs `Face::Chrome`)
  rather than a replication factor, which is the shape §7's "larger glyph set selected by panel
  height" asks for and costs nothing extra — `Chrome` is already in every image.

### 6.3 `crates/kernel/src/pal.rs:159`

Different complaint, same root: this one is too SMALL, not blocky. `Face::Body` is 16 px where the
panel currently gets 8. Deferred because `pal::draw_text` is the shared PAL trait method and its
callers' layouts are all expressed in `ui::Metrics`, which would have to learn the same axis split.

## 7. Memory cost — the number, because a second glyph table is bytes in every image

**This arc adds NO new glyph data.** `font.rs:223-226` declares four atlases — `REGULAR`, `BOLD`
(Size16) and `CHROME_REGULAR`, `CHROME_BOLD` (Size20 at `TITLE_HEIGHT = 34`) — and all four are
already reachable from `glyph()`, which `fbcon` calls on every build of either arch. Adopting the
face on a new surface therefore costs its call site and nothing else.

For the record, so a future arc can price a THIRD raster before adding one: the crate's per-raster
alpha data is `95 glyphs * height * width` bytes per weight — 10,640 B for Size16 (7 px advance) and
17,100 B for Size20 (9 px), so about 21 KiB and 34 KiB for a regular+bold pair. A `size_32` rung —
which `font.rs:130-131` already flags as what a taller bar would need — is the one to think twice
about.

### 7.1 ⚠ But the stripping is conditional, and this arc paid to learn how

"Already in every image" is true of the BODY pair and **not** of the chrome pair. `glyph()`'s
four-arm match on `face` is what lets the linker drop the atlases a build never draws with: with a
compile-time-constant `face` it folds to two statics, and in a build with no chrome — no menu bar,
no captions, no dock — the `Size20` pair is unreferenced and stripped.

Hand `glyph` a face the optimizer cannot see through and all four arms stay live. Measured, because
this arc did it by accident for one build: `core::hint::black_box(Face::Body)` in the fixture grew
the knob-off loadable image by **+87,482 B on aarch64 and +121,652 B on x86**
(`./arroyo knoboff wc f164b6fd`), and the aarch64 wc-off image came out the same size as the wc-on
one. That is the Size20 pair, priced independently at 98,040 B from the crate's geometry. The
fixture now black-boxes the STRING (which is what its legs actually need opaque) and leaves the face
constant.

**The rule, stated once: a `Face` must reach `glyph` as a compile-time constant at every call site.**
Every real call site already does — `crystal.rs:178`, `menubar.rs:167`, `dock.rs:344`,
`winmenu.rs:657`, `fbcon.rs:168`, and `instgui.rs`'s new `FACE` — because a surface's face is a
property of the surface, not of its data. A fixture is the one place that can get it wrong.

### 7.2 What this arc actually costs, bisected

`./arroyo knoboff wc f164b6fd` measures the DEFAULT (wc-off) loadable image against the baseline.
Three runs, each with its control fired:

| tree | x86 delta | aarch64 delta | exit |
|---|---|---|---|
| `draw_text` + the `const` gate + the whole instgui conversion, **fixture removed** | 0 | 0 | **0 — byte-identical** |
| + the `const` gate alone (measured with it removed: same numbers) | +34,868 | +570 | 1 |
| everything, as committed | +34,868 | +570 | 1 |

So: **the deliverable is free and the fixture is the entire cost.** The compile-time gate costs
nothing at all (it evaluates and is discarded — removing it changed neither number by one byte), and
`draw_text` plus the instgui conversion leave the default image byte-identical.

The fixture costs **570 bytes on aarch64** — the Orin's number, and the one that matters for this
track. The x86 wc-off figure of 34,868 B is close to one Size16 weight (36,480 B) and is **not
explained**; stated as an open number rather than reasoned about, since it appears only in a
configuration that compiles no compositor and ships no desktop. Whether to keep a 35 KB self-test in
that build is a seat call, and removing the fixture returns that image to byte-identical.

## 8. The gates: one at compile time, one at runtime

### 8.1 `font.rs`'s `const _` block — the one the battery actually holds

At `video/font.rs`'s tail, so every build of either arch evaluates it and `./arroyo check` is the
gate. It scans all 95 glyphs of both shipped rasters and requires PARTIAL coverage to exist
(`has_mid`), with `has_zero` as the control that the scan distinguished more than one class of byte.
Deliberately not required: a fully-opaque byte — whether a raster reaches 255 is a fact about Noto's
hinting at that size, not about whether the face is anti-aliased.

Go-red, both legs, measured in a throwaway host crate carrying the identical block (`cargo build`,
rc=101 each, text quoted in the commit body):

* thresholding every byte to 0-or-255 (a simulated 1-bit atlas) →
  `error[E0080]: evaluation panicked: FONTAA: the body atlas has no partial coverage — this is a 1-bit face`
* forcing every byte to 128 (a scan that sees one class) →
  `error[E0080]: evaluation panicked: FONTAA control: the body atlas scan saw no empty pixel — it read nothing`

### 8.2 `video.font.aa` in `selftest.rs` — the runtime half, and it is NOT on the battery wire

**Said plainly because the DONE gate asks for it:** this fixture is in `tste`'s LIVE section, and
that section does not execute during `./arroyo test`. Measured, not assumed — on the battery capture
this arc gated on, `LC_ALL=C grep -a -c -F ":: TSTE: suite start"` is 0, and so is the count for
`video.geometry`, a live fixture that has sat beside it for arcs. The `:: TSTE:` lines a battery
capture does carry come from boot-sequenced fixtures in `shell.rs` printing that tag directly. So
gate 3's rc=0 does **not** certify this fixture; §8.1 is what the battery holds.

Putting it on the boot wire is one line in `shell.rs`'s boot-fixture block —
`verdict("video.font.aa", ok, &why);` beside `shell.rs:3667`'s `verdict("vfsroute.route", …)` —
which was outside this arc's file list and is reported rather than written.

It is still the right fixture to have: it is the only one that exercises the actual blit, and a
bench operator can fire it. Four legs:

1. **advance** — the pen lands at `n * Face::cell_w()`, the face's own advance and not `8 * scale`.
2. **graded** — every pixel is background, ink, or a grey strictly between, and at least one is
   strictly between. Endpoints stay bit-exact so pixel-equality instruments elsewhere keep working.
3. **control, and it must hit** — the SAME string rendered through the 1-bit replication path this
   arc removes must come out at exactly two values. Without this leg, leg 2 is the kind of assertion
   that passes for a correct blit, a smeared buffer and an uncleared one alike.
4. **containment** — a glyph one pixel short of fitting draws nothing and does not advance the pen.

Reverting `draw_text`'s blend to a 1-bit threshold write reds leg 2 with the string in the message;
that mutation is the same one §8.1's first go-red applies to the atlas scan.

## 9. What is NOT here, said so it is not read as covered

* **`video/screen.rs` has no text blit.** It was named in this arc's file list; the search that
  established this was `grep -n -iE "text|glyph|font|blit" video/screen.rs` over the whole file,
  and every hit is prose or a row-copy. The only scale in it is `screen.rs:2225-2232`, the UVUG-7
  compat full-screen present, which no `wm` window takes. Nothing was changed there.
* **No metal.** Everything above is QEMU, source and the render14 capture. The Orin glass has not
  seen the converted face, and instgui does not build for aarch64 at all — the next Orin boot's wire
  is unchanged by this arc except for the one new `:: TSTE:` line.
