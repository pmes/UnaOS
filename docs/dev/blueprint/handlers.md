# BP-handlers — the CODEX manifest and the host userspace (read at 1719fe54)

## 1. What it is
- `docs/CODEX.md` §2 is the handler manifest: LOCKED, 21 handlers (Amendments I Helm, II Aether as a JS browser). Each owns one domain.
- Each handler is a host crate under `handlers/` (root workspace). `handlers/sonde/` is a 22nd directory; its CODEX row (Amendment IV, R112) awaits Peter's signature.
- On UnaOS a handler appears as (a) a ring-3 ELF, (b) a kernel fulfiller of its bus verbs, or (c) a kernel app file whose `CHARTER:` names it (seam `shared-core` / `owed B<n>`), listed in `unaos/scripts/charter.registry` (93 lines).
- Host libraries: `libs/gneiss_pal` (platform layer), `libs/bandy` (bus), `libs/quartzite` (GUI), `libs/elessar` (workspace binder), `libs/euclase` (wgpu), `libs/lux`, `libs/resonance`, `libs/text_host`.
- Vessels compose handlers into apps: `vessels/lumen`, `vessels/una`, `vessels/pulse`, `vessels/phonolite`, `vessels/facet-view`, `vessels/aether-shell`.

## 2. Invariants
| id | statement | anchor | held by |
|---|---|---|---|
| BP-handlers-1 | The manifest is LOCKED; it changes only by an Architect's amendment written into CODEX. | `docs/CODEX.md:15-23` | review |
| BP-handlers-2 | A handler's purpose is read from CODEX §2, never from its README. | `docs/dev/LAWS.md:33-34` | review |
| BP-handlers-3 | Kernel code in a handler's domain names the owner and seam on its first line; a file building beside a live host handler without sharing its core is `owed B<n>`. | `docs/dev/EXECUTOR-BRIEF.md:44-52`; `unaos/scripts/charter.registry` | `gates` row `charter`; `arch` row `seamcite` |
| BP-handlers-4 | Host handler and on-UnaOS face share ONE `no_std` core (the `midden_core` shape): Midden `midden_core`, Tabula `edit_core`/`syntax_core`, Matrix `matrix_core`/`trash_core`, Mica `jobs_core`, Stria `demux/vp8/vp9/av1/h264/audio_core`, Aether `html/css/js/layout/http_core`, Holocron `holocron_core`, Principia `prefs_core`, Sonde `probe_core`, Geode `geode_core` (GEODEMOD SR157). | `Cargo.toml:9-60`; `unaos/crates/kernel/src/aether.rs:1`; `unaos/crates/kernel/src/striaapp.rs:1`; `unaos/crates/kernel/src/middenapp.rs:1`; `unaos/crates/kernel/src/unaapp.rs:1`; `unaos/crates/kernel/src/fs/jobs.rs:4` (kernel paths `unaos/crates/kernel/src/`) | `arch` row `sharedcore` |
| BP-handlers-5 | Principia owns every setting; a preference lives in Principia's store (BP-storage-11). | `unaos/crates/kernel/src/prefs.rs:3-12`; `docs/dev/LAWS.md:35-38` | `gates` rows `prefs`, `charter` |
| BP-handlers-6 | Aether delegates A/V playback to Stria. | `docs/CODEX.md:22` `docs/CODEX.md:27` | unheld |
| BP-handlers-7 | Helm decides pass/ask/refuse from Principia's safety levels before any machine moves; the human override outranks everything. | `docs/CODEX.md:33` `docs/CODEX.md:73-79` | unheld (no kernel caller) |
| BP-handlers-8 | Vein runs when a caller needs it; no always-on system service (R82). | R82 | unheld |
| BP-handlers-9 | The host bus (`bandy`, in-process `SMessage` broadcast) is not the on-UnaOS wire; it migrates to the v1 format in a host arc. | `unaos/crates/kernel/src/bus.rs:12-18` | unheld |
| BP-handlers-10 | Help has no CODEX handler (the manifest is LOCKED and names none): the in-OS manual is kernel furniture (`CHARTER: Kernel — wm`, HELPSEARCH B729), its corpus is a tree directory packed at build time, never a handler's store or a server; the shell verbs' docs stay ONE table, `help::DOCS`, read by `help`, `man`, the Launcher and the Help window. | `docs/help/help.md`; `unaos/crates/kernel/src/help/index.rs:1`; `unaos/crates/kernel/src/help.rs:30` | `gates` row `charter`; `tests helpdoc`, `tests helpsearch` |
| BP-handlers-11 | Aether's chrome type follows Principia's display font size (TEXTSCALE2 rmbp-ledger B814, the seat's MEASURE line): AETHER.ELF subscribes `system.display.` and `system.accessibility.` beside `system.appearance.` and `aether.tabs.` on its ONE subscribe (`theme::due`, 4 = `PREF_SUB_MAX`), and its ONE re-read hook (`chrome::prefs`) restyles the tab titles, the omnibox field and caret, the menu, the badges, the status line and the find field: 11 px x font_size / 13. AETHERSCALE (LEDGER SR190): the header geometry GROWS with the face's line (`round(face x 1.25)`): the tab strip, the toolbar, a control, a menu row and the status line are their base heights (26 / 30 / 22 / 22 / 14) plus the line's excess over the default's 14 (the kernel's `TextMetrics::grow` mirrored), so `FONT_MAX` is gone and a moved face relays the page (`refit`); the metrics are one std-free model module the host test reads (`#[path]`-linked, R104); AETHERBAR (LEDGER SR192): the open menu keeps its grown row and its face and SCROLLS when the window holds fewer than its rows (`Metrics::menu`'s `vis`; the wheel over it and Up / Down move it; the wheel over the open menu never scrolls the side strip), its width the widest listed row at the face (at least 176, at most the window less its edges: `scale::menu_w`); the toolbar's optional controls follow ONE rule of `ctl` (`Metrics::bar` over `bar_need`: the omnibox keeps 24 px; SR143's 220 / 360 floors), read by `geo` for paint and hit, Reload leaving for the menu's last row below its width; the side column's row is `sidetabs::row_h(line) = grow(32, line)`, read by the layout, the hit test, the scroll and the paint. The one cap left: a window that cannot hold one grown menu row holds the row and its face. The header leaves a page line on the 288 surface across the schema (first breaking css 77). `[aether] restyle font_size=<px> text_size=<step> chrome_px=<n> chrome_h=<n> status_h=<n> menu_row=<n> menu_vis=<n> side_row=<n> bar=<reload><star><zoom>`. The page's faces follow the page's CSS, unchanged. | `handlers/aether/src/chrome/scale.rs` (the rule, `Metrics`, `Metrics::menu`, `Metrics::bar`, `bar_need`, `menu_w`); `handlers/aether/src/chrome/sidetabs.rs` (`row_h`, `SideStrip::row_h`); `unaos/crates/user-aether/src/theme.rs` (`due`'s `SUBSCRIBE`, `read_type` at the tail); `unaos/crates/user-aether/src/chrome.rs` (`prefs`; `tab_h`/`bar_h`/`chrome_h`/`ctl`; `geo`; `paint_menu`; the tail: `metrics`, `menu_m`, `menu_n`, `menu_need`, `menu_toggle`, `menu_scroll`, `font`, `font_small`, `restyle`); `unaos/crates/user-aether/src/paint.rs` (`status_h`, `status`) | `aether::engine_tests::tests::aetherscale_header_reflows_kat`, `aetherbar_fit_kat` (host); `tests textscale2` (`aether=ok`, under `UNAOS_AETHER`), unflown |
| BP-handlers-12 | Aether's top layer (AETHERFIT LEDGER SR189): popovers and modal dialogs are centred in the viewport by the UA sheet's `inset: 0; margin: auto; width / height: fit-content`. layout_core makes `fit-content` taffy's own sizing keyword (no second shrink-to-fit). Its fixed-box pass gives a fixed box the viewport as its containing block and records the viewport origin in ONE map, `LayoutTree::fixed_at`, which render, the hit test and `scroll::box_origin` read. The hit test tests the top layer first, in reverse order, skipping inert boxes, through render's `top_layer_box`. Every user interaction event aimed at an inert node goes to the topmost modal dialog through the ONE rule `toplayer::retarget`. | `unaos/libs/web/layout_core/src/css.rs` (`parse_dimension_str`); `unaos/libs/web/layout_core/src/tree.rs` (`fixed_at`, `fixed_viewport`, `record_fixed`); `handlers/aether/src/css/mod.rs` (`UA_SHEET`); `handlers/aether/src/render/mod.rs` (`top_layer_box`, `placed_origin`); `handlers/aether/src/lib.rs` (`hit_test_at`, `hit_origin`); `handlers/aether/src/js/toplayer.rs` (`retarget`) | `cargo test -p layout_core --test fit_content`; aether lib `test_hit_test_hoists_the_top_layer`; `wpt_apis` dialog / popovers counts (docs/dev/evidence/aetherwpt/WPT.md AETHERFIT), unflown |

## 3. The manifest (CODEX §2) and where each one is
| handler | domain | `handlers/` dir | on UnaOS at HEAD |
|---|---|---|---|
| Aether | Web | `aether` | `APPS/AETHER.ELF` (both arches) |
| Amber Bytes | Disks | `amber_bytes` | `amber_core` (GPT) in kernel; `unaos/crates/kernel/src/video/instgui.rs` charter |
| Aulë | Forge | `aule` | selfhost (`unaos/crates/kernel/src/selfhost/`) — owner `unverified` |
| Comscan | Signals | `comscan` | none found |
| Facet | Images | `facet` | kernel viewer `unaos/crates/kernel/src/video/facet.rs` (charter Facet) |
| Geode | Archives | `geode` | `geode_core` (shared-core, GEODEMOD SR157) in `unaos/crates/kernel/src/fs/geodew.rs` (GEODEBACKUP B740: Settings > Storage backs the home up and restores it); host crate `handlers/geode`: the `.geode` v1 archive carrying UnaFS records whole, GEODEMETA SR148 |
| Facet | Images | `facet` | kernel viewer `unaos/crates/kernel/src/video/facet.rs` (charter Facet); camera raw through `unaos/libs/media/raw_core` (shared-core) in `unaos/crates/kernel/src/video/facet_raw.rs`: the embedded preview FAST, the half-size white-balanced develop when a file has none (B711) through the colour matrix (DNG ColorMatrix2, else dcraw's Sony rows; B724); Canon CR2 (lossless JPEG, slices) and Nikon NEF (Huffman or uncompressed, the maker note's curve) decoded into the same mosaic (B724) |
| Geode | Archives | `geode` | none found |
| Helm | Control | `helm` | `unaos/libs/sys/helm` (host workspace only) |
| Holocron | Secrets | `holocron` | `APPS/HOLOCRON.ELF`; login screen charter (`unaos/crates/kernel/src/video/login.rs`) |
| Matrix | Files | `matrix` | Quarry (`video/quarry*.rs`, charter Matrix) |
| Mica | Data | `mica` | `jobs_core` (`unaos/crates/kernel/src/fs/jobs.rs`) |
| Midden | Shell | `midden` | `APPS/MIDDEN.ELF`; kernel shell + `midden_core` |
| Obsidian | Binary | `obsidian` | none found |
| Principia | System | `principia` | kernel prefs fulfiller + Settings (`unaos/crates/kernel/src/video/settings.rs`) |
| Junct | Colab | `junct` | none found |
| Stria | A/V | `stria` | `APPS/STRIA.ELF`; `unaos/crates/kernel/src/video/player.rs` owed B289 |
| Tabula | Text | `tabula` | `edit_core` in `UNA.ELF`; `video/{fileview,textedit}.rs` |
| Vairë | Repos | `vaire` | `git_core` (host-linked; kernel use `unverified`) |
| Vein | AI | `vein` | `vein_ring3` via `LUMEN.ELF`/`DIAG.ELF` |
| Vug | 3D | `vug` | `APPS/VUG*.ELF`; `temple_core` |
| Xenolith | VMs | `xenolith` | none found (compat box is `unaos/crates/kernel/src/arch/x86_64/linuxabi/`) |
| Zircon | Time | `zircon` | none found |
| (Sonde) | Probes | `sonde` | `unaos/crates/kernel/src/sonde.rs` (driver; R112, Amendment IV pending) |

## 4. Seams
- Host handlers may call `libs/` and the `unaos/libs/` cores; never the kernel crate.
- On UnaOS a handler speaks only through una-abi syscalls and bus verbs (ring3.md).
- A vessel binds handlers (Elessar's job, CODEX §4); it owns no domain.

## 5. Ruled, unbuilt
- CODEX Amendment IV (Sonde) signature (R112). Elessar's dynamic layout on UnaOS (CODEX §4): not on UnaOS at HEAD (`unverified`). R108 (Aether in the Una IDE, the 3D work environments): see RULINGS.
- AETHERBAR owed (SR192): (a) RAIL: a bar under `bar_need(ctl, 2, 1)` = 60 + 3 ctl (a side column on a window under ~200 px) still clamps the omnibox at 24 px over its neighbours: Back / Fwd would leave too; (b) MENUKEYS: the scrolled menu has no keyboard highlight or Enter and no drawn scroll cue; (c) GTK: the host GTK face's side strip (`libs/quartzite/src/platforms/gtk/tab_strip.rs`) keeps its own row height, not `sidetabs::row_h`.
- AETHERFIT owed (SR189): (a) focus refuses inert (AETHERTOP gap 3): `handlers/aether/src/js/dom.rs`'s focus path needs `toplayer::is_inert` (HTML §6.6.3: refuse, not retarget); (b) `handlers/aether/src/js/cssom.rs`: `element_boxes` gives a fixed box's client rect without taking the scroll off (it should read `fixed_at` and the scroll), and its `hit_test` (elementFromPoint / elementsFromPoint) has no top layer; (c) the spec's `dialog:modal { max-width / max-height: calc(100% - 6px - 2em) }` and the non-modal `dialog { position: absolute; inset-inline: 0; width / height: fit-content; margin: auto }`: a % size of a fixed box still resolves against its DOM parent (the fixed pass rewrites insets only); (d) `handlers/aether/src/lib.rs` MouseUp's own defaults (follow a link, focus) still use the raw inert hit; (e) `fit-content(<length-percentage>)` and the `min-content` / `max-content` keywords (taffy has them; layout_core does not parse them).
- AETHERLIB (AETHERNET2 LEDGER SR187's owed row): `unaos/crates/user-aether` is a no_std/no_main bin (its own workspace) with no host test target; its fetch sequencing (the resolver cache, the `reason=` mapping in `unaos/crates/user-aether/src/net.rs`) is untested on the host until a lib split lets a resolver stub drive `fetch`. The trust reader and cipher names are tested in `vein_ring3` (`trust::tests`, `tls::tests::cipher_name_kat`).

## 6. Read further
- `docs/CODEX.md`; `docs/dev/USERLAND/ARCHITECTURE.md`; each handler's own `README.md` for implementation (not charter).
- `docs/NAMING_ATLAS.md` (names), `docs/ROADMAP.md` (direction).
