# APPMENU — prep

## The finding

QUEUE.md `APPMENU`: **an app cannot publish a menu on ANY board — the ABI has no menu verb**
(MENUDROP, hw-rmbp 2026-09-17): `crates/una-abi/src/lib.rs` ends at `SYS_CPUPULSE = 49`;
`BUS_VERB_MENU_PUBLISH` is only the design ledger at `video/menubar.rs:1611`. The `View` menu
Peter knows from Pulse is the KERNEL pulse window's (`video/pulsewin.rs:172-183`, published
`:526`), every runtime call site `desktop_firmware`-gated, so on x86 the app's title shows and no
View follows. Two arcs, Peter's call: (a) a menu verb in una-abi + the bar's per-window menu path
taking it (apps own their menus, every board); (b) the kernel pulse window on x86 (cosmetic,
board-split).

## Mechanism

**Today, only the kernel itself can publish a menu**, through `winmenu::publish(owner: wm::WinId,
titles: &'static [MenuTitle], on_pick: fn(u32)) -> bool` (`video/winmenu.rs:361`) and
`winmenu::publish_app(owner, items: &'static [MenuItem])` (`:459`). Both take `&'static` data and
a kernel fn pointer — possible only because the caller is compiled into the image (e.g.
`pulsewin.rs:526` calls `winmenu::publish(id, tree_for(view()), on_menu_pick)` with two `const
[MenuTitle; 1]` arrays). Registry: `TREES: spin::Mutex<[Option<Tree>; WINMENU_MAX]>` keyed by
owner (`winmenu.rs:200-213`), `WINMENU_MAX = 4`. `MenuItem`/`MenuTitle`/`FLAG_*` at `:117-171`.

An EL0 app has **no syscall** that reaches this registry. `una-abi/src/lib.rs` syscalls run
`SYS_GETPID..SYS_RENAME` (6..50); none is a menu verb (`lib.rs:648-650`, the high-water-mark
assert: `SYS_RENAME > SYS_CPUPULSE > SYS_ACCEPT`, next free number is 51).

**The design ledger already exists**, written but unwired, at `video/menubar.rs:2524-2666`
("THE MENU PROTOCOL — DESIGN LEDGER. No implementation in this arc."). It specifies arc (a): no
new syscall — the tree rides the frozen bus (`SYS_MSEND`/`SYS_MRECV`, ABI 19/20, `lib.rs:198-204`),
which already kernel-stamps a caller-can't-forge principal (`BANDY-STAMP`, bytes 16..48 of the v1
frame). Three new `BUS_VERB_*` tags (`MENU_PUBLISH=7`, `MENU_CLEAR=8`, `MENU_GET=9` — `MV=6` is
today's high mark, `lib.rs:590`, so 7 is correctly next), one input event for the pick
(`INPUT_EV_MENU_PICK`), a fixed-width wire item (`id/parent/flags: u32, label_len: u8, label:
[u8; MENU_LABEL_MAX]`, caps `MENU_LABEL_MAX=24`/`MENU_DEPTH_MAX=2`/`MENU_ITEMS_MAX=64`, fits
`BUS_BODY_MAX=4096` at 64*40=2560B), a registry keyed by `owner_asid` beside `video/wm.rs` (reaped
by `close_owner`), and a pick delivered to the TREE'S OWNER by identity, never focus (leg 1 of the
ledger's falsification list, `:2648-2653`).

**Two corrections to the ledger, found reading current code, that change what M1 actually needs:**

1. **`INPUT_EV_MENU_PICK`'s proposed number 6 is now taken.** `INPUT_EV_WHEEL = 6`
   (`lib.rs:413`) and `INPUT_EV_ACTION = 7` (`lib.rs:432`) have both landed since the ledger was
   written. The next free `INPUT_EV_*` tag is **8**, not 6 — mint `MENU_PICK = 8`.
2. **`SYS_MSEND`/`SYS_MRECV` are no longer aarch64-only.** The ledger (and an un-updated doc
   comment at `lib.rs:198-204`, "aarch64 only today") predate `BUSX86`
   (`arch/x86_64/syscall.rs:175`, `:2623`, `:24531+`): x86 dispatches both verbs unconditionally
   today (`sys_msend`/`sys_mrecv`, same stamping and refusal rules as aarch64's). So the ledger's
   own arc (a) is **already board-uniform at the transport layer** — good news for "apps own
   their menus, every board" — nothing to add there, only the three verb tags, the wire struct,
   the registry, and the bar's lookup.

The wire-argument pattern to follow for the *verb tag* path (bus, not a raw syscall) is the
existing `BUS_VERB_MV` body (`crates/kernel/src/arch/x86_64/syscall.rs` — `busx_mv`, referenced
from `una-abi/src/lib.rs:318-345`'s `SYS_RENAME` doc): fixed-shape body `[src_len][src][dst]`,
parsed by the kernel with no heap allocation, refused whole (not truncated) on a bad shape. The
menu wire item copies that discipline: fixed 40-byte record, refuse-whole caps.

Dispatcher precedent for a syscall taking a raw user pointer (not needed for arc (a), which rides
`SYS_MSEND`, but relevant if Peter picks a dedicated syscall instead): `SYS_CPUPULSE`'s arm
(`arch/x86_64/syscall.rs:2612`, body at `:3141-3164`) — a `#[repr(C)]` POD struct written through
`copy_to_user`, no heap, `-EFAULT` on a bad pointer, never a task-kill.

Renderer side: `winmenu::bar_boxes`/`menu_of` (`winmenu.rs:854`, `:1027`) already read
`OWNERS`/`TREES` by owner — the bar's paint path does not care whether a tree came from a kernel
window or the new registry; `publish`/`publish_app` just need a bus-fed caller alongside their
existing kernel ones.

## Plan

- **M1 — mint the ABI (una-abi only, no dispatch yet).** `crates/una-abi/src/lib.rs`: add
  `BUS_VERB_MENU_PUBLISH = 7`, `BUS_VERB_MENU_CLEAR = 8`, `BUS_VERB_MENU_GET = 9` beside
  `BUS_VERB_MV` (`:590`); add `INPUT_EV_MENU_PICK = 8` beside `INPUT_EV_ACTION` (`:432`, corrected
  number, see Mechanism); add `MENU_WIRE_VERSION`, `MENU_LABEL_MAX=24`, `MENU_DEPTH_MAX=2`,
  `MENU_ITEMS_MAX=64`, the `MENU_FLAG_*` bits, and a `#[repr(C)]` `MenuWireItem` (id, parent,
  flags: u32 each; label_len: u8; label: [u8; 24]) plus a `const _: () = assert!(MENU_ITEMS_MAX *
  core::mem::size_of::<MenuWireItem>() <= BUS_BODY_MAX)`. No dispatcher change — this milestone
  cannot go red at runtime, only at compile time, so its "witness" is the build itself; print
  `:: APPMENU: abi items=64 wire_bytes=40 cap_bytes=2560 body_max=4096 -> PASS ::` from a
  host-side `#[test]` in `una-abi` (compiles + asserts fit). Go-red: bump `MENU_ITEMS_MAX` to 128
  in the fixture and confirm the `const _` assert fails the build (FORBID the wire ever exceeding
  `BUS_BODY_MAX`, not merely warn).
- **M2 — kernel registry + bus dispatch.** New `video/appmenu.rs` (or a `winmenu.rs` section):
  `PENDING: spin::Mutex<[Option<(owner_asid, [MenuWireItem; MENU_ITEMS_MAX], count, depth)>; N]>`,
  no heap. Wire the three `BUS_VERB_*` arms into the bus dispatcher (`arch/x86_64/syscall.rs`
  ~`25007+`'s `SYS_MSEND` body, and aarch64's `bus_*` equivalent) the way `busx_mv` is wired for
  `BUS_VERB_MV`: `MENU_PUBLISH` refuses whole on `count > MENU_ITEMS_MAX`, any `label_len >
  MENU_LABEL_MAX`, or depth > `MENU_DEPTH_MAX` (leg 3), stamps the KERNEL-derived principal, never
  caller-supplied (leg 2), replaces this principal's slot; `MENU_CLEAR` drops it; `MENU_GET`
  copies it into the reply. Witness: `:: APPMENU: verb=publish owner={asid} items={n} depth={d}
  -> PASS ::` / `reason=items-cap ... -> REFUSED ::`. Go-red: send 65 items, or a caller-stamped
  nonzero principal, assert the registry unchanged (legs 2+3 as one fixture).
- **M3 — the bar's per-window menu path takes it.** `video/winmenu.rs`: a thin
  `publish_from_wire(owner, &MenuWireItem, count) -> bool` feeding the SAME `TREES` table
  `bar_boxes`/`menu_of` already read — no new renderer (the ledger's "no renderer" claim covers
  M2's fixtures; M3 makes it visible on the real bar). Pick delivery: post
  `INPUT_EV_MENU_PICK` (payload = the app's own item id) to the TREE'S OWNER's input ring, not the
  focused slot (leg 1, the ledger's sharpest edge). Witness: `:: APPMENU: owner={asid} items={n}
  pick_to=owner -> PASS ::`. Go-red: publish tree A as owner A, focus owner B, click one of A's
  items, assert B's ring stays empty and A's carries the event (focus-addressed delivery is the
  wrong implementation this fixture catches).
- **M4 — reaping.** Hook the registry's owner-slot drop into `wm::close_owner` (same call site
  that already reaps windows), so a dead principal's tree disappears and a subsequent `MENU_GET`
  for it answers empty. Witness: `:: APPMENU: owner={asid} closed reaped=true -> PASS ::`. Go-red:
  publish, close the owner's only window, `MENU_GET` the same owner and assert an empty reply
  (not a stale tree).
- **M5 (stretch, after M1-M4)** — host side: `libs/bandy/src/signals.rs`
  `SMessage::MenuPublish/MenuCleared/MenuQuery/MenuIs/MenuPick` per the ledger
  (`menubar.rs:2607-2617`), KATs in `smessage_kats.rs`. Not a security boundary on host (`Synapse`
  has no principal/addressing, ledger's own disclosed gap, `:2620-2626`) — scope to metal-verified
  correctness, or land an envelope first (Peter's call, see below).

## Spec pins

New file `unaos/scripts/specs/appmenu.spec` (or a section appended to `x86-witness.spec` once
flown once on x86); no look-around, plain literal + `[0-9]+`/`.*` as the existing specs do:

```
REQUIRE \[winmenu\] publish owner=[0-9]+ titles=[0-9]+ items=[0-9]+ slot=[0-9]+ replaced=(true|false) app-menu=(custom|default)
REQUIRE :: APPMENU: verb=publish owner=[0-9]+ items=[0-9]+ depth=[0-9]+ -> PASS ::
REQUIRE :: APPMENU: owner=[0-9]+ items=[0-9]+ pick_to=owner -> PASS ::
REQUIRE :: APPMENU: owner=[0-9]+ closed reaped=true -> PASS ::
FORBID :: APPMENU: verb=publish owner=[0-9]+ items=6[5-9]|[7-9][0-9] .* -> PASS ::
FORBID \[winmenu\] publish owner=0 .*
```
(the last `FORBID` pins leg 2: `owner=0`/asid-0 is never a real published tree — a caller-supplied
principal must never survive far enough to reach `[winmenu] publish` at all.)

## Open questions

1. **Arc (a) vs (b) — Peter's call, stated in the queue line itself.** (a) is the real ABI verb
   (this doc); (b) is cosmetic — moving/duplicating the kernel pulse window's `View` menu pattern
   onto x86 without touching una-abi, board-split, no app ever gets a menu of its own. Which one
   does Tuesday's session build?
2. If (a): does M5 (host `SMessage` variants) matter for this round at all, given the disclosed
   gap that the host bus enforces no principal? Or does the arc stay metal-only until an envelope
   lands?
3. Does the bus route (`SYS_MSEND`/`MENU_PUBLISH` verb) win over a dedicated `SYS_MENU_PUBLISH`
   syscall (the `SYS_CPUPULSE`/`copy_to_user` pattern)? The ledger argues bus (no ABI widening,
   already board-uniform post-BUSX86) — worth Peter's explicit sign-off since it is the one
   un-reversible numbering decision (BUS_VERB tags, once shipped, are frozen like syscall numbers).

## Next-session start

1. `grep -n "BUS_VERB_\|INPUT_EV_" unaos/crates/una-abi/src/lib.rs` to reconfirm high-water marks
   are still 6/7 (BUS_VERB) and 8 (INPUT_EV) — both drift every round; re-derive before minting.
2. Add the M1 consts to `crates/una-abi/src/lib.rs` beside `BUS_VERB_MV`/`INPUT_EV_ACTION`, plus
   the `MenuWireItem` struct and its `BUS_BODY_MAX` fit assert; `cargo test -p una-abi` (host-side,
   no QEMU needed) to get the M1 witness before touching the kernel at all.
3. Read `arch/x86_64/syscall.rs:25007-25100` (`sys_msend` body) and the matching aarch64
   `bus_*` dispatch in full, to find the exact match arm `BUS_VERB_MV` sits in, before adding the
   three menu arms beside it for M2.
