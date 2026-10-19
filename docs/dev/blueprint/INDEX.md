# BLUEPRINT — the UnaOS architecture as it is in the tree

**The bar (Peter, 2026-10-19):** "not just a car — the greatest car ever made. what we are working on is the greatest OS.
they should think: wow, this is how computers should have been all along." Every sheet below serves that sentence;
an arc that does not move the glass toward it is a tangent.

Peter, 2026-10-19: "a set of blueprints for you and the executors to look over … keep the os architecture stable and work as a
front-side gate so that coding efforts are not wasted on some accidental tangent"; "easily digestible so that only what's
necessary is contextualized". Held to the tree by GATE-BLUEPRINT (`unaos/scripts/blueprint-check.py`, in `./arroyo gates`).

Code read at `1719fe54`; LAWS.md, PLAYBOOK.md, EXECUTOR-BRIEF.md and `unaos/arroyo` lines at `dee2d6ed` (that commit changes
only those docs and the gate list). Anchors are repo-relative `path:line`; GATE-BLUEPRINT checks every one that starts at a tree root.
A sheet states what the tree holds. It is not a roadmap. History and reasons live in the ledgers and RULINGS.md.

## How the sheets are used

1. An arc brief carries a BLUEPRINT block: the 1-3 sheets it touches, the BP-ids it KEEPS, the BP-ids it CHANGES.
2. The executor reads ONLY the named sheets (plus LAWS.md and its brief). Nothing else is required context.
3. A BP-id the arc changes needs a ruling (R-id) or a seat decision, written into the sheet IN THE SAME ARC.
4. An invariant changed in code with the sheet unchanged is a drift. The fold refuses it.
5. A new subsystem gets a new sheet in the same arc that creates it, and a line here.
6. Vocabulary: "read at <sha>", "measured", "the wire shows". Never "proven/verified/confirmed" (a status
   word needs an `ST<n>` row, docs/dev/STATUS.md). Not read = `unverified`. Ruled but absent = `ruled, unbuilt (R<n>)`.
7. Colour values are never written in a sheet (cite palette_core). Shared things carry subsystem names, never boards.
8. "held by" names the gate, test or witness that would go red if the invariant broke. `unheld` = nothing does.
9. Line anchors drift. An arc that moves an anchored line re-anchors it in the sheet it touches.

## Block shape for an arc brief

```
BLUEPRINT: sheets=<a>,<b>  keeps=BP-<A>-1,BP-<A>-4,…  changes=BP-<B>-3 (R<n> | seat: <one line>)  new=BP-<B>-<n>
```

## The sheets

| sheet | covers | files it owns (kernel paths under `unaos/crates/kernel/src/` unless stated) |
|---|---|---|
| [bar.md](bar.md) | THE BAR (R129, R130): what other OSes get wrong and the line UnaOS holds instead, one law + one measurable bar per row; read by EVERY arc that touches what a person sees or is asked | the research record `docs/dev/evidence/bar-research/` |
| [shape.md](shape.md) | rings, the tree, arches, the cross-cutting rules (ONE OS, charter, deps, licence) | `Cargo.toml`, `unaos/Cargo.toml`, the target specs `unaos/x86_64-unaos.json` `unaos/aarch64-unaos.json`, `unaos/crates/kernel/src/lib.rs`, `docs/CODEX.md` §1 |
| [boot.md](boot.md) | loader → kernel_main → splash hold → stage-resolve → setter/login → `phase=desktop` → `[desktop] built`; the knob line | `unaos/crates/kernel/src/main.rs`, `unaos/crates/kernel/src/boot.rs`, `unaos/crates/kernel/src/splash.rs`, `unaos/crates/kernel/src/bootpace.rs`, `unaos/crates/kernel/src/fs/bootstep.rs`, `unaos/crates/kernel/src/video/desktopbuild.rs`, `unaos/crates/kernel/src/tests.rs`, `unaos/crates/bootloader/`, `unaos/scripts/image-line.env` (+ `-pi`, `-orin`) |
| [cores.md](cores.md) | SMP bring-up, the split (render / service cores), placement, the scheduler, the compositor gate and its steal, watchdogs, lock ownership | `arch/x86_64/{smp,sched,interrupts}.rs`, `arch/aarch64/{smp,sched}.rs`, `unaos/crates/kernel/src/sessionwedge.rs`, `unaos/crates/kernel/src/deadman.rs`, `unaos/crates/kernel/src/lockowner.rs`, `unaos/crates/kernel/src/video/renderwedge.rs` |
| [video.md](video.md) | panel, dpi, wm (z, modal), composite, furniture, tokens, the two looks, wallpaper/STARFIELD, capture, window caps | `video/**`, `unaos/libs/sys/palette_core/` |
| [input.md](input.md) | HID decoders → `pal::Event` → keymap → login/router → ring-3 input ring; trackpad | `unaos/crates/kernel/src/pal.rs`, `unaos/crates/kernel/src/video/keymap.rs`, `unaos/crates/kernel/src/drivers/xhci/mod.rs` (HID), `drivers/ehci/**`, `arch/*/syscall.rs` (input ring) |
| [ring3.md](ring3.md) | una-abi, syscalls, bus codec and verbs, ELF load, slots, window ABI, FPU/SSE state | `unaos/crates/una-abi/`, `unaos/crates/kernel/src/bus.rs`, `arch/*/syscall.rs`, `unaos/crates/kernel/src/procslot.rs`, `unaos/crates/kernel/src/ring3abi.rs`, `unaos/crates/kernel/src/prefs_client.rs`, `unaos/crates/kernel/src/netring3.rs`, the `user-*` crates under `unaos/crates/` |
| [storage.md](storage.md) | block handles, AHCI/USB/SD, FAT, UnaFS, VFS mount table, root by content, boot FAT, prefs store, users store, install, Trash, name index | `drivers/{block,ahci}.rs`, `fs/**`, `install/**`, `unaos/crates/kernel/src/prefs.rs`, the `unaos/libs/fs/` cores |
| [net.md](net.md) | NIC drivers, usbnet, smoltcp vs the hand-rolled stack, DNS/SNTP, wifi ladder, ring-3 net | `unaos/crates/kernel/src/smolnet.rs`, `net_*.rs`, `unaos/crates/kernel/src/netring3.rs`, `unaos/crates/kernel/src/drivers/xhci/usbnet.rs`, `unaos/crates/kernel/src/drivers/bcma.rs`, `wifi/**`, `unaos/crates/net/` |
| [security.md](security.md) | users, login, sessions, principals, handles/ACL, keyring/Holocron, apptrust, Helm, root rules | `unaos/crates/kernel/src/fs/users.rs`, `unaos/crates/kernel/src/video/login.rs`, `unaos/crates/kernel/src/keyring.rs`, `fs/{apptrust,rootacl,holocron}.rs`, `unaos/crates/kernel/src/rand.rs`, `unaos/libs/sys/helm/` |
| [handlers.md](handlers.md) | CODEX manifest, host userspace (libs, handlers, vessels, tools), which handlers run as ring-3 ELFs on UnaOS | `handlers/`, `libs/`, `vessels/`, `tools/`, `docs/CODEX.md` §2-§3 |
| [gates.md](gates.md) | arroyo verbs, `./arroyo gates` (19 rows), foldleg/twoboot/knoboff, ledgers, queues, STATUS, evidence | `unaos/arroyo`, `unaos/scripts/`, `tools/status-check.py`, `docs/dev/LEDGER.md`, `docs/dev/QUEUE.md`, `docs/dev/STATUS.tsv`, the track ledgers and queues in `docs/dev/OS/` |
| [power.md](power.md) | the power verbs (reboot, shutdown, sleep), the SMC (read only), the lid, the sleep state `idle` and why S3 is unreachable, the power key, the SLEEP line (BP-bar-20) | `unaos/crates/kernel/src/power.rs`, `unaos/crates/kernel/src/drivers/smc.rs`, `unaos/crates/kernel/src/arch/x86_64/acpi_power.rs`, `unaos/crates/kernel/src/video/lidsleep.rs` |

## Not covered by any sheet yet (seat to decide)

Audio (`drivers/hda*.rs`, the doc in `docs/dev/OS/11_AUDIO/`), GPU driver ladders (`drivers/gpu/**`, the Kepler and gen7
rungs: R101, R127), the compatibility box (`unaos/crates/kernel/src/arch/x86_64/linuxabi/`, `docs/dev/OS/03_COMPATIBILITY_BOX/`), selfhost
(`unaos/crates/kernel/src/selfhost/`). Each is a candidate sheet. (Power/SMC: power.md, SLEEP B691.)

## Seat decisions on the drafter's open points (LAWS §8: engineering is the seat's; Peter is asked what to build and how it looks)

1. FPU/SSE for UnaOS ring 3: DECIDED — built by FPU (rmbp-ledger B666, wave 36): an FXSAVE context per task and the SSE
   target for every ring-3 ELF; ring3.md BP-ring3-11 is rewritten by that fold.
2. R128's scope: being built by STARTSCENE (B662): one scene under the splash hold, the login screen and the desktop.
3. Helm in Ring 0: the kernel links `unaos/libs/sys/helm` when TALUS's drive service lands (ROADMAP §4), not before;
   CODEX's "kernel core" names that seam, not a present link. security.md says so.
4. Sonde (CODEX Amendment IV, R112): what to build is Peter's — stays open in CODEX until he signs it.
5. Which arch the sheets bind: x86 and the shared code at HEAD. aarch64-only code binds from the next track sync
   (the Pi and Orin tracks are behind trunk); an arc on those tracks reads the sheet and re-anchors on landing.
