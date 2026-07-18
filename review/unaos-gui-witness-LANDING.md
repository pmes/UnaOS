# GUI-WITNESS — landing report (R22, hw-rmbp)

**An on-screen boot-status channel for GUI (non-usbdebug) builds.** A GUI boot emits zero serial and
detaches fbcon at the GUI handoff, so a post-handoff fault has no witness surface at all. This arc
builds that surface: a lock-light boot-milestone recorder written from existing milestone sites, a
`bootlog` shell verb to read it on-panel after handoff, and a serial-growth dump as the QEMU proof.

Worktree `../UnaOS-rmbp` @ branch `hw-rmbp` (level with `main` at start). Lane held: recorder module,
`main.rs` milestone sites (additive), pci/xhci/ehci milestone sites (additive), the shell verb, the
named docs. No xhci/ehci transfer logic, sched, or serial-transport internals touched — the
serial-silence bug remains its own separate investigation.

## What landed

**M1 — the recorder (`crates/kernel/src/bootlog.rs`, new).** A fixed-size (`CAP=32`) ring of
`(u64 ms, &'static str tag)` entries behind a `spin::Mutex`, heap-free (inline array, `const fn new`,
like the FTDI capture ring). `record(tag)` stamps `arch::ms()` and pushes (drop-oldest on overflow);
`snapshot(&mut [Entry]) -> usize` copies out under the lock then releases before any printing;
`service_serial_dump()` re-prints the whole ring to serial only when it has grown. Registered in
`lib.rs` (always linked, both arches). Additive `record()` calls wired at the existing milestone
sites — no behavioral change to any milestone:

- `arch/x86_64/pci.rs` — after the PORTSW-1 XUSB2PR witness: `portsw:flip` (real Intel-silicon write)
  vs `portsw:inert` (read-only no-op, e.g. QEMU).
- `drivers/ehci/mod.rs` — at the boot-protocol arm site: `ehci:kbd-armed` / `ehci:mouse-armed`; at
  the report-pointer arm site: `ehci:trackpad-armed`.
- `drivers/xhci/mod.rs` — FTDI console bring-up: `ftdi:console-up` on success, `ftdi:failed` on
  either failure branch (SET_CONFIG or vendor step); block bring-up: `block:up` at geometry publish.
- `main.rs` — `gui:handoff`, recorded immediately BEFORE the GUI `fbcon::detach()`.

**M2 — surfacing on GUI builds.**
- (a) On-panel-during-boot needed no code change: `fbcon` mirrors every `serial_println!` to the
  framebuffer from `fbcon::init` (top of `kernel_main`) until the handoff `detach()`, so the early
  milestone serial lines are already visible on-panel. GUI builds never `attach_shadow`.
- (b) `bootlog` shell verb (`shell.rs`, matching the `batmon` pattern): snapshots the ring, releases
  the lock, then prints each `[<ms> ms] <tag>` to the `Console` oldest-first (or "no boot milestones
  recorded"). Not arch/feature gated — reads the same ring on any build. Help line added under a new
  `WITNESS:` group.

**M3 — QEMU proof.** `service_serial_dump()` is called each main-loop iteration: `witness`-gated in
the GUI loop (so it is ABSENT from a real metal GUI build — there the shell verb is the witness) and
unconditional in the `usbdebug` loop. On the default `./arroyo test 22` boot the ring lands and the
dump is verifiable in `target/serial.log`:

```
:: BOOTLOG: 4 milestone(s) recorded (GUI-WITNESS ring) ::
:: BOOTLOG: [    2403 ms] ehci:kbd-armed ::
:: BOOTLOG: [    2467 ms] portsw:inert ::
:: BOOTLOG: [    2931 ms] gui:handoff ::
:: BOOTLOG: [    3030 ms] block:up ::
```

(QEMU has a usb-kbd but no FTDI and qemu-xhci is not switchable, so `ehci:kbd-armed` + `portsw:inert`
are the enumerated milestones; `block:up` arrives from inside the loop and the ring re-dumps to 4.)
**Verification path used: the QEMU serial dump in the default witness build** (`grep -a BOOTLOG
target/serial.log`), the growth-triggered `service_serial_dump`. Cross-arch: `test-arm` records
`gui:handoff` + `block:up` (the shared xHCI/handoff sites) proving the ring works on aarch64 too.

## Where the fbcon detach actually happens vs the milestones

`fbcon::init` (main.rs top) attaches the framebuffer console immediately; it mirrors serial to the
panel through the ENTIRE boot. GUI (non-usbdebug) builds never `attach_shadow`. The detach for the
x86 GUI path is at **`main.rs` ~933** (`fbcon::detach()`), AFTER the first `console.draw` +
`pal.render()` — i.e. after the first GUI frame. The `gui:handoff` milestone is recorded on the line
immediately before that detach, so every earlier milestone (PORTSW, EHCI arm, and — on real metal —
FTDI console) is recorded while fbcon is still mirroring to the panel and is therefore visible
on-screen during boot; `bootlog` then reproduces the whole ring after the panel is repainted. (The
`rast`-knob build has an earlier detach at ~922, before the demo; not on the default path.)

## bootlog verb output shape

```
bootlog: 4 milestone(s) (oldest first):
  [    2403 ms] ehci:kbd-armed
  [    2467 ms] portsw:inert
  [    2931 ms] gui:handoff
  [    3030 ms] block:up
```

Empty ring → `bootlog: no boot milestones recorded`.

## Gate results (verbatim)

- `./arroyo check` — both arches `Finished ... release` (x86 + aarch64 green).
- `./arroyo test 22` — `✅ Test run complete`; `xHCI: >>> MISSION SUCCESS (BOT + CSW). TARGET
  ACQUIRED. <<<`; no `FAIL`. BOOTLOG ring landed (4 milestones).
- `UNAOS_CPU=qemu64 ./arroyo test 22` — `✅` complete; MISSION SUCCESS; ring landed (4).
- `UNAOS_WITNESS=1 ./arroyo test 22` — `✅` complete; MISSION SUCCESS.
- `UNAOS_SCHED_DEMO=1 ./arroyo test 30` — `✅` complete; MISSION SUCCESS; sched/demo witnesses
  present (20 SCHED/DEMO lines); no FAIL; ring landed.
- `./arroyo test-arm 22` — `✅ aarch64 test complete`; 0 `FAIL`; steady-state reached; ring landed
  (`gui:handoff` + `block:up`).

## Flags / notes

- The `bootlog` shell verb sizes its snapshot buffer with a literal `32` (matching
  `bootlog::capacity()`), with a comment. If `CAP` ever changes, that literal must track it — a
  const-generic seam would remove the coupling but was out of scope for this additive arc.
- The recorder ring being always-linked adds a ~32-entry static (each entry = `u64` + fat `&str`
  pointer) — negligible, and it is the whole point that it is available in every build.
- The GUI-serial silence itself is untouched and remains a separate open bench question (per the
  dual-enum adjudication); this arc delivers only the witness surface to localize it.
