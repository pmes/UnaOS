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

## Results (compile legs and host proofs; R78 — no QEMU, the metal boot is the seat's)

| milestone | commit | content |
|---|---|---|
| M1 | 0c0ed4bc | `user-lumen/src/txt.rs` (faces over `SYS_PATH_READ`, per-byte advance tables, unshaped `draw_char_with` at the summed advances); `main.rs` lays the transcript with `vein_core::md::wrap_px` (new, host-tested) and draws through `txt`; start line `font=`; kernel LUMENUX `font=` via `video::text::volume_face`. Also the base's `ldso.rs` `elf::Plan { … pie: false }` (merge14 did not compile the x86 metal leg: E0063 missing `pie`). |
| M2 | 853c4d36 | `video/dpi.rs`; `text::Face::Grid`, `grid_cell` / `arm_grid`; fbcon lines 168 / 1780-1781 / 2306-2307 / 2689 (in place, line-neutral); `[kfont] load … ppi= scale= cell= grid= panel= … console=`; GLASSEYES golden README pin 1. |
| M3 | 7f8ecee6 | arroyo `ensure_kfont_noto` (before the x86 media and VM-image builder runs); builder test `noto_faces_stage_ten_of_ten`; font_core tests `kernelfont2_*`. |
| M4 | 2d3a6077 | `text::epoch`; `quarry::live::font_repaint_pass` + `font_repaint()` in settings / activity / fileview / textedit / login / instgui; `fbcon::font_repaint`; the Settings Display tab's Font picker (controls 9, 10; `prefs::key::FONT` / `FONT_SIZE`). |

### Numbers

| what | value |
|---|---|
| LUMEN-X86.ELF through arroyo's own `build_user_lumen_x86` (extracted verbatim, run) | before (cut 595732ed): **326,152 B**, `model=elf span=3395928 stack=262144 need=3662168`, ELFENTRY `insns=68431 rip_refs=1195 -> PASS` · after M1: **647,272 B (+321,120)**, `span=3717048 need=3983288 cap=67108864`, ELFENTRY `insns=106259 rip_refs=1406 -> PASS` (font_core's code + its UCD / shaping tables) |
| Lumen heap | window 67,108,864 − need 3,983,288 = **63,125,576 B** left for SYS_SBRK; the faces take 1,811,780 B (DejaVu Sans 759,720 + Sans Bold 708,920 + Sans Mono 343,140, read once, never freed) + a 512 KiB glyph cache — ~2.3 MB, 3.7 % of what is left |
| Lumen text (host, same font_core calls) | 15-px line box: DejaVu Sans **12.75 px**, baseline 12; Sans Mono 12.75 px, advance 7.68; **42 characters** of running text in the 280-px text column (the font8x8 grid gave 35); `kernelfont2/lumen-chat-3x.png` |
| the scale | `round(ppi x 2 / 96) / 2`: 221 ppi (15-inch, 2880x1800 native) and 227 ppi (13-inch, 2560x1600) -> **2.5**; 120..167 -> 1.5; no EDID -> 1.0 |
| console grid | cell 7x16 -> **18x40** at 2.5. **2880x1800 (the bench rMBP, 221 ppi): 160x45**; 2560x1600 @227: 142x40; **1440x900 @227 (as the row asks): 80x22**; a 13-inch run in a 1440x900 GOP mode reads its effective 128 ppi -> 1.5 -> 11x24 cells, 130x37; QEMU (no EDID) 7x16, 182x50 (unchanged) |
| console face | DejaVu Sans Mono at `font_size` x ppi / 96 capped by the 18x40 cell (fit **29.75 px**): font_size 9 / 11 / 12 -> 20.72 / 25.32 / 27.62 px @221 (21.28 / 26.01 / 28.38 @227) — takes effect; 13 wants 29.93 / 30.74 and draws 29.75; above 13 the cell caps it |
| builder host | `apt-get install -y fonts-noto-core` ran on this host (root): installed 20201225-2; the fallback (apt refused, simulated) fetched `fonts-noto-core_20201225-2_all.deb` (SHA-512 dd8187da…, 13,291,094 B), extracted with dpkg-deb (the package is zstd), each face SHA-256 checked: Arabic 504d7407… 244,072 B · Hebrew 436900d5… 26,892 B · Devanagari 79a47036… 229,336 B · Thai dfbd5ed0… 37,744 B |
| staging | builder `noto_faces_stage_ten_of_ten`: `KERNELFONT2 builder: faces=10/10 noto=4/4` (`FONTS: staged 10 face(s)`, `LICENSES/noto.txt` from the package copyright) |
| scripts | font_core `kernelfont2_noto_script_faces_draw`: Thai 6 glyphs from noto-thai, Devanagari 5 from noto-devanagari, Arabic / Hebrew from DejaVu Sans first (the stack's order), no .notdef |

### Gates

| leg | command | exit |
|---|---|---|
| charter | `bash /home/user/UnaOS/unaos/scripts/charter-check.sh <worktree>/unaos` | 0 |
| x86 metal shape | `cargo +nightly check --release --target ../../x86_64-unaos.json -Z build-std=core,compiler_builtins,alloc -Z build-std-features=compiler-builtins-mem -Z json-target-spec --features "wc,quarry,ftdirx,login,loginst,nvidia-kepler-vblank,smc,usbnet,hda,hda-tone,facet,beam,sdw,sdwrite,sdhcblk,selfhost,linuxabi,ahci,unafs,busreg,lumen,netring3,prefs_reset,census,installdemo,instgui,witness,selfdiag,ahciroot,btc"` | 0 (101 on the cut before the `pie: false` fix) |
| aarch64 desktop | `… --target ../../aarch64-unaos.json … --features "login,loginst,virt_el0,desktop_firmware,quarry,facet,lumen"` (user_blob head `28 00 80 d2`) | 0 |
| aarch64 no desktop | `… --features "login,loginst,virt_el0"` | 0 |
| LUMEN x86 | arroyo `build_user_lumen_x86` verbatim (`--features cryptocore`, window check, ELFENTRY) | 0 |
| LUMEN aarch64 | `cargo +nightly check --release` with `user-lumen.ld` (the crate's default aarch64 target) | 0 |
| builder | `cargo +nightly check --release` (unaos/builder); `cargo test -p unaos-builder kernelfont` (2 tests) | 0 / 0 |
| prefs | `python3 tools/prefs-schema-check.py` -> `declared=31 referenced=31 undeclared=0 -> PASS` | 0 |
| font_core | `cargo test --release -p font_core` (14 suites; `kernelfont` 11 tests) | 0 |
| vein_core | `cargo test -p vein_core` (33 tests incl. `md::tests::wrap_px_rows`) | 0 |

### The wire a metal boot should print

Once the volume is up (the desktop service pass), on the 15-inch bench rMBP (2880x1800, EDID 221 ppi):
`[kfont] load dir=/system/fonts faces=10/10 fallback=none scripts_missing=none font_kib=3341 cache_kib=2048 ppi=221 scale=2.5 cell=18x40 grid=160x45 panel=2880x1800 font=sans font_size=13 body=dejavu-mono-11.50 chrome=dejavu-sans-14.00 ui=dejavu-sans-13.50 console=dejavu-mono-29.75 ms=<n>`
(a 13-inch 227-ppi panel: `ppi=227 … grid=142x40`), then on the same pass
`[kfont] repaint epoch=1 console_rows=<n> windows=quarry,settings,activity,fileview,textedit,login,instgui face=dejavu-sans-13.50`.
`tests font`: `[kfont] fixture … ui=dejavu-sans-13.50 console=dejavu-mono-29.75` then
`:: KERNELFONT: faces=10 cache_kib=2048 evictions=0 fallback=none glyphs_drawn=<n> ms_per_1000=<n> -> PASS ::`.
`tests lumen`: `:: LUMENUX: md=ok history=ok clip=ok scroll=131072 -> PASS font=dejavu-sans ::`.
`lumen`: `:: LUMEN: start provider=<p> model=<m> key=<k> transport=<t> trust=<n> clock=<c> history=<N> files=path font=dejavu-sans font_kib=1769 ::`
(no file surface on the image: `font=font8x8 font_why=no-file-surface font_code=-38`).
Settings › Display › Font `serif`: `[settings] font=serif applied=1`, within a second
`[kfont] restyle font=serif font_size=13 ppi=221 chrome=dejavu-serif-… ui=dejavu-serif-… console=dejavu-mono-29.75` and
`[kfont] repaint epoch=2 …`; Font size `+` -> `[settings] font_size=14 applied=1` (the console stays 29.75, capped; the UI
face stays capped at 13.50 by the 16-px window cell — see Owed).

## Owed

1. **The furniture outside the console** — the bar / title strip (`theme::TITLE_HEIGHT` 34 px, MENUBAR-OCC `bar=2880x34`),
   the chrome face (14 px in its 9x20 cell), the control discs, `GAP`, dock and crystal menus — stays device px. It is
   `const` geometry with `const` assertions across `wm.rs` (line-neutral) and the fixtures (`CLUSTER_MIN_SRC_W` and its
   three asserting fixtures); scaling it is converting those to one runtime metrics table (`ui::Metrics` derived from
   `video::dpi`, not from `height / 900`) the wm reads. Seat decision: that arc, and whether `ui::SCALE_STEP`'s integer
   height rule retires in favour of `dpi`.
2. **Windows magnified by the compositor.** Settings, Activity, login, Quarry, Lumen lay out on the 7x16 cell and are
   placed at integer window scales (2x / 3x / 6x on the flight-21 wire), so their text is sized by that factor
   (nearest-neighbour), and `Face::Ui` keeps its 13.5-px cap inside them. The DPI-correct shape is those windows drawn at
   scale 1 on `Face::Grid`-style runtime cells — the same arc as (1).
3. The default 13 CSS px wants 29.93 / 30.74 device px on the console and gets 29.75 (the 18-px advance of a 7 x 2.5
   cell); sizing the grid cell from `font_size` instead (a regrid at restyle) is possible but moves the console window.
4. Facet's captions repaint on their next own repaint (not in the M4 pass). The Lumen window keeps ASCII (its
   transcript is ASCII by construction); UTF-8 through the same engine is a follow-on. Italics have no staged face.
5. The metal boot (R78).
