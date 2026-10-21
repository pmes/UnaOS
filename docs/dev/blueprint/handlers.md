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
- AETHERLIB (AETHERNET2 LEDGER SR187's owed row): `unaos/crates/user-aether` is a no_std/no_main bin (its own workspace) with no host test target; its fetch sequencing (the resolver cache, the `reason=` mapping in `unaos/crates/user-aether/src/net.rs`) is untested on the host until a lib split lets a resolver stub drive `fetch`. The trust reader and cipher names are tested in `vein_ring3` (`trust::tests`, `tls::tests::cipher_name_kat`).

## 6. Read further
- `docs/CODEX.md`; `docs/dev/USERLAND/ARCHITECTURE.md`; each handler's own `README.md` for implementation (not charter).
- `docs/NAMING_ATLAS.md` (names), `docs/ROADMAP.md` (direction).
