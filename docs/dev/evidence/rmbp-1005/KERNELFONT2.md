# KERNELFONT2 — the faces reach Lumen, the console grid follows the panel's ppi, Noto on the builder (rmbp-ledger B363, R85)

Branch `exec-rmbp-kernelfont2`, cut from `exec-rmbp-merge14` @595732ed (boot-24 integration: KERNELFONT B359,
WINDOW2 B361, SELFBUILD6, AETHERFONT). No merge. R78: no QEMU — compile legs, host proofs, the metal boot is the seat's.

## Design

**Finding.** KERNELFONT (B359) left four things owed and R85 answered each: (10) LUMEN.ELF still draws font8x8
inside its window — the 4 MiB window could not hold a face, and WINDOW2 (B361) raised it to 64 MiB; (11) the theme
is in device px, so on the 220/227-ppi panel `system.display.font_size` is capped at 13.5 px by the 16-px cell and
the console is a 411x112 grid of 7x16 cells (2880x1800); (12) the builder host had no Noto faces, so
`faces=6/10 scripts_missing=noto-arabic+…`; and the Settings Font row is a read-out, windows painted before the
faces load keep their bitmap pixels.

**The seam.** Unchanged from B359: **shared-core** — `font_core::ui` (no_std, zero deps) is the ONE engine; the
kernel's `video::text` is its fulfiller for kernel surfaces and now LUMEN.ELF links the same `font_core::ui` for its
own window (the R82 shape: a library linked by the caller, nothing resident). The DPI scale is a new **Kernel — wm**
file, `video/dpi.rs` (`//! CHARTER: Kernel — wm`): the panel's effective ppi (EDID ppi x framebuffer width /
the EDID's native width), the scale `ppi / 96` rounded to the half-pixel (x2 integer, 1.0..=4.0), `px(n)`, and the
grid report. A preference stays Principia's (`system.display.font`, `system.display.font_size`).

**Milestones.**
- **M1 LUMEN.ELF draws with the faces on the volume.** `user-lumen` links `font_core` (ui) and reads
  DejaVu Sans / Sans Bold / Sans Mono off `/system/fonts` (or `/boot/system/fonts`) over `SYS_PATH_READ` into its
  heap at start; the chat is DejaVu Sans (proportional, wrapped by pixel width through the same `vein_core::scroll`
  ring), fenced and inline code DejaVu Sans Mono, bold the bold face; no face = font8x8 as before, said on the wire.
  The start line gains `font=<dejavu-sans|font8x8> [font_why=<w>] font_kib=<n>`; the kernel's `tests lumen` LUMENUX
  line gains `font=<dejavu-sans|font8x8>` (the face LUMEN.ELF will load, found and parsed on the volume).
  LUMEN-X86.ELF measured before/after through arroyo's own `build_user_lumen_x86` (ELFENTRY, window check).
- **M2 the console grid scales by ppi/96.** `video::dpi` (above); `video::text` gains `Face::Grid` — the console's
  cell, `font::CELL_W x CELL_H` x the scale (ceil): 7x16 -> 18x40 at 2.5 (220/227 ppi); its mono face is
  `font_size` CSS px x ppi/96 capped by that cell (13 CSS px wants 30.7 px; the 18-px advance holds 29.75), so
  `font_size` takes effect on the console; before the faces load the bitmap atlas is drawn at the integer part of
  the scale inside the same cell. Both console arm sites (x86 takeover, aarch64 face arm) take the scaled cell;
  `[kfont] load … ppi=<p> scale=<s> grid=<cols>x<rows>`. GLASSEYES golden README re-pinned with the reason.
- **M3 Noto on the builder.** arroyo `ensure_kfont_noto` before the builder runs: the four Noto script faces
  present in a package dir or `UNAOS_FONT_DIRS` -> nothing; else `apt-get install -y fonts-noto-core` (sudo when
  not root); apt refused -> the Ubuntu pool `.deb` (SHA-512 pinned from the archive index), each face SHA-256
  pinned, extracted into the gitignored `target/fonts-noto/` and exported as `UNAOS_FONT_DIRS`. The builder takes
  a licence from a font dir's `copyright` too -> `faces=10/10 scripts_missing=none`.
- **M4 repaint on load + the Font picker.** When the faces load (and on every restyle) the kernel windows that
  cache their own pixels repaint once (Settings, Activity, Quarry's panes, the console from its cell store);
  the Settings Display tab's Font row becomes a picker (sans / serif / mono, and the size − / +) that writes
  `system.display.font` / `font_size` through Principia.

**Witness (metal).** `[kfont] load dir=/system/fonts faces=10/10 fallback=none scripts_missing=none … ppi=227
scale=2.5 grid=<w>x<h> …`; `tests font` -> `:: KERNELFONT: faces=10 … -> PASS ::`; `tests lumen` ->
`:: LUMENUX: md=ok history=ok clip=ok scroll=131072 -> PASS font=dejavu-sans ::`; `lumen` ->
`:: LUMEN: start … font=dejavu-sans font_kib=<n> ::`.

**Stays owed (named before the build).** The furniture outside the console — the menu bar / title strip
(`theme::TITLE_HEIGHT` 34), the control discs, `GAP`, the dock and crystal menus — stays in device px: it is a web
of `const` geometry with `const` assertions across `wm.rs` (a line-neutral file) and the fixtures. Kernel windows
whose painters lay out on the 7x16 cell (Settings, Activity, login, Quarry, Lumen) keep being magnified by the
compositor's integer window scale (2x..6x on the rMBP), so their text is DPI-sized by that factor, not by
`font_size`. Both are the next arc's: one runtime metrics table (`ui::Metrics` from `dpi`) the wm reads, with
those windows drawn at scale 1.
