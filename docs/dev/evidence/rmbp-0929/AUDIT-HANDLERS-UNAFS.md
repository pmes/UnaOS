# Audit — the kernel desktop versus the handlers it side-stepped, and UnaFS versus BeFS

Peter, 2026-10-01, on being told the kernel Settings window keeps its own flat file: "nope. this must
be fixed. what other train wrecks are there? what other handlers have been side-stepped? what about
file attributes? and how did file id and association work? we have UnaFS as a modern BeFS."

Two read-only sweeps (kernel at `exec-rmbp-merge8`, which carries every wave-3/4/5 arc; the unafs
crate is byte-identical between `main` and merge8). Rules the findings are measured against:
`docs/dev/LAWS.md` §Handler manifest ("Principia = System policy (every settings and preference
decision routes there)"), LAWS line 33 ("Handler charters come from `docs/CODEX.md`'s manifest"),
`docs/ROADMAP.md` §3b ("port the bus, not the binary convention"; the kernel verb table "is the
proto-form; it factors into midden's verb set, fulfilled by the capability owner"), ROADMAP §2
(UnaFS F-arcs) and §1c SH-2 (UnaFS as the system volume).

## 1. Handlers side-stepped by kernel code

| Kernel feature (file) | Owner per CODEX §2 | Owner's state | Verdict |
| :--- | :--- | :--- | :--- |
| Settings window `video/settings.rs` (SETTINGS, SETTINGS2): flat `key=value` in `<home>/.settings`, no namespaces | **Principia** (System). Live: `handlers/principia/src/prefs.rs`, TOML per namespace at `~/.config/unaos/preferences.toml`, atomic temp+fsync+rename, bus `PrincipiaCommand::{PrefGet,PrefSet,PrefChanged,PrefList,...}` | live | **Side-step.** A second settings store with a different format, path and schema. LAWS says every preference decision routes to Principia. |
| Dock pins `<home>/.dock`, wallpaper verb, idle-blank, pointer speed, volume | Principia (preferences) | live | Side-step: each a private dotfile or RAM-only; all are preferences. |
| Trash `fs/trash.rs` (`<home>/.Trash/.index`, TAB lines of paths) and **Quarry itself** (`video/quarry/live.rs` 4377 lines + `ops.rs`: rename, copy, delete, new folder) | **Matrix** (Files, "Finder") — live Finder verbs `open/new_folder/rename/copy/mv/delete` over `BrowseListing`/`FsOutcome` | live | Side-step, but **ruled**: R50 "quarry is our finder" put the Finder in the kernel. Matrix has no trash. Two file managers with two verb sets. |
| Text editor `video/textedit.rs` + `fileview.rs` (`edit`/`view`) | **Tabula** (Text) | partial, view-only, not on the bus | Side-step. Tabula's portable `TabulaDocument` is std-only. |
| Image viewer `video/facet.rs` | **Facet** (Images), `vessels/facet` live on macOS | live, std/WGPU | Side-step, **documented** in facet.rs:12-31 (DEFLATE window vs the 16 KiB ring-3 window). Takes the vessel's name. |
| `play <wav>` `drivers/hda_play.rs`, `tests hda`, volume/mute | **Stria** (A/V) | live, host-only (cpal); no media `SMessage` | Side-step. ROADMAP §3a calls the A/V lane "zero kernel scope", then the kernel grew a player. |
| Activity monitor `video/activity.rs`, `pulsewin.rs` | `vessels/pulse` (system monitor); ring-3 `user-pulse` already exists on x86 | live | Duplicate: three monitors (kernel window, kernel pulsewin, ring-3 PULSE). |
| Users/login `fs/users.rs` (`USERS.DAT`, PBKDF2), kernel `fs/holocron.rs` (BT-BOND record store) | **Holocron** (Secrets/identity) | design-only, no crate | Kernel is the only implementation. Not a side-step of working code; a charter the kernel now carries alone. |
| Self-install `install/selfinstall.rs`, `fdisk`, `dd`, GPT/ESP writers | **Amber Bytes** (Disks: GPT, formatting) | partial, forensic CLI, "GPT not yet implemented" | Kernel is the only implementation; R28/R31 require the installer to be a handler exported as a vessel. Gap, not a duplicate. |
| `src extract` (`selfhost/extract.rs`: gunzip, tar, sha256) | **Geode** (Archives) / **Aulë** (Forge) | design-only / scaffold | Kernel is the only implementation. |
| `help`/`man`, `termcolor`, `shellux`, the verb table | **Midden** (Shell) | verb table already shared: `unaos/libs/sys/midden_core` is `no_std` and the kernel links it | **Converged.** The one domain done the §3b way. |
| Linux ABI `arch/x86_64/linuxabi/` | `docs/dev/OS/03_COMPATIBILITY_BOX` (design docs); Xenolith owns full VMs only | design-only | Kernel is the only implementation; in scope by SH-5. |
| Clipboard, shortcuts overlay, window list, snap, power UI | no handler | — | Kernel wm territory; fine. Power policy and battery thresholds are Principia preferences. |
| `hexdump`, `snap` (UnaFS snapshots), `fetch`, `rast_demo` | Obsidian / Geode+Vairë / Aether / Vug | design-only / live / live / prototype | Thin overlaps; verbs, not apps. |

Root cause, stated once: the only transport between ring 3 and the kernel is the BANDY v1 wire
(`SYS_MSEND`/`SYS_MRECV`, `crate::bus`, both arches since BUSX86) and it carries six verbs
(ls/cat/cp/write/rm/mv) plus NOTICE/MENU. No handler builds for the kernel target (every one is
std/Tokio/GTK/cpal). Fulfiller registration is deferred to "BANDY-3's design pass" (ROADMAP:298).
So every desktop feature of the last three waves was built inside the kernel because there was
nowhere else on the metal to build it. R75 ("whatever makes a really good OS") was read as a go
for the feature; it was not a go to fork the settings store, and LAWS already said where settings go.

## 2. UnaFS versus BeFS

What BeFS had: typed attributes on every file; attribute indexes with live queries; `BEOS:TYPE`
(a MIME string) on every file; a MIME database mapping type → preferred application, overridable per
file (`BEOS:PREFERRED_APP`); content sniffing rules; application signatures; Tracker opening by type,
not by extension; applications keeping their data in attributes (People, Mail, Tracker columns).

| BeFS | UnaFS crate (`unaos/libs/fs/unafs`) | Kernel surface (rMBP) |
| :--- | :--- | :--- |
| Typed attributes | **Exceeds**: `Int/Float/String/Blob/Vector` (`inode.rs:107`), inline under 256 B, spilled above; cosine-similarity on `Vector` | **Remove only.** `setfattr -x key path` (`shell.rs:5430`). The VFS trait (`fs/vfs.rs:159`) has `remove_attr` and no set, get or list. No syscall. Only `tools/unafs` on the host can set or read one. |
| Attribute indexes, range queries | Flat FNV hash catalog rewritten whole on every `set_attribute` (O(n), README:442). Ops `Eq Neq Gt Lt SimilarityGt`; no `>=`, no two-sided range, no OR. F3 B+tree is written (`btree.rs`) but "nothing consumes it"; F4 not started. | No `query` verb or syscall; the kernel never calls `UnaFS::query`. ROADMAP K1/K3 "query deferred: std/sqrt-gated" is **stale** — the crate is `no_std` via `libm` with bit-identical scores. |
| Live queries | F5 not started; a raw bandy `FileEvent` on std only | none |
| `BEOS:TYPE` MIME attribute | none | none. Opening is by extension: `activate_row` (`quarry/live.rs:1264`) tests `.ELF/.BIN` → launch, `.PNG` → facet, `.WAV` → play, `.TXT/.MD/.LOG/.SPEC` or **no extension** → text, else "no opener". `open_kind` (live.rs:735) also lists `.sha/.cfg/.ini` as text but `is_text_name` refuses them, so those route to no-opener. live.rs:1260: "no registry, no association table". |
| MIME database, preferred app, per-file override, sniffing, app signatures | none anywhere in `unaos/`, `handlers/`, `libs/`, `vessels/` (only decode-side sniffers in lux/aether and extension→language in tabula) | none; dock pins are a fixed six-name table |
| File identity | inode ids are stable logical numbers since K8a (CoW inode map) | **paths everywhere.** `vfs.rs:124`: "backend identity is NOT exposed; re-resolution from the name is the contract". `Stat` is `{kind,size}`. Trash, dock, settings store paths. x86 ACL is keyed by `U10_NAMES` static ids; aarch64 ACL rows are keyed by FAT `(dir_lba, dir_off)`. No recents. |
| Timestamps | **UnaFS has no mtime/ctime/atime field** (`inode.rs:127`). Snapshot timestamps are `cntpct` ticks. | FAT gets real last-write stamps from the RTC (merge8 RTCCLOCK); native `DirEnt.mtime` is `None`. |
| The system volume | — | **The rMBP has no UnaFS.** `fs/unafs.rs` is `cfg(target_arch = "aarch64")` (`fs/mod.rs:44`), `NativeBackend` likewise, `unafs_state()` returns "unbuilt" on x86, `bind_root` forces FAT root (`bootdisk.rs:1478`). `/home/<user>` is `HOME/<8.3>` on FAT on both arches. SH-2 is open. |

The one-line verdict: UnaFS the crate is a BeFS successor on disk; UnaFS the operating system does
not exist yet on the rMBP, and on the Pi it has attributes nobody above the VFS can set or read.
Every "modern BeFS" property (type attribute, association, queries, attribute-keyed app data) is
blocked on three things in order: UnaFS mounting on x86, an attribute surface through the VFS and the
syscall table, and a type/association layer built on attributes rather than extensions.

## 3. The fix program (proposed; ordering is Peter's)

Each is an arc; each lands with a doc, ledger row, and the metal boot as its gate (R78).

| Arc | Content | Unblocks |
| :--- | :--- | :--- |
| **UNAFSX86** | Lift the `aarch64` cfg off `fs/unafs.rs` and `NativeBackend`; `with_unafs` over the x86 block layer (AHCI/SDHC handles already exist via SDSEAM); `bind_root` mounts a UnaFS volume as `/` when the boot disk carries one; the ESP image gains a UnaFS partition built by `tools/unafs` (SH-2 rung 1). `/home` moves to UnaFS. | everything below on the rMBP |
| **ATTRSURF** | VFS `set_attr/get_attr/list_attr/query`; verbs `setfattr k=v`, `getfattr`, `lsattr`, `query`; syscalls `SYS_ATTR_SET/GET/LIST`, `SYS_QUERY`, `SYS_STAT` with inode id and mtime; bus verbs for the same; FAT backend answers `-ENOTSUP`. | apps keeping data in attributes |
| **UNAFSTIME** | `Inode` gains `ctime/mtime/atime` (format v4 with a KAT and a migrate pass); writes stamp from the kernel clock; `DirEnt.mtime` real for native. | Quarry columns, Trash, recents |
| **FILETYPE** | `una:type` attribute (MIME string) stamped on create by the writer and by a sniff pass on untyped files (magic bytes, then extension as the last resort); `BEOS:TYPE` semantics. | association |
| **ASSOC** | A type database as attributes on a system-volume `/system/types/<type>` object: preferred opener, icon, short name; per-file `una:preferred` override; Quarry's `activate_row` consults it and the extension chain becomes the fallback only; `open <path>` verb; the dock pins by application signature (`una:signature` attribute on the image), not by table. | opening by type |
| **PREFS** | The kernel Settings window reads and writes Principia's store: namespaced dotted keys, TOML scalars, `<home>/.config/unaos/preferences.toml` (on UnaFS, the same keys also stamped as attributes so `query` finds them); `.settings` and `.dock` deleted; bus `PrefGet/PrefSet/PrefChanged` fulfilled in-kernel so Principia on the host reads the same file unchanged. | the settings fork is closed |
| **TRASHATTR** | Trash index becomes attributes on the trashed object (`una:trash-origin`, `una:trash-time`) found by `query`; `.Trash/.index` deleted; restore uses inode id. Matrix gains the same verbs over the bus. | one Finder model |
| **F3/F4 wiring** | Consume `btree.rs` for the catalog; per-attribute indexes; `>= <=` and two-sided ranges; query returns `(inode_id, path)`. Host-native, no hardware. | real queries |
| **BANDY-3** | Fulfiller registration on the wire so a ring-3 program can own a verb; first fulfiller: a `no_std` Principia prefs core shared by kernel and host (the midden_core pattern). | handlers on the metal, one at a time |

Doc fixes owed now: ROADMAP K1/K3 "query deferred" line; `open_kind` vs `is_text_name` disagreement.
