# QUARRY — UnaOS's file manager

Status: **M1 landed** — the window, the volume tree, the detailed list, and the first scrolling in
this tree. Knob: `UNAOS_QUARRY=1` (pair it with `UNAOS_PIDESK=1`).

Peter's direction, 2026-08-17, verbatim: *"Tree on left and start with detailed list view on right.
I'm not sure we have scrolling yet."* — and, on placement: *"pinned to the left side of the
taskbar/dock so it opens like Mac's Finder"*. The name is his, the same day.

He was right about the scrolling. There was none. §4 is the audit.

---

## 1. What it is

A compositor window carrying two panes and a path bar.

| region | contents |
| --- | --- |
| path bar | the absolute VFS path the list pane is showing, plus a truncation notice when the medium had more entries than the model holds |
| left pane | the **directory tree** — one root per mounted volume (`/`, `/fat`, `/usb` when the stick is present), expandable to `MAX_DEPTH` (8) levels, with a disclosure triangle per row |
| right pane | the **detailed list** of the selected directory — `NAME` · `SIZE` · `MODIFIED`, header pinned above the scrolled band |
| both panes | a scrollbar in `theme::SCROLL_TRACK`/`SCROLL_THUMB` whenever the content is taller than the pane |

Files: `unaos/crates/kernel/src/video/quarry.rs` (the knob seam) and
`unaos/crates/kernel/src/video/quarry/live.rs` (the implementation).

### Gestures

| gesture | effect |
| --- | --- |
| `Up` / `Down` | move the selection in the focused pane; the viewport follows it |
| `Left` | in the list, focus the tree; in the tree, collapse — or step to the parent row when already closed |
| `Right` | in the tree, expand — or, on an already-open row, cross into the list |
| `Enter` | tree: show this directory. list: descend into the selected directory, revealing it on the left |
| `Backspace` | up one level |
| `r` | re-read — **including the mount table**, so a stick plugged after Quarry opened appears |
| `Esc` | close (the keyboard goes straight back to the console) |

The keyboard contract is `instgui`'s: while Quarry's window is **on the glass** it takes first refusal
on the keys above, and consumes nothing else, so the console keeps working underneath. `Esc` hands the
keyboard straight back. "On the glass" and not merely "open" is load-bearing — `wm` expresses
*minimised* as a position (`z` below `SHELL_Z`), so a parked Quarry that kept first refusal would be
eating arrows for a window the operator cannot see. The pointer needs no equivalent guard:
`wm::hit_test` does not report a row that is not compositing.
| click a tree row | select and show it; clicking the **disclosure triangle** toggles instead |
| click a list row | select it |
| click a scrollbar **track** | page towards the press — see §4 for why the track, and not a wheel |
| title bar / borders | drag, minimise, zoom — the ordinary `wm` arms, deliberately not claimed |
| dock's `quarry` tile | reopen |

---

## 2. Architecture: why this is a kernel window and not an EL0 program

The ring-3 line (`user-vug`, `user-stat`, `user-pulse`) is where a new app should live, and the
question was asked seriously before it was answered the other way. Two facts in the tree make the EL0
version **impossible today**, not merely harder. Both are checkable rather than matters of taste.

### 2.1 An EL0 window is hard-capped at 128 x 128 pixels, on both arches

`arch/aarch64/boot.rs:89` and `arch/x86_64/memory.rs:511` both read `FB_WIN_MAX_W: u32 = 128`, and the
x86 side carries the assertion that ties it to the window region slot:

```rust
const _: () = assert!((FB_WIN_MAX_W * FB_WIN_MAX_H * 4) as usize == FB_WIN_SLOT_SIZE);
```

`sys_win_create` refuses anything larger with `-EINVAL` on both arches
(`arch/aarch64/syscall.rs:12119`, `arch/x86_64/syscall.rs:3567`). At the 8-px `font8x8` cell that is
**16 columns** — four short of a single FAT 8.3 name, before any thought of a size column beside a
date column beside a tree. The deliverable is not expressible in that surface, and raising the cap is
a window-region memory-map arc on **both** arches (the x86 half is the rmbp lane), not a file-manager
arc.

### 2.2 There is no directory syscall, and the one listing EL0 *can* do bypasses the VFS

`una-abi` is the frozen ABI. Its filesystem verbs are `SYS_OPEN(11)`, `SYS_READ(12)`, `SYS_SEEK(15)`,
`SYS_UNLINK(16)`, `SYS_CLOSE(17)`. There is **no** `readdir`, no `getdents`, no `stat`; numbers 34–39
are unallocated. `SYS_OPEN` itself does not route through the mount table — it binds `fat::mount()`
directly and caps the name at 12 bytes, so `/fat/DOCS/X.TXT` is not even expressible from ring 3
(`vfs.md` §12.4 names this as the outstanding adoption).

The one directory listing an EL0 program can perform today is the midden bus's `BUS_VERB_LS`
(`arch/aarch64/syscall.rs:21351` `bus_ls`), and it reads the **FAT root** through `fat::mount()`. That
is a raw backend. Routing Quarry through it would reproduce, in a new program, precisely the
"`ls` and `cat` name different files" defect VFS-1 (adoption) was written to delete.

### 2.3 What was built instead

The idiom this tree actually uses for a windowed tool that needs room and kernel data:
`video/instgui.rs` (the installer dialog), `fbcon::panel_console_window_open` (the console window),
`main.rs`'s `open_shell_window` (the shell window). All three are **kernel-owned `wm` rows over a
cached-RAM ARGB8888 surface**, presented through the ordinary `wm::present` path. Quarry is the
fourth.

It is written so the ring-3 version is a **port, not a rewrite**: the geometry accessor, the scroll
arithmetic, the tree splice, the list model and the whole painter are pure functions over a `DirEnt`
slice and a `&mut [u32]`. §7 lists what has to land first, in order.

**Front-buffer discipline.** Every pixel lands in the heap surface; presentation is `wm`'s. This
module never touches the framebuffer and never holds `WRITER` across a directory read.

---

## 3. The VFS seam

Every directory read goes through **`shell::vfs_ls_collect`** — the one collector VFS-1 (adoption)
left behind when it deleted the per-volume ones. Quarry therefore inherits, by construction:

* longest-prefix mount resolution at a component boundary (`/usb` never claims `/usbfoo`);
* the VFS-4 `-ENODEV` guard, so a reserved-but-unbound volume reports *the volume* as missing rather
  than a bare `-ENOENT` off the native root;
* the synthesized mount-point rows (`fat/`, `usb/`) immediately below a listed path, present only for
  bound prefixes — the honest hot-plug posture of `vfs.md` §6;
* the `.`/`..` filter;
* `DirEnt::size` and `DirEnt::mtime`, which `vfs.md` §12.3 added for exactly this shape of consumer.

The tree's **roots** are `MountTable::prefixes()` off the same table the collector builds. Quarry
names no backend, calls no `fat::mount()`, and calls no `unafs::with_unafs()`.

Two visibility widenings were needed and nothing else: `shell::vfs_ls_collect` and
`shell::vfs_mount_table` became `pub(crate)`. Both edits are one token; neither changes a line count.

### 3.1 The one `target_arch`, and why it is not Quarry's

`fs/vfs.rs` gates `NativeBackend` and `FatBackend`'s impls to aarch64 — `vfs.md` §12.4: *"x86 is
unchanged by design … that arch has no mount table to route through"*. So on x86 the collector and the
mount table **do not exist to be called**, and Quarry's two shims (`collect`, `roots`) carry the only
`cfg(target_arch)` in the module.

It is not a hardware decision and it is not a new one — it mirrors a gate already in the tree. Quarry
compiles, lays out, scrolls, hit-tests and paints identically on both arches; on x86 it opens on an
empty volume list and says `no VFS mount table on this arch yet (vfs.md 12.4)` in the list pane rather
than pretending. The day the x86 VFS adoption lands, the two shims collapse into one and x86 gets a
working file manager with no further work here.

---

## 4. Scrolling — the audit, the verdict, and what was built

**Verdict: nothing in the window/present/input stack could scroll a list, at any of the four layers a
scrolling list needs.** Quarry is the first scrolling anything in this tree.

| layer | what exists | usable? |
| --- | --- | --- |
| input | `drivers/xhci/mod.rs:3763` decodes the HID boot mouse as `byte0=buttons, byte1=dx, byte2=dy` and the comment says `(byte3 = wheel, ignored)`. `pal::Event` has no wheel variant; `una-abi` has no `INPUT_EV_WHEEL`. The HID keymap drops PageUp/PageDown (`0x4B`/`0x4E` → `(0,0)`). | **no** |
| compositor | `wm.rs` is 18 373 lines and `WindowInfo` has no scroll, content-offset or viewport field. There is no clip-to-content-rect. | **no** |
| blit | `FrameBuffer::scroll_up(dy, fill)` (`framebuffer.rs:552`) is a **whole-surface** memmove — no rect, no clip, kernel-private, behind no syscall. `video/vperf.rs` instruments its cost; it is a benchmark subject, not an API. | **no** |
| widget | `theme::SCROLL_TRACK` and `theme::SCROLL_THUMB` have existed since the theme table landed and **no scrollbar has ever consumed them** (their only uses were a row highlight in `instgui` and the dim minimised pip in `dock`). `theme::SCROLLBAR_WIDTH` had no consumer at all. | **no** |

Console *scrollback* exists (`console.rs`, a 256-line ring) but is tail-anchored with no position
variable, so it cannot show older lines either; `selftest.rs`'s `Pager` is forward-only. Everything
else a `grep -i scroll` finds is host-native ring-3 code (`libs/quartzite`'s AppKit/GTK views,
`handlers/aether`'s engine) that does not run on UnaOS at all.

### What Quarry implements

The **minimal honest version**: an integer row offset per pane, a clamp, and a full pane redraw at the
new offset.

```rust
fn scroll_max(len, visible) -> usize;                       // len - visible, saturating
fn scroll_follow(scroll, sel, len, visible) -> usize;       // clamp + keep the selection on screen
fn thumb(track_h, len, visible, scroll) -> Option<(y, h)>;  // None when everything fits
```

Two invariants, both asserted by the witness over a swept input space rather than three hand-picked
cases: the offset never exceeds `scroll_max` (so the viewport never shows blank rows under a short
list), and the selection is always inside the viewport (so the keyboard cannot drive a cursor the
operator cannot see).

### Its cost, stated rather than hidden

A scroll step costs **one full surface repaint plus one `wm::present`** — `w * h` word stores into
cached RAM, bounded by the `CEIL_W x CEIL_H` (1152 x 720) surface cap, then the compositor's ordinary
staged copy-out. It does **not** cost a per-row damage rectangle, and that is deliberate:
`wm::present_rows` is reached from the x86 syscall arm only (`SYS_WIN_PRESENT_ROWS` is documented
x86-only in `una-abi`, with a mandatory whole-box `-ENOSYS` fallback in `user-vug`). A row-band present
that one arch honours and the other does not is not a mechanism, it is a fork — so Quarry presents
whole boxes on both arches and the row-band optimisation waits for the arch to catch up.

The surface cap is what makes that affordable: at 1152 x 720 a repaint is ~830 k word stores, and a
repaint happens only on an actual gesture — there is no animation and no per-frame paint. A settled
Quarry costs nothing at all.

### Scroll GESTURES, given no wheel

The scrollbar **track** is the coarse gesture: a press above the thumb pages back, below pages
forward. That is not a design preference, it is the consequence of the wheel byte being discarded in
the xHCI decoder before the ABI ever sees it (§4's table). Thumb **dragging** is not implemented —
`wm`'s drag machinery is title-bar-scoped and a content-drag protocol is its own arc. Keyboard
`Up`/`Down` is the fine gesture and auto-follows.

---

## 5. Geometry and cost

One accessor, `geometry(pw, ph) -> Option<Geom>`, and both the painter and the router read it — the
crispywire law `dock::Layout` states, so there is no second copy of the arithmetic to drift.

* Text scale follows the **panel**, and it is the one number here that is not a proportion: `ts = 1`
  below 1280 px wide, `ts = 2` at or above. QEMU raspi4b is 640 x 480 and the bench Pi is 1920 x 1200,
  and a legibility decision cannot be a fixed fraction of a surface that differs by 7.5x in area.
* Content surface: 3/5 of the panel, floored at `320 x 200`, ceilinged at `1152 x 720`, rounded down
  to a whole text cell, and checked against the panel minus `wm`'s chrome. Below the floor `open()`
  DECLINEs with `[quarry] DECLINE reason=panel-below-floor` — an unreadable window is worse than no
  window, which is `pidesk`'s own CONSOLEWIN reasoning applied to a second tenant.
* Resolved: **640 x 480 → 384 x 288 at 1x** (48 cols x 26 list rows), **1920 x 1200 → 1152 x 720 at
  2x** (72 cols x 33 list rows).
* Panes tile the surface exactly: tree = 5/16 of the width (floored at ten columns, capped at half),
  one pixel of divider, list takes the rest. The witness asserts the tiling rather than trusting it.
* Columns **degrade rather than overlap**: below 34 columns the date goes, below 22 the size goes too.
  A narrow pane shows names, not three columns of ellipsis.

The surface is a heap `Vec<u8>` sized from the live panel — `try_reserve_exact` with a
`[quarry] DECLINE reason=alloc` arm, the `panel_console_window_open` idiom — rather than a
`[u32; W * H]` static, because a static large enough for the bench panel is `.bss` that QEMU pays for
and never uses.

### 5.1 CONSOLEWIN, applied to a second tenant

`open()` carries `wcx`'s CONSOLEWIN law, unchanged in substance and unchanged in reason: if
`dock::Layout::for_panel(wm::MAX_WINDOWS, pw, ph)` is `None`, Quarry DECLINEs.

Quarry is an ordinary `wm` row, so the kernel draws it the ordinary control cluster, minimise disc
included. Minimise is a POSITION — the row drops below `SHELL_Z`, stops compositing, and the only
gesture that brings it back is the dock. Its own pinned tile is no escape from this: that tile is *in*
the strip that will not fit. A control that hides a window with no way back is worse than no window.
`MAX_WINDOWS` and not the live count, because the check must hold for every table state the boot can
reach.

**This is also what keeps the armed QEMU gate honest, and that is said here rather than discovered
later.** 640 x 480 cannot host a twelve-tile dock, so on QEMU raspi4b Quarry declines exactly where the
console window declines, and the video witness battery — which asserts exact panel pixels and knows
nothing of a file manager — is unperturbed. The bench panel hosts the strip, so the bench is where the
window opens, and the armed **bench-geometry** run (§10) is where its effect on that battery is
measured rather than assumed.

---

## 6. Wiring, and why every seam is where it is

| seam | file | note |
| --- | --- | --- |
| module declaration | `video/mod.rs`, **at the file tail** | below `pidesk`, for `pidesk`'s reason: a `mod` line inserted higher renumbers every panic `Location` below it and moves the knob-off `kernel8.img` hash. Nothing is below this, so nothing moves. |
| knob seam | `video/quarry.rs` | the module is compiled under the furniture gate (`any(all(x86_64, wc), all(aarch64, pidesk))`) so **both** arches type-check it; `feature = "quarry"` decides whether the name resolves to the implementation or to `#[inline(always)] false`. See below for why the knob is here and not on the call sites. |
| open | `video/pidesk.rs` step 6 | the Pi's DESKTOP-READY seam, **last** in the sequence: Quarry reads directories and every step before it is pure geometry or a flag, so a slow or declined volume cannot delay the menu bar's paint. This mints no second launcher — Quarry is not a program, it is kernel furniture, the same class as the console window `panel_console_window_open` mints thirty lines above. |
| keyboard | `arch/aarch64/syscall.rs`, `user_input_enqueue` | folded onto the existing `crystal::key_escape` one-liner, **after** it: an open SHARD menu is modal and its `Esc` beats Quarry's. Before `wc_focus_key`, and Quarry does not bind TAB, so the compositor's key is never hostage. |
| pointer | `arch/aarch64/syscall.rs`, `wc_click_route` press edge | folded onto the existing `strip::press_route` one-liner, **after** it (the strips composite on top of the window layer). Quarry asks `wm::hit_test` itself and acts only when the top-most window at the point is its own, so folding it in this early can never let it claim a press that landed on a window above it. |
| dock tile | `video/dock.rs`, `pin_quarry` | see §6.2. |

### 6.1 The line-neutrality constraint, and what it forced

`arch/aarch64/syscall.rs` is compiled into the knob-off `kernel8.img`. `PARITY.md` §5.3 records the
measurement that makes this binding: a **`cfg`-only** change to `wm.rs` still moved the knob-off hash
(`42355ca2…` → `1143ecc5…`), because panic `Location` records embed file line numbers and eleven added
*comment* lines shifted every location below them.

So both of Quarry's input hooks are folded onto lines that were already there — the diff in that file
is **line-neutral, 22 504 lines before and after**. And a folded call cannot carry a second `cfg` of
its own without becoming a second line, which is the whole reason the knob lives inside
`video/quarry.rs` as a stub/implementation split rather than on the two call sites where it would
otherwise belong. With `UNAOS_QUARRY` off, `quarry/live.rs` is not compiled, the stubs inline to
nothing, and the two edited lines are the same length they were.

### 6.2 The dock's pinned tile — and why this does not make the dock a launcher

The dock's stated ruling (`dock.rs`, Peter's white-board Q10) is that it is *"a window switcher, and
nothing else … NOT an app launcher"*, because there is exactly one launch path in this kernel (the
shell's program source / `bg`). That ruling stands and is not being quietly reversed.

`pin_quarry` adds no launch path, because **Quarry is not a program**. It is a kernel-owned window, and
its tile is a *reopen* route for exactly the reason `pin_shell` is the shell's: a window an operator
can close and cannot call back is a window they have lost. While Quarry is LIVE its real row is
dock-addressable (kernel owners are), so the model is unchanged and no pin is added — the pin exists
only while it is closed, precisely as the shell's does.

Peter's placement — *"pinned to the left side of the taskbar/dock so it opens like Mac's Finder"* — is
why this one **prepends** where `pin_shell` appends. The settled strip reads
`[quarry] [live windows…] [shell]`, which is the macOS order.

Applied by all three readers of the model (`compose`, `press_at`, `strip_rect`), so painter, router and
the occlusion registry cannot disagree about the tile count.

**The press LATCHES rather than opens.** `press_at` runs inside a click router and Quarry's open reads
directories, so the tile sets a flag and `quarry::service()` drains it from the arch's input-drain
task — after `pump_usb_into_gui` has dropped its xHCI loan, which is the same context the shell's own
`ls` reads a volume from. The drain call is folded into the same aarch64 router line as the rest, so
**x86 has no drain yet**: when the x86 wiring lands it must place `service()` off the render core, for
the reason `wcx::desktop_app_service` documents (the render core holds the xHCI lock).

---

## 7. The ring-3 port — what has to land, in order

Quarry moves to EL0 the day these are true. Nothing here is Quarry's own work.

1. **A window larger than 128 x 128.** Raise `FB_WIN_MAX_W`/`H` and the window-region slot size, on
   both arches, or add a multi-slot surface. This is the blocking one and it is a memory-map arc.
2. **`SYS_READDIR` (and `SYS_STAT`), routed through `MountTable`.** Numbers 34–39 are free. The
   authorization is already composed — `MountTable::open_read` authorizes then stats, per `vfs.md` §2.
3. **`SYS_OPEN` off `fat::mount()` and onto the mount table**, so ring 3 and the shell agree about what
   a path names. `vfs.md` §12.4 already names this as outstanding.
4. **The x86 VFS adoption**, which deletes §3.1's two shims.
5. **A userspace drawing library.** Every EL0 crate today depends on `una-abi` alone, which is
   `const`-only, and there are four separately hand-rolled bitmap fonts across four programs (none
   covering full ASCII). Quarry needs ~96 glyphs, a `fill_rect`, a clip and a blit.
6. *(optional, for cost)* **A wheel event in the ABI** and un-ignoring `data_data[3]` in the xHCI HID
   decoder; and `SYS_WIN_PRESENT_ROWS` on both arches, so a scroll costs a row band rather than a box.

---

## 8. The witness

`:: QUARRY: … :: PASS ::`, five legs, `witness`-gated, run from `pidesk::activate` step 6 **before**
`open()` — its legs are pure functions over synthetic input (no panel, no volume, no window table), so
a DECLINE in the thing it proves the arithmetic of must not be able to skip it.

| leg | claim |
| --- | --- |
| geometry | a 200 x 200 panel is DECLINED; 640 x 480 and 1920 x 1200 both resolve, at `ts` 1 and 2; neither exceeds its panel or the ceiling; and on both, the two panes **tile the surface exactly** — one pixel of divider, nothing over, both filling below the path bar |
| `scroll_follow` | clamped to the top for a leading selection, follows to the tail, never exceeds `scroll_max`, and never scrolls a list shorter than the viewport — then the same two invariants asserted over a **swept** `len x visible x sel` space, not three hand-picked cases |
| `thumb` | absent when everything fits; at the floor at scroll 0; **exactly flush with the end of the track at max scroll**; never overruns mid-range |
| tree splice | `subtree_len` stops at the sibling, and `collapse` is an exact inverse of the splice — the property a hand-rolled index walk gets wrong when a sibling follows the expanded row. Then the SELECTION's three cases: a highlight on a later sibling stays on it (a bare `sel > i` test drags it back onto the closing row — the defect this leg was written against), and a highlight inside the subtree lands on the row that closed over it |
| press-to-row | the router's row arithmetic **inverts the painter's**, in both panes, with the list's body correctly clearing its pinned header |

It is arch-neutral and disk-free by construction, so it is honest on QEMU raspi4b (no stick) and on
x86 (no mount table at all) — the two surfaces where a volume-dependent leg would be vacuous.

Runtime evidence, on the wire, from the seams rather than from a fixture:

```
[quarry] open win=<id> surf=<w>x<h> ts=<n> box=<w>x<h> at (<x>,<y>) roots=<n> rows=<n> cwd-rows=<n>
[quarry] press close win=<id> at (<x>,<y>)
[quarry] closed win=<id> paints=<n>
[dock]   press at (<x>,<y>) tile=<t>/<n> quarry=pin -> open requested
[pidesk] quarry open=<bool> — the file manager is a desktop tenant
```

---

## 9. Bounds

Every model is capped, because both inputs are shaped by something other than this module: a
directory's entry count is whatever the medium says, and the tree's depth is whatever the operator
clicks. The caps are the loop bounds.

| cap | value | behaviour at the cap |
| --- | --- | --- |
| `MAX_TREE` | 192 rows | enforced against the **whole** tree, not per level, so six expanded 200-entry directories cannot grow it without bound; further children are simply not spliced |
| `MAX_LIST` | 2048 entries | the model is a prefix and the path bar SAYS `(list truncated)` — a silently short listing is a lie about the medium |
| `MAX_DEPTH` | 8 | an expand at the ceiling marks the row open with nothing under it |
| `PATH_MAX` | 160 chars | a longer child is skipped rather than built |
| surface | 1152 x 720 | the repaint cost §4 states is bounded by this |

---

## 10. Gate results (2026-08-17)

| gate | result |
| --- | --- |
| `./arroyo check` | green, both arches. The `arm-pi` **and** `x86-all` legs both carry `quarry`, so the implementation is type-checked on both arches, not merely the one that runs it |
| `UNAOS_WC=1 ./arroyo check` | green, both arches |
| `./arroyo kernel8` knob-off | `kernel8.img` sha256 `c3000ff5bd87ed0e…` — **byte-identical to the pre-arc baseline** |
| `./arroyo kernel8-test 210` knob-off | **PASS 108/108**, 0 forbidden, 36967 lines (no other aarch64 QEMU on the host) |
| `UNAOS_QUARRY=1 UNAOS_PIDESK=1 ./arroyo kernel8-test 300` | **PASS 108/108**, 0 forbidden, 48850 lines. `:: QUARRY: … :: PASS ::`, and `[quarry] DECLINE reason=dock-cannot-host-full-strip panel=640x480` — §5.1's law, the same decline the console window takes on this panel |

**Host-load caveat, stated because it produced two misleading intermediate runs.** This host carries
other sessions' QEMUs. Two knob-off runs taken while three to five were live returned `108/108 with 4
forbidden` (a lone `[wc-h] torn=1 -> AT-RISK`) and `107/108 with 14 forbidden, 3666 lines scanned` — a
visibly starved capture. Both were taken against a `kernel8.img` that is **byte-identical to the
pre-arc baseline**, so neither can be this arc's: same bytes, different verdict, is the definition of a
host term. The numbers in the table are from runs with no other aarch64 QEMU on the machine. (Note
`pgrep -c qemu` is not the quiet test here — it counts the seat's own x86 sandbox VM, which never
exits; `pgrep -f qemu-system-aarch64` is.)

### The bench-geometry run, controlled

`UNAOS_FBW=1920 UNAOS_FBH=1200`, both runs back to back on the same (non-quiet) host, so the delta is
attributable to the knob and not to load:

| run | verdict | forbidden | lines |
| --- | --- | --- | --- |
| control — `UNAOS_PIDESK=1` (Quarry OFF) | FAIL 104/108 | 26 | 18106 |
| armed — `+ UNAOS_QUARRY=1` | FAIL 104/108 | 37 | 18228 |

**The battery already fails at bench geometry without Quarry**, at the identical 104/108 with the
identical missing REQUIREs. That is the standing conflict CONSWIN-PI M2 recorded (`engine.md`
§CONSWIN-PI: 104/108 with 13 forbidden on a quiet host) — the arm-pi witness battery asserts exact
panel pixels and was written against a Pi desktop with no extra tenant on it.

Quarry's contribution is +11 forbidden hits, all of the **already-diagnosed** class:

* `[wc-d] verify win=3 … first=(307,158) got=0x2d2b55 want=0xc3c3c3` — `0x2d2b55` is `wm::DESKTOP_BG`,
  and `(307,158)` is the *same pixel* CONSWIN-PI M2's commit records verbatim for the console row. The
  fixture window's id also moved `2 → 3`, because Quarry took a table row ahead of it. Occlusion, with
  the surface itself intact.
* `[wc-h] torn=3` / `[wc-k] torn=2` against the control's `torn=1` / absent — one more compositor
  client on a loaded host, which is what the CONSWIN-PI measurement predicted.
* `[dragperf] admitted=4` against the control's `admitted=2` — the same FORBID the control already hits.

The armed run also **passed two legs the control failed** (`[wc-c] side-by-side drawn=2`, `[wc-g]
RACE-BLIT`), which is the honest read on how load-dependent these numbers are: the control's own 26
forbidden against the 13 the quiet-host arc recorded is the size of the host term.

No spec REQUIRE or FORBID was touched or re-worded. Teaching `pi4-regression.spec` about a quarry row —
the way x86's spec was taught "win=1 is the console window on this bench" — is the integrator's
reconciliation, not this arc's.
