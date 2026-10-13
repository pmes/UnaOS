# AETHERMETAL — Aether on the UnaOS image (LEDGER SR66)

Charter: **Aether** owns the Web (docs/CODEX.md §2). Seams: `shared-core` for html_core / css_core / font_core
(no_std + alloc, linked by both rings); the program `crates/user-aether` is Aether's own ring-3 program (the
user-lumen precedent, LUMENAPP B323 / R82). The kernel builds nothing of the browser: `crates/kernel/src/aether.rs`
validates, spawns and witnesses the image (lumen.rs's shape).

## M1 — what landed

| piece | where | what it does |
|---|---|---|
| program | `unaos/crates/user-aether/` (own `[workspace]` root) | `APPS/AETHER.ELF`: read `/system/doc/welcome.html` (SYS_PATH_READ, else SYS_OPEN) → `html_core::parse_document` → `style::compute` (css_core cascade over an `Element` impl on html_core's arena; UA sheet in `src/style.rs`, the page's `<style>`, `style=""`) → `layout::build` + `layout::layout_root` (CSS2 block + inline formatter written in the crate) → `paint::paint` (back buffer) → surface → `SYS_WIN_PRESENT`. Scroll by Up/Down/PageUp/PageDown/Space/Home/End/wheel; Esc or the window close exits. Faces: DejaVu Sans / Bold / Mono / Mono Bold off `/system/fonts/` via `font_core::ui` (font8x8 grid fallback). |
| formatter | `src/layout.rs` | block boxes (margin/padding/border, `width`, `max-width`, auto margins centre), anonymous block boxes for mixed content, vertical margin collapsing (siblings, parent/first-child, parent/last-child, through empty boxes; never the root's), inline runs shaped by font_core and broken at spaces, `white-space: pre`, half-leading line boxes with the block's strut, `text-align`, list markers (• ◦ ▪ n.) outside the content box. |
| page | `unaos/crates/user-aether/doc/welcome.html` | the fixture and the demo: title, h1/h2, paragraphs with em/strong/a/code, nested ul/ol, blockquote, pre, author colours/margins/padding/borders in `<style>`, a class, an id and a `style=""`. |
| build | `unaos/arroyo` `build_user_aether_x86` | Lumen's shape: ELF-window link script, syms before the strip, e_machine 3e00, `user_elf_window_check`, `una_res_stamp` (res/app.res + icon.svg). Called on the five x86 paths Lumen's build is. |
| staging | `unaos/builder/src/main.rs` | `APPS/AETHER.ELF` on the ESP and the DATA tree; `stage_welcome` writes `system/doc/welcome.html` on both. |
| knob | `UNAOS_AETHER=1` → feature `aether` (implies `selfdiag`: SYS_PATH_READ) | arroyo `_feats`, kernel Cargo.toml, builder env→feature, `scripts/banner-cert.sh` (`aether|:: AETHERAPP:|-|measured`), `scripts/k8-reach.registry` (NA: no aarch64 image yet). |
| fixture | `crates/kernel/src/aether.rs`, `tests aether` | validate (production `validate_elf`, ELF-window model, windowed app note) → spawn (`spawn_user_image_bg`, APPRES sight first) → wait for the program's `:: AETHER: page … -> PASS painted=<n> lines=<n> …` line at the console seam (line watch + the seam's verdict-tail recorder) → kill → `:: AETHERAPP: image=/apps/AETHER.ELF window=<elf|fixed|bad> painted=<n> lines=<n> -> PASS|FAIL|SKIP ::`. Registered from `tests::ensure_shellux` (tests.rs:175), so `tests-at-boot` lanes run it at boot. |

Host render of the page through the SAME style/layout/paint sources (a scratch harness over the host DejaVu files,
288 px wide): `doc_h=739 lines=34 blocks=36 elements=40 rules=121 painted=66041`.

QEMU witness: owed to the fold (see the end).

## M2 — the std surface of handlers/aether, and the ONE seam for the layout port

`cargo tree -p aether -e normal` (repo root, 2026-10-12): 22 direct dependencies, 215 distinct crates in the normal
graph. Direct: anyhow, bandy, base64, clap, crypto_core, **css_core**, directories, **font_core**, futures-util,
gneiss_pal, **html_core**, image, **js_core**, **pixel_core**, resvg, serde, serde_json, **taffy 0.14**, text_host,
thiserror, tokio, url (bold = already UnaOS cores, no_std + alloc). Full tree: 428 lines, kept out of the repo
(re-run the command; it is the evidence).

Which modules touch what (grep over `handlers/aether/src`, tests excluded):

| module | tokio | taffy | image | resvg | directories | std::fs | other std |
|---|---|---|---|---|---|---|---|
| lib.rs | x | x | | | | | env |
| main.rs | x | | | | | | |
| api/fetch.rs | x | | | | | | Mutex/OnceLock, time |
| net/mod.rs | x | | x | | | | locks, time |
| api/platform.rs | | | | | | x | time |
| images/mod.rs | | | x | x | | | |
| media/mod.rs | | | x | | | x | locks, env |
| headless.rs | | | x | | | x | |
| fonts/mod.rs | | | | | x | | Mutex/OnceLock |
| fonts/webfont.rs | | | | | | x | locks |
| storage/mod.rs | | | | | x | x | |
| js/host.rs, js/loader.rs | | | | | | x | locks/time, env |
| ledger/mod.rs | | | | | | x | |
| event_loop, workers, js/window | | | | | | | time, threads/locks |
| **layout/mod.rs, layout/inline.rs, layout/table.rs** | | **x** | | | | | OnceLock (one interned family id), HashMap |
| **css/mod.rs** | | **x** | | | | | thread_local HashMap caches, Instant timing, env profiling knobs |
| render/mod.rs | | x (reads boxes) | | | | | env |

Two findings that change M2's question:

1. **taffy is not std-bound.** taffy 0.14's manifest declares `std` a DEFAULT feature, and its `lib.rs` is
   `#![cfg_attr(not(feature = "std"), no_std)]` with its own float helpers under `alloc` (`src/util/sys.rs`) — no
   libm. `default-features = false, features = ["alloc", "taffy_tree", "block_layout", "flexbox", "grid", "calc",
   "content_size"]` builds without std (slotmap and arrayvec are no_std). The std in the layout path is AETHER'S, not
   taffy's: `std::collections::HashMap` (layout, inline, css), a `OnceLock` (layout/mod.rs:933), thread-local
   `RefCell` caches (layout/table.rs:79, css/mod.rs:1072/1219/1515), `Instant` timing and `env::var_os` profiling
   knobs (css/mod.rs), the `Rc<RefCell<Document>>` `NodeRef` (dom/mod.rs:29), and `crate::fonts` (33 call sites
   from layout: `face`, `line_height`, `line_metrics`, `baseline_offset`, `lines::break_lines`) whose face database
   is `text_host` + `directories` + `Mutex/OnceLock`.
2. **The ring-3 program already has every input the host layout consumes**: the same html_core arena, the same
   css_core cascade, the same font_core shaper. What it lacks is the host's box-generation + taffy style mapping +
   inline formatter.

**THE ONE SEAM — `layout_core`, a new no_std + alloc shared core (the midden_core shape) cut from
`handlers/aether/src/layout/` and the taffy-style half of `css/mod.rs`, over two traits:**

```rust
pub trait Dom   { /* html_core::Document + NodeId: children, element(), text — no Rc/RefCell */ }
pub trait Fonts { fn measure(&mut self, run: &str, sel: FontSel, size: f32) -> f32;
                  fn line_metrics(&self, sel: FontSel, size: f32) -> (f32, f32, f32);
                  fn break_lines(&mut self, text: &str, mode: &TextMode, max_w: f32, start_x: f32) -> Vec<Line>; }
pub fn layout(dom: &impl Dom, styles: &ComputedStyles, fonts: &mut impl Fonts, viewport: (f32, f32)) -> LayoutTree;
```

`LayoutTree` keeps its shape (`taffy::TaffyTree` with `default-features = false`, `node_map`, `paint_map`,
`inline`) with `HashMap` → `alloc::collections::BTreeMap`, the `OnceLock` and thread-locals → fields of the tree or of
the `Fonts` impl, `Instant`/`env` → caller-supplied hooks. The host implements `Fonts` over its `fonts/` database
(text_host stays host-side); AETHER.ELF implements it over `font_core::ui::Engine` with the volume's faces (this
crate's `txt.rs`). `render/` reads `LayoutTree` unchanged on the host; the ring-3 painter (this crate's
`paint.rs`) is rewritten against it. M1's `src/layout.rs` is then deleted, and the two rings lay out with ONE engine.
Everything else std in the table above (tokio/net, image/resvg, directories/storage, std::fs, js host) stays
behind the host's own seams and is out of M2's scope: M2 is "the host's layout runs on the image".

## M3 — interaction on the image (LEDGER SR96)

R108's pane contract on APPS/AETHER.ELF: a surface in, an INPUT_EV stream out, a resize. Nothing widget-shaped.

| piece | where | what it does |
|---|---|---|
| hit test | `unaos/libs/web/layout_core/src/hit.rs` (new, `shared-core`) | handlers/aether/src/selection.rs's hit test moved verbatim to `no_std` + `alloc`: `Point`, `align`, `utf16_to_byte`/`byte_to_utf16`, `words`/`word_at` (UAX #29), `line_of`, `collapses_at`, `hit_text` (caret x a caller closure — the host's `Advancer`, the image's `font_core::ui`), `pick` + `hit` over `TextBox`es, and `text_boxes` (the image's paint walk, document coordinates). The DOM identity is a caller closure (`id_of`). |
| host | `handlers/aether/src/selection.rs` | re-exports the moved items under their old paths; `hit()` maps `render::TextBox` (with its recorded scroll and `Advancer`) onto `layout_core::hit::hit`. Selection state, document order, `plan_ifc` and the innerText collector stay host-side. |
| model | `unaos/crates/user-aether/src/ui.rs` (new) | `Flat` — the painted text in paint order (`\n` between line boxes) with each box's offset; selection = two offsets, find = ranges, copy = a slice; `marks` back to per-fragment byte ranges. Per-box link (`a[href]` ancestor), the document's link list, `clip_text` (printable ASCII for `SYS_CLIP_SET`). |
| dispatch | `src/main.rs` | MOUSE_REL pointer estimate (x86 has no window origin for MOUSE_ABS — UNAMETAL's note) drawn in-window as an arrow / hand; hover → status line + `hover` wire; click → `html_core::url` resolve against `file://<path>` → read → parse → cascade → layout (old arena freed); history list (Backspace/Left/button 4 back, Right/button 5 forward); left drag selects (edge-scrolls), middle drag scrolls, wheel; Ctrl+C / Cmd+C → `SYS_CLIP_SET`; Tab/Enter link focus; Ctrl+F or `/` find bar. |
| paint | `src/paint.rs` | `Mark`s (selection blue `#3367D1` with white glyphs, find yellow, current find orange) under the fragment's glyphs; `status`, `outline`, `pointer` over the page. |
| pages | `unaos/crates/user-aether/doc/welcome.html`, `doc/keys.html` (new) | welcome.html's first link is `keys.html`; keys.html is the operator's card and links back. |
| fixture | `unaos/crates/kernel/src/aether.rs` | after the page report, drives the ring by owner (`user_input_push_owner`): Tab → `focus at=x0,y,x1`; MOUSE_REL to the corner then onto the box → `hover`; press, MOUSE_REL across, release → `select chars=`; Ctrl+C → `clip set=` (reported, not judged); press+release → `nav`. |

Wire (each a verdict line so the seam's tail recorder reads it):

```
:: AETHER: focus -> PASS href=keys.html at=<x0>,<y>,<x1> ::
:: AETHER: hover -> PASS href=keys.html ::
:: AETHER: select -> PASS chars=<n> ::
:: AETHER: clip -> PASS set=<rc> chars=<n> ::            (FAIL reason=refused set=-13 when not focused)
:: AETHER: nav -> PASS how=click path=/system/doc/keys.html lines=<n> hist=2/2 ::
[aetherapp] drive focus=Some("PASS …") hover=Some("PASS …") select=Some("PASS chars=<n>") clip=Some(…) nav=Some("PASS …")
:: AETHERAPP: image=/apps/AETHER.ELF window=elf painted=<n> lines=<n> hover=1 nav=1 select=<n> -> PASS ::
```

`select` is the link text's characters between the nearest boundaries to x0+2 and x1-2 ("the keys page" = 13, or one
fewer at a glyph edge).

OWED (not made — outside this arc's files or a new verb):

1. **Staging `keys.html`.** `unaos/builder/src/main.rs` `stage_welcome` copies only `doc/welcome.html` (and the DATA
   list names only `system/doc/welcome.html`). Until it copies every `crates/user-aether/doc/*.html` (and pushes
   `system/doc/keys.html` on `staged_data`), the click reads `nav -> FAIL reason=no-page code=<errno>` and the
   witness is `nav=0 -> FAIL reason=no-nav`.
2. **The system cursor.** No SYS_ verb sets the compositor's cursor shape; the hand is drawn inside the window at
   the program's pointer estimate. A cursor verb (and a window origin for MOUSE_ABS) is a kernel arc.
3. The clipboard is `SYS_CLIP_SET` (64), the verb LUMEN.ELF copies with — it exists; nothing owed.

## QEMU witness — owed to the fold

Not taken by the executor. The first `UNAOS_AETHER=1 UNAOS_WC=1 ./arroyo test` reached `build_selfbuild5_x86`, which
fetched `llvm-project-22.1.8.src.tar.xz` (167 MB) to build LLD.LNX — the brief's stop condition; the run was stopped
(its pid's cwd read first: under this worktree) and the tarball deleted. The re-run with `curl` stubbed (SELFBUILD5
printed `not staged LLD.LNX reason=fetch failed …`) built AETHER-X86.ELF (`:: ELFENTRY: AETHER-X86.ELF
entry=0x10000200000 insns=124037 rip_refs=898 -> PASS ::`, `model=elf span=840416 stack=262144`, APPRES stamped)
and then died in the kernel build on `No space left on device` (the shared disk at 1.1 GB free). R107's amendment
then moved every run to the fold. The tip's `video/winmemory.rs` E0069 also reds any `wc` kernel build until
930cdd79 is merged (measured: the same error with and without `aether`).

The wire the fold should read (x86, `tests-at-boot` lane):

```
:: AETHER: start src=path bytes=<n> font=dejavu-sans font_kib=<n> faces=4 ::
:: AETHER: page /system/doc/welcome.html -> PASS painted=<n> lines=34 blocks=36 elements=40 rules=121 doc_h=739 ms=<n> start_ms=<n> heap_kib=<n> ::
[aetherapp] bytes=<n> entry=0x10000200000 segs=<n> stack=262144 note_flags=Some(1)
[aetherapp] pid=<n> slot=<n> entry=0x… sight_ms=<n> spawn_ms=<n> report_ms=<n> wait_ms=20000 kill=<…> tail=Some("PASS painted=<n> lines=34 …")
:: AETHERAPP: image=/apps/AETHER.ELF window=elf painted=<n> lines=34 -> PASS ::
```

(`lines`/`blocks`/`elements`/`rules`/`doc_h` are the host harness's numbers for the same sources at 288 px; `painted`
on the 288x288 window is the first screen only, so smaller than the harness's whole-page 66041.)
